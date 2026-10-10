//! Wave 5: rollback-capable pair migrations.
//!
//! Forward-only single-file migrations stay sqlx-owned (`database/migrations`,
//! `_sqlx_migrations`) and never roll back. Each rollback-capable migration
//! ships as a pair under [`PAIRS_DIR`]:
//!
//! ```text
//! <ts>_<name>.up.sql     applied by `kelson migrate` (filename order)
//! <ts>_<name>.down.sql   required; applied by `kelson migrate:rollback`
//! ```
//!
//! Applied pairs are recorded in the [`PAIRS_TABLE`] bookkeeping table with
//! one batch number per `migrate` run that applied at least one pair
//! (Laravel semantics): `rollback --step n` reverts the last n batches in
//! reverse filename order, `reset` reverts every batch, `refresh` rolls back
//! then re-applies. A `.up.sql` without its `.down.sql` (or an orphan
//! `.down.sql`) is a broken shipment and is refused at scan time —
//! fail-closed, before any DDL runs.
//!
//! Documented divergence: within a run, legacy singles apply before pairs;
//! rollback applies only to pairs (singles are forward-only).

use std::path::{Path, PathBuf};

use kelson_foundation::AppError;
use sqlx::Connection as _; // `begin()` on the acquired connection

/// Pair-migration directory, relative to the workspace root. Separate from
/// `database/migrations` on purpose: sqlx's embedded migrator treats every
/// `*.sql` file as one migration, so `<ts>_<name>.up.sql` and
/// `<ts>_<name>.down.sql` would collide on a single version and refuse the
/// run.
pub const PAIRS_DIR: &str = "database/migrations-pairs";

/// Bookkeeping table: one row per applied pair with its batch number.
pub const PAIRS_TABLE: &str = "keel_migrations";

/// One migration pair resolved from disk (filename order upstream).
#[derive(Debug)]
pub struct PairMigration {
    /// Base filename `<ts>_<name>` (primary key in [`PAIRS_TABLE`]).
    pub migration: String,
    /// Forward SQL, applied by `migrate` inside its own transaction.
    pub up_sql: String,
    /// Reverse SQL, required — applied by `rollback`/`reset`.
    pub down_sql: String,
}

/// Resolve the pairs directory against the workspace root.
pub fn pairs_dir(root: &Path) -> PathBuf {
    root.join(PAIRS_DIR)
}

/// Scan the pairs directory: every `<ts>_<name>.up.sql` with a required
/// sibling `.down.sql`, sorted by filename. A missing directory means "no
/// pair migrations yet" (additive); a malformed base name, a missing down
/// file, or an orphan down file is refused (fail-closed).
pub fn scan_pairs(root: &Path) -> Result<Vec<PairMigration>, AppError> {
    let dir = pairs_dir(root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(AppError::persistence(format!(
                "pair migrations: cannot read {}: {e}",
                dir.display()
            )))
        }
    };
    let mut up_files: Vec<PathBuf> = Vec::new();
    let mut down_bases: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| {
            AppError::persistence(format!("pair migrations: cannot list {}: {e}", dir.display()))
        })?;
        let path = entry.path();
        if !path.is_file() {
            continue; // subdirectories and the README are not migrations
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some(base) = name.strip_suffix(".up.sql") {
            validate_base(base)?;
            up_files.push(path);
        } else if let Some(base) = name.strip_suffix(".down.sql") {
            down_bases.push(base.to_string());
        }
    }
    up_files.sort();
    let mut out = Vec::with_capacity(up_files.len());
    for up in up_files {
        let name = up
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        let base = name.strip_suffix(".up.sql").unwrap_or_default();
        let down_path = up.with_file_name(format!("{base}.down.sql"));
        let down_sql = std::fs::read_to_string(&down_path).map_err(|e| {
            AppError::persistence(format!(
                "pair {base}: missing .down.sql (a pair that cannot roll back never applies): {e}"
            ))
        })?;
        let up_sql = std::fs::read_to_string(&up).map_err(|e| {
            AppError::persistence(format!("pair {base}: cannot read .up.sql: {e}"))
        })?;
        out.push(PairMigration {
            migration: base.to_string(),
            up_sql,
            down_sql,
        });
    }
    // Orphan down files: the up file was deleted or renamed — the pair can
    // never apply, and rolling back against a half-deleted pair is exactly
    // the state that must not slip through silently.
    for base in &down_bases {
        if !out.iter().any(|p| &p.migration == base) {
            return Err(AppError::persistence(format!(
                "pair {base}: orphan .down.sql without a matching .up.sql"
            )));
        }
    }
    Ok(out)
}

