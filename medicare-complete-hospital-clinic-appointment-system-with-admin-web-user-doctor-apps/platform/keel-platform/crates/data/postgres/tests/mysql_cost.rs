//! The MySQL cost-axis matrix.
//!
//! The cheapest enterprise-scale failure is not a crash - it is one
//! tenant's runaway batch write quietly consuming the cluster's IO while
//! every other tenant waits. These cases prove the cost axis: a write that
//! would exceed the tenant's ceiling is **refused before it runs**, and
//! every statement's cost is recorded.
//!
//! Executed against a live MySQL 8.

#![cfg(feature = "pg-tests")]

use kelson_foundation::TenantId;
use kelson_models::store::{FieldFilter, ListOpts, ModelStore};
use kelson_models::{FieldDef, FieldType, ModelDef, Record};
use kelson_mysql::cost::{CostBudget, CostGuard, CostLedger};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

async fn pool() -> Option<kelson_mysql::MySqlPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(url) if url.starts_with("mysql://") => url,
        _ => {
            eprintln!("mysql-cost: DATABASE_URL is not mysql://; skipping");
            return None;
        }
    };
    match kelson_mysql::create_pool(&url, 4).await {
        Ok(pool) => Some(pool),
        Err(e) => {
            eprintln!("mysql-cost: cannot connect ({e}); skipping");
            None
        }
    }
}

