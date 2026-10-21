//! `EXPLAIN ANALYZE` - the query plan, executed.
//!
//! MySQL 8.0.18+ can EXPLAIN **and run** a statement, returning the plan
//! the optimiser actually chose with the real row counts and timings. That
//! is a capability Laravel's query builder does not surface: `EXPLAIN` in
//! Laravel is a `DB::select('EXPLAIN ...')` string, and MySQL's plain
//! `EXPLAIN` returns the *estimated* plan, not the executed one.
//!
//! Here the plan is a typed value, and the index check is a method - so a
//! CI gate can assert "the hot query uses its index" instead of a human
//! reading a plan.

use kelson_foundation::AppError;
use kelson_query::Value as Bind;
use sqlx::mysql::MySqlPool;
use sqlx::Row;

/// The executed plan of a statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplainPlan {
    /// The statement that was explained.
    pub sql: String,
    /// The plan lines, as the server returned them.
    pub lines: Vec<String>,
}

impl ExplainPlan {
    /// The plan as one text block.
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    /// Whether the executed plan used the named index.
    ///
    /// The check is deliberately conservative: it looks for the index name
    /// as a word in the plan. A plan that says `Using index` (a covering
    /// index) does **not** name the index, so the caller should pass the
    /// index name it expects to see.
    pub fn uses_index(&self, index: &str) -> bool {
        self.lines
            .iter()
            .any(|line| line.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').any(|w| w == index))
    }

    /// Whether the plan used a full table scan (`ALL` in the plan's type
    /// column). A gate can refuse a query that silently stopped using its
    /// index.
    pub fn is_table_scan(&self) -> bool {
        self.lines.iter().any(|line| line.contains("type: ALL"))
    }
}

/// Explain (and execute) a statement, returning the plan the optimiser
/// actually used.
///
/// The statement is rendered for MySQL by the caller (`kelson_query::MYSQL`),
/// so the plan describes the SQL that will really run.
pub async fn explain_analyze(
    pool: &MySqlPool,
    sql: &str,
    params: &[Bind],
) -> Result<ExplainPlan, AppError> {
    let statement = format!("EXPLAIN ANALYZE {sql}");
    let mut query = sqlx::query(&statement);
    for p in params {
        query = match p {
            Bind::Null => query.bind(None::<String>),
            Bind::Bool(b) => query.bind(b),
            Bind::Int(i) => query.bind(i),
            Bind::Float(f) => query.bind(f),
            Bind::Text(t) => query.bind(t),
            Bind::Uuid(u) => query.bind(u),
            Bind::Timestamp(ts) => query.bind(ts),
            Bind::Json(j) => query.bind(j),
        };
    }
    let rows = query
        .fetch_all(pool)
        .await
        .map_err(|e| AppError::persistence(format!("explain analyze: {e}")))?;
    let mut lines = Vec::with_capacity(rows.len());
    for row in rows {
        let line: String = row
            .try_get::<String, _>(0)
            .map_err(|e| AppError::persistence(format!("explain analyze: decode: {e}")))?;
        lines.push(line);
    }
    Ok(ExplainPlan {
        sql: sql.to_string(),
        lines,
    })
}
