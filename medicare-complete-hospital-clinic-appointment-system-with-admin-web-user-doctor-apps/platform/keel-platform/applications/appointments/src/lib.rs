//! Module: appointments ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â Clinic appointments: book, cancel, list.
//! Scaffolded by `kelson make:module`, then expanded into the first real domain
//! module. Contracts live in module.toml; code lives here. State sits behind
//! the framework `ModelStore` port (S8): `InMemoryModelStore` in tests and
//! degraded boot, `PgModelStore` in production. Both backends speak the same
//! semantic CRUD contract; PostgreSQL carries the real double-booking
//! contention guard via the partial unique index on (tenant, patient, slot).

use std::sync::{Arc, Mutex};

use kelson_context::ExecutionContext;
use kelson_foundation::{
    codes, AppError, ErrorCategory, ExecutionProfileKind, OperationId, Retryability, Severity,
    TenantId,
};
use kelson_execution::{
    BoxFuture, ExecutionPlan, ExecutionUnit, MutationRecord, OperationHandler, OutboxMessage,
};
use kelson_lifecycle::{BootError, ModuleState, Phase, Provider, ServiceRegistry};
use kelson_models::{
    FieldDef, FieldFilter, FieldType, ModelDef, ModelRuntime, ModelStore, ModelStoreRef, Record,
};
use kelson_notifications::{Channel, Mailable, NotificationContext, NotificationDef, NotifyHub};
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
    let status = match record.get("status").and_then(|v| v.as_str()).unwrap_or("booked") {
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

    /// TestKit convenience: private in-memory backend.
    #[cfg(test)]
    fn in_memory() -> Self {
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
        Ok(page
            .total
            .map(|t| t as usize)
            .unwrap_or(page.items.len()))
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

    fn resolve(
        &self,
        tenant: Option<TenantId>,
        id: Uuid,
    ) -> Result<Option<Record>, AppError> {
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

/// All operation handlers of this module (host builds a CommandBus from these).
/// `notifier` is the Wave 6 P6 producer hub: `None` (hosts that never wired
/// the platform notify hub) leaves the dispatch silent ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â the `Option`
/// degradation posture every optional service follows.
pub fn handlers(
    store: Arc<AppointmentStore>,
    notifier: Option<Arc<NotifyHub>>,
) -> Vec<Arc<dyn OperationHandler>> {
    vec![
        Arc::new(
            BookHandler::new(store.clone()).with_notifier(notifier.clone()),
        ),
        Arc::new(CancelHandler::new(store.clone()).with_notifier(notifier)),
        Arc::new(ListHandler::new(store.clone())),
        Arc::new(AvailabilityHandler::new(store)),
    ]
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
        Self { state: Mutex::new(ModuleState::Discovered) }
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
            probes
                .0
                .register(Arc::new(kelson_models::StoreProbe::new(
                    appointment_model(),
                    backend.0.clone(),
                )));
        }
        // Wave 6 P6: the notify hub arrives in the Platform phase (before
        // Modules); a host that never wired one leaves the producer silent
        // (the same tolerance posture as route.bindings).
        let notifier = registry.get::<NotifyHub>().ok();
        registry.insert(
            "module.appointments.handlers",
            Arc::new(handlers(store, notifier)),
        )?;
        Ok(())
    }
}
fn text_field(input: &serde_json::Value, field: &str) -> String {
    input
        .get(field)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn require_tenant(ctx: &ExecutionContext) -> Result<TenantId, AppError> {
    ctx.tenant
        .as_ref()
        .map(|t| t.id)
        .ok_or_else(|| AppError::validation("tenant context is required"))
}

/// `appointments.book` (COMMAND): book a patient into a slot.
/// Validations: patient + slot required; double-booking the same slot is a conflict.
pub struct BookHandler {
    plan: ExecutionPlan,
    store: Arc<AppointmentStore>,
    notifier: Option<Arc<NotifyHub>>,
}

impl BookHandler {
    pub fn new(store: Arc<AppointmentStore>) -> Self {
        Self {
            plan: ExecutionPlan::new(
                OperationId::from_static(OP_BOOK),
                ExecutionProfileKind::Command,
            )
            .with_capability(OP_BOOK),
            store,
            notifier: None,
        }
    }

    /// Wave 6 P6: attach the producer hub (kept out of `new` so host and
    /// test construction stay unchanged). With it wired, a successful
    /// booking dispatches the `appointment.booked` inbox row before the
    /// operation returns; dispatch failures fail the booking (fail-closed,
    /// never a silent drop).
    pub fn with_notifier(mut self, notifier: Option<Arc<NotifyHub>>) -> Self {
        self.notifier = notifier;
        self
    }
}

impl OperationHandler for BookHandler {
    fn plan(&self) -> &ExecutionPlan {
        &self.plan
    }

    fn handle(
        &self,
        ctx: Arc<ExecutionContext>,
        input: serde_json::Value,
        unit: &mut dyn ExecutionUnit,
    ) -> BoxFuture<'static, Result<serde_json::Value, AppError>> {
        let notifier = self.notifier.clone();
        let outcome = (|| -> Result<(serde_json::Value, String, String), AppError> {
            let tenant = require_tenant(&ctx)?;
            let patient = text_field(&input, "patient");
            let slot = text_field(&input, "slot");
            if patient.is_empty() || slot.is_empty() {
                return Err(AppError::validation("patient and slot are required"));
            }
            if self.store.has_conflict(tenant, &patient, &slot)? {
                return Err(AppError::new(
                    codes::CONFLICT,
                    ErrorCategory::Conflict,
                    Severity::Low,
                    Retryability::NotRetryable,
                    format!("slot {slot} is already booked for {patient}"),
                ));
            }
            let appointment = Appointment {
                id: Uuid::now_v7(),
                tenant,
                patient: patient.clone(),
                slot: slot.clone(),
                status: AppointmentStatus::Booked,
                version: 1,
            };
            let stored = self.store.insert(appointment.clone())?;
            unit.record_mutation(MutationRecord {
                resource_type: "appointment".into(),
                resource_id: Some(stored.id),
                resource_version: stored.version,
                before: None,
                after: json!(stored),
                stream: None,
            });
            unit.queue_outbox(OutboxMessage {
                event: EVENT_BOOKED.into(),
                topic: "appointments".into(),
                payload: json!({ "id": stored.id.to_string(), "patient": patient, "slot": slot }),
            });
            Ok((
                json!({ "id": stored.id.to_string(), "status": "booked" }),
                patient,
                slot,
            ))
        })();
        Box::pin(async move {
            let (value, patient, slot) = outcome?;
            // Wave 6 P6 producer dispatch: the `appointment.booked` inbox
            // row lands through the notification store port at the end of
            // the handler, before the unit commits (the documented
            // pre-commit posture ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â same database, same health). The
            // request locale rides both the dispatch context and the row
            // payload.
            if let Some(hub) = notifier.as_ref() {
                let locale = ctx.locale.lang.clone();
                hub.dispatch_notification(
                    &AppointmentBooked {
                        patient,
                        slot,
                        locale: Some(locale.clone()),
                    },
                    Channel::Database,
                    None,
                    &NotificationContext {
                        principal_id: ctx.principal.id,
                        tenant_id: Some(require_tenant(&ctx)?),
                        locale: Some(locale),
                    },
                )
                .await?;
            }
            Ok(value)
        })
    }
}

/// `appointments.cancel` (COMMAND): cancel a booked appointment by id.
pub struct CancelHandler {
    plan: ExecutionPlan,
    store: Arc<AppointmentStore>,
    notifier: Option<Arc<NotifyHub>>,
}

impl CancelHandler {
    pub fn new(store: Arc<AppointmentStore>) -> Self {
        Self {
            plan: ExecutionPlan::new(
                OperationId::from_static(OP_CANCEL),
                ExecutionProfileKind::Command,
            )
            .with_capability(OP_CANCEL),
            store,
            notifier: None,
        }
    }

    /// Wave 6 P6: attach the producer hub (see [`BookHandler::with_notifier`]);
    /// a successful cancel dispatches `appointment.cancelled` the same way.
    pub fn with_notifier(mut self, notifier: Option<Arc<NotifyHub>>) -> Self {
        self.notifier = notifier;
        self
    }
}

impl OperationHandler for CancelHandler {
    fn plan(&self) -> &ExecutionPlan {
        &self.plan
    }

    fn handle(
        &self,
        ctx: Arc<ExecutionContext>,
        input: serde_json::Value,
        unit: &mut dyn ExecutionUnit,
    ) -> BoxFuture<'static, Result<serde_json::Value, AppError>> {
        let notifier = self.notifier.clone();
        let outcome = (|| -> Result<(serde_json::Value, String, String), AppError> {
            let tenant = require_tenant(&ctx)?;
            let id = text_field(&input, "id")
                .parse::<Uuid>()
                .map_err(|_| AppError::validation("appointment id must be a uuid"))?;
            let (before, after) = self.store.cancel(tenant, id)?;
            unit.record_mutation(MutationRecord {
                resource_type: "appointment".into(),
                resource_id: Some(after.id),
                resource_version: after.version,
                before: Some(json!(before)),
                after: json!(after),
                stream: None,
            });
            unit.queue_outbox(OutboxMessage {
                event: EVENT_CANCELLED.into(),
                topic: "appointments".into(),
                payload: json!({ "id": after.id.to_string() }),
            });
            Ok((
                json!({ "id": after.id.to_string(), "status": "cancelled" }),
                after.patient,
                after.slot,
            ))
        })();
        Box::pin(async move {
            let (value, patient, slot) = outcome?;
            // Wave 6 P6 producer dispatch ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â same pre-commit, fail-closed
            // posture as the book handler, keyed `appointment.cancelled`.
            if let Some(hub) = notifier.as_ref() {
                let locale = ctx.locale.lang.clone();
                hub.dispatch_notification(
                    &AppointmentCancelled {
                        patient,
                        slot,
                        locale: Some(locale.clone()),
                    },
                    Channel::Database,
                    None,
                    &NotificationContext {
                        principal_id: ctx.principal.id,
                        tenant_id: Some(require_tenant(&ctx)?),
                        locale: Some(locale),
                    },
                )
                .await?;
            }
            Ok(value)
        })
    }
}

