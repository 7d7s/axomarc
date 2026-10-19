//! Shared `ModelStore` conformance matrix (LF-007): the pg and mysql drivers
//! run the SAME suite from `kelson_models::conformance` — this file only wires
//! pools, migrations and seeded tenants, so the contract lives in one place
//! and a driver drift cannot hide behind a hand-copied twin. The in-memory
//! backend runs the same cases in `kelson-models`' own tests, so the TestKit
//! fake is held to the identical contract.
//!
//! Activated with `--features pg-tests` (same gate as the sibling matrices);
//! every test skips gracefully when DATABASE_URL is unset or points at the
//! other fabric, so local runs without a database stay green.

#![cfg(feature = "pg-tests")]

use kelson_foundation::TenantId;
use kelson_models::conformance;
use kelson_postgres::{create_pool, run_migrations, PgModelStore, PgPool};
use uuid::Uuid;

async fn pool() -> Option<PgPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(url) if !url.is_empty() => url,
        _ => {
            eprintln!("pg-tests: DATABASE_URL not set; skipping");
            return None;
        }
    };
    match create_pool(&url, 4).await {
        Ok(pool) => {
            run_migrations(&pool, std::path::Path::new("."))
                .await
                .expect("migrations apply");
            Some(pool)
        }
        Err(e) => {
            eprintln!("pg-tests: cannot connect ({e}); skipping");
            None
        }
    }
}

async fn seed_tenant(pool: &PgPool) -> TenantId {
    let tenant = TenantId::new();
    sqlx::query("INSERT INTO tenants (id, slug, name) VALUES ($1, $2, $3)")
        .bind(tenant.as_uuid())
        .bind(format!("pg-conf-{}", Uuid::now_v7()))
        .bind("Pg Conformance Tenant")
        .execute(pool)
        .await
        .expect("tenant seeds");
    tenant
}

async fn mysql_pool() -> Option<kelson_mysql::MySqlPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(url) if !url.is_empty() => url,
        _ => {
            eprintln!("mysql matrix: DATABASE_URL not set; skipping");
            return None;
        }
    };
    if !url.starts_with("mysql://") {
        eprintln!("mysql matrix: DATABASE_URL is not mysql://; skipping");
        return None;
    }
    match kelson_mysql::create_pool(&url, 4).await {
        Ok(pool) => {
            kelson_mysql::run_migrations(&pool, std::path::Path::new("."))
                .await
                .expect("migrations apply");
            Some(pool)
        }
        Err(e) => {
            eprintln!("mysql matrix: cannot connect ({e}); skipping");
            None
        }
    }
}

async fn mysql_seed_tenant(pool: &kelson_mysql::MySqlPool) -> TenantId {
    let tenant = TenantId::new();
    sqlx::query("INSERT INTO tenants (id, slug, name) VALUES (?, ?, ?)")
        .bind(tenant.as_uuid())
        .bind(format!("mysql-conf-{}", Uuid::now_v7()))
        .bind("MySQL Conformance Tenant")
        .execute(pool)
        .await
        .expect("tenant seeds");
    tenant
}

