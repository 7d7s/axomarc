//! Typed SELECT builder (blueprint L14, S3 item 42).
//!
//! Builds parameterized SQL plus ordered bind values WITHOUT executing:
//! execution is owned by the data class (keel-postgres / stores). Safety
//! posture: values NEVER interpolate into the SQL text — they travel only as
//! `$N` bind parameters; every identifier is validated at construction
//! ([`Ident`] rejects anything outside the SQL-identifier charset).

use keel_foundation::AppError;
use serde::{Deserialize, Serialize};

/// Validated SQL identifier (table/column, optionally qualified
/// `table.column`). Construction rejects empty strings, leading digits, and
/// any character outside `[a-z0-9_.]` — nothing else reaches the SQL text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ident(String);

impl Ident {
    pub fn new(raw: impl Into<String>) -> Result<Self, AppError> {
        let raw = raw.into();
        Self::validate(&raw)?;
        Ok(Self(raw))
    }

    fn validate(raw: &str) -> Result<(), AppError> {
        let err =
            |why: &str| AppError::validation(format!("invalid SQL identifier {raw:?}: {why}"));
        if raw.is_empty() {
            return Err(err("empty"));
        }
        let segments: Vec<&str> = raw.split('.').collect();
        if segments.len() > 2 {
            return Err(err("at most one qualifier"));
        }
        for seg in segments {
            if seg.is_empty() {
                return Err(err("empty segment"));
            }
            let mut chars = seg.chars();
            let first = chars.next().expect("non-empty segment");
            if !(first.is_ascii_lowercase() || first == '_') {
                return Err(err("must start with a-z or _"));
            }
            if chars.any(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')) {
                return Err(err("must contain only a-z, 0-9, _"));
            }
            if seg.len() > 64 {
                return Err(err("longer than 64 chars"));
            }
        }
        Ok(())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Bind value carried alongside generated SQL. Never rendered into the SQL
/// text; the data layer maps these onto native driver binds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    Text(String),
    Uuid(uuid::Uuid),
    /// timestamptz bind (S8: model runtime writes).
    Timestamp(time::OffsetDateTime),
    /// jsonb bind (S8: model runtime writes).
    Json(serde_json::Value),
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Text(_) => "text",
            Value::Uuid(_) => "uuid",
            Value::Timestamp(_) => "timestamp",
            Value::Json(_) => "json",
        }
    }
}

impl From<&str> for Value {
    fn from(v: &str) -> Self {
        Value::Text(v.to_string())
    }
}
impl From<String> for Value {
    fn from(v: String) -> Self {
        Value::Text(v)
    }
}
impl From<bool> for Value {
    fn from(v: bool) -> Self {
        Value::Bool(v)
    }
}
impl From<i64> for Value {
    fn from(v: i64) -> Self {
        Value::Int(v)
    }
}
impl From<i32> for Value {
    fn from(v: i32) -> Self {
        Value::Int(v as i64)
    }
}
impl From<uuid::Uuid> for Value {
    fn from(v: uuid::Uuid) -> Self {
        Value::Uuid(v)
    }
}
impl From<time::OffsetDateTime> for Value {
    fn from(v: time::OffsetDateTime) -> Self {
        Value::Timestamp(v)
    }
}
impl From<serde_json::Value> for Value {
    fn from(v: serde_json::Value) -> Self {
        Value::Json(v)
    }
}
impl From<f64> for Value {
    fn from(v: f64) -> Self {
        Value::Float(v)
    }
}

/// Comparison operators supported by the WHERE contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operator {
    Eq,
    Ne,
    Gt,
    GtEq,
    Lt,
    LtEq,
    Like,
    In,
    IsNull,
    NotNull,
}

impl Operator {
    pub(crate) fn sql(self) -> &'static str {
        match self {
            Operator::Eq => "=",
            Operator::Ne => "<>",
            Operator::Gt => ">",
            Operator::GtEq => ">=",
            Operator::Lt => "<",
            Operator::LtEq => "<=",
            Operator::Like => "LIKE",
            Operator::In => "IN",
            Operator::IsNull => "IS NULL",
            Operator::NotNull => "IS NOT NULL",
        }
    }
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Asc,
    Desc,
}

/// One WHERE predicate. `IsNull`/`NotNull` ignore `values`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Condition {
    pub column: Ident,
    pub operator: Operator,
    pub values: Vec<Value>,
}

