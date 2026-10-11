//! Wave 9 B-30 facet suite: the WASM plugin runtime over the real edge.
//!
//! Covers the documented gates end to end against the checked-in fixtures
//! (crates/extension/wasm/fixtures): loader + guest-manifest refusal paths
//! (the exact `load_wasm_plugins` call production boot makes), bus
//! dispatch roundtrips, the engine's pre-dispatch authorization denial,
//! `host.capability_check` declared/scope verdicts, and the pinned
//! `KSN_WASM_FUEL` -> 503 envelope.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kelson_context::{ExecutionContext, PrincipalContext, ScopeNodeRef, ScopeSnapshot, TenantContext};
use kelson_execution::{
    AuthzDecision, AuthzRequest, AuthorizationPort, Engine, InMemoryBackend, InMemoryIdempotency,
};
use kelson_foundation::{
    codes, AuthorizationRevision, ExecutionProfileKind, OperationId, PrincipalId, ScopeNodeId,
    SecurityPlane, TenantId,
};
use kelson_registry::{PackageIndex, WasmModuleEntry};
use serde_json::{json, Value};

/// The checked-in plugin fixtures live with the runtime crate that hosts
/// them (Design C: kelson-demo tests read them by path, nothing embeds wasm).
fn fixtures_dir() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../crates/extension/wasm/fixtures"
    ))
}

