//! # kelson-jobs
//! Jobs are durable executions (11-SPEC §29/§41): same engine, same context,
//! same delta/audit semantics as any other front door. Persistence lives in the
//! postgres adapter (SKIP LOCKED, item 22); this crate defines the contract,
//! retry/backoff semantics, registries and batch/chain types.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use kelson_foundation::{AppError, CorrelationId, ExecutionId, TenantId};
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    Running,
    Succeeded,
    Failed,
    Dead,
}

fn default_queue() -> String {
    "default".into()
}

fn default_available() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

/// Pre-Phase-3 rows carry no version member — they ARE v1 payloads, so the
/// default is 1 (not 0): every in-flight job keeps loading, and the first
/// claim migrates 1 → current (the §10 DoD row).
fn default_schema_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DurableJob {
    pub id: Uuid,
    pub execution_id: ExecutionId,
    pub name: String,
    /// §5.1 (KEEL-005): payload schema version, stamped at enqueue and
    /// migrated at claim. Pre-Phase-3 rows have no member → default 1.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub payload: serde_json::Value,
    #[serde(default = "default_queue")]
    pub queue: String,
    pub attempt: u32,
    pub max_attempts: u32,
    #[serde(with = "time::serde::iso8601")]
    pub deadline: OffsetDateTime,
    /// Delayed jobs are invisible to claimers until this instant (backoff too).
    #[serde(with = "time::serde::iso8601", default = "default_available")]
    pub available_at: OffsetDateTime,
    pub status: JobStatus,
    /// Higher priority claims first within a queue.
    #[serde(default)]
    pub priority: i32,
    #[serde(default)]
    pub claimed_by: Option<String>,
    #[serde(default, with = "time::serde::iso8601::option")]
    pub claimed_at: Option<OffsetDateTime>,
    pub causation_id: Option<CorrelationId>,
    pub correlation_id: Option<CorrelationId>,
    pub last_error: Option<String>,
    #[serde(with = "time::serde::iso8601")]
    pub enqueued_at: OffsetDateTime,
    #[serde(default, with = "time::serde::iso8601::option")]
    pub finished_at: Option<OffsetDateTime>,
    /// Tenant scope (target-arch §3.1): stamped by
    /// [`TenantJobStore::enqueue_for`] so the queue can be claimed, completed
    /// and failed per tenant. `None` = platform job (infrastructure enqueues
    /// and pre-Phase-2 rows) — reachable only through the break-glass
    /// physical [`JobStore`], never through a tenant-scoped claim.
    /// `serde(default)` keeps every in-flight job payload loading.
    #[serde(default)]
    pub tenant_id: Option<TenantId>,
}

impl DurableJob {
    /// Retry semantics: exhausted attempts -> Dead (manual/intervention path).
    pub fn next_status_after_failure(&self) -> JobStatus {
        if self.attempt >= self.max_attempts {
            JobStatus::Dead
        } else {
            JobStatus::Pending
        }
    }

    pub fn exhausted(&self) -> bool {
        self.status == JobStatus::Dead
    }

    pub fn finished(&self) -> bool {
        matches!(self.status, JobStatus::Succeeded | JobStatus::Dead)
    }
}

/// Retry backoff (item 22): deterministic exponential, base * 2^(attempt-1),
/// capped at max. attempt 0 -> zero delay.
pub fn backoff_delay(attempt: u32, base: Duration, max: Duration) -> Duration {
    if attempt == 0 {
        return Duration::ZERO;
    }
    let exp = 2u128.saturating_pow(attempt.saturating_sub(1).min(30));
    let base_secs = base.whole_seconds().max(1) as u128;
    let max_secs = (max.whole_seconds().max(1) as u128).max(base_secs);
    let secs = base_secs.saturating_mul(exp).min(max_secs);
    Duration::seconds(secs.min(i64::MAX as u128) as i64)
}

/// Default retry backoff applied by stores: 30s base, 10min cap.
pub const DEFAULT_BACKOFF_BASE_SECS: i64 = 30;
pub const DEFAULT_BACKOFF_MAX_SECS: i64 = 600;

// ------------------------------------------------------------------ registry

/// Registered job type: retry budget, queue, execution deadline.
///
/// Wave 6 P8 adds the admission config the built-in middlewares read
/// (unique / rate-limit / retry-limit) — all `serde(default)` additive, so
/// pre-P8 serialized defs keep loading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobDef {
    pub name: String,
    #[serde(default = "default_max_attempts")]
    pub max_attempts: u32,
    #[serde(default = "default_queue")]
    pub queue: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// Wave 6 P8 unique jobs: a literal key guarding claims of this type
    /// through a [`UniqueLock`] (skip-if-locked; see
    /// [`UniqueMiddleware`]).
    #[serde(default)]
    pub unique_key: Option<String>,
    /// Wave 6 P8 unique jobs: how long a held lock survives a crashed
    /// holder (default [`DEFAULT_UNIQUE_TTL_SECS`]).
    #[serde(default)]
    pub unique_ttl_secs: Option<u64>,
    /// Wave 6 P8 rate-limited jobs: token bucket per job type — an
    /// exhausted checkout requeues with a refill-shaped delay instead of
    /// failing (see [`RateLimitMiddleware`]).
    #[serde(default)]
    pub rate_limit: Option<RateLimit>,
    /// Wave 6 P8 retry-limit middleware: cap on EXTRA runs enforced before
    /// the handler — over budget the job dies without running (the store's
    /// `max_attempts` budget is untouched; see [`RetryLimitMiddleware`]).
    #[serde(default)]
    pub retry_limit: Option<u32>,
}

/// Wave 6 P8: token-bucket config for one job type — `capacity` burst
/// checkouts, `refill_per_sec` steady refill. Bucket state is per-process
/// (per-node posture, the throttle-bucket posture).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RateLimit {
    pub capacity: u32,
    pub refill_per_sec: f64,
}

fn default_max_attempts() -> u32 {
    3
}
fn default_timeout_secs() -> u64 {
    300
}

/// Wave 6 P8: default unique-lock TTL — a crashed holder releases its
/// `queue:unique:*` key after this long (configurable per def).
pub const DEFAULT_UNIQUE_TTL_SECS: u64 = 300;
/// Wave 6 P8: while a unique key is locked, held claims are requeued on
/// this cadence (a poll-shaped delay; the lock TTL is the crash valve, not
/// the retry timer).
pub const UNIQUE_POLL_SECS: u64 = 5;

impl JobDef {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            max_attempts: default_max_attempts(),
            queue: default_queue(),
            timeout_secs: default_timeout_secs(),
            unique_key: None,
            unique_ttl_secs: None,
            rate_limit: None,
            retry_limit: None,
        }
    }
}

/// Compile-time-visible catalog of job types (item 22 registries).
/// Phase 3 §5.1: also owns the per-type [`JobMigrator`]s — `Debug` is
/// manual because trait objects are not `Debug`.
#[derive(Default)]
pub struct JobRegistry {
    defs: Vec<JobDef>,
    migrators: HashMap<String, Arc<dyn JobMigrator>>,
}

impl std::fmt::Debug for JobRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobRegistry")
            .field("defs", &self.defs)
            .field(
                "migrators",
                &self.migrators.keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl JobRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, def: JobDef) -> Result<(), AppError> {
        if self.defs.iter().any(|d| d.name == def.name) {
            return Err(AppError::validation(format!(
                "job registered twice: {}",
                def.name
            )));
        }
        if def.max_attempts == 0 {
            return Err(AppError::validation(format!(
                "job {} needs max_attempts >= 1",
                def.name
            )));
        }
        // Wave 6 P8: refuse admission configs that can never admit.
        if def.unique_key.as_deref() == Some("") {
            return Err(AppError::validation(format!(
                "job {} has an empty unique_key",
                def.name
            )));
        }
        if let Some(ttl) = def.unique_ttl_secs {
            if ttl == 0 {
                return Err(AppError::validation(format!(
                    "job {} unique_ttl_secs must be >= 1",
                    def.name
                )));
            }
        }
        if let Some(rate) = def.rate_limit {
            if rate.capacity == 0
                || !rate.refill_per_sec.is_finite()
                || rate.refill_per_sec <= 0.0
            {
                return Err(AppError::validation(format!(
                    "job {} rate_limit needs capacity >= 1 and refill_per_sec > 0",
                    def.name
                )));
            }
        }
        self.defs.push(def);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&JobDef> {
        self.defs.iter().find(|d| d.name == name)
    }

    pub fn names(&self) -> Vec<&str> {
        self.defs.iter().map(|d| d.name.as_str()).collect()
    }

    /// §7 (Phase 4): the schema version this registry is RUNNING for `name` —
    /// the migrator's `current_version`, or `1` for a job type with no
    /// registered migrator (rows are born at v1, `schema_version`'s
    /// serde-default).
    ///
    /// The rollback job-compatibility guard reads this to refuse a target
    /// that would strand in-flight rows: running code produces rows at
    /// `current`, so a target below `current` cannot read them back.
    pub fn current_schema_version(&self, name: &str) -> u32 {
        self.migrators
            .get(name)
            .map(|m| m.current_version().max(1))
            .unwrap_or(1)
    }

    /// §7 (Phase 4): every job type this registry knows, mapped to the
    /// schema version it is running. This is the checkpoint shape for the
    /// job dimension of a promote/rollback.
    pub fn schema_versions(&self) -> std::collections::BTreeMap<String, u32> {
        self.defs
            .iter()
            .map(|d| (d.name.clone(), self.current_schema_version(&d.name)))
            .collect()
    }

    /// Materialize a fresh Pending job from its definition.
    pub fn build(
        &self,
        name: &str,
        payload: serde_json::Value,
        now: OffsetDateTime,
    ) -> Result<DurableJob, AppError> {
        let def = self
            .get(name)
            .ok_or_else(|| AppError::not_found(format!("job definition {name}")))?;
        let mut job = build_from_def(def, payload, now);
        // New jobs are born current: stamp the migrator's version so a
        // fresh enqueue never pays a migrate-on-first-claim pass.
        if let Some(m) = self.migrators.get(name) {
            job.schema_version = m.current_version().max(1);
        }
        Ok(job)
    }

    /// §5.1 (KEEL-005): register the schema migrator for a job type.
    /// Deploying v2 = registering a migrator whose `current_version` is 2
    /// while v1 rows are still in flight — the next claim migrates them.
    pub fn register_migrator(&mut self, migrator: Arc<dyn JobMigrator>) -> Result<(), AppError> {
        let name = migrator.job_name().to_string();
        if self.migrators.contains_key(&name) {
            return Err(AppError::validation(format!(
                "job migrator registered twice: {name}"
            )));
        }
        if migrator.current_version() == 0 {
            return Err(AppError::validation(format!(
                "job migrator {name} needs current_version >= 1"
            )));
        }
        self.migrators.insert(name, migrator);
        Ok(())
    }

    /// §5.1: migrate a claimed job to its handler's current schema BEFORE
    /// dispatch. The worker calls this between claim and admission
    /// (kelson-worker `DispatchCtx::once`); tests call it directly.
    ///
    /// - No migrator for the name → identity (unversioned job type).
    /// - `schema_version == current` → identity (no-op, not an error).
    /// - `schema_version < current` → `migrate(from, payload)`, stamped
    ///   with `current_version`.
    /// - `schema_version > current` → VALIDATION error, fail closed: the
    ///   worker takes the normal fail path (retry budget) instead of
    ///   running a schema it cannot read.
    pub fn apply_migrations(&self, job: DurableJob) -> Result<DurableJob, AppError> {
        let Some(migrator) = self.migrators.get(&job.name) else {
            return Ok(job);
        };
        let current = migrator.current_version().max(1);
        if job.schema_version == current {
            return Ok(job);
        }
        if job.schema_version > current {
            return Err(AppError::validation(format!(
                "job {} schema v{} is newer than the registered v{current} — refusing to run",
                job.name, job.schema_version
            )));
        }
        let payload = migrator.migrate(job.schema_version, job.payload)?;
        Ok(DurableJob {
            payload,
            schema_version: current,
            ..job
        })
    }
}

/// §5.1 migration contract (KEEL-005, KEEL-V09): implemented by the
/// application per job type. SYNC by design — the frozen `JobStore`
/// contract (STAGE-0-SPEC §5) is sync, and migrations are pure functions
/// over the payload (no IO): `migrate(from, v1_payload) -> v2_payload`.
pub trait JobMigrator: Send + Sync {
    fn job_name(&self) -> &str;
    fn current_version(&self) -> u32;
    fn migrate(&self, from: u32, payload: serde_json::Value) -> Result<serde_json::Value, AppError>;
}

/// Construct a `DurableJob` from an explicit `JobDef` (used by the scheduler
/// and by callers holding a def outside a registry). Stamps schema v1 —
/// prefer [`JobRegistry::build`], which stamps the migrator's current
/// version; a v1-stamped row migrates on first claim anyway.
pub fn build_from_def(def: &JobDef, payload: serde_json::Value, now: OffsetDateTime) -> DurableJob {
    DurableJob {
        id: Uuid::now_v7(),
        execution_id: ExecutionId::new(),
        name: def.name.clone(),
        schema_version: default_schema_version(),
        payload,
        queue: def.queue.clone(),
        attempt: 0,
        max_attempts: def.max_attempts,
        deadline: now + Duration::seconds(def.timeout_secs as i64),
        available_at: now,
        status: JobStatus::Pending,
        priority: 0,
        claimed_by: None,
        claimed_at: None,
        causation_id: None,
        correlation_id: None,
        last_error: None,
        enqueued_at: now,
        finished_at: None,
        tenant_id: None,
    }
}

