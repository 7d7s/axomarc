// User writers: `create`, `set_role`, `touch`. The V0 `user` table does
// not store credentials (those live in the V1.5 G21 auth crate); this
// is just the RBAC principal.

use sqlx::{Pool, Sqlite};
use uuid::Uuid;

use sovereign_core::domain::{kind, AuditEvent, NewUser, Timestamp, User, UserId, UserRole};
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

pub(crate) async fn touch(pool: &Pool<Sqlite>, id: UserId, at: Timestamp) -> Result<(), AppError> {
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

pub(crate) async fn set_password_hash(
    pool: &Pool<Sqlite>,
    id: UserId,
    hash: Option<&str>,
) -> Result<(), AppError> {
    let rows = sqlx::query("UPDATE user SET password_hash = ? WHERE id = ?")
        .bind(hash)
        .bind(id.as_uuid())
        .execute(pool)
        .await?
        .rows_affected();
    if rows == 0 {
        return Err(AppError::not_found("user"));
    }
    Ok(())
}

pub(crate) async fn get_password_hash(
    pool: &Pool<Sqlite>,
    id: UserId,
) -> Result<Option<String>, AppError> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "SELECT password_hash FROM user WHERE id = ?",
    )
    .bind(id.as_uuid())
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|r| r.0))
}

pub(crate) async fn set_display_name(
    pool: &Pool<Sqlite>,
    id: UserId,
    display_name: &str,
) -> Result<(), AppError> {
    let rows = sqlx::query("UPDATE user SET display_name = ? WHERE id = ?")
        .bind(display_name)
        .bind(id.as_uuid())
        .execute(pool)
        .await?
        .rows_affected();
    if rows == 0 {
        return Err(AppError::not_found("user"));
    }
    Ok(())
}

pub(crate) async fn disable(pool: &Pool<Sqlite>, id: UserId, at: Timestamp) -> Result<(), AppError> {
    let rows = sqlx::query("UPDATE user SET disabled_at = ? WHERE id = ? AND disabled_at IS NULL")
        .bind(at.as_secs())
        .bind(id.as_uuid())
        .execute(pool)
        .await?
        .rows_affected();
    if rows == 0 {
        return Err(AppError::not_found("user or already disabled"));
    }
    Ok(())
}

pub(crate) async fn enable(pool: &Pool<Sqlite>, id: UserId) -> Result<(), AppError> {
    let rows = sqlx::query("UPDATE user SET disabled_at = NULL WHERE id = ?")
        .bind(id.as_uuid())
        .execute(pool)
        .await?
        .rows_affected();
    if rows == 0 {
        return Err(AppError::not_found("user"));
    }
    Ok(())
}

pub(crate) async fn create_api_token(
    pool: &Pool<Sqlite>,
    user_id: UserId,
    name: &str,
    hash: &str,
    scopes: &str,
    created_at: Timestamp,
    expires_at: Option<Timestamp>,
) -> Result<sovereign_core::domain::ApiToken, AppError> {
    use sovereign_core::domain::id::UserId;
    let id = UserId::generate();
    let id_bytes = id.as_uuid().as_bytes().to_vec();
    let mut tx = pool.begin().await?;

    let result = sqlx::query(
        "INSERT INTO api_token (id, user_id, name, hash, scopes, created_at, expires_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id_bytes)
    .bind(user_id.as_uuid())
    .bind(name)
    .bind(hash)
    .bind(scopes)
    .bind(created_at.as_secs())
    .bind(expires_at.map(|t| t.as_secs()))
    .execute(&mut *tx)
    .await;

    if let Err(e) = result {
        if let sqlx::Error::Database(db) = &e {
            if db.message().contains("UNIQUE") {
                return Err(AppError::validation(format!(
                    "token `{name}` already exists for this user"
                )));
            }
        }
        return Err(AppError::from(e));
    }

    append_audit(
        &mut tx,
        AuditEvent {
            id: None,
            ts: created_at,
            actor: format!("user:{user_id}"),
            kind: kind::USER_LOGIN,
            target: Some(format!("token:{name}")),
            payload: serde_json::json!({
                "event": "token_create",
                "name": name,
                "scopes": scopes,
            }),
            policy_decision: None,
        },
    )
    .await?;

    tx.commit().await?;

    Ok(sovereign_core::domain::ApiToken {
        id: id_bytes,
        user_id: user_id.as_uuid().as_bytes().to_vec(),
        name: name.to_string(),
        hash: hash.to_string(),
        scopes: scopes.to_string(),
        created_at,
        expires_at,
    })
}

pub(crate) async fn list_api_tokens(
    pool: &Pool<Sqlite>,
    user_id: UserId,
) -> Result<Vec<sovereign_core::domain::ApiToken>, AppError> {
    #[derive(sqlx::FromRow)]
    struct TokenRow {
        id: Vec<u8>,
        user_id: Vec<u8>,
        name: String,
        hash: String,
        scopes: String,
        created_at: i64,
        expires_at: Option<i64>,
    }

    let rows: Vec<TokenRow> = sqlx::query_as(
        "SELECT id, user_id, name, hash, scopes, created_at, expires_at \
         FROM api_token WHERE user_id = ? ORDER BY created_at DESC",
    )
    .bind(user_id.as_uuid())
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| sovereign_core::domain::ApiToken {
            id: r.id,
            user_id: r.user_id,
            name: r.name,
            hash: r.hash,
            scopes: r.scopes,
            created_at: Timestamp::from(r.created_at),
            expires_at: r.expires_at.map(Timestamp::from),
        })
        .collect())
}

