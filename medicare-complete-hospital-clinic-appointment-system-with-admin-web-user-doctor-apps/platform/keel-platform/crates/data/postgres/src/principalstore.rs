//! Identity stores on PostgreSQL (S7): principals + credentials tables.
//! Mirrors the tokenstore.rs pattern: sync trait over sqlx via block_on.

use kelson_foundation::{AppError, PrincipalId};
use kelson_identity::{
    store::{CredentialStore, PrincipalStore},
    Credential, CredentialKind, Principal, PrincipalStatus, Secret,
};
use sqlx::postgres::PgPool;

use crate::bridge::block_on;
use uuid::Uuid;

pub struct PgPrincipalStore {
    pool: PgPool,
}

impl PgPrincipalStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

pub struct PgCredentialStore {
    pool: PgPool,
}

impl PgCredentialStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}


fn pg(e: impl std::fmt::Display) -> AppError {
    AppError::persistence(format!("identity: {e}"))
}

fn status_text(status: PrincipalStatus) -> &'static str {
    match status {
        PrincipalStatus::Active => "active",
        PrincipalStatus::Suspended => "suspended",
        PrincipalStatus::Deleted => "deleted",
    }
}

fn status_from(text: &str) -> Result<PrincipalStatus, AppError> {
    Ok(match text {
        "active" => PrincipalStatus::Active,
        "suspended" => PrincipalStatus::Suspended,
        "deleted" => PrincipalStatus::Deleted,
        other => {
            return Err(AppError::internal(format!(
                "unknown principal status {other}"
            )))
        }
    })
}

fn kind_text(kind: CredentialKind) -> &'static str {
    match kind {
        CredentialKind::PasswordArgon2 => "password_argon2",
        CredentialKind::Otp => "otp",
        CredentialKind::ApiKey => "api_key",
    }
}

fn kind_from(text: &str) -> Result<CredentialKind, AppError> {
    Ok(match text {
        "password_argon2" => CredentialKind::PasswordArgon2,
        "otp" => CredentialKind::Otp,
        "api_key" => CredentialKind::ApiKey,
        other => {
            return Err(AppError::internal(format!(
                "unknown credential kind {other}"
            )))
        }
    })
}

fn principal_from_row(row: sqlx::postgres::PgRow) -> Result<Principal, AppError> {
    use sqlx::Row;
    let id: Uuid = row.try_get("id").map_err(pg)?;
    Ok(Principal {
        id: PrincipalId::from_uuid(id),
        status: status_from(&row.try_get::<String, _>("status").map_err(pg)?)?,
        email: row.try_get("email").map_err(pg)?,
        phone: row.try_get("phone").map_err(pg)?,
        email_verified_at: row.try_get("email_verified_at").map_err(pg)?,
        created_at: row.try_get("created_at").map_err(pg)?,
    })
}

fn credential_from_row(row: sqlx::postgres::PgRow) -> Result<Credential, AppError> {
    use sqlx::Row;
    let principal_id: Uuid = row.try_get("principal_id").map_err(pg)?;
    Ok(Credential {
        id: row.try_get("id").map_err(pg)?,
        principal_id: PrincipalId::from_uuid(principal_id),
        kind: kind_from(&row.try_get::<String, _>("kind").map_err(pg)?)?,
        secret_hash: Secret::new(row.try_get::<String, _>("secret_hash").map_err(pg)?),
        created_at: row.try_get("created_at").map_err(pg)?,
        expires_at: row.try_get("expires_at").map_err(pg)?,
    })
}