/// `<ts>_<name>` shape: numeric timestamp prefix, non-empty name.
fn validate_base(base: &str) -> Result<(), AppError> {
    let Some((ts, name)) = base.split_once('_') else {
        return Err(AppError::persistence(format!(
            "pair {base}: expected <ts>_<name>.up.sql (underscore-separated)"
        )));
    };
    if ts.is_empty() || !ts.bytes().all(|b| b.is_ascii_digit()) || name.is_empty() {
        return Err(AppError::persistence(format!(
            "pair {base}: expected a numeric <ts> prefix and a non-empty <name>"
        )));
    }
    Ok(())
}

/// Create the bookkeeping table if it does not exist (idempotent; runs no
/// migration files — this is `kelson migrate:install`). Any executor works:
/// a pool, the connection currently holding the migration lock, or a
/// transaction.
pub async fn install<'e, E>(exec: E) -> Result<(), AppError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {PAIRS_TABLE} (
             migration text PRIMARY KEY,
             batch integer NOT NULL,
             applied_at timestamptz NOT NULL DEFAULT now()
         )"
    ))
    .execute(exec)
    .await
    .map_err(|e| AppError::persistence(format!("migrate:install: {e}")))?;
    Ok(())
}

/// Every applied pair as `(migration, batch)`, filename order. Requires the
/// bookkeeping table (call [`install`] first).
pub async fn applied<'e, E>(exec: E) -> Result<Vec<(String, i32)>, AppError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query_as::<_, (String, i32)>(&format!(
        "SELECT migration, batch FROM {PAIRS_TABLE} ORDER BY migration"
    ))
    .fetch_all(exec)
    .await
    .map_err(|e| AppError::persistence(format!("{} read: {e}", PAIRS_TABLE)))
}

/// Pairs on disk that are not yet recorded, in filename order.
pub fn pending_pairs<'a>(
    applied: &[(String, i32)],
    scanned: &'a [PairMigration],
) -> Vec<&'a PairMigration> {
    scanned
        .iter()
        .filter(|p| !applied.iter().any(|(m, _)| m == &p.migration))
        .collect()
}

/// Revert order for `rollback --step n`: the last `steps` batches, newest
/// batch first, each batch in reverse filename order. `steps == u32::MAX`
/// (the `reset` sentinel) reverts every batch — a full reverse walk of the
/// applied history.
pub fn rollback_order(applied: &[(String, i32)], steps: u32) -> Vec<String> {
    let mut batches: Vec<i32> = applied.iter().map(|(_, b)| *b).collect();
    batches.sort_unstable();
    batches.dedup(); // ascending distinct batches
    let take = (steps as usize).min(batches.len());
    let selected = &batches[batches.len() - take..];
    let mut out: Vec<String> = Vec::new();
    for batch in selected.iter().rev() {
        let mut names: Vec<&str> = applied
            .iter()
            .filter(|(_, b)| b == batch)
            .map(|(m, _)| m.as_str())
            .collect();
        names.sort_unstable();
        out.extend(names.into_iter().rev().map(String::from));
    }
    out
}

/// Apply every pending pair in filename order, each inside its own
/// transaction (forward SQL + bookkeeping row commit together or not at
/// all). All newly applied pairs share the next batch number. Accepts any
/// Acquire (pool, or the connection holding the migration lock). Returns
/// the applied migration names.
pub async fn apply_pending<'a, A>(acquire: A, root: &Path) -> Result<Vec<String>, AppError>
where
    A: sqlx::Acquire<'a, Database = sqlx::Postgres>,
    A::Connection: std::ops::DerefMut<Target = sqlx::PgConnection>,
{
    let mut conn = acquire
        .acquire()
        .await
        .map_err(|e| AppError::persistence(format!("pair migrations: begin run: {e}")))?;
    install(&mut *conn).await?;
    let applied = applied(&mut *conn).await?;
    let scanned = scan_pairs(root)?;
    let queued = pending_pairs(&applied, &scanned);
    if queued.is_empty() {
        return Ok(Vec::new());
    }
    let next_batch = applied.iter().map(|(_, b)| *b).max().unwrap_or(0) + 1;
    let mut done = Vec::with_capacity(queued.len());
    for pair in queued {
        let mut tx = conn
            .begin()
            .await
            .map_err(|e| AppError::persistence(format!("{} (up): begin: {e}", pair.migration)))?;
        sqlx::raw_sql(&pair.up_sql).execute(&mut *tx).await.map_err(|e| {
            AppError::persistence(format!("{} (up): {e}", pair.migration))
        })?;
        sqlx::query(&format!(
            "INSERT INTO {PAIRS_TABLE} (migration, batch) VALUES ($1, $2)"
        ))
        .bind(&pair.migration)
        .bind(next_batch)
        .execute(&mut *tx)
        .await
        .map_err(|e| {
            AppError::persistence(format!("{} (up): record: {e}", pair.migration))
        })?;
        tx.commit().await.map_err(|e| {
            AppError::persistence(format!("{} (up): commit: {e}", pair.migration))
        })?;
        done.push(pair.migration.clone());
    }
    Ok(done)
}

