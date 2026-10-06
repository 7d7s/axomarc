//! Durable model store on PostgreSQL (S8 C3/B-04/B-05): implements the
//! keel-models `ModelStore` port by compiling records/filters through the
//! typed query builders and executing them here. Tenant isolation is applied
//! on EVERY path (`tenant_scope`); soft deletes honor `ModelDef::soft_deletes`.

use keel_foundation::{AppError, TenantId};
use keel_models::{
    json_value_to_bind, record_to_columns, ExistsFilter, FieldFilter, FieldType, FilterOp,
    ListOpts, ModelDef, ModelStore, Record,
};
use keel_models::store::{
    require_write_filters, validate_aggregate, validate_counter, validate_list_opts,
};
use keel_query::select::{Condition, Direction, Ident, Operator};
use keel_query::{CompiledSelect, CompiledWrite, Delete, Insert, ConflictPolicy, Select, Update, Value as Bind};
use serde_json::Value as Json;
use sqlx::postgres::{PgArguments, PgPool, PgRow};
use sqlx::{Column, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::bridge::block_on;

pub struct PgModelStore {
    pool: PgPool,
}

impl PgModelStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn pg(e: impl std::fmt::Display) -> AppError {
    AppError::persistence(format!("model store: {e}"))
}

fn ident(name: &str) -> Result<Ident, AppError> {
    Ident::new(name.to_string())
}

fn bind_params<'q>(
    mut query: sqlx::query::Query<'q, sqlx::Postgres, PgArguments>,
    params: &'q [Bind],
) -> sqlx::query::Query<'q, sqlx::Postgres, PgArguments> {
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
fn row_to_record(row: &PgRow, model: &ModelDef) -> Result<Record, AppError> {
    let mut record = Record::new();
    for column in row.columns() {
        let name = column.name();
        let value = decode_column(row, name, model)?;
        record.insert(name.to_string(), value);
    }
    Ok(record)
}

fn decode_column(row: &PgRow, name: &str, model: &ModelDef) -> Result<Json, AppError> {
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
            let v: Option<Uuid> = row.try_get(name).map_err(pg)?;
            v.map(|u| Json::String(u.to_string())).unwrap_or(Json::Null)
        }
        FieldType::Bool => {
            let v: Option<bool> = row.try_get(name).map_err(pg)?;
            v.map(Json::Bool).unwrap_or(Json::Null)
        }
        FieldType::Int32 => {
            let v: Option<i32> = row.try_get(name).map_err(pg)?;
            v.map(|i| Json::from(i64::from(i))).unwrap_or(Json::Null)
        }
        FieldType::Int64 => {
            let v: Option<i64> = row.try_get(name).map_err(pg)?;
            v.map(Json::from).unwrap_or(Json::Null)
        }
        FieldType::Float | FieldType::Decimal => {
            let v: Option<f64> = row.try_get(name).map_err(pg)?;
            v.map(Json::from).unwrap_or(Json::Null)
        }
        FieldType::Text | FieldType::TextLarge | FieldType::Date | FieldType::Enum { .. } => {
            let v: Option<String> = row.try_get(name).map_err(pg)?;
            v.map(Json::String).unwrap_or(Json::Null)
        }
        FieldType::Timestamp => {
            let v: Option<OffsetDateTime> = row.try_get(name).map_err(pg)?;
            match v {
                Some(ts) => Json::String(
                    ts.format(&time::format_description::well_known::Rfc3339)
                        .map_err(pg)?,
                ),
                None => Json::Null,
            }
        }
        FieldType::Json => {
            let v: Option<Json> = row.try_get(name).map_err(pg)?;
            v.unwrap_or(Json::Null)
        }
    })
}