impl PrincipalStore for PgPrincipalStore {
    fn create(&self, principal: Principal) -> Result<Principal, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let result = sqlx::query(
                "INSERT INTO principals (id, status, email, phone, created_at)
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(*principal.id.as_uuid())
            .bind(status_text(principal.status))
            .bind(&principal.email)
            .bind(&principal.phone)
            .bind(principal.created_at)
            .execute(&pool)
            .await;
            match result {
                Ok(_) => Ok(principal),
                Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                    Err(AppError::conflict("principal identity already exists"))
                }
                Err(e) => Err(pg(e)),
            }
        })
    }

    fn by_id(&self, id: PrincipalId) -> Result<Option<Principal>, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let row: Option<sqlx::postgres::PgRow> =
                sqlx::query("SELECT * FROM principals WHERE id = $1")
                    .bind(*id.as_uuid())
                    .fetch_optional(&pool)
                    .await
                    .map_err(pg)?;
            row.map(principal_from_row).transpose()
        })
    }

    fn by_email(&self, email: &str) -> Result<Option<Principal>, AppError> {
        let email = email.to_string();
        let pool = self.pool.clone();
        block_on(async move {
            let row: Option<sqlx::postgres::PgRow> =
                sqlx::query("SELECT * FROM principals WHERE email = $1")
                    .bind(&email)
                    .fetch_optional(&pool)
                    .await
                    .map_err(pg)?;
            row.map(principal_from_row).transpose()
        })
    }

    fn update_status(&self, id: PrincipalId, status: PrincipalStatus) -> Result<(), AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query("UPDATE principals SET status = $2 WHERE id = $1")
                .bind(*id.as_uuid())
                .bind(status_text(status))
                .execute(&pool)
                .await
                .map_err(pg)?;
            if done.rows_affected() == 0 {
                return Err(AppError::not_found(format!("principal {id}")));
            }
            Ok(())
        })
    }

    fn mark_email_verified(&self, id: PrincipalId) -> Result<(), AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query(
                "UPDATE principals SET email_verified_at = now()
                 WHERE id = $1 AND email_verified_at IS NULL",
            )
            .bind(*id.as_uuid())
            .execute(&pool)
            .await
            .map_err(pg)?;
            // rows_affected is 0 both for unknown principals and for
            // already-verified ones; verification is idempotent, so only
            // existence matters.
            if done.rows_affected() == 0 {
                let exists: Option<(Uuid,)> =
                    sqlx::query_as("SELECT id FROM principals WHERE id = $1")
                        .bind(*id.as_uuid())
                        .fetch_optional(&pool)
                        .await
                        .map_err(pg)?;
                if exists.is_none() {
                    return Err(AppError::not_found(format!("principal {id}")));
                }
            }
            Ok(())
        })
    }

    fn first_active(&self) -> Result<Option<Principal>, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let row = sqlx::query(
                "SELECT * FROM principals WHERE status = $1
                 ORDER BY created_at, id LIMIT 1",
            )
            .bind(status_text(PrincipalStatus::Active))
            .fetch_optional(&pool)
            .await
            .map_err(pg)?;
            row.map(principal_from_row).transpose()
        })
    }
}

impl CredentialStore for PgCredentialStore {
    fn create(&self, credential: Credential) -> Result<Credential, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let result = sqlx::query(
                "INSERT INTO credentials (id, principal_id, kind, secret_hash, created_at, expires_at)
                 VALUES ($1, $2, $3, $4, $5, $6)",
            )
            .bind(credential.id)
            .bind(*credential.principal_id.as_uuid())
            .bind(kind_text(credential.kind))
            .bind(credential.secret_hash.expose())
            .bind(credential.created_at)
            .bind(credential.expires_at)
            .execute(&pool)
            .await;
            match result {
                Ok(_) => Ok(credential),
                Err(sqlx::Error::Database(db)) if db.code().as_deref() == Some("23505") => {
                    Err(AppError::conflict("credential already exists"))
                }
                Err(e) => Err(pg(e)),
            }
        })
    }

    fn password_for(&self, principal: PrincipalId) -> Result<Option<Credential>, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let row: Option<sqlx::postgres::PgRow> = sqlx::query(
                "SELECT * FROM credentials WHERE principal_id = $1 AND kind = 'password_argon2'",
            )
            .bind(*principal.as_uuid())
            .fetch_optional(&pool)
            .await
            .map_err(pg)?;
            row.map(credential_from_row).transpose()
        })
    }

    fn replace_password_hash(
        &self,
        principal: PrincipalId,
        new_hash: Secret,
    ) -> Result<(), AppError> {
        let hash = new_hash.expose().to_string();
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query(
                "UPDATE credentials SET secret_hash = $2
                 WHERE principal_id = $1 AND kind = 'password_argon2'",
            )
            .bind(*principal.as_uuid())
            .bind(&hash)
            .execute(&pool)
            .await
            .map_err(pg)?;
            if done.rows_affected() == 0 {
                return Err(AppError::not_found(format!(
                    "password credential for {principal}"
                )));
            }
            Ok(())
        })
    }
}
