//! Interactive platform console (B-29, Tinker-class): a typed REPL over the
//! booted application graph that dispatches through the CommandBus under the
//! caller's principal. Zero new dependencies — plain stdin/stdout.
//!
//! Posture:
//! - The shell boots through the exact production path (`kelson-api --shell`,
//!   spawned by `kelson shell`; the dep-rules matrix keeps tools from linking
//!   runtime crates, so the REPL lives here). Same pool, host, graph gate and
//!   flag snapshot as the HTTP edge.
//! - `login` verifies argon2 credentials exactly like `POST /auth/login`
//!   (same dummy-hash timing equalization, same tenant resolution, same
//!   access-snapshot install). There is no authorization bypass: every `run`
//!   goes through the engine's PolicyAuthz against the installed snapshot, so
//!   capability denial behaves identically to the HTTP edge (tested).
//! - Dev/test tool: refused in prod unless `KEEL_SHELL_ALLOW=1`.

use std::io::Write;
use std::sync::Arc;

use kelson_context::{ExecutionContext, PrincipalContext, ScopeSnapshot, TenantContext};
use kelson_foundation::{
    AppError, AuthorizationRevision, ExecutionProfileKind, OperationId, PrincipalId, SecurityPlane,
    TenantId,
};
use kelson_identity::{verify_credential, Secret};
use kelson_registry::PlatformGraph;
use time::OffsetDateTime;

use crate::auth_routes::{dummy_hash, install_access_snapshot, resolve_tenant};
use crate::routes::AppState;

/// Identity established for this console session (None = anonymous).
#[derive(Debug, Default)]
pub struct ShellSession {
    pub principal: Option<PrincipalId>,
    pub tenant: Option<TenantId>,
    pub email: Option<String>,
}

/// Result of one REPL line.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Reply(String),
    Exit,
}

const HELP: &str = "\
commands:
  login <email> <password> [tenant-uuid|dev]  authenticate (argon2, same as POST /auth/login)
  whoami                                      current principal/tenant + capability count
  ops                                         registered operations (plane, profile, capability)
  routes                                      graph route inventory
  caps                                        declared capabilities + this session's grants
  models                                      registered models (bindings + probes)
  run <module.operation> [json]               dispatch through the engine under your principal
  help                                        this text
  exit                                        leave the console

dispatch runs under your installed access snapshot: capability denial is
enforced by the policy engine exactly like the HTTP edge.";

/// Execute one REPL line (the testable core of the console).
pub async fn execute_line(state: &AppState, session: &mut ShellSession, line: &str) -> Outcome {
    let mut tokens = line.split_whitespace();
    let Some(cmd) = tokens.next() else {
        return Outcome::Reply(String::new());
    };
    match cmd {
        "help" | "?" => Outcome::Reply(HELP.to_string()),
        "exit" | "quit" => Outcome::Exit,
        "whoami" => Outcome::Reply(whoami(state, session)),
        "ops" | "operations" => Outcome::Reply(operations(state)),
        "routes" => Outcome::Reply(routes(state)),
        "caps" | "capabilities" => Outcome::Reply(caps(state, session)),
        "models" => Outcome::Reply(models(state)),
        "login" => login(state, session, tokens.collect()),
        "run" => run_operation(state, session, line).await,
        other => Outcome::Reply(format!("unknown command: {other} — try `help`")),
    }
}

fn whoami(state: &AppState, session: &ShellSession) -> String {
    let Some(principal) = session.principal else {
        return "anonymous — `login <email> <password> [tenant]` first".to_string();
    };
    let caps = session
        .tenant
        .and_then(|tenant| state.policy.snapshot(principal, tenant))
        .map(|snapshot| snapshot.capabilities.len())
        .unwrap_or(0);
    format!(
        "principal: {principal}\nemail: {}\ntenant: {}\ncapabilities: {caps}",
        session.email.as_deref().unwrap_or("?"),
        session
            .tenant
            .map(|tenant| tenant.to_string())
            .unwrap_or_else(|| "?".to_string()),
    )
}

