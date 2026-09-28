//! Typed INSERT builder (S8): single or multi-row INSERT with optional
//! `ON CONFLICT` upsert policy and `RETURNING`. Same safety posture as
//! [`crate::select`]: identifiers validated via [`Ident`], values travel only
//! as `$N` binds. Builds SQL without executing — execution belongs to the
//! data class.

use keel_foundation::{AppError, TenantId};

use crate::select::{Ident, Value};
use crate::CompiledWrite;

/// `ON CONFLICT` policy for upserts. The conflict target is always explicit —
/// no target-less `ON CONFLICT` forms are generated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    /// `ON CONFLICT (target) DO NOTHING` — losing rows are skipped.
    DoNothing,
    /// `ON CONFLICT (target) DO UPDATE SET col = EXCLUDED.col` for every
    /// inserted column that is not part of the conflict target.
    DoUpdate,
}

/// Builder for parameterized INSERT statements.
///
/// Column set is declared once via [`Insert::columns`]; every row supplied
/// via [`Insert::row`] must align with it.
#[derive(Debug, Clone)]
pub struct Insert {
    table: Ident,
    columns: Vec<Ident>,
    rows: Vec<Vec<Value>>,
    conflict_target: Vec<Ident>,
    conflict: Option<ConflictPolicy>,
    conflict_update_exclude: Vec<Ident>,
    returning: Vec<Ident>,
    tenant_check: Option<(Ident, TenantId)>,
}

impl Insert {
    pub fn into_table(table: Ident) -> Self {
        Self {
            table,
            columns: Vec::new(),
            rows: Vec::new(),
            conflict_target: Vec::new(),
            conflict: None,
            conflict_update_exclude: Vec::new(),
            returning: Vec::new(),
            tenant_check: None,
        }
    }

    /// Declare the column list (once). Replaces any previous declaration.
    pub fn columns(mut self, columns: Vec<Ident>) -> Self {
        self.columns = columns;
        self
    }

    /// Append one row of values, aligned with [`Insert::columns`].
    pub fn row(mut self, values: Vec<Value>) -> Self {
        self.rows.push(values);
        self
    }

    /// Set the `ON CONFLICT` policy. Target columns must be a subset of the
    /// inserted columns (checked at build time).
    pub fn on_conflict(mut self, target: Vec<Ident>, policy: ConflictPolicy) -> Self {
        self.conflict_target = target;
        self.conflict = Some(policy);
        self
    }

    /// Extra columns excluded from the `DO UPDATE SET` list (in addition to
    /// the conflict target itself). Identity columns such as a primary key
    /// must never be overwritten by a merge: the surviving row keeps its id.
    pub fn exclude_from_conflict_update(mut self, columns: Vec<Ident>) -> Self {
        self.conflict_update_exclude = columns;
        self
    }

    /// `RETURNING` clause. Empty = no row data back.
    pub fn returning(mut self, columns: Vec<Ident>) -> Self {
        self.returning = columns;
        self
    }

    /// Tenant write guard: the build FAILS unless every row binds the
    /// expected tenant uuid into `column`. This is a compile-time-style check
    /// (no SQL shape change) that keeps tenant-scoped writes honest.
    pub fn tenant_scope(mut self, column: Ident, tenant: TenantId) -> Self {
        self.tenant_check = Some((column, tenant));
        self
    }

