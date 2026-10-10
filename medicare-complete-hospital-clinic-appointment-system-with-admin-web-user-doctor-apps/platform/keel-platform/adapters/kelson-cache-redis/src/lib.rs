//! # kelson-cache-redis
//! Redis adapter for the frozen `CachePort` (kelson-caching) and `JobStore`
//! (kelson-jobs) contracts. Sync/blocking redis client only: both ports are
//! sync traits, so async redis machinery would be pure overhead (D-62 slot
//! spent once, no tokio-comp feature).
//!
//! Cache semantics:
//! - `put` -> `SET key value [EX ttl]`; `Some(0)` TTL stores nothing (parity
//!   with the memory adapter's immediate expiry).
//! - `put_if_absent` -> `SET key value NX [EX ttl]` (atomic single round-trip).
//! - `delete_if_equals` -> Lua compare-and-delete (lock-holder safety).
//! - `invalidate_prefix` -> `SCAN MATCH prefix* COUNT 200` batches + `DEL`.
//! - Cache-side failures degrade to a miss / no-op with a warning: the cache
//!   is a speed layer, never a correctness barrier.
//!
//! Job semantics (single Redis instance; cluster out of scope):
//! - item JSON at `{prefix}jobs:item:{id}`; zsets `pending`, `running`,
//!   `done`, `dead` with score = available/claim/finish epoch seconds.
//! - members are `{MAX - priority:020}:{id}` so zset ordering yields
//!   priority DESC, then lex tie-break (id) within one score.
//! - claim = one Lua script (ZRANGEBYSCORE + ZREM + GET + ZADD running):
//!   exactly one worker wins a job (SKIP-LOCKED equivalent).
//! - retry/backoff via `kelson_jobs::backoff_delay` on failure; dead jobs park
//!   in the dead zset until `requeue`.

use std::sync::{Arc, Mutex};

use kelson_caching::CachePort;
use kelson_foundation::{AppError, TenantId};
use kelson_jobs::{
    backoff_delay, DurableJob, JobStatus, JobStore, JobStoreAdmin, QueueSnapshot, TenantJobStore,
    DEFAULT_BACKOFF_BASE_SECS, DEFAULT_BACKOFF_MAX_SECS,
};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

/// Compare-and-delete used by `CacheLock.release`: only the token holder
/// frees the slot, never a re-acquired lock.
const CAS_DEL_SCRIPT: &str = r#"
if redis.call('GET', KEYS[1]) == ARGV[1] then
  return redis.call('DEL', KEYS[1])
else
  return 0
end
"#;

/// Claim one due job atomically: pop the best pending member, move it to
/// running, and return the item JSON. KEYS: pending zset, item prefix,
/// running zset. ARGV: now-score. Returns raw JSON or nil.
const CLAIM_SCRIPT: &str = r#"
local m = redis.call('ZRANGEBYSCORE', KEYS[1], '-inf', ARGV[1], 'LIMIT', 0, 1)
if #m == 0 then return nil end
m = m[1]
redis.call('ZREM', KEYS[1], m)
local id = string.sub(m, 22)
local raw = redis.call('GET', KEYS[2] .. id)
if not raw then return nil end
redis.call('ZADD', KEYS[3], ARGV[1], m)
return raw
"#;

/// Enqueue with duplicate-id guard: KEYS: item key, pending zset. ARGV:
/// json, score, member. Returns 1 on insert, 0 when the id already exists.
const ENQUEUE_SCRIPT: &str = r#"
if redis.call('EXISTS', KEYS[1]) == 1 then return 0 end
redis.call('SET', KEYS[1], ARGV[1])
redis.call('ZADD', KEYS[2], ARGV[2], ARGV[3])
return 1
"#;

/// Claim one due job FOR A TENANT atomically (target-arch §3.1): scan the
/// best due candidates (up to a batch), skip every item not owned by
/// ARGV[2], claim the first match and leave all others exactly where they
/// are — never consumed, never reordered. Tenant-less (platform) and
/// corrupt items are skipped and stay claimable through the physical
/// `CLAIM_SCRIPT`. KEYS: pending zset, item prefix, running zset. ARGV:
/// now-score, tenant uuid. Returns raw JSON or nil.
const CLAIM_TENANT_SCRIPT: &str = r#"
local ms = redis.call('ZRANGEBYSCORE', KEYS[1], '-inf', ARGV[1], 'LIMIT', 0, 32)
for i = 1, #ms do
  local m = ms[i]
  local id = string.sub(m, 22)
  local raw = redis.call('GET', KEYS[2] .. id)
  if raw then
    local ok, job = pcall(cjson.decode, raw)
    if ok and job.tenant_id == ARGV[2] then
      redis.call('ZREM', KEYS[1], m)
      redis.call('ZADD', KEYS[3], ARGV[1], m)
      return raw
    end
  end
