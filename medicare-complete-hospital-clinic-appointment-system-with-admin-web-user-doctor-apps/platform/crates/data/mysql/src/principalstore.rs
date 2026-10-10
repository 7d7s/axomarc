//! Identity stores on MySQL 8 (S7, B-31 second driver): principals +
//! credentials tables. Mirrors the tokenstore.rs pattern: sync trait over
//! sqlx via block_on; implements kelson-identity's `PrincipalStore` and
//! `CredentialStore` on the foundation tables.
//!
//! MySQL port notes (vs the Postgres twin `kelson_postgres::principalstore`):
//! - `$1, $2, ...` become `?` in bind order: the `SET x = $2 WHERE y = $1`
//!   statements bind in MySQL appearance order (`SET x` first, then `WHERE
//!   y`); no index is ever formatted into SQL.
//! - `now()` becomes `NOW(6)`; `email_verified_at` is a `DATETIME(6)`
//!   column and every `OffsetDateTime` bind/get stays unchanged (rule 3).
//! - Duplicate keys: the twin's `is_unique_violation()` and its literal
//!   `code() == "23505"` check both become `is_unique_violation()` here --
//!   sqlx maps MySQL 1062 (ER_DUP_ENTRY) to `ErrorKind::UniqueViolation`,
//!   while MySQL's SQLSTATE for it (23000) is shared with foreign-key
//!   violations, so a literal code check would misclassify those as
//!   conflicts. Error behavior is otherwise verbatim: any unique key
//!   (id / email / phone) still yields `conflict`, anything else `identity:`.
//! - No `ON CONFLICT`, no `RETURNING`, no RLS/`set_config` and no tenant
//!   predicates exist in the twin statements, so rules 6/10/11 require no
//!   rewrite: `principals`/`credentials` have no `tenant_id` column in
//!   either dialect (the credentials lookup keeps its explicit
//!   `principal_id` + `kind` predicates).
//! - rows_affected (rule 16): sqlx-mysql requests CLIENT_FOUND_ROWS, so
//!   UPDATE counts matched rows exactly like pg -- `update_status`,
//!   `mark_email_verified` (with its existence re-check) and
//!   `replace_password_hash` keep their zero-rows branches verbatim.
//! - Case sensitivity: `email` is `VARCHAR(255)` under
//!   `utf8mb4_0900_ai_ci`, so `by_email` and the UNIQUE email index are
//!   case/accent-insensitive where pg's `text` equality is case-sensitive
//!   (the SQL is unchanged per the port brief; a differently-cased email
//!   now resolves to -- and collides with -- the stored one).

use kelson_foundation::{AppError, PrincipalId};
use kelson_identity::{
    store::{CredentialStore, PrincipalStore},
    Credential, CredentialKind, Principal, PrincipalStatus, Secret,
};
use sqlx::mysql::MySqlPool;

use crate::bridge::block_on;
use uuid::Uuid;

pub struct MySqlPrincipalStore {
    pool: MySqlPool,
}

impl MySqlPrincipalStore {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
}

pub struct MySqlCredentialStore {
    pool: MySqlPool,
}

impl MySqlCredentialStore {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
}


