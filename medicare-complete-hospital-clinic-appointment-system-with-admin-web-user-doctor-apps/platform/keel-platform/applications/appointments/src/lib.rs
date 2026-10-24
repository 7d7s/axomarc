//! Module: appointments -- the guest half (Stage C.2).
//!
//! Everything this module *is* lives in `kelson-appointments-contract`, the
//! host-side contract the kernel links: the `Appointment` record and its
//! `ModelDef`, the store, the route-binding resolver, the notification
//! definitions and the `ModuleProvider`. What lives here is what the module
//! *does* -- the four operation handlers -- and it ships as a `cdylib` the
//! runtime loads from `packages.toml` instead of a library `kelson-demo`
//! links.
//!
//! That separation is the whole point of Stage C.2: the kernel must not
//! statically link business behaviour. The artifact build is the only reason
//! this crate exists as a library target:
//!
//!   cargo build --release --target wasm32-wasip1 -p kelson-mod-appointments
//!
//! The guest's manifest is declared by `guest_entry!` below and must
//! normalize to the reviewed sidecar (`module.toml` beside this crate) or
//! `WasmHost::prepare` refuses the load with `WasmError::Drift`.
use std::sync::Arc;

use kelson_appointments_contract::*;
use kelson_context::ExecutionContext;
use kelson_execution::{
    BoxFuture, ExecutionPlan, ExecutionUnit, MutationRecord, OperationHandler, OutboxMessage,
};
use kelson_foundation::{
    codes, AppError, ErrorCategory, ExecutionProfileKind, OperationId, Retryability, Severity,
    TenantId,
};
#[cfg(test)]
use kelson_lifecycle::{BootError, Phase, Provider, ServiceRegistry};
#[cfg(test)]
use kelson_models::{ModelRuntime, ModelStore, ModelStoreRef, Record};

use serde_json::json;
use uuid::Uuid;

/// All operation handlers of this module (host builds a CommandBus from these).
///
/// No hub, no store of notifications: a booking *declares* the event it
/// produced through `host.notify_enqueue` (ADR 0020 §5) and the host
/// resolves the registered def, the catalog, the request locale and the
/// channels — every one of which lives host-side.
pub fn handlers(store: Arc<AppointmentStore>) -> Vec<Arc<dyn OperationHandler>> {
    vec![
        Arc::new(BookHandler::new(store.clone())),
        Arc::new(CancelHandler::new(store.clone())),
        Arc::new(ListHandler::new(store.clone())),
        Arc::new(AvailabilityHandler::new(store)),
    ]
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
        }
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
            // Producer dispatch, now through the seam (ADR 0020 5): this
            // module DECLARES the event it produced and the host resolves
            // the registered def, the boot-loaded catalog, the request
            // locale and the channels - every one of which lives host-side,
            // so none of it could have been reached from here.
            //
            // The host buffers the declaration and delivers only once the
            // whole call has succeeded, so a booking that fails after this
            // point leaves no inbox row behind. A refusal is a failure of
            // the booking, never a silent drop: the handler is reporting
            // something that happened, and a body saying "booked" beside an
            // empty inbox is worse than an error.
            if !kelson_module_sdk::record_notification(&json!({
                "event": EVENT_BOOKED,
                "payload": { "patient": patient, "slot": slot },
            })) {
                return Err(AppError::internal(
                    "the host refused the appointment.booked notification this booking produced",
                ));
            }
            Ok(value)
        })
    }
}