// ----------------------------------------------------------- batches/chains

/// Fan-out group (item 22 types; execution lands in S3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchSpec {
    pub id: Uuid,
    pub name: String,
    pub jobs: Vec<DurableJob>,
    /// Optional job name to enqueue when every member finished.
    pub on_complete: Option<String>,
}

impl BatchSpec {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.jobs.is_empty() {
            return Err(AppError::validation("batch requires at least one job"));
        }
        let mut ids: Vec<_> = self.jobs.iter().map(|j| j.id).collect();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        if ids.len() != before {
            return Err(AppError::validation("batch job ids must be unique"));
        }
        if self.jobs.iter().any(|j| j.status != JobStatus::Pending) {
            return Err(AppError::validation("batch members must start pending"));
        }
        Ok(())
    }

    pub fn size(&self) -> usize {
        self.jobs.len()
    }
}

/// Strictly ordered pipeline: next job starts only when the previous finished.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainSpec {
    pub id: Uuid,
    pub jobs: Vec<DurableJob>,
}

impl ChainSpec {
    pub fn validate(&self) -> Result<(), AppError> {
        if self.jobs.is_empty() {
            return Err(AppError::validation("chain requires at least one job"));
        }
        if self.jobs.iter().any(|j| j.status != JobStatus::Pending) {
            return Err(AppError::validation("chain members must start pending"));
        }
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    /// The job to run at position index (None once the chain is exhausted).
    pub fn step(&self, index: usize) -> Option<&DurableJob> {
        self.jobs.get(index)
    }
}

// ------------------------------------------------------------------- stores

/// Storage port (frozen, STAGE-0-SPEC §5). Implemented by kelson-postgres on a
/// SKIP LOCKED table and by InMemoryJobStore for degraded boot.
pub trait JobStore: Send + Sync {
    fn enqueue(&self, job: DurableJob) -> Result<Uuid, AppError>;
    fn claim(&self, worker: &str) -> Result<Option<DurableJob>, AppError>;
    fn complete(&self, id: Uuid) -> Result<(), AppError>;
    fn fail(&self, id: Uuid, error: &str) -> Result<JobStatus, AppError>;
}

/// Tenant-scoped queue port (target-arch §3.1): every call carries `TenantId`
/// at the port boundary. Enqueue stamps the job's tenant, claim only ever
/// hands back that tenant's jobs, and complete/fail treat a cross-tenant id
/// as INVISIBLE (`NOT_FOUND`) — never a mutation, never a conflict oracle
/// that leaks existence. The physical [`JobStore`] stays the platform layer
/// (the worker's cross-tenant dequeue and ops tooling — break-glass,
/// mirroring the `kelson_platform` database role).
///
/// Method names carry a `_for` suffix over the §3.1 sketch (`claim` →
/// `claim_for`): a store implements BOTH ports, and Rust resolves methods
/// without looking at arguments, so identically-named methods on one type
/// are `E0034`-ambiguous for every concrete call site. The tenant stays an
/// explicit first parameter either way — the §3.1 principle is unchanged.
pub trait TenantJobStore: Send + Sync {
    fn enqueue_for(&self, tenant: TenantId, job: DurableJob) -> Result<Uuid, AppError>;
    fn claim_for(&self, tenant: TenantId, worker: &str) -> Result<Option<DurableJob>, AppError>;
    fn complete_for(&self, tenant: TenantId, id: Uuid) -> Result<(), AppError>;
    fn fail_for(&self, tenant: TenantId, id: Uuid, error: &str) -> Result<JobStatus, AppError>;
}

/// Operational add-ons (item 22/29) beyond the frozen §5 surface.
pub trait JobStoreAdmin: Send + Sync {
    fn list(&self, status: Option<JobStatus>, limit: usize) -> Result<Vec<DurableJob>, AppError>;
    /// Wave 6 P9: fetch one job by id — the batch operations resolve each
    /// member's state precisely instead of paging `list()` (a member id set
    /// is never lost behind a limit).
    fn get(&self, id: Uuid) -> Result<Option<DurableJob>, AppError>;
    /// Manual retry: put a failed/dead job back to pending.
    fn requeue(&self, id: Uuid) -> Result<(), AppError>;
    /// Wave 5: purge every pending job (`queue:clear` — the CLI gates it to
    /// dev/test). Returns the number purged.
    fn purge_pending(&self) -> Result<u64, AppError>;
    /// Wave 5: delete dead jobs whose job ended strictly before the cutoff
    /// (`queue:flush` passes [`flush_cutoff()`] = every dead job;
    /// `queue:prune-failed` passes now - hours). Returns the number deleted.
    fn prune_dead(&self, before: OffsetDateTime) -> Result<u64, AppError>;
    /// Wave 5: delete one dead job (`queue:forget`); false = nothing deleted
    /// (unknown id or not dead).
    fn delete_dead(&self, id: Uuid) -> Result<bool, AppError>;
    /// Wave 5: queue depth + oldest pending age (`queue:monitor`).
    fn snapshot(&self) -> Result<QueueSnapshot, AppError>;
    /// Wave 6 P8: return a JUST-CLAIMED job to pending at `available_at`
    /// (admission Hold — the attempt `claim()` consumed is given back, so
    /// waiting never burns the retry budget and is never a failure).
    fn defer(&self, id: Uuid, available_at: OffsetDateTime) -> Result<(), AppError>;
    /// Wave 6 P8: terminal refusal — pending/running/failed → dead right
    /// now with `error` recorded (the retry-limit abort; batch cancel
    /// reuses it). Contrasts with `fail`, which walks the retry budget.
    fn kill(&self, id: Uuid, error: &str) -> Result<(), AppError>;
}

/// Queue depth + age facts behind `kelson queue:monitor`, kept as data so the
/// rendering is unit-testable without a store.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueueSnapshot {
    pub pending: usize,
    pub running: usize,
    pub dead: usize,
    pub succeeded: usize,
    /// Oldest pending job's enqueue time (age = now - this).
    pub oldest_pending: Option<OffsetDateTime>,
}

/// Degraded-boot store (D-52): same semantics, no durability.
#[derive(Debug, Default)]
pub struct InMemoryJobStore {
    jobs: Mutex<Vec<DurableJob>>,
}

impl InMemoryJobStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn find_index(jobs: &[DurableJob], id: Uuid) -> Result<usize, AppError> {
        jobs.iter()
            .position(|j| j.id == id)
            .ok_or_else(|| AppError::not_found(format!("job {id}")))
    }

    /// Scoped find: with a tenant in play, a cross-tenant id is INVISIBLE
    /// (`NOT_FOUND`) — a tenant can neither touch nor confirm the existence
    /// of another tenant's job.
    fn find_index_scoped(
        jobs: &[DurableJob],
        tenant: Option<TenantId>,
        id: Uuid,
    ) -> Result<usize, AppError> {
        jobs.iter()
            .position(|j| j.id == id && tenant.is_none_or(|t| j.tenant_id == Some(t)))
            .ok_or_else(|| AppError::not_found(format!("job {id}")))
    }

    /// Shared claim: `None` = platform view (any tenant), `Some` = the
    /// tenant-scoped port. The selection rules are identical; only the
    /// visibility filter differs.
    fn claim_impl(
        &self,
        tenant: Option<TenantId>,
        worker: &str,
    ) -> Result<Option<DurableJob>, AppError> {
        let now = OffsetDateTime::now_utc();
        let mut jobs = self.jobs.lock().unwrap();
        let idx = jobs
            .iter()
            .enumerate()
            .filter(|(_, j)| {
                j.status == JobStatus::Pending
                    && j.available_at <= now
                    && tenant.is_none_or(|t| j.tenant_id == Some(t))
            })
            .min_by(|(_, a), (_, b)| {
                b.priority
                    .cmp(&a.priority)
                    .then_with(|| a.available_at.cmp(&b.available_at))
                    .then_with(|| a.enqueued_at.cmp(&b.enqueued_at))
            })
            .map(|(i, _)| i);
        let Some(idx) = idx else {
            return Ok(None);
        };
        let job = &mut jobs[idx];
        job.status = JobStatus::Running;
        job.attempt += 1;
        job.claimed_by = Some(worker.to_string());
        job.claimed_at = Some(now);
        Ok(Some(job.clone()))
    }

    fn complete_impl(&self, tenant: Option<TenantId>, id: Uuid) -> Result<(), AppError> {
        let mut jobs = self.jobs.lock().unwrap();
        let idx = Self::find_index_scoped(&jobs, tenant, id)?;
        let job = &mut jobs[idx];
        if job.status != JobStatus::Running {
            return Err(AppError::conflict(format!("job {id} is not running")));
        }
        job.status = JobStatus::Succeeded;
        job.finished_at = Some(OffsetDateTime::now_utc());
        Ok(())
    }

    fn fail_impl(
        &self,
        tenant: Option<TenantId>,
        id: Uuid,
        error: &str,
    ) -> Result<JobStatus, AppError> {
        let now = OffsetDateTime::now_utc();
        let mut jobs = self.jobs.lock().unwrap();
        let idx = Self::find_index_scoped(&jobs, tenant, id)?;
        let job = &mut jobs[idx];
        if job.status != JobStatus::Running {
            return Err(AppError::conflict(format!("job {id} is not running")));
        }
        job.last_error = Some(error.to_string());
        let next = job.next_status_after_failure();
        job.status = next;
        match next {
            JobStatus::Pending => {
                let delay = backoff_delay(
                    job.attempt,
                    Duration::seconds(DEFAULT_BACKOFF_BASE_SECS),
                    Duration::seconds(DEFAULT_BACKOFF_MAX_SECS),
                );
                job.available_at = now + delay;
            }
            JobStatus::Dead => job.finished_at = Some(now),
            _ => {}
        }
        Ok(next)
    }
}

impl JobStore for InMemoryJobStore {
    fn enqueue(&self, job: DurableJob) -> Result<Uuid, AppError> {
        if job.status != JobStatus::Pending {
            return Err(AppError::validation("new jobs must be pending"));
        }
        let id = job.id;
        self.jobs.lock().unwrap().push(job);
        Ok(id)
    }

    fn claim(&self, worker: &str) -> Result<Option<DurableJob>, AppError> {
        self.claim_impl(None, worker)
    }

    fn complete(&self, id: Uuid) -> Result<(), AppError> {
        self.complete_impl(None, id)
    }

    fn fail(&self, id: Uuid, error: &str) -> Result<JobStatus, AppError> {
        self.fail_impl(None, id, error)
    }
}

impl TenantJobStore for InMemoryJobStore {
    fn enqueue_for(&self, tenant: TenantId, mut job: DurableJob) -> Result<Uuid, AppError> {
        if matches!(job.tenant_id, Some(set) if set != tenant) {
            return Err(AppError::validation(
                "job tenant does not match the enqueue tenant",
            ));
        }
        job.tenant_id = Some(tenant);
        if job.status != JobStatus::Pending {
            return Err(AppError::validation("new jobs must be pending"));
        }
        let id = job.id;
        self.jobs.lock().unwrap().push(job);
        Ok(id)
    }

    fn claim_for(&self, tenant: TenantId, worker: &str) -> Result<Option<DurableJob>, AppError> {
        self.claim_impl(Some(tenant), worker)
    }

    fn complete_for(&self, tenant: TenantId, id: Uuid) -> Result<(), AppError> {
        self.complete_impl(Some(tenant), id)
    }

    fn fail_for(&self, tenant: TenantId, id: Uuid, error: &str) -> Result<JobStatus, AppError> {
        self.fail_impl(Some(tenant), id, error)
    }
}

impl JobStoreAdmin for InMemoryJobStore {
    fn list(&self, status: Option<JobStatus>, limit: usize) -> Result<Vec<DurableJob>, AppError> {
        let jobs = self.jobs.lock().unwrap();
        let mut out: Vec<DurableJob> = jobs
            .iter()
            .filter(|j| status.is_none() || Some(j.status) == status)
            .cloned()
            .collect();
        out.sort_by(|a, b| b.enqueued_at.cmp(&a.enqueued_at));
        out.truncate(limit);
        Ok(out)
    }

    fn get(&self, id: Uuid) -> Result<Option<DurableJob>, AppError> {
        Ok(self
            .jobs
            .lock()
            .unwrap()
            .iter()
            .find(|j| j.id == id)
            .cloned())
    }

    fn requeue(&self, id: Uuid) -> Result<(), AppError> {
        let mut jobs = self.jobs.lock().unwrap();
        let idx = Self::find_index(&jobs, id)?;
        let job = &mut jobs[idx];
        if !matches!(job.status, JobStatus::Failed | JobStatus::Dead) {
            return Err(AppError::conflict(
                "only failed or dead jobs can be requeued",
            ));
        }
        job.status = JobStatus::Pending;
        job.available_at = OffsetDateTime::now_utc();
        job.claimed_by = None;
        job.claimed_at = None;
        job.finished_at = None;
        Ok(())
    }

    fn purge_pending(&self) -> Result<u64, AppError> {
        let mut jobs = self.jobs.lock().unwrap();
        let before = jobs.len();
        jobs.retain(|j| j.status != JobStatus::Pending);
        Ok((before - jobs.len()) as u64)
    }

    fn prune_dead(&self, before: OffsetDateTime) -> Result<u64, AppError> {
        let mut jobs = self.jobs.lock().unwrap();
        let len_before = jobs.len();
        jobs.retain(|j| {
            if j.status != JobStatus::Dead {
                return true;
            }
            // Keep dead jobs that ended at or after the cutoff.
            j.finished_at.unwrap_or(j.enqueued_at) >= before
        });
        Ok((len_before - jobs.len()) as u64)
    }

