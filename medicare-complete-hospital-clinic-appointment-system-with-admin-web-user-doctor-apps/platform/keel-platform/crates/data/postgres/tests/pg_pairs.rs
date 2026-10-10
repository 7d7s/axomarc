//! Wave 5 pair-migration round trip: apply with batches, rollback, reset —
//! exercised against a live database. Activated with `--features pg-tests`;
//! every test is a graceful skip when DATABASE_URL is unset, so local runs
//! without a database stay green (same contract as `pg_integration`).

#![cfg(feature = "pg-tests")]

use kelson_postgres::pairs::{
    apply_pending, applied, install, pairs_dir, reset, rollback, PAIRS_TABLE,
};
use kelson_postgres::{
    acquire_migration_lock_within, create_pool, release_migration_lock, MIGRATION_LOCK_KEY, PgPool,
};
use std::time::Duration;

async fn pool() -> Option<PgPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(url) if !url.is_empty() => url,
        _ => {
            eprintln!("pg-tests: DATABASE_URL not set; skipping pair round trip");
            return None;
        }
    };
    match create_pool(&url, 4).await {
        Ok(pool) => Some(pool),
        Err(e) => {
            eprintln!("pg-tests: cannot connect ({e}); skipping pair round trip");
            None
        }
    }
}

fn temp_root(tag: &str) -> std::path::PathBuf {
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "kelson-pg-pairs-{tag}-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    ));
    std::fs::create_dir_all(pairs_dir(&dir)).unwrap();
    dir
}

fn write_pair(root: &std::path::Path, base: &str, table: &str) {
    let dir = pairs_dir(root);
    std::fs::write(
        dir.join(format!("{base}.up.sql")),
        format!("CREATE TABLE {table} (id int PRIMARY KEY);"),
    )
    .unwrap();
    std::fs::write(
        dir.join(format!("{base}.down.sql")),
        format!("DROP TABLE {table};"),
    )
    .unwrap();
}

async fn table_exists(pool: &PgPool, table: &str) -> bool {
    let reg: Option<String> =
        sqlx::query_scalar(&format!("SELECT to_regclass('{table}')::text"))
            .fetch_one(pool)
            .await
            .expect("to_regclass runs");
    reg.is_some()
}

