//! # kelson-wasm — Wave 9 B-30: the sandboxed WASM plugin runtime.
//!
//! Executes plugin binaries (docs/19 Wave 9) inside a fuel-metered
//! [`wasmtime`] store with a fail-closed import surface (see [`mod imports`]).
//! The runtime is discovery-agnostic: the kelson-demo boot path reads the root
//! `packages.toml` `[[wasm]]` sidecar entries, and this crate validates,
//! instantiates and dispatches them.
//!
//! ## Guest ABI v1
//!
//! A plugin module must export exactly this surface (all three are required
//! at [`WasmHost::load`]; a missing or mistyped export refuses the load):
//!
//! * `memory` — the linear memory the host reads/writes through.
//! * `kelson_manifest() -> i64` — returns the packed `(ptr << 32) | len` of a
//!   NUL-terminated JSON document shaped like `module.toml`
//!   (`kelson_registry::ModuleManifest`). It is parsed with
//!   `ModuleManifest::from_json` and must serialize IDENTICALLY to the
//!   sidecar TOML the index shipped (a mismatch is [`WasmError::Drift`],
//!   refusing boot) — the guest cannot smuggle capabilities past review.
//! * `kelson_alloc(len: i32) -> i32` — host-driven scratch allocation; 0 on
//!   out-of-memory (the host treats that as a call error, never a panic).
//! * `kelson_call(op_ptr, op_len, in_ptr, in_len) -> i64` — dispatch one
//!   operation; input/output are UTF-8 JSON strings, returned packed the
//!   same way as the manifest. Any guest-side failure is a trap: the host
//!   maps it to an internal error, or to [`WasmError::Fuel`] when the
//!   instruction budget ran out.
//!
//! ## Sandbox posture
//!
//! * Fuel metering ([`DEFAULT_FUEL`]) bounds CPU; exhaustion becomes the
//!   append-only `KSN_WASM_FUEL` error, pinned to HTTP 503 at boot.
//! * A store-level memory limit ([`DEFAULT_MEMORY_BYTES`]) bounds the heap.
//! * `host.capability_check` only ever answers from the sidecar-verified
//!   capability set — a plugin cannot grant itself anything (see
//!   [`WasmHandler`]).
//! * Read-only profiles only: [`WasmHandler::new`] refuses any capability
//!   whose profile is not read-only (guest delta ABI backlog), matching the
//!   registry-side discovery rule so boot, console and dispatch agree.

use std::collections::BTreeSet;
use std::sync::Arc;

use kelson_context::{ExecutionContext, ScopeSnapshot};
use kelson_execution::{BoxFuture, ExecutionPlan, ExecutionUnit, OperationHandler};
use kelson_foundation::{
    codes, AppError, ErrorCategory, OperationId, Retryability, Severity,
};
use kelson_registry::ModuleManifest;
use serde_json::Value;
use wasmtime::{
    Config, Engine, Instance, InstancePre, Linker, Module, Store, StoreLimits, StoreLimitsBuilder,
    Trap,
};

mod imports;

/// Default per-call instruction budget (fuel). One shared, documented number:
/// enough for substantial business logic, small enough that a runaway loop
/// dies as a 503 instead of pinning a worker.
pub const DEFAULT_FUEL: u64 = 10_000_000;

/// Default store memory cap: 16 MiB of linear memory per plugin instance.
pub const DEFAULT_MEMORY_BYTES: usize = 16 * 1024 * 1024;