fn temp_root(tag: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "kelson-wasm-{tag}-{}-{}",
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

/// Same engine over a caller-held backend, so tests can assert committed
/// stones after dispatch.
fn engine_with(
    backend: Arc<kelson_execution::InMemoryBackend>,
    authz: Arc<dyn AuthorizationPort>,
) -> Arc<Engine> {
    Arc::new(Engine::new(
        authz,
        backend,
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

    // A COMMAND-profiled capability: ACCEPTED under Phase 3 §5.3 — the
    // op profile maps to the ReadWrite guest profile (the same rule `kelson
    // check`/the console apply at graph discovery). The guest can only
    // use what its sidecar declares; the write imports enforce the
    // profile per call.
    let root = temp_root("readwrite");
    write_index(&root, vec![fixture_entry("mutate")]);
    let bundle = crate::load_wasm_plugins(&root)
        .expect("command profile loads")
        .expect("bundle present");
    assert_eq!(bundle.0.len(), 1, "one handler per declared capability");
    let plan = bundle.0[0].plan();
    assert_eq!(plan.operation.to_string(), "mutate.write_all");
    assert!(
        !plan.profile.is_readonly(),
        "the plan keeps its Command semantics"
    );

    // A malformed guest manifest (the garbage fixture returns "not json"):
    // refused with plugin context BEFORE any drift comparison runs.
    let root = temp_root("garbage");
    write_index(&root, vec![fixture_entry("garbage")]);
    let err = crate::load_wasm_plugins(&root).expect_err("malformed manifest refuses");
    assert!(err.contains("wasm plugin garbage"), "{err}");
    assert!(err.contains("manifest parse failed"), "{err}");

    // Drift: the sidecar passes every discovery rule but disagrees with the
    // guest's own kelson_manifest() (inputs declared only on the reviewed
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

/// The plugin boundary is WASM-only, enforced by construction: kelson ships no
/// native-loading primitive anywhere in the dependency graph (no libloading,
/// no dlopen, no `.so`/`.dll`/`.dylib` handling), and discovery hands raw bytes
/// to wasmtime, which validates the binary format before anything else runs.
///
/// That guarantee is structural rather than a filter, so it is easy to break
/// silently later - a future "just also load native plugins" change would look
/// harmless. This pins it: an ELF/PE/Mach-O image offered as a plugin is
/// refused at boot with its load stage named, never opened as a library.
#[test]
fn wasm_only_boundary_refuses_native_images() {
    // Container magics for the three native plugin formats, each padded so the
    // bytes reach wasmtime's validator instead of failing as an empty file.
    let elf = {
        let mut v = b"\x7fELF\x02\x01\x01\x00".to_vec(); // Linux `.so`
        v.extend_from_slice(b"kelson-native-plugin-payload");
        v
    };
    let pe = {
        let mut v = b"MZ\x90\x00".to_vec(); // Windows `.dll`
        v.extend_from_slice(&[0u8; 12]);
        v.extend_from_slice(b"PE\0\0kelson-native-plugin-payload");
        v
    };
    let macho = {
        let mut v = b"\xcf\xfa\xed\xfe".to_vec(); // macOS `.dylib`
        v.extend_from_slice(b"kelson-native-plugin-payload");
        v
    };

    for (tag, bytes) in [("elf", &elf), ("pe", &pe), ("macho", &macho)] {
        let root = temp_root(&format!("native-{tag}"));
        let binary = root.join(format!("plugin-{tag}.wasm"));
        std::fs::write(&binary, bytes).expect("image writes");

        // A valid sidecar keeps the refusal attributable to the BYTES: the
        // manifest is well-formed, so failure cannot be explained by it.
        let mut entry = fixture_entry("echo");
        entry.path = binary.display().to_string();
        write_index(&root, vec![entry]);

        // `expect_err` would need the success type to be Debug; match instead.
        let err = match crate::load_wasm_plugins(&root) {
            Err(e) => e,
            Ok(_) => panic!(
                "{tag}: a native image was accepted - the WASM-only plugin boundary is broken"
            ),
        };
        assert!(err.contains("wasm plugin echo"), "{tag}: {err}");
        // The refusal happens at compile/validate, before linking or the ABI
        // gate - the image is never treated as a candidate library.
        assert!(err.contains("compile"), "{tag}: {err}");
    }
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

    let mut bus = kelson_command::CommandBus::new(engine(Arc::new(AllowAll)));
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
    let mut bus = kelson_command::CommandBus::new(engine(Arc::new(DenyAll)));
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
    let mut bus = kelson_command::CommandBus::new(engine(Arc::new(AllowAll)));
    for handler in &bundle.0 {
        bus.register(handler.clone()).expect("registers");
    }

    async fn probe(
        bus: &kelson_command::CommandBus,
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

/// The same context shape with a Command profile (the write-path
/// tests dispatch a COMMAND capability, whose plan is writable).
fn command_ctx() -> Arc<ExecutionContext> {
    let now = time::OffsetDateTime::now_utc();
    ExecutionContext::builder(ExecutionProfileKind::Command, now)
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

fn load_bundle(tag: &str, name: &str) -> (PathBuf, kelson_wasm::WasmHandlerBundle) {
    let root = temp_root(tag);
    write_index(&root, vec![fixture_entry(name)]);
    let bundle = crate::load_wasm_plugins(&root)
        .expect("fixture loads")
        .expect("bundle present");
    (root, bundle)
}

/// §5.3 profile mapping: the guest profile derives from the sidecar's op
/// profile — Read/Report stay read-only, System is Admin, the
/// Command-family writes. ReadOnly holds exactly when the engine judges
/// the operation read-only, so a guest can never record a delta the
/// engine would refuse to commit.
#[test]
fn wasm_capability_profiles_map_from_op_profiles() {
    use kelson_foundation::ExecutionProfileKind as Kind;
    use kelson_wasm::WasmCapabilityProfile as Profile;
    assert_eq!(Profile::from_op_profile(Kind::Read), Profile::ReadOnly);
    assert_eq!(Profile::from_op_profile(Kind::Report), Profile::ReadOnly);
    assert_eq!(Profile::from_op_profile(Kind::System), Profile::Admin);
    for kind in [
        Kind::Command,
        Kind::Workflow,
        Kind::Inventory,
        Kind::Financial,
        Kind::Batch,
        Kind::Replay,
    ] {
        assert_eq!(
            Profile::from_op_profile(kind),
            Profile::ReadWrite,
            "{kind:?} writes"
        );
    }
    assert!(!Profile::ReadOnly.can_write());
    assert!(Profile::ReadWrite.can_write());
    assert!(Profile::Admin.can_write());
}

/// §5.3 end to end: a COMMAND guest's `capability_write` records a delta
/// the handler commits through the execution unit (visible as a delta
/// stone), and `capability_audit` is accepted — the verdicts echo in the
/// guest output.
#[tokio::test]
async fn wasm_write_imports_commit_deltas_through_the_unit() {
    let (_root, bundle) = load_bundle("mutwrite", "mutate");
    assert_eq!(bundle.0.len(), 1);
    let backend = Arc::new(kelson_execution::InMemoryBackend::new());
    let mut bus = kelson_command::CommandBus::new(engine_with(backend.clone(), Arc::new(AllowAll)));
    for handler in &bundle.0 {
        bus.register(handler.clone()).expect("registers");
    }

    let out = bus
        .dispatch(
            command_ctx(),
            &op("mutate.write_all"),
            json!("write_all"),
            None,
        )
        .await
        .expect("write dispatches")
        .result;
    assert_eq!(out, json!({"write": 1, "audit": 1}));

    let stones = backend.stones();
    let note = stones
        .iter()
        .find(|s| s.resource_type == "notes")
        .expect("guest delta committed as a stone");
    assert_eq!(note.after["text"], json!("hi"));
}

/// Undeclared actions stay refused on the write imports too — a guest
/// cannot elevate itself past the reviewed sidecar, even for audit. The
/// refused call emits no mutations, so the engine rejects the no-op
/// command itself (`KSN_NO_EFFECT`): nothing is committed anywhere.
#[tokio::test]
async fn wasm_undeclared_write_and_audit_refuse() {
    let (_root, bundle) = load_bundle("mutrefuse", "mutate");
    let backend = Arc::new(kelson_execution::InMemoryBackend::new());
    let mut bus = kelson_command::CommandBus::new(engine_with(backend.clone(), Arc::new(AllowAll)));
    for handler in &bundle.0 {
        bus.register(handler.clone()).expect("registers");
    }

    let err = bus
        .dispatch(
            command_ctx(),
            &op("mutate.write_all"),
            json!("admin.wipe"),
            None,
        )
        .await
        .expect_err("refused writes emit no mutations");
    assert_eq!(err.code, codes::NO_EFFECT);
    assert!(
        backend.stones().is_empty(),
        "refused writes commit nothing"
    );
}

/// The profile gates per CALL, not per plugin: the same COMMAND guest run
/// under a ReadOnly profile records no deltas, while audit (append-only)
/// still records.
#[test]
fn wasm_readonly_profile_refuses_writes_but_keeps_audit() {
    let root = temp_root("mutro");
    write_index(&root, vec![fixture_entry("mutate")]);
    let index = PackageIndex::load(&root).expect("index loads");
    let entry = &index.wasm[0];
    let sidecar = index.wasm_manifest(&root, entry).expect("sidecar verifies");
    let bytes = std::fs::read(PackageIndex::resolve_path(&root, &entry.path)).expect("reads");
    let host = kelson_wasm::WasmHost::new().expect("host builds");
    let plugin = host.load(&bytes, &sidecar).expect("mutate loads");

    let out = plugin
        .call(
            "write_all",
            "\"write_all\"",
            None,
            kelson_wasm::WasmCapabilityProfile::ReadOnly,
        )
        .expect("call runs");
    assert_eq!(out.body, "{\"write\":0,\"audit\":1}");
    assert!(out.deltas.is_empty(), "no delta recorded");
    assert_eq!(out.audits.len(), 1, "audit still recorded");

    // And under its own ReadWrite profile the same bytes record both.
    let out = plugin
        .call(
            "write_all",
            "\"write_all\"",
            None,
            kelson_wasm::WasmCapabilityProfile::ReadWrite,
        )
        .expect("call runs");
    assert_eq!(out.body, "{\"write\":1,\"audit\":1}");
    assert_eq!(out.deltas.len(), 1);
    assert_eq!(out.audits.len(), 1);
}

/// Fuel exhaustion becomes KSN_WASM_FUEL, and the pinned renderer (both
/// boot sites set the same pin) serves it at HTTP 503.
#[tokio::test]
async fn wasm_fuel_exhaustion_serves_the_pinned_503_envelope() {
    let root = temp_root("fuel");
    write_index(&root, vec![fixture_entry("spin")]);

    // A tiny budget: instantiation + the manifest scan fit, the guest's
    // infinite loop does not.
    let host = kelson_wasm::WasmHost::with_fuel(50_000).expect("host builds");
    let index = PackageIndex::load(&root).expect("index loads");
    let entry = &index.wasm[0];
    let sidecar = index.wasm_manifest(&root, entry).expect("sidecar verifies");
    let bytes = std::fs::read(PackageIndex::resolve_path(&root, &entry.path))
        .expect("binary reads");
    let plugin = Arc::new(host.load(&bytes, &sidecar).expect("spin loads"));
    let handlers = kelson_wasm::handlers(plugin).expect("handlers build");
    assert_eq!(handlers.len(), 1);

    let mut bus = kelson_command::CommandBus::new(engine(Arc::new(AllowAll)));
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
    assert_eq!(body["error"]["code"], "KSN_WASM_FUEL");
    assert_eq!(body["error"]["retryable"], true);
}
