//! Seeders (item 20, L19): named, idempotent data-population routines driven
//! by `keel seed` (tools/cli). Modules register their own seeders; the
//! framework ships a dev-tenant seeder as proof of the contract. Convention:
//! seeders are re-runnable (guarded inserts / ON CONFLICT DO NOTHING).
//!
//! MySQL port notes (B-31), vs the Postgres twin
//! (`keel_postgres::seeders`):
//! - the `Seeder` trait takes `&MySqlPool` instead of `&PgPool` (rule 14);
//!   everything else (the trait shape, the ordered `SeederRegistry`, its
//!   register/get/names/run semantics, error styles and the
//!   `tracing::info!` pass log) is a line-for-line twin.
//! - `DevTenantSeeder`'s `ON CONFLICT (slug) DO NOTHING` becomes rule 6's
//!   `INSERT ... SELECT ... FROM DUAL WHERE NOT EXISTS (slug = ...)` guard
//!   over EXACTLY the pg conflict target (the slug): `rows_affected` is
//!   still 1 = inserted / 0 = skipped, a rerun stays a no-op, and a
//!   duplicate stable id under a DIFFERENT slug still errors because the
//!   guard does not cover the primary key - same as pg. One inherent
//!   divergence: the guard is check-then-insert, so two seed runs racing
//!   on the very first insert can surface a duplicate-key error where
//!   pg's ON CONFLICT would silently skip.
//! - the pg file's test module is deliberately not ported: this crate opts
//!   out of its own test target (the live matrix rides the shared suites).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use keel_foundation::AppError;
use sqlx::mysql::MySqlPool;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Outcome of one seeder pass.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeedReport {
    pub inserted: u64,
    pub skipped: u64,
}

/// One named seeding routine.
pub trait Seeder: Send + Sync {
    fn name(&self) -> &str;
    fn run<'a>(&'a self, pool: &'a MySqlPool) -> BoxFuture<'a, Result<SeedReport, AppError>>;
}

/// Ordered registry; `keel seed` runs seeders in registration order and
/// stops at the first error (seed runs are not transactional by design -
/// each seeder owns its own idempotency).
#[derive(Default)]
pub struct SeederRegistry {
    seeders: Vec<Arc<dyn Seeder>>,
}

impl SeederRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, seeder: Arc<dyn Seeder>) -> Result<(), AppError> {
        if self.seeders.iter().any(|s| s.name() == seeder.name()) {
            return Err(AppError::validation(format!(
                "seeder registered twice: {}",
                seeder.name()
            )));
        }
        self.seeders.push(seeder);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Seeder>> {
        self.seeders.iter().find(|s| s.name() == name).cloned()
    }

    pub fn names(&self) -> Vec<&str> {
        self.seeders.iter().map(|s| s.name()).collect()
    }

    /// Run all (or only the requested) seeders in registration order.
    pub async fn run(
        &self,
        pool: &MySqlPool,
        only: Option<&[String]>,
    ) -> Result<Vec<(String, SeedReport)>, AppError> {
        if let Some(only) = only {
            for name in only {
                if self.get(name).is_none() {
                    return Err(AppError::not_found(format!("seeder {name}")));
                }
            }
        }
        let mut results = Vec::new();
        for seeder in &self.seeders {
            if let Some(only) = only {
                if !only.iter().any(|n| n == seeder.name()) {
                    continue;
                }
            }
            let report = seeder
                .run(pool)
                .await
                .map_err(|e| e.with_meta("seeder", seeder.name().to_string()))?;
            tracing::info!(
                seeder = seeder.name(),
                inserted = report.inserted,
                skipped = report.skipped,
                "seed pass"
            );
            results.push((seeder.name().to_string(), report));
        }
        Ok(results)
    }
}

/// Framework demo seeder: idempotent development tenant. The deterministic
/// id plus the guarded insert keeps reruns no-op.
pub struct DevTenantSeeder;

impl DevTenantSeeder {
    /// Stable, printed-in-docs id for the dev tenant.
    pub const TENANT_ID: uuid::Uuid =
        uuid::Uuid::from_u128(0x0195_8f62_0000_7000_8000_0000_0000_00a1);
}

impl Seeder for DevTenantSeeder {
    fn name(&self) -> &str {
        "dev-tenant"
    }

    fn run<'a>(&'a self, pool: &'a MySqlPool) -> BoxFuture<'a, Result<SeedReport, AppError>> {
        Box::pin(async move {
            let result = sqlx::query(
                "INSERT INTO tenants (id, slug, name, status, plan)
                 SELECT ?, 'dev', 'Development Tenant', 'active', 'standard'
                 FROM DUAL WHERE NOT EXISTS (SELECT 1 FROM tenants WHERE slug = 'dev')",
            )
            .bind(Self::TENANT_ID)
            .execute(pool)
            .await
            .map_err(|e| AppError::persistence(format!("dev-tenant seed: {e}")))?;
            let inserted = result.rows_affected();
            Ok(SeedReport {
                inserted,
                skipped: u64::from(inserted == 0),
            })
        })
    }
}