end
return nil
"#;

// -------------------------------------------------------------- connection

/// Shared blocking connection with one reconnect-and-retry. Both ports are
/// sync and the adapter serializes commands per instance: fine for cache and
/// control-plane queue rates; a connection pool belongs in a later revision.
pub struct SharedConn {
    client: redis::Client,
    conn: Mutex<Option<redis::Connection>>,
}

impl SharedConn {
    /// Parse the URL (no I/O). Call [`SharedConn::probe`] to verify reachability.
    pub fn new(url: &str) -> Result<Arc<Self>, AppError> {
        let client = redis::Client::open(url)
            .map_err(|e| AppError::persistence(format!("redis url: {e}")))?;
        Ok(Arc::new(Self {
            client,
            conn: Mutex::new(None),
        }))
    }

    /// Verify connectivity (PING).
    pub fn probe(&self) -> Result<(), AppError> {
        self.with(|c| redis::cmd("PING").query::<String>(c))
            .map(|_| ())
    }

    /// Run a command with the shared connection; on any error, drop the
    /// connection, reconnect once and retry. Deterministic command errors
    /// simply fail twice.
    pub fn with<R>(&self, mut f: impl FnMut(&mut redis::Connection) -> redis::RedisResult<R>) -> Result<R, AppError> {
        let mut guard = self.conn.lock().expect("redis conn poisoned");
        if guard.is_none() {
            let conn = self
                .client
                .get_connection()
                .map_err(|e| AppError::persistence(format!("redis connect: {e}")))?;
            *guard = Some(conn);
        }
        let conn = guard.as_mut().expect("connection present");
        match f(conn) {
            Ok(v) => Ok(v),
            Err(first) => {
                tracing::warn!(error = %first, "redis command failed; reconnecting once");
                *guard = None;
                let conn = self
                    .client
                    .get_connection()
                    .map_err(|e| AppError::persistence(format!("redis reconnect: {e}")))?;
                *guard = Some(conn);
                let conn = guard.as_mut().expect("connection present");
                f(conn).map_err(|e| AppError::persistence(format!("redis: {e}")))
            }
        }
    }
}

// ------------------------------------------------------------- cache port

/// Redis-backed [`CachePort`]. Keys are namespaced with `prefix` (default
/// `kelson:`) so multiple apps can share one database safely.
pub struct RedisCache {
    conn: Arc<SharedConn>,
    prefix: String,
    cas: redis::Script,
}

impl RedisCache {
    pub fn connect(url: &str) -> Result<Self, AppError> {
        let conn = SharedConn::new(url)?;
        conn.probe()?;
        Ok(Self::from_conn(conn))
    }

    pub fn from_conn(conn: Arc<SharedConn>) -> Self {
        Self {
            conn,
            prefix: "kelson:".to_string(),
            cas: redis::Script::new(CAS_DEL_SCRIPT),
        }
    }

    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    fn key(&self, key: &str) -> String {
        format!("{}{}", self.prefix, key)
    }

    /// Escape redis-glob characters in a prefix before SCAN MATCH.
    pub fn escape_glob(prefix: &str) -> String {
        let mut out = String::with_capacity(prefix.len());
        for ch in prefix.chars() {
            match ch {
                '*' | '?' | '[' | ']' => {
                    out.push('[');
                    out.push(ch);
                    out.push(']');
                }
                _ => out.push(ch),
            }
        }
        out
    }
}

