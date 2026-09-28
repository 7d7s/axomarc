//! Model store port + in-memory implementation (S8 C3/B-04).
//!
//! The port is SEMANTIC (CRUD over validated records), not raw SQL: stores
//! receive records that already passed the [`crate::runtime::ModelRuntime`]
//! mass-assignment/type/tenant boundary. `InMemoryModelStore` backs TestKit
//! and degraded boot; `PgModelStore` (keel-postgres) backs production.
//!
//! Canonical record encoding: one JSON object per row. UUIDs are strings,
//! timestamps are RFC 3339 strings (UTC), numbers are JSON numbers, jsonb
//! columns are nested JSON. Both stores speak this same shape, so a module
//! can swap backends without touching domain code.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use keel_foundation::{AppError, TenantId};
use keel_query::PageReq;
use serde_json::Value as Json;
use uuid::Uuid;

use crate::ModelDef;

/// One persisted row, keyed by column name.
pub type Record = serde_json::Map<String, Json>;

/// Comparison operators for semantic filters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterOp {
    Eq,
    Ne,
    In,
    IsNull,
    NotNull,
}

/// One field-level filter (tenant-safe: stores apply tenant scope separately).
#[derive(Debug, Clone)]
pub struct FieldFilter {
    pub field: String,
    pub op: FilterOp,
    pub value: Json,
}

impl FieldFilter {
    pub fn eq(field: impl Into<String>, value: impl Into<Json>) -> Self {
        Self {
            field: field.into(),
            op: FilterOp::Eq,
            value: value.into(),
        }
    }

    pub fn is_in(field: impl Into<String>, values: Vec<Json>) -> Self {
        Self {
            field: field.into(),
            op: FilterOp::In,
            value: Json::Array(values),
        }
    }
}

/// List options: pagination (`None` = unbounded, for eager loads) and the
/// soft-delete trash toggle.
#[derive(Debug, Clone, Default)]
pub struct ListOpts {
    pub page: Option<PageReq>,
    pub include_deleted: bool,
}

/// Semantic CRUD port over a [`ModelDef`].
pub trait ModelStore: Send + Sync {
    fn list(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        opts: &ListOpts,
    ) -> Result<keel_query::Page<Record>, AppError>;

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
    ) -> Result<keel_query::Page<Record>, AppError> {
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
    if !model.tenant_scoped {
        return true;
    }
    let Some(tenant) = tenant else {
        // Tenant-scoped models queried without a tenant see nothing.
        return false;
    };
    uuid_str(record, "tenant_id") == Some(*tenant.as_uuid())
}

fn filter_matches(record: &Record, filter: &FieldFilter) -> bool {
    let actual = record.get(&filter.field);
    match filter.op {
        FilterOp::Eq => actual == Some(&filter.value),
        FilterOp::Ne => actual != Some(&filter.value),
        FilterOp::In => filter
            .value
            .as_array()
            .map(|list| actual.map(|a| list.contains(a)).unwrap_or(false))
            .unwrap_or(false),
        FilterOp::IsNull => matches!(actual, None | Some(Json::Null)),
        FilterOp::NotNull => !matches!(actual, None | Some(Json::Null)),
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
}

impl ModelStore for InMemoryModelStore {
    fn list(
        &self,
        model: &ModelDef,
        tenant: Option<TenantId>,
        filters: &[FieldFilter],
        opts: &ListOpts,
    ) -> Result<keel_query::Page<Record>, AppError> {
        let matched: Vec<Record> = self.with_table(&model.table, |rows| {
            Ok(rows
                .iter()
                .filter(|r| tenant_matches(model, r, tenant))
                .filter(|r| opts.include_deleted || !is_deleted(r))
                .filter(|r| filters.iter().all(|f| filter_matches(r, f)))
                .map(Self::clone_row)
                .collect())
        })?;
        let total = matched.len() as u64;
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
        Ok(keel_query::Page::new(items, page, per_page, Some(total)))
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
            let Some(row) = rows.iter_mut().find(|r| {
                uuid_str(r, "id") == Some(id)
                    && tenant_matches(model, r, tenant)
                    && !is_deleted(r)
            }) else {
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
            let updated = self.update(model, id, tenant, &{
                let mut changes = Record::new();
                changes.insert("deleted_at".into(), Json::String(now_rfc3339()));
                changes
            })?;
            return Ok(updated.is_some());
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
                    row.insert(key.clone(), value.clone());
                }
                return Ok(Self::clone_row(row));
            }
            rows.push(Self::clone_row(record));
            Ok(Self::clone_row(record))
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
        assert_eq!(err.category, keel_foundation::ErrorCategory::Conflict);
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
}