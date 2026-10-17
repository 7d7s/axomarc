//! Boot topology for kelson-demo (11-SPEC §10/G03/G04).
//! Providers boot in phase order: Foundation -> Kernel -> Platform -> Modules.
//! DB probing happens BEFORE host boot (init must stay I/O-free); the probe
//! result selects the postgres or in-memory backend (degraded boot, D-52).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use kelson_command::CommandBus;
use kelson_config::FrozenConfig;
use kelson_context::{ExecutionContext, PrincipalContext, ScopeSnapshot, TenantContext};
use kelson_foundation::{
    AppError, AuthorizationRevision, ExecutionProfileKind, OperationId, PrincipalId, TenantId,
};
use kelson_execution::{
    AuthorizationPort, BoxFuture, ExecutionBackend, ExecutionPlan, ExecutionUnit, IdempotencyPort,
    InMemoryBackend, InMemoryIdempotency, OperationHandler,
};
use kelson_lifecycle::{BootError, Phase, Provider, ServiceRegistry};
use kelson_policy::{AccessSnapshot, PolicyAuthz, PolicyEngine};
use kelson_mysql::DbPool;
use kelson_postgres::{PgIdempotency, PostgresBackend};
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
    pub registry: Arc<kelson_http::ErrorRenderRegistry>,
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
/// Wave 9 B-31: the pool is the boot-selected [`DbPool`] variant (postgres
/// or mysql, from `[database].driver`); `None` keeps the in-memory
/// degradation posture (D-52).
pub struct KernelProvider {
    pub pool: Option<DbPool>,
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
                Some(DbPool::Postgres(pool)) => (
                    Arc::new(PostgresBackend::new(pool.clone())),
                    Arc::new(PgIdempotency::new(pool.clone())),
                ),
                Some(DbPool::MySql(pool)) => (
                    Arc::new(kelson_mysql::MySqlBackend::new(pool.clone())),
                    Arc::new(kelson_mysql::MySqlIdempotency::new(pool.clone())),
                ),
                None => (
                    Arc::new(InMemoryBackend::new()),
                    Arc::new(InMemoryIdempotency::new()),
                ),
            };
        let engine = Arc::new(kelson_execution::Engine::new(authz, backend, idem));
        r.insert("policy.engine", policy)?;
        r.insert("execution.engine", engine)
    }
}
/// Platform phase: framework services (cache, notify hub). CommandBus is built in
/// the Modules phase because handlers are module-owned.
pub struct PlatformProvider {
    /// Optional Redis cache (S10 B-12). When absent, the cache port stays on
    /// the in-memory adapter (single node; degraded boot, D-52).
    pub redis: Option<Arc<kelson_cache_redis::RedisCache>>,
    /// Wave 6 P3: the boot-loaded message catalog — the notify hub
    /// resolves notification strings through the same catalog as views
    /// and validation.
    pub messages: Arc<kelson_foundation::MessageCatalog>,
    /// Wave 6 P5: the boot's database-channel store — the hub writes rows
    /// through it (the very instance the HTTP feed reads).
    pub notifications: Arc<dyn kelson_notifications::NotificationStore>,
    /// Wave 6 P5: views engine for notification HTML mail bodies
    /// (`views/notifications/...`); `None` keeps mail text-only.
    pub views: Option<Arc<kelson_views::ViewsEngine>>,
    /// Wave 6 P6: the pool-gated enqueue seam — the driver-selected job
    /// store + the registered `notification.send` def, present only when a
    /// database pool exists. `None` leaves `queue_via` fail-closed
    /// (`not_implemented`) — the D-52 degradation posture.
    pub notification_queue: Option<(Arc<dyn kelson_jobs::JobStore>, Arc<kelson_jobs::JobRegistry>)>,
}

/// Wave 6 P5: the Wave 2 views engine behind the notification HTML port —
/// the service layer cannot depend on the web layer, so the runtime
/// adapts and maps render failures onto the error channel.
pub(crate) struct ViewsNotificationRenderer(pub(crate) Arc<kelson_views::ViewsEngine>);

impl kelson_notifications::NotificationViewRenderer for ViewsNotificationRenderer {
    fn render(&self, template: &str, context: Value) -> Result<String, AppError> {
        self.0
            .render(template, context)
            .map_err(|e| AppError::internal(format!("notification view render failed: {e}")))
    }
}

