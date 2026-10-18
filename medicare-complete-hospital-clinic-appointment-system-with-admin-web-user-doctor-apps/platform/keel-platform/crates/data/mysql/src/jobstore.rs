//! Durable job queue on MySQL 8 (S2 item 22, B-31 second driver): SKIP
//! LOCKED claiming, so N workers pull concurrently without coordination.
//! Backoff on failure is applied by updating available_at (exponential,
//! 30s base, 10min cap).
//!
//! The JobStore contract is sync (frozen, STAGE-0-SPEC section 5); MySQL
//! calls bridge through crate::bridge (block_in_place inside multi-threaded
//! runtimes, a persistent bridge runtime otherwise).
//!
//! MySQL port notes (every divergence from `kelson_postgres::jobstore`):
//!
//! - `FOR UPDATE SKIP LOCKED` is MySQL 8 native and stays verbatim, but the
//!   pg one-liner cannot be carried over as written: MySQL rejects an UPDATE
//!   that reads its own target table in a subquery (error 1093) and has no
//!   `RETURNING *`. `claim` therefore runs three statements on ONE
//!   connection inside one transaction: the original candidate read
//!   (`SELECT ... LIMIT 1 FOR UPDATE SKIP LOCKED`), the claim `UPDATE` by id
//!   (which repeats `AND status = 'pending'`, already guaranteed by the
//!   locked candidate read, so the rows_affected check is a real guard), then
//!   `SELECT * FROM jobs_queue WHERE id = ?` -- the exact row pg hands back
//!   from RETURNING -- before committing. "Nothing to claim" is the candidate
//!   read coming back empty (pg: the RETURNING UPDATE matched no row); an
//!   UPDATE that matches nothing reports `Ok(None)` as well, so the claimed
//!   vs nothing distinction of the pg port is preserved (CLIENT_FOUND_ROWS
//!   counts matched rows, i.e. a pending candidate always matches).
//! - `fail`'s `RETURNING status` becomes `UPDATE ... WHERE id = ? AND
//!   status = 'running'` (rows_affected() == 0 still means "job is not
//!   running") followed by `SELECT status WHERE id = ?`, both inside one
//!   transaction so the status handed back is a snapshot of this write, like
//!   pg's RETURNING row (a plain pool round-trip could read a row another
//!   worker re-claimed in between).
//! - `now()` -> `UTC_TIMESTAMP(6)`; `now() + make_interval(secs => LEAST(600,
//!   30 * power(2, GREATEST(attempt - 1, 0)))::int)` -> `UTC_TIMESTAMP(6) +
//!   INTERVAL CAST(LEAST(600, 30 * POW(2, GREATEST(attempt - 1, 0))) AS
//!   SIGNED) SECOND`, so the `::int` cast survives as `CAST(... AS SIGNED)`
//!   and the interval expression stays an integer. (Theoretical edge: past
//!   attempt ~1024 `POW` overflows to NULL on MySQL where pg's LEAST would
//!   still cap at 600; unreachable while attempt < max_attempts is small.)
//!   `DATETIME` carries no timezone, so the store is explicit about UTC
//!   rather than leaning on the connection default: sqlx pins MySQL sessions
//!   to `time_zone='+00:00'` today (sqlx-mysql `MySqlConnectOptions` default),
//!   but a column default (`DEFAULT CURRENT_TIMESTAMP(6)`) or any non-sqlx
//!   writer would otherwise store SERVER-LOCAL wall clock and be read back
//!   five-and-a-half hours off on a non-UTC server.
//! - P0 crash recovery: `claim` also reclaims `running` rows whose claim is
//!   older than [`kelson_jobs::DEFAULT_CLAIM_LEASE_SECS`] (a crashed worker
//!   left them running forever) and dead-letters abandoned claims that have
//!   spent their retry budget. The epoch handed back is `attempt`, which
//!   `complete_claim`/`fail_claim` require on the way out, so a stalled
//!   zombie owner is fenced from finishing a job another worker reclaimed.
//! - `COUNT(*) FILTER (WHERE ...)` -> `COUNT(CASE WHEN ... THEN 1 END)` and
//!   `MIN(enqueued_at) FILTER (WHERE ...)` -> `MIN(CASE WHEN ... THEN
//!   enqueued_at END)`: COUNT stays 0 on an empty group, MIN stays NULL, so
//!   the QueueSnapshot row shape is unchanged.
//! - `($1::text IS NULL OR status = $1)` keeps its SQL text, but MySQL
//!   placeholders are positional by appearance, so the status value is bound
//!   twice (once per `?`).
//! - `COALESCE(finished_at, enqueued_at)`, `GREATEST`, `CASE` and `LIMIT ?`
//!   are used as in pg. Every rows_affected() branch (complete, requeue,
//!   defer, kill, purge, prune, delete) was audited: sqlx requests
//!   CLIENT_FOUND_ROWS so UPDATE counts matched rows -- identical to pg,
//!   because each of those WHERE clauses guards on a status predicate whose
//!   match implies the very change pg makes (and DELETE counts deleted rows
//!   on both sides).

