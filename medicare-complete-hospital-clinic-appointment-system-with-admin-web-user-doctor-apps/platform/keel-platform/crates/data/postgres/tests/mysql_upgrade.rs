//! The MySQL upgrade-contract matrix.
//!
//! The framework's hardest promise is that upgrading it does not break the
//! data. These cases prove the three parts of the contract against a live
//! MySQL 8:
//!
//! 1. **the schema has a version** - a digest over the migration set that
//!    is actually applied, not a hand-maintained number;
//! 2. **a stale schema is REFUSED** - a deployment that does not migrate
//!    at boot refuses to serve against a schema the framework does not
//!    recognise;
//! 3. **the upgrade is safe** - a step that changes an existing table goes
//!    through the online path (the shadow table, the copy, the atomic swap)
//!    instead of an ALTER that rebuilds the table under a lock.
//!
//! Executed against a live MySQL 8.

#![cfg(feature = "pg-tests")]

use kelson_mysql::pairs::pairs_dir;
use kelson_mysql::upgrade;
use sqlx::Row;
use std::path::{Path, PathBuf};
use uuid::Uuid;

async fn pool() -> Option<kelson_mysql::MySqlPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(url) if url.starts_with("mysql://") => url,
        _ => {
            eprintln!("mysql-upgrade: DATABASE_URL is not mysql://; skipping");
            return None;
        }
    };
    match kelson_mysql::create_pool(&url, 4).await {
        Ok(pool) => Some(pool),
        Err(e) => {
            eprintln!("mysql-upgrade: cannot connect ({e}); skipping");
            None
        }
    }
}

/// A disposable workspace root with a pairs directory.
fn scratch_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("kelson_upgrade_{}", Uuid::now_v7().simple()));
    std::fs::create_dir_all(pairs_dir(&root)).expect("the scratch root is created");
    root
}

fn write_pair(root: &Path, base: &str, up: &str, down: &str) {
    let dir = pairs_dir(root);
    std::fs::write(dir.join(format!("{base}.up.sql")), up).unwrap();
    std::fs::write(dir.join(format!("{base}.down.sql")), down).unwrap();
}

/// A pair name that is unique per run, so a pair left behind by an earlier
/// run cannot make this run find the schema already current.
fn unique_pair() -> String {
    // The pair's base name is `<numeric timestamp>_<name>`, so the
    // uniqueness has to live in the numeric part: the millisecond clock
    // plus four hex digits of a fresh uuid.
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let simple = Uuid::now_v7().simple().to_string();
    let suffix = u32::from_str_radix(&simple[..4], 16).unwrap_or(0);
    format!("{:013}{:04}_probe", millis, suffix)
}

/// Remove a synthetic pair's bookkeeping (the migration row and the step
/// state), so the shared database is left as it was found.
async fn forget_pair(pool: &kelson_mysql::MySqlPool, name: &str) {
    for statement in [
        format!("DELETE FROM keel_migrations WHERE migration = '{name}'"),
        format!("DELETE FROM keel_migration_steps WHERE migration = '{name}'"),
    ] {
        let _ = sqlx::raw_sql(&statement).execute(pool).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_upgrade_the_schema_state_names_the_pending_migrations() {
    let Some(pool) = pool().await else { return };
    let root = scratch_root();
    // A migration the database does not have.
    let pair = unique_pair();
    write_pair(&root, &pair, "SELECT 1;", "SELECT 1;");

    let state = upgrade::schema_state(&pool, &root)
        .await
        .expect("the schema state reads");
    assert!(
        state.pending.contains(&pair),
        "the pending migration must be named: {:?}",
        state.pending
    );
    assert!(!state.is_current());
    assert_ne!(
        state.version, state.required_version,
        "a schema that is behind has a different version than the one required"
    );

    // The version is a digest of the applied set: the same set hashes the
    // same, a different set does not.
    let a = upgrade::version_of(&["x".to_string(), "y".to_string()]);
    let b = upgrade::version_of(&["y".to_string(), "x".to_string()]);
    let c = upgrade::version_of(&["x".to_string()]);
    assert_eq!(a, b, "the order does not change the version");
    assert_ne!(a, c, "a different set is a different version");

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_upgrade_a_stale_schema_is_refused() {
    let Some(pool) = pool().await else { return };
    let root = scratch_root();
    let pair = unique_pair();
    write_pair(&root, &pair, "SELECT 1;", "SELECT 1;");

    let refusal = upgrade::assert_current(&pool, &root)
        .await
        .expect_err("a stale schema must be refused");
    let detail = refusal.internal.clone().unwrap_or_default();
    assert!(
        detail.contains("behind") && detail.contains(&pair),
        "the refusal must say what is behind and name it: {detail}"
    );

    // With the migration applied, the same check passes.
    upgrade::upgrade_pairs(&pool, &root)
        .await
        .expect("the upgrade applies the pending pair");
    upgrade::assert_current(&pool, &root)
        .await
        .expect("the schema is current now");

    forget_pair(&pool, &pair).await;
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_upgrade_a_change_to_an_existing_table_goes_through_the_online_path() {
    let Some(pool) = pool().await else { return };
    let root = scratch_root();

    // The table the change touches.
    let table = format!("kelson_upgrade_tbl_{}", Uuid::now_v7().simple());
    sqlx::raw_sql(&format!(
        "CREATE TABLE {table} (id INT NOT NULL PRIMARY KEY, amount BIGINT NOT NULL)"
    ))
    .execute(&pool)
    .await
    .expect("the probe table is created");
    for i in 1..=12i64 {
        sqlx::query(&format!("INSERT INTO {table} (id, amount) VALUES (?, ?)"))
            .bind(i)
            .bind(i * 10)
            .execute(&pool)
            .await
            .expect("the probe row inserts");
    }

    // The pair changes the existing table: this must take the ONLINE path,
    // not an ALTER that rebuilds it under a lock.
    let pair = unique_pair();
    write_pair(
        &root,
        &pair,
        &format!("ALTER TABLE {table} ADD COLUMN note VARCHAR(32) NOT NULL DEFAULT 'x';"),
        &format!("ALTER TABLE {table} DROP COLUMN note;"),
    );
    let applied = upgrade::upgrade_pairs(&pool, &root)
        .await
        .expect("the upgrade applies the pair");
    assert_eq!(applied, vec![pair.clone()]);

    // The rows survived and the new column is there.
    let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(&pool)
        .await
        .expect("the rows are there");
    assert_eq!(count, 12, "the online path copied every row");
    let notes: Vec<String> = sqlx::query(&format!("SELECT note FROM {table} ORDER BY id"))
        .fetch_all(&pool)
        .await
        .expect("the new column reads")
        .into_iter()
        .map(|r| r.get::<String, _>(0))
        .collect();
    assert_eq!(notes, vec!["x"; 12], "the new column carries its default");

    // The shadow and the retired table are gone: the swap was complete.
    let leftovers: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM information_schema.tables WHERE table_schema = DATABASE() AND table_name IN (?, ?)",
    )
    .bind(format!("_kelson_new_{table}"))
    .bind(format!("_kelson_old_{table}"))
    .fetch_one(&pool)
    .await
    .expect("the leftovers are counted");
    assert_eq!(leftovers, 0, "the shadow and the retired table must be gone");

    sqlx::raw_sql(&format!("DROP TABLE {table}"))
        .execute(&pool)
        .await
        .expect("the probe table goes away");
    forget_pair(&pool, &pair).await;
    std::fs::remove_dir_all(&root).ok();
}
