//! # keel-query
//! Read side of the three data-access modes (11-SPEC §5): `ReadProjection` for
//! operational reads, `ReportingQuery` for analytical reads. No write transactions.
//! The typed SELECT builder (L14) generates parameterized SQL without executing.

use keel_foundation::AppError;
use serde::{Deserialize, Serialize};

pub mod delete;
pub mod insert;
pub mod select;
pub mod update;

pub use delete::Delete;
pub use insert::{ConflictPolicy, Insert};
pub use select::{CompiledSelect, Ident, Operator, Select, Value};
pub use update::Update;

/// Compiled WRITE statement (INSERT/UPDATE/DELETE): SQL text with `$N`
/// placeholders and bind values in matching order. Execution is owned by the
/// data class, exactly like [`CompiledSelect`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CompiledWrite {
    pub sql: String,
    pub params: Vec<Value>,
}

/// Shared WHERE rendering for SELECT/UPDATE/DELETE builders. Appends binds to
/// `params` in order and returns the joined predicate text.
pub(crate) fn render_conditions(
    conditions: &[select::Condition],
    params: &mut Vec<Value>,
) -> Result<String, AppError> {
    let mut parts = Vec::with_capacity(conditions.len());
    for cond in conditions {
        match cond.operator {
            Operator::IsNull | Operator::NotNull => {
                parts.push(format!("{} {}", cond.column.as_str(), cond.operator.sql()));
            }
            Operator::In => {
                if cond.values.is_empty() {
                    return Err(AppError::validation(format!(
                        "IN condition on {} needs at least one value",
                        cond.column.as_str()
                    )));
                }
                let mut marks = Vec::with_capacity(cond.values.len());
                for value in &cond.values {
                    params.push(value.clone());
                    marks.push(format!("${}", params.len()));
                }
                parts.push(format!(
                    "{} IN ({})",
                    cond.column.as_str(),
                    marks.join(", ")
                ));
            }
            other => {
                params.push(cond.values[0].clone());
                parts.push(format!(
                    "{} {} ${}",
                    cond.column.as_str(),
                    other.sql(),
                    params.len()
                ));
            }
        }
    }
    Ok(parts.join(" AND "))
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
