//! Wave 9 B-30 facet suite: the WASM plugin runtime over the real edge.
//!
//! Covers the documented gates end to end against the checked-in fixtures
//! (crates/extension/wasm/fixtures): loader + guest-manifest refusal paths
//! (the exact `load_wasm_plugins` call production boot makes), bus
//! dispatch roundtrips, the engine's pre-dispatch authorization denial,
//! `host.capability_check` declared/scope verdicts, and the pinned
//! `KER_WASM_FUEL` -> 503 envelope.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use keel_context::{ExecutionContext, PrincipalContext, ScopeNodeRef, ScopeSnapshot, TenantContext};
use keel_execution::{
    AuthzDecision, AuthzRequest, AuthorizationPort, Engine, InMemoryBackend, InMemoryIdempotency,
};
use keel_foundation::{
    codes, AuthorizationRevision, ExecutionProfileKind, OperationId, PrincipalId, ScopeNodeId,
    SecurityPlane, TenantId,
};
use keel_registry::{PackageIndex, WasmModuleEntry};
use serde_json::{json, Value};

/// The checked-in plugin fixtures live with the runtime crate that hosts
/// them (Design C: keel-api tests read them by path, nothing embeds wasm).
fn fixtures_dir() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/extension/wasm/fixtures"
    ))
}

fn temp_root(tag: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "keel-wasm-{tag}-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp root");
    dir
}

fn write_index(root: &Path, wasm: Vec<WasmModuleEntry>) {
    PackageIndex {
        package: Vec::new(),
        wasm,
    }
    .save(root)
    .expect("index writes");
}

/// An index entry pointing at a checked-in fixture (guest + sidecar). The
/// fixture sidecars omit `version`, so both sides default to "v1".
fn fixture_entry(name: &str) -> WasmModuleEntry {
    let dir = fixtures_dir();
    WasmModuleEntry {
        name: name.to_string(),
        version: "v1".to_string(),
        path: dir.join(format!("{name}.wat")).display().to_string(),
        manifest: dir.join(format!("{name}.module.toml")).display().to_string(),
    }
}

fn op(name: &str) -> OperationId {
    OperationId::parse(name.to_string()).expect("operation parses")
}

fn ctx() -> Arc<ExecutionContext> {
    let now = time::OffsetDateTime::now_utc();
    ExecutionContext::builder(ExecutionProfileKind::Read, now)
        .principal(PrincipalContext {
            id: PrincipalId::new(),
            display: None,
        })
        .tenant(TenantContext {
            id: TenantId::new(),
            slug: "acme".into(),
            plan: "standard".into(),
        })
        .scope(ScopeSnapshot {
            nodes: vec![],
            revision: AuthorizationRevision(1),
            computed_at: now,
        })
        .build()
        .expect("context builds")
}

/// The same context with one resolved scope node on the execution (the
/// materialized path `host.capability_check` matches against).
fn scoped_ctx(path: &str) -> Arc<ExecutionContext> {
    let now = time::OffsetDateTime::now_utc();
    ExecutionContext::builder(ExecutionProfileKind::Read, now)
        .principal(PrincipalContext {
            id: PrincipalId::new(),
            display: None,
        })
        .tenant(TenantContext {
            id: TenantId::new(),
            slug: "acme".into(),
            plan: "standard".into(),
        })
        .scope(ScopeSnapshot {
            nodes: vec![ScopeNodeRef {
                id: ScopeNodeId::new(),
                kind: "LOCATION".into(),
                path: path.to_string(),
            }],
            revision: AuthorizationRevision(1),
            computed_at: now,
        })
        .build()
        .expect("context builds")
}

struct AllowAll;
impl AuthorizationPort for AllowAll {
    fn authorize(&self, _ctx: &ExecutionContext, _request: &AuthzRequest) -> AuthzDecision {
        AuthzDecision::allow("wasm.test.allow-all")
    }
}

struct DenyAll;
impl AuthorizationPort for DenyAll {
    fn authorize(&self, _ctx: &ExecutionContext, _request: &AuthzRequest) -> AuthzDecision {
        AuthzDecision::deny("wasm.test.deny-all", "pre-dispatch denial")
    }
}

fn engine(authz: Arc<dyn AuthorizationPort>) -> Arc<Engine> {
    Arc::new(Engine::new(
        authz,
        Arc::new(InMemoryBackend::new()),
        Arc::new(InMemoryIdempotency::new()),
    ))
}