impl CachePort for RedisCache {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        let key = self.key(key);
        match self
            .conn
            .with(|c| redis::cmd("GET").arg(&key).query::<Option<Vec<u8>>>(c))
        {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e.public_message, "redis cache get degraded to miss");
                None
            }
        }
    }

    fn put(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) {
        if ttl_secs == Some(0) {
            return; // immediate expiry: nothing to store (memory parity)
        }
        let key = self.key(key);
        let res = self.conn.with(|c| {
            let mut cmd = redis::cmd("SET");
            cmd.arg(&key).arg(&value);
            if let Some(ttl) = ttl_secs {
                cmd.arg("EX").arg(ttl);
            }
            cmd.query::<()>(c)
        });
        if let Err(e) = res {
            tracing::warn!(error = %e.public_message, "redis cache put failed");
        }
    }

    fn delete(&self, key: &str) {
        let key = self.key(key);
        let _ = self.conn.with(|c| redis::cmd("DEL").arg(&key).query::<u64>(c));
    }

    fn invalidate_prefix(&self, prefix: &str) {
        let pattern = format!("{}{}*", self.prefix, Self::escape_glob(prefix));
        let mut cursor: u64 = 0;
        loop {
            let step = self.conn.with(|c| {
                let mut cmd = redis::cmd("SCAN");
                cmd.arg(cursor)
                    .arg("MATCH")
                    .arg(&pattern)
                    .arg("COUNT")
                    .arg(200);
                cmd.query::<(u64, Vec<String>)>(c)
            });
            let (next, keys) = match step {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %e.public_message, "redis prefix scan aborted");
                    return;
                }
            };
            if !keys.is_empty() {
                let res =
                    self.conn
                        .with(|c| redis::cmd("DEL").arg(&keys).query::<u64>(c));
                if let Err(e) = res {
                    tracing::warn!(error = %e.public_message, "redis prefix delete failed");
                }
            }
            cursor = next;
            if cursor == 0 {
                break;
            }
        }
    }

    fn put_if_absent(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) -> bool {
        if ttl_secs == Some(0) {
            return false;
        }
        let key = self.key(key);
        match self.conn.with(|c| {
            let mut cmd = redis::cmd("SET");
            cmd.arg(&key).arg(&value).arg("NX");
            if let Some(ttl) = ttl_secs {
                cmd.arg("EX").arg(ttl);
            }
            cmd.query::<Option<String>>(c)
        }) {
            Ok(v) => v.is_some(),
            Err(e) => {
                tracing::warn!(error = %e.public_message, "redis put_if_absent failed; refusing acquisition");
                false
            }
        }
    }

    fn delete_if_equals(&self, key: &str, expected: &[u8]) -> bool {
        let key = self.key(key);
        match self.conn.with(|c| {
            self.cas
                .key(&key)
                .arg(expected.to_vec())
                .invoke::<i64>(c)
        }) {
            Ok(v) => v == 1,
            Err(e) => {
                tracing::warn!(error = %e.public_message, "redis CAS delete failed; refusing release");
                false
            }
        }
    }
}
// -------------------------------------------------------------- job store

/// Durable Redis-backed [`JobStore`] + [`JobStoreAdmin`]. Claim exclusivity
/// is provided by `CLAIM_SCRIPT` (ZREM inside one Lua round-trip), so any
/// number of workers can share one Redis without double-claims.
pub struct RedisJobStore {
    conn: Arc<SharedConn>,
    prefix: String,
    claim: redis::Script,
    claim_tenant: redis::Script,
    enqueue_script: redis::Script,
}

impl RedisJobStore {
    pub fn connect(url: &str) -> Result<Self, AppError> {
        let conn = SharedConn::new(url)?;
        conn.probe()?;
        Ok(Self::from_conn(conn))
    }

    pub fn from_conn(conn: Arc<SharedConn>) -> Self {
        Self {
            conn,
            prefix: "kelson:".to_string(),
            claim: redis::Script::new(CLAIM_SCRIPT),
            claim_tenant: redis::Script::new(CLAIM_TENANT_SCRIPT),
            enqueue_script: redis::Script::new(ENQUEUE_SCRIPT),
        }
    }

    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    fn item_key(&self, id: Uuid) -> String {
        format!("{}jobs:item:{id}", self.prefix)
    }

    fn zset(&self, which: &str) -> String {
        format!("{}jobs:{which}", self.prefix)
    }

    fn item_prefix(&self) -> String {
        format!("{}jobs:item:", self.prefix)
    }
}

/// ZSET member: priority DESC via zero-padded (MAX - priority), then id.
/// The fixed 21-char prefix (`20 digits + ':'`) is load-bearing for
/// CLAIM_SCRIPT's `string.sub(m, 22)`.
fn member(priority: i32, id: Uuid) -> String {
    let inverted = i64::MAX - priority as i64;
    format!("{inverted:020}:{id}")
}

