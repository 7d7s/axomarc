//! Typed DELETE builder (S8): parameterized WHERE with `RETURNING`.
//! Same unfiltered guard as [`crate::update`]: DELETE without WHERE is
//! refused unless explicitly allowed.

use keel_foundation::{AppError, TenantId};

use crate::select::{Clause, Condition, Ident, Junction, Operator, Value};
use crate::{ClauseContext, CompiledWrite};

/// Builder for parameterized DELETE statements.
#[derive(Debug, Clone)]
pub struct Delete {
    table: Ident,
    conditions: Vec<(Junction, Clause)>,
    returning: Vec<Ident>,
    allow_unfiltered: bool,
}

impl Delete {
    pub fn from_table(table: Ident) -> Self {
        Self {
            table,
            conditions: Vec::new(),
            returning: Vec::new(),
            allow_unfiltered: false,
        }
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

    /// Filter with an explicit operator and one value (the semantic-filter
    /// parity path: `Ne`, `Gt`, ... arrive here from `filter_condition`).
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

    /// Opt out of the unfiltered-delete guard (whole-table wipes, test
    /// teardown). Deliberately loud: callers must name it.
    pub fn allow_unfiltered(mut self) -> Self {
        self.allow_unfiltered = true;
        self
    }

    pub fn build(&self) -> Result<CompiledWrite, AppError> {
        if self.conditions.is_empty() && !self.allow_unfiltered {
            return Err(AppError::validation(
                "DELETE without WHERE refused (call allow_unfiltered to override)",
            ));
        }
        let mut params: Vec<Value> = Vec::new();
        let mut sql = format!("DELETE FROM {}", self.table.as_str());
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
    fn delete_with_where_and_returning() {
        let q = Delete::from_table(id("appointments"))
            .where_eq(id("id"), Value::Uuid(uuid::Uuid::now_v7()))
            .returning(vec![id("id")])
            .build()
            .unwrap();
        assert_eq!(q.sql, "DELETE FROM appointments WHERE id = $1 RETURNING id");
        assert_eq!(q.params.len(), 1);
    }

    #[test]
    fn tenant_scope_appends_predicate() {
        let tenant = TenantId::new();
        let q = Delete::from_table(id("records"))
            .where_eq(id("id"), Value::Uuid(uuid::Uuid::now_v7()))
            .tenant_scope(id("tenant_id"), tenant)
            .build()
            .unwrap();
        assert!(q.sql.ends_with("AND tenant_id = $2"));
    }

    #[test]
    fn unfiltered_delete_refused() {
        let err = Delete::from_table(id("t")).build().unwrap_err();
        assert!(err.public_message.contains("WHERE"));
        assert!(Delete::from_table(id("t")).allow_unfiltered().build().is_ok());
    }
}