    fn delete_dead(&self, id: Uuid) -> Result<bool, AppError> {
        let mut jobs = self.jobs.lock().unwrap();
        let Some(idx) = jobs.iter().position(|j| j.id == id && j.status == JobStatus::Dead)
        else {
            return Ok(false);
        };
        jobs.remove(idx);
        Ok(true)
    }

    fn snapshot(&self) -> Result<QueueSnapshot, AppError> {
        let jobs = self.jobs.lock().unwrap();
        let mut snap = QueueSnapshot::default();
        for job in jobs.iter() {
            match job.status {
                JobStatus::Pending => {
                    snap.pending += 1;
                    let oldest = snap.oldest_pending.get_or_insert(job.enqueued_at);
                    if job.enqueued_at < *oldest {
                        *oldest = job.enqueued_at;
                    }
                }
                JobStatus::Running => snap.running += 1,
                JobStatus::Dead => snap.dead += 1,
                JobStatus::Succeeded => snap.succeeded += 1,
                // Failed lives between claim and the next fail() call.
                JobStatus::Failed => {}
            }
        }
        Ok(snap)
    }

    fn defer(&self, id: Uuid, available_at: OffsetDateTime) -> Result<(), AppError> {
        let mut jobs = self.jobs.lock().unwrap();
        let idx = Self::find_index(&jobs, id)?;
        let job = &mut jobs[idx];
        if job.status != JobStatus::Running {
            return Err(AppError::conflict(format!(
                "job {id} is not running; only a claimed job can be deferred"
            )));
        }
        job.status = JobStatus::Pending;
        // The claim consumed an attempt; a Hold gives it back so admission
        // waiting never eats the retry budget.
        job.attempt = job.attempt.saturating_sub(1);
        job.available_at = available_at;
        job.claimed_by = None;
        job.claimed_at = None;
        Ok(())
    }

    fn kill(&self, id: Uuid, error: &str) -> Result<(), AppError> {
        let mut jobs = self.jobs.lock().unwrap();
        let idx = Self::find_index(&jobs, id)?;
        let job = &mut jobs[idx];
        if !matches!(
            job.status,
            JobStatus::Pending | JobStatus::Running | JobStatus::Failed
        ) {
            return Err(AppError::conflict(format!(
                "job {id} is already terminal; only pending, running or failed jobs can be killed"
            )));
        }
        job.status = JobStatus::Dead;
        job.last_error = Some(error.to_string());
        job.finished_at = Some(OffsetDateTime::now_utc());
        Ok(())
    }
}

// ------------------------------------------------- batch/chain runtime (S3)

/// Cutoff later than any real finish time (`Date::MAX`, year 9999):
/// `queue:flush` passes this to [`JobStoreAdmin::prune_dead`] so the
/// strictly-before rule ("ended before the cutoff") matches every dead job.
/// (A UNIX-epoch cutoff would match none — prune keeps jobs *after* the
/// cutoff and deletes jobs *before* it.)
pub fn flush_cutoff() -> OffsetDateTime {
    time::Date::MAX.midnight().assume_utc()
}

/// `kelson queue:restart` signal file (storage/framework/queue.restart):
/// running workers watch it and stop cooperatively between jobs; supervisors
/// (queue:listen) then respawn them.
pub fn queue_restart_path(root: &Path) -> PathBuf {
    root.join("storage").join("framework").join("queue.restart")
}

/// Write the restart signal (creating parent directories); returns the path.
pub fn write_queue_restart_signal(root: &Path) -> std::io::Result<PathBuf> {
    let path = queue_restart_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &path,
        format!("restart requested at {}\n", OffsetDateTime::now_utc()),
    )?;
    Ok(path)
}

/// True when the signal file exists AND was written at or after the worker
/// started. Stale signals from before the start are ignored, so a worker
/// spawned after `queue:restart` runs normally instead of exiting in a
/// restart loop. A missing file (or unreadable timestamp) is no signal.
pub fn queue_restart_requested(root: &Path, worker_started: std::time::SystemTime) -> bool {
    let Ok(meta) = std::fs::metadata(queue_restart_path(root)) else {
        return false;
    };
    match meta.modified() {
        Ok(mtime) => mtime >= worker_started,
        Err(_) => false,
    }
}

/// Tracks a running fan-out batch: which members are still outstanding and
/// whether any member died. Completed when `outstanding` is empty.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchRun {
    pub id: Uuid,
    pub name: String,
    /// Job ids that have not finished (succeeded OR dead) yet.
    pub outstanding: Vec<Uuid>,
    /// Job ids that exhausted retries.
    pub dead: Vec<Uuid>,
    /// Job name to enqueue when every member finished.
    pub on_complete: Option<String>,
}

impl BatchRun {
    pub fn finished(&self) -> bool {
        self.outstanding.is_empty()
    }

    pub fn ok(&self) -> bool {
        self.finished() && self.dead.is_empty()
    }
}

/// Wave 6 P9 (B-32): one row per dispatched batch in the durable ledger.
/// The membership is written once at dispatch and never rewritten —
/// per-member state lives in the job store, so `BatchOps` reads live queue
/// truth and concurrent member finishes / admin ops cannot lose updates.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchRecord {
    pub id: Uuid,
    pub name: String,
    /// Every dispatched member id (written once, at dispatch).
    pub members: Vec<Uuid>,
    /// Optional job name to enqueue when every member finished.
    pub on_complete: Option<String>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    /// Terminal stamp (set by the worker when the run finishes) — the
    /// strictly-before cutoff `queue:prune-batches` ages records out on.
    #[serde(default, with = "time::serde::iso8601::option")]
    pub finished_at: Option<OffsetDateTime>,
}

/// Wave 6 P9 (B-32): the durable batch ledger behind `batch.status` /
/// `batch.cancel` / `batch.retry`, `queue:retry-batch` and
/// `queue:prune-batches`. [`MemoryBatchLedger`] covers degraded boots
/// (D-52); the postgres adapter (`PgBatchLedger`) is the durable one —
/// rows persist across restarts, so any process can resolve a batch.
pub trait BatchLedger: Send + Sync {
    /// Upsert the record (dispatch re-saves are metadata refreshes).
    fn save(&self, record: &BatchRecord) -> Result<(), AppError>;
    fn get(&self, id: Uuid) -> Result<Option<BatchRecord>, AppError>;
    /// Every record, oldest first (ops tooling; `prune` does its own SQL).
    fn list(&self) -> Result<Vec<BatchRecord>, AppError>;
    /// Stamp the terminal time once the run finishes (id unknown = no-op).
    fn mark_finished(&self, id: Uuid, at: OffsetDateTime) -> Result<(), AppError>;
    /// Delete records finished strictly before the cutoff. Returns how many
    /// were deleted; unfinished records are never pruned.
    fn prune(&self, finished_before: OffsetDateTime) -> Result<u64, AppError>;
}

/// Degraded-boot ledger (D-52): same semantics, no durability.
#[derive(Debug, Default)]
pub struct MemoryBatchLedger {
    records: Mutex<HashMap<Uuid, BatchRecord>>,
}

impl MemoryBatchLedger {
    pub fn new() -> Self {
        Self::default()
    }
}

impl BatchLedger for MemoryBatchLedger {
    fn save(&self, record: &BatchRecord) -> Result<(), AppError> {
        self.records.lock().unwrap().insert(record.id, record.clone());
        Ok(())
    }

    fn get(&self, id: Uuid) -> Result<Option<BatchRecord>, AppError> {
        Ok(self.records.lock().unwrap().get(&id).cloned())
    }

    fn list(&self) -> Result<Vec<BatchRecord>, AppError> {
        let mut rows: Vec<BatchRecord> = self.records.lock().unwrap().values().cloned().collect();
        rows.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(rows)
    }

    fn mark_finished(&self, id: Uuid, at: OffsetDateTime) -> Result<(), AppError> {
        if let Some(record) = self.records.lock().unwrap().get_mut(&id) {
            record.finished_at = Some(at);
        }
        Ok(())
    }

    fn prune(&self, finished_before: OffsetDateTime) -> Result<u64, AppError> {
        let mut records = self.records.lock().unwrap();
        let before = records.len();
        records.retain(|_, record| {
            record
                .finished_at
                .is_none_or(|finished| finished >= finished_before)
        });
        Ok((before - records.len()) as u64)
    }
}

/// Executes a `BatchSpec` (item 22 types, L33): the runtime enqueues all
/// members, then tracks completion as the worker reports job finishes.
/// Membership is mirrored to the [`BatchLedger`] (Wave 6 P9, B-32) — memory
/// on degraded boots, postgres at full boot — so the batch operations
/// (`BatchOps`), `queue:retry-batch` and `queue:prune-batches` work from
/// any process. This runs on any `JobStore` (degraded boot included).
pub struct BatchRuntime {
    store: Arc<dyn JobStore>,
    ledger: Arc<dyn BatchLedger>,
    runs: Mutex<HashMap<Uuid, BatchRun>>,
}

impl BatchRuntime {
    /// Degraded-boot runtime: membership mirrors to an in-memory ledger
    /// (D-52). Full boots pass a durable one via [`Self::with_ledger`].
    pub fn new(store: Arc<dyn JobStore>) -> Self {
        Self::with_ledger(store, Arc::new(MemoryBatchLedger::new()))
    }

    /// Wave 6 P9 (B-32): run against an explicit [`BatchLedger`] — the
    /// worker wires the pool-gated postgres ledger so another process
    /// (kelson-api, kelson-console) sees the batches this one dispatches.
    pub fn with_ledger(store: Arc<dyn JobStore>, ledger: Arc<dyn BatchLedger>) -> Self {
        Self {
            store,
            ledger,
            runs: Mutex::new(HashMap::new()),
        }
    }

    /// Enqueue every member of a validated batch; returns the batch run id.
    /// The ledger record is written BEFORE the enqueue side effects, so a
    /// ledger refusal aborts dispatch with nothing enqueued.
    pub fn dispatch(&self, spec: BatchSpec) -> Result<Uuid, AppError> {
        spec.validate()?;
        let id = spec.id;
        let outstanding: Vec<Uuid> = spec.jobs.iter().map(|j| j.id).collect();
        self.ledger.save(&BatchRecord {
            id,
            name: spec.name.clone(),
            members: outstanding.clone(),
            on_complete: spec.on_complete.clone(),
            created_at: OffsetDateTime::now_utc(),
            finished_at: None,
        })?;
        for job in spec.jobs {
            self.store.enqueue(job)?;
        }
        let run = BatchRun {
            id,
            name: spec.name,
            outstanding,
            dead: Vec::new(),
            on_complete: spec.on_complete,
        };
        self.runs.lock().unwrap().insert(id, run);
        Ok(id)
    }

    /// Worker hook: a member finished. `dead` = it exhausted its retries.
    /// Returns the completed `BatchRun` when the last member lands.
    pub fn member_finished(
        &self,
        batch_id: Uuid,
        job_id: Uuid,
        dead: bool,
    ) -> Result<Option<BatchRun>, AppError> {
        let mut runs = self.runs.lock().unwrap();
        let Some(run) = runs.get_mut(&batch_id) else {
            return Err(AppError::not_found(format!("batch {batch_id}")));
        };
        run.outstanding.retain(|id| *id != job_id);
        if dead {
            run.dead.push(job_id);
        }
        if !run.finished() {
            return Ok(None);
        }
        let finished = run.clone();
        // Wave 6 P9: stamp the terminal time so `queue:prune-batches` can
        // age the record out (an id the ledger does not know is a no-op).
        self.ledger
            .mark_finished(batch_id, OffsetDateTime::now_utc())?;
        Ok(Some(finished))
    }

    /// Convenience: run the `on_complete` job when a batch finished ok.
    pub fn maybe_run_completion(
        &self,
        registry: &JobRegistry,
        run: &BatchRun,
        now: OffsetDateTime,
    ) -> Result<Option<Uuid>, AppError> {
        if !run.ok() {
            return Ok(None);
        }
        let Some(name) = &run.on_complete else {
            return Ok(None);
        };
        let payload = serde_json::json!({ "batch_id": run.id, "name": run.name });
        let job = registry.build(name, payload, now)?;
        self.store.enqueue(job).map(Some)
    }

    pub fn run(&self, batch_id: Uuid) -> Option<BatchRun> {
        self.runs.lock().unwrap().get(&batch_id).cloned()
    }

    /// Worker convenience: find the batch containing `job_id` (if any) and
    /// report its finish. Ok(None) = job not part of any known batch.
    pub fn finish_by_job(&self, job_id: Uuid, dead: bool) -> Result<Option<BatchRun>, AppError> {
        let containing = self
            .runs
            .lock()
            .unwrap()
            .iter()
            .find(|(_, run)| run.outstanding.contains(&job_id))
            .map(|(id, _)| *id);
        match containing {
            Some(batch_id) => self.member_finished(batch_id, job_id, dead),
            None => Ok(None),
        }
    }
}

/// Wave 6 P9 (docs/19 L304): `batch.status` progress counts — live job
/// store state per member, not ledger bookkeeping. `missing` members were
/// pruned out of the store; they are counted, never guessed at.
#[derive(Debug, Clone, Serialize)]
pub struct BatchProgress {
    pub id: Uuid,
    pub name: String,
    pub total: usize,
    pub pending: usize,
    pub running: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub dead: usize,
    pub missing: usize,
    /// No member is pending or running (missing members count as settled).
    pub finished: bool,
}