fn id_from_member(member: &str) -> Option<Uuid> {
    member.split_once(':').and_then(|(_, raw)| raw.parse().ok())
}

fn epoch_score(t: OffsetDateTime) -> String {
    format!("{:.3}", t.unix_timestamp_nanos() as f64 / 1e9)
}

fn now() -> OffsetDateTime {
    OffsetDateTime::now_utc()
}

impl RedisJobStore {
    fn read_item(&self, id: Uuid) -> Result<Option<DurableJob>, AppError> {
        let key = self.item_key(id);
        let raw = self
            .conn
            .with(|c| redis::cmd("GET").arg(&key).query::<Option<String>>(c))?;
        match raw {
            Some(raw) => serde_json::from_str(&raw)
                .map(Some)
                .map_err(|e| AppError::persistence(format!("job payload: {e}"))),
            None => Ok(None),
        }
    }

    fn write_item(&self, job: &DurableJob) -> Result<(), AppError> {
        let raw = serde_json::to_string(job)
            .map_err(|e| AppError::persistence(format!("job serialize: {e}")))?;
        self.conn
            .with(|c| redis::cmd("SET").arg(self.item_key(job.id)).arg(&raw).query::<()>(c))
    }

    fn list_from(&self, zset: &str, limit: usize) -> Result<Vec<DurableJob>, AppError> {
        let members = self.conn.with(|c| {
            redis::cmd("ZRANGEBYSCORE")
                .arg(zset)
                .arg("-inf")
                .arg("+inf")
                .arg("LIMIT")
                .arg(0)
                .arg(limit.max(1))
                .query::<Vec<String>>(c)
        })?;
        let mut out = Vec::new();
        for m in members {
            let Some(id) = id_from_member(&m) else { continue };
            if let Some(job) = self.read_item(id)? {
                out.push(job);
            }
        }
        Ok(out)
    }

    /// Queue maintenance (purge/prune/forget): drop a job's item and its
    /// membership in the given zset.
    fn drop_job(&self, job: &DurableJob, which: &str) -> Result<(), AppError> {
        let m = member(job.priority, job.id);
        self.conn.with(|c| {
            redis::cmd("ZREM")
                .arg(self.zset(which))
                .arg(&m)
                .query::<u64>(c)
        })?;
        self.conn
            .with(|c| redis::cmd("DEL").arg(self.item_key(job.id)).query::<u64>(c))?;
        Ok(())
    }
}

impl JobStore for RedisJobStore {
    fn enqueue(&self, job: DurableJob) -> Result<Uuid, AppError> {
        if job.status != JobStatus::Pending {
            return Err(AppError::validation("new jobs must be pending"));
        }
        let id = job.id;
        let raw = serde_json::to_string(&job)
            .map_err(|e| AppError::persistence(format!("job serialize: {e}")))?;
        let m = member(job.priority, id);
        let inserted = self.conn.with(|c| {
            self.enqueue_script
                .key(self.item_key(id))
                .key(self.zset("pending"))
                .arg(&raw)
                .arg(epoch_score(job.available_at))
                .arg(&m)
                .invoke::<i64>(c)
        })?;
        if inserted == 1 {
            Ok(id)
        } else {
            Err(AppError::conflict(format!("job {id} already enqueued")))
        }
    }

    fn claim(&self, worker: &str) -> Result<Option<DurableJob>, AppError> {
        let at = now();
        let raw = self.conn.with(|c| {
            self.claim
                .key(self.zset("pending"))
                .key(self.item_prefix())
                .key(self.zset("running"))
                .arg(epoch_score(at))
                .invoke::<Option<String>>(c)
        })?;
        let Some(raw) = raw else { return Ok(None) };
        let mut job: DurableJob = serde_json::from_str(&raw)
            .map_err(|e| AppError::persistence(format!("job payload: {e}")))?;
        job.status = JobStatus::Running;
        job.attempt += 1;
        job.claimed_by = Some(worker.to_string());
        job.claimed_at = Some(at);
        self.write_item(&job)?;
        Ok(Some(job))
    }

    fn complete(&self, id: Uuid) -> Result<(), AppError> {
        self.complete_impl(None, id)
    }

    fn fail(&self, id: Uuid, error: &str) -> Result<JobStatus, AppError> {
        self.fail_impl(None, id, error)
    }
}

