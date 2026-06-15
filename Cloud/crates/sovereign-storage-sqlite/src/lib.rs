// # sovereign-storage-sqlite
//
// **SQLite storage adapter for Sovereign.**
//
// Implements `StoragePort` against a single SQLite file. This is the
// default (and only V0) storage backend.
//
// ## Features
//
// - **7 core tables** — app, deployment, audit_event, secret, backup, domain, server
// - **Append-only audit** — UPDATE/DELETE rejected by SQLite triggers
// - **WAL mode** — concurrent reads during writes
// - **Forward-only migrations** — `sqlx::migrate!()` at startup
// - **Optimistic concurrency** — version column on mutable rows
//
// ## Quick Reference
//
// ```rust,no_run
// use sovereign_storage_sqlite::SqliteState;
//
// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
// let state = SqliteState::open(std::path::Path::new("/var/lib/sovereign/sovereign.db")).await?;
// # Ok(())
// # }
// ```
//
// ## Migrations
//
// | Migration | Description |
// |-----------|-------------|
// | 0001 | Core tables (app, deployment, audit_event, secret, backup, domain, server) |
// | 0002 | Audit triggers (reject_audit_mutation) |
// | 0003 | Service table |
// | 0004 | Auto-deploy fields |
// | 0005 | Partial unique index |
// | 0006 | Deploy mode |
// | 0007 | Source config |
// | 0008 | Native deploy mode |
// | 0009 | Auth credentials (password_hash, api_token, bootstrap_state) |

#![deny(unsafe_code)]
#![allow(missing_docs)]

use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::{Pool, Row, Sqlite};
use tracing::{info, instrument};

use sovereign_core::domain::{
    ApiToken, App, AppId, AppUpdate, AuditEvent, AuditQuery, Backup, BackupId, BackupStatus,
    Deployment, DeploymentEvent, DeploymentId, Domain, DomainId, NewApp, NewBackup, NewDeployment,
    NewDomain, NewSecret, NewServer, NewUser, Secret, SecretId, SecretStatus, Server, ServerId,
    ServerStatus, Timestamp, User, UserId, UserRole,
};
use sovereign_core::error::AppError;
use sovereign_core::ports::StoragePort;

mod app;
mod audit;
mod backup;
mod deployment;
mod domain;
mod row;
mod secret;
mod server;
mod user;

/// Re-exports of the migration SQL for use in tests and the doctor check.
pub mod migrations {
    /// The 7-table initial schema (`migrations/0001_init.sql`).
    pub const INIT_SQL: &str = include_str!("../migrations/0001_init.sql");
    /// The append-only audit schema (`migrations/0002_audit.sql`).
    pub const AUDIT_SQL: &str = include_str!("../migrations/0002_audit.sql");
}

/// The single state-mutation adapter. Owns the `sqlx::Pool<Sqlite>`
/// and runs migrations on first open.
#[derive(Debug, Clone)]
pub struct SqliteState {
    pool: Pool<Sqlite>,
}

