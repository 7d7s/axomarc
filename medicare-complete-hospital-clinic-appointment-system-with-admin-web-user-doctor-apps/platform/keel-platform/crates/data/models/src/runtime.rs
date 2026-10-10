//! Model runtime (S8 C4/B-05/B-10/B-11): the write/read boundary between
//! untyped JSON input and the [`crate::store::ModelStore`] port.
//!
//! Responsibilities:
//! - mass-assignment boundary (declared fields only, unknown keys rejected)
//! - field-level casts between JSON wire values and [`kelson_query::Value`]
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

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use kelson_foundation::{codes, AppError, ErrorCategory, Retryability, Sealer, Severity, TenantId};
use kelson_query::{Ident, PageReq, Value as Bind};
use serde_json::Value as Json;
use uuid::Uuid;

use crate::casts::{builtin_cast, Cast, EncryptedCast};
use crate::store::{
    fold_aggregate_values, now_rfc3339, sort_records, validate_aggregate, validate_counter,
    AggregateOp, ExistsFilter, FieldFilter, FilterOp, ListOpts, ModelStore, Record,
};
use crate::{FieldDef, FieldType, ModelDef, ModelRegistry, RelationDef, RelationKind};

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

/// Observer hook (bridge to kelson-event/outbox lives in the engine layer).
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

/// Prune predicate for a `Prunable` model: builds the row filter (e.g.
/// `created_at < now() - 90 days`). Invoked ONCE per prune pass so
/// time-based cutoffs recompute against the clock each pass.
pub type PrunePredicate = Arc<dyn Fn() -> Vec<FieldFilter> + Send + Sync>;

/// Per-call serialization overrides behind [`ModelRuntime::wire`].
#[derive(Default)]
struct WireOpts {
    force_visible: Vec<String>,
    force_hidden: Vec<String>,
    append: Vec<String>,
}

/// Per-call serialization builder: `rt.wire(&record)
///     .make_visible(["token"])
///     .make_hidden(["email"])
///     .append(["full_name"])
///     .to_record()?`
///
/// Precedence inside a projection: `sensitive` -> manifest `hidden` ->
/// manifest `visible` -> `make_visible` -> `make_hidden` (last wins);
/// accessors append after that and `make_hidden` can remove them by name.
pub struct WireBuilder<'a> {
    runtime: &'a ModelRuntime,
    record: &'a Record,
    opts: WireOpts,
}

impl WireBuilder<'_> {
    /// Force these keys into the projection even when `sensitive` or the
    /// manifest policy drops them (`make_hidden` still wins).
    pub fn make_visible(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.opts
            .force_visible
            .extend(names.into_iter().map(Into::into));
        self
    }

    /// Drop these keys from the projection — stored fields and computed
    /// accessors alike.
    pub fn make_hidden(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.opts
            .force_hidden
            .extend(names.into_iter().map(Into::into));
        self
    }

    /// Assert these accessor names exist and their computed values appear in
    /// the projection (accessors are on by default; this pins them and
    /// rejects unknown names with `KSN_VALIDATION`).
    pub fn append(mut self, names: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.opts.append.extend(names.into_iter().map(Into::into));
        self
    }

    /// Build the projection (or the first cast/append failure).
    pub fn to_record(self) -> Result<Record, AppError> {
        self.runtime.project(self.record, &self.opts)
    }
}

/// Dirty-tracking triple produced by [`ModelRuntime::update_tracked`]:
/// the row BEFORE the write, the full merged row that was handed to the
/// store (pending), and the row the store returned (stored). Field
/// presence counts — a key missing in one side and `null` in the other
/// reads dirty. Framework auto-changes (`updated_at`, the CAS version
/// stamp) therefore show up too, matching Laravel's `isDirty`/
/// `wasChanged`.
#[derive(Debug, Clone)]
pub struct RecordChanges {
    original: Record,
    pending: Record,
    stored: Record,
}

impl RecordChanges {
    /// Pre-write value of `field` (as last read from the store).
    pub fn original(&self, field: &str) -> Option<&Json> {
        self.original.get(field)
    }

    /// Whether the pending row (what was sent) differs from the original
    /// on `field` — Laravel `isDirty`.
    pub fn is_dirty(&self, field: &str) -> bool {
        self.original.get(field) != self.pending.get(field)
    }

    /// Whether the STORED row differs from the original on `field` —
    /// Laravel `wasChanged`.
    pub fn was_changed(&self, field: &str) -> bool {
        self.original.get(field) != self.stored.get(field)
    }

    /// Every field whose pending value differs from the original, sorted.
    pub fn dirty_fields(&self) -> Vec<String> {
        let mut fields: BTreeSet<String> = BTreeSet::new();
        for key in self.pending.keys().chain(self.original.keys()) {
            if self.is_dirty(key) {
                fields.insert(key.clone());
            }
        }
        fields.into_iter().collect()
    }

    /// Every field whose stored value differs from the original, sorted.
    pub fn changed_fields(&self) -> Vec<String> {
        let mut fields: BTreeSet<String> = BTreeSet::new();
        for key in self.stored.keys().chain(self.original.keys()) {
            if self.was_changed(key) {
                fields.insert(key.clone());
            }
        }
        fields.into_iter().collect()
    }

    /// The row exactly as the store returned it after the write.
    pub fn stored(&self) -> &Record {
        &self.stored
    }
}

/// Runtime semantics for one [`ModelDef`]. Cheap to clone (Arc internals).
#[derive(Clone)]
pub struct ModelRuntime {
    model: Arc<ModelDef>,
    registry: Option<Arc<ModelRegistry>>,
    observers: Vec<Arc<dyn ModelObserver>>,
    scopes: BTreeMap<String, ScopeFn>,
    accessors: BTreeMap<String, AccessorFn>,
    mutators: BTreeMap<String, MutatorFn>,
    /// Custom casts keyed by spec name (win over the built-ins).
    casts: BTreeMap<String, Arc<dyn Cast>>,
    /// Strict mode: `load_relation`/`load_missing` refuse with
    /// [`codes::LAZY_LOAD_PREVENTED`]; `with()` stays legal.
    prevent_lazy_loading: bool,
    /// Strict mode: `push` errors instead of skipping undeclared attributes.
    prevent_silently_discarding_attributes: bool,
    /// Strict mode: `value`/`pluck` error when a matched row lacks the field.
    prevent_accessing_missing_attributes: bool,
    /// Global scopes: applied FIRST on every read chokepoint (`list`/
    /// `first`/`pluck`), before named scopes and validation. Eager child
    /// runtimes are built fresh and never inherit these.
    global_scopes: BTreeMap<String, ScopeFn>,
    /// Global scope names suppressed by [`ModelRuntime::without_global_scopes`]
    /// (unknown names are ignored — suppression never errors).
    suppressed_globals: BTreeSet<String>,
    /// [`ModelRuntime::without_global_scopes_all`]: suppress EVERY registered
    /// global scope AND lift the soft-delete veil (all reads become
    /// `include_deleted`) — Laravel parity, where `SoftDeletingScope` IS a
    /// global scope. Named suppression never touches the veil.
    all_globals_suppressed: bool,
    /// [`ModelRuntime::without_timestamps`]: skip `created_at`/`updated_at`
    /// auto-injection; caller-supplied values still pass. Under this flag
    /// `touch` is a no-op write.
    without_timestamps: bool,
    /// [`ModelRuntime::with_prunable`]: maintenance predicate — only
    /// `prune` and [`ModelRuntime::is_prunable`] consult it.
    prunable: Option<PrunePredicate>,
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
            casts: BTreeMap::new(),
            prevent_lazy_loading: false,
            prevent_silently_discarding_attributes: false,
            prevent_accessing_missing_attributes: false,
            global_scopes: BTreeMap::new(),
            suppressed_globals: BTreeSet::new(),
            all_globals_suppressed: false,
            without_timestamps: false,
            prunable: None,
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

    /// Global query scope (Wave 3 Phase 6): applied FIRST to every read
    /// chokepoint (`list`/`first`/`pluck`) — before named scopes and field
    /// validation. Suppressed by name via [`Self::without_global_scopes`],
    /// or entirely (plus veil lift) via [`Self::without_global_scopes_all`].
    /// Divergence: eager child runtimes are built fresh and do not inherit
    /// globals (named scopes already do not apply to eager fetches).
    pub fn with_global_scope(
        mut self,
        name: impl Into<String>,
        f: impl Fn(Vec<FieldFilter>) -> Vec<FieldFilter> + Send + Sync + 'static,
    ) -> Self {
        self.global_scopes.insert(name.into(), Arc::new(f));
        self
    }

    /// Suppress these GLOBAL scopes by name (unknown names are ignored —
    /// suppression must never fail). The soft-delete veil stays in place;
    /// only [`Self::without_global_scopes_all`] lifts it.
    pub fn without_global_scopes(
        mut self,
        names: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.suppressed_globals
            .extend(names.into_iter().map(Into::into));
        self
    }

    /// Suppress every registered global scope AND lift the soft-delete
    /// veil: all reads on this runtime become `include_deleted`
    /// (`find`/`first`/`list`/`pluck`), matching Laravel, where removing
    /// all global scopes removes `SoftDeletingScope` too. Tenant scoping is
    /// never a global scope and is never lifted.
    pub fn without_global_scopes_all(mut self) -> Self {
        self.all_globals_suppressed = true;
        self
    }

    /// Skip framework timestamp injection: `created_at`/`updated_at` on
    /// insert and the `updated_at` refresh on update. Explicitly supplied
    /// timestamp values still pass the cast boundary. Under this flag
    /// `touch` performs a no-op write (nothing to stamp).
    pub fn without_timestamps(mut self) -> Self {
        self.without_timestamps = true;
        self
    }

    /// Mark this model prunable: `prune` calls `f` once per pass to build
    /// the row filter (e.g. `created_at < now() - 90 days`) and force-
    /// purges matching rows in `chunk`-sized pages through the store.
    pub fn with_prunable(
        mut self,
        f: impl Fn() -> Vec<FieldFilter> + Send + Sync + 'static,
    ) -> Self {
        self.prunable = Some(Arc::new(f));
        self
    }