/// Single aggregate result value (column 0) in the canonical JSON encoding.
/// COUNT/SUM/AVG come back as int8, float8, or numeric depending on the
/// input column type, so numeric decoding sniffs int first, then float.
fn decode_aggregate(row: &PgRow, field_type: Option<&FieldType>) -> Result<Json, AppError> {
    // NULL aggregate (empty match set): Option<T> short-circuits on NULL.
    if let Ok(None) = row.try_get::<Option<i64>, _>(0) {
        return Ok(Json::Null);
    }
    match field_type {
        Some(FieldType::Int32 | FieldType::Int64) => {
            if let Ok(Some(v)) = row.try_get::<Option<i64>, _>(0) {
                return Ok(Json::from(v));
            }
            match row.try_get::<Option<f64>, _>(0) {
                Ok(Some(v)) => Ok(Json::from(v)),
                Ok(None) => Ok(Json::Null),
                Err(e) => Err(pg(e)),
            }
        }
        Some(FieldType::Float | FieldType::Decimal) => {
            if let Ok(Some(v)) = row.try_get::<Option<f64>, _>(0) {
                return Ok(Json::from(v));
            }
            match row.try_get::<Option<i64>, _>(0) {
                Ok(Some(v)) => Ok(Json::from(v)),
                Ok(None) => Ok(Json::Null),
                Err(e) => Err(pg(e)),
            }
        }
        Some(field_type) => decode_typed_value(row, field_type),
        // COUNT(*): int8.
        None => match row.try_get::<Option<i64>, _>(0) {
            Ok(Some(v)) => Ok(Json::from(v)),
            Ok(None) => Ok(Json::Null),
            Err(e) => Err(pg(e)),
        },
    }
}

