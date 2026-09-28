//! Keel reference application (S6 public ecosystem): a small clinic-appointments
//! domain that exercises the platform end-to-end, the way a real module would —
//! module manifest -> platform graph -> policy-gated execution -> delta + outbox
//! -> search -> MCP tool manifest. Everything runs on the in-memory reference
//! implementations; production swaps in postgres/pgvector adapters behind the
//! same ports.

use std::sync::{Arc, Mutex};

use keel_command::CommandBus;
use keel_context::{ExecutionContext, PrincipalContext, ScopeSnapshot, TenantContext};
use keel_foundation::{
    AppError, AuthorizationRevision, ExecutionProfileKind, FeatureRevision, ModuleRevision,
    OperationId, PrincipalId, TenantId,
};
use keel_execution::{
    BoxFuture, ExecutionPlan, ExecutionUnit, IdempotencyKey, InMemoryBackend, InMemoryIdempotency,
    MutationRecord, OperationHandler, OutboxMessage,
};
use keel_lifecycle::{ApplicationHost, BootError, Phase, Provider, ServiceRegistry};
use keel_policy::{AccessSnapshot, PolicyAuthz, PolicyEngine};
use keel_registry::{ModuleManifest, PlatformGraph};
use keel_search::{InMemorySearchEngine, Index, SearchDocument, SearchEngine, SearchQuery};
use serde_json::json;
use time::OffsetDateTime;

/// The appointments module manifest — the exact shape `keel new` + `keel make:module`
/// emit for real modules, validated by the same graph compiler (G05/G06).
pub const MANIFEST: &str = include_str!("../module.toml");

pub fn op_book() -> OperationId {
    OperationId::from_static("appointments.book")
}

/// List operation id (GET /api/v1/appointments/{id}).
pub fn op_list() -> OperationId {
    OperationId::from_static("appointments.list")
}

/// Compile the platform graph from the embedded manifest.
pub fn compile_reference_graph() -> Result<PlatformGraph, String> {
    let manifest = ModuleManifest::from_toml(MANIFEST).map_err(|v| v.message)?;
    PlatformGraph::compile(vec![manifest]).map_err(|violations| {
        violations
            .iter()
            .map(|v| format!("{}: {}", v.code, v.message))
            .collect::<Vec<_>>()
            .join("; ")
    })
}

/// Process-local "appointments table" (stand-in for the data layer).
#[derive(Default)]
pub struct AppointmentStore(Mutex<Vec<serde_json::Value>>);

impl AppointmentStore {
    pub fn len(&self) -> usize {
        self.0.lock().expect("store poisoned").len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

struct BookAppointment {
    plan: ExecutionPlan,
    store: Arc<AppointmentStore>,
    search: Arc<InMemorySearchEngine>,
}

impl BookAppointment {
    fn new(store: Arc<AppointmentStore>, search: Arc<InMemorySearchEngine>) -> Self {
        Self {
            plan: ExecutionPlan::new(op_book(), ExecutionProfileKind::Command)
                .with_capability("appointments.book"),
            store,
            search,
        }
    }
}

impl OperationHandler for BookAppointment {
    fn plan(&self) -> &ExecutionPlan {
        &self.plan
    }

    fn handle(
        &self,
        ctx: Arc<ExecutionContext>,
        input: serde_json::Value,
        unit: &mut dyn ExecutionUnit,
    ) -> BoxFuture<'static, Result<serde_json::Value, AppError>> {
        let id = uuid::Uuid::now_v7();
        let appointment = json!({
            "id": id.to_string(),
            "patient": input.get("patient").cloned().unwrap_or(json!("")),
            "slot": input.get("slot").cloned().unwrap_or(json!("")),
            "status": "booked",
        });
        self.store
            .0
            .lock()
            .expect("store poisoned")
            .push(appointment.clone());
        if let Some(tenant) = ctx.tenant.as_ref() {
            // Index inside the same unit of work; production modules persist the
            // document through the data layer's transactional outbox.
            let _ = self.search.index(
                &Index("appointments".into()),
                SearchDocument {
                    id: id.to_string(),
                    tenant: tenant.id,
                    text: format!(
                        "appointment {} {}",
                        input.get("patient").and_then(|v| v.as_str()).unwrap_or(""),
                        input.get("slot").and_then(|v| v.as_str()).unwrap_or("")
                    ),
                    fields: [("status".to_string(), "booked".to_string())]
                        .into_iter()
                        .collect(),
                    vector: Vec::new(),
                },
            );
        }
        unit.record_mutation(MutationRecord {
            resource_type: "appointment".into(),
            resource_id: Some(id),
            resource_version: 1,
            before: None,
            after: appointment.clone(),
            stream: None,
        });
        unit.queue_outbox(OutboxMessage {
            event: "appointment.booked".into(),
            topic: "appointments".into(),
            payload: json!({ "id": id.to_string() }),
        });
        Box::pin(async move { Ok(json!({ "ok": true, "id": id.to_string() })) })
    }
}

struct ListAppointments {
    plan: ExecutionPlan,
    store: Arc<AppointmentStore>,
}

impl ListAppointments {
    fn new(store: Arc<AppointmentStore>) -> Self {
        Self {
            plan: ExecutionPlan::new(op_list(), ExecutionProfileKind::Read)
                .with_capability("appointments.list"),
            store,
        }
    }
}

impl OperationHandler for ListAppointments {
    fn plan(&self) -> &ExecutionPlan {
        &self.plan
    }