fn my(e: impl std::fmt::Display) -> AppError {
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

fn principal_from_row(row: sqlx::mysql::MySqlRow) -> Result<Principal, AppError> {
    use sqlx::Row;
    let id: Uuid = row.try_get("id").map_err(my)?;
    Ok(Principal {
        id: PrincipalId::from_uuid(id),
        status: status_from(&row.try_get::<String, _>("status").map_err(my)?)?,
        email: row.try_get("email").map_err(my)?,
        phone: row.try_get("phone").map_err(my)?,
        email_verified_at: row.try_get("email_verified_at").map_err(my)?,
        created_at: row.try_get("created_at").map_err(my)?,
    })
}

fn credential_from_row(row: sqlx::mysql::MySqlRow) -> Result<Credential, AppError> {
    use sqlx::Row;
    let principal_id: Uuid = row.try_get("principal_id").map_err(my)?;
    Ok(Credential {
        id: row.try_get("id").map_err(my)?,
        principal_id: PrincipalId::from_uuid(principal_id),
        kind: kind_from(&row.try_get::<String, _>("kind").map_err(my)?)?,
        secret_hash: Secret::new(row.try_get::<String, _>("secret_hash").map_err(my)?),
        created_at: row.try_get("created_at").map_err(my)?,
        expires_at: row.try_get("expires_at").map_err(my)?,
    })
}

impl PrincipalStore for MySqlPrincipalStore {
    fn create(&self, principal: Principal) -> Result<Principal, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let result = sqlx::query(
                "INSERT INTO principals (id, status, email, phone, created_at)
                 VALUES (?, ?, ?, ?, ?)",
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
                Err(e) => Err(my(e)),
            }
        })
    }

    fn by_id(&self, id: PrincipalId) -> Result<Option<Principal>, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let row: Option<sqlx::mysql::MySqlRow> =
                sqlx::query("SELECT * FROM principals WHERE id = ?")
                    .bind(*id.as_uuid())
                    .fetch_optional(&pool)
                    .await
                    .map_err(my)?;
            row.map(principal_from_row).transpose()
        })
    }

    fn by_email(&self, email: &str) -> Result<Option<Principal>, AppError> {
        let email = email.to_string();
        let pool = self.pool.clone();
        block_on(async move {
            let row: Option<sqlx::mysql::MySqlRow> =
                sqlx::query("SELECT * FROM principals WHERE email = ?")
                    .bind(&email)
                    .fetch_optional(&pool)
                    .await
                    .map_err(my)?;
            row.map(principal_from_row).transpose()
        })
    }

    fn update_status(&self, id: PrincipalId, status: PrincipalStatus) -> Result<(), AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query("UPDATE principals SET status = ? WHERE id = ?")
                .bind(status_text(status))
                .bind(*id.as_uuid())
                .execute(&pool)
                .await
                .map_err(my)?;
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
                "UPDATE principals SET email_verified_at = NOW(6)
                 WHERE id = ? AND email_verified_at IS NULL",
            )
            .bind(*id.as_uuid())
            .execute(&pool)
            .await
            .map_err(my)?;
            // rows_affected is 0 both for unknown principals and for
            // already-verified ones; verification is idempotent, so only
            // existence matters.
            if done.rows_affected() == 0 {
                let exists: Option<(Uuid,)> =
                    sqlx::query_as("SELECT id FROM principals WHERE id = ?")
                        .bind(*id.as_uuid())
                        .fetch_optional(&pool)
                        .await
                        .map_err(my)?;
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
                "SELECT * FROM principals WHERE status = ?
                 ORDER BY created_at, id LIMIT 1",
            )
            .bind(status_text(PrincipalStatus::Active))
            .fetch_optional(&pool)
            .await
            .map_err(my)?;
            row.map(principal_from_row).transpose()
        })
    }
}

impl CredentialStore for MySqlCredentialStore {
    fn create(&self, credential: Credential) -> Result<Credential, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let result = sqlx::query(
                "INSERT INTO credentials (id, principal_id, kind, secret_hash, created_at, expires_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
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
                Err(sqlx::Error::Database(db)) if db.is_unique_violation() => {
                    Err(AppError::conflict("credential already exists"))
                }
                Err(e) => Err(my(e)),
            }
        })
    }

    fn password_for(&self, principal: PrincipalId) -> Result<Option<Credential>, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let row: Option<sqlx::mysql::MySqlRow> = sqlx::query(
                "SELECT * FROM credentials WHERE principal_id = ? AND kind = 'password_argon2'",
            )
            .bind(*principal.as_uuid())
            .fetch_optional(&pool)
            .await
            .map_err(my)?;
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
                "UPDATE credentials SET secret_hash = ?
                 WHERE principal_id = ? AND kind = 'password_argon2'",
            )
            .bind(&hash)
            .bind(*principal.as_uuid())
            .execute(&pool)
            .await
            .map_err(my)?;
            if done.rows_affected() == 0 {
                return Err(AppError::not_found(format!(
                    "password credential for {principal}"
                )));
            }
            Ok(())
        })
    }
}
