//! Model runtime (S8 C4/B-05/B-10/B-11): the write/read boundary between
//! untyped JSON input and the [`crate::store::ModelStore`] port.
//!
//! Responsibilities:
//! - mass-assignment boundary (declared fields only, unknown keys rejected)
//! - field-level casts between JSON wire values and [`keel_query::Value`]
//! - required/enum/tenant validation on write
//! - hidden (`sensitive`) field redaction on serialization
//! - soft-delete runtime (honors `ModelDef::soft_deletes`)
//! - named query scopes, accessors and mutators
//! - model events (creating/created/updating/updated/deleting/deleted)
//! - relation loading from [`crate::RelationDef`] (max 2 queries/relation)
//!
//! The runtime never executes SQL itself — it prepares records and filters,
//! delegates persistence to the store port, and fires observers.

use std::collections::BTreeMap;
use std::sync::Arc;

use keel_foundation::{AppError, TenantId};
use keel_query::{Ident, PageReq, Value as Bind};
use serde_json::Value as Json;
use uuid::Uuid;

use crate::store::{
    now_rfc3339, FieldFilter, ListOpts, ModelStore, Record,
};
use crate::{FieldDef, FieldType, ModelDef, ModelRegistry, RelationKind};

// ------------------------------------------------------------------ events

/// Model lifecycle phase for observer hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelPhase {
    Creating,
    Created,
    Updating,
    Updated,
    Deleting,
    Deleted,
}

/// One observed model lifecycle event. Records are post-cast, pre/post store.
#[derive(Debug, Clone)]
pub struct ModelEvent {
    pub model: String,
    pub phase: ModelPhase,
    pub record: Record,
}

/// Observer hook (bridge to keel-event/outbox lives in the engine layer).
pub trait ModelObserver: Send + Sync {
    fn on_model_event(&self, event: &ModelEvent);
}

// ----------------------------------------------------------------- helpers

/// Cast a wire JSON value into a typed bind value per the field definition.
pub fn json_to_bind(value: &Json, field: &FieldDef) -> Result<Bind, AppError> {
    match &field.field_type {
        FieldType::Uuid => value
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
            .map(Bind::Uuid)
            .ok_or_else(|| {
                AppError::validation(format!("field {} expects a uuid string", field.name))
            }),
        FieldType::Bool => value.as_bool().map(Bind::Bool).ok_or_else(|| {
            AppError::validation(format!("field {} expects a boolean", field.name))
        }),
        FieldType::Int32 | FieldType::Int64 => value.as_i64().map(Bind::Int).ok_or_else(|| {
            AppError::validation(format!("field {} expects an integer", field.name))
        }),
        FieldType::Float => value
            .as_f64()
            .map(Bind::Float)
            .ok_or_else(|| AppError::validation(format!("field {} expects a number", field.name))),
        FieldType::Decimal => value.as_f64().map(Bind::Float).ok_or_else(|| {
            AppError::validation(format!("field {} expects a decimal number", field.name))
        }),
        FieldType::Text | FieldType::TextLarge | FieldType::Date => value
            .as_str()
            .map(|s| Bind::Text(s.to_string()))
            .ok_or_else(|| AppError::validation(format!("field {} expects a string", field.name))),
        FieldType::Timestamp => {
            let s = value.as_str().ok_or_else(|| {
                AppError::validation(format!(
                    "field {} expects an RFC 3339 timestamp string",
                    field.name
                ))
            })?;
            parse_timestamp(s).map(Bind::Timestamp)
        }
        FieldType::Json => Ok(Bind::Json(value.clone())),
        FieldType::Enum { variants } => {
            let s = value.as_str().ok_or_else(|| {
                AppError::validation(format!("field {} expects an enum label", field.name))
            })?;
            if !variants.iter().any(|v| v == s) {
                return Err(AppError::validation(format!(
                    "field {} value {s:?} is not a declared variant",
                    field.name
                )));
            }
            Ok(Bind::Text(s.to_string()))
        }
    }
}

