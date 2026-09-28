//! Outbox access for the pump (S2 item 24) over the foundation outbox table.
//! Rows only ever exist post-commit (D-53); the pump marks them published
//! after every listener succeeded. At-least-once: a failed event simply
//! stays unpublished and is redelivered on the next pass.

use keel_foundation::AppError;
use keel_event::{BoxFuture, OutboxEvent, OutboxStore};
use sqlx::postgres::PgPool;
use uuid::Uuid;

pub struct PgOutboxStore {
    pool: PgPool,
}

impl PgOutboxStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn pg(e: impl std::fmt::Display) -> AppError {
    AppError::persistence(format!("outbox: {e}"))
}

impl OutboxStore for PgOutboxStore {
    fn claim_unpublished(&self, batch: usize) -> BoxFuture<'_, Result<Vec<OutboxEvent>, AppError>> {
        Box::pin(async move {
            let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(
                "SELECT id, tenant_id, event, topic, payload, published, created_at
                 FROM outbox WHERE published = false
                 ORDER BY created_at, id
                 LIMIT $1",
            )
            .bind(batch as i64)
            .fetch_all(&self.pool)
            .await
            .map_err(pg)?;
            rows.into_iter()
                .map(|row| {
                    use sqlx::Row;
                    let tenant_id: Option<Uuid> = row.try_get("tenant_id").map_err(pg)?;
                    Ok(OutboxEvent {
                        id: row.try_get("id").map_err(pg)?,
                        event: row.try_get("event").map_err(pg)?,
                        topic: row.try_get("topic").map_err(pg)?,
                        tenant_id: tenant_id.map(keel_foundation::TenantId::from_uuid),
                        payload: row.try_get("payload").map_err(pg)?,
                        published: row.try_get("published").map_err(pg)?,
                        created_at: row.try_get("created_at").map_err(pg)?,
                    })
                })
                .collect()
        })
    }

    fn mark_published(&self, ids: &[Uuid]) -> BoxFuture<'_, Result<(), AppError>> {
        let ids: Vec<Uuid> = ids.to_vec();
        Box::pin(async move {
            sqlx::query("UPDATE outbox SET published = true WHERE id = ANY($1)")
                .bind(&ids)
                .execute(&self.pool)
                .await
                .map_err(pg)?;
            Ok(())
        })
    }
}
