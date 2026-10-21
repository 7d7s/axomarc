//! The guarded migration runner.
//!
//! MySQL's DDL **auto-commits**: a statement that changes the schema commits
//! the open transaction, so "the migration failed" never means "nothing
//! happened". The existing pair runner already refuses to pretend otherwise
//! (it fails stopped-and-reported, with the note). What it could not do is
//! **resume**: the bookkeeping row is written only after the whole pair
//! succeeds, so a pair that died half-way leaves no trace and the next run
//! re-applies it from the top - which fails on the DDL that already landed.
//!
//! This module adds the two things a production migration needs:
//!
//! 1. **Step-level state.** Every statement of a pair is recorded in
//!    [`STEPS_TABLE`] with a checksum of the statement text. A resumed run
//!    skips the statements that already landed, and **refuses** if the file
//!    changed underneath it (the checksum moved) - because then the recorded
//!    state describes a different migration.
//! 2. **A transaction verdict.** [`auto_commits`] tells whether a statement
//!    implicitly commits, so the runner can say *exactly* which steps of a
//!    pair are outside a transaction instead of a blanket warning.
//!
//! The statement splitter is deliberately conservative: it understands
//! string literals, the three quote styles MySQL accepts, and both comment
//! forms, so a `;` inside a string or a comment never ends a statement.

use kelson_foundation::AppError;
use sqlx::Row;

/// The bookkeeping table for the per-step state.
pub const STEPS_TABLE: &str = "keel_migration_steps";

/// The note every non-transactional failure carries.
pub const NONTRANSACTIONAL_NOTE: &str = "MySQL DDL auto-commits: the statements before this failure remain applied - the next run resumes from the recorded step state";

/// The DDL prefixes that cause an implicit commit in MySQL 8.
///
/// The list is the engine's, not the framework's: `ALTER TABLE`,
/// `CREATE/DROP TABLE`, `CREATE/DROP INDEX`, `RENAME TABLE`,
/// `TRUNCATE TABLE`, `CREATE/DROP DATABASE`, `CREATE/DROP VIEW`,
/// `LOCK/UNLOCK TABLES` and `ANALYZE/OPTIMIZE/REPAIR TABLE`.
const AUTO_COMMIT_PREFIXES: &[&str] = &[
    "ALTER TABLE",
    "ANALYZE TABLE",
    "CREATE DATABASE",
    "CREATE INDEX",
    "CREATE TABLE",
    "CREATE TEMPORARY TABLE",
    "CREATE VIEW",
    "DROP DATABASE",
    "DROP INDEX",
    "DROP TABLE",
    "DROP TEMPORARY TABLE",
    "DROP VIEW",
    "LOCK TABLES",
    "OPTIMIZE TABLE",
    "RENAME TABLE",
    "REPAIR TABLE",
    "TRUNCATE TABLE",
    "UNLOCK TABLES",
];

/// Whether a statement causes an implicit commit in MySQL 8.
///
/// The check is on the statement's **first keyword**: a DML statement
/// (`INSERT`, `UPDATE`, `DELETE`) does not auto-commit, and a `CREATE TABLE`
/// inside a comment or a string is not a statement at all.
pub fn auto_commits(statement: &str) -> bool {
    let trimmed = statement.trim();
    // Skip the leading comments so a commented-out DDL is not mistaken for
    // a real one.
    let mut rest = trimmed;
    loop {
        let lower = rest.to_ascii_lowercase();
        if lower.starts_with("-- ") || lower.starts_with("# ") || lower.starts_with("--\t") {
            match rest.find('\n') {
                Some(i) => rest = rest[i + 1..].trim_start(),
                None => return false,
            }
            continue;
        }
        if lower.starts_with("/*") {
            match rest.find("*/") {
                Some(i) => rest = rest[i + 2..].trim_start(),
                None => return false,
            }
            continue;
        }
        break;
    }
    let upper = rest.to_ascii_uppercase();
    AUTO_COMMIT_PREFIXES
        .iter()
        .any(|prefix| upper.starts_with(prefix))
}