/// Per-store host state reachable from `host.*` imports.
pub(crate) struct CallState {
    pub limits: StoreLimits,
    /// Plugin label used in `host.log` diagnostics.
    pub label: String,
    /// Sidecar-verified capability names (never computed by the guest).
    pub declared: Arc<BTreeSet<String>>,
    /// Resolved scope node paths of the current execution (empty when the
    /// execution carries no scope).
    pub scope_paths: Vec<String>,
    /// Whether the execution carries a scope snapshot at all — a non-empty
    /// `path` check fail-closes without one.
    pub has_scope: bool,
    /// The executing capability's [`WasmCapabilityProfile`] — the write
    /// imports consult it per call, so one shared linker serves every
    /// profile without leaking write power into read-only guests.
    pub profile: WasmCapabilityProfile,
    /// Mutation deltas the guest recorded via `host.capability_write`
    /// (raw JSON; converted to [`MutationRecord`](kelson_execution::MutationRecord)
    /// by the handler after a successful call, discarded on guest failure).
    pub deltas: Vec<serde_json::Value>,
    /// Audit records the guest recorded via `host.capability_audit`
    /// (same lifecycle as `deltas`).
    pub audits: Vec<serde_json::Value>,
}

/// Guest capability profile (§5.3, KEEL-007): which `host.*` imports a
/// guest may use. This is the WASM-ABI concept — it must not be confused
/// with [`ExecutionProfileKind`](kelson_foundation::ExecutionProfileKind),
/// the host-side tx/locking/retry semantics matrix. The mapping below
/// keeps one invariant: [`WasmCapabilityProfile::ReadOnly`] holds exactly
/// when the engine judges the operation read-only
/// ([`ExecutionProfile::is_readonly`]), so a guest can never record a
/// delta the engine would refuse to commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmCapabilityProfile {
    /// Projections, queries: `host.capability_check` + `host.log` +
    /// `host.capability_audit` only. `host.capability_write` refuses.
    ReadOnly,
    /// Mutations, deltas: additionally `host.capability_write`.
    ReadWrite,
    /// Schema changes, tenant provisioning: the write imports, with the
    /// broadest declared scope (still sidecar-bound — never ambient).
    Admin,
}

impl WasmCapabilityProfile {
    /// Derive the guest profile from the sidecar-declared op profile.
    pub fn from_op_profile(kind: kelson_foundation::ExecutionProfileKind) -> Self {
        use kelson_foundation::ExecutionProfileKind::*;
        match kind {
            Read | Report => Self::ReadOnly,
            System => Self::Admin,
            Command | Workflow | Inventory | Financial | Batch | Replay => Self::ReadWrite,
        }
    }

    /// Whether `host.capability_write` may record for this profile.
    pub fn can_write(&self) -> bool {
        matches!(self, Self::ReadWrite | Self::Admin)
    }
}

/// Everything that can go wrong loading or running a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WasmError {
    /// Compile/link/ABI-surface failure (boot refusal).
    Load(String),
    /// The guest's `kelson_manifest()` output is missing, malformed or not a
    /// valid `module.toml` shape (boot refusal).
    Manifest(String),
    /// The guest manifest and the reviewed sidecar TOML disagree (boot
    /// refusal; both documents are included for the diagnostic).
    Drift { plugin: String, sidecar: String },
    /// The instruction budget ran out mid-call (mapped to `KSN_WASM_FUEL`).
    Fuel,
    /// A runtime failure inside one `kelson_call`.
    Call(String),
}

impl std::fmt::Display for WasmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WasmError::Load(m) => write!(f, "wasm plugin failed to load: {m}"),
            WasmError::Manifest(m) => write!(f, "wasm plugin manifest invalid: {m}"),
            WasmError::Drift { plugin, sidecar } => write!(
                f,
                "wasm plugin manifest drifts from its sidecar: guest declares {plugin}, sidecar declares {sidecar}"
            ),
            WasmError::Fuel => write!(f, "wasm plugin exceeded its instruction budget"),
            WasmError::Call(m) => write!(f, "wasm plugin call failed: {m}"),
        }
    }
}

impl std::error::Error for WasmError {}

/// Unpack a guest-returned `ptr << 32 | len` pair.
fn unpack(packed: i64) -> (u32, u32) {
    let bits = packed as u64;
    ((bits >> 32) as u32, bits as u32)
}