/// One JOIN clause (`INNER`/`LEFT ... ON left = right`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Join {
    pub left: bool,
    pub table: Ident,
    pub on_left: Ident,
    pub on_right: Ident,
}

/// Compiled SELECT statement: SQL text with `$N` placeholders and the bind
/// values in matching order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledSelect {
    pub sql: String,
    pub params: Vec<Value>,
}

/// Typed SELECT builder. SQL is generated only by [`Select::build`]; every
/// identifier went through [`Ident`] validation on the way in.
#[derive(Debug, Clone, Default)]
pub struct Select {
    table: Option<Ident>,
    columns: Vec<Ident>,
    conditions: Vec<Condition>,
    joins: Vec<Join>,
    order: Vec<(Ident, Direction)>,
    limit: Option<u64>,
    offset: Option<u64>,
}

impl Select {
    pub fn from_table(table: Ident) -> Self {
        Self {
            table: Some(table),
            ..Default::default()
        }
    }

    /// Add a projected column (none = `SELECT *`).
    pub fn column(mut self, column: Ident) -> Self {
        self.columns.push(column);
        self
    }

    /// Filter: column equals value.
    pub fn where_eq(mut self, column: Ident, value: impl Into<Value>) -> Self {
        self.conditions.push(Condition {
            column,
            operator: Operator::Eq,
            values: vec![value.into()],
        });
        self
    }

    /// Filter with an explicit operator and one value.
    pub fn where_op(mut self, column: Ident, operator: Operator, value: impl Into<Value>) -> Self {
        self.conditions.push(Condition {
            column,
            operator,
            values: vec![value.into()],
        });
        self
    }

    /// Filter: column IN (values...). Empty value list is a build error.
    pub fn where_in(mut self, column: Ident, values: Vec<Value>) -> Self {
        self.conditions.push(Condition {
            column,
            operator: Operator::In,
            values,
        });
        self
    }

    pub fn where_null(mut self, column: Ident) -> Self {
        self.conditions.push(Condition {
            column,
            operator: Operator::IsNull,
            values: vec![],
        });
        self
    }

    pub fn where_not_null(mut self, column: Ident) -> Self {
        self.conditions.push(Condition {
            column,
            operator: Operator::NotNull,
            values: vec![],
        });
        self
    }

    pub fn join_inner(mut self, table: Ident, on_left: Ident, on_right: Ident) -> Self {
        self.joins.push(Join {
            left: false,
            table,
            on_left,
            on_right,
        });
        self
    }

    pub fn join_left(mut self, table: Ident, on_left: Ident, on_right: Ident) -> Self {
        self.joins.push(Join {
            left: true,
            table,
            on_left,
            on_right,
        });
        self
    }

    pub fn order_by(mut self, column: Ident, direction: Direction) -> Self {
        self.order.push((column, direction));
        self
    }

    pub fn limit(mut self, n: u64) -> Self {
        self.limit = Some(n);
        self
    }

    pub fn offset(mut self, n: u64) -> Self {
        self.offset = Some(n);
        self
    }

    /// TENANT-plane scoping helper (L11 tie-in): append `column = $tenant`.
    pub fn tenant_scope(self, column: Ident, tenant: keel_foundation::TenantId) -> Self {
        self.where_eq(column, Value::Uuid(*tenant.as_uuid()))
    }

