//! The MySQL migration-safety matrix.
//!
//! The production-grade migration story: a MySQL DDL auto-commits, so a
//! failed migration can leave the schema half-applied. These cases prove
//! the two answers to that:
//!
//! 1. **the guarded runner** - every statement of a pair is recorded, so a
//!    failed run RESUMES from the recorded step instead of re-applying the
//!    whole file, and a file that changed under the record is refused;
//! 2. **the online change** - a schema change on a live table is applied by
//!    building the new shape beside the old one, copying the rows, and
//!    swapping the names in one atomic statement.
//!
//! Executed against a live MySQL 8, not asserted on the SQL text.

#![cfg(feature = "pg-tests")]

use kelson_mysql::guarded;
use kelson_mysql::online::{self, OnlineChange, Strategy};
use sqlx::Row;
use uuid::Uuid;

async fn pool() -> Option<kelson_mysql::MySqlPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(url) if url.starts_with("mysql://") => url,
        _ => {
            eprintln!("mysql-migrations: DATABASE_URL is not mysql://; skipping");
            return None;
        }
    };
    match kelson_mysql::create_pool(&url, 4).await {
        Ok(pool) => Some(pool),
        Err(e) => {
            eprintln!("mysql-migrations: cannot connect ({e}); skipping");
            None
        }
    }
}

