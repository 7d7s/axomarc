//! Wave 5 Phase 7: `kelson call <name> [json]` — one-shot dispatch of a
//! `routes/console.toml` command through the CommandBus under the operator
//! principal (docs/19). kelson-console (Tools-class) spawns `kelson-api --call
//! ...` exactly like `--shell`/`--prune`: the dep-rules matrix keeps tools
//! from linking runtime crates, so the dispatch lives here — same production
//! boot, same graph gates (a console.toml entry referencing an unknown
//! operation refuses boot with GRAPH-ROUTEFILE-OP, the shared Wave 2 gate),
//! and the same PolicyAuthz capability checks as the HTTP edge.
//!
//! Posture (docs/19): the default operator is the first ACTIVE principal and
//! exists in dev/test only; staging/production refuse without an explicit
//! `--principal <uuid>`. There is no admin-role column yet — first active is
//! "first admin" (documented gap, docs/18 §11). Input is dispatched verbatim:
//! edge-stage validation and route-binding injection are not applied (the
//! operator surface, like `kelson shell`'s `run`).

use kelson_config::Environment;
use kelson_foundation::{AppError, OperationId, PrincipalId, TenantId};
use kelson_registry::{ConsoleCommandSpec, ModuleManifest, PlatformGraph};

use crate::routes::AppState;

/// Everything `kelson-api --call` receives from the CLI spawn. Raw strings are
/// parsed here (never silently dropped) so a typo fails closed.
pub struct CallRequest<'a> {
    pub state: &'a AppState,
    pub graph: &'a PlatformGraph,
    /// Command name from routes/console.toml.
    pub name: &'a str,
    /// Raw JSON payload (None/empty = `{}`).
    pub input: Option<&'a str>,
    /// Explicit operator principal (uuid); None = dev/test default.
    pub principal: Option<&'a str>,
    /// Explicit tenant (uuid); None = the dev/test fallback tenant.
    pub tenant: Option<&'a str>,
}

/// Resolve, authorize and dispatch one console command; the operation's JSON
/// result goes to stdout (the console exporter is off in `--call` mode), any
/// failure returns a stderr-ready message.
pub async fn run(req: CallRequest<'_>) -> Result<(), String> {
    let spec = console_command(&req.graph.modules, req.name)?;
    let principal = resolve_operator(req.state, req.principal)?;
    let tenant = resolve_tenant(req.state, req.tenant)?;
    // Login-equivalent grant: the operator gets the graph's module
    // capabilities exactly like `POST /auth/login` installs them. Checks are
    // not bypassed — an operation requiring a capability outside the granted
    // set (or a missing membership) is denied identically to the HTTP edge.
    crate::auth_routes::install_access_snapshot(req.state, principal, tenant);
    let result = execute(req.state, &spec.operation, req.input.unwrap_or(""), principal, tenant)
        .await
        .map_err(|err| match &err.internal {
            Some(detail) => format!("[{}] {detail}", err.code),
            None => format!("{err}"),
        })?;
    let text = serde_json::to_string_pretty(&result)
        .map_err(|e| format!("result is not JSON-serializable: {e}"))?;
    println!("{text}");
    Ok(())
}

/// Name -> console command against the compiled graph's console commands
/// (merged from routes/console.toml by the route-file discovery). The
/// available list rides along so a typo is actionable.
pub fn console_command<'a>(
    modules: &'a [ModuleManifest],
    name: &str,
) -> Result<&'a ConsoleCommandSpec, String> {
    let mut available: Vec<&str> = Vec::new();
    for m in modules {
        for cmd in &m.console_commands {
            if cmd.name == name {
                return Ok(cmd);
            }
            available.push(cmd.name.as_str());
        }
    }
    Err(if available.is_empty() {
        "no console commands declared — add a [[command]] entry to routes/console.toml"
            .to_string()
    } else {
        format!(
            "unknown console command {name:?} — declared: {}",
            available.join(", ")
        )
    })
}

/// Operator principal: an explicit `--principal <uuid>` (the principal must
/// exist — the snapshot install would otherwise authorize an arbitrary uuid)
/// or, in dev/test only, the store's first active principal.
fn resolve_operator(state: &AppState, explicit: Option<&str>) -> Result<PrincipalId, String> {
    if let Some(raw) = explicit {
        let uuid = uuid::Uuid::parse_str(raw)
            .map_err(|e| format!("invalid --principal {raw:?}: {e}"))?;
        let id = PrincipalId::from_uuid(uuid);
        let found = state
            .principals
            .by_id(id)
            .map_err(|e| format!("principal lookup failed: {e}"))?
            .ok_or_else(|| format!("no principal {id} exists"))?;
        return Ok(found.id);
    }
    if !matches!(state.env, Environment::Dev | Environment::Test) {
        return Err(format!(
            "the default operator exists in dev/test only (env: {}) — pass --principal <uuid>",
            state.env.as_str()
        ));
    }
    let first = state
        .principals
        .first_active()
        .map_err(|e| format!("principal lookup failed: {e}"))?
        .ok_or_else(|| {
            "no active principal yet — register a user first or pass --principal <uuid>"
                .to_string()
        })?;
    Ok(first.id)
}

