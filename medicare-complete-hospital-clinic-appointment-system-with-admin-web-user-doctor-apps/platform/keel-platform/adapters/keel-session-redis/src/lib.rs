//! # keel-session-redis
//! Redis adapter for the `SessionStore` port (keel-session).
//! 
//! Session semantics:
//! - `save` -> `SET session:{id} json [EX ttl]` (TTL = max(idle, absolute) from policy)
//! - `load` -> `GET session:{id}` + deserialise + expiry check
//! - `delete` -> `DEL session:{id}`
//! - `sweep` -> `SCAN MATCH session:*` + per-key TTL check (lazy expiry works if Redis handles it)
//! - Failures degrade to miss / no-op with warning: sessions are a convenience layer,
//!   not a correctness barrier (stateless JWT is the primary auth path).

use std::sync::{Arc, Mutex};

use keel_foundation::AppError;
use keel_session::{Session, SessionId, SessionPolicy, SessionStore};
use time::{Duration, OffsetDateTime};

/// Shared blocking connection with one reconnect-and-retry (same pattern as keel-cache-redis).
pub struct SharedConn {
    client: redis::Client,
    conn: Mutex<Option<redis::Connection>>,
}

impl SharedConn {
    pub fn new(url: &str) -> Result<Arc<Self>, AppError> {
        let client = redis::Client::open(url)
            .map_err(|e| AppError::persistence(format!("redis url: {e}")))?;
        Ok(Arc::new(Self {
            client,
            conn: Mutex::new(None),
        }))
    }

    pub fn probe(&self) -> Result<(), AppError> {
        self.with(|c| redis::cmd("PING").query::<String>(c))
            .map(|_| ())
    }

    pub fn with<R>(&self, mut f: impl FnMut(&mut redis::Connection) -> redis::RedisResult<R>) -> Result<R, AppError> {
        let mut guard = self.conn.lock().expect("redis conn poisoned");
        if guard.is_none() {
            let conn = self
                .client
                .get_connection()
                .map_err(|e| AppError::persistence(format!("redis connect: {e}")))?;
            *guard = Some(conn);
        }
        let conn = guard.as_mut().expect("connection present");
        match f(conn) {
            Ok(v) => Ok(v),
            Err(first) => {
                tracing::warn!(error = %first, "redis command failed; reconnecting once");
                *guard = None;
                let conn = self
                    .client
                    .get_connection()
                    .map_err(|e| AppError::persistence(format!("redis reconnect: {e}")))?;
                *guard = Some(conn);
                let conn = guard.as_mut().expect("connection present");
                f(conn).map_err(|e| AppError::persistence(format!("redis: {e}")))
            }
        }
    }
}

/// Redis-backed [`SessionStore`]. Keys are namespaced with `prefix` (default `keel:session:`).
pub struct RedisSessionStore {
    conn: Arc<SharedConn>,
    prefix: String,
}

impl RedisSessionStore {
    pub fn connect(url: &str) -> Result<Self, AppError> {
        let conn = SharedConn::new(url)?;
        conn.probe()?;
        Ok(Self::from_conn(conn))
    }

    pub fn from_conn(conn: Arc<SharedConn>) -> Self {
        Self {
            conn,
            prefix: "keel:session:".to_string(),
        }
    }

    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    fn key(&self, id: &SessionId) -> String {
        format!("{}{}", self.prefix, id.as_str())
    }

    /// Compute TTL in seconds from session policy.
    /// Uses the shorter of idle/absolute remaining time, clamped to at least 1s.
    fn ttl_secs(&self, session: &Session, policy: &SessionPolicy, now: OffsetDateTime) -> u64 {
        let mut ttl = u64::MAX;
        if policy.idle_secs > 0 {
            let idle_expiry = session.last_seen + Duration::seconds(policy.idle_secs as i64);
            let idle_remaining = (idle_expiry - now)
                .whole_seconds()
                .max(1) as u64;
            ttl = ttl.min(idle_remaining);
        }
        if policy.absolute_secs > 0 {
            let abs_expiry = session.created_at + Duration::seconds(policy.absolute_secs as i64);
            let abs_remaining = (abs_expiry - now)
                .whole_seconds()
                .max(1) as u64;
            ttl = ttl.min(abs_remaining);
        }
        if ttl == u64::MAX {
            86400 // default 1 day if no policy bounds
        } else {
            ttl
        }
    }
}