/// Parse an RFC 3339 timestamp (canonical record encoding).
pub fn parse_timestamp(raw: &str) -> Result<time::OffsetDateTime, AppError> {
    time::OffsetDateTime::parse(raw, &time::format_description::well_known::Rfc3339)
        .map_err(|_| AppError::validation(format!("invalid RFC 3339 timestamp {raw:?}")))
}
// ----------------------------------------------------------------- runtime

type ScopeFn = Arc<dyn Fn(Vec<FieldFilter>) -> Vec<FieldFilter> + Send + Sync>;
type AccessorFn = Arc<dyn Fn(&Record) -> Json + Send + Sync>;
type MutatorFn = Arc<dyn Fn(Json) -> Result<Json, AppError> + Send + Sync>;

/// Runtime semantics for one [`ModelDef`]. Cheap to clone (Arc internals).
#[derive(Clone)]
pub struct ModelRuntime {
    model: Arc<ModelDef>,
    registry: Option<Arc<ModelRegistry>>,
    observers: Vec<Arc<dyn ModelObserver>>,
    scopes: BTreeMap<String, ScopeFn>,
    accessors: BTreeMap<String, AccessorFn>,
    mutators: BTreeMap<String, MutatorFn>,
}

impl std::fmt::Debug for ModelRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelRuntime")
            .field("model", &self.model.name)
            .finish()
    }
}

impl ModelRuntime {
    pub fn new(model: ModelDef) -> Self {
        Self {
            model: Arc::new(model),
            registry: None,
            observers: Vec::new(),
            scopes: BTreeMap::new(),
            accessors: BTreeMap::new(),
            mutators: BTreeMap::new(),
        }
    }

