//! # kelson-postgres
//! The resource fabric for PostgreSQL (11-SPEC §5): pool + migrations + the
//! transaction engine + delta/audit/outbox writers + execution-level idempotency.
//!
//! Invariants implemented here:
//! - tenant anchor via `SET LOCAL kelson.tenant_id` + RLS (final containment barrier)
//! - single transaction: business state → DeltaStone → Audit → Outbox → COMMIT
//! - events dispatch only after commit (D-53)
//! - per-stream monotonic delta sequences
//! - runtime role never owns tables (see ../../../database/roles.sql)

pub mod batchledger;
pub mod bridge;
pub mod jobstore;
pub mod membershipstore;
pub mod modelstore;
pub mod notificationstore;
pub mod outboxstore;
pub mod pairs;
pub mod principalstore;
pub mod seeders;
pub mod sessionstore;
pub mod store;
pub mod test_support;
pub mod tokenstore;
pub mod tx;
pub mod txport;

pub use batchledger::PgBatchLedger;
pub use jobstore::PgJobStore;
pub use membershipstore::PgMembershipStore;
pub use modelstore::PgModelStore;
pub use notificationstore::PgNotificationStore;
pub use outboxstore::PgOutboxStore;
pub use principalstore::{PgCredentialStore, PgPrincipalStore};
pub use seeders::{DevTenantSeeder, SeedReport, Seeder, SeederRegistry};
pub use sessionstore::PgSessionStore;
pub use sqlx::postgres::{PgPool, PgPoolOptions};
pub use store::PgIdempotency;
pub use tokenstore::PgTokenStore;
pub use tx::PostgresBackend;
pub use txport::{PgTxHandle, PostgresTransactions};

use kelson_foundation::AppError;
use sqlx::Acquire;
use uuid::Uuid;

pub async fn create_pool(url: &str, max_connections: u32) -> Result<PgPool, AppError> {
    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect(url)
        .await
        .map_err(|e| AppError::persistence(format!("pool connect: {e}")))
}

/// Advisory-lock key guarding every migration run (docs/19 Wave 5): two
/// concurrent runs serialize instead of corrupting the bookkeeping.
pub const MIGRATION_LOCK_KEY: i64 = 7259;

/// Fail-closed acquisition deadline: a lock holder wedged this long refuses
/// the run with an error instead of hanging it.
pub const MIGRATION_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Poll interval while waiting for the migration lock.
pub const MIGRATION_LOCK_POLL: std::time::Duration = std::time::Duration::from_millis(200);

/// Take the session-scoped migration advisory lock ([`MIGRATION_LOCK_KEY`])
/// on `conn`, polling until [`MIGRATION_LOCK_TIMEOUT`] — fail-closed: a
/// wedged holder errors out instead of hanging the run. Session-scoped (not
/// xact), so the unlock must run on this same connection — callers pair
/// this with [`release_migration_lock`] on the connection they passed here.
pub async fn acquire_migration_lock(conn: &mut sqlx::PgConnection) -> Result<(), AppError> {
    acquire_migration_lock_within(conn, MIGRATION_LOCK_TIMEOUT, MIGRATION_LOCK_POLL).await
}

/// Testable core of the lock acquisition with an explicit deadline and poll
/// interval (the contention test uses a short deadline so the fail-closed
/// path runs fast).
pub async fn acquire_migration_lock_within(
    conn: &mut sqlx::PgConnection,
    timeout: std::time::Duration,
    poll: std::time::Duration,
) -> Result<(), AppError> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(MIGRATION_LOCK_KEY)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| AppError::persistence(format!("migrate: advisory lock: {e}")))?;
        if acquired {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(AppError::persistence(format!(
                "migrate: timed out after {}ms waiting for advisory lock {} (another migration run holds it; fail-closed)",
                timeout.as_millis(),
                MIGRATION_LOCK_KEY
            )));
        }
        tokio::time::sleep(poll).await;
    }
}