/// Install (idempotent, runs no files) -> apply pairs in one batch -> a new
/// pair lands in a later batch -> `rollback --step 1` reverts only that
/// batch in reverse filename order -> full rollback -> reset.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pair_migrations_apply_rollback_and_reset_with_batches() {
    let Some(pool) = pool().await else {
        return;
    };
    let root = temp_root("roundtrip");
    let alpha = "kelson_w5_pairs_alpha";
    let beta = "kelson_w5_pairs_beta";
    let gamma = "kelson_w5_pairs_gamma";

    // Idempotent across re-runs: clear this test's leftovers first (the
    // bookkeeping table is created before it can be cleaned).
    sqlx::query(&format!("DROP TABLE IF EXISTS {alpha}"))
        .execute(&pool)
        .await
        .expect("cleanup alpha");
    sqlx::query(&format!("DROP TABLE IF EXISTS {beta}"))
        .execute(&pool)
        .await
        .expect("cleanup beta");
    sqlx::query(&format!("DROP TABLE IF EXISTS {gamma}"))
        .execute(&pool)
        .await
        .expect("cleanup gamma");
    install(&pool).await.expect("install");
    sqlx::query(&format!(
        "DELETE FROM {PAIRS_TABLE} WHERE migration LIKE '2026093000000%'"
    ))
    .execute(&pool)
    .await
    .expect("cleanup bookkeeping");

    // migrate:install creates the bookkeeping table; running it again is
    // harmless and runs no migration files.
    install(&pool).await.expect("install idempotent");
    assert!(applied(&pool).await.expect("applied rows").is_empty());

    // Two pairs apply together in one batch (filename order).
    write_pair(&root, "20260930000001_alpha", alpha);
    write_pair(&root, "20260930000002_beta", beta);
    let applied_now = apply_pending(&pool, &root).await.expect("apply");
    assert_eq!(
        applied_now,
        [
            "20260930000001_alpha",
            "20260930000002_beta"
        ],
        "filename order"
    );
    assert!(table_exists(&pool, alpha).await, "alpha table created");
    assert!(table_exists(&pool, beta).await, "beta table created");
    let rows = applied(&pool).await.expect("applied rows");
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|(_, batch)| *batch == 1), "single batch: {rows:?}");

    // Second run: nothing pending (idempotent).
    assert!(apply_pending(&pool, &root).await.expect("re-apply").is_empty());

    // A later pair lands in the next batch.
    write_pair(&root, "20260930000003_gamma", gamma);
    let applied_now = apply_pending(&pool, &root).await.expect("apply gamma");
    assert_eq!(applied_now, ["20260930000003_gamma"]);
    let rows = applied(&pool).await.expect("applied rows");
    assert!(
        rows.iter().any(|(m, b)| m == "20260930000003_gamma" && *b == 2),
        "gamma is batch 2: {rows:?}"
    );

    // rollback --step 1: only the newest batch (gamma), reverse filename.
    let reverted = rollback(&pool, &root, 1).await.expect("rollback step 1");
    assert_eq!(reverted, ["20260930000003_gamma"]);
    assert!(!table_exists(&pool, gamma).await, "gamma table dropped");
    assert!(table_exists(&pool, alpha).await, "older batch untouched");
    assert_eq!(applied(&pool).await.expect("applied rows").len(), 2);

    // rollback --step 1 again: batch 1 reverts beta before alpha (reverse
    // filename order), and both tables disappear.
    let reverted = rollback(&pool, &root, 1).await.expect("rollback batch 1");
    assert_eq!(
        reverted,
        ["20260930000002_beta", "20260930000001_alpha"]
    );
    assert!(!table_exists(&pool, alpha).await, "alpha dropped");
    assert!(!table_exists(&pool, beta).await, "beta dropped");
    assert!(applied(&pool).await.expect("applied rows").is_empty());

    // Re-apply, then reset reverts the whole history in reverse order.
    let applied_now = apply_pending(&pool, &root).await.expect("re-apply");
    assert_eq!(applied_now.len(), 3, "all three pending again");
    let reverted = reset(&pool, &root).await.expect("reset");
    assert_eq!(
        reverted,
        [
            "20260930000003_gamma",
            "20260930000002_beta",
            "20260930000001_alpha"
        ],
        "full reverse walk"
    );
    assert!(applied(&pool).await.expect("applied rows").is_empty());
    assert!(!table_exists(&pool, alpha).await);
    assert!(!table_exists(&pool, beta).await);
    assert!(!table_exists(&pool, gamma).await);

    std::fs::remove_dir_all(&root).ok();
}

/// The advisory migration lock serializes sessions: a held lock makes a
/// second acquisition fail closed at its deadline (instead of hanging), and
/// freeing it lets the waiter through — the `pg_advisory_lock(7259)`
/// contract from docs/19 Wave 5.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn advisory_lock_serializes_runs_and_fails_closed_on_timeout() {
    let Some(pool) = pool().await else {
        return;
    };

    // Session A takes the lock.
    let mut holder = pool.acquire().await.expect("holder connection");
    let got: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
        .bind(MIGRATION_LOCK_KEY)
        .fetch_one(&mut *holder)
        .await
        .expect("try lock");
    assert!(got, "lock is free at the start of the test");

    // Session B: fail-closed within a short deadline instead of hanging.
    let mut waiter = pool.acquire().await.expect("waiter connection");
    let err = acquire_migration_lock_within(
        &mut waiter,
        Duration::from_millis(250),
        Duration::from_millis(50),
    )
    .await
    .expect_err("held lock times out fail-closed");
    let detail = err.internal.clone().unwrap_or_default();
    assert!(detail.contains(&MIGRATION_LOCK_KEY.to_string()), "{detail}");
    assert!(detail.contains("fail-closed"), "{detail}");

    // Holder releases; the waiter acquires and releases cleanly.
    let released: bool = sqlx::query_scalar("SELECT pg_advisory_unlock($1)")
        .bind(MIGRATION_LOCK_KEY)
        .fetch_one(&mut *holder)
        .await
        .expect("holder unlock");
    assert!(released, "holder owned the lock");
    acquire_migration_lock_within(&mut waiter, Duration::from_secs(2), Duration::from_millis(50))
        .await
        .expect("waiter acquires once the lock is free");
    release_migration_lock(&mut waiter)
        .await
        .expect("waiter release");
}

