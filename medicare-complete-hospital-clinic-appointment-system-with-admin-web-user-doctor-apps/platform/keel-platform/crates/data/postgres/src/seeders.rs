//! Seeders (item 20, L19): named, idempotent data-population routines driven
//! by `keel seed` (tools/cli). Modules register their own seeders; the
//! framework ships a dev-tenant seeder as proof of the contract. Convention:
//! seeders are re-runnable (guarded inserts / ON CONFLICT DO NOTHING).

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use keel_foundation::AppError;
use sqlx::PgPool;

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
    fn run<'a>(&'a self, pool: &'a PgPool) -> BoxFuture<'a, Result<SeedReport, AppError>>;
}

/// Ordered registry; `keel seed` runs seeders in registration order and
/// stops at the first error (seed runs are not transactional by design —
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
        pool: &PgPool,
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
/// id plus ON CONFLICT DO NOTHING keeps reruns no-op.
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

    fn run<'a>(&'a self, pool: &'a PgPool) -> BoxFuture<'a, Result<SeedReport, AppError>> {
        Box::pin(async move {
            let result = sqlx::query(
                "INSERT INTO tenants (id, slug, name, status, plan)
                 VALUES ($1, 'dev', 'Development Tenant', 'active', 'standard')
                 ON CONFLICT (slug) DO NOTHING",
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

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeSeeder {
        name: &'static str,
        rows: u64,
    }

    impl Seeder for FakeSeeder {
        fn name(&self) -> &str {
            self.name
        }

        fn run<'a>(&'a self, _pool: &'a PgPool) -> BoxFuture<'a, Result<SeedReport, AppError>> {
            Box::pin(async move {
                Ok(SeedReport {
                    inserted: self.rows,
                    skipped: 0,
                })
            })
        }
    }

    fn lazy_pool() -> PgPool {
        sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://localhost/keel_test")
            .expect("lazy pool must build")
    }

    #[tokio::test]
    async fn registry_runs_in_registration_order() {
        let mut reg = SeederRegistry::new();
        reg.register(Arc::new(FakeSeeder { name: "a", rows: 1 }))
            .unwrap();
        reg.register(Arc::new(FakeSeeder { name: "b", rows: 2 }))
            .unwrap();
        assert!(reg
            .register(Arc::new(FakeSeeder { name: "a", rows: 9 }))
            .is_err());
        assert_eq!(reg.names(), vec!["a", "b"]);

        let pool = lazy_pool();
        let results = reg.run(&pool, None).await.unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(
            results[0],
            (
                "a".to_string(),
                SeedReport {
                    inserted: 1,
                    skipped: 0
                }
            )
        );
        assert_eq!(
            results[1],
            (
                "b".to_string(),
                SeedReport {
                    inserted: 2,
                    skipped: 0
                }
            )
        );
    }

    #[tokio::test]
    async fn registry_filters_and_rejects_unknown_names() {
        let mut reg = SeederRegistry::new();
        reg.register(Arc::new(FakeSeeder { name: "a", rows: 1 }))
            .unwrap();
        reg.register(Arc::new(FakeSeeder { name: "b", rows: 2 }))
            .unwrap();
        let pool = lazy_pool();

        let only = vec!["b".to_string()];
        let results = reg.run(&pool, Some(&only)).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "b");

        let missing = vec!["ghost".to_string()];
        assert!(reg.run(&pool, Some(&missing)).await.is_err());
    }
}