/// Split a migration file into its statements.
///
/// Understands the single-quoted, double-quoted and backtick-quoted forms
/// (with both the `''` and `\'` escapes) and the `--`, `#` and `/* */`
/// comment forms, so a `;` inside a literal or a comment does not end a
/// statement. An empty statement (only whitespace and comments) is dropped.
pub fn split_statements(sql: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::with_capacity(128);
    let bytes: Vec<char> = sql.chars().collect();
    let mut i = 0usize;
    let mut quote: Option<char> = None;
    while i < bytes.len() {
        let c = bytes[i];
        match quote {
            Some(q) => {
                current.push(c);
                if c == '\\' && q != '`' && i + 1 < bytes.len() {
                    // A backslash escape: the next character is literal.
                    current.push(bytes[i + 1]);
                    i += 2;
                    continue;
                }
                if c == q {
                    // A doubled quote is an escaped quote, not the end.
                    if i + 1 < bytes.len() && bytes[i + 1] == q {
                        current.push(q);
                        i += 2;
                        continue;
                    }
                    quote = None;
                }
                i += 1;
            }
            None => {
                if c == '\'' || c == '"' || c == '`' {
                    quote = Some(c);
                    current.push(c);
                    i += 1;
                    continue;
                }
                if c == '#' {
                    // A `#` comment runs to the end of the line.
                    while i < bytes.len() && bytes[i] != '\n' {
                        i += 1;
                    }
                    continue;
                }
                if c == '-' && i + 1 < bytes.len() && bytes[i + 1] == '-' {
                    // A `--` comment needs whitespace after it to be a
                    // comment (MySQL's rule); `--x` is a minus sign.
                    let after = bytes.get(i + 2).copied();
                    if after.is_none() || after == Some(' ') || after == Some('\t') || after == Some('\n') {
                        while i < bytes.len() && bytes[i] != '\n' {
                            i += 1;
                        }
                        continue;
                    }
                }
                if c == '/' && i + 1 < bytes.len() && bytes[i + 1] == '*' {
                    current.push_str("/*");
                    i += 2;
                    while i < bytes.len() {
                        if bytes[i] == '*' && bytes.get(i + 1) == Some(&'/') {
                            current.push_str("*/");
                            i += 2;
                            break;
                        }
                        current.push(bytes[i]);
                        i += 1;
                    }
                    continue;
                }
                if c == ';' {
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        out.push(trimmed.to_string());
                    }
                    current.clear();
                    i += 1;
                    continue;
                }
                current.push(c);
                i += 1;
            }
        }
    }
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
    out
}

/// A stable checksum of a statement's text (FNV-1a, 64 bit).
///
/// Not a cryptographic hash: it detects that a migration file changed
/// between a failed run and its resume, which is the only job it has.
pub fn checksum(statement: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in statement.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The verdict of the guarded runner for one pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairVerdict {
    /// The pair's name.
    pub migration: String,
    /// The statements that were applied by this run.
    pub applied_steps: Vec<usize>,
    /// The statements that were already applied (the resume path).
    pub resumed_steps: Vec<usize>,
    /// The indexes of the statements that auto-commit (outside a
    /// transaction), named so the failure report can point at them.
    pub auto_committing_steps: Vec<usize>,
    /// Whether the whole pair is transactional (no auto-committing step).
    pub transactional: bool,
}

impl PairVerdict {
    /// A one-line summary for a migration report.
    pub fn summary(&self) -> String {
        format!(
            "{}: {} applied, {} resumed, {} auto-committing step(s){}",
            self.migration,
            self.applied_steps.len(),
            self.resumed_steps.len(),
            self.auto_committing_steps.len(),
            if self.transactional {
                " (transactional)"
            } else {
                " (NOT transactional)"
            }
        )
    }
}

/// The per-step state of one pair, read from [`STEPS_TABLE`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRecord {
    /// The statement's index within the pair (0-based).
    pub step: usize,
    /// The checksum of the statement text when it was applied.
    pub checksum: String,
}

/// Read the recorded step state for a pair.
pub async fn step_state<'a, A>(acquire: A, migration: &str) -> Result<Vec<StepRecord>, AppError>
where
    A: sqlx::Acquire<'a, Database = sqlx::MySql>,
    A::Connection: std::ops::DerefMut<Target = sqlx::MySqlConnection>,
{
    let mut conn = acquire
        .acquire()
        .await
        .map_err(|e| AppError::persistence(format!("migration steps: begin read: {e}")))?;
    let rows = sqlx::query(&format!(
        "SELECT step, checksum FROM {STEPS_TABLE} WHERE migration = ? ORDER BY step"
    ))
    .bind(migration)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| AppError::persistence(format!("migration steps: read: {e}")))?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let step: i32 = row
            .try_get(0)
            .map_err(|e| AppError::persistence(format!("migration steps: decode step: {e}")))?;
        let checksum: String = row
            .try_get(1)
            .map_err(|e| AppError::persistence(format!("migration steps: decode checksum: {e}")))?;
        out.push(StepRecord {
            step: usize::try_from(step).unwrap_or(0),
            checksum,
        });
    }
    Ok(out)
}