// ---------------------------------------------------- mysql pair round trip
// B-31 Phase C: the same `pairs` machinery over the second fabric — one
// `database/migrations-pairs` directory, one `keel_migrations` table, one
// rollback order; only execution differs (MySQL DDL auto-commits, so a pair
// runs statement by statement and reports what it already did). Each live
// test skips unless DATABASE_URL points at MySQL; the pg half above skips
// symmetrically when it is set.

async fn mysql_pool() -> Option<kelson_mysql::MySqlPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(url) if !url.is_empty() => url,
        _ => {
            eprintln!("mysql pairs: DATABASE_URL not set; skipping");
            return None;
        }
    };
    if !url.starts_with("mysql://") {
        eprintln!("mysql pairs: DATABASE_URL is not mysql://; skipping");
        return None;
    }
    match kelson_mysql::create_pool(&url, 4).await {
        Ok(pool) => Some(pool),
        Err(e) => {
            eprintln!("mysql pairs: cannot connect ({e}); skipping");
            None
        }
    }
}

/// Raw up/down pair files for shipment-shape tests (the shared `write_pair`
/// derives both sides from a table name).
fn write_pair_files(root: &std::path::Path, base: &str, up: &str, down: &str) {
    let dir = pairs_dir(root);
    std::fs::write(dir.join(format!("{base}.up.sql")), up).unwrap();
    std::fs::write(dir.join(format!("{base}.down.sql")), down).unwrap();
}

async fn mysql_table_exists(pool: &kelson_mysql::MySqlPool, table: &str) -> bool {
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables
         WHERE table_schema = DATABASE() AND table_name = ?",
    )
    .bind(table)
    .fetch_one(pool)
    .await
    .expect("information_schema is readable");
    rows > 0
}

/// `keel_migrations` is one table per database and batch numbers are global
/// arithmetic, so two pair round trips running at once could roll back each
/// other's rows (the newest batch would not be "theirs"). The live MySQL
/// pair tests therefore serialize on this lock for their whole body; the pg
/// suite needs no such thing because it ships a single pair test. A poisoned
/// lock means an earlier test panicked mid-round-trip — its leftovers are
/// cleaned by the next run's own pre-cleanup, so ignore the poison.
static MYSQL_PAIR_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn mysql_pair_guard() -> std::sync::MutexGuard<'static, ()> {
    MYSQL_PAIR_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Apply with batches -> rollback the newest batch -> the rest in reverse
