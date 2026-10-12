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
    fn new_store(
        &self,
        label: &str,
        declared: Arc<BTreeSet<String>>,
        scope_paths: Vec<String>,
        has_scope: bool,
        profile: WasmCapabilityProfile,
        budget: &CallBudget,
    ) -> Result<Store<CallState>, WasmError> {
        let limits = StoreLimitsBuilder::new()
            .memory_size(budget.memory_bytes)
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

        let module = Module::new(&self.engine, bytes)
            .map_err(|e| WasmError::Load(format!("compile: {e}")))?;
        // Link-time import validation: anything the shim does not define
        // (e.g. a `sock_*` call) fails HERE, not at first dispatch.
        let pre = self
            .linker
            .instantiate_pre(&module)
            .map_err(|e| WasmError::Load(format!("link: {e}")))?;

        // Instantiate once and read the guest's self-declared manifest. This
        // scan runs under the plugin's own budget, not the host ceiling: a
        // guest whose kelson_manifest() loops forever is bounded here too,
        // otherwise the boot path would be the one place a module could run
        // unbounded.
        let json = {
            let none_declared = Arc::new(BTreeSet::new());
            // The manifest scan runs under the most restrictive profile:
            // a guest that calls the write imports from kelson_manifest()
            // records into buffers `load` never reads.
            let mut store = self.new_store(
                sidecar.name(),
                none_declared,
                Vec::new(),
                false,
                WasmCapabilityProfile::ReadOnly,
                &budget,
            )?;
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
        };

        let parsed = ModuleManifest::from_json(&json).map_err(|v| WasmError::Manifest(v.message))?;
        // `[budget]` is HOST policy, never guest state: a guest has no way to
        // declare it and no reason to. Comparing it here would refuse every
        // budgeted plugin as "drift", so both documents are normalized with
        // the budget cleared before the comparison. Everything else —
        // name, version, capabilities, ABI — still compares byte for byte.
        let mut plugin_cmp = parsed.clone();
        plugin_cmp.budget = kelson_registry::ModuleBudget::default();
        let mut sidecar_cmp = sidecar.clone();
        sidecar_cmp.budget = kelson_registry::ModuleBudget::default();
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
        Ok(WasmPlugin {
            engine: self.engine.clone(),
            pre,
            manifest: parsed,
            declared,
            label,
            budget,
            // Held only when a deadline is enforced: a fuel-bounded plugin
            // keeps no thread alive, so the common case pays nothing for the
            // machinery.
            ticker,
            in_flight: std::sync::atomic::AtomicU32::new(0),
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
}

/// A loaded, drift-checked plugin: instantiate-per-call, budget-metered.
pub struct WasmPlugin {
    engine: Engine,
    pre: InstancePre<CallState>,
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
}

impl WasmPlugin {
    /// The guest manifest, exactly as validated against the sidecar.
    pub fn manifest(&self) -> &ModuleManifest {
        &self.manifest
    }

    /// The sandbox this plugin runs in, resolved from the manifest `[budget]`
    /// against the host at load and fixed for the plugin's lifetime.
    pub fn budget(&self) -> CallBudget {
        self.budget
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
        let limits = StoreLimitsBuilder::new()
            .memory_size(budget.memory_bytes)
            .build();
        let mut store = Store::new(
            &self.engine,
            CallState {
                limits,
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
            .set_fuel(budget.fuel)
            .map_err(|e| WasmError::Call(format!("set fuel: {e}")))?;
        store.set_epoch_deadline(budget.deadline_ticks);

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
        if total > budget.memory_bytes {
            return Err(WasmError::Call(format!(
                "input of {total} bytes exceeds the plugin memory cap {}",
                budget.memory_bytes
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
        let raw = read_result(&mut store, &instance, packed, "kelson_call", budget.memory_bytes)?;
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
