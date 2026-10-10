//! Durable HTTP sessions on PostgreSQL (S7 C6/B-03): http_sessions table.
//! Implements the kelson-session `SessionStore` port so browser sessions
//! survive restarts and scale across instances.

use kelson_foundation::{AppError, PrincipalId, TenantId};
use kelson_session::{Session, SessionId, SessionPolicy, SessionStore};
use sqlx::postgres::PgPool;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::bridge::block_on;

pub struct PgSessionStore {
    pool: PgPool,
}

impl PgSessionStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}


fn pg(e: impl std::fmt::Display) -> AppError {
    AppError::persistence(format!("http_sessions: {e}"))
}

fn row_to_session(row: sqlx::postgres::PgRow) -> Result<Session, AppError> {
    use sqlx::Row;
    let raw_id: String = row.try_get("id").map_err(pg)?;
    let id = SessionId::parse(&raw_id)?;
    let principal: Uuid = row.try_get("principal_id").map_err(pg)?;
    let tenant: Option<Uuid> = row.try_get("tenant_id").map_err(pg)?;
    let data: serde_json::Value = row.try_get("data").map_err(pg)?;
    Ok(Session {
        id,
        principal: PrincipalId::from_uuid(principal),
        tenant: tenant.map(TenantId::from_uuid),
        data: serde_json::from_value(data).map_err(pg)?,
        created_at: row.try_get("created_at").map_err(pg)?,
        last_seen: row.try_get("last_seen").map_err(pg)?,
    })
}

impl SessionStore for PgSessionStore {
    fn load(&self, id: &SessionId) -> Result<Option<Session>, AppError> {
        let id = id.as_str().to_string();
        let pool = self.pool.clone();
        block_on(async move {
            let row: Option<sqlx::postgres::PgRow> =
                sqlx::query("SELECT * FROM http_sessions WHERE id = $1")
                    .bind(&id)
                    .fetch_optional(&pool)
                    .await
                    .map_err(pg)?;
            row.map(row_to_session).transpose()
        })
    }

    fn save(&self, session: &Session) -> Result<(), AppError> {
        let data = serde_json::to_value(&session.data).map_err(pg)?;
        let session = session.clone();
        let pool = self.pool.clone();
        block_on(async move {
            sqlx::query(
                "INSERT INTO http_sessions (id, principal_id, tenant_id, data, created_at, last_seen)
                 VALUES ($1, $2, $3, $4, $5, $6)
                 ON CONFLICT (id) DO UPDATE
                 SET data = EXCLUDED.data, last_seen = EXCLUDED.last_seen",
            )
            .bind(session.id.as_str())
            .bind(*session.principal.as_uuid())
            .bind(session.tenant.map(|t| *t.as_uuid()))
            .bind(data)
            .bind(session.created_at)
            .bind(session.last_seen)
            .execute(&pool)
            .await
            .map_err(pg)?;
            Ok(())
        })
    }

    fn delete(&self, id: &SessionId) -> Result<(), AppError> {
        let id = id.as_str().to_string();
        let pool = self.pool.clone();
        block_on(async move {
            sqlx::query("DELETE FROM http_sessions WHERE id = $1")
                .bind(&id)
                .execute(&pool)
                .await
                .map_err(pg)?;
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
            clauses.push(format!("last_seen < ${}", binds.len()));
        }
        if policy.absolute_secs > 0 {
            binds.push(now - time::Duration::seconds(policy.absolute_secs as i64));
            clauses.push(format!("created_at < ${}", binds.len()));
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
            let done = q.execute(&pool).await.map_err(pg)?;
            Ok(done.rows_affected() as usize)
        })
    }
}