/// Revert the last `steps` batches in [`rollback_order`]: reverse SQL, then
/// the bookkeeping row, per migration inside its own transaction. Applied
/// rows without a pair file on disk are refused (fail-closed — the down
/// migration is missing, so reverting is impossible). Returns the reverted
/// names in revert order.
pub async fn rollback<'a, A>(acquire: A, root: &Path, steps: u32) -> Result<Vec<String>, AppError>
where
    A: sqlx::Acquire<'a, Database = sqlx::Postgres>,
    A::Connection: std::ops::DerefMut<Target = sqlx::PgConnection>,
{
    let mut conn = acquire
        .acquire()
        .await
        .map_err(|e| AppError::persistence(format!("pair migrations: begin rollback: {e}")))?;
    install(&mut *conn).await?;
    let applied = applied(&mut *conn).await?;
    let order = rollback_order(&applied, steps);
    if order.is_empty() {
        return Ok(Vec::new());
    }
    // Scan before touching any row: a broken pairs directory refuses the
    // rollback up front instead of half-way through.
    let scanned = scan_pairs(root)?;
    let mut done = Vec::with_capacity(order.len());
    for name in order {
        let Some(pair) = scanned.iter().find(|p| p.migration == name) else {
            return Err(AppError::persistence(format!(
                "{name}: recorded as applied but no pair file present (rollback impossible)"
            )));
        };
        let mut tx = conn
            .begin()
            .await
            .map_err(|e| AppError::persistence(format!("{name} (down): begin: {e}")))?;
        sqlx::raw_sql(&pair.down_sql)
            .execute(&mut *tx)
            .await
            .map_err(|e| AppError::persistence(format!("{name} (down): {e}")))?;
        sqlx::query(&format!(
            "DELETE FROM {PAIRS_TABLE} WHERE migration = $1"
        ))
        .bind(&name)
        .execute(&mut *tx)
        .await
        .map_err(|e| AppError::persistence(format!("{name} (down): record: {e}")))?;
        tx.commit().await.map_err(|e| {
            AppError::persistence(format!("{name} (down): commit: {e}"))
        })?;
        done.push(name);
    }
    Ok(done)
}

