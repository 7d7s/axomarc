//! Typed SELECT builder (blueprint L14, S3 item 42).
//!
//! Builds parameterized SQL plus ordered bind values WITHOUT executing:
//! execution is owned by the data class (kelson-postgres / stores). Safety
//! posture: values NEVER interpolate into the SQL text — they travel only as
//! `$N` bind parameters; every identifier is validated at construction
//! ([`Ident`] rejects anything outside the SQL-identifier charset).
//!
//! WHERE/HAVING predicates form a clause tree ([`Clause`]): leaf predicates
//! nest into parenthesized [`Clause::Group`]s with explicit [`Junction`]s, and
//! correlated subqueries ([`Select::where_exists`]) render into the SAME bind
//! sequence as the outer query — placeholders are numbered continuously by
//! SQL text order. Builder methods defer all validation to [`Select::build`].

use kelson_foundation::AppError;
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
    /// Case-insensitive match (renders `ILIKE`).
    Ilike,
    In,
    NotIn,
    Between,
    NotBetween,
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
            Operator::Ilike => "ILIKE",
            Operator::In => "IN",
            Operator::NotIn => "NOT IN",
            Operator::Between => "BETWEEN",
            Operator::NotBetween => "NOT BETWEEN",
            Operator::IsNull => "IS NULL",
            Operator::NotNull => "IS NOT NULL",
        }
    }

    /// The operator rendered for a specific dialect.
    ///
    /// Every operator is engine-neutral except the case-insensitive match:
    /// Postgres spells it `ILIKE`, MySQL has no `ILIKE` and its `LIKE` is
    /// already case-insensitive under the default collation.
    pub(crate) fn sql_for(self, dialect: &dyn crate::dialect::Dialect) -> String {
        match self {
            Operator::Ilike => dialect.ilike_operator().to_string(),
            other => other.sql().to_string(),
        }
    }

    /// Scalar comparison operators - usable between two COLUMNS or an
    /// aggregate and a bind. List/range/null operators are excluded: they
    /// either need bind lists or render as bare predicates.
    pub(crate) fn is_scalar(self) -> bool {
        matches!(
            self,
            Operator::Eq
                | Operator::Ne
                | Operator::Gt
                | Operator::GtEq
                | Operator::Lt
                | Operator::LtEq
                | Operator::Like
                | Operator::Ilike
        )
    }
}

/// Sort direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    Asc,
    Desc,
}

/// Junction between adjacent clauses at one nesting level. The first clause
/// of a list is joined implicitly (its junction is not rendered); subsequent
/// clauses render `AND`/`OR` per this tag. SQL precedence applies to flat
/// mixes — use [`Select::group`] to pin parenthesization explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Junction {
    And,
    Or,
}

/// Aggregate projection for `SELECT <agg>` (and `having_aggregate`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Aggregate {
    /// `COUNT(*)`
    Count,
    /// `COUNT(column)`
    CountColumn(Ident),
    Sum(Ident),
    Avg(Ident),
    Min(Ident),
    Max(Ident),
}

impl Aggregate {
    /// SQL text for the projection. Identifiers are [`Ident`]-validated.
    pub(crate) fn render(&self) -> String {
        match self {
            Aggregate::Count => "COUNT(*)".to_string(),
            Aggregate::CountColumn(c) => format!("COUNT({})", c.as_str()),
            Aggregate::Sum(c) => format!("SUM({})", c.as_str()),
            Aggregate::Avg(c) => format!("AVG({})", c.as_str()),
            Aggregate::Min(c) => format!("MIN({})", c.as_str()),
            Aggregate::Max(c) => format!("MAX({})", c.as_str()),
        }
    }
}

/// Date/time extraction part for the `where_date`/`where_year`/`where_month`/
/// `where_day`/`where_time` family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DatePart {
    Date,
    Year,
    Month,
    Day,
    Time,
}

impl DatePart {
    /// The Postgres rendering (`::date`, `TO_CHAR`). The dialect layer
    /// calls this for the Postgres dialect; MySQL renders its own form
    /// (`DATE()`, `DATE_FORMAT`).
    pub(crate) fn render_postgres(self, column: &str, placeholder: &str) -> String {
        match self {
            DatePart::Date => format!("{column}::date = {placeholder}::date"),
            DatePart::Year => format!("EXTRACT(YEAR FROM {column}) = {placeholder}"),
            DatePart::Month => format!("EXTRACT(MONTH FROM {column}) = {placeholder}"),
            DatePart::Day => format!("EXTRACT(DAY FROM {column}) = {placeholder}"),
            DatePart::Time => format!("TO_CHAR({column}, 'HH24:MI:SS') = {placeholder}"),
        }
    }
}

/// Row-level locking clause, rendered after LIMIT/OFFSET (Postgres order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lock {
    /// `FOR UPDATE`
    Update,
    /// `FOR SHARE`
    Share,
}

/// One WHERE predicate leaf. `IsNull`/`NotNull` ignore `values`.
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

