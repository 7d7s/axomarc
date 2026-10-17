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
use kelson_registry::{ModuleBudget, ModuleManifest};
use serde_json::Value;
use wasmtime::{
    Config, Engine, Instance, InstancePre, Linker, Module, Store, StoreLimits, StoreLimitsBuilder,
    Trap,
};

mod imports;

/// A manifest `[budget]` resolved against a host's ceilings, ready to apply
/// to one store.
///
/// Resolution happens once, at load, not per call: the numbers are fixed for
/// the plugin's lifetime, so re-deriving them on every dispatch would only add
/// a way for them to drift apart. Holding all three together also means a
/// store's fuel, memory cap and deadline are set from one value, so they
/// cannot end up describing different sandboxes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallBudget {
    /// Effective per-call instruction budget.
    pub fuel: u64,
    /// Effective per-instance linear memory cap, in bytes.
    pub memory_bytes: usize,
    /// Epoch ticks before the call is interrupted. [`NO_DEADLINE_TICKS`] when
    /// the manifest declares no `execution_timeout_ms`.
    pub deadline_ticks: u64,
    /// Maximum simultaneous in-flight calls, or `None` for unlimited.
    pub max_concurrency: Option<u32>,
}

impl CallBudget {
    /// Whether this call carries a real wall-clock bound (and therefore needs
    /// the host's epoch ticker running).
    pub fn has_deadline(&self) -> bool {
        self.deadline_ticks != NO_DEADLINE_TICKS
    }
}

/// Default per-call instruction budget (fuel). One shared, documented number:
/// enough for substantial business logic, small enough that a runaway loop
/// dies as a 503 instead of pinning a worker.
pub const DEFAULT_FUEL: u64 = 10_000_000;

/// Default store memory cap: 16 MiB of linear memory per plugin instance.
pub const DEFAULT_MEMORY_BYTES: usize = 16 * 1024 * 1024;

/// Host services a module guest reaches through the seam imports (ADR 0020).
///
/// Built at boot from the *same* store the rest of the platform writes, so a
/// module reads and writes the rows first-party code reads and writes rather
/// than a second copy of them — the property the whole port exists to
/// preserve. Carried into each dispatch by value: two `Arc`s, cloned once per
/// call.
///
/// `None` in either slot means the host was not wired for it, and the seam
/// answers with a named error rather than a silence, because a module that
/// silently gets no rows is worse than one that is told the store is missing.
#[derive(Clone, Default)]
pub struct ModuleHostServices {
    pub model: Option<Arc<dyn kelson_module_contract::ModelStore>>,
    pub tokens: Option<Arc<dyn kelson_module_contract::TokenStore>>,
    /// The notification sink a `host.notify_enqueue` import reaches (ADR 0020
    /// §5). `None` means the host was not wired for notifications, which the
    /// seam answers with a named error rather than a silence.
    pub notify: Option<Arc<dyn kelson_module_contract::NotificationSink>>,
}

impl ModuleHostServices {
    /// Services wired to both stores.
    pub fn new(
        model: Arc<dyn kelson_module_contract::ModelStore>,
        tokens: Arc<dyn kelson_module_contract::TokenStore>,
    ) -> Self {
        Self {
            model: Some(model),
            tokens: Some(tokens),
            notify: None,
        }
    }

    /// The store a `host.model_*` import should reach, if one is wired.
    pub fn model_store(&self) -> Option<&dyn kelson_module_contract::ModelStore> {
        self.model.as_deref()
    }

    /// The store a `host.token_*` import should reach, if one is wired.
    pub fn token_store(&self) -> Option<&dyn kelson_module_contract::TokenStore> {
        self.tokens.as_deref()
    }

    /// The sink a `host.notify_enqueue` import reaches, if one is wired.
    ///
    /// Additive rather than a third parameter to [`Self::new`]: a host that
    /// has no notification story at all keeps the two-store constructor it
    /// already had, and a module that notifies against it is refused loudly
    /// instead of being handed an empty inbox.
    pub fn with_notifications(mut self, sink: Arc<dyn kelson_module_contract::NotificationSink>) -> Self {
        self.notify = Some(sink);
        self
    }

    /// The sink a `host.notify_enqueue` import should reach, if one is wired.
    pub fn notification_sink(&self) -> Option<&dyn kelson_module_contract::NotificationSink> {
        self.notify.as_deref()
    }
}

/// Per-store host state reachable from `host.*` imports.
pub(crate) struct CallState {
    pub limits: StoreLimits,
    /// Plugin label used in `host.log` diagnostics.
    pub label: String,
    /// Sidecar-verified capability names (never computed by the guest).
    pub declared: Arc<BTreeSet<String>>,
    /// The qualified operation THIS dispatch serves, empty for a store built
    /// for a manifest scan.
    ///
    /// A capability may itself contain a dot (`clients.manage`), so the route
    /// cannot be split back into one - and a guest cannot be asked which it
    /// is, because believing that answer is the elevation the declared set
    /// exists to prevent. The host already knows, since it is the side that
    /// dispatched, and accepting exactly this operation is no elevation: the
    /// plugin was routed here for it in the first place.
    pub own_operation: String,
    /// Password-hash asks this dispatch has already made of the host.
    /// Bounded because each one is argon2's full baseline of host work,
    /// which the guest's own fuel does not pay for while it is running.
    pub password_hashes: u32,
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
    /// The stores a module guest's seam imports reach (ADR 0020), or all
    /// empty when this store was built for a manifest scan. Cloned in per
    /// call: two `Arc`s, and the read side of the import never touches
    /// anything behind them that isn't `Sync`.
    pub services: ModuleHostServices,
    /// Mutation deltas the guest recorded via `host.capability_write`
    /// (raw JSON; converted to [`MutationRecord`](kelson_execution::MutationRecord)
    /// by the handler after a successful call, discarded on guest failure).
    pub deltas: Vec<serde_json::Value>,
    /// Audit records the guest recorded via `host.capability_audit`
    /// (same lifecycle as `deltas`).
    pub audits: Vec<serde_json::Value>,
    /// Notifications the guest declared via `host.notify_enqueue` (ADR 0020
    /// §5). Same lifecycle as the two above: drained only after a successful
    /// call and dropped with the rest of the effects on a failed one, so a
    /// handler that notifies and then fails leaves nothing behind.
    pub notifications: Vec<kelson_module_contract::NotificationRequest>,
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

/// A generation's position in the staged activation lifecycle
/// `prepare → commit → drain` (ADR 0019 §6, sanctioned as an *extension of
/// v1* by root ADR 0016 §3 — so it is deliberately **not** gated behind the
/// `component-model` cargo feature).
///
/// The transitions are enforced by [`WasmPlugin::commit`] and
/// [`WasmPlugin::drain`], and [`WasmPlugin::call`] serves only a
/// [`Ready`](PluginState::Ready) generation. That last rule is what makes
/// **prepare** mean something: a plugin compiled, linked and drift-checked but
/// never committed cannot reach dispatch, so a prepare that fails partway
/// leaves the generation that is already serving untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PluginState {
    /// **prepare** — compiled, linked, ABI-checked and drift-checked, with
    /// **no** dispatch registration. Refuses every call.
    Prepared = 0,
    /// **commit** — the *host* registered the dispatch; this generation serves
    /// calls. The guest never registers itself, the same asymmetry as the
    /// sidecar-verified capability set.
    Ready = 1,
    /// **drain** — refusing new calls while the ones already inside retire.
    Draining = 2,
    /// No in-flight work remains; safe to drop.
    Drained = 3,
}

impl PluginState {
    /// Decode the discriminant kept in [`WasmPlugin`]'s state cell.
    ///
    /// Anything not one of the three known values decodes to
    /// [`Drained`](PluginState::Drained), the most conservative reading: an
    /// unrecognised state must refuse dispatch, never grant it. The cell is
    /// only ever written by [`WasmPlugin::commit`] and [`WasmPlugin::drain`],
    /// so this arm is unreachable today — it exists so a future state stays
    /// fail-closed rather than failing open.
    fn from_u8(v: u8) -> Self {
        match v {
            0 => PluginState::Prepared,
            1 => PluginState::Ready,
            2 => PluginState::Draining,
            _ => PluginState::Drained,
        }
    }
}

