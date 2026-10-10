//! Wave 9 B-31: rollback-capable pair migrations on MySQL.
//!
//! The shipment, the bookkeeping table and the batch arithmetic are
//! `kelson_postgres::pairs` — same `<ts>_<name>.up.sql` / `<ts>_<name>.down.sql`
//! files under `database/migrations-pairs`, same `keel_migrations` table,
//! same Laravel batch semantics, same fail-closed scan — so one directory
//! drives both fabrics and `kelson migrate:status` reads one bookkeeping
//! table with one statement. What differs is execution, and only because
//! MySQL DDL cannot be transactional:
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
//! Documented divergences (docs/05-DATABASE):
//! - **apply and rollback are non-transactional, fail-stop-and-report.**
//!   MySQL DDL causes an implicit commit, so the Postgres runner's
//!   "forward SQL + bookkeeping row commit together or not at all" cannot
//!   hold: wrapping this pair's statements in a transaction would promise an
//!   atomicity the engine does not offer. Laravel concedes the same point —
//!   `MySQLGrammar` inherits `$transactions = false` from `Grammar` (only
//!   `PostgresGrammar`/`SqlServerGrammar` set `true`), so `Migrator`'s
//!   `supportsSchemaTransactions()` gate runs a MySQL `up`/`down` outside
//!   any transaction. Each pair therefore runs
//!   statement by statement, best effort — the first error stops the run,
//!   and the message says that statements before it remain applied, so the
//!   operator repairs the schema and re-runs instead of trusting a rollback
//!   that never happened. A pair whose DDL landed but whose bookkeeping row
//!   did not re-applies on the next run and fails visibly ("already exists"
//!   for a non-idempotent up), i.e. still fail-closed — reported one layer
//!   up rather than swallowed.
//! - **the bookkeeping row is written after the forward SQL**, where
//!   Postgres writes it inside the same transaction. sqlx's own MySQL
//!   migrator pre-writes its row with `success = FALSE` for the identical
//!   implicit-commit reason; this table deliberately stays column-for-column
//!   the Postgres one instead, because `migrate:status` reads it from both
//!   drivers and a `success` flag would need filtering the Postgres half
//!   does not do.
//! - **pair SQL runs over the text protocol** (`sqlx::raw_sql` -> `COM_QUERY`
//!   with the `MULTI_STATEMENTS` capability sqlx declares at handshake,
//!   `capabilities.rs` `1 << 16`), the same path `migrate` pushes the nine
//!   forward files through. Probed against 8.0.44: `COM_STMT_PREPARE`
//!   accepts exactly one statement (two `CREATE TABLE`s in one prepare ->
//!   ERROR 1064 near the second) and refuses transaction control outright
//!   (ERROR 1295 — `test_support::Savepoint`), while the single-statement
//!   bookkeeping DDL and DML prepare and execute normally.
//! - **`migration` is `VARCHAR(255)`, not `text`** — InnoDB needs a prefix
//!   length to key a TEXT column, and the value is a `<ts>_<name>` filename.
//!   **`applied_at` is `DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)`**
//!   — MySQL has no `timestamptz`; sqlx reads and writes `DATETIME(6)` as
//!   UTC (crate header).

use std::path::{Path, PathBuf};

use kelson_foundation::AppError;

/// Pair-migration directory, relative to the workspace root. Separate from
/// `database/migrations-mysql` on purpose: sqlx's embedded migrator treats
/// every `*.sql` file as one migration, so `<ts>_<name>.up.sql` and
/// `<ts>_<name>.down.sql` would collide on a single version and refuse the
/// run.
pub const PAIRS_DIR: &str = "database/migrations-pairs";

/// Bookkeeping table: one row per applied pair with its batch number. Same
/// name and columns as the Postgres runner's table — `migrate:status` and
/// the promotion watermark read it with the same statement whichever driver
/// created it.
pub const PAIRS_TABLE: &str = "keel_migrations";

/// The report suffix every non-transactional failure carries: MySQL DDL
/// commits as it goes, so "it failed" must never be read as "nothing
/// happened".
const NONTRANSACTIONAL_NOTE: &str = "MySQL DDL auto-commits: statements before this failure remain applied — inspect the schema before re-running";