/// Wave 6 P6: the enqueue half of the queued-delivery seam — the runtime
/// owns it because the service crate cannot depend on the job store (19-PARITY
/// "queued delivery"). Renders happen at dispatch (request-time locale,
/// target, views); this port only durable-enqueues the pre-rendered
/// envelope under the def's `queue_via` name. `registry.build` refuses an
/// unregistered name, so a typo is a loud dispatch error, never a silent
/// drop. Wired pool-gated: the driver-selected job store (B-31) + the
/// registered `notification.send` def when a database pool exists; no pool
/// leaves `queue_via` fail-closed (`not_implemented`).
pub(crate) struct JobQueuePort {
    store: Arc<dyn kelson_jobs::JobStore>,
    registry: Arc<kelson_jobs::JobRegistry>,
}

impl JobQueuePort {
    pub(crate) fn new(store: Arc<dyn kelson_jobs::JobStore>, registry: Arc<kelson_jobs::JobRegistry>) -> Self {
        Self { store, registry }
    }
}

impl kelson_notifications::NotificationQueuePort for JobQueuePort {
    fn enqueue(
        &self,
        job_type: &str,
        delivery: &kelson_notifications::QueuedNotificationDelivery,
    ) -> Result<(), AppError> {
        let payload = serde_json::to_value(delivery)
            .map_err(|e| AppError::validation(format!("notification.send payload: {e}")))?;
        let job = self
            .registry
            .build(job_type, payload, time::OffsetDateTime::now_utc())?;
        self.store.enqueue(job).map(|_| ())
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
        let memory = kelson_cache_memory::MemoryCache::shared(4096);
        r.insert("cache.memory", memory.clone())?;
        let port: Arc<dyn kelson_caching::CachePort> = match &self.redis {
            Some(redis) => {
                r.insert("cache.redis", redis.clone())?;
                redis.clone()
            }
            None => memory,
        };
        r.insert(
            "cache.service",
            Arc::new(kelson_caching::CacheService::new(port.clone())),
        )?;
        r.insert("cache.port", Arc::new(CachePortRef(port)))?;
        let mut hub = kelson_notifications::NotifyHub::with_catalog(self.messages.clone())
            .with_store(self.notifications.clone());
        if let Some(views) = &self.views {
            hub = hub.with_view_renderer(Arc::new(ViewsNotificationRenderer(views.clone())));
        }
        if let Some((store, registry)) = &self.notification_queue {
            hub = hub.with_queue(Box::new(JobQueuePort::new(
                store.clone(),
                registry.clone(),
            )));
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
    /// Wave 9 B-30: the bus assembles strictly AFTER every handler bundle
    /// provider has inserted its bundle. (Init used to run in plain
    /// (phase, name) order — `module.demo` sorts BEFORE `module.oauth2`
    /// and `module.wasm`, so those bundles would have been registered
    /// into the registry too late to join the bus. The lifecycle host now
    /// inits in dependency order; declaring the edges makes that edge
    /// real and cycle-checked.)
    ///
    /// The edges MUST mirror what is actually registered: `module.wasm` always
    /// is, and Stage C.2 routed both application modules through it rather
    /// than through their own providers, so this list names the seams the
    /// graph binds rather than the crates a build happens to contain.
    fn dependencies(&self) -> Vec<&'static str> {
        vec!["module.wasm"]
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        let engine = r.get::<kelson_execution::Engine>()?;
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
        // Wave 8 P5 (B-21) the oauth2 module rode its own newtype bundle here;
        // Stage C.2 moved that module behind the seam, so nothing inserts the
        // newtype any more and the slot is gone with it.
        // Wave 9 B-30: WASM plugins ride their own newtype bundle too —
        // one handler per sidecar-verified READ capability. Registered
        // through the same provider graph, so the bus sees them exactly
        // like first-party handlers (Design C: sidecar manifests; the
        // console never links kelson-wasm).
        if let Ok(bundle) = r.get::<kelson_wasm::WasmHandlerBundle>() {
            for handler in bundle.0.iter() {
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
                feature_revision: kelson_foundation::FeatureRevision(1),
                module_revision: kelson_foundation::ModuleRevision(1),
                computed_at: OffsetDateTime::now_utc(),
            });
        }
        Ok(())
    }
}

/// WASM plugin module (Modules phase, Wave 9 B-30): publishes the
/// pre-loaded, drift-checked plugin handlers so the demo provider's bus
/// assembly picks them up. Loading happens BEFORE `host.boot()` (a plugin
/// that fails to compile, link, or whose guest manifest drifts from the
/// reviewed sidecar refuses the boot outright), so `init` only registers
/// — it never touches the filesystem or wasmtime.
pub struct WasmModulesProvider {
    /// `None` when the index declares no plugins — the provider still
    /// registers so `module.demo`'s declared dependency always resolves.
    pub bundle: Option<kelson_wasm::WasmHandlerBundle>,
}