    /// Whether [`Self::with_prunable`] registered a predicate — the
    /// [`crate::prune::PruneRegistry`] skips runtimes that did not.
    pub fn is_prunable(&self) -> bool {
        self.prunable.is_some()
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

    /// Register a custom [`Cast`] under its spec name (fields declare it
    /// with [`crate::FieldDef::cast`]). Registered casts win over the
    /// built-ins of the same name.
    pub fn with_cast(mut self, name: impl Into<String>, cast: Arc<dyn Cast>) -> Self {
        self.casts.insert(name.into(), cast);
        self
    }

    /// Bind the foundation encryption port so `cast = "encrypted"` works.
    /// Ships the [`EncryptedCast`] under the `encrypted` name; the concrete
    /// AES-256-GCM cipher is `kelson_tokens::AesGcmSealer`.
    pub fn with_sealer(mut self, sealer: Arc<dyn Sealer>) -> Self {
        self.casts
            .insert("encrypted".into(), Arc::new(EncryptedCast::new(sealer)));
        self
    }

    /// Strict mode (Wave 3 Phase 5): lazy relation loading —
    /// [`Self::load_relation`] and [`Self::load_missing`] — errors with
    /// [`codes::LAZY_LOAD_PREVENTED`]; declare relations up front with
    /// [`Self::with`] instead.
    pub fn prevent_lazy_loading(mut self) -> Self {
        self.prevent_lazy_loading = true;
        self
    }

    /// Strict mode: [`Self::push`] errors when the record carries an
    /// undeclared attribute instead of silently dropping it from the change
    /// set.
    pub fn prevent_silently_discarding_attributes(mut self) -> Self {
        self.prevent_silently_discarding_attributes = true;
        self
    }

    /// Strict mode: [`Self::value`] and [`Self::pluck`] error when a matched
    /// row lacks the requested declared field instead of yielding
    /// `None`/`null`.
    pub fn prevent_accessing_missing_attributes(mut self) -> Self {
        self.prevent_accessing_missing_attributes = true;
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

    // ---------------------------------------------------------------- casts

    /// Resolve a field's cast: registered spec first (exact, then base name
    /// before `:` so `with_cast("enum", ..)` can override `enum:a,b`), then
    /// the built-in registry.
    fn cast_instance(&self, field: &FieldDef) -> Result<Option<Arc<dyn Cast>>, AppError> {
        let Some(spec) = field.cast.as_deref() else {
            return Ok(None);
        };
        if let Some(cast) = self.casts.get(spec) {
            return Ok(Some(Arc::clone(cast)));
        }
        let base = spec.split(':').next().unwrap_or(spec);
        if base != spec {
            if let Some(cast) = self.casts.get(base) {
                return Ok(Some(Arc::clone(cast)));
            }
        }
        builtin_cast(spec).map(Arc::from).map(Some).map_err(|e| {
            AppError::validation(format!(
                "field {}.{} cast {spec:?}: {}",
                self.model.name, field.name, e.public_message
            ))
        })
    }

    /// Wire value -> storage value (cast `to_db`, validation at set time).
    fn cast_to_db(&self, field: &FieldDef, value: Json) -> Result<Json, AppError> {
        match self.cast_instance(field)? {
            Some(cast) => cast.to_db(&value),
            None => Ok(value),
        }
    }

    /// Storage value -> wire value (only `encrypted` transforms: it opens
    /// the sealed envelope back to plaintext).
    fn cast_from_db(&self, field: &FieldDef, value: Json) -> Result<Json, AppError> {
        match self.cast_instance(field)? {
            Some(cast) => cast.from_db(&value),
            None => Ok(value),
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

        // 5. Copy declared fields through the cast boundary: shape
        //    validation first, then the field's cast (if any).
        for field in &self.model.fields {
            if field.name == "id" || field.name == "tenant_id" {
                continue;
            }
            if let Some(value) = supplied.get(&field.name) {
                json_to_bind(value, field)?;
                let value = self.cast_to_db(field, value.clone())?;
                record.insert(field.name.clone(), value);
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

        // 7. Framework timestamps (only when declared on the model; skipped
        //    under `without_timestamps`, which still lets an explicitly
        //    supplied value from step 5 pass through).
        if !self.without_timestamps {
            let now = now_rfc3339();
            if self.model.field_named("created_at").is_some() {
                record.insert("created_at".into(), Json::String(now.clone()));
            }
            if self.model.field_named("updated_at").is_some() {
                record.insert("updated_at".into(), Json::String(now));
            }
        }

        // 7b. Optimistic-lock seed: a `version_column` model starts at
        //     version 1; an explicitly supplied value passes through.
        if let Some(column) = &self.model.version_column {
            if !record.contains_key(column) {
                record.insert(column.clone(), Json::from(1i64));
            }
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
            let value = self.cast_to_db(field, value.clone())?;
            record.insert(key.clone(), value);
        }
        if !self.without_timestamps
            && self.model.field_named("updated_at").is_some()
            && !record.contains_key("updated_at")
        {
            record.insert("updated_at".into(), Json::String(now_rfc3339()));
        }
        Ok(record)
    }

    /// Hidden-field policy + accessor projection for wire serialization:
    /// `sensitive` and manifest [`crate::ModelDef::hidden`] fields drop out,
    /// a non-empty manifest [`crate::ModelDef::visible`] narrows the stored
    /// fields, declared casts read back (`encrypted` opens to plaintext),
    /// and every registered accessor appends. Returns the projection or the
    /// first cast failure — corrupt sealed values fail closed.
    pub fn public_record(&self, record: &Record) -> Result<Record, AppError> {
        self.project(record, &WireOpts::default())
    }

    /// Per-call serialization overrides (Wave 3 Phase 5): chain
    /// [`WireBuilder::make_visible`], [`WireBuilder::make_hidden`] and
    /// [`WireBuilder::append`] on top of [`Self::public_record`].
    pub fn wire<'a>(&'a self, record: &'a Record) -> WireBuilder<'a> {
        WireBuilder {
            runtime: self,
            record,
            opts: WireOpts::default(),
        }
    }

    fn project(&self, record: &Record, opts: &WireOpts) -> Result<Record, AppError> {
        // `append` names must be registered accessors — fail early rather
        // than silently dropping the request.
        for name in &opts.append {
            if !self.accessors.contains_key(name) {
                return Err(AppError::validation(format!(
                    "append {name:?} is not an accessor of model {}",
                    self.model.name
                )));
            }
        }

        // 1. Read casts: only `encrypted` transforms (decrypts); failures
        //    propagate — every projection path fails closed.
        let mut decoded = Record::new();
        for (key, value) in record {
            let value = match self.model.field_named(key) {
                Some(field) => self.cast_from_db(field, value.clone())?,
                None => value.clone(),
            };
            decoded.insert(key.clone(), value);
        }

        // 2. Accessor input view: everything except `sensitive` fields
        //    (accessors read the decoded record, manifest policy only
        //    shapes the output).
        let mut view = Record::new();
        for (key, value) in &decoded {
            if !self.is_sensitive(key) {
                view.insert(key.clone(), value.clone());
            }
        }

        // 3. Stored-field projection: sensitive + manifest first, then the
        //    per-call overrides (make_hidden wins over make_visible).
        let restrict = !self.model.visible.is_empty();
        let mut out = Record::new();
        for (key, value) in &decoded {
            let mut keep = true;
            if self.is_sensitive(key) {
                keep = false;
            }
            if self.model.hidden.iter().any(|hidden| hidden == key) {
                keep = false;
            }
            if restrict && !self.model.visible.iter().any(|shown| shown == key) {
                keep = false;
            }
            if opts.force_visible.iter().any(|shown| shown == key) {
                keep = true;
            }
            if opts.force_hidden.iter().any(|hidden| hidden == key) {
                keep = false;
            }
            if keep {
                out.insert(key.clone(), value.clone());
            }
        }

        // 4. Accessors append over the projection (the `$appends` posture:
        //    manifest lists don't gate them); make_hidden removes by name.
        for (name, accessor) in &self.accessors {
            if opts.force_hidden.iter().any(|hidden| hidden == name) {
                continue;
            }
            out.insert(name.clone(), accessor(&view));
        }
        Ok(out)
    }

    fn is_sensitive(&self, key: &str) -> bool {
        self.model
            .field_named(key)
            .map(|field| field.sensitive)
            .unwrap_or(false)
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
        store.find(&self.model, id, tenant, self.include_deleted())
    }

    /// Find regardless of trash state (the `with_trashed` read): trashed,
    /// live and hard-delete rows all resolve. Missing stays `Ok(None)`.
    pub fn find_trashed(
        &self,
        store: &dyn ModelStore,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<Option<Record>, AppError> {
        store.find(&self.model, id, tenant, true)
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
        filters: Vec<FieldFilter>,
        scope_names: &[String],
        page: Option<PageReq>,
    ) -> Result<kelson_query::Page<Record>, AppError> {
        // Pipeline: global scopes FIRST, then named scopes, then validation.
        let mut filters = self.apply_global_scopes(filters);
        for name in scope_names {
            let scope = self
                .scopes
                .get(name)
                .ok_or_else(|| AppError::validation(format!("unknown scope {name:?}")))?;
            filters = scope(filters);
        }
        self.validate_predicates(&filters)?;
        let opts = ListOpts {
            page,
            include_deleted: self.include_deleted(),
            ..Default::default()
        };
        store.list(&self.model, tenant, &filters, &opts)
    }

    /// Eloquent `withTrashed()`: [`Self::list`] for one call with the
    /// soft-delete veil lifted (globals, named scopes and validation all
    /// still apply). The runtime's own veil posture is unchanged.
    pub fn with_trashed(
        &self,
        store: &dyn ModelStore,
        tenant: Option<TenantId>,
        filters: Vec<FieldFilter>,
        scope_names: &[String],
        page: Option<PageReq>,
    ) -> Result<kelson_query::Page<Record>, AppError> {
        let mut filters = self.apply_global_scopes(filters);
        for name in scope_names {
            let scope = self
                .scopes
                .get(name)
                .ok_or_else(|| AppError::validation(format!("unknown scope {name:?}")))?;
            filters = scope(filters);
        }
        self.validate_predicates(&filters)?;
        let opts = ListOpts {
            page,
            include_deleted: true,
            ..Default::default()
        };
        store.list(&self.model, tenant, &filters, &opts)
    }

    /// Eloquent `onlyTrashed()`: [`Self::with_trashed`] narrowed to trashed
    /// rows by an appended `deleted_at IS NOT NULL`. The term lands AFTER
    /// validation because `deleted_at` may be a store-level column that the
    /// manifest never declares. Requires `soft_deletes`.
    pub fn only_trashed(
        &self,
        store: &dyn ModelStore,
        tenant: Option<TenantId>,
        filters: Vec<FieldFilter>,
        scope_names: &[String],
        page: Option<PageReq>,
    ) -> Result<kelson_query::Page<Record>, AppError> {
        if !self.model.soft_deletes {
            return Err(AppError::validation(format!(
                "only_trashed requires soft_deletes on model {}",
                self.model.name
            )));
        }
        let mut filters = self.apply_global_scopes(filters);
        for name in scope_names {
            let scope = self
                .scopes
                .get(name)
                .ok_or_else(|| AppError::validation(format!("unknown scope {name:?}")))?;
            filters = scope(filters);
        }
        self.validate_predicates(&filters)?;
        filters.push(FieldFilter::Cmp {
            field: "deleted_at".into(),
            op: FilterOp::NotNull,
            value: Json::Null,
        });
        let opts = ListOpts {
            page,
            include_deleted: true,
            ..Default::default()
        };
        store.list(&self.model, tenant, &filters, &opts)
    }

    /// Fetch with full [`ListOpts`] (sort/limit/trash) — the eager-load
    /// entry point; same field validation as [`Self::list`], no scopes.
    fn list_with_opts(
        &self,
        store: &dyn ModelStore,
        tenant: Option<TenantId>,
        filters: Vec<FieldFilter>,
        opts: &ListOpts,
    ) -> Result<kelson_query::Page<Record>, AppError> {
        self.validate_predicates(&filters)?;
        store.list(&self.model, tenant, &filters, opts)
    }

    pub fn update(
        &self,
        store: &dyn ModelStore,
        id: Uuid,
        tenant: Option<TenantId>,
        changes: &Record,
    ) -> Result<Record, AppError> {
        self.update_inner(store, id, tenant, changes)
            .map(|(record, _)| record)
    }

    /// [`Self::update`] plus the [`RecordChanges`] triple (original /
    /// pending / stored) for `isDirty`/`wasChanged`-style assertions after
    /// the write. Framework auto-changes (`updated_at`, the version stamp)
    /// count as dirty — Laravel-consistent.
    pub fn update_tracked(
        &self,
        store: &dyn ModelStore,
        id: Uuid,
        tenant: Option<TenantId>,
        changes: &Record,
    ) -> Result<(Record, RecordChanges), AppError> {
        self.update_inner(store, id, tenant, changes)
    }

    /// Shared write path for `update`/`update_tracked`. With a
    /// `version_column` the store write becomes a compare-and-swap: the
    /// expected version is read from the freshly fetched row (the window
    /// between that read and the write IS the CAS scope), the prepared row
    /// is stamped `n + 1` — any caller-supplied version is overwritten —
    /// and a zero-row `update_filtered` means a concurrent writer won:
    /// [`AppError::stale_record`] (`Conflict` → 409 at the HTTP edge).
    /// Without a version column this is the plain read-merge-write path.
    fn update_inner(
        &self,
        store: &dyn ModelStore,
        id: Uuid,
        tenant: Option<TenantId>,
        changes: &Record,
    ) -> Result<(Record, RecordChanges), AppError> {
        // Fetch first: validates existence and gives observers the merge.
        let original = self.get(store, id, tenant)?;
        let mut prepared = self.prepare_update(changes)?;
        let cas = if let Some(column) = &self.model.version_column {
            let expected = original.get(column).cloned().unwrap_or(Json::Null);
            let expected_n = expected.as_i64().ok_or_else(|| {
                AppError::validation(format!(
                    "{} {id} has no integer version in {column:?} — seed the column \
                     (inserts stamp 1) before enabling version_column",
                    self.model.name
                ))
            })?;
            prepared.insert(column.clone(), Json::from(expected_n.saturating_add(1)));
            Some((
                FieldFilter::eq("id", Json::String(id.to_string())),
                FieldFilter::eq(column.clone(), expected),
            ))
        } else {
            None
        };
        let mut pending = original.clone();
        for (key, value) in &prepared {
            pending.insert(key.clone(), value.clone());
        }
        self.fire(ModelPhase::Updating, &pending);

        let stored = match cas {
            Some((id_filter, version_filter)) => {
                let rows = store.update_filtered(
                    &self.model,
                    tenant,
                    &[id_filter, version_filter],
                    &prepared,
                )?;
                rows.into_iter().next().ok_or_else(|| {
                    AppError::stale_record(format!(
                        "{} {id} was modified concurrently",
                        self.model.name
                    ))
                    .with_meta("model", &self.model.name)
                })?
            }
            None => store
                .update(&self.model, id, tenant, &prepared)?
                .ok_or_else(|| AppError::not_found(format!("{} {id}", self.model.name)))?,
        };
        self.fire(ModelPhase::Updated, &stored);
        Ok((
            stored.clone(),
            RecordChanges {
                original,
                pending,
                stored,
            },
        ))
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

    /// Whether a row carries a non-null `deleted_at` (a store-level column
    /// — it need not be declared on the manifest).
    fn is_trashed(record: &Record) -> bool {
        matches!(record.get("deleted_at"), Some(Json::String(s)) if !s.is_empty())
    }

    /// Eloquent `restore`: clear `deleted_at` on a trashed row. 404 when
    /// missing; an already-live row returns as-is (idempotent, no events).
    /// Fires `Updating` before the write and `Updated` after; a lost race
    /// re-reads the outcome (concurrent restore → its row, concurrent
    /// purge → 404). Requires `soft_deletes`.
    pub fn restore(
        &self,
        store: &dyn ModelStore,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<Record, AppError> {
        if !self.model.soft_deletes {
            return Err(AppError::validation(format!(
                "restore requires soft_deletes on model {}",
                self.model.name
            )));
        }
        let Some(current) = self.find_trashed(store, id, tenant)? else {
            return Err(AppError::not_found(format!("{} {id}", self.model.name)));
        };
        if !Self::is_trashed(&current) {
            return Ok(current); // already live — idempotent, no events
        }
        self.fire(ModelPhase::Updating, &current);
        let restored = match store.restore(&self.model, id, tenant)? {
            Some(row) => row,
            None => {
                // Lost the race: another writer restored or purged the row
                // between our read and our write — re-read the outcome.
                let row = self
                    .find_trashed(store, id, tenant)?
                    .ok_or_else(|| AppError::not_found(format!("{} {id}", self.model.name)))?;
                if Self::is_trashed(&row) {
                    return Err(AppError::conflict(format!(
                        "{} {id} changed concurrently during restore",
                        self.model.name
                    )));
                }
                row
            }
        };
        self.fire(ModelPhase::Updated, &restored);
        Ok(restored)
    }

    /// Eloquent `forceDelete`: purge the row regardless of trash state
    /// (on hard-delete models it simply purges a live row). 404 when
    /// missing; fires `Deleting`/`Deleted`. Returns the purged id.
    pub fn force_delete(
        &self,
        store: &dyn ModelStore,
        id: Uuid,
        tenant: Option<TenantId>,
    ) -> Result<Uuid, AppError> {
        let current = self
            .find_trashed(store, id, tenant)?
            .ok_or_else(|| AppError::not_found(format!("{} {id}", self.model.name)))?;
        self.fire(ModelPhase::Deleting, &current);
        store.force_delete(&self.model, id, tenant)?;
        self.fire(ModelPhase::Deleted, &current);
        Ok(id)
    }

    /// Wave 3 Phase 6 maintenance: purge every row matching the
    /// `with_prunable` predicate, `chunk` rows per store pass (clamped to
    /// the platform's 200-row page cap, matching `chunk`). Each pass lists
    /// page 1 (purged rows vanish, so the next pass sees the next batch)
    /// with `include_deleted: true` — soft-deleted rows purge hard too —
    /// sorted by `id` for a stable walk. Globals/named scopes do NOT
    /// apply: the predicate alone defines the sweep. Stops on an empty
    /// page or a pass that purged nothing (zero-progress guard).
    /// Returns the number of purged rows.
    pub fn prune(
        &self,
        store: &dyn ModelStore,
        tenant: Option<TenantId>,
        chunk: u32,
    ) -> Result<u64, AppError> {
        let predicate = self.prunable.as_ref().ok_or_else(|| {
            AppError::validation(format!(
                "model {} is not prunable (call with_prunable)",
                self.model.name
            ))
        })?;
        let per_page = chunk.clamp(1, 200);
        let mut total = 0u64;
        loop {
            let filters = predicate();
            self.validate_predicates(&filters)?;
            let opts = ListOpts {
                page: Some(PageReq {
                    page: 1,
                    per_page,
                }),
                include_deleted: true,
                sort: Some(("id".into(), false)),
                limit: None,
            };
            let page = store.list(&self.model, tenant, &filters, &opts)?;
            if page.items.is_empty() {
                break;
            }
            let mut purged = 0u64;
            for row in &page.items {
                let id = self.record_id(row)?;
                if store.force_delete(&self.model, id, tenant)? {
                    purged += 1;
                }
            }
            total += purged;
            if purged == 0 {
                break; // zero-progress pass — never spin on an unremovable row
            }
        }
        Ok(total)
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
    /// Field terms are checked against this model; existence terms were
    /// validated when built (relation exists, sub fields on the target).
    fn validate_predicates(&self, predicates: &[FieldFilter]) -> Result<(), AppError> {
        for filter in predicates {
            if let FieldFilter::Cmp { field, .. } = filter {
                if field != "id" && self.model.field_named(field).is_none() {
                    return Err(AppError::validation(format!(
                        "filter on unknown field {field:?}"
                    )));
                }
            }
        }
        Ok(())
    }

    /// Apply every registered global scope in name order (BTreeMap is
    /// deterministic), skipping suppressed ones. Named suppression and
    /// `without_global_scopes_all` both funnel through here; the function
    /// stays pure — validation is a separate step.
    fn apply_global_scopes(&self, mut filters: Vec<FieldFilter>) -> Vec<FieldFilter> {
        if self.all_globals_suppressed {
            return filters;
        }
        for (name, scope) in &self.global_scopes {
            if self.suppressed_globals.contains(name) {
                continue;
            }
            filters = scope(filters);
        }
        filters
    }

    /// Whether reads on this runtime see soft-deleted rows. Only
    /// `without_global_scopes_all` lifts the veil; named suppression and
    /// `with_trashed`/`only_trashed` (per-call) never change this posture.
    fn include_deleted(&self) -> bool {
        self.all_globals_suppressed
    }

    /// Read chokepoint for `first`/`pluck`: globals FIRST, then field
    /// validation. `chunk`/`cursor` validate raw and delegate to `list`,
    /// so globals apply exactly once there — never double-applied.
    fn prepare_filters(
        &self,
        filters: Vec<FieldFilter>,
    ) -> Result<Vec<FieldFilter>, AppError> {
        let filters = self.apply_global_scopes(filters);
        self.validate_predicates(&filters)?;
        Ok(filters)
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

    /// Strict-mode error for a matched row that lacks the requested field.
    fn missing_attribute(&self, field: &str) -> AppError {
        AppError::validation(format!(
            "missing attribute {field:?} on {} (strict mode)",
            self.model.name
        ))
    }

    /// First live row matching `predicates` (Eloquent
    /// `where(...)->first()`); `Ok(None)` on miss.
    pub fn first(
        &self,
        store: &dyn ModelStore,
        tenant: Option<TenantId>,
        predicates: &[FieldFilter],
    ) -> Result<Option<Record>, AppError> {
        let predicates = self.prepare_filters(predicates.to_vec())?;
        store.find_by(&self.model, tenant, &predicates, self.include_deleted())
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
    /// Strict mode (`prevent_accessing_missing_attributes`) errors when the
    /// matched row lacks the field instead of reporting a miss.
    pub fn value(
        &self,
        store: &dyn ModelStore,
        tenant: Option<TenantId>,
        predicates: &[FieldFilter],
        field: &str,
    ) -> Result<Option<Json>, AppError> {
        self.validate_read_field(field)?;
        let Some(row) = self.first(store, tenant, predicates)? else {
            return Ok(None);
        };
        match row.get(field) {
            Some(value) => Ok(Some(value.clone())),
            None if self.prevent_accessing_missing_attributes => {
                Err(self.missing_attribute(field))
            }
            None => Ok(None),
        }
    }

    /// Eloquent `pluck`: `field` values across every match in scan order
    /// (soft-deleted rows excluded). Strict mode
    /// (`prevent_accessing_missing_attributes`) errors when any matched row
    /// lacks the field instead of yielding `null`.
    /// Divergence from Laravel: the port fetches full rows — there is no
    /// column projection yet.
    pub fn pluck(
        &self,
        store: &dyn ModelStore,
        tenant: Option<TenantId>,
        predicates: &[FieldFilter],
        field: &str,
    ) -> Result<Vec<Json>, AppError> {
        self.validate_read_field(field)?;
        let predicates = self.prepare_filters(predicates.to_vec())?;
        let opts = ListOpts {
            include_deleted: self.include_deleted(),
            ..Default::default()
        };
        let page = store.list(&self.model, tenant, &predicates, &opts)?;
        if self.prevent_accessing_missing_attributes
            && page.items.iter().any(|row| !row.contains_key(field))
        {
            return Err(self.missing_attribute(field));
        }
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
    /// `Ok(None)` when the row was removed meanwhile, or soft-deleted
    /// unless the veil is lifted via `without_global_scopes_all`.
    pub fn fresh(
        &self,
        store: &dyn ModelStore,
        record: &Record,
        tenant: Option<TenantId>,
    ) -> Result<Option<Record>, AppError> {
        let id = self.record_id(record)?;
        store.find(&self.model, id, tenant, self.include_deleted())
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
                if self.prevent_silently_discarding_attributes {
                    return Err(AppError::validation(format!(
                        "field {key:?} would be silently discarded by push on {} (strict mode)",
                        self.model.name
                    )));
                }
                continue;
            }
            changes.insert(key.clone(), value.clone());
        }

        // 2. Save self: update the persisted row, else insert as new.
        let self_id = self.record_id(&attrs).ok();
        let persisted = match self_id {
            Some(id) => store
                .find(&self.model, id, tenant, self.include_deleted())?
                .is_some(),
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
        if self.prevent_lazy_loading {
            return Err(AppError::new(
                codes::LAZY_LOAD_PREVENTED,
                ErrorCategory::Validation,
                Severity::Low,
                Retryability::NotRetryable,
                format!(
                    "lazy loading of relation {relation_name:?} on {} is prevented; declare it \
                     with with() at query time",
                    self.model.name
                ),
            ));
        }
        self.load_relation_constrained(
            store,
            relation_name,
            parents,
            tenant,
            &EagerConstraint::default(),
        )
    }

    /// One constrained eager load — the primitive behind [`Self::load_relation`]
    /// and each level of [`Self::with`].
    fn load_relation_constrained(
        &self,
        store: &dyn ModelStore,
        relation_name: &str,
        parents: &mut [Record],
        tenant: Option<TenantId>,
        constraint: &EagerConstraint,
    ) -> Result<(), AppError> {
        // MorphTo resolves per stored TYPE — it branches before the
        // single-target path.
        if let Some(relation) = self
            .model
            .relations
            .iter()
            .find(|r| r.name == relation_name && r.kind == RelationKind::MorphTo)
        {
            return self.load_morph_to(store, relation, parents, tenant, constraint);
        }
        let (relation, target, _) = self.resolve_relation(relation_name)?;
        let related = self.collect_resolved(store, &relation, &target, parents, tenant, constraint)?;
        let single = matches!(
            relation.kind,
            RelationKind::BelongsTo | RelationKind::HasOne
        );
        for (parent, rows) in parents.iter_mut().zip(related) {
            let attached = if single {
                rows.first()
                    .cloned()
                    .map(Json::Object)
                    .unwrap_or(Json::Null)
            } else {
                Json::Array(rows.into_iter().map(Json::Object).collect())
            };
            parent.insert(relation.name.clone(), attached);
        }
        Ok(())
    }

    /// Eloquent `morphTo` eager load: group parents by their stored type,
    /// resolve each type through the morph map (unmapped = model name),
    /// fetch each distinct type in ONE query (at most one per type, as in
    /// Eloquent), attach `Object`/`Null`.
    fn load_morph_to(
        &self,
        store: &dyn ModelStore,
        relation: &RelationDef,
        parents: &mut [Record],
        tenant: Option<TenantId>,
        constraint: &EagerConstraint,
    ) -> Result<(), AppError> {
        if !constraint.filters.is_empty()
            || constraint.sort.is_some()
            || constraint.limit.is_some()
        {
            return Err(AppError::validation(format!(
                "morph-to relation {:?} cannot take eager constraints (the target model varies per row)",
                relation.name
            )));
        }
        let registry = self
            .registry
            .clone()
            .ok_or_else(|| AppError::validation("relation loading needs a model registry"))?;
        let type_col = relation.morph_type.clone().ok_or_else(|| {
            AppError::validation(format!(
                "morph-to relation {:?} needs a type column",
                relation.name
            ))
        })?;
        let id_col = &relation.foreign_key;
        let owner_key = relation.target_key.clone().unwrap_or_else(|| "id".into());
        for parent in parents.iter_mut() {
            parent.insert(relation.name.clone(), Json::Null);
        }
        let mut by_type: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, parent) in parents.iter().enumerate() {
            let Some(type_value) = parent.get(&type_col).and_then(|v| v.as_str()) else {
                continue;
            };
            if let Some(id) = parent.get(id_col) {
                if !id.is_null() {
                    by_type.entry(type_value.to_string()).or_default().push(i);
                }
            }
        }
        for (type_value, indices) in by_type {
            let model_name = registry
                .morph_map
                .get(&type_value)
                .cloned()
                .unwrap_or_else(|| type_value.clone());
            let target = registry.get(&model_name).cloned().ok_or_else(|| {
                AppError::validation(format!(
                    "morph type {type_value:?} resolves to unknown model {model_name:?}"
                ))
            })?;
            let target_rt = ModelRuntime::new(target).with_registry(registry.clone());
            let ids: Vec<Json> = indices
                .iter()
                .map(|&i| parents[i].get(id_col).cloned().unwrap_or(Json::Null))
                .collect();
            let loaded = fetch_keyed(
                store,
                &target_rt,
                &owner_key,
                ids,
                tenant,
                &EagerConstraint::default(),
            )?;
            for i in indices {
                let id = parents[i].get(id_col).cloned().unwrap_or(Json::Null);
                let attached = loaded
                    .get(&id.to_string())
                    .and_then(|rows| rows.first().cloned())
                    .map(Json::Object)
                    .unwrap_or(Json::Null);
                parents[i].insert(relation.name.clone(), attached);
            }
        }
        Ok(())
    }

    /// Eloquent `with('post.comments.author')`: eager-load a nested relation
    /// path level by level — each level stays at most 2 store queries (no
    /// N+1 at any depth).
    pub fn with(
        &self,
        store: &dyn ModelStore,
        parents: &mut [Record],
        tenant: Option<TenantId>,
        path: &str,
    ) -> Result<(), AppError> {
        self.with_constrained(store, parents, tenant, path, &mut |_, _, _| {})
    }

    /// `with` + per-level constraint closures: `constraints` runs once per
    /// level with `(depth, relation name, &mut constraint)` BEFORE that
    /// level's fetch, so each level can filter/sort/limit independently.
    pub fn with_constrained(
        &self,
        store: &dyn ModelStore,
        parents: &mut [Record],
        tenant: Option<TenantId>,
        path: &str,
        constraints: &mut dyn FnMut(usize, &str, &mut EagerConstraint),
    ) -> Result<(), AppError> {
        let parts: Vec<&str> = path
            .split('.')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .collect();
        if parts.is_empty() {
            return Err(AppError::validation(format!(
                "with() needs a dotted relation path, got {path:?}"
            )));
        }
        self.with_parts(store, parents, tenant, &parts, constraints, 0)
    }

    /// Recursive descent for [`Self::with_constrained`]: load `parts[level]`
    /// onto `parents`, then clone the attached children out, load the next
    /// level onto them, and write the enriched children back.
    fn with_parts(
        &self,
        store: &dyn ModelStore,
        parents: &mut [Record],
        tenant: Option<TenantId>,
        parts: &[&str],
        constraints: &mut dyn FnMut(usize, &str, &mut EagerConstraint),
        level: usize,
    ) -> Result<(), AppError> {
        let relation = parts[level];
        let mut constraint = EagerConstraint::default();
        constraints(level, relation, &mut constraint);
        self.load_relation_constrained(store, relation, parents, tenant, &constraint)?;
        if level + 1 >= parts.len() {
            return Ok(());
        }
        // Pull this level's children out (arrays keep their slots), recurse
        // on the CHILD model's runtime (the next part is its relation), then
        // write the enriched children back in the same shape.
        let mut children: Vec<Record> = Vec::new();
        let mut owners: Vec<(usize, Option<usize>)> = Vec::new();
        for (i, parent) in parents.iter().enumerate() {
            match parent.get(relation) {
                Some(Json::Array(items)) => {
                    for (j, item) in items.iter().enumerate() {
                        if let Json::Object(obj) = item {
                            owners.push((i, Some(j)));
                            children.push(obj.clone());
                        }
                    }
                }
                Some(Json::Object(obj)) => {
                    owners.push((i, None));
                    children.push(obj.clone());
                }
                _ => {}
            }
        }
        let (_, target, registry) = self.resolve_relation(relation)?;
        let child_rt = ModelRuntime::new(target).with_registry(registry);
        child_rt.with_parts(store, &mut children, tenant, parts, constraints, level + 1)?;
        let mut rebuilt: BTreeMap<usize, (Option<Json>, Vec<Json>)> = BTreeMap::new();
        for ((i, slot), child) in owners.into_iter().zip(children.into_iter()) {
            let entry = rebuilt.entry(i).or_insert((None, Vec::new()));
            match slot {
                Some(_) => entry.1.push(Json::Object(child)),
                None => entry.0 = Some(Json::Object(child)),
            }
        }
        for (i, (object, items)) in rebuilt {
            let attached = object.unwrap_or(Json::Array(items));
            parents[i].insert(relation.to_string(), attached);
        }
        Ok(())
    }

    // ------------------------------------------------------------- pivots

    /// Resolve a ManyToMany relation and its pivot model (registered
    /// `through`, or synthesized from `pivot_table`).
    fn resolve_pivot(
        &self,
        relation_name: &str,
    ) -> Result<(RelationDef, ModelDef, ModelDef), AppError> {
        let (relation, target, registry) = self.resolve_relation(relation_name)?;
        if !matches!(
            relation.kind,
            RelationKind::ManyToMany | RelationKind::MorphToMany
        ) {
            return Err(AppError::validation(format!(
                "relation {relation_name:?} on {} is not many-to-many",
                self.model.name
            )));
        }
        let pivot = match &relation.through {
            Some(name) => registry.get(name).cloned().ok_or_else(|| {
                AppError::validation(format!("unknown pivot model {name:?}"))
            })?,
            None => synthesize_pivot(&relation)?,
        };
        Ok((relation, target, pivot))
    }

    /// The pivot FK column pointing at the target model.
    fn pivot_target_key(relation: &RelationDef) -> Result<String, AppError> {
        relation.target_key.clone().ok_or_else(|| {
            AppError::validation(format!(
                "relation {:?} needs a pivot target key",
                relation.name
            ))
        })
    }

    /// Target ids currently linked to `parent_id` on the pivot (pinned to
    /// this owner's morph type for morph relations).
    fn pivot_links(
        &self,
        store: &dyn ModelStore,
        pivot: &ModelDef,
        relation: &RelationDef,
        parent_id: &Json,
        target_key: &str,
        tenant: Option<TenantId>,
    ) -> Result<Vec<Json>, AppError> {
        let mut filters = vec![FieldFilter::eq(&relation.foreign_key, parent_id.clone())];
        if let Some(type_col) = &relation.morph_type {
            filters.push(FieldFilter::eq(
                type_col.clone(),
                Json::String(self.morph_alias(relation)?),
            ));
        }
        let rows = store
            .list(pivot, tenant, &filters, &ListOpts::default())?
            .items;
        Ok(rows
            .iter()
            .filter_map(|r| r.get(target_key).cloned())
            .filter(|v| !v.is_null())
            .collect())
    }

    /// Eloquent `attach`: link `target_ids` to `parent_id` on the pivot —
    /// pairs already present are left alone (idempotent), so the return is
    /// the number of pivot rows added.
    pub fn attach(
        &self,
        store: &dyn ModelStore,
        relation_name: &str,
        parent_id: Json,
        target_ids: Vec<Json>,
        tenant: Option<TenantId>,
    ) -> Result<u64, AppError> {
        let (relation, _target, pivot) = self.resolve_pivot(relation_name)?;
        let target_key = Self::pivot_target_key(&relation)?;
        let current =
            self.pivot_links(store, &pivot, &relation, &parent_id, &target_key, tenant)?;
        // Morph pivots carry this owner's type value alongside the FK.
        let morph_value = if relation.morph_type.is_some() {
            Some(self.morph_alias(&relation)?)
        } else {
            None
        };
        let pivot_rt = ModelRuntime::new(pivot);
        let mut inserted = 0u64;
        for id in target_ids {
            if id.is_null() || current.contains(&id) {
                continue;
            }
            let mut row = Record::new();
            row.insert(relation.foreign_key.clone(), parent_id.clone());
            row.insert(target_key.clone(), id);
            if let (Some(type_col), Some(alias)) = (&relation.morph_type, &morph_value) {
                row.insert(type_col.clone(), Json::String(alias.clone()));
            }
            pivot_rt.insert(store, &row, tenant)?;
            inserted += 1;
        }
        Ok(inserted)
    }

    /// Eloquent `detach`: unlink `target_ids` from `parent_id` (empty list
    /// = every link for that parent) — returns the pivot rows removed.
    pub fn detach(
        &self,
        store: &dyn ModelStore,
        relation_name: &str,
        parent_id: Json,
        target_ids: Vec<Json>,
        tenant: Option<TenantId>,
    ) -> Result<u64, AppError> {
        let (relation, _target, pivot) = self.resolve_pivot(relation_name)?;
        let mut filters = vec![FieldFilter::eq(&relation.foreign_key, parent_id)];
        if let Some(type_col) = &relation.morph_type {
            filters.push(FieldFilter::eq(
                type_col.clone(),
                Json::String(self.morph_alias(&relation)?),
            ));
        }
        if !target_ids.is_empty() {
            filters.push(FieldFilter::is_in(
                Self::pivot_target_key(&relation)?,
                target_ids,
            ));
        }
        let rows = store.delete_filtered(&pivot, tenant, &filters)?;
        Ok(rows.len() as u64)
    }

    /// Eloquent `sync`: make the pivot links EXACTLY `target_ids` — missing
    /// pairs attached, stale pairs detached; returns both sides.
    pub fn sync(
        &self,
        store: &dyn ModelStore,
        relation_name: &str,
        parent_id: Json,
        target_ids: Vec<Json>,
        tenant: Option<TenantId>,
    ) -> Result<PivotSync, AppError> {
        let (relation, _target, pivot) = self.resolve_pivot(relation_name)?;
        let target_key = Self::pivot_target_key(&relation)?;
        let current =
            self.pivot_links(store, &pivot, &relation, &parent_id, &target_key, tenant)?;
        let mut desired: Vec<Json> = Vec::new();
        for id in target_ids {
            if !id.is_null() && !desired.contains(&id) {
                desired.push(id);
            }
        }
        let attached: Vec<Json> = desired
            .iter()
            .filter(|id| !current.contains(id))
            .cloned()
            .collect();
        let detached: Vec<Json> = current
            .iter()
            .filter(|id| !desired.contains(id))
            .cloned()
            .collect();
        if !attached.is_empty() {
            self.attach(
                store,
                relation_name,
                parent_id.clone(),
                attached.clone(),
                tenant,
            )?;
        }
        if !detached.is_empty() {
            self.detach(store, relation_name, parent_id, detached.clone(), tenant)?;
        }
        Ok(PivotSync { attached, detached })
    }

    /// Eloquent `toggle`: flip each listed id — unlinked pairs attached,
    /// linked pairs detached.
    pub fn toggle(
        &self,
        store: &dyn ModelStore,
        relation_name: &str,
        parent_id: Json,
        target_ids: Vec<Json>,
        tenant: Option<TenantId>,
    ) -> Result<PivotSync, AppError> {
        let (relation, _target, pivot) = self.resolve_pivot(relation_name)?;
        let target_key = Self::pivot_target_key(&relation)?;
        let current =
            self.pivot_links(store, &pivot, &relation, &parent_id, &target_key, tenant)?;
        let mut attached: Vec<Json> = Vec::new();
        let mut detached: Vec<Json> = Vec::new();
        for id in target_ids {
            if id.is_null() {
                continue;
            }
            if current.contains(&id) {
                detached.push(id);
            } else {
                attached.push(id);
            }
        }
        if !attached.is_empty() {
            self.attach(
                store,
                relation_name,
                parent_id.clone(),
                attached.clone(),
                tenant,
            )?;
        }
        if !detached.is_empty() {
            self.detach(store, relation_name, parent_id, detached.clone(), tenant)?;
        }
        Ok(PivotSync { attached, detached })
    }

    /// Eloquent `updateExistingPivot`: apply `changes` to the single pivot
    /// row linking `parent_id` to `target_id`.
    pub fn update_existing_pivot(
        &self,
        store: &dyn ModelStore,
        relation_name: &str,
        parent_id: Json,
        target_id: Json,
        changes: &Record,
        tenant: Option<TenantId>,
    ) -> Result<Record, AppError> {
        let (relation, _target, pivot) = self.resolve_pivot(relation_name)?;
        let target_key = Self::pivot_target_key(&relation)?;
        let mut filters = vec![
            FieldFilter::eq(&relation.foreign_key, parent_id),
            FieldFilter::eq(target_key, target_id.clone()),
        ];
        if let Some(type_col) = &relation.morph_type {
            filters.push(FieldFilter::eq(
                type_col.clone(),
                Json::String(self.morph_alias(&relation)?),
            ));
        }
        let mut rows = store.update_filtered(&pivot, tenant, &filters, changes)?;
        rows.pop()
            .ok_or_else(|| AppError::not_found(format!("pivot row linking {target_id}")))
    }

    /// One eager pass over a resolved relation: query the related rows (at
    /// most 2 queries) and return them grouped per parent, aligned with
    /// `parents`. Both [`Self::load_relation`] and the eager aggregate
    /// family ([`Self::with_aggregate`]) attach/fold from this same shape.
    fn collect_resolved(
        &self,
        store: &dyn ModelStore,
        relation: &RelationDef,
        target: &ModelDef,
        parents: &[Record],
        tenant: Option<TenantId>,
        constraint: &EagerConstraint,
    ) -> Result<Vec<Vec<Record>>, AppError> {
        // Callers resolve through [`Self::resolve_relation`], which already
        // requires the registry — reuse it rather than threading a 8th arg.
        let registry = self
            .registry
            .clone()
            .ok_or_else(|| AppError::validation("relation loading needs a model registry"))?;
        let target_rt = ModelRuntime::new(target.clone()).with_registry(registry.clone());
        Ok(match relation.kind {
            RelationKind::BelongsTo => {
                let target_key = relation.target_key.clone().unwrap_or_else(|| "id".into());
                let ids = distinct_values(parents, &relation.foreign_key);
                let loaded = fetch_keyed(store, &target_rt, &target_key, ids, tenant, constraint)?;
                parents
                    .iter()
                    .map(|parent| {
                        parent
                            .get(&relation.foreign_key)
                            .and_then(|k| loaded.get(&k.to_string()))
                            .and_then(|rows| rows.first().cloned())
                            .into_iter()
                            .collect()
                    })
                    .collect()
            }
            RelationKind::HasMany | RelationKind::HasOne | RelationKind::MorphMany => {
                let parent_ids = distinct_values(parents, "id");
                let mut effective = constraint.clone();
                if relation.kind == RelationKind::MorphMany {
                    // Pin the fetch to THIS owner's morph type — the id
                    // column alone would match sibling owners' rows.
                    let type_col = relation.morph_type.clone().ok_or_else(|| {
                        AppError::validation(format!(
                            "morph relation {:?} needs a type column",
                            relation.name
                        ))
                    })?;
                    let alias = self.morph_alias(relation)?;
                    effective
                        .filters
                        .push(FieldFilter::eq(type_col, Json::String(alias)));
                }
                let by_fk = fetch_keyed(
                    store,
                    &target_rt,
                    &relation.foreign_key,
                    parent_ids,
                    tenant,
                    &effective,
                )?;
                parents
                    .iter()
                    .map(|parent| {
                        parent
                            .get("id")
                            .and_then(|k| by_fk.get(&k.to_string()))
                            .cloned()
                            .unwrap_or_default()
                    })
                    .collect()
            }
            RelationKind::ManyThrough
            | RelationKind::ManyToMany
            | RelationKind::MorphToMany => {
                // Query 1: join/pivot rows for all parent ids; Query 2:
                // target rows for the far-side ids found on the join.
                let through = match &relation.through {
                    Some(name) => registry.get(name).cloned().ok_or_else(|| {
                        AppError::validation(format!("unknown join model {name:?}"))
                    })?,
                    None => synthesize_pivot(relation)?,
                };
                // FK on the join table pointing at the target model.
                let far_fk = relation.target_key.clone().unwrap_or_else(|| {
                    format!(
                        "{}_id",
                        target.table.to_lowercase().trim_end_matches('s')
                    )
                });
                let parent_ids = distinct_values(parents, "id");
                let mut join_filters =
                    vec![FieldFilter::is_in(&relation.foreign_key, parent_ids)];
                if relation.kind == RelationKind::MorphToMany {
                    // Pin the pivot rows to THIS owner's morph type.
                    let type_col = relation.morph_type.clone().ok_or_else(|| {
                        AppError::validation(format!(
                            "morph relation {:?} needs a type column",
                            relation.name
                        ))
                    })?;
                    let alias = self.morph_alias(relation)?;
                    join_filters.push(FieldFilter::eq(type_col, Json::String(alias)));
                }
                let join_rows = store
                    .list(&through, tenant, &join_filters, &ListOpts::default())?
                    .items;
                let far_ids = distinct_values(&join_rows, &far_fk);
                let loaded = fetch_keyed(store, &target_rt, "id", far_ids, tenant, constraint)?;
                parents
                    .iter()
                    .map(|parent| {
                        let Some(pid) = parent.get("id") else {
                            return Vec::new();
                        };
                        let mut rows: Vec<Record> = join_rows
                            .iter()
                            .filter(|r| r.get(&relation.foreign_key) == Some(pid))
                            .filter_map(|r| {
                                r.get(&far_fk)
                                    .and_then(|k| loaded.get(&k.to_string()))
                                    .and_then(|rows| rows.first().cloned())
                            })
                            .collect();
                        // Assembly follows join order — honor the level's
                        // sort once rows are grouped per parent.
                        if let Some((field, desc)) = &constraint.sort {
                            sort_records(&mut rows, field, *desc);
                        }
                        rows
                    })
                    .collect()
            }
            RelationKind::MorphTo => {
                // Defensive: load_relation_constrained branches before
                // single-target resolution.
                return Err(AppError::validation(format!(
                    "morph-to relation {:?} loads per stored type, not through one target",
                    relation.name
                )));
            }
        })
    }

    // ------------------------------------------------------- eager aggregates

    /// Eloquent `withCount`/`withSum`/`withMin`/`withMax`/`withAvg`:
    /// attach one aggregate of the related rows to each parent under
    /// `<relation>_count` (COUNT) or `<relation>_<op>_<field>` — one eager
    /// fetch per relation, never N+1. Landing note: the value is folded at
    /// runtime from the same related rows `load_relation` would attach
    /// (documented divergence — no inlined `SELECT` subquery); empty sets
    /// fold to `0` for COUNT and SQL `NULL` for the rest, on both stores.
    pub fn with_aggregate(
        &self,
        store: &dyn ModelStore,
        relation_name: &str,
        parents: &mut [Record],
        tenant: Option<TenantId>,
        op: &AggregateOp,
    ) -> Result<(), AppError> {
        let (relation, target, _) = self.resolve_relation(relation_name)?;
        validate_aggregate(&target, op)?;
        if parents.is_empty() {
            return Ok(());
        }
        let related = self.collect_resolved(
            store,
            &relation,
            &target,
            parents,
            tenant,
            &EagerConstraint::default(),
        )?;
        let alias = aggregate_alias(relation_name, op);
        for (parent, rows) in parents.iter_mut().zip(related) {
            let value = match op {
                AggregateOp::Count => Json::from(rows.len() as u64),
                other => {
                    let name = other
                        .field()
                        .expect("non-count aggregate carries a field");
                    let values: Vec<&Json> = rows
                        .iter()
                        .filter_map(|r| r.get(name))
                        .filter(|v| !matches!(v, Json::Null))
                        .collect();
                    fold_aggregate_values(&values, name, other)?
                }
            };
            parent.insert(alias.clone(), value);
        }
        Ok(())
    }

    /// Eloquent `withCount('relation')` -> `<relation>_count`.
    pub fn with_count(
        &self,
        store: &dyn ModelStore,
        relation_name: &str,
        parents: &mut [Record],
        tenant: Option<TenantId>,
    ) -> Result<(), AppError> {
        self.with_aggregate(store, relation_name, parents, tenant, &AggregateOp::Count)
    }

    /// Eloquent `withSum('relation', 'field')` -> `<relation>_sum_<field>`.
    pub fn with_sum(
        &self,
        store: &dyn ModelStore,
        relation_name: &str,
        parents: &mut [Record],
        tenant: Option<TenantId>,
        field: &str,
    ) -> Result<(), AppError> {
        self.with_aggregate(
            store,
            relation_name,
            parents,
            tenant,
            &AggregateOp::Sum(field.into()),
        )
    }

    /// Eloquent `withAvg` -> `<relation>_avg_<field>`.
    pub fn with_avg(
        &self,
        store: &dyn ModelStore,
        relation_name: &str,
        parents: &mut [Record],
        tenant: Option<TenantId>,
        field: &str,
    ) -> Result<(), AppError> {
        self.with_aggregate(
            store,
            relation_name,
            parents,
            tenant,
            &AggregateOp::Avg(field.into()),
        )
    }

    /// Eloquent `withMin` -> `<relation>_min_<field>`.
    pub fn with_min(
        &self,
        store: &dyn ModelStore,
        relation_name: &str,
        parents: &mut [Record],
        tenant: Option<TenantId>,
        field: &str,
    ) -> Result<(), AppError> {
        self.with_aggregate(
            store,
            relation_name,
            parents,
            tenant,
            &AggregateOp::Min(field.into()),
        )
    }

    /// Eloquent `withMax` -> `<relation>_max_<field>`.
    pub fn with_max(
        &self,
        store: &dyn ModelStore,
        relation_name: &str,
        parents: &mut [Record],
        tenant: Option<TenantId>,
        field: &str,
    ) -> Result<(), AppError> {
        self.with_aggregate(
            store,
            relation_name,
            parents,
            tenant,
            &AggregateOp::Max(field.into()),
        )
    }

    // ---------------------------------------------------- existence filters

    /// Resolve a declared relation, its target model, and the registry
    /// (shared by filter builds, eager loading, and aggregates).
    fn resolve_relation(
        &self,
        relation_name: &str,
    ) -> Result<(RelationDef, ModelDef, Arc<ModelRegistry>), AppError> {
        let registry = self
            .registry
            .clone()
            .ok_or_else(|| AppError::validation("relation access needs a model registry"))?;
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
        if relation.kind == RelationKind::MorphTo {
            return Err(AppError::validation(format!(
                "morph-to relation {relation_name:?} has no single target model \
                 (loads and filters resolve per stored type)"
            )));
        }
        let target = registry.get(&relation.target).cloned().ok_or_else(|| {
            AppError::validation(format!(
                "relation {} targets unknown model {}",
                relation.name, relation.target
            ))
        })?;
        Ok((relation, target, registry))
    }

    /// Resolve a declared relation and its target model (filter builds need
    /// the target's table and posture, not the registry).
    fn filter_relation(&self, relation_name: &str) -> Result<(RelationDef, ModelDef), AppError> {
        let (relation, target, _registry) = self.resolve_relation(relation_name)?;
        Ok((relation, target))
    }

    /// The morph-type value THIS model stores in `relation.morph_type`:
    /// the registered morph-map alias for this model, else the model name
    /// (Laravel's default with no morph map).
    fn morph_alias(&self, relation: &RelationDef) -> Result<String, AppError> {
        if relation.morph_type.is_none() {
            return Err(AppError::validation(format!(
                "relation {:?} needs a morph type column",
                relation.name
            )));
        }
        let registry = self
            .registry
            .clone()
            .ok_or_else(|| AppError::validation("morph relations need a model registry"))?;
        Ok(registry
            .morph_map
            .iter()
            .find(|(_, model)| model.as_str() == self.model.name)
            .map(|(alias, _)| alias.clone())
            .unwrap_or_else(|| self.model.name.clone()))
    }

    /// Eloquent `has`: keep parents whose relation resolves to at least one
    /// related row — a correlated `EXISTS` over the relation's key columns.
    /// `sub_filters` run against the TARGET model's rows (comparison terms
    /// only, e.g. [`FieldFilter::eq`]).
    pub fn has(
        &self,
        relation_name: &str,
        sub_filters: Vec<FieldFilter>,
    ) -> Result<FieldFilter, AppError> {
        self.exists_filter(relation_name, false, false, sub_filters)
    }

    /// Eloquent `doesntHave`: `NOT EXISTS`.
    pub fn doesnt_have(
        &self,
        relation_name: &str,
        sub_filters: Vec<FieldFilter>,
    ) -> Result<FieldFilter, AppError> {
        self.exists_filter(relation_name, true, false, sub_filters)
    }

    /// Eloquent `orWhereHas`: `EXISTS` OR-joined to the preceding term in
    /// the filter list (SQL precedence still groups adjacent ANDs first —
    /// both stores render/evaluate identically).
    pub fn or_where_has(
        &self,
        relation_name: &str,
        sub_filters: Vec<FieldFilter>,
    ) -> Result<FieldFilter, AppError> {
        self.exists_filter(relation_name, false, true, sub_filters)
    }

    /// Eloquent `whereRelation`: the related row's `field` equals `value`.
    pub fn where_relation(
        &self,
        relation_name: &str,
        field: &str,
        value: impl Into<Json>,
    ) -> Result<FieldFilter, AppError> {
        self.has(relation_name, vec![FieldFilter::eq(field, value)])
    }

    /// Eloquent `whereDoesntHaveRelation`: `NOT EXISTS` with the condition.
    pub fn where_doesnt_have_relation(
        &self,
        relation_name: &str,
        field: &str,
        value: impl Into<Json>,
    ) -> Result<FieldFilter, AppError> {
        self.doesnt_have(relation_name, vec![FieldFilter::eq(field, value)])
    }

    /// Assemble the correlated existence term: key columns from the relation
    /// kind, sub conditions validated against the target model, tenant/soft
    /// posture carried as flags (the stores apply them — they hold no
    /// registry). Only direct (single-join) kinds correlate here; join-walk
    /// and polymorphic kinds belong to eager loading.
    fn exists_filter(
        &self,
        relation_name: &str,
        negated: bool,
        or: bool,
        sub_filters: Vec<FieldFilter>,
    ) -> Result<FieldFilter, AppError> {
        let (relation, target) = self.filter_relation(relation_name)?;
        for filter in &sub_filters {
            match filter {
                FieldFilter::Cmp { field, .. } => {
                    if field != "id" && target.field_named(field).is_none() {
                        return Err(AppError::validation(format!(
                            "filter on unknown field {field:?} on {}",
                            target.name
                        )));
                    }
                }
                FieldFilter::Exists(_) => {
                    return Err(AppError::validation(
                        "relation filters cannot nest inside a relation filter (not supported)",
                    ));
                }
            }
        }
        let (sub_table, sub_key, outer_key, injected) = match relation.kind {
            RelationKind::BelongsTo => (
                target.table.clone(),
                relation
                    .target_key
                    .clone()
                    .unwrap_or_else(|| "id".into()),
                relation.foreign_key.clone(),
                None,
            ),
            RelationKind::HasMany | RelationKind::HasOne => (
                target.table.clone(),
                relation.foreign_key.clone(),
                "id".into(),
                None,
            ),
            RelationKind::MorphMany => {
                let type_col = relation.morph_type.clone().ok_or_else(|| {
                    AppError::validation(format!(
                        "morph relation {relation_name:?} needs a type column"
                    ))
                })?;
                // Pin the subquery to THIS owner's morph type — the
                // correlated id alone would match sibling owners' rows.
                (
                    target.table.clone(),
                    relation.foreign_key.clone(),
                    "id".into(),
                    Some((type_col, self.morph_alias(&relation)?)),
                )
            }
            other => {
                return Err(AppError::validation(format!(
                    "existence filter unsupported for {other:?} relation {relation_name:?} \
                     (only BelongsTo/HasMany/HasOne/MorphMany correlate directly)"
                )));
            }
        };
        let mut sub_filters = sub_filters;
        if let Some((col, alias)) = injected {
            sub_filters.push(FieldFilter::eq(col, Json::String(alias)));
        }
        Ok(FieldFilter::Exists(ExistsFilter {
            negated,
            or,
            sub_table,
            sub_key,
            outer_table: self.model.table.clone(),
            outer_key,
            sub_tenant_scoped: target.tenant_scoped,
            sub_soft_deletes: target.soft_deletes,
            filters: sub_filters,
        }))
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

/// Laravel's eager-aggregate alias: `withCount('posts')` -> `posts_count`,
/// `withSum('posts', 'votes')` -> `posts_sum_votes`.
pub fn aggregate_alias(relation: &str, op: &AggregateOp) -> String {
    match op {
        AggregateOp::Count => format!("{relation}_count"),
        AggregateOp::Sum(f) => format!("{relation}_sum_{f}"),
        AggregateOp::Avg(f) => format!("{relation}_avg_{f}"),
        AggregateOp::Min(f) => format!("{relation}_min_{f}"),
        AggregateOp::Max(f) => format!("{relation}_max_{f}"),
    }
}

/// Per-level constraint for nested eager paths — `filters`/`sort`/`limit`
/// applied to ONE relation level's fetch (Laravel's
/// `with(['a.b' => fn ($q) => $q->where(...)])` closure constraints, without
/// the query-builder object). `filters` land in the store WHERE (both
/// backends), `sort` orders the target fetch (assembly order for
/// many_through is re-sorted too), and `limit` caps the fetch — the eager
/// subquery limit, applied to the whole fetch like Laravel's documented
/// behavior, not per parent.
#[derive(Debug, Clone, Default)]
pub struct EagerConstraint {
    pub filters: Vec<FieldFilter>,
    pub sort: Option<(String, bool)>,
    pub limit: Option<u32>,
}

/// Result of [`ModelRuntime::sync`] / [`ModelRuntime::toggle`]: the pivot
/// rows added and removed (Laravel's `[attached => [...], detached => [...]]`
/// map, counts omitted).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PivotSync {
    pub attached: Vec<Json>,
    pub detached: Vec<Json>,
}

/// Synthesize the pivot [`ModelDef`] for a ManyToMany relation declared
/// without a registered join model: `pivot_table` + id + both FK columns.
fn synthesize_pivot(relation: &RelationDef) -> Result<ModelDef, AppError> {
    let table = relation.pivot_table.clone().ok_or_else(|| {
        AppError::validation(format!(
            "relation {:?} has neither a join model nor a pivot table",
            relation.name
        ))
    })?;
    let target_key = relation.target_key.clone().ok_or_else(|| {
        AppError::validation(format!(
            "relation {:?} needs a pivot target key",
            relation.name
        ))
    })?;
    Ok({
        let mut model = ModelDef::new(table.clone(), table)
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new(&relation.foreign_key, FieldType::Uuid))
            .field(FieldDef::new(&target_key, FieldType::Uuid));
        if let Some(type_col) = &relation.morph_type {
            model = model.field(FieldDef::new(type_col, FieldType::Text));
        }
        model
    })
}

/// One unbounded IN query against `target`, grouped by the stringified
/// `column` value (every matching row is kept — a has_many key maps to ALL
/// of its children, never just one).
fn fetch_keyed(
    store: &dyn ModelStore,
    target: &ModelRuntime,
    column: &str,
    ids: Vec<Json>,
    tenant: Option<TenantId>,
    constraint: &EagerConstraint,
) -> Result<BTreeMap<String, Vec<Record>>, AppError> {
    let mut out: BTreeMap<String, Vec<Record>> = BTreeMap::new();
    if ids.is_empty() {
        return Ok(out);
    }
    let mut filters = vec![FieldFilter::is_in(column, ids)];
    filters.extend(constraint.filters.iter().cloned());
    let opts = ListOpts {
        page: None,
        include_deleted: false,
        sort: constraint.sort.clone(),
        limit: constraint.limit,
    };
    let page = target.list_with_opts(store, tenant, filters, &opts)?;
    for row in page.items {
        if let Some(key) = row.get(column).cloned() {
            if !key.is_null() {
                out.entry(key.to_string()).or_default().push(row);
            }
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

/// Field-aware twin of [`record_to_columns`] for stores persisting a
/// [`ModelDef`] (`PgModelStore` / `MySqlModelStore`).
///
/// A declared `Text`/`TextLarge`/`Date` field stores its **raw string**.
/// The field-blind sniffing in [`json_value_to_bind`] promotes
/// `"2026-12-01T10:00:00Z"` (>= 20 chars, contains `T`) to a native
/// timestamp, so the row holds the normalized `2026-12-01 10:00:00` while
/// the `unique` row probe compares against the untouched request value and
/// finds nothing — a silent `unique` false pass, i.e. a second appointment
/// booked into a slot the manifest already guards. (`update` already binds
/// field-aware; this aligns the insert paths with it.)
///
/// Every other value keeps the untyped path unchanged, so nothing that
/// binds today binds differently tomorrow.
pub fn record_to_columns_for(
    record: &Record,
    model: &ModelDef,
) -> Result<(Vec<Ident>, Vec<Bind>), AppError> {
    let mut columns = Vec::with_capacity(record.len());
    let mut binds = Vec::with_capacity(record.len());
    for (key, value) in record {
        columns.push(Ident::new(key.clone())?);
        binds.push(bind_for_field(key, value, model)?);
    }
    Ok((columns, binds))
}

/// Bind one record entry: a declared text-like field keeps its raw string,
/// everything else (framework timestamps, undeclared columns, non-string
/// values) goes through the untyped converter exactly as before.
fn bind_for_field(key: &str, value: &Json, model: &ModelDef) -> Result<Bind, AppError> {
    if let Json::String(s) = value {
        if let Some(field) = model.field_named(key) {
            if matches!(
                field.field_type,
                FieldType::Text | FieldType::TextLarge | FieldType::Date
            ) {
                return Ok(Bind::Text(s.clone()));
            }
        }
    }
    json_value_to_bind(value)
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
        let public = rt().public_record(&stored).unwrap();
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
        assert!(rt.public_record(&all.items[0]).unwrap().contains_key("summary"));
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

    /// Regression guard for a silent `unique` false pass (found live against
    /// MySQL in Phase A): a declared `Text` field must store its **raw**
    /// string. Field-blind sniffing promotes `"2026-12-01T10:00:00Z"`
    /// (>= 20 chars, contains `T`) to a native timestamp, the row keeps the
    /// normalized `2026-12-01 10:00:00`, and the `unique` row probe — which
    /// compares the untouched request value — then finds no match and lets a
    /// second appointment book a slot the manifest already guards.
    #[test]
    fn record_to_columns_for_keeps_raw_text_for_declared_text_fields() {
        fn bind_of<'a>(cols: &'a [Ident], binds: &'a [Bind], name: &str) -> &'a Bind {
            let idx = cols
                .iter()
                .position(|c| c.as_str() == name)
                .unwrap_or_else(|| panic!("missing column {name}"));
            &binds[idx]
        }

        let model = ModelDef::new("Appointment", "appointments")
            .field(FieldDef::new("id", FieldType::Uuid).required())
            .field(FieldDef::new("patient", FieldType::Text).required())
            .field(FieldDef::new("slot", FieldType::Text).required())
            .field(FieldDef::new("created_at", FieldType::Timestamp));
        let mut record = Record::new();
        record.insert("id".into(), Json::String(Uuid::now_v7().to_string()));
        record.insert("patient".into(), Json::from("alice"));
        record.insert("slot".into(), Json::from("2026-12-01T10:00:00Z"));
        record.insert("created_at".into(), Json::from("2026-12-01T10:00:00Z"));
        // not a model field at all -> untyped sniffing still applies
        record.insert("external_ref".into(), Json::from("2026-12-01T10:00:00Z"));

        let (cols, binds) = record_to_columns_for(&record, &model).unwrap();
        assert_eq!(cols.len(), 5);

        // declared Text field: raw request value, never a timestamp
        assert!(
            matches!(bind_of(&cols, &binds, "slot"), Bind::Text(t) if t == "2026-12-01T10:00:00Z"),
            "slot = {:?}; must keep the raw request value",
            bind_of(&cols, &binds, "slot")
        );
        assert!(
            matches!(bind_of(&cols, &binds, "patient"), Bind::Text(t) if t == "alice"),
            "patient = {:?}",
            bind_of(&cols, &binds, "patient")
        );
        // the SAME value in an undeclared column still sniffs to a timestamp:
        // only declared text-like fields changed, nothing else did
        assert!(matches!(
            bind_of(&cols, &binds, "external_ref"),
            Bind::Timestamp(_)
        ));
        // declared Timestamp keeps timestamp semantics; uuid keeps uuid
        assert!(matches!(
            bind_of(&cols, &binds, "created_at"),
            Bind::Timestamp(_)
        ));
        assert!(matches!(bind_of(&cols, &binds, "id"), Bind::Uuid(_)));
    }

    /// Nothing that binds today may stop binding: a non-string landing in a
    /// `Text` field takes the untyped path instead of erroring out.
    #[test]
    fn record_to_columns_for_never_rejects_a_value_that_bound_before() {
        let model = ModelDef::new("Reading", "readings")
            .field(FieldDef::new("value", FieldType::Text));
        let mut record = Record::new();
        record.insert("value".into(), Json::from(7));
        let (cols, binds) = record_to_columns_for(&record, &model).unwrap();
        assert_eq!(cols.len(), 1);
        assert!(
            matches!(&binds[0], Bind::Int(7)),
            "non-string in a Text field = {:?}; must fall back to the untyped path",
            binds[0]
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

    // ------------------------------------------ Wave 4 (phase 4): relations

    /// Patients (has_many appointments) + appointments (belongs_to patient)
    /// over a shared registry and store.
    fn rel_fixture() -> (ModelRuntime, ModelRuntime, Arc<InMemoryModelStore>) {
        let mut registry = ModelRegistry::new();
        let patient = ModelDef::new("Patient", "patients")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("name", FieldType::Text).required())
            .relation(RelationDef::has_many("appointments", "Appointment", "patient_id"));
        let appointment = ModelDef::new("Appointment", "appointments")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("patient_id", FieldType::Uuid))
            .field(FieldDef::new("status", FieldType::Text).required())
            .field(FieldDef::new("visits", FieldType::Int64))
            .relation(RelationDef::belongs_to("patient", "Patient", "patient_id"));
        registry.register(patient.clone()).unwrap();
        registry.register(appointment.clone()).unwrap();
        registry.validate_graph().unwrap();
        let registry = Arc::new(registry);
        (
            ModelRuntime::new(patient).with_registry(registry.clone()),
            ModelRuntime::new(appointment).with_registry(registry),
            InMemoryModelStore::new(),
        )
    }

    fn patient_named(rt: &ModelRuntime, store: &InMemoryModelStore, name: &str) -> Record {
        let mut r = Record::new();
        r.insert("name".into(), Json::String(name.into()));
        rt.insert(store, &r, None).unwrap()
    }

    fn appt(
        rt: &ModelRuntime,
        store: &InMemoryModelStore,
        patient_id: &Json,
        status: &str,
    ) -> Record {
        let mut r = Record::new();
        r.insert("patient_id".into(), patient_id.clone());
        r.insert("status".into(), Json::String(status.into()));
        rt.insert(store, &r, None).unwrap()
    }

    fn listed_names(page: kelson_query::Page<Record>) -> Vec<String> {
        page.items
            .iter()
            .map(|r| r["name"].as_str().unwrap_or("?").to_string())
            .collect()
    }

    #[test]
    fn has_and_doesnt_have_filter_on_relation_existence() {
        let (patient_rt, appointment_rt, store) = rel_fixture();
        let alice = patient_named(&patient_rt, &store, "Alice");
        let bob = patient_named(&patient_rt, &store, "Bob");
        let _carol = patient_named(&patient_rt, &store, "Carol");
        appt(&appointment_rt, &store, &alice["id"], "booked");
        appt(&appointment_rt, &store, &alice["id"], "cancelled");
        appt(&appointment_rt, &store, &bob["id"], "cancelled");

        // Any related row.
        let page = patient_rt
            .list(
                &store,
                None,
                vec![patient_rt.has("appointments", vec![]).unwrap()],
                &[],
                None,
            )
            .unwrap();
        assert_eq!(listed_names(page), vec!["Alice", "Bob"]);

        // Sub conditions run against the RELATED rows.
        let page = patient_rt
            .list(
                &store,
                None,
                vec![patient_rt
                    .has("appointments", vec![FieldFilter::eq("status", "booked")])
                    .unwrap()],
                &[],
                None,
            )
            .unwrap();
        assert_eq!(listed_names(page), vec!["Alice"]);

        // NOT EXISTS: no cancelled appointment (Carol has none at all).
        let page = patient_rt
            .list(
                &store,
                None,
                vec![patient_rt
                    .doesnt_have("appointments", vec![FieldFilter::eq("status", "cancelled")])
                    .unwrap()],
                &[],
                None,
            )
            .unwrap();
        assert_eq!(listed_names(page), vec!["Carol"]);
    }

    #[test]
    fn or_where_has_ors_with_the_preceding_terms() {
        let (patient_rt, appointment_rt, store) = rel_fixture();
        let alice = patient_named(&patient_rt, &store, "Alice");
        let _bob = patient_named(&patient_rt, &store, "Bob");
        let _carol = patient_named(&patient_rt, &store, "Carol");
        appt(&appointment_rt, &store, &alice["id"], "booked");

        // `name = Bob OR EXISTS(...)` — Alice qualifies through EXISTS,
        // Bob through the comparison, Carol through neither.
        let page = patient_rt
            .list(
                &store,
                None,
                vec![
                    FieldFilter::eq("name", "Bob"),
                    patient_rt.or_where_has("appointments", vec![]).unwrap(),
                ],
                &[],
                None,
            )
            .unwrap();
        assert_eq!(listed_names(page), vec!["Alice", "Bob"]);

        // Plain `has` stays AND-joined: name AND existence.
        let page = patient_rt
            .list(
                &store,
                None,
                vec![
                    FieldFilter::eq("name", "Carol"),
                    patient_rt.has("appointments", vec![]).unwrap(),
                ],
                &[],
                None,
            )
            .unwrap();
        assert!(page.items.is_empty());
    }

    #[test]
    fn where_relation_filters_on_related_columns() {
        let (patient_rt, appointment_rt, store) = rel_fixture();
        let alice = patient_named(&patient_rt, &store, "Alice");
        let bob = patient_named(&patient_rt, &store, "Bob");
        appt(&appointment_rt, &store, &alice["id"], "booked");
        let b_appt = appt(&appointment_rt, &store, &bob["id"], "booked");

        let page = appointment_rt
            .list(
                &store,
                None,
                vec![appointment_rt
                    .where_relation("patient", "name", "Alice")
                    .unwrap()],
                &[],
                None,
            )
            .unwrap();
        assert_eq!(page.total, Some(1));
        assert_eq!(page.items[0]["patient_id"], alice["id"]);

        let page = appointment_rt
            .list(
                &store,
                None,
                vec![appointment_rt
                    .where_doesnt_have_relation("patient", "name", "Alice")
                    .unwrap()],
                &[],
                None,
            )
            .unwrap();
        assert_eq!(page.total, Some(1));
        assert_eq!(page.items[0]["id"], b_appt["id"]);
    }

    #[test]
    fn existence_filters_scope_tenant_and_soft_deletes() {
        let mut registry = ModelRegistry::new();
        let patient = ModelDef::new("Patient", "patients")
            .tenant_scoped()
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("tenant_id", FieldType::Uuid).required())
            .field(FieldDef::new("name", FieldType::Text).required())
            .relation(RelationDef::has_many("appointments", "Appointment", "patient_id"));
        let appointment = ModelDef::new("Appointment", "appointments")
            .tenant_scoped()
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("tenant_id", FieldType::Uuid).required())
            .field(FieldDef::new("patient_id", FieldType::Uuid))
            .field(FieldDef::new("status", FieldType::Text).required());
        registry.register(patient.clone()).unwrap();
        registry.register(appointment.clone()).unwrap();
        registry.validate_graph().unwrap();
        let registry = Arc::new(registry);
        let patient_rt = ModelRuntime::new(patient).with_registry(registry.clone());
        let appointment_rt = ModelRuntime::new(appointment).with_registry(registry);
        let store = InMemoryModelStore::new();

        let t = TenantId::new();
        let foreign = TenantId::new();
        let mut name = Record::new();
        name.insert("name".into(), Json::String("P".into()));
        let p = patient_rt.insert(&store, &name, Some(t)).unwrap();

        // Cross-tenant child must not satisfy the existence filter.
        let mut cross = Record::new();
        cross.insert("patient_id".into(), p["id"].clone());
        cross.insert("status".into(), Json::String("booked".into()));
        appointment_rt.insert(&store, &cross, Some(foreign)).unwrap();
        let page = patient_rt
            .list(
                &store,
                Some(t),
                vec![patient_rt.has("appointments", vec![]).unwrap()],
                &[],
                None,
            )
            .unwrap();
        assert!(page.items.is_empty(), "cross-tenant child leaked into EXISTS");

        // Same-tenant child qualifies...
        let mut same = Record::new();
        same.insert("patient_id".into(), p["id"].clone());
        same.insert("status".into(), Json::String("booked".into()));
        let child = appointment_rt.insert(&store, &same, Some(t)).unwrap();
        let page = patient_rt
            .list(
                &store,
                Some(t),
                vec![patient_rt.has("appointments", vec![]).unwrap()],
                &[],
                None,
            )
            .unwrap();
        assert_eq!(page.total, Some(1));

        // ...until it is trashed: soft-deleted children never satisfy EXISTS.
        appointment_rt.delete(&store, rid(&child), Some(t)).unwrap();
        let page = patient_rt
            .list(
                &store,
                Some(t),
                vec![patient_rt.has("appointments", vec![]).unwrap()],
                &[],
                None,
            )
            .unwrap();
        assert!(page.items.is_empty(), "trashed child still satisfied EXISTS");
    }

    #[test]
    fn existence_filter_guards_reject_bad_relations() {
        let (patient_rt, appointment_rt, _store) = rel_fixture();

        let err = patient_rt.has("nope", vec![]).unwrap_err();
        assert!(err.public_message.contains("no relation"), "{}", err.public_message);

        let err = patient_rt
            .has("appointments", vec![FieldFilter::eq("bogus", 1)])
            .unwrap_err();
        assert!(err.public_message.contains("unknown field"), "{}", err.public_message);

        // Nesting an existence term inside another is refused at build.
        let inner = appointment_rt.has("patient", vec![]).unwrap();
        let err = patient_rt.has("appointments", vec![inner]).unwrap_err();
        assert!(err.public_message.contains("nest"), "{}", err.public_message);

        // A relation declared but no registry wired: filter build needs it.
        let bare_patient = ModelDef::new("Patient", "patients")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("name", FieldType::Text).required())
            .relation(RelationDef::has_many("appointments", "Appointment", "patient_id"));
        let err = ModelRuntime::new(bare_patient)
            .has("appointments", vec![])
            .unwrap_err();
        assert!(
            err.public_message.contains("model registry"),
            "{}",
            err.public_message
        );

        // Join-walk kinds don't correlate directly.
        let mut registry = ModelRegistry::new();
        let patient = ModelDef::new("Patient", "patients")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .relation(RelationDef::many_through(
                "visits",
                "Visit",
                "VisitLink",
                "patient_id",
            ));
        let visit = ModelDef::new("Visit", "visits")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique());
        let link = ModelDef::new("VisitLink", "visit_links")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique());
        registry.register(patient.clone()).unwrap();
        registry.register(visit).unwrap();
        registry.register(link).unwrap();
        registry.validate_graph().unwrap();
        let rt = ModelRuntime::new(patient).with_registry(Arc::new(registry));
        let err = rt.has("visits", vec![]).unwrap_err();
        assert!(
            err.public_message.contains("unsupported"),
            "{}",
            err.public_message
        );
    }

    #[test]
    fn has_many_load_keeps_every_child_per_parent() {
        let (patient_rt, appointment_rt, store) = rel_fixture();
        let alice = patient_named(&patient_rt, &store, "Alice");
        let bob = patient_named(&patient_rt, &store, "Bob");
        appt(&appointment_rt, &store, &alice["id"], "booked");
        appt(&appointment_rt, &store, &alice["id"], "cancelled");
        appt(&appointment_rt, &store, &bob["id"], "booked");

        let mut parents = vec![alice, bob];
        patient_rt
            .load_relation(&store, "appointments", &mut parents, None)
            .unwrap();
        // Regression: the grouped fetch once collapsed each parent's
        // children to a single row (map-key overwrite).
        assert_eq!(parents[0]["appointments"].as_array().unwrap().len(), 2);
        assert_eq!(parents[1]["appointments"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn with_count_and_friends_fold_related_rows() {
        let (patient_rt, appointment_rt, store) = rel_fixture();
        let alice = patient_named(&patient_rt, &store, "Alice");
        let bob = patient_named(&patient_rt, &store, "Bob");

        for visits in [3i64, 5] {
            let mut a = Record::new();
            a.insert("patient_id".into(), alice["id"].clone());
            a.insert("status".into(), Json::String("booked".into()));
            a.insert("visits".into(), Json::from(visits));
            appointment_rt.insert(&store, &a, None).unwrap();
        }
        // An appointment with no patient (belongs_to side, count 0).
        let mut orphan = Record::new();
        orphan.insert("status".into(), Json::String("booked".into()));
        appointment_rt.insert(&store, &orphan, None).unwrap();

        let mut parents = vec![alice, bob];
        patient_rt.with_count(&store, "appointments", &mut parents, None).unwrap();
        patient_rt
            .with_sum(&store, "appointments", &mut parents, None, "visits")
            .unwrap();
        patient_rt
            .with_min(&store, "appointments", &mut parents, None, "visits")
            .unwrap();
        patient_rt
            .with_max(&store, "appointments", &mut parents, None, "visits")
            .unwrap();
        patient_rt
            .with_avg(&store, "appointments", &mut parents, None, "visits")
            .unwrap();

        // Laravel aliases: `<rel>_count`, `<rel>_<op>_<field>`.
        assert_eq!(parents[0]["appointments_count"], Json::from(2u64));
        assert_eq!(parents[0]["appointments_sum_visits"], Json::from(8));
        assert_eq!(parents[0]["appointments_min_visits"], Json::from(3));
        assert_eq!(parents[0]["appointments_max_visits"], Json::from(5));
        assert_eq!(parents[0]["appointments_avg_visits"], Json::from(4.0));
        // Empty set: COUNT = 0, the rest SQL NULL.
        assert_eq!(parents[1]["appointments_count"], Json::from(0u64));
        assert_eq!(parents[1]["appointments_sum_visits"], Json::Null);
        assert_eq!(parents[1]["appointments_min_visits"], Json::Null);

        // belongs_to folds the single related row: 1 when the FK resolves,
        // 0 when it does not.
        let mut appts = appointment_rt
            .list(&store, None, vec![], &[], None)
            .unwrap()
            .items;
        appointment_rt
            .with_count(&store, "patient", &mut appts, None)
            .unwrap();
        let counts: Vec<u64> = appts
            .iter()
            .map(|r| r["patient_count"].as_u64().unwrap())
            .collect();
        assert_eq!(counts, vec![1, 1, 0]);
    }

    #[test]
    fn aggregate_eager_guards_reject_bad_targets() {
        let (patient_rt, _appointment_rt, store) = rel_fixture();
        let mut parents: Vec<Record> = Vec::new();

        let err = patient_rt
            .with_count(&store, "nope", &mut parents, None)
            .unwrap_err();
        assert!(err.public_message.contains("no relation"), "{}", err.public_message);

        let err = patient_rt
            .with_sum(&store, "appointments", &mut parents, None, "status")
            .unwrap_err();
        assert!(err.public_message.contains("numeric"), "{}", err.public_message);

        let err = patient_rt
            .with_sum(&store, "appointments", &mut parents, None, "missing")
            .unwrap_err();
        assert!(err.public_message.contains("not on model"), "{}", err.public_message);
    }

    #[test]
    fn many_through_eager_load_and_aggregates() {
        let mut registry = ModelRegistry::new();
        let patient = ModelDef::new("Patient", "patients")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("name", FieldType::Text).required())
            .relation(RelationDef::many_through(
                "visits",
                "Visit",
                "VisitLink",
                "patient_id",
            ));
        let visit = ModelDef::new("Visit", "visits")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("duration_min", FieldType::Int64));
        let link = ModelDef::new("VisitLink", "visit_links")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("patient_id", FieldType::Uuid))
            .field(FieldDef::new("visit_id", FieldType::Uuid));
        registry.register(patient.clone()).unwrap();
        registry.register(visit.clone()).unwrap();
        registry.register(link.clone()).unwrap();
        registry.validate_graph().unwrap();
        let registry = Arc::new(registry);
        let patient_rt = ModelRuntime::new(patient).with_registry(registry.clone());
        let visit_rt = ModelRuntime::new(visit).with_registry(registry.clone());
        let link_rt = ModelRuntime::new(link).with_registry(registry);
        let store = InMemoryModelStore::new();

        let p1 = patient_named(&patient_rt, &store, "P1");
        let p2 = patient_named(&patient_rt, &store, "P2");
        let p3 = patient_named(&patient_rt, &store, "P3");
        let mut durations = Vec::new();
        for (patient_id, minutes) in [
            (p1["id"].clone(), 30i64),
            (p1["id"].clone(), 10),
            (p2["id"].clone(), 5),
        ] {
            let mut v = Record::new();
            v.insert("duration_min".into(), Json::from(minutes));
            let stored = visit_rt.insert(&store, &v, None).unwrap();
            let mut l = Record::new();
            l.insert("patient_id".into(), patient_id);
            l.insert("visit_id".into(), stored["id"].clone());
            link_rt.insert(&store, &l, None).unwrap();
            durations.push(minutes);
        }
        assert_eq!(durations.len(), 3);

        let mut parents = vec![p1, p2, p3];
        patient_rt
            .load_relation(&store, "visits", &mut parents, None)
            .unwrap();
        assert_eq!(parents[0]["visits"].as_array().unwrap().len(), 2);
        assert_eq!(parents[1]["visits"].as_array().unwrap().len(), 1);
        assert_eq!(parents[2]["visits"].as_array().unwrap().len(), 0);

        patient_rt
            .with_count(&store, "visits", &mut parents, None)
            .unwrap();
        patient_rt
            .with_sum(&store, "visits", &mut parents, None, "duration_min")
            .unwrap();
        assert_eq!(parents[0]["visits_count"], Json::from(2u64));
        assert_eq!(parents[0]["visits_sum_duration_min"], Json::from(40));
        assert_eq!(parents[1]["visits_count"], Json::from(1u64));
        assert_eq!(parents[1]["visits_sum_duration_min"], Json::from(5));
        assert_eq!(parents[2]["visits_count"], Json::from(0u64));
        assert_eq!(parents[2]["visits_sum_duration_min"], Json::Null);
    }

    /// Posts -> comments (has_many) -> author (belongs_to): a 3-model
    /// chain for nested `with` paths.
    fn comment_fixture() -> (
        ModelRuntime,
        ModelRuntime,
        ModelRuntime,
        Arc<InMemoryModelStore>,
    ) {
        let mut registry = ModelRegistry::new();
        let user = ModelDef::new("User", "users")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("name", FieldType::Text).required());
        let post = ModelDef::new("Post", "posts")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("title", FieldType::Text).required())
            .relation(RelationDef::has_many("comments", "Comment", "post_id"));
        let comment = ModelDef::new("Comment", "comments")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("post_id", FieldType::Uuid))
            .field(FieldDef::new("author_id", FieldType::Uuid))
            .field(FieldDef::new("body", FieldType::Text).required())
            .relation(RelationDef::belongs_to("author", "User", "author_id"));
        registry.register(user.clone()).unwrap();
        registry.register(post.clone()).unwrap();
        registry.register(comment.clone()).unwrap();
        registry.validate_graph().unwrap();
        let registry = Arc::new(registry);
        (
            ModelRuntime::new(post).with_registry(registry.clone()),
            ModelRuntime::new(comment).with_registry(registry.clone()),
            ModelRuntime::new(user).with_registry(registry),
            InMemoryModelStore::new(),
        )
    }

    fn comment_seed(
        post_rt: &ModelRuntime,
        comment_rt: &ModelRuntime,
        user_rt: &ModelRuntime,
        store: &InMemoryModelStore,
    ) -> (Record, Record) {
        let mut users = Vec::new();
        for name in ["Ada", "Grace"] {
            let mut u = Record::new();
            u.insert("name".into(), Json::String(name.into()));
            users.push(user_rt.insert(store, &u, None).unwrap());
        }
        let mut posts = Vec::new();
        for title in ["P1", "P2"] {
            let mut p = Record::new();
            p.insert("title".into(), Json::String(title.into()));
            posts.push(post_rt.insert(store, &p, None).unwrap());
        }
        for (post, author, body) in [
            (&posts[0], &users[0], "alpha"),
            (&posts[0], &users[1], "beta"),
            (&posts[1], &users[0], "gamma"),
        ] {
            let mut c = Record::new();
            c.insert("post_id".into(), post["id"].clone());
            c.insert("author_id".into(), author["id"].clone());
            c.insert("body".into(), Json::String(body.into()));
            comment_rt.insert(store, &c, None).unwrap();
        }
        (posts[0].clone(), posts[1].clone())
    }

    #[test]
    fn with_loads_a_nested_path_level_by_level() {
        let (post_rt, comment_rt, user_rt, store) = comment_fixture();
        let (p1, p2) = comment_seed(&post_rt, &comment_rt, &user_rt, &store);

        let mut posts = vec![p1, p2];
        post_rt.with(&store, &mut posts, None, "comments.author").unwrap();

        let p1_comments = posts[0]["comments"].as_array().unwrap();
        assert_eq!(p1_comments.len(), 2);
        assert_eq!(p1_comments[0]["body"], Json::String("alpha".into()));
        assert_eq!(p1_comments[0]["author"]["name"], Json::String("Ada".into()));
        assert_eq!(p1_comments[1]["body"], Json::String("beta".into()));
        assert_eq!(
            p1_comments[1]["author"]["name"],
            Json::String("Grace".into())
        );
        let p2_comments = posts[1]["comments"].as_array().unwrap();
        assert_eq!(p2_comments.len(), 1);
        assert_eq!(p2_comments[0]["author"]["name"], Json::String("Ada".into()));
    }

    #[test]
    fn with_constrained_applies_per_level_constraints() {
        let (post_rt, comment_rt, user_rt, store) = comment_fixture();

        // Level 0 sorts its fetch; level 1 stays unconstrained.
        let (p1, p2) = comment_seed(&post_rt, &comment_rt, &user_rt, &store);
        let mut posts = vec![p1, p2];
        let mut seen: Vec<(usize, String)> = Vec::new();
        post_rt
            .with_constrained(
                &store,
                &mut posts,
                None,
                "comments.author",
                &mut |level, name, constraint| {
                    seen.push((level, name.to_string()));
                    if level == 0 {
                        constraint.sort = Some(("body".into(), true));
                    }
                },
            )
            .unwrap();
        assert_eq!(
            seen,
            vec![(0, "comments".to_string()), (1, "author".to_string())]
        );
        let bodies: Vec<String> = posts[0]["comments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["body"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(bodies, vec!["beta", "alpha"]);
        // Sorted fetch still cascades into the level-1 load.
        assert_eq!(
            posts[0]["comments"][0]["author"]["name"],
            Json::String("Grace".into())
        );

        // Level-0 filter narrows which rows load at all.
        let (p1, p2) = comment_seed(&post_rt, &comment_rt, &user_rt, &store);
        let mut posts = vec![p1, p2];
        post_rt
            .with_constrained(
                &store,
                &mut posts,
                None,
                "comments.author",
                &mut |level, _name, constraint| {
                    if level == 0 {
                        constraint.filters.push(FieldFilter::eq("body", "alpha"));
                    }
                },
            )
            .unwrap();
        let p1_comments = posts[0]["comments"].as_array().unwrap();
        assert_eq!(p1_comments.len(), 1);
        assert_eq!(p1_comments[0]["body"], Json::String("alpha".into()));
        assert!(posts[1]["comments"].as_array().unwrap().is_empty());
    }

    #[test]
    fn with_rejects_an_empty_path() {
        let (post_rt, comment_rt, user_rt, store) = comment_fixture();
        let (p1, _p2) = comment_seed(&post_rt, &comment_rt, &user_rt, &store);
        let mut posts = vec![p1];
        let err = post_rt.with(&store, &mut posts, None, " . ").unwrap_err();
        assert!(err.public_message.contains("relation path"), "{}", err.public_message);
    }

    /// Patient <-> Doctor through a pivot: the Patient side goes through a
    /// REGISTERED join model (extra `role` column); the Doctor side uses
    /// the pivot synthesized from `pivot_table`.
    fn pairing_fixture() -> (ModelRuntime, ModelRuntime, Arc<InMemoryModelStore>) {
        let mut registry = ModelRegistry::new();
        let mut doctors = RelationDef::many_to_many(
            "doctors",
            "Doctor",
            RelationDef::pivot_table_for("Patient", "Doctor"),
            "patient_id",
            "doctor_id",
        );
        doctors.through = Some("Pairing".into());
        let patient = ModelDef::new("Patient", "patients")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("name", FieldType::Text).required())
            .relation(doctors);
        let doctor = ModelDef::new("Doctor", "doctors")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("name", FieldType::Text).required())
            .relation(RelationDef::many_to_many(
                "patients",
                "Patient",
                RelationDef::pivot_table_for("Doctor", "Patient"),
                "doctor_id",
                "patient_id",
            ));
        let pairing = ModelDef::new("Pairing", "doctor_patient")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("patient_id", FieldType::Uuid))
            .field(FieldDef::new("doctor_id", FieldType::Uuid))
            .field(FieldDef::new("role", FieldType::Text));
        registry.register(patient.clone()).unwrap();
        registry.register(doctor.clone()).unwrap();
        registry.register(pairing).unwrap();
        registry.validate_graph().unwrap();
        let registry = Arc::new(registry);
        (
            ModelRuntime::new(patient).with_registry(registry.clone()),
            ModelRuntime::new(doctor).with_registry(registry),
            InMemoryModelStore::new(),
        )
    }

    fn pairing_seed(
        patient_rt: &ModelRuntime,
        doctor_rt: &ModelRuntime,
        store: &InMemoryModelStore,
    ) -> (Vec<Record>, Vec<Record>) {
        let mut patients = Vec::new();
        for name in ["Alice", "Bob"] {
            let mut p = Record::new();
            p.insert("name".into(), Json::String(name.into()));
            patients.push(patient_rt.insert(store, &p, None).unwrap());
        }
        let mut doctors = Vec::new();
        for name in ["House", "Chase", "Wilson"] {
            let mut d = Record::new();
            d.insert("name".into(), Json::String(name.into()));
            doctors.push(doctor_rt.insert(store, &d, None).unwrap());
        }
        (patients, doctors)
    }

    #[test]
    fn many_to_many_eager_loads_and_aggregates() {
        let (patient_rt, doctor_rt, store) = pairing_fixture();
        let (patients, doctors) = pairing_seed(&patient_rt, &doctor_rt, &store);
        let (alice, bob) = (&patients[0], &patients[1]);

        // attach counts fresh pairs only — repeats are idempotent.
        assert_eq!(
            patient_rt
                .attach(
                    &store,
                    "doctors",
                    alice["id"].clone(),
                    vec![doctors[0]["id"].clone(), doctors[1]["id"].clone()],
                    None,
                )
                .unwrap(),
            2
        );
        assert_eq!(
            patient_rt
                .attach(
                    &store,
                    "doctors",
                    alice["id"].clone(),
                    vec![doctors[0]["id"].clone()],
                    None,
                )
                .unwrap(),
            0
        );
        patient_rt
            .attach(
                &store,
                "doctors",
                bob["id"].clone(),
                vec![doctors[0]["id"].clone()],
                None,
            )
            .unwrap();

        let mut parents = vec![alice.clone(), bob.clone()];
        patient_rt
            .load_relation(&store, "doctors", &mut parents, None)
            .unwrap();
        assert_eq!(parents[0]["doctors"].as_array().unwrap().len(), 2);
        assert_eq!(parents[1]["doctors"].as_array().unwrap().len(), 1);

        patient_rt
            .with_count(&store, "doctors", &mut parents, None)
            .unwrap();
        assert_eq!(parents[0]["doctors_count"], Json::from(2u64));
        assert_eq!(parents[1]["doctors_count"], Json::from(1u64));

        // The reverse direction reads the same pivot via the SYNTHESIZED
        // model (Doctor declares no `through`).
        let mut docs = vec![doctors[0].clone(), doctors[1].clone()];
        doctor_rt
            .load_relation(&store, "patients", &mut docs, None)
            .unwrap();
        assert_eq!(docs[0]["patients"].as_array().unwrap().len(), 2);
        assert_eq!(docs[1]["patients"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn pivot_sync_detach_toggle_and_update() {
        let (patient_rt, doctor_rt, store) = pairing_fixture();
        let (patients, doctors) = pairing_seed(&patient_rt, &doctor_rt, &store);
        let alice = &patients[0];
        let (d1, d2, d3) = (&doctors[0], &doctors[1], &doctors[2]);
        patient_rt
            .attach(
                &store,
                "doctors",
                alice["id"].clone(),
                vec![d1["id"].clone(), d2["id"].clone()],
                None,
            )
            .unwrap();

        // sync converges to EXACTLY the desired set.
        let sync = patient_rt
            .sync(
                &store,
                "doctors",
                alice["id"].clone(),
                vec![d2["id"].clone(), d3["id"].clone()],
                None,
            )
            .unwrap();
        assert_eq!(sync.attached, vec![d3["id"].clone()]);
        assert_eq!(sync.detached, vec![d1["id"].clone()]);

        // toggle flips each listed id: d1 in -> attached, d2 out -> detached.
        let sync = patient_rt
            .toggle(
                &store,
                "doctors",
                alice["id"].clone(),
                vec![d1["id"].clone(), d2["id"].clone()],
                None,
            )
            .unwrap();
        assert_eq!(sync.attached, vec![d1["id"].clone()]);
        assert_eq!(sync.detached, vec![d2["id"].clone()]);

        let mut parents = vec![alice.clone()];
        patient_rt
            .load_relation(&store, "doctors", &mut parents, None)
            .unwrap();
        assert_eq!(parents[0]["doctors"].as_array().unwrap().len(), 2);

        // detach with no ids removes EVERY link for that parent.
        let removed = patient_rt
            .detach(&store, "doctors", alice["id"].clone(), vec![], None)
            .unwrap();
        assert_eq!(removed, 2);
        patient_rt
            .load_relation(&store, "doctors", &mut parents, None)
            .unwrap();
        assert!(parents[0]["doctors"].as_array().unwrap().is_empty());

        // updateExistingPivot edits through the REGISTERED pivot model
        // (its `role` column is what makes the join model necessary).
        patient_rt
            .attach(
                &store,
                "doctors",
                alice["id"].clone(),
                vec![d1["id"].clone()],
                None,
            )
            .unwrap();
        let mut changes = Record::new();
        changes.insert("role".into(), Json::String("attending".into()));
        let row = patient_rt
            .update_existing_pivot(
                &store,
                "doctors",
                alice["id"].clone(),
                d1["id"].clone(),
                &changes,
                None,
            )
            .unwrap();
        assert_eq!(row["role"], Json::String("attending".into()));

        // A pair that is not linked is a typed 404.
        let err = patient_rt
            .update_existing_pivot(
                &store,
                "doctors",
                alice["id"].clone(),
                d2["id"].clone(),
                &changes,
                None,
            )
            .unwrap_err();
        assert_eq!(err.code, "KSN_NOT_FOUND");
    }

    #[test]
    fn pivot_ops_reject_non_many_to_many_and_existence_stays_refused() {
        // A has_many relation refuses pivot ops with a typed error.
        let (patient_rt, _appointment_rt, store) = rel_fixture();
        let err = patient_rt
            .attach(&store, "appointments", Json::Null, vec![], None)
            .unwrap_err();
        assert!(
            err.public_message.contains("not many-to-many"),
            "{}",
            err.public_message
        );

        // Existence filters over a pivot relation remain a documented gap.
        let (patient_rt, _doctor_rt, _store) = pairing_fixture();
        let err = patient_rt.has("doctors", vec![]).unwrap_err();
        assert!(
            err.public_message.contains("unsupported"),
            "{}",
            err.public_message
        );
    }

    /// Polymorphic fixture: `Image` morphTo's its imageable (Post/Video via
    /// a morph map); `Post` morphMany's Comments and morphToMany's Tags
    /// through a synthesized `taggables` pivot.
    #[allow(clippy::type_complexity)]
    fn morph_fixture()
    -> (
        ModelRuntime,
        ModelRuntime,
        ModelRuntime,
        ModelRuntime,
        ModelRuntime,
        Arc<InMemoryModelStore>,
    ) {
        let mut registry = ModelRegistry::new();
        let post = ModelDef::new("Post", "posts")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("title", FieldType::Text).required())
            .relation(RelationDef::morph_many(
                "comments",
                "Comment",
                "commentable_type",
                "commentable_id",
            ))
            .relation(RelationDef::morph_to_many(
                "tags",
                "Tag",
                "taggables",
                "taggable_type",
                "taggable_id",
                "tag_id",
            ));
        let video = ModelDef::new("Video", "videos")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("duration", FieldType::Int64));
        let image = ModelDef::new("Image", "images")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("url", FieldType::Text).required())
            .field(FieldDef::new("imageable_type", FieldType::Text))
            .field(FieldDef::new("imageable_id", FieldType::Uuid))
            .relation(RelationDef::morph_to(
                "imageable",
                "imageable_type",
                "imageable_id",
            ));
        let comment = ModelDef::new("Comment", "comments")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("body", FieldType::Text).required())
            .field(FieldDef::new("commentable_type", FieldType::Text))
            .field(FieldDef::new("commentable_id", FieldType::Uuid));
        let tag = ModelDef::new("Tag", "tags")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("label", FieldType::Text).required());
        registry.register(post.clone()).unwrap();
        registry.register(video.clone()).unwrap();
        registry.register(image.clone()).unwrap();
        registry.register(comment.clone()).unwrap();
        registry.register(tag.clone()).unwrap();
        registry.register_morph("post", "Post").unwrap();
        registry.register_morph("video", "Video").unwrap();
        registry.validate_graph().unwrap();
        let registry = Arc::new(registry);
        (
            ModelRuntime::new(post).with_registry(registry.clone()),
            ModelRuntime::new(image).with_registry(registry.clone()),
            ModelRuntime::new(video).with_registry(registry.clone()),
            ModelRuntime::new(comment).with_registry(registry.clone()),
            ModelRuntime::new(tag).with_registry(registry),
            InMemoryModelStore::new(),
        )
    }

    #[test]
    fn morph_to_loads_each_type_through_the_morph_map() {
        let (post_rt, image_rt, video_rt, _comment_rt, _tag_rt, store) = morph_fixture();
        let mut p = Record::new();
        p.insert("title".into(), Json::String("Hello".into()));
        let post = post_rt.insert(&store, &p, None).unwrap();
        let mut v = Record::new();
        v.insert("duration".into(), Json::from(120));
        let video = video_rt.insert(&store, &v, None).unwrap();

        let image = |type_value: Option<&str>, id: Option<Json>| {
            let mut r = Record::new();
            r.insert("url".into(), Json::String("u".into()));
            if let Some(t) = type_value {
                r.insert("imageable_type".into(), Json::String(t.into()));
            }
            if let Some(id) = id {
                r.insert("imageable_id".into(), id);
            }
            image_rt.insert(&store, &r, None).unwrap()
        };
        let i1 = image(Some("post"), Some(post["id"].clone()));
        let i2 = image(Some("video"), Some(video["id"].clone()));
        let i3 = image(None, None);
        let mut parents = vec![i1, i2, i3];
        image_rt
            .load_relation(&store, "imageable", &mut parents, None)
            .unwrap();
        // Stored aliases resolve through the morph map; a blank morph is Null.
        assert_eq!(parents[0]["imageable"]["title"], Json::String("Hello".into()));
        assert_eq!(parents[1]["imageable"]["duration"], Json::from(120));
        assert_eq!(parents[2]["imageable"], Json::Null);

        // An unmapped stored type is a typed error, not a silent miss.
        let ghost = image(Some("ghost"), Some(post["id"].clone()));
        let mut parents = vec![ghost];
        let err = image_rt
            .load_relation(&store, "imageable", &mut parents, None)
            .unwrap_err();
        assert!(
            err.public_message.contains("unknown model"),
            "{}",
            err.public_message
        );
    }

    #[test]
    fn morph_many_pins_owner_type_on_load_and_exists() {
        let (post_rt, _image_rt, video_rt, comment_rt, _tag_rt, store) = morph_fixture();
        let mk_post = |title: &str| {
            let mut r = Record::new();
            r.insert("title".into(), Json::String(title.into()));
            post_rt.insert(&store, &r, None).unwrap()
        };
        let p1 = mk_post("A");
        let p2 = mk_post("B");
        let _ = video_rt;
        let mk_comment = |kind: &str, owner: &Json, body: &str| {
            let mut r = Record::new();
            r.insert("body".into(), Json::String(body.into()));
            r.insert("commentable_type".into(), Json::String(kind.into()));
            r.insert("commentable_id".into(), owner.clone());
            comment_rt.insert(&store, &r, None).unwrap()
        };
        mk_comment("post", &p1["id"], "one");
        mk_comment("post", &p1["id"], "two");
        // Sibling-owner traps: same id column, DIFFERENT morph type.
        mk_comment("video", &p1["id"], "leak-1");
        mk_comment("video", &p2["id"], "leak-2");

        let mut parents = vec![p1.clone(), p2.clone()];
        post_rt
            .load_relation(&store, "comments", &mut parents, None)
            .unwrap();
        assert_eq!(parents[0]["comments"].as_array().unwrap().len(), 2);
        assert_eq!(parents[1]["comments"].as_array().unwrap().len(), 0);

        // EXISTS carries the same type pin: p2's video-typed row must not
        // make the post look commented-on.
        let filter = post_rt.has("comments", vec![]).unwrap();
        let found = store
            .list(&post_rt.model, None, &[filter], &ListOpts::default())
            .unwrap()
            .items;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["id"], p1["id"]);
    }

