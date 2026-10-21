//! Durable model store on MySQL 8 (S8 C3/B-04/B-05 twin, Wave 9 B-31 second
//! driver): implements the kelson-models `ModelStore` port by compiling
//! records/filters through the typed kelson-query builders and executing them
//! here. Tenant isolation is applied on EVERY path (`tenant_scope`); soft
//! deletes honor `ModelDef::soft_deletes`.
//!
//! MySQL port notes (divergences from `kelson_postgres::modelstore`):
//!
//! - Placeholders (rule 1): the builder renders **for this engine** through
//!   `kelson_query::MYSQL`, so the compiled SQL already carries ordered `?`
//!   placeholders. kelson-query appends binds in SQL-text order, so
//!   appearance order == bind order across WHERE / ORDER BY / LIMIT /
//!   OFFSET (the same positional trust the pg twin places on its binds).
//!   Nothing formats a literal number into SQL. **No SQL is rewritten after
//!   the fact**: the earlier `qmarks` / `to_mysql_upsert` textual rewrites
//!   are gone, replaced by the dialect layer in `kelson-query`.
//! - No RETURNING (rule 10): every write executes, then the row(s) come
//!   from a follow-up SELECT on the same pool under the SAME scope
//!   predicates, projecting `all_columns(model)` -- byte-equivalent to the
//!   pg `RETURNING` row. `update_filtered` captures the matched keys BEFORE
//!   the write and re-reads them after (changes may stop rows from matching
//!   their own filters, which pg's RETURNING would still return); hard
//!   `delete_filtered` reads the pre-image first, then deletes (pg's
//!   `DELETE ... RETURNING` yields the rows as they were). Readbacks are
//!   separate statements, so they are not atomic with the write.
//! - No RLS (rule 11): every `tenant_id` predicate the pg twin carries is
//!   kept verbatim, including the `tenant_id = NULL` match-nothing posture
//!   for scoped models queried/written without a tenant, and each keyed
//!   readback repeats the write's tenant scope (plus the soft veil only
//!   where the write had one), so a foreign-tenant write re-selects nothing
//!   exactly like pg's empty RETURNING. The `insert`/`insert_or_ignore`/
//!   `upsert` readbacks add the record's `tenant_id` scope (defense in
//!   depth: the write guard already pinned that value into the row).
//! - rows_affected (rule 16): `delete` (soft and hard), `force_delete` and
//!   `restore` branch on `rows_affected`; every other write re-selects
//!   unconditionally and never consults it. Soft delete and restore always
//!   change a matched row (`deleted_at` NULL <-> stamp) and DELETE counts
//!   removed rows, so the branches equal pg's RETURNING-presence with OR
//!   without CLIENT_FOUND_ROWS.
//! - `insert_or_ignore` (rule 17): pg's target-less `ON CONFLICT DO
//!   NOTHING` has no MySQL form; a plain INSERT runs and a duplicate-key
//!   error (1062, `is_unique_violation`) maps to `Ok(None)` -- any OTHER
//!   error still surfaces, exactly like pg. `INSERT IGNORE` was rejected:
//!   it would also swallow the not-null/range errors pg raises. Plain
//!   `insert` does NOT swallow 1062, so the appointments partial-unique
//!   double-booking guard surfaces as a persistence error, like pg.
//! - `upsert` (rule 7): the builder's conflict intent (`ON CONFLICT (target)
//!   DO UPDATE SET x = EXCLUDED.x`) is rendered by the MySQL dialect as the
//!   8.0.19+ row-alias form `VALUES (...) AS new ON DUPLICATE KEY UPDATE
//!   x = new.x`. The conflict target is NOT expressible: ANY unique key of
//!   the table fires the merge, so a collision on a *different* unique key
//!   merges where pg errors (flagged; the readback then finds no row and
//!   fails closed as a persistence error). `id` stays out of the update
//!   list, so the surviving row keeps its primary key -- the readback
//!   therefore matches `id OR (every
//!   conflict column)` under the record's tenant scope (pg's RETURNING
//!   carries no tenant predicate; scoping it can only turn a cross-tenant
//!   conflict-target collision into an error, never leak a foreign row).
//!   pg additionally requires the target to be a real unique index; MySQL
//!   does not verify that, so the readback takes `LIMIT 1`.
//! - DECIMAL (rule 17): MySQL answers SUM/AVG over exact numerics (and
//!   DECIMAL columns) with DECIMAL, which sqlx-mysql refuses to decode as
//!   i64/f64; aggregate/column decode falls back to the textual value the
//!   protocol carries and re-encodes it (integral text stays an integral
//!   JSON number for int fields, mirroring pg's int8 SUM).
//! - Date fields (rule 17): a VARCHAR date column decodes as `String` like
//!   pg; a native DATE column is read as `time::Date` and rendered
//!   `YYYY-MM-DD`.
//! - Native codecs (rules 2/3/4): uuid binds/gets stay `BINARY(16)`
//!   `Uuid`, timestamps stay `OffsetDateTime` (`DATETIME(6)`, UTC), JSON
//!   stays `serde_json::Value` -- no string round-trips.
//! - `insert` / `insert_or_ignore` / `upsert` re-select by the record's
//!   `id`, so (like `InMemoryModelStore`) the record must carry a uuid `id`
//!   before the write runs; it is validated BEFORE the INSERT executes.
//! - pg-only constructs reachable from the shared builder (`ILIKE`,
//!   `::date`/`TO_CHAR` date parts, `FOR SHARE`, arrays) never occur here:
//!   `FilterOp` compiles only to `= <> IN IS [NOT] NULL < <= > >=`, so no
//!   operator translation was needed.
//! - Generated columns (rule 17): `20260822000002_appointments.sql` emulates
//!   pg's partial unique index with STORED generated `*_active` columns that
//!   no `ModelDef` declares. Reads keep `SELECT *` (like pg) and drop those
//!   columns in [`row_to_record`], so the Record matches the pg row exactly
//!   instead of decoding BINARY(16) as VARCHAR.

