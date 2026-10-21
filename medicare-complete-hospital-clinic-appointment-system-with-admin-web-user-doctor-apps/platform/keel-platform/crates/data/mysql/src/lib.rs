//! # kelson-mysql
//! The resource fabric for MySQL 8.0.19+ (Wave 9 B-31, docs/19): pool +
//! SQL-first migrations + the second driver behind the existing store ports.
//! `kelson-postgres` stays the Postgres fabric; this crate never depends on it
//! (the shared sync-store bridge and the pool handle are deliberate copies /
//! a neutral enum, so neither publishable fabric reaches the other).
//!
//! Divergences from the Postgres blade, all documented in docs/05-DATABASE:
//! - **no RLS** — tenant isolation rests solely on the tenant-guarded query
//!   builders (every port keeps the explicit `tenant_id` predicate it already
//!   carried for the `SET LOCAL kelson.tenant_id` anchor);
//! - **no `RETURNING`** — writes re-select by key on the same connection;
//! - **no advisory-lock keys** — the migration lock is `GET_LOCK()`, the
//!   same session-scoped fail-closed contract as `pg_try_advisory_lock`;
//! - **rollback-capable pairs are transaction-free** — the same
//!   `database/migrations-pairs` shipment, `keel_migrations` bookkeeping and
//!   Laravel batches as `kelson_postgres::pairs`, but applied statement by
//!   statement with fail-stop-and-report, because MySQL DDL auto-commits
//!   (see [`pairs`] for the whole divergence list);
//! - uuid columns are `BINARY(16)` (sqlx's MySQL `Uuid` codec is binary),
//!   timestamps are `DATETIME(6)` (sqlx stores `OffsetDateTime` as UTC).

pub mod batchledger;
pub mod bridge;
pub mod explain;
pub mod jobstore;
pub mod modelstore;
pub mod notificationstore;
pub mod outboxstore;
pub mod pairs;
pub mod principalstore;
pub mod schema;
pub mod seeders;
pub mod sessionstore;
pub mod store;
pub mod test_support;
pub mod tokenstore;
pub mod tx;
pub mod txport;

pub use batchledger::MySqlBatchLedger;
pub use explain::{explain_analyze, ExplainPlan};
pub use jobstore::MySqlJobStore;
pub use modelstore::MySqlModelStore;
pub use notificationstore::MySqlNotificationStore;
pub use outboxstore::MySqlOutboxStore;
pub use principalstore::{MySqlCredentialStore, MySqlPrincipalStore};
pub use seeders::{DevTenantSeeder, SeedReport, Seeder, SeederRegistry};
pub use sessionstore::MySqlSessionStore;
pub use sqlx::mysql::{MySqlPool, MySqlPoolOptions};
pub use store::MySqlIdempotency;
pub use tokenstore::MySqlTokenStore;
pub use tx::MySqlBackend;
pub use txport::{MySqlTxHandle, MySqlTransactions};

use kelson_foundation::AppError;
use uuid::Uuid;

/// The boot-time driver switch (B-31): boot constructs exactly one variant
/// from `[database].driver` + the probe, then hands it to the store
/// constructors in kelson-demo / kelson-worker / kelson-console. The Postgres
/// variant carries `sqlx::PgPool` — the same type `kelson_postgres::PgPool`
/// aliases — which is what keeps the two publishable fabrics independent.
#[derive(Clone, Debug)]
pub enum DbPool {
    Postgres(sqlx::PgPool),
    MySql(MySqlPool),
}

impl DbPool {
    /// `[database].driver` spelling for health messages and logs.
    pub fn driver_name(&self) -> &'static str {
        match self {
            DbPool::Postgres(_) => "postgres",
            DbPool::MySql(_) => "mysql",
        }
    }
}

pub async fn create_pool(url: &str, max_connections: u32) -> Result<MySqlPool, AppError> {
    MySqlPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                // Pin EVERY connection to UTC, explicitly.
                //
                // MySQL's `DATETIME` carries no timezone, so a server-side
                // `NOW(6)` (or a `DEFAULT CURRENT_TIMESTAMP(6)`) lands as
                // whatever the SERVER's zone says. The driver decodes it back
                // as UTC, so on a server whose zone is not UTC the stored
                // instant is off by the zone offset — measured on this host:
                // +5h30m (19800s). sqlx's connect options pin
                // `time_zone='+00:00'` today; doing it HERE makes the
                // guarantee the framework's own rather than a dependency's
                // default that could change underneath it.
                sqlx::query("SET time_zone = '+00:00'")
                    .execute(conn)
                    .await
                    .map(|_| ())
            })
        })
        .connect(url)
        .await
        .map_err(|e| AppError::persistence(format!("pool connect: {e}")))
}

/// Migration lock name (server-wide, session-scoped): the MySQL counterpart
/// of the Postgres advisory key — two concurrent runs serialize instead of
/// corrupting the bookkeeping.
pub const MIGRATION_LOCK_NAME: &str = "kelson:migrate";

