//! # keel-query
//! Read side of the three data-access modes (11-SPEC §5): `ReadProjection` for
//! operational reads, `ReportingQuery` for analytical reads. No write transactions.
//! The typed SELECT builder (L14) generates parameterized SQL without executing.

use keel_foundation::AppError;
use serde::{Deserialize, Serialize};

pub mod delete;
pub mod insert;
pub mod select;
pub mod truncate;
pub mod update;

pub use delete::Delete;
pub use insert::{ConflictPolicy, Insert};
pub use select::{
    Aggregate, Clause, CompiledSelect, Condition, DatePart, Direction, Ident, Junction, Lock,
    Operator, Select, Value,
};
pub use truncate::Truncate;
pub use update::Update;

/// Compiled WRITE statement (INSERT/UPDATE/DELETE): SQL text with `$N`
/// placeholders and bind values in matching order. Execution is owned by the
/// data class, exactly like [`CompiledSelect`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CompiledWrite {
    pub sql: String,
    pub params: Vec<Value>,
}

/// Rendering context for a clause list: aggregate comparisons
/// ([`select::Clause::AggregateCmp`]) are HAVING-only — a `COUNT(*) > n`
/// predicate in WHERE is invalid SQL and fails closed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClauseContext {
    Where,
    Having,
}

/// Shared WHERE/HAVING rendering for the SELECT/UPDATE/DELETE builders: a
/// flat list of junction-tagged [`select::Clause`]s joined left-to-right (the
/// first clause's junction is implicit). Appends binds to `params` in SQL
/// order — including binds inside nested groups and composed subqueries — and
/// returns the predicate text. No free-form SQL: every leaf goes through
/// [`select::Ident`] validation and `$N` binds.
pub(crate) fn render_conditions(
    items: &[(select::Junction, select::Clause)],
    params: &mut Vec<Value>,
    context: ClauseContext,
) -> Result<String, AppError> {
    let mut out = String::new();
    for (index, (junction, clause)) in items.iter().enumerate() {
        if index > 0 {
            out.push_str(match junction {
                select::Junction::And => " AND ",
                select::Junction::Or => " OR ",
            });
        }
        out.push_str(&render_clause(clause, params, context)?);
    }
    Ok(out)
}

/// Recursive clause-tree rendering. Subqueries render into the same `params`
/// vector, so `$N` numbering stays continuous with the enclosing statement.
fn render_clause(
    clause: &select::Clause,
    params: &mut Vec<Value>,
    context: ClauseContext,
) -> Result<String, AppError> {
    use select::{Clause, Operator};
    match clause {
        Clause::Pred(cond) => render_predicate(cond, params),
        Clause::Column { left, operator, right } => {
            if !operator.is_scalar() {
                return Err(AppError::validation(format!(
                    "column comparison does not accept operator {}",
                    operator.sql()
                )));
            }
            Ok(format!(
                "{} {} {}",
                left.as_str(),
                operator.sql(),
                right.as_str()
            ))
        }
        Clause::Row {
            columns,
            operator,
            values,
        } => {
            if columns.is_empty() {
                return Err(AppError::validation(
                    "row comparison needs at least one column",
                ));
            }
            if columns.len() != values.len() {
                return Err(AppError::validation(format!(
                    "row comparison binds {} values for {} columns",
                    values.len(),
                    columns.len()
                )));
            }
            if !matches!(
                operator,
                Operator::Eq | Operator::Ne | Operator::Gt | Operator::GtEq | Operator::Lt
                    | Operator::LtEq
            ) {
                return Err(AppError::validation(format!(
                    "row comparison does not accept operator {}",
                    operator.sql()
                )));
            }
            let names: Vec<&str> = columns.iter().map(|c| c.as_str()).collect();
            let mut marks = Vec::with_capacity(values.len());
            for value in values {
                params.push(value.clone());
                marks.push(format!("${}", params.len()));
            }
            Ok(format!(
                "({}) {} ({})",
                names.join(", "),
                operator.sql(),
                marks.join(", ")
            ))
        }
        Clause::DatePart {
            part,
            column,
            value,
        } => {
            params.push(value.clone());
            let placeholder = format!("${}", params.len());
            Ok(part.render_sql(column.as_str(), &placeholder))
        }
        Clause::Exists { negated, subquery } => {
            let inner = subquery.render(params)?;
            Ok(format!(
                "{}EXISTS ({inner})",
                if *negated { "NOT " } else { "" }
            ))
        }
        Clause::Group { items, .. } => {
            if items.is_empty() {
                return Err(AppError::validation("empty boolean group refused"));
            }
            Ok(format!("({})", render_conditions(items, params, context)?))
        }
        Clause::AggregateCmp {
            aggregate,
            operator,
            value,
        } => {
            if context == ClauseContext::Where {
                return Err(AppError::validation(
                    "aggregate comparison is HAVING-only (use having_aggregate)",
                ));
            }
            if !operator.is_scalar() {
                return Err(AppError::validation(format!(
                    "aggregate comparison does not accept operator {}",
                    operator.sql()
                )));
            }
            params.push(value.clone());
            Ok(format!(
                "{} {} ${}",
                aggregate.render(),
                operator.sql(),
                params.len()
            ))
        }
    }
}

