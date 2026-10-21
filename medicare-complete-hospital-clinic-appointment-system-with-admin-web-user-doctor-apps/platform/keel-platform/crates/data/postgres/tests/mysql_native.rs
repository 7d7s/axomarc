//! The MySQL 8 native capability matrix.
//!
//! The features Laravel's query builder does **not** have — common table
//! expressions, window functions, `NOWAIT` and `SKIP LOCKED` — executed
//! against a live MySQL 8, not asserted on the SQL text. A rendered
//! statement that the server rejects is not a feature.
//!
//! Activated with `--features pg-tests`; every test is a graceful skip when
//! `DATABASE_URL` does not point at a MySQL instance, so a local run
//! without a database stays green and CI (the service container) enforces
//! them.

#![cfg(feature = "pg-tests")]

use kelson_query::{CastTarget, Direction, Ident, Select, Value, WindowFunction, WindowSpec, MYSQL};
use sqlx::{Column, Row};
use uuid::Uuid;

/// The scratch table the native cases use. Its shape is deliberate: a
/// tenant column and a timestamp, so a partition and an ordering have
/// something to work on, plus a self-reference for the recursive case.
const PROBE_DDL: &str = "CREATE TABLE IF NOT EXISTS kelson_native_probe (
    id CHAR(36) NOT NULL PRIMARY KEY,
    tenant_id CHAR(36) NOT NULL,
    parent_id CHAR(36) NULL,
    label VARCHAR(64) NOT NULL,
    amount BIGINT NOT NULL DEFAULT 0,
    created_at DATETIME(6) NOT NULL,
    KEY idx_native_tenant_created (tenant_id, created_at)
) ENGINE=InnoDB";

async fn pool() -> Option<kelson_mysql::MySqlPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(url) if url.starts_with("mysql://") => url,
        _ => {
            eprintln!("mysql-native: DATABASE_URL is not mysql://; skipping");
            return None;
        }
    };
    let pool = match kelson_mysql::create_pool(&url, 4).await {
        Ok(pool) => pool,
        Err(e) => {
            eprintln!("mysql-native: cannot connect ({e}); skipping");
            return None;
        }
    };
    if let Err(e) = sqlx::query(PROBE_DDL).execute(&pool).await {
        eprintln!("mysql-native: cannot create the probe table ({e}); skipping");
        return None;
    }
    Some(pool)
}

fn id(raw: &str) -> Ident {
    Ident::new(raw).expect("the identifier is valid")
}

fn text(v: &str) -> Value {
    Value::Text(v.to_string())
}

/// Bind the builder's values onto a sqlx query (the `?` placeholder form
/// the MySQL dialect renders).
fn bind<'q>(
    mut query: sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments>,
    params: &'q [Value],
) -> sqlx::query::Query<'q, sqlx::MySql, sqlx::mysql::MySqlArguments> {
    for p in params {
        query = match p {
            Value::Null => query.bind(None::<String>),
            Value::Bool(b) => query.bind(b),
            Value::Int(i) => query.bind(i),
            Value::Float(f) => query.bind(f),
            Value::Text(t) => query.bind(t),
            Value::Uuid(u) => query.bind(u),
            Value::Timestamp(ts) => query.bind(ts),
            Value::Json(j) => query.bind(j),
        };
    }
    query
}

/// Run a builder statement and return the rows.
async fn fetch(
    pool: &kelson_mysql::MySqlPool,
    q: &kelson_query::CompiledSelect,
) -> Result<Vec<sqlx::mysql::MySqlRow>, sqlx::Error> {
    assert_eq!(
        q.dialect,
        kelson_query::DialectId::MySql,
        "the statement must be rendered for MySQL"
    );
    bind(sqlx::query(&q.sql), &q.params)
        .fetch_all(pool)
        .await
}

/// A deterministic timestamp built from a sequence number (the cases must
/// not depend on the wall clock).
fn at(seq: i64) -> time::OffsetDateTime {
    time::OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(seq)
}