use kelson_foundation::{AppError, TenantId};
use kelson_jobs::{DurableJob, JobStatus, JobStore, JobStoreAdmin, TenantJobStore};
use sqlx::mysql::MySqlPool;

use crate::bridge::block_on;
use sqlx::Row;
use uuid::Uuid;

pub struct MySqlJobStore {
    pool: MySqlPool,
}

impl MySqlJobStore {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
}

fn status_text(status: JobStatus) -> &'static str {
    match status {
        JobStatus::Pending => "pending",
        JobStatus::Running => "running",
        JobStatus::Succeeded => "succeeded",
        JobStatus::Failed => "failed",
        JobStatus::Dead => "dead",
    }
}

fn status_from(text: &str) -> Result<JobStatus, AppError> {
    Ok(match text {
        "pending" => JobStatus::Pending,
        "running" => JobStatus::Running,
        "succeeded" => JobStatus::Succeeded,
        "failed" => JobStatus::Failed,
        "dead" => JobStatus::Dead,
        other => return Err(AppError::internal(format!("unknown job status {other}"))),
    })
}

fn job_from_row(row: sqlx::mysql::MySqlRow) -> Result<DurableJob, AppError> {
    let causation: Option<Uuid> = row.try_get("causation_id").map_err(my)?;
    let correlation: Option<Uuid> = row.try_get("correlation_id").map_err(my)?;
    Ok(DurableJob {
        id: row.try_get("id").map_err(my)?,
        execution_id: kelson_foundation::ExecutionId::from_uuid(row.try_get("execution_id").map_err(my)?),
        name: row.try_get("name").map_err(my)?,
        payload: row.try_get("payload").map_err(my)?,
        queue: row.try_get("queue").map_err(my)?,
        attempt: row.try_get::<i32, _>("attempt").map_err(my)? as u32,
        max_attempts: row.try_get::<i32, _>("max_attempts").map_err(my)? as u32,
        deadline: row.try_get("deadline").map_err(my)?,
        available_at: row.try_get("available_at").map_err(my)?,
        status: status_from(&row.try_get::<String, _>("status").map_err(my)?)?,
        priority: row.try_get("priority").map_err(my)?,
        claimed_by: row.try_get("claimed_by").map_err(my)?,
        claimed_at: row.try_get("claimed_at").map_err(my)?,
        causation_id: causation.map(kelson_foundation::CorrelationId::from_uuid),
        correlation_id: correlation.map(kelson_foundation::CorrelationId::from_uuid),
        last_error: row.try_get("last_error").map_err(my)?,
        enqueued_at: row.try_get("enqueued_at").map_err(my)?,
        finished_at: row.try_get("finished_at").map_err(my)?,
        tenant_id: row
            .try_get::<Option<Uuid>, _>("tenant_id")
            .map_err(my)?
            .map(kelson_foundation::TenantId::from_uuid),
        // §5.1: pre-Phase-3 rows have no column (NULL) → v1, so every
        // in-flight job loads with zero deserialization errors.
        schema_version: row
            .try_get::<Option<i32>, _>("schema_version")
            .map_err(my)?
            .map(|v| v as u32)
            .unwrap_or(1),
    })
}

fn my(e: impl std::fmt::Display) -> AppError {
    AppError::persistence(format!("jobs_queue: {e}"))
}

