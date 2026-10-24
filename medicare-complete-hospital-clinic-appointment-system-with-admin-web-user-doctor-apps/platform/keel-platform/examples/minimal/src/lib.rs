//! Kelson golden path (11-SPEC §4): host boot -> kernel wiring -> module handler ->
//! engine execution -> delta stone, proven through the REAL pipeline
//! (PolicyAuthz + CommandBus + InMemoryBackend + idempotency).

use std::sync::Arc;

use kelson_command::CommandBus;
use kelson_context::{ExecutionContext, PrincipalContext, ScopeSnapshot, TenantContext};
use kelson_execution::{
    BoxFuture, ExecutionPlan, ExecutionUnit, IdempotencyKey, InMemoryBackend, InMemoryIdempotency,
    MutationRecord, OperationHandler,
};
use kelson_foundation::{
    AppError, AuthorizationRevision, ExecutionProfileKind, FeatureRevision, ModuleRevision,
    OperationId, PrincipalId, TenantId,
};
use kelson_lifecycle::{BootError, Phase, Provider, ServiceRegistry};
use kelson_policy::{AccessSnapshot, PolicyAuthz, PolicyEngine};
use serde_json::json;
use time::OffsetDateTime;

const CAPABILITY: &str = "demo.create_note";

pub struct Report {
    pub stones_written: usize,
    pub replayed: bool,
    pub denied_code: String,
}

struct CreateNote {
    plan: ExecutionPlan,
}

impl CreateNote {
    fn new() -> Self {
        Self {
            plan: ExecutionPlan::new(
                OperationId::from_static("demo.create_note"),
                ExecutionProfileKind::Command,
            )
            .with_capability(CAPABILITY),
        }
    }
}

impl OperationHandler for CreateNote {
    fn plan(&self) -> &ExecutionPlan {
        &self.plan
    }
    fn handle(
        &self,
        _ctx: Arc<ExecutionContext>,
        input: serde_json::Value,
        unit: &mut dyn ExecutionUnit,
    ) -> BoxFuture<'static, Result<serde_json::Value, AppError>> {
        unit.record_mutation(MutationRecord {
            resource_type: "note".into(),
            resource_id: Some(uuid::Uuid::now_v7()),
            resource_version: 1,
            before: None,
            after: input,
            stream: None,
        });
        Box::pin(async move { Ok(json!({ "ok": true })) })
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
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        let policy = Arc::new(PolicyEngine::new());
        // S-001c: this scaffold grants its operator edge through the same
        // MembershipStore port a login uses, then reads the membership bit
        // back from it - the snapshot is never fabricated.
        let memberships = kelson_identity::InMemoryMembershipStore::new();
        kelson_identity::store::MembershipStore::grant(
            &memberships,
            kelson_identity::Membership {
                id: kelson_foundation::MembershipId::new(),
                principal_id: self.principal,
                tenant_id: self.tenant,
                status: kelson_identity::MembershipStatus::Active,
                joined_at: OffsetDateTime::now_utc(),
            },
        )
        .expect("the scaffold edge grants");
        let membership_active = kelson_identity::store::MembershipStore::active_for(
            &memberships,
            self.principal,
            self.tenant,
        )
        .expect("the store answers")
        .is_some();
        policy.install_snapshot(AccessSnapshot {
            principal_id: self.principal,
            tenant_id: self.tenant,
            membership_active,
            capabilities: [CAPABILITY.to_string()].into_iter().collect(),
            scope_paths: ["LE1".to_string()].into_iter().collect(),
            authorization_revision: policy.revision(),
            feature_revision: FeatureRevision(1),
            module_revision: ModuleRevision(1),
            computed_at: OffsetDateTime::now_utc(),
        });
        let engine = Arc::new(kelson_execution::Engine::new(
            Arc::new(PolicyAuthz {
                engine: policy.clone(),
            }),
            Arc::new(InMemoryBackend::new()),
            Arc::new(InMemoryIdempotency::new()),
        ));
        r.insert("execution.engine", engine)
    }
}

struct ModuleProvider;