pub(crate) async fn get_api_token(
    pool: &Pool<Sqlite>,
    user_id: UserId,
    name: &str,
) -> Result<Option<sovereign_core::domain::ApiToken>, AppError> {
    #[derive(sqlx::FromRow)]
    struct TokenRow {
        id: Vec<u8>,
        user_id: Vec<u8>,
        name: String,
        hash: String,
        scopes: String,
        created_at: i64,
        expires_at: Option<i64>,
    }

    let row: Option<TokenRow> = sqlx::query_as(
        "SELECT id, user_id, name, hash, scopes, created_at, expires_at \
         FROM api_token WHERE user_id = ? AND name = ?",
    )
    .bind(user_id.as_uuid())
    .bind(name)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| sovereign_core::domain::ApiToken {
        id: r.id,
        user_id: r.user_id,
        name: r.name,
        hash: r.hash,
        scopes: r.scopes,
        created_at: Timestamp::from(r.created_at),
        expires_at: r.expires_at.map(Timestamp::from),
    }))
}

pub(crate) async fn delete_api_token(
    pool: &Pool<Sqlite>,
    user_id: UserId,
    name: &str,
) -> Result<(), AppError> {
    let rows = sqlx::query("DELETE FROM api_token WHERE user_id = ? AND name = ?")
        .bind(user_id.as_uuid())
        .bind(name)
        .execute(pool)
        .await?
        .rows_affected();
    if rows == 0 {
        return Err(AppError::not_found("token"));
    }
    Ok(())
}

pub(crate) async fn get_bootstrap_admin(
    pool: &Pool<Sqlite>,
) -> Result<Option<UserId>, AppError> {
    let row: Option<(Vec<u8>,)> = sqlx::query_as(
        "SELECT admin_user_id FROM bootstrap_state WHERE id = 1",
    )
    .fetch_optional(pool)
    .await?;

    match row {
        Some((id_bytes,)) => {
            let uuid = Uuid::from_slice(&id_bytes)
                .map_err(|e| AppError::internal(format!("invalid bootstrap UUID: {e}")))?;
            Ok(Some(UserId::from(uuid)))
        }
        None => Ok(None),
    }
}

pub(crate) async fn set_bootstrap_admin(
    pool: &Pool<Sqlite>,
    user_id: UserId,
) -> Result<(), AppError> {
    let rows = sqlx::query(
        "INSERT INTO bootstrap_state (id, admin_user_id) VALUES (1, ?) \
         ON CONFLICT(id) DO UPDATE SET admin_user_id = ?",
    )
    .bind(user_id.as_uuid())
    .bind(user_id.as_uuid())
    .execute(pool)
    .await?
    .rows_affected();
    let _ = rows;
    Ok(())
}