impl std::fmt::Display for PluginState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            PluginState::Prepared => "prepared",
            PluginState::Ready => "ready",
            PluginState::Draining => "draining",
            PluginState::Drained => "drained",
        };
        f.write_str(s)
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
    /// The wall-clock deadline (wasmtime epoch interruption) fired mid-call
    /// (mapped to `KSN_WASM_DEADLINE`). Distinct from [`Fuel`]: fuel bounds
    /// work, this bounds elapsed time, and a guest that blocks in a host call
    /// burns no fuel while still eating a worker.
    Deadline,
    /// The plugin's declared `budget.max_concurrency` is fully occupied, so
    /// the call was refused BEFORE a guest was instantiated (mapped to
    /// `KSN_WASM_SATURATED`).
    Saturated { limit: u32 },
    /// A runtime failure inside one `kelson_call`.
    Call(String),
    /// A staged-lifecycle transition was refused because the plugin is not in
    /// the state the operation requires: dispatching before **commit**,
    /// committing a generation that is already draining, draining one that was
    /// never committed, or a **drain** whose deadline passed with calls still
    /// in flight.
    ///
    /// This falls through `to_app_error`'s wildcard and renders as an internal
    /// error — the right verdict for the only case that can reach it today, a
    /// caller dispatching a generation it never committed, which is a host
    /// bug. The *draining* reading, which deserves HTTP 503, is unreachable
    /// until a generation-swap path exists; its mapping is sequenced with that
    /// path rather than added now, when no code could produce it.
    Lifecycle(String),
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
            WasmError::Deadline => write!(f, "wasm plugin exceeded its wall-clock deadline"),
            WasmError::Saturated { limit } => write!(
                f,
                "wasm plugin concurrency limit of {limit} in-flight calls is saturated"
            ),
            WasmError::Call(m) => write!(f, "wasm plugin call failed: {m}"),
            WasmError::Lifecycle(m) => write!(f, "wasm plugin lifecycle refused: {m}"),
        }
    }
}

impl std::error::Error for WasmError {}

/// Unpack a guest-returned `ptr << 32 | len` pair.
fn unpack(packed: i64) -> (u32, u32) {
    let bits = packed as u64;
    ((bits >> 32) as u32, bits as u32)
}

/// Pack a `(ptr, len)` pair the way a guest reads it back: `ptr << 32 | len`.
///
/// The mirror of [`unpack`], needed now that the host is the side writing
/// into guest memory — a seam import allocates a buffer *inside* the guest
/// and fills it, so the answer travels in the same encoding a guest's own
/// `kelson_manifest` return uses. Masked rather than assumed: both halves
/// are 32-bit by construction, and shifting an unmasked `usize` would let a
/// bogus pointer bleed into the length.
fn pack(ptr: usize, len: usize) -> i64 {
    (((ptr as u64 & 0xFFFF_FFFF) << 32) | (len as u64 & 0xFFFF_FFFF)) as i64
}

/// Classify a failed guest call: fuel exhaustion and deadline expiry are each
/// their own category (both become pinned 503s), everything else is an
/// ordinary call failure.
///
/// [`Trap::Interrupt`] is the deadline, not a user interrupt: it is what
/// wasmtime returns when the store's epoch deadline has passed and no
/// [`Store::epoch_deadline_callback`] was installed (`store.rs` maps the
/// default `None` behavior to exactly this trap). [`Trap::OutOfFuel`] is its
/// twin — the same guest loop, stopped by a different meter.
fn call_error(op: &str, e: wasmtime::Error) -> WasmError {
    match e.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => WasmError::Fuel,
        Some(Trap::Interrupt) => WasmError::Deadline,
        Some(trap) => WasmError::Call(format!("{op}: trap {trap}")),
        None => WasmError::Call(format!("{op}: {e}")),
    }
}

/// Releases a concurrency slot taken by [`WasmPlugin::call`] when it drops.
///
/// The slot is acquired before the guest is instantiated and released on
/// every exit — success, trap, or the early `?` returns that follow — because
/// a slot leaked by a failing call would count against the cap forever and
/// could take the plugin out of service one failed request at a time.
struct InFlightSlot<'a>(&'a std::sync::atomic::AtomicU32);

impl Drop for InFlightSlot<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
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

/// A background thread advancing the wasmtime epoch, which is what makes a
/// wall-clock deadline possible at all: the guest is never preempted from
/// outside, it checks the epoch at its own loop back-edges, so a deadline can
/// only interrupt a guest that is actually executing.
///
/// One ticker serves every plugin loaded through its [`WasmHost`], and is
/// created on demand — only when some plugin declares `execution_timeout_ms`.
/// A workspace whose plugins are all fuel-bounded never starts a thread.
///
/// ## Why this counts a clock instead of sleep calls
///
/// The epoch must advance once per millisecond for `execution_timeout_ms` to
/// mean milliseconds. Counting "one tick per `sleep(1ms)`" does NOT do that:
/// `sleep` sets a *lower* bound, and on Windows the default timer resolution
/// makes a 1ms sleep land near 15.6ms — so a declared 50ms deadline would
/// fire around 780ms, fifteen times late, and by a factor that varies per
/// machine. Instead the loop reads a monotonic clock and catches the epoch up
/// by however many milliseconds have *actually* elapsed, so time accrues at
/// the right rate on every platform. The sleep only bounds how often the
/// clock is consulted, which means a deadline can still fire up to one sleep
/// interval (plus OS timer slop) after it was asked for — it can run late,
/// never early, and never by a multiple of the requested budget.
///
/// Stopped by [`Drop`]: the flag is set and the thread joined, so a host that
/// goes out of scope leaves no spinning thread behind. That join costs up to
/// one sleep interval, which is why hosts that never declare a deadline do
/// not pay for a ticker at all.
struct EpochTicker {
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl EpochTicker {
    fn start(engine: Engine) -> Result<Self, WasmError> {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("wasm-epoch-ticker".into())
            .spawn(move || {
                let started = std::time::Instant::now();
                let mut advanced_ms: u128 = 0;
                while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                    let elapsed_ms = started.elapsed().as_millis();
                    // Catch up by real elapsed time, not by iteration count:
                    // after a long sleep we owe several ticks, not one.
                    while advanced_ms < elapsed_ms {
                        engine.increment_epoch();
                        advanced_ms += 1;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
            })
            .map_err(|e| WasmError::Load(format!("epoch ticker thread: {e}")))?;
        Ok(Self {
            stop,
            handle: Some(handle),
        })
    }
}

/// Deadline ticks for a call that declares no `execution_timeout_ms`.
///
/// [`wasmtime::Store::set_epoch_deadline`] **must** be called on every store
/// once `Config::epoch_interruption(true)` is set: the store's deadline starts
/// at `0`, which wasmtime documents as having "always elapsed", so a store left
/// at the default traps the guest the instant it runs. Plugins without a
/// wall-clock budget therefore get a deadline so far out it can never be
/// reached — `u64::MAX` is deliberately not used, because the deadline is
/// computed as `current_epoch + ticks` and would overflow it.
pub const NO_DEADLINE_TICKS: u64 = u64::MAX / 2;

impl Drop for EpochTicker {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            // A ticker parked in `sleep` wakes within one tick, so the join
            // bounds this at ~1ms rather than leaking a thread per host.
            let _ = handle.join();
        }
    }
}

/// Which guest ABI generation an artifact implements.
///
/// Decided by the artifact, never by anything it declares: ADR 0019 §2 is
/// explicit that "the artifact shape decides; nothing is inferred from a
/// manifest field that could drift". The two generations coexist through a
/// transition window, and neither is ever recompiled, re-instantiated or
/// reinterpreted as the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuestAbi {
    /// ABI v1 — a core module exporting `kelson_manifest`, `kelson_alloc` and
    /// `kelson_call`. The contract every module that ships today implements.
    V1,
    /// ABI v2 — a Component Model component exporting the
    /// `kelson:plugin/dispatch` world (`wit/kelson-plugin.wit`).
    V2,
}

impl GuestAbi {
    /// How this generation is spelled in the `[[wasm]]` sidecar's optional
    /// `abi` key. Absent means [`GuestAbi::V1`], so every shipped fixture
    /// keeps working untouched (ADR 0019 §2).
    pub fn as_declared(self) -> u8 {
        match self {
            GuestAbi::V1 => 1,
            GuestAbi::V2 => 2,
        }
    }
}

