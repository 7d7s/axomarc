//! Module: appointments ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â Clinic appointments: book, cancel, list.
//! Scaffolded by `kelson make:module`, then expanded into the first real domain
//! module. Contracts live in module.toml; code lives here. State sits behind
//! the framework `ModelStore` port (S8): `InMemoryModelStore` in tests and
//! degraded boot, `PgModelStore` in production. Both backends speak the same
//! semantic CRUD contract; PostgreSQL carries the real double-booking
//! contention guard via the partial unique index on (tenant, patient, slot).

use std::sync::{Arc, Mutex};

use kelson_foundation::{codes, AppError, ErrorCategory, Retryability, Severity, TenantId};
use kelson_lifecycle::{BootError, ModuleState, Phase, Provider, ServiceRegistry};
use kelson_models::{
    FieldDef, FieldFilter, FieldType, ModelDef, ModelRuntime, ModelStore, ModelStoreRef, Record,
};
use kelson_notifications::{Mailable, NotificationDef};
use serde_json::json;
use uuid::Uuid;

pub const OP_BOOK: &str = "appointments.book";
pub const OP_CANCEL: &str = "appointments.cancel";
pub const OP_LIST: &str = "appointments.list";
pub const OP_AVAILABILITY: &str = "appointments.availability";

pub const EVENT_BOOKED: &str = "appointment.booked";
pub const EVENT_CANCELLED: &str = "appointment.cancelled";

/// Model names this module registers binding resolvers for in `route.bindings`
/// (static knowledge for tooling such as the `kelson check`
/// GRAPH-BINDING-RESOLVER gate; production registration happens in
/// `Provider::init`).
pub const REGISTERED_BINDING_MODELS: &[&str] = &["Appointment"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppointmentStatus {
    Booked,
    Cancelled,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Appointment {
    pub id: Uuid,
    pub tenant: TenantId,
    pub patient: String,
    pub slot: String,
    pub status: AppointmentStatus,
    pub version: i64,
}

/// The persisted model: mirrors the `appointments` migration exactly.
pub fn appointment_model() -> ModelDef {
    ModelDef::new("Appointment", "appointments")
        .tenant_scoped()
        .field(FieldDef::new("id", FieldType::Uuid).required())
        .field(FieldDef::new("tenant_id", FieldType::Uuid).required())
        .field(FieldDef::new("patient", FieldType::Text).required())
        .field(FieldDef::new("slot", FieldType::Text).required())
        .field(
            FieldDef::new(
                "status",
                FieldType::Enum {
                    variants: vec!["booked".into(), "cancelled".into()],
                },
            )
            .required()
            .with_default("'booked'"),
        )
        .field(
            FieldDef::new("version", FieldType::Int64)
                .required()
                .with_default("1"),
        )
        .field(
            FieldDef::new("created_at", FieldType::Timestamp)
                .required()
                .with_default("now()"),
        )
        .field(
            FieldDef::new("updated_at", FieldType::Timestamp)
                .required()
                .with_default("now()"),
        )
        .field(FieldDef::new("deleted_at", FieldType::Timestamp))
}

fn status_str(status: AppointmentStatus) -> &'static str {
    match status {
        AppointmentStatus::Booked => "booked",
        AppointmentStatus::Cancelled => "cancelled",
    }
}

fn from_record(record: &Record) -> Result<Appointment, AppError> {
    let parse_uuid = |name: &str| -> Result<Uuid, AppError> {
        record
            .get(name)
            .and_then(|v| v.as_str())
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| AppError::validation(format!("appointment record missing {name}")))
    };
    let status = match record
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("booked")
    {
        "cancelled" => AppointmentStatus::Cancelled,
        _ => AppointmentStatus::Booked,
    };
    Ok(Appointment {
        id: parse_uuid("id")?,
        tenant: TenantId::from_uuid(parse_uuid("tenant_id")?),
        patient: record
            .get("patient")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        slot: record
            .get("slot")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        status,
        version: record.get("version").and_then(|v| v.as_i64()).unwrap_or(1),
    })
}
/// Domain store over the framework [`ModelStore`] port. Every capability is
/// expressed through the portable model contract (filters, status updates,
/// tenant scope); the backend supplies the concurrency mechanism (in-memory:
/// filter scan; PostgreSQL: partial unique index).
#[derive(Clone)]
pub struct AppointmentStore {
    backend: Arc<dyn ModelStore>,
    runtime: ModelRuntime,
}

impl AppointmentStore {
    pub fn new(backend: Arc<dyn ModelStore>) -> Self {
        Self {
            backend,
            runtime: ModelRuntime::new(appointment_model()),
        }
    }

    /// An in-memory backend, for degraded boot and for tests that need to
    /// exercise the store without a database. It lives here rather than
    /// behind `cfg(test)` so the module's guest crate -- a *different*
    /// crate, which links this one as its contract -- can reach it from its
    /// own suite.
    pub fn in_memory() -> Self {
        Self::new(Arc::new(kelson_models::InMemoryModelStore::new()))
    }