/// Leaf predicate rendering: `column <op> $N` (single value), `column IN
/// (...)`/`NOT IN (...)` (bind list), `column [NOT] BETWEEN $a AND $b`, or a
/// bare `IS [NOT] NULL`.
fn render_predicate(
    cond: &select::Condition,
    params: &mut Vec<Value>,
) -> Result<String, AppError> {
    use select::Operator;
    let column = cond.column.as_str();
    match cond.operator {
        Operator::IsNull | Operator::NotNull => Ok(format!("{} {}", column, cond.operator.sql())),
        Operator::In | Operator::NotIn => {
            if cond.values.is_empty() {
                return Err(AppError::validation(format!(
                    "{} condition on {} needs at least one value",
                    cond.operator.sql(),
                    column
                )));
            }
            let mut marks = Vec::with_capacity(cond.values.len());
            for value in &cond.values {
                params.push(value.clone());
                marks.push(format!("${}", params.len()));
            }
            Ok(format!(
                "{} {} ({})",
                column,
                cond.operator.sql(),
                marks.join(", ")
            ))
        }
        Operator::Between | Operator::NotBetween => {
            if cond.values.len() != 2 {
                return Err(AppError::validation(format!(
                    "{} condition on {} needs exactly two values",
                    cond.operator.sql(),
                    column
                )));
            }
            let mut marks = Vec::with_capacity(2);
            for value in &cond.values {
                params.push(value.clone());
                marks.push(format!("${}", params.len()));
            }
            Ok(format!(
                "{} {} {} AND {}",
                column,
                cond.operator.sql(),
                marks[0],
                marks[1]
            ))
        }
        other => {
            if cond.values.len() != 1 {
                return Err(AppError::validation(format!(
                    "condition on {} needs exactly one value",
                    column
                )));
            }
            params.push(cond.values[0].clone());
            Ok(format!("{} {} ${}", column, other.sql(), params.len()))
        }
    }
}

/// Pagination contract (stable wire shape).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PageReq {
    #[serde(default = "default_page")]
    pub page: u32,
    #[serde(default = "default_per_page")]
    pub per_page: u32,
}

fn default_page() -> u32 {
    1
}
fn default_per_page() -> u32 {
    25
}

impl Default for PageReq {
    fn default() -> Self {
        Self {
            page: 1,
            per_page: 25,
        }
    }
}

impl PageReq {
    pub fn validate(self) -> Result<Self, AppError> {
        if self.page < 1 {
            return Err(AppError::validation("page must be >= 1"));
        }
        if !(1..=200).contains(&self.per_page) {
            return Err(AppError::validation("per_page must be 1..=200"));
        }
        Ok(self)
    }

    pub fn offset(&self) -> u32 {
        (self.page - 1) * self.per_page
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: u32,
    pub per_page: u32,
    pub total: Option<u64>,
}

impl<T> Page<T> {
    pub fn new(items: Vec<T>, page: u32, per_page: u32, total: Option<u64>) -> Self {
        Self {
            items,
            page,
            per_page,
            total,
        }
    }
}

/// Marker for operational read projections.
pub trait ReadProjection: Send + Sync + 'static {
    fn name(&self) -> &str;
}

/// Marker for large analytical queries.
pub trait ReportingQuery: Send + Sync + 'static {
    fn name(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagination_bounds() {
        assert!(PageReq {
            page: 0,
            per_page: 10
        }
        .validate()
        .is_err());
        assert!(PageReq {
            page: 1,
            per_page: 500
        }
        .validate()
        .is_err());
        let ok = PageReq {
            page: 3,
            per_page: 20,
        }
        .validate()
        .unwrap();
        assert_eq!(ok.offset(), 40);
    }
}
