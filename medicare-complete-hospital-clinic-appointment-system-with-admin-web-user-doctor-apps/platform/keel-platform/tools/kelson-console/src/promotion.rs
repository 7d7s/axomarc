//! §7 (Phase 4): the concrete adapters behind `kelson promote` / `kelson rollback`.
//!
//! `kelson-deployment` defines the five rollback dimensions as PORTS because it
//! is Enterprise-class and may reach neither `Data` (`kelson-postgres`) nor
//! `Extension` (the WASM host). The wiring therefore lives here, in
//! `kelson-console` — Tools class, which the layer matrix lets reach
//! everything. See `kelson-deployment::ports` for the full rationale.
//!
//! ## Known limitation — the config dimension is process-local
//!
//! `ConfigRuntime::rollback` swaps a REVISION HELD BY THE RUNNING PROCESS and
//! deliberately does **not** rewrite the source file (runtime.rs §5.4:
//! "rollback is a runtime-state decision"). Revisions are minted per process,
//! so a `kelson` invocation boots its own runtime at revision 1.
//!
//! Consequences, stated rather than hidden:
//!
//! * the checkpoint `kelson promote` writes records THIS process's revision,
//!   and `kelson rollback` restores it in its own process — consistent, and
//!   fails closed (`KSN_NOT_FOUND`) if a target revision isn't held;
//! * moving config CONTENT between releases is a file-level operation
//!   (`config:clear` + restart, or a source control revert), not something
//!   a CLI can do safely. This adapter never rewrites the user's config.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kelson_deployment::ports::{BoxFuture, ConfigPort, EventPort, HealthCheck, JobPort, MigrationPort, ModulePort};
use kelson_deployment::promote::ReleaseArtifact;
use kelson_deployment::rollback::PromotionTarget;
use kelson_foundation::AppError;
use kelson_jobs::{JobRegistry, JobStatus, JobStoreAdmin};
use kelson_mysql::DbPool;
use kelson_postgres::PgOutboxStore;
use kelson_registry::PackageIndex;

// ------------------------------------------------------- plan / checkpoint IO

/// Where `kelson promote` reads its plan (the §7.2 `ReleaseArtifact`).
pub const PLAN_PATH: &str = "storage/framework/promotion.json";
/// Where `kelson promote` persists the pre-promotion checkpoint (§7.2
/// `create_checkpoint`) that a later `kelson rollback` restores.
pub const CHECKPOINT_PATH: &str = "storage/framework/promotion.checkpoint.json";
/// §7.1 step 3: WASM module version pins.
pub const MODULE_PINS_PATH: &str = "storage/framework/module-pins.json";

/// Outbox rows the idempotency-stability probe scans. Bounded: this is a
/// verification pass, not a migration.
const OUTBOX_SCAN: usize = 1000;
/// In-flight jobs consulted by the job-compatibility guard.
const INFLIGHT_SCAN: usize = 5000;

fn io(what: &str, e: impl std::fmt::Display) -> AppError {
    AppError::persistence(format!("{what}: {e}"))
}

/// Resolve an operator-supplied artifact path: absolute paths stand alone,
/// relative ones resolve against the workspace root, and `None` falls back to
/// `default`.
pub fn resolve(root: &Path, override_: Option<&str>, default: &str) -> PathBuf {
    match override_ {
        Some(p) => {
            let p = PathBuf::from(p);
            if p.is_absolute() { p } else { root.join(p) }
        }
        None => root.join(default),
    }
}

/// Read the promotion plan. Missing/undecodable is a hard error — an
/// operator must not promote against an absent target.
pub fn load_plan(path: &Path) -> Result<ReleaseArtifact, AppError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| io(&format!("read {}", path.display()), e))?;
    serde_json::from_str(&raw).map_err(|e| io(&format!("parse {}", path.display()), e))
}

/// Persist the pre-promotion checkpoint before any dimension moves.
pub fn save_checkpoint(path: &Path, target: &PromotionTarget) -> Result<(), AppError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| io("create checkpoint dir", e))?;
    }
    let raw = serde_json::to_string_pretty(target).map_err(|e| io("serialize checkpoint", e))?;
    std::fs::write(path, raw).map_err(|e| io(&format!("write {}", path.display()), e))
}