    /// Attach the registry (required for relation loading).
    pub fn with_registry(mut self, registry: Arc<ModelRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    pub fn with_observer(mut self, observer: Arc<dyn ModelObserver>) -> Self {
        self.observers.push(observer);
        self
    }

    /// Named query scope: a pure filter transform applied by name in `list`.
    pub fn with_scope(
        mut self,
        name: impl Into<String>,
        f: impl Fn(Vec<FieldFilter>) -> Vec<FieldFilter> + Send + Sync + 'static,
    ) -> Self {
        self.scopes.insert(name.into(), Arc::new(f));
        self
    }

    /// Accessor: computed value appended by [`ModelRuntime::public_record`].
    pub fn with_accessor(
        mut self,
        name: impl Into<String>,
        f: impl Fn(&Record) -> Json + Send + Sync + 'static,
    ) -> Self {
        self.accessors.insert(name.into(), Arc::new(f));
        self
    }

    /// Mutator: wire-value transform applied before write validation.
    pub fn with_mutator(
        mut self,
        name: impl Into<String>,
        f: impl Fn(Json) -> Result<Json, AppError> + Send + Sync + 'static,
    ) -> Self {
        self.mutators.insert(name.into(), Arc::new(f));
        self
    }

    pub fn model(&self) -> &ModelDef {
        &self.model
    }

    fn fire(&self, phase: ModelPhase, record: &Record) {
        if self.observers.is_empty() {
            return;
        }
        let event = ModelEvent {
            model: self.model.name.clone(),
            phase,
            record: record.clone(),
        };
        for observer in &self.observers {
            observer.on_model_event(&event);
        }
    }

    // ------------------------------------------------------------- prepare

    /// Mass-assignment + validation boundary for INSERT. Returns a complete
    /// record (id/tenant/timestamps injected per model shape).
    pub fn prepare_insert(
        &self,
        input: &Record,
        tenant: Option<TenantId>,
    ) -> Result<Record, AppError> {
        let mut record = Record::new();

        // 1. Mutators transform supplied wire values.
        let mut supplied: Record = input.clone();
        for (key, mutator) in &self.mutators {
            if let Some(value) = supplied.remove(key) {
                supplied.insert(key.clone(), mutator(value)?);
            }
        }

        // 2. Mass-assignment boundary: declared fields only.
        for key in supplied.keys() {
            if self.model.field_named(key).is_none() {
                return Err(AppError::validation(format!(
                    "unknown field {key:?} for model {}",
                    self.model.name
                )));
            }
        }

        // 3. Tenant injection for tenant-scoped models.
        if self.model.tenant_scoped {
            let tenant = tenant.ok_or_else(|| {
                AppError::validation(format!(
                    "model {} is tenant-scoped; a tenant is required",
                    self.model.name
                ))
            })?;
            record.insert(
                "tenant_id".into(),
                Json::String(tenant.as_uuid().to_string()),
            );
        }

        // 4. Identity: generate UUIDv7 when the model declares a uuid `id`.
        if let Some(id_field) = self.model.field_named("id") {
            if id_field.field_type == FieldType::Uuid {
                let id = match supplied.get("id") {
                    Some(Json::String(s)) => Uuid::parse_str(s).map_err(|_| {
                        AppError::validation("id must be a valid uuid string")
                    })?,
                    Some(_) => return Err(AppError::validation("id must be a uuid string")),
                    None => Uuid::now_v7(),
                };
                record.insert("id".into(), Json::String(id.to_string()));
            }
        }

        // 5. Copy declared fields through the cast boundary.
        for field in &self.model.fields {
            if field.name == "id" || field.name == "tenant_id" {
                continue;
            }
            if let Some(value) = supplied.get(&field.name) {
                json_to_bind(value, field)?;
                record.insert(field.name.clone(), value.clone());
            }
        }

        // 6. Required fields must be present (unless a DDL default exists).
        for field in &self.model.fields {
            if field.required && !record.contains_key(&field.name) && field.default_sql.is_none() {
                return Err(AppError::validation(format!(
                    "field {} is required for model {}",
                    field.name, self.model.name
                )));
            }
        }

        // 7. Framework timestamps (only when declared on the model).
        let now = now_rfc3339();
        if self.model.field_named("created_at").is_some() {
            record.insert("created_at".into(), Json::String(now.clone()));
        }
        if self.model.field_named("updated_at").is_some() {
            record.insert("updated_at".into(), Json::String(now));
        }

        Ok(record)
    }

    /// Mass-assignment + validation boundary for UPDATE (no required check;
    /// partial changes allowed). Refreshes `updated_at` when declared.
    pub fn prepare_update(&self, changes: &Record) -> Result<Record, AppError> {
        let mut record = Record::new();
        let mut supplied: Record = changes.clone();
        for (key, mutator) in &self.mutators {
            if let Some(value) = supplied.remove(key) {
                supplied.insert(key.clone(), mutator(value)?);
            }
        }
        for (key, value) in &supplied {
            if key == "id" || key == "tenant_id" {
                return Err(AppError::validation(format!("field {key:?} is immutable")));
            }
            let field = self.model.field_named(key).ok_or_else(|| {
                AppError::validation(format!(
                    "unknown field {key:?} for model {}",
                    self.model.name
                ))
            })?;
            json_to_bind(value, field)?;
            record.insert(key.clone(), value.clone());
        }
        if self.model.field_named("updated_at").is_some() && !record.contains_key("updated_at") {
            record.insert("updated_at".into(), Json::String(now_rfc3339()));
        }
        Ok(record)
    }

    /// Hidden-field policy + accessor projection for wire serialization.
    pub fn public_record(&self, record: &Record) -> Record {
        let mut out = Record::new();
        for (key, value) in record {
            let sensitive = self
                .model
                .field_named(key)
                .map(|f| f.sensitive)
                .unwrap_or(false);
            if !sensitive {
                out.insert(key.clone(), value.clone());
            }
        }
        for (name, accessor) in &self.accessors {
            out.insert(name.clone(), accessor(&out));
        }
        out
    }
    // ---------------------------------------------------------------- CRUD

    pub fn insert(
        &self,
        store: &dyn ModelStore,
        input: &Record,
        tenant: Option<TenantId>,
    ) -> Result<Record, AppError> {
        let record = self.prepare_insert(input, tenant)?;
        self.fire(ModelPhase::Creating, &record);
        let stored = store.insert(&self.model, &record)?;
        self.fire(ModelPhase::Created, &stored);
        Ok(stored)
    }

    pub fn find(
        &self,
        store: &dyn ModelStore,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<Option<Record>, AppError> {
        store.find(&self.model, id, tenant, false)
    }

    /// Like `find`, but NOT_FOUND errors instead of `None`.
    pub fn get(
        &self,
        store: &dyn ModelStore,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<Record, AppError> {
        self.find(store, id, tenant)?
            .ok_or_else(|| AppError::not_found(format!("{} {id}", self.model.name)))
    }

    pub fn list(
        &self,
        store: &dyn ModelStore,
        tenant: Option<TenantId>,
        mut filters: Vec<FieldFilter>,
        scope_names: &[String],
        page: Option<PageReq>,
    ) -> Result<keel_query::Page<Record>, AppError> {
        for name in scope_names {
            let scope = self
                .scopes
                .get(name)
                .ok_or_else(|| AppError::validation(format!("unknown scope {name:?}")))?;
            filters = scope(filters);
        }
        for filter in &filters {
            if filter.field != "id" && self.model.field_named(&filter.field).is_none() {
                return Err(AppError::validation(format!(
                    "filter on unknown field {:?}",
                    filter.field
                )));
            }
        }
        let opts = ListOpts {
            page,
            include_deleted: false,
        };
        store.list(&self.model, tenant, &filters, &opts)
    }

    pub fn update(
        &self,
        store: &dyn ModelStore,
        id: Uuid,
        tenant: Option<TenantId>,
        changes: &Record,
    ) -> Result<Record, AppError> {
        // Fetch first: validates existence and gives observers the merge.
        let current = self.get(store, id, tenant)?;
        let prepared = self.prepare_update(changes)?;
        let mut next = current;
        for (key, value) in &prepared {
            next.insert(key.clone(), value.clone());
        }
        self.fire(ModelPhase::Updating, &next);
        let stored = store
            .update(&self.model, id, tenant, &prepared)?
            .ok_or_else(|| AppError::not_found(format!("{} {id}", self.model.name)))?;
        self.fire(ModelPhase::Updated, &stored);
        Ok(stored)
    }

    /// Soft- or hard-delete per model posture. Returns the removed id.
    pub fn delete(
        &self,
        store: &dyn ModelStore,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<Uuid, AppError> {
        let current = self.get(store, id, tenant)?;
        self.fire(ModelPhase::Deleting, &current);
        store.delete(&self.model, id, tenant)?;
        self.fire(ModelPhase::Deleted, &current);
        Ok(id)
    }

    pub fn upsert(
        &self,
        store: &dyn ModelStore,
        input: &Record,
        conflict: &[String],
        tenant: Option<TenantId>,
    ) -> Result<Record, AppError> {
        for field_name in conflict {
            if self.model.field_named(field_name).is_none() && field_name != "tenant_id" {
                return Err(AppError::validation(format!(
                    "conflict field {field_name:?} is not declared on {}",
                    self.model.name
                )));
            }
        }
        let record = self.prepare_insert(input, tenant)?;
        self.fire(ModelPhase::Creating, &record);
        let stored = store.upsert(&self.model, &record, conflict)?;
        self.fire(ModelPhase::Created, &stored);
        Ok(stored)
    }

    // ------------------------------------------------------------ relations

    /// Eager-load one declared relation onto `parents`, attaching results
    /// under the relation name (`Record` for belongs_to/has_one, `Vec` for
    /// has_many/many_through). At most 2 store queries per relation.
    pub fn load_relation(
        &self,
        store: &dyn ModelStore,
        relation_name: &str,
        parents: &mut [Record],
        tenant: Option<TenantId>,
    ) -> Result<(), AppError> {
        let registry = self.registry.clone().ok_or_else(|| {
            AppError::validation("relation loading needs a model registry")
        })?;
        let relation = self
            .model
            .relations
            .iter()
            .find(|r| r.name == relation_name)
            .ok_or_else(|| {
                AppError::validation(format!(
                    "model {} has no relation {relation_name:?}",
                    self.model.name
                ))
            })?
            .clone();
        let target = registry.get(&relation.target).cloned().ok_or_else(|| {
            AppError::validation(format!(
                "relation {} targets unknown model {}",
                relation.name, relation.target
            ))
        })?;
        let target_rt = ModelRuntime::new(target.clone()).with_registry(registry.clone());

        match relation.kind {
            RelationKind::BelongsTo => {
                let target_key = relation.target_key.clone().unwrap_or_else(|| "id".into());
                let ids = distinct_values(parents, &relation.foreign_key);
                let loaded = fetch_keyed(store, &target_rt, &target_key, ids, tenant)?;
                for parent in parents.iter_mut() {
                    let related = parent
                        .get(&relation.foreign_key)
                        .and_then(|k| loaded.get(&k.to_string()))
                        .cloned()
                        .unwrap_or(Json::Null);
                    parent.insert(relation.name.clone(), related);
                }
            }
            RelationKind::HasMany | RelationKind::HasOne => {
                let parent_ids = distinct_values(parents, "id");
                let by_fk = fetch_keyed(store, &target_rt, &relation.foreign_key, parent_ids, tenant)?;
                let mut grouped: BTreeMap<String, Vec<Json>> = BTreeMap::new();
                for (fk, row) in by_fk {
                    grouped.entry(fk).or_default().push(row);
                }
                let single = relation.kind == RelationKind::HasOne;
                for parent in parents.iter_mut() {
                    let Some(pid) = parent.get("id").cloned() else {
                        continue;
                    };
                    let children = grouped.remove(&pid.to_string()).unwrap_or_default();
                    let attached = if single {
                        children.first().cloned().unwrap_or(Json::Null)
                    } else {
                        Json::Array(children)
                    };
                    parent.insert(relation.name.clone(), attached);
                }
            }
            RelationKind::ManyThrough => {
                // Query 1: join rows for all parent ids; Query 2: target rows
                // for the far-side ids found on the join table.
                let through_name = relation.through.clone().ok_or_else(|| {
                    AppError::validation("ManyThrough relation needs a join model")
                })?;
                let through = registry.get(&through_name).cloned().ok_or_else(|| {
                    AppError::validation(format!("unknown join model {through_name:?}"))
                })?;
                // FK on the join table pointing at the target model.
                let far_fk = relation
                    .target_key
                    .clone()
                    .unwrap_or_else(|| format!("{}_id", target.table.to_lowercase().trim_end_matches('s')));
                let parent_ids = distinct_values(parents, "id");
                let join_rows = store
                    .list(
                        &through,
                        tenant,
                        &[FieldFilter::is_in(&relation.foreign_key, parent_ids)],
                        &ListOpts::default(),
                    )?
                    .items;
                let far_ids = distinct_values(&join_rows, &far_fk);
                let loaded = fetch_keyed(store, &target_rt, "id", far_ids, tenant)?;
                for parent in parents.iter_mut() {
                    let Some(pid) = parent.get("id").cloned() else {
                        continue;
                    };
                    let attached: Vec<Json> = join_rows
                        .iter()
                        .filter(|r| r.get(&relation.foreign_key) == Some(&pid))
                        .filter_map(|r| r.get(&far_fk).and_then(|k| loaded.get(&k.to_string())).cloned())
                        .collect();
                    parent.insert(relation.name.clone(), Json::Array(attached));
                }
            }
        }
        Ok(())
    }
}

/// Distinct non-null JSON values for `key` across records (dedupe by the
/// value's canonical JSON string — `serde_json::Value` is not `Ord`).
fn distinct_values(records: &[Record], key: &str) -> Vec<Json> {
    let mut seen: BTreeMap<String, Json> = BTreeMap::new();
    for record in records {
        if let Some(value) = record.get(key).cloned() {
            if !value.is_null() {
                seen.entry(value.to_string()).or_insert(value);
            }
        }
    }
    seen.into_values().collect()
}

/// One unbounded IN query against `target`, keyed by the stringified
/// `column` value.
fn fetch_keyed(
    store: &dyn ModelStore,
    target: &ModelRuntime,
    column: &str,
    ids: Vec<Json>,
    tenant: Option<TenantId>,
) -> Result<BTreeMap<String, Json>, AppError> {
    let mut out = BTreeMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let page = target.list(store, tenant, vec![FieldFilter::is_in(column, ids)], &[], None)?;
    for row in page.items {
        if let Some(key) = row.get(column).cloned() {
            out.insert(key.to_string(), Json::Object(row));
        }
    }
    Ok(out)
}

/// Compile a runtime record into an ordered `(columns, binds)` pair for the
/// write builders (used by `PgModelStore`). Columns are [`Ident`]-validated;
/// invalid column names are rejected here, never in the SQL text.
pub fn record_to_columns(record: &Record) -> Result<(Vec<Ident>, Vec<Bind>), AppError> {
    let mut columns = Vec::with_capacity(record.len());
    let mut binds = Vec::with_capacity(record.len());
    for (key, value) in record {
        columns.push(Ident::new(key.clone())?);
        binds.push(json_value_to_bind(value)?);
    }
    Ok((columns, binds))
}

/// Untyped JSON -> bind conversion for store rows (schema-free columns like
/// framework timestamps). Uuid/timestamp strings are detected so they bind
/// as native types; everything else stays text/json.
pub fn json_value_to_bind(value: &Json) -> Result<Bind, AppError> {
    Ok(match value {
        Json::Null => Bind::Null,
        Json::Bool(b) => Bind::Bool(*b),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                Bind::Int(i)
            } else {
                Bind::Float(n.as_f64().unwrap_or_default())
            }
        }
        Json::String(s) => {
            if let Ok(u) = Uuid::parse_str(s) {
                Bind::Uuid(u)
            } else if s.len() >= 20 && s.contains('T') {
                if let Ok(ts) = parse_timestamp(s) {
                    Bind::Timestamp(ts)
                } else {
                    Bind::Text(s.clone())
                }
            } else {
                Bind::Text(s.clone())
            }
        }
        other => Bind::Json(other.clone()),
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::InMemoryModelStore;
    use crate::{FieldDef, RelationDef};

    fn appointment_model() -> ModelDef {
        ModelDef::new("Appointment", "appointments")
            .tenant_scoped()
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("tenant_id", FieldType::Uuid).required())
            .field(FieldDef::new("patient", FieldType::Text).required())
            .field(
                FieldDef::new(
                    "status",
                    FieldType::Enum {
                        variants: vec!["booked".into(), "cancelled".into()],
                    },
                )
                .required(),
            )
            .field(FieldDef::new("notes", FieldType::Text).sensitive())
            .field(FieldDef::new("created_at", FieldType::Timestamp))
            .field(FieldDef::new("updated_at", FieldType::Timestamp))
            .field(FieldDef::new("deleted_at", FieldType::Timestamp))
    }

    fn rt() -> ModelRuntime {
        ModelRuntime::new(appointment_model())
    }

    fn input(patient: &str) -> Record {
        let mut r = Record::new();
        r.insert("patient".into(), Json::String(patient.into()));
        r.insert("status".into(), Json::String("booked".into()));
        r
    }

    fn rid(record: &Record) -> Uuid {
        Uuid::parse_str(record["id"].as_str().unwrap()).unwrap()
    }

    #[test]
    fn insert_requires_tenant_and_generates_id() {
        assert!(rt().prepare_insert(&input("p"), None).is_err());
        let rec = rt().prepare_insert(&input("p"), Some(TenantId::new())).unwrap();
        assert!(Uuid::parse_str(rec["id"].as_str().unwrap()).is_ok());
        assert!(rec.contains_key("tenant_id"));
        assert!(rec.contains_key("created_at"));
    }

    #[test]
    fn unknown_field_rejected() {
        let mut bad = input("p");
        bad.insert("hacker".into(), Json::Bool(true));
        assert!(rt().prepare_insert(&bad, Some(TenantId::new())).is_err());
    }

    #[test]
    fn enum_variant_validated() {
        let mut bad = input("p");
        bad.insert("status".into(), Json::String("nope".into()));
        assert!(rt().prepare_insert(&bad, Some(TenantId::new())).is_err());
    }

    #[test]
    fn sensitive_fields_redacted_in_public_record() {
        let store = InMemoryModelStore::new();
        let mut i = input("p");
        i.insert("notes".into(), Json::String("phi".into()));
        let stored = rt().insert(&store, &i, Some(TenantId::new())).unwrap();
        let public = rt().public_record(&stored);
        assert!(!public.contains_key("notes"));
        assert!(public.contains_key("patient"));
    }

    #[test]
    fn scopes_accessors_and_listing() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let rt = ModelRuntime::new(appointment_model())
            .with_scope("booked", |mut f| {
                f.push(FieldFilter::eq("status", "booked"));
                f
            })
            .with_accessor("summary", |r| {
                Json::String(format!(
                    "{} ({})",
                    r.get("patient").and_then(|v| v.as_str()).unwrap_or("?"),
                    r.get("status").and_then(|v| v.as_str()).unwrap_or("?")
                ))
            });
        let a = rt.insert(&store, &input("one"), Some(tenant)).unwrap();
        let mut changes = Record::new();
        changes.insert("status".into(), Json::String("cancelled".into()));
        rt.update(&store, rid(&a), Some(tenant), &changes).unwrap();
        rt.insert(&store, &input("two"), Some(tenant)).unwrap();

        let booked = rt
            .list(&store, Some(tenant), vec![], &["booked".into()], None)
            .unwrap();
        assert_eq!(booked.total, Some(1));
        let all = rt.list(&store, Some(tenant), vec![], &[], None).unwrap();
        assert_eq!(all.total, Some(2));
        assert!(rt.public_record(&all.items[0]).contains_key("summary"));
    }

    #[test]
    fn soft_delete_hides_rows_until_trashed() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let row = rt().insert(&store, &input("gone"), Some(tenant)).unwrap();
        rt().delete(&store, rid(&row), Some(tenant)).unwrap();
        assert!(rt().find(&store, rid(&row), Some(tenant)).unwrap().is_none());
        assert!(store
            .find(&appointment_model(), rid(&row), Some(tenant), true)
            .unwrap()
            .is_some());
    }

    #[test]
    fn mutator_transforms_wire_values() {
        let rt = ModelRuntime::new(appointment_model()).with_mutator("patient", |v| {
            Ok(Json::String(v.as_str().unwrap_or("").trim().to_uppercase()))
        });
        let rec = rt.prepare_insert(&input("  jane "), Some(TenantId::new())).unwrap();
        assert_eq!(rec["patient"], Json::String("JANE".into()));
    }

    #[test]
    fn update_rejects_immutable_and_unknown_fields() {
        let mut bad = Record::new();
        bad.insert("tenant_id".into(), Json::String(Uuid::now_v7().to_string()));
        assert!(rt().prepare_update(&bad).is_err());
        let mut unknown = Record::new();
        unknown.insert("what".into(), Json::Null);
        assert!(rt().prepare_update(&unknown).is_err());
    }

    #[test]
    fn model_events_fire_in_order() {
        #[derive(Default)]
        struct Recorder(std::sync::Mutex<Vec<ModelPhase>>);
        impl ModelObserver for Recorder {
            fn on_model_event(&self, event: &ModelEvent) {
                self.0.lock().unwrap().push(event.phase);
            }
        }
        let recorder = Arc::new(Recorder::default());
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let rt = ModelRuntime::new(appointment_model()).with_observer(recorder.clone());
        let row = rt.insert(&store, &input("evt"), Some(tenant)).unwrap();
        let mut changes = Record::new();
        changes.insert("status".into(), Json::String("cancelled".into()));
        rt.update(&store, rid(&row), Some(tenant), &changes).unwrap();
        rt.delete(&store, rid(&row), Some(tenant)).unwrap();
        assert_eq!(
            *recorder.0.lock().unwrap(),
            vec![
                ModelPhase::Creating,
                ModelPhase::Created,
                ModelPhase::Updating,
                ModelPhase::Updated,
                ModelPhase::Deleting,
                ModelPhase::Deleted
            ]
        );
    }

    #[test]
    fn record_to_columns_binds_shapes() {
        let mut record = Record::new();
        record.insert("id".into(), Json::String(Uuid::now_v7().to_string()));
        record.insert("n".into(), Json::from(3));
        record.insert("x".into(), Json::from(2.5));
        record.insert("t".into(), Json::String(now_rfc3339()));
        record.insert("b".into(), Json::Bool(true));
        let (cols, binds) = record_to_columns(&record).unwrap();
        assert_eq!(cols.len(), 5);
        let names: Vec<&str> = cols.iter().map(|c| c.as_str()).collect();
        for expected in ["id", "n", "x", "t", "b"] {
            assert!(names.contains(&expected));
        }
        assert!(binds.iter().any(|b| matches!(b, Bind::Uuid(_))));
        assert!(binds.iter().any(|b| matches!(b, Bind::Timestamp(_))));
    }

    #[test]
    fn has_many_eager_load() {
        let mut registry = ModelRegistry::new();
        let patient = ModelDef::new("Patient", "patients")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("name", FieldType::Text).required())
            .relation(RelationDef::has_many("appointments", "Appointment", "patient_id"));
        let appointment = ModelDef::new("Appointment", "appointments")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("patient_id", FieldType::Uuid))
            .field(FieldDef::new("slot", FieldType::Text).required());
        registry.register(patient.clone()).unwrap();
        registry.register(appointment.clone()).unwrap();
        registry.validate_graph().unwrap();
        let registry = Arc::new(registry);
        let store = InMemoryModelStore::new();