/// Measure an artifact's ABI generation, or report that it is not a container
/// this detector can classify.
///
/// **Classification only, never validation.** Both containers lead with the
/// same four magic bytes and are told apart by a 4-byte layer/version word:
/// `01 00 00 00` is a core module, `0d 00 01 00` a Component Model component.
/// That word is part of the *container* header rather than the payload, so a
/// guest cannot choose it and it cannot drift — which is what makes it a
/// sounder signal than a manifest field (ADR 0019 §2: "the artifact shape
/// decides; nothing is inferred from a manifest field that could drift").
///
/// Two encodings reach this, because `Module::new` and `Component::new` both
/// take WAT text as well as binary — and every checked-in fixture is `.wat`.
/// Binary input is read from its header; text input from its first token,
/// which the text format makes the container itself.
///
/// `None` is a deliberate answer for anything it does not recognize (a native
/// image, an empty file, a truncated header). Inventing a verdict about a
/// malformed image would mean competing with wasmtime's validator, so the
/// caller hands those to `Module::new` and takes its `compile:` refusal as
/// the authoritative one. The *exports* are likewise the load path's business
/// — v1 needs `kelson_manifest`/`kelson_alloc`/`kelson_call`, v2 needs
/// `kelson:plugin/dispatch` — so this never has to guess what is inside.
fn detect_abi(bytes: &[u8]) -> Option<GuestAbi> {
    const MAGIC: [u8; 4] = [0x00, b'a', b's', b'm'];
    /// `01 00 00 00` — the core WebAssembly module version.
    const CORE_MODULE: u32 = 0x0000_0001;
    /// `0d 00 01 00` — component layer `0x000d`, component version `0x0001`.
    const COMPONENT: u32 = 0x0001_000d;

    if bytes.starts_with(&MAGIC) {
        if bytes.len() < 8 {
            return None;
        }
        let layer = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        return match layer {
            CORE_MODULE => Some(GuestAbi::V1),
            COMPONENT => Some(GuestAbi::V2),
            // A container header we do not know: defer to the validator
            // rather than claim to have a better opinion about it.
            _ => None,
        };
    }
    detect_wat_container(bytes)
}

/// Classify WAT **text** by its first token, which the text format makes the
/// container: `(module ...)` or `(component ...)`.
fn detect_wat_container(bytes: &[u8]) -> Option<GuestAbi> {
    let mut i = wat_trivia_end(bytes);
    if bytes.get(i)? != &b'(' {
        return None;
    }
    i += 1;
    while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
        i += 1;
    }
    let start = i;
    while bytes
        .get(i)
        .is_some_and(|c| c.is_ascii_alphanumeric() || matches!(*c, b'_' | b'-' | b'.' | b'$'))
    {
        i += 1;
    }
    let token = bytes.get(start..i)?;
    if token == &b"module"[..] {
        Some(GuestAbi::V1)
    } else if token == &b"component"[..] {
        Some(GuestAbi::V2)
    } else {
        None
    }
}

/// Advance past WAT leading trivia — whitespace, `;;` line comments and
/// `(; ;)` block comments, which nest per the text format — and return where
/// the first real token starts.
///
/// Every checked-in fixture opens with `;;` banner comments, so skipping them
/// is not an edge case: without it every fixture would fail to classify.
fn wat_trivia_end(bytes: &[u8]) -> usize {
    let mut i = 0;
    loop {
        while bytes.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if bytes[i..].starts_with(b";;") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i..].starts_with(b"(;") {
            let mut depth = 1usize;
            i += 2;
            while i < bytes.len() && depth > 0 {
                if bytes[i..].starts_with(b"(;") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b";)") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        return i;
    }
}

/// Refuse an artifact whose container does not match what the reviewed
/// sidecar declared (ADR 0019 §2's fail-closed review gate).
///
/// This runs before a byte is compiled. Absent `abi` means v1, so a component
/// handed to a host that was never told to expect one is a refusal rather
/// than a silent upgrade — the "no silent swap" half of the migration shape.
fn check_declared_abi(sidecar: &ModuleManifest, actual: GuestAbi) -> Result<(), WasmError> {
    let declared = sidecar.abi.unwrap_or(GuestAbi::V1.as_declared());
    if declared == actual.as_declared() {
        return Ok(());
    }
    // Name what the sidecar actually said. Reporting a bare "declares abi = 1"
    // for a sidecar that declared nothing at all would send an operator looking
    // for a key that was never there.
    let stated = match sidecar.abi {
        Some(v) => format!("declares abi = {v}"),
        None => "has no `abi` key, so it declares abi = 1 (absent means v1)".to_string(),
    };
    Err(WasmError::Load(format!(
        "guest ABI mismatch: the [[wasm]] sidecar {stated} but the artifact is a {} \
         (guest ABI v{}); review must state the generation it ships",
        match actual {
            GuestAbi::V1 => "core module",
            GuestAbi::V2 => "Component Model component",
        },
        actual.as_declared(),
    )))
}

