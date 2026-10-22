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

use kelson_models::store::ModelStore;
use kelson_models::{FieldDef, FieldType, ModelDef, Record};
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_native_intersect_and_except_combine_two_queries() {
    let Some(pool) = pool().await else { return };
    let tenant = Uuid::now_v7().to_string();
    let other = Uuid::now_v7().to_string();
    seed(&pool, &tenant, None, "shared", 1, 1).await;
    seed(&pool, &tenant, None, "only-a", 2, 2).await;
    seed(&pool, &other, None, "shared", 3, 3).await;

    // The rows both tenants have.
    let q = Select::from_table(id("kelson_native_probe"))
        .select(vec![id("label")])
        .where_eq(id("tenant_id"), text(&tenant))
        .intersect(
            Select::from_table(id("kelson_native_probe"))
                .select(vec![id("label")])
                .where_eq(id("tenant_id"), text(&other)),
        )
        .build(&MYSQL)
        .expect("the intersect statement compiles");

    assert!(q.sql.contains("INTERSECT"), "{}", q.sql);
    let rows = fetch(&pool, &q).await.expect("the intersect query runs");
    assert_eq!(rows.len(), 1, "only the shared label is in both sides");
    assert_eq!(rows[0].get::<String, _>(0), "shared");

    // The rows the first tenant has that the second does not.
    let q = Select::from_table(id("kelson_native_probe"))
        .select(vec![id("label")])
        .where_eq(id("tenant_id"), text(&tenant))
        .except(
            Select::from_table(id("kelson_native_probe"))
                .select(vec![id("label")])
                .where_eq(id("tenant_id"), text(&other)),
        )
        .build(&MYSQL)
        .expect("the except statement compiles");

    assert!(q.sql.contains("EXCEPT"), "{}", q.sql);
    let rows = fetch(&pool, &q).await.expect("the except query runs");
    assert_eq!(rows.len(), 1, "only the tenant's own label remains");
    assert_eq!(rows[0].get::<String, _>(0), "only-a");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_native_explain_analyze_proves_the_index_is_used() {
    let Some(pool) = pool().await else { return };
    let tenant = Uuid::now_v7().to_string();
    for i in 0..8 {
        seed(&pool, &tenant, None, &format!("row-{i}"), i, i + 1).await;
    }

    // The same shape the model store issues: the tenant filter plus the
    // created_at ordering, which the (tenant_id, created_at) index covers.
    let q = Select::from_table(id("kelson_native_probe"))
        .select(vec![id("id"), id("amount")])
        .where_eq(id("tenant_id"), text(&tenant))
        .order_by(id("created_at"), Direction::Asc)
        .limit(5)
        .build(&MYSQL)
        .expect("the statement compiles");

    let plan = kelson_mysql::explain_analyze(&pool, &q.sql, &q.params)
        .await
        .expect("EXPLAIN ANALYZE runs");

    assert!(
        plan.uses_index("idx_native_tenant_created"),
        "the executed plan must use the index:\n{}",
        plan.text()
    );
    assert!(
        !plan.is_table_scan(),
        "the executed plan must not be a full table scan:\n{}",
        plan.text()
    );
}

/// The app-author case: a MySQL-native query the typed builder cannot
/// model, written through the VALIDATED raw escape and EXECUTED.
///
/// A reporting app needs to expand a JSON document into rows. MySQL's
/// `JSON_TABLE` does that; the typed builder has no concept of a derived
/// table, and before the raw escape the only way to write this query was
/// to bypass the framework entirely with a raw `sqlx::query` - losing
/// every guarantee the builder offers.
///
/// This case proves the whole path: the expression is validated, its
/// values are BOUND (not interpolated), and the server returns the
/// expanded rows.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_native_an_app_can_expand_a_json_document_with_json_table() {
    let Some(pool) = pool().await else { return };
    let table = format!("kelson_json_{}", Uuid::now_v7().simple());
    sqlx::raw_sql(&format!(
        "CREATE TABLE {table} (id CHAR(36) NOT NULL PRIMARY KEY, doc JSON NOT NULL)"
    ))
    .execute(&pool)
    .await
    .expect("the probe table is created");

    // The document is BOUND, not interpolated: the app passes a tenant's
    // JSON as a value.
    let document = serde_json::json!([
        { "sku": "a-1", "qty": 2 },
        { "sku": "b-2", "qty": 5 }
    ]);
    sqlx::query(&format!(
        "INSERT INTO {table} (id, doc) VALUES (?, ?)"
    ))
    .bind(Uuid::now_v7().to_string())
    .bind(document.to_string())
    .execute(&pool)
    .await
    .expect("the probe row inserts");

    // The expansion: JSON_TABLE over the bound document, as a derived table.
    let doc_expr = kelson_query::SqlExpr::new("doc", vec![]).expect("accepted");
    let expansion = kelson_query::SqlExpr::json_table(
        &doc_expr,
        "$[*]",
        "sku VARCHAR(32) PATH '$.sku', qty INT PATH '$.qty'",
        vec![],
    )
    .expect("the JSON_TABLE expression is accepted");

    let q = Select::from_table(id(&table))
        .from_expr(expansion, id("jt"))
        .select(vec![id("jt.sku"), id("jt.qty")])
        .order_by_expr(
            kelson_query::SqlExpr::raw("jt.sku").expect("accepted"),
            Direction::Asc,
        )
        .build(&MYSQL)
        .expect("the statement compiles");

    // The expansion is a derived table beside the table, and the ORDER BY
    // carries the raw expression.
    assert!(
        q.sql.contains("JSON_TABLE(doc , '$[*]' COLUMNS("),
        "the derived table must render: {}",
        q.sql
    );
    assert!(q.sql.contains(" AS jt"), "{}", q.sql);
    assert!(q.sql.contains("ORDER BY jt.sku ASC"), "{}", q.sql);

    let rows = fetch(&pool, &q).await.expect("the query runs");
    assert_eq!(rows.len(), 2, "the document expands into its items");
    let skus: Vec<String> = rows.iter().map(|r| r.get::<String, _>(0)).collect();
    let qtys: Vec<i64> = rows.iter().map(|r| r.get::<i64, _>(1)).collect();
    assert_eq!(skus, vec!["a-1".to_string(), "b-2".to_string()]);
    assert_eq!(qtys, vec![2, 5]);

    // A raw expression that tries to smuggle a statement is refused at
    // BUILD time - the app cannot write an injection through it.
    let refusal = kelson_query::SqlExpr::raw("1; DROP TABLE users").expect_err("refused");
    assert!(
        refusal.public_message.contains("statement separator"),
        "{}",
        refusal.public_message
    );

    sqlx::raw_sql(&format!("DROP TABLE {table}"))
        .execute(&pool)
        .await
        .expect("the probe table goes away");
}

