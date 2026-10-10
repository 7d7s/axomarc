//! Auth tokens on MySQL 8 (S2 item 21, B-31 second driver): auth_tokens
//! table. Only sha256 hashes are stored; plaintext exists once, in the issue
//! response. mark_consumed is atomic (UPDATE ... WHERE consumed_at IS NULL),
//! so concurrent redemptions of a single-use token cannot both win.
//!
//! MySQL port notes (vs the Postgres twin `kelson_postgres::tokenstore`):
//! - `$1, $2` become `?` in bind order: `UPDATE ... SET consumed_at = $2
//!   WHERE id = $1` binds `at` first, then `id`; no index is ever
//!   formatted into SQL.
//! - Timestamp columns are `DATETIME(6)`; every `OffsetDateTime` bind/get
//!   (`expires_at`, `consumed_at`, `created_at`, the `mark_consumed` `at`)
//!   stays unchanged (rule 3).
//! - No `ON CONFLICT`, no `RETURNING`, no RLS/`set_config` and no tenant
//!   predicates in the twin statements, so rules 6/10/11 need no rewrite:
//!   `auth_tokens` has no `tenant_id` column in either dialect, and every
//!   predicate it does carry (`purpose`, `token_hash`, `principal_id`,
//!   `consumed_at IS NULL`) is kept verbatim.
//! - rows_affected (rule 16): sqlx-mysql requests CLIENT_FOUND_ROWS, so
//!   UPDATE/DELETE count matched rows exactly like pg -- `mark_consumed`
//!   still returns false when the WHERE clause matches no row (already
//!   consumed / gone) and `revoke` still errors NOT_FOUND on 0 rows.
//! - Case sensitivity: `token_hash` is `VARCHAR(64)` under
//!   `utf8mb4_0900_ai_ci`, so `find_by_hash` equality (and its UNIQUE
//!   index) is case/accent-insensitive where pg's `text` equality is
//!   case-sensitive. kelson-tokens emits lowercase hex, so lookups behave
//!   identically in practice; the SQL is unchanged per the port brief.

use kelson_foundation::{AppError, PrincipalId};
use kelson_tokens::{AuthToken, TokenPurpose, TokenStore};
use sqlx::mysql::MySqlPool;

use crate::bridge::block_on;
use time::OffsetDateTime;
use uuid::Uuid;

pub struct MySqlTokenStore {
    pool: MySqlPool,
}

impl MySqlTokenStore {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
}


fn purpose_from(text: &str) -> Result<TokenPurpose, AppError> {
    Ok(match text {
        "api" => TokenPurpose::Api,
        "reset" => TokenPurpose::Reset,
        "verify" => TokenPurpose::Verify,
        "oauth_code" => TokenPurpose::OauthCode,
        "oauth_refresh" => TokenPurpose::OauthRefresh,
        "signed_url" => TokenPurpose::SignedUrl,
        other => return Err(AppError::internal(format!("unknown token purpose {other}"))),
    })
}

fn token_from_row(row: sqlx::mysql::MySqlRow) -> Result<AuthToken, AppError> {
    use sqlx::Row;
    let principal_id: Uuid = row.try_get("principal_id").map_err(my)?;
    let purpose = purpose_from(&row.try_get::<String, _>("purpose").map_err(my)?)?;
    Ok(AuthToken {
        id: row.try_get("id").map_err(my)?,
        principal_id: PrincipalId::from_uuid(principal_id),
        purpose,
        token_hash: row.try_get("token_hash").map_err(my)?,
        label: row.try_get("label").map_err(my)?,
        expires_at: row.try_get("expires_at").map_err(my)?,
        consumed_at: row.try_get("consumed_at").map_err(my)?,
        created_at: row.try_get("created_at").map_err(my)?,
    })
}

fn my(e: impl std::fmt::Display) -> AppError {
    AppError::persistence(format!("auth_tokens: {e}"))
}

impl TokenStore for MySqlTokenStore {
    fn insert(&self, token: AuthToken) -> Result<(), AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            sqlx::query(
                "INSERT INTO auth_tokens (id, principal_id, purpose, token_hash, label, expires_at, consumed_at, created_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(token.id)
            .bind(*token.principal_id.as_uuid())
            .bind(token.purpose.as_str())
            .bind(&token.token_hash)
            .bind(&token.label)
            .bind(token.expires_at)
            .bind(token.consumed_at)
            .bind(token.created_at)
            .execute(&pool)
            .await
            .map_err(my)?;
            Ok(())
        })
    }

    fn find_by_hash(
        &self,
        purpose: TokenPurpose,
        hash: &str,
    ) -> Result<Option<AuthToken>, AppError> {
        let hash = hash.to_string();
        let pool = self.pool.clone();
        block_on(async move {
            let row: Option<sqlx::mysql::MySqlRow> =
                sqlx::query("SELECT * FROM auth_tokens WHERE purpose = ? AND token_hash = ?")
                    .bind(purpose.as_str())
                    .bind(&hash)
                    .fetch_optional(&pool)
                    .await
                    .map_err(my)?;
            row.map(token_from_row).transpose()
        })
    }

    fn mark_consumed(&self, id: Uuid, at: OffsetDateTime) -> Result<bool, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query(
                "UPDATE auth_tokens SET consumed_at = ?
                 WHERE id = ? AND consumed_at IS NULL",
            )
            .bind(at)
            .bind(id)
            .execute(&pool)
            .await
            .map_err(my)?;
            Ok(done.rows_affected() > 0)
        })
    }

    fn revoke(&self, id: Uuid) -> Result<(), AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query("DELETE FROM auth_tokens WHERE id = ?")
                .bind(id)
                .execute(&pool)
                .await
                .map_err(my)?;
            if done.rows_affected() == 0 {
                return Err(AppError::not_found(format!("token {id}")));
            }
            Ok(())
        })
    }

    fn list_for(&self, principal: PrincipalId) -> Result<Vec<AuthToken>, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let rows: Vec<sqlx::mysql::MySqlRow> = sqlx::query(
                "SELECT * FROM auth_tokens WHERE principal_id = ? ORDER BY created_at DESC",
            )
            .bind(*principal.as_uuid())
            .fetch_all(&pool)
            .await
            .map_err(my)?;
            rows.into_iter().map(token_from_row).collect()
        })
    }
}
