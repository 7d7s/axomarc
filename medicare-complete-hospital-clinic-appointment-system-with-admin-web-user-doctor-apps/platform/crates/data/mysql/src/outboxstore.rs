//! Outbox access for the pump (S2 item 24) over the foundation outbox table.
//! Rows only ever exist post-commit (D-53); the pump marks them published
//! after every listener succeeded. At-least-once: a failed event simply
//! stays unpublished and is redelivered on the next pass.
//!
//! MySQL port notes (B-31): `= ANY($1)` over an array becomes a rebuilt
//! `IN (?, ...)` list (empty list = no ids = no-op), and `LIMIT ?` takes a
//! placeholder directly.

use kelson_foundation::AppError;
use kelson_event::{BoxFuture, OutboxEvent, OutboxStore};
use sqlx::mysql::MySqlPool;
use uuid::Uuid;

pub struct MySqlOutboxStore {
    pool: MySqlPool,
}

impl MySqlOutboxStore {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
}

fn my(e: impl std::fmt::Display) -> AppError {
    AppError::persistence(format!("outbox: {e}"))
}

impl OutboxStore for MySqlOutboxStore {
    fn claim_unpublished(&self, batch: usize) -> BoxFuture<'_, Result<Vec<OutboxEvent>, AppError>> {
        Box::pin(async move {
            let rows: Vec<sqlx::mysql::MySqlRow> = sqlx::query(
                "SELECT id, tenant_id, event, topic, payload, published, created_at
                 FROM outbox WHERE published = 0
                 ORDER BY created_at, id
                 LIMIT ?",
            )
            .bind(batch as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(my)?;
            rows.into_iter()
                .map(|row| {
                    use sqlx::Row;
                    let tenant_id: Option<Uuid> = row.try_get("tenant_id").map_err(my)?;
                    Ok(OutboxEvent {
                        id: row.try_get("id").map_err(my)?,
                        event: row.try_get("event").map_err(my)?,
                        topic: row.try_get("topic").map_err(my)?,
                        tenant_id: tenant_id.map(kelson_foundation::TenantId::from_uuid),
                        payload: row.try_get("payload").map_err(my)?,
                        published: row.try_get("published").map_err(my)?,
                        created_at: row.try_get("created_at").map_err(my)?,
                    })
                })
                .collect()
        })
    }

    fn mark_published(&self, ids: &[Uuid]) -> BoxFuture<'_, Result<(), AppError>> {
        let ids: Vec<Uuid> = ids.to_vec();
        Box::pin(async move {
            if ids.is_empty() {
                return Ok(());
            }
            let placeholders = vec!["?"; ids.len()].join(", ");
            let sql = format!("UPDATE outbox SET published = 1 WHERE id IN ({placeholders})");
            let mut q = sqlx::query(&sql);
            for id in &ids {
                q = q.bind(id);
            }
            q.execute(&self.pool).await.map_err(my)?;
            Ok(())
        })
    }
}