/// Classify a failed guest call: fuel exhaustion is its own category (it
/// becomes a pinned 503), everything else is an ordinary call failure.
fn call_error(op: &str, e: wasmtime::Error) -> WasmError {
    match e.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => WasmError::Fuel,
        Some(trap) => WasmError::Call(format!("{op}: trap {trap}")),
        None => WasmError::Call(format!("{op}: {e}")),
    }
}

/// Copy a packed guest result out of linear memory, bounded by the plugin's
/// memory cap so a bogus length can never drive an allocation.
fn read_result(
    store: &mut Store<CallState>,
    instance: &Instance,
    packed: i64,
    ctx: &str,
    max_len: usize,
) -> Result<Vec<u8>, WasmError> {
    let (ptr, len) = unpack(packed);
    if len as usize > max_len {
        return Err(WasmError::Call(format!(
            "{ctx}: result length {len} exceeds the plugin memory cap {max_len}"
        )));
    }
    let Some(memory) = instance.get_memory(&mut *store, "memory") else {
        return Err(WasmError::Call(format!(
            "{ctx}: module does not export memory"
        )));
    };
    let mut buf = vec![0u8; len as usize];
    memory
        .read(&*store, ptr as usize, &mut buf)
        .map_err(|e| WasmError::Call(format!("{ctx}: result out of bounds: {e}")))?;
    Ok(buf)
}

/// The wasmtime side of the runtime: one engine + linker (the import
/// surface) shared by every plugin loaded through it.
pub struct WasmHost {
    engine: Engine,
    linker: Linker<CallState>,
    fuel: u64,
    memory_bytes: usize,
}

impl WasmHost {
    /// A host with the default fuel and memory caps.
    pub fn new() -> Result<Self, WasmError> {
        Self::with_limits(DEFAULT_FUEL, DEFAULT_MEMORY_BYTES)
    }

    /// A host whose every plugin call burns at most `fuel` instructions
    /// (tests use this to exercise the `KSN_WASM_FUEL` path cheaply).
    pub fn with_fuel(fuel: u64) -> Result<Self, WasmError> {
        Self::with_limits(fuel, DEFAULT_MEMORY_BYTES)
    }

    /// A host with explicit caps.
    pub fn with_limits(fuel: u64, memory_bytes: usize) -> Result<Self, WasmError> {
        let mut config = Config::new();
        config.consume_fuel(true);
        let engine = Engine::new(&config).map_err(|e| WasmError::Load(format!("engine: {e}")))?;
        let mut linker: Linker<CallState> = Linker::new(&engine);
        imports::define(&mut linker)?;
        Ok(Self {
            engine,
            linker,
            fuel,
            memory_bytes,
        })
    }