/// Typed single-value decode (column 0) for MIN/MAX aggregate results —
/// mirrors [`decode_column`]'s encoding for the field's type.
fn decode_typed_value(row: &PgRow, field_type: &FieldType) -> Result<Json, AppError> {
    Ok(match field_type {
        FieldType::Uuid => {
            let v: Option<Uuid> = row.try_get(0).map_err(pg)?;
            v.map(|u| Json::String(u.to_string())).unwrap_or(Json::Null)
        }
        FieldType::Bool => {
            let v: Option<bool> = row.try_get(0).map_err(pg)?;
            v.map(Json::Bool).unwrap_or(Json::Null)
        }
        FieldType::Int32 => {
            let v: Option<i32> = row.try_get(0).map_err(pg)?;
            v.map(|i| Json::from(i64::from(i))).unwrap_or(Json::Null)
        }
        FieldType::Int64 => {
            let v: Option<i64> = row.try_get(0).map_err(pg)?;
            v.map(Json::from).unwrap_or(Json::Null)
        }
        FieldType::Float | FieldType::Decimal => {
            let v: Option<f64> = row.try_get(0).map_err(pg)?;
            v.map(Json::from).unwrap_or(Json::Null)
        }
        FieldType::Text | FieldType::TextLarge | FieldType::Date | FieldType::Enum { .. } => {
            let v: Option<String> = row.try_get(0).map_err(pg)?;
            v.map(Json::String).unwrap_or(Json::Null)
        }
        FieldType::Timestamp => {
            let v: Option<OffsetDateTime> = row.try_get(0).map_err(pg)?;
            match v {
                Some(ts) => Json::String(
                    ts.format(&time::format_description::well_known::Rfc3339)
                        .map_err(pg)?,
                ),
                None => Json::Null,
            }
        }
        FieldType::Json => {
            let v: Option<Json> = row.try_get(0).map_err(pg)?;
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
            Some(f) => keel_models::json_to_bind(value, f),
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

/// Append the filter list as WHERE clauses. Every filter is prepared into a
/// clause pusher first (Ident/bind errors surface outside the group
/// closure); a list containing an OR-tagged existence term wraps in ONE
/// parenthesized group so OR can never escape the tenant/soft-delete scope
/// ahead of it — `tenant AND (a OR EXISTS (...))`.
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


fn fetch_rows(pool: &PgPool, compiled: &CompiledSelect) -> Result<Vec<PgRow>, AppError> {
    let sql = compiled.sql.clone();
    let params = compiled.params.clone();
    let pool = pool.clone();
    block_on(async move {
        let query = bind_params(sqlx::query(&sql), &params);
        query.fetch_all(&pool).await.map_err(pg)
    })
}

/// Execute a write compiled by the typed builders; returns RETURNING rows.
fn write_rows(pool: &PgPool, compiled: &CompiledWrite) -> Result<Vec<PgRow>, AppError> {
    let sql = if compiled.sql.contains(" RETURNING ") {
        compiled.sql.clone()
    } else {
        format!("{} RETURNING *", compiled.sql)
    };
    let params = compiled.params.clone();
    let pool = pool.clone();
    block_on(async move {
        let query = bind_params(sqlx::query(&sql), &params);
        query.fetch_all(&pool).await.map_err(pg)
    })
}

fn map_rows(rows: Vec<PgRow>, model: &ModelDef) -> Result<Vec<Record>, AppError> {
    rows.iter().map(|r| row_to_record(r, model)).collect()
}

impl ModelStore for PgModelStore {
    fn list(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        opts: &ListOpts,
    ) -> Result<keel_query::Page<Record>, AppError> {
        validate_list_opts(model, opts)?;
        let mut select = base_select(model, tenant, filters, opts.include_deleted)?;
        if let Some((field, desc)) = &opts.sort {
            select = select.order_by(
                ident(field)?,
                if *desc { Direction::Desc } else { Direction::Asc },
            );
        }
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
        let compiled = select.build()?;
        let rows = fetch_rows(&self.pool, &compiled)?;
        let total = if opts.page.is_some() {
            // COUNT over the same shape (rebuild without limit/offset).
            let count_select = base_select(model, tenant, filters, opts.include_deleted)?.build()?;
            let count_sql = format!(
                "SELECT COUNT(*) FROM ({}) AS _keel_count",
                count_select.sql
            );
            let params = count_select.params.clone();
            let pool = self.pool.clone();
            let count: i64 = block_on(async move {
                let query = bind_params(sqlx::query(&count_sql), &params);
                let row: PgRow = query.fetch_one(&pool).await.map_err(pg)?;
                row.try_get::<i64, usize>(0).map_err(pg)
            })?;
            count.max(0) as u64
        } else {
            rows.len() as u64
        };
        let items = map_rows(rows, model)?;
        Ok(keel_query::Page::new(items, page, per_page, Some(total)))
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
        let rows = fetch_rows(&self.pool, &select.build()?)?;
        Ok(map_rows(rows, model)?.into_iter().next())
    }

    fn insert(&self, model: &ModelDef, record: &Record) -> Result<Record, AppError> {
        let (columns, params) = record_to_columns(record)?;
        let mut insert = Insert::into_table(ident(&model.table)?).columns(columns);
        insert = insert.row(params);
        if model.tenant_scoped {
            insert = insert.tenant_scope(
                ident("tenant_id")?,
                keel_foundation::TenantId::from_uuid(record_uuid(record, "tenant_id")?),
            );
        }
        let compiled = insert.returning(all_columns(model)?).build()?;
        let rows = write_rows(&self.pool, &compiled)?;
        map_rows(rows, model)?
            .into_iter()
            .next()
            .ok_or_else(|| pg("insert returned no row"))
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
                Some(f) => keel_models::json_to_bind(value, f)?,
                None => json_value_to_bind(value)?,
            };
            update = update.set(ident(key)?, bind);
        }
        let compiled = update.returning(all_columns(model)?).build()?;
        let rows = write_rows(&self.pool, &compiled)?;
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
            let compiled = update.returning(vec![ident("id")?]).build()?;
            let rows = write_rows(&self.pool, &compiled)?;
            return Ok(!rows.is_empty());
        }
        let mut delete = Delete::from_table(ident(&model.table)?).where_eq(ident("id")?, Bind::Uuid(id));
        if model.tenant_scoped {
            match tenant {
                Some(t) => delete = delete.tenant_scope(ident("tenant_id")?, t),
                None => delete = delete.where_eq(ident("tenant_id")?, Bind::Null),
            }
        }
        let compiled = delete.returning(vec![ident("id")?]).build()?;
        let rows = write_rows(&self.pool, &compiled)?;
        Ok(!rows.is_empty())
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
        let (columns, params) = record_to_columns(record)?;
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
                keel_foundation::TenantId::from_uuid(record_uuid(record, "tenant_id")?),
            );
        }
        let compiled = insert.returning(all_columns(model)?).build()?;
        let rows = write_rows(&self.pool, &compiled)?;
        map_rows(rows, model)?
            .into_iter()
            .next()
            .ok_or_else(|| pg("upsert returned no row"))
    }

    fn aggregate(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        op: keel_models::AggregateOp,
    ) -> Result<Json, AppError> {
        let field_type = validate_aggregate(model, &op)?;
        let mut select = base_select(model, tenant, filters, false)?;
        select = match &op {
            keel_models::AggregateOp::Count => select.count(),
            keel_models::AggregateOp::Sum(f) => select.sum(ident(f)?),
            keel_models::AggregateOp::Avg(f) => select.avg(ident(f)?),
            keel_models::AggregateOp::Min(f) => select.min(ident(f)?),
            keel_models::AggregateOp::Max(f) => select.max(ident(f)?),
        };
        let compiled = select.build()?;
        let rows = fetch_rows(&self.pool, &compiled)?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| pg("aggregate returned no row"))?;
        decode_aggregate(&row, field_type)
    }

    fn exists(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
    ) -> Result<bool, AppError> {
        let select = base_select(model, tenant, filters, false)?.exists();
        let compiled = select.build()?;
        let rows = fetch_rows(&self.pool, &compiled)?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| pg("EXISTS returned no row"))?;
        row.try_get::<bool, _>(0).map_err(pg)
    }

    fn find_by(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError> {
        let select = base_select(model, tenant, filters, include_deleted)?.limit(1);
        let compiled = select.build()?;
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
        let compiled = select.build()?;
        let rows = fetch_rows(&self.pool, &compiled)?;
        Ok(map_rows(rows, model)?.into_iter().next())
    }

    fn insert_or_ignore(&self, model: &ModelDef, record: &Record) -> Result<Option<Record>, AppError> {
        let (columns, params) = record_to_columns(record)?;
        let mut insert = Insert::into_table(ident(&model.table)?)
            .columns(columns)
            .row(params)
            // Target-less DO NOTHING: ignores ANY unique violation.
            .on_conflict(Vec::new(), ConflictPolicy::DoNothing);
        if model.tenant_scoped {
            insert = insert.tenant_scope(
                ident("tenant_id")?,
                keel_foundation::TenantId::from_uuid(record_uuid(record, "tenant_id")?),
            );
        }
        let compiled = insert.returning(all_columns(model)?).build()?;
        let rows = write_rows(&self.pool, &compiled)?;
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
                Some(f) => keel_models::json_to_bind(value, f)?,
                None => json_value_to_bind(value)?,
            };
            update = update.set(ident(field)?, bind);
        }
        let compiled = update.returning(all_columns(model)?).build()?;
        let rows = write_rows(&self.pool, &compiled)?;
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
            changes.insert("deleted_at".into(), Json::String(keel_models::now_rfc3339()));
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
        let compiled = delete.returning(all_columns(model)?).build()?;
        let rows = write_rows(&self.pool, &compiled)?;
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
        let compiled = delete.build()?;
        let rows = write_rows(&self.pool, &compiled)?;
        Ok(!rows.is_empty())
    }

    fn truncate(&self, model: &ModelDef, env: &str) -> Result<(), AppError> {
        let compiled = keel_query::Truncate::table(ident(&model.table)?).build_gated(env)?;
        let sql = compiled.sql;
        let params = compiled.params;
        let pool = self.pool.clone();
        block_on(async move {
            let query = bind_params(sqlx::query(&sql), &params);
            query.execute(&pool).await.map_err(pg)?;
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
            // Atomic `SET field = field + by` — one statement with `extra`.
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
                Some(f) => keel_models::json_to_bind(value, f)?,
                None => json_value_to_bind(value)?,
            };
            update = update.set(ident(key)?, bind);
        }
        let compiled = update.returning(all_columns(model)?).build()?;
        let rows = write_rows(&self.pool, &compiled)?;
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
        let compiled = update.returning(all_columns(model)?).build()?;
        let rows = write_rows(&self.pool, &compiled)?;
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

/// RETURNING column list: the model's declared fields (framework timestamp
/// columns are returned only when the model declares them).
fn all_columns(model: &ModelDef) -> Result<Vec<Ident>, AppError> {
    let mut names: Vec<String> = model.fields.iter().map(|f| f.name.clone()).collect();
    names.sort();
    names.dedup();
    names.iter().map(|n| ident(n)).collect()
}