/// One migration pair resolved from disk (filename order upstream).
#[derive(Debug)]
pub struct PairMigration {
    /// Base filename `<ts>_<name>` (primary key in [`PAIRS_TABLE`]).
    pub migration: String,
    /// Forward SQL, applied by `migrate` statement by statement (no
    /// transaction to wrap it in — see the module doc).
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
/// a pool, or the connection currently holding the migration lock.
pub async fn install<'e, E>(exec: E) -> Result<(), AppError>
where
    E: sqlx::Executor<'e, Database = sqlx::MySql>,
{
    sqlx::query(&format!(
        "CREATE TABLE IF NOT EXISTS {PAIRS_TABLE} (
             migration VARCHAR(255) PRIMARY KEY,
             batch INT NOT NULL,
             applied_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
         )"
    ))
    .execute(exec)
    .await
    .map_err(|e| AppError::persistence(format!("migrate:install: {e}")))?;
    Ok(())
}

/// Every applied pair as `(migration, batch)`, filename order. Requires the
/// bookkeeping table (call [`install`] first). Identical SQL to the
/// Postgres runner — nothing here is dialect-specific.
pub async fn applied<'e, E>(exec: E) -> Result<Vec<(String, i32)>, AppError>
where
    E: sqlx::Executor<'e, Database = sqlx::MySql>,
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

/// Apply every pending pair in filename order, each **without** a
/// transaction (MySQL DDL would commit it out from under the bookkeeping
/// row — see the module doc): forward SQL, then the bookkeeping row, the
/// first error stopping the run with the pair name and what already
/// happened. All newly applied pairs share the next batch number. Accepts
/// any Acquire (pool, or the connection holding the migration lock).
/// Returns the applied migration names.
pub async fn apply_pending<'a, A>(acquire: A, root: &Path) -> Result<Vec<String>, AppError>
where
    A: sqlx::Acquire<'a, Database = sqlx::MySql>,
    A::Connection: std::ops::DerefMut<Target = sqlx::MySqlConnection>,
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
        sqlx::raw_sql(&pair.up_sql).execute(&mut *conn).await.map_err(|e| {
            AppError::persistence(format!(
                "{} (up): {e} ({NONTRANSACTIONAL_NOTE})",
                pair.migration
            ))
        })?;
        sqlx::query(&format!(
            "INSERT INTO {PAIRS_TABLE} (migration, batch) VALUES (?, ?)"
        ))
        .bind(&pair.migration)
        .bind(next_batch)
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            AppError::persistence(format!("{} (up): record: {e}", pair.migration))
        })?;
        done.push(pair.migration.clone());
    }
    Ok(done)
}

/// Revert the last `steps` batches in [`rollback_order`]: reverse SQL, then
/// the removal of the bookkeeping row, per migration, fail-stop-and-report
/// (no transaction to give back the DDL — the report says so). Applied rows
/// without a pair file on disk are refused (fail-closed — the down
/// migration is missing, so reverting is impossible). Returns the reverted
/// names in revert order.
pub async fn rollback<'a, A>(acquire: A, root: &Path, steps: u32) -> Result<Vec<String>, AppError>
where
    A: sqlx::Acquire<'a, Database = sqlx::MySql>,
    A::Connection: std::ops::DerefMut<Target = sqlx::MySqlConnection>,
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
        sqlx::raw_sql(&pair.down_sql)
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                AppError::persistence(format!(
                    "{name} (down): {e} ({NONTRANSACTIONAL_NOTE})"
                ))
            })?;
        sqlx::query(&format!(
            "DELETE FROM {PAIRS_TABLE} WHERE migration = ?"
        ))
        .bind(&name)
        .execute(&mut *conn)
        .await
        .map_err(|e| AppError::persistence(format!("{name} (down): record: {e}")))?;
        done.push(name);
    }
    Ok(done)
}

/// Revert every applied pair (`rollback` with the full-history sentinel).
pub async fn reset<'a, A>(acquire: A, root: &Path) -> Result<Vec<String>, AppError>
where
    A: sqlx::Acquire<'a, Database = sqlx::MySql>,
    A::Connection: std::ops::DerefMut<Target = sqlx::MySqlConnection>,
{
    rollback(acquire, root, u32::MAX).await
}