/// `appointments.list` (READ): list the tenant's appointments. The read guard
/// (READ_MUTATED) proves this operation can never transition state.
pub struct ListHandler {
    plan: ExecutionPlan,
    store: Arc<AppointmentStore>,
}

impl ListHandler {
    pub fn new(store: Arc<AppointmentStore>) -> Self {
        Self {
            plan: ExecutionPlan::new(
                OperationId::from_static(OP_LIST),
                ExecutionProfileKind::Read,
            )
            .with_capability(OP_LIST),
            store,
        }
    }
}
impl OperationHandler for ListHandler {
    fn plan(&self) -> &ExecutionPlan {
        &self.plan
    }

    fn handle(
        &self,
        ctx: Arc<ExecutionContext>,
        input: serde_json::Value,
        _unit: &mut dyn ExecutionUnit,
    ) -> BoxFuture<'static, Result<serde_json::Value, AppError>> {
        let outcome = (|| -> Result<serde_json::Value, AppError> {
            let tenant = require_tenant(&ctx)?;
            // Wave 1 pagination reference: GET routes inject `_page`
            // (page/per_page, clamped at the edge). When present, return one
            // page plus a `page` meta block; the `appointments` key is kept
            // for existing consumers.
            let page_req = input.get("_page").and_then(|v| {
                let page = v.get("page")?.as_u64()? as u32;
                let per_page = v.get("per_page")?.as_u64()? as u32;
                Some(kelson_query::PageReq { page, per_page })
            });
            match page_req {
                Some(req) => {
                    let page = self.store.list_paged(tenant, req)?;
                    let has_more = page.total.map_or(
                        page.items.len() as u64 >= page.per_page as u64,
                        |total| u64::from(page.page) * u64::from(page.per_page) < total,
                    );
                    Ok(json!({
                        "appointments": page.items,
                        "page": {
                            "page": page.page,
                            "per_page": page.per_page,
                            "total": page.total,
                            "has_more": has_more,
                        }
                    }))
                }
                None => {
                    let rows = self.store.list(tenant)?;
                    Ok(json!({ "appointments": rows }))
                }
            }
        })();
        Box::pin(async move { outcome })
    }
}