impl SqliteState {
    /// Open (and migrate) a SQLite file at `path`. Creates the parent
    /// directory if it does not exist. Returns `AppError::Storage` on
    /// any failure.
    #[instrument(skip_all, fields(path = %path.as_ref().display()))]
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, AppError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    AppError::Storage(format!("cannot create data dir {}: {e}", parent.display()))
                })?;
            }
        }

        let connect = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .busy_timeout(Duration::from_secs(5))
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(1) // SQLite single-writer
            .min_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(connect)
            .await
            .map_err(|e| AppError::Storage(format!("connect: {e}")))?;

        Self::from_pool(pool).await
    }

    /// Open an in-memory database (`:memory:` with a shared cache so
    /// the pool can open multiple connections to the same DB). Used by
    /// tests and the `sovereign doctor` Level 1 sanity check.
    #[instrument(skip_all)]
    pub async fn open_in_memory() -> Result<Self, AppError> {
        // `file::memory:?cache=shared` is the URI form required for an
        // in-memory DB to be visible to multiple connections. Plain
        // `:memory:` is private to the first connection that opens it.
        let connect = SqliteConnectOptions::new()
            .filename("file::memory:?cache=shared")
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .min_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(connect)
            .await
            .map_err(|e| AppError::Storage(format!("connect (memory): {e}")))?;

        Self::from_pool(pool).await
    }

    /// Open a uniquely-named in-memory database. The `name` is included
    /// in the SQLite shared-cache key, so two calls with different
    /// names get completely independent databases. Used by tests that
    /// need full isolation.
    #[instrument(skip_all, fields(name = %name))]
    pub async fn open_in_memory_named(name: &str) -> Result<Self, AppError> {
        let filename = format!("file:{name}:?mode=memory&cache=shared");
        let connect = SqliteConnectOptions::new()
            .filename(&filename)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true);

        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .min_connections(1)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(connect)
            .await
            .map_err(|e| AppError::Storage(format!("connect (memory:{name}): {e}")))?;

        Self::from_pool(pool).await
    }

    /// Adopt an already-constructed pool and run migrations. Useful for
    /// tests that want to seed data first.
    pub async fn from_pool(pool: Pool<Sqlite>) -> Result<Self, AppError> {
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|e| AppError::Storage(format!("migrate: {e}")))?;
        info!("sqlite state opened and migrated");
        Ok(Self { pool })
    }

    /// Borrow the underlying pool. For tests and the doctor check only.
    pub fn pool(&self) -> &Pool<Sqlite> {
        &self.pool
    }

    /// Returns `true` iff WAL mode is currently active on the connection.
    pub async fn is_wal(&self) -> Result<bool, AppError> {
        let row: (String,) = sqlx::query_as("PRAGMA journal_mode")
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::from)?;
        Ok(row.0.eq_ignore_ascii_case("wal"))
    }
}