impl JobStore for MySqlJobStore {
    fn enqueue(&self, job: DurableJob) -> Result<Uuid, AppError> {
        if job.status != JobStatus::Pending {
            return Err(AppError::validation("new jobs must be pending"));
        }
        let id = job.id;
        let pool = self.pool.clone();
        block_on(async move {
            sqlx::query(
                "INSERT INTO jobs_queue
                 (id, execution_id, name, payload, queue, attempt, max_attempts, deadline,
                  available_at, status, priority, causation_id, correlation_id, last_error,
                  enqueued_at, finished_at, tenant_id, schema_version)
                 VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            )
            .bind(id)
            .bind(*job.execution_id.as_uuid())
            .bind(&job.name)
            .bind(&job.payload)
            .bind(&job.queue)
            .bind(job.attempt as i32)
            .bind(job.max_attempts as i32)
            .bind(job.deadline)
            .bind(job.available_at)
            .bind(status_text(job.status))
            .bind(job.priority)
            .bind(job.causation_id.map(|c| *c.as_uuid()))
            .bind(job.correlation_id.map(|c| *c.as_uuid()))
            .bind(&job.last_error)
            .bind(job.enqueued_at)
            .bind(job.finished_at)
            .bind(job.tenant_id.map(|t| *t.as_uuid()))
            .bind(job.schema_version as i32)
            .execute(&pool)
            .await
            .map_err(my)?;
            Ok::<_, AppError>(id)
        })
    }

    fn claim(&self, worker: &str) -> Result<Option<DurableJob>, AppError> {
        let worker = worker.to_string();
        let pool = self.pool.clone();
        block_on(async move {
            // One connection, one transaction: the candidate lock read has to
            // outlive the UPDATE it feeds (autocommit would drop the row lock
            // between the two statements and two workers could claim twice).
            //
            // MySQL divergence, measured live against 8.0.44
            // (REPEATABLE-READ): the one-statement
            // `ORDER BY ... LIMIT 1 FOR UPDATE SKIP LOCKED` locks EVERY row
            // the sort reads, not only the row it returns — the filesort has
            // to consume all candidates before LIMIT can stop it — so a second
            // worker running the claim concurrently gets ZERO rows back while
            // pending work still exists (starvation). The pg store issues the
            // same shape of query and is deliberately untouched here: only the
            // MySQL behaviour above was measured (no pg instance is available
            // in this environment), and this fix is scoped to MySQL.
            // Removing ORDER BY alone fixes the starvation, but it would
            // silently drop priority ordering.
            //
            // Instead: read a bounded batch of candidates UNLOCKED in
            // priority order, then lock them one at a time by primary key.
            // Locking a single row by PK cannot sweep up its neighbours, so
            // the next worker skips exactly that row and takes the next
            // candidate — priority order is kept and no claimant is starved.
            // P0 crash recovery: the candidate set is now TWO classes. (1) the
            // due `pending` rows, exactly as before; and (2) `running` rows
            // whose claim lease has expired — a worker that crashed, was
            // evicted, or lost its host left them `running` forever, and
            // before this arm existed such a job could neither run again nor
            // be requeued (`requeue` refuses `running`). Reclaiming re-enters
            // the normal path and advances `attempt`, so the retry budget
            // still bounds a job that keeps killing its worker.
            let cutoff = time::OffsetDateTime::now_utc()
                - time::Duration::seconds(kelson_jobs::DEFAULT_CLAIM_LEASE_SECS as i64);
            // Dead-letter abandoned claims with no retries left, so an
            // exhausted orphan stops being invisible: it becomes terminal,
            // countable and prunable instead of sitting `running` forever.
            // This runs in AUTOCOMMIT, before the claim transaction opens: a
            // claim that finds no candidate rolls its transaction back, and a
            // dead-letter written inside it would be undone with it.
            sqlx::query(
                "UPDATE jobs_queue
                 SET status = 'dead',
                     last_error = COALESCE(last_error, 'claim lease expired; retries exhausted'),
                     finished_at = UTC_TIMESTAMP(6)
                 WHERE status = 'running' AND attempt >= max_attempts
                   AND claimed_at IS NOT NULL AND claimed_at < ?",
            )
            .bind(cutoff)
            .execute(&pool)
            .await
            .map_err(my)?;
            let mut tx = pool.begin().await.map_err(my)?;
            let candidates: Vec<Uuid> = sqlx::query_scalar(
                "SELECT id FROM jobs_queue
                 WHERE (status = 'pending' AND available_at <= UTC_TIMESTAMP(6))
                    OR (status = 'running' AND attempt < max_attempts
                        AND claimed_at IS NOT NULL AND claimed_at < ?)
                 ORDER BY priority DESC, available_at, enqueued_at
                 LIMIT 8",
            )
            .bind(cutoff)
            .fetch_all(&mut *tx)
            .await
            .map_err(my)?;
            let mut claimed: Option<Uuid> = None;
            for candidate in candidates {
                let locked: Option<Uuid> = sqlx::query_scalar(
                    "SELECT id FROM jobs_queue WHERE id = ? FOR UPDATE SKIP LOCKED",
                )
                .bind(candidate)
                .fetch_optional(&mut *tx)
                .await
                .map_err(my)?;
                if locked.is_some() {
                    claimed = Some(candidate);
                    break;
                }
            }
            let Some(id) = claimed else {
                // The queue is empty, or every candidate in the batch is held
                // by a concurrent worker: report the same None pg does on a
                // miss and let the caller ask again.
                tx.rollback().await.map_err(my)?;
                return Ok(None);
            };
            let claimed = sqlx::query(
                "UPDATE jobs_queue
                 SET status = 'running', attempt = attempt + 1,
                     claimed_by = ?, claimed_at = UTC_TIMESTAMP(6)
                 WHERE id = ?
                   AND (status = 'pending'
                        OR (status = 'running' AND attempt < max_attempts
                            AND claimed_at IS NOT NULL AND claimed_at < ?))",
            )
            .bind(&worker)
            .bind(id)
            .bind(cutoff)
            .execute(&mut *tx)
            .await
            .map_err(my)?;
            if claimed.rows_affected() == 0 {
                tx.rollback().await.map_err(my)?;
                return Ok(None);
            }
            // No RETURNING: re-select the claimed row by key on this same
            // connection (the shape pg returned from `UPDATE ... RETURNING *`).
            let row: Option<sqlx::mysql::MySqlRow> =
                sqlx::query("SELECT * FROM jobs_queue WHERE id = ?")
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(my)?;
            tx.commit().await.map_err(my)?;
            row.map(job_from_row).transpose()
        })
    }