/// Insert one probe row.
async fn seed(
    pool: &kelson_mysql::MySqlPool,
    tenant: &str,
    parent: Option<&str>,
    label: &str,
    amount: i64,
    seq: i64,
) -> String {
    let row_id = Uuid::now_v7().to_string();
    sqlx::query(
        "INSERT INTO kelson_native_probe (id, tenant_id, parent_id, label, amount, created_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&row_id)
    .bind(tenant)
    .bind(parent)
    .bind(label)
    .bind(amount)
    .bind(at(seq))
    .execute(pool)
    .await
    .expect("the probe row inserts");
    row_id
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_native_a_cte_projects_through_a_common_table_expression() {
    let Some(pool) = pool().await else { return };
    let tenant = Uuid::now_v7().to_string();
    seed(&pool, &tenant, None, "a", 10, 1).await;
    seed(&pool, &tenant, None, "b", 20, 2).await;

    let q = Select::from_table(id("paid_orders"))
        .with_cte("paid_orders", |cte| {
            cte.table(id("kelson_native_probe"))
                .select(vec![id("label"), id("amount")])
                .where_eq(id("tenant_id"), text(&tenant))
        })
        .select(vec![id("label"), id("amount")])
        .build(&MYSQL)
        .expect("the CTE statement compiles");

    assert!(
        q.sql.starts_with("WITH paid_orders AS (SELECT label, amount FROM"),
        "the CTE must lead the statement: {}",
        q.sql
    );
    let rows = fetch(&pool, &q).await.expect("the CTE query runs");
    assert_eq!(rows.len(), 2, "both probe rows are visible through the CTE");
    let total: i64 = rows.iter().map(|r| r.get::<i64, _>(1)).sum();
    assert_eq!(total, 30);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_native_a_recursive_cte_walks_a_hierarchy() {
    let Some(pool) = pool().await else { return };
    let tenant = Uuid::now_v7().to_string();
    let root = seed(&pool, &tenant, None, "root", 1, 1).await;
    let child = seed(&pool, &tenant, Some(&root), "child", 2, 2).await;
    seed(&pool, &tenant, Some(&child), "grandchild", 3, 3).await;

    // MySQL's recursive member must reference the CTE in its FROM clause
    // (the base table JOINed to the CTE) — a reference in the WHERE clause
    // alone is rejected with "Unknown column".
    let q = Select::from_table(id("tree"))
        .with_recursive_cte(
            "tree",
            vec![id("id"), id("parent_id"), id("label")],
            |cte| {
                cte.table(id("kelson_native_probe"))
                    .select(vec![
                        id("kelson_native_probe.id"),
                        id("kelson_native_probe.parent_id"),
                        id("kelson_native_probe.label"),
                    ])
                    .where_eq(id("tenant_id"), text(&tenant))
                    .where_null(id("parent_id"))
                    .union_all(
                        Select::from_table(id("kelson_native_probe"))
                            .select(vec![
                                id("kelson_native_probe.id"),
                                id("kelson_native_probe.parent_id"),
                                id("kelson_native_probe.label"),
                            ])
                            .where_eq(id("tenant_id"), text(&tenant))
                            .join_inner(
                                id("tree"),
                                id("kelson_native_probe.parent_id"),
                                id("tree.id"),
                            ),
                    )
            },
        )
        .select(vec![id("label")])
        .build(&MYSQL)
        .expect("the recursive CTE statement compiles");

    assert!(q.sql.starts_with("WITH RECURSIVE tree ("), "{}", q.sql);
    let rows = fetch(&pool, &q).await.expect("the recursive CTE runs");
    assert_eq!(
        rows.len(),
        3,
        "the hierarchy walk must reach every level: {:?}",
        q.sql
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_native_a_window_function_sums_per_partition() {
    let Some(pool) = pool().await else { return };
    let tenant = Uuid::now_v7().to_string();
    seed(&pool, &tenant, None, "a", 10, 1).await;
    seed(&pool, &tenant, None, "b", 20, 2).await;
    seed(&pool, &tenant, None, "c", 30, 3).await;

    let q = Select::from_table(id("kelson_native_probe"))
        .select(vec![id("label"), id("amount")])
        .window(
            WindowSpec::new(WindowFunction::Sum(id("amount")))
                .partition_by(vec![id("tenant_id")])
                .order_by(vec![(id("created_at"), Direction::Asc)])
                .cast(CastTarget::BigInt),
        )
        .where_eq(id("tenant_id"), text(&tenant))
        .build(&MYSQL)
        .expect("the window statement compiles");

    assert!(
        q.sql.contains("SUM(amount) OVER (PARTITION BY tenant_id ORDER BY created_at ASC)"),
        "{}",
        q.sql
    );
    assert!(
        q.sql.contains("CAST(SUM(amount) OVER (") && q.sql.contains("AS SIGNED)"),
        "the cast is dialect-rendered: {}",
        q.sql
    );
    let rows = fetch(&pool, &q).await.expect("the window query runs");
    assert_eq!(rows.len(), 3);
    // The running sum over the ordered partition: 10, 30, 60.
    let running: Vec<i64> = rows.iter().map(|r| r.get::<i64, _>(2)).collect();
    assert_eq!(running, vec![10, 30, 60]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_native_a_window_ranking_numbers_the_partition() {
    let Some(pool) = pool().await else { return };
    let tenant = Uuid::now_v7().to_string();
    seed(&pool, &tenant, None, "a", 10, 1).await;
    seed(&pool, &tenant, None, "b", 20, 2).await;
    seed(&pool, &tenant, None, "c", 30, 3).await;

    let q = Select::from_table(id("kelson_native_probe"))
        .select(vec![id("label")])
        .window(
            WindowSpec::new(WindowFunction::RowNumber)
                .partition_by(vec![id("tenant_id")])
                .order_by(vec![(id("amount"), Direction::Desc)])
                .cast(CastTarget::BigInt),
        )
        .where_eq(id("tenant_id"), text(&tenant))
        .build(&MYSQL)
        .expect("the ranking statement compiles");

    assert!(
        q.sql.contains("ROW_NUMBER() OVER (PARTITION BY tenant_id"),
        "{}",
        q.sql
    );
    let rows = fetch(&pool, &q).await.expect("the ranking query runs");
    assert_eq!(rows.len(), 3);
    // Ordered by amount descending: c(30)=1, b(20)=2, a(10)=3.
    let numbers: Vec<i64> = rows.iter().map(|r| r.get::<i64, _>(1)).collect();
    assert_eq!(numbers, vec![1, 2, 3]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_native_skip_locked_skips_a_row_another_connection_holds() {
    let Some(pool) = pool().await else { return };
    let tenant = Uuid::now_v7().to_string();
    let held = seed(&pool, &tenant, None, "held", 1, 1).await;
    let free = seed(&pool, &tenant, None, "free", 2, 2).await;

    // The lock must be HELD: in autocommit mode a FOR UPDATE releases the
    // row lock the moment the statement ends, and nothing would be
    // contended. An explicit transaction holds it.
    let mut tx = pool.begin().await.expect("the transaction begins");
    sqlx::query("SELECT id FROM kelson_native_probe WHERE id = ? FOR UPDATE")
        .bind(&held)
        .fetch_one(&mut *tx)
        .await
        .expect("the blocker locks its row");

    // Another connection asks for the same rows with SKIP LOCKED: it must
    // get the UNLOCKED row and skip the locked one, without waiting.
    let q = Select::from_table(id("kelson_native_probe"))
        .select(vec![id("id")])
        .where_eq(id("tenant_id"), text(&tenant))
        .lock_for_update_skip_locked()
        .build(&MYSQL)
        .expect("the skip-locked statement compiles");

    assert!(q.sql.ends_with("FOR UPDATE SKIP LOCKED"), "{}", q.sql);
    let rows = fetch(&pool, &q).await.expect("the skip-locked query runs");
    let got: Vec<String> = rows.iter().map(|r| r.get::<String, _>(0)).collect();
    assert!(
        got.contains(&free) && !got.contains(&held),
        "SKIP LOCKED must return the free row and skip the locked one: {got:?}"
    );

    tx.rollback().await.expect("the transaction rolls back");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_native_nowait_fails_fast_instead_of_waiting() {
    let Some(pool) = pool().await else { return };
    let tenant = Uuid::now_v7().to_string();
    let held = seed(&pool, &tenant, None, "held", 1, 1).await;

    let mut tx = pool.begin().await.expect("the transaction begins");
    sqlx::query("SELECT id FROM kelson_native_probe WHERE id = ? FOR UPDATE")
        .bind(&held)
        .fetch_one(&mut *tx)
        .await
        .expect("the blocker locks its row");

    let q = Select::from_table(id("kelson_native_probe"))
        .select(vec![id("id")])
        .where_eq(id("id"), text(&held))
        .lock_for_update_nowait()
        .build(&MYSQL)
        .expect("the nowait statement compiles");

    assert!(q.sql.ends_with("FOR UPDATE NOWAIT"), "{}", q.sql);
    let outcome = fetch(&pool, &q).await;
    assert!(
        outcome.is_err(),
        "NOWAIT must fail fast on a contended row, not wait: {outcome:?}"
    );

    tx.rollback().await.expect("the transaction rolls back");
}