/// The escape must not weaken the framework's central guarantee.
///
/// An app author who needs a query the typed builder cannot model hands the
/// statement to the STORE - and the STORE applies the tenant scope, not the
/// caller. If the caller applied the scope, the escape would be a way to
/// forget it, and "forgot the tenant scope" is the exact failure the
/// framework exists to make impossible.
///
/// The proof is deliberately blunt: the app's statement does **not** mention
/// the tenant at all. It filters on a column the app owns. The other
/// tenant's rows would match that filter - and the store still hides them,
/// because the scope is applied where the guarantee lives.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_native_the_store_scopes_a_raw_statement_the_app_built() {
    let Some(pool) = pool().await else { return };
    let table = format!("kelson_rawscope_{}", Uuid::now_v7().simple());
    sqlx::raw_sql(&format!(
        "CREATE TABLE {table} (id BINARY(16) NOT NULL PRIMARY KEY, tenant_id BINARY(16) NOT NULL, \
         amount BIGINT NOT NULL, deleted_at DATETIME(6) NULL)"
    ))
    .execute(&pool)
    .await
    .expect("the probe table is created");

    let tenant_a = kelson_foundation::TenantId::new();
    let tenant_b = kelson_foundation::TenantId::new();
    // Both tenants have rows ABOVE the filter's threshold, so the filter
    // alone would return both.
    for tenant in [tenant_a, tenant_b] {
        for i in 10..13i64 {
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
    }

    let model = ModelDef::new("RawScopeProbe", &table)
        .tenant_scoped()
        .field(FieldDef::new("id", FieldType::Uuid).required())
        .field(FieldDef::new("tenant_id", FieldType::Uuid).required())
        .field(FieldDef::new("amount", FieldType::Int64).required())
        .field(FieldDef::new("deleted_at", FieldType::Timestamp));

    let store = kelson_mysql::MySqlModelStore::new(pool.clone());

    // The app's custom statement: a filter the typed builder does not
    // model (a raw expression), and NO tenant predicate - the app is not
    // responsible for one.
    let filter = kelson_query::SqlExpr::raw("amount >= 10").expect("accepted");
    let statement = Select::from_table(id(&table))
        .where_expr(filter)
        .select(vec![id("id"), id("amount")]);

    // Tenant A sees ONLY its own rows. Both tenants hold three rows that
    // match the app's filter, so the count IS the proof: the filter alone
    // would have returned six.
    let rows = store
        .query_raw(&model, Some(tenant_a), statement.clone())
        .expect("the scoped raw query runs");
    assert_eq!(
        rows.len(),
        3,
        "the store returned only the caller's rows: {}",
        rows.len()
    );

    // Tenant B sees only its own rows - the same statement.
    let rows = store
        .query_raw(&model, Some(tenant_b), statement.clone())
        .expect("the scoped raw query runs");
    assert_eq!(rows.len(), 3);

    // A tenant-scoped model read with NO tenant matches nothing (the same
    // posture every other path has) rather than everything.
    let rows = store
        .query_raw(&model, None, statement.clone())
        .expect("the scoped raw query runs");
    assert!(
        rows.is_empty(),
        "a tenant-scoped model without a tenant must match nothing: {:?}",
        rows.len()
    );

    // A statement that reads a DIFFERENT table is refused: the scope cannot
    // land on the wrong relation.
    let foreign = Select::from_table(id("tenants")).select(vec![id("id")]);
    let refusal = store
        .query_raw(&model, Some(tenant_a), foreign)
        .expect_err("a statement over another table is refused");
    assert!(
        refusal.public_message.contains("cannot be applied"),
        "{}",
        refusal.public_message
    );

    // And the in-memory store refuses the escape outright rather than
    // pretending: it cannot evaluate SQL.
    let fake = kelson_models::conformance::appointments_model();
    let refusal = fake_store_refusal(&fake);
    assert!(
        refusal.contains("cannot evaluate"),
        "the in-memory store must say so: {refusal}"
    );

    sqlx::raw_sql(&format!("DROP TABLE {table}"))
        .execute(&pool)
        .await
        .expect("the probe table goes away");
}

/// The in-memory store's refusal (the trait's default implementation).
fn fake_store_refusal(model: &ModelDef) -> String {
    let fake = kelson_models::store::InMemoryModelStore::new();
    let statement = Select::from_table(id("anything")).select(vec![id("id")]);
    fake.query_raw(model, None, statement)
        .expect_err("the in-memory store refuses")
        .public_message
}
