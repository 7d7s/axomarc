// P7: SQLite-backed binding store — persists chat_id→user_id and
// binding codes across restarts.

use async_trait::async_trait;
use tracing::debug;

use crate::commands::{BindingCodeStore, BindingStore, ChatopsError};
use crate::types::BindingCode;

/// SQLite-backed binding store. Uses sqlx for async queries.
pub struct SqliteBindingStore {
    pool: sqlx::SqlitePool,
}

impl SqliteBindingStore {
    pub fn new(pool: sqlx::SqlitePool) -> Self {
        Self { pool }
    }

    /// Run the migration to create the binding tables.
    pub async fn migrate(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS chatops_binding (
                chat_id     INTEGER PRIMARY KEY,
                user_id     TEXT NOT NULL,
                created_at  INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS chatops_binding_code (
                code        TEXT PRIMARY KEY,
                user_id     TEXT NOT NULL,
                created_at  INTEGER NOT NULL
            );
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

#[async_trait]
impl BindingStore for SqliteBindingStore {
    async fn get_user_id(&self, chat_id: i64) -> Result<Option<String>, ChatopsError> {
        let row: Option<(String,)> = sqlx::query_as(
            "SELECT user_id FROM chatops_binding WHERE chat_id = ?",
        )
        .bind(chat_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ChatopsError::Storage(e.to_string()))?;
        Ok(row.map(|r| r.0))
    }

    async fn bind(&self, chat_id: i64, user_id: &str) -> Result<(), ChatopsError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        sqlx::query(
            "INSERT OR REPLACE INTO chatops_binding (chat_id, user_id, created_at) VALUES (?, ?, ?)",
        )
        .bind(chat_id)
        .bind(user_id)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| ChatopsError::Storage(e.to_string()))?;
        debug!(chat_id, user_id, "binding created");
        Ok(())
    }

    async fn unbind(&self, chat_id: i64) -> Result<(), ChatopsError> {
        sqlx::query("DELETE FROM chatops_binding WHERE chat_id = ?")
            .bind(chat_id)
            .execute(&self.pool)
            .await
            .map_err(|e| ChatopsError::Storage(e.to_string()))?;
        debug!(chat_id, "binding removed");
        Ok(())
    }

    async fn list_bindings(&self) -> Result<Vec<(i64, String)>, ChatopsError> {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT chat_id, user_id FROM chatops_binding ORDER BY created_at",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| ChatopsError::Storage(e.to_string()))?;
        Ok(rows)
    }

    async fn unbind_by_user(&self, user_id: &str) -> Result<u64, ChatopsError> {
        let result = sqlx::query("DELETE FROM chatops_binding WHERE user_id = ?")
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(|e| ChatopsError::Storage(e.to_string()))?;
        let count = result.rows_affected();
        debug!(user_id, count, "bindings revoked for user");
        Ok(count)
    }
}

#[async_trait]
impl BindingCodeStore for SqliteBindingStore {
    async fn create_code(&self, user_id: &str) -> Result<String, ChatopsError> {
        let code = BindingCode::new(user_id.to_string());
        let code_str = code.code.clone();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        sqlx::query(
            "INSERT OR REPLACE INTO chatops_binding_code (code, user_id, created_at) VALUES (?, ?, ?)",
        )
        .bind(&code_str)
        .bind(user_id)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| ChatopsError::Storage(e.to_string()))?;
        debug!(user_id, code = %code_str, "binding code created");
        Ok(code_str)
    }

    async fn consume_code(&self, code: &str) -> Result<Option<String>, ChatopsError> {
        let row: Option<(String, i64)> = sqlx::query_as(
            "SELECT user_id, created_at FROM chatops_binding_code WHERE code = ?",
        )
        .bind(code)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| ChatopsError::Storage(e.to_string()))?;

        match row {
            Some((user_id, created_at)) => {
                // Check TTL (5 minutes)
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                if now - created_at > 300 {
                    // Expired — delete and return None
                    let _ = sqlx::query("DELETE FROM chatops_binding_code WHERE code = ?")
                        .bind(code)
                        .execute(&self.pool)
                        .await;
                    return Ok(None);
                }
                // Valid — consume (delete) and return user_id
                let _ = sqlx::query("DELETE FROM chatops_binding_code WHERE code = ?")
                    .bind(code)
                    .execute(&self.pool)
                    .await;
                debug!(user_id = %user_id, "binding code consumed");
                Ok(Some(user_id))
            }
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> sqlx::SqlitePool {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = format!("chatops-binding-test-{n}");
        let url = format!("file:{name}?mode=memory&cache=shared");
        let pool = sqlx::SqlitePool::connect(&url)
            .await
            .unwrap();
        let store = SqliteBindingStore::new(pool.clone());
        store.migrate().await.unwrap();
        pool
    }

    #[tokio::test]
    async fn sqlite_binding_roundtrip() {
        let pool = test_pool().await;
        let store = SqliteBindingStore::new(pool);
        assert_eq!(store.get_user_id(123).await.unwrap(), None);
        store.bind(123, "alice").await.unwrap();
        assert_eq!(store.get_user_id(123).await.unwrap(), Some("alice".to_string()));
        store.unbind(123).await.unwrap();
        assert_eq!(store.get_user_id(123).await.unwrap(), None);
    }

    #[tokio::test]
    async fn sqlite_binding_code_roundtrip() {
        let pool = test_pool().await;
        let store = SqliteBindingStore::new(pool);
        let code = store.create_code("bob").await.unwrap();
        assert_eq!(code.len(), 6);
        let user = store.consume_code(&code).await.unwrap();
        assert_eq!(user, Some("bob".to_string()));
        // Code consumed, second use returns None
        let user2 = store.consume_code(&code).await.unwrap();
        assert_eq!(user2, None);
    }

    #[tokio::test]
    async fn sqlite_binding_overwrites_existing() {
        let pool = test_pool().await;
        let store = SqliteBindingStore::new(pool);
        store.bind(100, "alice").await.unwrap();
        store.bind(100, "bob").await.unwrap();
        assert_eq!(store.get_user_id(100).await.unwrap(), Some("bob".to_string()));
    }

    #[tokio::test]
    async fn sqlite_list_bindings() {
        let pool = test_pool().await;
        let store = SqliteBindingStore::new(pool);
        store.bind(10, "alice").await.unwrap();
        store.bind(20, "bob").await.unwrap();
        let mut bindings = store.list_bindings().await.unwrap();
        bindings.sort_by_key(|b| b.0);
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0], (10, "alice".to_string()));
        assert_eq!(bindings[1], (20, "bob".to_string()));
    }

    #[tokio::test]
    async fn sqlite_unbind_by_user() {
        let pool = test_pool().await;
        let store = SqliteBindingStore::new(pool);
        store.bind(10, "alice").await.unwrap();
        store.bind(20, "alice").await.unwrap();
        store.bind(30, "bob").await.unwrap();
        let revoked = store.unbind_by_user("alice").await.unwrap();
        assert_eq!(revoked, 2);
        assert_eq!(store.get_user_id(10).await.unwrap(), None);
        assert_eq!(store.get_user_id(20).await.unwrap(), None);
        assert_eq!(store.get_user_id(30).await.unwrap(), Some("bob".to_string()));
    }
}