    fn complete(&self, id: Uuid) -> Result<(), AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query(
                "UPDATE jobs_queue
                 SET status = 'succeeded', finished_at = UTC_TIMESTAMP(6)
                 WHERE id = ? AND status = 'running'",
            )
            .bind(id)
            .execute(&pool)
            .await
            .map_err(my)?;
            if done.rows_affected() == 0 {
                return Err(AppError::conflict(format!("job {id} is not running")));
            }
            Ok(())
        })
    }

    fn fail(&self, id: Uuid, error: &str) -> Result<JobStatus, AppError> {
        let error = error.to_string();
        let pool = self.pool.clone();
        block_on(async move {
            // Write and status re-read on one connection inside one
            // transaction: the returned status is this write's own result,
            // exactly the row pg's `RETURNING status` produced.
            let mut tx = pool.begin().await.map_err(my)?;
            let done = sqlx::query(
                "UPDATE jobs_queue
                 SET last_error = ?,
                     status = CASE WHEN attempt >= max_attempts THEN 'dead' ELSE 'pending' END,
                     available_at = CASE WHEN attempt >= max_attempts THEN available_at
                                         ELSE UTC_TIMESTAMP(6) + INTERVAL CAST(LEAST(600, 30 * POW(2, GREATEST(attempt - 1, 0))) AS SIGNED) SECOND
                                    END,
                     finished_at = CASE WHEN attempt >= max_attempts THEN UTC_TIMESTAMP(6) ELSE NULL END
                 WHERE id = ? AND status = 'running'",
            )
            .bind(&error)
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(my)?;
            if done.rows_affected() == 0 {
                tx.rollback().await.map_err(my)?;
                return Err(AppError::conflict(format!("job {id} is not running")));
            }
            let (status,): (String,) = sqlx::query_as("SELECT status FROM jobs_queue WHERE id = ?")
                .bind(id)
                .fetch_one(&mut *tx)
                .await
                .map_err(my)?;
            tx.commit().await.map_err(my)?;
            status_from(&status)
        })
    }

    fn complete_claim(&self, id: Uuid, attempt: u32) -> Result<(), AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            // The fence: `attempt` is the epoch the caller was handed at
            // claim time. If the lease expired and another worker reclaimed
            // the row, `attempt` has moved on and this write matches nothing.
            let done = sqlx::query(
                "UPDATE jobs_queue
                 SET status = 'succeeded', finished_at = UTC_TIMESTAMP(6)
                 WHERE id = ? AND status = 'running' AND attempt = ?",
            )
            .bind(id)
            .bind(attempt as i64)
            .execute(&pool)
            .await
            .map_err(my)?;
            if done.rows_affected() == 0 {
                return Err(AppError::conflict(format!(
                    "job {id} claim is stale (attempt {attempt} was reclaimed); completion refused"
                )));
            }
            Ok(())
        })
    }

    fn fail_claim(&self, id: Uuid, attempt: u32, error: &str) -> Result<JobStatus, AppError> {
        let error = error.to_string();
        let pool = self.pool.clone();
        block_on(async move {
            let mut tx = pool.begin().await.map_err(my)?;
            let done = sqlx::query(
                "UPDATE jobs_queue
                 SET last_error = ?,
                     status = CASE WHEN attempt >= max_attempts THEN 'dead' ELSE 'pending' END,
                     available_at = CASE WHEN attempt >= max_attempts THEN available_at
                                         ELSE UTC_TIMESTAMP(6) + INTERVAL CAST(LEAST(600, 30 * POW(2, GREATEST(attempt - 1, 0))) AS SIGNED) SECOND
                                    END,
                     finished_at = CASE WHEN attempt >= max_attempts THEN UTC_TIMESTAMP(6) ELSE NULL END
                 WHERE id = ? AND status = 'running' AND attempt = ?",
            )
            .bind(&error)
            .bind(id)
            .bind(attempt as i64)
            .execute(&mut *tx)
            .await
            .map_err(my)?;
            if done.rows_affected() == 0 {
                tx.rollback().await.map_err(my)?;
                return Err(AppError::conflict(format!(
                    "job {id} claim is stale (attempt {attempt} was reclaimed); failure refused"
                )));
            }
            let (status,): (String,) = sqlx::query_as("SELECT status FROM jobs_queue WHERE id = ?")
                .bind(id)
                .fetch_one(&mut *tx)
                .await
                .map_err(my)?;
            tx.commit().await.map_err(my)?;
            status_from(&status)
        })
    }
}