/// Tenant: explicit `--tenant <uuid>`, else the dev/test fallback tenant.
/// Production has no membership store to resolve one from yet, so it must be
/// passed (documented gap, docs/18 §11).
fn resolve_tenant(state: &AppState, explicit: Option<&str>) -> Result<TenantId, String> {
    if let Some(raw) = explicit {
        let uuid =
            uuid::Uuid::parse_str(raw).map_err(|e| format!("invalid --tenant {raw:?}: {e}"))?;
        return Ok(TenantId::from_uuid(uuid));
    }
    state.default_tenant_id.ok_or_else(|| {
        "no tenant resolved — pass --tenant <uuid> (production has no dev fallback tenant yet)"
            .to_string()
    })
}

/// Dispatch one operation under the given principal + tenant: input parse,
/// handler lookup, context build (same builder as `kelson shell`), engine
/// authorization (PolicyAuthz against whatever snapshot is installed) and
/// the CommandBus round-trip. Kept free of snapshot installation so tests
/// can prove capability denial is enforced, not skipped.
pub async fn execute(
    state: &AppState,
    operation: &str,
    input: &str,
    principal: PrincipalId,
    tenant: TenantId,
) -> Result<serde_json::Value, AppError> {
    let operation = OperationId::parse(operation)?;
    let input: serde_json::Value = if input.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(input)
            .map_err(|e| AppError::validation(format!("invalid JSON input: {e}")))?
    };
    let Some(handler) = state.bus.handler(&operation) else {
        return Err(AppError::not_found(format!(
            "no handler registered for {operation}"
        )));
    };
    let plan = handler.plan();
    let ctx = crate::shell::build_context(plan.plane, plan.profile.kind, principal, tenant)?;
    let outcome = state
        .bus
        .dispatch(ctx, &operation, input, None)
        .await?;
    Ok(outcome.result)
}

#[cfg(test)]
mod tests {
    use super::*;
    // Only the demo-gated capability test below names a status code.
    #[cfg(feature = "demo")]
    use kelson_foundation::codes;