    #[test]
    fn morph_to_many_eager_loads_and_pivots_stay_type_pinned() {
        let (post_rt, _image_rt, _video_rt, _comment_rt, tag_rt, store) = morph_fixture();
        let mk_post = |title: &str| {
            let mut r = Record::new();
            r.insert("title".into(), Json::String(title.into()));
            post_rt.insert(&store, &r, None).unwrap()
        };
        let p1 = mk_post("A");
        let p2 = mk_post("B");
        let mk_tag = |label: &str| {
            let mut r = Record::new();
            r.insert("label".into(), Json::String(label.into()));
            tag_rt.insert(&store, &r, None).unwrap()
        };
        let t1 = mk_tag("rust");
        let t2 = mk_tag("web");

        assert_eq!(
            post_rt
                .attach(
                    &store,
                    "tags",
                    p1["id"].clone(),
                    vec![t1["id"].clone(), t2["id"].clone()],
                    None,
                )
                .unwrap(),
            2
        );
        post_rt
            .attach(&store, "tags", p2["id"].clone(), vec![t2["id"].clone()], None)
            .unwrap();

        // A pivot row for ANOTHER owner type sharing the id must not leak.
        let taggables = ModelDef::new("Taggable", "taggables")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("taggable_id", FieldType::Uuid))
            .field(FieldDef::new("taggable_type", FieldType::Text))
            .field(FieldDef::new("tag_id", FieldType::Uuid));
        let mut rogue = Record::new();
        rogue.insert("id".into(), Json::String(Uuid::now_v7().to_string()));
        rogue.insert("taggable_id".into(), p1["id"].clone());
        rogue.insert("taggable_type".into(), Json::String("video".into()));
        rogue.insert("tag_id".into(), t1["id"].clone());
        store.insert(&taggables, &rogue).unwrap();