/// The loader's boot-refusal gates: absent index, happy path with plan
/// assertions, then every refusal (missing binary, non-READ capability,
/// malformed guest manifest, guest/sidecar drift).
#[test]
fn wasm_loader_gates_discovery_and_the_guest_abi() {
    // Absent index = Ok(None): the default path loads nothing, changes nothing.
    let empty = temp_root("none");
    assert!(crate::load_wasm_plugins(&empty).expect("absent index is empty").is_none());

    // Happy path: the echo fixture compiles, links, passes the ABI gate and
    // drift check, yielding one handler with the sidecar-verified plan.
    let root = temp_root("echo");
    write_index(&root, vec![fixture_entry("echo")]);
    let bundle = crate::load_wasm_plugins(&root)
        .expect("echo loads")
        .expect("bundle present");
    assert_eq!(bundle.0.len(), 1);
    let plan = bundle.0[0].plan();
    assert_eq!(plan.operation.to_string(), "echo.echo");
    assert_eq!(plan.authorization.capability.as_deref(), Some("echo.echo"));
    assert_eq!(plan.plane, SecurityPlane::Tenant);
    assert!(plan.profile.is_readonly(), "plugins are read-only ABI v1");

    // Missing binary: refused at discovery, before any wasm is touched.
    let root = temp_root("missing");
    let mut entry = fixture_entry("echo");
    entry.path = fixtures_dir().join("absent.wasm").display().to_string();
    write_index(&root, vec![entry]);
    let err = crate::load_wasm_plugins(&root).expect_err("missing binary refuses");
    assert!(err.contains("does not exist"), "{err}");

    // A non-READ capability: refused (guest delta ABI backlog) — the same
    // rule `keel check`/the console apply at graph discovery.
    let root = temp_root("readonly");
    let sidecar = root.join("mutate.toml");
    std::fs::write(
        &sidecar,
        "[module]\nname = \"mutate\"\n\n[capabilities.write_all]\nprofile = \"COMMAND\"\n",
    )
    .expect("sidecar writes");
    let mut entry = fixture_entry("echo");
    entry.name = "mutate".to_string();
    entry.version = "v1".to_string();
    entry.manifest = sidecar.display().to_string();
    write_index(&root, vec![entry]);
    let err = crate::load_wasm_plugins(&root).expect_err("write profile refuses");
    assert!(err.contains("read-only"), "{err}");

    // A malformed guest manifest (the garbage fixture returns "not json"):
    // refused with plugin context BEFORE any drift comparison runs.
    let root = temp_root("garbage");
    write_index(&root, vec![fixture_entry("garbage")]);
    let err = crate::load_wasm_plugins(&root).expect_err("malformed manifest refuses");
    assert!(err.contains("wasm plugin garbage"), "{err}");
    assert!(err.contains("manifest parse failed"), "{err}");

    // Drift: the sidecar passes every discovery rule but disagrees with the
    // guest's own keel_manifest() (inputs declared only on the reviewed
    // side) — boot refused with both documents in the message.
    let root = temp_root("drift");
    let sidecar = root.join("drift.toml");
    std::fs::write(
        &sidecar,
        "[module]\nname = \"echo\"\n\n[capabilities.echo]\nprofile = \"READ\"\ninputs = [\"msg\"]\n",
    )
    .expect("sidecar writes");
    let mut entry = fixture_entry("echo");
    entry.manifest = sidecar.display().to_string();
    write_index(&root, vec![entry]);
    let err = crate::load_wasm_plugins(&root).expect_err("drift refuses");
    assert!(err.contains("drifts from its sidecar"), "{err}");
}

/// Loaded plugins dispatch through the CommandBus exactly like
/// first-party handlers: JSON in, JSON out, guest computation intact.
#[tokio::test]
async fn wasm_operations_dispatch_through_the_bus() {
    let root = temp_root("dispatch");
    write_index(&root, vec![fixture_entry("echo"), fixture_entry("fib")]);
    let bundle = crate::load_wasm_plugins(&root)
        .expect("both plugins load")
        .expect("bundle present");
    assert_eq!(bundle.0.len(), 2, "one handler per declared capability");

    let mut bus = keel_command::CommandBus::new(engine(Arc::new(AllowAll)));
    for handler in &bundle.0 {
        bus.register(handler.clone()).expect("registers");
    }

    let out = bus
        .dispatch(ctx(), &op("echo.echo"), json!({"msg": "hi"}), None)
        .await
        .expect("echo dispatches");
    assert_eq!(out.result, json!({"echo": {"msg": "hi"}}));

    // The fib plugin parses the decimal digits out of the JSON input and
    // runs an iterated Fibonacci inside the fuel budget: F(10) = 55.
    let out = bus
        .dispatch(ctx(), &op("fib.fib"), json!({"n": 10}), None)
        .await
        .expect("fib dispatches");
    assert_eq!(out.result, json!({"fib": 55}));
}