    /// Validate shape and generate SQL + ordered binds.
    pub fn build(self) -> Result<CompiledSelect, AppError> {
        let table = self
            .table
            .ok_or_else(|| AppError::validation("SELECT requires a table"))?;
        let mut params: Vec<Value> = Vec::new();
        let mut sql = String::with_capacity(128);

        sql.push_str("SELECT ");
        if self.columns.is_empty() {
            sql.push('*');
        } else {
            let cols: Vec<&str> = self.columns.iter().map(|c| c.as_str()).collect();
            sql.push_str(&cols.join(", "));
        }
        sql.push_str(" FROM ");
        sql.push_str(table.as_str());

        for join in &self.joins {
            sql.push_str(if join.left { " LEFT JOIN " } else { " JOIN " });
            sql.push_str(join.table.as_str());
            sql.push_str(" ON ");
            sql.push_str(join.on_left.as_str());
            sql.push_str(" = ");
            sql.push_str(join.on_right.as_str());
        }

        if !self.conditions.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&crate::render_conditions(&self.conditions, &mut params)?);
        }

        if !self.order.is_empty() {
            let parts: Vec<String> = self
                .order
                .iter()
                .map(|(col, dir)| {
                    format!(
                        "{} {}",
                        col.as_str(),
                        match dir {
                            Direction::Asc => "ASC",
                            Direction::Desc => "DESC",
                        }
                    )
                })
                .collect();
            sql.push_str(" ORDER BY ");
            sql.push_str(&parts.join(", "));
        }

        if let Some(n) = self.limit {
            params.push(Value::Int(n as i64));
            sql.push_str(&format!(" LIMIT ${}", params.len()));
        }
        if let Some(n) = self.offset {
            params.push(Value::Int(n as i64));
            sql.push_str(&format!(" OFFSET ${}", params.len()));
        }

        Ok(CompiledSelect { sql, params })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(raw: &str) -> Ident {
        Ident::new(raw).unwrap()
    }

    #[test]
    fn identifier_validation_rejects_injection_shapes() {
        assert!(Ident::new("").is_err());
        assert!(Ident::new("users; DROP TABLE users").is_err());
        assert!(Ident::new("Users").is_err());
        assert!(Ident::new("1col").is_err());
        assert!(Ident::new("a.b.c").is_err());
        assert!(Ident::new("col name").is_err());
        assert!(Ident::new("col-name").is_err());
        assert!(Ident::new("x".repeat(65)).is_err());
        assert!(Ident::new("users.id").is_ok());
        assert!(Ident::new("_private").is_ok());
    }

    #[test]
    fn simple_select_star() {
        let q = Select::from_table(id("users")).build().unwrap();
        assert_eq!(q.sql, "SELECT * FROM users");
        assert!(q.params.is_empty());
    }

    #[test]
    fn projection_conditions_and_params_ordered() {
        let q = Select::from_table(id("appointments"))
            .column(id("appointments.id"))
            .column(id("appointments.status"))
            .where_eq(
                id("appointments.tenant_id"),
                Value::Uuid(uuid::Uuid::now_v7()),
            )
            .where_op(id("status"), Operator::Eq, "confirmed")
            .build()
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT appointments.id, appointments.status FROM appointments WHERE appointments.tenant_id = $1 AND status = $2"
        );
        assert_eq!(q.params.len(), 2);
        assert!(matches!(q.params[0], Value::Uuid(_)));
        assert_eq!(q.params[1], Value::Text("confirmed".into()));
    }

    #[test]
    fn in_join_order_limit_offset() {
        let q = Select::from_table(id("doctors"))
            .join_left(id("clinics"), id("doctors.clinic_id"), id("clinics.id"))
            .where_in(id("specialty"), vec!["cardio".into(), "neuro".into()])
            .where_not_null(id("clinic_id"))
            .order_by(id("name"), Direction::Asc)
            .order_by(id("created_at"), Direction::Desc)
            .limit(20)
            .offset(40)
            .build()
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT * FROM doctors LEFT JOIN clinics ON doctors.clinic_id = clinics.id WHERE specialty IN ($1, $2) AND clinic_id IS NOT NULL ORDER BY name ASC, created_at DESC LIMIT $3 OFFSET $4"
        );
        assert_eq!(q.params.len(), 4);
        assert_eq!(q.params[2], Value::Int(20));
        assert_eq!(q.params[3], Value::Int(40));
    }

    #[test]
    fn empty_in_list_rejected() {
        let err = Select::from_table(id("t"))
            .where_in(id("c"), vec![])
            .build()
            .unwrap_err();
        assert!(err.public_message.contains("IN"));
    }

    #[test]
    fn tenant_scope_binds_uuid() {
        let tenant = keel_foundation::TenantId::new();
        let q = Select::from_table(id("records"))
            .tenant_scope(id("tenant_id"), tenant)
            .build()
            .unwrap();
        assert_eq!(q.sql, "SELECT * FROM records WHERE tenant_id = $1");
        assert_eq!(q.params[0], Value::Uuid(*tenant.as_uuid()));
    }

    #[test]
    fn value_type_names_and_conversions() {
        assert_eq!(Value::from("x").type_name(), "text");
        assert_eq!(Value::from(true).type_name(), "bool");
        assert_eq!(Value::from(3i32).type_name(), "int");
        assert_eq!(Value::from(uuid::Uuid::now_v7()).type_name(), "uuid");
        assert_eq!(Value::Null.type_name(), "null");
    }
}
