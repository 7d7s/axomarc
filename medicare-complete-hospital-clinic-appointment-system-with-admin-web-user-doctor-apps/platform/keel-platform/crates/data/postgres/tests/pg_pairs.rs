//! Wave 5 pair-migration round trip: apply with batches, rollback, reset —
//! exercised against a live database. Activated with `--features pg-tests`;
//! every test is a graceful skip when DATABASE_URL is unset, so local runs
//! without a database stay green (same contract as `pg_integration`).

#![cfg(feature = "pg-tests")]

use keel_postgres::pairs::{
    apply_pending, applied, install, pairs_dir, reset, rollback, PAIRS_TABLE,
};
use keel_postgres::{
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
        "keel-pg-pairs-{tag}-{}-{}",
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
    let alpha = "keel_w5_pairs_alpha";
    let beta = "keel_w5_pairs_beta";
    let gamma = "keel_w5_pairs_gamma";

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