impl JobStoreAdmin for MySqlJobStore {
    fn list(&self, status: Option<JobStatus>, limit: usize) -> Result<Vec<DurableJob>, AppError> {
        let status = status.map(status_text).map(|s| s.to_string());
        let pool = self.pool.clone();
        block_on(async move {
            // The pg guard binds `$1` twice; MySQL has no numbered
            // placeholders, so the same value is bound once per `?`.
            let rows: Vec<sqlx::mysql::MySqlRow> = sqlx::query(
                "SELECT * FROM jobs_queue
                 WHERE (? IS NULL OR status = ?)
                 ORDER BY enqueued_at DESC
                 LIMIT ?",
            )
            .bind(status.clone())
            .bind(status)
            .bind(limit as i64)
            .fetch_all(&pool)
            .await
            .map_err(my)?;
            rows.into_iter().map(job_from_row).collect()
        })
    }

    fn get(&self, id: Uuid) -> Result<Option<DurableJob>, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let row: Option<sqlx::mysql::MySqlRow> =
                sqlx::query("SELECT * FROM jobs_queue WHERE id = ?")
                    .bind(id)
                    .fetch_optional(&pool)
                    .await
                    .map_err(my)?;
            row.map(job_from_row).transpose()
        })
    }

    fn requeue(&self, id: Uuid) -> Result<(), AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query(
                "UPDATE jobs_queue
                 SET status = 'pending', available_at = UTC_TIMESTAMP(6),
                     claimed_by = NULL, claimed_at = NULL, finished_at = NULL
                 WHERE id = ? AND status IN ('failed', 'dead')",
            )
            .bind(id)
            .execute(&pool)
            .await
            .map_err(my)?;
            if done.rows_affected() == 0 {
                return Err(AppError::conflict(
                    "only failed or dead jobs can be requeued",
                ));
            }
            Ok(())
        })
    }

    fn purge_pending(&self) -> Result<u64, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query("DELETE FROM jobs_queue WHERE status = 'pending'")
                .execute(&pool)
                .await
                .map_err(my)?;
            Ok(done.rows_affected())
        })
    }

    fn prune_dead(&self, before: time::OffsetDateTime) -> Result<u64, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query(
                "DELETE FROM jobs_queue
                 WHERE status = 'dead'
                   AND COALESCE(finished_at, enqueued_at) < ?",
            )
            .bind(before)
            .execute(&pool)
            .await
            .map_err(my)?;
            Ok(done.rows_affected())
        })
    }

    fn delete_dead(&self, id: Uuid) -> Result<bool, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query("DELETE FROM jobs_queue WHERE id = ? AND status = 'dead'")
                .bind(id)
                .execute(&pool)
                .await
                .map_err(my)?;
            Ok(done.rows_affected() == 1)
        })
    }

    fn snapshot(&self) -> Result<kelson_jobs::QueueSnapshot, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let row = sqlx::query(
                "SELECT COUNT(CASE WHEN status = 'pending' THEN 1 END) AS pending,
                        COUNT(CASE WHEN status = 'running' THEN 1 END) AS running,
                        COUNT(CASE WHEN status = 'dead' THEN 1 END) AS dead,
                        COUNT(CASE WHEN status = 'succeeded' THEN 1 END) AS succeeded,
                        MIN(CASE WHEN status = 'pending' THEN enqueued_at END) AS oldest_pending
                   FROM jobs_queue",
            )
            .fetch_one(&pool)
            .await
            .map_err(my)?;
            let count = |col: &str| -> Result<usize, AppError> {
                let n: i64 = row.try_get(col).map_err(my)?;
                Ok(n.max(0) as usize)
            };
            Ok(kelson_jobs::QueueSnapshot {
                pending: count("pending")?,
                running: count("running")?,
                dead: count("dead")?,
                succeeded: count("succeeded")?,
                oldest_pending: row.try_get("oldest_pending").map_err(my)?,
            })
        })
    }

    fn defer(&self, id: Uuid, available_at: time::OffsetDateTime) -> Result<(), AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query(
                "UPDATE jobs_queue
                 SET status = 'pending',
                     attempt = GREATEST(attempt - 1, 0),
                     available_at = ?,
                     claimed_by = NULL, claimed_at = NULL
                 WHERE id = ? AND status = 'running'",
            )
            .bind(available_at)
            .bind(id)
            .execute(&pool)
            .await
            .map_err(my)?;
            if done.rows_affected() == 0 {
                return Err(AppError::conflict(format!(
                    "job {id} is not running; only a claimed job can be deferred"
                )));
            }
            Ok(())
        })
    }

    fn kill(&self, id: Uuid, error: &str) -> Result<(), AppError> {
        let error = error.to_string();
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query(
                "UPDATE jobs_queue
                 SET status = 'dead', last_error = ?, finished_at = UTC_TIMESTAMP(6)
                 WHERE id = ? AND status IN ('pending', 'running', 'failed')",
            )
            .bind(&error)
            .bind(id)
            .execute(&pool)
            .await
            .map_err(my)?;
            if done.rows_affected() == 0 {
                return Err(AppError::conflict(format!(
                    "job {id} is already terminal; only pending, running or failed jobs can be killed"
                )));
            }
            Ok(())
        })
    }

    fn defer_claim(
        &self,
        id: Uuid,
        attempt: u32,
        available_at: time::OffsetDateTime,
    ) -> Result<(), AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query(
                "UPDATE jobs_queue
                 SET status = 'pending',
                     attempt = GREATEST(attempt - 1, 0),
                     available_at = ?,
                     claimed_by = NULL, claimed_at = NULL
                 WHERE id = ? AND status = 'running' AND attempt = ?",
            )
            .bind(available_at)
            .bind(id)
            .bind(attempt as i64)
            .execute(&pool)
            .await
            .map_err(my)?;
            if done.rows_affected() == 0 {
                return Err(AppError::conflict(format!(
                    "job {id} claim is stale (attempt {attempt} was reclaimed); defer refused"
                )));
            }
            Ok(())
        })
    }

    fn kill_claim(&self, id: Uuid, attempt: u32, error: &str) -> Result<(), AppError> {
        let error = error.to_string();
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query(
                "UPDATE jobs_queue
                 SET status = 'dead', last_error = ?, finished_at = UTC_TIMESTAMP(6)
                 WHERE id = ? AND status = 'running' AND attempt = ?",
            )
            .bind(&error)
            .bind(id)
            .bind(attempt as i64)
            .execute(&pool)
            .await
            .map_err(my)?;
            if done.rows_affected() == 0 {
                return Err(AppError::conflict(format!(
                    "job {id} claim is stale (attempt {attempt} was reclaimed); kill refused"
                )));
            }
            Ok(())
        })
    }
}