    fn new_store(
        &self,
        label: &str,
        declared: Arc<BTreeSet<String>>,
        scope_paths: Vec<String>,
        has_scope: bool,
        profile: WasmCapabilityProfile,
    ) -> Result<Store<CallState>, WasmError> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(self.memory_bytes)
            .build();
        let mut store = Store::new(
            &self.engine,
            CallState {
                limits,
                label: label.to_string(),
                declared,
                scope_paths,
                has_scope,
                profile,
                deltas: Vec::new(),
                audits: Vec::new(),
            },
        );
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(self.fuel)
            .map_err(|e| WasmError::Call(format!("set fuel: {e}")))?;
        Ok(store)
    }

    /// Compile, link, validate the ABI and cross-check the guest manifest
    /// against the reviewed sidecar. Every failure here is a boot refusal
    /// (fail-closed): a plugin never reaches dispatch in a half-trusted
    /// state.
    pub fn load(&self, bytes: &[u8], sidecar: &ModuleManifest) -> Result<WasmPlugin, WasmError> {
        let module = Module::new(&self.engine, bytes)
            .map_err(|e| WasmError::Load(format!("compile: {e}")))?;
        // Link-time import validation: anything the shim does not define
        // (e.g. a `sock_*` call) fails HERE, not at first dispatch.
        let pre = self
            .linker
            .instantiate_pre(&module)
            .map_err(|e| WasmError::Load(format!("link: {e}")))?;

        // Instantiate once and read the guest's self-declared manifest.
        let json = {
            let none_declared = Arc::new(BTreeSet::new());
            // The manifest scan runs under the most restrictive profile:
            // a guest that calls the write imports from kelson_manifest()
            // records into buffers `load` never reads.
            let mut store =
                self.new_store(sidecar.name(), none_declared, Vec::new(), false, WasmCapabilityProfile::ReadOnly)?;
            let instance = pre
                .instantiate(&mut store)
                .map_err(|e| WasmError::Load(format!("instantiate: {e}")))?;
            let manifest_fn = instance
                .get_typed_func::<(), i64>(&mut store, "kelson_manifest")
                .map_err(|e| WasmError::Load(format!("export kelson_manifest: {e}")))?;
            instance
                .get_typed_func::<i32, i32>(&mut store, "kelson_alloc")
                .map_err(|e| WasmError::Load(format!("export kelson_alloc: {e}")))?;
            instance
                .get_typed_func::<(i32, i32, i32, i32), i64>(&mut store, "kelson_call")
                .map_err(|e| WasmError::Load(format!("export kelson_call: {e}")))?;

            let packed = manifest_fn
                .call(&mut store, ())
                .map_err(|e| call_error("kelson_manifest", e))?;
            let raw = read_result(
                &mut store,
                &instance,
                packed,
                "kelson_manifest",
                self.memory_bytes,
            )?;
            String::from_utf8(raw)
                .map_err(|e| WasmError::Manifest(format!("kelson_manifest is not UTF-8: {e}")))?
        };

        let parsed = ModuleManifest::from_json(&json).map_err(|v| WasmError::Manifest(v.message))?;
        let plugin_ser = serde_json::to_string(&parsed)
            .map_err(|e| WasmError::Manifest(format!("manifest re-encode failed: {e}")))?;
        let sidecar_ser = serde_json::to_string(sidecar)
            .map_err(|e| WasmError::Manifest(format!("sidecar re-encode failed: {e}")))?;
        if plugin_ser != sidecar_ser {
            return Err(WasmError::Drift {
                plugin: plugin_ser,
                sidecar: sidecar_ser,
            });
        }

        let declared: Arc<BTreeSet<String>> =
            Arc::new(parsed.capabilities.keys().cloned().collect());
        let label = parsed.name().to_string();
        Ok(WasmPlugin {
            engine: self.engine.clone(),
            pre,
            manifest: parsed,
            declared,
            label,
            fuel: self.fuel,
            memory_bytes: self.memory_bytes,
        })
    }
}

/// A loaded, drift-checked plugin: instantiate-per-call, fuel-metered.
pub struct WasmPlugin {
    engine: Engine,
    pre: InstancePre<CallState>,
    manifest: ModuleManifest,
    declared: Arc<BTreeSet<String>>,
    label: String,
    fuel: u64,
    memory_bytes: usize,
}

/// One guest invocation's full result: the JSON body PLUS the side
/// effects the guest recorded through the write imports. Effects are
/// drained from the per-call store, so a second call never replays the
/// first call's deltas.
#[derive(Debug, Clone)]
pub struct CallOutput {
    /// The guest's `kelson_call` return value, decoded as UTF-8 JSON text.
    pub body: String,
    /// Raw mutation deltas from `host.capability_write` (converted to
    /// [`MutationRecord`](kelson_execution::MutationRecord) by the handler).
    pub deltas: Vec<serde_json::Value>,
    /// Raw audit records from `host.capability_audit`.
    pub audits: Vec<serde_json::Value>,
}

impl WasmPlugin {
    /// The guest manifest, exactly as validated against the sidecar.
    pub fn manifest(&self) -> &ModuleManifest {
        &self.manifest
    }

