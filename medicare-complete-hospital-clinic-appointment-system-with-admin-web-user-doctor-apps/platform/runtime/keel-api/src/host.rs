//! Boot topology for keel-api (11-SPEC §10/G03/G04).
//! Providers boot in phase order: Foundation -> Kernel -> Platform -> Modules.
//! DB probing happens BEFORE host boot (init must stay I/O-free); the probe
//! result selects the postgres or in-memory backend (degraded boot, D-52).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use keel_command::CommandBus;
use keel_config::FrozenConfig;
use keel_context::{ExecutionContext, PrincipalContext, ScopeSnapshot, TenantContext};
use keel_foundation::{
    AppError, AuthorizationRevision, ExecutionProfileKind, OperationId, PrincipalId, TenantId,
};
use keel_execution::{
    AuthorizationPort, BoxFuture, ExecutionBackend, ExecutionPlan, ExecutionUnit, IdempotencyPort,
    InMemoryBackend, InMemoryIdempotency, OperationHandler,
};
use keel_lifecycle::{BootError, Phase, Provider, ServiceRegistry};
use keel_policy::{AccessSnapshot, PolicyAuthz, PolicyEngine};
use keel_postgres::{PgIdempotency, PgPool, PostgresBackend};
use serde_json::Value;
use time::OffsetDateTime;

/// Dev-only fixed identities for the demo.echo golden path (never used in prod).
pub const DEV_TENANT_UUID: &str = "00000000-0000-0000-0000-0000000000d1";
pub const DEV_PRINCIPAL_UUID: &str = "00000000-0000-0000-0000-0000000000a1";
/// Synthetic internal actor for boot-time flag evaluation (the host itself).
pub const BOOT_ACTOR_UUID: &str = "00000000-0000-0000-0000-0000000000fe";
/// Wave 1: synthetic principal for anonymous reach on public routes whose
/// handler plan does not require authorization (never used outside that
/// narrow path).
pub const ANONYMOUS_PRINCIPAL_UUID: &str = "00000000-0000-0000-0000-0000000000a0";

/// The anonymous principal identity (public-route dispatch only).
pub fn anonymous_principal() -> PrincipalId {
    PrincipalId::from_uuid(uuid::Uuid::parse_str(ANONYMOUS_PRINCIPAL_UUID).expect("static uuid"))
}

/// Liveness/readiness carrier shared between kernel providers and routes.
pub struct SystemHealth {
    pub degraded: AtomicBool,
    pub reason: Mutex<String>,
    pub services: Mutex<Vec<String>>,
}

impl SystemHealth {
    pub fn new(degraded: bool, reason: impl Into<String>) -> Arc<Self> {
        Arc::new(Self {
            degraded: AtomicBool::new(degraded),
            reason: Mutex::new(reason.into()),
            services: Mutex::new(Vec::new()),
        })
    }
    pub fn is_degraded(&self) -> bool {
        self.degraded.load(Ordering::SeqCst)
    }
}

pub struct ConfigProvider {
    pub config: FrozenConfig,
    pub health: Arc<SystemHealth>,
}

impl Provider for ConfigProvider {
    fn name(&self) -> &str {
        "config"
    }
    fn phase(&self) -> Phase {
        Phase::Foundation
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        r.insert("platform.config", self.config.shared())?;
        r.insert("system.health", self.health.clone())
    }
}

/// Wave 2 error rendering: publishes the shared error-render registry so
/// modules register per-code status/view/reportable overrides and custom
/// renderers from their own `Provider::init`. Foundation phase, so every
/// module provider sees it; the dispatch edge reads the same `Arc`.
pub struct ErrorRenderProvider {
    pub registry: Arc<keel_http::ErrorRenderRegistry>,
}

impl Provider for ErrorRenderProvider {
    fn name(&self) -> &str {
        "error-renders"
    }
    fn phase(&self) -> Phase {
        Phase::Foundation
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        r.insert("platform.error_renders", self.registry.clone())
    }
}

/// Wave 2 terminating hooks: publishes the post-response callback registry
/// so modules register observers (metrics, audit trails) from their own
/// `Provider::init`. Foundation phase, so every module provider sees it; the
/// edge layer runs the very same `Arc` once each response is final.
pub struct TerminatingProvider {
    pub hooks: Arc<crate::edge::TerminatingHooks>,
}