        let patient_rt = ModelRuntime::new(patient).with_registry(registry.clone());
        let appointment_rt = ModelRuntime::new(appointment).with_registry(registry);

        let mut name = Record::new();
        name.insert("name".into(), Json::String("P1".into()));
        let p = patient_rt.insert(&store, &name, None).unwrap();
        let mut a = Record::new();
        a.insert("patient_id".into(), p["id"].clone());
        a.insert("slot".into(), Json::String("09:00".into()));
        appointment_rt.insert(&store, &a, None).unwrap();

        let mut parents = vec![p];
        patient_rt
            .load_relation(&store, "appointments", &mut parents, None)
            .unwrap();
        let attached = parents[0]["appointments"].as_array().unwrap();
        assert_eq!(attached.len(), 1);
        assert_eq!(attached[0]["slot"], Json::String("09:00".into()));
    }

    #[test]
    fn belongs_to_eager_load() {
        let mut registry = ModelRegistry::new();
        let patient = ModelDef::new("Patient", "patients")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("name", FieldType::Text).required());
        let appointment = ModelDef::new("Appointment", "appointments")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("patient_id", FieldType::Uuid))
            .relation(RelationDef::belongs_to("patient", "Patient", "patient_id"));
        registry.register(patient.clone()).unwrap();
        registry.register(appointment.clone()).unwrap();
        registry.validate_graph().unwrap();
        let registry = Arc::new(registry);
        let store = InMemoryModelStore::new();

        let patient_rt = ModelRuntime::new(patient).with_registry(registry.clone());
        let appointment_rt = ModelRuntime::new(appointment).with_registry(registry);

        let mut name = Record::new();
        name.insert("name".into(), Json::String("P1".into()));
        let p = patient_rt.insert(&store, &name, None).unwrap();
        let mut a = Record::new();
        a.insert("patient_id".into(), p["id"].clone());
        let appt = appointment_rt.insert(&store, &a, None).unwrap();

        let mut children = vec![appt];
        appointment_rt
            .load_relation(&store, "patient", &mut children, None)
            .unwrap();
        assert_eq!(children[0]["patient"]["name"], Json::String("P1".into()));
    }
}