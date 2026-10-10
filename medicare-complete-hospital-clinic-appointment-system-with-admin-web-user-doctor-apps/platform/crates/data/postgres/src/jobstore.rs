//! Durable job queue on PostgreSQL (S2 item 22): SKIP LOCKED claiming, so N
//! workers pull concurrently without coordination. Backoff on failure is
//! applied by updating available_at (exponential, 30s base, 10min cap).
//!
//! The JobStore contract is sync (frozen, STAGE-0-SPEC §5); pg calls bridge
//! through crate::bridge (block_in_place inside multi-threaded runtimes, a
//! persistent bridge runtime otherwise).

use kelson_foundation::AppError;
use kelson_jobs::{DurableJob, JobStatus, JobStore, JobStoreAdmin};
use sqlx::postgres::PgPool;

use crate::bridge::block_on;
use sqlx::Row;
use uuid::Uuid;

pub struct PgJobStore {
    pool: PgPool,
}

impl PgJobStore {
    pub fn new(pool: PgPool) -> Self {
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

fn job_from_row(row: sqlx::postgres::PgRow) -> Result<DurableJob, AppError> {
    let causation: Option<Uuid> = row.try_get("causation_id").map_err(pg)?;
    let correlation: Option<Uuid> = row.try_get("correlation_id").map_err(pg)?;
    Ok(DurableJob {
        id: row.try_get("id").map_err(pg)?,
        execution_id: kelson_foundation::ExecutionId::from_uuid(row.try_get("execution_id").map_err(pg)?),
        name: row.try_get("name").map_err(pg)?,
        payload: row.try_get("payload").map_err(pg)?,
        queue: row.try_get("queue").map_err(pg)?,
        attempt: row.try_get::<i32, _>("attempt").map_err(pg)? as u32,
        max_attempts: row.try_get::<i32, _>("max_attempts").map_err(pg)? as u32,
        deadline: row.try_get("deadline").map_err(pg)?,
        available_at: row.try_get("available_at").map_err(pg)?,
        status: status_from(&row.try_get::<String, _>("status").map_err(pg)?)?,
        priority: row.try_get("priority").map_err(pg)?,
        claimed_by: row.try_get("claimed_by").map_err(pg)?,
        claimed_at: row.try_get("claimed_at").map_err(pg)?,
        causation_id: causation.map(kelson_foundation::CorrelationId::from_uuid),
        correlation_id: correlation.map(kelson_foundation::CorrelationId::from_uuid),
        last_error: row.try_get("last_error").map_err(pg)?,
        enqueued_at: row.try_get("enqueued_at").map_err(pg)?,
        finished_at: row.try_get("finished_at").map_err(pg)?,
    })
}

fn pg(e: impl std::fmt::Display) -> AppError {
    AppError::persistence(format!("jobs_queue: {e}"))
}

impl JobStore for PgJobStore {
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
                  enqueued_at, finished_at)
                 VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16)",
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
            .execute(&pool)
            .await
            .map_err(pg)?;
            Ok::<_, AppError>(id)
        })
    }

    fn claim(&self, worker: &str) -> Result<Option<DurableJob>, AppError> {
        let worker = worker.to_string();
        let pool = self.pool.clone();
        block_on(async move {
            let row: Option<sqlx::postgres::PgRow> = sqlx::query(
                "UPDATE jobs_queue
                 SET status = 'running', attempt = attempt + 1,
                     claimed_by = $1, claimed_at = now()
                 WHERE id = (
                     SELECT id FROM jobs_queue
                     WHERE status = 'pending' AND available_at <= now()
                     ORDER BY priority DESC, available_at, enqueued_at
                     LIMIT 1
                     FOR UPDATE SKIP LOCKED
                 )
                 RETURNING *",
            )
            .bind(&worker)
            .fetch_optional(&pool)
            .await
            .map_err(pg)?;
            row.map(job_from_row).transpose()
        })
    }

    fn complete(&self, id: Uuid) -> Result<(), AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query(
                "UPDATE jobs_queue
                 SET status = 'succeeded', finished_at = now()
                 WHERE id = $1 AND status = 'running'",
            )
            .bind(id)
            .execute(&pool)
            .await
            .map_err(pg)?;
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
            let row: Option<(String,)> = sqlx::query_as(
                "UPDATE jobs_queue
                 SET last_error = $2,
                     status = CASE WHEN attempt >= max_attempts THEN 'dead' ELSE 'pending' END,
                     available_at = CASE WHEN attempt >= max_attempts THEN available_at
                                         ELSE now() + make_interval(secs => LEAST(600, 30 * power(2, GREATEST(attempt - 1, 0)))::int)
                                    END,
                     finished_at = CASE WHEN attempt >= max_attempts THEN now() ELSE NULL END
                 WHERE id = $1 AND status = 'running'
                 RETURNING status",
            )
            .bind(id)
            .bind(&error)
            .fetch_optional(&pool)
            .await
            .map_err(pg)?;
            let Some((status,)) = row else {
                return Err(AppError::conflict(format!("job {id} is not running")));
            };
            status_from(&status)
        })
    }
}

