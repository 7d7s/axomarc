//! Model store port + in-memory implementation (S8 C3/B-04).
//!
//! The port is SEMANTIC (CRUD over validated records), not raw SQL: stores
//! receive records that already passed the [`crate::runtime::ModelRuntime`]
//! mass-assignment/type/tenant boundary. `InMemoryModelStore` backs TestKit
//! and degraded boot; `PgModelStore` (kelson-postgres) backs production.
//!
//! Canonical record encoding: one JSON object per row. UUIDs are strings,
//! timestamps are RFC 3339 strings (UTC), numbers are JSON numbers, jsonb
//! columns are nested JSON. Both stores speak this same shape, so a module
//! can swap backends without touching domain code.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use kelson_foundation::{AppError, TenantId};
use kelson_query::PageReq;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use uuid::Uuid;

use crate::{FieldType, ModelDef};

/// One persisted row, keyed by column name.
pub type Record = serde_json::Map<String, Json>;

/// Build the keyset predicate for a **cursor** page: the `(column, value)` pairs
/// the page starts *after*, as the query builder's row comparison
/// (`(col1, col2) > ($1, $2)`).
///
/// The columns are validated against the model — an unknown column or a
/// value that does not fit the field is a validation error, never a silently
/// ignored anchor.
pub fn keyset_predicate(
    model: &ModelDef,
    after: &[(String, Json)],
) -> Result<(Vec<kelson_query::Ident>, Vec<kelson_query::Value>), AppError> {
    if after.is_empty() {
        return Err(AppError::validation(
            "a keyset anchor needs at least one column",
        ));
    }
    let mut columns = Vec::with_capacity(after.len());
    let mut values = Vec::with_capacity(after.len());
    for (name, value) in after {
        let field = model.field_named(name).ok_or_else(|| {
            AppError::validation(format!("{name} is not a field of {}", model.name))
        })?;
        columns.push(kelson_query::Ident::new(name.clone())?);
        values.push(crate::runtime::json_to_bind(value, field)?);
    }
    Ok((columns, values))
}

/// Does `value` sort strictly **after** `anchor`?
///
/// Numbers compare numerically, strings lexicographically (RFC 3339 timestamps
/// therefore sort chronologically). A null anchor or a null value is never
/// "after" — a null-sorted column does not participate in cursor mode, and the
/// page is expected to be driven by a non-null sort column.
pub fn json_sorts_after(value: &Json, anchor: &Json) -> bool {
    match (value, anchor) {
        (Json::Number(a), Json::Number(b)) => a.as_f64().unwrap_or(f64::NEG_INFINITY)
            > b.as_f64().unwrap_or(f64::NEG_INFINITY),
        (Json::String(a), Json::String(b)) => a > b,
        _ => false,
    }
}

/// Does `value` sort strictly **before** `anchor`? The mirror of
/// [`json_sorts_after`], used for a descending sort (where the window after
/// the anchor is the rows with a *smaller* value).
pub fn json_sorts_before(value: &Json, anchor: &Json) -> bool {
    match (value, anchor) {
        (Json::Number(a), Json::Number(b)) => a.as_f64().unwrap_or(f64::NEG_INFINITY)
            < b.as_f64().unwrap_or(f64::NEG_INFINITY),
        (Json::String(a), Json::String(b)) => a < b,
        _ => false,
    }
}

/// Comparison operators for semantic filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterOp {
    Eq,
    Ne,
    In,
    IsNull,
    NotNull,
    /// Range comparisons (Wave 3 Phase 6 prune predicates): ordered by the
    /// same scalar ordering as MIN/MAX — numbers numerically, strings
    /// lexicographically (RFC3339 timestamps therefore sort chronologically).
    /// A missing field, SQL NULL, or a non-orderable pair (mixed kinds)
    /// never matches.
    Lt,
    LtEq,
    Gt,
    GtEq,
}

/// One query filter: a field-level comparison (tenant-safe: stores apply
/// tenant scope separately) or a correlated relation-existence condition.
/// A filter list evaluates with SQL precedence — adjacent AND terms form
/// groups split at OR-tagged [`FieldFilter::Exists`] terms and the groups OR
/// together (the first term's junction is implicit, mirroring
/// [`kelson_query::Select`]'s clause rendering).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FieldFilter {
    /// Column comparison against a declared field.
    Cmp {
        field: String,
        op: FilterOp,
        value: Json,
    },
    /// `EXISTS` / `NOT EXISTS` over a related table, correlated through the
    /// relation's key columns. Built by [`crate::ModelRuntime`] from a
    /// declared relation; both stores apply the sub model's tenant scope and
    /// soft-delete veil from the flags (no registry lookup at evaluation).
    Exists(ExistsFilter),
}

/// Correlated existence condition: `EXISTS (SELECT * FROM sub_table WHERE
/// sub_table.sub_key = outer_table.outer_key [AND sub_filters...])`.
/// Nested existence (`Exists` inside `filters`) is refused by both stores —
/// keep the sub conditions field-level.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExistsFilter {
    /// `NOT EXISTS` instead of `EXISTS` (`doesnt_have` semantics).
    pub negated: bool,
    /// OR-join the preceding clause instead of AND (`or_where_has`); the
    /// first term of a list stays implicitly joined.
    pub or: bool,
    /// Related model's table.
    pub sub_table: String,
    /// Column on the sub table carrying the correlation value.
    pub sub_key: String,
    /// Owning table — the model the filter list belongs to.
    pub outer_table: String,
    /// Column on the outer table the correlation value comes from.
    pub outer_key: String,
    /// Sub model is tenant-scoped: sub rows must sit in the query's tenant.
    pub sub_tenant_scoped: bool,
    /// Sub model soft-deletes: trashed sub rows never satisfy `EXISTS`.
    pub sub_soft_deletes: bool,
    /// Field conditions evaluated against each sub row (ANDed) — field
    /// names live on the sub model.
    pub filters: Vec<FieldFilter>,
}

impl FieldFilter {
    pub fn eq(field: impl Into<String>, value: impl Into<Json>) -> Self {
        Self::Cmp {
            field: field.into(),
            op: FilterOp::Eq,
            value: value.into(),
        }
    }

    pub fn is_in(field: impl Into<String>, values: Vec<Json>) -> Self {
        Self::Cmp {
            field: field.into(),
            op: FilterOp::In,
            value: Json::Array(values),
        }
    }

    /// `field < value` (Wave 3 Phase 6 range filters / prune predicates).
    pub fn lt(field: impl Into<String>, value: impl Into<Json>) -> Self {
        Self::Cmp {
            field: field.into(),
            op: FilterOp::Lt,
            value: value.into(),
        }
    }

    /// `field <= value`.
    pub fn lte(field: impl Into<String>, value: impl Into<Json>) -> Self {
        Self::Cmp {
            field: field.into(),
            op: FilterOp::LtEq,
            value: value.into(),
        }
    }

    /// `field > value`.
    pub fn gt(field: impl Into<String>, value: impl Into<Json>) -> Self {
        Self::Cmp {
            field: field.into(),
            op: FilterOp::Gt,
            value: value.into(),
        }
    }

    /// `field >= value`.
    pub fn gte(field: impl Into<String>, value: impl Into<Json>) -> Self {
        Self::Cmp {
            field: field.into(),
            op: FilterOp::GtEq,
            value: value.into(),
        }
    }
}

/// Aggregate request for [`ModelStore::aggregate`]. COUNT(*) needs no field;
/// SUM/AVG require a numeric field and MIN/MAX any orderable field — both
/// validated against the model by the store at call time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AggregateOp {
    /// COUNT(*)
    Count,
    Sum(String),
    Avg(String),
    Min(String),
    Max(String),
}

impl AggregateOp {
    pub fn count() -> Self {
        AggregateOp::Count
    }

    pub fn sum(field: impl Into<String>) -> Self {
        AggregateOp::Sum(field.into())
    }

    pub fn avg(field: impl Into<String>) -> Self {
        AggregateOp::Avg(field.into())
    }

    pub fn min(field: impl Into<String>) -> Self {
        AggregateOp::Min(field.into())
    }

    pub fn max(field: impl Into<String>) -> Self {
        AggregateOp::Max(field.into())
    }

    /// The field the aggregate reads (`None` = COUNT(*)).
    pub fn field(&self) -> Option<&str> {
        match self {
            AggregateOp::Count => None,
            AggregateOp::Sum(f)
            | AggregateOp::Avg(f)
            | AggregateOp::Min(f)
            | AggregateOp::Max(f) => Some(f.as_str()),
        }
    }
}

/// List options: pagination (`None` = unbounded, for eager loads), the
/// soft-delete trash toggle, and (for eager fetches) a sort + row cap.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListOpts {
    pub page: Option<PageReq>,
    pub include_deleted: bool,
    /// Sort by `(field, desc)` — applied BEFORE pagination (stable JSON
    /// ordering in memory, `ORDER BY` in pg).
    pub sort: Option<(String, bool)>,
    /// Row cap — the eager "subquery limit". Mutually exclusive with
    /// `page` (validated by [`validate_list_opts`]).
    pub limit: Option<u32>,
    /// Keyset anchor for **cursor** pagination: the `(column, value)` pairs the
    /// page starts *after*, in the same order as `sort`. `None` = offset
    /// pagination. The stores render it as a row comparison (`(col) > ($1)`),
    /// so a concurrent insert cannot shift the window the way OFFSET does.
    #[serde(default)]
    pub after: Option<Vec<(String, serde_json::Value)>>,
}