#[async_trait]
impl StoragePort for SqliteState {
    #[instrument(skip(self))]
    async fn schema_version(&self) -> Result<Option<i64>, AppError> {
        // `sqlx::migrate!` tracks applied migrations in its own
        // `_sqlx_migrations` table — it does not bump `PRAGMA
        // user_version`. Read the max version from the bookkeeping
        // table; return `None` if migrations have not run yet (the
        // table does not exist before the first migrate).
        let exists: (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type = 'table' AND name = '_sqlx_migrations'",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(AppError::from)?;
        if exists.0 == 0 {
            return Ok(None);
        }
        let row: (i64,) = sqlx::query_as("SELECT COALESCE(MAX(version), 0) FROM _sqlx_migrations")
            .fetch_one(&self.pool)
            .await
            .map_err(AppError::from)?;
        Ok(Some(row.0))
    }

    #[instrument(skip(self))]
    async fn core_tables(&self) -> Result<Vec<String>, AppError> {
        // Exclude the `_sqlx_migrations` bookkeeping table; that one
        // belongs to sqlx, not to our domain schema.
        let rows = sqlx::query(
            "SELECT name FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%' \
               AND name != '_sqlx_migrations' \
             ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(AppError::from)?;
        Ok(rows.into_iter().map(|r| r.get::<String, _>(0)).collect())
    }

    async fn get_app(&self, id: AppId) -> Result<Option<App>, AppError> {
        row::select_app_by_id(&self.pool, id).await
    }

    async fn get_app_by_name(&self, name: &str) -> Result<Option<App>, AppError> {
        row::select_app_by_name(&self.pool, name).await
    }

    async fn list_apps(&self, owner: Option<&str>) -> Result<Vec<App>, AppError> {
        match owner {
            Some(o) => row::select_apps_by_owner(&self.pool, o).await,
            None => row::select_all_apps(&self.pool).await,
        }
    }

    async fn create_app(&self, new: NewApp, actor: &str) -> Result<App, AppError> {
        app::create(&self.pool, new, actor).await
    }

    async fn update_app(
        &self,
        id: AppId,
        update: AppUpdate,
        expected_version: i64,
        actor: &str,
    ) -> Result<App, AppError> {
        app::update(&self.pool, id, update, expected_version, actor).await
    }

    async fn archive_app(
        &self,
        id: AppId,
        expected_version: i64,
        actor: &str,
    ) -> Result<App, AppError> {
        app::archive(&self.pool, id, expected_version, actor).await
    }

    async fn begin_deployment(
        &self,
        new: NewDeployment,
        actor: &str,
    ) -> Result<Deployment, AppError> {
        deployment::begin(&self.pool, new, actor).await
    }

    async fn transition_deployment(
        &self,
        id: DeploymentId,
        event: DeploymentEvent,
        actor: &str,
    ) -> Result<Deployment, AppError> {
        deployment::transition(&self.pool, id, event, actor).await
    }

    async fn get_deployment(&self, id: DeploymentId) -> Result<Option<Deployment>, AppError> {
        row::select_deployment_by_id(&self.pool, id).await
    }

    async fn list_deployments(&self, app: AppId, limit: u32) -> Result<Vec<Deployment>, AppError> {
        row::select_deployments_by_app(&self.pool, app, limit.max(1) as i64).await
    }

    async fn get_current_deployment(&self, app: AppId) -> Result<Option<Deployment>, AppError> {
        row::select_current_deployment(&self.pool, app).await
    }

    async fn list_healthy_deployments_before(
        &self,
        app: AppId,
        before: Timestamp,
        limit: u32,
    ) -> Result<Vec<Deployment>, AppError> {
        row::select_healthy_before(&self.pool, app, before.as_secs(), limit.max(1) as i64).await
    }

    async fn set_rollback_target(
        &self,
        id: DeploymentId,
        target: DeploymentId,
        actor: &str,
    ) -> Result<Deployment, AppError> {
        deployment::set_rollback_target(&self.pool, id, target, actor).await
    }

    async fn add_domain(&self, new: NewDomain, actor: &str) -> Result<Domain, AppError> {
        domain::add(&self.pool, new, actor).await
    }

    async fn remove_domain(&self, id: DomainId, actor: &str) -> Result<(), AppError> {
        domain::remove(&self.pool, id, actor).await
    }

    async fn list_domains(&self, app: AppId) -> Result<Vec<Domain>, AppError> {
        row::select_domains_by_app(&self.pool, app).await
    }

    async fn put_secret(&self, new: NewSecret, actor: &str) -> Result<Secret, AppError> {
        secret::put(&self.pool, new, actor).await
    }

    async fn get_secret(&self, id: SecretId) -> Result<Option<Secret>, AppError> {
        row::select_secret_by_id(&self.pool, id).await
    }

    async fn list_secrets(&self, app: AppId) -> Result<Vec<Secret>, AppError> {
        row::select_secrets_by_app(&self.pool, app).await
    }

    async fn delete_secret(&self, id: SecretId, actor: &str) -> Result<(), AppError> {
        secret::delete(&self.pool, id, actor).await
    }

    async fn rotate_secret(
        &self,
        id: SecretId,
        new_ciphertext: Vec<u8>,
        actor: &str,
    ) -> Result<Secret, AppError> {
        secret::rotate(&self.pool, id, new_ciphertext, actor).await
    }

    async fn set_secret_status(
        &self,
        id: SecretId,
        status: SecretStatus,
        actor: &str,
    ) -> Result<Secret, AppError> {
        secret::set_status(&self.pool, id, status, actor).await
    }

    async fn add_server(&self, new: NewServer) -> Result<Server, AppError> {
        server::add(&self.pool, new).await
    }

    async fn get_server(&self, id: ServerId) -> Result<Option<Server>, AppError> {
        row::select_server_by_id(&self.pool, id).await
    }

    async fn list_servers(&self) -> Result<Vec<Server>, AppError> {
        row::select_all_servers(&self.pool).await
    }

    async fn set_server_status(
        &self,
        id: ServerId,
        status: ServerStatus,
        actor: &str,
    ) -> Result<Server, AppError> {
        server::set_status(&self.pool, id, status, actor).await
    }

    async fn touch_server(&self, id: ServerId, at: Timestamp) -> Result<(), AppError> {
        server::touch(&self.pool, id, at).await
    }

    async fn remove_server(&self, id: ServerId, actor: &str) -> Result<(), AppError> {
        server::remove(&self.pool, id, actor).await
    }

    async fn begin_backup(&self, new: NewBackup, actor: &str) -> Result<Backup, AppError> {
        backup::begin(&self.pool, new, actor).await
    }

    async fn complete_backup(
        &self,
        id: BackupId,
        status: BackupStatus,
        size_bytes: Option<i64>,
        location: Option<String>,
        error: Option<String>,
        actor: &str,
    ) -> Result<Backup, AppError> {
        backup::complete(&self.pool, id, status, size_bytes, location, error, actor).await
    }

    async fn verify_backup(
        &self,
        id: BackupId,
        verify_result: serde_json::Value,
        actor: &str,
    ) -> Result<Backup, AppError> {
        backup::verify(&self.pool, id, verify_result, actor).await
    }

    async fn get_backup(&self, id: BackupId) -> Result<Option<Backup>, AppError> {
        row::select_backup_by_id(&self.pool, id).await
    }

    async fn list_backups(&self, limit: u32) -> Result<Vec<Backup>, AppError> {
        row::select_recent_backups(&self.pool, limit.max(1) as i64).await
    }

    async fn create_user(&self, new: NewUser) -> Result<User, AppError> {
        user::create(&self.pool, new).await
    }

    async fn get_user(&self, id: UserId) -> Result<Option<User>, AppError> {
        row::select_user_by_id(&self.pool, id).await
    }

    async fn get_user_by_email(&self, email: &str) -> Result<Option<User>, AppError> {
        row::select_user_by_email(&self.pool, email).await
    }

    async fn list_users(&self) -> Result<Vec<User>, AppError> {
        row::select_all_users(&self.pool).await
    }

    async fn set_user_role(
        &self,
        id: UserId,
        role: UserRole,
        actor: &str,
    ) -> Result<User, AppError> {
        user::set_role(&self.pool, id, role, actor).await
    }

    async fn touch_user(&self, id: UserId, at: Timestamp) -> Result<(), AppError> {
        user::touch(&self.pool, id, at).await
    }

    async fn set_user_password_hash(
        &self,
        id: UserId,
        hash: Option<&str>,
    ) -> Result<(), AppError> {
        user::set_password_hash(&self.pool, id, hash).await
    }

    async fn get_user_password_hash(&self, id: UserId) -> Result<Option<String>, AppError> {
        user::get_password_hash(&self.pool, id).await
    }

    async fn set_user_display_name(
        &self,
        id: UserId,
        display_name: &str,
    ) -> Result<(), AppError> {
        user::set_display_name(&self.pool, id, display_name).await
    }

    async fn disable_user(&self, id: UserId, at: Timestamp) -> Result<(), AppError> {
        user::disable(&self.pool, id, at).await
    }

    async fn enable_user(&self, id: UserId) -> Result<(), AppError> {
        user::enable(&self.pool, id).await
    }

    async fn create_api_token(
        &self,
        user_id: UserId,
        name: &str,
        hash: &str,
        scopes: &str,
        created_at: Timestamp,
        expires_at: Option<Timestamp>,
    ) -> Result<ApiToken, AppError> {
        user::create_api_token(&self.pool, user_id, name, hash, scopes, created_at, expires_at).await
    }

    async fn list_api_tokens(&self, user_id: UserId) -> Result<Vec<ApiToken>, AppError> {
        user::list_api_tokens(&self.pool, user_id).await
    }

    async fn get_api_token(
        &self,
        user_id: UserId,
        name: &str,
    ) -> Result<Option<ApiToken>, AppError> {
        user::get_api_token(&self.pool, user_id, name).await
    }

    async fn delete_api_token(&self, user_id: UserId, name: &str) -> Result<(), AppError> {
        user::delete_api_token(&self.pool, user_id, name).await
    }

    async fn get_bootstrap_admin(&self) -> Result<Option<UserId>, AppError> {
        user::get_bootstrap_admin(&self.pool).await
    }

    async fn set_bootstrap_admin(&self, user_id: UserId) -> Result<(), AppError> {
        user::set_bootstrap_admin(&self.pool, user_id).await
    }

    async fn append_audit(&self, event: AuditEvent) -> Result<(), AppError> {
        audit::append(&self.pool, event).await
    }

    async fn query_audit(&self, q: AuditQuery) -> Result<Vec<AuditEvent>, AppError> {
        audit::query(&self.pool, &q).await
    }

    async fn count_audit(&self) -> Result<i64, AppError> {
        audit::count(&self.pool).await
    }

    #[instrument(skip(self))]
    async fn vacuum_into(&self, target_path: &Path) -> Result<u64, AppError> {
        // SQLite's `VACUUM INTO` (3.27+) writes a clean, defragmented
        // copy of the database to the given path. The argument is a
        // string literal expression, so bind it as TEXT. We then
        // stat() the file to learn its size.
        let path_str = target_path.to_string_lossy().into_owned();
        sqlx::query(&format!("VACUUM INTO '{}'", path_str.replace('\'', "''")))
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Storage(format!("VACUUM INTO: {e}")))?;
        let meta = std::fs::metadata(target_path).map_err(|e| {
            AppError::Storage(format!(
                "stat {} after VACUUM INTO: {e}",
                target_path.display()
            ))
        })?;
        Ok(meta.len())
    }

    async fn record_update(
        &self,
        record: &sovereign_core::ports::UpdateRecord,
    ) -> Result<(), AppError> {
        sqlx::query(
            "INSERT OR REPLACE INTO update_history \
             (sha256, from_version, to_version, channel, applied_at, backup_path, rolled_back_at) \
             VALUES (?, ?, ?, ?, ?, ?, NULL)",
        )
        .bind(&record.sha256)
        .bind(&record.from_version)
        .bind(&record.to_version)
        .bind(&record.channel)
        .bind(&record.applied_at)
        .bind(&record.backup_path)
        .execute(&self.pool)
        .await
        .map_err(|e| AppError::Storage(format!("record_update: {e}")))?;
        Ok(())
    }

    async fn last_update(&self) -> Result<Option<sovereign_core::ports::UpdateRecord>, AppError> {
        self.list_updates(1).await.map(|v| v.into_iter().next())
    }

    async fn list_updates(
        &self,
        limit: u32,
    ) -> Result<Vec<sovereign_core::ports::UpdateRecord>, AppError> {
        let rows = sqlx::query_as::<_, (String, String, String, String, String, String, Option<String>)>(
            "SELECT sha256, from_version, to_version, channel, applied_at, backup_path, rolled_back_at \
             FROM update_history \
             WHERE rolled_back_at IS NULL \
             ORDER BY applied_at DESC, sha256 DESC \
             LIMIT ?",
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| AppError::Storage(format!("list_updates: {e}")))?;
        let records = rows
            .into_iter()
            .map(|(sha, from, to, channel, applied_at, backup, _rolled)| {
                sovereign_core::ports::UpdateRecord {
                    from_version: from,
                    to_version: to,
                    channel,
                    sha256: sha,
                    applied_at,
                    backup_path: backup,
                }
            })
            .collect();
        Ok(records)
    }

    async fn mark_update_rolled_back(&self, sha256: &str) -> Result<(), AppError> {
        sqlx::query("UPDATE update_history SET rolled_back_at = ? WHERE sha256 = ?")
            .bind(chrono::Utc::now().to_rfc3339())
            .bind(sha256)
            .execute(&self.pool)
            .await
            .map_err(|e| AppError::Storage(format!("mark_update_rolled_back: {e}")))?;
        Ok(())
    }
}

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;
    use sovereign_core::domain::{NewUser, Timestamp, UserRole};
    use sovereign_core::ports::StoragePort;