/// The wasmtime side of the runtime: one engine + linker (the import
/// surface) shared by every plugin loaded through it.
pub struct WasmHost {
    engine: Engine,
    linker: Linker<CallState>,
    fuel: u64,
    memory_bytes: usize,
    /// Created on demand, not eagerly: a workspace whose plugins are all
    /// fuel-bounded never starts a thread. A plugin that declares
    /// `execution_timeout_ms` takes the first [`Arc`] out of this cell and
    /// keeps it, so the ticker outlives the host — `load_wasm_plugins` drops
    /// its host before the handlers are ever dispatched.
    ticker: std::sync::Mutex<Option<Arc<EpochTicker>>>,
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
        // Wave M2: epoch-based interruption, so a `[budget] execution_timeout_ms`
        // can bound WALL-CLOCK time. Fuel bounds work done; this bounds time
        // spent, which matters because the two are not interchangeable: fuel
        // does not advance while a guest sits in a host call, and how much
        // fuel equals "one millisecond" varies with the work being done.
        //
        // Enabled for every host, even one whose plugins declare no deadline,
        // because the engine's config is fixed at construction and a plugin
        // may only declare its budget later. The cost is borne only by stores
        // that opt in: a plugin with no `execution_timeout_ms` is given
        // [`NO_DEADLINE_TICKS`], a deadline so distant it cannot be reached.
        // Both APIs are available on this feature set — they are gated on
        // `target_has_atomic = "64"`, not on a cargo feature — which every
        // target this workspace builds for satisfies.
        config.epoch_interruption(true);
        let engine = Engine::new(&config).map_err(|e| WasmError::Load(format!("engine: {e}")))?;
        let mut linker: Linker<CallState> = Linker::new(&engine);
        imports::define(&mut linker)?;
        Ok(Self {
            engine,
            linker,
            fuel,
            memory_bytes,
            ticker: std::sync::Mutex::new(None),
        })
    }

    /// The host's epoch ticker, starting it on first use.
    ///
    /// Returns `None` only if the thread could not be spawned, so the caller
    /// can refuse the load rather than silently serving an unenforceable
    /// deadline — a budget that cannot be enforced is worse than none, since
    /// it invites an operator to believe a wall-clock bound exists.
    fn ticker(&self) -> Option<Arc<EpochTicker>> {
        let mut slot = self.ticker.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            match EpochTicker::start(self.engine.clone()) {
                Ok(t) => *slot = Some(Arc::new(t)),
                Err(_) => return None,
            }
        }
        slot.as_ref().map(Arc::clone)
    }

    /// Resolve a manifest `[budget]` against this host's ceilings.
    ///
    /// Returns every violation at once so an operator fixes their sidecar in
    /// one pass, and computes the effective numbers only when the budget is
    /// legal. Tighten-only is enforced here rather than at each call site: a
    /// budget that could raise a host ceiling would let a third-party module
    /// widen the sandbox it was reviewed under, which is the whole reason the
    /// field is a floor-and-not-a-negotiation.
    fn resolve_budget(&self, budget: &ModuleBudget) -> Result<CallBudget, WasmError> {
        budget
            .validate_against(self.memory_bytes, self.fuel)
            .map_err(|violations| {
                WasmError::Manifest(format!("budget is not tighten-only: {}", violations.join("; ")))
            })?;
        Ok(CallBudget {
            fuel: budget.effective_fuel(self.fuel),
            memory_bytes: budget.effective_memory_bytes(self.memory_bytes),
            deadline_ticks: budget
                .execution_timeout_ms
                .map_or(NO_DEADLINE_TICKS, |ms| ms.max(1)),
            max_concurrency: budget.max_concurrency,
        })
    }

    /// Build a store for one call or one manifest scan.
    ///
    /// `budget` supplies fuel, memory and the epoch deadline together, so the
    /// three can never disagree about which sandbox a guest runs in. The
    /// deadline is set on every store, not only on deadline-declaring ones:
    /// with `epoch_interruption(true)` wasmtime's default deadline is `0`,
    /// which it documents as having "always elapsed", so a store that skipped
    /// this call would trap its guest before it ran a single instruction.
    ///
    /// It takes an `engine` rather than `&self` because its two callers do not
    /// share a type: `prepare` runs on the host, while [`WasmPlugin::call`]
    /// runs on the plugin that host produced. Being an associated function is
    /// what lets the manifest scan and the dispatch path build the *same*
    /// store from the *same* code — otherwise the two could drift into
    /// applying different sandboxes to one plugin and nothing would report it.
    // Kept flat deliberately: the two callers must build byte-identical
    // sandboxes, and an argument struct would let one of them start leaving
    // fields defaulted. The doc on `Self::prepare` explains why that matters.
    #[allow(clippy::too_many_arguments)]
    fn new_store(
        engine: &Engine,
        label: &str,
        declared: Arc<BTreeSet<String>>,
        scope_paths: Vec<String>,
        has_scope: bool,
        profile: WasmCapabilityProfile,
        services: ModuleHostServices,
        budget: &CallBudget,
    ) -> Result<Store<CallState>, WasmError> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(budget.memory_bytes)
            .build();
        let mut store = Store::new(
            engine,
            CallState {
                limits,
                label: label.to_string(),
                declared,
                own_operation: String::new(),
                password_hashes: 0,
                scope_paths,
                has_scope,
                profile,
                services,
                deltas: Vec::new(),
                audits: Vec::new(),
                notifications: Vec::new(),
            },
        );
        store.limiter(|state| &mut state.limits);
        store
            .set_fuel(budget.fuel)
            .map_err(|e| WasmError::Call(format!("set fuel: {e}")))?;
        store.set_epoch_deadline(budget.deadline_ticks);
        Ok(store)
    }

    /// **prepare** — compile, link, validate the ABI and cross-check the guest
    /// manifest against the reviewed sidecar, then STOP (ADR 0019 §6).
    ///
    /// Nothing is registered and the returned plugin refuses every
    /// [`WasmPlugin::call`], so this phase's failures leave the generation that
    /// is already serving untouched. Every failure here is still a boot refusal
    /// (fail-closed): a plugin never reaches dispatch in a half-trusted state.
    ///
    /// The host promotes the result with [`WasmPlugin::commit`].
    pub fn prepare(
        &self,
        bytes: &[u8],
        sidecar: &ModuleManifest,
    ) -> Result<WasmPlugin, WasmError> {
        // Wave M2: resolve the manifest `[budget]` FIRST, before a byte is
        // compiled or instantiated. A budget that asks for more than the host
        // allows is a refusal with no guest involvement at all, so an
        // over-declared module is rejected by reading its sidecar rather than
        // by running it and observing the result.
        let budget = self.resolve_budget(&sidecar.budget)?;
        // A wall-clock budget is unenforceable without a ticker to advance the
        // epoch. Refuse rather than load a plugin whose declared deadline
        // would silently never fire — a bound that is documented but inert is
        // worse than no bound, because operators will size their fleet to it.
        let ticker = if budget.has_deadline() {
            Some(self.ticker().ok_or_else(|| {
                WasmError::Load(
                    "budget.execution_timeout_ms is declared but the epoch ticker thread \
                     could not be started, so the deadline could not be enforced"
                        .to_string(),
                )
            })?)
        } else {
            None
        };

        // ADR 0019 §2: detection is structural, and the reviewed sidecar's
        // optional `abi` key is a fail-closed gate ON TOP of it. Both run
        // before a byte is compiled, so an artifact whose generation does not
        // match its review is refused without ever being instantiated — the
        // "no silent swap" half of the migration shape.
        //
        // `None` (an image this cannot classify) skips the gate rather than
        // failing it: there is nothing to compare the declaration against
        // yet. It still cannot slip past, because `None` routes to the v1
        // path where `Module::new` refuses a malformed image outright.
        let abi = detect_abi(bytes);
        if let Some(abi) = abi {
            check_declared_abi(sidecar, abi)?;
        }

        let guest = match abi.unwrap_or(GuestAbi::V1) {
            GuestAbi::V1 => {
                let module = Module::new(&self.engine, bytes)
                    .map_err(|e| WasmError::Load(format!("compile: {e}")))?;
                // Link-time import validation: anything the shim does not define
                // (e.g. a `sock_*` call) fails HERE, not at first dispatch.
                let pre = self
                    .linker
                    .instantiate_pre(&module)
                    .map_err(|e| WasmError::Load(format!("link: {e}")))?;
                Guest::V1(pre)
            }
            GuestAbi::V2 => self.prepare_v2(bytes)?,
        };

        // Instantiate once and read the guest's self-declared manifest. This
        // scan runs under the plugin's own budget, not the host ceiling: a
        // guest whose manifest export loops forever is bounded here too,
        // otherwise the boot path would be the one place a guest could run
        // unbounded.
        let json = {
            let none_declared = Arc::new(BTreeSet::new());
            // The manifest scan runs under the most restrictive profile:
            // a guest that calls the write imports from kelson_manifest()
            // records into buffers `load` never reads.
            let mut store = Self::new_store(
                &self.engine,
                sidecar.name(),
                none_declared,
                Vec::new(),
                false,
                WasmCapabilityProfile::ReadOnly,
                // No services for the manifest scan: this store exists to
                // read one export's bytes, and a guest that reached for a
                // model from `kelson_manifest()` would be refused rather
                // than handed a store it was never reviewed to use.
                ModuleHostServices::default(),
                &budget,
            )?;
            match &guest {
                Guest::V1(pre) => {
                    // Instantiating resolves all three v1 exports, so a module
                    // missing any of them refuses HERE rather than at first
                    // dispatch.
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
                        budget.memory_bytes,
                    )?;
                    String::from_utf8(raw)
                        .map_err(|e| WasmError::Manifest(format!("kelson_manifest is not UTF-8: {e}")))?
                }
                // Symmetric with the arm above: `PluginPre::instantiate`
                // type-checks the world's exports through `indices.load`, so a
                // component missing `manifest` or `call` refuses at load. The
                // world's `manifest()` is then the drift source, exactly as
                // `kelson_manifest()` is for v1.
                #[cfg(feature = "component-model")]
                Guest::V2(pre) => {
                    let plugin = pre
                        .instantiate(&mut store)
                        .map_err(|e| WasmError::Load(format!("instantiate component: {e}")))?;
                    let dispatch = plugin.kelson_plugin_dispatch();
                    dispatch
                        .call_manifest(&mut store)
                        .map_err(|e| WasmError::Manifest(format!("dispatch.manifest(): {e}")))?
                }
            }
        };

        let parsed = ModuleManifest::from_json(&json).map_err(|v| WasmError::Manifest(v.message))?;
        // `[budget]` is HOST policy, never guest state: a guest has no way to
        // declare it and no reason to. Comparing it here would refuse every
        // budgeted plugin as "drift", so both documents are normalized with
        // the budget cleared before the comparison. Everything else —
        // name, version, capabilities, ABI — still compares byte for byte.
        let mut plugin_cmp = parsed.clone();
        plugin_cmp.budget = kelson_registry::ModuleBudget::default();
        // `abi` is cleared for exactly the reason `budget` is: detection
        // measured the artifact above and never read this field, so a field
        // detection ignores must not be a field that can fail the comparison.
        // Declaring it in the guest manifest therefore neither helps nor
        // hurts — the sidecar's copy is the review gate, checked separately.
        plugin_cmp.abi = None;
        // Host-side review state, not guest-declared (see the field's own
        // note); the drift check compares what the guest committed to, and a
        // guest cannot commit to whether its host wraps its payload.
        plugin_cmp.envelope_context = false;
        let mut sidecar_cmp = sidecar.clone();
        sidecar_cmp.budget = kelson_registry::ModuleBudget::default();
        sidecar_cmp.abi = None;
        sidecar_cmp.envelope_context = false;
        let plugin_ser = serde_json::to_string(&plugin_cmp)
            .map_err(|e| WasmError::Manifest(format!("manifest re-encode failed: {e}")))?;
        let sidecar_ser = serde_json::to_string(&sidecar_cmp)
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
        // Drift has already been refused above, so the two documents agree on
        // everything a guest CAN declare. What is left to take from the
        // sidecar is the host-side review state a guest cannot know about —
        // how this generation is dispatched, which is what the review
        // approved when it read the handlers.
        let mut manifest = parsed;
        manifest.envelope_context = sidecar.envelope_context;
        Ok(WasmPlugin {
            engine: self.engine.clone(),
            guest,
            manifest,
            declared,
            label,
            budget,
            // Held only when a deadline is enforced: a fuel-bounded plugin
            // keeps no thread alive, so the common case pays nothing for the
            // machinery.
            ticker,
            in_flight: std::sync::atomic::AtomicU32::new(0),
            // Empty until the boot path installs the host's stores; a call
            // that reaches dispatch before then is told the seam has no
            // store rather than handed rows nobody wired.
            services: std::sync::OnceLock::new(),
            // prepare's product: validated but uncommitted, so it cannot
            // dispatch until the host says so.
            state: std::sync::atomic::AtomicU8::new(PluginState::Prepared as u8),
        })
    }

    /// **prepare + commit** in one step — the simple path for a caller that is
    /// about to dispatch this generation immediately.
    ///
    /// Equivalent to [`Self::prepare`] followed by [`WasmPlugin::commit`], and
    /// both halves are literal: the plugin is fully validated *and* marked
    /// [`PluginState::Ready`]. The staged boot path
    /// (`kelson_demo::load_wasm_plugins`) uses the two calls separately so the
    /// host's decision to register stays an explicit, separate act — that
    /// separation is the whole point of ADR 0019 §6, and it is not lost here,
    /// because this method registers nothing either.
    pub fn load(&self, bytes: &[u8], sidecar: &ModuleManifest) -> Result<WasmPlugin, WasmError> {
        let plugin = self.prepare(bytes, sidecar)?;
        plugin.commit()?;
        Ok(plugin)
    }

    /// Compile and link an ABI v2 component against the generated
    /// `kelson:plugin/host` bindings.
    ///
    /// The component linker is built here rather than held by [`WasmHost`]
    /// because it only exists for components: a host whose plugins are all v1
    /// modules never constructs one, and the feature-off build gets a refusal
    /// (below) instead of a linker over a module that does not exist.
    #[cfg(feature = "component-model")]
    fn prepare_v2(&self, bytes: &[u8]) -> Result<Guest, WasmError> {
        use wasmtime::component::{Component, HasSelf, Linker};

        let component = Component::new(&self.engine, bytes)
            .map_err(|e| WasmError::Load(format!("compile component: {e}")))?;
        let mut linker = Linker::new(&self.engine);
        // `HasSelf` says the linker's store data IS the host implementation,
        // so the four imports resolve straight onto `CallState` — the same
        // methods the v1 `host.*` registrations call (see `imports.rs`). One
        // host, two front doors, one set of verdicts.
        crate::component::Plugin::add_to_linker::<CallState, HasSelf<CallState>>(
            &mut linker,
            |state| state,
        )
        .map_err(|e| WasmError::Load(format!("link kelson:plugin/host: {e}")))?;
        let pre = linker
            .instantiate_pre(&component)
            .map_err(|e| WasmError::Load(format!("link component: {e}")))?;
        // The structural half of ADR 0019 §2's v2 detection: this fails
        // unless the component exports `kelson:plugin/dispatch`.
        let pre = crate::component::PluginPre::new(pre).map_err(|e| {
            WasmError::Load(format!(
                "component does not export the kelson:plugin/dispatch world: {e}"
            ))
        })?;
        Ok(Guest::V2(pre))
    }

    /// The feature-off twin of [`Self::prepare_v2`].
    ///
    /// A v2 artifact reaching a host built without `component-model` is
    /// refused with a message naming the missing half, rather than being
    /// recompiled as a module or quietly ignored. That is the fail-closed
    /// posture behind ADR 0019 §4 keeping the feature off by default: an
    /// artifact the host cannot serve is a boot refusal, never a downgrade.
    #[cfg(not(feature = "component-model"))]
    fn prepare_v2(&self, _bytes: &[u8]) -> Result<Guest, WasmError> {
        Err(WasmError::Load(
            "artifact is a Component Model component (guest ABI v2) but this host was \
             built without the `component-model` cargo feature; refusing it rather than \
             reinterpreting it as a v1 module"
                .to_string(),
        ))
    }
}

