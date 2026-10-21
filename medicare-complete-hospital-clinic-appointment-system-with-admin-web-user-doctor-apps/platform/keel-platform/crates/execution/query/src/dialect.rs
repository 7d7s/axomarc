//! The SQL dialect layer.
//!
//! The query builder used to emit **Postgres** SQL unconditionally: `$N`
//! placeholders, `::date` casts, `TO_CHAR`, `ILIKE`, `ON CONFLICT ... DO
//! UPDATE`, `RETURNING`. The MySQL adapter then rewrote the finished SQL
//! **textually** (`to_mysql_upsert`, `qmarks`) — a string rewrite of
//! generated SQL, which is exactly the kind of fragility that hides a
//! divergence until it bites in production.
//!
//! This module makes the dialect an explicit, first-class input:
//!
//! * a [`Dialect`] owns every construct that differs between engines
//!   (placeholders, casts, date parts, case-insensitive matching, the
//!   upsert form, `RETURNING`, the row-lock clause);
//! * the builder renders **dialect-neutral intent** through the trait, so a
//!   Postgres-only construct can never reach MySQL;
//! * the compiled statement carries the [`DialectId`] it was rendered for,
//!   so an adapter can **fail closed** when a statement was built for the
//!   wrong engine instead of sending it and reading the server's error.
//!
//! The Postgres rendering is unchanged, byte for byte — the golden
//! catalogue in `tests/golden_postgres.txt` is the proof.

use crate::insert::ConflictPolicy;
use crate::select::{DatePart, Ident, Lock};
use serde::{Deserialize, Serialize};

/// Which engine a statement was rendered for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DialectId {
    Postgres,
    MySql,
}

impl DialectId {
    /// The `[database].driver` spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            DialectId::Postgres => "postgres",
            DialectId::MySql => "mysql",
        }
    }
}

impl std::fmt::Display for DialectId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The two parts of a conflict clause: the row alias that must follow the
/// `VALUES` list, and the trailing clause itself.
///
/// MySQL 8.0.19+ needs `VALUES (...) AS new ON DUPLICATE KEY UPDATE c =
/// new.c` — the alias sits *inside* the statement, right after the values,
/// while the clause sits at the end. Postgres needs neither the alias nor
/// the position: `ON CONFLICT (t) DO UPDATE SET c = EXCLUDED.c`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UpsertRendering {
    /// Appended after the `VALUES (...)` list (MySQL's `AS new`).
    pub alias: Option<String>,
    /// Appended at the end of the statement.
    pub clause: Option<String>,
}

/// A SQL dialect.
///
/// Implementations are zero-sized and cheap to clone; the crate ships
/// [`PostgresDialect`] and [`MySqlDialect`] as unit structs.
pub trait Dialect: Send + Sync {
    /// Which engine this dialect renders for.
    fn id(&self) -> DialectId;

    /// The bind placeholder for the `index`-th parameter (1-based).
    ///
    /// Postgres numbers its placeholders (`$1`); MySQL uses a bare `?` and
    /// binds positionally. The **order** of the binds is identical in both
    /// engines, so the parameter vector is dialect-independent.
    fn placeholder(&self, index: usize) -> String;

    /// The case-insensitive match operator.
    ///
    /// Postgres has `ILIKE`. MySQL has no `ILIKE`: its `LIKE` is already
    /// case-insensitive under the default `utf8mb4_*_ci` collation, so the
    /// case-insensitive match renders as `LIKE`. **The semantics are not
    /// identical** — on MySQL the collation decides, and a `_bin` or `_cs`
    /// collation makes `LIKE` case-sensitive again. The divergence is
    /// recorded in the Laravel-parity map.
    fn ilike_operator(&self) -> &'static str;

    /// Render a date-part comparison (`column`'s DATE / YEAR / MONTH / DAY /
    /// TIME part against `placeholder`).
    fn render_date_part(&self, part: DatePart, column: &str, placeholder: &str) -> String;

    /// Whether the engine can return the written rows from the statement
    /// (`RETURNING`).
    ///
    /// MySQL has no `RETURNING`; the MySQL adapter reads the row back by
    /// key on the same connection instead. The builder **refuses** a
    /// `returning` on a dialect that cannot honour it, rather than
    /// silently dropping it.
    fn supports_returning(&self) -> bool;

    /// Render the row-lock clause.
    fn render_lock(&self, lock: Lock) -> String;

