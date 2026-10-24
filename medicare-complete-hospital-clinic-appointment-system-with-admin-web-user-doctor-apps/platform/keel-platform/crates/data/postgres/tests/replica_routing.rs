//! **Read-replica routing proof** (F-9, the read/write split).
//!
//! A store may carry a second pool — the read replica — beside the primary.
//! Only the reads that explicitly opt in (`ListOpts::replica_ok`) are served
//! from it; every other read, and every write, stays on the primary. That
//! distinction is the whole point of the split: a browse list may tolerate
//! replication lag, a read that follows a write may not.
//!
//! The proof is **observable through the data**: the replica points at a
//! second schema that does NOT have the row the primary has. So
//!
//! - a list WITHOUT the opt-in sees the row (the primary served it), and
//! - a list WITH the opt-in does not (the replica served it).
//!
//! Skipped unless DATABASE_URL points at the fabric (the same graceful-skip
//! posture every other matrix here uses).
//!
//! Activated with `--features pg-tests`.

#![cfg(feature = "pg-tests")]

use kelson_foundation::TenantId;
use kelson_models::conformance::appointments_model;
use kelson_models::runtime::ModelRuntime;
use kelson_models::{FieldFilter, ListOpts, ModelStore};
use uuid::Uuid;

/// The row the primary has and the replica does not.
fn booking(patient: &str, slot: &str) -> kelson_models::Record {
    let mut record = kelson_models::Record::new();
    record.insert("patient".into(), serde_json::json!(patient));
    record.insert("slot".into(), serde_json::json!(slot));
    record.insert("status".into(), serde_json::json!("booked"));
    record.insert("version".into(), serde_json::json!(1));
    record
}

/// The mysql replica is a SECOND schema on the same server: `keel_fresh` is
/// a spare schema with its own grants, and nothing else in the repo uses it.
const MYSQL_REPLICA_DB: &str = "keel_fresh";

/// The pg replica is a second DATABASE: the pg superuser can create it, and
/// `CREATE DATABASE` is idempotent enough for a test (the duplicate is
/// ignored).
const PG_REPLICA_DB: &str = "keel_replica";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_a_replica_eligible_list_is_served_by_the_replica() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        return;
    };
    if !url.starts_with("mysql://") {
        return;
    }
    let Ok(primary) = kelson_mysql::create_pool(&url, 4).await else {
        eprintln!("mysql matrix: cannot connect; skipping");
        return;
    };
    kelson_mysql::run_migrations(&primary, std::path::Path::new("."))
        .await
        .expect("the primary migrations apply");

    // The replica: a second schema with the same schema, no data.
    let replica_url = url.replace("/keel", &format!("/{MYSQL_REPLICA_DB}"));
    let Ok(replica) = kelson_mysql::create_pool(&replica_url, 4).await else {
        eprintln!("mysql matrix: no replica schema; skipping");
        return;
    };
    kelson_mysql::run_migrations(&replica, std::path::Path::new("."))
        .await
        .expect("the replica migrations apply");

    let store = kelson_mysql::MySqlModelStore::new(primary.clone()).with_read_pool(replica.clone());
    let runtime = ModelRuntime::new(appointments_model());

    let tenant = TenantId::new();
    sqlx::query("INSERT INTO tenants (id, slug, name) VALUES (?, ?, ?)")
        .bind(tenant.as_uuid())
        .bind(format!("mysql-replica-{}", Uuid::now_v7()))
        .bind("MySQL Replica Tenant")
        .execute(&primary)
        .await
        .expect("the tenant seeds");

    let patient = format!("replica-probe-{}", Uuid::now_v7());
    runtime
        .insert(&store, &booking(&patient, "slot-1"), Some(tenant))
        .expect("the row lands on the PRIMARY");

    let filter = vec![FieldFilter::eq("patient", patient.clone())];

    // Without the opt-in the read stays on the primary: the row is there.
    let primary_read = store
        .list(runtime.model(), Some(tenant), &filter, &ListOpts::default())
        .expect("the primary read succeeds");
    assert_eq!(
        primary_read.items.len(),
        1,
        "a read without the opt-in is served by the primary"
    );

    // With the opt-in the read is served by the replica, which does not have
    // the row (it was never replicated here).
    let replica_read = store
        .list(
            runtime.model(),
            Some(tenant),
            &filter,
            &ListOpts {
                replica_ok: true,
                ..Default::default()
            },
        )
        .expect("the replica read succeeds");
    assert!(
        replica_read.items.is_empty(),
        "a replica-eligible read is served by the REPLICA (which has no such row)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pg_a_replica_eligible_list_is_served_by_the_replica() {
    let Ok(url) = std::env::var("DATABASE_URL") else {
        return;
    };
    if !url.starts_with("postgres://") {
        return;
    };
    let Ok(primary) = kelson_postgres::create_pool(&url, 4).await else {
        eprintln!("pg-tests: cannot connect; skipping");
        return;
    };
    kelson_postgres::run_migrations(&primary, std::path::Path::new("."))
        .await
        .expect("the primary migrations apply");

    // The replica: a second database. `CREATE DATABASE` cannot run inside a
    // transaction, and a duplicate is the normal case after the first run.
    let _ = sqlx::query(&format!("CREATE DATABASE {PG_REPLICA_DB}"))
        .execute(&primary)
        .await;
    let replica_url = match url.rfind('/') {
        Some(idx) => format!("{}/{PG_REPLICA_DB}", &url[..idx]),
        None => return,
    };
    let Ok(replica) = kelson_postgres::create_pool(&replica_url, 4).await else {
        eprintln!("pg-tests: no replica database; skipping");
        return;
    };
    kelson_postgres::run_migrations(&replica, std::path::Path::new("."))
        .await
        .expect("the replica migrations apply");

    let store = kelson_postgres::PgModelStore::new(primary.clone()).with_read_pool(replica.clone());
    let runtime = ModelRuntime::new(appointments_model());

    let tenant = TenantId::new();
    sqlx::query("INSERT INTO tenants (id, slug, name) VALUES ($1, $2, $3)")
        .bind(tenant.as_uuid())
        .bind(format!("pg-replica-{}", Uuid::now_v7()))
        .bind("Pg Replica Tenant")
        .execute(&primary)
        .await
        .expect("the tenant seeds");

    let patient = format!("replica-probe-{}", Uuid::now_v7());
    runtime
        .insert(&store, &booking(&patient, "slot-1"), Some(tenant))
        .expect("the row lands on the PRIMARY");

    let filter = vec![FieldFilter::eq("patient", patient.clone())];

    let primary_read = store
        .list(runtime.model(), Some(tenant), &filter, &ListOpts::default())
        .expect("the primary read succeeds");
    assert_eq!(
        primary_read.items.len(),
        1,
        "a read without the opt-in is served by the primary"
    );

    let replica_read = store
        .list(
            runtime.model(),
            Some(tenant),
            &filter,
            &ListOpts {
                replica_ok: true,
                ..Default::default()
            },
        )
        .expect("the replica read succeeds");
    assert!(
        replica_read.items.is_empty(),
        "a replica-eligible read is served by the REPLICA (which has no such row)"
    );
}