    fn handle(
        &self,
        _ctx: Arc<ExecutionContext>,
        _input: serde_json::Value,
        _unit: &mut dyn ExecutionUnit,
    ) -> BoxFuture<'static, Result<serde_json::Value, AppError>> {
        let appointments = self.store.0.lock().expect("store poisoned").clone();
        Box::pin(async move { Ok(json!({ "appointments": appointments })) })
    }
}

struct KernelProvider {
    principal: PrincipalId,
    tenant: TenantId,
}

impl Provider for KernelProvider {
    fn name(&self) -> &str {
        "kernel"
    }
    fn phase(&self) -> Phase {
        Phase::Kernel
    }
    fn init(&self, registry: &mut ServiceRegistry) -> Result<(), BootError> {
        let policy = Arc::new(PolicyEngine::new());
        policy.install_snapshot(AccessSnapshot {
            principal_id: self.principal,
            tenant_id: self.tenant,
            membership_active: true,
            capabilities: [
                "appointments.book".to_string(),
                "appointments.list".to_string(),
            ]
            .into_iter()
            .collect(),
            scope_paths: ["/".to_string()].into_iter().collect(),
            authorization_revision: policy.revision(),
            feature_revision: FeatureRevision(1),
            module_revision: ModuleRevision(1),
            computed_at: OffsetDateTime::now_utc(),
        });
        let engine = Arc::new(keel_execution::Engine::new(
            Arc::new(PolicyAuthz {
                engine: policy.clone(),
            }),
            Arc::new(InMemoryBackend::new()),
            Arc::new(InMemoryIdempotency::new()),
        ));
        registry.insert("execution.engine", engine)
    }
}

struct ModuleProvider;

impl Provider for ModuleProvider {
    fn name(&self) -> &str {
        "module.appointments"
    }
    fn phase(&self) -> Phase {
        Phase::Modules
    }
    fn init(&self, registry: &mut ServiceRegistry) -> Result<(), BootError> {
        let engine = registry.get::<keel_execution::Engine>()?;
        let store = Arc::new(AppointmentStore::default());
        let search = Arc::new(InMemorySearchEngine::new());
        let mut bus = CommandBus::new(engine);
        for handler in [
            Arc::new(BookAppointment::new(store.clone(), search.clone()))
                as Arc<dyn OperationHandler>,
            Arc::new(ListAppointments::new(store.clone())) as Arc<dyn OperationHandler>,
        ] {
            bus.register(handler)
                .map_err(|e| BootError::ProviderFailed {
                    provider: "module.appointments".into(),
                    source: e.to_string(),
                })?;
        }
        registry.insert("appointments.store", store)?;
        registry.insert("search.engine", search)?;
        registry.insert("command.bus", Arc::new(bus))
    }
}

/// One full pass over the reference application. Every field becomes an
/// assertion in `tests/reference.rs` — together they read as documentation.
pub struct Report {
    pub graph_operations: usize,
    pub graph_routes: usize,
    pub graph_events: usize,
    pub booked_id: String,
    pub delta_stones: usize,
    pub outbox_queued: usize,
    pub listed: usize,
    pub replayed: bool,
    pub denied_code: String,
    pub search_hits: usize,
    pub mcp_tools: usize,
}

