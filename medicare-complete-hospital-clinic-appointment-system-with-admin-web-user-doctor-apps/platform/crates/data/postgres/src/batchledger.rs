//! Wave 6 P9 durable batch ledger (19-PARITY "Queues advanced" bullet 304;
//! B-32): `job_batches` rows behind `batch.status` / `batch.cancel` /
//! `batch.retry`, `queue:retry-batch` and `queue:prune-batches`. Membership
//! is written once at dispatch (an upsert refreshes metadata only and never
//! touches `created_at`/`finished_at`); per-member state lives in
//! `jobs_queue`, so reads are live queue truth and concurrent member
//! finishes plus admin processes cannot lose updates.
//!
//! The ledger contract is sync (like [`keel_jobs::JobStore`]); pg calls
//! bridge through [`crate::bridge::block_on`].

use keel_foundation::AppError;
use keel_jobs::{BatchLedger, BatchRecord};
use sqlx::postgres::PgPool;
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::bridge::block_on;

pub struct PgBatchLedger {
    pool: PgPool,
}

impl PgBatchLedger {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn pg(e: impl std::fmt::Display) -> AppError {
    AppError::persistence(format!("job_batches: {e}"))
}

fn members_json(members: &[Uuid]) -> Result<String, AppError> {
    serde_json::to_string(members)
        .map_err(|e| AppError::persistence(format!("job_batches members: {e}")))
}

fn record_from_row(row: sqlx::postgres::PgRow) -> Result<BatchRecord, AppError> {
    let raw: serde_json::Value = row.try_get("members").map_err(pg)?;
    let members: Vec<Uuid> = serde_json::from_value(raw)
        .map_err(|e| AppError::persistence(format!("job_batches members: {e}")))?;
    Ok(BatchRecord {
        id: row.try_get("id").map_err(pg)?,
        name: row.try_get("name").map_err(pg)?,
        members,
        on_complete: row.try_get("on_complete").map_err(pg)?,
        created_at: row.try_get("created_at").map_err(pg)?,
        finished_at: row.try_get("finished_at").map_err(pg)?,
    })
}

impl BatchLedger for PgBatchLedger {
    fn save(&self, record: &BatchRecord) -> Result<(), AppError> {
        let members = members_json(&record.members)?;
        let id = record.id;
        let name = record.name.clone();
        let on_complete = record.on_complete.clone();
        let created_at = record.created_at;
        let finished_at = record.finished_at;
        let pool = self.pool.clone();
        block_on(async move {
            sqlx::query(
                "INSERT INTO job_batches
                     (id, name, members, on_complete, created_at, finished_at)
                 VALUES ($1, $2, $3::jsonb, $4, $5, $6)
                 ON CONFLICT (id) DO UPDATE SET
                     name = EXCLUDED.name,
                     on_complete = EXCLUDED.on_complete",
            )
            .bind(id)
            .bind(name)
            .bind(members)
            .bind(on_complete)
            .bind(created_at)
            .bind(finished_at)
            .execute(&pool)
            .await
            .map_err(pg)?;
            Ok(())
        })
    }

    fn get(&self, id: Uuid) -> Result<Option<BatchRecord>, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let row: Option<sqlx::postgres::PgRow> =
                sqlx::query("SELECT * FROM job_batches WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&pool)
                    .await
                    .map_err(pg)?;
            row.map(record_from_row).transpose()
        })
    }

    fn list(&self) -> Result<Vec<BatchRecord>, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let rows: Vec<sqlx::postgres::PgRow> =
                sqlx::query("SELECT * FROM job_batches ORDER BY created_at")
                    .fetch_all(&pool)
                    .await
                    .map_err(pg)?;
            rows.into_iter().map(record_from_row).collect()
        })
    }

    fn mark_finished(&self, id: Uuid, at: OffsetDateTime) -> Result<(), AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            sqlx::query(
                "UPDATE job_batches SET finished_at = $2
                 WHERE id = $1 AND finished_at IS NULL",
            )
            .bind(id)
            .bind(at)
            .execute(&pool)
            .await
            .map_err(pg)?;
            Ok(())
        })
    }

    fn prune(&self, finished_before: OffsetDateTime) -> Result<u64, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            // Strictly-before, matching prune_dead: a record finished exactly
            // at the cutoff stays; open batches (finished_at IS NULL) never go.
            let done = sqlx::query(
                "DELETE FROM job_batches
                 WHERE finished_at IS NOT NULL AND finished_at < $1",
            )
            .bind(finished_before)
            .execute(&pool)
            .await
            .map_err(pg)?;
            Ok(done.rows_affected())
        })
    }
}