/// Release the migration advisory lock on `conn` (the same connection it
/// was taken on). A false unlock means the session did not hold the lock —
/// surfaced as an error rather than swallowed (the invariant is lock/run
/// pairing).
pub async fn release_migration_lock(conn: &mut sqlx::PgConnection) -> Result<(), AppError> {
    let released: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1)")
        .bind(MIGRATION_LOCK_KEY)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| AppError::persistence(format!("migrate: advisory unlock: {e}")))?;
    if released {
        Ok(())
    } else {
        Err(AppError::persistence(format!(
            "migrate: advisory lock {} was not held by this connection",
            MIGRATION_LOCK_KEY
        )))
    }
}

/// Migrations from ../../../database/migrations (boot-time, idempotent),
/// then the Wave 5 rollback-capable pairs under `database/migrations-pairs`
/// (validated fail-closed before any DDL; pairs carry the batch bookkeeping
/// and return their applied names). The whole run holds
/// [`MIGRATION_LOCK_KEY`] on a single pooled connection, so concurrent runs
/// serialize (fail-closed on timeout).
pub async fn run_migrations(
    pool: &PgPool,
    root: &std::path::Path,
) -> Result<Vec<String>, AppError> {
    // Validate the pairs directory first: a broken pair shipment refuses
    // the run before the first statement (sqlx singles included).
    let _validated = pairs::scan_pairs(root)?;
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| AppError::persistence(format!("migrate: connect: {e}")))?;
    acquire_migration_lock(&mut conn).await?;
    // The run's outcome wins over a release failure, but the release is
    // always attempted first — a held lock would wedge every later run.
    let outcome = run_migrations_held(&mut conn, root).await;
    let released = release_migration_lock(&mut conn).await;
    let names = outcome?;
    released?;
    Ok(names)
}

/// The migration run proper — sqlx singles, then pairs — on a connection
/// the caller already holds [`MIGRATION_LOCK_KEY`] on. External callers
/// (the console's `migrate:refresh`, which locks across rollback + apply)
/// must take the lock first via [`acquire_migration_lock`]; prefer
/// [`run_migrations`] otherwise.
pub async fn run_migrations_held(
    conn: &mut sqlx::PgConnection,
    root: &std::path::Path,
) -> Result<Vec<String>, AppError> {
    let migrator = sqlx::migrate!("../../../database/migrations");
    migrator
        .run(&mut *conn)
        .await
        .map_err(|e| AppError::persistence(format!("migrate: {e}")))?;
    pairs::apply_pending(&mut *conn, root).await
}

/// Dev seeder: tenant row (platform plane writes — run with a platform context).
pub async fn seed_tenant(pool: &PgPool, id: Uuid, slug: &str, name: &str) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO tenants (id, slug, name, status, plan, created_at)
         VALUES ($1,$2,$3,'active','standard', now())
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(id)
    .bind(slug)
    .bind(name)
    .execute(pool)
    .await
    .map_err(|e| AppError::persistence(format!("seed tenant: {e}")))?;
    Ok(())
}

/// Tenant-guarded pool: wraps both pools and ensures session state is reset
/// on connection release (SEC-002, no GUC leakage). Pooled connections
/// guarded by tenant context (RLS enforcement).
///
/// Two pools: tenant (`kelson_tenant` role, RLS enforced) and platform
/// (`kelson_platform` role, break-glass, NO RLS policies applied).
pub struct TenantGuardedPool {
    tenant_pool: PgPool,   // connects as kelson_tenant (RLS enforced)
    platform_pool: PgPool, // connects as kelson_platform (break-glass, NO RLS)
}

/// Boxed async task bound to a borrowed transaction: the callback passed to
/// [`TenantGuardedPool::with_tenant`] / [`TenantGuardedPool::platform`] returns
/// this so queries can `.await` inside while the transaction is alive.
pub type TxnTask<'a, T> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<T, AppError>> + Send + 'a>>;

impl TenantGuardedPool {
    /// Create both pools from config. `tenant_url` uses `kelson_tenant` role,
    /// `platform_url` uses `kelson_platform` role (separate connection strings).
    pub async fn new(
        tenant_url: &str,
        platform_url: &str,
        max_connections: u32,
    ) -> Result<Self, AppError> {
        let tenant_pool = create_pool(tenant_url, max_connections).await?;
        let platform_pool = create_pool(platform_url, max_connections).await?;
        Ok(Self {
            tenant_pool,
            platform_pool,
        })
    }