impl Provider for WasmModulesProvider {
    fn name(&self) -> &str {
        "module.wasm"
    }
    fn phase(&self) -> Phase {
        Phase::Modules
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        if let Some(bundle) = &self.bundle {
            // ADR 0020: hand every generation the stores its seam imports
            // read and write. This runs at Modules phase, after Platform —
            // where `PersistenceProvider` and `IdentityProvider` create those
            // stores — and that ordering is the whole reason the wiring sits
            // here rather than in `load_wasm_plugins`, which runs BEFORE boot.
            //
            // Both are required. A bundle with no store behind it would still
            // serve, and a module would discover at first dispatch that it
            // reads nothing; failing here says which store was missing.
            let model = r.get::<kelson_models::ModelStoreRef>()?;
            let tokens = r.get::<TokenStoreRef>()?;
            // ADR 0020 §5: the fifth import reaches the same hub the
            // framework registered in the Platform phase, so a module that
            // declares a booking announces it through exactly the catalog,
            // locale and channels every other notification uses. A boot with
            // no hub wired still serves — the import then refuses loudly at
            // the first declaration, which is the posture the seam takes for
            // a store it was never given.
            let notify = r.get::<kelson_notifications::NotifyHub>().ok();
            let services = kelson_wasm::ModuleHostServices::new(
                Arc::clone(&model.0),
                Arc::clone(&tokens.0),
            );
            let services = match notify {
                Some(hub) => services.with_notifications(Arc::new(crate::HubSink(hub))),
                None => services,
            };
            bundle
                .install_services(services)
                .map_err(|e| BootError::ProviderFailed {
                    provider: "module.wasm".into(),
                    source: e.to_string(),
                })?;
            r.insert("module.wasm.handlers", Arc::new(bundle.clone()))?;
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
    pub registry: Arc<kelson_feature_flags::FlagRegistry>,
    pub store: Arc<dyn kelson_feature_flags::FlagStore>,
}

impl Provider for FeatureFlagsProvider {
    fn name(&self) -> &str {
        "platform.flags"
    }
    fn phase(&self) -> Phase {
        Phase::Platform
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        let service = Arc::new(kelson_feature_flags::FlagService::new(
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
    pub store: Arc<dyn kelson_session::SessionStore>,
}

impl Provider for SessionsProvider {
    fn name(&self) -> &str {
        "platform.sessions"
    }
    fn phase(&self) -> Phase {
        Phase::Platform
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        let manager = Arc::new(kelson_session::SessionManager::new(
            self.store.clone(),
            kelson_session::SessionPolicy::default(),
            self.secret.clone(),
        ));
        r.insert_with_lifetime(
            "sessions.manager",
            kelson_lifecycle::ServiceLifetime::Request,
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
        r.insert("realtime.hub", Arc::new(kelson_realtime::BroadcastHub::new()))
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
pub struct PrincipalStoreRef(pub Arc<dyn kelson_identity::PrincipalStore>);

#[derive(Clone)]
pub struct CredentialStoreRef(pub Arc<dyn kelson_identity::CredentialStore>);

/// The token store *behind* `security.tokens`. `TokenService` holds its
/// store privately, and the module seam needs the port rather than the
/// service — a guest implementing `TokenStore` must reach the same rows PAT
/// issuance writes, not a second copy of them (ADR 0020).
#[derive(Clone)]
pub struct TokenStoreRef(pub Arc<dyn kelson_tokens::TokenStore>);

#[derive(Clone)]
pub struct StorageRef(pub Arc<dyn kelson_storage::StoragePort>);

/// Wave 6 P2: the named-disk registry carrier — one registry per boot,
/// built from `[storage]` by `crate::disk_registry`.
#[derive(Clone)]
pub struct DiskRegistryRef(pub Arc<kelson_storage::DiskRegistry>);

/// Carrier for the active cache port (memory or redis; S10 B-12).
#[derive(Clone)]
pub struct CachePortRef(pub Arc<dyn kelson_caching::CachePort>);

/// Platform phase: identity + token services (S7 C2/B-02). Fabric-backed
/// when the driver-selected pool is up; in-memory on degraded boot (D-52).
pub struct IdentityProvider {
    pub pool: Option<DbPool>,
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
            Arc<dyn kelson_identity::PrincipalStore>,
            Arc<dyn kelson_identity::CredentialStore>,
            Arc<dyn kelson_tokens::TokenStore>,
        ) = match &self.pool {
            Some(DbPool::Postgres(pool)) => (
                Arc::new(kelson_postgres::PgPrincipalStore::new(pool.clone())),
                Arc::new(kelson_postgres::PgCredentialStore::new(pool.clone())),
                Arc::new(kelson_postgres::PgTokenStore::new(pool.clone())),
            ),
            Some(DbPool::MySql(pool)) => (
                Arc::new(kelson_mysql::MySqlPrincipalStore::new(pool.clone())),
                Arc::new(kelson_mysql::MySqlCredentialStore::new(pool.clone())),
                Arc::new(kelson_mysql::MySqlTokenStore::new(pool.clone())),
            ),
            None => (
                Arc::new(kelson_identity::InMemoryPrincipalStore::new()),
                Arc::new(kelson_identity::InMemoryCredentialStore::new()),
                Arc::new(kelson_tokens::InMemoryTokenStore::new()),
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
            "security.token_store",
            Arc::new(TokenStoreRef(token_store.clone())),
        )?;
        r.insert(
            "security.tokens",
            Arc::new(kelson_tokens::TokenService::new(token_store)),
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
    pub disks: kelson_config::StorageConfig,
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
/// (appointments first) consume the [`kelson_models::ModelStoreRef`] carrier;
/// fabric-backed when the driver-selected pool is up, in-memory on degraded
/// boot (D-52).
pub struct PersistenceProvider {
    pub pool: Option<DbPool>,
}

impl Provider for PersistenceProvider {
    fn name(&self) -> &str {
        "platform.persistence"
    }
    fn phase(&self) -> Phase {
        Phase::Platform
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        let store: Arc<dyn kelson_models::ModelStore> = match &self.pool {
            Some(DbPool::Postgres(pool)) => {
                Arc::new(kelson_postgres::PgModelStore::new(pool.clone()))
            }
            Some(DbPool::MySql(pool)) => Arc::new(kelson_mysql::MySqlModelStore::new(pool.clone())),
            None => Arc::new(kelson_models::InMemoryModelStore::new()),
        };
        r.insert("model.store", Arc::new(kelson_models::ModelStoreRef(store)))?;
        // Wave 1 model binding: the well-known route.bindings registry.
        // Modules register one resolver per bound model in Provider::init;
        // the GRAPH-BINDING-RESOLVER boot gate refuses routes whose model
        // has no resolver (fail closed).
        r.insert(
            "route.bindings",
            Arc::new(kelson_models::RouteBindingsRef(Arc::new(
                kelson_models::RouteBindingRegistry::new(),
            ))),
        )?;
        // Wave 4 Phase 4: well-known model.probes registry — modules
        // register one row probe per model in Provider::init; the
        // `unique`/`exists` validation rules fail closed (503) when their
        // model has no probe.
        r.insert(
            "model.probes",
            Arc::new(kelson_models::ProbeRegistryRef::default()),
        )?;
        // Wave 3 Phase 6: well-known model.prune registry — modules register
        // `with_prunable` runtimes; `kelson model:prune` sweeps them through
        // this store (empty registry → the CLI reports "none").
        r.insert(
            "model.prune",
            Arc::new(kelson_models::PruneRegistryRef::default()),
        )
    }
}

#[cfg(test)]
mod error_render_tests {
    use super::*;
    use kelson_http::ErrorRenderRegistry;
    use kelson_lifecycle::ServiceRegistry;

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
        published.set_status("KSN_NOT_FOUND", axum::http::StatusCode::GONE);
        published.register(
            "KSN_NOT_FOUND",
            Arc::new(|err: &kelson_foundation::AppError| {
                Some(serde_json::json!({ "code": err.code }))
            }),
        );

        // Same instance: what a module registered is what the edge serves.
        assert_eq!(
            registry.status_for(&kelson_foundation::AppError::not_found("x")),
            axum::http::StatusCode::GONE
        );
        let detail = registry
            .detail(&kelson_foundation::AppError::not_found("x"))
            .expect("renderer registered");
        assert_eq!(detail["code"], serde_json::json!("KSN_NOT_FOUND"));
    }
}

#[cfg(test)]
mod demo_module_dependency_tests {
    use super::*;

    /// Stage C.2 routed both application modules through `module.wasm`, so
    /// neither has a provider of its own to depend on. The declared edges
    /// must mirror what is actually registered in EITHER direction: naming a
    /// provider that no longer registers is refused by the lifecycle host as
    /// `UnknownDependency`, which took `kelson serve`, `call`, `shell` and
    /// `model:prune` down with it back when the mismatch first appeared.
    #[test]
    fn demo_module_dependencies_mirror_the_registered_providers() {
        let deps = DemoModuleProvider {
            dev_snapshot: false,
        }
        .dependencies();
        assert!(
            deps.contains(&"module.wasm"),
            "the wasm provider is registered unconditionally, so the edge stays"
        );

        let demo_edges = deps
            .iter()
            .any(|d| matches!(*d, "module.appointments" | "module.oauth2"));
        assert_eq!(
            demo_edges,
            false,
            "Stage C.2 removed those providers, so naming them would be a dead edge"
        );
    }
}
