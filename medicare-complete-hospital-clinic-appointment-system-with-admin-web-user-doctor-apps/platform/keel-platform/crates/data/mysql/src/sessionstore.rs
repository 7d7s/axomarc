//! Durable HTTP sessions on MySQL 8 (S7 C6/B-03, B-31 second driver):
//! http_sessions table. Implements the kelson-session `SessionStore` port so
//! browser sessions survive restarts and scale across instances.
//!
//! Dialect notes vs the Postgres twin (`kelson_postgres::sessionstore`):
//! `$n` placeholders become `?`, the `ON CONFLICT ... EXCLUDED` upsert
//! becomes `ON DUPLICATE KEY UPDATE` with the MySQL 8.0.19+ row alias, and
//! `DELETE ... WHERE` counts matched rows identically (sqlx requests
//! CLIENT_FOUND_ROWS).

use kelson_foundation::{AppError, PrincipalId, TenantId};
use kelson_session::{Session, SessionId, SessionPolicy, SessionStore};
use sqlx::mysql::MySqlPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::bridge::block_on;

pub struct MySqlSessionStore {
    pool: MySqlPool,
}

impl MySqlSessionStore {
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }
}

fn my(e: impl std::fmt::Display) -> AppError {
    AppError::persistence(format!("http_sessions: {e}"))
}

fn row_to_session(row: sqlx::mysql::MySqlRow) -> Result<Session, AppError> {
    use sqlx::Row;
    let raw_id: String = row.try_get("id").map_err(my)?;
    let id = SessionId::parse(&raw_id)?;
    let principal: Uuid = row.try_get("principal_id").map_err(my)?;
    let tenant: Option<Uuid> = row.try_get("tenant_id").map_err(my)?;
    let data: serde_json::Value = row.try_get("data").map_err(my)?;
    Ok(Session {
        id,
        principal: PrincipalId::from_uuid(principal),
        tenant: tenant.map(TenantId::from_uuid),
        data: serde_json::from_value(data).map_err(my)?,
        created_at: row.try_get("created_at").map_err(my)?,
        last_seen: row.try_get("last_seen").map_err(my)?,
    })
}

impl SessionStore for MySqlSessionStore {
    fn load(&self, id: &SessionId) -> Result<Option<Session>, AppError> {
        let id = id.as_str().to_string();
        let pool = self.pool.clone();
        block_on(async move {
            let row: Option<sqlx::mysql::MySqlRow> =
                sqlx::query("SELECT * FROM http_sessions WHERE id = ?")
                    .bind(&id)
                    .fetch_optional(&pool)
                    .await
                    .map_err(my)?;
            row.map(row_to_session).transpose()
        })
    }

    fn save(&self, session: &Session) -> Result<(), AppError> {
        let data = serde_json::to_value(&session.data).map_err(my)?;
        let session = session.clone();
        let pool = self.pool.clone();
        block_on(async move {
            sqlx::query(
                "INSERT INTO http_sessions (id, principal_id, tenant_id, data, created_at, last_seen)
                 VALUES (?, ?, ?, ?, ?, ?) AS new
                 ON DUPLICATE KEY UPDATE data = new.data, last_seen = new.last_seen",
            )
            .bind(session.id.as_str())
            .bind(*session.principal.as_uuid())
            .bind(session.tenant.map(|t| *t.as_uuid()))
            .bind(data)
            .bind(session.created_at)
            .bind(session.last_seen)
            .execute(&pool)
            .await
            .map_err(my)?;
            Ok(())
        })
    }

    fn delete(&self, id: &SessionId) -> Result<(), AppError> {
        let id = id.as_str().to_string();
        let pool = self.pool.clone();
        block_on(async move {
            sqlx::query("DELETE FROM http_sessions WHERE id = ?")
                .bind(&id)
                .execute(&pool)
                .await
                .map_err(my)?;
            Ok(())
        })
    }

    fn count(&self) -> usize {
        let pool = self.pool.clone();
        block_on(async move {
            let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM http_sessions")
                .fetch_one(&pool)
                .await
                .map_err(|_| AppError::persistence("http_sessions count"))
                .unwrap_or((0,));
            row.0.max(0) as usize
        })
    }

    fn sweep(&self, now: OffsetDateTime, policy: &SessionPolicy) -> Result<usize, AppError> {
        // Sessions die idle OR absolute-expired (Session::expired semantics).
        // Zero disables that bound, so only attach live conditions.
        let mut clauses: Vec<String> = Vec::new();
        let mut binds: Vec<OffsetDateTime> = Vec::new();
        if policy.idle_secs > 0 {
            binds.push(now - time::Duration::seconds(policy.idle_secs as i64));
            clauses.push("last_seen < ?".to_string());
        }
        if policy.absolute_secs > 0 {
            binds.push(now - time::Duration::seconds(policy.absolute_secs as i64));
            clauses.push("created_at < ?".to_string());
        }
        if clauses.is_empty() {
            return Ok(0);
        }
        let sql = format!("DELETE FROM http_sessions WHERE ({})", clauses.join(" OR "));
        let pool = self.pool.clone();
        block_on(async move {
            let mut q = sqlx::query(&sql);
            for b in binds {
                q = q.bind(b);
            }
            let done = q.execute(&pool).await.map_err(my)?;
            Ok(done.rows_affected() as usize)
        })
    }
}