impl Provider for ModuleProvider {
    fn name(&self) -> &str {
        "module.demo"
    }
    fn phase(&self) -> Phase {
        Phase::Modules
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        let engine = r.get::<kelson_execution::Engine>()?;
        let mut bus = CommandBus::new(engine);
        bus.register(Arc::new(CreateNote::new()))
            .map_err(|e| BootError::ProviderFailed {
                provider: "module.demo".into(),
                source: e.to_string(),
            })?;
        r.insert("command.bus", Arc::new(bus))
    }
}

/// Sentinel (S5): the golden path boots the telemetry runtime through the
/// provider lifecycle, exactly like production hosts do (STAGE-SENTINEL-SPEC
/// §18). Storage paths live in a per-run temp dir so tests never touch the
/// workspace `storage/`.
fn telemetry_provider() -> kelson_observability::ObservabilityProvider {
    let base = std::env::temp_dir().join(format!(
        "kelson-example-minimal-telemetry-{}",
        uuid::Uuid::now_v7()
    ));
    let mut tcfg = kelson_observability::TelemetryConfig::default();
    tcfg.wal.dir = base.join("wal");
    tcfg.paths.incident_dir = base.join("control");
    tcfg.paths.cost_file = base.join("cost.json");
    tcfg.paths.health_dir = base.join("exporters");
    tcfg.exporters.insert(
        "file".into(),
        kelson_observability::ExporterConfig {
            kind: "file".into(),
            target: Some(base.join("events.jsonl").to_string_lossy().into_owned()),
            format: None,
            critical: true,
            partition_by_tenant: false,
            retry: None,
            breaker: None,
        },
    );
    kelson_observability::ObservabilityProvider::new(
        tcfg,
        kelson_observability::ServiceIdentity::new("kelson-example-minimal", "test"),
    )
}

/// The full golden path. Every assertion here is a platform invariant.
pub async fn run() -> Result<Report, Box<dyn std::error::Error>> {
    let principal = PrincipalId::new();
    let tenant = TenantId::new();
    let mut host = kelson_lifecycle::ApplicationHost::new();
    host.add_provider(Box::new(telemetry_provider()));
    host.add_provider(Box::new(KernelProvider { principal, tenant }));
    host.add_provider(Box::new(ModuleProvider));
    host.boot().await?;

    let bus = host.registry().get::<CommandBus>()?;
    let ctx = ExecutionContext::builder(ExecutionProfileKind::Command, OffsetDateTime::now_utc())
        .principal(PrincipalContext {
            id: principal,
            display: Some("golden".into()),
        })
        .tenant(TenantContext {
            id: tenant,
            slug: "acme".into(),
            plan: "standard".into(),
        })
        .scope(ScopeSnapshot {
            nodes: vec![],
            revision: AuthorizationRevision(1),
            computed_at: OffsetDateTime::now_utc(),
        })
        .build()?;
    let op = OperationId::from_static("demo.create_note");

    // 1. first execution writes exactly one delta stone
    let first = bus
        .dispatch(
            ctx.clone(),
            &op,
            json!({ "body": "hello kelson" }),
            Some(IdempotencyKey("g1".into())),
        )
        .await?;
    assert_eq!(
        first.receipt.deltas_written, 1,
        "command must write exactly one delta stone"
    );

    // 2. replay under the same key returns the stored result, no new stone
    let replay = bus
        .dispatch(
            ctx.clone(),
            &op,
            json!({ "body": "hello kelson" }),
            Some(IdempotencyKey("g1".into())),
        )
        .await?;
    assert!(
        replay.receipt.replayed,
        "same key + same payload must replay"
    );

    // 3. unknown principal has no snapshot -> AUTH_DENIED, nothing written
    let stranger_ctx =
        ExecutionContext::builder(ExecutionProfileKind::Command, OffsetDateTime::now_utc())
            .principal(PrincipalContext {
                id: PrincipalId::new(),
                display: None,
            })
            .tenant(TenantContext {
                id: tenant,
                slug: "acme".into(),
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
            stranger_ctx,
            &op,
            json!({}),
            Some(IdempotencyKey("g2".into())),
        )
        .await
        .unwrap_err();
    assert_eq!(denied.code, kelson_foundation::codes::DENIED);

    Ok(Report {
        stones_written: first.receipt.deltas_written,
        replayed: replay.receipt.replayed,
        denied_code: denied.code,
    })
}