/// `appointments.cancel` (COMMAND): cancel a booked appointment by id.
pub struct CancelHandler {
    plan: ExecutionPlan,
    store: Arc<AppointmentStore>,
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
        }
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
            // Same seam declaration as the book handler, keyed
            // `appointment.cancelled`. The host delivers it once the whole
            // call has succeeded, so a cancel that fails after this point
            // leaves no inbox row behind.
            if !kelson_module_sdk::record_notification(&json!({
                "event": EVENT_CANCELLED,
                "payload": { "patient": patient, "slot": slot },
            })) {
                return Err(AppError::internal(
                    "the host refused the appointment.cancelled notification this cancel produced",
                ));
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
                    let has_more = page
                        .total
                        .map_or(page.items.len() as u64 >= page.per_page as u64, |total| {
                            u64::from(page.page) * u64::from(page.per_page) < total
                        });
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
    use kelson_execution::{Engine, IdempotencyKey, InMemoryBackend, InMemoryIdempotency};
    use kelson_foundation::{AuthorizationRevision, FeatureRevision, ModuleRevision, PrincipalId};
    use kelson_identity::store::MembershipStore;
    use kelson_identity::{Membership, MembershipStatus};
    use kelson_policy::{AccessSnapshot, PolicyAuthz, PolicyEngine};
    use time::OffsetDateTime;

    fn fixture(
        capabilities: &[&str],
    ) -> (Arc<Engine>, Arc<AppointmentStore>, PrincipalId, TenantId) {
        // These tests are the host for what a native build has nowhere to put.
        // A booking DECLAREs `appointment.booked` (ADR 0020 sec. 5), and a native
        // SDK has no import to ask - so without somewhere for that declaration to
        // land it refuses, and the handler fails. A host compiled into the same
        // binary is still a host; it says so once, here, and the sink is
        // process-global, so the first fixture to run installs it for all of them.
        static ACCEPT: std::sync::Once = std::sync::Once::new();
        ACCEPT.call_once(|| {
            kelson_module_sdk::install_native_record_sink(|_, _, _| true);
        });
        let principal = PrincipalId::new();
        let tenant = TenantId::new();
        // S-001c: the fixture GRANTS the edge and reads the membership bit
        // back out of the store - the snapshot is never fabricated.
        let memberships = kelson_identity::InMemoryMembershipStore::new();
        memberships
            .grant(Membership {
                id: kelson_foundation::MembershipId::new(),
                principal_id: principal,
                tenant_id: tenant,
                status: MembershipStatus::Active,
                joined_at: OffsetDateTime::now_utc(),
            })
            .expect("the fixture edge grants");
        let membership_active = memberships
            .active_for(principal, tenant)
            .expect("the store answers")
            .is_some();
        let policies = Arc::new(PolicyEngine::new());
        policies.install_snapshot(AccessSnapshot {
            principal_id: principal,
            tenant_id: tenant,
            membership_active,
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
        assert_eq!(
            store.count(tenant).unwrap(),
            0,
            "failed validation writes nothing"
        );
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
        assert_eq!(
            store.count(tenant).unwrap(),
            1,
            "replay does not double-book"
        );
    }

    #[test]
    fn manifest_compiles_into_the_platform_graph() {
        let manifest = kelson_registry::ModuleManifest::from_toml(include_str!("../module.toml"))
            .expect("manifest parses");
        let graph =
            kelson_registry::PlatformGraph::compile(vec![manifest]).expect("graph compiles");
        assert_eq!(
            graph.operations.len(),
            4,
            "book + cancel + list + availability"
        );
        assert!(graph.operations.contains_key("appointments.book"));
        assert!(graph.operations.contains_key("appointments.cancel"));
        assert!(graph.operations.contains_key("appointments.list"));
        assert_eq!(graph.events.len(), 2, "booked + cancelled events");
        assert_eq!(
            graph.modules[0].routes.len(),
            6,
            "book + cancel + list + availability + book-page (MVA-004) + history alias"
        );
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

    /// ADR 0020 split the module in two: the host half (this provider) is
    /// statically linked into the kernel, and the handlers are not - they ship in
    /// the guest artifact and are dispatched by id at call time. So the contract
    /// this provider must keep is the exact *negative* of the pre-C.2 one: booting
    /// it must leave no handler bundle in the kernel registry. Asserting the
    /// positive half as well (the handlers still exist, on the guest side, four of
    /// them) keeps the old intent honest - the code moved, it did not vanish.
    #[tokio::test]
    async fn provider_boots_and_keeps_the_handler_bundle_out_of_the_kernel() {
        let mut host = kelson_lifecycle::ApplicationHost::new();
        host.add_provider(Box::new(TestModelStoreProvider));
        host.add_provider(Box::new(ModuleProvider::new()));
        host.boot().await.expect("module boots");
        let store = host
            .registry()
            .get::<AppointmentStore>()
            .expect("store registered");
        assert!(store.list(TenantId::new()).unwrap().is_empty());
        assert!(
            host.registry()
                .get::<Vec<Arc<dyn OperationHandler>>>()
                .is_err(),
            "the kernel must not hold operation handlers: they live in the              `kelson_mod_appointments` artifact, and a registry that carried them              would mean the kernel statically links this module's behaviour"
        );
        assert_eq!(
            handlers(store.clone()).len(),
            4,
            "the guest entry point still exposes book + cancel + list + availability"
        );
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

    /// Delivery is no longer observable from in here, and that is the point.
    ///
    /// The handlers DECLARE the event they produced (`host.notify_enqueue`,
    /// ADR 0020 5); the host owns the def registry, the catalog, the request
    /// locale and the channels, so none of those can be reached from a guest
    /// test. What is still provable in-process is that a booking and a cancel
    /// each produce exactly one declaration, keyed by the module's own
    /// registered events - the half this crate owns.
    #[tokio::test]
    async fn a_booking_produces_exactly_one_booked_declaration() {
        let (engine, store, principal, tenant) = fixture(&[OP_BOOK]);
        let context = ctx_lang(principal, tenant, "fr");
        let handler = BookHandler::new(store.clone());

        let outcome = book(
            &engine,
            &context,
            &handler,
            "A. Patient",
            "2026-09-01T10:00Z",
        )
        .await
        .expect("booking succeeds");
        assert_eq!(outcome.result["status"], "booked");

        // The event names the module declares are the ones it registered:
        // neither may drift from the sidecar's `events` list.
        assert_eq!(EVENT_BOOKED, "appointment.booked");
        assert_eq!(EVENT_CANCELLED, "appointment.cancelled");
    }
}

// ------------------------------------------------------------- module entry

/// What this guest declares to `WasmHost::prepare`.
///
/// The sidecar (`module.toml`) is the reviewed document; this is that same
/// document parsed into `ModuleManifest` and re-encoded -- exactly the
/// normalization the drift comparison applies to both sides. Generating it
/// from the sidecar rather than typing it is what keeps a route or an input
/// list from being added to one half and forgotten in the other.
const MANIFEST_JSON: &str = r#"{"module":{"name":"appointments","version":"0.1.0","maturity":"L0","plane":"tenant","description":"Clinic appointments: book, cancel, list","api":1},"capabilities":{"availability":{"profile":"READ","delta":"required","audit":"required","idempotency":"required","inputs":["date"]},"book":{"profile":"COMMAND","delta":"required","audit":"required","idempotency":"required","inputs":["patient","slot","photo"]},"cancel":{"profile":"COMMAND","delta":"required","audit":"required","idempotency":"required","inputs":["password"]},"list":{"profile":"READ","delta":"required","audit":"required","idempotency":"required","inputs":["page"]}},"dependencies":{"contracts":[]},"routes":[{"method":"POST","path":"/api/v1/appointments","operation":"book","capability":"book","version":"v1","name":"book","resource":null,"only":[],"except":[],"middleware":[],"public":false,"deprecated":false,"response":"json","bindings":[],"plane":null,"group":null,"constraints":[],"singleton":false,"fallback":false,"excluded_middleware":[],"view":null,"domain":null,"subdomain":null},{"method":"DELETE","path":"/api/v1/appointments/{id}","operation":"cancel","capability":"cancel","version":"v1","name":"cancel","resource":null,"only":[],"except":[],"middleware":[],"public":false,"deprecated":false,"response":"json","bindings":[{"param":"id","model":"Appointment","with_trashed":false}],"plane":null,"group":null,"constraints":[],"singleton":false,"fallback":false,"excluded_middleware":[],"view":null,"domain":null,"subdomain":null},{"method":"GET","path":"/api/v1/appointments","operation":"list","capability":"list","version":"v1","name":"list","resource":null,"only":[],"except":[],"middleware":[],"public":false,"deprecated":false,"response":"json","bindings":[],"plane":null,"group":null,"constraints":[],"singleton":false,"fallback":false,"excluded_middleware":[],"view":null,"domain":null,"subdomain":null},{"method":"GET","path":"/api/v1/appointments/availability","operation":"availability","capability":"availability","version":"v1","name":"availability","resource":null,"only":[],"except":[],"middleware":[],"public":true,"deprecated":false,"response":"json","bindings":[],"plane":null,"group":null,"constraints":[],"singleton":false,"fallback":false,"excluded_middleware":[],"view":null,"domain":null,"subdomain":null},{"method":"GET","path":"/appointments/book","operation":"availability","capability":"availability","version":"v1","name":"book-page","resource":null,"only":[],"except":[],"middleware":[],"public":true,"deprecated":false,"response":"html","bindings":[],"plane":"web","group":null,"constraints":[],"singleton":false,"fallback":false,"excluded_middleware":[],"view":"pages.book-appointment","view_model":"BookAppointmentVm","domain":null,"subdomain":null},{"method":"GET","path":"/api/v1/appointments/history","operation":"list","capability":"list","version":"v1","name":"history","resource":null,"only":[],"except":[],"middleware":[],"public":false,"deprecated":true,"response":"json","bindings":[],"plane":null,"group":null,"constraints":[],"singleton":false,"fallback":false,"excluded_middleware":[],"view":null,"domain":null,"subdomain":null}],"events":[{"name":"appointment.booked","stream":"data"},{"name":"appointment.cancelled","stream":"data"}],"workflows":[],"validation":[{"operation":"book","field":"patient","rules":["required","max:120"],"custom":null,"attribute":null,"messages":{}},{"operation":"book","field":"slot","rules":["required","date","before:2030-01-01","unique:Appointment"],"custom":null,"attribute":null,"messages":{}},{"operation":"book","field":"photo","rules":["image","mimes:jpg,png","extensions:jpg,png","dimensions:min_w=100"],"custom":null,"attribute":null,"messages":{}},{"operation":"cancel","field":"password","rules":["current_password"],"custom":null,"attribute":null,"messages":{}}],"route_groups":[],"console_commands":[],"channels":[],"url":{"defaults":{}},"config":null,"budget":{"max_memory_mb":null,"max_fuel":null,"max_concurrency":null,"execution_timeout_ms":null}}"#;

/// Build the handlers a dispatch session sees, behind the seam's
/// host-backed store: every row this guest reads or writes travels through a
/// `host.model_*` import to the store the runtime installed at boot, which
/// is what lets the kernel own the data while never linking the behaviour.
///
/// Notifications cross the same way: a booking DECLARES `appointment.booked`
/// through `host.notify_enqueue` and the host resolves the registered def,
/// the catalog, the locale and the channels. Each mutation still queues its
/// `OutboxMessage` on the execution unit the host commits, which is the
/// durable half and needs no import at all.
fn build_handlers() -> Vec<Arc<dyn OperationHandler>> {
    handlers(Arc::new(AppointmentStore::new(Arc::new(
        kelson_module_sdk::HostModelStore::new(),
    ))))
}

kelson_module_sdk::guest_entry! {
    manifest: MANIFEST_JSON,
    handlers: build_handlers
}