/// Read the checkpoint `kelson rollback` restores.
pub fn load_checkpoint(path: &Path) -> Result<PromotionTarget, AppError> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| io(&format!("read {}", path.display()), e))?;
    serde_json::from_str(&raw).map_err(|e| io(&format!("parse {}", path.display()), e))
}

// ------------------------------------------------------------- dimension 1

/// Config dimension: the workspace's own `ConfigRuntime`.
pub struct ConfigAdapter {
    runtime: Arc<kelson_config::ConfigRuntime>,
    source_path: PathBuf,
}

impl ConfigAdapter {
    /// Boot a runtime over the same source `load_config` reads
    /// (`KEEL_CONFIG` or `config/dev.toml`).
    pub fn boot(root: &Path) -> Result<Self, AppError> {
        let source_path = crate::config_path(root);
        let runtime = kelson_config::ConfigRuntime::from_source(
            kelson_config::FileConfigSource::new(source_path.clone()),
        )?;
        Ok(Self {
            runtime: Arc::new(runtime),
            source_path,
        })
    }

    /// Effective config content hash, for the post-traffic drift probe.
    pub fn effective_hash(&self) -> Result<String, AppError> {
        let cfg = kelson_config::PlatformConfig::load(Some(&self.source_path))
            .map_err(|e| AppError::validation(format!("config load: {e}")))?
            .freeze();
        Ok(kelson_registry::config_hash(&cfg.cache_json()))
    }

    pub fn live_hash(&self) -> String {
        self.runtime.get().hash.clone()
    }
}

impl ConfigPort for ConfigAdapter {
    fn current_version(&self) -> BoxFuture<'_, Result<u64, AppError>> {
        let v = self.runtime.get().version;
        Box::pin(async move { Ok(v) })
    }

    fn activate(&self, version: u64) -> BoxFuture<'_, Result<u64, AppError>> {
        let runtime = self.runtime.clone();
        Box::pin(async move {
            let snapshot = runtime.rollback(version)?;
            Ok(snapshot.version)
        })
    }

    fn reload(&self) -> BoxFuture<'_, Result<u64, AppError>> {
        let runtime = self.runtime.clone();
        Box::pin(async move {
            let snapshot = runtime.reload()?;
            Ok(snapshot.version)
        })
    }
}

// ------------------------------------------------------------- dimension 2

/// Pair-migration dimension: the same `pairs` machinery `migrate:rollback`
/// drives, addressed by BATCH WATERMARK rather than a step count.
///
/// The watermark is the highest applied batch number (`0` = nothing applied).
/// `rollback_to(t)` reverts exactly the batches strictly above `t`, which it
/// derives from `pairs::applied` rather than assuming batches are contiguous.
/// Distinct applied batches, ascending (`0` = nothing applied).
///
/// The DB work lives in named `async fn`s rather than inline inside the
/// `Box::pin(async move …)` ports: sqlx's `Executor` impl for
/// `&mut PgConnection` is only implemented for ONE lifetime, so asking for it
/// from a boxed future's generic context fails with "implementation of
/// `Executor` is not general enough". A named fn sidesteps the HRTB entirely.
/// Distinct applied batch numbers, ascending — shared by both drivers
/// (`pairs::applied` returns one `(migration, batch)` row per file; `0` =
/// nothing applied).
fn distinct_batches(applied: Vec<(String, i32)>) -> Vec<i32> {
    let mut batches: Vec<i32> = applied.iter().map(|(_, b)| *b).collect();
    batches.sort_unstable();
    batches.dedup();
    batches
}

async fn applied_batches(pool: DbPool) -> Result<Vec<i32>, AppError> {
    match pool {
        DbPool::Postgres(pool) => {
            let mut conn = pool
                .acquire()
                .await
                .map_err(|e| AppError::persistence(format!("migrations: connect: {e}")))?;
            let applied = kelson_postgres::pairs::applied(&mut *conn).await?;
            Ok(distinct_batches(applied))
        }
        DbPool::MySql(pool) => {
            let mut conn = pool
                .acquire()
                .await
                .map_err(|e| AppError::persistence(format!("migrations: connect: {e}")))?;
            let applied = kelson_mysql::pairs::applied(&mut *conn).await?;
            Ok(distinct_batches(applied))
        }
    }
}

async fn db_watermark(pool: DbPool) -> Result<u32, AppError> {
    let batches = applied_batches(pool).await?;
    Ok(batches.last().copied().unwrap_or(0).max(0) as u32)
}