/// `appointments.availability` (READ, public): the Wave 1 `public = true`
/// reference route. The implicit auth/verified stages are stripped from the
/// chain (throttle stays) and the plan opts out of authorization, so
/// anonymous callers get a 200 envelope.
pub struct AvailabilityHandler {
    plan: ExecutionPlan,
    store: Arc<AppointmentStore>,
}

impl AvailabilityHandler {
    pub fn new(store: Arc<AppointmentStore>) -> Self {
        let mut plan = ExecutionPlan::new(
            OperationId::from_static(OP_AVAILABILITY),
            ExecutionProfileKind::Read,
        );
        // Public route: the engine skips authorization entirely; the HTTP
        // edge only admits this anonymously because the plan says so.
        plan.authorization.required = false;
        Self { plan, store }
    }
}

impl OperationHandler for AvailabilityHandler {
    fn plan(&self) -> &ExecutionPlan {
        &self.plan
    }

    fn handle(
        &self,
        ctx: Arc<ExecutionContext>,
        input: serde_json::Value,
        _unit: &mut dyn ExecutionUnit,
    ) -> BoxFuture<'static, Result<serde_json::Value, AppError>> {
        let outcome = (|| -> Result<serde_json::Value, AppError> {
            let tenant = require_tenant(&ctx)?;
            let date = text_field(&input, "date");
            let booked = self.store.count(tenant)?;
            Ok(json!({ "date": date, "appointments_booked": booked }))
        })();
        Box::pin(async move { outcome })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kelson_context::{PrincipalContext, ScopeSnapshot, TenantContext};
    use kelson_notifications::NotificationStore;
    use kelson_foundation::{AuthorizationRevision, FeatureRevision, ModuleRevision, PrincipalId};
    use kelson_execution::{Engine, IdempotencyKey, InMemoryBackend, InMemoryIdempotency};
    use kelson_policy::{AccessSnapshot, PolicyAuthz, PolicyEngine};
    use time::OffsetDateTime;