/// Install the per-step bookkeeping table (idempotent).
pub async fn install_steps<'a, A>(acquire: A) -> Result<(), AppError>
where
    A: sqlx::Acquire<'a, Database = sqlx::MySql>,
    A::Connection: std::ops::DerefMut<Target = sqlx::MySqlConnection>,
{
    let mut conn = acquire
        .acquire()
        .await
        .map_err(|e| AppError::persistence(format!("migration steps: begin install: {e}")))?;
    sqlx::raw_sql(&format!(
        "CREATE TABLE IF NOT EXISTS {STEPS_TABLE} (
            migration VARCHAR(255) NOT NULL,
            step INT NOT NULL,
            checksum CHAR(16) NOT NULL,
            applied_at DATETIME(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
            PRIMARY KEY (migration, step)
        ) ENGINE=InnoDB"
    ))
    .execute(&mut *conn)
    .await
    .map_err(|e| AppError::persistence(format!("migration steps: install: {e}")))?;
    Ok(())
}

/// Apply one pair with the guarded runner: statement by statement, each
/// step recorded, a resume that skips what already landed, and a refusal
/// when the file changed under a recorded step.
///
/// Returns the pair's verdict. On failure the error names the pair, the
/// step that failed and the steps that remain applied.
pub async fn apply_pair_guarded<'a, A>(
    acquire: A,
    migration: &str,
    up_sql: &str,
) -> Result<PairVerdict, AppError>
where
    A: sqlx::Acquire<'a, Database = sqlx::MySql>,
    A::Connection: std::ops::DerefMut<Target = sqlx::MySqlConnection>,
{
    let statements = split_statements(up_sql);
    let mut conn = acquire
        .acquire()
        .await
        .map_err(|e| AppError::persistence(format!("{migration}: begin: {e}")))?;
    install_steps(&mut *conn).await?;

    let recorded = step_state(&mut *conn, migration).await?;
    let mut applied_steps = Vec::new();
    let mut resumed_steps = Vec::new();
    let mut auto_committing_steps = Vec::new();

    for (index, statement) in statements.iter().enumerate() {
        if auto_commits(statement) {
            auto_committing_steps.push(index);
        }
        let digest = checksum(statement);
        // A recorded step with a DIFFERENT checksum means the migration
        // file changed after the failed run: the recorded state describes a
        // different migration, so resuming would apply the wrong thing.
        if let Some(previous) = recorded.iter().find(|r| r.step == index) {
            if previous.checksum != digest {
                return Err(AppError::persistence(format!(
                    "{migration}: step {index} changed since it was applied (the file was edited after a failed run); \
                     inspect the schema and reconcile by hand - refusing to resume"
                )));
            }
            resumed_steps.push(index);
            continue;
        }
        // A step beyond the recorded ones must not run: the earlier ones
        // are missing (the record was edited or the pair was rolled back
        // by hand).
        if recorded.iter().any(|r| r.step > index) {
            let later = recorded
                .iter()
                .filter(|r| r.step > index)
                .map(|r| r.step)
                .collect::<Vec<_>>();
            return Err(AppError::persistence(format!(
                "{migration}: step {index} is missing from the recorded state while step {later:?} is \
                 recorded; the state was edited by hand - refusing to apply out of order"
            )));
        }
        // The summary of the auto-committing steps is computed before the
        // statement runs, so the failure report can name them.
        let committed_steps = auto_committing_steps.clone();
        sqlx::raw_sql(statement)
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                let committed = if committed_steps.is_empty() {
                    String::new()
                } else {
                    format!(
                        "; the auto-committing steps {} remain applied",
                        committed_steps
                            .iter()
                            .map(usize::to_string)
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                AppError::persistence(format!(
                    "{migration}: step {index} failed: {e} ({NONTRANSACTIONAL_NOTE}{committed})"
                ))
            })?;
        sqlx::query(&format!(
            "INSERT INTO {STEPS_TABLE} (migration, step, checksum) VALUES (?, ?, ?)"
        ))
        .bind(migration)
        .bind(index as i32)
        .bind(&digest)
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            AppError::persistence(format!(
                "{migration}: step {index} applied but its state could not be recorded: {e} \
                 (the next run will re-apply this step)"
            ))
        })?;
        applied_steps.push(index);
    }

    let transactional = auto_committing_steps.is_empty();
    Ok(PairVerdict {
        migration: migration.to_string(),
        applied_steps,
        resumed_steps,
        auto_committing_steps,
        transactional,
    })
}