// ----------------------------------------- tenant-scoped port (target §3.1)

/// Disambiguation for a scoped no-op: a cross-tenant id is INVISIBLE — the
/// same `NOT_FOUND` a missing id produces, so no existence oracle leaks
/// across tenants — while a VISIBLE job in the wrong state keeps the usual
/// conflict. Infra errors from the probe propagate unchanged.
async fn scoped_miss(
    pool: &MySqlPool,
    id: Uuid,
    tenant: Uuid,
    conflict: impl FnOnce() -> AppError,
) -> Result<AppError, AppError> {
    let visible: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs_queue WHERE id = ? AND tenant_id = ?)")
            .bind(id)
            .bind(tenant)
            .fetch_one(pool)
            .await
            .map_err(my)?;
    Ok(if visible {
        conflict()
    } else {
        AppError::not_found(format!("job {id}"))
    })
}

impl TenantJobStore for MySqlJobStore {
    fn enqueue_for(&self, tenant: TenantId, mut job: DurableJob) -> Result<Uuid, AppError> {
        if matches!(job.tenant_id, Some(set) if set != tenant) {
            return Err(AppError::validation(
                "job tenant does not match the enqueue tenant",
            ));
        }
        job.tenant_id = Some(tenant);
        // The physical INSERT persists tenant_id, so the stamp round-trips.
        self.enqueue(job)
    }