impl Provider for TerminatingProvider {
    fn name(&self) -> &str {
        "terminating-hooks"
    }
    fn phase(&self) -> Phase {
        Phase::Foundation
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        r.insert("platform.terminating_hooks", self.hooks.clone())
    }
}

/// Kernel phase: policy engine, execution backend, idempotency port, engine.
pub struct KernelProvider {
    pub pool: Option<PgPool>,
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
        let authz: Arc<dyn AuthorizationPort> = Arc::new(PolicyAuthz {
            engine: policy.clone(),
        });
        let (backend, idem): (Arc<dyn ExecutionBackend>, Arc<dyn IdempotencyPort>) =
            match &self.pool {
                Some(pool) => (
                    Arc::new(PostgresBackend::new(pool.clone())),
                    Arc::new(PgIdempotency::new(pool.clone())),
                ),
                None => (
                    Arc::new(InMemoryBackend::new()),
                    Arc::new(InMemoryIdempotency::new()),
                ),
            };
        let engine = Arc::new(keel_execution::Engine::new(authz, backend, idem));
        r.insert("policy.engine", policy)?;
        r.insert("execution.engine", engine)
    }
}
/// Platform phase: framework services (cache, notify hub). CommandBus is built in
/// the Modules phase because handlers are module-owned.
pub struct PlatformProvider {
    /// Optional Redis cache (S10 B-12). When absent, the cache port stays on
    /// the in-memory adapter (single node; degraded boot, D-52).
    pub redis: Option<Arc<keel_cache_redis::RedisCache>>,
    /// Wave 6 P3: the boot-loaded message catalog — the notify hub
    /// resolves notification strings through the same catalog as views
    /// and validation.
    pub messages: Arc<keel_foundation::MessageCatalog>,
    /// Wave 6 P5: the boot's database-channel store — the hub writes rows
    /// through it (the very instance the HTTP feed reads).
    pub notifications: Arc<dyn keel_notifications::NotificationStore>,
    /// Wave 6 P5: views engine for notification HTML mail bodies
    /// (`views/notifications/...`); `None` keeps mail text-only.
    pub views: Option<Arc<keel_views::ViewsEngine>>,
}

/// Wave 6 P5: the Wave 2 views engine behind the notification HTML port —
/// the service layer cannot depend on the web layer, so the runtime
/// adapts and maps render failures onto the error channel.
pub(crate) struct ViewsNotificationRenderer(pub(crate) Arc<keel_views::ViewsEngine>);

impl keel_notifications::NotificationViewRenderer for ViewsNotificationRenderer {
    fn render(&self, template: &str, context: Value) -> Result<String, AppError> {
        self.0
            .render(template, context)
            .map_err(|e| AppError::internal(format!("notification view render failed: {e}")))
    }
}

impl Provider for PlatformProvider {
    fn name(&self) -> &str {
        "platform-services"
    }
    fn phase(&self) -> Phase {
        Phase::Platform
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        let memory = keel_cache_memory::MemoryCache::shared(4096);
        r.insert("cache.memory", memory.clone())?;
        let port: Arc<dyn keel_caching::CachePort> = match &self.redis {
            Some(redis) => {
                r.insert("cache.redis", redis.clone())?;
                redis.clone()
            }
            None => memory,
        };
        r.insert(
            "cache.service",
            Arc::new(keel_caching::CacheService::new(port.clone())),
        )?;
        r.insert("cache.port", Arc::new(CachePortRef(port)))?;
        let mut hub = keel_notifications::NotifyHub::with_catalog(self.messages.clone())
            .with_store(self.notifications.clone());
        if let Some(views) = &self.views {
            hub = hub.with_view_renderer(Arc::new(ViewsNotificationRenderer(views.clone())));
        }
        r.insert("notify.hub", Arc::new(hub))
    }
}

/// Demo module (Modules phase): builds the command bus on the kernel engine and
/// registers the dev golden-path handler. In dev/test it also installs a dev
/// access snapshot so demo.echo is authorized without full identity wiring.
pub struct DemoModuleProvider {
    pub dev_snapshot: bool,
}

