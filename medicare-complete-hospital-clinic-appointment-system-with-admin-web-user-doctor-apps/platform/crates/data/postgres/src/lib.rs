//! # keel-postgres
//! The resource fabric for PostgreSQL (11-SPEC §5): pool + migrations + the
//! transaction engine + delta/audit/outbox writers + execution-level idempotency.
//!
//! Invariants implemented here:
//! - tenant anchor via `SET LOCAL keel.tenant_id` + RLS (final containment barrier)
//! - single transaction: business state → DeltaStone → Audit → Outbox → COMMIT
//! - events dispatch only after commit (D-53)
//! - per-stream monotonic delta sequences
//! - runtime role never owns tables (see ../../../database/roles.sql)

pub mod bridge;
pub mod jobstore;
pub mod modelstore;
pub mod notificationstore;
pub mod outboxstore;
pub mod pairs;
pub mod principalstore;
pub mod seeders;
pub mod sessionstore;
pub mod store;
pub mod tokenstore;
pub mod tx;
pub mod txport;

pub use jobstore::PgJobStore;
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

use keel_foundation::AppError;
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
