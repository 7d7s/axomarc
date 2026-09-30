//! Wave 6 P5 database channel: durable `notifications` rows (19-PARITY
//! "Notifications v2"). Implements the keel-notifications `NotificationStore`
//! port so the inbox survives restarts and scales across instances; the
//! in-memory store backs degraded boot (D-52).

use keel_foundation::{AppError, PrincipalId};
use keel_notifications::{BoxFuture, NotificationPage, NotificationRow, NotificationStore};
use sqlx::postgres::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

pub struct PgNotificationStore {
    pool: PgPool,
}

impl PgNotificationStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn pg(e: impl std::fmt::Display) -> AppError {
    AppError::persistence(format!("notifications: {e}"))
}

fn row_to_notification(row: sqlx::postgres::PgRow) -> Result<NotificationRow, AppError> {
    use sqlx::Row;
    let id: Uuid = row.try_get("id").map_err(pg)?;
    let principal: Uuid = row.try_get("principal_id").map_err(pg)?;
    let tenant: Option<Uuid> = row.try_get("tenant_id").map_err(pg)?;
    Ok(NotificationRow {
        id,
        principal_id: PrincipalId::from_uuid(principal),
        tenant_id: tenant.map(keel_foundation::TenantId::from_uuid),
        kind: row.try_get("type").map_err(pg)?,
        data: row.try_get("data").map_err(pg)?,
        read_at: row.try_get("read_at").map_err(pg)?,
        created_at: row.try_get("created_at").map_err(pg)?,
    })
}

impl NotificationStore for PgNotificationStore {
    fn insert(&self, row: NotificationRow) -> BoxFuture<'_, Result<(), AppError>> {
        let pool = self.pool.clone();
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO notifications (id, principal_id, tenant_id, type, data, read_at, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)",
            )
            .bind(row.id)
            .bind(*row.principal_id.as_uuid())
            .bind(row.tenant_id.map(|t| *t.as_uuid()))
            .bind(&row.kind)
            .bind(&row.data)
            .bind(row.read_at)
            .bind(row.created_at)
            .execute(&pool)
            .await
            .map_err(pg)?;
            Ok(())
        })
    }

    fn list(
        &self,
        principal: PrincipalId,
        offset: u64,
        limit: u64,
    ) -> BoxFuture<'_, Result<NotificationPage, AppError>> {
        let pool = self.pool.clone();
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT id, principal_id, tenant_id, type, data, read_at, created_at
                 FROM notifications WHERE principal_id = $1
                 ORDER BY created_at DESC, id DESC
                 LIMIT $2 OFFSET $3",
            )
            .bind(*principal.as_uuid())
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(&pool)
            .await
            .map_err(pg)?;
            let rows = rows
                .into_iter()
                .map(row_to_notification)
                .collect::<Result<Vec<_>, _>>()?;

            let (total, unread): (i64, i64) = sqlx::query_as(
                "SELECT COUNT(*), COUNT(*) FILTER (WHERE read_at IS NULL)
                 FROM notifications WHERE principal_id = $1",
            )
            .bind(*principal.as_uuid())
            .fetch_one(&pool)
            .await
            .map_err(pg)?;

            Ok(NotificationPage {
                rows,
                total: total.max(0) as u64,
                unread: unread.max(0) as u64,
            })
        })
    }

    fn mark_read(
        &self,
        principal: PrincipalId,
        ids: Vec<Uuid>,
    ) -> BoxFuture<'_, Result<u64, AppError>> {
        let pool = self.pool.clone();
        Box::pin(async move {
            let done = sqlx::query(
                "UPDATE notifications SET read_at = $1
                 WHERE principal_id = $2 AND id = ANY($3) AND read_at IS NULL",
            )
            .bind(OffsetDateTime::now_utc())
            .bind(*principal.as_uuid())
            .bind(&ids)
            .execute(&pool)
            .await
            .map_err(pg)?;
            Ok(done.rows_affected())
        })
    }
}