        let mut parents = vec![p1.clone(), p2.clone()];
        post_rt
            .load_relation(&store, "tags", &mut parents, None)
            .unwrap();
        assert_eq!(parents[0]["tags"].as_array().unwrap().len(), 2);
        assert_eq!(parents[1]["tags"].as_array().unwrap().len(), 1);

        // sync/detach touch only THIS owner's rows: the video row survives.
        let sync = post_rt
            .sync(
                &store,
                "tags",
                p1["id"].clone(),
                vec![t2["id"].clone()],
                None,
            )
            .unwrap();
        assert_eq!(sync.detached, vec![t1["id"].clone()]);
        assert!(sync.attached.is_empty());
        let removed = post_rt
            .detach(&store, "tags", p1["id"].clone(), vec![], None)
            .unwrap();
        assert_eq!(removed, 1);
        let remaining = store
            .list(&taggables, None, &[], &ListOpts::default())
            .unwrap()
            .items;
        // p2's own row and the video-typed rogue survive — nothing
        // Post-typed tagged to p1 does.
        assert_eq!(remaining.len(), 2);
        assert!(remaining
            .iter()
            .any(|r| r["taggable_type"] == Json::String("video".into())));
        assert!(!remaining.iter().any(|r| {
            r["taggable_id"] == p1["id"] && r["taggable_type"] == Json::String("post".into())
        }));
    }

    #[test]
    fn morph_guards_refuse_single_target_resolution_and_constraints() {
        let (_post_rt, image_rt, _video_rt, _comment_rt, _tag_rt, store) = morph_fixture();
        // MorphTo has no single target: existence builders refuse it...
        let err = image_rt.has("imageable", vec![]).unwrap_err();
        assert!(err.public_message.contains("morph-to"), "{}", err.public_message);
        // ...and per-level eager constraints cannot apply (targets vary).
        let mut images: Vec<Record> = Vec::new();
        let err = image_rt
            .with_constrained(&store, &mut images, None, "imageable", &mut |_, _, c| {
                c.limit = Some(1);
            })
            .unwrap_err();
        assert!(
            err.public_message.contains("eager constraints"),
            "{}",
            err.public_message
        );
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
        assert_eq!(err.code, "KSN_NOT_FOUND");
        let err = rt()
            .first_or_not_found(&store, Some(tenant), &[FieldFilter::eq("patient", "ghost")])
            .unwrap_err();
        assert_eq!(err.code, "KSN_NOT_FOUND");
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
        assert_eq!(err.code, "KSN_NOT_FOUND");
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
        assert_eq!(err.code, "KSN_NOT_FOUND");
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
        assert_eq!(err.code, "KSN_NOT_FOUND");
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
        assert_eq!(err.code, "KSN_NOT_FOUND");

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

    // ------------------------------------------- casts (Phase 5)

    fn cast_model() -> ModelDef {
        ModelDef::new("Ledger", "ledgers")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("name", FieldType::Text).required())
            .field(FieldDef::new("born_on", FieldType::Date).cast("date"))
            .field(FieldDef::new("amount", FieldType::Decimal).cast("decimal:2"))
            .field(FieldDef::new("kind", FieldType::Text).cast("enum:debit,credit"))
            .field(FieldDef::new("meta", FieldType::Json).cast("json"))
            .field(FieldDef::new("tags", FieldType::Json).cast("array"))
            .field(FieldDef::new("secret", FieldType::Text).cast("encrypted"))
            .field(FieldDef::new("password", FieldType::Text).cast("hashed"))
    }

    fn cast_input() -> Record {
        let mut r = Record::new();
        r.insert("name".into(), Json::String("entry".into()));
        r
    }

    #[test]
    fn cast_kinds_validate_and_normalize_at_set_time() {
        let store = InMemoryModelStore::new();
        let rt = ModelRuntime::new(cast_model());

        let mut good = cast_input();
        good.insert("born_on".into(), Json::String("2024-01-02".into()));
        good.insert(
            "amount".into(),
            Json::Number(serde_json::Number::from_f64(12.3456).unwrap()),
        );
        good.insert("kind".into(), Json::String("debit".into()));
        good.insert("meta".into(), Json::Object(serde_json::Map::new()));
        good.insert("tags".into(), Json::Array(vec![Json::Number(1.into())]));
        let stored = rt.insert(&store, &good, None).unwrap();
        assert_eq!(stored["born_on"], Json::String("2024-01-02".into()));
        assert_eq!(stored["amount"].as_f64(), Some(12.35));
        assert_eq!(stored["kind"], Json::String("debit".into()));
        assert_eq!(stored["tags"], Json::Array(vec![Json::Number(1.into())]));

        // Set-time validation refuses bad values at the prepare boundary.
        let mut bad = cast_input();
        bad.insert("born_on".into(), Json::String("2024-02-30".into()));
        let err = rt.insert(&store, &bad, None).unwrap_err();
        assert!(
            err.public_message.contains("valid calendar date"),
            "{}",
            err.public_message
        );

        let mut bad = cast_input();
        bad.insert("kind".into(), Json::String("refund".into()));
        let err = rt.insert(&store, &bad, None).unwrap_err();
        assert!(
            err.public_message.contains("debit, credit"),
            "{}",
            err.public_message
        );

        let mut bad = cast_input();
        bad.insert("meta".into(), Json::String("scalar".into()));
        let err = rt.insert(&store, &bad, None).unwrap_err();
        assert!(
            err.public_message.contains("object or array"),
            "{}",
            err.public_message
        );

        let mut bad = cast_input();
        bad.insert("tags".into(), Json::Object(serde_json::Map::new()));
        let err = rt.insert(&store, &bad, None).unwrap_err();
        assert!(
            err.public_message.contains("array cast"),
            "{}",
            err.public_message
        );

        // Column shape still validates before the cast runs.
        let mut bad = cast_input();
        bad.insert("amount".into(), Json::String("12.5".into()));
        let err = rt.insert(&store, &bad, None).unwrap_err();
        assert!(
            err.public_message.contains("decimal number"),
            "{}",
            err.public_message
        );

        // An unbound sealer refuses the write instead of storing plaintext.
        let mut secret = cast_input();
        secret.insert("secret".into(), Json::String("hush".into()));
        let err = rt.insert(&store, &secret, None).unwrap_err();
        assert!(
            err.public_message.contains("with_sealer"),
            "{}",
            err.public_message
        );
    }

    #[test]
    fn encrypted_cast_seals_through_the_runtime_sealer() {
        use kelson_tokens::{AesGcmSealer, KEY_LEN};

        let store = InMemoryModelStore::new();
        let sealed_rt = ModelRuntime::new(cast_model())
            .with_sealer(Arc::new(AesGcmSealer::new([7u8; KEY_LEN])));
        let mut input = cast_input();
        input.insert("secret".into(), Json::String("hush".into()));
        let stored = sealed_rt.insert(&store, &input, None).unwrap();

        // At rest the value is a sealed envelope, never plaintext...
        let at_rest = stored["secret"].as_str().unwrap();
        assert_ne!(at_rest, "hush");
        // ...and the projection opens it back up.
        let public = sealed_rt.public_record(&stored).unwrap();
        assert_eq!(public["secret"], Json::String("hush".into()));

        // A different master key cannot open the envelope.
        let wrong = ModelRuntime::new(cast_model())
            .with_sealer(Arc::new(AesGcmSealer::new([9u8; KEY_LEN])));
        assert!(wrong.public_record(&stored).is_err());
    }

    #[test]
    fn hashed_cast_hashes_with_argon2_and_never_round_trips() {
        let store = InMemoryModelStore::new();
        let rt = ModelRuntime::new(cast_model());
        let mut input = cast_input();
        input.insert("password".into(), Json::String("hunter2".into()));

        let first = rt.insert(&store, &input, None).unwrap();
        let second = rt.insert(&store, &input, None).unwrap();
        let hash = first["password"].as_str().unwrap();
        assert!(hash.starts_with("$argon2"), "got {hash:?}");
        assert_ne!(first["password"], Json::String("hunter2".into()));
        assert_ne!(
            first["password"], second["password"],
            "fresh salt per write"
        );
        // Reads pass the PHC string through — plaintext never reappears.
        let public = rt.public_record(&first).unwrap();
        assert_eq!(public["password"], first["password"]);
    }

    #[test]
    fn manifest_hidden_and_visible_shape_the_projection() {
        let store = InMemoryModelStore::new();

        // Manifest hidden: dropped by default, accessors still append.
        let doc = ModelDef::new("Doc", "docs")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("title", FieldType::Text).required())
            .field(FieldDef::new("body", FieldType::Text))
            .hide(["body"]);
        let rt = ModelRuntime::new(doc).with_accessor("label", |r| {
            Json::String(format!("doc:{}", r.get("title").and_then(|v| v.as_str()).unwrap_or("?")))
        });
        let mut doc_in = Record::new();
        doc_in.insert("title".into(), Json::String("t1".into()));
        doc_in.insert("body".into(), Json::String("hidden".into()));
        let stored = rt.insert(&store, &doc_in, None).unwrap();

        let public = rt.public_record(&stored).unwrap();
        assert!(!public.contains_key("body"));
        assert!(public.contains_key("title"));
        assert_eq!(public["label"], Json::String("doc:t1".into()));
        // Per-call make_visible opts one key back in.
        let shown = rt
            .wire(&stored)
            .make_visible(["body"])
            .to_record()
            .unwrap();
        assert_eq!(shown["body"], Json::String("hidden".into()));

        // Manifest visible: a non-empty list narrows the stored fields;
        // accessors bypass it (the $appends posture).
        let ticket = ModelDef::new("Ticket", "tickets")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("title", FieldType::Text).required())
            .field(FieldDef::new("body", FieldType::Text))
            .show(["id", "title"]);
        let rt = ModelRuntime::new(ticket).with_accessor("flag", |_| Json::Bool(true));
        let mut t_in = Record::new();
        t_in.insert("title".into(), Json::String("open".into()));
        t_in.insert("body".into(), Json::String("internal".into()));
        let stored = rt.insert(&store, &t_in, None).unwrap();
        let public = rt.public_record(&stored).unwrap();
        assert!(public.contains_key("title"));
        assert!(!public.contains_key("body"), "not in the visible list");
        assert_eq!(public["flag"], Json::Bool(true));
    }

    #[test]
    fn per_call_overrides_win_over_sensitive_and_manifest_policy() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let mut i = input("p");
        i.insert("notes".into(), Json::String("phi".into()));
        let stored = rt().insert(&store, &i, Some(tenant)).unwrap();

        // make_visible opts a `sensitive` field back in for this call only.
        let shown = rt()
            .wire(&stored)
            .make_visible(["notes"])
            .to_record()
            .unwrap();
        assert_eq!(shown["notes"], Json::String("phi".into()));
        assert!(!rt().public_record(&stored).unwrap().contains_key("notes"));

        // make_hidden drops stored fields; make_hidden also beats
        // make_visible when both name the same key.
        let hidden = rt()
            .wire(&stored)
            .make_hidden(["patient"])
            .to_record()
            .unwrap();
        assert!(!hidden.contains_key("patient"));
        let both = rt()
            .wire(&stored)
            .make_visible(["notes"])
            .make_hidden(["notes"])
            .to_record()
            .unwrap();
        assert!(!both.contains_key("notes"), "make_hidden wins");

        // append pins an accessor by name and rejects unknown names.
        let doc = ModelDef::new("Note", "notes")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("title", FieldType::Text));
        let rt = ModelRuntime::new(doc).with_accessor("label", |_| Json::String("L".into()));
        let mut n = Record::new();
        n.insert("title".into(), Json::String("t".into()));
        let stored = rt.insert(&store, &n, None).unwrap();
        let err = rt
            .wire(&stored)
            .append(["nope"])
            .to_record()
            .unwrap_err();
        assert!(
            err.public_message.contains("not an accessor"),
            "{}",
            err.public_message
        );
        let ok = rt
            .wire(&stored)
            .make_hidden(["label"])
            .append(["label"])
            .to_record()
            .unwrap();
        assert!(!ok.contains_key("label"), "make_hidden beats append");
        let ok = rt.wire(&stored).append(["label"]).to_record().unwrap();
        assert_eq!(ok["label"], Json::String("L".into()));
    }

    #[test]
    fn prevent_lazy_loading_flags_the_lazy_path_only() {
        let mut registry = ModelRegistry::new();
        let patient = ModelDef::new("Patient", "patients")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("name", FieldType::Text).required())
            .relation(RelationDef::has_many("appointments", "Appointment", "patient_id"));
        let appointment = ModelDef::new("Appointment", "appointments")
            .field(FieldDef::new("id", FieldType::Uuid).required().unique())
            .field(FieldDef::new("patient_id", FieldType::Uuid))
            .field(FieldDef::new("status", FieldType::Text).required())
            .relation(RelationDef::belongs_to("patient", "Patient", "patient_id"));
        registry.register(patient.clone()).unwrap();
        registry.register(appointment.clone()).unwrap();
        registry.validate_graph().unwrap();
        let registry = Arc::new(registry);

        let lenient = ModelRuntime::new(patient.clone()).with_registry(registry.clone());
        let appointment_rt = ModelRuntime::new(appointment).with_registry(registry.clone());
        let store = InMemoryModelStore::new();
        let alice = patient_named(&lenient, &store, "Alice");
        appt(&appointment_rt, &store, &alice["id"], "booked");

        let strict = ModelRuntime::new(patient)
            .with_registry(registry)
            .prevent_lazy_loading();
        let mut rows = lenient
            .list(&store, None, vec![], &[], None)
            .unwrap()
            .items;

        // The lazy path refuses with the Phase 5 code...
        let err = strict
            .load_relation(&store, "appointments", &mut rows, None)
            .unwrap_err();
        assert_eq!(err.code, codes::LAZY_LOAD_PREVENTED);
        let err = strict
            .load_missing(&store, &["appointments".into()], &mut rows, None)
            .unwrap_err();
        assert_eq!(err.code, codes::LAZY_LOAD_PREVENTED);

        // ...while the declared eager path stays legal under the flag...
        strict.with(&store, &mut rows, None, "appointments").unwrap();
        assert!(rows[0]["appointments"].as_array().is_some());

        // ...and the lenient runtime still lazy-loads.
        lenient
            .load_relation(&store, "appointments", &mut rows, None)
            .unwrap();
        assert!(rows[0]["appointments"].as_array().is_some());
    }

    #[test]
    fn prevent_silently_discarding_attributes_guards_push() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let rt = ModelRuntime::new(appointment_model());
        let row = rt.insert(&store, &input("p"), Some(tenant)).unwrap();

        // Lenient: the undeclared attribute skips silently (historical posture).
        let mut lenient = row.clone();
        lenient.insert("bogus".into(), Json::Bool(true));
        rt.push(&store, &mut lenient, Some(tenant)).unwrap();

        // Strict: refused with a validation error naming the attribute.
        let strict = ModelRuntime::new(appointment_model())
            .prevent_silently_discarding_attributes();
        let mut strict_row = row.clone();
        strict_row.insert("bogus".into(), Json::Bool(true));
        let err = strict
            .push(&store, &mut strict_row, Some(tenant))
            .unwrap_err();
        assert_eq!(err.code, codes::VALIDATION);
        assert!(
            err.public_message.contains("silently discarded"),
            "{}",
            err.public_message
        );

        // Framework columns (id/tenant/timestamps) are not attribute discards.
        let mut clean = row.clone();
        strict.push(&store, &mut clean, Some(tenant)).unwrap();
    }

    #[test]
    fn prevent_accessing_missing_attributes_guards_value_and_pluck() {
        let store = InMemoryModelStore::new();
        let rt = ModelRuntime::new(cast_model());
        rt.insert(&store, &cast_input(), None).unwrap(); // no born_on supplied

        // Lenient: absent declared field reads as None / null.
        assert_eq!(rt.value(&store, None, &[], "born_on").unwrap(), None);
        assert_eq!(
            rt.pluck(&store, None, &[], "born_on").unwrap(),
            vec![Json::Null]
        );

        // Strict: matched-but-missing errors instead.
        let strict = ModelRuntime::new(cast_model()).prevent_accessing_missing_attributes();
        let err = strict.value(&store, None, &[], "born_on").unwrap_err();
        assert!(
            err.public_message.contains("missing attribute"),
            "{}",
            err.public_message
        );
        let err = strict.pluck(&store, None, &[], "born_on").unwrap_err();
        assert!(
            err.public_message.contains("missing attribute"),
            "{}",
            err.public_message
        );

        // No matched row is a miss, not an attribute access.
        assert!(strict
            .value(&store, None, &[FieldFilter::eq("name", "ghost")], "born_on")
            .unwrap()
            .is_none());

        // Undeclared fields still fail the column guard in both modes.
        assert!(strict
            .value(&store, None, &[], "nope")
            .unwrap_err()
            .public_message
            .contains("not on model"));
    }

    // ------------------------------------ Wave 3 Phase 6: optimistic locking

    /// A versioned model: `version` seeds to 1 on insert and every update
    /// compare-and-swaps it to `n + 1`.
    fn versioned_model() -> ModelDef {
        ModelDef::new("Ledger", "ledgers")
            .field(FieldDef::new("id", FieldType::Uuid).required())
            .field(FieldDef::new("note", FieldType::Text))
            .field(FieldDef::new("version", FieldType::Int64))
            .version_column("version")
    }

    fn note(text: &str) -> Record {
        let mut r = Record::new();
        r.insert("note".into(), Json::String(text.into()));
        r
    }

    #[test]
    fn version_column_seeds_one_and_stamps_every_update() {
        let store = InMemoryModelStore::new();
        let rt = ModelRuntime::new(versioned_model());
        let row = rt.insert(&store, &note("a"), None).unwrap();
        assert_eq!(row.get("version").and_then(Json::as_i64), Some(1));

        let mut changes = Record::new();
        changes.insert("note".into(), Json::String("b".into()));
        let updated = rt.update(&store, rid(&row), None, &changes).unwrap();
        assert_eq!(updated.get("version").and_then(Json::as_i64), Some(2));

        // A caller-supplied (stale) version is overwritten by the stamp —
        // re-submitting the old number never bypasses the CAS.
        let mut resubmitted = changes;
        resubmitted.insert("version".into(), Json::from(1i64));
        let updated = rt.update(&store, rid(&row), None, &resubmitted).unwrap();
        assert_eq!(updated.get("version").and_then(Json::as_i64), Some(3));
        assert_eq!(updated.get("note").and_then(Json::as_str), Some("b"));
    }

    #[test]
    fn cas_stale_write_reports_stale_record() {
        use std::sync::atomic::{AtomicBool, Ordering};

        /// A concurrent writer: on `Updating`, bump the stored version
        /// behind the CAS's back (straight to the store — observers never
        /// recurse through the runtime).
        struct Interferer {
            store: Arc<InMemoryModelStore>,
            model: ModelDef,
            fired: AtomicBool,
        }
        impl ModelObserver for Interferer {
            fn on_model_event(&self, event: &ModelEvent) {
                if event.phase != ModelPhase::Updating {
                    return;
                }
                let Some(id) = event.record.get("id").and_then(Json::as_str) else {
                    return;
                };
                let Ok(id) = Uuid::parse_str(id) else { return };
                let mut bump = Record::new();
                bump.insert("version".into(), Json::from(99i64));
                let _ = self.store.update(&self.model, id, None, &bump);
                self.fired.store(true, Ordering::SeqCst);
            }
        }

        let store = InMemoryModelStore::new();
        let model = versioned_model();
        let interferer = Arc::new(Interferer {
            store: store.clone(),
            model: model.clone(),
            fired: AtomicBool::new(false),
        });
        let rt = ModelRuntime::new(model).with_observer(interferer.clone());
        let row = rt.insert(&store, &note("a"), None).unwrap();

        let mut changes = Record::new();
        changes.insert("note".into(), Json::String("b".into()));
        let err = rt.update(&store, rid(&row), None, &changes).unwrap_err();

        assert!(interferer.fired.load(Ordering::SeqCst), "interferer ran");
        assert_eq!(err.code, kelson_foundation::codes::STALE_RECORD);
        assert_eq!(err.category, kelson_foundation::ErrorCategory::Conflict);
        // The stale write was refused: the row keeps the interfering version
        // and the original note.
        let current = rt.get(&store, rid(&row), None).unwrap();
        assert_eq!(current.get("version").and_then(Json::as_i64), Some(99));
        assert_eq!(current.get("note").and_then(Json::as_str), Some("a"));
    }

    #[test]
    fn cas_refuses_a_row_without_an_integer_version() {
        let store = InMemoryModelStore::new();
        let model = versioned_model();
        let id = Uuid::now_v7();
        // Seed straight through the store with NO version value at all.
        let mut seeded = Record::new();
        seeded.insert("id".into(), Json::String(id.to_string()));
        seeded.insert("note".into(), Json::String("seeded".into()));
        store.insert(&model, &seeded).unwrap();

        let rt = ModelRuntime::new(model);
        let mut changes = Record::new();
        changes.insert("note".into(), Json::String("x".into()));
        let err = rt.update(&store, id, None, &changes).unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::VALIDATION);
        assert!(
            err.public_message.contains("version"),
            "{}",
            err.public_message
        );
    }

    #[test]
    fn update_tracked_reports_original_pending_and_stored() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        // Insert WITHOUT timestamps so the update's auto-stamp is a
        // deterministic dirty field (absent before, present after).
        let bare = ModelRuntime::new(appointment_model()).without_timestamps();
        let row = bare.insert(&store, &input("p"), Some(tenant)).unwrap();
        assert!(row.get("updated_at").is_none());

        let rt = ModelRuntime::new(appointment_model());
        let mut changes = Record::new();
        changes.insert("status".into(), Json::String("cancelled".into()));
        let (stored, tracking) = rt
            .update_tracked(&store, rid(&row), Some(tenant), &changes)
            .unwrap();

        // Original snapshot predates the write.
        assert_eq!(
            tracking.original("status").and_then(Json::as_str),
            Some("booked")
        );
        assert!(tracking.is_dirty("status"));
        assert!(tracking.was_changed("status"));
        // Framework auto-change (updated_at) counts dirty too: absent on the
        // original row, stamped in the pending/stored rows.
        assert!(tracking.is_dirty("updated_at"));
        assert!(tracking.was_changed("updated_at"));
        // Untouched fields stay clean.
        assert!(!tracking.is_dirty("patient"));
        assert!(!tracking.was_changed("patient"));
        assert!(tracking
            .dirty_fields()
            .contains(&"status".to_string()));
        assert!(tracking
            .changed_fields()
            .contains(&"status".to_string()));
        assert_eq!(
            tracking.stored().get("status").and_then(Json::as_str),
            Some("cancelled")
        );
        assert_eq!(stored.get("status").and_then(Json::as_str), Some("cancelled"));
    }

    // ---------------------------------------- Wave 3 Phase 6: soft-delete trio

    #[test]
    fn soft_delete_trio_round_trips() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let rt = ModelRuntime::new(appointment_model());
        let row = rt.insert(&store, &input("p"), Some(tenant)).unwrap();
        let id = rid(&row);

        // Live row: visible to list, absent from only_trashed.
        assert_eq!(
            rt.list(&store, Some(tenant), vec![], &[], None)
                .unwrap()
                .items
                .len(),
            1
        );
        assert_eq!(
            rt.only_trashed(&store, Some(tenant), vec![], &[], None)
                .unwrap()
                .items
                .len(),
            0
        );

        // Soft delete: hidden from find/list, visible to the trashed reads.
        rt.delete(&store, id, Some(tenant)).unwrap();
        assert!(rt.find(&store, id, Some(tenant)).unwrap().is_none());
        assert!(rt.find_trashed(&store, id, Some(tenant)).unwrap().is_some());
        assert_eq!(
            rt.list(&store, Some(tenant), vec![], &[], None)
                .unwrap()
                .items
                .len(),
            0
        );
        assert_eq!(
            rt.with_trashed(&store, Some(tenant), vec![], &[], None)
                .unwrap()
                .items
                .len(),
            1
        );
        assert_eq!(
            rt.only_trashed(&store, Some(tenant), vec![], &[], None)
                .unwrap()
                .items
                .len(),
            1
        );

        // Restore: back to live; restoring an already-live row is a no-op.
        let restored = rt.restore(&store, id, Some(tenant)).unwrap();
        assert!(!matches!(
            restored.get("deleted_at"),
            Some(Json::String(s)) if !s.is_empty()
        ));
        assert!(rt.find(&store, id, Some(tenant)).unwrap().is_some());
        rt.restore(&store, id, Some(tenant)).unwrap();
        assert_eq!(
            rt.only_trashed(&store, Some(tenant), vec![], &[], None)
                .unwrap()
                .items
                .len(),
            0
        );

        // Missing rows are 404s for restore and force_delete.
        let missing = Uuid::now_v7();
        assert_eq!(
            rt.restore(&store, missing, Some(tenant)).unwrap_err().code,
            kelson_foundation::codes::NOT_FOUND
        );

        // force_delete purges outright; a second call misses.
        rt.force_delete(&store, id, Some(tenant)).unwrap();
        assert!(rt.find_trashed(&store, id, Some(tenant)).unwrap().is_none());
        assert_eq!(
            rt.force_delete(&store, id, Some(tenant))
                .unwrap_err()
                .code,
            kelson_foundation::codes::NOT_FOUND
        );
    }

    #[test]
    fn trashed_reads_refuse_hard_delete_models() {
        let store = InMemoryModelStore::new();
        let rt = ModelRuntime::new(
            ModelDef::new("Event", "events")
                .hard_deletes()
                .field(FieldDef::new("id", FieldType::Uuid).required())
                .field(FieldDef::new("name", FieldType::Text)),
        );
        let err = rt
            .only_trashed(&store, None, vec![], &[], None)
            .unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::VALIDATION);
        let err = rt.restore(&store, Uuid::now_v7(), None).unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::VALIDATION);
        // force_delete needs no soft-delete posture — a missing row is
        // simply 404.
        let err = rt.force_delete(&store, Uuid::now_v7(), None).unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::NOT_FOUND);
    }

    // ------------------------------------------ Wave 3 Phase 6: global scopes

    #[test]
    fn global_scopes_run_first_and_named_suppression_relaxes_reads() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let rt = ModelRuntime::new(appointment_model())
            .with_global_scope("only_booked", |mut f| {
                f.push(FieldFilter::eq("status", "booked"));
                f
            })
            .with_scope("cancelled", |mut f| {
                f.push(FieldFilter::eq("status", "cancelled"));
                f
            });
        let a = rt.insert(&store, &input("one"), Some(tenant)).unwrap();
        let mut changes = Record::new();
        changes.insert("status".into(), Json::String("cancelled".into()));
        rt.update(&store, rid(&a), Some(tenant), &changes).unwrap();

        // Global FIRST, then the named scope: booked AND cancelled → none.
        let page = rt
            .list(&store, Some(tenant), vec![], &["cancelled".into()], None)
            .unwrap();
        assert_eq!(page.items.len(), 0);
        assert!(rt
            .first(
                &store,
                Some(tenant),
                &[FieldFilter::eq("status", "cancelled")]
            )
            .unwrap()
            .is_none());

        // Suppress the global by name: the named scope alone matches now.
        let relaxed = rt.clone().without_global_scopes(["only_booked"]);
        let page = relaxed
            .list(&store, Some(tenant), vec![], &["cancelled".into()], None)
            .unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(relaxed
            .first(
                &store,
                Some(tenant),
                &[FieldFilter::eq("status", "cancelled")]
            )
            .unwrap()
            .is_some());
        // pluck runs the same chokepoint.
        assert_eq!(
            relaxed
                .pluck(
                    &store,
                    Some(tenant),
                    &[FieldFilter::eq("status", "cancelled")],
                    "patient"
                )
                .unwrap(),
            vec![Json::String("one".into())]
        );
        // Unknown names are ignored — suppression never errors.
        let _ = rt.clone().without_global_scopes(["nonexistent"]);
    }

    #[test]
    fn without_global_scopes_all_lifts_the_soft_delete_veil() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let rt = ModelRuntime::new(appointment_model()).with_global_scope(
            "only_booked",
            |mut f| {
                f.push(FieldFilter::eq("status", "booked"));
                f
            },
        );
        let row = rt.insert(&store, &input("p"), Some(tenant)).unwrap();
        let id = rid(&row);
        rt.delete(&store, id, Some(tenant)).unwrap();

        // Default posture: veil down, global in force.
        assert!(rt.find(&store, id, Some(tenant)).unwrap().is_none());
        assert_eq!(
            rt.list(&store, Some(tenant), vec![], &[], None)
                .unwrap()
                .items
                .len(),
            0
        );

        // All-globals suppressed: veil lifted AND every global scope gone.
        let lifted = rt.clone().without_global_scopes_all();
        assert!(lifted.find(&store, id, Some(tenant)).unwrap().is_some());
        let page = lifted
            .list(&store, Some(tenant), vec![], &[], None)
            .unwrap();
        assert_eq!(
            page.items.len(),
            1,
            "trashed row visible, no status filter left"
        );
        assert!(lifted.first(&store, Some(tenant), &[]).unwrap().is_some());
        assert_eq!(
            lifted
                .pluck(&store, Some(tenant), &[], "patient")
                .unwrap()
                .len(),
            1
        );
    }

    // ---------------------------------------------- Wave 3 Phase 6: pruning

    #[test]
    fn prune_walks_matches_in_chunked_passes() {
        let store = InMemoryModelStore::new();
        let model = ModelDef::new("Event", "events")
            .hard_deletes()
            .field(FieldDef::new("id", FieldType::Uuid).required())
            .field(FieldDef::new("kind", FieldType::Text));
        let rt = ModelRuntime::new(model.clone())
            .with_prunable(|| vec![FieldFilter::eq("kind", "expired")]);
        assert!(rt.is_prunable());
        for kind in ["expired", "expired", "expired", "keep"] {
            let mut row = Record::new();
            row.insert("id".into(), Json::String(Uuid::now_v7().to_string()));
            row.insert("kind".into(), Json::String(kind.into()));
            store.insert(&model, &row).unwrap();
        }

        // chunk=1 walks every match across passes (clamped to the 200-row
        // page cap, page 1 each pass as purged rows vanish).
        assert_eq!(rt.prune(&store, None, 1).unwrap(), 3);
        let page = store
            .list(&model, None, &[], &ListOpts::default())
            .unwrap();
        assert_eq!(page.items.len(), 1, "the live row survives");
        // An exhausted sweep purges nothing (no infinite zero-progress loop).
        assert_eq!(rt.prune(&store, None, 50).unwrap(), 0);
    }

    #[test]
    fn prune_refuses_models_without_a_predicate() {
        let rt = ModelRuntime::new(appointment_model());
        assert!(!rt.is_prunable());
        let store = InMemoryModelStore::new();
        let err = rt.prune(&store, None, 10).unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::VALIDATION);
        assert!(err.public_message.contains("not prunable"));
    }

    // --------------------------------------- Wave 3 Phase 6: without_timestamps

    #[test]
    fn without_timestamps_skips_injection_and_touch_becomes_a_noop() {
        let store = InMemoryModelStore::new();
        let tenant = TenantId::new();
        let plain = ModelRuntime::new(appointment_model());
        let bare = ModelRuntime::new(appointment_model()).without_timestamps();

        // Insert: no created_at/updated_at stamped under the flag.
        let row = bare.insert(&store, &input("b"), Some(tenant)).unwrap();
        assert!(row.get("created_at").is_none());
        assert!(row.get("updated_at").is_none());
        // Control: the flagless runtime stamps both.
        let stamped = plain.insert(&store, &input("a"), Some(tenant)).unwrap();
        assert!(stamped.get("created_at").and_then(Json::as_str).is_some());

        // Explicitly supplied timestamps still pass under the flag.
        let mut explicit = input("c");
        explicit.insert("created_at".into(), Json::String("2020-01-01T00:00:00Z".into()));
        let explicit = bare.insert(&store, &explicit, Some(tenant)).unwrap();
        assert_eq!(
            explicit.get("created_at").and_then(Json::as_str),
            Some("2020-01-01T00:00:00Z")
        );

        // Update under the flag leaves updated_at alone; touch is a no-op
        // write (nothing to stamp).
        let mut changes = Record::new();
        changes.insert("patient".into(), Json::String("z".into()));
        let updated = bare
            .update(&store, rid(&row), Some(tenant), &changes)
            .unwrap();
        assert!(updated.get("updated_at").is_none());
        let touched = bare.touch(&store, rid(&row), Some(tenant)).unwrap();
        assert!(touched.get("updated_at").is_none());
        assert_eq!(touched.get("patient").and_then(Json::as_str), Some("z"));

        // Control: touch stamps updated_at without the flag.
        let touched = plain.touch(&store, rid(&stamped), Some(tenant)).unwrap();
        assert!(touched.get("updated_at").and_then(Json::as_str).is_some());
    }
}