impl RedisJobStore {
    /// Shared complete/fail with tenant visibility: `None` = the platform
    /// view; `Some` = the scoped port, where a cross-tenant id is INVISIBLE
    /// (`NOT_FOUND`) — never a mutation, never a conflict oracle that leaks
    /// existence.
    fn complete_impl(&self, tenant: Option<TenantId>, id: Uuid) -> Result<(), AppError> {
        let mut job = self
            .read_item(id)?
            .ok_or_else(|| AppError::not_found(format!("job {id}")))?;
        if tenant.is_some_and(|t| job.tenant_id != Some(t)) {
            return Err(AppError::not_found(format!("job {id}")));
        }
        if job.status != JobStatus::Running {
            return Err(AppError::conflict("complete requires a running job"));
        }
        job.status = JobStatus::Succeeded;
        job.finished_at = Some(now());
        let m = member(job.priority, id);
        self.write_item(&job)?;
        self.conn.with(|c| {
            redis::cmd("ZREM").arg(self.zset("running")).arg(&m).query::<u64>(c)
        })?;
        self.conn.with(|c| {
            redis::cmd("ZADD")
                .arg(self.zset("done"))
                .arg(epoch_score(now()))
                .arg(&m)
                .query::<u64>(c)
        })?;
        Ok(())
    }

    fn fail_impl(
        &self,
        tenant: Option<TenantId>,
        id: Uuid,
        error: &str,
    ) -> Result<JobStatus, AppError> {
        let mut job = self
            .read_item(id)?
            .ok_or_else(|| AppError::not_found(format!("job {id}")))?;
        if tenant.is_some_and(|t| job.tenant_id != Some(t)) {
            return Err(AppError::not_found(format!("job {id}")));
        }
        if job.status != JobStatus::Running {
            return Err(AppError::conflict("fail requires a running job"));
        }
        job.last_error = Some(error.to_string());
        let next = job.next_status_after_failure();
        job.status = next;
        let m = member(job.priority, id);
        let at = now();
        match next {
            JobStatus::Pending => {
                let delay = backoff_delay(
                    job.attempt,
                    Duration::seconds(DEFAULT_BACKOFF_BASE_SECS),
                    Duration::seconds(DEFAULT_BACKOFF_MAX_SECS),
                );
                job.available_at = at + delay;
                job.claimed_by = None;
                job.claimed_at = None;
            }
            JobStatus::Dead => {
                job.finished_at = Some(at);
            }
            other => {
                return Err(AppError::internal(format!(
                    "unexpected post-failure status {other:?}"
                )))
            }
        }
        self.write_item(&job)?;
        self.conn
            .with(|c| redis::cmd("ZREM").arg(self.zset("running")).arg(&m).query::<u64>(c))?;
        let target = if next == JobStatus::Pending { "pending" } else { "dead" };
        self.conn.with(|c| {
            redis::cmd("ZADD")
                .arg(self.zset(target))
                .arg(epoch_score(job.available_at))
                .arg(&m)
                .query::<u64>(c)
        })?;
        Ok(next)
    }
}

/// §3.1 tenant-scoped queue port over Redis — see [`kelson_jobs::TenantJobStore`]:
/// enqueue stamps the item JSON, claim runs `CLAIM_TENANT_SCRIPT`
/// (tenant-filtered, atomic), complete/fail are invisible across tenants.
impl TenantJobStore for RedisJobStore {
    fn enqueue_for(&self, tenant: TenantId, mut job: DurableJob) -> Result<Uuid, AppError> {
        if matches!(job.tenant_id, Some(set) if set != tenant) {
            return Err(AppError::validation(
                "job tenant does not match the enqueue tenant",
            ));
        }
        job.tenant_id = Some(tenant);
        // The item JSON carries the tenant, so the stamp round-trips.
        self.enqueue(job)
    }

    fn claim_for(&self, tenant: TenantId, worker: &str) -> Result<Option<DurableJob>, AppError> {
        let at = now();
        let raw = self.conn.with(|c| {
            self.claim_tenant
                .key(self.zset("pending"))
                .key(self.item_prefix())
                .key(self.zset("running"))
                .arg(epoch_score(at))
                .arg(tenant.to_string())
                .invoke::<Option<String>>(c)
        })?;
        let Some(raw) = raw else { return Ok(None) };
        let mut job: DurableJob = serde_json::from_str(&raw)
            .map_err(|e| AppError::persistence(format!("job payload: {e}")))?;
        job.status = JobStatus::Running;
        job.attempt += 1;
        job.claimed_by = Some(worker.to_string());
        job.claimed_at = Some(at);
        self.write_item(&job)?;
        Ok(Some(job))
    }