/// Semantic CRUD port over a [`ModelDef`].
pub trait ModelStore: Send + Sync {
    fn list(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        opts: &ListOpts,
    ) -> Result<kelson_query::Page<Record>, AppError>;

    fn find(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError>;

    /// Persist a prepared (runtime-validated) record; returns the stored row.
    fn insert(&self, model: &ModelDef, record: &Record) -> Result<Record, AppError>;

    /// Apply `changes` to the row `id`; returns the updated row.
    ///
    /// **Trash contract (adjudicated 2026-10-19, backlog #1)**: this is the
    /// raw write primitive and it carries **no trash veil** — a trashed row
    /// IS updated and stays trashed (Laravel's instance-`save()` semantics).
    /// `None` means "no such row" (wrong id, wrong tenant), never "the row is
    /// trashed". The veil lives one layer up: [`ModelStore::update_filtered`]
    /// and [`ModelStore::delete`] skip trashed rows, and the runtime's
    /// `update` pre-reads with the veil so a trashed row reads as "not found"
    /// unless the caller opts in with `update_with_trashed`.
    fn update(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        changes: &Record,
    ) -> Result<Option<Record>, AppError>;

    /// Soft- or hard-delete per `model.soft_deletes`. True when a row was
    /// removed/marked.
    fn delete(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<bool, AppError>;

    /// Insert-or-merge keyed on `conflict` field values. Returns the stored
    /// row either way.
    fn upsert(
        &self,
        model: &ModelDef,
        record: &Record,
        conflict: &[String],
    ) -> Result<Record, AppError>;

    /// Aggregate over tenant+filter-scoped LIVE rows (soft-deleted rows never
    /// count): COUNT(*) always; SUM/AVG need a numeric field, MIN/MAX any
    /// orderable non-jsonb field (validated here). Empty match sets: Count →
    /// 0, SUM/AVG/MIN/MAX → JSON null (SQL NULL semantics).
    fn aggregate(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        op: AggregateOp,
    ) -> Result<Json, AppError>;

    /// True when at least one LIVE row matches tenant+filters.
    fn exists(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
    ) -> Result<bool, AppError>;

    /// First row matching tenant+filters (scan/insertion order); the
    /// soft-delete veil applies unless `include_deleted`.
    fn find_by(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError>;

    /// [`ModelStore::find`] with a row lock (`SELECT ... FOR UPDATE`); the
    /// in-memory backend relies on its store-wide mutex (equivalent effect
    /// for a process-local store).
    fn find_for_update(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError>;

    /// Insert unless a conflicting row exists — same `id` or a declared
    /// unique field matching within the record's tenant (Postgres also
    /// ignores any other unique violation via target-less
    /// `ON CONFLICT DO NOTHING`). Returns the stored row, or `None` when the
    /// insert was ignored.
    fn insert_or_ignore(&self, model: &ModelDef, record: &Record) -> Result<Option<Record>, AppError>;

    /// Filter-keyed UPDATE (pivot sync, batch edits): applies `changes` to
    /// every row matching tenant+filters (live rows only); returns the
    /// updated rows. An empty filter list is refused — no unfiltered write.
    fn update_filtered(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        changes: &Record,
    ) -> Result<Vec<Record>, AppError>;

    /// Filter-keyed DELETE (pivot detach, batch removal): soft-marks or
    /// removes every matching live row per `model.soft_deletes`; returns the
    /// affected rows. An empty filter list is refused.
    fn delete_filtered(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
    ) -> Result<Vec<Record>, AppError>;

    /// Hard-delete the row even when `model.soft_deletes` (trash purge,
    /// prune). True when a row was removed — live or already trashed.
    fn force_delete(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<bool, AppError>;

    /// TRUNCATE the model's table — refused unless `env` is a dev/test
    /// environment (fail-closed gate shared with the SQL builder).
    fn truncate(&self, model: &ModelDef, env: &str) -> Result<(), AppError>;

    /// Atomic counter write: `SET field = field + by` plus every `extra`
    /// assignment (e.g. `updated_at`) in ONE statement, so concurrent
    /// increments never lose an update. Returns the updated row; `Ok(None)`
    /// when no live row matches (missing/soft-deleted/foreign tenant).
    fn increment(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        field: &str,
        by: i64,
        extra: &Record,
    ) -> Result<Option<Record>, AppError>;

    /// Soft-delete undo (Wave 3 Phase 6): clear the trash mark on the
    /// TRASHED row `id` — `Some(row)` = the row was trashed and is now live;
    /// `None` = no such row or the row was already live. Never resurrects a
    /// live row (idempotent at the runtime layer) and never touches
    /// `updated_at` — the port only clears the mark, mirroring how the soft
    /// delete only sets it. Refused by both backends' callers for models
    /// that don't soft-delete (`ModelRuntime::restore` guards).
    fn restore(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<Option<Record>, AppError>;
}

/// Arc delegation: `Arc<impl ModelStore>` (and `Arc<dyn ModelStore>`) is
/// itself a store, so registries can share one instance across runtimes.
impl<T: ModelStore + ?Sized> ModelStore for Arc<T> {
    fn list(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        opts: &ListOpts,
    ) -> Result<kelson_query::Page<Record>, AppError> {
        (**self).list(model, tenant, filters, opts)
    }

    fn find(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError> {
        (**self).find(model, id, tenant, include_deleted)
    }

    fn insert(&self, model: &ModelDef, record: &Record) -> Result<Record, AppError> {
        (**self).insert(model, record)
    }

    fn update(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        changes: &Record,
    ) -> Result<Option<Record>, AppError> {
        (**self).update(model, id, tenant, changes)
    }

    fn delete(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<bool, AppError> {
        (**self).delete(model, id, tenant)
    }

    fn upsert(
        &self,
        model: &ModelDef,
        record: &Record,
        conflict: &[String],
    ) -> Result<Record, AppError> {
        (**self).upsert(model, record, conflict)
    }

    fn aggregate(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        op: AggregateOp,
    ) -> Result<Json, AppError> {
        (**self).aggregate(model, tenant, filters, op)
    }

    fn exists(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
    ) -> Result<bool, AppError> {
        (**self).exists(model, tenant, filters)
    }

    fn find_by(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError> {
        (**self).find_by(model, tenant, filters, include_deleted)
    }

    fn find_for_update(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError> {
        (**self).find_for_update(model, id, tenant, include_deleted)
    }

    fn insert_or_ignore(&self, model: &ModelDef, record: &Record) -> Result<Option<Record>, AppError> {
        (**self).insert_or_ignore(model, record)
    }

    fn update_filtered(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        changes: &Record,
    ) -> Result<Vec<Record>, AppError> {
        (**self).update_filtered(model, tenant, filters, changes)
    }

    fn delete_filtered(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
    ) -> Result<Vec<Record>, AppError> {
        (**self).delete_filtered(model, tenant, filters)
    }

    fn force_delete(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<bool, AppError> {
        (**self).force_delete(model, id, tenant)
    }

    fn truncate(&self, model: &ModelDef, env: &str) -> Result<(), AppError> {
        (**self).truncate(model, env)
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
        (**self).increment(model, id, tenant, field, by, extra)
    }

    fn restore(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<Option<Record>, AppError> {
        (**self).restore(model, id, tenant)
    }
}

/// RFC 3339 UTC timestamp string for "now" (canonical record encoding).
pub fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("rfc3339 always formats")
}

fn uuid_str(record: &Record, key: &str) -> Option<Uuid> {
    record
        .get(key)
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
}

fn is_deleted(record: &Record) -> bool {
    matches!(record.get("deleted_at"), Some(Json::String(s)) if !s.is_empty())
}

fn tenant_matches(model: &ModelDef, record: &Record, tenant: Option<TenantId>) -> bool {
    tenant_row_ok(model.tenant_scoped, record, tenant)
}

/// Tenant posture check without a `ModelDef` — existence sub rows carry
/// their posture as flags on [`ExistsFilter`].
fn tenant_row_ok(tenant_scoped: bool, record: &Record, tenant: Option<TenantId>) -> bool {
    if !tenant_scoped {
        return true;
    }
    let Some(tenant) = tenant else {
        // Tenant-scoped rows queried without a tenant see nothing.
        return false;
    };
    uuid_str(record, "tenant_id") == Some(*tenant.as_uuid())
}

fn cmp_matches(record: &Record, field: &str, op: FilterOp, value: &Json) -> bool {
    let actual = record.get(field);
    match op {
        FilterOp::Eq => actual == Some(value),
        FilterOp::Ne => actual != Some(value),
        FilterOp::In => value
            .as_array()
            .map(|list| actual.map(|a| list.contains(a)).unwrap_or(false))
            .unwrap_or(false),
        FilterOp::IsNull => matches!(actual, None | Some(Json::Null)),
        FilterOp::NotNull => !matches!(actual, None | Some(Json::Null)),
        // Range terms: unorderable pairs (missing/NULL/mixed kinds) never
        // match, mirroring SQL three-valued logic where `NULL < x` is
        // unknown rather than true.
        FilterOp::Lt | FilterOp::LtEq | FilterOp::Gt | FilterOp::GtEq => {
            let Some(ord) = actual.and_then(|a| json_order(a, value)) else {
                return false;
            };
            match op {
                FilterOp::Lt => ord == std::cmp::Ordering::Less,
                FilterOp::LtEq => ord != std::cmp::Ordering::Greater,
                FilterOp::Gt => ord == std::cmp::Ordering::Greater,
                FilterOp::GtEq => ord != std::cmp::Ordering::Less,
                _ => unreachable!("guarded by the outer match"),
            }
        }
    }
}

/// A filter list compiled for in-memory evaluation: comparison terms borrow
/// from the caller's list; existence terms are pre-resolved against the
/// store's tables (one lock, outside the row cursor) into qualifying-key
/// sets.
struct FilterPlan<'a> {
    terms: Vec<PlannedTerm<'a>>,
}

enum PlannedTerm<'a> {
    Cmp {
        field: &'a str,
        op: FilterOp,
        value: &'a Json,
    },
    Exists {
        /// Column on the OUTER row carrying the correlation value.
        outer_key: &'a str,
        negated: bool,
        or: bool,
        /// Stringified sub-key values of sub rows satisfying tenant scope,
        /// the soft-delete veil, and every sub condition.
        qualifying: HashSet<String>,
    },
}

impl FilterPlan<'_> {
    /// SQL-precedence evaluation: adjacent AND terms form groups split at
    /// OR-tagged existence terms, groups OR together, and the first term's
    /// junction is implicit (mirrors the SQL builder's rendering). An empty
    /// list matches every row — the filters are the WHERE clause, tenant and
    /// soft-delete scope are applied separately by the caller.
    fn matches(&self, row: &Record) -> bool {
        let mut groups_or = false;
        let mut group: Option<bool> = None;
        for term in &self.terms {
            let (hit, or) = match term {
                PlannedTerm::Cmp { field, op, value } => (cmp_matches(row, field, *op, value), false),
                PlannedTerm::Exists {
                    outer_key,
                    negated,
                    or,
                    qualifying,
                } => {
                    let linked = row
                        .get(*outer_key)
                        .filter(|v| !v.is_null())
                        .map(|v| qualifying.contains(&v.to_string()))
                        .unwrap_or(false);
                    (*negated != linked, *or)
                }
            };
            if or && group.is_some() {
                groups_or |= group.take().expect("group was Some");
            }
            group = Some(match group {
                None => hit,
                Some(acc) => acc && hit,
            });
        }
        groups_or | group.unwrap_or(true)
    }
}

/// Resolve one filter into a planned term; existence terms scan the sub
/// table for rows passing tenant scope + soft veil + sub conditions.
fn resolve_exists<'a>(
    tables: &HashMap<String, Vec<Record>>,
    tenant: Option<TenantId>,
    ex: &'a ExistsFilter,
) -> Result<PlannedTerm<'a>, AppError> {
    if ex
        .filters
        .iter()
        .any(|f| matches!(f, FieldFilter::Exists(_)))
    {
        return Err(AppError::validation(
            "relation filters cannot nest inside a relation filter (not supported)",
        ));
    }
    let mut qualifying = HashSet::new();
    if let Some(rows) = tables.get(&ex.sub_table) {
        for row in rows {
            if !tenant_row_ok(ex.sub_tenant_scoped, row, tenant) {
                continue;
            }
            if ex.sub_soft_deletes && is_deleted(row) {
                continue;
            }
            let sub_ok = ex.filters.iter().all(|f| {
                matches!(
                    f,
                    FieldFilter::Cmp { field, op, value } if cmp_matches(row, field, *op, value)
                )
            });
            if !sub_ok {
                continue;
            }
            if let Some(key) = row.get(&ex.sub_key) {
                if !key.is_null() {
                    qualifying.insert(key.to_string());
                }
            }
        }
    }
    Ok(PlannedTerm::Exists {
        outer_key: ex.outer_key.as_str(),
        negated: ex.negated,
        or: ex.or,
        qualifying,
    })
}

/// Validate an aggregate against the model (shared by both backends):
/// unknown fields are refused; SUM/AVG need numeric fields; MIN/MAX any
/// field except jsonb (no portable ordering). Returns the field type when a
/// field is involved (`None` = COUNT(*)).
pub fn validate_aggregate<'m>(
    model: &'m ModelDef,
    op: &AggregateOp,
) -> Result<Option<&'m FieldType>, AppError> {
    let Some(name) = op.field() else {
        return Ok(None);
    };
    let field = model.field_named(name).ok_or_else(|| {
        AppError::validation(format!(
            "aggregate field {name:?} is not on model {}",
            model.table
        ))
    })?;
    match op {
        AggregateOp::Sum(_) | AggregateOp::Avg(_) => {
            if !matches!(
                field.field_type,
                FieldType::Int32 | FieldType::Int64 | FieldType::Float | FieldType::Decimal
            ) {
                return Err(AppError::validation(format!(
                    "{op:?} aggregate needs a numeric field, {name:?} is {:?}",
                    field.field_type
                )));
            }
        }
        AggregateOp::Min(_) | AggregateOp::Max(_) => {
            if matches!(field.field_type, FieldType::Json) {
                return Err(AppError::validation(format!(
                    "{op:?} aggregate cannot order jsonb field {name:?}"
                )));
            }
        }
        AggregateOp::Count => {}
    }
    Ok(Some(&field.field_type))
}

/// Fold extracted non-null values for SUM/AVG/MIN/MAX — shared by the
/// in-memory store's `aggregate` and the runtime's eager aggregate family
/// (`with_sum`/`with_avg`/...), so both compute byte-identical values.
/// COUNT never reaches here (it counts rows, not values). Empty input folds
/// to SQL NULL.
pub fn fold_aggregate_values(
    values: &[&Json],
    name: &str,
    op: &AggregateOp,
) -> Result<Json, AppError> {
    if values.is_empty() {
        // SQL NULL semantics: no rows to aggregate.
        return Ok(Json::Null);
    }
    let finite = |f: f64| {
        serde_json::Number::from_f64(f)
            .map(Json::Number)
            .ok_or_else(|| AppError::validation("aggregate produced a non-finite number"))
    };
    match op {
        AggregateOp::Count => Err(AppError::validation(
            "COUNT folds rows, not values — count at the call site",
        )),
        AggregateOp::Sum(_) => {
            let mut int_sum: Option<i64> = Some(0);
            let mut float_sum = 0f64;
            for v in values {
                let Json::Number(n) = v else {
                    return Err(AppError::validation(format!(
                        "field {name:?} holds a non-numeric value"
                    )));
                };
                match (int_sum.as_mut(), n.as_i64()) {
                    (Some(acc), Some(i)) => {
                        *acc += i;
                        float_sum += i as f64;
                    }
                    _ => {
                        float_sum += n.as_f64().ok_or_else(|| {
                            AppError::validation(format!(
                                "field {name:?} holds a non-numeric value"
                            ))
                        })?;
                        int_sum = None;
                    }
                }
            }
            match int_sum {
                Some(total) => Ok(Json::from(total)),
                None => finite(float_sum),
            }
        }
        AggregateOp::Avg(_) => {
            let mut sum = 0f64;
            for v in values {
                let Json::Number(n) = v else {
                    return Err(AppError::validation(format!(
                        "field {name:?} holds a non-numeric value"
                    )));
                };
                sum += n.as_f64().ok_or_else(|| {
                    AppError::validation(format!("field {name:?} holds a non-numeric value"))
                })?;
            }
            finite(sum / values.len() as f64)
        }
        AggregateOp::Min(_) | AggregateOp::Max(_) => {
            let want_min = matches!(op, AggregateOp::Min(_));
            let mut best = values[0];
            for v in &values[1..] {
                let ord = json_order(best, v).ok_or_else(|| {
                    AppError::validation(format!("min/max on {name:?} needs comparable values"))
                })?;
                let take = (want_min && ord == std::cmp::Ordering::Greater)
                    || (!want_min && ord == std::cmp::Ordering::Less);
                if take {
                    best = v;
                }
            }
            Ok(best.clone())
        }
    }
}

/// Shared `ListOpts` validation for both stores: `sort` must name a field
/// on the model, and `page`/`limit` are mutually exclusive (pagination vs
/// eager-fetch row cap).
pub fn validate_list_opts(model: &ModelDef, opts: &ListOpts) -> Result<(), AppError> {
    if opts.page.is_some() && opts.limit.is_some() {
        return Err(AppError::validation(
            "list: page and limit are mutually exclusive",
        ));
    }
    if let Some((field, _)) = &opts.sort {
        if field != "id" && model.field_named(field).is_none() {
            return Err(AppError::validation(format!(
                "sort on unknown field {field:?}"
            )));
        }
    }
    Ok(())
}

/// Stable sort of records by `field` using JSON ordering (missing keys
/// compare as null); `desc` reverses. Shared by the in-memory store's
/// `list` and the runtime's eager assembly.
pub fn sort_records(rows: &mut [Record], field: &str, desc: bool) {
    rows.sort_by(|a, b| {
        let ord = json_order(a.get(field).unwrap_or(&Json::Null), b.get(field).unwrap_or(&Json::Null))
            .unwrap_or(std::cmp::Ordering::Equal);
        if desc {
            ord.reverse()
        } else {
            ord
        }
    });
}

/// Unfiltered-write guard for the filter-keyed store operations.
pub fn require_filters(op: &str, filters: &[FieldFilter]) -> Result<(), AppError> {
    if filters.is_empty() {
        return Err(AppError::validation(format!(
            "{op} requires at least one filter"
        )));
    }
    Ok(())
}

/// Write-path filter guard: `update_filtered`/`delete_filtered` accept
/// comparison terms only. The UPDATE/DELETE builders render WHERE from
/// plain conditions (no EXISTS/group clauses), so relation-existence terms
/// are refused on both backends — they live on the read surface
/// (list/find/aggregate/exists).
pub fn require_write_filters(op: &str, filters: &[FieldFilter]) -> Result<(), AppError> {
    require_filters(op, filters)?;
    if filters.iter().any(|f| matches!(f, FieldFilter::Exists(_))) {
        return Err(AppError::validation(format!(
            "{op} does not accept relation-existence filters (read-side only)"
        )));
    }
    Ok(())
}

/// Counter-field validation for [`ModelStore::increment`] (shared by both
/// backends and the runtime): the field must be declared, mutable (not
/// `id`/`tenant_id`), and numeric.
pub fn validate_counter(model: &ModelDef, field: &str) -> Result<(), AppError> {
    if field == "id" || field == "tenant_id" {
        return Err(AppError::validation(format!(
            "counter field {field:?} is immutable"
        )));
    }
    let target = model.field_named(field).ok_or_else(|| {
        AppError::validation(format!(
            "counter field {field:?} is not on model {}",
            model.table
        ))
    })?;
    if !matches!(
        target.field_type,
        FieldType::Int32 | FieldType::Int64 | FieldType::Float | FieldType::Decimal
    ) {
        return Err(AppError::validation(format!(
            "counter field {field:?} is not numeric (is {:?})",
            target.field_type
        )));
    }
    Ok(())
}

/// Scalar ordering for MIN/MAX in the in-memory backend (numbers compare
/// numerically, strings lexicographically, bools false<true; mixed or
/// object/array values have no portable order).
fn json_order(a: &Json, b: &Json) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (Json::Number(x), Json::Number(y)) => x.as_f64().partial_cmp(&y.as_f64()),
        (Json::String(x), Json::String(y)) => Some(x.cmp(y)),
        (Json::Bool(x), Json::Bool(y)) => Some(x.cmp(y)),
        _ => None,
    }
}

/// Process-local record store: TestKit fake + degraded boot backend.
#[derive(Debug, Default)]
pub struct InMemoryModelStore {
    tables: Mutex<HashMap<String, Vec<Record>>>,
}

impl InMemoryModelStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn with_table<R>(
        &self,
        table: &str,
        f: impl FnOnce(&mut Vec<Record>) -> Result<R, AppError>,
    ) -> Result<R, AppError> {
        let mut tables = self.tables.lock().expect("model store poisoned");
        f(tables.entry(table.to_string()).or_default())
    }

    /// Deep-ish row copy (JSON clone).
    fn clone_row(row: &Record) -> Record {
        row.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    }

    /// Compile a filter list for row evaluation. Existence terms resolve
    /// against the tables here (single lock, dropped before any row cursor)
    /// into qualifying-key sets; comparison terms borrow. Refuses existence
    /// nested inside existence.
    fn filter_plan<'a>(
        &'a self,
        tenant: Option<TenantId>,
        filters: &'a [FieldFilter],
    ) -> Result<FilterPlan<'a>, AppError> {
        let tables = self.tables.lock().expect("model store poisoned");
        let mut terms = Vec::with_capacity(filters.len());
        for filter in filters {
            terms.push(match filter {
                FieldFilter::Cmp { field, op, value } => PlannedTerm::Cmp {
                    field: field.as_str(),
                    op: *op,
                    value,
                },
                FieldFilter::Exists(ex) => resolve_exists(&tables, tenant, ex)?,
            });
        }
        Ok(FilterPlan { terms })
    }
}