/// Fail-closed acquisition deadline: a lock holder wedged this long refuses
/// the run with an error instead of hanging it.
pub const MIGRATION_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Poll interval while waiting for the migration lock.
pub const MIGRATION_LOCK_POLL: std::time::Duration = std::time::Duration::from_millis(200);

/// Take the session-scoped migration lock ([`MIGRATION_LOCK_NAME`]) on
/// `conn`, polling `GET_LOCK(name, 0)` until [`MIGRATION_LOCK_TIMEOUT`] —
/// fail-closed: a wedged holder errors out instead of hanging the run.
/// Session-scoped, so the release must run on this same connection.
pub async fn acquire_migration_lock(conn: &mut sqlx::MySqlConnection) -> Result<(), AppError> {
    acquire_migration_lock_within(conn, MIGRATION_LOCK_TIMEOUT, MIGRATION_LOCK_POLL).await
}

/// Testable core of the lock acquisition with an explicit deadline and poll
/// interval (the contention path mirrors the Postgres lock tests).
pub async fn acquire_migration_lock_within(
    conn: &mut sqlx::MySqlConnection,
    timeout: std::time::Duration,
    poll: std::time::Duration,
) -> Result<(), AppError> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let acquired: Option<i64> = sqlx::query_scalar("SELECT GET_LOCK(?, 0)")
            .bind(MIGRATION_LOCK_NAME)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| AppError::persistence(format!("migrate: lock: {e}")))?;
        match acquired {
            Some(1) => return Ok(()),
            Some(0) => {}
            // GET_LOCK only ever returns 1, 0 or NULL; anything else is an
            // unexpected server reply - fail-closed like the NULL arm.
            Some(other) => {
                return Err(AppError::persistence(format!(
                    "migrate: GET_LOCK returned unexpected value {other}; fail-closed"
                )));
            }
            None => {
                return Err(AppError::persistence(
                    "migrate: GET_LOCK returned no result (invalid lock name or no privilege); fail-closed",
                ));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Err(AppError::persistence(format!(
                "migrate: timed out after {}ms waiting for migration lock {} (another migration run holds it; fail-closed)",
                timeout.as_millis(),
                MIGRATION_LOCK_NAME
            )));
        }
        tokio::time::sleep(poll).await;
    }
}

/// Release the migration lock on `conn` (the same connection it was taken
/// on). A false release means the session did not hold the lock — surfaced
/// as an error rather than swallowed (the invariant is lock/run pairing).
pub async fn release_migration_lock(conn: &mut sqlx::MySqlConnection) -> Result<(), AppError> {
    let released: Option<i64> = sqlx::query_scalar("SELECT RELEASE_LOCK(?)")
        .bind(MIGRATION_LOCK_NAME)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| AppError::persistence(format!("migrate: lock release: {e}")))?;
    if released == Some(1) {
        Ok(())
    } else {
        Err(AppError::persistence(format!(
            "migrate: migration lock {} was not held by this connection",
            MIGRATION_LOCK_NAME
        )))
    }
}

/// Migrations from ../../../database/migrations-mysql (boot-time,
/// idempotent, SQL-first — no schema is built from Rust), then the
/// rollback-capable pairs under `database/migrations-pairs` (validated
/// fail-closed before any DDL; pairs carry the batch bookkeeping and return
/// their applied names). The whole run holds [`MIGRATION_LOCK_NAME`] on a
/// single pooled connection, so concurrent runs serialize (fail-closed on
/// timeout).
pub async fn run_migrations(
    pool: &MySqlPool,
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
/// the caller already holds [`MIGRATION_LOCK_NAME`] on. External callers
/// take the lock first via [`acquire_migration_lock`]; prefer
/// [`run_migrations`] otherwise.
pub async fn run_migrations_held(
    conn: &mut sqlx::MySqlConnection,
    root: &std::path::Path,
) -> Result<Vec<String>, AppError> {
    let migrator = sqlx::migrate!("../../../database/migrations-mysql");
    migrator
        .run(&mut *conn)
        .await
        .map_err(|e| AppError::persistence(format!("migrate: {e}")))?;
    pairs::apply_pending(&mut *conn, root).await
}

/// Dev seeder: tenant row (platform plane writes — run with a platform
/// context). The `WHERE NOT EXISTS` guard keeps the Postgres conflict
/// target exact: a duplicate id skips, a slug collision still errors.
pub async fn seed_tenant(pool: &MySqlPool, id: Uuid, slug: &str, name: &str) -> Result<(), AppError> {
    sqlx::query(
        "INSERT INTO tenants (id, slug, name, status, plan, created_at)
         SELECT ?, ?, ?, 'active', 'standard', NOW(6)
         FROM DUAL WHERE NOT EXISTS (SELECT 1 FROM tenants WHERE id = ?)",
    )
    .bind(id)
    .bind(slug)
    .bind(name)
    .bind(id)
    .execute(pool)
    .await
    .map_err(|e| AppError::persistence(format!("seed tenant: {e}")))?;
    Ok(())
}
