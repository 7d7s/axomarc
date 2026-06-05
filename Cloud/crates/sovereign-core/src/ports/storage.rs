// `StoragePort` — the only port the use cases know about. Per
// `docs/architecture.md` §1.3, the trait is **one** big trait (it
// may grow, but it stays in one file). All I/O — including the audit
// append — flows through this trait. Concrete adapters (SQLite V0,
// rqlite V2) implement it.

use async_trait::async_trait;

use crate::domain::{
    App, AppId, AppUpdate, AuditEvent, AuditQuery, Backup, BackupId, BackupStatus, Deployment,
    DeploymentEvent, DeploymentId, Domain, DomainId, NewApp, NewBackup, NewDeployment, NewDomain,
    NewSecret, NewServer, NewUser, Secret, SecretId, SecretStatus, Server, ServerId, ServerStatus,
    Timestamp, User, UserId, UserRole,
};
use crate::error::AppError;

/// The single state-mutation port. Every use case that needs to read
/// or write state takes a `&dyn StoragePort`. The composition root
/// in `main.rs` is the only place that names a concrete implementation.
///
/// All methods are `async` and return `Result<T, AppError>`. Optimistic
/// concurrency is the caller's responsibility: every mutating method
/// that takes a `version` will return `AppError::Conflict` on mismatch.
#[async_trait]
pub trait StoragePort: Send + Sync {
    // --- meta ----------------------------------------------------------------

    /// Returns the storage backend's `PRAGMA user_version` (i.e. the
    /// current migration level). `None` for backends that don't track
    /// it.
    async fn schema_version(&self) -> Result<Option<i64>, AppError>;

    /// Returns the names of the core tables that exist in the database.
    /// Used by `sovereign doctor` to confirm the 7 tables are present.
    async fn core_tables(&self) -> Result<Vec<String>, AppError>;

    // --- app -----------------------------------------------------------------

    async fn get_app(&self, id: AppId) -> Result<Option<App>, AppError>;

    /// Fetch an app by its (globally unique, human-readable) name.
    async fn get_app_by_name(&self, name: &str) -> Result<Option<App>, AppError>;

    /// List apps, optionally filtered by owner (substring match against
    /// `app.owner` to support the `"team:<name>"` case).
    async fn list_apps(&self, owner: Option<&str>) -> Result<Vec<App>, AppError>;

    async fn create_app(&self, new: NewApp, actor: &str) -> Result<App, AppError>;

    /// Optimistic-concurrency update. The row is updated only if
    /// `expected_version` matches the row's current `version`. On
    /// mismatch: `AppError::Conflict("app", current_version)`.
    async fn update_app(
        &self,
        id: AppId,
        update: AppUpdate,
        expected_version: i64,
        actor: &str,
    ) -> Result<App, AppError>;

    /// Soft-delete: sets `status = Archived` and bumps `version`. The
    /// row is **never** physically removed (audit trail integrity).
    async fn archive_app(
        &self,
        id: AppId,
        expected_version: i64,
        actor: &str,
    ) -> Result<App, AppError>;

    // --- deployment ----------------------------------------------------------

    /// Create a new deployment in `Pending` state. The actor and the
    /// initial audit row are recorded atomically.
    async fn begin_deployment(
        &self,
        new: NewDeployment,
        actor: &str,
    ) -> Result<Deployment, AppError>;

    /// Apply a state-machine transition. The transition is rejected
    /// with `AppError::InvalidTransition` if the documented state
    /// machine forbids it. The mutation and the audit append happen
    /// in the same transaction.
    async fn transition_deployment(
        &self,
        id: DeploymentId,
        event: DeploymentEvent,
        actor: &str,
    ) -> Result<Deployment, AppError>;

    async fn get_deployment(&self, id: DeploymentId) -> Result<Option<Deployment>, AppError>;

    /// Most recent first, capped at `limit`.
    async fn list_deployments(&self, app: AppId, limit: u32) -> Result<Vec<Deployment>, AppError>;

    /// The most recent `Healthy` deployment for the app — i.e. what's
    /// currently serving. `None` if the app has never had a healthy
    /// deploy. F5's `get_current_deployment`.
    async fn get_current_deployment(&self, app: AppId) -> Result<Option<Deployment>, AppError>;

    /// `Healthy` deployments that started strictly before `before`,
    /// newest first, capped at `limit`. F5 rollback uses this to find
    /// the "previous version" to roll back to.
    async fn list_healthy_deployments_before(
        &self,
        app: AppId,
        before: Timestamp,
        limit: u32,
    ) -> Result<Vec<Deployment>, AppError>;