impl ModelStore for InMemoryModelStore {
    fn list(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        opts: &ListOpts,
    ) -> Result<kelson_query::Page<Record>, AppError> {
        validate_list_opts(model, opts)?;
        let plan = self.filter_plan(tenant, filters)?;
        let mut matched: Vec<Record> = self.with_table(&model.table, |rows| {
            Ok(rows
                .iter()
                .filter(|r| tenant_matches(model, r, tenant))
                .filter(|r| opts.include_deleted || !is_deleted(r))
                .filter(|r| plan.matches(r))
                .map(Self::clone_row)
                .collect())
        })?;
        if let Some((field, desc)) = &opts.sort {
            sort_records(&mut matched, field, *desc);
        }
        // Cursor (keyset) mode: drop everything at or before the anchor, so a
        // concurrent insert cannot shift the window the way OFFSET does. The
        // filter runs BEFORE the count, so `total` is the size of the window
        // that is left, not of the whole table.
        if let Some(after) = &opts.after {
            // Validate the anchor against the model the same way the SQL
            // backends do: an unknown column or a value that does not fit the
            // field is a validation error, never a silently empty page.
            let _ = keyset_predicate(model, after)?;
            let (column, anchor) = after.first().ok_or_else(|| {
                AppError::validation("a keyset anchor needs at least one column")
            })?;
            let desc = opts.sort.as_ref().map(|(_, d)| *d).unwrap_or(false);
            let anchor = anchor.clone();
            matched.retain(|row| {
                let Some(value) = row.get(column) else {
                    return false;
                };
                if desc {
                    json_sorts_before(value, &anchor)
                } else {
                    json_sorts_after(value, &anchor)
                }
            });
        }
        let total = matched.len() as u64;
        if let Some(n) = opts.limit {
            matched.truncate(n as usize);
        }
        let (items, page, per_page) = match opts.page {
            Some(req) => {
                let req = req.validate()?;
                let start = req.offset() as usize;
                let slice: Vec<Record> = matched
                    .into_iter()
                    .skip(start)
                    .take(req.per_page as usize)
                    .collect();
                (slice, req.page, req.per_page)
            }
            None => (matched, 1, 0),
        };
        Ok(kelson_query::Page::new(items, page, per_page, Some(total)))
    }