/// Wave 6 P9: `batch.cancel` — pending members go dead (`kill`, P8);
/// running members keep their in-flight claim, terminal members are left
/// alone. `settled` = already succeeded/failed/dead when cancel ran.
#[derive(Debug, Clone, Serialize)]
pub struct BatchCancelOutcome {
    pub id: Uuid,
    pub killed: usize,
    pub running: usize,
    pub settled: usize,
    pub missing: usize,
}

/// Wave 6 P9: `batch.retry` — failed/dead members go back to pending
/// (`requeue`). Buckets sum to the batch total.
#[derive(Debug, Clone, Serialize)]
pub struct BatchRetryOutcome {
    pub id: Uuid,
    pub requeued: usize,
    pub pending: usize,
    pub running: usize,
    pub succeeded: usize,
    pub missing: usize,
}

/// Wave 6 P9 (docs/19 L304): the batch operations behind the
/// capability-gated endpoints (`GET /api/v1/batches/{id}`, `POST ...
/// /cancel`, `POST ... /retry`) and `queue:retry-batch` — membership from
/// the [`BatchLedger`], per-member state from one precise
/// [`JobStoreAdmin::get`] each (no `list()` paging to lose ids behind).
pub struct BatchOps<'a> {
    pub ledger: &'a dyn BatchLedger,
    pub admin: &'a dyn JobStoreAdmin,
}

impl<'a> BatchOps<'a> {
    pub fn new(ledger: &'a dyn BatchLedger, admin: &'a dyn JobStoreAdmin) -> Self {
        Self { ledger, admin }
    }

    fn record(&self, id: Uuid) -> Result<BatchRecord, AppError> {
        self.ledger
            .get(id)?
            .ok_or_else(|| AppError::not_found(format!("batch {id}")))
    }

    /// Progress counts over every member of the batch.
    pub fn status(&self, id: Uuid) -> Result<BatchProgress, AppError> {
        let record = self.record(id)?;
        let mut progress = BatchProgress {
            id,
            name: record.name.clone(),
            total: record.members.len(),
            pending: 0,
            running: 0,
            succeeded: 0,
            failed: 0,
            dead: 0,
            missing: 0,
            finished: false,
        };
        for member in &record.members {
            match self.admin.get(*member)? {
                None => progress.missing += 1,
                Some(job) => match job.status {
                    JobStatus::Pending => progress.pending += 1,
                    JobStatus::Running => progress.running += 1,
                    JobStatus::Succeeded => progress.succeeded += 1,
                    JobStatus::Failed => progress.failed += 1,
                    JobStatus::Dead => progress.dead += 1,
                },
            }
        }
        progress.finished = progress.pending == 0 && progress.running == 0;
        Ok(progress)
    }

    /// Pending members → dead; everything else is reported, untouched.
    pub fn cancel(&self, id: Uuid) -> Result<BatchCancelOutcome, AppError> {
        let record = self.record(id)?;
        let mut outcome = BatchCancelOutcome {
            id,
            killed: 0,
            running: 0,
            settled: 0,
            missing: 0,
        };
        let reason = format!("batch {} cancelled", record.name);
        for member in &record.members {
            match self.admin.get(*member)? {
                None => outcome.missing += 1,
                Some(job) => match job.status {
                    JobStatus::Pending => {
                        self.admin.kill(*member, &reason)?;
                        outcome.killed += 1;
                    }
                    JobStatus::Running => outcome.running += 1,
                    JobStatus::Succeeded | JobStatus::Failed | JobStatus::Dead => {
                        outcome.settled += 1
                    }
                },
            }
        }
        Ok(outcome)
    }

    /// Failed/dead members → pending; every other state is reported.
    pub fn retry(&self, id: Uuid) -> Result<BatchRetryOutcome, AppError> {
        let record = self.record(id)?;
        let mut outcome = BatchRetryOutcome {
            id,
            requeued: 0,
            pending: 0,
            running: 0,
            succeeded: 0,
            missing: 0,
        };
        for member in &record.members {
            match self.admin.get(*member)? {
                None => outcome.missing += 1,
                Some(job) => match job.status {
                    JobStatus::Pending => outcome.pending += 1,
                    JobStatus::Running => outcome.running += 1,
                    JobStatus::Succeeded => outcome.succeeded += 1,
                    JobStatus::Failed | JobStatus::Dead => {
                        self.admin.requeue(*member)?;
                        outcome.requeued += 1;
                    }
                },
            }
        }
        Ok(outcome)
    }
}

/// Wave 6 P8: liveness of one chain — a failed member leaves the chain
/// dead with its position recorded (docs/19), later members were never
/// enqueued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainStatus {
    Running,
    /// The member at `position` is the one that killed the chain.
    Halted { position: usize },
    /// Every member ran to success.
    Completed,
}

struct ChainArm {
    jobs: Vec<DurableJob>,
    position: usize,
    status: ChainStatus,
}

/// Executes a `ChainSpec` (L34): strictly ordered pipeline where the next
/// job is enqueued only when the previous one SUCCEEDED. A failed member
/// halts the chain (its retries are handled by the store; once it goes
/// dead the chain stops, records the failed position, and reports).
pub struct ChainRuntime {
    store: Arc<dyn JobStore>,
    chains: Mutex<HashMap<Uuid, ChainArm>>,
}

impl ChainRuntime {
    pub fn new(store: Arc<dyn JobStore>) -> Self {
        Self {
            store,
            chains: Mutex::new(HashMap::new()),
        }
    }

    /// Enqueue the first step and arm the chain.
    pub fn dispatch(&self, spec: ChainSpec) -> Result<Uuid, AppError> {
        spec.validate()?;
        let id = spec.id;
        let jobs = spec.jobs;
        let first = jobs[0].clone();
        self.store.enqueue(first)?;
        self.chains.lock().unwrap().insert(
            id,
            ChainArm {
                jobs,
                position: 0,
                status: ChainStatus::Running,
            },
        );
        Ok(id)
    }

    /// Worker hook for the current step. `succeeded` decides whether the
    /// chain advances. Returns the next enqueued job id, if the chain moved.
    pub fn step_finished(&self, chain_id: Uuid, succeeded: bool) -> Result<Option<Uuid>, AppError> {
        let mut chains = self.chains.lock().unwrap();
        let Some(arm) = chains.get_mut(&chain_id) else {
            return Err(AppError::not_found(format!("chain {chain_id}")));
        };
        if succeeded {
            if arm.status == ChainStatus::Completed {
                return Ok(None);
            }
            arm.position += 1;
            let Some(next) = arm.jobs.get(arm.position).cloned() else {
                arm.status = ChainStatus::Completed;
                return Ok(None);
            };
            // A revived member (`queue:retry` on the dead failure) resumes
            // the chain — the halt record is revocable on success.
            arm.status = ChainStatus::Running;
            // Enqueue while holding the lock to keep order deterministic.
            self.store.enqueue(next.clone())?;
            return Ok(Some(next.id));
        }
        // Failure records THIS position as the failure site; later members
        // were never enqueued. The first failure wins (repeat notifications
        // are idempotent) until a retry revives the member.
        if matches!(arm.status, ChainStatus::Halted { .. }) {
            return Ok(None);
        }
        arm.status = ChainStatus::Halted {
            position: arm.position,
        };
        Ok(None)
    }

    /// Position of the currently-armed step (for observability).
    pub fn position(&self, chain_id: Uuid) -> Option<usize> {
        self.chains.lock().unwrap().get(&chain_id).map(|arm| arm.position)
    }

    /// Wave 6 P8: the chain's liveness record — `Running`,
    /// `Halted { position }` (the failed member) or `Completed`.
    pub fn status(&self, chain_id: Uuid) -> Option<ChainStatus> {
        self.chains
            .lock()
            .unwrap()
            .get(&chain_id)
            .map(|arm| arm.status.clone())
    }

    /// Worker convenience: advance/halt the chain whose CURRENT step is
    /// `job_id`. Ok(None) = not a chain job (or halted by failure).
    pub fn finish_by_job(&self, job_id: Uuid, succeeded: bool) -> Result<Option<Uuid>, AppError> {
        let containing = self
            .chains
            .lock()
            .unwrap()
            .iter()
            .find(|(_, arm)| {
                arm.jobs
                    .get(arm.position)
                    .is_some_and(|current| current.id == job_id)
            })
            .map(|(id, _)| *id);
        match containing {
            Some(chain_id) => self.step_finished(chain_id, succeeded),
            None => Ok(None),
        }
    }
}

// ----------------------------------------------------------------- handlers

/// Future type used by async job handlers (item 23).
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// Executes a durable job: one implementation per job name. Handlers are
/// registered in a WorkerRegistry and dispatched by the kelson-worker claim
/// loop; Err maps to JobStore::fail (backoff, retry budget, then dead).
pub trait JobHandler: Send + Sync {
    /// Job name served by this handler (must match a JobRegistry entry).
    fn name(&self) -> &str;
    /// Runs the job for an already-claimed durable execution.
    fn handle<'a>(&'a self, job: &'a DurableJob) -> BoxFuture<'a, Result<(), AppError>>;
}

/// Name -> handler dispatch table used by the worker loop (item 23).
#[derive(Default)]
pub struct WorkerRegistry {
    handlers: Vec<Arc<dyn JobHandler>>,
}

impl WorkerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, handler: Arc<dyn JobHandler>) -> Result<(), AppError> {
        if self.handlers.iter().any(|h| h.name() == handler.name()) {
            return Err(AppError::validation(format!(
                "job handler registered twice: {}",
                handler.name()
            )));
        }
        self.handlers.push(handler);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn JobHandler>> {
        self.handlers.iter().find(|h| h.name() == name).cloned()
    }

    pub fn names(&self) -> Vec<&str> {
        self.handlers.iter().map(|h| h.name()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}

// ------------------------------------------------------------- middleware (P8)

/// What a middleware pre-hook decided for one claimed execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Control {
    /// Run the handler (post-hooks observe the outcome afterwards).
    Proceed,
    /// Skip this pass: the job returns to pending at `now + delay` and the
    /// attempt `claim()` consumed is given back — held is not failed (the
    /// unique / rate-limit posture).
    Hold(Duration),
    /// Terminal: the job goes dead immediately and the handler never runs
    /// (the retry-limit posture).
    Abort(&'static str),
}

/// Composable pre/post hooks around one job execution (docs/19:
/// retry-limit, rate-limit and unique ship as built-in middlewares; modules
/// register their own through the codegen `job-middleware` kind). Delivery
/// is at-least-once, so anything a middleware does must be idempotent.
pub trait JobMiddleware: Send + Sync {
    /// Stable name (registration key / log field).
    fn name(&self) -> &str;
    /// Pre-hook: runs in stack order BEFORE the handler; the first
    /// non-[`Control::Proceed`] short-circuits the rest of the chain (no
    /// later pre-hook and no handler run).
    fn before<'a>(&'a self, _job: &'a DurableJob) -> BoxFuture<'a, Result<Control, AppError>> {
        Box::pin(async { Ok(Control::Proceed) })
    }
    /// Post-hook: runs in REVERSE order when — and only when — the handler
    /// ran. Errors are logged by the dispatcher; they never rewrite the
    /// job's own outcome.
    fn after<'a>(
        &'a self,
        _job: &'a DurableJob,
        _result: &'a Result<(), AppError>,
    ) -> BoxFuture<'a, Result<(), AppError>> {
        Box::pin(async { Ok(()) })
    }
}

/// The ordered middleware of one job type: built-ins (admission) first,
/// then registered module layers. `after` walks the stack in reverse and
/// keeps going past a failing post-hook, returning the first error for the
/// dispatcher to log.
#[derive(Default)]
pub struct MiddlewareStack {
    layers: Vec<Arc<dyn JobMiddleware>>,
}

impl MiddlewareStack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, layer: Arc<dyn JobMiddleware>) {
        self.layers.push(layer);
    }

    pub fn len(&self) -> usize {
        self.layers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// Pre-hooks in stack order; short-circuits on the first decision.
    pub async fn before(&self, job: &DurableJob) -> Result<Control, AppError> {
        for layer in &self.layers {
            match layer.before(job).await? {
                Control::Proceed => {}
                other => return Ok(other),
            }
        }
        Ok(Control::Proceed)
    }

    /// Post-hooks in reverse order (only called when the handler ran).
    pub async fn after(&self, job: &DurableJob, result: &Result<(), AppError>) -> Option<AppError> {
        let mut first_error = None;
        for layer in self.layers.iter().rev() {
            if let Err(err) = layer.after(job, result).await {
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
        }
        first_error
    }
}

/// Claim-guard port for unique jobs (skip-if-locked): the worker wires a
/// cache-backed lock (`CacheService` over Redis/memory — the schedule-lock
/// seam), tests and degraded boots use [`MemoryUniqueLock`]. Async stays
/// off Services, so the port lives here (D-52 posture).
pub trait UniqueLock: Send + Sync {
    /// True = lock taken now; false = someone else holds it (skip).
    fn try_acquire(&self, key: &str, ttl: Duration) -> Result<bool, AppError>;
    /// Release early (the success path); TTL is the crash valve.
    fn release(&self, key: &str) -> Result<(), AppError>;
}

/// In-process [`UniqueLock`] (D-52 default / tests): expiry-purged map.
#[derive(Default)]
pub struct MemoryUniqueLock {
    held: Mutex<HashMap<String, OffsetDateTime>>,
}

impl MemoryUniqueLock {
    pub fn new() -> Self {
        Self::default()
    }
}

impl UniqueLock for MemoryUniqueLock {
    fn try_acquire(&self, key: &str, ttl: Duration) -> Result<bool, AppError> {
        let mut held = self.held.lock().unwrap();
        let now = OffsetDateTime::now_utc();
        held.retain(|_, until| *until > now);
        if held.contains_key(key) {
            return Ok(false);
        }
        held.insert(key.to_string(), now + ttl);
        Ok(true)
    }

    fn release(&self, key: &str) -> Result<(), AppError> {
        self.held.lock().unwrap().remove(key);
        Ok(())
    }
}

/// The cache key one unique configuration claims (`queue:unique:<job>:<key>`).
pub fn unique_lock_key(job_name: &str, unique_key: &str) -> String {
    format!("queue:unique:{job_name}:{unique_key}")
}

/// Wave 6 P8 built-in: unique-job admission. Claims of a `unique_key` job
/// are guarded by [`UniqueLock`] — while the key is held the claim is
/// SKIPPED (requeued [`UNIQUE_POLL_SECS`] later, attempt returned, never
/// failed). A successful run releases the key; failures keep it until the
/// TTL so retries stay exclusive, and a crashed holder releases via TTL.
pub struct UniqueMiddleware {
    lock: Arc<dyn UniqueLock>,
    key: String,
    ttl: Duration,
    retry: Duration,
}

impl UniqueMiddleware {
    pub fn new(lock: Arc<dyn UniqueLock>, key: String, ttl: Duration, retry: Duration) -> Self {
        Self {
            lock,
            key,
            ttl,
            retry,
        }
    }
}

impl JobMiddleware for UniqueMiddleware {
    fn name(&self) -> &str {
        "unique"
    }

    fn before<'a>(&'a self, _job: &'a DurableJob) -> BoxFuture<'a, Result<Control, AppError>> {
        Box::pin(async move {
            let acquired = self.lock.try_acquire(&self.key, self.ttl)?;
            Ok(if acquired {
                Control::Proceed
            } else {
                Control::Hold(self.retry)
            })
        })
    }

    fn after<'a>(
        &'a self,
        _job: &'a DurableJob,
        result: &'a Result<(), AppError>,
    ) -> BoxFuture<'a, Result<(), AppError>> {
        Box::pin(async move {
            if result.is_ok() {
                self.lock.release(&self.key)?;
            }
            Ok(())
        })
    }
}