impl Provider for DemoModuleProvider {
    fn name(&self) -> &str {
        "module.demo"
    }
    fn phase(&self) -> Phase {
        Phase::Modules
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        let engine = r.get::<keel_execution::Engine>()?;
        let mut bus = CommandBus::new(engine);
        bus.register(Arc::new(EchoHandler::new()))
            .map_err(|e| BootError::ProviderFailed {
                provider: "module.demo".into(),
                source: e.to_string(),
            })?;
        // S7: the bus is assembled here (Modules phase) from every handler
        // bundle registered by module providers (appointments ships first).
        if let Ok(bundle) = r.get::<Vec<Arc<dyn OperationHandler>>>() {
            for handler in bundle.iter() {
                bus.register(handler.clone())
                    .map_err(|e| BootError::ProviderFailed {
                        provider: "module.demo".into(),
                        source: e.to_string(),
                    })?;
            }
        }
        r.insert("command.bus", Arc::new(bus))?;
        if self.dev_snapshot {
            let policy = r.get::<PolicyEngine>()?;
            let principal =
                PrincipalId::from_uuid(uuid::Uuid::parse_str(DEV_PRINCIPAL_UUID).unwrap());
            let tenant = TenantId::from_uuid(uuid::Uuid::parse_str(DEV_TENANT_UUID).unwrap());
            policy.install_snapshot(AccessSnapshot {
                principal_id: principal,
                tenant_id: tenant,
                membership_active: true,
                capabilities: ["demo.echo".to_string()].into_iter().collect(),
                scope_paths: ["LE1".to_string()].into_iter().collect(),
                authorization_revision: policy.revision(),
                feature_revision: keel_foundation::FeatureRevision(1),
                module_revision: keel_foundation::ModuleRevision(1),
                computed_at: OffsetDateTime::now_utc(),
            });
        }
        Ok(())
    }
}

/// Golden-path read operation: `demo.echo` (Read profile, tenant plane).
pub struct EchoHandler {
    plan: ExecutionPlan,
}

impl EchoHandler {
    pub fn new() -> Self {
        Self {
            plan: ExecutionPlan::new(
                OperationId::from_static("demo.echo"),
                ExecutionProfileKind::Read,
            ),
        }
    }
}

impl Default for EchoHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl OperationHandler for EchoHandler {
    fn plan(&self) -> &ExecutionPlan {
        &self.plan
    }
    fn handle(
        &self,
        _ctx: Arc<ExecutionContext>,
        input: Value,
        _unit: &mut dyn ExecutionUnit,
    ) -> BoxFuture<'static, Result<Value, AppError>> {
        Box::pin(async move { Ok(serde_json::json!({ "echo": input })) })
    }
}

/// Platform phase: feature-flag service (L33, item 32). Registry and store
/// are chosen BEFORE boot (main.rs) so provider init stays I/O-free.
pub struct FeatureFlagsProvider {
    pub registry: Arc<keel_feature_flags::FlagRegistry>,
    pub store: Arc<dyn keel_feature_flags::FlagStore>,
}

impl Provider for FeatureFlagsProvider {
    fn name(&self) -> &str {
        "platform.flags"
    }
    fn phase(&self) -> Phase {
        Phase::Platform
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        let service = Arc::new(keel_feature_flags::FlagService::new(
            self.registry.clone(),
            self.store.clone(),
        ));
        r.insert("flags.service", service)
    }
}

/// Platform phase: opt-in server sessions (L26/D-66). Registered under the
/// REQUEST lifetime: session backing serves per-request scopes, never pools.
pub struct SessionsProvider {
    pub secret: Vec<u8>,
    /// Backing store: in-memory (degraded boot) or PgSessionStore (durable,
    /// survives restart + multi-instance).
    pub store: Arc<dyn keel_session::SessionStore>,
}

impl Provider for SessionsProvider {
    fn name(&self) -> &str {
        "platform.sessions"
    }
    fn phase(&self) -> Phase {
        Phase::Platform
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        let manager = Arc::new(keel_session::SessionManager::new(
            self.store.clone(),
            keel_session::SessionPolicy::default(),
            self.secret.clone(),
        ));
        r.insert_with_lifetime(
            "sessions.manager",
            keel_lifecycle::ServiceLifetime::Request,
            manager,
        )
    }
}

/// Platform phase: opt-in realtime hub (L42/L43). Endpoints are merged into
/// the edge router in main.rs only when the `app.realtime` flag is on.
pub struct RealtimeProvider;