/// sqlx guard: drive the pair machinery to completion on a blocking worker.
///
/// `pairs::apply_pending`/`pairs::rollback` are generic over
/// `A: Acquire<'a>` with an associated bound `A::Connection: DerefMut<Target
/// = PgConnection>`. When that future is placed inside a `BoxFuture`, rustc
/// must prove the projection generalizes for *any* region and refuses —
/// "implementation of `sqlx::Acquire`/`Executor`/`Send` is not general
/// enough". Every parameter shape tried (owned, `&Path` param, `&'a PgPool`
/// param) hits the same wall, because the problem is the projection itself,
/// not how its inputs are bound.
///
/// The codebase already bridges sync-over-async this way — `rt`'s doc note
/// records that "the durable store ports bridge sync calls via
/// `block_in_place`". Doing the same here means the sqlx future is created,
/// driven and dropped INSIDE a synchronous call, so it never lives across an
/// await point and never enters this generator's state machine.
///
/// Correct for a CLI: `rt` builds a multi-thread runtime, and these are
/// operator-initiated commands, not per-request work.
fn block_db<F, T>(f: F) -> T
where
    F: FnOnce() -> T,
{
    tokio::task::block_in_place(f)
}

async fn apply_at(pool: DbPool, root: PathBuf) -> Result<Vec<String>, AppError> {
    block_db(move || {
        tokio::runtime::Handle::current().block_on(async move {
            match pool {
                DbPool::Postgres(pool) => {
                    let mut conn = pool
                        .acquire()
                        .await
                        .map_err(|e| AppError::persistence(format!("migrations: connect: {e}")))?;
                    kelson_postgres::pairs::apply_pending(&mut *conn, &root).await
                }
                DbPool::MySql(pool) => {
                    let mut conn = pool
                        .acquire()
                        .await
                        .map_err(|e| AppError::persistence(format!("migrations: connect: {e}")))?;
                    kelson_mysql::pairs::apply_pending(&mut *conn, &root).await
                }
            }
        })
    })
}

/// Same bridging as [`apply_at`], plus the batch arithmetic. `pairs::rollback`
/// reverts the LAST `steps` distinct batches, so `steps` counts the batches
/// strictly above `target` — derived from `applied` rather than assuming
/// batches are contiguous from 1.
async fn rollback_at(pool: DbPool, root: PathBuf, target: u32) -> Result<Vec<String>, AppError> {
    let steps = block_db({
        let pool = pool.clone();
        move || {
            tokio::runtime::Handle::current().block_on(async move {
                let applied = match pool {
                    DbPool::Postgres(pool) => {
                        let mut conn = pool.acquire().await.map_err(|e| {
                            AppError::persistence(format!("migrations: connect: {e}"))
                        })?;
                        kelson_postgres::pairs::applied(&mut *conn).await?
                    }
                    DbPool::MySql(pool) => {
                        let mut conn = pool.acquire().await.map_err(|e| {
                            AppError::persistence(format!("migrations: connect: {e}"))
                        })?;
                        kelson_mysql::pairs::applied(&mut *conn).await?
                    }
                };
                let batches = distinct_batches(applied);
                Ok::<u32, AppError>(
                    batches.iter().filter(|b| **b > target as i32).count() as u32,
                )
            })
        }
    })?;
    if steps == 0 {
        return Ok(Vec::new());
    }
    block_db(move || {
        tokio::runtime::Handle::current().block_on(async move {
            match pool {
                DbPool::Postgres(pool) => {
                    let mut conn = pool
                        .acquire()
                        .await
                        .map_err(|e| AppError::persistence(format!("migrations: connect: {e}")))?;
                    kelson_postgres::pairs::rollback(&mut *conn, &root, steps).await
                }
                DbPool::MySql(pool) => {
                    let mut conn = pool
                        .acquire()
                        .await
                        .map_err(|e| AppError::persistence(format!("migrations: connect: {e}")))?;
                    kelson_mysql::pairs::rollback(&mut *conn, &root, steps).await
                }
            }
        })
    })
}

pub struct MigrationAdapter {
    root: PathBuf,
    pool: DbPool,
}

impl MigrationAdapter {
    pub fn new(root: PathBuf, pool: DbPool) -> Self {
        Self { root, pool }
    }
}