    fn fixture(
        capabilities: &[&str],
    ) -> (Arc<Engine>, Arc<AppointmentStore>, PrincipalId, TenantId) {
        let principal = PrincipalId::new();
        let tenant = TenantId::new();
        let policies = Arc::new(PolicyEngine::new());
        policies.install_snapshot(AccessSnapshot {
            principal_id: principal,
            tenant_id: tenant,
            membership_active: true,
            capabilities: capabilities.iter().map(|c| c.to_string()).collect(),
            scope_paths: ["/".to_string()].into_iter().collect(),
            authorization_revision: policies.revision(),
            feature_revision: FeatureRevision(1),
            module_revision: ModuleRevision(1),
            computed_at: OffsetDateTime::now_utc(),
        });
        let engine = Arc::new(Engine::new(
            Arc::new(PolicyAuthz { engine: policies }),
            Arc::new(InMemoryBackend::new()),
            Arc::new(InMemoryIdempotency::new()),
        ));
        (
            engine,
            Arc::new(AppointmentStore::in_memory()),
            principal,
            tenant,
        )
    }

    fn ctx(principal: PrincipalId, tenant: TenantId) -> Arc<ExecutionContext> {
        ExecutionContext::builder(ExecutionProfileKind::Command, OffsetDateTime::now_utc())
            .principal(PrincipalContext {
                id: principal,
                display: Some("appointments-test".into()),
            })
            .tenant(TenantContext {
                id: tenant,
                slug: "clinic".into(),
                plan: "standard".into(),
            })
            .scope(ScopeSnapshot {
                nodes: vec![],
                revision: AuthorizationRevision(1),
                computed_at: OffsetDateTime::now_utc(),
            })
            .build()
            .unwrap()
    }