impl Provider for RealtimeProvider {
    fn name(&self) -> &str {
        "platform.realtime"
    }
    fn phase(&self) -> Phase {
        Phase::Platform
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        r.insert("realtime.hub", Arc::new(keel_realtime::BroadcastHub::new()))
    }
}

/// Build the dev golden-path execution context (demo route only).
pub fn dev_context() -> Result<Arc<ExecutionContext>, AppError> {
    ExecutionContext::builder(ExecutionProfileKind::Read, OffsetDateTime::now_utc())
        .principal(PrincipalContext {
            id: PrincipalId::from_uuid(uuid::Uuid::parse_str(DEV_PRINCIPAL_UUID).unwrap()),
            display: Some("dev-principal".into()),
        })
        .tenant(TenantContext {
            id: TenantId::from_uuid(uuid::Uuid::parse_str(DEV_TENANT_UUID).unwrap()),
            slug: "dev".into(),
            plan: "standard".into(),
        })
        .scope(ScopeSnapshot {
            nodes: vec![],
            revision: AuthorizationRevision(1),
            computed_at: OffsetDateTime::now_utc(),
        })
        .build()
}

// --------------------------------------------------------------- S7 wiring

/// Typed-registry carriers for trait-object edge services. The lifecycle
/// registry is keyed by TypeId (one slot per concrete type), so port
/// implementations ride inside these newtype refs.
#[derive(Clone)]
pub struct PrincipalStoreRef(pub Arc<dyn keel_identity::PrincipalStore>);

#[derive(Clone)]
pub struct CredentialStoreRef(pub Arc<dyn keel_identity::CredentialStore>);

#[derive(Clone)]
pub struct StorageRef(pub Arc<dyn keel_storage::StoragePort>);

/// Wave 6 P2: the named-disk registry carrier — one registry per boot,
/// built from `[storage]` by `crate::disk_registry`.
#[derive(Clone)]
pub struct DiskRegistryRef(pub Arc<keel_storage::DiskRegistry>);

/// Carrier for the active cache port (memory or redis; S10 B-12).
#[derive(Clone)]
pub struct CachePortRef(pub Arc<dyn keel_caching::CachePort>);

/// Platform phase: identity + token services (S7 C2/B-02). Postgres-backed
/// when the pool is up; in-memory on degraded boot (D-52).
pub struct IdentityProvider {
    pub pool: Option<PgPool>,
}

impl Provider for IdentityProvider {
    fn name(&self) -> &str {
        "platform.identity"
    }
    fn phase(&self) -> Phase {
        Phase::Platform
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        let (principals, credentials, token_store): (
            Arc<dyn keel_identity::PrincipalStore>,
            Arc<dyn keel_identity::CredentialStore>,
            Arc<dyn keel_tokens::TokenStore>,
        ) = match &self.pool {
            Some(pool) => (
                Arc::new(keel_postgres::PgPrincipalStore::new(pool.clone())),
                Arc::new(keel_postgres::PgCredentialStore::new(pool.clone())),
                Arc::new(keel_postgres::PgTokenStore::new(pool.clone())),
            ),
            None => (
                Arc::new(keel_identity::InMemoryPrincipalStore::new()),
                Arc::new(keel_identity::InMemoryCredentialStore::new()),
                Arc::new(keel_tokens::InMemoryTokenStore::new()),
            ),
        };
        r.insert(
            "identity.principals",
            Arc::new(PrincipalStoreRef(principals)),
        )?;
        r.insert(
            "identity.credentials",
            Arc::new(CredentialStoreRef(credentials)),
        )?;
        r.insert(
            "security.tokens",
            Arc::new(keel_tokens::TokenService::new(token_store)),
        )
    }
}

/// Platform phase: object storage for the upload edge (S7 C5). Wave 6 P2:
/// the disk registry is built from `[storage]` — declared disks resolve to
/// their adapters, or one synthetic `local` disk under
/// <root>/storage/local when nothing is declared. The default disk's port
/// also rides as `storage.port`; the full registry as `storage.registry`.
/// An invalid section (missing root/bucket, unknown default) refuses boot.
pub struct StorageProvider {
    pub root: std::path::PathBuf,
    pub disks: keel_config::StorageConfig,
}