impl MigrationPort for MigrationAdapter {
    fn watermark(&self) -> BoxFuture<'_, Result<u32, AppError>> {
        // `'static`: everything captured is owned (`PgPool` is an Arc), so
        // there is no region for rustc to unify with the elided `&self`.
        let fut: BoxFuture<'static, Result<u32, AppError>> =
            Box::pin(db_watermark(self.pool.clone()));
        fut
    }

    fn apply_pending(&self) -> BoxFuture<'_, Result<Vec<String>, AppError>> {
        Box::pin(apply_at(self.pool.clone(), self.root.clone()))
    }

    fn rollback_to(&self, target: u32) -> BoxFuture<'_, Result<Vec<String>, AppError>> {
        Box::pin(rollback_at(self.pool.clone(), self.root.clone(), target))
    }
}

// ------------------------------------------------------------- dimension 3

/// WASM module pin dimension.
///
/// Pins live in their own artifact (`storage/framework/module-pins.json`)
/// rather than being written back into the package index: the index records
/// what is *installed* (name/version/path pointing at real bytes), and
/// rewriting its `version` field would label bytes that did not change.
/// The pin file is the authoritative "which installed version is active"
/// record — the one a rollback restores.
pub struct ModulePinAdapter {
    pins_path: PathBuf,
    index_root: PathBuf,
}

impl ModulePinAdapter {
    pub fn new(root: &Path) -> Self {
        Self {
            pins_path: root.join(MODULE_PINS_PATH),
            index_root: root.to_path_buf(),
        }
    }

    /// Current pins, seeded from the package index on first use.
    async fn read(&self) -> Result<BTreeMap<String, String>, AppError> {
        if self.pins_path.exists() {
            let raw = std::fs::read_to_string(&self.pins_path)
                .map_err(|e| io(&format!("read {}", self.pins_path.display()), e))?;
            return serde_json::from_str(&raw)
                .map_err(|e| io(&format!("parse {}", self.pins_path.display()), e));
        }
        let index = PackageIndex::load(&self.index_root)
            .map_err(|e| AppError::persistence(format!("package index: {e}")))?;
        Ok(index
            .wasm
            .iter()
            .map(|w| (w.name.clone(), w.version.clone()))
            .collect())
    }
}

impl ModulePort for ModulePinAdapter {
    fn current_pins(&self) -> BoxFuture<'_, Result<BTreeMap<String, String>, AppError>> {
        let this = Self {
            pins_path: self.pins_path.clone(),
            index_root: self.index_root.clone(),
        };
        Box::pin(async move { this.read().await })
    }

    fn pin_versions<'a>(
        &'a self,
        pins: &'a BTreeMap<String, String>,
    ) -> BoxFuture<'a, Result<usize, AppError>> {
        Box::pin(async move {
            let before = self.read().await?;
            let changed = pins
                .iter()
                // `filter` yields `&(&String, &String)`.
                .filter(|(k, v)| before.get(k.as_str()) != Some(*v))
                .count();
            if changed == 0 {
                return Ok(0);
            }
            if let Some(dir) = self.pins_path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| io("create pins dir", e))?;
            }
            let raw = serde_json::to_string_pretty(pins).map_err(|e| io("serialize pins", e))?;
            std::fs::write(&self.pins_path, raw)
                .map_err(|e| io(&format!("write {}", self.pins_path.display()), e))?;
            Ok(changed)
        })
    }
}

// ------------------------------------------------------------- dimension 4

/// Job schema compatibility (§7.1 step 4: "ensure in-flight jobs can
/// complete").
///
/// Two sources, deliberately:
///
/// * the **registry**, when the caller has one — what the running code is
///   capable of (`kelson-worker`/`kernel-api` build one from application code;
///   the `kelson` CLI cannot, because it does not link application handlers);
/// * the **queue**, always — what is ACTUALLY in flight. This is the load-
///   bearing check: a rollback is unsafe exactly when a row is stamped
///   newer than the target understands.
///
/// A job absent from `target` is treated as `v1` (the `schema_version`
/// serde default) — fail closed rather than assume.
pub struct JobAdapter {
    admin: Arc<dyn JobStoreAdmin>,
    registry: Arc<JobRegistry>,
}

impl JobAdapter {
    pub fn new(admin: Arc<dyn JobStoreAdmin>, registry: Arc<JobRegistry>) -> Self {
        Self { admin, registry }
    }

