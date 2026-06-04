// User writers: `create`, `set_role`, `touch`. The V0 `user` table does
// not store credentials (those live in the V1.5 G21 auth crate); this
// is just the RBAC principal.

use sqlx::{Pool, Sqlite};

use sovereign_core::domain::{
    kind, AuditEvent, NewUser, Timestamp, User, UserId, UserRole,
};
use sovereign_core::error::AppError;

use crate::app::append_audit;
use crate::row;

pub(crate) async fn create(pool: &Pool<Sqlite>, new: NewUser) -> Result<User, AppError> {
    if new.email.trim().is_empty() {
        return Err(AppError::validation("user email must not be empty"));
    }
    let id = UserId::generate();
    let now = Timestamp::now();
    let mut tx = pool.begin().await?;

    let result = sqlx::query(
        "INSERT INTO user (id, email, role, created_at, last_seen) \
         VALUES (?, ?, ?, ?, NULL)",
    )
    .bind(id.as_uuid())
    .bind(&new.email)
    .bind(new.role.to_string())
    .bind(now.as_secs())
    .execute(&mut *tx)
    .await;

    if let Err(e) = result {
        if let sqlx::Error::Database(db) = &e {
            if db.message().contains("UNIQUE") {
                return Err(AppError::validation(format!(
                    "user email `{}` already exists",
                    new.email
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
            kind: kind::USER_LOGIN,
            target: Some(format!("user:{id}")),
            payload: serde_json::json!({
                "email": new.email,
                "role": new.role.to_string(),
                "event": "create",
            }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    row::select_user_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::internal("user vanished after insert"))
}

pub(crate) async fn set_role(
    pool: &Pool<Sqlite>,
    id: UserId,
    role: UserRole,
    actor: &str,
) -> Result<User, AppError> {
    let mut tx = pool.begin().await?;
    let now = Timestamp::now();

    let rows = sqlx::query("UPDATE user SET role = ? WHERE id = ?")
        .bind(role.to_string())
        .bind(id.as_uuid())
        .execute(&mut *tx)
        .await?
        .rows_affected();

    if rows == 0 {
        return Err(AppError::not_found("user"));
    }

    append_audit(
        &mut tx,
        AuditEvent {
            id: None,
            ts: now,
            actor: actor.to_string(),
            kind: kind::USER_LOGIN,
            target: Some(format!("user:{id}")),
            payload: serde_json::json!({ "role": role.to_string(), "event": "set_role" }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;
    row::select_user_by_id(pool, id)
        .await?
        .ok_or_else(|| AppError::internal("user vanished after set_role"))
}

pub(crate) async fn touch(
    pool: &Pool<Sqlite>,
    id: UserId,
    at: Timestamp,
) -> Result<(), AppError> {
    let rows = sqlx::query("UPDATE user SET last_seen = ? WHERE id = ?")
        .bind(at.as_secs())
        .bind(id.as_uuid())
        .execute(pool)
        .await?
        .rows_affected();
    if rows == 0 {
        return Err(AppError::not_found("user"));
    }
    Ok(())
}