/// Sentinel (S5): the reference application boots the telemetry runtime
/// through the provider lifecycle, exactly like production hosts do
/// (STAGE-SENTINEL-SPEC §18). Storage paths live in a per-run temp dir so
/// tests never touch the workspace `storage/`.
fn telemetry_provider() -> keel_observability::ObservabilityProvider {
    let base = std::env::temp_dir().join(format!(
        "keel-example-reference-telemetry-{}",
        uuid::Uuid::now_v7()
    ));
    let mut tcfg = keel_observability::TelemetryConfig::default();
    tcfg.wal.dir = base.join("wal");
    tcfg.paths.incident_dir = base.join("control");
    tcfg.paths.cost_file = base.join("cost.json");
    tcfg.paths.health_dir = base.join("exporters");
    tcfg.exporters.insert(
        "file".into(),
        keel_observability::ExporterConfig {
            kind: "file".into(),
            target: Some(base.join("events.jsonl").to_string_lossy().into_owned()),
            format: None,
            critical: true,
            partition_by_tenant: false,
            retry: None,
            breaker: None,
        },
    );
    keel_observability::ObservabilityProvider::new(
        tcfg,
        keel_observability::ServiceIdentity::new("keel-example-reference", "test"),
    )
}

/// Boot the reference application and run the golden scenario.
pub async fn run() -> Result<Report, Box<dyn std::error::Error>> {
    let graph = compile_reference_graph()?;

    let principal = PrincipalId::new();
    let tenant = TenantId::new();
    let mut host = ApplicationHost::new();
    host.add_provider(Box::new(telemetry_provider()));
    host.add_provider(Box::new(KernelProvider { principal, tenant }));
    host.add_provider(Box::new(ModuleProvider));
    host.boot().await?;

    let bus = host.registry().get::<CommandBus>()?;
    let search = host.registry().get::<InMemorySearchEngine>()?;
    let ctx = ExecutionContext::builder(ExecutionProfileKind::Command, OffsetDateTime::now_utc())
        .principal(PrincipalContext {
            id: principal,
            display: Some("reference".into()),
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
        .build()?;

    // 1. Booking writes exactly one delta stone and queues one outbox event.
    let booked = bus
        .dispatch(
            ctx.clone(),
            &op_book(),
            json!({ "patient": "A. Patient", "slot": "2026-09-01T10:00Z" }),
            Some(IdempotencyKey("booking-1".into())),
        )
        .await?;
    let booked_id = booked
        .result
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    // 2. The read path sees the booking.
    let listed = bus
        .dispatch(ctx.clone(), &op_list(), json!({}), None)
        .await?;
    let listed_count = listed
        .result
        .get("appointments")
        .and_then(|v| v.as_array())
        .map(|a| a.len())
        .unwrap_or(0);

    // 3. Replay under the same key returns the stored result, nothing new.
    let replay = bus
        .dispatch(
            ctx.clone(),
            &op_book(),
            json!({ "patient": "A. Patient", "slot": "2026-09-01T10:00Z" }),
            Some(IdempotencyKey("booking-1".into())),
        )
        .await?;

    // 4. A stranger without an access snapshot is denied; nothing is written.
    let stranger =
        ExecutionContext::builder(ExecutionProfileKind::Command, OffsetDateTime::now_utc())
            .principal(PrincipalContext {
                id: PrincipalId::new(),
                display: None,
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
            .build()?;
    let denied = bus
        .dispatch(
            stranger,
            &op_book(),
            json!({}),
            Some(IdempotencyKey("booking-2".into())),
        )
        .await
        .unwrap_err();

    // 5. Search finds the booking, scoped to the tenant.
    let hits = search.search(
        &Index("appointments".into()),
        &SearchQuery {
            tenant,
            text: "patient".into(),
            limit: 10,
            ..Default::default()
        },
    )?;

    // 6. The ecosystem surface is generated from the same graph.
    let tools = keel_ai_mcp::tools_from_graph(&graph);

    Ok(Report {
        graph_operations: graph.operations.len(),
        graph_routes: graph.modules.iter().map(|m| m.routes.len()).sum(),
        graph_events: graph.events.len(),
        booked_id,
        delta_stones: booked.receipt.deltas_written,
        outbox_queued: booked.receipt.outbox_queued,
        listed: listed_count,
        replayed: replay.receipt.replayed,
        denied_code: denied.code,
        search_hits: hits.total,
        mcp_tools: tools.len(),
    })
}