/// A unique table name, so the cases never collide.
fn table(prefix: &str) -> String {
    format!("{prefix}_{}", Uuid::now_v7().simple())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_migration_the_guarded_runner_records_every_step() {
    let Some(pool) = pool().await else { return };
    let name = table("kelson_guard_ok");
    let migration = format!("20260101000000_{name}");

    let sql = format!(
        "CREATE TABLE {name} (id INT NOT NULL PRIMARY KEY); \
         INSERT INTO {name} (id) VALUES (1); \
         INSERT INTO {name} (id) VALUES (2);"
    );
    let verdict = guarded::apply_pair_guarded(&pool, &migration, &sql)
        .await
        .expect("the pair applies");
    assert_eq!(verdict.applied_steps, vec![0, 1, 2]);
    assert!(verdict.resumed_steps.is_empty());
    assert!(
        verdict.auto_committing_steps.contains(&0),
        "the CREATE TABLE step auto-commits: {:?}",
        verdict.auto_committing_steps
    );
    assert!(!verdict.transactional, "the pair is NOT transactional");
    assert!(verdict.summary().contains("NOT transactional"));

    // The state is recorded, statement by statement.
    let state = guarded::step_state(&pool, &migration)
        .await
        .expect("the step state reads");
    assert_eq!(state.len(), 3);
    assert_eq!(state[0].step, 0);
    assert_eq!(
        state[0].checksum,
        guarded::checksum(&sql.split(';').next().unwrap().trim())
    );

    // The rows landed.
    let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {name}"))
        .fetch_one(&pool)
        .await
        .expect("the rows are there");
    assert_eq!(count, 2);

    sqlx::raw_sql(&format!("DROP TABLE {name}"))
        .execute(&pool)
        .await
        .expect("the probe table goes away");
    guarded::clear_steps(&pool, &migration)
        .await
        .expect("the step state clears");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_migration_the_guarded_runner_resumes_after_a_failed_step() {
    let Some(pool) = pool().await else { return };
    let name = table("kelson_guard_resume");
    let source = table("kelson_guard_src");
    let migration = format!("20260101000001_{name}");

    // Step 1 reads a table that does not exist yet: the run fails there,
    // and the steps before it stay applied.
    let sql = format!(
        "CREATE TABLE {name} (id INT NOT NULL PRIMARY KEY); \
         INSERT INTO {name} (id) SELECT id FROM {source};"
    );
    let failure = guarded::apply_pair_guarded(&pool, &migration, &sql)
        .await
        .expect_err("the second step fails");
    // The public message is redacted on purpose (the framework's
    // discipline); the detail that names the step lives in `internal`.
    let message = failure.internal.clone().unwrap_or_default();
    assert!(
        message.contains("step 1"),
        "the failure must name the step: {message}"
    );
    assert!(
        message.contains("auto-commits") || message.contains("remain applied"),
        "the failure must say what stays applied: {message}"
    );

    // The first step IS applied (MySQL committed it) and IS recorded.
    let state = guarded::step_state(&pool, &migration)
        .await
        .expect("the step state reads");
    assert_eq!(state.len(), 1, "only the applied step is recorded");
    assert_eq!(state[0].step, 0);

    // Now the missing table exists, and the SAME pair runs again: the
    // recorded step is skipped and the run continues from where it stopped.
    sqlx::raw_sql(&format!("CREATE TABLE {source} (id INT NOT NULL)"))
        .execute(&pool)
        .await
        .expect("the source table is created");
    let verdict = guarded::apply_pair_guarded(&pool, &migration, &sql)
        .await
        .expect("the resumed run succeeds");
    assert_eq!(verdict.resumed_steps, vec![0], "step 0 is not re-applied");
    assert_eq!(verdict.applied_steps, vec![1], "the run continues at step 1");

    for probe in [name, source] {
        sqlx::raw_sql(&format!("DROP TABLE IF EXISTS {probe}"))
            .execute(&pool)
            .await
            .expect("the probe table goes away");
    }
    guarded::clear_steps(&pool, &migration)
        .await
        .expect("the step state clears");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_migration_the_guarded_runner_refuses_a_resume_after_the_file_changed() {
    let Some(pool) = pool().await else { return };
    let name = table("kelson_guard_changed");
    let source = table("kelson_guard_changed_src");
    let migration = format!("20260101000002_{name}");

    let sql = format!(
        "CREATE TABLE {name} (id INT NOT NULL PRIMARY KEY); \
         INSERT INTO {name} (id) SELECT id FROM {source};"
    );
    let _ = guarded::apply_pair_guarded(&pool, &migration, &sql)
        .await
        .expect_err("the second step fails");

    // The file is edited: the recorded step's text no longer matches.
    let edited = format!(
        "CREATE TABLE {name} (id INT NOT NULL PRIMARY KEY, extra INT NULL); \
         INSERT INTO {name} (id) SELECT id FROM {source};"
    );
    let refusal = guarded::apply_pair_guarded(&pool, &migration, &edited)
        .await
        .expect_err("the changed file must be refused");
    let detail = refusal.internal.clone().unwrap_or_default();
    assert!(
        detail.contains("changed since it was applied"),
        "the refusal must name the cause: {detail}"
    );

    for probe in [name, source] {
        sqlx::raw_sql(&format!("DROP TABLE IF EXISTS {probe}"))
            .execute(&pool)
            .await
            .expect("the probe table goes away");
    }
    guarded::clear_steps(&pool, &migration)
        .await
        .expect("the step state clears");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_migration_the_online_change_adds_a_column_without_rebuilding() {
    let Some(pool) = pool().await else { return };
    let name = table("kelson_online_instant");
    sqlx::raw_sql(&format!(
        "CREATE TABLE {name} (id INT NOT NULL PRIMARY KEY, amount BIGINT NOT NULL)"
    ))
    .execute(&pool)
    .await
    .expect("the probe table is created");
    for i in 1..=5 {
        sqlx::query(&format!("INSERT INTO {name} (id, amount) VALUES (?, ?)"))
            .bind(i)
            .bind(i * 10)
            .execute(&pool)
            .await
            .expect("the probe row inserts");
    }

    let change = OnlineChange::new(&name, "ADD COLUMN note VARCHAR(32) NOT NULL DEFAULT ''", "id");
    let report = online::apply_online(&pool, &change)
        .await
        .expect("the online change applies");
    assert_eq!(report.strategy, Strategy::Instant, "{report:?}");
    assert!(report.swapped);

    // The rows are intact and the new column is there.
    let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {name}"))
        .fetch_one(&pool)
        .await
        .expect("the rows are there");
    assert_eq!(count, 5);
    let notes: Vec<String> = sqlx::query(&format!("SELECT note FROM {name} ORDER BY id"))
        .fetch_all(&pool)
        .await
        .expect("the new column reads")
        .into_iter()
        .map(|r| r.get::<String, _>(0))
        .collect();
    assert_eq!(notes, vec![""; 5]);

    sqlx::raw_sql(&format!("DROP TABLE {name}"))
        .execute(&pool)
        .await
        .expect("the probe table goes away");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_migration_the_online_change_copies_the_rows_and_swaps_atomically() {
    let Some(pool) = pool().await else { return };
    let name = table("kelson_online_copy");
    sqlx::raw_sql(&format!(
        "CREATE TABLE {name} (id INT NOT NULL PRIMARY KEY, amount BIGINT NOT NULL)"
    ))
    .execute(&pool)
    .await
    .expect("the probe table is created");
    for i in 1..=25 {
        sqlx::query(&format!("INSERT INTO {name} (id, amount) VALUES (?, ?)"))
            .bind(i)
            .bind(i * 10)
            .execute(&pool)
            .await
            .expect("the probe row inserts");
    }

    // A MODIFY is NOT instant: the copy path must run - the shadow table is
    // built, the rows are copied in chunks, and the names are swapped in
    // ONE statement.
    let change = OnlineChange::new(&name, "MODIFY COLUMN amount BIGINT UNSIGNED NOT NULL", "id")
        .chunk(10);
    let report = online::apply_online(&pool, &change)
        .await
        .expect("the online change applies");
    assert_eq!(report.strategy, Strategy::Copy, "{report:?}");
    assert_eq!(report.rows_copied, 25);
    assert_eq!(report.source_rows, 25);
    assert_eq!(report.shadow_rows, 25);
    assert!(report.swapped);

    // The rows survived the swap. (MySQL's SUM over an exact-value column
    // returns DECIMAL, so the sum is cast back to an integer.)
    let sum: i64 = sqlx::query_scalar(&format!(
        "SELECT CAST(COALESCE(SUM(amount), 0) AS SIGNED) FROM {name}"
    ))
    .fetch_one(&pool)
    .await
    .expect("the rows are there");
    assert_eq!(sum, (1..=25).sum::<i64>() * 10);

    // The shadow and the retired table are gone: the swap was complete.
    let leftovers: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME IN (?, ?)",
    )
    .bind(format!("_kelson_new_{name}"))
    .bind(format!("_kelson_old_{name}"))
    .fetch_one(&pool)
    .await
    .expect("the leftovers are counted");
    assert_eq!(leftovers, 0, "the shadow and the retired table must be gone");

    sqlx::raw_sql(&format!("DROP TABLE {name}"))
        .execute(&pool)
        .await
        .expect("the probe table goes away");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_migration_a_runner_that_loses_the_lock_waits_and_re_checks() {
    let Some(pool) = pool().await else { return };
    // The cold-start race: a rolling deploy with several instances booting
    // at once. One runner takes the migration lock; the others must NOT
    // fail the boot - they wait for the holder and then re-check whether
    // the migrations are applied.
    let mut holder = pool.acquire().await.expect("a connection is free");
    kelson_mysql::acquire_migration_lock(&mut holder)
        .await
        .expect("the holder takes the lock");

    // The other runner starts while the lock is held. It cannot take the
    // lock (the 10s fail-closed deadline), so it WAITS for the holder.
    let mut waiter = Box::pin(kelson_mysql::run_migrations(&pool, std::path::Path::new(".")));
    let polled = tokio::time::timeout(std::time::Duration::from_millis(800), &mut waiter).await;
    assert!(
        polled.is_err(),
        "the waiter must WAIT for the holder, not fail: {polled:?}"
    );

    // The holder finishes and releases: the migrations are applied.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    kelson_mysql::release_migration_lock(&mut holder)
        .await
        .expect("the holder releases the lock");
    drop(holder);

    // The waiter now finishes: it re-checks the state, finds the
    // migrations applied, and reports that it applied none.
    let outcome = waiter
        .await
        .expect("the waiter must NOT fail: it re-checks the state after the holder releases");
    assert!(
        outcome.is_empty(),
        "the waiter must apply nothing: the holder did it: {outcome:?}"
    );
}
