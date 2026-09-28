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
//! - Eloquent finder/mutation surface: `first_or_create`/`first_or_new`/
//!   `update_or_create`/`force_create`/`create_many`, atomic counters,
//!   `replicate`/`refresh`/`fresh`/`touch`/`push`, and the `pluck`/`value`/
//!   `chunk`/`each`/`cursor` retrieval family
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
    now_rfc3339, validate_counter, FieldFilter, ListOpts, ModelStore, Record,
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
        self.prepare_insert_inner(input, tenant, false)
    }

    /// Same pipeline WITHOUT the mass-assignment guard (Eloquent
    /// `forceCreate`): undeclared keys pass through raw — tenant, identity,
    /// required, and cast validation all still apply.
    pub(crate) fn prepare_insert_unrestricted(
        &self,
        input: &Record,
        tenant: Option<TenantId>,
    ) -> Result<Record, AppError> {
        self.prepare_insert_inner(input, tenant, true)
    }

    fn prepare_insert_inner(
        &self,
        input: &Record,
        tenant: Option<TenantId>,
        force: bool,
    ) -> Result<Record, AppError> {
        let mut record = Record::new();

        // 1. Mutators transform supplied wire values.
        let mut supplied: Record = input.clone();
        for (key, mutator) in &self.mutators {
            if let Some(value) = supplied.remove(key) {
                supplied.insert(key.clone(), mutator(value)?);
            }
        }

        // 2. Mass-assignment boundary: declared fields only (skipped for
        //    forceCreate, which bypasses the guard by definition).
        if !force {
            for key in supplied.keys() {
                if self.model.field_named(key).is_none() {
                    return Err(AppError::validation(format!(
                        "unknown field {key:?} for model {}",
                        self.model.name
                    )));
                }
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
        // 5b. Force mode: undeclared keys pass through raw — no field
        //     definition exists to cast or validate them against.
        if force {
            for (key, value) in &supplied {
                if self.model.field_named(key).is_none() {
                    record.insert(key.clone(), value.clone());
                }
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
        self.insert_prepared(store, record)
    }

    /// Insert an already-prepared record: observers fire around the store
    /// write, in order.
    fn insert_prepared(
        &self,
        store: &dyn ModelStore,
        record: Record,
    ) -> Result<Record, AppError> {
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

    // ------------------------------------------------------------- finders

    /// Predicate guard shared by the finder family (same rule as `list`).
    fn validate_predicates(&self, predicates: &[FieldFilter]) -> Result<(), AppError> {
        for filter in predicates {
            if filter.field != "id" && self.model.field_named(&filter.field).is_none() {
                return Err(AppError::validation(format!(
                    "filter on unknown field {:?}",
                    filter.field
                )));
            }
        }
        Ok(())
    }

    /// Canonical uuid id of a record (every Wave 3 model keys on a uuid
    /// `id`).
    fn record_id(&self, record: &Record) -> Result<Uuid, AppError> {
        record
            .get("id")
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| {
                AppError::validation(format!("{} record needs a uuid id", self.model.name))
            })
    }

    /// Column guard for `value`/`pluck`: the field must exist on the model.
    fn validate_read_field(&self, field: &str) -> Result<(), AppError> {
        if self.model.field_named(field).is_none() {
            return Err(AppError::validation(format!(
                "field {field:?} is not on model {}",
                self.model.name
            )));
        }
        Ok(())
    }

    /// First live row matching `predicates` (Eloquent
    /// `where(...)->first()`); `Ok(None)` on miss.
    pub fn first(
        &self,
        store: &dyn ModelStore,
        tenant: Option<TenantId>,
        predicates: &[FieldFilter],
    ) -> Result<Option<Record>, AppError> {
        self.validate_predicates(predicates)?;
        store.find_by(&self.model, tenant, predicates, false)
    }

    /// First match, else a typed `NOT_FOUND` error — the 404 path through
    /// the Wave 2 error registry. (`find` itself keeps `Ok(None)` for the
    /// Wave 1 route-binding contract; `get` is its id-based error twin.)
    pub fn first_or_not_found(
        &self,
        store: &dyn ModelStore,
        tenant: Option<TenantId>,
        predicates: &[FieldFilter],
    ) -> Result<Record, AppError> {
        self.first(store, tenant, predicates)?
            .ok_or_else(|| AppError::not_found(format!("{} first match", self.model.name)))
    }

    /// Eloquent `firstOrNew`: first match, else a prepared (mutators and
    /// casts ran) but UNSAVED record — persist it with
    /// `store.insert(model, &record)`.
    pub fn first_or_new(
        &self,
        store: &dyn ModelStore,
        tenant: Option<TenantId>,
        predicates: &[FieldFilter],
        attrs: &Record,
    ) -> Result<Record, AppError> {
        if let Some(found) = self.first(store, tenant, predicates)? {
            return Ok(found);
        }
        self.prepare_insert(attrs, tenant)
    }

    /// Eloquent `firstOrCreate`: first match, else create through the full
    /// pipeline (events fire only on the create path).
    pub fn first_or_create(
        &self,
        store: &dyn ModelStore,
        tenant: Option<TenantId>,
        predicates: &[FieldFilter],
        attrs: &Record,
    ) -> Result<Record, AppError> {
        if let Some(found) = self.first(store, tenant, predicates)? {
            return Ok(found);
        }
        self.insert(store, attrs, tenant)
    }

    /// Eloquent `updateOrCreate`: update the first match in place, else
    /// create it — same id on the update path, never a duplicate row.
    pub fn update_or_create(
        &self,
        store: &dyn ModelStore,
        tenant: Option<TenantId>,
        predicates: &[FieldFilter],
        attrs: &Record,
    ) -> Result<Record, AppError> {
        if let Some(found) = self.first(store, tenant, predicates)? {
            let id = self.record_id(&found)?;
            return self.update(store, id, tenant, attrs);
        }
        self.insert(store, attrs, tenant)
    }

    /// Eloquent `forceCreate`: prepare WITHOUT the mass-assignment guard
    /// (undeclared keys pass through), then insert through the pipeline —
    /// tenant/required/cast validation still applies.
    pub fn force_create(
        &self,
        store: &dyn ModelStore,
        input: &Record,
        tenant: Option<TenantId>,
    ) -> Result<Record, AppError> {
        let record = self.prepare_insert_unrestricted(input, tenant)?;
        self.insert_prepared(store, record)
    }

    /// Eloquent `createMany`: batch insert — the pipeline (mutators, casts,
    /// observers) runs once per record; the first error aborts the batch.
    pub fn create_many(
        &self,
        store: &dyn ModelStore,
        inputs: &[Record],
        tenant: Option<TenantId>,
    ) -> Result<Vec<Record>, AppError> {
        inputs
            .iter()
            .map(|input| self.insert(store, input, tenant))
            .collect()
    }

    /// Insert and return the generated id (the store write is a single
    /// `INSERT ... RETURNING id`).
    pub fn insert_get_id(
        &self,
        store: &dyn ModelStore,
        input: &Record,
        tenant: Option<TenantId>,
    ) -> Result<Uuid, AppError> {
        let stored = self.insert(store, input, tenant)?;
        self.record_id(&stored)
    }

    /// Insert unless a conflicting row exists (same id or a declared unique
    /// field): `Ok(None)` when the insert was ignored. Observers see
    /// `Creating` for the attempt and `Created` only when a row was stored.
    pub fn insert_or_ignore(
        &self,
        store: &dyn ModelStore,
        input: &Record,
        tenant: Option<TenantId>,
    ) -> Result<Option<Record>, AppError> {
        let record = self.prepare_insert(input, tenant)?;
        self.fire(ModelPhase::Creating, &record);
        let stored = store.insert_or_ignore(&self.model, &record)?;
        if let Some(row) = &stored {
            self.fire(ModelPhase::Created, row);
        }
        Ok(stored)
    }

    /// Eloquent `updateOrInsert`: upsert keyed on `conflict` fields — the
    /// conflict values travel inside `input` (same shape as the SQL
    /// upsert), so this delegates to the runtime upsert pipeline.
    pub fn update_or_insert(
        &self,
        store: &dyn ModelStore,
        input: &Record,
        conflict: &[String],
        tenant: Option<TenantId>,
    ) -> Result<Record, AppError> {
        self.upsert(store, input, conflict, tenant)
    }

    // ----------------------------------------------------------- retrieval

    /// Eloquent `value`: the first match's `field` value (`None` on miss).
    pub fn value(
        &self,
        store: &dyn ModelStore,
        tenant: Option<TenantId>,
        predicates: &[FieldFilter],
        field: &str,
    ) -> Result<Option<Json>, AppError> {
        self.validate_read_field(field)?;
        Ok(self
            .first(store, tenant, predicates)?
            .and_then(|row| row.get(field).cloned()))
    }

    /// Eloquent `pluck`: `field` values across every match in scan order
    /// (soft-deleted rows excluded). Divergence from Laravel: the port
    /// fetches full rows — there is no column projection yet.
    pub fn pluck(
        &self,
        store: &dyn ModelStore,
        tenant: Option<TenantId>,
        predicates: &[FieldFilter],
        field: &str,
    ) -> Result<Vec<Json>, AppError> {
        self.validate_read_field(field)?;
        self.validate_predicates(predicates)?;
        let page = store.list(&self.model, tenant, predicates, &ListOpts::default())?;
        Ok(page
            .items
            .iter()
            .map(|row| row.get(field).cloned().unwrap_or(Json::Null))
            .collect())
    }

    /// Eloquent `chunk`: walk all matches in offset pages of `size`
    /// (clamped to the platform's 200-row page cap), handing each page to
    /// `f` with its 1-based page number. `Ok(false)` = the callback stopped
    /// iteration early; `Ok(true)` = the result set was exhausted.
    pub fn chunk<F>(
        &self,
        store: &dyn ModelStore,
        tenant: Option<TenantId>,
        predicates: &[FieldFilter],
        size: u32,
        mut f: F,
    ) -> Result<bool, AppError>
    where
        F: FnMut(&[Record], u32) -> Result<bool, AppError>,
    {
        self.validate_predicates(predicates)?;
        let per_page = size.clamp(1, 200);
        let mut page: u32 = 1;
        loop {
            let result = self.list(
                store,
                tenant,
                predicates.to_vec(),
                &[],
                Some(PageReq {
                    page,
                    per_page,
                }),
            )?;
            if result.items.is_empty() {
                return Ok(true);
            }
            if !f(&result.items, page)? {
                return Ok(false);
            }
            if (result.items.len() as u32) < per_page {
                return Ok(true);
            }
            page += 1;
        }
    }

    /// Eloquent `each`: a per-ROW callback over the same paging as
    /// [`ModelRuntime::chunk`] (the argument is the 0-based row index);
    /// `Ok(false)` from `f` stops iteration early.
    pub fn each<F>(
        &self,
        store: &dyn ModelStore,
        tenant: Option<TenantId>,
        predicates: &[FieldFilter],
        size: u32,
        mut f: F,
    ) -> Result<bool, AppError>
    where
        F: FnMut(&Record, u32) -> Result<bool, AppError>,
    {
        let mut index: u32 = 0;
        let mut completed = true;
        self.chunk(store, tenant, predicates, size, |rows, _| {
            for row in rows {
                if !f(row, index)? {
                    completed = false;
                    return Ok(false);
                }
                index += 1;
            }
            Ok(true)
        })?;
        Ok(completed)
    }

    /// Eloquent `cursor`: a lazy row iterator that fetches each page only
    /// when the previous buffer drains. Divergence: `cursor()` streams one
    /// statement in Laravel — this walks offset pages through the port, so
    /// concurrent writes between pages can shift results.
    pub fn cursor<'r>(
        &'r self,
        store: &'r dyn ModelStore,
        tenant: Option<TenantId>,
        predicates: &[FieldFilter],
        page_size: u32,
    ) -> Result<RecordCursor<'r>, AppError> {
        self.validate_predicates(predicates)?;
        Ok(RecordCursor {
            runtime: self,
            store,
            tenant,
            filters: predicates.to_vec(),
            page: 1,
            per_page: page_size.clamp(1, 200),
            buffer: Vec::new().into_iter(),
            done: false,
            error: None,
        })
    }

    // ------------------------------------------------------------- counters

    /// Atomic `SET field = field + by` with prepared `extra` columns (e.g.
    /// `updated_at`) applied in the SAME statement — concurrent increments
    /// never lose an update. Typed `NOT_FOUND` when the row is missing,
    /// soft-deleted, or in another tenant.
    pub fn increment(
        &self,
        store: &dyn ModelStore,
        id: Uuid,
        tenant: Option<TenantId>,
        field: &str,
        by: i64,
        extra: &Record,
    ) -> Result<Record, AppError> {
        validate_counter(&self.model, field)?;
        let prepared_extra = self.prepare_update(extra)?;
        let mut preview = self.get(store, id, tenant)?;
        let preview_value = match preview.get(field) {
            Some(Json::Number(n)) => n
                .as_i64()
                .map(Json::from)
                .unwrap_or_else(|| Json::from(n.as_f64().unwrap_or(0.0) + by as f64)),
            _ => Json::from(by),
        };
        preview.insert(field.to_string(), preview_value);
        for (key, value) in &prepared_extra {
            preview.insert(key.clone(), value.clone());
        }
        self.fire(ModelPhase::Updating, &preview);
        let stored = store
            .increment(&self.model, id, tenant, field, by, &prepared_extra)?
            .ok_or_else(|| AppError::not_found(format!("{} {id}", self.model.name)))?;
        self.fire(ModelPhase::Updated, &stored);
        Ok(stored)
    }

    /// [`ModelRuntime::increment`] with a negative step.
    pub fn decrement(
        &self,
        store: &dyn ModelStore,
        id: Uuid,
        tenant: Option<TenantId>,
        field: &str,
        by: i64,
        extra: &Record,
    ) -> Result<Record, AppError> {
        let by = by
            .checked_neg()
            .ok_or_else(|| AppError::validation("decrement amount is out of range"))?;
        self.increment(store, id, tenant, field, by, extra)
    }

    // ---------------------------------------------------------- record ops

    /// Eloquent `replicate`: an UNSAVED clone OF `record` — `id`,
    /// `tenant_id`, `created_at`, `updated_at`, and every `except` field
    /// removed (the insert boundary generates a fresh id and timestamps);
    /// with `detach_relations` the loaded relation payloads are dropped too
    /// (keeping them would fail mass assignment). The source row must
    /// still exist (typed `NOT_FOUND` otherwise); persist the clone with
    /// `store.insert(model, &clone)`.
    pub fn replicate(
        &self,
        store: &dyn ModelStore,
        record: &Record,
        tenant: Option<TenantId>,
        except: &[String],
        detach_relations: bool,
    ) -> Result<Record, AppError> {
        let id = self.record_id(record)?;
        self.get(store, id, tenant)?;
        let mut clone = record.clone();
        for key in ["id", "tenant_id", "created_at", "updated_at"] {
            clone.remove(key);
        }
        for key in except {
            clone.remove(key);
        }
        if detach_relations {
            for rel in &self.model.relations {
                clone.remove(&rel.name);
            }
        }
        self.prepare_insert(&clone, tenant)
    }

    /// Re-fetch this record's row as a NEW record (Eloquent `fresh()`):
    /// `Ok(None)` when the row was soft-deleted or removed meanwhile.
    pub fn fresh(
        &self,
        store: &dyn ModelStore,
        record: &Record,
        tenant: Option<TenantId>,
    ) -> Result<Option<Record>, AppError> {
        let id = self.record_id(record)?;
        store.find(&self.model, id, tenant, false)
    }

    /// Re-hydrate `record` in place from the store (Eloquent `refresh()`);
    /// typed `NOT_FOUND` when the row is gone. Loaded relation keys are
    /// replaced by the store row — use `load_missing` to bring them back.
    pub fn refresh(
        &self,
        store: &dyn ModelStore,
        record: &mut Record,
        tenant: Option<TenantId>,
    ) -> Result<(), AppError> {
        let id = self.record_id(record)?;
        let fresh = store
            .find(&self.model, id, tenant, false)?
            .ok_or_else(|| AppError::not_found(format!("{} {id}", self.model.name)))?;
        *record = fresh;
        Ok(())
    }

    /// Load only the relations that are not yet present on every record
    /// (Eloquent `loadMissing`).
    pub fn load_missing(
        &self,
        store: &dyn ModelStore,
        relation_names: &[String],
        parents: &mut [Record],
        tenant: Option<TenantId>,
    ) -> Result<(), AppError> {
        for name in relation_names {
            if parents.iter().all(|p| p.contains_key(name)) {
                continue;
            }
            self.load_relation(store, name, parents, tenant)?;
        }
        Ok(())
    }

    /// Eloquent `touch`: bump `updated_at` (typed `NOT_FOUND` when the row
    /// is gone). Models without an `updated_at` column are refused.
    pub fn touch(
        &self,
        store: &dyn ModelStore,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<Record, AppError> {
        if self.model.field_named("updated_at").is_none() {
            return Err(AppError::validation(format!(
                "model {} has no updated_at column to touch",
                self.model.name
            )));
        }
        // prepare_update injects a fresh updated_at for empty changes.
        self.update(store, id, tenant, &Record::new())
    }

    /// Eloquent `push`: save THIS record (update when its id already
    /// exists, insert otherwise), then recursively save every loaded
    /// relation record; the (re-saved) relation payloads stay attached.
    pub fn push(
        &self,
        store: &dyn ModelStore,
        record: &mut Record,
        tenant: Option<TenantId>,
    ) -> Result<(), AppError> {
        self.push_inner(store, record, tenant, 0)
    }

    fn push_inner(
        &self,
        store: &dyn ModelStore,
        record: &mut Record,
        tenant: Option<TenantId>,
        depth: u8,
    ) -> Result<(), AppError> {
        if depth > 8 {
            return Err(AppError::validation(format!(
                "{} push exceeded the relation depth limit",
                self.model.name
            )));
        }

        // 1. Split payloads: relation keys are saved separately, and the
        //    framework identity columns never enter the change set.
        let mut relation_payloads: Vec<(String, Json)> = Vec::new();
        for rel in &self.model.relations {
            if let Some(payload) = record.get(&rel.name).cloned() {
                if !payload.is_null() {
                    relation_payloads.push((rel.name.clone(), payload));
                }
            }
        }
        let mut attrs = record.clone();
        for rel in &self.model.relations {
            attrs.remove(&rel.name);
        }
        // The insert boundary re-injects the tenant from the call argument.
        attrs.remove("tenant_id");
        let mut changes = Record::new();
        for (key, value) in &attrs {
            if matches!(key.as_str(), "id" | "tenant_id" | "created_at" | "updated_at") {
                continue;
            }
            if self.model.field_named(key).is_none() {
                continue;
            }
            changes.insert(key.clone(), value.clone());
        }

        // 2. Save self: update the persisted row, else insert as new.
        let self_id = self.record_id(&attrs).ok();
        let persisted = match self_id {
            Some(id) => store.find(&self.model, id, tenant, false)?.is_some(),
            None => false,
        };
        let saved = if persisted {
            let id = self_id.expect("persisted implies an id");
            if changes.is_empty() && self.model.field_named("updated_at").is_none() {
                // Nothing writable changed — keep the stored row as-is.
                self.get(store, id, tenant)?
            } else {
                self.update(store, id, tenant, &changes)?
            }
        } else {
            self.insert(store, &attrs, tenant)?
        };

        // 3. Recursively save loaded relation records (needs the registry).
        let mut reattached: Vec<(String, Json)> = Vec::new();
        if !relation_payloads.is_empty() {
            let registry = self.registry.clone().ok_or_else(|| {
                AppError::validation("push needs a model registry to save relations")
            })?;
            for (name, payload) in &relation_payloads {
                let rel = self
                    .model
                    .relations
                    .iter()
                    .find(|r| r.name == *name)
                    .ok_or_else(|| {
                        AppError::validation(format!("unknown relation {name:?}"))
                    })?;
                let target = registry.get(&rel.target).cloned().ok_or_else(|| {
                    AppError::validation(format!(
                        "relation {name:?} targets unknown model {}",
                        rel.target
                    ))
                })?;
                let child_rt = ModelRuntime::new(target).with_registry(registry.clone());
                match payload {
                    Json::Object(child) => {
                        let mut child = child.clone();
                        child_rt.push_inner(store, &mut child, tenant, depth + 1)?;
                        reattached.push((name.clone(), Json::Object(child)));
                    }
                    Json::Array(items) => {
                        let mut out = Vec::with_capacity(items.len());
                        for item in items {
                            match item {
                                Json::Object(child) => {
                                    let mut child = child.clone();
                                    child_rt.push_inner(store, &mut child, tenant, depth + 1)?;
                                    out.push(Json::Object(child));
                                }
                                other => out.push(other.clone()),
                            }
                        }
                        reattached.push((name.clone(), Json::Array(out)));
                    }
                    other => reattached.push((name.clone(), other.clone())),
                }
            }
        }

        // 4. The record now carries its saved attributes plus the re-saved
        //    relation payloads.
        *record = saved;
        for (name, value) in reattached {
            record.insert(name, value);
        }
        Ok(())
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

/// Lazy page iterator returned by [`ModelRuntime::cursor`]: the next page
/// is fetched only when the current buffer drains. Yields
/// `Err` exactly once if a page fetch fails, then ends.
pub struct RecordCursor<'r> {
    runtime: &'r ModelRuntime,
    store: &'r dyn ModelStore,
    tenant: Option<TenantId>,
    filters: Vec<FieldFilter>,
    page: u32,
    per_page: u32,
    buffer: std::vec::IntoIter<Record>,
    done: bool,
    error: Option<AppError>,
}

impl RecordCursor<'_> {
    /// Next row, fetching the next page on demand.
    pub fn next_record(&mut self) -> Option<Result<Record, AppError>> {
        loop {
            if let Some(row) = self.buffer.next() {
                return Some(Ok(row));
            }
            if self.done {
                return self.error.take().map(Err);
            }
            match self.runtime.list(
                self.store,
                self.tenant,
                self.filters.clone(),
                &[],
                Some(PageReq {
                    page: self.page,
                    per_page: self.per_page,
                }),
            ) {
                Ok(page) => {
                    let fetched = page.items.len() as u32;
                    self.page += 1;
                    if fetched < self.per_page {
                        self.done = true;
                    }
                    self.buffer = page.items.into_iter();
                }
                Err(err) => {
                    self.done = true;
                    self.error = Some(err);
                }
            }
        }
    }
}

impl Iterator for RecordCursor<'_> {
    type Item = Result<Record, AppError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_record()
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
            .field(FieldDef::new("visits", FieldType::Int64))
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

    // ---------------------------------------------------- Wave 3 (phase 3)

    #[test]
    fn find_keeps_ok_none_while_error_finders_raise_not_found() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        // Route-binding contract: a miss is Ok(None) (the edge maps it to404)...
        assert!(rt()
            .find(&store, Uuid::now_v7(), Some(tenant))
            .unwrap()
            .is_none());
        // ...while the error finders raise typed NOT_FOUND (the Wave 2
        // error-registry 404 path).
        let err = rt().get(&store, Uuid::now_v7(), Some(tenant)).unwrap_err();
        assert_eq!(err.code, "KER_NOT_FOUND");
        let err = rt()
            .first_or_not_found(&store, Some(tenant), &[FieldFilter::eq("patient", "ghost")])
            .unwrap_err();
        assert_eq!(err.code, "KER_NOT_FOUND");
        // A hit satisfies first_or_not_found.
        rt().insert(&store, &input("here"), Some(tenant)).unwrap();
        let found = rt()
            .first_or_not_found(&store, Some(tenant), &[FieldFilter::eq("patient", "here")])
            .unwrap();
        assert_eq!(found["patient"], Json::String("here".into()));
    }

    #[test]
    fn first_or_create_and_first_or_new() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let a = rt()
            .first_or_create(
                &store,
                Some(tenant),
                &[FieldFilter::eq("patient", "jane")],
                &input("jane"),
            )
            .unwrap();
        assert_eq!(a["patient"], Json::String("jane".into()));

        // Second call finds it — same id, no duplicate row.
        let again = rt()
            .first_or_create(
                &store,
                Some(tenant),
                &[FieldFilter::eq("patient", "jane")],
                &input("jane"),
            )
            .unwrap();
        assert_eq!(rid(&again), rid(&a));
        assert_eq!(
            rt().list(&store, Some(tenant), vec![], &[], None).unwrap().total,
            Some(1)
        );

        // first: hit and miss.
        assert!(rt()
            .first(&store, Some(tenant), &[FieldFilter::eq("patient", "jane")])
            .unwrap()
            .is_some());
        assert!(rt()
            .first(&store, Some(tenant), &[FieldFilter::eq("patient", "ghost")])
            .unwrap()
            .is_none());

        // first_or_new: hit returns the stored row...
        let found = rt()
            .first_or_new(
                &store,
                Some(tenant),
                &[FieldFilter::eq("patient", "jane")],
                &input("jane"),
            )
            .unwrap();
        assert_eq!(rid(&found), rid(&a));
        // ...miss returns a prepared but UNSAVED record (id + timestamps).
        let prepared = rt()
            .first_or_new(
                &store,
                Some(tenant),
                &[FieldFilter::eq("patient", "bob")],
                &input("bob"),
            )
            .unwrap();
        assert!(prepared.contains_key("id"));
        assert!(prepared.contains_key("created_at"));
        assert_eq!(
            rt().list(&store, Some(tenant), vec![], &[], None).unwrap().total,
            Some(1)
        );

        // Unknown predicate fields are refused everywhere.
        let err = rt()
            .first(&store, Some(tenant), &[FieldFilter::eq("ghost_col", "x")])
            .unwrap_err();
        assert!(err.public_message.contains("unknown field"), "{}", err.public_message);
    }

    #[test]
    fn update_or_create_updates_in_place_then_creates() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let a = rt().insert(&store, &input("jane"), Some(tenant)).unwrap();

        // Hit: attrs update the SAME row (no duplicate).
        let mut attrs = Record::new();
        attrs.insert("status".into(), Json::String("cancelled".into()));
        let updated = rt()
            .update_or_create(
                &store,
                Some(tenant),
                &[FieldFilter::eq("patient", "jane")],
                &attrs,
            )
            .unwrap();
        assert_eq!(rid(&updated), rid(&a));
        assert_eq!(updated["status"], Json::String("cancelled".into()));
        assert_eq!(
            rt().list(&store, Some(tenant), vec![], &[], None).unwrap().total,
            Some(1)
        );

        // Miss: creates through the full pipeline.
        let created = rt()
            .update_or_create(
                &store,
                Some(tenant),
                &[FieldFilter::eq("patient", "bob")],
                &input("bob"),
            )
            .unwrap();
        assert_eq!(created["patient"], Json::String("bob".into()));
        assert_eq!(
            rt().list(&store, Some(tenant), vec![], &[], None).unwrap().total,
            Some(2)
        );
    }

    #[test]
    fn force_create_bypasses_the_mass_assignment_guard() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let mut guarded = input("p");
        guarded.insert("extra".into(), Json::String("raw".into()));

        // Normal insert refuses the undeclared key...
        let err = rt().insert(&store, &guarded, Some(tenant)).unwrap_err();
        assert!(err.public_message.contains("unknown field"), "{}", err.public_message);
        // ...force_create writes it through.
        let stored = rt().force_create(&store, &guarded, Some(tenant)).unwrap();
        assert_eq!(stored.get("extra"), Some(&Json::String("raw".into())));

        // Required and tenant validation still apply under force.
        let err = rt()
            .force_create(&store, &Record::new(), Some(tenant))
            .unwrap_err();
        assert!(err.public_message.contains("required"), "{}", err.public_message);
        let err = rt().force_create(&store, &guarded, None).unwrap_err();
        assert!(err.public_message.contains("tenant"), "{}", err.public_message);
    }

    #[test]
    fn create_many_runs_the_pipeline_per_record() {
        #[derive(Default)]
        struct Recorder(std::sync::Mutex<Vec<ModelPhase>>);
        impl ModelObserver for Recorder {
            fn on_model_event(&self, event: &ModelEvent) {
                self.0.lock().unwrap().push(event.phase);
            }
        }
        let recorder = Arc::new(Recorder::default());
        let rt = ModelRuntime::new(appointment_model())
            .with_observer(recorder.clone())
            .with_mutator("patient", |v| {
                Ok(Json::String(v.as_str().unwrap_or("").to_uppercase()))
            });
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();

        let inputs = vec![input("a"), input("b"), input("c")];
        let rows = rt.create_many(&store, &inputs, Some(tenant)).unwrap();
        assert_eq!(rows.len(), 3);
        let names: Vec<&str> = rows.iter().filter_map(|r| r["patient"].as_str()).collect();
        assert_eq!(names, vec!["A", "B", "C"]);
        // Mutators, casts, and observers ran once per record.
        let phases = recorder.0.lock().unwrap().clone();
        assert_eq!(
            phases.iter().filter(|p| **p == ModelPhase::Creating).count(),
            3
        );
        assert_eq!(
            phases.iter().filter(|p| **p == ModelPhase::Created).count(),
            3
        );

        // A bad record aborts the batch with a typed error.
        let mut bad = input("d");
        bad.insert("hacker".into(), Json::Bool(true));
        let err = rt
            .create_many(&store, &[input("d2"), bad], Some(tenant))
            .unwrap_err();
        assert!(err.public_message.contains("unknown field"), "{}", err.public_message);
    }

    #[test]
    fn insert_get_id_and_insert_or_ignore() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let id1 = rt().insert_get_id(&store, &input("a"), Some(tenant)).unwrap();
        let id2 = rt().insert_get_id(&store, &input("b"), Some(tenant)).unwrap();
        assert_ne!(id1, id2);
        assert!(rt().find(&store, id1, Some(tenant)).unwrap().is_some());

        // Conflicting id → ignored (Ok(None)), not an error.
        let mut dup = input("c");
        dup.insert("id".into(), Json::String(id1.to_string()));
        assert!(rt().insert_or_ignore(&store, &dup, Some(tenant)).unwrap().is_none());
        assert_eq!(
            rt().list(&store, Some(tenant), vec![], &[], None).unwrap().total,
            Some(2)
        );
        // A fresh row stores normally.
        assert!(rt()
            .insert_or_ignore(&store, &input("d"), Some(tenant))
            .unwrap()
            .is_some());
        assert_eq!(
            rt().list(&store, Some(tenant), vec![], &[], None).unwrap().total,
            Some(3)
        );
    }

    #[test]
    fn update_or_insert_upserts_keyed_on_conflict_fields() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let conflict = ["patient".to_string()];
        let first = rt()
            .update_or_insert(&store, &input("jane"), &conflict, Some(tenant))
            .unwrap();

        let mut second = input("jane");
        second.insert("status".into(), Json::String("cancelled".into()));
        let merged = rt()
            .update_or_insert(&store, &second, &conflict, Some(tenant))
            .unwrap();
        assert_eq!(rid(&merged), rid(&first));
        assert_eq!(merged["status"], Json::String("cancelled".into()));
        assert_eq!(
            rt().list(&store, Some(tenant), vec![], &[], None).unwrap().total,
            Some(1)
        );
    }

    #[test]
    fn increment_decrement_counter_and_guards() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let row = rt().insert(&store, &input("c"), Some(tenant)).unwrap();
        let id = rid(&row);

        // NULL counters start at zero.
        let out = rt()
            .increment(&store, id, Some(tenant), "visits", 3, &Record::new())
            .unwrap();
        assert_eq!(out["visits"], Json::from(3));
        let out = rt()
            .increment(&store, id, Some(tenant), "visits", 2, &Record::new())
            .unwrap();
        assert_eq!(out["visits"], Json::from(5));
        let out = rt()
            .decrement(&store, id, Some(tenant), "visits", 7, &Record::new())
            .unwrap();
        assert_eq!(out["visits"], Json::from(-2));

        // Extra columns ride the same statement.
        let mut extra = Record::new();
        extra.insert("status".into(), Json::String("cancelled".into()));
        let out = rt()
            .increment(&store, id, Some(tenant), "visits", 1, &extra)
            .unwrap();
        assert_eq!(out["visits"], Json::from(-1));
        assert_eq!(out["status"], Json::String("cancelled".into()));

        // Typed NOT_FOUND for a missing row...
        let err = rt()
            .increment(&store, Uuid::now_v7(), Some(tenant), "visits", 1, &Record::new())
            .unwrap_err();
        assert_eq!(err.code, "KER_NOT_FOUND");
        // ...validation for undeclared, non-numeric, and immutable fields.
        let err = rt()
            .increment(&store, id, Some(tenant), "nope", 1, &Record::new())
            .unwrap_err();
        assert!(err.public_message.contains("not on model"), "{}", err.public_message);
        let err = rt()
            .increment(&store, id, Some(tenant), "patient", 1, &Record::new())
            .unwrap_err();
        assert!(err.public_message.contains("not numeric"), "{}", err.public_message);
        let err = rt()
            .increment(&store, id, Some(tenant), "id", 1, &Record::new())
            .unwrap_err();
        assert!(err.public_message.contains("immutable"), "{}", err.public_message);
    }

    #[test]
    fn value_and_pluck_read_column_values() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        rt().insert(&store, &input("jane"), Some(tenant)).unwrap();
        rt().insert(&store, &input("jane"), Some(tenant)).unwrap();
        let john = rt().insert(&store, &input("john"), Some(tenant)).unwrap();

        let v = rt()
            .value(
                &store,
                Some(tenant),
                &[FieldFilter::eq("patient", "jane")],
                "status",
            )
            .unwrap();
        assert_eq!(v, Some(Json::String("booked".into())));
        assert!(rt()
            .value(
                &store,
                Some(tenant),
                &[FieldFilter::eq("patient", "ghost")],
                "status",
            )
            .unwrap()
            .is_none());
        // Unknown columns are refused.
        let err = rt().value(&store, Some(tenant), &[], "nope").unwrap_err();
        assert!(err.public_message.contains("not on model"), "{}", err.public_message);
        let err = rt().pluck(&store, Some(tenant), &[], "nope").unwrap_err();
        assert!(err.public_message.contains("not on model"), "{}", err.public_message);

        let mut all: Vec<String> = rt()
            .pluck(&store, Some(tenant), &[], "patient")
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str().map(String::from))
            .collect();
        all.sort();
        assert_eq!(all, vec!["jane", "jane", "john"]);

        // Soft-deleted rows drop out of pluck (store veil).
        rt().delete(&store, rid(&john), Some(tenant)).unwrap();
        let patients = rt().pluck(&store, Some(tenant), &[], "patient").unwrap();
        assert_eq!(patients.len(), 2);
    }

    #[test]
    fn chunk_and_each_page_and_stop() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        for name in ["a", "b", "c", "d", "e"] {
            rt().insert(&store, &input(name), Some(tenant)).unwrap();
        }

        // Pages arrive in order: 2 + 2 + 1, then exhaustion.
        let mut pages: Vec<(u32, usize)> = Vec::new();
        let done = rt()
            .chunk(&store, Some(tenant), &[], 2, |rows, page| {
                pages.push((page, rows.len()));
                Ok(true)
            })
            .unwrap();
        assert!(done);
        assert_eq!(pages, vec![(1, 2), (2, 2), (3, 1)]);

        // The callback can stop iteration early.
        let mut pages = Vec::new();
        let done = rt()
            .chunk(&store, Some(tenant), &[], 2, |rows, page| {
                pages.push((page, rows.len()));
                Ok(page < 2)
            })
            .unwrap();
        assert!(!done);
        assert_eq!(pages, vec![(1, 2), (2, 2)]);

        // Callback errors propagate.
        let err = rt()
            .chunk(&store, Some(tenant), &[], 2, |_, _| Err(AppError::validation("boom")))
            .unwrap_err();
        assert!(err.public_message.contains("boom"));

        // each: per-row 0-based indices, full walk...
        let mut seen: Vec<u32> = Vec::new();
        let completed = rt()
            .each(&store, Some(tenant), &[], 2, |_row, index| {
                seen.push(index);
                Ok(true)
            })
            .unwrap();
        assert!(completed);
        assert_eq!(seen, vec![0, 1, 2, 3, 4]);

        // ...then early stop.
        let mut count = 0u32;
        let completed = rt()
            .each(&store, Some(tenant), &[], 2, |_row, _index| {
                count += 1;
                Ok(count < 3)
            })
            .unwrap();
        assert!(!completed);
        assert_eq!(count, 3);
    }

    #[test]
    fn cursor_iterates_lazily_across_pages() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        for name in ["a", "b", "c", "d", "e"] {
            rt().insert(&store, &input(name), Some(tenant)).unwrap();
        }

        let rows: Vec<Record> = rt()
            .cursor(&store, Some(tenant), &[], 2)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(rows.len(), 5);
        let names: Vec<&str> = rows.iter().filter_map(|r| r["patient"].as_str()).collect();
        assert_eq!(names, vec!["a", "b", "c", "d", "e"]);

        // Predicates filter the stream; a miss yields nothing.
        let rows: Vec<Record> = rt()
            .cursor(&store, Some(tenant), &[FieldFilter::eq("patient", "c")], 2)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(rows.len(), 1);
        let empty: Vec<Record> = rt()
            .cursor(&store, Some(tenant), &[FieldFilter::eq("patient", "ghost")], 2)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn replicate_makes_an_unsaved_clone() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let mut i = input("jane");
        i.insert("notes".into(), Json::String("private".into()));
        let a = rt().insert(&store, &i, Some(tenant)).unwrap();
        // Pin the source timestamps so the clone's are provably fresh.
        let mut past = Record::new();
        past.insert("created_at".into(), Json::String("2000-01-01T00:00:00Z".into()));
        store
            .update(&appointment_model(), rid(&a), Some(tenant), &past)
            .unwrap();
        let a = rt().get(&store, rid(&a), Some(tenant)).unwrap();

        let clone = rt()
            .replicate(&store, &a, Some(tenant), &["notes".into()], true)
            .unwrap();
        // Fresh identity, fresh framework timestamps, except-fields removed.
        assert_ne!(clone["id"], a["id"]);
        assert!(!clone.contains_key("notes"));
        assert_ne!(clone["created_at"], Json::String("2000-01-01T00:00:00Z".into()));
        assert!(clone.contains_key("updated_at"));
        assert_eq!(clone["patient"], Json::String("jane".into()));

        // ...and the clone is UNSAVED until persisted.
        assert_eq!(
            rt().list(&store, Some(tenant), vec![], &[], None).unwrap().total,
            Some(1)
        );
        store.insert(&appointment_model(), &clone).unwrap();
        assert_eq!(
            rt().list(&store, Some(tenant), vec![], &[], None).unwrap().total,
            Some(2)
        );

        // A record whose row no longer exists raises typed NOT_FOUND.
        let mut bogus = Record::new();
        bogus.insert("id".into(), Json::String(Uuid::now_v7().to_string()));
        let err = rt().replicate(&store, &bogus, Some(tenant), &[], true).unwrap_err();
        assert_eq!(err.code, "KER_NOT_FOUND");
    }

    #[test]
    fn replicate_detaches_loaded_relation_payloads() {
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
        let patient_rt = ModelRuntime::new(patient.clone()).with_registry(Arc::new(registry));
        let store = InMemoryModelStore::new();

        let mut name = Record::new();
        name.insert("name".into(), Json::String("P1".into()));
        let mut p = patient_rt.insert(&store, &name, None).unwrap();
        p.insert("appointments".into(), Json::Array(vec![])); // pretend loaded

        // detach=true strips the payload → the clone prepares cleanly...
        let clone = patient_rt
            .replicate(&store, &p, None, &[], true)
            .unwrap();
        assert!(!clone.contains_key("appointments"));
        assert_ne!(clone["id"], p["id"]);

        // ...detach=false keeps it → mass assignment refuses the payload.
        let err = patient_rt
            .replicate(&store, &p, None, &[], false)
            .unwrap_err();
        assert!(err.public_message.contains("unknown field"), "{}", err.public_message);
    }

    #[test]
    fn refresh_and_fresh_rehydrate_from_the_store() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let a = rt().insert(&store, &input("jane"), Some(tenant)).unwrap();
        let mut snapshot = rt().find(&store, rid(&a), Some(tenant)).unwrap().unwrap();

        // Mutate the row behind the snapshot.
        let mut changes = Record::new();
        changes.insert("patient".into(), Json::String("jane-renamed".into()));
        store
            .update(&appointment_model(), rid(&a), Some(tenant), &changes)
            .unwrap();

        // fresh: a NEW record, snapshot untouched.
        let fresh = rt().fresh(&store, &snapshot, Some(tenant)).unwrap().unwrap();
        assert_eq!(fresh["patient"], Json::String("jane-renamed".into()));
        assert_eq!(snapshot["patient"], Json::String("jane".into()));

        // refresh: re-hydrates IN PLACE.
        rt().refresh(&store, &mut snapshot, Some(tenant)).unwrap();
        assert_eq!(snapshot["patient"], Json::String("jane-renamed".into()));

        // Deleted meanwhile: refresh → typed NOT_FOUND, fresh → None.
        rt().delete(&store, rid(&a), Some(tenant)).unwrap();
        let err = rt().refresh(&store, &mut snapshot, Some(tenant)).unwrap_err();
        assert_eq!(err.code, "KER_NOT_FOUND");
        assert!(rt().fresh(&store, &snapshot, Some(tenant)).unwrap().is_none());
    }

    #[test]
    fn load_missing_only_loads_unloaded_relations() {
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
        let patient_rt = ModelRuntime::new(patient.clone()).with_registry(registry.clone());
        let appointment_rt = ModelRuntime::new(appointment.clone()).with_registry(registry);
        let store = InMemoryModelStore::new();

        let mut name = Record::new();
        name.insert("name".into(), Json::String("P1".into()));
        let p = patient_rt.insert(&store, &name, None).unwrap();
        let mut a = Record::new();
        a.insert("patient_id".into(), p["id"].clone());
        a.insert("slot".into(), Json::String("09:00".into()));
        appointment_rt.insert(&store, &a, None).unwrap();

        let mut parents = vec![p];
        // Already "loaded" (sentinel payload) → skipped entirely.
        parents[0].insert("appointments".into(), Json::String("cached".into()));
        patient_rt
            .load_missing(&store, &["appointments".into()], &mut parents, None)
            .unwrap();
        assert_eq!(parents[0]["appointments"], Json::String("cached".into()));

        // Unloaded → fetched through the normal relation path.
        parents[0].remove("appointments");
        patient_rt
            .load_missing(&store, &["appointments".into()], &mut parents, None)
            .unwrap();
        let attached = parents[0]["appointments"].as_array().unwrap();
        assert_eq!(attached.len(), 1);
        assert_eq!(attached[0]["slot"], Json::String("09:00".into()));
    }

    #[test]
    fn touch_bumps_updated_at() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let a = rt().insert(&store, &input("t"), Some(tenant)).unwrap();

        // Pin updated_at to the past, then touch.
        let mut past = Record::new();
        past.insert("updated_at".into(), Json::String("2000-01-01T00:00:00Z".into()));
        rt().update(&store, rid(&a), Some(tenant), &past).unwrap();
        let touched = rt().touch(&store, rid(&a), Some(tenant)).unwrap();
        assert_ne!(
            touched.get("updated_at"),
            Some(&Json::String("2000-01-01T00:00:00Z".into()))
        );
        // Missing row → typed NOT_FOUND.
        let err = rt().touch(&store, Uuid::now_v7(), Some(tenant)).unwrap_err();
        assert_eq!(err.code, "KER_NOT_FOUND");

        // Models without an updated_at column are refused.
        let plain = ModelDef::new("Plain", "plains")
            .field(FieldDef::new("id", FieldType::Uuid).required())
            .field(FieldDef::new("name", FieldType::Text));
        let err = ModelRuntime::new(plain)
            .touch(&store, Uuid::now_v7(), None)
            .unwrap_err();
        assert!(err.public_message.contains("no updated_at"), "{}", err.public_message);
    }

    #[test]
    fn push_saves_self_and_loaded_relations() {
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
        let patient_rt = ModelRuntime::new(patient.clone()).with_registry(registry.clone());
        let appointment_rt = ModelRuntime::new(appointment.clone()).with_registry(registry);
        let store = InMemoryModelStore::new();

        let mut name = Record::new();
        name.insert("name".into(), Json::String("P1".into()));
        let p = patient_rt.insert(&store, &name, None).unwrap();
        let mut a = Record::new();
        a.insert("patient_id".into(), p["id"].clone());
        a.insert("slot".into(), Json::String("09:00".into()));
        let child = appointment_rt.insert(&store, &a, None).unwrap();

        let mut parents = vec![p];
        patient_rt
            .load_relation(&store, "appointments", &mut parents, None)
            .unwrap();

        // Dirty both the parent and its loaded child.
        parents[0].insert("name".into(), Json::String("P1-renamed".into()));
        let mut children = parents[0]["appointments"].as_array().unwrap().clone();
        children[0]["slot"] = Json::String("11:00".into());
        parents[0].insert("appointments".into(), Json::Array(children));

        patient_rt.push(&store, &mut parents[0], None).unwrap();

        // Parent persisted...
        let stored_parent = store
            .find(&patient, rid(&parents[0]), None, false)
            .unwrap()
            .unwrap();
        assert_eq!(stored_parent["name"], Json::String("P1-renamed".into()));
        // ...child persisted...
        let stored_child = store
            .find(&appointment, rid(&child), None, false)
            .unwrap()
            .unwrap();
        assert_eq!(stored_child["slot"], Json::String("11:00".into()));
        // ...and the record still carries its relation payload.
        assert_eq!(
            parents[0]["appointments"][0]["slot"],
            Json::String("11:00".into())
        );
    }
}