    /// Invoke one operation: fresh instance, bounded fuel, JSON in/out.
    /// `scope` feeds `host.capability_check`'s path verdict; `profile`
    /// gates the write imports for this call.
    pub fn call(
        &self,
        operation: &str,
        input_json: &str,
        scope: Option<&ScopeSnapshot>,
        profile: WasmCapabilityProfile,
    ) -> Result<CallOutput, WasmError> {
        let (scope_paths, has_scope) = match scope {
            Some(s) => (s.nodes.iter().map(|n| n.path.clone()).collect(), true),
            None => (Vec::new(), false),
        };
        let mut store = Store::new(
            &self.engine,
            CallState {
                limits: StoreLimitsBuilder::new()
                    .memory_size(self.memory_bytes)
                    .build(),
                label: self.label.clone(),
                declared: self.declared.clone(),
                scope_paths,
                has_scope,
                profile,
                deltas: Vec::new(),
                audits: Vec::new(),
            },
        );
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(self.fuel)
            .map_err(|e| WasmError::Call(format!("set fuel: {e}")))?;

        let instance = self
            .pre
            .instantiate(&mut store)
            .map_err(|e| WasmError::Call(format!("instantiate: {e}")))?;
        let Some(memory) = instance.get_memory(&mut store, "memory") else {
            return Err(WasmError::Call(
                "module does not export memory".to_string(),
            ));
        };
        let alloc = instance
            .get_typed_func::<i32, i32>(&mut store, "kelson_alloc")
            .map_err(|e| WasmError::Call(format!("export kelson_alloc: {e}")))?;
        let kelson_call = instance
            .get_typed_func::<(i32, i32, i32, i32), i64>(&mut store, "kelson_call")
            .map_err(|e| WasmError::Call(format!("export kelson_call: {e}")))?;

        let op = operation.as_bytes();
        let input = input_json.as_bytes();
        let total = op
            .len()
            .checked_add(input.len())
            .ok_or_else(|| WasmError::Call("input length overflow".to_string()))?;
        if total > self.memory_bytes {
            return Err(WasmError::Call(format!(
                "input of {total} bytes exceeds the plugin memory cap {}",
                self.memory_bytes
            )));
        }
        let base = alloc
            .call(&mut store, total as i32)
            .map_err(|e| call_error("kelson_alloc", e))?;
        if base <= 0 {
            return Err(WasmError::Call(
                "kelson_alloc returned null (out of memory)".to_string(),
            ));
        }
        let base = base as usize;
        memory
            .write(&mut store, base, op)
            .map_err(|e| WasmError::Call(format!("write operation: {e}")))?;
        memory
            .write(&mut store, base + op.len(), input)
            .map_err(|e| WasmError::Call(format!("write input: {e}")))?;

        let packed = kelson_call
            .call(
                &mut store,
                (
                    base as i32,
                    op.len() as i32,
                    (base + op.len()) as i32,
                    input.len() as i32,
                ),
            )
            .map_err(|e| call_error("kelson_call", e))?;
        let raw = read_result(&mut store, &instance, packed, "kelson_call", self.memory_bytes)?;
        let body =
            String::from_utf8(raw).map_err(|e| WasmError::Call(format!("output is not UTF-8: {e}")))?;
        // Drain the side effects the guest recorded. A guest failure above
        // already returned, so reaching here means the call succeeded and
        // the effects are the handler's to commit (or refuse, if malformed).
        let effects = &mut store.data_mut();
        let deltas = std::mem::take(&mut effects.deltas);
        let audits = std::mem::take(&mut effects.audits);
        Ok(CallOutput {
            body,
            deltas,
            audits,
        })
    }
}

/// One plugin capability, dressed as a first-class [`OperationHandler`]:
/// plan (operation/profile/capability/plane) comes from the verified
/// sidecar, execution runs the guest. The sidecar's op profile also sets
/// the guest [`WasmCapabilityProfile`] (Read/Report → [`WasmCapabilityProfile::ReadOnly`],
/// System → [`WasmCapabilityProfile::Admin`], anything else →
/// [`WasmCapabilityProfile::ReadWrite`]) — the write imports the guest may
/// call, enforced per call in [`CallState`].
pub struct WasmHandler {
    plan: ExecutionPlan,
    plugin: Arc<WasmPlugin>,
    local: String,
    profile: WasmCapabilityProfile,
}