/// One generation's compiled guest.
///
/// Exactly one of the two ABI generations is held, and the variant is decided
/// by [`detect_abi`] from the artifact itself — a v1 module is never
/// re-instantiated as v2 nor a v2 component run through the v1 path
/// (ADR 0019 §2's "no silent swap").
enum Guest {
    /// ABI v1 core module, pre-linked against the `host` / WASI import
    /// surface registered by [`imports::define`].
    V1(InstancePre<CallState>),
    /// ABI v2 Component Model component, pre-linked against the generated
    /// `kelson:plugin/host` bindings and pre-checked for the
    /// `kelson:plugin/dispatch` export.
    #[cfg(feature = "component-model")]
    V2(crate::component::PluginPre<CallState>),
}

/// A loaded, drift-checked plugin: instantiate-per-call, budget-metered.
pub struct WasmPlugin {
    engine: Engine,
    guest: Guest,
    manifest: ModuleManifest,
    declared: Arc<BTreeSet<String>>,
    label: String,
    /// The manifest's `[budget]` resolved against the host. Fixed for the
    /// plugin's lifetime, so every call runs in the same sandbox.
    budget: CallBudget,
    /// Keeps the epoch ticker alive while this plugin can still run a guest.
    /// The host that created the plugin is dropped at the end of
    /// `load_wasm_plugins`, long before the handlers are dispatched, so a
    /// plugin that did not hold its own reference would see its deadline
    /// silently stop firing.
    ticker: Option<Arc<EpochTicker>>,
    /// Concurrent calls in flight, against [`CallBudget::max_concurrency`].
    in_flight: std::sync::atomic::AtomicU32,
    /// Host services the seam imports reach (ADR 0020).
    ///
    /// Installed once, *after* `prepare` and before the bundle is published:
    /// the boot path loads plugins before `host.boot()`, and the stores those
    /// plugins will read and write are created by boot, so the ordering is
    /// fixed and no other arrangement works. A cell rather than a field for
    /// that reason alone — the value still only ever goes in once.
    services: std::sync::OnceLock<ModuleHostServices>,
    /// Position in the `prepare → commit → drain` lifecycle (ADR 0019 §6),
    /// encoded as a [`PluginState`] discriminant so it can be read with one
    /// relaxed-free atomic load on the dispatch path.
    state: std::sync::atomic::AtomicU8,
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
    /// The notifications the guest declared, in order, drained only once the
    /// call succeeded.
    pub notifications: Vec<kelson_module_contract::NotificationRequest>,
}

impl WasmPlugin {
    /// The guest manifest, exactly as validated against the sidecar.
    pub fn manifest(&self) -> &ModuleManifest {
        &self.manifest
    }

    /// Which ABI generation this plugin is — v1 core module or v2 component.
    ///
    /// Exposed so a caller (and the upgrade proof) can assert *which*
    /// generation is serving without inferring it from behaviour.
    pub fn abi(&self) -> GuestAbi {
        match &self.guest {
            Guest::V1(_) => GuestAbi::V1,
            #[cfg(feature = "component-model")]
            Guest::V2(_) => GuestAbi::V2,
        }
    }

    /// The sandbox this plugin runs in, resolved from the manifest `[budget]`
    /// against the host at load and fixed for the plugin's lifetime.
    pub fn budget(&self) -> CallBudget {
        self.budget
    }

    /// The services this generation's seam imports may reach.
    ///
    /// Empty before [`Self::install_services`] runs, which is the right
    /// answer rather than a placeholder: a call that got that far without
    /// services hears "no store is configured" from the seam instead of
    /// reading an empty one.
    pub fn services(&self) -> ModuleHostServices {
        self.services.get().cloned().unwrap_or_default()
    }

    /// Wire this generation's seam imports to the host's stores.
    ///
    /// Called once by the boot path, after boot has created those stores and
    /// before the bundle is published. A second install is refused rather
    /// than silently swapping the stores a generation serves with: which
    /// rows a plugin reads is a property of the generation under review, not
    /// a per-call choice.
    pub fn install_services(&self, services: ModuleHostServices) -> Result<(), WasmError> {
        self.services.set(services).map_err(|_| {
            WasmError::Lifecycle(format!(
                "plugin {} already has its module host services installed",
                self.manifest.name()
            ))
        })
    }