impl Provider for StorageProvider {
    fn name(&self) -> &str {
        "platform.storage"
    }
    fn phase(&self) -> Phase {
        Phase::Platform
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        let registry =
            crate::disk_registry(&self.disks, &self.root.join("storage").join("local")).map_err(
                |e| BootError::ProviderFailed {
                    provider: "platform.storage".into(),
                    source: e.public_message.clone(),
                },
            )?;
        let port = registry
            .get(registry.default_name())
            .map_err(|e| BootError::ProviderFailed {
                provider: "platform.storage".into(),
                source: e.public_message.clone(),
            })?
            .port
            .clone();
        r.insert("storage.port", Arc::new(StorageRef(port)))?;
        r.insert(
            "storage.registry",
            Arc::new(DiskRegistryRef(Arc::new(registry))),
        )
    }
}

/// Platform phase: semantic model store port (S8 C3/B-04). Domain modules
/// (appointments first) consume the [`keel_models::ModelStoreRef`] carrier;
/// Pg-backed when the pool is up, in-memory on degraded boot (D-52).
pub struct PersistenceProvider {
    pub pool: Option<PgPool>,
}

impl Provider for PersistenceProvider {
    fn name(&self) -> &str {
        "platform.persistence"
    }
    fn phase(&self) -> Phase {
        Phase::Platform
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        let store: Arc<dyn keel_models::ModelStore> = match &self.pool {
            Some(pool) => Arc::new(keel_postgres::PgModelStore::new(pool.clone())),
            None => Arc::new(keel_models::InMemoryModelStore::new()),
        };
        r.insert("model.store", Arc::new(keel_models::ModelStoreRef(store)))?;
        // Wave 1 model binding: the well-known route.bindings registry.
        // Modules register one resolver per bound model in Provider::init;
        // the GRAPH-BINDING-RESOLVER boot gate refuses routes whose model
        // has no resolver (fail closed).
        r.insert(
            "route.bindings",
            Arc::new(keel_models::RouteBindingsRef(Arc::new(
                keel_models::RouteBindingRegistry::new(),
            ))),
        )?;
        // Wave 4 Phase 4: well-known model.probes registry — modules
        // register one row probe per model in Provider::init; the
        // `unique`/`exists` validation rules fail closed (503) when their
        // model has no probe.
        r.insert(
            "model.probes",
            Arc::new(keel_models::ProbeRegistryRef::default()),
        )?;
        // Wave 3 Phase 6: well-known model.prune registry — modules register
        // `with_prunable` runtimes; `keel model:prune` sweeps them through
        // this store (empty registry → the CLI reports "none").
        r.insert(
            "model.prune",
            Arc::new(keel_models::PruneRegistryRef::default()),
        )
    }
}

#[cfg(test)]
mod error_render_tests {
    use super::*;
    use keel_http::ErrorRenderRegistry;
    use keel_lifecycle::ServiceRegistry;

    /// Wave 2: the error-render registry is published to the ServiceRegistry
    /// during `Provider::init`, so a module provider registers per-code
    /// status/view/reportable overrides and custom renderers into the very
    /// Arc the dispatch edge reads at request time.
    #[test]
    fn error_render_registry_reaches_modules_and_the_edge() {
        let registry = Arc::new(ErrorRenderRegistry::new());
        let provider = ErrorRenderProvider {
            registry: registry.clone(),
        };
        let mut services = ServiceRegistry::new();
        provider.init(&mut services).expect("provider init succeeds");

        let published = services
            .get::<ErrorRenderRegistry>()
            .expect("registry published to modules");
        published.set_status("KER_NOT_FOUND", axum::http::StatusCode::GONE);
        published.register(
            "KER_NOT_FOUND",
            Arc::new(|err: &keel_foundation::AppError| {
                Some(serde_json::json!({ "code": err.code }))
            }),
        );

        // Same instance: what a module registered is what the edge serves.
        assert_eq!(
            registry.status_for(&keel_foundation::AppError::not_found("x")),
            axum::http::StatusCode::GONE
        );
        let detail = registry
            .detail(&keel_foundation::AppError::not_found("x"))
            .expect("renderer registered");
        assert_eq!(detail["code"], serde_json::json!("KER_NOT_FOUND"));
    }
}
