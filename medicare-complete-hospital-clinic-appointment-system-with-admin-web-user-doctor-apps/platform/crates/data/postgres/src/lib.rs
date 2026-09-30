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

/// Migrations from ../../../database/migrations (boot-time, idempotent),
/// then the Wave 5 rollback-capable pairs under `database/migrations-pairs`
/// (validated fail-closed before any DDL; pairs carry the batch bookkeeping
/// and return their applied names).
pub async fn run_migrations(
    pool: &PgPool,
    root: &std::path::Path,
) -> Result<Vec<String>, AppError> {
    // Validate the pairs directory first: a broken pair shipment refuses
    // the run before the first statement (sqlx singles included).
    let _validated = pairs::scan_pairs(root)?;
    let migrator = sqlx::migrate!("../../../database/migrations");
    migrator
        .run(pool)
        .await
        .map_err(|e| AppError::persistence(format!("migrate: {e}")))?;
    pairs::apply_pending(pool, root).await
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