/// Forget the recorded step state for a pair (used by a rollback, which
/// reverses the pair's statements).
pub async fn clear_steps<'a, A>(acquire: A, migration: &str) -> Result<(), AppError>
where
    A: sqlx::Acquire<'a, Database = sqlx::MySql>,
    A::Connection: std::ops::DerefMut<Target = sqlx::MySqlConnection>,
{
    let mut conn = acquire
        .acquire()
        .await
        .map_err(|e| AppError::persistence(format!("migration steps: begin clear: {e}")))?;
    sqlx::query(&format!("DELETE FROM {STEPS_TABLE} WHERE migration = ?"))
        .bind(migration)
        .execute(&mut *conn)
        .await
        .map_err(|e| AppError::persistence(format!("migration steps: clear: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_splitter_respects_the_literals() {
        let sql = "INSERT INTO t (a) VALUES ('x;y'); SELECT 1;";
        let parts = split_statements(sql);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "INSERT INTO t (a) VALUES ('x;y')");
        assert_eq!(parts[1], "SELECT 1");
    }

    #[test]
    fn the_splitter_respects_the_backticks_and_the_comments() {
        let sql = "CREATE TABLE `a;b` (x INT); -- a comment; with a semicolon\nSELECT 1;";
        let parts = split_statements(sql);
        assert_eq!(parts.len(), 2);
        assert!(parts[0].starts_with("CREATE TABLE `a;b`"));
        assert_eq!(parts[1], "SELECT 1");
    }

    #[test]
    fn the_splitter_respects_the_escaped_quotes() {
        let sql = "INSERT INTO t VALUES ('it\\'s; fine'); SELECT 2;";
        let parts = split_statements(sql);
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "INSERT INTO t VALUES ('it\\'s; fine')");
    }

    #[test]
    fn the_splitter_drops_the_empty_statements() {
        assert!(split_statements("").is_empty());
        assert!(split_statements("  \n\n ").is_empty());
        assert!(split_statements(";;;").is_empty());
        assert_eq!(split_statements("SELECT 1;;SELECT 2;").len(), 2);
    }

    #[test]
    fn the_auto_commit_verdict_names_the_ddl() {
        assert!(auto_commits("ALTER TABLE t ADD COLUMN x INT"));
        assert!(auto_commits("CREATE TABLE t (x INT)"));
        assert!(auto_commits("  create index i on t (x)"));
        assert!(auto_commits("TRUNCATE TABLE t"));
        assert!(auto_commits("-- a comment\nDROP TABLE t"));
        assert!(!auto_commits("INSERT INTO t VALUES (1)"));
        assert!(!auto_commits("UPDATE t SET x = 1"));
        assert!(!auto_commits("DELETE FROM t"));
        assert!(!auto_commits("SELECT 1"));
        assert!(!auto_commits("-- CREATE TABLE t (x INT)"));
    }

    #[test]
    fn the_checksum_is_stable_and_discriminating() {
        assert_eq!(checksum("SELECT 1"), checksum("SELECT 1"));
        assert_ne!(checksum("SELECT 1"), checksum("SELECT 2"));
        assert_eq!(checksum("SELECT 1").len(), 16);
    }

    #[test]
    fn the_verdict_summarises_the_transaction_shape() {
        let v = PairVerdict {
            migration: "20260101_x".to_string(),
            applied_steps: vec![0, 1],
            resumed_steps: vec![],
            auto_committing_steps: vec![1],
            transactional: false,
        };
        assert!(v.summary().contains("NOT transactional"));
        assert!(v.summary().contains("2 applied"));
    }
}