    fn claim_for(&self, tenant: TenantId, worker: &str) -> Result<Option<DurableJob>, AppError> {
        let worker = worker.to_string();
        let tenant = *tenant.as_uuid();
        let pool = self.pool.clone();
        block_on(async move {
            // Same one-connection transaction as the physical claim — the
            // candidate read is tenant-filtered so another tenant's pending
            // row is never selected, locked or claimed.
            //
            // P0 crash recovery mirrors the physical claim: an expired claim
            // lease re-enters this tenant's candidate set, and a spent-budget
            // orphan is dead-lettered first.
            let cutoff = time::OffsetDateTime::now_utc()
                - time::Duration::seconds(kelson_jobs::DEFAULT_CLAIM_LEASE_SECS as i64);
            // Autocommit, before the claim transaction: a claim that finds no
            // candidate rolls back, and would undo a dead-letter written
            // inside it (see `claim`).
            sqlx::query(
                "UPDATE jobs_queue
                 SET status = 'dead',
                     last_error = COALESCE(last_error, 'claim lease expired; retries exhausted'),
                     finished_at = UTC_TIMESTAMP(6)
                 WHERE status = 'running' AND attempt >= max_attempts AND tenant_id = ?
                   AND claimed_at IS NOT NULL AND claimed_at < ?",
            )
            .bind(tenant)
            .bind(cutoff)
            .execute(&pool)
            .await
            .map_err(my)?;
            let mut tx = pool.begin().await.map_err(my)?;
            let candidate: Option<Uuid> = sqlx::query_scalar(
                "SELECT id FROM jobs_queue
                 WHERE tenant_id = ?
                   AND ((status = 'pending' AND available_at <= UTC_TIMESTAMP(6))
                        OR (status = 'running' AND attempt < max_attempts
                            AND claimed_at IS NOT NULL AND claimed_at < ?))
                 ORDER BY priority DESC, available_at, enqueued_at
                 LIMIT 1
                 FOR UPDATE SKIP LOCKED",
            )
            .bind(tenant)
            .bind(cutoff)
            .fetch_optional(&mut *tx)
            .await
            .map_err(my)?;
            let Some(id) = candidate else {
                tx.rollback().await.map_err(my)?;
                return Ok(None);
            };
            let claimed = sqlx::query(
                "UPDATE jobs_queue
                 SET status = 'running', attempt = attempt + 1,
                     claimed_by = ?, claimed_at = UTC_TIMESTAMP(6)
                 WHERE id = ? AND tenant_id = ?
                   AND (status = 'pending'
                        OR (status = 'running' AND attempt < max_attempts
                            AND claimed_at IS NOT NULL AND claimed_at < ?))",
            )
            .bind(&worker)
            .bind(id)
            .bind(tenant)
            .bind(cutoff)
            .execute(&mut *tx)
            .await
            .map_err(my)?;
            if claimed.rows_affected() == 0 {
                tx.rollback().await.map_err(my)?;
                return Ok(None);
            }
            let row: Option<sqlx::mysql::MySqlRow> =
                sqlx::query("SELECT * FROM jobs_queue WHERE id = ?")
                    .bind(id)
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(my)?;
            tx.commit().await.map_err(my)?;
            row.map(job_from_row).transpose()
        })
    }