    static TEST_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    async fn test_store() -> SqliteState {
        let n = TEST_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let pid = std::process::id();
        let name = format!("auth-test-{pid}-{n}");
        SqliteState::open_in_memory_named(&name).await.unwrap()
    }

    #[tokio::test]
    async fn bootstrap_first_user_becomes_owner() {
        let store = test_store().await;

        // No users yet.
        let users = store.list_users().await.unwrap();
        assert!(users.is_empty());

        // No bootstrap admin yet.
        assert!(store.get_bootstrap_admin().await.unwrap().is_none());

        // Create first user with Owner role (CLI promotes first user to Owner).
        let user = store
            .create_user(NewUser {
                email: "admin@example.com".into(),
                role: UserRole::Owner,
            })
            .await
            .unwrap();
        assert_eq!(user.role, UserRole::Owner);

        // Set bootstrap state.
        store.set_bootstrap_admin(user.id).await.unwrap();

        // Bootstrap admin is set.
        let bootstrap_id = store.get_bootstrap_admin().await.unwrap();
        assert!(bootstrap_id.is_some());
        assert_eq!(bootstrap_id.unwrap(), user.id);
    }

    #[tokio::test]
    async fn second_user_gets_specified_role() {
        let store = test_store().await;

        // First user = Owner (CLI promotes).
        let admin = store
            .create_user(NewUser {
                email: "admin@example.com".into(),
                role: UserRole::Owner,
            })
            .await
            .unwrap();
        assert_eq!(admin.role, UserRole::Owner);

        // Second user = Developer (as requested).
        let dev = store
            .create_user(NewUser {
                email: "dev@example.com".into(),
                role: UserRole::Developer,
            })
            .await
            .unwrap();
        assert_eq!(dev.role, UserRole::Developer);

        // Third user = Readonly.
        let viewer = store
            .create_user(NewUser {
                email: "viewer@example.com".into(),
                role: UserRole::Readonly,
            })
            .await
            .unwrap();
        assert_eq!(viewer.role, UserRole::Readonly);
    }