    /// True when the patient already holds a BOOKED appointment in this slot.
    pub fn has_conflict(
        &self,
        tenant: TenantId,
        patient: &str,
        slot: &str,
    ) -> Result<bool, AppError> {
        let page = self.runtime.list(
            &*self.backend,
            Some(tenant),
            vec![
                FieldFilter::eq("status", "booked"),
                FieldFilter::eq("patient", patient),
                FieldFilter::eq("slot", slot),
            ],
            &[],
            None,
        )?;
        Ok(page.total.map(|t| t > 0).unwrap_or(!page.items.is_empty()))
    }

    pub fn insert(&self, appointment: Appointment) -> Result<Appointment, AppError> {
        let mut input = Record::new();
        input.insert("id".into(), json!(appointment.id.to_string()));
        input.insert("patient".into(), json!(appointment.patient));
        input.insert("slot".into(), json!(appointment.slot));
        input.insert("status".into(), json!(status_str(appointment.status)));
        input.insert("version".into(), json!(appointment.version));
        let stored = self
            .runtime
            .insert(&*self.backend, &input, Some(appointment.tenant))?;
        from_record(&stored)
    }

    /// Flip a booked appointment to cancelled. NOT_FOUND when the id is
    /// unknown in this tenant; CONFLICT when it is already cancelled.
    pub fn cancel(
        &self,
        tenant: TenantId,
        id: Uuid,
    ) -> Result<(Appointment, Appointment), AppError> {
        let current = self.runtime.get(&*self.backend, id, Some(tenant))?;
        let before = from_record(&current)?;
        if before.status == AppointmentStatus::Cancelled {
            return Err(AppError::new(
                codes::CONFLICT,
                ErrorCategory::Conflict,
                Severity::Low,
                Retryability::NotRetryable,
                format!("appointment {id} is already cancelled"),
            ));
        }
        let mut changes = Record::new();
        changes.insert("status".into(), json!("cancelled"));
        changes.insert("version".into(), json!(before.version + 1));
        let stored = self
            .runtime
            .update(&*self.backend, id, Some(tenant), &changes)?;
        Ok((before, from_record(&stored)?))
    }

    pub fn list(&self, tenant: TenantId) -> Result<Vec<Appointment>, AppError> {
        let page = self
            .runtime
            .list(&*self.backend, Some(tenant), vec![], &[], None)?;
        page.items.iter().map(from_record).collect()
    }

    pub fn count(&self, tenant: TenantId) -> Result<usize, AppError> {
        let page = self
            .runtime
            .list(&*self.backend, Some(tenant), vec![], &[], None)?;
        Ok(page.total.map(|t| t as usize).unwrap_or(page.items.len()))
    }

    /// Wave 1 model binding support: tenant-scoped find for the route
    /// binding resolver. `Ok(None)` = miss (the edge maps it to a 404).
    pub fn find_for_binding(
        &self,
        tenant: Option<TenantId>,
        id: Uuid,
    ) -> Result<Option<Record>, AppError> {
        self.runtime.find(&*self.backend, id, tenant)
    }

    /// Wave 3 Phase 6: trashed-aware binding find (`with_trashed` route
    /// bindings) ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â surfaces soft-deleted appointments too.
    pub fn find_for_binding_trashed(
        &self,
        tenant: Option<TenantId>,
        id: Uuid,
    ) -> Result<Option<Record>, AppError> {
        self.runtime.find_trashed(&*self.backend, id, tenant)
    }

    /// Wave 1 pagination reference: one page of the tenant's appointments.
    pub fn list_paged(
        &self,
        tenant: TenantId,
        page: kelson_query::PageReq,
    ) -> Result<kelson_query::Page<Appointment>, AppError> {
        let page = page.validate()?;
        let result = self
            .runtime
            .list(&*self.backend, Some(tenant), vec![], &[], Some(page))?;
        let items = result
            .items
            .iter()
            .map(from_record)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(kelson_query::Page::new(
            items,
            result.page,
            result.per_page,
            result.total,
        ))
    }
}

/// Wave 1 model binding reference: resolves `{id}` path parameters to an
/// `Appointment` record (tenant-scoped). Registered in the well-known
/// `route.bindings` service by the module provider; the HTTP edge injects the
/// record as `_bindings.id` and maps misses/cross-tenant lookups to 404.
pub struct AppointmentBindingResolver {
    store: Arc<AppointmentStore>,
}

impl AppointmentBindingResolver {
    pub fn new(store: Arc<AppointmentStore>) -> Self {
        Self { store }
    }
}

impl kelson_models::RouteBindingResolver for AppointmentBindingResolver {
    fn model(&self) -> &str {
        "Appointment"
    }

    fn resolve(&self, tenant: Option<TenantId>, id: Uuid) -> Result<Option<Record>, AppError> {
        self.store.find_for_binding(tenant, id)
    }

    /// Wave 3 Phase 6: `with_trashed` bindings resolve soft-deleted
    /// appointments (the default trait impl would fail closed).
    fn resolve_trashed(
        &self,
        tenant: Option<TenantId>,
        id: Uuid,
    ) -> Result<Option<Record>, AppError> {
        self.store.find_for_binding_trashed(tenant, id)
    }
}

