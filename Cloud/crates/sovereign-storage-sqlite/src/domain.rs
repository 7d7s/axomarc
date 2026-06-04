// Domain writers: `add`, `remove`.

use sqlx::{Pool, Row, Sqlite};

use sovereign_core::domain::{
    kind, AuditEvent, Domain, DomainId, NewDomain, Timestamp, TlsStatus,
};
use sovereign_core::error::AppError;

use crate::app::append_audit;
use crate::row;

pub(crate) async fn add(
    pool: &Pool<Sqlite>,
    new: NewDomain,
    actor: &str,
) -> Result<Domain, AppError> {
    if new.hostname.trim().is_empty() {
        return Err(AppError::validation("domain hostname must not be empty"));
    }
    let id = DomainId::generate();
    let now = Timestamp::now();
    let mut tx = pool.begin().await?;

    let result = sqlx::query(
        "INSERT INTO domain (id, app_id, hostname, tls_status, tls_expires, created_at) \
         VALUES (?, ?, ?, ?, NULL, ?)",
    )
    .bind(id.as_uuid())
    .bind(new.app_id.as_uuid())
    .bind(&new.hostname)
    .bind(TlsStatus::Provisioning.to_string())
    .bind(now.as_secs())
    .execute(&mut *tx)
    .await;

    if let Err(e) = result {
        if let sqlx::Error::Database(db) = &e {
            if db.message().contains("UNIQUE") {
                return Err(AppError::validation(format!(
                    "domain hostname `{}` already exists",
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
            actor: actor.to_string(),
            kind: kind::DOMAIN_ADD,
            target: Some(format!("domain:{id}")),
            payload: serde_json::json!({
                "hostname": new.hostname,
                "app_id": new.app_id.to_string(),
            }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    row::select_domains_by_app(pool, new.app_id)
        .await?
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(|| AppError::internal("domain vanished after insert"))
}

pub(crate) async fn remove(
    pool: &Pool<Sqlite>,
    id: DomainId,
    actor: &str,
) -> Result<(), AppError> {
    let mut tx = pool.begin().await?;
    let now = Timestamp::now();

    // Fetch the row first to record the hostname in the audit payload.
    let rows = sqlx::query("SELECT hostname FROM domain WHERE id = ?")
        .bind(id.as_uuid())
        .fetch_optional(&mut *tx)
        .await?;
    let hostname: Option<String> = rows.map(|r| r.get::<String, _>(0));
    let hostname = hostname.ok_or(AppError::not_found("domain"))?;

    sqlx::query("DELETE FROM domain WHERE id = ?")
        .bind(id.as_uuid())
        .execute(&mut *tx)
        .await?;

    append_audit(
        &mut tx,
        AuditEvent {
            id: None,
            ts: now,
            actor: actor.to_string(),
            kind: kind::DOMAIN_REMOVE,
            target: Some(format!("domain:{id}")),
            payload: serde_json::json!({ "hostname": hostname }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    Ok(())
}