    pub fn build(&self) -> Result<CompiledWrite, AppError> {
        if self.columns.is_empty() {
            return Err(AppError::validation("INSERT needs at least one column"));
        }
        if self.rows.is_empty() {
            return Err(AppError::validation("INSERT needs at least one row"));
        }
        let width = self.columns.len();
        for (idx, row) in self.rows.iter().enumerate() {
            if row.len() != width {
                return Err(AppError::validation(format!(
                    "INSERT row {idx} has {} values for {width} columns",
                    row.len()
                )));
            }
        }
        if let Some((col, tenant)) = &self.tenant_check {
            let pos = self
                .columns
                .iter()
                .position(|c| c == col)
                .ok_or_else(|| {
                    AppError::validation(format!(
                        "tenant_scope column {} is not an inserted column",
                        col.as_str()
                    ))
                })?;
            let expected = Value::Uuid(*tenant.as_uuid());
            for (idx, row) in self.rows.iter().enumerate() {
                if row[pos] != expected {
                    return Err(AppError::validation(format!(
                        "INSERT row {idx} does not bind the tenant into {}",
                        col.as_str()
                    )));
                }
            }
        }

        let mut params: Vec<Value> = Vec::new();
        let mut sql = format!(
            "INSERT INTO {} ({})",
            self.table.as_str(),
            self.columns
                .iter()
                .map(|c| c.as_str().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        sql.push_str(" VALUES ");
        let row_sqls: Vec<String> = self
            .rows
            .iter()
            .map(|row| {
                let marks: Vec<String> = row
                    .iter()
                    .map(|v| {
                        params.push(v.clone());
                        format!("${}", params.len())
                    })
                    .collect();
                format!("({})", marks.join(", "))
            })
            .collect();
        sql.push_str(&row_sqls.join(", "));

        if let Some(policy) = self.conflict {
            if self.conflict_target.is_empty() {
                return Err(AppError::validation(
                    "ON CONFLICT needs at least one target column",
                ));
            }
            for t in &self.conflict_target {
                if !self.columns.iter().any(|c| c == t) {
                    return Err(AppError::validation(format!(
                        "conflict target {} is not an inserted column",
                        t.as_str()
                    )));
                }
            }
            sql.push_str(" ON CONFLICT (");
            sql.push_str(
                &self
                    .conflict_target
                    .iter()
                    .map(|c| c.as_str().to_string())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            sql.push(')');
            match policy {
                ConflictPolicy::DoNothing => sql.push_str(" DO NOTHING"),
                ConflictPolicy::DoUpdate => {
                    let updates: Vec<String> = self
                        .columns
                        .iter()
                        .filter(|c| {
                            !self.conflict_target.iter().any(|t| t == *c)
                                && !self.conflict_update_exclude.iter().any(|x| x == *c)
                        })
                        .map(|c| format!("{} = EXCLUDED.{}", c.as_str(), c.as_str()))
                        .collect();
                    if updates.is_empty() {
                        return Err(AppError::validation(
                            "ON CONFLICT DO UPDATE needs at least one non-target column",
                        ));
                    }
                    sql.push_str(" DO UPDATE SET ");
                    sql.push_str(&updates.join(", "));
                }
            }
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
    fn simple_insert() {
        let q = Insert::into_table(id("appointments"))
            .columns(vec![id("id"), id("patient")])
            .row(vec![
                Value::Uuid(uuid::Uuid::now_v7()),
                Value::Text("Jane".into()),
            ])
            .build()
            .unwrap();
        assert_eq!(q.sql, "INSERT INTO appointments (id, patient) VALUES ($1, $2)");
        assert_eq!(q.params.len(), 2);
    }

    #[test]
    fn multi_row_insert_numbers_params_across_rows() {
        let q = Insert::into_table(id("tags"))
            .columns(vec![id("name")])
            .row(vec![Value::Text("a".into())])
            .row(vec![Value::Text("b".into())])
            .build()
            .unwrap();
        assert_eq!(q.sql, "INSERT INTO tags (name) VALUES ($1), ($2)");
        assert_eq!(q.params[1], Value::Text("b".into()));
    }

    #[test]
    fn upsert_do_update_excludes_target_columns() {
        let q = Insert::into_table(id("http_sessions"))
            .columns(vec![id("id"), id("data"), id("last_seen")])
            .row(vec![
                Value::Text("s1".into()),
                Value::Text("{}".into()),
                Value::Text("t".into()),
            ])
            .on_conflict(vec![id("id")], ConflictPolicy::DoUpdate)
            .returning(vec![id("id")])
            .build()
            .unwrap();
        assert_eq!(
            q.sql,
            "INSERT INTO http_sessions (id, data, last_seen) VALUES ($1, $2, $3) ON CONFLICT (id) DO UPDATE SET data = EXCLUDED.data, last_seen = EXCLUDED.last_seen RETURNING id"
        );
    }

    #[test]
    fn upsert_do_update_excludes_identity_column() {
        let q = Insert::into_table(id("probes"))
            .columns(vec![id("id"), id("tenant_id"), id("name"), id("value")])
            .row(vec![
                Value::Uuid(uuid::Uuid::now_v7()),
                Value::Uuid(uuid::Uuid::now_v7()),
                Value::Text("gauge".into()),
                Value::Text("v".into()),
            ])
            .on_conflict(vec![id("tenant_id"), id("name")], ConflictPolicy::DoUpdate)
            .exclude_from_conflict_update(vec![id("id")])
            .build()
            .unwrap();
        assert!(
            q.sql.ends_with("ON CONFLICT (tenant_id, name) DO UPDATE SET value = EXCLUDED.value"),
            "id stays untouched on merge: {}",
            q.sql
        );
    }

    #[test]
    fn upsert_do_nothing() {
        let q = Insert::into_table(id("idempotency_keys"))
            .columns(vec![id("key"), id("response")])
            .row(vec![Value::Text("k".into()), Value::Null])
            .on_conflict(vec![id("key")], ConflictPolicy::DoNothing)
            .build()
            .unwrap();
        assert!(q.sql.ends_with("ON CONFLICT (key) DO NOTHING"));
    }

    #[test]
    fn build_errors() {
        assert!(Insert::into_table(id("t")).build().is_err());
        assert!(Insert::into_table(id("t"))
            .columns(vec![id("a")])
            .build()
            .is_err());
        assert!(Insert::into_table(id("t"))
            .columns(vec![id("a")])
            .row(vec![Value::Null, Value::Null])
            .build()
            .is_err());
        // conflict target not among columns
        assert!(Insert::into_table(id("t"))
            .columns(vec![id("a")])
            .row(vec![Value::Null])
            .on_conflict(vec![id("zz")], ConflictPolicy::DoNothing)
            .build()
            .is_err());
        // DO UPDATE with only target columns
        assert!(Insert::into_table(id("t"))
            .columns(vec![id("a")])
            .row(vec![Value::Null])
            .on_conflict(vec![id("a")], ConflictPolicy::DoUpdate)
            .build()
            .is_err());
    }

    #[test]
    fn tenant_scope_guard() {
        let tenant = TenantId::new();
        let ok = Insert::into_table(id("records"))
            .columns(vec![id("tenant_id"), id("x")])
            .row(vec![Value::Uuid(*tenant.as_uuid()), Value::Int(1)])
            .tenant_scope(id("tenant_id"), tenant)
            .build()
            .unwrap();
        assert_eq!(ok.params.len(), 2);

        // Wrong tenant bound -> build refuses.
        let bad = Insert::into_table(id("records"))
            .columns(vec![id("tenant_id"), id("x")])
            .row(vec![Value::Uuid(uuid::Uuid::now_v7()), Value::Int(1)])
            .tenant_scope(id("tenant_id"), tenant)
            .build();
        assert!(bad.is_err());

        // tenant column missing from the insert -> build refuses.
        assert!(Insert::into_table(id("records"))
            .columns(vec![id("x")])
            .row(vec![Value::Int(1)])
            .tenant_scope(id("tenant_id"), tenant)
            .build()
            .is_err());
    }
}