/// The token bucket behind [`RateLimitMiddleware`] — `capacity` burst
/// checkouts refilled continuously at `refill_per_sec`. Pure time math
/// (`checkout(now)` takes the instant) so tests stay deterministic.
#[derive(Debug, Clone)]
pub struct TokenBucket {
    capacity: f64,
    refill_per_sec: f64,
    tokens: f64,
    last: OffsetDateTime,
}

impl TokenBucket {
    pub fn new(capacity: u32, refill_per_sec: f64) -> Self {
        Self {
            capacity: capacity as f64,
            refill_per_sec,
            tokens: capacity as f64,
            last: OffsetDateTime::now_utc(),
        }
    }

    /// Checkout one token at `now`: `None` = granted; `Some(delay)` =
    /// exhausted — the wait until the next token refills.
    pub fn checkout(&mut self, now: OffsetDateTime) -> Option<Duration> {
        let elapsed_ms = (now - self.last).whole_milliseconds().max(0);
        let gained = (elapsed_ms as f64 / 1000.0) * self.refill_per_sec;
        self.tokens = (self.tokens + gained).min(self.capacity);
        self.last = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            None
        } else {
            let need = (1.0 - self.tokens) / self.refill_per_sec;
            let ms = (need * 1000.0).ceil().max(1.0) as i64;
            Some(Duration::milliseconds(ms))
        }
    }
}

/// Wave 6 P8 built-in: rate-limit admission — one token bucket per job
/// type. An exhausted bucket HOLDS the claim (requeued when the next token
/// refills) instead of failing it, so waiting never eats the retry budget.
pub struct RateLimitMiddleware {
    bucket: Mutex<TokenBucket>,
}

impl RateLimitMiddleware {
    pub fn new(cfg: RateLimit) -> Self {
        Self {
            bucket: Mutex::new(TokenBucket::new(cfg.capacity, cfg.refill_per_sec)),
        }
    }
}

impl JobMiddleware for RateLimitMiddleware {
    fn name(&self) -> &str {
        "rate-limit"
    }

    fn before<'a>(&'a self, _job: &'a DurableJob) -> BoxFuture<'a, Result<Control, AppError>> {
        Box::pin(async move {
            let wait = self
                .bucket
                .lock()
                .unwrap()
                .checkout(OffsetDateTime::now_utc());
            Ok(match wait {
                None => Control::Proceed,
                Some(delay) => Control::Hold(delay),
            })
        })
    }
}

/// The reason [`RetryLimitMiddleware`] aborts over-budget jobs (recorded
/// as the dead job's `last_error`).
pub const RETRY_LIMIT_ABORT: &str = "retry limit reached";

/// Wave 6 P8 built-in: retry-limit admission — a cap on EXTRA runs,
/// enforced before the handler. A claim beyond `retry_limit + 1` executions
/// aborts to dead without running (the store's own `max_attempts` budget
/// stays untouched; `None` on the def = no extra cap).
pub struct RetryLimitMiddleware {
    retry_limit: u32,
}

impl RetryLimitMiddleware {
    pub fn new(retry_limit: u32) -> Self {
        Self { retry_limit }
    }
}

impl JobMiddleware for RetryLimitMiddleware {
    fn name(&self) -> &str {
        "retry-limit"
    }

    fn before<'a>(&'a self, job: &'a DurableJob) -> BoxFuture<'a, Result<Control, AppError>> {
        Box::pin(async move {
            let allowed = u64::from(self.retry_limit) + 1;
            Ok(if u64::from(job.attempt) > allowed {
                Control::Abort(RETRY_LIMIT_ABORT)
            } else {
                Control::Proceed
            })
        })
    }
}

/// Wave 6 P8: per-job-name middleware composition for the dispatcher —
/// built-ins derived once per registered def (rate buckets persist across
/// passes) plus module layers registered at boot. One claim loop builds
/// the lazy stacks (single-dispatcher posture).
pub struct JobMiddlewareStacks {
    unique: Arc<dyn UniqueLock>,
    module: HashMap<String, Vec<Arc<dyn JobMiddleware>>>,
    built: Mutex<HashMap<String, Arc<MiddlewareStack>>>,
}

impl JobMiddlewareStacks {
    pub fn new(unique: Arc<dyn UniqueLock>) -> Self {
        Self {
            unique,
            module: HashMap::new(),
            built: Mutex::new(HashMap::new()),
        }
    }

    /// Register a module middleware for one job name. Boot-time only: the
    /// first `for_job` freezes the composed stacks.
    pub fn register(&mut self, job: &str, layer: Arc<dyn JobMiddleware>) -> Result<(), AppError> {
        if !self.built.lock().unwrap().is_empty() {
            return Err(AppError::conflict(
                "job middleware must register before the first dispatch",
            ));
        }
        self.module
            .entry(job.to_string())
            .or_default()
            .push(layer);
        Ok(())
    }

    /// The stack for one job name: built-ins in admission order (unique →
    /// rate-limit → retry-limit, each present only when the def configures
    /// it), then the module layers. Unknown names get module layers alone.
    pub fn for_job(&self, name: &str, def: Option<&JobDef>) -> Arc<MiddlewareStack> {
        if let Some(hit) = self.built.lock().unwrap().get(name) {
            return hit.clone();
        }
        let mut stack = MiddlewareStack::new();
        if let Some(def) = def {
            if let Some(unique_key) = &def.unique_key {
                stack.push(Arc::new(UniqueMiddleware::new(
                    self.unique.clone(),
                    unique_lock_key(name, unique_key),
                    Duration::seconds(def.unique_ttl_secs.unwrap_or(DEFAULT_UNIQUE_TTL_SECS) as i64),
                    Duration::seconds(UNIQUE_POLL_SECS as i64),
                )));
            }
            if let Some(rate) = def.rate_limit {
                stack.push(Arc::new(RateLimitMiddleware::new(rate)));
            }
            if let Some(retries) = def.retry_limit {
                stack.push(Arc::new(RetryLimitMiddleware::new(retries)));
            }
        }
        if let Some(layers) = self.module.get(name) {
            for layer in layers {
                stack.push(layer.clone());
            }
        }
        let stack = Arc::new(stack);
        self.built
            .lock()
            .unwrap()
            .insert(name.to_string(), stack.clone());
        stack
    }
}