    /// Jobs that still have to run after the rollback lands.
    async fn inflight(&self) -> Result<Vec<(String, u32)>, AppError> {
        let mut out = Vec::new();
        for status in [JobStatus::Pending, JobStatus::Running] {
            let rows = self.admin.list(Some(status), INFLIGHT_SCAN)?;
            out.extend(rows.into_iter().map(|j| (j.name, j.schema_version)));
        }
        Ok(out)
    }
}

impl JobPort for JobAdapter {
    fn supported_versions(&self) -> BoxFuture<'_, Result<BTreeMap<String, u32>, AppError>> {
        let registry = self.registry.clone();
        let mut merged = registry.schema_versions();
        let this = Self {
            admin: self.admin.clone(),
            registry,
        };
        Box::pin(async move {
            // Never claim less than what is actually in flight.
            for (name, version) in this.inflight().await? {
                let slot = merged.entry(name).or_insert(0);
                *slot = (*slot).max(version);
            }
            Ok(merged)
        })
    }

    fn ensure_compatibility<'a>(
        &'a self,
        target: &'a BTreeMap<String, u32>,
    ) -> BoxFuture<'a, Result<(), AppError>> {
        Box::pin(async move {
            // (a) what the running code produces must be readable at target.
            for name in self.registry.names() {
                let current = self.registry.current_schema_version(name);
                let expected = target.get(name).copied().unwrap_or(1);
                if current > expected {
                    return Err(AppError::validation(format!(
                        "job `{name}` runs schema v{current} but the target only \
                         understands v{expected} — drain that queue before rolling back"
                    )));
                }
            }
            // (b) what is actually in flight must be readable at target.
            for (name, version) in self.inflight().await? {
                let expected = target.get(&name).copied().unwrap_or(1);
                if version > expected {
                    return Err(AppError::validation(format!(
                        "job `{name}` has in-flight rows at schema v{version} but the \
                         target only understands v{expected} — drain `{name}` first"
                    )));
                }
            }
            Ok(())
        })
    }
}

// ------------------------------------------------------------- dimension 5

/// Event idempotency stability (§7.1 step 5).
///
/// Recomputes the §5.2 key for every unpublished outbox row that has one and
/// compares it against what was stored at append time. A mismatch means the
/// key formula shifted under rows the pump has not dispatched yet — those
/// rows would silently stop deduplicating (or start colliding).
///
/// Rows with an empty/NULL key are skipped: they predate the column and are
/// dispatched WITHOUT dedup by design (`kelson-event`'s documented back-compat).
///
/// `claim_unpublished` is a pure `SELECT` (see `kelson-postgres::outboxstore`
/// and its `kelson-mysql::outboxstore` twin) — it never writes, so
/// verification cannot perturb the queue.
pub struct EventAdapter {
    store: Arc<dyn kelson_event::OutboxStore>,
}

impl EventAdapter {
    pub fn new(pool: DbPool) -> Self {
        Self {
            store: match pool {
                DbPool::Postgres(p) => Arc::new(PgOutboxStore::new(p)),
                DbPool::MySql(p) => Arc::new(kelson_mysql::MySqlOutboxStore::new(p)),
            },
        }
    }
}

impl EventPort for EventAdapter {
    fn verify_idempotency_stability(&self) -> BoxFuture<'_, Result<(), AppError>> {
        let store = self.store.clone();
        Box::pin(async move {
            let rows = store.claim_unpublished(OUTBOX_SCAN).await?;
            let mut checked = 0usize;
            for row in rows {
                if row.idempotency_key.is_empty() {
                    continue; // legacy row: never deduped
                }
                let recomputed =
                    kelson_event::idempotency_key(&row.event, row.tenant_id, &row.payload);
                if recomputed != row.idempotency_key {
                    return Err(AppError::validation(format!(
                        "outbox row {}: stored idempotency key {} no longer matches the \
                         current §5.2 formula ({recomputed}) — the key changed under \
                         undispatched rows",
                        row.id, row.idempotency_key
                    )));
                }
                checked += 1;
            }
            // `checked` is the scan size for this pass; logged by the caller
            // if it wants the count (kept off the fast path here).
            let _ = checked;
            Ok(())
        })
    }
}

// ------------------------------------------------------------ health probes

/// A §7.2 stage probe. Built from a closure returning a fresh future so each
/// `run()` re-evaluates against LIVE state rather than replaying a captured
/// answer.
///
/// `Ok(false)` = unhealthy; `Err` = unhealthy *with a reason* (the pipeline
/// surfaces the message). Either way the promotion aborts.
pub struct Probe {
    name: &'static str,
    body: Box<dyn Fn() -> BoxFuture<'static, Result<bool, String>> + Send + Sync>,
}