/// One element of a WHERE/HAVING expression tree. Constructed through the
/// `Select` builder methods; the variants exist so the renderer can express
/// shapes a single column/value predicate cannot (column comparisons, row
/// constructors, subqueries, nesting).
#[derive(Debug, Clone)]
pub enum Clause {
    /// Leaf predicate (`column <op> value(s)`).
    Pred(Condition),
    /// Column-to-column comparison (`left <op> right`), no binds.
    Column {
        left: Ident,
        operator: Operator,
        right: Ident,
    },
    /// Composite row comparison for keyset pagination
    /// (`(a, b) > ($1, $2)`).
    Row {
        columns: Vec<Ident>,
        operator: Operator,
        values: Vec<Value>,
    },
    /// Date-part equality (`created_at::date = $1::date`, `EXTRACT(...) = ...`).
    DatePart {
        part: DatePart,
        column: Ident,
        value: Value,
    },
    /// `EXISTS (subquery)` / `NOT EXISTS (subquery)`; the subquery renders
    /// into the caller's bind sequence (continuous `$N` numbering).
    Exists {
        negated: bool,
        subquery: Box<Select>,
    },
    /// Parenthesized nested group with its own junctions.
    Group {
        junction: Junction,
        items: Vec<(Junction, Clause)>,
    },
    /// Aggregate comparison — HAVING-only (`COUNT(*) > $1`); rejected in a
    /// WHERE context by the renderer.
    AggregateCmp {
        aggregate: Aggregate,
        operator: Operator,
        value: Value,
    },
}

/// Compiled SELECT statement: SQL text with the dialect's placeholders and
/// the bind values in matching order.
///
/// `dialect` records the engine the statement was rendered for, so an
/// adapter can fail closed on a statement built for the wrong engine
/// instead of sending it and reading the server's error.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompiledSelect {
    pub sql: String,
    pub params: Vec<Value>,
    pub dialect: crate::dialect::DialectId,
}

/// Typed SELECT builder. SQL is generated only by [`Select::build`]; every
/// identifier went through [`Ident`] validation on the way in.
#[derive(Debug, Clone, Default)]
pub struct Select {
    table: Option<Ident>,
    columns: Vec<Ident>,
    clauses: Vec<(Junction, Clause)>,
    joins: Vec<Join>,
    order: Vec<(Ident, Direction)>,
    limit: Option<u64>,
    offset: Option<u64>,
    group_by: Vec<Ident>,
    having: Vec<(Junction, Clause)>,
    distinct: bool,
    lock: Option<Lock>,
    aggregate: Option<Aggregate>,
    aggregate_alias: Option<Ident>,
    exists: Option<bool>,
    unions: Vec<(bool, Select)>,
    /// Validation deferred from infallible builder methods to `build`.
    deferred_error: Option<String>,
}