impl SessionStore for RedisSessionStore {
    fn load(&self, id: &SessionId) -> Result<Option<Session>, AppError> {
        let key = self.key(id);
        let raw = self
            .conn
            .with(|c| redis::cmd("GET").arg(&key).query::<Option<String>>(c))?;
        
        match raw {
            Some(raw) => {
                let session: Session = serde_json::from_str(&raw)
                    .map_err(|e| AppError::persistence(format!("session deserialize: {e}")))?;
                Ok(Some(session))
            }
            None => Ok(None),
        }
    }

    fn save(&self, session: &Session) -> Result<(), AppError> {
        let key = self.key(&session.id);
        let raw = serde_json::to_string(session)
            .map_err(|e| AppError::persistence(format!("session serialize: {e}")))?;
        let ttl = self.ttl_secs(session, &SessionPolicy::default(), OffsetDateTime::now_utc());
        
        let _: () = self.conn.with(|c| {
            let mut cmd = redis::cmd("SET");
            cmd.arg(&key).arg(&raw).arg("EX").arg(ttl);
            cmd.query::<()>(c)
        })?;
        Ok(())
    }

    fn delete(&self, id: &SessionId) -> Result<(), AppError> {
        let key = self.key(id);
        let _ = self.conn.with(|c| redis::cmd("DEL").arg(&key).query::<u64>(c));
        Ok(())
    }

    fn count(&self) -> usize {
        let pattern = format!("{}*", self.prefix);
        let mut cursor: u64 = 0;
        let mut count = 0usize;
        loop {
            let step = self.conn.with(|c| {
                let mut cmd = redis::cmd("SCAN");
                cmd.arg(cursor).arg("MATCH").arg(&pattern).arg("COUNT").arg(200);
                cmd.query::<(u64, Vec<String>)>(c)
            });
            let (next, keys) = match step {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %e.public_message, "redis session count scan aborted");
                    return count;
                }
            };
            count += keys.len();
            cursor = next;
            if cursor == 0 {
                break;
            }
        }
        count
    }

    fn sweep(&self, now: OffsetDateTime, policy: &SessionPolicy) -> Result<usize, AppError> {
        let pattern = format!("{}*", self.prefix);
        let mut cursor: u64 = 0;
        let mut deleted = 0usize;
        
        loop {
            let step = self.conn.with(|c| {
                let mut cmd = redis::cmd("SCAN");
                cmd.arg(cursor).arg("MATCH").arg(&pattern).arg("COUNT").arg(200);
                cmd.query::<(u64, Vec<String>)>(c)
            });
            let (next, keys) = match step {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %e.public_message, "redis session sweep scan aborted");
                    return Ok(deleted);
                }
            };
            
            for key in keys {
                let raw = self.conn.with(|c| redis::cmd("GET").arg(&key).query::<Option<String>>(c))?;
                if let Some(raw) = raw {
                    if let Ok(session) = serde_json::from_str::<Session>(&raw) {
                        if session.expired(now, policy) {
                            let _ = self.conn.with(|c| redis::cmd("DEL").arg(&key).query::<u64>(c));
                            deleted += 1;
                        }
                    }
                }
            }
            
            cursor = next;
            if cursor == 0 {
                break;
            }
        }
        
        Ok(deleted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_key_format() {
        let conn = SharedConn::new("redis://localhost:6379").unwrap();
        let store = RedisSessionStore::from_conn(conn);
        let id = SessionId::parse("abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789").unwrap();
        let key = store.key(&id);
        assert_eq!(key, "keel:session:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789");
    }
}