/// The engine's pre-dispatch authorization gate answers with the denied
/// error — the guest never executes (the 403 facet is AppError::denied).
#[tokio::test]
async fn wasm_dispatch_denies_before_the_guest_runs() {
    let root = temp_root("denied");
    write_index(&root, vec![fixture_entry("echo")]);
    let bundle = crate::load_wasm_plugins(&root)
        .expect("echo loads")
        .expect("bundle present");
    let mut bus = keel_command::CommandBus::new(engine(Arc::new(DenyAll)));
    for handler in &bundle.0 {
        bus.register(handler.clone()).expect("registers");
    }

    let err = bus
        .dispatch(ctx(), &op("echo.echo"), json!({"msg": "hi"}), None)
        .await
        .expect_err("denied before dispatch");
    assert_eq!(err.code, codes::DENIED);
}

/// `host.capability_check` verdicts: declared set + scope chain, with the
/// no-scope and out-of-scope cases fail-closed to `false`.
#[tokio::test]
async fn guest_capability_checks_fail_closed_declared_and_scoped() {
    let root = temp_root("capcheck");
    write_index(&root, vec![fixture_entry("capcheck")]);
    let bundle = crate::load_wasm_plugins(&root)
        .expect("capcheck loads")
        .expect("bundle present");
    let mut bus = keel_command::CommandBus::new(engine(Arc::new(AllowAll)));
    for handler in &bundle.0 {
        bus.register(handler.clone()).expect("registers");
    }

    async fn probe(
        bus: &keel_command::CommandBus,
        ctx: Arc<ExecutionContext>,
        input: Value,
    ) -> Value {
        bus.dispatch(ctx, &op("capcheck.probe"), input, None)
            .await
            .expect("probe dispatches")
            .result
    }

    // Execution carries the resolved scope chain `LE1|LOC5`.
    let scoped = || scoped_ctx("LE1|LOC5");
    // Declared action, no path: allowed from the sidecar-verified set.
    assert_eq!(
        probe(&bus, scoped(), json!("probe")).await,
        json!({"allowed": true})
    );
    // Ancestor path within the scope (prefix chain): allowed.
    assert_eq!(
        probe(&bus, scoped(), json!("probe|LE1")).await,
        json!({"allowed": true})
    );
    // Exact node path: allowed.
    assert_eq!(
        probe(&bus, scoped(), json!("probe|LE1|LOC5")).await,
        json!({"allowed": true})
    );
    // Sibling outside the resolved scope: denied.
    assert_eq!(
        probe(&bus, scoped(), json!("probe|LOC5")).await,
        json!({"allowed": false})
    );
    // An action the plugin never declared can never be allowed — a guest
    // cannot elevate itself past the reviewed sidecar.
    assert_eq!(
        probe(&bus, scoped(), json!("admin.wipe")).await,
        json!({"allowed": false})
    );
    // No scope on the execution: any non-empty path check fail-closes.
    assert_eq!(
        probe(&bus, ctx(), json!("probe|LE1")).await,
        json!({"allowed": false})
    );
    // The declared action with no path still answers without a scope.
    assert_eq!(
        probe(&bus, ctx(), json!("probe")).await,
        json!({"allowed": true})
    );
}

/// Fuel exhaustion becomes KER_WASM_FUEL, and the pinned renderer (both
/// boot sites set the same pin) serves it at HTTP 503.
#[tokio::test]
async fn wasm_fuel_exhaustion_serves_the_pinned_503_envelope() {
    let root = temp_root("fuel");
    write_index(&root, vec![fixture_entry("spin")]);

    // A tiny budget: instantiation + the manifest scan fit, the guest's
    // infinite loop does not.
    let host = keel_wasm::WasmHost::with_fuel(50_000).expect("host builds");
    let index = PackageIndex::load(&root).expect("index loads");
    let entry = &index.wasm[0];
    let sidecar = index.wasm_manifest(&root, entry).expect("sidecar verifies");
    let bytes = std::fs::read(PackageIndex::resolve_path(&root, &entry.path))
        .expect("binary reads");
    let plugin = Arc::new(host.load(&bytes, &sidecar).expect("spin loads"));
    let handlers = keel_wasm::handlers(plugin).expect("handlers build");
    assert_eq!(handlers.len(), 1);

    let mut bus = keel_command::CommandBus::new(engine(Arc::new(AllowAll)));
    for handler in handlers {
        bus.register(handler).expect("registers");
    }
    let err = bus
        .dispatch(ctx(), &op("spin.spin"), json!({}), None)
        .await
        .expect_err("fuel runs out");
    assert_eq!(err.code, codes::WASM_FUEL);

    // Production renderer parity: the harness pin (mirroring the boot pin)
    // serves the envelope at 503 with the append-only code.
    let dir = temp_root("fuel-render");
    let (state, _, _) = crate::build_app_state(&dir, false, true);
    let (status, envelope) = state.error_envelope(&err);
    assert_eq!(status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    let body = serde_json::to_value(envelope).expect("envelope serializes");
    assert_eq!(body["error"]["code"], "KER_WASM_FUEL");
    assert_eq!(body["error"]["retryable"], true);
}