/// Identifier for a fixed, always-valid column (no Result plumbing needed).
fn static_ident(raw: &str) -> Ident {
    Ident(raw.to_string())
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

    /// Batch projection: `SELECT a, b, ...` (empty iterator leaves `*`).
    pub fn select(mut self, columns: impl IntoIterator<Item = Ident>) -> Self {
        self.columns.extend(columns);
        self
    }

    // ---------------------------------------------------------------- WHERE
    // AND-joined family. Builder methods are infallible; validation happens
    // at `build`.

    /// Filter: column equals value.
    pub fn where_eq(mut self, column: Ident, value: impl Into<Value>) -> Self {
        self.push_and(Condition {
            column,
            operator: Operator::Eq,
            values: vec![value.into()],
        });
        self
    }

    /// Filter with an explicit operator and one value.
    pub fn where_op(mut self, column: Ident, operator: Operator, value: impl Into<Value>) -> Self {
        self.push_and(Condition {
            column,
            operator,
            values: vec![value.into()],
        });
        self
    }

    /// Filter: column IN (values...). Empty value list is a build error.
    pub fn where_in(mut self, column: Ident, values: Vec<Value>) -> Self {
        self.push_and(Condition {
            column,
            operator: Operator::In,
            values,
        });
        self
    }

    /// Filter: column NOT IN (values...). Empty value list is a build error.
    pub fn where_not_in(mut self, column: Ident, values: Vec<Value>) -> Self {
        self.push_and(Condition {
            column,
            operator: Operator::NotIn,
            values,
        });
        self
    }

    /// Filter: `column BETWEEN low AND high`.
    pub fn where_between(
        mut self,
        column: Ident,
        low: impl Into<Value>,
        high: impl Into<Value>,
    ) -> Self {
        self.push_and(Condition {
            column,
            operator: Operator::Between,
            values: vec![low.into(), high.into()],
        });
        self
    }

    /// Filter: `column NOT BETWEEN low AND high`.
    pub fn where_not_between(
        mut self,
        column: Ident,
        low: impl Into<Value>,
        high: impl Into<Value>,
    ) -> Self {
        self.push_and(Condition {
            column,
            operator: Operator::NotBetween,
            values: vec![low.into(), high.into()],
        });
        self
    }

    pub fn where_null(mut self, column: Ident) -> Self {
        self.push_and(Condition {
            column,
            operator: Operator::IsNull,
            values: vec![],
        });
        self
    }

    pub fn where_not_null(mut self, column: Ident) -> Self {
        self.push_and(Condition {
            column,
            operator: Operator::NotNull,
            values: vec![],
        });
        self
    }

    /// Column-to-column equality (`left = right`), no binds.
    pub fn where_column(mut self, left: Ident, right: Ident) -> Self {
        self.clauses.push((
            Junction::And,
            Clause::Column {
                left,
                operator: Operator::Eq,
                right,
            },
        ));
        self
    }

    /// Column-to-column comparison with an explicit scalar operator.
    pub fn where_column_op(mut self, left: Ident, operator: Operator, right: Ident) -> Self {
        self.clauses.push((
            Junction::And,
            Clause::Column {
                left,
                operator,
                right,
            },
        ));
        self
    }

    /// Case-insensitive match: renders `column ILIKE $n`. (Exact `LIKE` via
    /// `where_op(.., Operator::Like, ..)`.)
    pub fn where_like(mut self, column: Ident, pattern: impl Into<Value>) -> Self {
        self.push_and(Condition {
            column,
            operator: Operator::Ilike,
            values: vec![pattern.into()],
        });
        self
    }

    /// Filter on the DATE part of a timestamp column
    /// (`column::date = $n::date`).
    pub fn where_date(mut self, column: Ident, value: impl Into<Value>) -> Self {
        self.push_date(DatePart::Date, column, value.into());
        self
    }

    /// Filter on the YEAR part (`EXTRACT(YEAR FROM column) = $n`).
    pub fn where_year(mut self, column: Ident, value: impl Into<Value>) -> Self {
        self.push_date(DatePart::Year, column, value.into());
        self
    }

    /// Filter on the MONTH part (`EXTRACT(MONTH FROM column) = $n`).
    pub fn where_month(mut self, column: Ident, value: impl Into<Value>) -> Self {
        self.push_date(DatePart::Month, column, value.into());
        self
    }

    /// Filter on the DAY part (`EXTRACT(DAY FROM column) = $n`).
    pub fn where_day(mut self, column: Ident, value: impl Into<Value>) -> Self {
        self.push_date(DatePart::Day, column, value.into());
        self
    }

    /// Filter on the TIME part (`TO_CHAR(column, 'HH24:MI:SS') = $n`).
    pub fn where_time(mut self, column: Ident, value: impl Into<Value>) -> Self {
        self.push_date(DatePart::Time, column, value.into());
        self
    }

    /// `EXISTS (subquery)` — the subquery's binds merge into the outer
    /// sequence (continuous `$N`), so correlation stays parameterized.
    pub fn where_exists(mut self, subquery: Select) -> Self {
        self.clauses.push((
            Junction::And,
            Clause::Exists {
                negated: false,
                subquery: Box::new(subquery),
            },
        ));
        self
    }

    /// `NOT EXISTS (subquery)`.
    pub fn where_not_exists(mut self, subquery: Select) -> Self {
        self.clauses.push((
            Junction::And,
            Clause::Exists {
                negated: true,
                subquery: Box::new(subquery),
            },
        ));
        self
    }

    /// `OR EXISTS (subquery)` — existence filter OR-joined to the preceding
    /// clause (`orWhereHas`); the first clause of a list stays implicit.
    pub fn or_where_exists(mut self, subquery: Select) -> Self {
        self.clauses.push((
            Junction::Or,
            Clause::Exists {
                negated: false,
                subquery: Box::new(subquery),
            },
        ));
        self
    }

    /// `OR NOT EXISTS (subquery)` — negated existence OR-joined to the
    /// preceding clause (`orWhereDoesntHave`).
    pub fn or_where_not_exists(mut self, subquery: Select) -> Self {
        self.clauses.push((
            Junction::Or,
            Clause::Exists {
                negated: true,
                subquery: Box::new(subquery),
            },
        ));
        self
    }

    /// Composite row comparison for keyset pagination: `(a, b) > ($1, $2)`.
    /// Columns and values must be non-empty and equal in length; ordering
    /// operators only (`=`, `<>`, `<`, `<=`, `>`, `>=`) — checked at build.
    pub fn where_row_cmp(
        mut self,
        columns: Vec<Ident>,
        operator: Operator,
        values: Vec<Value>,
    ) -> Self {
        self.clauses.push((
            Junction::And,
            Clause::Row {
                columns,
                operator,
                values,
            },
        ));
        self
    }

    // OR-joined family. SQL precedence applies to flat mixes (`a AND b OR c`
    // = `(a AND b) OR c`) — pin grouping with `group()` when mixing.

    /// OR-filter with an explicit operator and one value.
    pub fn or_where(mut self, column: Ident, operator: Operator, value: impl Into<Value>) -> Self {
        self.push_or(Condition {
            column,
            operator,
            values: vec![value.into()],
        });
        self
    }

    pub fn or_where_in(mut self, column: Ident, values: Vec<Value>) -> Self {
        self.push_or(Condition {
            column,
            operator: Operator::In,
            values,
        });
        self
    }

    pub fn or_where_not_in(mut self, column: Ident, values: Vec<Value>) -> Self {
        self.push_or(Condition {
            column,
            operator: Operator::NotIn,
            values,
        });
        self
    }

    pub fn or_where_null(mut self, column: Ident) -> Self {
        self.push_or(Condition {
            column,
            operator: Operator::IsNull,
            values: vec![],
        });
        self
    }

    pub fn or_where_not_null(mut self, column: Ident) -> Self {
        self.push_or(Condition {
            column,
            operator: Operator::NotNull,
            values: vec![],
        });
        self
    }

    pub fn or_where_between(
        mut self,
        column: Ident,
        low: impl Into<Value>,
        high: impl Into<Value>,
    ) -> Self {
        self.push_or(Condition {
            column,
            operator: Operator::Between,
            values: vec![low.into(), high.into()],
        });
        self
    }

    pub fn or_where_not_between(
        mut self,
        column: Ident,
        low: impl Into<Value>,
        high: impl Into<Value>,
    ) -> Self {
        self.push_or(Condition {
            column,
            operator: Operator::NotBetween,
            values: vec![low.into(), high.into()],
        });
        self
    }

    /// OR case-insensitive match (see [`Select::where_like`]).
    pub fn or_where_like(mut self, column: Ident, pattern: impl Into<Value>) -> Self {
        self.push_or(Condition {
            column,
            operator: Operator::Ilike,
            values: vec![pattern.into()],
        });
        self
    }

    /// OR column-to-column equality.
    pub fn or_where_column(mut self, left: Ident, right: Ident) -> Self {
        self.clauses.push((
            Junction::Or,
            Clause::Column {
                left,
                operator: Operator::Eq,
                right,
            },
        ));
        self
    }

    /// Nested boolean group: the closure receives a fresh builder and its
    /// conditions render parenthesized — `... AND (a = $1 OR b = $2)`.
    /// Anything beyond conditions in the closure (joins, order, limit,
    /// projection, ...) is a build error — groups carry predicates only.
    pub fn group(mut self, build: impl FnOnce(Select) -> Select) -> Self {
        let inner = build(Select::default());
        let conditions_only = inner.table.is_none()
            && !inner.distinct
            && inner.aggregate.is_none()
            && inner.aggregate_alias.is_none()
            && inner.exists.is_none()
            && inner.columns.is_empty()
            && inner.joins.is_empty()
            && inner.order.is_empty()
            && inner.limit.is_none()
            && inner.offset.is_none()
            && inner.group_by.is_empty()
            && inner.having.is_empty()
            && inner.unions.is_empty();
        if !conditions_only {
            self.deferred_error = Some(
                "group() closure accepts conditions only (projection/order/limit/...) not allowed"
                    .to_string(),
            );
            return self;
        }
        self.clauses.push((
            Junction::And,
            Clause::Group {
                junction: Junction::And,
                items: inner.clauses,
            },
        ));
        self
    }

    // --------------------------------------------------------------- HAVING

    pub fn having_eq(mut self, column: Ident, value: impl Into<Value>) -> Self {
        self.push_having(Condition {
            column,
            operator: Operator::Eq,
            values: vec![value.into()],
        });
        self
    }

    /// HAVING with an explicit operator and one value — the same operator
    /// set as the WHERE family.
    pub fn having_op(mut self, column: Ident, operator: Operator, value: impl Into<Value>) -> Self {
        self.push_having(Condition {
            column,
            operator,
            values: vec![value.into()],
        });
        self
    }

    pub fn having_in(mut self, column: Ident, values: Vec<Value>) -> Self {
        self.push_having(Condition {
            column,
            operator: Operator::In,
            values,
        });
        self
    }

    pub fn having_between(
        mut self,
        column: Ident,
        low: impl Into<Value>,
        high: impl Into<Value>,
    ) -> Self {
        self.push_having(Condition {
            column,
            operator: Operator::Between,
            values: vec![low.into(), high.into()],
        });
        self
    }

    pub fn having_null(mut self, column: Ident) -> Self {
        self.push_having(Condition {
            column,
            operator: Operator::IsNull,
            values: vec![],
        });
        self
    }

    pub fn having_not_null(mut self, column: Ident) -> Self {
        self.push_having(Condition {
            column,
            operator: Operator::NotNull,
            values: vec![],
        });
        self
    }

    /// Aggregate HAVING: `SUM(total) > $n` / `HAVING COUNT(*) > $n`.
    pub fn having_aggregate(
        mut self,
        aggregate: Aggregate,
        operator: Operator,
        value: impl Into<Value>,
    ) -> Self {
        self.having.push((
            Junction::And,
            Clause::AggregateCmp {
                aggregate,
                operator,
                value: value.into(),
            },
        ));
        self
    }

    // -------------------------------------------- GROUPING / SETS / LOCKING

    /// GROUP BY column (repeatable).
    pub fn group_by(mut self, column: Ident) -> Self {
        self.group_by.push(column);
        self
    }

    /// `SELECT DISTINCT`.
    pub fn distinct(mut self) -> Self {
        self.distinct = true;
        self
    }

    /// Set operation: `... UNION (other)`. The main branch's ORDER/LIMIT/
    /// OFFSET move to the composite end; each union operand keeps its own
    /// tail inside its parentheses.
    pub fn union(mut self, other: Select) -> Self {
        self.unions.push((false, other));
        self
    }

    /// `UNION ALL` (keeps duplicates).
    pub fn union_all(mut self, other: Select) -> Self {
        self.unions.push((true, other));
        self
    }

    /// Locking clause: `SELECT ... FOR UPDATE` (rows for update).
    pub fn lock_for_update(mut self) -> Self {
        self.lock = Some(Lock::Update);
        self
    }

    /// Locking clause: `SELECT ... FOR SHARE` (share lock).
    pub fn shared_lock(mut self) -> Self {
        self.lock = Some(Lock::Share);
        self
    }

    // ------------------------------------------------------------- ORDERING

    pub fn order_by(mut self, column: Ident, direction: Direction) -> Self {
        self.order.push((column, direction));
        self
    }

    /// Newest-first on `created_at` (Laravel `latest()` default).
    pub fn latest(mut self) -> Self {
        self.order
            .push((static_ident("created_at"), Direction::Desc));
        self
    }

    /// Newest-first on an explicit column.
    pub fn latest_by(mut self, column: Ident) -> Self {
        self.order.push((column, Direction::Desc));
        self
    }

    /// Oldest-first on `created_at`.
    pub fn oldest(mut self) -> Self {
        self.order.push((static_ident("created_at"), Direction::Asc));
        self
    }

    /// Oldest-first on an explicit column.
    pub fn oldest_by(mut self, column: Ident) -> Self {
        self.order.push((column, Direction::Asc));
        self
    }

    // ------------------------------------------------------------ AGGREGATES

    /// Aggregate projection (`SELECT COUNT(*) FROM ...`). Conflicts with an
    /// explicit column list (build error).
    pub fn aggregate(mut self, aggregate: Aggregate) -> Self {
        self.aggregate = Some(aggregate);
        self
    }

    /// `SELECT COUNT(*)`.
    pub fn count(self) -> Self {
        self.aggregate(Aggregate::Count)
    }

    /// `SELECT SUM(column)`.
    pub fn sum(self, column: Ident) -> Self {
        self.aggregate(Aggregate::Sum(column))
    }

    /// `SELECT AVG(column)`.
    pub fn avg(self, column: Ident) -> Self {
        self.aggregate(Aggregate::Avg(column))
    }

    /// `SELECT MIN(column)`.
    pub fn min(self, column: Ident) -> Self {
        self.aggregate(Aggregate::Min(column))
    }

    /// `SELECT MAX(column)`.
    pub fn max(self, column: Ident) -> Self {
        self.aggregate(Aggregate::Max(column))
    }

    /// Alias for the aggregate projection (`... AS alias`). Requires an
    /// aggregate (build error otherwise).
    pub fn alias(mut self, alias: Ident) -> Self {
        self.aggregate_alias = Some(alias);
        self
    }

    /// Wrap the whole statement: `SELECT EXISTS (composite)`.
    pub fn exists(mut self) -> Self {
        self.exists = Some(true);
        self
    }

    /// Wrap the whole statement: `SELECT NOT EXISTS (composite)`.
    pub fn doesnt_exist(mut self) -> Self {
        self.exists = Some(false);
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

    // ---------------------------------------------------------------- JOINS

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

    /// TENANT-plane scoping helper (L11 tie-in): append `column = $tenant`.
    pub fn tenant_scope(self, column: Ident, tenant: kelson_foundation::TenantId) -> Self {
        self.where_eq(column, Value::Uuid(*tenant.as_uuid()))
    }

    // ---------------------------------------------------------- CONDITIONAL

    /// Apply the closure only when `condition` holds (Laravel `when`).
    pub fn when(self, condition: bool, build: impl FnOnce(Self) -> Self) -> Self {
        if condition {
            build(self)
        } else {
            self
        }
    }

    /// Inspect/mutate the builder mid-chain (Laravel `tap`).
    pub fn tap(mut self, build: impl FnOnce(&mut Self)) -> Self {
        build(&mut self);
        self
    }

    // ------------------------------------------------------------- INTERNALS

    fn push_and(&mut self, condition: Condition) {
        self.clauses.push((Junction::And, Clause::Pred(condition)));
    }

    fn push_or(&mut self, condition: Condition) {
        self.clauses.push((Junction::Or, Clause::Pred(condition)));
    }

    fn push_having(&mut self, condition: Condition) {
        self.having.push((Junction::And, Clause::Pred(condition)));
    }

    fn push_date(&mut self, part: DatePart, column: Ident, value: Value) {
        self.clauses.push((
            Junction::And,
            Clause::DatePart {
                part,
                column,
                value,
            },
        ));
    }

    // -------------------------------------------------------------- RENDER

    /// Validate shape and generate SQL + ordered binds.
    pub fn build(&self, dialect: &dyn crate::dialect::Dialect) -> Result<CompiledSelect, AppError> {
        let mut params: Vec<Value> = Vec::new();
        let sql = self.render(&mut params, dialect)?;
        Ok(CompiledSelect {
            sql,
            params,
            dialect: dialect.id(),
        })
    }

    /// Render the full statement, appending binds to the CALLER's sequence —
    /// this is how subqueries and union operands keep the placeholder
    /// numbering continuous with the outer query.
    pub(crate) fn render(
        &self,
        params: &mut Vec<Value>,
        dialect: &dyn crate::dialect::Dialect,
    ) -> Result<String, AppError> {
        if let Some(message) = &self.deferred_error {
            return Err(AppError::validation(message.clone()));
        }
        let table = self
            .table
            .as_ref()
            .ok_or_else(|| AppError::validation("SELECT requires a table"))?;
        if self.aggregate_alias.is_some() && self.aggregate.is_none() {
            return Err(AppError::validation(
                "aggregate alias requires an aggregate projection",
            ));
        }
        if self.aggregate.is_some() && !self.columns.is_empty() {
            return Err(AppError::validation(
                "aggregate projection conflicts with explicit column list",
            ));
        }
        let composite = self.render_composite(table, params, dialect)?;
        Ok(match self.exists {
            None => composite,
            Some(true) => format!("SELECT EXISTS ({composite})"),
            Some(false) => format!("SELECT NOT EXISTS ({composite})"),
        })
    }

    fn render_composite(
        &self,
        table: &Ident,
        params: &mut Vec<Value>,
        dialect: &dyn crate::dialect::Dialect,
    ) -> Result<String, AppError> {
        let mut sql = String::with_capacity(128);

        sql.push_str("SELECT ");
        if self.distinct {
            sql.push_str("DISTINCT ");
        }
        match &self.aggregate {
            Some(aggregate) => {
                sql.push_str(&aggregate.render());
                if let Some(alias) = &self.aggregate_alias {
                    sql.push_str(" AS ");
                    sql.push_str(alias.as_str());
                }
            }
            None if self.columns.is_empty() => sql.push('*'),
            None => {
                let cols: Vec<&str> = self.columns.iter().map(|c| c.as_str()).collect();
                sql.push_str(&cols.join(", "));
            }
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

        if !self.clauses.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&crate::render_conditions(
                &self.clauses,
                params,
                crate::ClauseContext::Where,
                dialect,
            )?);
        }

        if !self.group_by.is_empty() {
            let cols: Vec<&str> = self.group_by.iter().map(|c| c.as_str()).collect();
            sql.push_str(" GROUP BY ");
            sql.push_str(&cols.join(", "));
        }

        if !self.having.is_empty() {
            sql.push_str(" HAVING ");
            sql.push_str(&crate::render_conditions(
                &self.having,
                params,
                crate::ClauseContext::Having,
                dialect,
            )?);
        }

        if self.unions.is_empty() {
            self.render_tail(&mut sql, params, dialect);
        } else {
            for (all, branch) in &self.unions {
                sql.push_str(if *all { " UNION ALL (" } else { " UNION (" });
                sql.push_str(&branch.render(params, dialect)?);
                sql.push(')');
            }
            self.render_tail(&mut sql, params, dialect);
        }
        Ok(sql)
    }

    /// ORDER BY / LIMIT / OFFSET / locking — always the statement tail.
    fn render_tail(
        &self,
        sql: &mut String,
        params: &mut Vec<Value>,
        dialect: &dyn crate::dialect::Dialect,
    ) {
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
            sql.push_str(&format!(" LIMIT {}", dialect.placeholder(params.len())));
        }
        if let Some(n) = self.offset {
            params.push(Value::Int(n as i64));
            sql.push_str(&format!(" OFFSET {}", dialect.placeholder(params.len())));
        }
        if let Some(lock) = self.lock {
            sql.push(' ');
            sql.push_str(&dialect.render_lock(lock));
        }
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
        let q = Select::from_table(id("users")).build(&crate::dialect::POSTGRES).unwrap();
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
            .build(&crate::dialect::POSTGRES)
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
            .build(&crate::dialect::POSTGRES)
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
            .build(&crate::dialect::POSTGRES)
            .unwrap_err();
        assert!(err.public_message.contains("IN"));
    }

    #[test]
    fn tenant_scope_binds_uuid() {
        let tenant = kelson_foundation::TenantId::new();
        let q = Select::from_table(id("records"))
            .tenant_scope(id("tenant_id"), tenant)
            .build(&crate::dialect::POSTGRES)
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

    // ------------------------------------------------------- Wave 3 (phase 1)

    #[test]
    fn or_where_renders_top_level_or() {
        let q = Select::from_table(id("t"))
            .where_eq(id("a"), 1i64)
            .or_where(id("b"), Operator::Eq, 2i64)
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT * FROM t WHERE a = $1 OR b = $2");
        assert_eq!(q.params.len(), 2);
    }

    #[test]
    fn leading_or_where_renders_as_first_clause() {
        let q = Select::from_table(id("t"))
            .or_where_null(id("a"))
            .where_eq(id("b"), 1i64)
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT * FROM t WHERE a IS NULL AND b = $1");
    }

    #[test]
    fn flat_mixture_follows_sql_precedence() {
        // Documented: AND binds tighter than OR — `a AND b OR c AND d`
        // evaluates as `(a AND b) OR (c AND d)` (Laravel semantics).
        let q = Select::from_table(id("t"))
            .where_eq(id("a"), 1i64)
            .where_eq(id("b"), 2i64)
            .or_where(id("c"), Operator::Eq, 3i64)
            .where_eq(id("d"), 4i64)
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT * FROM t WHERE a = $1 AND b = $2 OR c = $3 AND d = $4"
        );
    }

    #[test]
    fn nested_group_renders_parenthesized() {
        let q = Select::from_table(id("t"))
            .where_eq(id("tenant_id"), 1i64)
            .group(|g| g.where_eq(id("a"), 2i64).or_where(id("b"), Operator::Eq, 3i64))
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT * FROM t WHERE tenant_id = $1 AND (a = $2 OR b = $3)"
        );
        assert_eq!(q.params.len(), 3);
    }

    #[test]
    fn group_rejects_non_condition_builder() {
        let err = Select::from_table(id("t"))
            .group(|g| g.where_eq(id("a"), 1i64).limit(5))
            .build(&crate::dialect::POSTGRES)
            .unwrap_err();
        assert!(err.public_message.contains("conditions only"));

        // An empty group is a build error, not a silent `()`.
        let err = Select::from_table(id("t"))
            .group(|g| g)
            .build(&crate::dialect::POSTGRES)
            .unwrap_err();
        assert!(err.public_message.contains("group"));
    }

    #[test]
    fn where_not_in_shape_and_empty_rejected() {
        let q = Select::from_table(id("t"))
            .where_not_in(id("status"), vec!["a".into(), "b".into()])
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT * FROM t WHERE status NOT IN ($1, $2)");

        let err = Select::from_table(id("t"))
            .where_not_in(id("status"), vec![])
            .build(&crate::dialect::POSTGRES)
            .unwrap_err();
        assert!(err.public_message.contains("IN"));
    }

    #[test]
    fn where_between_shapes() {
        let q = Select::from_table(id("t"))
            .where_between(id("n"), 1i64, 5i64)
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT * FROM t WHERE n BETWEEN $1 AND $2");

        let q = Select::from_table(id("t"))
            .where_not_between(id("n"), 1i64, 5i64)
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT * FROM t WHERE n NOT BETWEEN $1 AND $2");
    }

    #[test]
    fn where_column_shapes_and_operator_guard() {
        let q = Select::from_table(id("schedules"))
            .where_column(id("starts_at"), id("ends_at"))
            .where_column_op(id("a"), Operator::Gt, id("b"))
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT * FROM schedules WHERE starts_at = ends_at AND a > b"
        );
        assert!(q.params.is_empty());

        let err = Select::from_table(id("t"))
            .where_column_op(id("a"), Operator::In, id("b"))
            .build(&crate::dialect::POSTGRES)
            .unwrap_err();
        assert!(err.public_message.contains("operator"));
    }

    #[test]
    fn where_like_renders_ilike() {
        let q = Select::from_table(id("doctors"))
            .where_like(id("name"), "%jo%")
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT * FROM doctors WHERE name ILIKE $1");
        assert_eq!(q.params[0], Value::Text("%jo%".into()));
    }

    #[test]
    fn date_family_shapes() {
        let q = Select::from_table(id("t"))
            .where_date(id("created_at"), "2026-01-01")
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT * FROM t WHERE created_at::date = $1::date");

        let q = Select::from_table(id("t"))
            .where_year(id("created_at"), 2026i64)
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT * FROM t WHERE EXTRACT(YEAR FROM created_at) = $1");

        let q = Select::from_table(id("t"))
            .where_month(id("created_at"), 9i64)
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT * FROM t WHERE EXTRACT(MONTH FROM created_at) = $1"
        );

        let q = Select::from_table(id("t"))
            .where_day(id("created_at"), 29i64)
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT * FROM t WHERE EXTRACT(DAY FROM created_at) = $1");

        let q = Select::from_table(id("t"))
            .where_time(id("starts_at"), "09:30:00")
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT * FROM t WHERE TO_CHAR(starts_at, 'HH24:MI:SS') = $1"
        );
    }

    #[test]
    fn distinct_projection_and_batch_select() {
        let q = Select::from_table(id("t"))
            .distinct()
            .select(vec![id("a"), id("b")])
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT DISTINCT a, b FROM t");
    }

    #[test]
    fn latest_oldest_default_and_overridden_columns() {
        let q = Select::from_table(id("posts")).latest().build(&crate::dialect::POSTGRES).unwrap();
        assert_eq!(q.sql, "SELECT * FROM posts ORDER BY created_at DESC");

        let q = Select::from_table(id("posts")).oldest().build(&crate::dialect::POSTGRES).unwrap();
        assert_eq!(q.sql, "SELECT * FROM posts ORDER BY created_at ASC");

        let q = Select::from_table(id("posts"))
            .latest_by(id("published_at"))
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT * FROM posts ORDER BY published_at DESC");

        let q = Select::from_table(id("posts"))
            .oldest_by(id("published_at"))
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT * FROM posts ORDER BY published_at ASC");
    }

    #[test]
    fn lock_clauses_render_after_limit_offset() {
        let q = Select::from_table(id("jobs"))
            .lock_for_update()
            .limit(10)
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT * FROM jobs LIMIT $1 FOR UPDATE");

        let q = Select::from_table(id("jobs"))
            .shared_lock()
            .limit(5)
            .offset(10)
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT * FROM jobs LIMIT $1 OFFSET $2 FOR SHARE");
    }

    #[test]
    fn group_by_and_having_shapes() {
        let q = Select::from_table(id("bookings"))
            .group_by(id("clinic_id"))
            .having_op(id("clinic_id"), Operator::Eq, "x")
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT * FROM bookings GROUP BY clinic_id HAVING clinic_id = $1"
        );

        let q = Select::from_table(id("bookings"))
            .group_by(id("clinic_id"))
            .having_aggregate(Aggregate::Count, Operator::Gt, 5i64)
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT * FROM bookings GROUP BY clinic_id HAVING COUNT(*) > $1"
        );

        let q = Select::from_table(id("bookings"))
            .group_by(id("clinic_id"))
            .having_in(id("status"), vec!["open".into()])
            .having_between(id("n"), 1i64, 3i64)
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT * FROM bookings GROUP BY clinic_id HAVING status IN ($1) AND n BETWEEN $2 AND $3"
        );
    }

    #[test]
    fn union_and_union_all_shapes() {
        let branch = Select::from_table(id("b")).where_eq(id("y"), 2i64);
        let q = Select::from_table(id("a"))
            .where_eq(id("x"), 1i64)
            .union(branch)
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT * FROM a WHERE x = $1 UNION (SELECT * FROM b WHERE y = $2)"
        );
        assert_eq!(q.params.len(), 2);

        let q = Select::from_table(id("a"))
            .union_all(Select::from_table(id("b")))
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT * FROM a UNION ALL (SELECT * FROM b)");
    }

    #[test]
    fn union_moves_main_tail_to_composite_end() {
        let q = Select::from_table(id("a"))
            .where_eq(id("x"), 1i64)
            .order_by(id("id"), Direction::Desc)
            .limit(5)
            .union(Select::from_table(id("b")))
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT * FROM a WHERE x = $1 UNION (SELECT * FROM b) ORDER BY id DESC LIMIT $2"
        );
        assert_eq!(q.params, vec![Value::Int(1), Value::Int(5)]);
    }

    #[test]
    fn union_branch_keeps_its_own_tail_inside_parens() {
        let branch = Select::from_table(id("b"))
            .order_by(id("y"), Direction::Asc)
            .limit(3);
        let q = Select::from_table(id("a"))
            .union(branch)
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT * FROM a UNION (SELECT * FROM b ORDER BY y ASC LIMIT $1)"
        );
    }

    #[test]
    fn aggregate_projection_shapes() {
        let q = Select::from_table(id("appointments"))
            .count()
            .where_eq(id("status"), "done")
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT COUNT(*) FROM appointments WHERE status = $1");

        let q = Select::from_table(id("invoices"))
            .sum(id("total"))
            .alias(id("total_sum"))
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT SUM(total) AS total_sum FROM invoices");

        let q = Select::from_table(id("invoices")).avg(id("total")).build(&crate::dialect::POSTGRES).unwrap();
        assert_eq!(q.sql, "SELECT AVG(total) FROM invoices");

        let q = Select::from_table(id("invoices")).min(id("total")).build(&crate::dialect::POSTGRES).unwrap();
        assert_eq!(q.sql, "SELECT MIN(total) FROM invoices");

        let q = Select::from_table(id("invoices")).max(id("total")).build(&crate::dialect::POSTGRES).unwrap();
        assert_eq!(q.sql, "SELECT MAX(total) FROM invoices");

        let q = Select::from_table(id("bookings"))
            .aggregate(Aggregate::CountColumn(id("doctor_id")))
            .alias(id("doctor_count"))
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT COUNT(doctor_id) AS doctor_count FROM bookings"
        );
    }

    #[test]
    fn aggregate_validation_conflicts() {
        let err = Select::from_table(id("t"))
            .count()
            .column(id("a"))
            .build(&crate::dialect::POSTGRES)
            .unwrap_err();
        assert!(err.public_message.contains("aggregate"));

        let err = Select::from_table(id("t"))
            .alias(id("x"))
            .build(&crate::dialect::POSTGRES)
            .unwrap_err();
        assert!(err.public_message.contains("alias"));
    }

    #[test]
    fn exists_wrappers_and_correlated_subquery() {
        let subquery = Select::from_table(id("doctors"))
            .where_column(id("doctors.id"), id("appointments.doctor_id"))
            .where_eq(id("doctors.specialty"), "cardio");
        let q = Select::from_table(id("appointments"))
            .where_exists(subquery)
            .where_eq(id("status"), "booked")
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT * FROM appointments WHERE EXISTS (SELECT * FROM doctors WHERE doctors.id = appointments.doctor_id AND doctors.specialty = $1) AND status = $2"
        );
        // Subquery binds merge into the outer sequence, in SQL text order.
        assert_eq!(
            q.params,
            vec![Value::Text("cardio".into()), Value::Text("booked".into())]
        );

        let q = Select::from_table(id("t"))
            .where_not_exists(Select::from_table(id("u")).where_eq(id("u.t_id"), 1i64))
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT * FROM t WHERE NOT EXISTS (SELECT * FROM u WHERE u.t_id = $1)"
        );
    }

    #[test]
    fn or_where_exists_joins_the_preceding_clause() {
        let q = Select::from_table(id("patients"))
            .where_eq(id("status"), "active")
            .or_where_exists(
                Select::from_table(id("appointments"))
                    .where_column(id("appointments.patient_id"), id("patients.id")),
            )
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT * FROM patients WHERE status = $1 OR EXISTS (SELECT * FROM appointments WHERE appointments.patient_id = patients.id)"
        );

        let q = Select::from_table(id("patients"))
            .where_eq(id("status"), "active")
            .or_where_not_exists(
                Select::from_table(id("appointments"))
                    .where_column(id("appointments.patient_id"), id("patients.id"))
                    .where_eq(id("appointments.status"), "booked"),
            )
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT * FROM patients WHERE status = $1 OR NOT EXISTS (SELECT * FROM appointments WHERE appointments.patient_id = patients.id AND appointments.status = $2)"
        );
        // The first clause's junction is implicit: a leading or_where_exists
        // renders without a dangling OR.
        let q = Select::from_table(id("patients"))
            .or_where_exists(Select::from_table(id("appointments")))
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT * FROM patients WHERE EXISTS (SELECT * FROM appointments)"
        );
    }

    #[test]
    fn exists_and_doesnt_exist_wrap_the_statement() {
        let q = Select::from_table(id("doctors"))
            .where_eq(id("specialty"), "cardio")
            .exists()
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT EXISTS (SELECT * FROM doctors WHERE specialty = $1)"
        );

        let q = Select::from_table(id("doctors")).doesnt_exist().build(&crate::dialect::POSTGRES).unwrap();
        assert_eq!(q.sql, "SELECT NOT EXISTS (SELECT * FROM doctors)");
    }

    #[test]
    fn keyset_row_comparison_shape() {
        let q = Select::from_table(id("events"))
            .where_row_cmp(
                vec![id("created_at"), id("id")],
                Operator::Lt,
                vec![Value::Text("2026-01-01".into()), Value::Int(9)],
            )
            .order_by(id("created_at"), Direction::Desc)
            .order_by(id("id"), Direction::Desc)
            .limit(50)
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(
            q.sql,
            "SELECT * FROM events WHERE (created_at, id) < ($1, $2) ORDER BY created_at DESC, id DESC LIMIT $3"
        );
        assert_eq!(q.params.len(), 3);
    }

    #[test]
    fn row_comparison_validates_shape() {
        let err = Select::from_table(id("t"))
            .where_row_cmp(vec![], Operator::Gt, vec![Value::Int(1)])
            .build(&crate::dialect::POSTGRES)
            .unwrap_err();
        assert!(err.public_message.contains("at least one column"));

        let err = Select::from_table(id("t"))
            .where_row_cmp(vec![id("a"), id("b")], Operator::Gt, vec![Value::Int(1)])
            .build(&crate::dialect::POSTGRES)
            .unwrap_err();
        assert!(err.public_message.contains("values for"));

        let err = Select::from_table(id("t"))
            .where_row_cmp(vec![id("a")], Operator::In, vec![Value::Int(1)])
            .build(&crate::dialect::POSTGRES)
            .unwrap_err();
        assert!(err.public_message.contains("operator"));
    }

    #[test]
    fn values_never_interpolate_into_sql_text() {
        let payload = "%'; DROP TABLE doctors; --";
        let q = Select::from_table(id("doctors"))
            .where_like(id("name"), payload)
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert!(!q.sql.contains("DROP"));
        assert!(!q.sql.contains("'"));
        assert_eq!(q.params[0], Value::Text(payload.to_string()));
    }

    #[test]
    fn when_and_tap_conditionals() {
        let q = Select::from_table(id("t"))
            .when(false, |s| s.where_eq(id("a"), 1i64))
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT * FROM t");

        let q = Select::from_table(id("t"))
            .when(true, |s| s.where_eq(id("a"), 1i64))
            .tap(|s| s.limit = Some(7))
            .build(&crate::dialect::POSTGRES)
            .unwrap();
        assert_eq!(q.sql, "SELECT * FROM t WHERE a = $1 LIMIT $2");
        assert_eq!(q.params, vec![Value::Int(1), Value::Int(7)]);
    }
}
