// Secret writers: `put`, `delete`, `rotate`, `set_status`.
// The `ciphertext` field is never logged and never returned via
// `list_secrets` keys-only views in higher layers (the read here is the
// raw row; the secrets port layer is responsible for redacting the
// payload when it crosses the trust boundary).

use sqlx::types::Uuid;
use sqlx::{Pool, Row, Sqlite};

use sovereign_core::domain::{
    kind, AuditEvent, NewSecret, Secret, SecretId, SecretStatus, Timestamp,
};
use sovereign_core::error::AppError;

use crate::app::append_audit;
use crate::row;

/// Fallback hex encoder for the unlikely case the app_id bytes are not
/// a valid UUID (would indicate database corruption, but we don't want
/// to panic in the audit path).
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

pub(crate) async fn put(
    pool: &Pool<Sqlite>,
    new: NewSecret,
    actor: &str,
) -> Result<Secret, AppError> {
    if new.key.trim().is_empty() {
        return Err(AppError::validation("secret key must not be empty"));
    }
    let id = SecretId::generate();
    let now = Timestamp::now();
    let mut tx = pool.begin().await?;

    let result = sqlx::query(
        "INSERT INTO secret (id, app_id, key, ciphertext, status, created_at, \
         rotated_at, version) VALUES (?, ?, ?, ?, ?, ?, NULL, 1)",
    )
    .bind(id.as_uuid())
    .bind(new.app_id.as_uuid())
    .bind(&new.key)
    .bind(&new.ciphertext)
    .bind(SecretStatus::Active.to_string())
    .bind(now.as_secs())
    .execute(&mut *tx)
    .await;

    if let Err(e) = result {
        if let sqlx::Error::Database(db) = &e {
            if db.message().contains("UNIQUE") {
                return Err(AppError::validation(format!(
                    "secret `{}` already exists for this app",
                    new.key
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
            actor: actor.to_string(),
            kind: kind::SECRET_SET,
            target: Some(format!("secret:{id}")),
            payload: serde_json::json!({
                "app_id": new.app_id.to_string(),
                "key": new.key,
            }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    row::select_secret_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::internal("secret vanished after insert"))
}

pub(crate) async fn delete(
    pool: &Pool<Sqlite>,
    id: SecretId,
    actor: &str,
) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    let now = Timestamp::now();

    let rows = sqlx::query("SELECT app_id, key FROM secret WHERE id = ?")
        .bind(id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
    let row = rows.ok_or(AppError::not_found("secret"))?;
    // `app_id` is stored as BLOB; read as raw bytes and render to the
    // hyphenated form for the audit payload. The audit log records
    // *what* happened in human-readable form; the bytes are 16 anyway.
    let app_id_bytes: Vec<u8> = row.get(0);
    let app_id = Uuid::from_slice(&app_id_bytes)
        .map(|u: Uuid| u.to_string())
        .unwrap_or_else(|_| hex_encode(&app_id_bytes));
    let key: String = row.get(1);

    sqlx::query("DELETE FROM secret WHERE id = ?")
        .bind(id.as_uuid())
        .execute(&mut *tx)
        .await?;

    append_audit(
        &mut tx,
        AuditEvent {
            id: None,
            ts: now,
            actor: actor.to_string(),
            kind: kind::SECRET_DELETE,
            target: Some(format!("secret:{id}")),
            payload: serde_json::json!({ "app_id": app_id, "key": key }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    Ok(())
}

pub(crate) async fn rotate(
    pool: &Pool<Sqlite>,
    id: SecretId,
    new_ciphertext: Vec<u8>,
    actor: &str,
) -> Result<Secret, AppError> {
    let mut tx = pool.begin().await?;
    let now = Timestamp::now();

    let rows = sqlx::query("UPDATE secret SET ciphertext = ?, rotated_at = ?, \
                            version = version + 1 WHERE id = ?")
        .bind(&new_ciphertext)
        .bind(now.as_secs())
        .bind(id.as_uuid())
        .execute(&mut *tx)
        .await?
        .rows_affected();

    if rows == 0 {
        return Err(AppError::not_found("secret"));
    }

    append_audit(
        &mut tx,
        AuditEvent {
            id: None,
            ts: now,
            actor: actor.to_string(),
            kind: kind::SECRET_ROTATE,
            target: Some(format!("secret:{id}")),
            payload: serde_json::json!({ "id": id.to_string() }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    row::select_secret_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::internal("secret vanished after rotate"))
}

pub(crate) async fn set_status(
    pool: &Pool<Sqlite>,
    id: SecretId,
    status: SecretStatus,
    actor: &str,
) -> Result<Secret, AppError> {
    let mut tx = pool.begin().await?;
    let now = Timestamp::now();

    let rows = sqlx::query("UPDATE secret SET status = ?, version = version + 1 WHERE id = ?")
        .bind(status.to_string())
        .bind(id.as_uuid())
        .execute(&mut *tx)
        .await?
        .rows_affected();

    if rows == 0 {
        return Err(AppError::not_found("secret"));
    }

    append_audit(
        &mut tx,
        AuditEvent {
            id: None,
            ts: now,
            actor: actor.to_string(),
            kind: kind::SECRET_ROTATE,
            target: Some(format!("secret:{id}")),
            payload: serde_json::json!({ "id": id.to_string(), "status": status.to_string() }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    row::select_secret_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::internal("secret vanished after set_status"))
}