    /// Dev/test harness: production bus assembly (appointments handlers +
    /// graph-derived granted capabilities), Test env, dev fallback tenant,
    /// empty principal store.
    fn app() -> AppState {
        let dir = std::env::temp_dir().join(format!(
            "kelson-api-call-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp root");
        let (state, _graph, _store) = crate::build_app_state(&dir, false, true);
        state
    }

    fn active_principal(email: &str) -> kelson_identity::Principal {
        kelson_identity::Principal {
            id: kelson_foundation::PrincipalId::from_uuid(uuid::Uuid::now_v7()),
            status: kelson_identity::PrincipalStatus::Active,
            email: Some(email.to_string()),
            phone: None,
            email_verified_at: None,
            created_at: time::OffsetDateTime::now_utc(),
        }
    }

    /// Wave 5 Phase 7: console commands resolve by name with an actionable
    /// list on a miss; an empty surface says how to declare one.
    #[test]
    fn console_command_resolves_by_name_and_lists_on_a_miss() {
        let raw = r#"
[[command]]
name = "book"
operation = "appointments.book"

[[command]]
name = "list"
operation = "appointments.list"
"#;
        let module = kelson_registry::RouteFileManifest::from_toml(raw, "routes/console.toml", None)
            .expect("route file parses")
            .into_module("routes_console");
        let modules = [module];

        let spec = console_command(&modules, "book").expect("resolves");
        assert_eq!(spec.operation, "appointments.book");

        let err = console_command(&modules, "boo").expect_err("refuses");
        assert!(err.contains("\"boo\"") && err.contains("book") && err.contains("list"), "{err}");

        let err = console_command(&[], "book").expect_err("refuses");
        assert!(err.contains("routes/console.toml"), "{err}");
    }

    /// Wave 5 Phase 7: the default operator is the first ACTIVE principal in
    /// dev/test; an explicit --principal must exist (typo fails closed).
    #[test]
    fn operator_resolution_picks_first_active_and_rejects_unknown_explicit() {
        let state = app();
        // Empty store: nothing to default to.
        let err = resolve_operator(&state, None).expect_err("refuses");
        assert!(err.contains("no active principal"), "{err}");

        let first = state
            .principals
            .create(active_principal("first@example.test"))
            .expect("creates");
        let mut suspended = active_principal("suspended@example.test");
        suspended.status = kelson_identity::PrincipalStatus::Suspended;
        state.principals.create(suspended).expect("creates");

        let operator = resolve_operator(&state, None).expect("defaults");
        assert_eq!(operator, first.id, "first ACTIVE principal wins");

        // Explicit principal must exist.
        let known = resolve_operator(&state, Some(first.id.to_string().as_str()))
            .expect("existing principal resolves");
        assert_eq!(known, first.id);
        let ghost = uuid::Uuid::now_v7();
        let err = resolve_operator(&state, Some(ghost.to_string().as_str())).expect_err("refuses");
        assert!(err.contains("no principal"), "{err}");
        let err = resolve_operator(&state, Some("not-a-uuid")).expect_err("refuses");
        assert!(err.contains("invalid --principal"), "{err}");
    }

    /// Wave 5 Phase 7 (docs/19 test plan): console-route capability denial —
    /// the one-shot dispatch runs through PolicyAuthz like the HTTP edge:
    /// a snapshot without the operation's capability is DENIED with the
    /// "not granted" reason, and the granted snapshot executes for real.
    // `appointments.list` is registered by the demo module's handler bundle,
    // so this dispatch can only be exercised with `--features demo`.
    #[cfg(feature = "demo")]
    #[tokio::test]
    async fn console_route_dispatch_enforces_capability_checks() {
        let state = app();
        let principal = state
            .principals
            .create(active_principal("operator@example.test"))
            .expect("creates");
        let tenant = state.default_tenant_id.expect("dev fallback tenant");

        // Snapshot present but without `appointments.list`: the capability
        // check (not identity) is what denies.
        state
            .policy
            .install_snapshot(kelson_policy::AccessSnapshot {
                principal_id: principal.id,
                tenant_id: tenant,
                membership_active: true,
                capabilities: Default::default(),
                scope_paths: ["/".to_string()].into_iter().collect(),
                authorization_revision: state.policy.revision(),
                feature_revision: kelson_foundation::FeatureRevision(1),
                module_revision: kelson_foundation::ModuleRevision(1),
                computed_at: time::OffsetDateTime::now_utc(),
            });
        let err = execute(&state, "appointments.list", "{}", principal.id, tenant)
            .await
            .expect_err("denied");
        assert_eq!(err.code, codes::DENIED, "{err}");
        assert!(err.public_message.contains("appointments.list"), "{err}");

        // Grant the graph's capabilities (the login-equivalent install) and
        // the same dispatch runs for real.
        state
            .policy
            .install_snapshot(kelson_policy::AccessSnapshot {
                principal_id: principal.id,
                tenant_id: tenant,
                membership_active: true,
                capabilities: state.granted_capabilities.iter().cloned().collect(),
                scope_paths: ["/".to_string()].into_iter().collect(),
                authorization_revision: state.policy.revision(),
                feature_revision: kelson_foundation::FeatureRevision(1),
                module_revision: kelson_foundation::ModuleRevision(1),
                computed_at: time::OffsetDateTime::now_utc(),
            });
        let result = execute(&state, "appointments.list", "{}", principal.id, tenant)
            .await
            .expect("granted");
        assert!(result.is_object(), "{result}");

        // Unregistered operation and malformed input fail closed, loudly.
        let err = execute(&state, "nope.nope", "{}", principal.id, tenant)
            .await
            .expect_err("refuses");
        assert_eq!(err.code, codes::NOT_FOUND, "{err}");
        let err = execute(&state, "appointments.list", "{bad", principal.id, tenant)
            .await
            .expect_err("refuses");
        assert!(err.public_message.contains("JSON"), "{err}");
    }

    /// Wave 5 Phase 7: tenant resolution — explicit uuid wins, else the
    /// dev/test fallback, else an actionable refusal.
    #[test]
    fn tenant_resolution_falls_back_to_the_dev_tenant() {
        let state = app();
        let fallback = resolve_tenant(&state, None).expect("dev fallback");
        assert_eq!(fallback, state.default_tenant_id.expect("some"));

        let explicit = uuid::Uuid::now_v7();
        let resolved =
            resolve_tenant(&state, Some(explicit.to_string().as_str())).expect("explicit");
        assert_eq!(resolved, TenantId::from_uuid(explicit));

        let err = resolve_tenant(&state, Some("nope")).expect_err("refuses");
        assert!(err.contains("invalid --tenant"), "{err}");
    }
}