/// filename order -> reset: the pg round trip, byte-for-byte, over MySQL,
/// with every bookkeeping assertion scoped to THIS test's migration prefix
/// (rows from other suites share the table).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_pair_migrations_apply_rollback_and_reset_with_batches() {
    let _guard = mysql_pair_guard();
    let Some(pool) = mysql_pool().await else {
        return;
    };
    let root = temp_root("mysql-roundtrip");
    let alpha = "kelson_c_pairs_alpha";
    let beta = "kelson_c_pairs_beta";
    let gamma = "kelson_c_pairs_gamma";
    const PREFIX: &str = "2026093000000";
    let own = |rows: &[(String, i32)]| -> Vec<(String, i32)> {
        rows.iter()
            .filter(|(m, _)| m.starts_with(PREFIX))
            .cloned()
            .collect()
    };

    // Idempotent across re-runs: clear this test's leftovers first.
    for table in [alpha, beta, gamma] {
        sqlx::query(&format!("DROP TABLE IF EXISTS {table}"))
            .execute(&pool)
            .await
            .expect("cleanup table");
    }
    kelson_mysql::pairs::install(&pool).await.expect("install");
    sqlx::query(&format!(
        "DELETE FROM {} WHERE migration LIKE '2026093000000%'
            OR migration LIKE '2026093100000%' OR migration LIKE '2026093200000%'",
        kelson_mysql::pairs::PAIRS_TABLE
    ))
    .execute(&pool)
    .await
    .expect("cleanup bookkeeping");

    // migrate:install creates the bookkeeping table; running it again is
    // harmless and runs no migration files.
    kelson_mysql::pairs::install(&pool)
        .await
        .expect("install idempotent");
    assert!(own(&kelson_mysql::pairs::applied(&pool).await.expect("applied")).is_empty());

    // Two pairs apply together in one batch (filename order).
    write_pair(&root, "20260930000001_alpha", alpha);
    write_pair(&root, "20260930000002_beta", beta);
    let applied_now = kelson_mysql::pairs::apply_pending(&pool, &root)
        .await
        .expect("apply");
    assert_eq!(
        applied_now,
        ["20260930000001_alpha", "20260930000002_beta"],
        "filename order"
    );
    assert!(mysql_table_exists(&pool, alpha).await, "alpha table created");
    assert!(mysql_table_exists(&pool, beta).await, "beta table created");
    let rows = own(&kelson_mysql::pairs::applied(&pool).await.expect("applied"));
    assert_eq!(rows.len(), 2, "two rows recorded: {rows:?}");
    assert!(
        rows.iter().all(|(_, batch)| *batch == rows[0].1),
        "one batch for the whole run: {rows:?}"
    );
    let first_batch = rows[0].1;

    // Second run: nothing pending (idempotent).
    assert!(
        kelson_mysql::pairs::apply_pending(&pool, &root)
            .await
            .expect("re-apply")
            .is_empty()
    );

    // A later pair lands in the next batch.
    write_pair(&root, "20260930000003_gamma", gamma);
    let applied_now = kelson_mysql::pairs::apply_pending(&pool, &root)
        .await
        .expect("apply gamma");
    assert_eq!(applied_now, ["20260930000003_gamma"]);
    let rows = own(&kelson_mysql::pairs::applied(&pool).await.expect("applied"));
    assert!(
        rows.iter()
            .any(|(m, b)| m == "20260930000003_gamma" && *b == first_batch + 1),
        "gamma lands in the batch after the first run ({first_batch} + 1): {rows:?}"
    );

    // rollback --step 1: only the newest batch (gamma), reverse filename.
    let reverted = kelson_mysql::pairs::rollback(&pool, &root, 1)
        .await
        .expect("rollback step 1");
    assert_eq!(reverted, ["20260930000003_gamma"]);
    assert!(!mysql_table_exists(&pool, gamma).await, "gamma table dropped");
    assert!(mysql_table_exists(&pool, alpha).await, "older batch untouched");
    assert_eq!(own(&kelson_mysql::pairs::applied(&pool).await.expect("applied")).len(), 2);

    // rollback --step 1 again: batch 1 reverts beta before alpha (reverse
    // filename order), and both tables disappear.
    let reverted = kelson_mysql::pairs::rollback(&pool, &root, 1)
        .await
        .expect("rollback batch 1");
    assert_eq!(
        reverted,
        ["20260930000002_beta", "20260930000001_alpha"]
    );
    assert!(!mysql_table_exists(&pool, alpha).await, "alpha dropped");
    assert!(!mysql_table_exists(&pool, beta).await, "beta dropped");
    assert!(own(&kelson_mysql::pairs::applied(&pool).await.expect("applied")).is_empty());

    // Re-apply, then reset reverts the whole history in reverse order.
    let applied_now = kelson_mysql::pairs::apply_pending(&pool, &root)
        .await
        .expect("re-apply");
    assert_eq!(applied_now.len(), 3, "all three pending again");
    let reverted = kelson_mysql::pairs::reset(&pool, &root).await.expect("reset");
    assert_eq!(
        reverted,
        [
            "20260930000003_gamma",
            "20260930000002_beta",
            "20260930000001_alpha"
        ],
        "full reverse walk"
    );
    assert!(own(&kelson_mysql::pairs::applied(&pool).await.expect("applied")).is_empty());
    assert!(!mysql_table_exists(&pool, alpha).await);
    assert!(!mysql_table_exists(&pool, beta).await);
    assert!(!mysql_table_exists(&pool, gamma).await);

    std::fs::remove_dir_all(&root).ok();
}