use kelson_foundation::{AppError, TenantId};
use kelson_models::{
    json_value_to_bind, record_to_columns_for, ExistsFilter, FieldFilter, FieldType, FilterOp,
    ListOpts, ModelDef, ModelStore, Record,
};
use kelson_models::store::{
    require_write_filters, validate_aggregate, validate_counter, validate_list_opts,
};
use kelson_query::select::{Condition, Direction, Ident, Operator};
use kelson_query::{CompiledSelect, CompiledWrite, Delete, Insert, ConflictPolicy, Select, Update, Value as Bind};
use serde_json::Value as Json;
use sqlx::mysql::{MySqlArguments, MySqlPool, MySqlRow};
use sqlx::{Column, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::bridge::block_on;

pub struct MySqlModelStore {
    /// The primary pool: every write, and every read that must be fresh.
    pool: MySqlPool,
    /// An optional **read replica** pool. It serves ONLY the reads that opt in
    /// (`ListOpts::replica_ok`); without the opt-in every read stays on the
    /// primary, so a read that follows a write still sees it.
    read_pool: Option<MySqlPool>,
    /// The optional cost guard: the ceiling a multi-row write may touch
    /// before it is refused (the cost axis). `None` means unbounded.
    cost_guard: Option<crate::cost::CostGuard>,
    /// The optional cost ledger: what every statement actually cost. Shared
    /// behind a lock so the aggregate survives the store's concurrency (the
    /// store is `Sync` and every call takes `&self`).
    cost_ledger: Option<std::sync::Arc<std::sync::Mutex<crate::cost::CostLedger>>>,
}

impl MySqlModelStore {
    pub fn new(pool: MySqlPool) -> Self {
        Self {
            pool,
            read_pool: None,
            cost_guard: None,
            cost_ledger: None,
        }
    }

    /// Attach a read replica. Reads that opt in (`ListOpts::replica_ok`) are
    /// served from this pool; everything else stays on the primary.
    pub fn with_read_pool(mut self, pool: MySqlPool) -> Self {
        self.read_pool = Some(pool);
        self
    }

    /// Attach a cost guard: a multi-row write that would touch more rows
    /// than the budget allows is **refused before it runs** (fail-closed).
    ///
    /// This is the cost axis: one tenant's runaway batch write must not be
    /// able to consume the cluster's IO while everyone else waits.
    pub fn with_cost_guard(mut self, guard: crate::cost::CostGuard) -> Self {
        self.cost_guard = Some(guard);
        self
    }

    /// Attach a cost ledger: what every statement cost, per tenant.
    ///
    /// The ledger is shared: the caller keeps the handle and reads the
    /// aggregate whenever it wants it.
    pub fn with_cost_ledger(
        mut self,
        ledger: std::sync::Arc<std::sync::Mutex<crate::cost::CostLedger>>,
    ) -> Self {
        self.cost_ledger = Some(ledger);
        self
    }

    /// The pool a read must use: the replica only for an explicitly
    /// replica-eligible read, the primary otherwise.
    fn pool_for_read(&self, opts: &ListOpts) -> &MySqlPool {
        match (&self.read_pool, opts.replica_ok) {
            (Some(read), true) => read,
            _ => &self.pool,
        }
    }
}

fn my(e: impl std::fmt::Display) -> AppError {
    AppError::persistence(format!("model store: {e}"))
}

fn ident(name: &str) -> Result<Ident, AppError> {
    Ident::new(name.to_string())
}


fn bind_params<'q>(
    mut query: sqlx::query::Query<'q, sqlx::MySql, MySqlArguments>,
    params: &'q [Bind],
) -> sqlx::query::Query<'q, sqlx::MySql, MySqlArguments> {
    for param in params {
        query = match param {
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
    query
}

/// Map one row into the canonical Record encoding using the model's field
/// types (framework timestamp columns are typed by convention).
fn row_to_record(row: &MySqlRow, model: &ModelDef) -> Result<Record, AppError> {
    let mut record = Record::new();
    for column in row.columns() {
        let name = column.name();
        // Driver-only STORED generated columns (`tenant_id_active`,
        // `patient_active`, `slot_active` -- the partial-unique-index
        // emulation in 20260822000002_appointments.sql) are not model
        // fields. A pg table has no such columns, so dropping them keeps
        // the Record byte-equivalent across the two drivers; keeping them
        // would also ask VARCHAR out of BINARY(16), because no ModelDef
        // declares a type for them (that is the 503 seen on
        // `GET /api/v1/appointments`). A model that DOES declare an
        // `_active` field keeps it (declared type wins in `decode_column`).
        if name.ends_with("_active") && model.field_named(name).is_none() {
            continue;
        }
        let value = decode_column(row, name, model)?;
        record.insert(name.to_string(), value);
    }
    Ok(record)
}

fn decode_column(row: &MySqlRow, name: &str, model: &ModelDef) -> Result<Json, AppError> {
    let column_type = model
        .field_named(name)
        .map(|f| f.field_type.clone())
        .unwrap_or_else(|| match name {
            "id" | "tenant_id" => FieldType::Uuid,
            "created_at" | "updated_at" | "deleted_at" => FieldType::Timestamp,
            "version" => FieldType::Int64,
            _ => FieldType::Text,
        });
    Ok(match column_type {
        FieldType::Uuid => {
            let v: Option<Uuid> = row.try_get(name).map_err(my)?;
            v.map(|u| Json::String(u.to_string())).unwrap_or(Json::Null)
        }
        FieldType::Bool => {
            let v: Option<bool> = row.try_get(name).map_err(my)?;
            v.map(Json::Bool).unwrap_or(Json::Null)
        }
        FieldType::Int32 => {
            let v: Option<i32> = row.try_get(name).map_err(my)?;
            v.map(|i| Json::from(i64::from(i))).unwrap_or(Json::Null)
        }
        FieldType::Int64 => {
            let v: Option<i64> = row.try_get(name).map_err(my)?;
            v.map(Json::from).unwrap_or(Json::Null)
        }
        FieldType::Float | FieldType::Decimal => decode_numeric_column(row, name)?,
        FieldType::Text | FieldType::TextLarge | FieldType::Enum { .. } => {
            let v: Option<String> = row.try_get(name).map_err(my)?;
            v.map(Json::String).unwrap_or(Json::Null)
        }
        FieldType::Date => decode_date_column(row, name)?,
        FieldType::Timestamp => {
            let v: Option<OffsetDateTime> = row.try_get(name).map_err(my)?;
            match v {
                Some(ts) => Json::String(
                    ts.format(&time::format_description::well_known::Rfc3339)
                        .map_err(my)?,
                ),
                None => Json::Null,
            }
        }
        FieldType::Json => {
            let v: Option<Json> = row.try_get(name).map_err(my)?;
            v.unwrap_or(Json::Null)
        }
    })
}

/// FLOAT/DOUBLE columns decode natively; a DECIMAL column does not
/// (sqlx-mysql refuses DECIMAL as f64) and travels as its textual value
/// instead -- re-encode it as the JSON number pg's numeric decode yields.
fn decode_numeric_column(row: &MySqlRow, name: &str) -> Result<Json, AppError> {
    match row.try_get::<Option<f64>, _>(name) {
        Ok(v) => Ok(v.map(Json::from).unwrap_or(Json::Null)),
        Err(_) => decode_decimal(row, name, false),
    }
}

/// A date field reads as `String` when the column is VARCHAR (the pg
/// shape); a native DATE column does not decode as String on MySQL, so
/// fall back to `time::Date` and render `YYYY-MM-DD`.
fn decode_date_column(row: &MySqlRow, name: &str) -> Result<Json, AppError> {
    match row.try_get::<Option<String>, _>(name) {
        Ok(v) => Ok(v.map(Json::String).unwrap_or(Json::Null)),
        Err(_) => {
            let v: Option<time::Date> = row.try_get(name).map_err(my)?;
            match v {
                Some(d) => Ok(Json::String(format!(
                    "{:04}-{:02}-{:02}",
                    d.year(),
                    u8::from(d.month()),
                    d.day()
                ))),
                None => Ok(Json::Null),
            }
        }
    }
}

/// MySQL answers SUM/AVG over exact numerics (and DECIMAL columns) with
/// DECIMAL, which sqlx-mysql refuses to decode as i64/f64 (rule 17) -- the
/// protocol still carries the value as text, so re-encode from that.
/// `prefer_int` keeps integral text an integral JSON number (pg returns
/// int8 for SUM over integers); otherwise a JSON float (pg's numeric path).
fn decode_decimal(
    row: &MySqlRow,
    index: impl sqlx::ColumnIndex<MySqlRow>,
    prefer_int: bool,
) -> Result<Json, AppError> {
    let text: Option<String> = row.try_get_unchecked(index).map_err(my)?;
    let Some(text) = text else {
        return Ok(Json::Null);
    };
    if prefer_int {
        if let Ok(v) = text.parse::<i64>() {
            return Ok(Json::from(v));
        }
    }
    match text.parse::<f64>() {
        Ok(v) => Ok(Json::from(v)),
        Err(e) => Err(my(e)),
    }
}

/// Single aggregate result value (column 0) in the canonical JSON encoding.
/// COUNT/SUM/AVG come back as int8, float8, or DECIMAL depending on the
/// input column type (MySQL returns DECIMAL where pg returns int8/numeric),
/// so numeric decoding sniffs int first, then float, then the textual
/// DECIMAL form.
fn decode_aggregate(row: &MySqlRow, field_type: Option<&FieldType>) -> Result<Json, AppError> {
    // NULL aggregate (empty match set): Option<T> short-circuits on NULL.
    if let Ok(None) = row.try_get::<Option<i64>, _>(0) {
        return Ok(Json::Null);
    }
    match field_type {
        Some(FieldType::Int32 | FieldType::Int64) => {
            if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(0) {
                return Ok(Json::from(v));
            }
            if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(0) {
                return Ok(Json::from(v));
            }
            decode_decimal(row, 0, true)
        }
        Some(FieldType::Float | FieldType::Decimal) => {
            if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(0) {
                return Ok(Json::from(v));
            }
            if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(0) {
                return Ok(Json::from(v));
            }
            decode_decimal(row, 0, false)
        }
        Some(field_type) => decode_typed_value(row, field_type),
        // COUNT(*): int8.
        None => match row.try_get::<Option<i64>, _>(0) {
            Ok(Some(v)) => Ok(Json::from(v)),
            Ok(None) => Ok(Json::Null),
            Err(e) => Err(my(e)),
        },
    }
}

/// Typed single-value decode (column 0) for MIN/MAX aggregate results --
/// mirrors [`decode_column`]'s encoding for the field's type.
fn decode_typed_value(row: &MySqlRow, field_type: &FieldType) -> Result<Json, AppError> {
    Ok(match field_type {
        FieldType::Uuid => {
            let v: Option<Uuid> = row.try_get(0).map_err(my)?;
            v.map(|u| Json::String(u.to_string())).unwrap_or(Json::Null)
        }
        FieldType::Bool => {
            let v: Option<bool> = row.try_get(0).map_err(my)?;
            v.map(Json::Bool).unwrap_or(Json::Null)
        }
        FieldType::Int32 => {
            // MIN/MAX may answer an int4 field as int8 on MySQL (pg keeps
            // int4); widen before falling through to the DECIMAL text path.
            match row.try_get::<Option<i32>, _>(0) {
                Ok(v) => v.map(|i| Json::from(i64::from(i))).unwrap_or(Json::Null),
                Err(_) => match row.try_get::<Option<i64>, _>(0) {
                    Ok(v) => v.map(Json::from).unwrap_or(Json::Null),
                    Err(_) => return decode_decimal(row, 0, true),
                },
            }
        }
        FieldType::Int64 => {
            let v: Option<i64> = row.try_get(0).map_err(my)?;
            v.map(Json::from).unwrap_or(Json::Null)
        }
        FieldType::Float | FieldType::Decimal => match row.try_get::<Option<f64>, _>(0) {
            Ok(v) => v.map(Json::from).unwrap_or(Json::Null),
            Err(_) => return decode_decimal(row, 0, false),
        },
        FieldType::Text | FieldType::TextLarge | FieldType::Enum { .. } => {
            let v: Option<String> = row.try_get(0).map_err(my)?;
            v.map(Json::String).unwrap_or(Json::Null)
        }
        FieldType::Date => match row.try_get::<Option<String>, _>(0) {
            Ok(v) => v.map(Json::String).unwrap_or(Json::Null),
            Err(_) => {
                let v: Option<time::Date> = row.try_get(0).map_err(my)?;
                match v {
                    Some(d) => Json::String(format!(
                        "{:04}-{:02}-{:02}",
                        d.year(),
                        u8::from(d.month()),
                        d.day()
                    )),
                    None => Json::Null,
                }
            }
        },
        FieldType::Timestamp => {
            let v: Option<OffsetDateTime> = row.try_get(0).map_err(my)?;
            match v {
                Some(ts) => Json::String(
                    ts.format(&time::format_description::well_known::Rfc3339)
                        .map_err(my)?,
                ),
                None => Json::Null,
            }
        }
        FieldType::Json => {
            let v: Option<Json> = row.try_get(0).map_err(my)?;
            v.unwrap_or(Json::Null)
        }
    })
}

/// Semantic filter -> typed WHERE condition. `model` types the bind when the
/// field is declared on it; `None` (conditions inside an existence subquery,
/// whose model the store never sees) falls back to untyped JSON bind
/// detection (uuid/timestamp/number strings).
fn filter_condition(model: Option<&ModelDef>, filter: &FieldFilter) -> Result<Condition, AppError> {
    let FieldFilter::Cmp { field, op, value } = filter else {
        return Err(AppError::validation(
            "existence filters compose through base_select, never as a bare condition",
        ));
    };
    let column = ident(field)?;
    let field_def = model.and_then(|m| m.field_named(field));
    let bind_of = |value: &Json| -> Result<Bind, AppError> {
        match field_def {
            Some(f) => kelson_models::json_to_bind(value, f),
            None => json_value_to_bind(value),
        }
    };
    Ok(match op {
        FilterOp::Eq => Condition {
            column,
            operator: Operator::Eq,
            values: vec![bind_of(value)?],
        },
        FilterOp::Ne => Condition {
            column,
            operator: Operator::Ne,
            values: vec![bind_of(value)?],
        },
        FilterOp::In => {
            let list = value
                .as_array()
                .ok_or_else(|| AppError::validation("IN filter needs an array value"))?;
            if list.is_empty() {
                return Err(AppError::validation("IN filter needs at least one value"));
            }
            let mut values = Vec::with_capacity(list.len());
            for item in list {
                values.push(bind_of(item)?);
            }
            Condition {
                column,
                operator: Operator::In,
                values,
            }
        }
        FilterOp::IsNull => Condition {
            column,
            operator: Operator::IsNull,
            values: Vec::new(),
        },
        FilterOp::NotNull => Condition {
            column,
            operator: Operator::NotNull,
            values: Vec::new(),
        },
        // Range terms (Wave 3 Phase 6): bind through the declared field type
        // like Eq; the SQL operator does the ordering (timestamps compare
        // chronologically, text lexicographically).
        FilterOp::Lt => Condition {
            column,
            operator: Operator::Lt,
            values: vec![bind_of(value)?],
        },
        FilterOp::LtEq => Condition {
            column,
            operator: Operator::LtEq,
            values: vec![bind_of(value)?],
        },
        FilterOp::Gt => Condition {
            column,
            operator: Operator::Gt,
            values: vec![bind_of(value)?],
        },
        FilterOp::GtEq => Condition {
            column,
            operator: Operator::GtEq,
            values: vec![bind_of(value)?],
        },
    })
}

/// Base SELECT for a model list: tenant scope + soft-delete veil (unless
/// `include_deleted`) + filters.
fn base_select(
    model: &ModelDef,
    tenant: Option<TenantId>,
    filters: &[FieldFilter],
    include_deleted: bool,
) -> Result<Select, AppError> {
    let table = ident(&model.table)?;
    let mut select = Select::from_table(table);
    if model.tenant_scoped {
        if let Some(t) = tenant {
            select = select.tenant_scope(ident("tenant_id")?, t);
        } else {
            // Tenant-scoped model without a tenant: match nothing.
            select = select.where_eq(ident("tenant_id")?, Bind::Null);
        }
    }
    if model.soft_deletes && !include_deleted {
        select = select.where_null(ident("deleted_at")?);
    }
    push_filters(select, model, tenant, filters)
}

/// Apply the **cursor** (keyset) anchor of a list to a SELECT: the page
/// starts *after* the anchor, so a concurrent insert cannot shift the window
/// the way OFFSET does. A descending sort flips the comparison.
///
/// The anchor is applied to the page query AND to the COUNT query, so `total`
/// is the size of the window that is left.
fn apply_keyset(
    select: Select,
    model: &ModelDef,
    opts: &ListOpts,
) -> Result<Select, AppError> {
    let Some(after) = &opts.after else {
        return Ok(select);
    };
    let (columns, values) = kelson_models::keyset_predicate(model, after)?;
    let operator = match opts.sort.as_ref().map(|(_, d)| *d) {
        Some(true) => Operator::Lt,
        _ => Operator::Gt,
    };
    Ok(select.where_row_cmp(columns, operator, values))
}

/// Append the filter list as WHERE clauses. Every filter is prepared into a
/// clause pusher first (Ident/bind errors surface outside the group
/// closure); a list containing an OR-tagged existence term wraps in ONE
/// parenthesized group so OR can never escape the tenant/soft-delete scope
/// ahead of it -- `tenant AND (a OR EXISTS (...))`.
fn push_filters(
    select: Select,
    model: &ModelDef,
    tenant: Option<TenantId>,
    filters: &[FieldFilter],
) -> Result<Select, AppError> {
    let pushers: Vec<Box<dyn FnOnce(Select) -> Select>> = filters
        .iter()
        .map(|filter| -> Result<Box<dyn FnOnce(Select) -> Select>, AppError> {
            match filter {
                FieldFilter::Cmp { .. } => {
                    let cond = filter_condition(Some(model), filter)?;
                    Ok(Box::new(move |select| match cond.operator {
                        Operator::In => select.where_in(cond.column, cond.values),
                        Operator::IsNull => select.where_null(cond.column),
                        Operator::NotNull => select.where_not_null(cond.column),
                        other => select.where_op(
                            cond.column,
                            other,
                            cond.values.into_iter().next().unwrap_or(Bind::Null),
                        ),
                    }))
                }
                FieldFilter::Exists(ex) => {
                    let sub = exists_select(tenant, ex)?;
                    let (negated, or) = (ex.negated, ex.or);
                    Ok(Box::new(move |select| match (negated, or) {
                        (false, false) => select.where_exists(sub),
                        (true, false) => select.where_not_exists(sub),
                        (false, true) => select.or_where_exists(sub),
                        (true, true) => select.or_where_not_exists(sub),
                    }))
                }
            }
        })
        .collect::<Result<_, _>>()?;
    let has_or = filters
        .iter()
        .any(|f| matches!(f, FieldFilter::Exists(ex) if ex.or));
    if has_or {
        Ok(select
            .group(move |inner| pushers.into_iter().fold(inner, |acc, push| push(acc))))
    } else {
        Ok(pushers
            .into_iter()
            .fold(select, |acc, push| push(acc)))
    }
}

/// The correlated subquery for an existence term: sub table + tenant scope +
/// soft veil + key correlation + field conditions.
fn exists_select(tenant: Option<TenantId>, ex: &ExistsFilter) -> Result<Select, AppError> {
    if ex
        .filters
        .iter()
        .any(|f| matches!(f, FieldFilter::Exists(_)))
    {
        return Err(AppError::validation(
            "relation filters cannot nest inside a relation filter (not supported)",
        ));
    }
    let mut sub = Select::from_table(ident(&ex.sub_table)?);
    if ex.sub_tenant_scoped {
        if let Some(t) = tenant {
            sub = sub.tenant_scope(ident("tenant_id")?, t);
        } else {
            sub = sub.where_eq(ident("tenant_id")?, Bind::Null);
        }
    }
    if ex.sub_soft_deletes {
        sub = sub.where_null(ident("deleted_at")?);
    }
    sub = sub.where_column(
        ident(&format!("{}.{}", ex.sub_table, ex.sub_key))?,
        ident(&format!("{}.{}", ex.outer_table, ex.outer_key))?,
    );
    for filter in &ex.filters {
        let cond = filter_condition(None, filter)?;
        sub = match cond.operator {
            Operator::In => sub.where_in(cond.column, cond.values),
            Operator::IsNull => sub.where_null(cond.column),
            Operator::NotNull => sub.where_not_null(cond.column),
            other => sub.where_op(
                cond.column,
                other,
                cond.values.into_iter().next().unwrap_or(Bind::Null),
            ),
        };
    }
    Ok(sub)
}


fn fetch_rows(pool: &MySqlPool, compiled: &CompiledSelect) -> Result<Vec<MySqlRow>, AppError> {
    let sql = compiled.sql.clone();
    let params = compiled.params.clone();
    let pool = pool.clone();
    block_on(async move {
        let query = bind_params(sqlx::query(&sql), &params);
        query.fetch_all(&pool).await.map_err(my)
    })
}

/// Execute a write compiled by the typed builders. MySQL has no RETURNING
/// (rule 10), so the statement never carries one and this returns
/// `rows_affected` -- only `delete` (soft and hard), `force_delete` and
/// `restore` consult it (rule 16); every other write re-selects its row(s)
/// with a follow-up SELECT under the same scope predicates.
fn exec_write(pool: &MySqlPool, compiled: &CompiledWrite) -> Result<u64, AppError> {
    let sql = compiled.sql.clone();
    let params = compiled.params.clone();
    let pool = pool.clone();
    block_on(async move {
        let query = bind_params(sqlx::query(&sql), &params);
        let done = query.execute(&pool).await.map_err(my)?;
        Ok(done.rows_affected())
    })
}


fn map_rows(rows: Vec<MySqlRow>, model: &ModelDef) -> Result<Vec<Record>, AppError> {
    rows.iter().map(|r| row_to_record(r, model)).collect()
}

impl ModelStore for MySqlModelStore {
    fn list(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        opts: &ListOpts,
    ) -> Result<kelson_query::Page<Record>, AppError> {
        validate_list_opts(model, opts)?;
        let mut select = base_select(model, tenant, filters, opts.include_deleted)?;
        if let Some((field, desc)) = &opts.sort {
            select = select.order_by(
                ident(field)?,
                if *desc { Direction::Desc } else { Direction::Asc },
            );
        }
        // Cursor (keyset) mode: the page starts AFTER the anchor, so a
        // concurrent insert cannot shift the window the way OFFSET does.
        let mut select = apply_keyset(select, model, opts)?;
        let (page, per_page) = match opts.page {
            Some(req) => {
                let req = req.validate()?;
                select = select.limit(u64::from(req.per_page)).offset(u64::from(req.offset()));
                (req.page, req.per_page)
            }
            None => {
                if let Some(n) = opts.limit {
                    select = select.limit(u64::from(n));
                }
                (1, 0)
            }
        };
        let compiled = select.build(&kelson_query::MYSQL)?;
        // The read may be served by the replica (only when the caller opted in).
        let read_pool = self.pool_for_read(opts);
        let rows = fetch_rows(read_pool, &compiled)?;
        let total = if opts.page.is_some() || opts.limit.is_some() {
            // COUNT over the same shape (rebuild without limit/offset). The
            // port pins `total` to the pre-limit match count: `ListOpts::limit`
            // is a row cap, so a capped eager fetch must still report how many
            // rows matched (kelson-models `store` list contract test).
            let count_select = apply_keyset(
                base_select(model, tenant, filters, opts.include_deleted)?,
                model,
                opts,
            )?
            .build(&kelson_query::MYSQL)?;
            let count_sql = format!(
                "SELECT COUNT(*) FROM ({}) AS _kelson_count",
                count_select.sql
            );
            let params = count_select.params.clone();
            // The count is taken from the SAME pool as the page, so the two
            // agree even when the page came from the replica.
            let pool = read_pool.clone();
            let count: i64 = block_on(async move {
                let query = bind_params(sqlx::query(&count_sql), &params);
                let row: MySqlRow = query.fetch_one(&pool).await.map_err(my)?;
                row.try_get::<i64, usize>(0).map_err(my)
            })?;
            count.max(0) as u64
        } else {
            rows.len() as u64
        };
        let items = map_rows(rows, model)?;
        Ok(kelson_query::Page::new(items, page, per_page, Some(total)))
    }

    fn find(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError> {
        let mut select = Select::from_table(ident(&model.table)?).where_eq(
            ident("id")?,
            Bind::Uuid(id),
        );
        if model.tenant_scoped {
            match tenant {
                Some(t) => select = select.tenant_scope(ident("tenant_id")?, t),
                None => select = select.where_eq(ident("tenant_id")?, Bind::Null),
            }
        }
        if model.soft_deletes && !include_deleted {
            select = select.where_null(ident("deleted_at")?);
        }
        let rows = fetch_rows(&self.pool, &select.build(&kelson_query::MYSQL)?)?;
        Ok(map_rows(rows, model)?.into_iter().next())
    }

    fn insert(&self, model: &ModelDef, record: &Record) -> Result<Record, AppError> {
        // Rule 10: the readback re-selects by the record's uuid `id`, so it
        // is validated BEFORE the INSERT executes (like `InMemoryModelStore`,
        // a record without one cannot be written here).
        let id = record_uuid(record, "id")?;
        let (columns, params) = record_to_columns_for(record, model)?;
        let mut insert = Insert::into_table(ident(&model.table)?).columns(columns);
        insert = insert.row(params);
        if model.tenant_scoped {
            insert = insert.tenant_scope(
                ident("tenant_id")?,
                kelson_foundation::TenantId::from_uuid(record_uuid(record, "tenant_id")?),
            );
        }
        let compiled = insert.build(&kelson_query::MYSQL)?;
        // Readback under the write's scope: the record's own tenant is
        // pinned in as defense in depth (the write guard already bound that
        // value into the row); the insert carried no soft veil.
        let mut select = Select::from_table(ident(&model.table)?)
            .select(all_columns(model)?)
            .where_eq(ident("id")?, Bind::Uuid(id));
        if model.tenant_scoped {
            select = select.tenant_scope(
                ident("tenant_id")?,
                kelson_foundation::TenantId::from_uuid(record_uuid(record, "tenant_id")?),
            );
        }
        let readback = select.build(&kelson_query::MYSQL)?;
        exec_write(&self.pool, &compiled)?;
        let rows = fetch_rows(&self.pool, &readback)?;
        map_rows(rows, model)?
            .into_iter()
            .next()
            .ok_or_else(|| my("insert returned no row"))
    }

    fn update(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        changes: &Record,
    ) -> Result<Option<Record>, AppError> {
        if changes.is_empty() {
            return self.find(model, id, tenant, false);
        }
        let mut update = Update::table(ident(&model.table)?).where_eq(ident("id")?, Bind::Uuid(id));
        if model.tenant_scoped {
            match tenant {
                Some(t) => update = update.tenant_scope(ident("tenant_id")?, t),
                None => update = update.where_eq(ident("tenant_id")?, Bind::Null),
            }
        }
        for (key, value) in changes {
            let bind = match model.field_named(key) {
                Some(f) => kelson_models::json_to_bind(value, f)?,
                None => json_value_to_bind(value)?,
            };
            update = update.set(ident(key)?, bind);
        }
        let compiled = update.build(&kelson_query::MYSQL)?;
        // Rule 10: readback repeats the write's scope (the update carried no
        // soft veil), so changes to `tenant_id` re-select nothing -- exactly
        // like a foreign-tenant write's empty pg RETURNING.
        let mut select = Select::from_table(ident(&model.table)?)
            .select(all_columns(model)?)
            .where_eq(ident("id")?, Bind::Uuid(id));
        if model.tenant_scoped {
            match tenant {
                Some(t) => select = select.tenant_scope(ident("tenant_id")?, t),
                None => select = select.where_eq(ident("tenant_id")?, Bind::Null),
            }
        }
        let readback = select.build(&kelson_query::MYSQL)?;
        exec_write(&self.pool, &compiled)?;
        let rows = fetch_rows(&self.pool, &readback)?;
        Ok(map_rows(rows, model)?.into_iter().next())
    }

    fn delete(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<bool, AppError> {
        if model.soft_deletes {
            let mut update = Update::table(ident(&model.table)?)
                .where_eq(ident("id")?, Bind::Uuid(id))
                .where_is_null(ident("deleted_at")?)
                .set(ident("deleted_at")?, Bind::Timestamp(OffsetDateTime::now_utc()));
            if model.tenant_scoped {
                match tenant {
                    Some(t) => update = update.tenant_scope(ident("tenant_id")?, t),
                    None => update = update.where_eq(ident("tenant_id")?, Bind::Null),
                }
            }
            let compiled = update.build(&kelson_query::MYSQL)?;
            // Rule 16: a matched soft delete always stamps `deleted_at`, so
            // rows_affected equals pg's RETURNING-presence.
            let affected = exec_write(&self.pool, &compiled)?;
            return Ok(affected > 0);
        }
        let mut delete = Delete::from_table(ident(&model.table)?).where_eq(ident("id")?, Bind::Uuid(id));
        if model.tenant_scoped {
            match tenant {
                Some(t) => delete = delete.tenant_scope(ident("tenant_id")?, t),
                None => delete = delete.where_eq(ident("tenant_id")?, Bind::Null),
            }
        }
        let compiled = delete.build(&kelson_query::MYSQL)?;
        // Rule 16: DELETE counts the removed rows.
        let affected = exec_write(&self.pool, &compiled)?;
        Ok(affected > 0)
    }

    fn upsert(
        &self,
        model: &ModelDef,
        record: &Record,
        conflict: &[String],
    ) -> Result<Record, AppError> {
        if conflict.is_empty() {
            return Err(AppError::validation("upsert needs conflict fields"));
        }
        // Rule 10: the readback re-selects by the record's uuid `id`.
        let id = record_uuid(record, "id")?;
        let (columns, params) = record_to_columns_for(record, model)?;
        let conflict_columns: Vec<Ident> = conflict
            .iter()
            .map(|c| ident(c))
            .collect::<Result<_, _>>()?;
        let mut insert = Insert::into_table(ident(&model.table)?)
            .columns(columns)
            .row(params)
            .on_conflict(conflict_columns, ConflictPolicy::DoUpdate)
            // Identity is never overwritten by a merge: the surviving row
            // keeps its primary key (Laravel updateOrCreate semantics).
            .exclude_from_conflict_update(vec![ident("id")?]);
        if model.tenant_scoped {
            insert = insert.tenant_scope(
                ident("tenant_id")?,
                kelson_foundation::TenantId::from_uuid(record_uuid(record, "tenant_id")?),
            );
        }
        // The MySQL dialect renders the conflict clause natively (the
        // 8.0.19+ row-alias form `AS new ON DUPLICATE KEY UPDATE c =
        // new.c`), so no SQL is rewritten here any more. Flagged: MySQL
        // fires the merge for ANY unique key of the table.
        let compiled = insert.build(&kelson_query::MYSQL)?;
        // Prepare the readback BEFORE the write, so a malformed record fails
        // closed without inserting.
        let id_column = ident("id")?;
        let keys: Vec<(Ident, Bind)> = conflict
            .iter()
            .map(|c| -> Result<(Ident, Bind), AppError> {
                let value = record.get(c).ok_or_else(|| {
                    AppError::validation(format!("upsert record needs a value in {c:?}"))
                })?;
                let bind = match model.field_named(c) {
                    Some(f) => kelson_models::json_to_bind(value, f)?,
                    None => json_value_to_bind(value)?,
                };
                Ok((ident(c)?, bind))
            })
            .collect::<Result<_, _>>()?;
        let mut select = Select::from_table(ident(&model.table)?).select(all_columns(model)?);
        if model.tenant_scoped {
            select = select.tenant_scope(
                ident("tenant_id")?,
                kelson_foundation::TenantId::from_uuid(record_uuid(record, "tenant_id")?),
            );
        }
        // `id OR (every conflict column)`: the first term joins with OR, the
        // conflict columns AND together under SQL precedence --
        // `tenant_id = ? AND (id = ? OR (c1 = ? AND c2 = ?))`.
        select = select.group(move |inner| {
            let mut grouped = inner.where_eq(id_column, Bind::Uuid(id));
            for (index, (column, value)) in keys.into_iter().enumerate() {
                grouped = if index == 0 {
                    grouped.or_where(column, Operator::Eq, value)
                } else {
                    grouped.where_eq(column, value)
                };
            }
            grouped
        });
        let readback = select.limit(1).build(&kelson_query::MYSQL)?;
        let params = compiled.params.clone();
        let sql = compiled.sql.clone();
        let pool = self.pool.clone();
        block_on(async move {
            let query = bind_params(sqlx::query(&sql), &params);
            query.execute(&pool).await.map_err(my)?;
            Ok(())
        })?;
        let rows = fetch_rows(&self.pool, &readback)?;
        map_rows(rows, model)?
            .into_iter()
            .next()
            .ok_or_else(|| my("upsert returned no row"))
    }

    fn aggregate(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        op: kelson_models::AggregateOp,
    ) -> Result<Json, AppError> {
        let field_type = validate_aggregate(model, &op)?;
        let mut select = base_select(model, tenant, filters, false)?;
        select = match &op {
            kelson_models::AggregateOp::Count => select.count(),
            kelson_models::AggregateOp::Sum(f) => select.sum(ident(f)?),
            kelson_models::AggregateOp::Avg(f) => select.avg(ident(f)?),
            kelson_models::AggregateOp::Min(f) => select.min(ident(f)?),
            kelson_models::AggregateOp::Max(f) => select.max(ident(f)?),
        };
        let compiled = select.build(&kelson_query::MYSQL)?;
        let rows = fetch_rows(&self.pool, &compiled)?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| my("aggregate returned no row"))?;
        decode_aggregate(&row, field_type)
    }

    fn exists(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
    ) -> Result<bool, AppError> {
        let select = base_select(model, tenant, filters, false)?.exists();
        let compiled = select.build(&kelson_query::MYSQL)?;
        let rows = fetch_rows(&self.pool, &compiled)?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| my("EXISTS returned no row"))?;
        row.try_get::<bool, _>(0).map_err(my)
    }

    fn find_by(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError> {
        let select = base_select(model, tenant, filters, include_deleted)?.limit(1);
        let compiled = select.build(&kelson_query::MYSQL)?;
        let rows = fetch_rows(&self.pool, &compiled)?;
        Ok(map_rows(rows, model)?.into_iter().next())
    }

    fn find_for_update(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError> {
        let mut select = Select::from_table(ident(&model.table)?)
            .where_eq(ident("id")?, Bind::Uuid(id))
            .lock_for_update();
        if model.tenant_scoped {
            match tenant {
                Some(t) => select = select.tenant_scope(ident("tenant_id")?, t),
                None => select = select.where_eq(ident("tenant_id")?, Bind::Null),
            }
        }
        if model.soft_deletes && !include_deleted {
            select = select.where_null(ident("deleted_at")?);
        }
        let compiled = select.build(&kelson_query::MYSQL)?;
        let rows = fetch_rows(&self.pool, &compiled)?;
        Ok(map_rows(rows, model)?.into_iter().next())
    }

    fn insert_or_ignore(&self, model: &ModelDef, record: &Record) -> Result<Option<Record>, AppError> {
        // Rule 10: the readback re-selects by the record's uuid `id`, so it
        // is validated BEFORE the INSERT executes.
        let id = record_uuid(record, "id")?;
        let (columns, params) = record_to_columns_for(record, model)?;
        let mut insert = Insert::into_table(ident(&model.table)?)
            .columns(columns)
            .row(params);
        if model.tenant_scoped {
            insert = insert.tenant_scope(
                ident("tenant_id")?,
                kelson_foundation::TenantId::from_uuid(record_uuid(record, "tenant_id")?),
            );
        }
        let compiled = insert.build(&kelson_query::MYSQL)?;
        // Rule 17: pg's target-less `ON CONFLICT DO NOTHING` has no MySQL
        // form -- a plain INSERT runs, and ONLY a duplicate-key error maps
        // to `Ok(None)` below; every other error still surfaces.
        let mut select = Select::from_table(ident(&model.table)?)
            .select(all_columns(model)?)
            .where_eq(ident("id")?, Bind::Uuid(id));
        if model.tenant_scoped {
            select = select.tenant_scope(
                ident("tenant_id")?,
                kelson_foundation::TenantId::from_uuid(record_uuid(record, "tenant_id")?),
            );
        }
        let readback = select.build(&kelson_query::MYSQL)?;
        let sql = compiled.sql.clone();
        let params = compiled.params.clone();
        let pool = self.pool.clone();
        match block_on(async move {
            let query = bind_params(sqlx::query(&sql), &params);
            query.execute(&pool).await
        }) {
            Ok(_) => {}
            Err(sqlx::Error::Database(db)) if db.is_unique_violation() => return Ok(None),
            Err(e) => return Err(my(e)),
        }
        let rows = fetch_rows(&self.pool, &readback)?;
        Ok(map_rows(rows, model)?.into_iter().next())
    }

    fn update_filtered(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        changes: &Record,
    ) -> Result<Vec<Record>, AppError> {
        require_write_filters("update_filtered", filters)?;
        if changes.is_empty() {
            // Nothing to write: return the matching rows (mirrors `update`).
            return Ok(self.list(model, tenant, filters, &ListOpts::default())?.items);
        }
        let mut update = Update::table(ident(&model.table)?);
        if model.tenant_scoped {
            match tenant {
                Some(t) => update = update.tenant_scope(ident("tenant_id")?, t),
                None => update = update.where_eq(ident("tenant_id")?, Bind::Null),
            }
        }
        if model.soft_deletes {
            update = update.where_is_null(ident("deleted_at")?);
        }
        for filter in filters {
            let cond = filter_condition(Some(model), filter)?;
            update = match cond.operator {
                Operator::In => update.where_in(cond.column, cond.values),
                Operator::IsNull => update.where_is_null(cond.column),
                Operator::NotNull => update.where_not_null(cond.column),
                other => update.where_op(
                    cond.column,
                    other,
                    cond.values.into_iter().next().unwrap_or(Bind::Null),
                ),
            };
        }
        for (field, value) in changes {
            let bind = match model.field_named(field) {
                Some(f) => kelson_models::json_to_bind(value, f)?,
                None => json_value_to_bind(value)?,
            };
            update = update.set(ident(field)?, bind);
        }
        let compiled = update.build(&kelson_query::MYSQL)?;
        // Rule 10: capture the matched keys BEFORE the write -- changes may
        // stop rows from matching their own filters, which pg's RETURNING
        // would still return.
        let key_select = base_select(model, tenant, filters, false)?
            .column(ident("id")?)
            .build(&kelson_query::MYSQL)?;
        let key_rows = fetch_rows(&self.pool, &key_select)?;
        let mut keys: Vec<Uuid> = Vec::with_capacity(key_rows.len());
        for row in &key_rows {
            keys.push(row.try_get::<Uuid, _>("id").map_err(my)?);
        }
        // The cost guard: the write is refused BEFORE it runs, when the rows
        // it would touch exceed the tenant's ceiling. The count comes from
        // the SAME scope the write uses (the captured keys), so the guard
        // and the write agree.
        if let Some(guard) = &self.cost_guard {
            if let Err(e) = guard.check_write(tenant, keys.len() as u64) {
                if let Some(ledger) = &self.cost_ledger {
                    if let Ok(mut ledger) = ledger.lock() {
                        ledger.record_refusal(tenant);
                    }
                }
                return Err(e);
            }
        }
        let started = std::time::Instant::now();
        exec_write(&self.pool, &compiled)?;
        if let Some(ledger) = &self.cost_ledger {
            if let Ok(mut ledger) = ledger.lock() {
                ledger.record(&crate::cost::StatementCost {
                    tenant,
                    kind: crate::cost::StatementKind::Write,
                    elapsed: started.elapsed(),
                    rows: keys.len() as u64,
                });
            }
        }
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        // Re-read the captured keys under the write's tenant posture. The
        // filters are NOT re-applied (the keys already matched them, and the
        // changes may have moved rows past them) -- including the soft veil,
        // which the soft `delete_filtered` path just set `deleted_at` past.
        let mut select = Select::from_table(ident(&model.table)?)
            .select(all_columns(model)?)
            .where_in(ident("id")?, keys.into_iter().map(Bind::Uuid).collect());
        if model.tenant_scoped {
            match tenant {
                Some(t) => select = select.tenant_scope(ident("tenant_id")?, t),
                None => select = select.where_eq(ident("tenant_id")?, Bind::Null),
            }
        }
        let readback = select.build(&kelson_query::MYSQL)?;
        let rows = fetch_rows(&self.pool, &readback)?;
        map_rows(rows, model)
    }

    fn delete_filtered(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
    ) -> Result<Vec<Record>, AppError> {
        require_write_filters("delete_filtered", filters)?;
        if model.soft_deletes {
            let mut changes = Record::new();
            changes.insert("deleted_at".into(), Json::String(kelson_models::now_rfc3339()));
            return self.update_filtered(model, tenant, filters, &changes);
        }
        let mut delete = Delete::from_table(ident(&model.table)?);
        if model.tenant_scoped {
            match tenant {
                Some(t) => delete = delete.tenant_scope(ident("tenant_id")?, t),
                None => delete = delete.where_eq(ident("tenant_id")?, Bind::Null),
            }
        }
        for filter in filters {
            let cond = filter_condition(Some(model), filter)?;
            delete = match cond.operator {
                Operator::In => delete.where_in(cond.column, cond.values),
                Operator::IsNull => delete.where_is_null(cond.column),
                Operator::NotNull => delete.where_not_null(cond.column),
                other => delete.where_op(
                    cond.column,
                    other,
                    cond.values.into_iter().next().unwrap_or(Bind::Null),
                ),
            };
        }
        let compiled = delete.build(&kelson_query::MYSQL)?;
        // Rule 10: read the pre-image FIRST (pg's `DELETE ... RETURNING`
        // yields the rows as they were), then delete.
        let pre_select = base_select(model, tenant, filters, false)?
            .select(all_columns(model)?)
            .build(&kelson_query::MYSQL)?;
        let rows = fetch_rows(&self.pool, &pre_select)?;
        exec_write(&self.pool, &compiled)?;
        map_rows(rows, model)
    }

    fn force_delete(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<bool, AppError> {
        // No deleted_at veil: purges live AND trashed rows alike.
        let mut delete = Delete::from_table(ident(&model.table)?).where_eq(ident("id")?, Bind::Uuid(id));
        if model.tenant_scoped {
            match tenant {
                Some(t) => delete = delete.tenant_scope(ident("tenant_id")?, t),
                None => delete = delete.where_eq(ident("tenant_id")?, Bind::Null),
            }
        }
        let compiled = delete.build(&kelson_query::MYSQL)?;
        // Rule 16: DELETE counts the removed rows.
        let affected = exec_write(&self.pool, &compiled)?;
        Ok(affected > 0)
    }

    fn truncate(&self, model: &ModelDef, env: &str) -> Result<(), AppError> {
        let compiled =
            kelson_query::Truncate::table(ident(&model.table)?).build_gated(env, &kelson_query::MYSQL)?;
        let sql = compiled.sql.clone();
        let params = compiled.params;
        let pool = self.pool.clone();
        block_on(async move {
            let query = bind_params(sqlx::query(&sql), &params);
            query.execute(&pool).await.map_err(my)?;
            Ok(())
        })
    }

    fn increment(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        field: &str,
        by: i64,
        extra: &Record,
    ) -> Result<Option<Record>, AppError> {
        validate_counter(model, field)?;
        let mut update = Update::table(ident(&model.table)?)
            .where_eq(ident("id")?, Bind::Uuid(id))
            // Atomic `SET field = field + by` -- one statement with `extra`.
            .increment(ident(field)?, by);
        if model.tenant_scoped {
            match tenant {
                Some(t) => update = update.tenant_scope(ident("tenant_id")?, t),
                None => update = update.where_eq(ident("tenant_id")?, Bind::Null),
            }
        }
        if model.soft_deletes {
            update = update.where_is_null(ident("deleted_at")?);
        }
        for (key, value) in extra {
            let bind = match model.field_named(key) {
                Some(f) => kelson_models::json_to_bind(value, f)?,
                None => json_value_to_bind(value)?,
            };
            update = update.set(ident(key)?, bind);
        }
        let compiled = update.build(&kelson_query::MYSQL)?;
        // Rule 10: readback repeats the write's scope -- tenant posture AND
        // the soft-delete veil the write carried (rule 11).
        let mut select = Select::from_table(ident(&model.table)?)
            .select(all_columns(model)?)
            .where_eq(ident("id")?, Bind::Uuid(id));
        if model.tenant_scoped {
            match tenant {
                Some(t) => select = select.tenant_scope(ident("tenant_id")?, t),
                None => select = select.where_eq(ident("tenant_id")?, Bind::Null),
            }
        }
        if model.soft_deletes {
            select = select.where_null(ident("deleted_at")?);
        }
        let readback = select.build(&kelson_query::MYSQL)?;
        exec_write(&self.pool, &compiled)?;
        let rows = fetch_rows(&self.pool, &readback)?;
        Ok(map_rows(rows, model)?.into_iter().next())
    }

    fn restore(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<Option<Record>, AppError> {
        // Hard-delete models have no trash to return to (and no deleted_at
        // column to clear).
        if !model.soft_deletes {
            return Ok(None);
        }
        let mut update = Update::table(ident(&model.table)?)
            .where_eq(ident("id")?, Bind::Uuid(id))
            // Only TRASHED rows qualify: a live row matches nothing, so the
            // write is naturally idempotent (mirrors in-memory `is_deleted`).
            .where_not_null(ident("deleted_at")?)
            .set(ident("deleted_at")?, Bind::Null);
        if model.tenant_scoped {
            match tenant {
                Some(t) => update = update.tenant_scope(ident("tenant_id")?, t),
                None => update = update.where_eq(ident("tenant_id")?, Bind::Null),
            }
        }
        let compiled = update.build(&kelson_query::MYSQL)?;
        // Rule 16: a matched restore always clears the stamp.
        let affected = exec_write(&self.pool, &compiled)?;
        if affected == 0 {
            return Ok(None);
        }
        // Rule 10: re-select the now-live row under the write's tenant scope
        // (the write carried no soft veil; the row is `deleted_at IS NULL`
        // again after the clear).
        let mut select = Select::from_table(ident(&model.table)?)
            .select(all_columns(model)?)
            .where_eq(ident("id")?, Bind::Uuid(id));
        if model.tenant_scoped {
            match tenant {
                Some(t) => select = select.tenant_scope(ident("tenant_id")?, t),
                None => select = select.where_eq(ident("tenant_id")?, Bind::Null),
            }
        }
        let readback = select.build(&kelson_query::MYSQL)?;
        let rows = fetch_rows(&self.pool, &readback)?;
        Ok(map_rows(rows, model)?.into_iter().next())
    }
}

fn record_uuid(record: &Record, key: &str) -> Result<Uuid, AppError> {
    record
        .get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| AppError::validation(format!("record needs a uuid in {key:?}")))
}

/// Readback column list (rule 10): the model's declared fields (framework
/// timestamp columns are read back only when the model declares them) --
/// byte-equivalent to the pg `RETURNING` row.
fn all_columns(model: &ModelDef) -> Result<Vec<Ident>, AppError> {
    let mut names: Vec<String> = model.fields.iter().map(|f| f.name.clone()).collect();
    names.sort();
    names.dedup();
    names.iter().map(|n| ident(n)).collect()
}