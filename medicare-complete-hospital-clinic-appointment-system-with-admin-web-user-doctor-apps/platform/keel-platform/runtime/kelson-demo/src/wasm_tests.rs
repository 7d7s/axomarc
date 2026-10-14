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
/// fixture sidecars have `version = "0.1.0"`, matching the index entry.
fn fixture_entry(name: &str) -> WasmModuleEntry {
    let dir = fixtures_dir();
    WasmModuleEntry {
        name: name.to_string(),
        version: "0.1.0".to_string(),
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

    // A COMMAND-profiled capability: ACCEPTED under Phase 3 Ã‚Â§5.3 Ã¢â‚¬â€ the
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
    // side) Ã¢â‚¬â€ boot refused with both documents in the message.
    let root = temp_root("drift");
    let sidecar = root.join("drift.toml");
    std::fs::write(
        &sidecar,
        "[module]\nname = \"echo\"\nversion = \"0.1.0\"\napi = 1\n\n[capabilities.echo]\nprofile = \"READ\"\ninputs = [\"msg\"]\n",
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
/// error Ã¢â‚¬â€ the guest never executes (the 403 facet is AppError::denied).
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
    // An action the plugin never declared can never be allowed Ã¢â‚¬â€ a guest
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

/// Ã‚Â§5.3 profile mapping: the guest profile derives from the sidecar's op
/// profile Ã¢â‚¬â€ Read/Report stay read-only, System is Admin, the
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

/// Ã‚Â§5.3 end to end: a COMMAND guest's `capability_write` records a delta
/// the handler commits through the execution unit (visible as a delta
/// stone), and `capability_audit` is accepted Ã¢â‚¬â€ the verdicts echo in the
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

/// Undeclared actions stay refused on the write imports too Ã¢â‚¬â€ a guest
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

// ---------------------------------------------------- Wave M1: module pins

/// Write a pin file into a temp root, in the shape `ModulePinAdapter` writes.
fn write_pins(root: &Path, json: &str) {
    let dir = root.join("storage").join("framework");
    std::fs::create_dir_all(&dir).expect("pins dir");
    std::fs::write(dir.join("module-pins.json"), json).expect("pins write");
}

#[test]
fn pin_mismatch_refuses_boot_before_any_plugin_loads() {
    // The exact defect Wave M1 closes: pins were written by `kelson promote`
    // and restored by `kelson rollback`, but nothing that executed read them.
    // A pin naming 0.3.1 while 0.4.0 sat installed was silently ignored and
    // the unpinned bytes ran.
    //
    // The `echo` fixture loads cleanly on its own, so the ONLY reason boot
    // fails here is the pin. The refusal must also name the pin, not a
    // plugin Ã¢â‚¬â€ that is what proves the check ran BEFORE instantiation rather
    // than after.
    let root = temp_root("pins-drift");
    write_index(&root, vec![fixture_entry("echo")]);
    write_pins(&root, r#"{"echo": "0.3.1"}"#);

    let err =
        crate::load_wasm_plugins(&root).expect_err("a drifted pin refuses boot rather than running");
    assert!(err.contains("module pins"), "names the gate: {err}");
    assert!(err.contains("0.3.1"), "names the pinned version: {err}");
    assert!(err.contains("0.1.0"), "names the installed version: {err}");
    assert!(
        !err.contains("wasm plugin"),
        "refused by the pin gate, not by a plugin load: {err}"
    );
    assert!(
        err.contains("refusing to boot"),
        "says what it is doing and why: {err}"
    );
}

#[test]
fn a_pin_matching_the_index_still_boots_and_loads_the_plugin() {
    // The gate must not become an obstacle: a workspace that pins what it has
    // installed boots exactly as an unpinned one does, and the caller is told
    // how many pins were checked.
    let root = temp_root("pins-match");
    write_index(&root, vec![fixture_entry("echo")]);
    // The fixture sidecars have `version = "0.1.0"`, matching the index entry.
    write_pins(&root, r#"{"echo": "0.1.0"}"#);

    let bundle = crate::load_wasm_plugins(&root)
        .expect("a matching pin boots")
        .expect("bundle present");
    assert_eq!(bundle.0.len(), 1, "the plugin still loads");
}

#[test]
fn an_absent_pin_file_is_not_an_error_so_unpinned_workspaces_keep_booting() {
    // Pinning is opt-in. If a missing pin file refused boot, every workspace
    // that had never run `kelson promote` Ã¢â‚¬â€ including this fixture tree Ã¢â‚¬â€
    // would break on upgrade.
    let root = temp_root("pins-absent");
    write_index(&root, vec![fixture_entry("echo")]);

    let bundle = crate::load_wasm_plugins(&root)
        .expect("an absent pin file is not an error")
        .expect("bundle present");
    assert_eq!(bundle.0.len(), 1);
}

#[test]
fn a_malformed_pin_file_refuses_boot_rather_than_being_ignored() {
    // Fail-closed on the file itself: an operator who wrote pins and mistyped
    // one must not have every pin silently skipped.
    let root = temp_root("pins-malformed");
    write_index(&root, vec![fixture_entry("echo")]);
    write_pins(&root, "{not json");

    let err = crate::load_wasm_plugins(&root).expect_err("a malformed pin file refuses boot");
    assert!(err.contains("module pins"), "{err}");
    assert!(err.contains("invalid JSON"), "{err}");
}

// ------------------------------------------------------ Wave M2: [budget]

/// An index entry pairing an existing guest `.wat` with a DIFFERENT sidecar.
///
/// The M2 fixtures vary only `[budget]`, which a guest cannot declare, so the
/// checked-in `.wat` binaries are reused as-is. The drift check still has to
/// pass: it compares the guest's self-declared manifest against the sidecar's
/// name and capabilities, so those stay exactly what the guest says and only
/// the budget section differs between this sidecar and the plain one.
fn fixture_entry_with(wat: &str, sidecar: &str) -> WasmModuleEntry {
    let dir = fixtures_dir();
    WasmModuleEntry {
        name: wat.to_string(),
        version: "0.1.0".to_string(),
        path: dir.join(format!("{wat}.wat")).display().to_string(),
        manifest: dir.join(format!("{sidecar}.module.toml")).display().to_string(),
    }
}

#[test]
fn a_budget_asking_for_more_sandbox_than_the_host_refuses_boot() {
    // Tighten-only is the whole safety property of `[budget]`: a third-party
    // module must not be able to widen the sandbox an operator configured.
    // The refusal happens in `load`, so it is a boot refusal rather than a
    // surprising per-call failure.
    let root = temp_root("budget-raise");
    write_index(&root, vec![fixture_entry_with("echo", "budget")]);

    let err = crate::load_wasm_plugins(&root)
        .expect_err("an over-declared budget refuses the plugin");
    // BOTH violations in one message. A check that stopped at the first would
    // send an operator back to their editor once per attempt, and the second
    // violation would still be waiting to fail their next run.
    assert!(err.contains("max_memory_mb"), "names the memory field: {err}");
    assert!(err.contains("max_fuel"), "names the fuel field: {err}");
    assert!(err.contains("only LOWER"), "states the rule it broke: {err}");
    // And it names the offending plugin, so the operator knows which sidecar
    // to open.
    assert!(err.contains("echo"), "names the plugin: {err}");
}

#[tokio::test]
async fn a_wall_clock_deadline_interrupts_a_spinning_guest_and_pins_503() {
    // `spin.wat`'s kelson_call loops forever. Against a host whose fuel cap
    // is effectively unbounded, fuel can no longer be what stops it Ã¢â‚¬â€ so the
    // interrupt is unambiguously the epoch deadline, not instruction
    // metering wearing a different hat.
    let root = temp_root("budget-deadline");
    write_index(&root, vec![fixture_entry_with("spin", "spin_timeout")]);

    let host =
        kelson_wasm::WasmHost::with_limits(1_000_000_000_000, kelson_wasm::DEFAULT_MEMORY_BYTES)
            .expect("host builds");
    let index = PackageIndex::load(&root).expect("index loads");
    let entry = &index.wasm[0];
    let sidecar = index.wasm_manifest(&root, entry).expect("sidecar verifies");
    let bytes = std::fs::read(PackageIndex::resolve_path(&root, &entry.path))
        .expect("binary reads");
    let plugin = Arc::new(host.load(&bytes, &sidecar).expect("spin loads"));

    // The deadline is enforced, not merely parsed: the epoch ticker it depends
    // on exists and is held by this plugin. Checking this matters because a
    // deadline with no ticker behind it would never fire, and an operator
    // sizing their fleet to a bound that is documented but inert would be
    // sized to nothing at all.
    assert!(
        plugin.budget().has_deadline(),
        "the budget resolved a wall-clock deadline"
    );
    assert!(plugin.deadline_enforced(), "the epoch ticker is live");

    let handlers = kelson_wasm::handlers(Arc::clone(&plugin)).expect("handlers build");
    let mut bus = kelson_command::CommandBus::new(engine(Arc::new(AllowAll)));
    for handler in handlers {
        bus.register(handler).expect("registers");
    }

    let started = std::time::Instant::now();
    let err = bus
        .dispatch(ctx(), &op("spin.spin"), json!({}), None)
        .await
        .expect_err("the deadline interrupts the guest");
    let elapsed = started.elapsed();

    assert_eq!(err.code, codes::WASM_DEADLINE, "the deadline, not fuel");
    // Not EARLY: the budget declares 250ms and the epoch counts real elapsed
    // milliseconds, so an interrupt sooner than that would mean the clock was
    // never consulted. Slack is allowed on the far side only Ã¢â‚¬â€ a deadline may
    // run late (it is bounded by one sleep interval and OS timer slop), never
    // early, and never as a multiple of what was asked for.
    assert!(
        elapsed >= std::time::Duration::from_millis(200),
        "interrupted before its 250ms budget elapsed: {elapsed:?}"
    );

    // Production renderer parity: the harness pin (mirroring the boot pin)
    // serves it at 503, not the ResourceExhausted default of 429 Ã¢â‚¬â€ this is
    // our plugin's clock running out, not the caller sending too fast.
    let dir = temp_root("deadline-render");
    let (state, _, _) = crate::build_app_state(&dir, false, true);
    let (status, envelope) = state.error_envelope(&err);
    assert_eq!(status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    let body = serde_json::to_value(envelope).expect("envelope serializes");
    assert_eq!(body["error"]["code"], "KSN_WASM_DEADLINE");
    assert_eq!(body["error"]["retryable"], true);
}

/// A draining/drained plugin refuses calls with KSN_WASM_LIFECYCLE at 503.
/// This is the generation-swap path's mid-drain refusal: the plugin is
/// retiring, so the service is temporarily unavailable.
#[tokio::test]
async fn wasm_lifecycle_refusal_serves_the_pinned_503_envelope() {
    let root = temp_root("lifecycle");
    write_index(&root, vec![fixture_entry("echo")]);

    let host = kelson_wasm::WasmHost::new().expect("host builds");
    let index = PackageIndex::load(&root).expect("index loads");
    let entry = &index.wasm[0];
    let sidecar = index.wasm_manifest(&root, entry).expect("sidecar verifies");
    let bytes = std::fs::read(PackageIndex::resolve_path(&root, &entry.path))
        .expect("binary reads");
    let plugin = Arc::new(host.load(&bytes, &sidecar).expect("echo loads"));
    let handlers = kelson_wasm::handlers(Arc::clone(&plugin)).expect("handlers build");
    let mut bus = kelson_command::CommandBus::new(engine(Arc::new(AllowAll)));
    for handler in handlers {
        bus.register(handler).expect("registers");
    }

    // Commit the generation so it can be drained.
    plugin.commit().expect("commit");

    // Drain the generation — it's idle so this returns immediately.
    plugin.drain(std::time::Duration::from_secs(5)).expect("drain");

    // Now dispatch — the generation is drained, so the call is refused
    // with KSN_WASM_LIFECYCLE at 503.
    let err = bus
        .dispatch(ctx(), &op("echo.echo"), json!({"msg": "hi"}), None)
        .await
        .expect_err("drained generation refuses calls");

    assert_eq!(err.code, codes::WASM_LIFECYCLE, "lifecycle refusal code");

    // Production renderer parity: the harness pin (mirroring the boot pin)
    // serves it at 503 — a draining plugin is a service availability signal.
    let dir = temp_root("lifecycle-render");
    let (state, _, _) = crate::build_app_state(&dir, false, true);
    let (status, envelope) = state.error_envelope(&err);
    assert_eq!(status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    let body = serde_json::to_value(envelope).expect("envelope serializes");
    assert_eq!(body["error"]["code"], "KSN_WASM_LIFECYCLE");
    assert_eq!(body["error"]["retryable"], true);
}

#[tokio::test]
async fn a_saturated_plugin_refuses_a_call_before_a_guest_is_instantiated() {
    let root = temp_root("budget-saturated");
    write_index(&root, vec![fixture_entry_with("spin", "spin_timeout")]);

    let host =
        kelson_wasm::WasmHost::with_limits(1_000_000_000_000, kelson_wasm::DEFAULT_MEMORY_BYTES)
            .expect("host builds");
    let index = PackageIndex::load(&root).expect("index loads");
    let entry = &index.wasm[0];
    let sidecar = index.wasm_manifest(&root, entry).expect("sidecar verifies");
    let bytes = std::fs::read(PackageIndex::resolve_path(&root, &entry.path))
        .expect("binary reads");
    let plugin = Arc::new(host.load(&bytes, &sidecar).expect("spin loads"));
    assert_eq!(
        plugin.budget().max_concurrency,
        Some(1),
        "the concurrency cap resolved"
    );

    let handlers = kelson_wasm::handlers(Arc::clone(&plugin)).expect("handlers build");
    let mut bus = kelson_command::CommandBus::new(engine(Arc::new(AllowAll)));
    for handler in handlers {
        bus.register(handler).expect("registers");
    }

    // Occupy the plugin's single slot with a call that spins until its 250ms
    // deadline releases it. Driven directly rather than through a second bus
    // because it is the same `WasmPlugin::call` the dispatch edge makes, and a
    // plain thread keeps it running while this thread drives its own Ã¢â‚¬â€ on the
    // current-thread runtime a spawned task would block this one instead.
    let holder = Arc::clone(&plugin);
    let holder_thread = std::thread::spawn(move || {
        holder.call(
            "spin.spin",
            "{}",
            None,
            kelson_wasm::WasmCapabilityProfile::ReadOnly,
        )
    });

    // Wait for the slot to be genuinely occupied instead of guessing a delay.
    // A fixed sleep would be a race with the scheduler: too short and the test
    // would pass without ever testing the cap, too long and it would slow
    // every run for nothing.
    let wait_until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while plugin.in_flight() == 0 {
        assert!(
            std::time::Instant::now() < wait_until,
            "the holding call never took its slot Ã¢â‚¬â€ spin_timeout.module.toml's \
             execution_timeout_ms may be shorter than scheduler jitter"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }

    let started = std::time::Instant::now();
    let err = bus
        .dispatch(ctx(), &op("spin.spin"), json!({}), None)
        .await
        .expect_err("the only slot is occupied");
    let refused_in = started.elapsed();

    assert_eq!(err.code, codes::WASM_SATURATED);
    assert!(
        plugin.in_flight() == 1,
        "the refused call did not consume a slot of its own"
    );
    // Refusal, not queueing, and without running a guest: had a second
    // instance been created it would have spun to its own deadline, so the
    // error would be KSN_WASM_DEADLINE and this would have taken ~250ms.
    assert!(
        refused_in < std::time::Duration::from_millis(100),
        "the refusal ran a guest rather than refusing before instantiation: {refused_in:?}"
    );

    // The slot is released when the holder ends, so the cap sheds load
    // instead of latching the plugin shut forever.
    let held = holder_thread.join().expect("holder thread finishes");
    assert_eq!(
        held.expect_err("the holder is stopped by its deadline").to_string(),
        "wasm plugin exceeded its wall-clock deadline"
    );
    assert_eq!(plugin.in_flight(), 0, "the slot was returned");

    let dir = temp_root("saturated-render");
    let (state, _, _) = crate::build_app_state(&dir, false, true);
    let (status, envelope) = state.error_envelope(&err);
    assert_eq!(status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    let body = serde_json::to_value(envelope).expect("envelope serializes");
    assert_eq!(body["error"]["code"], "KSN_WASM_SATURATED");
}

/// ADR 0019 Ã‚Â§6 Ã¢â‚¬â€ **prepare** stops short of registration. The guest compiles,
/// links, passes the ABI gate and drift check, and is still refused every call
/// until the HOST commits; building the dispatchers does not commit it either,
/// so the staging window is real rather than nominal.
#[test]
fn prepared_generation_refuses_dispatch_until_the_host_commits() {
    let root = temp_root("lifecycle-prepare");
    write_index(&root, vec![fixture_entry("mutate")]);
    let index = PackageIndex::load(&root).expect("index loads");
    let entry = &index.wasm[0];
    let sidecar = index.wasm_manifest(&root, entry).expect("sidecar verifies");
    let bytes = std::fs::read(PackageIndex::resolve_path(&root, &entry.path)).expect("reads");
    let host = kelson_wasm::WasmHost::new().expect("host builds");

    let plugin = Arc::new(host.prepare(&bytes, &sidecar).expect("mutate prepares"));
    assert_eq!(plugin.state(), kelson_wasm::PluginState::Prepared);

    // Fully validated and still dispatching nothing Ã¢â‚¬â€ refused before a store,
    // an instance or a guest exists, so a prepare that failed this late would
    // never have disturbed whatever was already serving.
    let refused = plugin
        .call(
            "write_all",
            "\"write_all\"",
            None,
            kelson_wasm::WasmCapabilityProfile::ReadWrite,
        )
        .expect_err("a prepared generation dispatches nothing");
    assert!(
        matches!(refused, kelson_wasm::WasmError::Lifecycle(_)),
        "expected a lifecycle refusal, got {refused:?}"
    );
    assert!(refused.to_string().contains("prepared"), "{refused}");

    // The demo boot path builds the handlers first and commits afterwards, so
    // handler construction must not be what promotes the generation.
    let _handlers = kelson_wasm::handlers(Arc::clone(&plugin)).expect("handlers build");
    assert_eq!(
        plugin.state(),
        kelson_wasm::PluginState::Prepared,
        "building handlers committed the generation"
    );

    plugin.commit().expect("the host commits");
    assert_eq!(plugin.state(), kelson_wasm::PluginState::Ready);
    // Committing the same generation twice describes one host decision, not
    // two Ã¢â‚¬â€ the thing that must never be idempotent is the move *out* of a
    // retire, which `drain_is_a_one_way_retire` pins below.
    plugin.commit().expect("commit is idempotent");

    let out = plugin
        .call(
            "write_all",
            "\"write_all\"",
            None,
            kelson_wasm::WasmCapabilityProfile::ReadWrite,
        )
        .expect("a committed generation dispatches");
    assert_eq!(out.deltas.len(), 1, "the guest ran and recorded its delta");
}

/// ADR 0019 Ã‚Â§6 Ã¢â‚¬â€ **drain** is a one-way retire: it stops accepting calls,
/// a second caller joins the same wait, and nothing can put the generation
/// back in service afterwards.
#[test]
fn drain_is_a_one_way_retire_that_refuses_calls_and_cannot_be_undone() {
    let root = temp_root("lifecycle-drain");
    write_index(&root, vec![fixture_entry("mutate")]);
    let index = PackageIndex::load(&root).expect("index loads");
    let entry = &index.wasm[0];
    let sidecar = index.wasm_manifest(&root, entry).expect("sidecar verifies");
    let bytes = std::fs::read(PackageIndex::resolve_path(&root, &entry.path)).expect("reads");
    let host = kelson_wasm::WasmHost::new().expect("host builds");

    let plugin = Arc::new(host.prepare(&bytes, &sidecar).expect("mutate prepares"));
    plugin.commit().expect("the host commits");
    assert_eq!(plugin.state(), kelson_wasm::PluginState::Ready);

    // Nothing in flight, so an idle generation retires immediately.
    plugin
        .drain(std::time::Duration::from_millis(250))
        .expect("an idle generation drains");
    assert_eq!(plugin.state(), kelson_wasm::PluginState::Drained);

    // Drained means drained: dispatch is refused on the same gate that held it
    // before commit, so a generation only ever serves between those two points.
    let refused = plugin
        .call(
            "write_all",
            "\"write_all\"",
            None,
            kelson_wasm::WasmCapabilityProfile::ReadWrite,
        )
        .expect_err("a drained generation dispatches nothing");
    assert!(
        matches!(refused, kelson_wasm::WasmError::Lifecycle(_)),
        "expected a lifecycle refusal, got {refused:?}"
    );
    assert!(refused.to_string().contains("drained"), "{refused}");

    // The asymmetry that matters: commit is idempotent going *forward*, but
    // never resurrects a retired generation. Without this a swap that drained
    // the old one could accidentally re-arm it on a second commit call.
    let err = plugin
        .commit()
        .expect_err("a drained generation cannot be recommitted");
    assert!(err.to_string().contains("drained"), "{err}");

    // Draining again is a joiner, not an error: idempotent all the way down.
    plugin
        .drain(std::time::Duration::from_millis(250))
        .expect("drain is idempotent");
    assert_eq!(plugin.state(), kelson_wasm::PluginState::Drained);
}

/// A generation the host never committed accepted no work, so a drain request
/// for it means the caller has lost track of which generation it holds Ã¢â‚¬â€
/// refused rather than silently answered.
#[test]
fn draining_a_never_committed_generation_is_refused() {
    let root = temp_root("lifecycle-drain-uncommitted");
    write_index(&root, vec![fixture_entry("mutate")]);
    let index = PackageIndex::load(&root).expect("index loads");
    let entry = &index.wasm[0];
    let sidecar = index.wasm_manifest(&root, entry).expect("sidecar verifies");
    let bytes = std::fs::read(PackageIndex::resolve_path(&root, &entry.path)).expect("reads");
    let host = kelson_wasm::WasmHost::new().expect("host builds");

    let plugin = Arc::new(host.prepare(&bytes, &sidecar).expect("mutate prepares"));
    assert_eq!(plugin.state(), kelson_wasm::PluginState::Prepared);

    let err = plugin
        .drain(std::time::Duration::from_millis(250))
        .expect_err("there is nothing to retire");
    assert!(err.to_string().contains("never committed"), "{err}");
    // Refused without moving it: the caller still owns the same staged
    // generation it started with, free to commit it or drop it.
    assert_eq!(plugin.state(), kelson_wasm::PluginState::Prepared);
}

/// Drain has to prove it *waits*: a guest genuinely inside the guest must hold
/// it, the deadline must report the calls it did not see finish instead of
/// claiming a success it never observed, and the generation must stay retired
/// across that failure.
#[test]
fn drain_waits_for_in_flight_calls_and_reports_the_wait_it_could_not_finish() {
    let root = temp_root("lifecycle-drain-wait");
    write_index(&root, vec![fixture_entry_with("spin", "spin_timeout")]);
    // Unbounded fuel, so what stops the spinning guest is unambiguously its
    // 250ms wall-clock budget rather than instruction metering.
    let host =
        kelson_wasm::WasmHost::with_limits(1_000_000_000_000, kelson_wasm::DEFAULT_MEMORY_BYTES)
            .expect("host builds");
    let index = PackageIndex::load(&root).expect("index loads");
    let entry = &index.wasm[0];
    let sidecar = index.wasm_manifest(&root, entry).expect("sidecar verifies");
    let bytes = std::fs::read(PackageIndex::resolve_path(&root, &entry.path))
        .expect("binary reads");

    let plugin = Arc::new(host.prepare(&bytes, &sidecar).expect("spin prepares"));
    plugin.commit().expect("the host commits");

    // A plain thread, not a spawned task: on a current-thread runtime the task
    // would block this one instead of running alongside it.
    let holder = Arc::clone(&plugin);
    let holder_thread = std::thread::spawn(move || {
        holder.call(
            "spin.spin",
            "{}",
            None,
            kelson_wasm::WasmCapabilityProfile::ReadOnly,
        )
    });

    // Wait for the slot to be genuinely occupied rather than guessing a delay.
    let wait_until = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while plugin.in_flight() == 0 {
        assert!(
            std::time::Instant::now() < wait_until,
            "the holding call never took its slot"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(plugin.in_flight(), 1, "the guest is inside the generation");

    // 20ms against a guest spinning for its whole 250ms budget: drain cannot
    // finish, and must name the calls it is still waiting on rather than
    // reporting a completion it never saw.
    let err = plugin
        .drain(std::time::Duration::from_millis(20))
        .expect_err("a running guest holds the drain");
    assert!(err.to_string().contains("still in flight"), "{err}");

    // A timed-out drain does not reopen the generation. It has already stopped
    // accepting calls, and putting it back in service mid-retire is precisely
    // what drain exists to prevent Ã¢â‚¬â€ so the caller's move is to drain again,
    // not to give up and route traffic back through it.
    assert_eq!(plugin.state(), kelson_wasm::PluginState::Draining);
    let refused = plugin
        .call(
            "spin.spin",
            "{}",
            None,
            kelson_wasm::WasmCapabilityProfile::ReadOnly,
        )
        .expect_err("a draining generation dispatches nothing");
    assert!(
        matches!(refused, kelson_wasm::WasmError::Lifecycle(_)),
        "expected a lifecycle refusal, got {refused:?}"
    );

    // Let the in-flight call retire on its own deadline, then a second drain
    // observes the completion the first one could not.
    let held = holder_thread.join().expect("holder finishes");
    held.expect_err("the holder is stopped by its deadline");
    assert_eq!(plugin.in_flight(), 0, "the in-flight work retired");

    plugin
        .drain(std::time::Duration::from_secs(5))
        .expect("the second drain observes the completion");
    assert_eq!(plugin.state(), kelson_wasm::PluginState::Drained);
}

/// ADR 0019's first unproven claim: *"hand-authoring a component fixture
/// without guest-side `wit-bindgen`"*. `wast` 261 ships `component-model` as a
/// **default** feature (`wast-261.0.0/Cargo.toml`, `default = ["wasm-module",
/// "component-model"]`), and `wat` 1.261 defaults the same feature on, so the
/// text parser already routes a top-level `(component ...)` to the component
/// encoder (`wast-261.0.0/src/wat.rs:44-45` -> `component/binary.rs:13`);
/// wasmtime's own doctest takes that text directly
/// (`wasmtime-36.0.17/src/runtime/component/component.rs:156-164`, which routes
/// through `wat::parse_bytes`). What had never been tested is that the round
/// trip yields something *loadable and callable*, so this fixture is written
/// entirely by hand: no `wit` file, no `wit-bindgen`, no guest code generator
/// anywhere in the workspace.
#[cfg(feature = "component-model")]
#[test]
fn a_hand_authored_component_fixture_round_trips_without_guest_side_wit_bindgen() {
    // Hand-written component text: a core guest module, a core instance built
    // from it, a core function aliased out of that instance's exports, and that
    // core function lifted back to a component-level export. Every line here is
    // authored by hand rather than generated.
    const COMPONENT: &str = r#"
        (component
          (core module $guest
            (func (export "answer") (result i32) (i32.const 42))
          )
          (core instance $inst (instantiate $guest))
          (core func $answer (alias core export $inst "answer"))
          (func (export "answer") (result s32) (canon lift (core func $answer)))
        )
    "#;

    let mut config = wasmtime::Config::new();
    config.wasm_component_model(true);
    let engine = wasmtime::Engine::new(&config).expect("engine with the component model");

    // The claim under test: the hand-authored text parses, encodes to a
    // component binary, validates and compiles without any generated guest code.
    let component = wasmtime::component::Component::new(&engine, COMPONENT)
        .expect("the hand-authored component text must load");

    // Loaded is not enough Ã¢â‚¬â€ it has to be callable through the host side.
    let mut store = wasmtime::Store::new(&engine, ());
    let linker = wasmtime::component::Linker::new(&engine);
    let instance = linker
        .instantiate(&mut store, &component)
        .expect("the fixture instantiates with no imports to satisfy");
    let func = instance
        .get_func(&mut store, "answer")
        .expect("the lifted function is exported");

    let mut results = [wasmtime::component::Val::S32(0)];
    func.call(&mut store, &[], &mut results)
        .expect("the host can call the lifted core function");
    assert_eq!(
        results[0],
        wasmtime::component::Val::S32(42),
        "the hand-authored guest computed the expected value"
    );
}

/// ADR 0019 section 5's world is now a *compiled* artifact rather than a code
/// block inside a decision record: `kelson_wasm::component` runs
/// `wasmtime::component::bindgen!` over `wit/kelson-plugin.wit`, so if the
/// package stops parsing or the `plugin` world stops resolving, the
/// feature-on build fails instead of the defect surfacing later. What this
/// case pins is the design point that would be easiest to lose in a refactor:
/// a denial is a `false` the guest sees, never a trap the host has to survive.
#[cfg(feature = "component-model")]
#[test]
fn the_v2_wit_world_compiles_and_a_denying_host_returns_false_instead_of_trapping() {
    // `bindgen!` mirrors the WIT package path rather than flattening it, so
    // the host interface lands at `kelson::plugin::host` and the export at
    // `exports::kelson::plugin::dispatch` Ã¢â‚¬â€ the same nesting a WIT
    // `use kelson:plugin/{host}` would address.
    use kelson_wasm::component::exports::kelson::plugin::dispatch::CallError;
    use kelson_wasm::component::kelson::plugin::host::Host;

    // Naming the world type is what makes "`plugin` resolved" a compiled and
    // executed fact rather than a comment: `bindgen!` emits a `Plugin` struct
    // per world, and it only exists if the package resolved end to end.
    let world = core::any::type_name::<kelson_wasm::component::Plugin>();
    assert!(
        world.ends_with("Plugin"),
        "the WIT world should resolve to a `Plugin` type, got {world}"
    );

    /// A host that denies everything Ã¢â‚¬â€ what a plugin whose sidecar declares no
    /// capabilities meets at runtime, and the shape the fail-closed rule
    /// requires.
    struct DenyingHost;

    impl Host for DenyingHost {
        fn log(&mut self, _message: String) {}

        fn capability_check(&mut self, _action: String, _path: String) -> bool {
            false
        }

        fn capability_write(&mut self, _action: String, _delta: String) -> bool {
            false
        }

        fn capability_audit(&mut self, _action: String, _record: String) -> bool {
            false
        }
    }

    let mut host = DenyingHost;
    // The refusals are values, not panics: a hostile guest cannot turn a
    // denial into an unwinding path through the host.
    assert!(!host.capability_check("appointments.read".into(), "patients/1".into()));
    assert!(!host.capability_write("appointments.write".into(), "{}".into()));
    assert!(!host.capability_audit("appointments.read".into(), "{}".into()));

    // The two failures v1 reported through bare return codes are now distinct
    // values the guest can name.
    assert!(matches!(CallError::Failed, CallError::Failed));
    assert!(!matches!(CallError::Failed, CallError::OutOfFuel));
}

/// The ABI v2 fixture's guest manifest: a minimal but *real* `module.toml`
/// shape (only `module.name` is required; every other field has a serde
/// default). Mirrored by [`V2_SIDECAR_TOML`], so `WasmHost::prepare`'s
/// normalized comparison between the two passes — and fails loudly as
/// [`kelson_wasm::WasmError::Drift`] if anyone edits one without the other.
#[cfg(feature = "component-model")]
const V2_MANIFEST_JSON: &str =
    r#"{"module":{"name":"v2-fixture"},"capabilities":{"appointments.read":{"profile":"READ"}}}"#;

/// The reviewed sidecar that [`V2_MANIFEST_JSON`] must serialize identically
/// to, declared as TOML because that is how a sidecar actually ships.
///
/// `capabilities."appointments.read"` is not decoration: `prepare` copies the
/// guest manifest's capability keys into the host's `declared` set, and the
/// ABI v2 guest passes `appointments.read` to `capability_check`, so a name
/// that did not appear here would be refused by the very check the test is
/// there to exercise.
///
/// Not gated on `component-model`: the review-gate test below runs in a
/// default build too, because detection reads the *artifact* and the refusal
/// that keeps a default build from silently accepting a component has to be
/// reachable there.
const V2_SIDECAR_TOML: &str = r#"
[module]
name = "v2-fixture"
version = "0.1.0"
api = 1

[capabilities."appointments.read"]
profile = "READ"
"#;

/// Build the ABI v2 guest fixture: a hand-authored canonical-ABI component
/// importing `kelson:plugin/host@0.2.0` and exporting
/// `kelson:plugin/dispatch@0.2.0`.
///
/// Two parts vary — `manifest_json`, which `dispatch.manifest()` returns and
/// which [`kelson_wasm::WasmHost::prepare`] normalized-compares against the
/// reviewed sidecar; and `body_json`, what an allowed `dispatch.call()`
/// returns. Supplying a marker `body_json` is what lets a test tell *which*
/// generation served a call rather than having to infer it.
///
/// Authored as WAT rather than checked in as a `.wat` fixture or a prebuilt
/// blob: the guest and its expected memory layout stay reviewable side by
/// side, and neither this file nor the boundary test needs a `wit-bindgen`
/// dependency or a binary in git.
///
/// Memory map of the one shared linear memory — `$guest` imports
/// `canonical_abi`'s memory rather than defining its own, so these segments
/// land in the same bytes the canonical ABI allocates from:
///
/// | range     | contents                                          |
/// |-----------|---------------------------------------------------|
/// | 64..192   | `manifest_json`                                   |
/// | 192..209  | the `appointments.read` action string             |
/// | 224..288  | `body_json`                                       |
/// | 512..     | `cabi_realloc`'s bump: host-lowered `op`/`input`   |
/// | 4000/4016 | the `manifest` and `call` return areas            |
#[cfg(feature = "component-model")]
fn v2_component_wat(manifest_json: &str, body_json: &str) -> String {
    /// WAT string escaping: backslashes first, then quotes. The reverse
    /// order would escape the backslashes it had just inserted.
    fn wat_string(s: &str) -> String {
        s.replace('\\', "\\\\").replace('"', "\\\"")
    }

    const MANIFEST_CAP: usize = 128;
    const BODY_CAP: usize = 64;

    // Asserted rather than assumed: a manifest that outgrew its slot would
    // silently overwrite the action string, and the capability check would
    // then see garbage instead of the test failing.
    assert!(
        manifest_json.len() <= MANIFEST_CAP,
        "guest manifest is {} bytes but only {MANIFEST_CAP} are reserved at offset 64",
        manifest_json.len()
    );
    assert!(
        body_json.len() <= BODY_CAP,
        "call body is {} bytes but only {BODY_CAP} are reserved at offset 224",
        body_json.len()
    );

    const TEMPLATE: &str = r#"
        (component
          (type $host-ty (instance
            (type $log-ty (func (param "message" string)))
            (type $check-ty (func (param "action" string) (param "path" string) (result bool)))
            (type $write-ty (func (param "action" string) (param "delta" string) (result bool)))
            (type $audit-ty (func (param "action" string) (param "record" string) (result bool)))
            (export "log" (func (type $log-ty)))
            (export "capability-check" (func (type $check-ty)))
            (export "capability-write" (func (type $write-ty)))
            (export "capability-audit" (func (type $audit-ty)))))
          (import "kelson:plugin/host@0.2.0" (instance $h (type $host-ty)))

          (alias export $h "log" (func $log))
          (alias export $h "capability-check" (func $check))
          (alias export $h "capability-write" (func $write))
          (alias export $h "capability-audit" (func $audit))

          ;; memory + realloc live in their own core module so the lowered host
          ;; imports can be built before the guest is instantiated
          (core module $canon
            (memory (export "memory") 1)
            (global $bump (mut i32) (i32.const 512))
            (func (export "cabi_realloc") (param i32 i32 i32 i32) (result i32)
              (local $p i32)
              (local.set $p (global.get $bump))
              (global.set $bump (i32.add (global.get $bump) (local.get 3)))
              (local.get $p))
            (func (export "cabi_free") (param i32 i32 i32)))

          (core instance $canon-inst (instantiate $canon))
          (alias core export $canon-inst "memory" (core memory $memory))
          (alias core export $canon-inst "cabi_realloc" (core func $realloc))

          (core func $log-low
            (canon lower (func $log) (memory $memory) (realloc $realloc) string-encoding=utf8))
          (core func $check-low
            (canon lower (func $check) (memory $memory) (realloc $realloc) string-encoding=utf8))
          (core func $write-low
            (canon lower (func $write) (memory $memory) (realloc $realloc) string-encoding=utf8))
          (core func $audit-low
            (canon lower (func $audit) (memory $memory) (realloc $realloc) string-encoding=utf8))

          (core module $guest
            (import "canonical_abi" "memory" (memory 1))
            (import "canonical_abi" "realloc" (func (param i32 i32 i32 i32) (result i32)))
            (import "host" "log" (func $log (param i32 i32)))
            (import "host" "capability-check" (func $check (param i32 i32 i32 i32) (result i32)))
            (import "host" "capability-write" (func $write (param i32 i32 i32 i32) (result i32)))
            (import "host" "capability-audit" (func $audit (param i32 i32 i32 i32) (result i32)))

            ;; static strings: manifest at 64, the action at 192, the returned
            ;; body at 224. Return areas sit far above the realloc bump.
            (data (i32.const 64) "@MANIFEST@")
            (data (i32.const 192) "appointments.read")
            (data (i32.const 224) "@BODY@")

            (func (export "manifest_impl") (result i32)
              (i32.store (i32.const 4000) (i32.const 64))
              (i32.store (i32.const 4004) (i32.const @MANIFEST_LEN@))
              (i32.const 4000))

            (func (export "call_impl")
              (param $op i32) (param $opn i32) (param $in i32) (param $inn i32) (result i32)
              (call $log (local.get $op) (local.get $opn))
              ;; `result<string, call-error>` in the return area: the variant
              ;; discriminant sits at byte 0, the payload at
              ;; `align_to(1, align(payload))` = 4 (string is 4-aligned).
              (if (i32.eqz (call $check
                    (i32.const 192) (i32.const 17) (local.get $in) (local.get $inn)))
                (then
                  (i32.store8 (i32.const 4016) (i32.const 1))
                  (i32.store8 offset=4 (i32.const 4016) (i32.const 0))
                  (return (i32.const 4016))))
              (i32.store8 (i32.const 4016) (i32.const 0))
              (i32.store offset=4 (i32.const 4016) (i32.const 224))
              (i32.store offset=8 (i32.const 4016) (i32.const @BODY_LEN@))
              (i32.const 4016))

            (func (export "post_return") (param i32)))

          (core instance $guest-inst (instantiate $guest
            (with "canonical_abi" (instance
              (export "memory" (memory $memory))
              (export "realloc" (func $realloc))))
            (with "host" (instance
              (export "log" (func $log-low))
              (export "capability-check" (func $check-low))
              (export "capability-write" (func $write-low))
              (export "capability-audit" (func $audit-low))))))

          (alias core export $guest-inst "manifest_impl" (core func $mi))
          (alias core export $guest-inst "call_impl" (core func $ci))
          (alias core export $guest-inst "post_return" (core func $post))

          (type $e (enum "failed" "out-of-fuel"))
          (type $r2 (result string (error $e)))
          (type $manifest-ty (func (result string)))
          (type $call-ty (func (param "op" string) (param "input" string) (result $r2)))

          (func $manifest (type $manifest-ty)
            (canon lift (core func $mi) (memory $memory) (realloc $realloc)
              string-encoding=utf8 (post-return $post)))
          (func $call (type $call-ty)
            (canon lift (core func $ci) (memory $memory) (realloc $realloc)
              string-encoding=utf8 (post-return $post)))

          (instance $dispatch
            (export "call-error" (type $e))
            (export "manifest" (func $manifest))
            (export "call" (func $call)))
          (export "kelson:plugin/dispatch@0.2.0" (instance $dispatch)))
    "#;

    TEMPLATE
        .replace("@MANIFEST@", &wat_string(manifest_json))
        .replace("@MANIFEST_LEN@", &manifest_json.len().to_string())
        .replace("@BODY@", &wat_string(body_json))
        .replace("@BODY_LEN@", &body_json.len().to_string())
}

/// The ABI v2 guest, built here as a hand-authored canonical-ABI component and
/// then **called** through the generated `kelson:plugin/dispatch` bindings.
///
/// This is the executable half of ADR 0019 §6's upgrade proof: before a v1 → v2
/// swap can be claimed, the v2 side must demonstrably serve under the same
/// `WasmHost`. It settles three things by round-trip rather than by reading the
/// spec:
///
/// 1. **Detection & instantiation.** The artifact is a Component Model component
///    exporting `kelson:plugin/dispatch@0.2.0` — the structural rule of ADR 0019
///    §2 — and it loads through the same `Plugin` handle as a v1 core module.
/// 2. **Both directions of the canonical ABI.**
///    * guest → host: the lowered `log` / `capability-check` imports deliver
///      `("appointments.read", "{\"id\":1}")` intact;
///    * host → guest: `manifest` returns a plain `string`, and `call` returns
///      `result<string, call-error>` whose payload sits at the variant's
///      `payload_offset` (discriminant at byte 0, payload at 4).
/// 3. **Refusal is a value, not a trap.** With the scope unresolvable the guest
///    returns `Err(CallError::Failed)`; the call does not unwind, so a
///    capability denial can never be mistaken for a crash.
///
/// The component is authored inline in WAT rather than shipped as a fixture: it
/// keeps the boundary test free of a `wit-bindgen` dependency and of any
/// prebuilt binary blob, so the guest and its expected layout are reviewable
/// side by side in this file.
#[cfg(feature = "component-model")]
#[test]
fn v2_world_guest_round_trip() {
    use kelson_wasm::component::exports::kelson::plugin::dispatch::CallError;
    use kelson_wasm::component::kelson::plugin::host::Host;
    use kelson_wasm::component::Plugin;
    use wasmtime::component::{HasSelf, Linker};
    use wasmtime::{Engine, Store};

    let world = v2_component_wat(V2_MANIFEST_JSON, "{\"echo\":true}");

    let mut config = wasmtime::Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config).unwrap();
    let component = match wasmtime::component::Component::new(&engine, &world) {
        Ok(c) => c,
        Err(e) => panic!("component failed to build: {e:?}"),
    };

    /// Stands in for the real `Rec` capability host: records everything the
    /// guest managed to pass across the four `kelson:plugin/host` imports so
    /// the test can assert on the *lowered* direction as well as the return.
    #[derive(Default)]
    struct Rec {
        allow: bool,
        logs: Vec<String>,
        checks: Vec<(String, String)>,
    }
    impl Host for Rec {
        fn log(&mut self, message: String) {
            self.logs.push(message);
        }
        fn capability_check(&mut self, action: String, path: String) -> bool {
            self.checks.push((action, path));
            self.allow
        }
        fn capability_write(&mut self, _action: String, _delta: String) -> bool {
            false
        }
        fn capability_audit(&mut self, _action: String, _record: String) -> bool {
            false
        }
    }

    let mut linker = Linker::new(&engine);
    Plugin::add_to_linker::<Rec, HasSelf<Rec>>(&mut linker, |x| x)
        .expect("add_to_linker");

    let mut store = Store::new(&engine, Rec { allow: true, ..Rec::default() });
    let plugin = Plugin::instantiate(&mut store, &component, &linker)
        .expect("instantiate");

    // (1) `manifest: func() -> string` — a plain string result round-trips.
    let m = plugin.kelson_plugin_dispatch().call_manifest(&mut store).expect("manifest");
    assert_eq!(m, V2_MANIFEST_JSON);

    // (2) `call(op, input) -> result<string, call-error>` with the scope
    //     resolving: the Ok payload must decode as the guest's echo, and the
    //     two lowered strings must have reached the host intact.
    let ok = plugin
        .kelson_plugin_dispatch()
        .call_call(&mut store, "read", "{\"id\":1}")
        .expect("call(allow)");
    match ok {
        Ok(s) => assert_eq!(s, "{\"echo\":true}"),
        Err(e) => panic!("expected Ok from an allowed call, got {e:?}"),
    }
    assert_eq!(store.data().logs, ["read".to_string()]);
    assert_eq!(
        store.data().checks,
        [(
            "appointments.read".to_string(),
            "{\"id\":1}".to_string()
        )]
    );

    // (3) Same call with the scope unresolvable: the guest returns Err rather
    //     than trapping, so a refusal stays a refusal and not a crash.
    store.data_mut().allow = false;
    let denied = plugin
        .kelson_plugin_dispatch()
        .call_call(&mut store, "read", "{\"id\":1}")
        .expect("call(deny)");
    match denied {
        Err(CallError::Failed) => {}
        other => panic!("a denial should surface as CallError::Failed, got {other:?}"),
    }
    assert_eq!(store.data().logs.len(), 2, "each call logs exactly once");
    assert_eq!(
        store.data().checks.len(),
        2,
        "each call consults the capability exactly once"
    );
}

/// The operation every frame of the upgrade proof probes with. The ABI v2
/// fixture's guest forwards it to `host.log`, so a successful probe also
/// exercises the production `Host for CallState` implementation.
#[cfg(feature = "component-model")]
const UPGRADE_PROBE_OP: &str = "appointments.read";

/// Probe one generation with `input`.
///
/// `Ok(body)` means this generation ran a guest and returned its output.
/// `Err` covers everything that counts as "does not answer right now": a
/// lifecycle refusal (not committed, or already draining) and a capability
/// denial, both of which the swap below must not mistake for service.
#[cfg(feature = "component-model")]
fn upgrade_probe(
    g: &kelson_wasm::WasmPlugin,
    input: &str,
) -> Result<String, kelson_wasm::WasmError> {
    g.call(
        UPGRADE_PROBE_OP,
        input,
        None,
        kelson_wasm::WasmCapabilityProfile::ReadOnly,
    )
    .map(|o| o.body)
}

/// The no-gap invariant ADR 0019 §6 asks of a generation swap: does at least
/// one of `gens` answer *right now*?
///
/// It issues a real call rather than reading `state()`, so `Some(body)` means
/// guest code actually ran and produced output. A generation that is merely
/// staged, or already draining, refuses with `WasmError::Lifecycle` and
/// contributes nothing — which is precisely the bookkeeping a swap can lose.
#[cfg(feature = "component-model")]
fn upgrade_serving(gens: &[&kelson_wasm::WasmPlugin]) -> Option<String> {
    gens.iter().find_map(|g| upgrade_probe(g, "").ok())
}

/// ADR 0019 §6's three upgrade cases, all under **one** [`kelson_wasm::WasmHost`]:
///
/// 1. **v1 against v1** — the status-quo upgrade, run first so the new
///    detection cannot be what broke it. `echo.module.toml` declares no `abi`
///    key at all, and "absent means v1" has to stay enough.
/// 2. **v1 against v2** — the cross-generation swap, checked frame by frame
///    against [`upgrade_serving`]: there must be no window in which neither
///    generation answers.
/// 3. **v2 against v2** — the upgrade *after* that one, so the component
///    generation is not a one-off that only ever replaces a module.
///
/// The three are sequenced inside one test rather than split across three
/// because the claim is about one host: with a host per case, "they coexist"
/// would be an inference from three separate runs rather than an observation.
#[cfg(feature = "component-model")]
#[test]
fn three_upgrade_cases_under_one_host_leave_no_window_where_neither_serves() {
    use kelson_registry::ModuleManifest;
    use kelson_wasm::{GuestAbi, PluginState, WasmError, WasmHost};

    let host = WasmHost::new().expect("host builds");

    // ---- the two artifacts, one of each generation ----------------------
    // v1 arrives exactly as production ships it: a checked-in `.wat` fixture
    // and a sidecar TOML that declares no `abi` key.
    let v1_bytes = std::fs::read(fixtures_dir().join("echo.wat")).expect("v1 fixture reads");
    let v1_sidecar = ModuleManifest::from_toml(
        &std::fs::read_to_string(fixtures_dir().join("echo.module.toml"))
            .expect("v1 sidecar reads"),
    )
    .expect("v1 sidecar parses");
    // v2 arrives as the hand-authored component, carrying a real `module.toml`
    // shape so `prepare` runs its full normalized comparison rather than
    // being waved past it.
    let mut v2_sidecar = ModuleManifest::from_toml(V2_SIDECAR_TOML).expect("v2 sidecar parses");
    let v2a_wat = v2_component_wat(V2_MANIFEST_JSON, "{\"gen\":2}");
    let v2b_wat = v2_component_wat(V2_MANIFEST_JSON, "{\"gen\":2,\"slot\":\"b\"}");

    // The review gate fires before a byte is compiled: `V2_SIDECAR_TOML`
    // deliberately omits `abi`, so the first attempt at the v2 generation must
    // be refused rather than silently promoted. Declaring the generation and
    // loading the very same artifact immediately after is what makes the
    // refusal a rule rather than an obstacle.
    let undeclared = match host.prepare(v2a_wat.as_bytes(), &v2_sidecar) {
        Ok(_) => panic!("an undeclared component must be refused"),
        Err(e) => e,
    };
    assert!(
        undeclared.to_string().contains("guest ABI mismatch"),
        "{undeclared}"
    );
    v2_sidecar.abi = Some(2);

    // ---- case 1: v1 against v1 ------------------------------------------
    let v1a = host.prepare(&v1_bytes, &v1_sidecar).expect("v1 #1 prepares");
    assert_eq!(v1a.abi(), GuestAbi::V1, "a core module is guest ABI v1");
    assert_eq!(v1a.state(), PluginState::Prepared);
    assert!(
        upgrade_probe(&v1a, "").is_err(),
        "a prepared generation dispatches nothing"
    );
    v1a.commit().expect("v1 #1 commits");

    let v1b = host.prepare(&v1_bytes, &v1_sidecar).expect("v1 #2 prepares");
    assert_eq!(v1b.abi(), GuestAbi::V1);
    assert!(
        upgrade_serving(&[&v1a, &v1b]).is_some(),
        "case 1: the incumbent answers while its replacement is only staged"
    );
    assert!(
        upgrade_probe(&v1b, "").is_err(),
        "case 1: the staged replacement serves nothing yet"
    );
    v1b.commit().expect("v1 #2 commits");
    assert!(
        upgrade_serving(&[&v1a, &v1b]).is_some(),
        "case 1: both generations answer"
    );
    v1a.drain(std::time::Duration::from_secs(5)).expect("v1 #1 drains");
    let after1 = upgrade_serving(&[&v1a, &v1b]).expect("case 1: still serving");
    assert_eq!(after1, upgrade_probe(&v1b, "").expect("v1 #2 answers"));
    assert!(
        upgrade_probe(&v1a, "").is_err(),
        "case 1: the drained generation is retired"
    );

    // ---- case 2: v1 against v2 ------------------------------------------
    // v1b is still Ready from case 1 — it never stopped serving while its
    // successor was compiled, linked and checked, which is the whole point.
    let v2a = host.prepare(v2a_wat.as_bytes(), &v2_sidecar).expect("v2 prepares");
    assert_eq!(v2a.abi(), GuestAbi::V2, "a component is guest ABI v2");
    assert!(
        upgrade_serving(&[&v1b, &v2a]).is_some(),
        "case 2: the v1 incumbent answers while its v2 replacement is only staged"
    );
    assert!(
        upgrade_probe(&v2a, "").is_err(),
        "case 2: a prepared component dispatches nothing"
    );
    v2a.commit().expect("v2 commits");

    // Without two distinguishable bodies, "the component took over" below
    // could not be told apart from the module still answering.
    let v1_body = upgrade_probe(&v1b, "").expect("v1 serves");
    let v2_body = upgrade_probe(&v2a, "").expect("v2 serves");
    assert_ne!(
        v1_body, v2_body,
        "the generations must answer differently or the handover is unobservable"
    );

    // The v2 guest forwards `input` to `capability_check` as the path, and the
    // real `CallState` rule denies a non-empty path to an unscoped execution.
    // A test double that simply returned "allowed" would not fail here, so
    // this is what pins the production `Host for CallState` into the link.
    let unscoped = upgrade_probe(&v2a, "patients/1");
    assert!(
        matches!(unscoped, Err(WasmError::Call(_))),
        "the real fail-closed scope rule must run for the v2 guest, got {unscoped:?}"
    );
    // ...while the v1 guest never consults it, and still answers.
    assert!(
        upgrade_probe(&v1b, "patients/1").is_ok(),
        "case 2: the v1 guest is unaffected by a check it never makes"
    );

    v1b.drain(std::time::Duration::from_secs(5)).expect("v1 #2 drains");
    let after2 = upgrade_serving(&[&v1b, &v2a])
        .expect("case 2: no window in which neither generation serves");
    assert_eq!(after2, v2_body, "case 2: the component took the handover");
    assert!(
        upgrade_probe(&v1b, "").is_err(),
        "case 2: the v1 generation is retired"
    );

    // ---- case 3: v2 against v2 ------------------------------------------
    let v2b = host.prepare(v2b_wat.as_bytes(), &v2_sidecar).expect("v2 #2 prepares");
    assert_eq!(v2b.abi(), GuestAbi::V2);
    assert!(
        upgrade_serving(&[&v2a, &v2b]).is_some(),
        "case 3: the incumbent component answers while its replacement is staged"
    );
    assert!(
        upgrade_probe(&v2b, "").is_err(),
        "case 3: the staged component serves nothing yet"
    );
    v2b.commit().expect("v2 #2 commits");
    let v2b_body = upgrade_probe(&v2b, "").expect("v2 #2 serves");
    assert_ne!(
        v2_body, v2b_body,
        "case 3: the two components must be distinguishable"
    );
    v2a.drain(std::time::Duration::from_secs(5)).expect("v2 #1 drains");
    let after3 = upgrade_serving(&[&v2a, &v2b]).expect("case 3: still serving");
    assert_eq!(after3, v2b_body, "case 3: the newer component took the handover");
    assert!(
        upgrade_probe(&v2a, "").is_err(),
        "case 3: the retired component is out"
    );
}

/// ADR 0019 §2's fail-closed review gate: the `[[wasm]]` sidecar's optional
/// `abi` key must equal the artifact's actual shape, and absent means v1.
///
/// Deliberately feature-independent. Detection reads the *artifact*, so this
/// refusal has to fire in a default build — that is the build which must never
/// quietly accept a component it was never told to expect.
#[test]
fn a_component_is_refused_when_the_sidecar_never_declared_one() {
    let host = kelson_wasm::WasmHost::new().expect("host builds");
    let sidecar =
        kelson_registry::ModuleManifest::from_toml(V2_SIDECAR_TOML).expect("sidecar parses");
    assert_eq!(
        sidecar.abi, None,
        "the fixture sidecar deliberately omits `abi`, so absent must mean v1"
    );

    // The first token is the whole of detection here: nothing is compiled,
    // linked or instantiated before the refusal.
    // `expect_err` would need `WasmPlugin: Debug`, which the type deliberately
    // does not carry; matching keeps the refusal observable without adding a
    // trait impl purely for a test.
    let err = match host.prepare(b"(component)", &sidecar) {
        Ok(_) => panic!("a component with no declared generation must be refused"),
        Err(e) => e,
    };
    assert!(
        matches!(err, kelson_wasm::WasmError::Load(_)),
        "expected a load refusal, got {err:?}"
    );
    let text = err.to_string();
    assert!(text.contains("guest ABI mismatch"), "{text}");
    assert!(text.contains("Component Model component"), "{text}");
}

/// ADR 0019 §4's off-by-default posture, on its refusal path: a host built
/// without the `component-model` feature must *refuse* a declared component
/// rather than downgrade it to a module or ignore it.
///
/// The sidecar here approves `abi = 2`, so the gate above passes and what is
/// being observed is the feature-off build's own answer.
#[cfg(not(feature = "component-model"))]
#[test]
fn a_declared_component_still_refuses_in_a_build_without_the_feature() {
    let host = kelson_wasm::WasmHost::new().expect("host builds");
    let mut sidecar =
        kelson_registry::ModuleManifest::from_toml(V2_SIDECAR_TOML).expect("sidecar parses");
    sidecar.abi = Some(2);

    let err = match host.prepare(b"(component)", &sidecar) {
        Ok(_) => panic!("a default build must refuse a component rather than downgrade it"),
        Err(e) => e,
    };
    let text = err.to_string();
    assert!(text.contains("component-model"), "{text}");
    assert!(text.contains("refusing"), "{text}");
}