/// Wave 6 P6: `appointment.booked` ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â the producer notification the book
/// handler dispatches on the Database channel (the caller's inbox row).
/// Mail/SMS text resolves `notifications.appointment_booked` through the
/// hub catalog with `{patient}`/`{slot}` interpolated from the payload
/// scalars; the payload also carries the dispatch locale so readers see
/// which language the row was produced in. `template()` stays `None`: no
/// view file ships yet, so mail stays text-only instead of failing closed
/// on a missing view once a view port is wired.
#[derive(Debug, Clone)]
pub struct AppointmentBooked {
    pub patient: String,
    pub slot: String,
    pub locale: Option<String>,
}

impl NotificationDef for AppointmentBooked {
    fn type_key(&self) -> &str {
        EVENT_BOOKED
    }

    fn to_mail(&self) -> Mailable {
        Mailable {
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            from: None,
            reply_to: None,
            subject: "notifications.appointment_booked".into(),
            text: Some("notifications.appointment_booked".into()),
            html: None,
            headers: Vec::new(),
            attachments: Vec::new(),
        }
    }

    fn to_sms(&self) -> String {
        "notifications.appointment_booked".into()
    }

    fn to_push(&self) -> serde_json::Value {
        json!({})
    }

    fn to_database(&self) -> serde_json::Value {
        json!({ "patient": self.patient, "slot": self.slot, "locale": self.locale })
    }

    fn template(&self) -> Option<String> {
        None
    }
}

/// Wave 6 P6: `appointment.cancelled` ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â the cancel handler's producer
/// notification (same posture as [`AppointmentBooked`], keyed
/// `notifications.appointment_cancelled`).
#[derive(Debug, Clone)]
pub struct AppointmentCancelled {
    pub patient: String,
    pub slot: String,
    pub locale: Option<String>,
}

impl NotificationDef for AppointmentCancelled {
    fn type_key(&self) -> &str {
        EVENT_CANCELLED
    }

    fn to_mail(&self) -> Mailable {
        Mailable {
            to: Vec::new(),
            cc: Vec::new(),
            bcc: Vec::new(),
            from: None,
            reply_to: None,
            subject: "notifications.appointment_cancelled".into(),
            text: Some("notifications.appointment_cancelled".into()),
            html: None,
            headers: Vec::new(),
            attachments: Vec::new(),
        }
    }

    fn to_sms(&self) -> String {
        "notifications.appointment_cancelled".into()
    }

    fn to_push(&self) -> serde_json::Value {
        json!({})
    }

    fn to_database(&self) -> serde_json::Value {
        json!({ "patient": self.patient, "slot": self.slot, "locale": self.locale })
    }

    fn template(&self) -> Option<String> {
        None
    }
}

/// Module provider: binds the domain store to the framework ModelStore port
/// (registered by the Platform phase) and registers the handler bundle.
/// Tracks the module's lifecycle state (Discovered -> Validated -> Registered
/// -> Initialized -> Ready -> Degraded/Disabled).
pub struct ModuleProvider {
    state: Mutex<ModuleState>,
}

impl ModuleProvider {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(ModuleState::Discovered),
        }
    }

    pub fn state(&self) -> ModuleState {
        *self.state.lock().unwrap()
    }

    #[allow(dead_code)]
    fn set_state(&self, state: ModuleState) {
        *self.state.lock().unwrap() = state;
    }

    #[allow(dead_code)]
    fn transition_to_validated(&self) {
        *self.state.lock().unwrap() = ModuleState::Validated;
    }

    #[allow(dead_code)]
    fn transition_to_registered(&self) {
        *self.state.lock().unwrap() = ModuleState::Registered;
    }

    #[allow(dead_code)]
    fn transition_to_ready(&self) {
        *self.state.lock().unwrap() = ModuleState::Ready;
    }
}

impl Default for ModuleProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for ModuleProvider {
    fn name(&self) -> &str {
        "module.appointments"
    }
    fn phase(&self) -> Phase {
        Phase::Modules
    }
    fn init(&self, registry: &mut ServiceRegistry) -> Result<(), BootError> {
        let backend = registry.get::<ModelStoreRef>()?;
        let store = Arc::new(AppointmentStore::new(backend.0.clone()));
        registry.insert("module.appointments.store", store.clone())?;
        // Wave 1 model binding reference: register the Appointment resolver in
        // the route.bindings service when the platform provides one. Its
        // absence is tolerated so older hosts (and the ModelStore-only boot
        // failure test) are unaffected.
        if let Ok(bindings) = registry.get::<kelson_models::RouteBindingsRef>() {
            bindings
                .0
                .register(Arc::new(AppointmentBindingResolver::new(store.clone())));
        }
        // Wave 4 Phase 4: model.probes ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â register the Appointment row probe
        // for the `unique`/`exists` validation rules when the platform
        // provides the registry (same tolerance posture as route.bindings).
        if let Ok(probes) = registry.get::<kelson_models::ProbeRegistryRef>() {
            probes.0.register(Arc::new(kelson_models::StoreProbe::new(
                appointment_model(),
                backend.0.clone(),
            )));
        }
        Ok(())
    }
}