/// The MySQL scanner is the Postgres scanner: filename order, README and
/// stray singles ignored, and a broken shipment refused before any DDL runs.
/// Pure filesystem work, so it runs without a database.
#[test]
fn mysql_pair_scan_orders_files_and_refuses_broken_shipments() {
    let root = temp_root("mysql-scan");
    // Written out of order to prove sorting is by filename.
    write_pair_files(
        &root,
        "20260902000001_beta",
        "CREATE TABLE beta (id int);",
        "DROP TABLE beta;",
    );
    write_pair_files(
        &root,
        "20260901000001_alpha",
        "CREATE TABLE alpha (id int);",
        "DROP TABLE alpha;",
    );
    std::fs::write(pairs_dir(&root).join("README.md"), "docs").unwrap();
    std::fs::write(pairs_dir(&root).join("stray.sql"), "SELECT 1;").unwrap();

    let scanned = kelson_mysql::pairs::scan_pairs(&root).expect("scan runs");
    let names: Vec<&str> = scanned.iter().map(|p| p.migration.as_str()).collect();
    assert_eq!(
        names,
        ["20260901000001_alpha", "20260902000001_beta"],
        "filename order, non-pair files ignored"
    );
    assert_eq!(scanned[0].down_sql, "DROP TABLE alpha;");

    // Missing down file -> refused (pair cannot roll back).
    std::fs::write(
        pairs_dir(&root).join("20260903000001_gamma.up.sql"),
        "CREATE TABLE gamma (id int);",
    )
    .unwrap();
    let err = kelson_mysql::pairs::scan_pairs(&root).expect_err("missing down refuses the scan");
    let detail = err.internal.clone().unwrap_or_else(|| err.to_string());
    assert!(detail.contains("20260903000001_gamma"), "{detail}");

    // Complete the pair; add an orphan down file -> refused.
    std::fs::write(
        pairs_dir(&root).join("20260903000001_gamma.down.sql"),
        "DROP TABLE gamma;",
    )
    .unwrap();
    std::fs::write(
        pairs_dir(&root).join("20260904000001_delta.down.sql"),
        "DROP TABLE delta;",
    )
    .unwrap();
    let err = kelson_mysql::pairs::scan_pairs(&root).expect_err("orphan down refuses the scan");
    let detail = err.internal.clone().unwrap_or_else(|| err.to_string());
    assert!(detail.contains("orphan"), "{detail}");

    // Bad base name (no numeric <ts>) -> refused even with both files.
    std::fs::remove_file(pairs_dir(&root).join("20260904000001_delta.down.sql")).unwrap();
    write_pair_files(&root, "no_timestamp_pair", "SELECT 1;", "SELECT 1;");
    let err = kelson_mysql::pairs::scan_pairs(&root).expect_err("bad base name refuses the scan");
    let detail = err.internal.clone().unwrap_or_else(|| err.to_string());
    assert!(detail.contains("numeric"), "{detail}");
    std::fs::remove_dir_all(&root).ok();
}

