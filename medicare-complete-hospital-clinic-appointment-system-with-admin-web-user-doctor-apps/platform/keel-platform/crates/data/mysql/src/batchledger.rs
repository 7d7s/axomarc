//! Wave 6 P9 durable batch ledger (19-PARITY "Queues advanced" bullet 304;
//! B-32) on MySQL 8: `job_batches` rows behind `batch.status` /
//! `batch.cancel` / `batch.retry`, `queue:retry-batch` and
//! `queue:prune-batches`. Membership is written once at dispatch (an upsert
//! refreshes metadata only and never touches `created_at`/`finished_at`);
//! per-member state lives in `jobs_queue`, so reads are live queue truth and
//! concurrent member finishes plus admin processes cannot lose updates.
//!
//! The ledger contract is sync (like [`kelson_jobs::JobStore`]); MySQL calls
//! bridge through [`crate::bridge::block_on`].
//!
//! MySQL port notes (every divergence from `kelson_postgres::batchledger`):
//!
//! - `ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name, on_complete =
//!   EXCLUDED.on_complete` -> `INSERT ... VALUES (...) AS new ON DUPLICATE
//!   KEY UPDATE name = new.name, on_complete = new.on_complete` (MySQL 8
//!   row alias). `job_batches` has exactly one unique key -- the `id`
//!   primary key -- so the upsert can only fire on an id collision: the pg
//!   conflict target is reproduced exactly, and a collision on any other key
//!   (there is none) would still be a plain error on both sides. `members`,
//!   `created_at` and `finished_at` are never rewritten on a refresh, as in
//!   pg.
//! - `VALUES ($1, $2, $3::jsonb, $4, $5, $6)` drops the `::jsonb` cast: the
//!   column is `JSON` and MySQL converts the bound members string itself;
//!   `serde_json::Value` binds and gets are unchanged (JSON column).
//! - rows_affected is not observed by `save` (like pg), so the MySQL
//!   upsert's 1 = inserted / 2 = updated / 0 = "values already current"
//!   difference is invisible here; `prune` still returns rows_affected (a
//!   DELETE count, deleted = matched, identical to pg) and `mark_finished`
//!   still ignores it (unknown id = no-op).
//! - Everything else is a plain `?` translation: `SELECT *` reads keep the
//!   pg row shape (members are parsed from the JSON column back into
//!   `Vec<Uuid>`), the `finished_at IS NOT NULL AND finished_at < ?`
//!   strictly-before cutoff is untouched.

use kelson_foundation::AppError;
use kelson_jobs::{BatchLedger, BatchRecord};
use sqlx::mysql::MySqlPool;
use sqlx::Row;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::bridge::block_on;

pub struct MySqlBatchLedger {
    pool: MySqlPool,
}

impl MySqlBatchLedger {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
}

fn my(e: impl std::fmt::Display) -> AppError {
    AppError::persistence(format!("job_batches: {e}"))
}

fn members_json(members: &[Uuid]) -> Result<String, AppError> {
    serde_json::to_string(members)
        .map_err(|e| AppError::persistence(format!("job_batches members: {e}")))
}

fn record_from_row(row: sqlx::mysql::MySqlRow) -> Result<BatchRecord, AppError> {
    let raw: serde_json::Value = row.try_get("members").map_err(my)?;
    let members: Vec<Uuid> = serde_json::from_value(raw)
        .map_err(|e| AppError::persistence(format!("job_batches members: {e}")))?;
    Ok(BatchRecord {
        id: row.try_get("id").map_err(my)?,
        name: row.try_get("name").map_err(my)?,
        members,
        on_complete: row.try_get("on_complete").map_err(my)?,
        created_at: row.try_get("created_at").map_err(my)?,
        finished_at: row.try_get("finished_at").map_err(my)?,
    })
}

impl BatchLedger for MySqlBatchLedger {
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
                 VALUES (?, ?, ?, ?, ?, ?) AS new
                 ON DUPLICATE KEY UPDATE
                     name = new.name,
                     on_complete = new.on_complete",
            )
            .bind(id)
            .bind(name)
            .bind(members)
            .bind(on_complete)
            .bind(created_at)
            .bind(finished_at)
            .execute(&pool)
            .await
            .map_err(my)?;
            Ok(())
        })
    }

    fn get(&self, id: Uuid) -> Result<Option<BatchRecord>, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let row: Option<sqlx::mysql::MySqlRow> =
                sqlx::query("SELECT * FROM job_batches WHERE id = ?")
                    .bind(id)
                    .fetch_optional(&pool)
                    .await
                    .map_err(my)?;
            row.map(record_from_row).transpose()
        })
    }

    fn list(&self) -> Result<Vec<BatchRecord>, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let rows: Vec<sqlx::mysql::MySqlRow> =
                sqlx::query("SELECT * FROM job_batches ORDER BY created_at")
                    .fetch_all(&pool)
                    .await
                    .map_err(my)?;
            rows.into_iter().map(record_from_row).collect()
        })
    }

    fn mark_finished(&self, id: Uuid, at: OffsetDateTime) -> Result<(), AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            sqlx::query(
                "UPDATE job_batches SET finished_at = ?
                 WHERE id = ? AND finished_at IS NULL",
            )
            .bind(at)
            .bind(id)
            .execute(&pool)
            .await
            .map_err(my)?;
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
                 WHERE finished_at IS NOT NULL AND finished_at < ?",
            )
            .bind(finished_before)
            .execute(&pool)
            .await
            .map_err(my)?;
            Ok(done.rows_affected())
        })
    }
}
