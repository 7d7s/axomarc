//! The online schema change.
//!
//! A MySQL `ALTER TABLE` on a live table takes a metadata lock and blocks
//! the table for the duration of the rebuild - on InnoDB that means the
//! table is unreadable while the copy runs, which on a hot table is a
//! production incident. Laravel concedes the problem (there is no online
//! DDL in the migrator) and Laravel's ` doctrine/dbal` "rename and copy"
//! path still takes the lock.
//!
//! This module applies a change the way the safe tools do: **build the new
//! shape beside the old one, copy the rows, then swap the names in one
//! atomic statement**.
//!
//! 1. `CREATE TABLE _kelson_new_<t> LIKE <t>` - the same structure, a new
//!    name.
//! 2. Apply the change to the shadow table.
//! 3. Copy the rows in chunks, ordered by the key, so the copy is
//!    resumable and never holds a big transaction.
//! 4. Verify the row counts agree.
//! 5. `RENAME TABLE <t> TO _kelson_old_<t>, _kelson_new_<t> TO <t>` -
//!    **one statement**, so the swap is atomic: a reader sees either the
//!    old table or the new one, never neither.
//! 6. Drop the old table.
//!
//! The lock is therefore held only for the rename (milliseconds), not for
//! the copy (which can take hours). The cost is the disk space for the
//! shadow copy and the write amplification of the copy itself - which is
//! the trade every online-schema-change tool makes.
//!
//! The engine's **instant** DDL is the cheaper path when the change
//! qualifies (adding a column with a default, extending a VARCHAR, adding
//! an index). [`apply_instant_when_possible`] tries it first and falls back
//! to the copy path, reporting which one ran.

use kelson_foundation::AppError;
use sqlx::Row;

/// The prefix of the shadow table.
pub const SHADOW_PREFIX: &str = "_kelson_new_";
/// The prefix of the retired table (kept briefly, dropped at the end).
pub const RETIRED_PREFIX: &str = "_kelson_old_";

/// What an online change did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnlineReport {
    /// The table that was changed.
    pub table: String,
    /// The path that ran.
    pub strategy: Strategy,
    /// The rows copied.
    pub rows_copied: u64,
    /// The rows the source held (the verification).
    pub source_rows: u64,
    /// The rows the shadow holds after the copy (the verification).
    pub shadow_rows: u64,
    /// Whether the swap happened.
    pub swapped: bool,
}

/// How the change was applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strategy {
    /// The engine's instant DDL did it (no copy, a brief lock).
    Instant,
    /// The shadow-table copy did it.
    Copy,
}

impl Strategy {
    /// The report spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Strategy::Instant => "instant",
            Strategy::Copy => "copy",
        }
    }
}

/// A change to apply to a table.
#[derive(Debug, Clone)]
pub struct OnlineChange {
    /// The table to change.
    pub table: String,
    /// The DDL that produces the new shape (applied to the shadow table).
    ///
    /// For example `ADD COLUMN email VARCHAR(255) NOT NULL DEFAULT ''`.
    pub alter: String,
    /// The column the copy is ordered by (the primary key, or any unique
    /// indexed column).
    pub key: String,
    /// How many rows one copy statement moves.
    pub chunk: u64,
}

impl OnlineChange {
    /// A change with the default chunk size.
    pub fn new(table: impl Into<String>, alter: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            table: table.into(),
            alter: alter.into(),
            key: key.into(),
            chunk: 1_000,
        }
    }

    /// Set the copy chunk size.
    pub fn chunk(mut self, chunk: u64) -> Self {
        self.chunk = chunk.max(1);
        self
    }

    fn shadow(&self) -> String {
        format!("{SHADOW_PREFIX}{}", self.table)
    }

    fn retired(&self) -> String {
        format!("{RETIRED_PREFIX}{}", self.table)
    }
}

/// Whether a change qualifies for the engine's instant DDL.
///
/// MySQL 8.0's `ALGORITHM=INSTANT` covers exactly one shape: **adding a
/// column at the end**. Extending a `VARCHAR` and adding an index are
/// `INPLACE` (they rebuild the table under a lock that permits concurrent
/// reads but not writes), and everything else is a copy. The conservative
/// rule here is the safe subset - anything else takes the copy path, which
/// is always correct.
pub fn qualifies_for_instant(alter: &str) -> bool {
    let upper = alter.trim().to_ascii_uppercase();
    upper.starts_with("ADD COLUMN") || upper.starts_with("ADD  COLUMN")
}