    fn complete_for(&self, tenant: TenantId, id: Uuid) -> Result<(), AppError> {
        let tenant = *tenant.as_uuid();
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query(
                "UPDATE jobs_queue
                 SET status = 'succeeded', finished_at = UTC_TIMESTAMP(6)
                 WHERE id = ? AND status = 'running' AND tenant_id = ?",
            )
            .bind(id)
            .bind(tenant)
            .execute(&pool)
            .await
            .map_err(my)?;
            if done.rows_affected() == 0 {
                let miss = scoped_miss(&pool, id, tenant, || {
                    AppError::conflict(format!("job {id} is not running"))
                })
                .await?;
                return Err(miss);
            }
            Ok(())
        })
    }

    fn fail_for(&self, tenant: TenantId, id: Uuid, error: &str) -> Result<JobStatus, AppError> {
        let error = error.to_string();
        let tenant = *tenant.as_uuid();
        let pool = self.pool.clone();
        block_on(async move {
            let mut tx = pool.begin().await.map_err(my)?;
            let done = sqlx::query(
                "UPDATE jobs_queue
                 SET last_error = ?,
                     status = CASE WHEN attempt >= max_attempts THEN 'dead' ELSE 'pending' END,
                     available_at = CASE WHEN attempt >= max_attempts THEN available_at
                                         ELSE UTC_TIMESTAMP(6) + INTERVAL CAST(LEAST(600, 30 * POW(2, GREATEST(attempt - 1, 0))) AS SIGNED) SECOND
                                    END,
                     finished_at = CASE WHEN attempt >= max_attempts THEN UTC_TIMESTAMP(6) ELSE NULL END
                 WHERE id = ? AND status = 'running' AND tenant_id = ?",
            )
            .bind(&error)
            .bind(id)
            .bind(tenant)
            .execute(&mut *tx)
            .await
            .map_err(my)?;
            if done.rows_affected() == 0 {
                tx.rollback().await.map_err(my)?;
                let miss = scoped_miss(&pool, id, tenant, || {
                    AppError::conflict(format!("job {id} is not running"))
                })
                .await?;
                return Err(miss);
            }
            let (status,): (String,) = sqlx::query_as("SELECT status FROM jobs_queue WHERE id = ?")
                .bind(id)
                .fetch_one(&mut *tx)
                .await
                .map_err(my)?;
            tx.commit().await.map_err(my)?;
            status_from(&status)
        })
    }

    fn complete_claim_for(&self, tenant: TenantId, id: Uuid, attempt: u32) -> Result<(), AppError> {
        let tenant = *tenant.as_uuid();
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query(
                "UPDATE jobs_queue
                 SET status = 'succeeded', finished_at = UTC_TIMESTAMP(6)
                 WHERE id = ? AND status = 'running' AND tenant_id = ? AND attempt = ?",
            )
            .bind(id)
            .bind(tenant)
            .bind(attempt as i64)
            .execute(&pool)
            .await
            .map_err(my)?;
            if done.rows_affected() == 0 {
                let miss = scoped_miss(&pool, id, tenant, || {
                    AppError::conflict(format!(
                        "job {id} claim is stale (attempt {attempt} was reclaimed); completion refused"
                    ))
                })
                .await?;
                return Err(miss);
            }
            Ok(())
        })
    }

    fn fail_claim_for(
        &self,
        tenant: TenantId,
        id: Uuid,
        attempt: u32,
        error: &str,
    ) -> Result<JobStatus, AppError> {
        let error = error.to_string();
        let tenant = *tenant.as_uuid();
        let pool = self.pool.clone();
        block_on(async move {
            let mut tx = pool.begin().await.map_err(my)?;
            let done = sqlx::query(
                "UPDATE jobs_queue
                 SET last_error = ?,
                     status = CASE WHEN attempt >= max_attempts THEN 'dead' ELSE 'pending' END,
                     available_at = CASE WHEN attempt >= max_attempts THEN available_at
                                         ELSE UTC_TIMESTAMP(6) + INTERVAL CAST(LEAST(600, 30 * POW(2, GREATEST(attempt - 1, 0))) AS SIGNED) SECOND
                                    END,
                     finished_at = CASE WHEN attempt >= max_attempts THEN UTC_TIMESTAMP(6) ELSE NULL END
                 WHERE id = ? AND status = 'running' AND tenant_id = ? AND attempt = ?",
            )
            .bind(&error)
            .bind(id)
            .bind(tenant)
            .bind(attempt as i64)
            .execute(&mut *tx)
            .await
            .map_err(my)?;
            if done.rows_affected() == 0 {
                tx.rollback().await.map_err(my)?;
                let miss = scoped_miss(&pool, id, tenant, || {
                    AppError::conflict(format!(
                        "job {id} claim is stale (attempt {attempt} was reclaimed); failure refused"
                    ))
                })
                .await?;
                return Err(miss);
            }
            let (status,): (String,) = sqlx::query_as("SELECT status FROM jobs_queue WHERE id = ?")
                .bind(id)
                .fetch_one(&mut *tx)
                .await
                .map_err(my)?;
            tx.commit().await.map_err(my)?;
            status_from(&status)
        })
    }
}