impl JobStoreAdmin for PgJobStore {
    fn list(&self, status: Option<JobStatus>, limit: usize) -> Result<Vec<DurableJob>, AppError> {
        let status = status.map(status_text).map(|s| s.to_string());
        let pool = self.pool.clone();
        block_on(async move {
            let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(
                "SELECT * FROM jobs_queue
                 WHERE ($1::text IS NULL OR status = $1)
                 ORDER BY enqueued_at DESC
                 LIMIT $2",
            )
            .bind(status)
            .bind(limit as i64)
            .fetch_all(&pool)
            .await
            .map_err(pg)?;
            rows.into_iter().map(job_from_row).collect()
        })
    }

    fn get(&self, id: Uuid) -> Result<Option<DurableJob>, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let row: Option<sqlx::postgres::PgRow> =
                sqlx::query("SELECT * FROM jobs_queue WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&pool)
                    .await
                    .map_err(pg)?;
            row.map(job_from_row).transpose()
        })
    }

    fn requeue(&self, id: Uuid) -> Result<(), AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query(
                "UPDATE jobs_queue
                 SET status = 'pending', available_at = now(),
                     claimed_by = NULL, claimed_at = NULL, finished_at = NULL
                 WHERE id = $1 AND status IN ('failed', 'dead')",
            )
            .bind(id)
            .execute(&pool)
            .await
            .map_err(pg)?;
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
                .map_err(pg)?;
            Ok(done.rows_affected())
        })
    }

    fn prune_dead(&self, before: time::OffsetDateTime) -> Result<u64, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query(
                "DELETE FROM jobs_queue
                 WHERE status = 'dead'
                   AND COALESCE(finished_at, enqueued_at) < $1",
            )
            .bind(before)
            .execute(&pool)
            .await
            .map_err(pg)?;
            Ok(done.rows_affected())
        })
    }

    fn delete_dead(&self, id: Uuid) -> Result<bool, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query("DELETE FROM jobs_queue WHERE id = $1 AND status = 'dead'")
                .bind(id)
                .execute(&pool)
                .await
                .map_err(pg)?;
            Ok(done.rows_affected() == 1)
        })
    }

    fn snapshot(&self) -> Result<kelson_jobs::QueueSnapshot, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let row = sqlx::query(
                "SELECT COUNT(*) FILTER (WHERE status = 'pending') AS pending,
                        COUNT(*) FILTER (WHERE status = 'running') AS running,
                        COUNT(*) FILTER (WHERE status = 'dead') AS dead,
                        COUNT(*) FILTER (WHERE status = 'succeeded') AS succeeded,
                        MIN(enqueued_at) FILTER (WHERE status = 'pending') AS oldest_pending
                   FROM jobs_queue",
            )
            .fetch_one(&pool)
            .await
            .map_err(pg)?;
            let count = |col: &str| -> Result<usize, AppError> {
                let n: i64 = row.try_get(col).map_err(pg)?;
                Ok(n.max(0) as usize)
            };
            Ok(kelson_jobs::QueueSnapshot {
                pending: count("pending")?,
                running: count("running")?,
                dead: count("dead")?,
                succeeded: count("succeeded")?,
                oldest_pending: row.try_get("oldest_pending").map_err(pg)?,
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
                     available_at = $2,
                     claimed_by = NULL, claimed_at = NULL
                 WHERE id = $1 AND status = 'running'",
            )
            .bind(id)
            .bind(available_at)
            .execute(&pool)
            .await
            .map_err(pg)?;
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
                 SET status = 'dead', last_error = $2, finished_at = now()
                 WHERE id = $1 AND status IN ('pending', 'running', 'failed')",
            )
            .bind(id)
            .bind(&error)
            .execute(&pool)
            .await
            .map_err(pg)?;
            if done.rows_affected() == 0 {
                return Err(AppError::conflict(format!(
                    "job {id} is already terminal; only pending, running or failed jobs can be killed"
                )));
            }
            Ok(())
        })
    }
}