    /// Whether this plugin's `execution_timeout_ms` is actually enforceable —
    /// that is, whether the epoch ticker the deadline depends on exists and is
    /// held by this plugin.
    ///
    /// Exposed rather than assumed, because the two halves are set in
    /// different places (the budget is read from the sidecar, the ticker is
    /// started from the host) and a mismatch would mean a deadline that is
    /// declared, documented, and silently inert. Callers that surface this
    /// should treat `false` next to [`CallBudget::has_deadline`] returning
    /// `true` as a fault, not a configuration.
    pub fn deadline_enforced(&self) -> bool {
        debug_assert_eq!(
            self.ticker.is_some(),
            self.budget.has_deadline(),
            "a declared execution_timeout_ms must be paired with a live epoch ticker"
        );
        self.ticker.is_some()
    }

    /// Calls currently executing inside this plugin (driven by
    /// [`Self::budget`]'s `max_concurrency`, but counted whether or not a cap
    /// is declared, so it stays meaningful as a gauge).
    pub fn in_flight(&self) -> u32 {
        self.in_flight.load(std::sync::atomic::Ordering::Acquire)
    }

    /// This generation's position in the `prepare → commit → drain` lifecycle.
    pub fn state(&self) -> PluginState {
        PluginState::from_u8(self.state.load(std::sync::atomic::Ordering::Acquire))
    }

    /// **commit** — the host registers this generation for dispatch
    /// (ADR 0019 §6: the guest never registers itself).
    ///
    /// The only promotion is `prepared → ready`, and it is idempotent once
    /// there: committing the same generation twice describes one host decision,
    /// not two. What is refused is any move out of a retire — committing a
    /// [`PluginState::Draining`] or [`PluginState::Drained`] generation fails,
    /// so a swap can never resurrect the generation it just drained.
    pub fn commit(&self) -> Result<(), WasmError> {
        use std::sync::atomic::Ordering;
        match self.state.compare_exchange(
            PluginState::Prepared as u8,
            PluginState::Ready as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            // Common path: this call is the commit.
            Ok(_) => Ok(()),
            // CAS lost. Either another committer just did the same thing
            // (idempotent -> OK) or the generation has been retired (never OK).
            Err(_) => match self.state() {
                PluginState::Ready => Ok(()),
                other => Err(WasmError::Lifecycle(format!(
                    "cannot commit a plugin in state {other}"
                ))),
            },
        }
    }