/// Apply a change, preferring the engine's instant DDL and falling back to
/// the shadow-table copy.
pub async fn apply_online<'a, A>(
    acquire: A,
    change: &OnlineChange,
) -> Result<OnlineReport, AppError>
where
    A: sqlx::Acquire<'a, Database = sqlx::MySql>,
    A::Connection: std::ops::DerefMut<Target = sqlx::MySqlConnection>,
{
    let mut conn = acquire
        .acquire()
        .await
        .map_err(|e| AppError::persistence(format!("online change: begin: {e}")))?;

    // The engine's instant path: the lock is a metadata lock for the
    // rebuild only, and the table stays readable.
    if qualifies_for_instant(&change.alter) {
        let statement = format!(
            "ALTER TABLE {} {}, ALGORITHM=INSTANT",
            change.table, change.alter
        );
        match sqlx::raw_sql(&statement).execute(&mut *conn).await {
            Ok(_) => {
                return Ok(OnlineReport {
                    table: change.table.clone(),
                    strategy: Strategy::Instant,
                    rows_copied: 0,
                    source_rows: 0,
                    shadow_rows: 0,
                    swapped: true,
                });
            }
            Err(e) => {
                // The engine refused the instant path; fall through to the
                // copy path, which is always correct.
                eprintln!(
                    "online change: the instant algorithm was refused for {} ({e}); using the copy path",
                    change.table
                );
            }
        }
    }

    let shadow = change.shadow();
    let retired = change.retired();

    // 1. the shadow table, with the same structure.
    sqlx::raw_sql(&format!(
        "DROP TABLE IF EXISTS {shadow}"
    ))
    .execute(&mut *conn)
    .await
    .map_err(|e| AppError::persistence(format!("online change: drop the stale shadow: {e}")))?;
    sqlx::raw_sql(&format!(
        "CREATE TABLE {shadow} LIKE {}",
        change.table
    ))
    .execute(&mut *conn)
    .await
    .map_err(|e| AppError::persistence(format!("online change: create the shadow: {e}")))?;

    // 2. the change, applied to the shadow.
    sqlx::raw_sql(&format!("ALTER TABLE {shadow} {}", change.alter))
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            AppError::persistence(format!(
                "online change: alter the shadow table {shadow}: {e} (the original table is untouched)"
            ))
        })?;

    // 3. the copy, chunked and ordered by the key.
    let columns: Vec<String> = column_names(&mut *conn, &change.table).await?;
    if columns.is_empty() {
        return Err(AppError::persistence(format!(
            "online change: the table {} has no columns",
            change.table
        )));
    }
    let column_list = columns.join(", ");
    let source_rows = count_rows(&mut *conn, &change.table).await?;
    let mut copied: u64 = 0;
    let mut last_key: Option<String> = None;
    loop {
        let statement = match &last_key {
            Some(_) => format!(
                "INSERT INTO {shadow} ({column_list}) SELECT {column_list} FROM {} WHERE {} > ? ORDER BY {} LIMIT {}",
                change.table, change.key, change.key, change.chunk
            ),
            None => format!(
                "INSERT INTO {shadow} ({column_list}) SELECT {column_list} FROM {} ORDER BY {} LIMIT {}",
                change.table, change.key, change.chunk
            ),
        };
        let affected = match &last_key {
            Some(last) => {
                sqlx::query(&statement)
                    .bind(last)
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| AppError::persistence(format!("online change: copy: {e}")))?
                    .rows_affected()
            }
            None => sqlx::raw_sql(&statement)
                .execute(&mut *conn)
                .await
                .map_err(|e| AppError::persistence(format!("online change: copy: {e}")))?
                .rows_affected(),
        };
        if affected == 0 {
            break;
        }
        copied += affected;
        if affected < change.chunk {
            break;
        }
        // The resume point is the last COPIED key - the shadow's maximum,
        // not the source's (the source still holds every row, so its
        // maximum would skip the rest of the copy).
        last_key = Some(max_key(&mut *conn, &shadow, &change.key).await?);
    }

    // 4. the verification: the shadow must hold exactly what the source held.
    let shadow_rows = count_rows(&mut *conn, &shadow).await?;
    if shadow_rows != source_rows {
        return Err(AppError::persistence(format!(
            "online change: the shadow holds {shadow_rows} rows but the source holds {source_rows}; \
             refusing to swap (the original table is untouched)"
        )));
    }

    // 5. the atomic swap: ONE statement, so a reader never sees a missing
    // table.
    sqlx::raw_sql(&format!(
        "RENAME TABLE {} TO {retired}, {shadow} TO {}",
        change.table, change.table
    ))
    .execute(&mut *conn)
    .await
    .map_err(|e| {
        AppError::persistence(format!(
            "online change: the swap failed: {e} (the original table is untouched; the shadow {shadow} holds the copy)"
        ))
    })?;

    // 6. the retired table goes away.
    sqlx::raw_sql(&format!("DROP TABLE IF EXISTS {retired}"))
        .execute(&mut *conn)
        .await
        .map_err(|e| AppError::persistence(format!("online change: drop the retired table: {e}")))?;

    Ok(OnlineReport {
        table: change.table.clone(),
        strategy: Strategy::Copy,
        rows_copied: copied,
        source_rows,
        shadow_rows,
        swapped: true,
    })
}

