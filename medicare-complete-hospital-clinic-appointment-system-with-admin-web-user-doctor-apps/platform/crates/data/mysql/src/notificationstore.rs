//! Wave 6 P5 database channel on MySQL 8: durable `notifications` rows
//! (19-PARITY "Notifications v2"). Implements the keel-notifications
//! `NotificationStore` port so the inbox survives restarts and scales
//! across instances; the in-memory store backs degraded boot (D-52).
//!
//! MySQL port notes (B-31), vs the Postgres twin
//! (`keel_postgres::notificationstore`):
//! - `$n` placeholders become `?` in bind order (rule 1); uuid binds stay
//!   raw `BINARY(16)` (rule 2), timestamp binds stay `OffsetDateTime` over
//!   `DATETIME(6)` (rule 3) and `data` stays a `serde_json::Value` over a
//!   `JSON` column (rule 4).
//! - `COUNT(*) FILTER (WHERE read_at IS NULL)` has no MySQL `FILTER`
//!   clause (rule 17): it becomes `COUNT(CASE WHEN read_at IS NULL THEN 1
//!   END)` - the same integer count, 0 rather than NULL on an empty set,
//!   so the unread count and the (total, unread) page meta are unchanged.
//! - `id = ANY($3)` over the id list becomes a rebuilt `IN (?, ...)`
//!   list (rule 8); an empty list short-circuits to `Ok(0)` without a
//!   query (pg matches no rows there anyway).
//! - no RLS and no `SET LOCAL keel.tenant_id` (rule 11): every pg
//!   statement already carried its explicit `principal_id` predicate, and
//!   all three are principal-scoped - those predicates are kept verbatim
//!   and are the isolation story here; nothing was added or dropped.
//! - `rows_affected()` on the mark-read UPDATE counts MATCHED rows (rule
//!   16, sqlx requests CLIENT_FOUND_ROWS); the `AND read_at IS NULL`
//!   guard makes matched == changed, so re-marking already-read rows
//!   still reports 0 exactly like pg.

use keel_foundation::{AppError, PrincipalId};
use keel_notifications::{BoxFuture, NotificationPage, NotificationRow, NotificationStore};
use sqlx::mysql::MySqlPool;
use time::OffsetDateTime;
use uuid::Uuid;

pub struct MySqlNotificationStore {
    pool: MySqlPool,
}

impl MySqlNotificationStore {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
}

fn my(e: impl std::fmt::Display) -> AppError {
    AppError::persistence(format!("notifications: {e}"))
}

fn row_to_notification(row: sqlx::mysql::MySqlRow) -> Result<NotificationRow, AppError> {
    use sqlx::Row;
    let id: Uuid = row.try_get("id").map_err(my)?;
    let principal: Uuid = row.try_get("principal_id").map_err(my)?;
    let tenant: Option<Uuid> = row.try_get("tenant_id").map_err(my)?;
    Ok(NotificationRow {
        id,
        principal_id: PrincipalId::from_uuid(principal),
        tenant_id: tenant.map(keel_foundation::TenantId::from_uuid),
        kind: row.try_get("type").map_err(my)?,
        data: row.try_get("data").map_err(my)?,
        read_at: row.try_get("read_at").map_err(my)?,
        created_at: row.try_get("created_at").map_err(my)?,
    })
}

impl NotificationStore for MySqlNotificationStore {
    fn insert(&self, row: NotificationRow) -> BoxFuture<'_, Result<(), AppError>> {
        let pool = self.pool.clone();
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO notifications (id, principal_id, tenant_id, type, data, read_at, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
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
            .map_err(my)?;
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
                 FROM notifications WHERE principal_id = ?
                 ORDER BY created_at DESC, id DESC
                 LIMIT ? OFFSET ?",
            )
            .bind(*principal.as_uuid())
            .bind(limit as i64)
            .bind(offset as i64)
            .fetch_all(&pool)
            .await
            .map_err(my)?;
            let rows = rows
                .into_iter()
                .map(row_to_notification)
                .collect::<Result<Vec<_>, _>>()?;

            let (total, unread): (i64, i64) = sqlx::query_as(
                "SELECT COUNT(*), COUNT(CASE WHEN read_at IS NULL THEN 1 END)
                 FROM notifications WHERE principal_id = ?",
            )
            .bind(*principal.as_uuid())
            .fetch_one(&pool)
            .await
            .map_err(my)?;

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
            if ids.is_empty() {
                return Ok(0);
            }
            let placeholders = vec!["?"; ids.len()].join(", ");
            let sql = format!(
                "UPDATE notifications SET read_at = ?
                 WHERE principal_id = ? AND id IN ({placeholders}) AND read_at IS NULL"
            );
            let mut q = sqlx::query(&sql)
                .bind(OffsetDateTime::now_utc())
                .bind(*principal.as_uuid());
            for id in &ids {
                q = q.bind(id);
            }
            let done = q.execute(&pool).await.map_err(my)?;
            Ok(done.rows_affected())
        })
    }
}