fn operations(state: &AppState) -> String {
    let mut lines = Vec::new();
    for operation in state.bus.operations() {
        let Some(handler) = state.bus.handler(&operation) else {
            continue;
        };
        let plan = handler.plan();
        let capability = plan.authorization.capability.as_deref().unwrap_or("-");
        lines.push(format!(
            "{operation}  plane={:?} profile={:?} capability={capability}",
            plan.plane, plan.profile.kind
        ));
    }
    if lines.is_empty() {
        "no operations registered".to_string()
    } else {
        lines.join("\n")
    }
}

fn routes(state: &AppState) -> String {
    if state.route_inventory.is_empty() {
        return "no graph routes declared".to_string();
    }
    state
        .route_inventory
        .iter()
        .map(|route| {
            format!(
                "{} {} -> {} ({})",
                route.method, route.path, route.operation, route.module
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Wave 4 Phase 9 (B-29 completion): the graph-declared capability catalog
/// with this session's grants marked. Anonymous sessions see the catalog
/// without grants — the installed access snapshot is the only grant source
/// (the same engine the HTTP edge authorizes against).
fn caps(state: &AppState, session: &ShellSession) -> String {
    let granted = session
        .principal
        .zip(session.tenant)
        .and_then(|(principal, tenant)| state.policy.snapshot(principal, tenant))
        .map(|snapshot| snapshot.capabilities)
        .unwrap_or_default();
    let mut catalog: std::collections::BTreeSet<String> =
        state.granted_capabilities.iter().cloned().collect();
    catalog.extend(granted.iter().cloned());
    if catalog.is_empty() {
        return "no capabilities declared".to_string();
    }
    let mut lines: Vec<String> = catalog
        .iter()
        .map(|capability| {
            format!(
                "{capability}  {}",
                if granted.contains(capability) {
                    "granted"
                } else {
                    "-"
                }
            )
        })
        .collect();
    if session.principal.is_none() {
        lines.push("(anonymous — `login` to see session grants)".to_string());
    }
    lines.join("\n")
}

/// Wave 4 Phase 9 (B-29 completion): every model the boot registered,
/// tagged by the services that know it — route bindings (path-param
/// resolution) and row probes (`unique`/`exists` validation).
fn models(state: &AppState) -> String {
    let bindings = state.route_bindings.models();
    let probes = state.probes.models();
    let names: std::collections::BTreeSet<String> =
        bindings.iter().chain(probes.iter()).cloned().collect();
    if names.is_empty() {
        return "no models registered".to_string();
    }
    names
        .iter()
        .map(|name| {
            let services = match (bindings.contains(name), probes.contains(name)) {
                (true, true) => "bindings+probes",
                (true, false) => "bindings",
                (false, true) => "probes",
                (false, false) => unreachable!("union member comes from one of the registries"),
            };
            format!("{name}  {services}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn login(state: &AppState, session: &mut ShellSession, args: Vec<&str>) -> Outcome {
    if args.len() < 2 || args.len() > 3 {
        return Outcome::Reply("usage: login <email> <password> [tenant-uuid|dev]".to_string());
    }
    let identifier = args[0].trim().to_lowercase();
    let password = args[1];
    let result = (|| -> Result<(PrincipalId, TenantId), AppError> {
        let principal = match state.principals.by_email(&identifier)? {
            Some(found) => found,
            None => {
                // Equalized timing, same as the HTTP login handler.
                let _ = kelson_identity::verify_password(&Secret::new(password), &dummy_hash(state));
                return Err(AppError::unauthenticated());
            }
        };
        let credential = match state.credentials.password_for(principal.id)? {
            Some(found) => found,
            None => {
                let _ = kelson_identity::verify_password(&Secret::new(password), &dummy_hash(state));
                return Err(AppError::unauthenticated());
            }
        };
        if !verify_credential(&credential, &Secret::new(password))? {
            return Err(AppError::unauthenticated());
        }
        if !principal.is_valid() {
            return Err(AppError::denied("principal is suspended"));
        }
        let tenant = resolve_tenant(state, args.get(2).copied())?;
        Ok((principal.id, tenant))
    })();
    match result {
        Ok((principal, tenant)) => {
            install_access_snapshot(state, principal, tenant);
            session.principal = Some(principal);
            session.tenant = Some(tenant);
            session.email = Some(identifier);
            Outcome::Reply(format!("logged in as {principal} (tenant {tenant})"))
        }
        Err(err) => Outcome::Reply(format!("{err}")),
    }
}

async fn run_operation(state: &AppState, session: &ShellSession, line: &str) -> Outcome {
    let Some(rest) = line.trim().strip_prefix("run").map(str::trim) else {
        return Outcome::Reply("usage: run <module.operation> [json]".to_string());
    };
    let (op_raw, json_raw) = match rest.split_once(char::is_whitespace) {
        Some((op, json)) => (op, json.trim()),
        None => (rest, ""),
    };
    if op_raw.is_empty() {
        return Outcome::Reply("usage: run <module.operation> [json]".to_string());
    }
    let Some(principal) = session.principal else {
        return Outcome::Reply("not logged in — `login <email> <password>` first".to_string());
    };
    let operation = match OperationId::parse(op_raw) {
        Ok(op) => op,
        Err(err) => return Outcome::Reply(format!("{err}")),
    };
    let Some(handler) = state.bus.handler(&operation) else {
        return Outcome::Reply(format!(
            "[KSN_NOT_FOUND] no handler registered for {operation} — see `ops`"
        ));
    };
    let input: serde_json::Value = if json_raw.is_empty() {
        serde_json::json!({})
    } else {
        match serde_json::from_str(json_raw) {
            Ok(value) => value,
            Err(err) => return Outcome::Reply(format!("invalid JSON input: {err}")),
        }
    };
    let Some(tenant) = session.tenant.or(state.default_tenant_id) else {
        return Outcome::Reply("no tenant resolved for this session".to_string());
    };
    let plan = handler.plan();
    let ctx = match build_context(plan.plane, plan.profile.kind, principal, tenant) {
        Ok(ctx) => ctx,
        Err(err) => return Outcome::Reply(format!("{err}")),
    };
    match state.bus.dispatch(ctx, &operation, input, None).await {
        Ok(outcome) => {
            let mut text = serde_json::to_string_pretty(&outcome.result).unwrap_or_default();
            if outcome.receipt.replayed {
                text.push_str("\n(idempotent replay)");
            }
            Outcome::Reply(text)
        }
        Err(err) => Outcome::Reply(format!("{err}")),
    }
}

/// Context construction mirrors `graph_dispatch` (same plane/profile/tenant
/// semantics); the plan comes from the registered handler, which is what the
/// engine authorizes against. `kelson call` (Wave 5) reuses it so one-shot
/// dispatch builds the exact context the REPL does.
pub(crate) fn build_context(
    plane: SecurityPlane,
    kind: ExecutionProfileKind,
    principal: PrincipalId,
    tenant: TenantId,
) -> Result<Arc<ExecutionContext>, AppError> {
    let now = OffsetDateTime::now_utc();
    let mut builder =
        ExecutionContext::builder(kind, now)
            .plane(plane)
            .principal(PrincipalContext {
                id: principal,
                display: None,
            });
    if plane == SecurityPlane::Tenant {
        builder = builder
            .tenant(TenantContext {
                id: tenant,
                slug: "shell".into(),
                plan: "standard".into(),
            })
            .scope(ScopeSnapshot {
                nodes: vec![],
                revision: AuthorizationRevision(1),
                computed_at: now,
            });
    }
    builder.build()
}

/// True while pasted JSON still has open delimiters (outside strings) so the
/// REPL can continue reading lines.
fn json_unbalanced(s: &str) -> bool {
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escaped = false;
    for c in s.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => in_string = true,
            '{' | '[' => depth += 1,
            '}' | ']' => depth -= 1,
            _ => {}
        }
    }
    depth > 0 || in_string
}

/// The REPL loop. `state` is the fully booted production state; `graph` feeds
/// the banner. Stdin is read line-by-line (interactive prompt when a TTY,
/// silent pump otherwise so scripts can pipe commands).
pub async fn run(state: AppState, graph: &PlatformGraph) -> Result<(), Box<dyn std::error::Error>> {
    if state.env.is_prod()
        && !std::env::var("KEEL_SHELL_ALLOW")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    {
        return Err("kelson shell is disabled in prod (set KEEL_SHELL_ALLOW=1 to override)".into());
    }
    println!(
        "kelson shell (B-29) — env {} | {} module(s), {} operation(s) | type `help`",
        state.env.as_str(),
        graph.modules.len(),
        graph.operations.len()
    );
    let stdin = std::io::stdin();
    let interactive = std::io::IsTerminal::is_terminal(&stdin);
    let mut session = ShellSession::default();
    loop {
        if interactive {
            print!("kelson> ");
            let _ = std::io::stdout().flush();
        }
        let mut line = String::new();
        if stdin.read_line(&mut line)? == 0 {
            break; // EOF
        }
        let mut line = line.trim_end().to_string();
        if line.trim().is_empty() {
            continue;
        }
        // Pasted multi-line JSON: keep reading until delimiters balance.
        while json_unbalanced(&line) {
            if interactive {
                print!("  ... ");
                let _ = std::io::stdout().flush();
            }
            let mut cont = String::new();
            if stdin.read_line(&mut cont)? == 0 {
                break;
            }
            line.push(' ');
            line.push_str(cont.trim_end());
        }
        match execute_line(&state, &mut session, &line).await {
            Outcome::Reply(text) => {
                if !text.is_empty() {
                    println!("{text}");
                }
            }
            Outcome::Exit => break,
        }
    }
    if interactive {
        println!("bye");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kelson_identity::{hash_password, Credential, CredentialKind, Principal, PrincipalStatus};
    use kelson_policy::AccessSnapshot;

    fn harness() -> (AppState, Arc<kelson_mod_appointments::AppointmentStore>) {
        let dir = std::env::temp_dir().join(format!("kelson-shell-{}", uuid::Uuid::now_v7()));
        let (state, _graph, store) = crate::build_app_state(&dir, false, true);
        (state, store)
    }

    fn seed_principal(
        state: &AppState,
        email: &str,
        password: &str,
        status: PrincipalStatus,
    ) -> PrincipalId {
        let now = OffsetDateTime::now_utc();
        let principal = state
            .principals
            .create(Principal {
                id: PrincipalId::new(),
                status,
                email: Some(email.to_string()),
                phone: None,
                email_verified_at: None,
                created_at: now,
            })
            .expect("principal creates");
        let hash = hash_password(&Secret::new(password)).expect("argon2 works");
        state
            .credentials
            .create(Credential {
                id: uuid::Uuid::now_v7(),
                principal_id: principal.id,
                kind: CredentialKind::PasswordArgon2,
                secret_hash: Secret::new(hash),
                created_at: now,
                expires_at: None,
            })
            .expect("credential creates");
        principal.id
    }

    async fn login_ok(state: &AppState, session: &mut ShellSession) {
        seed_principal(
            state,
            "shell@example.test",
            "correct-horse-9",
            PrincipalStatus::Active,
        );
        match execute_line(state, session, "login shell@example.test correct-horse-9").await {
            Outcome::Reply(text) => assert!(text.starts_with("logged in"), "{text}"),
            Outcome::Exit => panic!("login must not exit the shell"),
        }
    }

    #[tokio::test]
    async fn help_lists_commands() {
        let (state, _store) = harness();
        let mut session = ShellSession::default();
        let Outcome::Reply(text) = execute_line(&state, &mut session, "help").await else {
            panic!("help replies");
        };
        for expected in [
            "login", "whoami", "ops", "routes", "caps", "models", "run", "exit",
        ] {
            assert!(text.contains(expected), "help missing {expected}");
        }
    }

    #[tokio::test]
    async fn unknown_command_points_at_help() {
        let (state, _store) = harness();
        let mut session = ShellSession::default();
        let Outcome::Reply(text) = execute_line(&state, &mut session, "frobnicate").await else {
            panic!("unknown command replies");
        };
        assert!(text.contains("unknown command"), "{text}");
        assert!(text.contains("help"), "{text}");
    }

    #[tokio::test]
    async fn exit_returns_exit() {
        let (state, _store) = harness();
        let mut session = ShellSession::default();
        assert_eq!(
            execute_line(&state, &mut session, "exit").await,
            Outcome::Exit
        );
        assert_eq!(
            execute_line(&state, &mut session, "quit").await,
            Outcome::Exit
        );
    }

    #[tokio::test]
    async fn run_requires_login() {
        let (state, _store) = harness();
        let mut session = ShellSession::default();
        let Outcome::Reply(text) = execute_line(&state, &mut session, "run demo.echo {}").await
        else {
            panic!("run replies");
        };
        assert!(text.contains("not logged in"), "{text}");
        let Outcome::Reply(text) = execute_line(&state, &mut session, "whoami").await else {
            panic!("whoami replies");
        };
        assert!(text.contains("anonymous"), "{text}");
    }

    #[tokio::test]
    async fn login_rejects_bad_password_and_unknown_email() {
        let (state, _store) = harness();
        let mut session = ShellSession::default();
        seed_principal(
            &state,
            "shell@example.test",
            "correct-horse-9",
            PrincipalStatus::Active,
        );
        let Outcome::Reply(text) = execute_line(
            &state,
            &mut session,
            "login shell@example.test wrong-password",
        )
        .await
        else {
            panic!("login replies");
        };
        assert!(text.contains("AUTH_UNAUTHENTICATED"), "{text}");
        let Outcome::Reply(text) =
            execute_line(&state, &mut session, "login nobody@example.test whatever").await
        else {
            panic!("login replies");
        };
        assert!(text.contains("AUTH_UNAUTHENTICATED"), "{text}");
        assert!(session.principal.is_none(), "failed login installs nothing");
    }

    #[tokio::test]
    async fn login_denies_suspended_principal() {
        let (state, _store) = harness();
        let mut session = ShellSession::default();
        seed_principal(
            &state,
            "suspended@example.test",
            "pw-12345678",
            PrincipalStatus::Suspended,
        );
        let Outcome::Reply(text) = execute_line(
            &state,
            &mut session,
            "login suspended@example.test pw-12345678",
        )
        .await
        else {
            panic!("login replies");
        };
        assert!(text.contains("AUTH_DENIED"), "{text}");
        assert!(session.principal.is_none());
    }

    #[tokio::test]
    async fn login_then_run_dispatches_under_principal() {
        let (state, _store) = harness();
        let mut session = ShellSession::default();
        login_ok(&state, &mut session).await;

        let Outcome::Reply(text) = execute_line(&state, &mut session, "whoami").await else {
            panic!("whoami replies");
        };
        assert!(text.contains("shell@example.test"), "{text}");
        assert!(text.contains("capabilities:"), "{text}");

        // Read-profile operation under the caller principal.
        let Outcome::Reply(text) =
            execute_line(&state, &mut session, "run demo.echo {\"hi\": 1}").await
        else {
            panic!("run replies");
        };
        assert!(text.contains("\"echo\""), "{text}");

        // Capability-bearing domain operation through the engine.
        let Outcome::Reply(text) = execute_line(
            &state,
            &mut session,
            "run appointments.book {\"patient\": \"A. Patient\", \"slot\": \"2026-09-01T10:00Z\"}",
        )
        .await
        else {
            panic!("run replies");
        };
        assert!(text.contains("\"booked\""), "{text}");
    }

    /// The gap-row gate: the REPL executes through the engine under the caller
    /// principal, and capability denial is enforced (no store write happens).
    #[tokio::test]
    async fn capability_denial_enforced_and_writes_nothing() {
        let (state, store) = harness();
        let mut session = ShellSession::default();
        login_ok(&state, &mut session).await;
        let principal = session.principal.expect("logged in");
        let tenant = session.tenant.expect("tenant resolved");

        // Downgrade the installed snapshot: only demo.echo remains granted.
        state.policy.install_snapshot(AccessSnapshot {
            principal_id: principal,
            tenant_id: tenant,
            membership_active: true,
            capabilities: ["demo.echo".to_string()].into_iter().collect(),
            scope_paths: ["/".to_string()].into_iter().collect(),
            authorization_revision: state.policy.revision(),
            feature_revision: kelson_foundation::FeatureRevision(1),
            module_revision: kelson_foundation::ModuleRevision(1),
            computed_at: OffsetDateTime::now_utc(),
        });

        let Outcome::Reply(text) = execute_line(
            &state,
            &mut session,
            "run appointments.book {\"patient\": \"A. Patient\", \"slot\": \"2026-09-02T10:00Z\"}",
        )
        .await
        else {
            panic!("run replies");
        };
        assert!(text.contains("AUTH_DENIED"), "{text}");
        assert_eq!(
            store.count(tenant).expect("store counts"),
            0,
            "denial writes nothing"
        );
    }

    #[tokio::test]
    async fn ops_and_routes_introspect_the_booted_graph() {
        let (state, _store) = harness();
        let mut session = ShellSession::default();
        let Outcome::Reply(text) = execute_line(&state, &mut session, "ops").await else {
            panic!("ops replies");
        };
        assert!(text.contains("appointments.book"), "{text}");
        assert!(text.contains("appointments.cancel"), "{text}");
        assert!(text.contains("appointments.list"), "{text}");
        assert!(text.contains("demo.echo"), "{text}");

        let Outcome::Reply(text) = execute_line(&state, &mut session, "routes").await else {
            panic!("routes replies");
        };
        assert!(text.contains("appointments"), "{text}");
    }

    /// Wave 4 Phase 9 (B-29 completion): `caps` marks session grants over
    /// the declared catalog (anonymous sees the catalog without grants);
    /// `models` lists the binding + probe registries with their services.
    #[tokio::test]
    async fn caps_and_models_introspect_grants_and_registries() {
        let (state, _store) = harness();
        let mut session = ShellSession::default();

        // Anonymous: the declared catalog is visible, nothing granted yet.
        let Outcome::Reply(text) = execute_line(&state, &mut session, "caps").await else {
            panic!("caps replies");
        };
        assert!(text.contains("appointments.book"), "{text}");
        assert!(text.contains("appointments.availability"), "{text}");
        assert!(text.contains("anonymous"), "{text}");
        assert!(!text.contains("  granted"), "{text}");

        // After login the installed snapshot marks every declared grant.
        login_ok(&state, &mut session).await;
        let Outcome::Reply(text) = execute_line(&state, &mut session, "caps").await else {
            panic!("caps replies");
        };
        assert!(text.contains("appointments.book  granted"), "{text}");
        assert!(text.contains("appointments.list  granted"), "{text}");
        assert!(!text.contains("anonymous"), "{text}");

        // Models: the harness registers Appointment in both registries.
        let Outcome::Reply(text) = execute_line(&state, &mut session, "models").await else {
            panic!("models replies");
        };
        assert!(text.contains("Appointment  bindings+probes"), "{text}");
    }

    #[test]
    fn json_balance_detects_open_delimiters() {
        assert!(!json_unbalanced("run op {}"));
        assert!(!json_unbalanced("run op {\"a\": [1, 2]}"));
        assert!(json_unbalanced("run op {"));
        assert!(json_unbalanced("run op {\"a\": \"}\"")); // open string
        assert!(!json_unbalanced("run op {\"a\": \"{}\"}")); // braces in string
    }
}