/// Newtype wrappers so trait objects can live in the lifecycle
/// ServiceRegistry (TypeId-keyed; get::<T>() requires a sized type).
pub struct JobStoreService(pub Arc<dyn JobStore>);
pub struct JobStoreAdminService(pub Arc<dyn JobStoreAdmin>);
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn job(attempt: u32, max: u32) -> DurableJob {
        let now = OffsetDateTime::now_utc();
        DurableJob {
            id: Uuid::now_v7(),
            execution_id: ExecutionId::new(),
            name: "demo.job".into(),
            schema_version: default_schema_version(),
            payload: serde_json::Value::Null,
            queue: "default".into(),
            attempt,
            max_attempts: max,
            deadline: now + Duration::minutes(5),
            available_at: now,
            status: JobStatus::Running,
            priority: 0,
            claimed_by: None,
            claimed_at: None,
            causation_id: None,
            correlation_id: None,
            last_error: None,
            enqueued_at: now,
            finished_at: None,
            tenant_id: None,
        }
    }

    /// Wave 5 Phase 5: the queue:clear / queue:flush / queue:forget /
    /// queue:prune-failed / queue:monitor store maintenance — purge counts,
    /// cutoff semantics (strictly-before), delete guard (dead only), and the
    /// depth/age snapshot.
    #[test]
    fn admin_maintenance_purges_prunes_and_snapshots() {
        let store = InMemoryJobStore::new();
        let now = OffsetDateTime::now_utc();

        let mut p_old = job(1, 3);
        p_old.status = JobStatus::Pending;
        p_old.enqueued_at = now - Duration::minutes(30);
        let mut p_new = job(1, 3);
        p_new.status = JobStatus::Pending;
        p_new.enqueued_at = now - Duration::minutes(5);
        let mut d_old = job(3, 3);
        d_old.status = JobStatus::Dead;
        d_old.enqueued_at = now - Duration::days(4);
        d_old.finished_at = Some(now - Duration::days(4));
        let mut d_new = job(3, 3);
        d_new.status = JobStatus::Dead;
        d_new.finished_at = Some(now - Duration::hours(1));
        let mut running = job(1, 3);
        running.status = JobStatus::Running;
        let mut succeeded = job(1, 3);
        succeeded.status = JobStatus::Succeeded;
        succeeded.finished_at = Some(now);

        for j in [&p_old, &p_new, &d_old, &d_new, &running, &succeeded] {
            store.jobs.lock().unwrap().push((*j).clone());
        }

        // Snapshot: depths per status + the oldest pending enqueue time.
        let snap = store.snapshot().expect("snapshot runs");
        assert_eq!(
            (snap.pending, snap.running, snap.dead, snap.succeeded),
            (2, 1, 2, 1),
            "{snap:?}"
        );
        assert_eq!(snap.oldest_pending, Some(p_old.enqueued_at));

        // prune_dead with a 72h cutoff removes only the 4-day-old dead job
        // (strictly-before semantics keep the 1h-old one at the boundary).
        let pruned = store
            .prune_dead(now - Duration::hours(72))
            .expect("prune runs");
        assert_eq!(pruned, 1, "only the old dead job");
        // The flush cutoff (later than any real finish time) takes the rest
        // of the dead jobs.
        let flushed = store.prune_dead(flush_cutoff()).expect("flush runs");
        assert_eq!(flushed, 1, "flush deletes every dead job");

        // delete_dead: only dead rows count; pending ids report "not deleted".
        assert!(!store.delete_dead(p_old.id).expect("delete guard"));
        let mut d_extra = job(3, 3);
        d_extra.status = JobStatus::Dead;
        d_extra.finished_at = Some(now);
        store.jobs.lock().unwrap().push(d_extra.clone());
        assert!(store.delete_dead(d_extra.id).expect("delete dead"));

        // purge_pending: both pending jobs go; running/succeeded survive.
        assert_eq!(store.purge_pending().expect("purge"), 2);
        let snap = store.snapshot().expect("snapshot runs");
        assert_eq!(
            (snap.pending, snap.running, snap.dead, snap.succeeded),
            (0, 1, 0, 1),
            "{snap:?}"
        );
        assert_eq!(snap.oldest_pending, None, "no pending left to age");
    }

    /// Wave 5 Phase 5: the queue:restart signal file stops workers that were
    /// running when it was written — and is ignored by workers started after
    /// it (no restart loop); missing file = no signal.
    #[test]
    fn restart_signal_is_ignored_when_older_than_the_worker_start() {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "kelson-jobs-restart-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&root).unwrap();

        // No signal file yet: nothing requested.
        let started = std::time::SystemTime::now();
        assert!(!queue_restart_requested(&root, started));

        // Signal written now: a worker that started before it stops.
        let path = write_queue_restart_signal(&root).expect("signal writes");
        assert!(path.exists(), "signal file exists");
        let earlier = std::time::SystemTime::now() - std::time::Duration::from_secs(10);
        assert!(
            queue_restart_requested(&root, earlier),
            "signal newer than the worker start stops it"
        );

        // A worker spawned after the signal ignores the stale file — that is
        // what keeps queue:listen respawns out of a restart loop.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(10);
        assert!(
            !queue_restart_requested(&root, later),
            "stale signal is ignored by a fresh worker"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn retry_exhaustion_marks_dead() {
        assert_eq!(job(1, 3).next_status_after_failure(), JobStatus::Pending);
        assert_eq!(job(3, 3).next_status_after_failure(), JobStatus::Dead);
    }

    #[test]
    fn backoff_curve_and_cap() {
        let base = Duration::seconds(30);
        let max = Duration::seconds(600);
        assert_eq!(backoff_delay(0, base, max), Duration::ZERO);
        assert_eq!(backoff_delay(1, base, max), Duration::seconds(30));
        assert_eq!(backoff_delay(2, base, max), Duration::seconds(60));
        assert_eq!(backoff_delay(3, base, max), Duration::seconds(120));
        assert_eq!(backoff_delay(9, base, max), Duration::seconds(600));
    }

    #[test]
    fn registry_rejects_duplicates_and_builds_jobs() {
        let mut reg = JobRegistry::new();
        reg.register(JobDef::new("mail.send")).unwrap();
        assert!(reg.register(JobDef::new("mail.send")).is_err());
        let now = OffsetDateTime::now_utc();
        let built = reg.build("mail.send", json!({"to": "a@b.c"}), now).unwrap();
        assert_eq!(built.status, JobStatus::Pending);
        assert_eq!(built.max_attempts, 3);
        assert_eq!(built.deadline, now + Duration::seconds(300));
        assert!(reg.build("missing.job", json!(null), now).is_err());
    }

    #[test]
    fn store_lifecycle_with_priority_and_backoff() {
        let store = InMemoryJobStore::new();
        let mut low = job(0, 3);
        low.status = JobStatus::Pending;
        low.priority = 0;
        let mut high = job(0, 3);
        high.status = JobStatus::Pending;
        high.priority = 10;
        store.enqueue(low).unwrap();
        store.enqueue(high).unwrap();

        let claimed = store.claim("w1").unwrap().unwrap();
        assert_eq!(claimed.priority, 10, "higher priority claims first");
        assert_eq!(claimed.attempt, 1);

        let status = store.fail(claimed.id, "boom").unwrap();
        assert_eq!(status, JobStatus::Pending);
        let after_fail = store.list(Some(JobStatus::Pending), 10).unwrap();
        assert!(after_fail
            .iter()
            .any(|j| j.last_error.as_deref() == Some("boom")));
        assert!(after_fail
            .iter()
            .all(|j| j.available_at > OffsetDateTime::now_utc() - Duration::seconds(1)));

        store
            .complete(store.claim("w1").unwrap().unwrap().id)
            .unwrap();
        let done = store.list(Some(JobStatus::Succeeded), 10).unwrap();
        assert_eq!(done.len(), 1);
    }

    #[test]
    fn delayed_jobs_wait_and_dead_jobs_requeue() {
        let store = InMemoryJobStore::new();
        let mut j = job(0, 1);
        j.status = JobStatus::Pending;
        let id = store.enqueue(j).unwrap();
        let claimed = store.claim("w1").unwrap().unwrap();
        let status = store.fail(id, "fatal").unwrap();
        assert_eq!(status, JobStatus::Dead, "attempt 1 of max 1 dies");
        assert!(claimed.finished() || store.list(Some(JobStatus::Dead), 10).unwrap().len() == 1);
        assert!(
            store.claim("w1").unwrap().is_none(),
            "dead jobs are not claimable"
        );

        store.requeue(id).unwrap();
        assert!(
            store.claim("w2").unwrap().is_some(),
            "requeued dead job is claimable"
        );
        assert!(store.requeue(Uuid::now_v7()).is_err(), "unknown id errors");
    }

    #[test]
    fn batch_and_chain_validation() {
        let mut b = BatchSpec {
            id: Uuid::now_v7(),
            name: "nightly".into(),
            jobs: vec![],
            on_complete: None,
        };
        assert!(b.validate().is_err());
        let mut j1 = job(0, 3);
        j1.status = JobStatus::Pending;
        b.jobs = vec![j1.clone(), j1.clone()];
        assert!(b.validate().is_err(), "duplicate ids rejected");
        b.jobs = vec![j1.clone()];
        assert!(b.validate().is_ok());
        assert_eq!(b.size(), 1);

        let c = ChainSpec {
            id: Uuid::now_v7(),
            jobs: vec![j1],
        };
        assert!(c.validate().is_ok());
        assert!(c.step(0).is_some());
        assert!(c.step(1).is_none());
        assert!(ChainSpec {
            id: Uuid::now_v7(),
            jobs: vec![]
        }
        .validate()
        .is_err());
    }

    struct CountingHandler {
        calls: std::sync::atomic::AtomicU32,
    }

    impl JobHandler for CountingHandler {
        fn name(&self) -> &str {
            "demo.job"
        }

        fn handle<'a>(&'a self, _job: &'a DurableJob) -> BoxFuture<'a, Result<(), AppError>> {
            Box::pin(async move {
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
        }
    }

    #[test]
    fn worker_registry_registers_and_rejects_duplicates() {
        let mut reg = WorkerRegistry::new();
        assert!(reg.is_empty());
        let handler = Arc::new(CountingHandler {
            calls: std::sync::atomic::AtomicU32::new(0),
        });
        reg.register(handler.clone()).unwrap();
        assert!(reg.register(handler.clone()).is_err());
        assert_eq!(reg.names(), vec!["demo.job"]);
        assert!(reg.get("demo.job").is_some());
        assert!(reg.get("missing.job").is_none());
        assert!(!reg.is_empty());
    }

    #[tokio::test]
    async fn worker_registry_dispatches_to_handler() {
        let mut reg = WorkerRegistry::new();
        let handler = Arc::new(CountingHandler {
            calls: std::sync::atomic::AtomicU32::new(0),
        });
        reg.register(handler.clone()).unwrap();
        let mut j = job(0, 3);
        j.status = JobStatus::Pending;
        let h = reg.get("demo.job").unwrap();
        for _ in 0..3 {
            h.handle(&j).await.unwrap();
        }
        assert_eq!(handler.calls.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[test]
    fn batch_runtime_fans_out_and_completes() {
        let store = Arc::new(InMemoryJobStore::new());
        let runtime = BatchRuntime::new(store.clone());
        let mut j1 = job(0, 3);
        j1.status = JobStatus::Pending;
        let mut j2 = job(0, 3);
        j2.status = JobStatus::Pending;
        let spec = BatchSpec {
            id: Uuid::now_v7(),
            name: "nightly".into(),
            jobs: vec![j1.clone(), j2.clone()],
            on_complete: Some("batch.done".into()),
        };
        let id = runtime.dispatch(spec).unwrap();
        assert_eq!(
            store.list(Some(JobStatus::Pending), 10).unwrap().len(),
            2,
            "both members enqueued"
        );

        assert!(
            runtime.member_finished(id, j1.id, false).unwrap().is_none(),
            "not finished yet"
        );
        let done = runtime.member_finished(id, j2.id, false).unwrap().unwrap();
        assert!(done.finished());
        assert!(done.ok());

        let mut registry = JobRegistry::new();
        registry.register(JobDef::new("batch.done")).unwrap();
        let enqueued = runtime
            .maybe_run_completion(&registry, &done, OffsetDateTime::now_utc())
            .unwrap();
        assert!(enqueued.is_some(), "on_complete job enqueued for ok batch");
        assert!(store
            .list(Some(JobStatus::Pending), 10)
            .unwrap()
            .iter()
            .any(|j| j.name == "batch.done"));
    }

    #[test]
    fn batch_with_dead_member_skips_completion() {
        let store = Arc::new(InMemoryJobStore::new());
        let runtime = BatchRuntime::new(store.clone());
        let mut j1 = job(0, 3);
        j1.status = JobStatus::Pending;
        let spec = BatchSpec {
            id: Uuid::now_v7(),
            name: "risky".into(),
            jobs: vec![j1.clone()],
            on_complete: Some("batch.done".into()),
        };
        let id = runtime.dispatch(spec).unwrap();
        let done = runtime.member_finished(id, j1.id, true).unwrap().unwrap();
        assert!(done.finished());
        assert!(!done.ok(), "dead member marks the batch failed");
        let mut registry = JobRegistry::new();
        registry.register(JobDef::new("batch.done")).unwrap();
        assert!(runtime
            .maybe_run_completion(&registry, &done, OffsetDateTime::now_utc())
            .unwrap()
            .is_none());
    }

    // ------------------------------------- Wave 6 P9: durable batch ledger

    /// Wave 6 P9: `batch.status` counts live store state per member (the
    /// ledger carries membership only) and an unknown batch id is a
    /// not-found.
    #[test]
    fn batch_status_counts_members_from_the_store() {
        let store = Arc::new(InMemoryJobStore::new());
        let ledger = Arc::new(MemoryBatchLedger::new());
        let runtime = BatchRuntime::with_ledger(store.clone(), ledger.clone());
        let mut members = Vec::new();
        for _ in 0..3 {
            let mut member = job(0, 3);
            member.status = JobStatus::Pending;
            members.push(member);
        }
        let spec = BatchSpec {
            id: Uuid::now_v7(),
            name: "nightly".into(),
            jobs: members,
            on_complete: None,
        };
        let batch = runtime.dispatch(spec).unwrap();
        let ops = BatchOps::new(ledger.as_ref(), store.as_ref());

        let progress = ops.status(batch).unwrap();
        assert_eq!(
            (progress.total, progress.pending, progress.finished),
            (3, 3, false),
            "three members, all pending"
        );

        let claimed = store.claim("worker").unwrap().expect("one claimable");
        store.complete(claimed.id).unwrap();
        let progress = ops.status(batch).unwrap();
        assert_eq!(
            (progress.total, progress.pending, progress.succeeded, progress.running),
            (3, 2, 1, 0),
            "the finished member leaves pending"
        );
        assert!(!progress.finished, "members still outstanding");

        let unknown = ops.status(Uuid::now_v7()).expect_err("unknown batch");
        assert_eq!(unknown.code, kelson_foundation::codes::NOT_FOUND);
    }

    /// Wave 6 P9: `batch.cancel` takes pending members to dead and leaves
    /// the in-flight claim; `batch.retry` puts the dead back to pending
    /// (the P8 `kill` / Wave 5 `requeue` primitives behind the ops).
    #[test]
    fn batch_cancel_and_retry_move_members() {
        let store = Arc::new(InMemoryJobStore::new());
        let ledger = Arc::new(MemoryBatchLedger::new());
        let runtime = BatchRuntime::with_ledger(store.clone(), ledger.clone());
        let mut a = job(0, 3);
        a.status = JobStatus::Pending;
        let mut b = job(0, 3);
        b.status = JobStatus::Pending;
        let spec = BatchSpec {
            id: Uuid::now_v7(),
            name: "risky".into(),
            jobs: vec![a, b],
            on_complete: None,
        };
        let batch = runtime.dispatch(spec).unwrap();
        let ops = BatchOps::new(ledger.as_ref(), store.as_ref());

        store.claim("worker").unwrap().expect("one claimable");
        let cancelled = ops.cancel(batch).unwrap();
        assert_eq!(
            (cancelled.killed, cancelled.running, cancelled.settled),
            (1, 1, 0),
            "pending member killed, running member untouched"
        );
        let progress = ops.status(batch).unwrap();
        assert_eq!(
            (progress.dead, progress.running, progress.finished),
            (1, 1, false)
        );

        let retried = ops.retry(batch).unwrap();
        assert_eq!(
            (retried.requeued, retried.pending, retried.running),
            (1, 0, 1),
            "the dead member returns to pending"
        );
        let progress = ops.status(batch).unwrap();
        assert_eq!((progress.pending, progress.dead, progress.running), (1, 0, 1));
    }

    /// Wave 6 P9 (B-32): dispatch writes the membership, the last member
    /// stamps the terminal time, and prune ages out only finished records —
    /// an unfinished batch survives any cutoff.
    #[test]
    fn batch_ledger_records_finishes_and_prunes() {
        let store = Arc::new(InMemoryJobStore::new());
        let ledger = Arc::new(MemoryBatchLedger::new());
        let runtime = BatchRuntime::with_ledger(store.clone(), ledger.clone());

        let mut member = job(0, 3);
        member.status = JobStatus::Pending;
        let spec = BatchSpec {
            id: Uuid::now_v7(),
            name: "one".into(),
            jobs: vec![member.clone()],
            on_complete: None,
        };
        let done_batch = runtime.dispatch(spec).unwrap();
        let record = ledger
            .get(done_batch)
            .unwrap()
            .expect("record written at dispatch");
        assert_eq!(record.name, "one");
        assert_eq!(record.members, vec![member.id]);
        assert!(record.finished_at.is_none(), "open until the last member lands");

        let mut open_member = job(0, 3);
        open_member.status = JobStatus::Pending;
        let open_spec = BatchSpec {
            id: Uuid::now_v7(),
            name: "open".into(),
            jobs: vec![open_member],
            on_complete: None,
        };
        let open_batch = runtime.dispatch(open_spec).unwrap();

        runtime
            .member_finished(done_batch, member.id, false)
            .unwrap()
            .expect("batch finished");
        let record = ledger.get(done_batch).unwrap().expect("record");
        assert!(record.finished_at.is_some(), "terminal stamp set");

        let past = OffsetDateTime::now_utc() - Duration::hours(1);
        assert_eq!(
            ledger.prune(past).unwrap(),
            0,
            "strictly-before: a stamp newer than the cutoff stays"
        );
        let future = OffsetDateTime::now_utc() + Duration::hours(1);
        assert_eq!(ledger.prune(future).unwrap(), 1, "finished record aged out");
        assert!(ledger.get(done_batch).unwrap().is_none());
        assert!(
            ledger.get(open_batch).unwrap().is_some(),
            "unfinished batches survive every cutoff"
        );
    }

    #[test]
    fn chain_advances_only_on_success() {
        let store = Arc::new(InMemoryJobStore::new());
        let runtime = ChainRuntime::new(store.clone());
        let mut a = job(0, 3);
        a.status = JobStatus::Pending;
        let mut b = job(0, 3);
        b.status = JobStatus::Pending;
        let mut c = job(0, 3);
        c.status = JobStatus::Pending;
        let spec = ChainSpec {
            id: Uuid::now_v7(),
            jobs: vec![a.clone(), b.clone(), c.clone()],
        };
        let id = runtime.dispatch(spec).unwrap();
        assert_eq!(runtime.position(id), Some(0));
        assert_eq!(
            store.list(Some(JobStatus::Pending), 10).unwrap().len(),
            1,
            "only step 0 enqueued"
        );

        assert!(
            runtime.step_finished(id, true).unwrap().is_some(),
            "advance to step 1"
        );
        assert_eq!(runtime.position(id), Some(1));
        assert_eq!(
            store.list(Some(JobStatus::Pending), 10).unwrap().len(),
            2,
            "steps 0+1 enqueued"
        );
        assert!(
            runtime.step_finished(id, false).unwrap().is_none(),
            "failure halts the chain"
        );
        assert_eq!(
            store.list(Some(JobStatus::Pending), 10).unwrap().len(),
            2,
            "step 2 never enqueued"
        );
        assert!(
            runtime.step_finished(id, true).unwrap().is_some(),
            "advance to step 2 after recovery"
        );
        assert_eq!(
            store.list(Some(JobStatus::Pending), 10).unwrap().len(),
            3,
            "all three steps enqueued"
        );
        assert!(
            runtime.step_finished(id, true).unwrap().is_none(),
            "chain exhausted"
        );
        assert_eq!(runtime.position(id), Some(3));
    }

    // ------------------------------------- Wave 6 P8: admission + middleware

    /// Wave 6 P8 unique jobs: the first claim takes the key, a concurrent
    /// claim HOLDS (skip-if-locked, never failed), failure keeps the key
    /// exclusive for retries, success releases it.
    #[tokio::test]
    async fn unique_admission_holds_locked_claims_until_release() {
        let lock = Arc::new(MemoryUniqueLock::new());
        let key = unique_lock_key("demo.job", "row-1");
        let ttl = Duration::seconds(30);
        let retry = Duration::seconds(UNIQUE_POLL_SECS as i64);
        let a = UniqueMiddleware::new(lock.clone(), key.clone(), ttl, retry);
        let b = UniqueMiddleware::new(lock.clone(), key, ttl, retry);
        let job = job(1, 3);

        assert_eq!(
            a.before(&job).await.unwrap(),
            Control::Proceed,
            "first claim takes the lock"
        );
        assert_eq!(
            b.before(&job).await.unwrap(),
            Control::Hold(retry),
            "second claim of a held key skips a poll later"
        );

        let bad: Result<(), AppError> = Err(AppError::validation("boom"));
        a.after(&job, &bad).await.unwrap();
        assert_eq!(
            b.before(&job).await.unwrap(),
            Control::Hold(retry),
            "a failed run keeps the key — retries stay exclusive"
        );

        let ok: Result<(), AppError> = Ok(());
        a.after(&job, &ok).await.unwrap();
        assert_eq!(
            b.before(&job).await.unwrap(),
            Control::Proceed,
            "success releases the key"
        );
    }

    /// Wave 6 P8 rate-limited jobs: burst up to capacity, an exhausted
    /// bucket holds shaped by the refill rate, time restores service. The
    /// middleware front-end holds real claims the same way.
    #[tokio::test]
    async fn rate_limited_admission_holds_until_the_bucket_refills() {
        let mut bucket = TokenBucket::new(2, 0.5); // 1 token / 2s
        let t0 = OffsetDateTime::now_utc();
        assert_eq!(bucket.checkout(t0), None, "burst 1");
        assert_eq!(bucket.checkout(t0), None, "burst 2");
        assert_eq!(
            bucket.checkout(t0),
            Some(Duration::seconds(2)),
            "exhausted: next token in 1/refill_per_sec"
        );
        assert!(
            bucket
                .checkout(t0 + Duration::milliseconds(999))
                .is_some(),
            "still refilling"
        );
        assert_eq!(
            bucket.checkout(t0 + Duration::seconds(3)),
            None,
            "the refill restored service"
        );

        let mw = RateLimitMiddleware::new(RateLimit {
            capacity: 1,
            refill_per_sec: 0.001,
        });
        let job = job(0, 3);
        assert_eq!(mw.before(&job).await.unwrap(), Control::Proceed, "burst");
        match mw.before(&job).await.unwrap() {
            Control::Hold(delay) => assert!(
                delay >= Duration::seconds(900),
                "exhausted checkout waits for the refill, got {delay}"
            ),
            other => panic!("expected Hold, got {other:?}"),
        }
    }

    /// Wave 6 P8: a Hold gives the claim back — `defer` returns the
    /// attempt, clears the claim and reschedules availability (a future
    /// hold makes the job unclaimable, a past one restarts the cycle);
    /// `kill` is terminal and refuses terminal jobs.
    #[test]
    fn admission_defer_returns_the_attempt_and_kill_is_terminal() {
        let store = InMemoryJobStore::new();
        let def = JobDef::new("demo.job");
        let now = OffsetDateTime::now_utc();
        let id = store
            .enqueue(build_from_def(&def, json!({}), now))
            .unwrap();

        let claimed = store.claim("w1").unwrap().unwrap();
        assert_eq!(claimed.attempt, 1, "claim consumes an attempt");

        // Hold into the past: the attempt comes back and the cycle restarts.
        store.defer(id, now - Duration::seconds(1)).unwrap();
        let back = store
            .list(Some(JobStatus::Pending), 10)
            .unwrap()
            .remove(0);
        assert_eq!(back.attempt, 0, "Hold returns the attempt");
        assert!(back.claimed_by.is_none(), "claim cleared");
        assert!(back.claimed_at.is_none(), "claim cleared");

        // Hold into the future: invisible to claimers until it elapses.
        let again = store.claim("w1").unwrap().unwrap();
        assert_eq!(again.attempt, 1, "the cycle restarted at 1");
        store.defer(id, now + Duration::seconds(30)).unwrap();
        assert!(
            store.claim("w1").unwrap().is_none(),
            "held job is not available yet"
        );
        assert!(
            store.defer(id, now).is_err(),
            "defer only accepts a claimed (running) job"
        );

        // Make it claimable again and kill it.
        {
            let mut jobs = store.jobs.lock().unwrap();
            jobs[0].available_at = now - Duration::seconds(1);
        }
        let running = store.claim("w1").unwrap().unwrap();
        store
            .kill(running.id, RETRY_LIMIT_ABORT)
            .expect("kill is terminal");
        let dead = store
            .list(Some(JobStatus::Dead), 10)
            .unwrap()
            .remove(0);
        assert_eq!(dead.last_error.as_deref(), Some(RETRY_LIMIT_ABORT));
        assert!(dead.finished_at.is_some(), "kill stamps finished_at");
        assert!(store.kill(id, "again").is_err(), "already terminal");
        assert!(store.defer(id, now).is_err(), "dead cannot be deferred");
    }

    /// Wave 6 P8 retry-limit: executions beyond `retry_limit + 1` abort to
    /// dead without running the handler.
    #[tokio::test]
    async fn retry_limit_aborts_over_budget_claims() {
        let mw = RetryLimitMiddleware::new(1); // two executions allowed
        assert_eq!(
            mw.before(&job(1, 3)).await.unwrap(),
            Control::Proceed,
            "first run allowed"
        );
        assert_eq!(
            mw.before(&job(2, 3)).await.unwrap(),
            Control::Proceed,
            "one retry allowed"
        );
        assert_eq!(
            mw.before(&job(3, 3)).await.unwrap(),
            Control::Abort(RETRY_LIMIT_ABORT),
            "budget spent"
        );
    }

    /// Wave 6 P8 chain execution: a failed member leaves the chain dead
    /// with ITS position recorded (docs/19); repeat notifications keep the
    /// first failure site, later members stay unenqueued, and a revived
    /// member (`queue:retry`) resumes the chain.
    #[test]
    fn chain_failure_records_the_halted_position() {
        let store = Arc::new(InMemoryJobStore::new());
        let runtime = ChainRuntime::new(store.clone());
        let mut a = job(0, 3);
        a.status = JobStatus::Pending;
        let mut b = job(0, 3);
        b.status = JobStatus::Pending;
        let mut c = job(0, 3);
        c.status = JobStatus::Pending;
        let b_id = b.id;
        let spec = ChainSpec {
            id: Uuid::now_v7(),
            jobs: vec![a, b, c],
        };
        let id = runtime.dispatch(spec).unwrap();
        assert_eq!(runtime.status(id), Some(ChainStatus::Running));

        runtime.step_finished(id, true).unwrap();
        assert_eq!(runtime.position(id), Some(1));

        assert!(
            runtime.finish_by_job(b_id, false).unwrap().is_none(),
            "a dead member halts the chain"
        );
        assert_eq!(
            runtime.status(id),
            Some(ChainStatus::Halted { position: 1 }),
            "the failed position is recorded"
        );
        assert!(
            runtime.finish_by_job(b_id, false).unwrap().is_none(),
            "repeat notifications are idempotent"
        );
        assert_eq!(
            runtime.status(id),
            Some(ChainStatus::Halted { position: 1 }),
            "the FIRST failure site stands"
        );
        assert_eq!(
            store.list(Some(JobStatus::Pending), 10).unwrap().len(),
            2,
            "member 2 never enqueued"
        );

        assert!(
            runtime.finish_by_job(b_id, true).unwrap().is_some(),
            "a revived member resumes the chain"
        );
        assert_eq!(runtime.status(id), Some(ChainStatus::Running));
        assert!(
            runtime.step_finished(id, true).unwrap().is_none(),
            "last member completes the chain"
        );
        assert_eq!(runtime.status(id), Some(ChainStatus::Completed));
    }

    struct Recorder {
        tag: &'static str,
        trace: Arc<Mutex<Vec<String>>>,
        hold: Option<Duration>,
    }

    impl JobMiddleware for Recorder {
        fn name(&self) -> &str {
            self.tag
        }

        fn before<'a>(&'a self, _job: &'a DurableJob) -> BoxFuture<'a, Result<Control, AppError>> {
            let tag = self.tag;
            let hold = self.hold;
            let trace = self.trace.clone();
            Box::pin(async move {
                trace.lock().unwrap().push(format!("{tag}:before"));
                Ok(match hold {
                    Some(delay) => Control::Hold(delay),
                    None => Control::Proceed,
                })
            })
        }

        fn after<'a>(
            &'a self,
            _job: &'a DurableJob,
            _result: &'a Result<(), AppError>,
        ) -> BoxFuture<'a, Result<(), AppError>> {
            let tag = self.tag;
            let trace = self.trace.clone();
            Box::pin(async move {
                trace.lock().unwrap().push(format!("{tag}:after"));
                Ok(())
            })
        }
    }

    /// Wave 6 P8: pre-hooks run in stack order and short-circuit on the
    /// first non-Proceed decision; post-hooks run in reverse when — and
    /// only when — the handler ran.
    #[tokio::test]
    async fn middleware_stack_orders_hooks_and_short_circuits() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut stack = MiddlewareStack::new();
        stack.push(Arc::new(Recorder {
            tag: "a",
            trace: trace.clone(),
            hold: None,
        }));
        stack.push(Arc::new(Recorder {
            tag: "b",
            trace: trace.clone(),
            hold: Some(Duration::seconds(7)),
        }));
        stack.push(Arc::new(Recorder {
            tag: "c",
            trace: trace.clone(),
            hold: None,
        }));
        let job = job(0, 3);

        match stack.before(&job).await.unwrap() {
            Control::Hold(delay) => assert_eq!(delay, Duration::seconds(7)),
            other => panic!("expected Hold from the middle layer, got {other:?}"),
        }
        assert_eq!(
            *trace.lock().unwrap(),
            vec!["a:before", "b:before"],
            "c never ran — the Hold short-circuits the chain"
        );

        trace.lock().unwrap().clear();
        let ok: Result<(), AppError> = Ok(());
        assert!(stack.after(&job, &ok).await.is_none());
        assert_eq!(
            *trace.lock().unwrap(),
            vec!["c:after", "b:after", "a:after"],
            "post-hooks reverse"
        );
    }

    /// Wave 6 P8: built-ins compose from the def in admission order, the
    /// module layer appends, registration freezes at first dispatch, and
    /// an unconfigured job gets module layers alone.
    #[tokio::test]
    async fn stacks_compose_builtins_then_module_layers() {
        let mut def = JobDef::new("demo.job");
        def.unique_key = Some("row-9".into());
        def.rate_limit = Some(RateLimit {
            capacity: 2,
            refill_per_sec: 1.0,
        });
        def.retry_limit = Some(4);
        let mut registry = JobRegistry::new();
        registry.register(def).unwrap();

        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut stacks = JobMiddlewareStacks::new(Arc::new(MemoryUniqueLock::new()));
        stacks
            .register(
                "demo.job",
                Arc::new(Recorder {
                    tag: "module",
                    trace: trace.clone(),
                    hold: None,
                }),
            )
            .unwrap();

        let stack = stacks.for_job("demo.job", registry.get("demo.job"));
        assert_eq!(
            stack.len(),
            4,
            "unique + rate-limit + retry-limit + module layer"
        );
        let job = job(0, 3);
        assert_eq!(stack.before(&job).await.unwrap(), Control::Proceed);
        assert_eq!(
            *trace.lock().unwrap(),
            vec!["module:before"],
            "the module layer runs after admission granted"
        );

        let other = Arc::new(Recorder {
            tag: "late",
            trace: trace.clone(),
            hold: None,
        });
        assert!(
            stacks.register("other.job", other).is_err(),
            "middleware must register before the first dispatch"
        );
        assert!(
            stacks.for_job("unconfigured.job", None).is_empty(),
            "no def, no built-ins"
        );
    }

    /// Wave 6 P8: admission configs that can never admit refuse
    /// registration (boot-time fail-closed).
    #[test]
    fn registry_refuses_impossible_admission_configs() {
        let mut registry = JobRegistry::new();
        let mut def = JobDef::new("demo.job");
        def.unique_key = Some(String::new());
        assert!(registry.register(def).is_err(), "empty unique_key");

        let mut def = JobDef::new("demo.job");
        def.rate_limit = Some(RateLimit {
            capacity: 0,
            refill_per_sec: 1.0,
        });
        assert!(registry.register(def).is_err(), "zero capacity");

        let mut def = JobDef::new("demo.job");
        def.rate_limit = Some(RateLimit {
            capacity: 1,
            refill_per_sec: 0.0,
        });
        assert!(registry.register(def).is_err(), "zero refill");

        let mut def = JobDef::new("demo.job");
        def.unique_ttl_secs = Some(0);
        assert!(registry.register(def).is_err(), "zero unique TTL");
    }

    // ------------------------------------------- tenant isolation probes (DoD)

    fn scoped_job(name: &str) -> DurableJob {
        let now = OffsetDateTime::now_utc();
        DurableJob {
            id: Uuid::now_v7(),
            execution_id: ExecutionId::new(),
            name: name.into(),
            schema_version: default_schema_version(),
            payload: serde_json::Value::Null,
            queue: "default".into(),
            attempt: 0,
            max_attempts: 3,
            deadline: now + Duration::minutes(5),
            available_at: now,
            status: JobStatus::Pending,
            priority: 0,
            claimed_by: None,
            claimed_at: None,
            causation_id: None,
            correlation_id: None,
            last_error: None,
            enqueued_at: now,
            finished_at: None,
            tenant_id: None,
        }
    }

    /// Claim is tenant-scoped: a pending job enqueued by another tenant —
    /// even one enqueued EARLIER — never surfaces through this tenant's
    /// claim.
    #[test]
    fn tenant_isolation_queue_claim_never_crosses_tenants() {
        let store = InMemoryJobStore::new();
        let a = TenantId::new();
        let b = TenantId::new();
        store.enqueue_for(b, scoped_job("mail.send")).unwrap(); // enqueued first
        store.enqueue_for(a, scoped_job("mail.send")).unwrap();

        let job_a = store
            .claim_for(a, "w1")
            .unwrap()
            .expect("A claims its own job");
        assert_eq!(job_a.tenant_id, Some(a));
        // B's job sits FIRST in the queue but must not surface through A's
        // claim (the unscoped platform claim would hand it out).
        assert!(
            store.claim_for(a, "w1").unwrap().is_none(),
            "cross-tenant pending job must not surface"
        );

        let job_b = store
            .claim_for(b, "w1")
            .unwrap()
            .expect("B claims its own job");
        assert_eq!(job_b.tenant_id, Some(b));
        assert!(store.claim_for(b, "w1").unwrap().is_none());
    }

    /// A cross-tenant job id is INVISIBLE to complete/fail: `NOT_FOUND`,
    /// never a mutation and never a conflict oracle leaking existence.
    #[test]
    fn tenant_isolation_queue_cross_tenant_complete_and_fail_are_invisible() {
        let store = InMemoryJobStore::new();
        let a = TenantId::new();
        let b = TenantId::new();
        store.enqueue_for(a, scoped_job("report.build")).unwrap();
        let job = store.claim_for(a, "w1").unwrap().expect("A claims its job");

        let err = store.complete_for(b, job.id).unwrap_err();
        assert_eq!(
            err.code,
            kelson_foundation::codes::NOT_FOUND,
            "cross-tenant complete must be invisible"
        );
        let err = store.fail_for(b, job.id, "sabotage").unwrap_err();
        assert_eq!(
            err.code,
            kelson_foundation::codes::NOT_FOUND,
            "cross-tenant fail must be invisible"
        );

        // A's own completion still works — B's attempts disturbed nothing.
        store.complete_for(a, job.id).unwrap();
    }

    /// Enqueue stamps the caller's tenant, refuses a mismatched pre-stamp,
    /// and the stamped job is visible to the break-glass platform claim.
    #[test]
    fn tenant_isolation_queue_enqueue_stamps_tenant_and_rejects_mismatch() {
        let store = InMemoryJobStore::new();
        let a = TenantId::new();
        let b = TenantId::new();

        let mut foreign = scoped_job("mail.send");
        foreign.tenant_id = Some(b);
        let err = store.enqueue_for(a, foreign).unwrap_err();
        assert_eq!(
            err.code,
            kelson_foundation::codes::VALIDATION,
            "a job pre-stamped for another tenant is refused"
        );

        let id = store.enqueue_for(a, scoped_job("mail.send")).unwrap();
        let seen = store
            .claim("platform-worker")
            .unwrap()
            .expect("the platform claim sees every tenant's jobs");
        assert_eq!(seen.id, id);
        assert_eq!(
            seen.tenant_id,
            Some(a),
            "enqueue stamps the tenant on the stored job"
        );
    }

    // -------------------------------------- pentest (Phase 5, §9 box 2)

    /// Penetration test: a foreign job id must be indistinguishable from an
    /// id that never existed.
    ///
    /// If "belongs to another tenant" and "does not exist" answered
    /// differently, tenant A could enumerate tenant B's job ids one probe at
    /// a time and then target them. Both must return the same code AND the
    /// same public message, and neither probe may mutate the real job.
    #[test]
    fn pentest_queue_foreign_id_leaks_no_existence_oracle() {
        let store = InMemoryJobStore::new();
        let a = TenantId::new();
        let b = TenantId::new();
        store.enqueue_for(a, scoped_job("mail.send")).unwrap();
        let real = store
            .claim_for(a, "w1")
            .unwrap()
            .expect("A claims its own job");
        let never_existed = Uuid::new_v4();

        let foreign = store.fail_for(b, real.id, "probe").unwrap_err();
        let absent = store.fail_for(b, never_existed, "probe").unwrap_err();

        assert_eq!(
            foreign.code, absent.code,
            "a foreign id must not answer differently from a missing one"
        );
        assert_eq!(
            foreign.code,
            kelson_foundation::codes::NOT_FOUND,
            "both must be NOT_FOUND"
        );
        // The messages necessarily quote the id the CALLER supplied — that is
        // the attacker's own input reflected back, not a leak. What matters
        // is that both follow one template: neither ever confirms existence
        // with a distinct phrase ("forbidden", "wrong tenant", …).
        let template = |msg: &str, id: Uuid| msg.replace(&id.to_string(), "<probed-id>");
        assert_eq!(
            template(&foreign.public_message, real.id),
            template(&absent.public_message, never_existed),
            "the wording must not reveal whether the job exists"
        );

        // `complete` is equally silent — the oracle must not open on the
        // mutating path either.
        let foreign_complete = store.complete_for(b, real.id).unwrap_err();
        assert_eq!(foreign_complete.code, absent.code);
        assert_eq!(
            template(&foreign_complete.public_message, real.id),
            template(&absent.public_message, never_existed)
        );

        // B's probing left the job intact: A still finishes it.
        store.complete_for(a, real.id).unwrap();
    }

    /// The worker string is unscoped BY DESIGN — the tenant argument is the
    /// only thing that selects a queue. Claiming under a worker identity
    /// that names another tenant (or the platform) must not widen the view.
    #[test]
    fn pentest_queue_worker_identity_does_not_widen_the_claim_view() {
        let store = InMemoryJobStore::new();
        let a = TenantId::new();
        let b = TenantId::new();
        // Only B has work: anything A pulls here is by definition a leak.
        store.enqueue_for(b, scoped_job("mail.send")).unwrap();

        let workers = vec![
            "w-b".to_string(),
            format!("t:{b}:w"),
            "platform-worker".to_string(),
            "*".to_string(),
            String::new(),
        ];
        for worker in &workers {
            assert!(
                store.claim_for(a, worker).unwrap().is_none(),
                "worker identity {worker:?} must not widen A's claim view"
            );
        }

        // B's job is still queued, claimable only by B.
        assert!(store.claim_for(b, "w-b").unwrap().is_some());
    }

    // ----------------------------------------- job schema probes (§5.1 DoD)

    /// A v1→v2 migrator for the probes: v1 `{ "to": "..." }` gains the v2
    /// `{ "to": "...", "locale": "en" }` shape (defaults the new field).
    struct LocaleMigrator;

    impl JobMigrator for LocaleMigrator {
        fn job_name(&self) -> &str {
            "mail.send"
        }

        fn current_version(&self) -> u32 {
            2
        }

        fn migrate(
            &self,
            from: u32,
            mut payload: serde_json::Value,
        ) -> Result<serde_json::Value, AppError> {
            assert_eq!(from, 1, "only v1 rows exist in this probe");
            payload["locale"] = json!("en");
            Ok(payload)
        }
    }

    fn versioned_registry() -> JobRegistry {
        let mut registry = JobRegistry::new();
        registry.register(JobDef::new("mail.send")).unwrap();
        registry
            .register_migrator(Arc::new(LocaleMigrator))
            .unwrap();
        registry
    }

    /// Legacy rows (no `schema_version` member — pre-Phase-3 serializations)
    /// load as v1, so every in-flight job deserializes with zero errors
    /// after the v2 deploy (the §10 DoD row).
    #[test]
    fn job_schema_version_defaults_to_1_for_legacy_rows() {
        let mut legacy = serde_json::to_value(scoped_job("mail.send")).unwrap();
        legacy.as_object_mut().unwrap().remove("schema_version");
        let row: DurableJob = serde_json::from_value(legacy).unwrap();
        assert_eq!(row.schema_version, 1);
    }

    /// The §10 DoD row: a v1 job in flight while v2 deploys migrates on
    /// claim — payload upgraded, version stamped, no deserialization
    /// error anywhere on the path.
    #[test]
    fn job_migrator_upgrades_v1_payload_to_v2_on_claim() {
        let registry = versioned_registry();
        let store = InMemoryJobStore::new();
        let now = OffsetDateTime::now_utc();

        // Enqueued by the OLD code: schema v1 payload shape (a registry
        // with the def but WITHOUT the migrator stamps v1).
        let mut old_code = JobRegistry::new();
        old_code.register(JobDef::new("mail.send")).unwrap();
        let v1 = old_code
            .build("mail.send", json!({"to": "a@test.dev"}), now)
            .unwrap();
        assert_eq!(v1.schema_version, 1);
        store.enqueue(v1).unwrap();

        // Claimed after the v2 deploy (migrator registered).
        let claimed = store.claim("worker-1").unwrap().expect("job claimed");
        let migrated = registry.apply_migrations(claimed).unwrap();
        assert_eq!(migrated.schema_version, 2, "stamped current");
        assert_eq!(
            migrated.payload["locale"], json!("en"),
            "v2 field defaulted by the migration"
        );
        assert_eq!(
            migrated.payload["to"], json!("a@test.dev"),
            "v1 content preserved"
        );
    }

    /// New jobs are born current — no migrate-on-first-claim pass.
    #[test]
    fn job_build_stamps_current_version() {
        let registry = versioned_registry();
        let job = registry
            .build("mail.send", json!({"to": "b@test.dev"}), OffsetDateTime::now_utc())
            .unwrap();
        assert_eq!(job.schema_version, 2);
        let same = registry.apply_migrations(job.clone()).unwrap();
        assert_eq!(same.schema_version, 2);
        assert_eq!(same.payload, job.payload, "current jobs pass through untouched");
    }

    /// No migrator for the type → identity (unversioned job types keep
    /// working with zero registration).
    #[test]
    fn job_without_migrator_passes_through() {
        let mut registry = JobRegistry::new();
        registry.register(JobDef::new("plain.job")).unwrap();
        let job = scoped_job("plain.job");
        let same = registry.apply_migrations(job.clone()).unwrap();
        assert_eq!(same.schema_version, job.schema_version);
        assert_eq!(same.payload, job.payload);
    }

    /// Fail closed: a row NEWER than the registered schema is refused, not
    /// run blind — the worker takes the normal fail path (retry budget).
    #[test]
    fn job_migration_refuses_newer_than_known() {
        let registry = versioned_registry();
        let mut job = scoped_job("mail.send");
        job.schema_version = 99;
        let err = registry.apply_migrations(job).unwrap_err();
        assert_eq!(
            err.code,
            kelson_foundation::codes::VALIDATION,
            "newer-than-known refuses with VALIDATION"
        );
    }

    /// Migrator registration is guarded: duplicates and version 0 refuse.
    #[test]
    fn job_migrator_registration_is_guarded() {
        let mut registry = versioned_registry();
        assert!(
            registry.register_migrator(Arc::new(LocaleMigrator)).is_err(),
            "duplicate migrator refused"
        );
        struct ZeroMigrator;
        impl JobMigrator for ZeroMigrator {
            fn job_name(&self) -> &str {
                "zero.job"
            }
            fn current_version(&self) -> u32 {
                0
            }
            fn migrate(
                &self,
                _from: u32,
                payload: serde_json::Value,
            ) -> Result<serde_json::Value, AppError> {
                Ok(payload)
            }
        }
        assert!(
            registry.register_migrator(Arc::new(ZeroMigrator)).is_err(),
            "version 0 refused"
        );
    }
}
