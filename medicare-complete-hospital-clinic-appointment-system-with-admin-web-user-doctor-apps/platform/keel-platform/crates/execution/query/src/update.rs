//! Typed UPDATE builder (S8): parameterized SET/WHERE with `RETURNING`.
//! Safety posture mirrors [`crate::select`]: [`Ident`]-validated identifiers,
//! `$N` binds only, and an UNFILTERED-UPDATE guard — building without a WHERE
//! clause fails unless explicitly allowed.

use kelson_foundation::{AppError, TenantId};

use crate::select::{Clause, Condition, Ident, Junction, Operator, Value};
use crate::{ClauseContext, CompiledWrite};

/// One SET assignment: a plain `col = $n` bind, or an arithmetic adjust
/// `col = col ± $n` (atomic increment/decrement — the increment reads the
/// CURRENT row value in-database, never a read-modify-write roundtrip).
#[derive(Debug, Clone)]
enum SetItem {
    Assign(Ident, Value),
    Adjust {
        column: Ident,
        sign: &'static str,
        value: Value,
    },
}

impl SetItem {
    fn column(&self) -> &Ident {
        match self {
            SetItem::Assign(column, _) | SetItem::Adjust { column, .. } => column,
        }
    }
}

/// Builder for parameterized UPDATE statements.
#[derive(Debug, Clone)]
pub struct Update {
    table: Ident,
    sets: Vec<SetItem>,
    conditions: Vec<(Junction, Clause)>,
    returning: Vec<Ident>,
    allow_unfiltered: bool,
}

impl Update {
    pub fn table(table: Ident) -> Self {
        Self {
            table,
            sets: Vec::new(),
            conditions: Vec::new(),
            returning: Vec::new(),
            allow_unfiltered: false,
        }
    }

    /// One `SET column = value` assignment. Duplicate columns are rejected at
    /// build time (last-writer-wins hides bugs).
    pub fn set(mut self, column: Ident, value: impl Into<Value>) -> Self {
        self.sets.push(SetItem::Assign(column, value.into()));
        self
    }

    /// Atomic `SET column = column + by`.
    pub fn increment(mut self, column: Ident, by: i64) -> Self {
        self.sets.push(SetItem::Adjust {
            column,
            sign: "+",
            value: Value::Int(by),
        });
        self
    }

    /// Atomic `SET column = column - by`.
    pub fn decrement(mut self, column: Ident, by: i64) -> Self {
        self.sets.push(SetItem::Adjust {
            column,
            sign: "-",
            value: Value::Int(by),
        });
        self
    }

    fn push(&mut self, condition: Condition) {
        self.conditions.push((Junction::And, Clause::Pred(condition)));
    }

    pub fn where_eq(mut self, column: Ident, value: impl Into<Value>) -> Self {
        self.push(Condition {
            column,
            operator: Operator::Eq,
            values: vec![value.into()],
        });
        self
    }

    pub fn where_op(
        mut self,
        column: Ident,
        operator: Operator,
        value: impl Into<Value>,
    ) -> Self {
        self.push(Condition {
            column,
            operator,
            values: vec![value.into()],
        });
        self
    }

    pub fn where_in(mut self, column: Ident, values: Vec<Value>) -> Self {
        self.push(Condition {
            column,
            operator: Operator::In,
            values,
        });
        self
    }

    pub fn where_is_null(mut self, column: Ident) -> Self {
        self.push(Condition {
            column,
            operator: Operator::IsNull,
            values: Vec::new(),
        });
        self
    }

    pub fn where_not_null(mut self, column: Ident) -> Self {
        self.push(Condition {
            column,
            operator: Operator::NotNull,
            values: Vec::new(),
        });
        self
    }

    /// Tenant isolation predicate (`column = tenant uuid`).
    pub fn tenant_scope(self, column: Ident, tenant: TenantId) -> Self {
        self.where_eq(column, Value::Uuid(*tenant.as_uuid()))
    }

    pub fn returning(mut self, columns: Vec<Ident>) -> Self {
        self.returning = columns;
        self
    }

    /// Opt out of the unfiltered-update guard (whole-table rewrites, admin
    /// tooling). Deliberately loud: callers must name it.
    pub fn allow_unfiltered(mut self) -> Self {
        self.allow_unfiltered = true;
        self
    }