    fn find(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError> {
        self.with_table(&model.table, |rows| {
            Ok(rows
                .iter()
                .find(|r| {
                    uuid_str(r, "id") == Some(id)
                        && tenant_matches(model, r, tenant)
                        && (include_deleted || !is_deleted(r))
                })
                .map(Self::clone_row))
        })
    }

    fn insert(&self, model: &ModelDef, record: &Record) -> Result<Record, AppError> {
        let id = uuid_str(record, "id")
            .ok_or_else(|| AppError::validation("record needs a uuid id before insert"))?;
        self.with_table(&model.table, |rows| {
            if rows.iter().any(|r| uuid_str(r, "id") == Some(id)) {
                return Err(AppError::conflict(format!(
                    "{} already contains id {id}",
                    model.table
                )));
            }
            rows.push(Self::clone_row(record));
            Ok(Self::clone_row(record))
        })
    }

    fn update(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        changes: &Record,
    ) -> Result<Option<Record>, AppError> {
        self.with_table(&model.table, |rows| {
            // No trash veil (backlog #1): the raw write primitive updates a
            // trashed row too, and the row stays trashed — Laravel's
            // instance-`save()` semantics. The veil lives in `delete` /
            // `update_filtered` and in the runtime's pre-read.
            let Some(row) = rows
                .iter_mut()
                .find(|r| uuid_str(r, "id") == Some(id) && tenant_matches(model, r, tenant))
            else {
                return Ok(None);
            };
            for (key, value) in changes {
                row.insert(key.clone(), value.clone());
            }
            Ok(Some(Self::clone_row(row)))
        })
    }

    fn delete(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<bool, AppError> {
        if model.soft_deletes {
            // The soft delete keeps its OWN trash guard (backlog #1): the raw
            // `update` no longer veils, so without this guard a second delete
            // would re-stamp `deleted_at` and report `true`. Mirrors the SQL
            // adapters' `WHERE deleted_at IS NULL`.
            let deleted = self.with_table(&model.table, |rows| {
                let Some(row) = rows.iter_mut().find(|r| {
                    uuid_str(r, "id") == Some(id)
                        && tenant_matches(model, r, tenant)
                        && !is_deleted(r)
                }) else {
                    return Ok(false);
                };
                row.insert("deleted_at".into(), Json::String(now_rfc3339()));
                Ok(true)
            })?;
            return Ok(deleted);
        }
        self.with_table(&model.table, |rows| {
            let before = rows.len();
            rows.retain(|r| {
                !(uuid_str(r, "id") == Some(id)
                    && tenant_matches(model, r, tenant)
                    && !is_deleted(r))
            });
            Ok(rows.len() < before)
        })
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
        let record_tenant = uuid_str(record, "tenant_id").map(TenantId::from_uuid);
        if model.tenant_scoped && record_tenant.is_none() {
            return Err(AppError::validation(
                "tenant-scoped upsert record must carry tenant_id",
            ));
        }
        self.with_table(&model.table, |rows| {
            // Match on conflict-field equality within the record's tenant.
            if let Some(row) = rows.iter_mut().find(|r| {
                tenant_matches(model, r, record_tenant)
                    && conflict.iter().all(|f| r.get(f) == record.get(f))
            }) {
                for (key, value) in record {
                    // Identity is never overwritten by a merge — mirrors the
                    // pg `exclude_from_conflict_update(["id"])` posture.
                    if key == "id" {
                        continue;
                    }
                    row.insert(key.clone(), value.clone());
                }
                return Ok(Self::clone_row(row));
            }
            rows.push(Self::clone_row(record));
            Ok(Self::clone_row(record))
        })
    }

    fn aggregate(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        op: AggregateOp,
    ) -> Result<Json, AppError> {
        validate_aggregate(model, &op)?;
        let plan = self.filter_plan(tenant, filters)?;
        self.with_table(&model.table, |rows| {
            let matched: Vec<&Record> = rows
                .iter()
                .filter(|r| tenant_matches(model, r, tenant))
                .filter(|r| !is_deleted(r))
                .filter(|r| plan.matches(r))
                .collect();
            match op {
                AggregateOp::Count => Ok(Json::from(matched.len() as u64)),
                ref other => {
                    let name = other.field().expect("non-count aggregate has a field");
                    let values: Vec<&Json> = matched
                        .iter()
                        .filter_map(|r| r.get(name))
                        .filter(|v| !matches!(v, Json::Null))
                        .collect();
                    fold_aggregate_values(&values, name, other)
                }
            }
        })
    }

    fn exists(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
    ) -> Result<bool, AppError> {
        let plan = self.filter_plan(tenant, filters)?;
        self.with_table(&model.table, |rows| {
            Ok(rows.iter().any(|r| {
                tenant_matches(model, r, tenant) && !is_deleted(r) && plan.matches(r)
            }))
        })
    }

    fn find_by(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError> {
        let plan = self.filter_plan(tenant, filters)?;
        self.with_table(&model.table, |rows| {
            Ok(rows
                .iter()
                .find(|r| {
                    tenant_matches(model, r, tenant)
                        && (include_deleted || !is_deleted(r))
                        && plan.matches(r)
                })
                .map(Self::clone_row))
        })
    }

    fn find_for_update(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
        include_deleted: bool,
    ) -> Result<Option<Record>, AppError> {
        // The store-wide mutex is already held for the whole lookup, which is
        // the process-local equivalent of SELECT ... FOR UPDATE.
        self.find(model, id, tenant, include_deleted)
    }

    fn insert_or_ignore(&self, model: &ModelDef, record: &Record) -> Result<Option<Record>, AppError> {
        let id = uuid_str(record, "id")
            .ok_or_else(|| AppError::validation("record needs a uuid id before insert"))?;
        let record_tenant = uuid_str(record, "tenant_id").map(TenantId::from_uuid);
        let unique_fields: Vec<String> = model
            .fields
            .iter()
            .filter(|f| f.unique)
            .map(|f| f.name.clone())
            .collect();
        self.with_table(&model.table, |rows| {
            let conflict = rows.iter().any(|r| {
                if uuid_str(r, "id") == Some(id) {
                    return true;
                }
                // Unique-field collisions are tenant-scoped, mirroring upsert.
                tenant_matches(model, r, record_tenant)
                    && unique_fields.iter().any(|fname| {
                        record
                            .get(fname)
                            .is_some_and(|value| r.get(fname) == Some(value))
                    })
            });
            if conflict {
                return Ok(None);
            }
            rows.push(Self::clone_row(record));
            Ok(Some(Self::clone_row(record)))
        })
    }

    fn update_filtered(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        changes: &Record,
    ) -> Result<Vec<Record>, AppError> {
        require_write_filters("update_filtered", filters)?;
        let plan = self.filter_plan(tenant, filters)?;
        self.with_table(&model.table, |rows| {
            let mut updated = Vec::new();
            for row in rows.iter_mut() {
                if tenant_matches(model, row, tenant) && !is_deleted(row) && plan.matches(row) {
                    for (key, value) in changes {
                        row.insert(key.clone(), value.clone());
                    }
                    updated.push(Self::clone_row(row));
                }
            }
            Ok(updated)
        })
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
            changes.insert("deleted_at".into(), Json::String(now_rfc3339()));
            return self.update_filtered(model, tenant, filters, &changes);
        }
        let plan = self.filter_plan(tenant, filters)?;
        self.with_table(&model.table, |rows| {
            let mut removed = Vec::new();
            let mut kept = Vec::new();
            for row in rows.drain(..) {
                if tenant_matches(model, &row, tenant) && !is_deleted(&row) && plan.matches(&row) {
                    removed.push(row);
                } else {
                    kept.push(row);
                }
            }
            *rows = kept;
            Ok(removed)
        })
    }

    fn force_delete(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<bool, AppError> {
        self.with_table(&model.table, |rows| {
            let before = rows.len();
            // No soft-delete veil: purges live AND trashed rows alike.
            rows.retain(|r| !(uuid_str(r, "id") == Some(id) && tenant_matches(model, r, tenant)));
            Ok(rows.len() < before)
        })
    }

    fn truncate(&self, model: &ModelDef, env: &str) -> Result<(), AppError> {
        kelson_query::truncate::check_env(env)?;
        self.with_table(&model.table, |rows| {
            rows.clear();
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
        self.with_table(&model.table, |rows| {
            let mut updated = None;
            for row in rows.iter_mut() {
                if uuid_str(row, "id") == Some(id)
                    && tenant_matches(model, row, tenant)
                    && !is_deleted(row)
                {
                    let next = match row.get(field) {
                        Some(Json::Number(n)) => {
                            if let Some(current) = n.as_i64() {
                                Json::from(
                                    current
                                        .checked_add(by)
                                        .ok_or_else(|| AppError::validation("counter overflow"))?,
                                )
                            } else {
                                Json::from(n.as_f64().unwrap_or(0.0) + by as f64)
                            }
                        }
                        // Unset/NULL counters start at zero.
                        None | Some(Json::Null) => Json::from(by),
                        _ => {
                            return Err(AppError::validation(format!(
                                "counter field {field:?} holds a non-numeric value"
                            )))
                        }
                    };
                    row.insert(field.to_string(), next);
                    for (key, value) in extra {
                        row.insert(key.clone(), value.clone());
                    }
                    updated = Some(Self::clone_row(row));
                    break;
                }
            }
            Ok(updated)
        })
    }

    fn restore(
        &self,
        model: &ModelDef,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<Option<Record>, AppError> {
        // Hard-delete models have no trash to return to.
        if !model.soft_deletes {
            return Ok(None);
        }
        self.with_table(&model.table, |rows| {
            let mut restored = None;
            for row in rows.iter_mut() {
                if uuid_str(row, "id") == Some(id)
                    && tenant_matches(model, row, tenant)
                    && is_deleted(row)
                {
                    // Clear the mark (NULL, as in pg); `updated_at` stays
                    // untouched — the port only undoes the soft delete.
                    row.insert("deleted_at".into(), Json::Null);
                    restored = Some(Self::clone_row(row));
                    break;
                }
            }
            Ok(restored)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{FieldDef, FieldType};

    fn widget_model() -> ModelDef {
        ModelDef::new("Widget", "widgets")
            .tenant_scoped()
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("tenant_id", FieldType::Uuid).required())
            .field(FieldDef::new("name", FieldType::Text).required())
            .field(FieldDef::new("deleted_at", FieldType::Timestamp))
    }

    fn widget(tenant: TenantId, name: &str) -> Record {
        let mut r = Record::new();
        r.insert("id".into(), Json::String(Uuid::now_v7().to_string()));
        r.insert("tenant_id".into(), Json::String(tenant.as_uuid().to_string()));
        r.insert("name".into(), Json::String(name.into()));
        r
    }

    #[test]
    fn insert_find_list_delete_roundtrip() {
        let store = InMemoryModelStore::new();
        let model = widget_model();
        let tenant = TenantId::new();
        let row = store.insert(&model, &widget(tenant, "one")).unwrap();
        let id = uuid_str(&row, "id").unwrap();

        assert!(store.find(&model, id, Some(tenant), false).unwrap().is_some());
        assert!(store.find(&model, id, Some(TenantId::new()), false).unwrap().is_none());

        let page = store.list(&model, Some(tenant), &[], &ListOpts::default()).unwrap();
        assert_eq!(page.total, Some(1));

        assert!(store.delete(&model, id, Some(tenant)).unwrap());
        // Soft-deleted: invisible without trash toggle, visible with it.
        assert!(store.find(&model, id, Some(tenant), false).unwrap().is_none());
        assert!(store.find(&model, id, Some(tenant), true).unwrap().is_some());
    }

    #[test]
    fn duplicate_id_is_conflict() {
        let store = InMemoryModelStore::new();
        let model = widget_model();
        let tenant = TenantId::new();
        let row = widget(tenant, "dup");
        store.insert(&model, &row).unwrap();
        let err = store.insert(&model, &row).unwrap_err();
        assert_eq!(err.category, kelson_foundation::ErrorCategory::Conflict);
    }

    #[test]
    fn filters_and_pagination() {
        let store = InMemoryModelStore::new();
        let model = widget_model();
        let tenant = TenantId::new();
        for i in 0..5 {
            store.insert(&model, &widget(tenant, &format!("w{i}"))).unwrap();
        }
        let filtered = store
            .list(
                &model,
                Some(tenant),
                &[FieldFilter::eq("name", "w2")],
                &ListOpts::default(),
            )
            .unwrap();
        assert_eq!(filtered.total, Some(1));

        let paged = store
            .list(
                &model,
                Some(tenant),
                &[],
                &ListOpts {
                    page: Some(PageReq { page: 2, per_page: 2 }),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(paged.items.len(), 2);
        assert_eq!(paged.total, Some(5));
    }

    #[test]
    fn upsert_merges_on_conflict() {
        let store = InMemoryModelStore::new();
        let model = widget_model();
        let tenant = TenantId::new();
        let mut first = widget(tenant, "same");
        let key = first.get("name").cloned().unwrap();
        store.insert(&model, &first).unwrap();
        first.insert("name".into(), Json::String("same".into()));
        first.insert("extra".into(), Json::Bool(true));
        let merged = store.upsert(&model, &first, &["name".into()]).unwrap();
        assert_eq!(merged.get("extra"), Some(&Json::Bool(true)));
        let all = store.list(&model, Some(tenant), &[], &ListOpts::default()).unwrap();
        assert_eq!(all.total, Some(1));
        let _ = key;
    }

    #[test]
    fn hard_delete_removes_row() {
        let store = InMemoryModelStore::new();
        let model = ModelDef::new("Log", "logs")
            .hard_deletes()
            .field(FieldDef::new("id", FieldType::Uuid).required())
            .field(FieldDef::new("msg", FieldType::Text));
        let mut row = Record::new();
        row.insert("id".into(), Json::String(Uuid::now_v7().to_string()));
        row.insert("msg".into(), Json::String("hi".into()));
        store.insert(&model, &row).unwrap();
        let id = uuid_str(&row, "id").unwrap();
        assert!(store.delete(&model, id, None).unwrap());
        assert!(store.find(&model, id, None, true).unwrap().is_none());
    }

    // ---------------------------------------------------- Wave 3 (phase 2)

    fn meter_model() -> ModelDef {
        ModelDef::new("Meter", "meters")
            .tenant_scoped()
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("tenant_id", FieldType::Uuid).required())
            .field(FieldDef::new("reading", FieldType::Int64).required())
            .field(FieldDef::new("label", FieldType::Text).required().unique())
            .field(FieldDef::new("deleted_at", FieldType::Timestamp))
    }

    fn meter(tenant: TenantId, label: &str, reading: i64) -> Record {
        let mut r = Record::new();
        r.insert("id".into(), Json::String(Uuid::now_v7().to_string()));
        r.insert("tenant_id".into(), Json::String(tenant.as_uuid().to_string()));
        r.insert("label".into(), Json::String(label.into()));
        r.insert("reading".into(), Json::from(reading));
        r
    }

    fn hard_log_model() -> ModelDef {
        ModelDef::new("Log", "logs")
            .hard_deletes()
            .field(FieldDef::new("id", FieldType::Uuid).required())
            .field(FieldDef::new("msg", FieldType::Text))
    }

    fn log_row(msg: &str) -> Record {
        let mut r = Record::new();
        r.insert("id".into(), Json::String(Uuid::now_v7().to_string()));
        r.insert("msg".into(), Json::String(msg.into()));
        r
    }

    #[test]
    fn aggregate_is_tenant_scoped_and_typed() {
        let store = InMemoryModelStore::new();
        let model = meter_model();
        let a = TenantId::new();
        let b = TenantId::new();
        for (label, reading) in [("m0", 10i64), ("m1", 20), ("m2", 30)] {
            store.insert(&model, &meter(a, label, reading)).unwrap();
        }
        store.insert(&model, &meter(b, "other", 999)).unwrap();

        let count = store
            .aggregate(&model, Some(a), &[], AggregateOp::count())
            .unwrap();
        assert_eq!(count.as_u64(), Some(3));
        let sum = store
            .aggregate(&model, Some(a), &[], AggregateOp::sum("reading"))
            .unwrap();
        assert_eq!(sum.as_i64(), Some(60));
        let avg = store
            .aggregate(&model, Some(a), &[], AggregateOp::avg("reading"))
            .unwrap();
        assert_eq!(avg.as_f64(), Some(20.0));
        let min = store
            .aggregate(&model, Some(a), &[], AggregateOp::min("reading"))
            .unwrap();
        assert_eq!(min.as_i64(), Some(10));
        let max = store
            .aggregate(&model, Some(a), &[], AggregateOp::max("reading"))
            .unwrap();
        assert_eq!(max.as_i64(), Some(30));

        // Cross-tenant rows never leak into the aggregate.
        let sum_b = store
            .aggregate(&model, Some(b), &[], AggregateOp::sum("reading"))
            .unwrap();
        assert_eq!(sum_b.as_i64(), Some(999));

        // Filters narrow the match set.
        let filtered = store
            .aggregate(
                &model,
                Some(a),
                &[FieldFilter::eq("label", "m0")],
                AggregateOp::count(),
            )
            .unwrap();
        assert_eq!(filtered.as_u64(), Some(1));

        // Soft-deleted rows never count.
        let row = store
            .find_by(&model, Some(a), &[FieldFilter::eq("label", "m0")], false)
            .unwrap()
            .unwrap();
        store
            .delete(&model, uuid_str(&row, "id").unwrap(), Some(a))
            .unwrap();
        let count = store
            .aggregate(&model, Some(a), &[], AggregateOp::count())
            .unwrap();
        assert_eq!(count.as_u64(), Some(2));

        // Empty match sets: COUNT is 0, SUM/AVG/MIN/MAX are SQL NULL.
        let miss = [FieldFilter::eq("label", "nope")];
        let zero = store
            .aggregate(&model, Some(a), &miss, AggregateOp::count())
            .unwrap();
        assert_eq!(zero.as_u64(), Some(0));
        let null = store
            .aggregate(&model, Some(a), &miss, AggregateOp::sum("reading"))
            .unwrap();
        assert_eq!(null, Json::Null);

        // Field validation: non-numeric SUM and unknown fields are refused.
        let err = store
            .aggregate(&model, Some(a), &[], AggregateOp::sum("label"))
            .unwrap_err();
        assert!(err.public_message.contains("numeric"), "{}", err.public_message);
        let err = store
            .aggregate(&model, Some(a), &[], AggregateOp::max("missing"))
            .unwrap_err();
        assert!(err.public_message.contains("not on model"), "{}", err.public_message);
    }

    #[test]
    fn exists_and_find_by_first_match() {
        let store = InMemoryModelStore::new();
        let model = meter_model();
        let a = TenantId::new();
        let b = TenantId::new();
        store.insert(&model, &meter(a, "m0", 1)).unwrap();

        assert!(store.exists(&model, Some(a), &[]).unwrap());
        // Tenant isolation and filter misses.
        assert!(!store.exists(&model, Some(b), &[]).unwrap());
        assert!(
            !store
                .exists(&model, Some(a), &[FieldFilter::eq("label", "zz")])
                .unwrap()
        );

        let found = store
            .find_by(&model, Some(a), &[FieldFilter::eq("label", "m0")], false)
            .unwrap();
        assert_eq!(
            found.as_ref().and_then(|r| r.get("label")),
            Some(&Json::String("m0".into()))
        );
        assert!(store.find_by(&model, Some(b), &[], false).unwrap().is_none());

        // Soft-deleted rows: invisible live, visible with the trash toggle.
        let id = uuid_str(found.as_ref().unwrap(), "id").unwrap();
        store.delete(&model, id, Some(a)).unwrap();
        assert!(
            !store
                .exists(&model, Some(a), &[FieldFilter::eq("label", "m0")])
                .unwrap()
        );
        assert!(
            store
                .find_by(&model, Some(a), &[FieldFilter::eq("label", "m0")], true)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn find_for_update_follows_find_semantics() {
        let store = InMemoryModelStore::new();
        let model = meter_model();
        let a = TenantId::new();
        let row = store.insert(&model, &meter(a, "m0", 1)).unwrap();
        let id = uuid_str(&row, "id").unwrap();

        assert!(store
            .find_for_update(&model, id, Some(a), false)
            .unwrap()
            .is_some());
        assert!(store
            .find_for_update(&model, id, Some(TenantId::new()), false)
            .unwrap()
            .is_none());

        store.delete(&model, id, Some(a)).unwrap();
        assert!(store
            .find_for_update(&model, id, Some(a), false)
            .unwrap()
            .is_none());
        assert!(store
            .find_for_update(&model, id, Some(a), true)
            .unwrap()
            .is_some());
    }

    #[test]
    fn insert_or_ignore_skips_conflicts() {
        let store = InMemoryModelStore::new();
        let model = meter_model();
        let a = TenantId::new();
        let b = TenantId::new();

        let r0 = meter(a, "gate", 1);
        assert!(store.insert_or_ignore(&model, &r0).unwrap().is_some());
        // Same id → ignored, not an error.
        assert!(store.insert_or_ignore(&model, &r0).unwrap().is_none());

        // Same unique label within the tenant → ignored.
        let mut dupe = meter(a, "gate", 2);
        dupe.insert("id".into(), Json::String(Uuid::now_v7().to_string()));
        assert!(store.insert_or_ignore(&model, &dupe).unwrap().is_none());

        // The same label in ANOTHER tenant still inserts (tenant-scoped
        // uniqueness, mirroring the upsert conflict matching).
        assert!(store.insert_or_ignore(&model, &meter(b, "gate", 3))
            .unwrap()
            .is_some());

        let page = store.list(&model, Some(a), &[], &ListOpts::default()).unwrap();
        assert_eq!(page.total, Some(1));
        let page = store.list(&model, Some(b), &[], &ListOpts::default()).unwrap();
        assert_eq!(page.total, Some(1));

        // A record without an id is still refused up front.
        let mut no_id = meter(a, "x", 1);
        no_id.remove("id");
        let err = store.insert_or_ignore(&model, &no_id).unwrap_err();
        assert!(err.public_message.contains("uuid id"), "{}", err.public_message);
    }

    #[test]
    fn truncate_is_gated_and_clears_the_table() {
        let store = InMemoryModelStore::new();
        let model = meter_model();
        let a = TenantId::new();
        let b = TenantId::new();
        store.insert(&model, &meter(a, "m0", 1)).unwrap();
        store.insert(&model, &meter(b, "m1", 2)).unwrap();

        for env in ["prod", "staging", "", "DEV"] {
            let err = store.truncate(&model, env).unwrap_err();
            assert!(
                err.public_message.contains("dev/test"),
                "env {env:?} must be refused, got: {}",
                err.public_message
            );
        }
        // Nothing was cleared by the refused calls.
        assert_eq!(
            store.list(&model, Some(a), &[], &ListOpts::default()).unwrap().total,
            Some(1)
        );

        store.truncate(&model, "test").unwrap();
        assert_eq!(
            store.list(&model, Some(a), &[], &ListOpts::default()).unwrap().total,
            Some(0)
        );
        assert_eq!(
            store.list(&model, Some(b), &[], &ListOpts::default()).unwrap().total,
            Some(0)
        );
        assert!(store.truncate(&model, "dev").is_ok());
    }

    #[test]
    fn update_filtered_updates_matches_only() {
        let store = InMemoryModelStore::new();
        let model = meter_model();
        let a = TenantId::new();
        let b = TenantId::new();
        for label in ["m0", "m1", "m2"] {
            store.insert(&model, &meter(a, label, 1)).unwrap();
        }
        store.insert(&model, &meter(b, "m0", 1)).unwrap();

        let mut changes = Record::new();
        changes.insert("label".into(), Json::String("renamed".into()));
        let updated = store
            .update_filtered(&model, Some(a), &[FieldFilter::eq("label", "m0")], &changes)
            .unwrap();
        assert_eq!(updated.len(), 1);
        assert_eq!(
            updated[0].get("label"),
            Some(&Json::String("renamed".into()))
        );

        // Only the matching tenant-a row changed; other rows intact.
        let labels = |tenant| -> Vec<String> {
            store
                .list(&model, Some(tenant), &[], &ListOpts::default())
                .unwrap()
                .items
                .iter()
                .filter_map(|r| r.get("label").and_then(|v| v.as_str()).map(String::from))
                .collect()
        };
        let mut a_labels = labels(a);
        a_labels.sort();
        assert_eq!(a_labels, vec!["m1", "m2", "renamed"]);
        assert_eq!(labels(b), vec!["m0"]);

        // Unfiltered batch writes are refused.
        let err = store
            .update_filtered(&model, Some(a), &[], &changes)
            .unwrap_err();
        assert!(
            err.public_message.contains("at least one filter"),
            "{}",
            err.public_message
        );

        // Empty changes return the matches without writing.
        let unchanged = store
            .update_filtered(
                &model,
                Some(a),
                &[FieldFilter::eq("label", "m1")],
                &Record::new(),
            )
            .unwrap();
        assert_eq!(unchanged.len(), 1);
        assert_eq!(unchanged[0].get("label"), Some(&Json::String("m1".into())));
    }

    #[test]
    fn delete_filtered_marks_or_removes() {
        let store = InMemoryModelStore::new();
        let model = meter_model(); // soft-delete model
        let a = TenantId::new();
        let b = TenantId::new();
        for label in ["m0", "m1"] {
            store.insert(&model, &meter(a, label, 1)).unwrap();
        }
        store.insert(&model, &meter(b, "m0", 1)).unwrap();

        // Soft model: affected rows are trash-marked and returned.
        let affected = store
            .delete_filtered(&model, Some(a), &[FieldFilter::eq("label", "m0")])
            .unwrap();
        assert_eq!(affected.len(), 1);
        assert!(is_deleted(&affected[0]));
        assert_eq!(
            store.list(&model, Some(a), &[], &ListOpts::default()).unwrap().total,
            Some(1)
        );
        // Cross-tenant row untouched.
        assert_eq!(
            store.list(&model, Some(b), &[], &ListOpts::default()).unwrap().total,
            Some(1)
        );

        // Empty filters refused.
        let err = store
            .delete_filtered(&model, Some(a), &[])
            .unwrap_err();
        assert!(
            err.public_message.contains("at least one filter"),
            "{}",
            err.public_message
        );

        // Hard model: affected rows are removed outright.
        let log = hard_log_model();
        store.insert(&log, &log_row("keep")).unwrap();
        let doomed = store.insert(&log, &log_row("doomed")).unwrap();
        let affected = store
            .delete_filtered(&log, None, &[FieldFilter::eq("msg", "doomed")])
            .unwrap();
        assert_eq!(affected.len(), 1);
        assert_eq!(
            affected[0].get("msg"),
            Some(&Json::String("doomed".into()))
        );
        assert!(store
            .find(&log, uuid_str(&doomed, "id").unwrap(), None, true)
            .unwrap()
            .is_none());
        assert_eq!(
            store.list(&log, None, &[], &ListOpts::default()).unwrap().total,
            Some(1)
        );
    }

    #[test]
    fn force_delete_purges_trashed_and_live_rows() {
        let store = InMemoryModelStore::new();
        let model = meter_model();
        let a = TenantId::new();
        let other = TenantId::new();
        let row = store.insert(&model, &meter(a, "m0", 1)).unwrap();
        let id = uuid_str(&row, "id").unwrap();
        let foreign = store.insert(&model, &meter(other, "x", 1)).unwrap();
        let foreign_id = uuid_str(&foreign, "id").unwrap();

        // Soft-delete, then purge from the trash.
        store.delete(&model, id, Some(a)).unwrap();
        assert!(store.find(&model, id, Some(a), true).unwrap().is_some());
        assert!(store.force_delete(&model, id, Some(a)).unwrap());
        assert!(store.find(&model, id, Some(a), true).unwrap().is_none());
        // Second purge finds nothing.
        assert!(!store.force_delete(&model, id, Some(a)).unwrap());

        // A live row is purgeable the same way.
        let live = store.insert(&model, &meter(a, "live", 1)).unwrap();
        let live_id = uuid_str(&live, "id").unwrap();
        assert!(store.force_delete(&model, live_id, Some(a)).unwrap());
        assert!(store.find(&model, live_id, Some(a), true).unwrap().is_none());

        // Tenant isolation holds for force deletes too.
        assert!(!store.force_delete(&model, foreign_id, Some(a)).unwrap());
        assert!(store
            .find(&model, foreign_id, Some(other), true)
            .unwrap()
            .is_some());
    }

    #[test]
    fn arc_delegation_forwards_new_port_methods() {
        let store = InMemoryModelStore::new();
        let model = meter_model();
        let tenant = TenantId::new();
        store.insert(&model, &meter(tenant, "m0", 7)).unwrap();

        // &Arc<InMemoryModelStore> as &dyn ModelStore dispatches through the
        // Arc<T> delegation impl, not the concrete type.
        let dyn_store: &dyn ModelStore = &store;
        assert!(dyn_store.exists(&model, Some(tenant), &[]).unwrap());
        assert_eq!(
            dyn_store
                .aggregate(&model, Some(tenant), &[], AggregateOp::count())
                .unwrap()
                .as_u64(),
            Some(1)
        );
        assert!(dyn_store
            .find_by(&model, Some(tenant), &[], false)
            .unwrap()
            .is_some());
        dyn_store.truncate(&model, "dev").unwrap();
        assert_eq!(
            dyn_store.list(&model, Some(tenant), &[], &ListOpts::default()).unwrap().total,
            Some(0)
        );
    }

    #[test]
    fn increment_updates_counter_extra_and_guards() {
        let store = InMemoryModelStore::new();
        let model = meter_model();
        let a = TenantId::new();
        let b = TenantId::new();
        let row = store.insert(&model, &meter(a, "m0", 10)).unwrap();
        let id = uuid_str(&row, "id").unwrap();
        let foreign = store.insert(&model, &meter(b, "x", 1)).unwrap();
        let foreign_id = uuid_str(&foreign, "id").unwrap();

        let mut extra = Record::new();
        extra.insert("label".into(), Json::String("bumped".into()));
        let updated = store
            .increment(&model, id, Some(a), "reading", 5, &extra)
            .unwrap()
            .unwrap();
        assert_eq!(updated.get("reading"), Some(&Json::from(15)));
        assert_eq!(updated.get("label"), Some(&Json::String("bumped".into())));

        // Negative steps decrement in the same primitive.
        let updated = store
            .increment(&model, id, Some(a), "reading", -7, &Record::new())
            .unwrap()
            .unwrap();
        assert_eq!(updated.get("reading"), Some(&Json::from(8)));

        // Missing, foreign-tenant, and trashed rows are all Ok(None).
        assert!(store
            .increment(&model, Uuid::now_v7(), Some(a), "reading", 1, &Record::new())
            .unwrap()
            .is_none());
        assert!(store
            .increment(&model, foreign_id, Some(a), "reading", 1, &Record::new())
            .unwrap()
            .is_none());
        store.delete(&model, id, Some(a)).unwrap();
        assert!(store
            .increment(&model, id, Some(a), "reading", 1, &Record::new())
            .unwrap()
            .is_none());

        // Field validation: undeclared, non-numeric, immutable.
        let err = store
            .increment(&model, foreign_id, Some(b), "missing", 1, &Record::new())
            .unwrap_err();
        assert!(err.public_message.contains("not on model"), "{}", err.public_message);
        let err = store
            .increment(&model, foreign_id, Some(b), "label", 1, &Record::new())
            .unwrap_err();
        assert!(err.public_message.contains("not numeric"), "{}", err.public_message);
        let err = store
            .increment(&model, foreign_id, Some(b), "id", 1, &Record::new())
            .unwrap_err();
        assert!(err.public_message.contains("immutable"), "{}", err.public_message);
    }

    /// Correlated-existence fixture: `votes.widget_id -> widgets.id`.
    fn vote_model() -> ModelDef {
        ModelDef::new("Vote", "votes")
            .tenant_scoped()
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("tenant_id", FieldType::Uuid).required())
            .field(FieldDef::new("widget_id", FieldType::Uuid))
            .field(FieldDef::new("kind", FieldType::Text))
            .field(FieldDef::new("deleted_at", FieldType::Timestamp))
    }

    fn vote(tenant: TenantId, widget_id: &Json, kind: &str) -> Record {
        let mut r = Record::new();
        r.insert("id".into(), Json::String(Uuid::now_v7().to_string()));
        r.insert("tenant_id".into(), Json::String(tenant.as_uuid().to_string()));
        r.insert("widget_id".into(), widget_id.clone());
        r.insert("kind".into(), Json::String(kind.into()));
        r
    }

    fn votes_exists(negated: bool, or: bool, sub: Vec<FieldFilter>) -> FieldFilter {
        FieldFilter::Exists(ExistsFilter {
            negated,
            or,
            sub_table: "votes".into(),
            sub_key: "widget_id".into(),
            outer_table: "widgets".into(),
            outer_key: "id".into(),
            sub_tenant_scoped: true,
            sub_soft_deletes: true,
            filters: sub,
        })
    }

    fn names(page: kelson_query::Page<Record>) -> Vec<String> {
        page.items
            .iter()
            .map(|r| r["name"].as_str().unwrap_or("?").to_string())
            .collect()
    }

    #[test]
    fn list_applies_correlated_existence_filters() {
        let store = InMemoryModelStore::new();
        let widgets = widget_model();
        let votes = vote_model();
        let t = TenantId::new();
        let foreign = TenantId::new();

        let w1 = store.insert(&widgets, &widget(t, "w1")).unwrap();
        let w2 = store.insert(&widgets, &widget(t, "w2")).unwrap();
        let _w3 = store.insert(&widgets, &widget(t, "w3")).unwrap();
        // w1's only "up" vote sits in ANOTHER tenant; w2's is in `t`.
        store
            .insert(&votes, &vote(foreign, &w1["id"], "up"))
            .unwrap();
        let w2_up = store.insert(&votes, &vote(t, &w2["id"], "up")).unwrap();

        // EXISTS: only w2 qualifies (sub tenant scope enforced).
        let page = store
            .list(
                &widgets,
                Some(t),
                &[votes_exists(false, false, vec![])],
                &ListOpts::default(),
            )
            .unwrap();
        assert_eq!(names(page), vec!["w2"]);

        // NOT EXISTS: everything without an in-tenant vote.
        let page = store
            .list(
                &widgets,
                Some(t),
                &[votes_exists(true, false, vec![])],
                &ListOpts::default(),
            )
            .unwrap();
        assert_eq!(names(page), vec!["w1", "w3"]);

        // Sub conditions run against the related rows.
        let page = store
            .list(
                &widgets,
                Some(t),
                &[votes_exists(
                    false,
                    false,
                    vec![FieldFilter::eq("kind", "down")],
                )],
                &ListOpts::default(),
            )
            .unwrap();
        assert!(page.items.is_empty());

        // OR junction: `name = w1 OR EXISTS(...)`.
        let page = store
            .list(
                &widgets,
                Some(t),
                &[
                    FieldFilter::eq("name", "w1"),
                    votes_exists(false, true, vec![]),
                ],
                &ListOpts::default(),
            )
            .unwrap();
        assert_eq!(names(page), vec!["w1", "w2"]);

        // Soft-deleted related rows never satisfy EXISTS.
        store
            .delete(&votes, uuid_str(&w2_up, "id").unwrap(), Some(t))
            .unwrap();
        let page = store
            .list(
                &widgets,
                Some(t),
                &[votes_exists(false, false, vec![])],
                &ListOpts::default(),
            )
            .unwrap();
        assert!(page.items.is_empty());
    }

    #[test]
    fn write_filters_refuse_existence_terms() {
        let store = InMemoryModelStore::new();
        let model = widget_model();
        let exists = [votes_exists(false, false, vec![])];
        let mut changes = Record::new();
        changes.insert("name".into(), Json::String("x".into()));

        let err = store
            .update_filtered(&model, None, &exists, &changes)
            .unwrap_err();
        assert!(
            err.public_message.contains("relation-existence"),
            "{}",
            err.public_message
        );
        let err = store.delete_filtered(&model, None, &exists).unwrap_err();
        assert!(
            err.public_message.contains("relation-existence"),
            "{}",
            err.public_message
        );
    }

    #[test]
    fn nested_existence_filters_refused_on_both_sides_of_the_plan() {
        let store = InMemoryModelStore::new();
        let widgets = widget_model();
        let outer = FieldFilter::Exists(ExistsFilter {
            negated: false,
            or: false,
            sub_table: "votes".into(),
            sub_key: "widget_id".into(),
            outer_table: "widgets".into(),
            outer_key: "id".into(),
            sub_tenant_scoped: true,
            sub_soft_deletes: true,
            filters: vec![votes_exists(false, false, vec![])],
        });
        let err = store
            .list(&widgets, None, &[outer], &ListOpts::default())
            .unwrap_err();
        assert!(err.public_message.contains("nest"), "{}", err.public_message);
    }

    #[test]
    fn list_sorts_limits_and_validates_opts() {
        let store = InMemoryModelStore::new();
        let model = widget_model();
        let t = TenantId::new();
        store.insert(&model, &widget(t, "b")).unwrap();
        store.insert(&model, &widget(t, "a")).unwrap();
        store.insert(&model, &widget(t, "c")).unwrap();

        let page = store
            .list(
                &model,
                Some(t),
                &[],
                &ListOpts {
                    sort: Some(("name".into(), true)),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(names(page), vec!["c", "b", "a"]);

        // Limit caps rows AFTER sorting; total stays pre-limit.
        let page = store
            .list(
                &model,
                Some(t),
                &[],
                &ListOpts {
                    sort: Some(("name".into(), false)),
                    limit: Some(2),
                    ..Default::default()
                },
            )
            .unwrap();
        let total = page.total;
        assert_eq!(names(page), vec!["a", "b"]);
        assert_eq!(total, Some(3));

        // page XOR limit — pagination and eager row caps never mix.
        let err = store
            .list(
                &model,
                Some(t),
                &[],
                &ListOpts {
                    page: Some(PageReq {
                        page: 1,
                        per_page: 1,
                    }),
                    limit: Some(1),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(
            err.public_message.contains("mutually exclusive"),
            "{}",
            err.public_message
        );

        let err = store
            .list(
                &model,
                Some(t),
                &[],
                &ListOpts {
                    sort: Some(("bogus".into(), false)),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(
            err.public_message.contains("sort on unknown field"),
            "{}",
            err.public_message
        );
    }

    // ------------------------------------ Wave 3 Phase 6: range filters + restore

    #[test]
    fn range_filters_order_scalars_and_skip_missing_fields() {
        let store = InMemoryModelStore::new();
        let model = ModelDef::new("Metric", "metrics")
            .hard_deletes()
            .field(FieldDef::new("id", FieldType::Uuid).required())
            .field(FieldDef::new("name", FieldType::Text))
            .field(FieldDef::new("score", FieldType::Int64));
        for (name, score) in [("a", Some(1i64)), ("b", Some(5)), ("c", Some(10)), ("d", None)] {
            let mut r = Record::new();
            r.insert("id".into(), Json::String(Uuid::now_v7().to_string()));
            r.insert("name".into(), Json::String(name.into()));
            if let Some(score) = score {
                r.insert("score".into(), Json::from(score));
            }
            store.insert(&model, &r).unwrap();
        }
        let names = |filters: Vec<FieldFilter>| -> Vec<String> {
            let page = store
                .list(
                    &model,
                    None,
                    &filters,
                    &ListOpts {
                        sort: Some(("name".into(), false)),
                        ..Default::default()
                    },
                )
                .unwrap();
            page.items
                .iter()
                .filter_map(|r| r.get("name").and_then(Json::as_str).map(str::to_string))
                .collect()
        };
        // Numeric ordering; the score-less row "d" never matches a range.
        assert_eq!(names(vec![FieldFilter::lt("score", Json::from(5i64))]), ["a"]);
        assert_eq!(
            names(vec![FieldFilter::lte("score", Json::from(5i64))]),
            ["a", "b"]
        );
        assert_eq!(names(vec![FieldFilter::gt("score", Json::from(5i64))]), ["c"]);
        assert_eq!(
            names(vec![FieldFilter::gte("score", Json::from(5i64))]),
            ["b", "c"]
        );
        // Lexicographic ordering for strings.
        assert_eq!(
            names(vec![FieldFilter::lt("name", Json::String("c".into()))]),
            ["a", "b"]
        );
        assert_eq!(
            names(vec![FieldFilter::gte("name", Json::String("c".into()))]),
            ["c", "d"]
        );
    }

    #[test]
    fn restore_clears_the_trash_mark_and_hard_delete_models_stay_none() {
        let store = InMemoryModelStore::new();
        let model = widget_model();
        let tenant = TenantId::new();
        let row = store.insert(&model, &widget(tenant, "one")).unwrap();
        let id = uuid_str(&row, "id").unwrap();

        // Live row: nothing to restore.
        assert!(store.restore(&model, id, Some(tenant)).unwrap().is_none());

        store.delete(&model, id, Some(tenant)).unwrap();
        let trashed = store
            .find(&model, id, Some(tenant), true)
            .unwrap()
            .expect("trashed row still findable with the veil lifted");
        assert!(is_deleted(&trashed));

        let restored = store
            .restore(&model, id, Some(tenant))
            .unwrap()
            .expect("restore returns the row");
        assert!(!is_deleted(&restored));
        assert!(store
            .find(&model, id, Some(tenant), false)
            .unwrap()
            .is_some());

        // Hard-delete models have no trash: the port reports "nothing done".
        let hard = ModelDef::new("Log", "logs")
            .hard_deletes()
            .field(FieldDef::new("id", FieldType::Uuid).required())
            .field(FieldDef::new("msg", FieldType::Text));
        assert!(store.restore(&hard, Uuid::now_v7(), None).unwrap().is_none());
    }
}