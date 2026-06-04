// Backup writers: `begin`, `complete`, `verify`.

use sqlx::{Pool, Sqlite};

use sovereign_core::domain::{
    kind, AuditEvent, Backup, BackupId, BackupStatus, NewBackup, Timestamp,
};
use sovereign_core::error::AppError;

use crate::app::append_audit;
use crate::row;

pub(crate) async fn begin(
    pool: &Pool<Sqlite>,
    new: NewBackup,
    actor: &str,
) -> Result<Backup, AppError> {
    if new.target.trim().is_empty() {
        return Err(AppError::validation("backup target must not be empty"));
    }
    let id = BackupId::generate();
    let now = Timestamp::now();
    let mut tx = pool.begin().await?;

    sqlx::query(
        "INSERT INTO backup (id, target, status, size_bytes, location, started_at, \
         finished_at, verified_at, verify_result, error) \
         VALUES (?, ?, ?, NULL, NULL, ?, NULL, NULL, NULL, NULL)",
    )
    .bind(id.as_uuid())
    .bind(&new.target)
    .bind(BackupStatus::Pending.to_string())
    .bind(now.as_secs())
    .execute(&mut *tx)
    .await?;

    append_audit(
        &mut tx,
        AuditEvent {
            id: None,
            ts: now,
            actor: actor.to_string(),
            kind: kind::BACKUP_CREATE,
            target: Some(format!("backup:{id}")),
            payload: serde_json::json!({ "target": new.target }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    row::select_backup_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::internal("backup vanished after insert"))
}

pub(crate) async fn complete(
    pool: &Pool<Sqlite>,
    id: BackupId,
    status: BackupStatus,
    size_bytes: Option<i64>,
    location: Option<String>,
    error: Option<String>,
    actor: &str,
) -> Result<Backup, AppError> {
    let mut tx = pool.begin().await?;
    let now = Timestamp::now();

    let rows = sqlx::query(
        "UPDATE backup SET status = ?, size_bytes = ?, location = ?, finished_at = ?, \
         error = ? WHERE id = ?",
    )
    .bind(status.to_string())
    .bind(size_bytes)
    .bind(location.as_deref())
    .bind(now.as_secs())
    .bind(error.as_deref())
    .bind(id.as_uuid())
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if rows == 0 {
        return Err(AppError::not_found("backup"));
    }

    append_audit(
        &mut tx,
        AuditEvent {
            id: None,
            ts: now,
            actor: actor.to_string(),
            kind: kind::BACKUP_CREATE,
            target: Some(format!("backup:{id}")),
            payload: serde_json::json!({
                "status": status.to_string(),
                "size_bytes": size_bytes,
                "error": error,
            }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    row::select_backup_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::internal("backup vanished after complete"))
}

pub(crate) async fn verify(
    pool: &Pool<Sqlite>,
    id: BackupId,
    verify_result: serde_json::Value,
    actor: &str,
) -> Result<Backup, AppError> {
    let mut tx = pool.begin().await?;
    let now = Timestamp::now();

    let serialized = serde_json::to_string(&verify_result)
        .map_err(|e| AppError::internal(format!("verify_result serialize: {e}")))?;

    let rows = sqlx::query(
        "UPDATE backup SET status = ?, verified_at = ?, verify_result = ? WHERE id = ?",
    )
    .bind(BackupStatus::Verified.to_string())
    .bind(now.as_secs())
    .bind(serialized)
    .bind(id.as_uuid())
    .execute(&mut *tx)
    .await?
    .rows_affected();

    if rows == 0 {
        return Err(AppError::not_found("backup"));
    }

    append_audit(
        &mut tx,
        AuditEvent {
            id: None,
            ts: now,
            actor: actor.to_string(),
            kind: kind::BACKUP_VERIFY,
            target: Some(format!("backup:{id}")),
            payload: serde_json::json!({ "result": verify_result }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    row::select_backup_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::internal("backup vanished after verify"))
}