    /// Render the conflict clause for an upsert.
    ///
    /// `target` is the conflict target (empty means "any violation" on
    /// Postgres). `set_columns` are the columns the `DO UPDATE` assigns.
    fn render_upsert(
        &self,
        policy: ConflictPolicy,
        target: &[Ident],
        set_columns: &[Ident],
    ) -> UpsertRendering;

    /// Quote an identifier for a DDL statement.
    ///
    /// The builder's own identifiers are validated to `[a-z0-9_.]`, so they
    /// need no quoting; the DDL layer (the schema builder) accepts free-form
    /// names and quotes them per engine.
    fn quote_ident(&self, ident: &str) -> String;
}

/// The Postgres dialect. The default: the builder's historical rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PostgresDialect;

impl Dialect for PostgresDialect {
    fn id(&self) -> DialectId {
        DialectId::Postgres
    }

    fn placeholder(&self, index: usize) -> String {
        format!("${index}")
    }

    fn ilike_operator(&self) -> &'static str {
        "ILIKE"
    }

    fn render_date_part(&self, part: DatePart, column: &str, placeholder: &str) -> String {
        part.render_postgres(column, placeholder)
    }

    fn supports_returning(&self) -> bool {
        true
    }

    fn render_lock(&self, lock: Lock) -> String {
        match lock {
            Lock::Update => "FOR UPDATE".to_string(),
            Lock::Share => "FOR SHARE".to_string(),
        }
    }

    fn render_upsert(
        &self,
        policy: ConflictPolicy,
        target: &[Ident],
        set_columns: &[Ident],
    ) -> UpsertRendering {
        let clause = match policy {
            ConflictPolicy::DoNothing => {
                if target.is_empty() {
                    "ON CONFLICT DO NOTHING".to_string()
                } else {
                    let names: Vec<&str> = target.iter().map(|c| c.as_str()).collect();
                    format!("ON CONFLICT ({}) DO NOTHING", names.join(", "))
                }
            }
            ConflictPolicy::DoUpdate => {
                let target_sql = if target.is_empty() {
                    String::new()
                } else {
                    let names: Vec<&str> = target.iter().map(|c| c.as_str()).collect();
                    format!(" ({})", names.join(", "))
                };
                let assigns: Vec<String> = set_columns
                    .iter()
                    .map(|c| format!("{} = EXCLUDED.{}", c.as_str(), c.as_str()))
                    .collect();
                format!("ON CONFLICT{target_sql} DO UPDATE SET {}", assigns.join(", "))
            }
        };
        UpsertRendering {
            alias: None,
            clause: Some(clause),
        }
    }

    fn quote_ident(&self, ident: &str) -> String {
        format!("\"{ident}\"")
    }
}

/// The MySQL 8 dialect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MySqlDialect;

impl Dialect for MySqlDialect {
    fn id(&self) -> DialectId {
        DialectId::MySql
    }

    fn placeholder(&self, _index: usize) -> String {
        "?".to_string()
    }

    fn ilike_operator(&self) -> &'static str {
        // MySQL has no ILIKE: LIKE is already case-insensitive under the
        // default collation.
        "LIKE"
    }

    fn render_date_part(&self, part: DatePart, column: &str, placeholder: &str) -> String {
        match part {
            // MySQL has no `::date` cast; DATE() is the equivalent.
            DatePart::Date => format!("DATE({column}) = CAST({placeholder} AS DATE)"),
            // EXTRACT is standard SQL and MySQL 8 supports it.
            DatePart::Year => format!("EXTRACT(YEAR FROM {column}) = {placeholder}"),
            DatePart::Month => format!("EXTRACT(MONTH FROM {column}) = {placeholder}"),
            DatePart::Day => format!("EXTRACT(DAY FROM {column}) = {placeholder}"),
            // TO_CHAR is Postgres; MySQL's DATE_FORMAT is the equivalent.
            DatePart::Time => format!("DATE_FORMAT({column}, '%H:%i:%s') = {placeholder}"),
        }
    }

    fn supports_returning(&self) -> bool {
        false
    }

    fn render_lock(&self, lock: Lock) -> String {
        match lock {
            Lock::Update => "FOR UPDATE".to_string(),
            // MySQL spells the shared lock `LOCK IN SHARE MODE`; `FOR SHARE`
            // landed in 8.0 and is the modern spelling.
            Lock::Share => "FOR SHARE".to_string(),
        }
    }

    fn render_upsert(
        &self,
        policy: ConflictPolicy,
        _target: &[Ident],
        set_columns: &[Ident],
    ) -> UpsertRendering {
        match policy {
            // MySQL's `INSERT IGNORE` would also swallow truncation and
            // constraint errors, not only duplicate keys — a wider failure
            // surface than Postgres's `DO NOTHING`. The MySQL adapter runs a
            // plain INSERT and maps ONLY the duplicate-key error to
            // "skipped", which keeps the Postgres semantics.
            ConflictPolicy::DoNothing => UpsertRendering::default(),
            ConflictPolicy::DoUpdate => {
                // The conflict target is not expressible: ANY unique key of
                // the table fires the merge. The row alias (`AS new`) is the
                // MySQL 8.0.19+ replacement for `EXCLUDED`.
                let assigns: Vec<String> = set_columns
                    .iter()
                    .map(|c| format!("{} = new.{}", c.as_str(), c.as_str()))
                    .collect();
                UpsertRendering {
                    alias: Some(" AS new".to_string()),
                    clause: Some(format!("ON DUPLICATE KEY UPDATE {}", assigns.join(", "))),
                }
            }
        }
    }

    fn quote_ident(&self, ident: &str) -> String {
        format!("`{ident}`")
    }
}

