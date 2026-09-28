//! kernel-db: pool + boot-time migrations + the Tx wrapper that enforces:
//! tenant GUC → mutation → delta(s) → commit (SRS KER-03, 03-DATA-MODEL §2).
//! NOTE: V1 keeps all tables in `public` behind RLS; the platform/app schema
//! split (09-RESEARCH §7.2) is a W9 refactor task, not a V1 gate.
use kernel_core::{Delta, TenantId};
use sqlx::postgres::{PgPool, PgPoolOptions};
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("sqlx: {0}")]
    Sqlx(#[from] sqlx::Error),
    #[error("migrate: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error("KER-03: mutating tx attempted commit without a delta")]
    MissingDelta,
}

pub async fn create_pool(url: &str, max_connections: u32) -> Result<PgPool, DbError> {
    Ok(PgPoolOptions::new()
        .max_connections(max_connections)
        .connect(url)
        .await?)
}

pub async fn run_migrations(pool: &PgPool) -> Result<(), DbError> {
    let migrator = sqlx::migrate!("./../../migrations");
    migrator.run(pool).await?;
    Ok(())
}

/// Transaction with the tenant GUC set for the whole scope (RLS anchor).
pub struct Tx<'c> {
    inner: sqlx::Transaction<'c, sqlx::Postgres>,
    pub tenant_id: TenantId,
    deltas: Vec<Delta>,
}

impl<'c> Tx<'c> {
    pub async fn begin(pool: &'c PgPool, tenant_id: TenantId) -> Result<Self, DbError> {
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT set_config('medicare.tenant_id', $1, true)")
            .bind(tenant_id.0.to_string())
            .execute(&mut *tx)
            .await?;
        Ok(Self { inner: tx, tenant_id, deltas: Vec::new() })
    }

    pub fn as_mut(&mut self) -> &mut sqlx::Transaction<'c, sqlx::Postgres> {
        &mut self.inner
    }

    /// Executor for sqlx calls (10-GUIDELINES §3.4): `&mut PgConnection` satisfies the Executor bound.
    pub fn executor(&mut self) -> &mut sqlx::PgConnection {
        &mut self.inner
    }

    /// Queue a delta; flushed in the same transaction on commit (DLS-01).
    pub fn emit(&mut self, delta: Delta) {
        self.deltas.push(delta);
    }

    /// KER-03 guard: mutating transactions must emit at least one delta.
    pub async fn commit(mut self, require_delta: bool) -> Result<(), DbError> {
        if require_delta && self.deltas.is_empty() {
            return Err(DbError::MissingDelta);
        }
        for d in &self.deltas {
            kernel_delta::write(&mut *self.inner, d).await.map_err(|e| DbError::Sqlx(e.0))?;
        }
        self.inner.commit().await?;
        Ok(())
    }

    pub async fn rollback(self) -> Result<(), DbError> {
        self.inner.rollback().await?;
        Ok(())
    }
}

/// Seeder helper: create a tenant row (platform role in prod; superuser in dev).
pub async fn seed_tenant(pool: &PgPool, id: Uuid, slug: &str, name: &str) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO tenants (id, slug, name, status, plan)
         VALUES ($1,$2,$3,'active','standard')
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(id)
    .bind(slug)
    .bind(name)
    .execute(pool)
    .await?;
    Ok(())
}