/// Revert every applied pair (`rollback` with the full-history sentinel).
pub async fn reset<'a, A>(acquire: A, root: &Path) -> Result<Vec<String>, AppError>
where
    A: sqlx::Acquire<'a, Database = sqlx::Postgres>,
    A::Connection: std::ops::DerefMut<Target = sqlx::PgConnection>,
{
    rollback(acquire, root, u32::MAX).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kelson-pairs-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(dir.join(PAIRS_DIR)).unwrap();
        dir
    }

    fn write_pair(root: &Path, base: &str, up: &str, down: &str) {
        let dir = pairs_dir(root);
        std::fs::write(dir.join(format!("{base}.up.sql")), up).unwrap();
        std::fs::write(dir.join(format!("{base}.down.sql")), down).unwrap();
    }

    /// AppError's Display is the public envelope (`[DB_ERROR] ...`); the
    /// pair-scan detail lives in `internal` — assert on that, exactly as
    /// the console renders it for the operator.
    fn detail(e: &AppError) -> String {
        e.internal.clone().unwrap_or_else(|| e.to_string())
    }

    /// Scan: filename order, both files captured, README/stray singles
    /// ignored; then fail-closed refusals (missing down, orphan down, bad
    /// base name).
    #[test]
    fn scan_pairs_orders_files_and_refuses_broken_shipments() {
        let root = temp_root("scan");
        // Written out of order to prove sorting is by filename.
        write_pair(
            &root,
            "20260902000001_beta",
            "CREATE TABLE beta (id int);",
            "DROP TABLE beta;",
        );
        write_pair(
            &root,
            "20260901000001_alpha",
            "CREATE TABLE alpha (id int);",
            "DROP TABLE alpha;",
        );
        // Non-pair files: a legacy-style single (lives in database/migrations
        // in real workspaces, but stray files here must not scan) and docs.
        std::fs::write(pairs_dir(&root).join("README.md"), "docs").unwrap();
        std::fs::write(pairs_dir(&root).join("stray.sql"), "SELECT 1;").unwrap();

        let scanned = scan_pairs(&root).expect("scan runs");
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
        let err = scan_pairs(&root).expect_err("missing down refuses the scan");
        assert!(detail(&err).contains("20260903000001_gamma"), "{err:?}");

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
        let err = scan_pairs(&root).expect_err("orphan down refuses the scan");
        assert!(detail(&err).contains("orphan"), "{err:?}");

        // Bad base name (no numeric <ts>) -> refused even with both files.
        std::fs::remove_file(pairs_dir(&root).join("20260904000001_delta.down.sql")).unwrap();
        write_pair(&root, "no_timestamp_pair", "SELECT 1;", "SELECT 1;");
        let err = scan_pairs(&root).expect_err("bad base name refuses the scan");
        assert!(detail(&err).contains("numeric"), "{err:?}");
        std::fs::remove_dir_all(&root).ok();
    }

    /// A missing pairs directory scans as empty (additive: workspaces with
    /// no pair migrations boot and migrate normally).
    #[test]
    fn scan_pairs_treats_a_missing_directory_as_empty() {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "kelson-pairs-absent-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        assert!(scan_pairs(&root).expect("absent dir scans").is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    /// Rollback order: last n batches, newest first, reverse filename order
    /// within a batch; `u32::MAX` (reset) walks the whole history backwards.
    #[test]
    fn rollback_order_walks_batches_newest_first_in_reverse_filename_order() {
        let applied: Vec<(String, i32)> = vec![
            ("20260901_a".into(), 1),
            ("20260902_b".into(), 1),
            ("20260903_c".into(), 1),
            ("20260910_d".into(), 2),
            ("20260911_e".into(), 2),
            ("20260920_f".into(), 3),
        ];
        assert_eq!(rollback_order(&applied, 0), Vec::<String>::new());
        // Default step 1: only the newest batch, reverse filename order.
        assert_eq!(rollback_order(&applied, 1), ["20260920_f"]);
        // Step 2: batch 3, then batch 2 newest-first.
        assert_eq!(
            rollback_order(&applied, 2),
            ["20260920_f", "20260911_e", "20260910_d"]
        );
        // Full history (reset sentinel): strict reverse walk.
        assert_eq!(
            rollback_order(&applied, u32::MAX),
            [
                "20260920_f",
                "20260911_e",
                "20260910_d",
                "20260903_c",
                "20260902_b",
                "20260901_a",
            ]
        );
        // Over-asking is clamped to the applied history.
        assert_eq!(rollback_order(&applied, 99).len(), 6);
    }

    /// Pending computation: applied pairs are skipped, new ones stay queued
    /// in filename order (including legacy rows whose files are gone).
    #[test]
    fn pending_pairs_skips_applied_rows() {
        let scan = |names: &[&str]| -> Vec<PairMigration> {
            names
                .iter()
                .map(|n| PairMigration {
                    migration: (*n).to_string(),
                    up_sql: String::new(),
                    down_sql: String::new(),
                })
                .collect()
        };
        let scanned = scan(&["20260901_a", "20260902_b", "20260903_c"]);
        let applied: Vec<(String, i32)> = vec![("20260902_b".into(), 1)];
        let queued: Vec<&str> = pending_pairs(&applied, &scanned)
            .iter()
            .map(|p| p.migration.as_str())
            .collect();
        assert_eq!(queued, ["20260901_a", "20260903_c"]);
        // Everything applied -> nothing queued (idempotent second run).
        let all_applied: Vec<(String, i32)> = scanned
            .iter()
            .map(|p| (p.migration.clone(), 2))
            .collect();
        assert!(pending_pairs(&all_applied, &scanned).is_empty());
    }
}