/// The column names of a table, in declaration order.
async fn column_names(
    conn: &mut sqlx::MySqlConnection,
    table: &str,
) -> Result<Vec<String>, AppError> {
    let rows = sqlx::query(
        "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? ORDER BY ORDINAL_POSITION",
    )
    .bind(table)
    .fetch_all(conn)
    .await
    .map_err(|e| AppError::persistence(format!("online change: read the columns: {e}")))?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        out.push(
            row.try_get::<String, _>(0)
                .map_err(|e| AppError::persistence(format!("online change: decode the column: {e}")))?,
        );
    }
    Ok(out)
}

/// The primary key of a table (the column the copy orders by).
///
/// The copy needs a unique, indexed column to resume from; the primary key
/// is that column. A table without one cannot be copied safely (the copy
/// would not be resumable), so it is refused.
pub async fn primary_key_of(
    conn: &mut sqlx::MySqlConnection,
    table: &str,
) -> Result<String, AppError> {
    let rows = sqlx::query(
        "SELECT COLUMN_NAME FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE
         WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? AND CONSTRAINT_NAME = 'PRIMARY'
         ORDER BY ORDINAL_POSITION LIMIT 1",
    )
    .bind(table)
    .fetch_all(conn)
    .await
    .map_err(|e| AppError::persistence(format!("online change: read the primary key: {e}")))?;
    let key: String = rows
        .first()
        .map(|row| {
            row.try_get::<String, _>(0)
                .map_err(|e| AppError::persistence(format!("online change: decode the key: {e}")))
        })
        .transpose()?
        .unwrap_or_default();
    if key.is_empty() {
        return Err(AppError::persistence(format!(
            "online change: the table {table} has no primary key, so the copy cannot be resumed \
             safely; refusing"
        )));
    }
    Ok(key)
}

/// How many rows a table holds.
async fn count_rows(
    conn: &mut sqlx::MySqlConnection,
    table: &str,
) -> Result<u64, AppError> {
    let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
        .fetch_one(conn)
        .await
        .map_err(|e| AppError::persistence(format!("online change: count: {e}")))?;
    Ok(count.max(0) as u64)
}

/// The largest value of the key column (the copy's resume point).
async fn max_key(
    conn: &mut sqlx::MySqlConnection,
    table: &str,
    key: &str,
) -> Result<String, AppError> {
    // The key can be any column type (an INT id, a CHAR uuid, a DATETIME),
    // so the resume point is CAST to text: the comparison in the copy is
    // then a text comparison against the same column, which MySQL coerces
    // back to the column's type.
    let value: String = sqlx::query_scalar(&format!("SELECT CAST(MAX({key}) AS CHAR) FROM {table}"))
        .fetch_one(conn)
        .await
        .map_err(|e| AppError::persistence(format!("online change: read the resume point: {e}")))?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_instant_rule_covers_the_safe_subset() {
        // Only "add a column" is INSTANT in MySQL 8.0. Extending a VARCHAR
        // and adding an index are INPLACE (they rebuild under a lock that
        // permits reads but not writes), so they take the copy path.
        assert!(qualifies_for_instant("ADD COLUMN email VARCHAR(255) NOT NULL DEFAULT ''"));
        assert!(qualifies_for_instant("  add column x INT"));
        assert!(!qualifies_for_instant("ADD INDEX idx (x)"));
        assert!(!qualifies_for_instant("MODIFY COLUMN email VARCHAR(512)"));
        assert!(!qualifies_for_instant("DROP COLUMN email"));
        assert!(!qualifies_for_instant("MODIFY COLUMN amount BIGINT"));
    }

    #[test]
    fn the_shadow_names_are_derived_from_the_table() {
        let change = OnlineChange::new("appointments", "ADD COLUMN x INT", "id");
        assert_eq!(change.shadow(), "_kelson_new_appointments");
        assert_eq!(change.retired(), "_kelson_old_appointments");
    }
}