    pub fn build(&self) -> Result<CompiledWrite, AppError> {
        if self.sets.is_empty() {
            return Err(AppError::validation("UPDATE needs at least one SET"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for item in &self.sets {
            if !seen.insert(item.column().as_str()) {
                return Err(AppError::validation(format!(
                    "UPDATE sets column {} twice",
                    item.column().as_str()
                )));
            }
        }
        if self.conditions.is_empty() && !self.allow_unfiltered {
            return Err(AppError::validation(
                "UPDATE without WHERE refused (call allow_unfiltered to override)",
            ));
        }

        let mut params: Vec<Value> = Vec::new();
        let mut sql = format!("UPDATE {} SET ", self.table.as_str());
        let assigns: Vec<String> = self
            .sets
            .iter()
            .map(|item| match item {
                SetItem::Assign(col, v) => {
                    params.push(v.clone());
                    format!("{} = ${}", col.as_str(), params.len())
                }
                SetItem::Adjust {
                    column,
                    sign,
                    value,
                } => {
                    params.push(value.clone());
                    format!(
                        "{} = {} {} ${}",
                        column.as_str(),
                        column.as_str(),
                        sign,
                        params.len()
                    )
                }
            })
            .collect();
        sql.push_str(&assigns.join(", "));

        if !self.conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&crate::render_conditions(
                &self.conditions,
                &mut params,
                ClauseContext::Where,
            )?);
        }

        if !self.returning.is_empty() {
            sql.push_str(" RETURNING ");
            sql.push_str(
                &self
                    .returning
                    .iter()
                    .map(|c| c.as_str().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }

        Ok(CompiledWrite { sql, params })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(raw: &str) -> Ident {
        Ident::new(raw).unwrap()
    }

    #[test]
    fn update_with_where_and_returning() {
        let q = Update::table(id("appointments"))
            .set(id("status"), "cancelled")
            .set(id("version"), 2i64)
            .where_eq(id("id"), Value::Uuid(uuid::Uuid::now_v7()))
            .returning(vec![id("id"), id("status")])
            .build()
            .unwrap();
        assert_eq!(
            q.sql,
            "UPDATE appointments SET status = $1, version = $2 WHERE id = $3 RETURNING id, status"
        );
        assert_eq!(q.params.len(), 3);
    }

    #[test]
    fn tenant_scope_appends_predicate() {
        let tenant = TenantId::new();
        let q = Update::table(id("records"))
            .set(id("x"), 1i64)
            .where_eq(id("id"), Value::Uuid(uuid::Uuid::now_v7()))
            .tenant_scope(id("tenant_id"), tenant)
            .build()
            .unwrap();
        assert!(q.sql.ends_with("AND tenant_id = $3"));
    }

    #[test]
    fn unfiltered_update_refused() {
        let err = Update::table(id("t"))
            .set(id("a"), 1i64)
            .build()
            .unwrap_err();
        assert!(err.public_message.contains("WHERE"));
        assert!(Update::table(id("t"))
            .set(id("a"), 1i64)
            .allow_unfiltered()
            .build()
            .is_ok());
    }

    #[test]
    fn duplicate_set_refused() {
        assert!(Update::table(id("t"))
            .set(id("a"), 1i64)
            .set(id("a"), 2i64)
            .where_eq(id("id"), 1i64)
            .build()
            .is_err());
    }

    #[test]
    fn empty_set_refused() {
        assert!(Update::table(id("t"))
            .where_eq(id("id"), 1i64)
            .build()
            .is_err());
    }

    // ---------------------------------------------------- Wave 3 (phase 1)

    #[test]
    fn increment_and_decrement_shapes() {
        let q = Update::table(id("t"))
            .increment(id("views"), 1)
            .where_eq(id("id"), 1i64)
            .build()
            .unwrap();
        assert_eq!(q.sql, "UPDATE t SET views = views + $1 WHERE id = $2");
        assert_eq!(q.params, vec![Value::Int(1), Value::Int(1)]);

        let q = Update::table(id("t"))
            .decrement(id("balance"), 25)
            .where_eq(id("id"), 1i64)
            .build()
            .unwrap();
        assert_eq!(
            q.sql,
            "UPDATE t SET balance = balance - $1 WHERE id = $2"
        );
    }

    #[test]
    fn mixed_set_and_increment_keep_insertion_order() {
        let q = Update::table(id("t"))
            .set(id("a"), 1i64)
            .increment(id("b"), 2)
            .set(id("c"), 3i64)
            .where_eq(id("id"), 9i64)
            .build()
            .unwrap();
        assert_eq!(
            q.sql,
            "UPDATE t SET a = $1, b = b + $2, c = $3 WHERE id = $4"
        );
        assert_eq!(
            q.params,
            vec![Value::Int(1), Value::Int(2), Value::Int(3), Value::Int(9)]
        );
    }

    #[test]
    fn increment_duplicate_of_set_refused() {
        let err = Update::table(id("t"))
            .set(id("views"), 5i64)
            .increment(id("views"), 1)
            .where_eq(id("id"), 1i64)
            .build()
            .unwrap_err();
        assert!(err.public_message.contains("twice"));
    }
}