impl Probe {
    pub fn new(
        name: &'static str,
        body: impl Fn() -> BoxFuture<'static, Result<bool, String>> + Send + Sync + 'static,
    ) -> Self {
        Self {
            name,
            body: Box::new(body),
        }
    }
}

impl HealthCheck for Probe {
    fn name(&self) -> &str {
        self.name
    }

    fn run(&self) -> BoxFuture<'_, Result<bool, AppError>> {
        let fut = (self.body)();
        Box::pin(async move { fut.await.map_err(AppError::internal) })
    }
}

/// Stage 1 probe: every pinned module is installed at the pinned version.
pub fn post_deploy_probe(root: PathBuf) -> Probe {
    Probe::new("post-deploy", move || {
        let root = root.clone();
        Box::pin(async move {
            let pins_raw = std::fs::read_to_string(root.join(MODULE_PINS_PATH))
                .map_err(|e| format!("pin file unreadable: {e}"))?;
            let pins: BTreeMap<String, String> = serde_json::from_str(&pins_raw)
                .map_err(|e| format!("pin file invalid: {e}"))?;
            let index = PackageIndex::load(&root).map_err(|e| format!("package index: {e}"))?;
            let known: BTreeMap<String, String> = index
                .wasm
                .iter()
                .map(|w| (w.name.clone(), w.version.clone()))
                .collect();
            for (name, version) in &pins {
                match known.get(name) {
                    None => return Err(format!("pinned module `{name}` is not installed")),
                    Some(installed) if installed != version => {
                        return Err(format!(
                            "module `{name}` is pinned to {version} but {installed} is installed"
                        ));
                    }
                    _ => {}
                }
            }
            Ok(true)
        })
    })
}

/// Stage 2 probe: the pair-migration watermark is readable and, when the
/// release claims a watermark, has actually reached it.
pub fn post_migrate_probe(root: PathBuf, pool: DbPool, expected: Option<u32>) -> Probe {
    Probe::new("post-migrate", move || {
        let root = root.clone();
        let pool = pool.clone();
        Box::pin(async move {
            let adapter = MigrationAdapter::new(root.clone(), pool.clone());
            let watermark = adapter
                .watermark()
                .await
                .map_err(|e| format!("watermark unreadable: {}", e.public_message))?;
            match expected {
                Some(want) if watermark < want => Err(format!(
                    "schema watermark is {watermark}, release expects {want}"
                )),
                _ => Ok(true),
            }
        })
    })
}

/// Stage 3 probe: the live config snapshot hash matches a fresh load of the
/// source — i.e. what is running is what the release ships (no drift).
pub fn post_traffic_probe(config: Arc<ConfigAdapter>) -> Probe {
    Probe::new("post-traffic", move || {
        let config = config.clone();
        Box::pin(async move {
            let live = config.live_hash();
            let fresh = config
                .effective_hash()
                .map_err(|e| format!("config re-derivation failed: {}", e.public_message))?;
            if live != fresh {
                return Err(format!("live config hash {live} drifted from source {fresh}"));
            }
            Ok(true)
        })
    })
}

/// Canary: the §5.2 idempotency invariant still holds against live rows.
///
/// This is the end-to-end check — it reaches the database and the event
/// system, the two things a promotion just moved through.
pub fn canary_probe(event_store: Arc<EventAdapter>) -> Probe {
    Probe::new("canary", move || {
        let event_store = event_store.clone();
        Box::pin(async move {
            event_store
                .verify_idempotency_stability()
                .await
                .map_err(|e| e.public_message.to_string())?;
            Ok(true)
        })
    })
}

/// The four probes, in stage order, for a promotion over `root`.
pub fn promotion_probes(
    root: &Path,
    pool: DbPool,
    expected_watermark: Option<u32>,
    config: Arc<ConfigAdapter>,
    event_store: Arc<EventAdapter>,
) -> Vec<Arc<dyn HealthCheck>> {
    vec![
        Arc::new(post_deploy_probe(root.to_path_buf())),
        Arc::new(post_migrate_probe(root.to_path_buf(), pool, expected_watermark)),
        Arc::new(post_traffic_probe(config)),
        Arc::new(canary_probe(event_store)),
    ]
}

