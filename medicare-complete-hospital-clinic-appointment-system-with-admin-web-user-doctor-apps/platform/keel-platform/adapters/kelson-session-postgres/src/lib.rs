//! # kelson-session-postgres
//! PostgreSQL adapter for the `SessionStore` port (kelson-session).
//!
//! Schema (created by migration if needed):
//! ```sql
//! CREATE TABLE kelson_sessions (
//!     id          text PRIMARY KEY,      -- 64-char hex session id
//!     data        jsonb NOT NULL,        -- full Session serialised
//!     expires_at  timestamptz NOT NULL,  -- computed from policy + created_at
//!     created_at  timestamptz NOT NULL DEFAULT now()
//! );
//! CREATE INDEX kelson_sessions_expires_idx ON kelson_sessions (expires_at);
//! ```
//!
//! Session semantics:
//! - `save` -> `INSERT ... ON CONFLICT DO UPDATE` with computed `expires_at`
//! - `load` -> `SELECT data FROM kelson_sessions WHERE id = $1 AND expires_at > now()`
//! - `delete` -> `DELETE FROM kelson_sessions WHERE id = $1`
//! - `sweep` -> `DELETE FROM kelson_sessions WHERE expires_at <= now()` (batched)

use kelson_foundation::AppError;
use kelson_session::{Session, SessionId, SessionPolicy, SessionStore};
use sqlx::postgres::PgPool; // sqlx directly (pgvector precedent): adapters do not depend on the data layer
use sqlx::Row;
use time::{Duration, OffsetDateTime};

/// PostgreSQL-backed [`SessionStore`].
pub struct PgSessionStore {
    pool: PgPool,
    table: String,
}

impl PgSessionStore {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            table: "kelson_sessions".to_string(),
        }
    }

    pub fn with_table(mut self, table: impl Into<String>) -> Self {
        self.table = table.into();
        self
    }

    /// Ensure the sessions table exists (idempotent). Called at startup.
    pub async fn ensure_table(&self) -> Result<(), AppError> {
        let sql = format!(
            r#"
            CREATE TABLE IF NOT EXISTS {} (
                id          text PRIMARY KEY,
                data        jsonb NOT NULL,
                expires_at  timestamptz NOT NULL,
                created_at  timestamptz NOT NULL DEFAULT now()
            );
            CREATE INDEX IF NOT EXISTS {}_expires_idx ON {} (expires_at);
            "#,
            self.table, self.table, self.table
        );
        sqlx::query(&sql)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::persistence(format!("ensure session table: {e}")))?;
        Ok(())
    }

    /// Compute expiry timestamp from session + policy.
    fn expires_at(&self, session: &Session, policy: &SessionPolicy) -> OffsetDateTime {
        let mut expiry = OffsetDateTime::now_utc() + Duration::days(365); // far future default
        if policy.idle_secs > 0 {
            let idle_expiry = session.last_seen + Duration::seconds(policy.idle_secs as i64);
            if idle_expiry < expiry {
                expiry = idle_expiry;
            }
        }
        if policy.absolute_secs > 0 {
            let abs_expiry = session.created_at + Duration::seconds(policy.absolute_secs as i64);
            if abs_expiry < expiry {
                expiry = abs_expiry;
            }
        }
        expiry
    }
}

impl SessionStore for PgSessionStore {
    fn load(&self, id: &SessionId) -> Result<Option<Session>, AppError> {
        let pool = self.pool.clone();
        let table = self.table.clone();
        let id_str = id.as_str().to_string();

        // Blocking load - in practice this runs on the async runtime
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async move {
            let row = sqlx::query(&format!(
                "SELECT data FROM {} WHERE id = $1 AND expires_at > now()",
                table
            ))
            .bind(&id_str)
            .fetch_optional(&pool)
            .await
            .map_err(|e| AppError::persistence(format!("session load: {e}")))?;

            match row {
                Some(row) => {
                    let data: serde_json::Value = row
                        .try_get("data")
                        .map_err(|e| AppError::persistence(format!("session row get: {e}")))?;
                    let session: Session = serde_json::from_value(data)
                        .map_err(|e| AppError::persistence(format!("session deserialize: {e}")))?;
                    Ok(Some(session))
                }
                None => Ok(None),
            }
        })
    }

    fn save(&self, session: &Session) -> Result<(), AppError> {
        let pool = self.pool.clone();
        let table = self.table.clone();
        let id = session.id.as_str().to_string();
        let raw = serde_json::to_string(session)
            .map_err(|e| AppError::persistence(format!("session serialize: {e}")))?;
        let expires_at = self.expires_at(session, &SessionPolicy::default());

        let rt = tokio::runtime::Handle::current();
        rt.block_on(async move {
            sqlx::query(&format!(
                r#"
                    INSERT INTO {} (id, data, expires_at, created_at)
                    VALUES ($1, $2, $3, now())
                    ON CONFLICT (id) DO UPDATE SET
                        data = EXCLUDED.data,
                        expires_at = EXCLUDED.expires_at
                    "#,
                table
            ))
            .bind(&id)
            .bind(&raw)
            .bind(expires_at)
            .execute(&pool)
            .await
            .map_err(|e| AppError::persistence(format!("session save: {e}")))?;
            Ok(())
        })
    }

    fn delete(&self, id: &SessionId) -> Result<(), AppError> {
        let pool = self.pool.clone();
        let table = self.table.clone();
        let id_str = id.as_str().to_string();

        let rt = tokio::runtime::Handle::current();
        rt.block_on(async move {
            let _ = sqlx::query(&format!("DELETE FROM {} WHERE id = $1", table))
                .bind(&id_str)
                .execute(&pool)
                .await;
            Ok(())
        })
    }

    fn count(&self) -> usize {
        let pool = self.pool.clone();
        let table = self.table.clone();

        let rt = tokio::runtime::Handle::current();
        rt.block_on(async move {
            let count: Option<i64> = sqlx::query_scalar(&format!(
                "SELECT count(*) FROM {} WHERE expires_at > now()",
                table
            ))
            .fetch_optional(&pool)
            .await
            .unwrap_or(None);
            count.unwrap_or(0) as usize
        })
    }

    fn sweep(&self, now: OffsetDateTime, _policy: &SessionPolicy) -> Result<usize, AppError> {
        let pool = self.pool.clone();
        let table = self.table.clone();
        let now_iso = now
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| now.to_string());

        let rt = tokio::runtime::Handle::current();
        rt.block_on(async move {
            let deleted = sqlx::query(&format!("DELETE FROM {} WHERE expires_at <= $1", table))
                .bind(now_iso)
                .execute(&pool)
                .await
                .map_err(|e| AppError::persistence(format!("session sweep: {e}")))?;
            Ok(deleted.rows_affected() as usize)
        })
    }
}

#[cfg(test)]
mod tests {
    // expires_at computation is tested indirectly via integration tests with a live Postgres.
    // Unit test here requires Tokio context for PgPool::connect_lazy.
}