    fn complete_for(&self, tenant: TenantId, id: Uuid) -> Result<(), AppError> {
        self.complete_impl(Some(tenant), id)
    }

    fn fail_for(&self, tenant: TenantId, id: Uuid, error: &str) -> Result<JobStatus, AppError> {
        self.fail_impl(Some(tenant), id, error)
    }
}

impl JobStoreAdmin for RedisJobStore {
    fn list(&self, status: Option<JobStatus>, limit: usize) -> Result<Vec<DurableJob>, AppError> {
        let zsets: Vec<&str> = match status {
            Some(JobStatus::Pending) => vec!["pending"],
            Some(JobStatus::Running) => vec!["running"],
            Some(JobStatus::Succeeded) => vec!["done"],
            Some(JobStatus::Dead) => vec!["dead"],
            // Failed lives between claim and next fail() call; nothing rests here.
            Some(JobStatus::Failed) => vec![],
            None => vec!["pending", "running", "done", "dead"],
        };
        let mut out = Vec::new();
        for z in zsets {
            out.extend(self.list_from(&self.zset(z), limit)?);
        }
        out.sort_by(|a, b| b.enqueued_at.cmp(&a.enqueued_at));
        out.truncate(limit);
        Ok(out)
    }

    fn get(&self, id: Uuid) -> Result<Option<DurableJob>, AppError> {
        self.read_item(id)
    }

    fn requeue(&self, id: Uuid) -> Result<(), AppError> {
        let mut job = self
            .read_item(id)?
            .ok_or_else(|| AppError::not_found(format!("job {id}")))?;
        if !matches!(job.status, JobStatus::Failed | JobStatus::Dead) {
            return Err(AppError::conflict(
                "only failed or dead jobs can be requeued",
            ));
        }
        job.status = JobStatus::Pending;
        job.available_at = now();
        job.claimed_by = None;
        job.claimed_at = None;
        job.finished_at = None;
        let m = member(job.priority, id);
        self.write_item(&job)?;
        self.conn
            .with(|c| redis::cmd("ZREM").arg(self.zset("dead")).arg(&m).query::<u64>(c))?;
        self.conn.with(|c| {
            redis::cmd("ZADD")
                .arg(self.zset("pending"))
                .arg(epoch_score(job.available_at))
                .arg(&m)
                .query::<u64>(c)
        })?;
        Ok(())
    }

    fn purge_pending(&self) -> Result<u64, AppError> {
        let jobs = self.list_from(&self.zset("pending"), usize::MAX)?;
        let mut purged = 0u64;
        for job in jobs {
            self.drop_job(&job, "pending")?;
            purged += 1;
        }
        Ok(purged)
    }

    fn prune_dead(&self, before: OffsetDateTime) -> Result<u64, AppError> {
        let jobs = self.list_from(&self.zset("dead"), usize::MAX)?;
        let mut pruned = 0u64;
        for job in jobs {
            // Same strictly-before contract as the pg adapter: the job "ended"
            // when it was marked dead, falling back to its enqueue time.
            if job.finished_at.unwrap_or(job.enqueued_at) < before {
                self.drop_job(&job, "dead")?;
                pruned += 1;
            }
        }
        Ok(pruned)
    }

    fn delete_dead(&self, id: Uuid) -> Result<bool, AppError> {
        let Some(job) = self.read_item(id)? else {
            return Ok(false);
        };
        if job.status != JobStatus::Dead {
            return Ok(false);
        }
        self.drop_job(&job, "dead")?;
        Ok(true)
    }

    fn snapshot(&self) -> Result<QueueSnapshot, AppError> {
        let card = |which: &str| -> Result<usize, AppError> {
            let n: u64 = self
                .conn
                .with(|c| redis::cmd("ZCARD").arg(self.zset(which)).query::<u64>(c))?;
            Ok(n as usize)
        };
        let pending_jobs = self.list_from(&self.zset("pending"), usize::MAX)?;
        Ok(QueueSnapshot {
            pending: card("pending")?,
            running: card("running")?,
            dead: card("dead")?,
            succeeded: card("done")?,
            oldest_pending: pending_jobs.iter().map(|j| j.enqueued_at).min(),
        })
    }