    #[tokio::test]
    async fn password_hash_roundtrip() {
        let store = test_store().await;

        let user = store
            .create_user(NewUser {
                email: "alice@example.com".into(),
                role: UserRole::Admin,
            })
            .await
            .unwrap();

        // No password yet.
        assert!(store.get_user_password_hash(user.id).await.unwrap().is_none());

        // Set password hash.
        let hash = sovereign_auth::password::hash_password("hunter2").unwrap();
        store.set_user_password_hash(user.id, Some(&hash)).await.unwrap();

        // Verify it's stored.
        let stored = store.get_user_password_hash(user.id).await.unwrap();
        assert!(stored.is_some());
        assert!(sovereign_auth::password::verify_password("hunter2", stored.as_ref().unwrap()).unwrap());
        assert!(!sovereign_auth::password::verify_password("wrong", stored.as_ref().unwrap()).unwrap());

        // Clear password.
        store.set_user_password_hash(user.id, None).await.unwrap();
        assert!(store.get_user_password_hash(user.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn user_disable_enable() {
        let store = test_store().await;

        let user = store
            .create_user(NewUser {
                email: "bob@example.com".into(),
                role: UserRole::Developer,
            })
            .await
            .unwrap();

        // Disable.
        store.disable_user(user.id, Timestamp::now()).await.unwrap();

        // Enable.
        store.enable_user(user.id).await.unwrap();

        // Disable again.
        store.disable_user(user.id, Timestamp::now()).await.unwrap();

        // Double-disable fails.
        let result = store.disable_user(user.id, Timestamp::now()).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn api_token_crud() {
        let store = test_store().await;

        let user = store
            .create_user(NewUser {
                email: "ci@example.com".into(),
                role: UserRole::Admin,
            })
            .await
            .unwrap();

        let now = Timestamp::now();

        // Create token.
        let tok = store
            .create_api_token(user.id, "deploy-key", "abc123hash", "app.deploy,secret.read", now, None)
            .await
            .unwrap();
        assert_eq!(tok.name, "deploy-key");
        assert_eq!(tok.scopes, "app.deploy,secret.read");

        // List tokens.
        let tokens = store.list_api_tokens(user.id).await.unwrap();
        assert_eq!(tokens.len(), 1);
        assert_eq!(tokens[0].name, "deploy-key");

        // Get by name.
        let found = store.get_api_token(user.id, "deploy-key").await.unwrap();
        assert!(found.is_some());

        // Duplicate name fails.
        let dup = store
            .create_api_token(user.id, "deploy-key", "otherhash", "app.deploy", now, None)
            .await;
        assert!(dup.is_err());

        // Delete.
        store.delete_api_token(user.id, "deploy-key").await.unwrap();
        let tokens = store.list_api_tokens(user.id).await.unwrap();
        assert!(tokens.is_empty());

        // Delete nonexistent fails.
        let result = store.delete_api_token(user.id, "nonexistent").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn duplicate_email_rejected() {
        let store = test_store().await;

        store
            .create_user(NewUser {
                email: "dup@example.com".into(),
                role: UserRole::Admin,
            })
            .await
            .unwrap();

        let result = store
            .create_user(NewUser {
                email: "dup@example.com".into(),
                role: UserRole::Readonly,
            })
            .await;
        assert!(result.is_err());
    }
}