/// The engine cannot roll MySQL DDL back, so the runner must not pretend it
/// can: a `.up.sql` whose later statement fails stops the run, names the
/// pair, states that earlier statements remain applied, keeps pair 1, and
/// never records pair 2. That last bit is the fail-closed half — an
/// unrecorded pair re-applies next run and fails visibly rather than being
/// silently considered done.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_pair_apply_fails_stopped_and_reports_what_already_ran() {
    let _guard = mysql_pair_guard();
    let Some(pool) = mysql_pool().await else {
        return;
    };
    let root = temp_root("mysql-failstop");
    const PREFIX: &str = "2026093100000";
    let own = |rows: &[(String, i32)]| -> Vec<(String, i32)> {
        rows.iter()
            .filter(|(m, _)| m.starts_with(PREFIX))
            .cloned()
            .collect()
    };

    for table in ["kelson_c_pairs_ok", "kelson_c_pairs_partial"] {
        sqlx::query(&format!("DROP TABLE IF EXISTS {table}"))
            .execute(&pool)
            .await
            .expect("cleanup table");
    }
    kelson_mysql::pairs::install(&pool).await.expect("install");
    sqlx::query(&format!(
        "DELETE FROM {} WHERE migration LIKE '2026093000000%'
            OR migration LIKE '2026093100000%' OR migration LIKE '2026093200000%'",
        kelson_mysql::pairs::PAIRS_TABLE
    ))
    .execute(&pool)
    .await
    .expect("cleanup bookkeeping");

    // Pair 1 is healthy; pair 2's FIRST statement succeeds and its second
    // cannot possibly succeed.
    write_pair(&root, "20260931000001_ok", "kelson_c_pairs_ok");
    std::fs::write(
        pairs_dir(&root).join("20260931000002_broken.up.sql"),
        "CREATE TABLE kelson_c_pairs_partial (id int PRIMARY KEY);\n\
         DROP TABLE kelson_c_pairs_definitely_absent;",
    )
    .unwrap();
    std::fs::write(
        pairs_dir(&root).join("20260931000002_broken.down.sql"),
        "DROP TABLE kelson_c_pairs_partial;",
    )
    .unwrap();

    let err = kelson_mysql::pairs::apply_pending(&pool, &root)
        .await
        .expect_err("the broken pair stops the run");
    let detail = err.internal.clone().unwrap_or_else(|| err.to_string());
    assert!(detail.contains("20260931000002_broken"), "{detail}");
    assert!(detail.contains("(up)"), "{detail}");
    assert!(
        detail.contains("remain applied"),
        "the report must say what the engine did not undo: {detail}"
    );

    // Fail-stop: pair 1 applied and recorded, pair 2 not recorded.
    assert!(
        mysql_table_exists(&pool, "kelson_c_pairs_ok").await,
        "the healthy pair before it applied"
    );
    let rows = own(&kelson_mysql::pairs::applied(&pool).await.expect("applied"));
    assert!(
        rows.iter().any(|(m, _)| m == "20260931000001_ok"),
        "first pair recorded: {rows:?}"
    );
    assert!(
        !rows.iter().any(|(m, _)| m == "20260931000002_broken"),
        "failed pair never recorded: {rows:?}"
    );

    // The non-transactional truth, asserted rather than papered over.
    assert!(
        mysql_table_exists(&pool, "kelson_c_pairs_partial").await,
        "MySQL DDL before the failure remains applied — that is the documented contract"
    );

    // And the repair path: roll the healthy pair back, drop the partial one.
    let reverted = kelson_mysql::pairs::rollback(&pool, &root, 1)
        .await
        .expect("rollback the healthy batch");
    assert_eq!(reverted, ["20260931000001_ok"]);
    assert!(!mysql_table_exists(&pool, "kelson_c_pairs_ok").await);
    sqlx::query("DROP TABLE IF EXISTS kelson_c_pairs_partial")
        .execute(&pool)
        .await
        .expect("drop partial");
    std::fs::remove_dir_all(&root).ok();
}