    /// Set the `target_deployment_id` on an existing deployment. F5
    /// rollback uses this to record "this rollback replaced
    /// deployment X". Bumps the version. The audit append is in the
    /// same transaction.
    async fn set_rollback_target(
        &self,
        id: DeploymentId,
        target: DeploymentId,
        actor: &str,
    ) -> Result<Deployment, AppError>;

    // --- domain --------------------------------------------------------------

    async fn add_domain(&self, new: NewDomain, actor: &str) -> Result<Domain, AppError>;
    async fn remove_domain(&self, id: DomainId, actor: &str) -> Result<(), AppError>;
    async fn list_domains(&self, app: AppId) -> Result<Vec<Domain>, AppError>;

    // --- secrets -------------------------------------------------------------

    async fn put_secret(&self, new: NewSecret, actor: &str) -> Result<Secret, AppError>;
    async fn get_secret(&self, id: SecretId) -> Result<Option<Secret>, AppError>;
    async fn list_secrets(&self, app: AppId) -> Result<Vec<Secret>, AppError>;
    async fn delete_secret(&self, id: SecretId, actor: &str) -> Result<(), AppError>;
    async fn rotate_secret(
        &self,
        id: SecretId,
        new_ciphertext: Vec<u8>,
        actor: &str,
    ) -> Result<Secret, AppError>;
    /// Bulk status update for rotation windows (the V1.5 secrets port
    /// drives this).
    async fn set_secret_status(
        &self,
        id: SecretId,
        status: SecretStatus,
        actor: &str,
    ) -> Result<Secret, AppError>;

    // --- servers -------------------------------------------------------------

    async fn add_server(&self, new: NewServer) -> Result<Server, AppError>;
    async fn get_server(&self, id: ServerId) -> Result<Option<Server>, AppError>;
    async fn list_servers(&self) -> Result<Vec<Server>, AppError>;
    async fn set_server_status(
        &self,
        id: ServerId,
        status: ServerStatus,
        actor: &str,
    ) -> Result<Server, AppError>;
    async fn touch_server(&self, id: ServerId, at: Timestamp) -> Result<(), AppError>;
    async fn remove_server(&self, id: ServerId, actor: &str) -> Result<(), AppError>;

    // --- backups -------------------------------------------------------------

    async fn begin_backup(&self, new: NewBackup, actor: &str) -> Result<Backup, AppError>;
    async fn complete_backup(
        &self,
        id: BackupId,
        status: BackupStatus,
        size_bytes: Option<i64>,
        location: Option<String>,
        error: Option<String>,
        actor: &str,
    ) -> Result<Backup, AppError>;
    async fn verify_backup(
        &self,
        id: BackupId,
        verify_result: serde_json::Value,
        actor: &str,
    ) -> Result<Backup, AppError>;
    async fn get_backup(&self, id: BackupId) -> Result<Option<Backup>, AppError>;
    async fn list_backups(&self, limit: u32) -> Result<Vec<Backup>, AppError>;

    // --- users ---------------------------------------------------------------

    async fn create_user(&self, new: NewUser) -> Result<User, AppError>;
    async fn get_user(&self, id: UserId) -> Result<Option<User>, AppError>;
    async fn get_user_by_email(&self, email: &str) -> Result<Option<User>, AppError>;
    async fn list_users(&self) -> Result<Vec<User>, AppError>;
    async fn set_user_role(
        &self,
        id: UserId,
        role: UserRole,
        actor: &str,
    ) -> Result<User, AppError>;
    async fn touch_user(&self, id: UserId, at: Timestamp) -> Result<(), AppError>;

    // --- audit ---------------------------------------------------------------

    /// Append an event to the `audit_event` table. The underlying
    /// triggers will reject any subsequent `UPDATE` or `DELETE`; this
    /// method itself only ever performs an `INSERT`.
    async fn append_audit(&self, event: AuditEvent) -> Result<(), AppError>;

    /// Query the audit log. The returned rows are ordered most-recent
    /// first; `q.effective_limit()` is the cap.
    async fn query_audit(&self, q: AuditQuery) -> Result<Vec<AuditEvent>, AppError>;

    /// The number of audit events currently stored. Used by `sovereign
    /// doctor` and the dashboard.
    async fn count_audit(&self) -> Result<i64, AppError>;
}