    async fn book(
        engine: &Engine,
        context: &Arc<ExecutionContext>,
        handler: &BookHandler,
        patient: &str,
        slot: &str,
    ) -> Result<kelson_execution::OperationOutcome, AppError> {
        engine
            .execute(
                context.clone(),
                handler,
                json!({ "patient": patient, "slot": slot }),
                Some(IdempotencyKey(format!("book-{patient}-{slot}"))),
            )
            .await
    }

    #[tokio::test]
    async fn booking_writes_one_stone_and_one_event() {
        let (engine, store, principal, tenant) = fixture(&[OP_BOOK, OP_LIST]);
        let context = ctx(principal, tenant);
        let handler = BookHandler::new(store.clone());
        let outcome = book(
            &engine,
            &context,
            &handler,
            "A. Patient",
            "2026-09-01T10:00Z",
        )
        .await
        .unwrap();
        assert_eq!(outcome.receipt.deltas_written, 1);
        assert_eq!(outcome.receipt.outbox_queued, 1);
        assert_eq!(store.count(tenant).unwrap(), 1);
        assert_eq!(
            store.list(tenant).unwrap()[0].status,
            AppointmentStatus::Booked
        );
    }

    /// Wave 3 Phase 6: the binding resolver's trashed hook. The row is
    /// soft-deleted behind the store's back ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â `resolve` (default bindings)
    /// hides it, `resolve_trashed` (`with_trashed` bindings) surfaces it.
    #[test]
    fn trashed_binding_resolver_surfaces_soft_deleted_rows() {
        use kelson_models::RouteBindingResolver;

        let tenant = TenantId::new();
        let backend: Arc<dyn ModelStore> = kelson_models::InMemoryModelStore::new();
        let store = Arc::new(AppointmentStore::new(backend.clone()));
        let resolver = AppointmentBindingResolver::new(store);

        // Insert a live appointment straight through the model runtime.
        let rt = ModelRuntime::new(appointment_model());
        let mut record = Record::new();
        record.insert("patient".into(), json!("A. Patient"));
        record.insert("slot".into(), json!("2026-09-01T10:00Z"));
        let row = rt.insert(&*backend, &record, Some(tenant)).unwrap();
        let id = Uuid::parse_str(row["id"].as_str().unwrap()).unwrap();

        // Live row: both hooks resolve it.
        assert!(resolver.resolve(Some(tenant), id).unwrap().is_some());
        assert!(resolver
            .resolve_trashed(Some(tenant), id)
            .unwrap()
            .is_some());

        // Soft delete: `resolve` misses, `resolve_trashed` still finds it.
        rt.delete(&*backend, id, Some(tenant)).unwrap();
        assert!(resolver.resolve(Some(tenant), id).unwrap().is_none());
        assert!(resolver
            .resolve_trashed(Some(tenant), id)
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn booking_requires_patient_and_slot() {
        let (engine, store, principal, tenant) = fixture(&[OP_BOOK]);
        let context = ctx(principal, tenant);
        let handler = BookHandler::new(store.clone());
        let error = engine
            .execute(context, &handler, json!({ "patient": "A. Patient" }), None)
            .await
            .unwrap_err();
        assert_eq!(error.code, codes::VALIDATION);
        assert_eq!(store.count(tenant).unwrap(), 0, "failed validation writes nothing");
    }

    #[tokio::test]
    async fn double_booking_the_same_slot_conflicts() {
        let (engine, store, principal, tenant) = fixture(&[OP_BOOK]);
        let context = ctx(principal, tenant);
        let handler = BookHandler::new(store.clone());
        book(
            &engine,
            &context,
            &handler,
            "A. Patient",
            "2026-09-01T10:00Z",
        )
        .await
        .unwrap();
        let error = engine
            .execute(
                context,
                &handler,
                json!({ "patient": "A. Patient", "slot": "2026-09-01T10:00Z" }),
                Some(IdempotencyKey("second".into())),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, codes::CONFLICT);
        assert_eq!(store.count(tenant).unwrap(), 1);
    }
    #[tokio::test]
    async fn cancelling_flips_status_and_conflict_on_repeat() {
        let (engine, store, principal, tenant) = fixture(&[OP_BOOK, OP_CANCEL]);
        let context = ctx(principal, tenant);
        let outcome = book(
            &engine,
            &context,
            &BookHandler::new(store.clone()),
            "A. Patient",
            "2026-09-01T10:00Z",
        )
        .await
        .unwrap();
        let id = outcome.result["id"].as_str().unwrap().to_string();

        let cancel = CancelHandler::new(store.clone());
        engine
            .execute(
                context.clone(),
                &cancel,
                json!({ "id": id }),
                Some(IdempotencyKey("cancel-1".into())),
            )
            .await
            .unwrap();
        let again = engine
            .execute(
                context,
                &cancel,
                json!({ "id": id }),
                Some(IdempotencyKey("cancel-2".into())),
            )
            .await
            .unwrap_err();
        assert_eq!(
            again.code,
            codes::CONFLICT,
            "cancelling twice is a conflict"
        );
        // Cancelled frees the slot: the conflict filter only sees BOOKED rows.
        assert!(!store
            .has_conflict(tenant, "A. Patient", "2026-09-01T10:00Z")
            .unwrap());
    }

    #[tokio::test]
    async fn cancelling_unknown_id_is_not_found() {
        let (engine, store, principal, tenant) = fixture(&[OP_CANCEL]);
        let context = ctx(principal, tenant);
        let handler = CancelHandler::new(store.clone());
        let error = engine
            .execute(
                context,
                &handler,
                json!({ "id": Uuid::now_v7().to_string() }),
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, codes::NOT_FOUND);
    }

    #[tokio::test]
    async fn listing_is_readonly_and_tenant_scoped() {
        let (engine, store, principal, tenant) = fixture(&[OP_BOOK, OP_LIST]);
        let context = ctx(principal, tenant);
        book(
            &engine,
            &context,
            &BookHandler::new(store.clone()),
            "A. Patient",
            "2026-09-01T10:00Z",
        )
        .await
        .unwrap();

        // Another tenant's booking must never leak into this tenant's list.
        let other_tenant = TenantId::new();
        store
            .insert(Appointment {
                id: Uuid::now_v7(),
                tenant: other_tenant,
                patient: "Other Tenant".into(),
                slot: "2026-09-01T11:00Z".into(),
                status: AppointmentStatus::Booked,
                version: 1,
            })
            .unwrap();

        let list = ListHandler::new(store.clone());
        let outcome = engine
            .execute(context, &list, json!({}), None)
            .await
            .unwrap();
        let rows = outcome.result["appointments"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "tenant isolation holds");
        assert_eq!(rows[0]["patient"], "A. Patient");
    }

    #[tokio::test]
    async fn principal_without_capability_is_denied_and_writes_nothing() {
        let (engine, store, principal, tenant) = fixture(&[OP_LIST]);
        let context = ctx(principal, tenant);
        let handler = BookHandler::new(store.clone());
        let error = book(
            &engine,
            &context,
            &handler,
            "A. Patient",
            "2026-09-01T10:00Z",
        )
        .await
        .unwrap_err();
        assert_eq!(error.code, codes::DENIED);
        assert_eq!(store.count(tenant).unwrap(), 0);
    }

    #[tokio::test]
    async fn replaying_the_same_booking_is_idempotent() {
        let (engine, store, principal, tenant) = fixture(&[OP_BOOK]);
        let context = ctx(principal, tenant);
        let handler = BookHandler::new(store.clone());
        let first = engine
            .execute(
                context.clone(),
                &handler,
                json!({ "patient": "A. Patient", "slot": "2026-09-01T10:00Z" }),
                Some(IdempotencyKey("same-key".into())),
            )
            .await
            .unwrap();
        let replay = engine
            .execute(
                context,
                &handler,
                json!({ "patient": "A. Patient", "slot": "2026-09-01T10:00Z" }),
                Some(IdempotencyKey("same-key".into())),
            )
            .await
            .unwrap();
        assert!(replay.receipt.replayed);
        assert_eq!(replay.result, first.result);
        assert_eq!(store.count(tenant).unwrap(), 1, "replay does not double-book");
    }

    #[test]
    fn manifest_compiles_into_the_platform_graph() {
        let manifest = kelson_registry::ModuleManifest::from_toml(include_str!("../module.toml"))
            .expect("manifest parses");
        let graph = kelson_registry::PlatformGraph::compile(vec![manifest]).expect("graph compiles");
        assert_eq!(graph.operations.len(), 4, "book + cancel + list + availability");
        assert!(graph.operations.contains_key("appointments.book"));
        assert!(graph.operations.contains_key("appointments.cancel"));
        assert!(graph.operations.contains_key("appointments.list"));
        assert_eq!(graph.events.len(), 2, "booked + cancelled events");
        assert_eq!(graph.modules[0].routes.len(), 5, "book + cancel + list + availability + history alias");
    }

    /// Test provider for the Platform phase: registers the in-memory
    /// ModelStore carrier the module provider consumes at boot.
    struct TestModelStoreProvider;

    impl Provider for TestModelStoreProvider {
        fn name(&self) -> &str {
            "test.model-store"
        }
        fn phase(&self) -> Phase {
            Phase::Platform
        }
        fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
            r.insert(
                "model.store",
                Arc::new(ModelStoreRef(Arc::new(
                    kelson_models::InMemoryModelStore::new(),
                ))),
            )?;
            Ok(())
        }
    }

    #[tokio::test]
    async fn provider_boots_and_registers_the_handler_bundle() {
        let mut host = kelson_lifecycle::ApplicationHost::new();
        host.add_provider(Box::new(TestModelStoreProvider));
        host.add_provider(Box::new(ModuleProvider::new()));
        host.boot().await.expect("module boots");
        let bundle = host
            .registry()
            .get::<Vec<Arc<dyn OperationHandler>>>()
            .expect("handlers registered");
        assert_eq!(bundle.len(), 4);
        let store = host
            .registry()
            .get::<AppointmentStore>()
            .expect("store registered");
        assert!(store.list(TenantId::new()).unwrap().is_empty());
    }

    #[tokio::test]
    async fn provider_fails_boot_without_a_model_store() {
        let mut host = kelson_lifecycle::ApplicationHost::new();
        host.add_provider(Box::new(ModuleProvider::new()));
        assert!(
            host.boot().await.is_err(),
            "module must refuse to boot without the ModelStore port"
        );
    }

    // ------------------------------------------------- Wave 6 P6: producer dispatch

    /// Same shape as [`ctx`] with a pinned request locale ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â the P6 e2e
    /// asserts the producer carries it into the row payload.
    fn ctx_lang(principal: PrincipalId, tenant: TenantId, lang: &str) -> Arc<ExecutionContext> {
        ExecutionContext::builder(ExecutionProfileKind::Command, OffsetDateTime::now_utc())
            .principal(PrincipalContext {
                id: principal,
                display: Some("appointments-test".into()),
            })
            .tenant(TenantContext {
                id: tenant,
                slug: "clinic".into(),
                plan: "standard".into(),
            })
            .scope(ScopeSnapshot {
                nodes: vec![],
                revision: AuthorizationRevision(1),
                computed_at: OffsetDateTime::now_utc(),
            })
            .locale(kelson_foundation::Locale {
                lang: lang.into(),
                timezone: "UTC".into(),
            })
            .build()
            .unwrap()
    }

    /// Wave 6 P6: a successful booking dispatches the `appointment.booked`
    /// inbox row through the hub ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â addressed to the caller's principal in
    /// the caller's locale (the payload carries it), landing in the very
    /// store shape the HTTP feed reads.
    #[tokio::test]
    async fn producer_dispatches_the_inbox_row_and_carries_the_locale() {
        let (engine, store, principal, tenant) = fixture(&[OP_BOOK, OP_LIST]);
        let context = ctx_lang(principal, tenant, "fr");
        let inbox = Arc::new(kelson_notifications::InMemoryNotificationStore::new());
        let hub = Arc::new(kelson_notifications::NotifyHub::empty().with_store(inbox.clone()));
        let handler = BookHandler::new(store.clone()).with_notifier(Some(hub));

        book(&engine, &context, &handler, "A. Patient", "2026-09-01T10:00Z")
            .await
            .expect("booking succeeds");

        let page = inbox.list(principal, 0, 10).await.expect("inbox reads");
        assert_eq!(page.total, 1);
        assert_eq!(page.unread, 1);
        let row = &page.rows[0];
        assert_eq!(row.kind, "appointment.booked");
        assert_eq!(row.principal_id, principal);
        assert_eq!(row.tenant_id, Some(tenant));
        assert_eq!(row.data["patient"], "A. Patient");
        assert_eq!(row.data["slot"], "2026-09-01T10:00Z");
        assert_eq!(row.data["locale"], "fr", "request locale rides the row");
        assert!(row.read_at.is_none());
    }

    /// Wave 6 P6: cancellation dispatches its own `appointment.cancelled`
    /// row alongside the booking's ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â two events, two inbox rows.
    #[tokio::test]
    async fn cancel_dispatches_the_cancelled_inbox_row() {
        let (engine, store, principal, tenant) = fixture(&[OP_BOOK, OP_CANCEL]);
        let context = ctx(principal, tenant);
        let inbox = Arc::new(kelson_notifications::InMemoryNotificationStore::new());
        let hub = Arc::new(kelson_notifications::NotifyHub::empty().with_store(inbox.clone()));
        let book_handler = BookHandler::new(store.clone()).with_notifier(Some(hub.clone()));
        let outcome = book(
            &engine,
            &context,
            &book_handler,
            "B. Patient",
            "2026-09-02T11:00Z",
        )
        .await
        .expect("booking succeeds");
        let id = outcome.result["id"].as_str().unwrap().to_string();

        let cancel = CancelHandler::new(store.clone()).with_notifier(Some(hub));
        engine
            .execute(
                context.clone(),
                &cancel,
                json!({ "id": id }),
                Some(IdempotencyKey("cancel-p6-1".into())),
            )
            .await
            .expect("cancel succeeds");

        let page = inbox.list(principal, 0, 10).await.expect("inbox reads");
        assert_eq!(page.total, 2);
        assert_eq!(page.rows[0].kind, "appointment.cancelled", "newest first");
        assert_eq!(page.rows[1].kind, "appointment.booked");
        assert_eq!(page.rows[0].data["patient"], "B. Patient");
    }

    /// Wave 6 P6: a wired hub that cannot store the row fails the
    /// operation (fail-closed) ÃƒÆ’Ã‚Â¢ÃƒÂ¢Ã¢â‚¬Å¡Ã‚Â¬ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â the producer never drops its dispatch
    /// silently; a host with no hub at all stays on the degraded path
    /// the untouched booking tests already prove.
    #[tokio::test]
    async fn producer_dispatch_failure_fails_the_operation() {
        let (engine, store, principal, tenant) = fixture(&[OP_BOOK]);
        let context = ctx(principal, tenant);
        let hub = Arc::new(kelson_notifications::NotifyHub::empty());
        let handler = BookHandler::new(store.clone()).with_notifier(Some(hub));

        let err = book(
            &engine,
            &context,
            &handler,
            "C. Patient",
            "2026-09-03T12:00Z",
        )
        .await
        .expect_err("dispatch failure fails the booking");
        assert_eq!(err.code, kelson_foundation::codes::NOT_IMPLEMENTED);
    }
}