/// A tenant-scoped model over a probe table.
fn model(table: &str) -> ModelDef {
    ModelDef::new("CostProbe", table)
        .tenant_scoped()
        .field(FieldDef::new("id", FieldType::Uuid).required())
        .field(FieldDef::new("tenant_id", FieldType::Uuid).required())
        .field(FieldDef::new("amount", FieldType::Int64).required())
        .field(FieldDef::new("deleted_at", FieldType::Timestamp))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_cost_the_budget_refuses_a_runaway_write_before_it_runs() {
    let Some(pool) = pool().await else { return };
    let table = format!("kelson_cost_{}", Uuid::now_v7().simple());
    sqlx::raw_sql(&format!(
        "CREATE TABLE {table} (id BINARY(16) NOT NULL PRIMARY KEY, tenant_id BINARY(16) NOT NULL, amount BIGINT NOT NULL, deleted_at DATETIME(6) NULL)"
    ))
    .execute(&pool)
    .await
    .expect("the probe table is created");

    let tenant = TenantId::new();
    for i in 0..10i64 {
        sqlx::query(&format!(
            "INSERT INTO {table} (id, tenant_id, amount) VALUES (?, ?, ?)"
        ))
        .bind(Uuid::now_v7())
        .bind(*tenant.as_uuid())
        .bind(i)
        .execute(&pool)
        .await
        .expect("the probe row inserts");
    }

    let ledger = Arc::new(Mutex::new(CostLedger::new()));
    // The ceiling: five rows per write. The write below would touch ten.
    let store = kelson_mysql::MySqlModelStore::new(pool.clone())
        .with_cost_guard(CostGuard::with_budget(CostBudget::writes_at_most(5)))
        .with_cost_ledger(Arc::clone(&ledger));

    let mut changes = Record::new();
    changes.insert("amount".into(), serde_json::json!(999));
    let filters = vec![FieldFilter::eq(
        "tenant_id",
        serde_json::json!(tenant.as_uuid().to_string()),
    )];

    let refusal = store
        .update_filtered(&model(&table), Some(tenant), &filters, &changes)
        .expect_err("the over-budget write is refused");
    assert!(
        refusal
            .public_message
            .contains("cost budget refused the write"),
        "the refusal must name the cause: {}",
        refusal.public_message
    );
    assert!(
        refusal.public_message.contains("10 rows")
            && refusal.public_message.contains("ceiling is 5"),
        "the refusal must carry the numbers: {}",
        refusal.public_message
    );

    // The write did NOT happen: the amounts are untouched.
    let max_amount: i64 = sqlx::query_scalar(&format!("SELECT MAX(amount) FROM {table}"))
        .fetch_one(&pool)
        .await
        .expect("the amounts are there");
    assert_eq!(max_amount, 9, "the refused write must not have run");

    // The refusal is on the ledger.
    let cost = ledger
        .lock()
        .expect("the ledger locks")
        .cost_of(&tenant)
        .expect("the tenant is known");
    assert_eq!(cost.refused, 1, "the refusal is recorded");
    assert_eq!(
        cost.statements, 0,
        "the refused write is not counted as work"
    );

    // With a ceiling that fits, the same write goes through and the cost
    // is recorded.
    let store = kelson_mysql::MySqlModelStore::new(pool.clone())
        .with_cost_guard(CostGuard::with_budget(CostBudget::writes_at_most(50)))
        .with_cost_ledger(Arc::clone(&ledger));
    store
        .update_filtered(&model(&table), Some(tenant), &filters, &changes)
        .expect("the write within the budget applies");
    let max_amount: i64 = sqlx::query_scalar(&format!("SELECT MAX(amount) FROM {table}"))
        .fetch_one(&pool)
        .await
        .expect("the amounts are there");
    assert_eq!(max_amount, 999, "the allowed write ran");

    let cost = ledger
        .lock()
        .expect("the ledger locks")
        .cost_of(&tenant)
        .expect("the tenant is known");
    assert_eq!(cost.statements, 1, "the applied write is recorded");
    assert_eq!(cost.rows, 10, "the rows it touched are recorded");

    sqlx::raw_sql(&format!("DROP TABLE {table}"))
        .execute(&pool)
        .await
        .expect("the probe table goes away");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_cost_the_ledger_aggregates_per_tenant() {
    let Some(pool) = pool().await else { return };
    let table = format!("kelson_cost_agg_{}", Uuid::now_v7().simple());
    sqlx::raw_sql(&format!(
        "CREATE TABLE {table} (id BINARY(16) NOT NULL PRIMARY KEY, tenant_id BINARY(16) NOT NULL, amount BIGINT NOT NULL, deleted_at DATETIME(6) NULL)"
    ))
    .execute(&pool)
    .await
    .expect("the probe table is created");

    let tenant = TenantId::new();
    let other = TenantId::new();
    for (t, count) in [(tenant, 3i64), (other, 2i64)] {
        for i in 0..count {
            sqlx::query(&format!(
                "INSERT INTO {table} (id, tenant_id, amount) VALUES (?, ?, ?)"
            ))
            .bind(Uuid::now_v7())
            .bind(*t.as_uuid())
            .bind(i)
            .execute(&pool)
            .await
            .expect("the probe row inserts");
        }
    }

    let ledger = Arc::new(Mutex::new(CostLedger::new()));
    let store =
        kelson_mysql::MySqlModelStore::new(pool.clone()).with_cost_ledger(Arc::clone(&ledger));

    let mut changes = Record::new();
    changes.insert("amount".into(), serde_json::json!(7));
    for t in [tenant, other] {
        let filters = vec![FieldFilter::eq(
            "tenant_id",
            serde_json::json!(t.as_uuid().to_string()),
        )];
        store
            .update_filtered(&model(&table), Some(t), &filters, &changes)
            .expect("the write applies");
    }

    let guard = ledger.lock().expect("the ledger locks");
    let first = guard.cost_of(&tenant).expect("the tenant is known");
    let second = guard.cost_of(&other).expect("the tenant is known");
    assert_eq!(first.rows, 3, "the first tenant's rows");
    assert_eq!(second.rows, 2, "the second tenant's rows");
    assert_eq!(guard.tenants(), 2, "the ledger knows both tenants");
    assert_eq!(guard.statements(), 2);

    sqlx::raw_sql(&format!("DROP TABLE {table}"))
        .execute(&pool)
        .await
        .expect("the probe table goes away");
}