impl WasmHandler {
    /// Build the handler for one capability of a loaded plugin. Every
    /// sidecar profile is accepted: read-only capabilities run with the
    /// read imports, Command-family capabilities additionally get
    /// `host.capability_write`, System capabilities the full set. A guest
    /// can never exceed its sidecar: the linker only exposes declared
    /// actions (see `host.capability_check`), and the profile gates the
    /// write imports per call.
    pub fn new(plugin: Arc<WasmPlugin>, capability: &str) -> Result<Self, WasmError> {
        let manifest = plugin.manifest();
        let spec = manifest.capabilities.get(capability).ok_or_else(|| {
            WasmError::Manifest(format!(
                "capability {capability} is not declared by {}",
                manifest.name()
            ))
        })?;
        let profile = WasmCapabilityProfile::from_op_profile(spec.profile);
        let qualified = format!("{}.{}", manifest.name(), capability);
        let operation = OperationId::parse(qualified.clone())
            .map_err(|e| WasmError::Manifest(format!("operation {qualified}: {e}")))?;
        let plan = ExecutionPlan::new(operation, spec.profile)
            .with_capability(qualified)
            .with_plane(manifest.module.plane.to_security_plane());
        Ok(Self {
            plan,
            plugin,
            local: capability.to_string(),
            profile,
        })
    }

    /// The guest capability profile for this handler (what the write
    /// imports will enforce at call time).
    pub fn capability_profile(&self) -> WasmCapabilityProfile {
        self.profile
    }
}

impl OperationHandler for WasmHandler {
    fn plan(&self) -> &ExecutionPlan {
        &self.plan
    }

    fn handle(
        &self,
        ctx: Arc<ExecutionContext>,
        input: Value,
        unit: &mut dyn ExecutionUnit,
    ) -> BoxFuture<'static, Result<Value, AppError>> {
        // The guest runs synchronously here (wasmtime calls block the
        // worker thread either way — the pre-existing posture), so the
        // side effects can be recorded on `unit` directly: `unit` cannot
        // cross into the `'static` future, exactly like every first-party
        // handler (see `CreateNote` in kelson-execution's own tests).
        let plugin = Arc::clone(&self.plugin);
        let local = self.local.clone();
        let profile = self.profile;
        let scope = ctx.scope.clone();
        let result = (|| -> Result<Value, AppError> {
            let input_json = serde_json::to_string(&input)
                .map_err(|e| AppError::internal(format!("wasm input encode: {e}")))?;
            let output = plugin
                .call(&local, &input_json, scope.as_ref(), profile)
                .map_err(to_app_error)?;
            // Commit the guest's side effects through the unit BEFORE
            // returning the body, so body + deltas + audit land in the
            // same unit the engine commits atomically. A malformed delta
            // or audit record is a guest bug: fail closed (500) rather
            // than silently dropping a declared write.
            for (i, delta) in output.deltas.iter().enumerate() {
                let record = decode_mutation(&local, i, delta)?;
                unit.record_mutation(record);
            }
            for (i, entry) in output.audits.iter().enumerate() {
                let record = decode_audit(&local, i, entry)?;
                unit.record_audit(record);
            }
            serde_json::from_str(&output.body).map_err(|e| {
                AppError::internal(format!("wasm plugin output for {local} is not JSON: {e}"))
            })
        })();
        Box::pin(async move { result })
    }
}

