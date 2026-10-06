//! Target-arch §7: the five rollback dimensions (KEEL-004) as ports.
//!
//! WHY PORTS AND NOT CONCRETE TYPES (§7.1 sketches `Arc<ConfigRuntime>`
//! etc. — this is a deliberate, matrix-forced deviation):
//!
//! `keel-deployment` is **Enterprise** class (`crates/enterprise/...`), and
//! the layer matrix (STAGE-0-SPEC §3, `keel-analyzer::allowed_deps`) lets
//! Enterprise reach Foundation / Security / Execution / Application / Web /
//! Services / Async / Observability / Enterprise — but **not `Data`** and
//! **not `Extension`**:
//!
//! * dimension 2 (migration rollback) lives in `keel-postgres::pairs` → Data ✗
//! * dimension 3's WASM binaries are hosted by `keel-wasm` → Extension ✗
//!
//! Two of the five are unreachable as concrete types, so ALL FIVE are ports:
//! uniform instead of "3 concrete + 2 ports", unit-testable in-crate against
//! fakes with no PG/WASM, and wired by a composition root that *can* see
//! everything — `keel-console` (Tools) and `kernel-api` (Runtime) both
//! legitimately reach Data and Extension.
//!
//! This also matches §8, which lists `RollbackExecutor` as this component's
//! own port: keel-deployment *offers* the rollback capability, and consumes
//! these collaborators to reach each dimension.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;

use keel_foundation::AppError;

/// Boxed future used across the ports (no external async-trait dependency —
/// the same posture `keel-event`'s `BoxFuture` takes).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Dimension 1 — configuration (§7.1 step 1): atomic snapshot swap.
pub trait ConfigPort: Send + Sync {
    /// The revision currently live. Input to [`crate::rollback::RollbackExecutor::checkpoint`].
    fn current_version(&self) -> BoxFuture<'_, Result<u64, AppError>>;

    /// Make `version` the live revision and notify subscribers, returning
    /// the revision now live. This is the ROLLBACK path: `version` must be a
    /// revision this runtime already holds (see
    /// [`crate::rollback::RollbackExecutor::checkpoint`]).
    fn activate(&self, version: u64) -> BoxFuture<'_, Result<u64, AppError>>;

    /// Promotion stage 3 (§7.2): pick the release's configuration up from
    /// SOURCE and make it live, returning the revision now live.
    ///
    /// Deliberately distinct from [`ConfigPort::activate`]: a forward
    /// promotion targets a config revision that does not exist yet (revisions
    /// are minted by the reload that observes changed content), so there is
    /// no held revision to swap to. Swapping and re-reading are different
    /// operations, so they are different methods rather than one
    /// "best-effort" one.
    fn reload(&self) -> BoxFuture<'_, Result<u64, AppError>>;
}

/// Dimension 2 — pair migrations (§7.1 step 2).
///
/// The watermark is the applied-**batch** count: `keel-postgres::pairs`
/// reverts whole batches (`rollback(applied, steps)`), so a "database
/// version" in this codebase is naturally "how many batches are applied".
/// `rollback_to(t)` reverts every batch above `t`.
///
/// Foundation tables are forward-only by design (§11) — this port only ever
/// touches PAIR migrations, and an applied row with no pair file on disk
/// refuses the revert (the existing fail-closed rule in `pairs::rollback`).
pub trait MigrationPort: Send + Sync {
    /// Applied pair-batch watermark (`0` = nothing applied).
    fn watermark(&self) -> BoxFuture<'_, Result<u32, AppError>>;

    /// Apply every pending pair (promotion stage 2 — forward only).
    /// Returns the applied migration names in apply order.
    fn apply_pending(&self) -> BoxFuture<'_, Result<Vec<String>, AppError>>;

    /// Revert every batch above `target`, newest batch first. Returns the
    /// reverted names in revert order (empty = already at/below target).
    fn rollback_to(&self, target: u32) -> BoxFuture<'_, Result<Vec<String>, AppError>>;
}

/// Dimension 3 — WASM module version pins (§7.1 step 3).
///
/// Pins are name -> version, mirroring the package index entries that
/// `PackageIndex::load`/`save` already round-trip. Promotion stage 1 writes
/// the release's pins; rollback restores the checkpoint's.
pub trait ModulePort: Send + Sync {
    /// The pins currently in force (checkpoint input).
    fn current_pins(&self) -> BoxFuture<'_, Result<BTreeMap<String, String>, AppError>>;

    /// Write `pins` wholesale, returning how many entries changed.
    fn pin_versions<'a>(
        &'a self,
        pins: &'a BTreeMap<String, String>,
    ) -> BoxFuture<'a, Result<usize, AppError>>;
}

/// Dimension 4 — job schema compatibility (§7.1 step 4).
///
/// Refuses a rollback that would strand in-flight rows: if the target
/// understands schema `v` but the running code has already stamped rows at
/// `v+1`, those rows cannot run once the target is live. An adapter may
/// additionally consult the queue for row-level drain state — this crate
/// deliberately cannot (it never reaches `Data`).
pub trait JobPort: Send + Sync {
    /// Schema versions the RUNNING code understands, per job type
    /// (checkpoint input).
    fn supported_versions(&self) -> BoxFuture<'_, Result<BTreeMap<String, u32>, AppError>>;

    /// Fail closed unless every job type in `target` can still run whatever
    /// is in flight.
    fn ensure_compatibility<'a>(
        &'a self,
        target: &'a BTreeMap<String, u32>,
    ) -> BoxFuture<'a, Result<(), AppError>>;
}

/// Dimension 5 — event idempotency stability (§7.1 step 5).
///
/// The §5.2 key must not have shifted under rows already written: keys are
/// content-derived (SHA-256 over event + tenant + correlation + payload), so
/// a formula revision would make old rows re-dispatch as "new". Pure
/// verification — it takes no target, matching the doc's argument list.
pub trait EventPort: Send + Sync {
    fn verify_idempotency_stability(&self) -> BoxFuture<'_, Result<(), AppError>>;
}

/// Promotion gate (§7.2): one probe per stage. `Ok(false)` = unhealthy,
/// which trips the automatic rollback. Probe errors are treated the same
/// as `Ok(false)` by the pipeline (fail closed), and surface as `Err` here
/// only when the probe itself cannot run.
pub trait HealthCheck: Send + Sync {
    /// Stage name the probe guards (`post-deploy`, `post-migrate`,
    /// `post-traffic`, `canary`).
    fn name(&self) -> &str;
    fn run(&self) -> BoxFuture<'_, Result<bool, AppError>>;
}