    /// Execute an async closure within a tenant transaction.
    /// ALWAYS sets `kelson.tenant_id` GUC, verifies it on commit, and
    /// resets session state on release via `DISCARD ALL`.
    ///
    /// Callers write `|tx| Box::pin(async move { ... })` — the future
    /// borrows the transaction for its whole execution:
    ///
    /// ```ignore
    /// pool.with_tenant(tenant, |tx| Box::pin(async move {
    ///     sqlx::query("SELECT 1").fetch_all(&mut *tx).await.map_err(...)
    /// })).await?
    /// ```
    pub async fn with_tenant<T, F>(
        &self,
        tenant_id: kelson_foundation::TenantId,
        f: F,
    ) -> Result<T, AppError>
    where
        T: Send,
        F: for<'a, 'b> FnOnce(&'a mut sqlx::Transaction<'b, sqlx::Postgres>) -> TxnTask<'a, T>
            + Send,
    {
        let mut conn = self
            .tenant_pool
            .acquire()
            .await
            .map_err(|e| AppError::persistence(format!("acquire: {e}")))?;

        // Set session-scoped GUC (not LOCAL — persists across transaction)
        sqlx::query("SELECT set_config('kelson.tenant_id', $1, false)")
            .bind(tenant_id.to_string())
            .execute(&mut *conn)
            .await
            .map_err(|e| AppError::persistence(format!("set tenant guc: {e}")))?;

        let mut tx = conn
            .begin()
            .await
            .map_err(|e| AppError::persistence(format!("begin: {e}")))?;

        let result = f(&mut tx).await?;

        // Verify GUC still set before commit (catches handler bugs that RESET it)
        let guc: Option<String> = sqlx::query_scalar("SHOW kelson.tenant_id")
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| AppError::persistence(format!("verify guc: {e}")))?;
        if guc != Some(tenant_id.to_string()) {
            return Err(AppError::persistence(
                "tenant GUC mismatch: handler must not RESET kelson.tenant_id",
            ));
        }

        tx.commit()
            .await
            .map_err(|e| AppError::persistence(format!("commit: {e}")))?;

        // DISCARD ALL on release: resets GUC, advisory locks, prepared statements, etc.
        sqlx::query("DISCARD ALL")
            .execute(&mut *conn)
            .await
            .map_err(|e| AppError::persistence(format!("discard all: {e}")))?;

        Ok(result)
    }

    /// Execute an async closure within a platform (break-glass) transaction.
    /// NEVER sets `kelson.tenant_id` — uses separate pool with `kelson_platform` role.
    /// RLS policies do NOT apply to this role. Same call shape as
    /// [`with_tenant`](Self::with_tenant): `|tx| Box::pin(async move { ... })`.
    pub async fn platform<T, F>(&self, f: F) -> Result<T, AppError>
    where
        T: Send,
        F: for<'a, 'b> FnOnce(&'a mut sqlx::Transaction<'b, sqlx::Postgres>) -> TxnTask<'a, T>
            + Send,
    {
        let mut conn = self
            .platform_pool
            .acquire()
            .await
            .map_err(|e| AppError::persistence(format!("platform acquire: {e}")))?;

        let mut tx = conn
            .begin()
            .await
            .map_err(|e| AppError::persistence(format!("platform begin: {e}")))?;

        let result = f(&mut tx).await?;

        tx.commit()
            .await
            .map_err(|e| AppError::persistence(format!("platform commit: {e}")))?;

        // DISCARD ALL for hygiene (no GUC was set, but cleans advisory locks etc.)
        sqlx::query("DISCARD ALL")
            .execute(&mut *conn)
            .await
            .map_err(|e| AppError::persistence(format!("platform discard all: {e}")))?;

        Ok(result)
    }

    /// Get reference to tenant pool for read-only operations that don't need transactions.
    /// Caller MUST manually set/verify GUC if used outside `with_tenant`.
    pub fn tenant_pool(&self) -> &PgPool {
        &self.tenant_pool
    }

    /// Get reference to platform pool for break-glass operations.
    pub fn platform_pool(&self) -> &PgPool {
        &self.platform_pool
    }
}