// --------------------------------------------------- PostgreSQL wrappers

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_conformance_crud_roundtrip() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    conformance::crud_roundtrip(&store, seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_conformance_tenant_isolation() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    let tenant_a = seed_tenant(&pool).await;
    let tenant_b = seed_tenant(&pool).await;
    conformance::tenant_isolation(&store, tenant_a, tenant_b);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_conformance_soft_delete_restore() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    conformance::soft_delete_restore(&store, seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_conformance_update_on_a_trashed_row() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    conformance::update_on_a_trashed_row_writes_and_stays_trashed(&store, seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_conformance_delete_twice() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    conformance::delete_twice_is_a_noop_and_does_not_restamp(&store, seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_conformance_reads_and_batch_writes_honour_the_trash_veil() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    conformance::reads_and_batch_writes_honour_the_trash_veil(&store, seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_conformance_find_by_and_find_for_update() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    conformance::find_by_and_find_for_update(&store, seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_conformance_list_sort_pagination_and_validation() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    conformance::list_sort_pagination_and_validation(&store, seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_conformance_aggregate_and_exists() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    conformance::aggregate_and_exists(&store, seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_conformance_increment_counter() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    conformance::increment_counter(&store, seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_conformance_batch_update_and_delete_filtered() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    conformance::batch_update_and_delete_filtered(&store, seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_conformance_force_delete_purges_trash() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    conformance::force_delete_purges_trash(&store, seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_conformance_upsert_merges_on_plain_unique() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    let tenant_a = seed_tenant(&pool).await;
    let tenant_b = seed_tenant(&pool).await;
    conformance::upsert_merges_on_plain_unique(&store, tenant_a, tenant_b);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_conformance_insert_or_ignore_dedupes() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    let tenant_a = seed_tenant(&pool).await;
    let tenant_b = seed_tenant(&pool).await;
    conformance::insert_or_ignore_dedupes(&store, tenant_a, tenant_b);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_conformance_hard_delete_and_restore_noop() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    conformance::hard_delete_and_restore_noop(&store, seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_conformance_unique_index_blocks_double_booking() {
    let Some(pool) = pool().await else { return };
    let store = PgModelStore::new(pool.clone());
    conformance::unique_index_blocks_double_booking(&store, seed_tenant(&pool).await);
}

// -------------------------------------------------------- MySQL wrappers

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_conformance_crud_roundtrip() {
    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlModelStore::new(pool.clone());
    conformance::crud_roundtrip(&store, mysql_seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_conformance_tenant_isolation() {
    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlModelStore::new(pool.clone());
    let tenant_a = mysql_seed_tenant(&pool).await;
    let tenant_b = mysql_seed_tenant(&pool).await;
    conformance::tenant_isolation(&store, tenant_a, tenant_b);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_conformance_soft_delete_restore() {
    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlModelStore::new(pool.clone());
    conformance::soft_delete_restore(&store, mysql_seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_conformance_update_on_a_trashed_row() {
    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlModelStore::new(pool.clone());
    conformance::update_on_a_trashed_row_writes_and_stays_trashed(
        &store,
        mysql_seed_tenant(&pool).await,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_conformance_delete_twice() {
    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlModelStore::new(pool.clone());
    conformance::delete_twice_is_a_noop_and_does_not_restamp(&store, mysql_seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_conformance_reads_and_batch_writes_honour_the_trash_veil() {
    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlModelStore::new(pool.clone());
    conformance::reads_and_batch_writes_honour_the_trash_veil(
        &store,
        mysql_seed_tenant(&pool).await,
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_conformance_find_by_and_find_for_update() {
    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlModelStore::new(pool.clone());
    conformance::find_by_and_find_for_update(&store, mysql_seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_conformance_list_sort_pagination_and_validation() {
    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlModelStore::new(pool.clone());
    conformance::list_sort_pagination_and_validation(&store, mysql_seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_conformance_aggregate_and_exists() {
    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlModelStore::new(pool.clone());
    conformance::aggregate_and_exists(&store, mysql_seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_conformance_increment_counter() {
    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlModelStore::new(pool.clone());
    conformance::increment_counter(&store, mysql_seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_conformance_batch_update_and_delete_filtered() {
    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlModelStore::new(pool.clone());
    conformance::batch_update_and_delete_filtered(&store, mysql_seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_conformance_force_delete_purges_trash() {
    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlModelStore::new(pool.clone());
    conformance::force_delete_purges_trash(&store, mysql_seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_conformance_upsert_merges_on_plain_unique() {
    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlModelStore::new(pool.clone());
    let tenant_a = mysql_seed_tenant(&pool).await;
    let tenant_b = mysql_seed_tenant(&pool).await;
    conformance::upsert_merges_on_plain_unique(&store, tenant_a, tenant_b);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_conformance_insert_or_ignore_dedupes() {
    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlModelStore::new(pool.clone());
    let tenant_a = mysql_seed_tenant(&pool).await;
    let tenant_b = mysql_seed_tenant(&pool).await;
    conformance::insert_or_ignore_dedupes(&store, tenant_a, tenant_b);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_conformance_hard_delete_and_restore_noop() {
    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlModelStore::new(pool.clone());
    conformance::hard_delete_and_restore_noop(&store, mysql_seed_tenant(&pool).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_conformance_unique_index_blocks_double_booking() {
    let Some(pool) = mysql_pool().await else { return };
    let store = kelson_mysql::MySqlModelStore::new(pool.clone());
    conformance::unique_index_blocks_double_booking(&store, mysql_seed_tenant(&pool).await);
}