/// The Postgres dialect (a shared instance).
pub static POSTGRES: PostgresDialect = PostgresDialect;
/// The MySQL 8 dialect (a shared instance).
pub static MYSQL: MySqlDialect = MySqlDialect;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_placeholders_differ_but_the_order_does_not() {
        assert_eq!(POSTGRES.placeholder(1), "$1");
        assert_eq!(POSTGRES.placeholder(3), "$3");
        assert_eq!(MYSQL.placeholder(1), "?");
        assert_eq!(MYSQL.placeholder(3), "?");
    }

    #[test]
    fn the_ids_name_their_engine() {
        assert_eq!(POSTGRES.id(), DialectId::Postgres);
        assert_eq!(MYSQL.id(), DialectId::MySql);
        assert_eq!(DialectId::MySql.as_str(), "mysql");
    }

    #[test]
    fn the_case_insensitive_match_renders_per_engine() {
        assert_eq!(POSTGRES.ilike_operator(), "ILIKE");
        assert_eq!(MYSQL.ilike_operator(), "LIKE");
    }

    #[test]
    fn the_date_parts_render_per_engine() {
        let ph = POSTGRES.placeholder(1);
        assert_eq!(
            POSTGRES.render_date_part(DatePart::Date, "created_at", &ph),
            "created_at::date = $1::date"
        );
        assert_eq!(
            MYSQL.render_date_part(DatePart::Date, "created_at", "?"),
            "DATE(created_at) = CAST(? AS DATE)"
        );
        assert_eq!(
            POSTGRES.render_date_part(DatePart::Time, "created_at", &ph),
            "TO_CHAR(created_at, 'HH24:MI:SS') = $1"
        );
        assert_eq!(
            MYSQL.render_date_part(DatePart::Time, "created_at", "?"),
            "DATE_FORMAT(created_at, '%H:%i:%s') = ?"
        );
    }

    #[test]
    fn the_returning_support_differs() {
        assert!(POSTGRES.supports_returning());
        assert!(!MYSQL.supports_returning());
    }

    #[test]
    fn the_upsert_renders_per_engine() {
        let pg = POSTGRES.render_upsert(
            ConflictPolicy::DoUpdate,
            &[Ident::new("email").unwrap()],
            &[Ident::new("id").unwrap()],
        );
        assert_eq!(pg.alias, None);
        assert_eq!(
            pg.clause.as_deref(),
            Some("ON CONFLICT (email) DO UPDATE SET id = EXCLUDED.id")
        );

        let my = MYSQL.render_upsert(
            ConflictPolicy::DoUpdate,
            &[Ident::new("email").unwrap()],
            &[Ident::new("id").unwrap()],
        );
        assert_eq!(my.alias.as_deref(), Some(" AS new"));
        assert_eq!(
            my.clause.as_deref(),
            Some("ON DUPLICATE KEY UPDATE id = new.id")
        );
    }

    #[test]
    fn the_do_nothing_upsert_differs() {
        let pg = POSTGRES.render_upsert(ConflictPolicy::DoNothing, &[], &[]);
        assert_eq!(pg.clause.as_deref(), Some("ON CONFLICT DO NOTHING"));
        let my = MYSQL.render_upsert(ConflictPolicy::DoNothing, &[], &[]);
        assert_eq!(my.clause, None);
        assert_eq!(my.alias, None);
    }

    #[test]
    fn the_identifier_quoting_differs() {
        assert_eq!(POSTGRES.quote_ident("users"), "\"users\"");
        assert_eq!(MYSQL.quote_ident("users"), "`users`");
    }
}