/// Guest delta schema (`host.capability_write`): `resource_type` and
/// `after` are required; `resource_id` (uuid), `resource_version`
/// (default 0) and `before` are optional. Anything else-shaped is a guest
/// bug and refuses the whole call.
fn decode_mutation(
    capability: &str,
    index: usize,
    delta: &Value,
) -> Result<kelson_execution::MutationRecord, AppError> {
    let bad = |what: &str| {
        AppError::internal(format!(
            "wasm plugin capability {capability} delta #{index} invalid: {what}"
        ))
    };
    let obj = delta.as_object().ok_or_else(|| bad("delta must be a JSON object"))?;
    let resource_type = obj
        .get("resource_type")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad("missing string resource_type"))?;
    let after = obj
        .get("after")
        .cloned()
        .ok_or_else(|| bad("missing after image"))?;
    let resource_id = match obj.get("resource_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(
            uuid::Uuid::parse_str(s).map_err(|_| bad("resource_id is not a uuid"))?,
        ),
        Some(_) => return Err(bad("resource_id must be a uuid string")),
    };
    let resource_version = obj
        .get("resource_version")
        .map(|v| {
            v.as_i64()
                .ok_or_else(|| bad("resource_version must be an integer"))
        })
        .transpose()?
        .unwrap_or(0);
    Ok(kelson_execution::MutationRecord {
        resource_type: resource_type.to_string(),
        resource_id,
        resource_version,
        before: obj.get("before").cloned(),
        after,
        stream: None,
    })
}

/// Guest audit schema (`host.capability_audit`): `action` is required;
/// `resource` defaults to the capability, `authority` to the capability,
/// `outcome` to success, `detail` to null.
fn decode_audit(
    capability: &str,
    index: usize,
    entry: &Value,
) -> Result<kelson_execution::AuditRecord, AppError> {
    let bad = |what: &str| {
        AppError::internal(format!(
            "wasm plugin capability {capability} audit #{index} invalid: {what}"
        ))
    };
    let obj = entry.as_object().ok_or_else(|| bad("entry must be a JSON object"))?;
    let action = obj
        .get("action")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| bad("missing string action"))?;
    let outcome = match obj.get("outcome").and_then(Value::as_str) {
        None | Some("success") => kelson_execution::AuditOutcome::Success,
        Some("denied") => kelson_execution::AuditOutcome::Denied,
        Some("failed") => kelson_execution::AuditOutcome::Failed,
        Some(other) => {
            return Err(bad(&format!(
                "outcome must be success|denied|failed, got {other:?}"
            )))
        }
    };
    Ok(kelson_execution::AuditRecord {
        action: action.to_string(),
        resource: obj
            .get("resource")
            .and_then(Value::as_str)
            .unwrap_or(capability)
            .to_string(),
        authority: obj
            .get("authority")
            .and_then(Value::as_str)
            .unwrap_or(capability)
            .to_string(),
        outcome,
        detail: obj.get("detail").cloned().unwrap_or(Value::Null),
    })
}

/// Fuel exhaustion is a resource error pinned to 503 at boot (it behaves
/// like a timeout, not a client rate limit); everything else is internal.
fn to_app_error(err: WasmError) -> AppError {
    match err {
        WasmError::Fuel => AppError::new(
            codes::WASM_FUEL,
            ErrorCategory::ResourceExhausted,
            Severity::Medium,
            Retryability::Retryable,
            "wasm plugin exceeded its instruction budget",
        ),
        other => AppError::internal(format!("wasm plugin failed: {other}")),
    }
}

/// Build one handler per declared capability (the boot path).
pub fn handlers(
    plugin: Arc<WasmPlugin>,
) -> Result<Vec<Arc<dyn OperationHandler>>, WasmError> {
    let names: Vec<String> = plugin.manifest().capabilities.keys().cloned().collect();
    let mut out: Vec<Arc<dyn OperationHandler>> = Vec::with_capacity(names.len());
    for name in &names {
        out.push(Arc::new(WasmHandler::new(Arc::clone(&plugin), name)?));
    }
    Ok(out)
}

/// Handler bundle carrier for the `ServiceRegistry` — the same newtype
/// pattern as oauth2's `OauthHandlerBundle` (one instance per concrete
/// type; the plain `Vec<Arc<dyn OperationHandler>>` slot is appointments').
#[derive(Clone)]
pub struct WasmHandlerBundle(pub Vec<Arc<dyn OperationHandler>>);

impl std::fmt::Debug for WasmHandlerBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("WasmHandlerBundle").field(&self.0.len()).finish()
    }
}
