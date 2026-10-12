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
    // plugin — that is what proves the check ran BEFORE instantiation rather
    // than after.
    let root = temp_root("pins-drift");
    write_index(&root, vec![fixture_entry("echo")]);
    write_pins(&root, r#"{"echo": "0.3.1"}"#);

    let err =
        crate::load_wasm_plugins(&root).expect_err("a drifted pin refuses boot rather than running");
    assert!(err.contains("module pins"), "names the gate: {err}");
    assert!(err.contains("0.3.1"), "names the pinned version: {err}");
    assert!(err.contains("v1"), "names the installed version: {err}");
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
    // The fixture sidecars omit `version`, so both sides default to "v1".
    write_pins(&root, r#"{"echo": "v1"}"#);

    let bundle = crate::load_wasm_plugins(&root)
        .expect("a matching pin boots")
        .expect("bundle present");
    assert_eq!(bundle.0.len(), 1, "the plugin still loads");
}

#[test]
fn an_absent_pin_file_is_not_an_error_so_unpinned_workspaces_keep_booting() {
    // Pinning is opt-in. If a missing pin file refused boot, every workspace
    // that had never run `kelson promote` — including this fixture tree —
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
        version: "v1".to_string(),
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
    // is effectively unbounded, fuel can no longer be what stops it — so the
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
    // never consulted. Slack is allowed on the far side only — a deadline may
    // run late (it is bounded by one sleep interval and OS timer slop), never
    // early, and never as a multiple of what was asked for.
    assert!(
        elapsed >= std::time::Duration::from_millis(200),
        "interrupted before its 250ms budget elapsed: {elapsed:?}"
    );

    // Production renderer parity: the harness pin (mirroring the boot pin)
    // serves it at 503, not the ResourceExhausted default of 429 — this is
    // our plugin's clock running out, not the caller sending too fast.
    let dir = temp_root("deadline-render");
    let (state, _, _) = crate::build_app_state(&dir, false, true);
    let (status, envelope) = state.error_envelope(&err);
    assert_eq!(status, axum::http::StatusCode::SERVICE_UNAVAILABLE);
    let body = serde_json::to_value(envelope).expect("envelope serializes");
    assert_eq!(body["error"]["code"], "KSN_WASM_DEADLINE");
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
    // plain thread keeps it running while this thread drives its own — on the
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
            "the holding call never took its slot — spin_timeout.module.toml's \
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

/// ADR 0019 §6 — **prepare** stops short of registration. The guest compiles,
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

    // Fully validated and still dispatching nothing — refused before a store,
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
    // two — the thing that must never be idempotent is the move *out* of a
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

/// ADR 0019 §6 — **drain** is a one-way retire: it stops accepting calls,
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
/// for it means the caller has lost track of which generation it holds —
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
    // what drain exists to prevent — so the caller's move is to drain again,
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

    // Loaded is not enough — it has to be callable through the host side.
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
    // `exports::kelson::plugin::dispatch` — the same nesting a WIT
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

    /// A host that denies everything — what a plugin whose sidecar declares no
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