    /// **drain** — retire this generation: refuse new calls, then wait for the
    /// ones already inside to finish (ADR 0019 §6).
    ///
    /// Returns once nothing is in flight, or with a [`WasmError::Lifecycle`]
    /// if `timeout` expires first — naming how many calls were still running
    /// rather than reporting success it did not observe. Draining a generation
    /// that was never committed is refused: it accepted no work, so the request
    /// means the caller has lost track of which generation it holds.
    ///
    /// Idempotent; a second caller joins the same wait.
    pub fn drain(&self, timeout: std::time::Duration) -> Result<(), WasmError> {
        use std::sync::atomic::Ordering;
        loop {
            let from = self.state.load(Ordering::Acquire);
            if from == PluginState::Drained as u8 {
                return Ok(());
            }
            if from == PluginState::Prepared as u8 {
                return Err(WasmError::Lifecycle(
                    "cannot drain a plugin that was never committed (state: prepared)"
                        .to_string(),
                ));
            }
            // Claim the retire. On failure re-read: `ready -> draining` is the
            // real transition, `draining -> draining` is a second joiner, and
            // `drained` is handled above.
            let claimed = self.state.compare_exchange(
                from,
                PluginState::Draining as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            if claimed.is_ok() {
                break;
            }
        }

        // New dispatch is refused from here, BEFORE the count is read, so no
        // call can begin after the moment observation starts. The other half is
        // closed by `call` reading this state only *after* incrementing
        // `in_flight`: either that call sees `draining` and leaves, or we see
        // its count and wait for it. Neither can slip past both checks.
        let deadline = std::time::Instant::now() + timeout;
        loop {
            let running = self.in_flight.load(Ordering::Acquire);
            if running == 0 {
                let _ = self.state.compare_exchange(
                    PluginState::Draining as u8,
                    PluginState::Drained as u8,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(WasmError::Lifecycle(format!(
                    "drain timed out after {}ms with {running} call(s) still in flight",
                    timeout.as_millis()
                )));
            }
            // Poll rather than block: `in_flight` is a bare atomic with no
            // wait queue, and the calls being waited on are bounded by fuel and
            // the epoch deadline — so this spans one guest invocation, not an
            // unbounded wait. 1ms is finer than any realistic call and far
            // coarser than the instantiation cost that dominates it.
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// Invoke one operation: fresh instance, budgeted fuel/memory/deadline,
    /// JSON in/out.
    /// `scope` feeds `host.capability_check`'s path verdict; `profile`
    /// gates the write imports for this call.
    pub fn call(
        &self,
        operation: &str,
        input_json: &str,
        scope: Option<&ScopeSnapshot>,
        profile: WasmCapabilityProfile,
    ) -> Result<CallOutput, WasmError> {
        // Wave M2: take a concurrency slot BEFORE instantiating. Refusing here
        // means a saturated plugin does no work at all — no store, no compile,
        // no guest — so the cap sheds load instead of queueing it. The counter
        // is incremented on every call, capped or not, so [`Self::in_flight`]
        // reports the truth for an uncapped plugin too; only the *refusal* is
        // conditional on the cap. The guard releases on every exit, including
        // the early `?` returns below, so a failed call cannot leak its slot
        // and wedge the plugin shut.
        let _slot = {
            let prev = self
                .in_flight
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            // ADR 0019 §6 — only a committed generation dispatches. The read
            // deliberately comes AFTER the increment: a call that read `ready`
            // first and incremented second could land in a generation `drain`
            // had already declared idle, and would run guest code against a
            // retired plugin. Reading second makes the pair airtight — either
            // this call sees the retire state and leaves (decrementing here),
            // or `drain` sees this call's count and waits for it. Neither can
            // slip past both checks.
            let state = self.state();
            if state != PluginState::Ready {
                self.in_flight
                    .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                return Err(WasmError::Lifecycle(format!(
                    "plugin dispatch refused: generation is {state}; only a ready generation serves calls"
                )));
            }
            match self.budget.max_concurrency {
                Some(limit) if prev >= limit => {
                    self.in_flight.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
                    return Err(WasmError::Saturated { limit });
                }
                _ => Some(InFlightSlot(&self.in_flight)),
            }
        };

        let (scope_paths, has_scope) = match scope {
            Some(s) => (s.nodes.iter().map(|n| n.path.clone()).collect(), true),
            None => (Vec::new(), false),
        };
        let budget = self.budget;
        // The same constructor `prepare`'s manifest scan uses, so the two
        // cannot end up applying different sandboxes to the same plugin.
        let mut store = WasmHost::new_store(
            &self.engine,
            &self.label,
            self.declared.clone(),
            scope_paths,
            has_scope,
            profile,
            self.services(),
            &budget,
        )?;
        store.data_mut().own_operation = operation.to_string();

        let body = match &self.guest {
            Guest::V1(pre) => dispatch_v1(&mut store, pre, operation, input_json, budget)?,
            // A v2 component reaches the guest through the canonical ABI
            // instead of through `kelson_alloc`: wasmtime lowers `op` and
            // `input` into guest memory itself, bounded by the store's
            // `StoreLimits`, so the same memory cap holds without the host
            // performing the write.
            #[cfg(feature = "component-model")]
            Guest::V2(pre) => dispatch_v2(&mut store, pre, operation, input_json, budget)?,
        };
        // Drain the side effects the guest recorded. A guest failure above
        // already returned, so reaching here means the call succeeded and
        // the effects are the handler's to commit (or refuse, if malformed).
        let effects = &mut store.data_mut();
        let deltas = std::mem::take(&mut effects.deltas);
        let audits = std::mem::take(&mut effects.audits);
        let notifications = std::mem::take(&mut effects.notifications);
        Ok(CallOutput {
            body,
            deltas,
            audits,
            notifications,
        })
    }
}

/// Run one ABI **v1** call: resolve the three exports, copy `op` and `input`
/// through `kelson_alloc`, dispatch, then read the packed result back out.
///
/// Split out of [`WasmPlugin::call`] so that function reads as
/// slot -> store -> dispatch -> drain, and so the v2 generation's dispatch
/// sits beside this one as a peer rather than a nested branch.
fn dispatch_v1(
    store: &mut Store<CallState>,
    pre: &InstancePre<CallState>,
    operation: &str,
    input_json: &str,
    budget: CallBudget,
) -> Result<String, WasmError> {
    let instance = pre
        .instantiate(&mut *store)
        .map_err(|e| WasmError::Call(format!("instantiate: {e}")))?;
    let Some(memory) = instance.get_memory(&mut *store, "memory") else {
        return Err(WasmError::Call("module does not export memory".to_string()));
    };
    let alloc = instance
        .get_typed_func::<i32, i32>(&mut *store, "kelson_alloc")
        .map_err(|e| WasmError::Call(format!("export kelson_alloc: {e}")))?;
    let kelson_call = instance
        .get_typed_func::<(i32, i32, i32, i32), i64>(&mut *store, "kelson_call")
        .map_err(|e| WasmError::Call(format!("export kelson_call: {e}")))?;

    let op = operation.as_bytes();
    let input = input_json.as_bytes();
    let total = op
        .len()
        .checked_add(input.len())
        .ok_or_else(|| WasmError::Call("input length overflow".to_string()))?;
    if total > budget.memory_bytes {
        return Err(WasmError::Call(format!(
            "input of {total} bytes exceeds the plugin memory cap {}",
            budget.memory_bytes
        )));
    }
    let base = alloc
        .call(&mut *store, total as i32)
        .map_err(|e| call_error("kelson_alloc", e))?;
    if base <= 0 {
        return Err(WasmError::Call(
            "kelson_alloc returned null (out of memory)".to_string(),
        ));
    }
    let base = base as usize;
    memory
        .write(&mut *store, base, op)
        .map_err(|e| WasmError::Call(format!("write operation: {e}")))?;
    memory
        .write(&mut *store, base + op.len(), input)
        .map_err(|e| WasmError::Call(format!("write input: {e}")))?;

    let packed = kelson_call
        .call(
            &mut *store,
            (
                base as i32,
                op.len() as i32,
                (base + op.len()) as i32,
                input.len() as i32,
            ),
        )
        .map_err(|e| call_error("kelson_call", e))?;
    let raw = read_result(&mut *store, &instance, packed, "kelson_call", budget.memory_bytes)?;
    String::from_utf8(raw).map_err(|e| WasmError::Call(format!("output is not UTF-8: {e}")))
}

/// Run one ABI **v2** call: instantiate the component and dispatch through
/// the generated `kelson:plugin/dispatch` bindings.
///
/// The guest's `op` and `input` strings are lowered into guest memory by the
/// canonical ABI rather than copied by the host, so there is no `kelson_alloc`
/// step; the memory cap still holds because it is applied as a `StoreLimit`.
/// Everything else — the concurrency slot, the ready-state gate, the fuel and
/// deadline, and the effects drain back in [`WasmPlugin::call`] — is shared
/// with [`dispatch_v1`], which is what makes "one host, two generations"
/// literally true rather than two hosts that happen to look alike.
#[cfg(feature = "component-model")]
fn dispatch_v2(
    store: &mut Store<CallState>,
    pre: &crate::component::PluginPre<CallState>,
    operation: &str,
    input_json: &str,
    budget: CallBudget,
) -> Result<String, WasmError> {
    use crate::component::exports::kelson::plugin::dispatch::CallError;

    // The same explicit input bound v1 applies before allocating: the one
    // place a too-large request can be refused before any of it happens.
    let total = operation
        .len()
        .checked_add(input_json.len())
        .ok_or_else(|| WasmError::Call("input length overflow".to_string()))?;
    if total > budget.memory_bytes {
        return Err(WasmError::Call(format!(
            "input of {total} bytes exceeds the plugin memory cap {}",
            budget.memory_bytes
        )));
    }

    let plugin = pre
        .instantiate(&mut *store)
        .map_err(|e| WasmError::Call(format!("instantiate component: {e}")))?;
    let dispatch = plugin.kelson_plugin_dispatch();
    // The typed binding takes `&str` for both `string` parameters, so no
    // conversion sits between the host's values and the lift.
    match dispatch.call_call(&mut *store, operation, input_json) {
        // The guest completed and returned a body.
        Ok(Ok(body)) => Ok(body),
        // The guest REFUSED. ADR 0019 §5's refusal-as-return-value
        // discipline: this is a value the guest chose to produce, not a
        // trap, so it stays an ordinary call failure rather than a crash.
        Ok(Err(CallError::Failed)) => Err(WasmError::Call(format!(
            "dispatch.call: the guest reported that {operation} could not be completed"
        ))),
        // The guest self-reported budget exhaustion; the host answers with
        // the same verdict a trap would have produced.
        Ok(Err(CallError::OutOfFuel)) => Err(WasmError::Fuel),
        // A real trap (host-enforced fuel, epoch deadline, guest panic)
        // arrives on the outer result and is classified exactly as v1's is.
        Err(e) => Err(call_error("dispatch.call", e)),
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
        let mut plan = ExecutionPlan::new(operation, spec.profile)
            .with_capability(qualified.clone())
            .with_plane(manifest.module.plane.to_security_plane());
        relax_public_routes(manifest, capability, &mut plan);
        relax_optional_delta(spec, &mut plan);
        Ok(Self {
            plan,
            plugin,
            // What the guest is dispatched. It is the operation the host
            // registered this handler under, not the bare capability name: a
            // module's handlers declare `oauth2.authorize`, and a seam that
            // sent `authorize` would leave them unfindable inside the guest.
            // The same string is recorded on the mutations and audits this
            // call produces, and a module-qualified id is the only spelling
            // that stays unique once more than one module is loaded.
            local: qualified,
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
        let sink = self.plugin.services().notify;
        // Only the guests whose handlers take no `ctx` parameter need it
        // carried inside the payload; the checked-in WASM fixtures echo their
        // input verbatim and were written against the bare-payload shape.
        // The sidecar records which is which, as review state (`abi`'s
        // precedent), so this is a fact the review approved rather than a
        // guess made per call.
        let envelope_context = self.plugin.manifest().envelope_context;
        let result = (|| -> Result<(Value, Vec<kelson_module_contract::NotificationRequest>), AppError> {
            let ctx_json = serde_json::to_value(ctx.as_ref())
                .map_err(|e| AppError::internal(format!("wasm context encode: {e}")))?;
            // A guest entrypoint has no `ctx` parameter, so the executing
            // context travels INSIDE the document it is handed — the one
            // shape both halves of the seam name. Sending the bare payload
            // instead is how a module ends up reporting "execution context is
            // not readable" for every call it makes.
            let wire = if envelope_context {
                kelson_module_contract::encode_envelope(&ctx_json, &input)
            } else {
                input
            };
            let input_json = serde_json::to_string(&wire)
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
            let value: Value = serde_json::from_str(&output.body).map_err(|e| {
                AppError::internal(format!("wasm plugin output for {local} is not JSON: {e}"))
            })?;
            // A guest answers EITHER with its own document OR with the failure
            // it raised, and the two must not be confusable. Without this the
            // host reads `"[KSN_VALIDATION] ..."` - a bare JSON string - as a
            // successful body, so every rejection a module makes reaches the
            // caller as a 200 with the message in it, and the engine then
            // reports the op as having produced nothing at all. The error
            // crosses WHOLE (code, category and severity included), so the
            // caller sees exactly what the module refused and why.
            if let Some(raw) = value
                .as_object()
                .and_then(|o| o.get(kelson_module_contract::ENVELOPE_ERR))
            {
                return Err(serde_json::from_value::<AppError>(raw.clone()).unwrap_or_else(|_| {
                    AppError::internal(format!(
                        "wasm plugin {local} reported a failure the host could not decode"
                    ))
                }));
            }
            Ok((value, output.notifications))
        })();
        // Notifications are the one effect that leaves the process rather
        // than being written into the unit, so they are delivered HERE —
        // after the call and after its deltas have been accepted, never
        // before. A handler that notified and then failed has already been
        // returned as an error by the closure above, so nothing is sent for
        // it; a host wired with no sink refuses the call loudly rather than
        // letting the module believe the notification went out.
        Box::pin(async move {
            let (value, notifications) = result?;
            // Principal, tenant and locale are taken from the envelope the
            // host built for this call, never from the guest: a module can
            // name the event it produced but not who it notifies as.
            let envelope = kelson_module_contract::NotificationEnvelope {
                principal_id: ctx.principal.id,
                tenant_id: ctx.tenant.as_ref().map(|t| t.id),
                locale: Some(ctx.locale.lang.clone()),
            };
            for (i, request) in notifications.into_iter().enumerate() {
                let Some(sink) = sink.as_deref() else {
                    return Err(AppError::internal(format!(
                        "wasm plugin {local} declared notification #{i} ({}) but the host wired no notification sink",
                        request.event
                    )));
                };
                sink.deliver(request, envelope.clone()).await.map_err(|e| {
                    AppError::internal(format!(
                        "wasm plugin {local} notification #{i} could not be delivered: {e}"
                    ))
                })?;
            }
            Ok(value)
        })
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

/// Fuel exhaustion, saturation and deadline expiry are all sandbox budgets,
/// and all three are pinned to 503 at boot — each behaves like a timeout, not
/// a client rate limit (429 would blame the caller for our plugin being slow
/// or small). Everything else is internal.
///
/// The deadline deliberately does *not* reuse `KSN_DEADLINE`: that code
/// already renders from two other sites' own categories (the execution
/// engine's `Internal`/500 and a worker job's `Workflow`/409), and
/// [`kelson_http::ErrorRenderRegistry::set_status`] is a global per-code
/// override — pinning it here would have silently re-skinned both. A plugin's
/// wall-clock budget is a *sandbox* budget, the same family as
/// [`WASM_FUEL`], so it takes the sibling code `KSN_WASM_DEADLINE`.
fn to_app_error(err: WasmError) -> AppError {
    match err {
        WasmError::Fuel => AppError::new(
            codes::WASM_FUEL,
            ErrorCategory::ResourceExhausted,
            Severity::Medium,
            Retryability::Retryable,
            "wasm plugin exceeded its instruction budget",
        ),
        WasmError::Saturated { limit } => AppError::new(
            codes::WASM_SATURATED,
            ErrorCategory::ResourceExhausted,
            Severity::Medium,
            Retryability::Retryable,
            format!("wasm plugin concurrency limit of {limit} in-flight calls is saturated"),
        ),
        WasmError::Deadline => AppError::new(
            codes::WASM_DEADLINE,
            ErrorCategory::ResourceExhausted,
            Severity::Medium,
            Retryability::Retryable,
            "wasm plugin exceeded its wall-clock deadline",
        ),
        WasmError::Lifecycle(msg) => AppError::new(
            codes::WASM_LIFECYCLE,
            ErrorCategory::ResourceExhausted,
            Severity::Medium,
            Retryability::Retryable,
            format!("wasm plugin lifecycle refused: {msg}"),
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

/// Handler bundle carrier for the `ServiceRegistry` - the same newtype
/// pattern as oauth2's `OauthHandlerBundle` (one instance per concrete
/// type; the plain `Vec<Arc<dyn OperationHandler>>` slot is appointments').
///
/// Carries the generations behind the handlers as well, because the boot
/// path loads plugins *before* `host.boot()` and the stores they read and
/// write only exist after it: wiring the seam (ADR 0020) is therefore a
/// second act, done on this bundle once boot has produced the stores.
#[derive(Clone)]
pub struct WasmHandlerBundle(
    /// One handler per declared capability of every loaded generation.
    pub Vec<Arc<dyn OperationHandler>>,
    /// The generations those handlers dispatch to.
    pub Vec<Arc<WasmPlugin>>,
);

impl WasmHandlerBundle {
    /// Wire every generation in this bundle to the same host stores.
    ///
    /// All-or-nothing: a bundle where one plugin is wired and another is not
    /// would serve one capability with rows and another without, and nothing
    /// would report which.
    pub fn install_services(
        &self,
        services: ModuleHostServices,
    ) -> Result<(), WasmError> {
        for plugin in &self.1 {
            plugin.install_services(services.clone())?;
        }
        Ok(())
    }
}

impl std::fmt::Debug for WasmHandlerBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("WasmHandlerBundle")
            .field(&self.0.len())
            .finish()
    }
}

/// ABI v2 — the WIT world that mirrors the four host imports and the two guest
/// exports (ADR 0019 section 5). Declared at the end of this module on purpose:
/// every citation into this file points above this line, and moving the
/// existing items would invalidate them for no gain.
///
/// Compiled **only** when the `component-model` cargo feature is on, which is
/// what turns "the world is valid" from a claim into a compile-time fact:
/// `wasmtime::component::bindgen!` parses `wit/kelson-plugin.wit`, resolves the
/// `plugin` world out of the `kelson:plugin` package, and fails the build if
/// either stops holding. A default build never reads that file, so nothing here
/// changes the surface this crate ships.
///
/// What is generated: the `host` interface as a `Host` trait the runtime will
/// implement, the `dispatch` interface as a `Guest` trait a component exports,
/// and `call_error::CallError` carrying the two refusals the v1 ABI reported
/// through return codes. Wiring them into a live v2 dispatch path is the
/// in-place upgrade proof, which is sequenced separately in ADR 0019.
#[cfg(feature = "component-model")]
pub mod component {
    wasmtime::component::bindgen!({
        world: "plugin",
        path: "wit",
    });
}

/// Drop the engine's authorization boundary for a capability the reviewed
/// sidecar also publishes on a `public = true` route.
///
/// The edge strips the implicit `auth`/`verified` stages for a route the module
/// declared public, so an anonymous caller reaches dispatch with no principal
/// to authorize. Dispatch then asks the handler's plan whether one is required,
/// and an unconditional "yes" answers that caller 401 for a route the module
/// itself published as anonymous — unreachable rather than protected. The
/// first-party handlers answer the same question by hand, per operation
/// (`applications/appointments`, `applications/oauth2`); this is the same
/// answer, taken from the manifest so a module's contract is one document.
///
/// Read out of `routes`, not out of the module: a capability stays guarded when
/// its other routes are not public. And only out of the sidecar that already
/// passed drift — the guest declared the same routes, so this reads what was
/// reviewed, never what a request asked for.
fn relax_public_routes(manifest: &ModuleManifest, capability: &str, plan: &mut ExecutionPlan) {
    let publishes_publicly = manifest
        .routes
        .iter()
        .any(|route| route.public && route.capability.as_deref() == Some(capability));
    if publishes_publicly {
        plan.authorization.required = false;
    }
}

// Declared at the END of the file on purpose. `platform/docs/
// 20-ARCHITECTURE-VERIFICATION.md` cites this file by exact line number more
// than thirty times, and appending is the only kind of edit that shifts none
// of them - so the per-instance construction lives in its own module, and
// this is where its declaration goes.
mod instances;


/// A capability whose manifest declares `delta = "optional"` may return
/// having transitioned nothing, and the host has to believe that instead of
/// re-deriving the demand from the profile.
///
/// It cannot re-derive it correctly. `oauth2.clients.manage` serves `list`,
/// a pure read, alongside create/update/delete, and `oauth2.revoke` may
/// legally revoke a token that never existed (RFC 7009 sec 2.2 answers 200),
/// so the guest's own plans relax the Command delta demand for those calls -
/// per action, on the one plan the guest runs. The host holds a plan per
/// CAPABILITY, rebuilt from the sidecar profile, and demanding a delta there
/// turns a protocol-legal no-op into `KSN_NO_EFFECT`.
///
/// So the demand comes from the declaration rather than from a guess, and it
/// comes from the sidecar rather than from the guest: asking the guest would
/// hand it the one input that turns the lineage guard off, which is the
/// self-elevation `declared` exists to prevent. This is `relax_public_routes`
/// again in a different dimension - a field nothing reads is not a
/// declaration, and the review only means something once both halves act on
/// what it said.
///
/// Relaxing here does not weaken a capability that still declares
/// `required`: the guard keeps failing those that promise a transition and
/// emit none, and every write under an `optional` capability still records
/// the mutations it performed. Only the promise changes - which is the point,
/// because a promise the module never made is not a promise.
fn relax_optional_delta(spec: &kelson_registry::CapabilitySpec, plan: &mut ExecutionPlan) {
    if spec.delta == kelson_registry::Requirement::Optional {
        // G06: the profile flag and the plan flag move together. Relaxing
        // one without the other fails `ExecutionPlan::validate`, which is why
        // the guest's handlers clear both.
        plan.profile.delta_required = false;
        plan.delta.required = false;
    }
}
