//! Auth tokens on PostgreSQL (S2 item 21): auth_tokens table. Only sha256
//! hashes are stored; plaintext exists once, in the issue response.
//! mark_consumed is atomic (UPDATE ... WHERE consumed_at IS NULL), so
//! concurrent redemptions of a single-use token cannot both win.

use kelson_foundation::{AppError, PrincipalId};
use kelson_tokens::{AuthToken, TokenPurpose, TokenStore};
use sqlx::postgres::PgPool;

use crate::bridge::block_on;
use time::OffsetDateTime;
use uuid::Uuid;

pub struct PgTokenStore {
    pool: PgPool,
}

impl PgTokenStore {
    pub fn new(pool: PgPool) -> Self {
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

fn token_from_row(row: sqlx::postgres::PgRow) -> Result<AuthToken, AppError> {
    use sqlx::Row;
    let principal_id: Uuid = row.try_get("principal_id").map_err(pg)?;
    let purpose = purpose_from(&row.try_get::<String, _>("purpose").map_err(pg)?)?;
    Ok(AuthToken {
        id: row.try_get("id").map_err(pg)?,
        principal_id: PrincipalId::from_uuid(principal_id),
        purpose,
        token_hash: row.try_get("token_hash").map_err(pg)?,
        label: row.try_get("label").map_err(pg)?,
        expires_at: row.try_get("expires_at").map_err(pg)?,
        consumed_at: row.try_get("consumed_at").map_err(pg)?,
        created_at: row.try_get("created_at").map_err(pg)?,
    })
}

fn pg(e: impl std::fmt::Display) -> AppError {
    AppError::persistence(format!("auth_tokens: {e}"))
}

impl TokenStore for PgTokenStore {
    fn insert(&self, token: AuthToken) -> Result<(), AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            sqlx::query(
                "INSERT INTO auth_tokens (id, principal_id, purpose, token_hash, label, expires_at, consumed_at, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
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
            .map_err(pg)?;
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
            let row: Option<sqlx::postgres::PgRow> =
                sqlx::query("SELECT * FROM auth_tokens WHERE purpose = $1 AND token_hash = $2")
                    .bind(purpose.as_str())
                    .bind(&hash)
                    .fetch_optional(&pool)
                    .await
                    .map_err(pg)?;
            row.map(token_from_row).transpose()
        })
    }

    fn mark_consumed(&self, id: Uuid, at: OffsetDateTime) -> Result<bool, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query(
                "UPDATE auth_tokens SET consumed_at = $2
                 WHERE id = $1 AND consumed_at IS NULL",
            )
            .bind(id)
            .bind(at)
            .execute(&pool)
            .await
            .map_err(pg)?;
            Ok(done.rows_affected() > 0)
        })
    }

    fn revoke(&self, id: Uuid) -> Result<(), AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let done = sqlx::query("DELETE FROM auth_tokens WHERE id = $1")
                .bind(id)
                .execute(&pool)
                .await
                .map_err(pg)?;
            if done.rows_affected() == 0 {
                return Err(AppError::not_found(format!("token {id}")));
            }
            Ok(())
        })
    }

    fn list_for(&self, principal: PrincipalId) -> Result<Vec<AuthToken>, AppError> {
        let pool = self.pool.clone();
        block_on(async move {
            let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(
                "SELECT * FROM auth_tokens WHERE principal_id = $1 ORDER BY created_at DESC",
            )
            .bind(*principal.as_uuid())
            .fetch_all(&pool)
            .await
            .map_err(pg)?;
            rows.into_iter().map(token_from_row).collect()
        })
    }
}