/// `kelson migrate` / `kelson migrate:rollback` wiring end to end over MySQL:
/// a broken shipment refuses the run BEFORE any DDL (the old
/// `refuse_shipped_pairs` fail-closed contract, now enforced by the shared
/// scanner because pairs actually apply), a valid pair applies through
/// `run_migrations` and is returned by name, a second run is a no-op, and
/// `pairs::rollback` — the entry point behind `migrate:rollback` — takes
/// the table back off the schema and the row out of the bookkeeping.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_run_migrations_refuses_broken_shipments_then_applies_and_rolls_back() {
    let _guard = mysql_pair_guard();
    let Some(pool) = mysql_pool().await else {
        return;
    };
    let table = "kelson_c_pairs_applied";

    sqlx::query(&format!("DROP TABLE IF EXISTS {table}"))
        .execute(&pool)
        .await
        .expect("cleanup table");
    kelson_mysql::pairs::install(&pool).await.expect("install");
    sqlx::query(&format!(
        "DELETE FROM {} WHERE migration LIKE '2026093000000%'
            OR migration LIKE '2026093100000%' OR migration LIKE '2026093200000%'",
        kelson_mysql::pairs::PAIRS_TABLE
    ))
    .execute(&pool)
    .await
    .expect("cleanup bookkeeping");

    // (a) Broken shipment: an orphan down file refuses the whole run, and
    // nothing at all was executed.
    let broken = temp_root("mysql-broken");
    std::fs::write(
        pairs_dir(&broken).join("20260932000001_orphan.down.sql"),
        format!("DROP TABLE {table};"),
    )
    .unwrap();
    let err = kelson_mysql::run_migrations(&pool, &broken)
        .await
        .expect_err("an orphan .down.sql refuses the run");
    let detail = err.internal.clone().unwrap_or_else(|| err.to_string());
    assert!(detail.contains("orphan"), "{detail}");
    assert!(
        !mysql_table_exists(&pool, table).await,
        "refused before the first statement"
    );
    std::fs::remove_dir_all(&broken).ok();

    // (b) Valid shipment: `kelson migrate` applies it and reports the name.
    let root = temp_root("mysql-apply");
    write_pair(&root, "20260932000001_applied", table);
    let applied_now = kelson_mysql::run_migrations(&pool, &root)
        .await
        .expect("apply");
    assert_eq!(
        applied_now,
        ["20260932000001_applied"],
        "run_migrations returns the pair it applied"
    );
    assert!(mysql_table_exists(&pool, table).await, "table exists");
    let rows: Vec<(String, i32)> = kelson_mysql::pairs::applied(&pool).await.expect("applied");
    assert!(
        rows.iter().any(|(m, _)| m == "20260932000001_applied"),
        "bookkeeping row present: {rows:?}"
    );
    // Idempotent: a second run applies nothing (and re-checks the singles).
    assert!(
        kelson_mysql::run_migrations(&pool, &root)
            .await
            .expect("re-run")
            .is_empty()
    );

    // (c) `migrate:rollback`'s entry point reverts it.
    let reverted = kelson_mysql::pairs::rollback(&pool, &root, 1)
        .await
        .expect("rollback");
    assert_eq!(reverted, ["20260932000001_applied"]);
    assert!(
        !mysql_table_exists(&pool, table).await,
        "table dropped by its .down.sql"
    );
    let rows: Vec<(String, i32)> = kelson_mysql::pairs::applied(&pool).await.expect("applied");
    assert!(
        !rows.iter().any(|(m, _)| m == "20260932000001_applied"),
        "bookkeeping row removed: {rows:?}"
    );

    std::fs::remove_dir_all(&root).ok();
}