    fn defer(&self, id: Uuid, available_at: OffsetDateTime) -> Result<(), AppError> {
        let mut job = self
            .read_item(id)?
            .ok_or_else(|| AppError::not_found(format!("job {id}")))?;
        if job.status != JobStatus::Running {
            return Err(AppError::conflict(
                "job is not running; only a claimed job can be deferred",
            ));
        }
        job.status = JobStatus::Pending;
        // The claim consumed an attempt; a Hold gives it back (P8).
        job.attempt = job.attempt.saturating_sub(1);
        job.available_at = available_at;
        job.claimed_by = None;
        job.claimed_at = None;
        let m = member(job.priority, id);
        self.write_item(&job)?;
        self.conn.with(|c| {
            redis::cmd("ZREM")
                .arg(self.zset("running"))
                .arg(&m)
                .query::<u64>(c)
        })?;
        self.conn.with(|c| {
            redis::cmd("ZADD")
                .arg(self.zset("pending"))
                .arg(epoch_score(available_at))
                .arg(&m)
                .query::<u64>(c)
        })?;
        Ok(())
    }

    fn kill(&self, id: Uuid, error: &str) -> Result<(), AppError> {
        let mut job = self
            .read_item(id)?
            .ok_or_else(|| AppError::not_found(format!("job {id}")))?;
        if !matches!(
            job.status,
            JobStatus::Pending | JobStatus::Running | JobStatus::Failed
        ) {
            return Err(AppError::conflict(
                "job is already terminal; only pending, running or failed jobs can be killed",
            ));
        }
        // Failed never rests in a zset (the claim/fail comments); pending
        // and running do.
        let source = match job.status {
            JobStatus::Pending => Some("pending"),
            JobStatus::Running => Some("running"),
            _ => None,
        };
        job.status = JobStatus::Dead;
        job.last_error = Some(error.to_string());
        job.finished_at = Some(now());
        let m = member(job.priority, id);
        self.write_item(&job)?;
        if let Some(which) = source {
            self.conn.with(|c| {
                redis::cmd("ZREM")
                    .arg(self.zset(which))
                    .arg(&m)
                    .query::<u64>(c)
            })?;
        }
        self.conn.with(|c| {
            redis::cmd("ZADD")
                .arg(self.zset("dead"))
                .arg(epoch_score(job.available_at))
                .arg(&m)
                .query::<u64>(c)
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kelson_jobs::{build_from_def, JobDef};

    fn sample_job(priority: i32, max_attempts: u32) -> DurableJob {
        let mut def = JobDef::new("test.job");
        def.max_attempts = max_attempts;
        build_from_def(&def, serde_json::json!({"k": priority}), now())
    }

    #[test]
    fn member_orders_priority_desc_and_round_trips_id() {
        let id = Uuid::now_v7();
        let hi = member(10, id);
        let lo = member(1, id);
        assert!(hi < lo, "higher priority sorts first (lexicographically)");
        assert_eq!(id_from_member(&hi), Some(id));
        assert_eq!(&hi[20..21], ":", "fixed 21-char prefix for Lua substr");
    }

    #[test]
    fn epoch_score_is_monotonic() {
        let a = epoch_score(OffsetDateTime::UNIX_EPOCH);
        let b = epoch_score(OffsetDateTime::UNIX_EPOCH + Duration::seconds(1));
        assert!(b > a);
        assert!(a.starts_with("0.000"));
    }

    #[test]
    fn glob_prefix_is_escaped() {
        assert_eq!(RedisCache::escape_glob("authz:*"), "authz:[*]");
        assert_eq!(RedisCache::escape_glob("plain:"), "plain:");
        assert_eq!(RedisCache::escape_glob("a[0]b?"), "a[[]0[]]b[?]");
    }

    #[test]
    fn durable_job_round_trips_through_json() {
        let mut job = sample_job(3, 5);
        job.priority = 3;
        let raw = serde_json::to_string(&job).unwrap();
        let back: DurableJob = serde_json::from_str(&raw).unwrap();
        assert_eq!(back.id, job.id);
        assert_eq!(back.priority, 3);
        assert_eq!(back.max_attempts, 5);
    }
}