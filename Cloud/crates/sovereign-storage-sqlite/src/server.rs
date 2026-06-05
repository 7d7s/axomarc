// Server writers: `add`, `set_status`, `touch`, `remove`.

use sqlx::{Pool, Sqlite};

use sovereign_core::domain::{
    kind, AuditEvent, NewServer, Server, ServerId, ServerStatus, Timestamp,
};
use sovereign_core::error::AppError;

use crate::app::append_audit;
use crate::row;

pub(crate) async fn add(pool: &Pool<Sqlite>, new: NewServer) -> Result<Server, AppError> {
    if new.hostname.trim().is_empty() {
        return Err(AppError::validation("server hostname must not be empty"));
    }
    if new.api_url.trim().is_empty() {
        return Err(AppError::validation("server api_url must not be empty"));
    }

    let id = ServerId::generate();
    let now = Timestamp::now();
    let mut tx = pool.begin().await?;

    let result = sqlx::query(
        "INSERT INTO server (id, hostname, role, api_url, status, last_seen, \
         cpu_cores, mem_mb, disk_gb, joined_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.as_uuid())
    .bind(&new.hostname)
    .bind(new.role.to_string())
    .bind(&new.api_url)
    .bind(ServerStatus::Healthy.to_string())
    .bind(now.as_secs())
    .bind(new.cpu_cores)
    .bind(new.mem_mb)
    .bind(new.disk_gb)
    .bind(now.as_secs())
    .execute(&mut *tx)
    .await;

    if let Err(e) = result {
        if let sqlx::Error::Database(db) = &e {
            if db.message().contains("UNIQUE") {
                return Err(AppError::validation(format!(
                    "server hostname `{}` already exists",
                    new.hostname
                )));
            }
        }
        return Err(AppError::from(e));
    }

    append_audit(
        &mut tx,
        AuditEvent {
            id: None,
            ts: now,
            actor: "system".to_string(),
            kind: kind::SERVER_ADD,
            target: Some(format!("server:{id}")),
            payload: serde_json::json!({
                "hostname": new.hostname,
                "role": new.role.to_string(),
                "api_url": new.api_url,
            }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    row::select_server_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::internal("server vanished after insert"))
}

pub(crate) async fn set_status(
    pool: &Pool<Sqlite>,
    id: ServerId,
    status: ServerStatus,
    actor: &str,
) -> Result<Server, AppError> {
    let mut tx = pool.begin().await?;
    let now = Timestamp::now();

    let rows = sqlx::query("UPDATE server SET status = ?, last_seen = ? WHERE id = ?")
        .bind(status.to_string())
        .bind(now.as_secs())
        .bind(id.as_uuid())
        .execute(&mut *tx)
        .await?
        .rows_affected();

    if rows == 0 {
        return Err(AppError::not_found("server"));
    }

    let audit_kind = if status == ServerStatus::Removed {
        kind::SERVER_REMOVE
    } else {
        kind::SERVER_DRAIN
    };

    append_audit(
        &mut tx,
        AuditEvent {
            id: None,
            ts: now,
            actor: actor.to_string(),
            kind: audit_kind,
            target: Some(format!("server:{id}")),
            payload: serde_json::json!({ "status": status.to_string() }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    row::select_server_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::internal("server vanished after set_status"))
}

pub(crate) async fn touch(
    pool: &Pool<Sqlite>,
    id: ServerId,
    at: Timestamp,
) -> Result<(), AppError> {
    let rows = sqlx::query("UPDATE server SET last_seen = ? WHERE id = ?")
        .bind(at.as_secs())
        .bind(id.as_uuid())
        .execute(pool)
        .await?
        .rows_affected();
    if rows == 0 {
        return Err(AppError::not_found("server"));
    }
    Ok(())
}

pub(crate) async fn remove(pool: &Pool<Sqlite>, id: ServerId, actor: &str) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    let now = Timestamp::now();

    let rows = sqlx::query("DELETE FROM server WHERE id = ?")
        .bind(id.as_uuid())
        .execute(&mut *tx)
        .await?
        .rows_affected();

    if rows == 0 {
        return Err(AppError::not_found("server"));
    }

    append_audit(
        &mut tx,
        AuditEvent {
            id: None,
            ts: now,
            actor: actor.to_string(),
            kind: kind::SERVER_REMOVE,
            target: Some(format!("server:{id}")),
            payload: serde_json::json!({}),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    Ok(())
}
