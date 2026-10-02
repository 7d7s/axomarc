//! keel-api: HTTP edge host (11-SPEC SS20, S7 edge wiring). Binary entry is
//! `main.rs`; this lib exposes the routing modules and boot function so the
//! integration suite can drive the exact production wiring.

pub mod auth_routes;
pub mod broadcast_routes;
pub mod call;
pub mod dashboard;
pub mod edge;
pub mod graph_routes;
pub mod host;
pub mod i18n;
pub mod notification_routes;
pub mod batch_routes;
pub mod routes;
pub mod shell;
pub mod static_assets;
pub mod telemetry_routes;
pub mod upload_routes;

use std::sync::Arc;

use host::{
    ConfigProvider, CredentialStoreRef, DemoModuleProvider, DiskRegistryRef, FeatureFlagsProvider,
    IdentityProvider, KernelProvider, PersistenceProvider, PlatformProvider, PrincipalStoreRef,
    RealtimeProvider, SessionsProvider, StorageProvider, StorageRef, SystemHealth,
};
use keel_config::{Environment, PlatformConfig};
use keel_foundation::{PrincipalId, TenantId};
use keel_lifecycle::ApplicationHost;
pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cfg_path = std::env::var("KEEL_CONFIG").unwrap_or_else(|_| "config/dev.toml".into());
    // B-29: interactive console mode (`keel shell` spawns `keel-api --shell`).
    let shell_mode = std::env::args().any(|arg| arg == "--shell");
    // Wave 3 Phase 6: maintenance sweep (`keel model:prune` spawns
    // `keel-api --prune [--chunk N] [--tenant T]`) — boots the graph, runs
    // the registry sweep, prints per-model counts and exits.
    let args: Vec<String> = std::env::args().collect();
    let prune_mode = args.iter().any(|arg| arg == "--prune");
    let flag_value = |flag: &str| {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let prune_chunk: u32 = flag_value("--chunk")
        .and_then(|v| v.parse().ok())
        .unwrap_or(200);
    let prune_tenant: Option<TenantId> = flag_value("--tenant")
        .and_then(|v| uuid::Uuid::parse_str(&v).ok())
        .map(TenantId::from_uuid);
    // Wave 5 Phase 7: one-shot console-route dispatch (`keel call <name> [json]`
    // spawns `keel-api --call <name> [--input <json>] [--principal <uuid>]
    // [--tenant <uuid>]`) — boots the graph, dispatches through the CommandBus
    // under the operator principal, prints the JSON result and exits.
    let call_mode = args.iter().any(|arg| arg == "--call");
    let call_name = flag_value("--call").unwrap_or_default();
    let call_input = flag_value("--input");
    let call_principal = flag_value("--principal");
    let call_tenant = flag_value("--tenant");
    let cfg = PlatformConfig::load(Some(std::path::Path::new(&cfg_path)))?.freeze();
    let env = cfg.app().env.clone();
    let listen = cfg.app().listen.clone();

    // Sentinel (S5): the telemetry runtime replaces S1 `init_logging` as
    // the single logging init point. Layering: compiled defaults <-
    // `[telemetry]` section <- env overlay; the S1 env conventions stay
    // honored (RUST_LOG, KEEL_LOG_FILE, environment-driven console format).
    let mut telemetry_config =
        keel_observability::TelemetryConfig::from_section(cfg.telemetry());
    if cfg
        .telemetry()
        .and_then(|s| s.get("format"))
        .is_none()
    {
        telemetry_config.format = if cfg.app().env.is_prod()
            || matches!(cfg.app().env, Environment::Staging)
        {
            "json".into()
        } else {
            "pretty".into()
        };
    }
    if let Ok(level) = std::env::var("RUST_LOG") {
        telemetry_config.level = level;
    }
    if let Ok(path) = std::env::var("KEEL_LOG_FILE") {
        telemetry_config.exporters.insert(
            "file".into(),
            keel_observability::ExporterConfig {
                kind: "file".into(),
                target: Some(path),
                format: None,
                critical: true,
                partition_by_tenant: false,
                retry: None,
                breaker: None,
            },
        );
    }
    telemetry_config.apply_env(); // KEEL_TELEMETRY_* wins over the S1 knobs
    if shell_mode || prune_mode || call_mode {
        // The REPL/sweep/one-shot call owns stdout: keep the console
        // exporter from interleaving log lines with the printed output
        // (file/OTLP still run).
        telemetry_config.exporters.remove("console");
    }

    // Wave 4 Phase 5: the config is frozen here (post-boot mutation is
    // impossible by type — see keel_config::FrozenConfig).

    // Feature flags at boot (S3 item 32): decide which opt-in platform
    // services start. The boot input uses a synthetic internal actor.
    let root = std::env::current_dir()?;
    // Config cache (Wave 4 Phase 5): a present artifact must pin the exact
    // effective config boot just derived (file + environment overlay),
    // else boot is refused — drift-refuse semantics identical to the route
    // cache below.
    keel_registry::config_cache_check(
        &root,
        &keel_registry::config_hash(&cfg.cache_json()),
    )
    .map_err(Box::<dyn std::error::Error>::from)?;
    // Resolve telemetry storage paths against the workspace root, then
    // construct + start the runtime BEFORE the lifecycle host boots so
    // boot-time events flow through it (S1 parity); the provider boot()
    // below is idempotent.
    telemetry_config.rebase(&root);
    let telemetry_runtime = keel_observability::ObservabilityRuntime::new(
        telemetry_config,
        keel_observability::ServiceIdentity::new("keel-api", env.as_str()),
        &[],
    )
    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    telemetry_runtime
        .start()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    let flags_registry = Arc::new(keel_feature_flags::platform_registry());
    let flags_store: Arc<dyn keel_feature_flags::FlagStore> = Arc::new(
        keel_feature_flags::FileFlagStore::new(keel_feature_flags::default_flags_path(&root)),
    );
    let boot_caps = std::collections::BTreeSet::new();
    let boot_input = keel_feature_flags::FlagInput {
        environment: env.as_str(),
        tenant: None,
        region: None,
        capabilities: &boot_caps,
        actor: PrincipalId::from_uuid(
            uuid::Uuid::parse_str(host::BOOT_ACTOR_UUID).expect("static uuid"),
        ),
        internal_actor: true,
    };
    let flag_service =
        keel_feature_flags::FlagService::new(flags_registry.clone(), flags_store.clone());
    let boot_snapshot = flag_service
        .snapshot(&boot_input)
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    let sessions_enabled = boot_snapshot.flags.contains("app.sessions");
    let realtime_enabled = boot_snapshot.flags.contains("app.realtime");
    // S10 B-17/B-20: metrics recording is always on; EXPOSITION is flag-gated
    // (the handler checks the boot snapshot in state.flags). The dashboard
    // is served by default outside prod; prod needs the flag.
    let dashboard_enabled = boot_snapshot.flags.contains("app.dashboard")
        || matches!(env, Environment::Dev | Environment::Test);
    let enabled_flags: Vec<String> = boot_snapshot.flags.iter().cloned().collect();
    tracing::info!(flags = ?enabled_flags, "boot feature snapshot");

    // B-22: opt-in stateless HS256 JWT bearer interop (report R5). Wired only
    // when the `app.jwt` flag is on; hashed PAT + HMAC session stays the
    // default auth model. An empty jwt_secret disables interop with a warning
    // rather than booting with a zero-key verifier.
    let jwt = if boot_snapshot.flags.contains("app.jwt") {
        let secret = cfg.auth().jwt_secret.as_bytes().to_vec();
        if secret.is_empty() {
            tracing::warn!(
                "app.jwt is on but auth.jwt_secret is empty; JWT bearer interop disabled"
            );
            None
        } else {
            tracing::info!("stateless JWT bearer interop enabled (HS256)");
            Some(std::sync::Arc::new(keel_http::JwtHs256::new(secret)))
        }
    } else {
        None
    };

    let pool = match cfg.database() {
        Some(db) => {
            let probe = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                keel_postgres::create_pool(&db.url, db.max_connections),
            )
            .await;
            match probe {
                Ok(Ok(pool)) => {
                    if db.run_migrations {
                        keel_postgres::run_migrations(&pool, &root).await?;
                    }
                    // Dev bootstrap: register/login assign the fixed dev tenant, so a fresh
                    // database needs its tenants row to exist (FK target for domain writes).
                    // ON CONFLICT DO NOTHING keeps this idempotent; failure only warns.
                    if matches!(env, Environment::Dev | Environment::Test) {
                        if let Ok(dev_tenant) = uuid::Uuid::parse_str(host::DEV_TENANT_UUID) {
                            if let Err(e) = keel_postgres::seed_tenant(
                                &pool,
                                dev_tenant,
                                "dev",
                                "Development Tenant",
                            )
                            .await
                            {
                                tracing::warn!(error = %e, "dev tenant seed failed");
                            }
                        }
                    }
                    Some(pool)
                }
                other => {
                    if db.required {
                        eprintln!("boot refused: database required but unreachable ({other:?})");
                        std::process::exit(1);
                    }
                    tracing::warn!("database unreachable; booting degraded on in-memory backend");
                    None
                }
            }
        }
        None => None,
    };

    // Wave 1 route cache: a present artifact must match the current
    // manifests + middleware.toml (and module ABI) or boot is refused with a
    // pointer to `keel route:cache`; absent artifact = live compile.
    let route_plans = graph_routes::route_cache_check(&root)
        .map_err(|e| format!("route cache rejected: {e}"))?;
    // Wave 1 middleware model: middleware.toml at the discovery root (absent
    // file = builtin catalog); malformed files refuse boot (fail closed).
    let middleware_catalog = keel_http::MiddlewareCatalog::load(&root)
        .map_err(|e| format!("middleware catalog rejected: {e}"))?;
    // Wave 6 P3: the message catalog (`lang/<locale>.toml` at the discovery
    // root, `[i18n] paths`) — absent roots are empty catalogs, malformed
    // files or invalid locales refuse boot (the middleware.toml posture).
    let messages = Arc::new(
        keel_config::load_messages(&root, cfg.i18n())
            .map_err(|e| format!("message catalog rejected: {}", e.public_message))?,
    );

    let health = SystemHealth::new(
        pool.is_none(),
        if pool.is_none() {
            "postgres unreachable or not configured"
        } else {
            "ok"
        },
    );
    let secret = session_secret();
    let session_store: Arc<dyn keel_session::SessionStore> = match &pool {
        Some(pool) => Arc::new(keel_postgres::PgSessionStore::new(pool.clone())),
        None => keel_session::InMemorySessionStore::shared(),
    };
    // Wave 6 P5: the database-channel inbox store — postgres when reachable,
    // in-memory otherwise (D-52, the session-store posture). Built before the
    // providers so the notify hub and the HTTP feed share ONE instance.
    let notification_store: Arc<dyn keel_notifications::NotificationStore> = match &pool {
        Some(pool) => Arc::new(keel_postgres::PgNotificationStore::new(pool.clone())),
        None => Arc::new(keel_notifications::InMemoryNotificationStore::new()),
    };
    // Wave 6 P6: the pool-gated enqueue seam — `PgJobStore` + the registered
    // `notification.send` def, so `queue_via` defs build a durable job at
    // dispatch (the worker drains it). No pool = no port = `queue_via`
    // fail-closed (`not_implemented`), the D-52 posture.
    // Wave 6 P8: the SAME store instance doubles as the `/metrics` queue
    // census view (admin trait) — depth/age/finished, snapshot per scrape.
    // Wave 6 P9: and as the batch-operations store next to the durable
    // batch ledger (`PgBatchLedger`, B-32) the batch endpoints read.
    let (notification_queue, queue_stats, batch_ledger) = match &pool {
        Some(pool) => {
            let store = Arc::new(keel_postgres::PgJobStore::new(pool.clone()));
            let mut registry = keel_jobs::JobRegistry::new();
            registry
                .register(keel_jobs::JobDef::new(
                    keel_notifications::NOTIFICATION_SEND_JOB,
                ))
                .expect("notification.send def registers");
            (
                Some((
                    store.clone() as Arc<dyn keel_jobs::JobStore>,
                    Arc::new(registry),
                )),
                Some(store as Arc<dyn keel_jobs::JobStoreAdmin>),
                Some(Arc::new(keel_postgres::PgBatchLedger::new(
                    pool.clone(),
                )) as Arc<dyn keel_jobs::BatchLedger>),
            )
        }
        None => (None, None, None),
    };

    let mut host = ApplicationHost::new();
    // Wave 2 error rendering: one registry per boot, published to every
    // module provider (Foundation phase) and read by the dispatch edge.
    let error_renders = Arc::new(keel_http::ErrorRenderRegistry::new());
    host.add_provider(Box::new(host::ErrorRenderProvider {
        registry: error_renders.clone(),
    }));
    // Wave 2 terminating hooks: same pattern — one registry per boot that
    // module providers extend from Provider::init; the edge layer runs it.
    let terminating = Arc::new(crate::edge::TerminatingHooks::new());
    host.add_provider(Box::new(host::TerminatingProvider {
        hooks: terminating.clone(),
    }));
    // Sentinel runtime is adopted, not rebuilt, by the provider.
    host.add_provider(Box::new(
        keel_observability::ObservabilityProvider::from_runtime(telemetry_runtime.clone()),
    ));
    // S10 B-12: optional Redis cache. REDIS_URL present + reachable -> redis
    // backs the cache port; otherwise the port stays in-memory (D-52 parity).
    let redis_cache = match std::env::var("REDIS_URL") {
        Ok(url) => match keel_cache_redis::RedisCache::connect(&url) {
            Ok(cache) => {
                tracing::info!("cache backend: redis");
                Some(Arc::new(cache))
            }
            Err(e) => {
                tracing::warn!(
                    error = %e.public_message,
                    "redis unreachable; cache stays in-memory"
                );
                None
            }
        },
        Err(_) => None,
    };

    // S10 B-17: process metrics registry (recording is unconditional; the
    // /metrics route checks the `app.metrics` flag before answering).
    let metrics = Arc::new(keel_metrics::MetricsRegistry::new(4096));
    let booted_at = std::time::Instant::now();

    // Wave 2 views: engine built from the `[views]` section. Roots and the
    // compiled cache resolve against the workspace root, so the same templates
    // are served regardless of the cwd the host was started from. The
    // `@asset("...")` URL helper is registered here (once per engine) so
    // templates see production behaviour in every harness too. Built BEFORE
    // the providers (Wave 6 P5): the platform phase wires the engine into
    // the notify hub for notification HTML mail bodies.
    let assets = load_asset_manifest(&root.join("public"));
    let views_engine =
        keel_views::ViewsEngine::new(views_config(cfg.views(), &root, env.as_str()));
    register_asset_directive(&views_engine, assets.clone());
    // Wave 8 B-25: indexed packages register their view roots (no copy —
    // the namespace resolves into the package directory).
    register_package_view_namespaces(&root, &views_engine);
    let views = Some(Arc::new(views_engine));

    host.add_provider(Box::new(ConfigProvider {
        config: cfg.clone(),
        health: health.clone(),
    }));
    host.add_provider(Box::new(KernelProvider { pool: pool.clone() }));
    host.add_provider(Box::new(PlatformProvider {
        redis: redis_cache,
        messages: messages.clone(),
        notifications: notification_store.clone(),
        views: views.clone(),
        notification_queue,
    }));
    host.add_provider(Box::new(FeatureFlagsProvider {
        registry: flags_registry,
        store: flags_store,
    }));
    host.add_provider(Box::new(IdentityProvider { pool: pool.clone() }));
    host.add_provider(Box::new(StorageProvider {
        root: root.clone(),
        disks: cfg.storage().clone(),
    }));
    host.add_provider(Box::new(PersistenceProvider { pool: pool.clone() }));
    if sessions_enabled {
        host.add_provider(Box::new(SessionsProvider {
            secret: secret.clone(),
            store: session_store,
        }));
    }
    if realtime_enabled {
        host.add_provider(Box::new(RealtimeProvider));
    }
    // Modules phase: domain module first (registers its handler bundle), then
    // the demo provider assembles the CommandBus from every registered bundle.
    host.add_provider(Box::new(keel_mod_appointments::ModuleProvider));
    // Wave 8 P5 (B-21): the OAuth2 module registers its own handler bundle
    // (distinct newtype slot) BEFORE the bus assembly below.
    host.add_provider(Box::new(keel_mod_oauth2::ModuleProvider));
    host.add_provider(Box::new(DemoModuleProvider {
        dev_snapshot: matches!(env, Environment::Dev | Environment::Test),
    }));
    host.boot()
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    *health.services.lock().unwrap() = host.registry().service_names();
    tracing::info!(services = ?host.registry().service_names(), "keel-api booted");

    // Platform graph compile is a BOOT GATE (parity with `keel check`): a
    // broken manifest never produces a serving edge (S7 C1). It runs AFTER
    // provider boot on purpose (Wave 4 Phase 4): module providers register
    // their custom validation rules from `Provider::init`, and the
    // GRAPH-RULE-UNKNOWN gate consults that registry — compiling before
    // boot would refuse every manifest that names a legitimately
    // registered rule.
    let graph =
        graph_routes::compile_graph(&root).map_err(|e| format!("platform graph rejected: {e}"))?;
    tracing::info!(
        modules = graph.modules.len(),
        routes = graph.modules.iter().map(|m| m.routes.len()).sum::<usize>(),
        "platform graph compiled"
    );

    let bus = host
        .registry()
        .get::<keel_command::CommandBus>()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    let tokens = host
        .registry()
        .get::<keel_tokens::TokenService>()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    let policy = host
        .registry()
        .get::<keel_policy::PolicyEngine>()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    let principals = host
        .registry()
        .get::<PrincipalStoreRef>()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?
        .0
        .clone();
    let credentials = host
        .registry()
        .get::<CredentialStoreRef>()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?
        .0
        .clone();
    let storage = host
        .registry()
        .get::<StorageRef>()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?
        .0
        .clone();
    let disks = host
        .registry()
        .get::<DiskRegistryRef>()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?
        .0
        .clone();
    let sessions_mw = if sessions_enabled {
        let manager = host
            .registry()
            .get::<keel_session::SessionManager>()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
        Some(Arc::new(
            keel_http::SessionMiddleware::new(manager, secret.clone())
                .with_secure(env.is_prod())
                .with_cookie_ttl_secs(28_800),
        ))
    } else {
        None
    };

    // Capabilities granted to authenticated principals at login: every
    // capability declared by a graph module (`module.capability`) plus the
    // framework capabilities (Wave 6 P5 notification read-marking —
    // self-service but still a real, revocable gate).
    let granted_capabilities: Vec<String> = graph
        .modules
        .iter()
        .flat_map(|m| m.capabilities.keys().map(|c| format!("{}.{}", m.name(), c)))
        .chain(
            notification_routes::FRAMEWORK_CAPABILITIES
                .iter()
                .map(|c| (*c).to_string()),
        )
        .collect();
    let route_inventory = graph_routes::route_inventory(&graph);
    // Dev/test convenience tenant (host's stable dev uuid); prod requires an
    // explicit tenant on register/login until tenant resolution lands.
    let default_tenant_id = if matches!(env, Environment::Dev | Environment::Test) {
        Some(TenantId::from_uuid(
            uuid::Uuid::parse_str(host::DEV_TENANT_UUID).expect("static uuid"),
        ))
    } else {
        None
    };
    let registration_open = matches!(env, Environment::Dev | Environment::Test)
        || std::env::var("KEEL_ALLOW_REGISTER")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);

    // Wave 1 boot gates (fail closed, collect-all): every route's middleware
    // entries must resolve against the catalog, and every bound model must
    // have a resolver registered in route.bindings (modules register them
    // during boot). Violations refuse boot with all codes listed.
    let route_bindings = host
        .registry()
        .get::<keel_models::RouteBindingsRef>()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?
        .0
        .clone();
    // Wave 4 Phase 4: model.probes — row probes for the `unique`/`exists`
    // validation rules (modules register one per model during boot).
    let probes = host
        .registry()
        .get::<keel_models::ProbeRegistryRef>()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?
        .0
        .clone();
    let mut gate_violations = keel_registry::middleware_violations(&graph, &middleware_catalog);
    gate_violations.extend(keel_registry::binding_resolver_violations(
        &graph,
        &route_bindings.models(),
    ));
    if !gate_violations.is_empty() {
        let report = gate_violations
            .iter()
            .map(|v| format!("{}: {}", v.code, v.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("platform graph rejected by boot gates: {report}").into());
    }

    // Wave 2 static assets (19-PARITY "Static assets"): `public/` answers
    // real files through the router fallback installed below. The directory
    // is allowed to be missing (dev before the first build) — the resolver
    // then misses every request and unmatched paths render the not-found.
    let static_files = Some(Arc::new(keel_http::StaticFiles::new(
        root.join("public"),
    )));

    // Wave 7 P6 signed-URL TTL bounds (19-PARITY "over-bound →
    // boot-refusing config error"): a `max_ttl_secs` above the hard ceiling
    // refuses boot here, before anything can mint — the validated bound then
    // binds the gate below (`sign_once` refuses any TTL above it).
    let signed_ttl_bound = signed_url_max_ttl_secs(cfg.security())
        .map_err(|error| error.public_message)?;

    // Wave 2 signed URLs (19-PARITY "URL generation"): the `signed`
    // middleware builtin verifies links through this gate. An empty
    // application key leaves no trustworthy secret, so the gate stays off
    // and every route declaring `signed` refuses (fail-closed 500) instead
    // of accepting an unsigned link.
    let signed_urls = if cfg.app().key.is_empty() {
        tracing::warn!(
            "app.key is empty; routes declaring `signed` will refuse requests (fail-closed)"
        );
        None
    } else {
        Some(Arc::new(
            keel_http::SignedGate::new(
                keel_http::derive_signed_key(&cfg.app().key),
                tokens.clone(),
            )
            .with_max_ttl(signed_ttl_bound),
        ))
    };

    // Wave 2 subdomain tenancy (19-PARITY route host matching): the host
    // gate resolves a subdomain's tenant here and feeds it into tenant
    // resolution. The registry is the in-memory control-plane port until
    // the keel-postgres `tenants` adapter lands; Dev/Test get the seeded
    // dev tenant the database bootstrap writes, so `subdomain = "dev"`
    // resolves out of the box. Unknown slugs fall back to the default
    // tenant (the host match itself still applies).
    let tenant_store = Arc::new(keel_tenancy::InMemoryTenantStore::new());
    if matches!(env, Environment::Dev | Environment::Test) {
        if let Ok(dev_id) = uuid::Uuid::parse_str(host::DEV_TENANT_UUID) {
            use keel_tenancy::TenantStore as _;
            let seeded = tenant_store.upsert(keel_tenancy::TenantRecord {
                id: TenantId::from_uuid(dev_id),
                slug: "dev".into(),
                name: "Development Tenant".into(),
                plan: "standard".into(),
                status: keel_tenancy::TenantStatus::Active,
                region: "in".into(),
                capabilities: std::collections::BTreeSet::new(),
                created_at: time::OffsetDateTime::now_utc(),
                revision: keel_foundation::TenantRevision(1),
            });
            if let Err(error) = seeded {
                tracing::warn!(%error, "dev tenant not registered for subdomain resolution");
            }
        }
    }
    let tenants = Some(Arc::new(keel_tenancy::TenantService::new(tenant_store)));

    // Wave 7 P5: refuse boot on out-of-range `[identity]` factors — a typo
    // must not surface halfway through the first register or login.
    password_params_from(cfg.identity())
        .to_argon2()
        .map_err(|error| error.public_message)?;

    let state = routes::AppState {
        bus,
        health: health.clone(),
        env,
        flags: enabled_flags,
        tokens,
        sessions: sessions_mw,
        principals,
        credentials,
        policy,
        granted_capabilities,
        default_tenant_id,
        registration_open,
        require_verified_email: cfg.app().require_verified_email,
        confirm_timeout_secs: cfg.auth().confirm_timeout_secs,
        jwt,
        body_limit: 1024 * 1024,
        storage,
        disks,
        notifications: notification_store,
        channels: broadcast_routes::resolve_channels(&graph),
        presence: Arc::new(keel_realtime::PresenceHub::new()),
        queue: queue_stats,
        batches: batch_ledger,
        route_inventory,
        middleware_catalog,
        route_bindings,
        probes,
        throttle_buckets: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        route_plans,
        metrics: metrics.clone(),
        booted_at,
        dashboard_enabled,
        telemetry: Some(telemetry_runtime.clone()),
        views,
        messages,
        error_renders,
        debug: cfg.app().debug,
        problem_json: cfg.http().problem_json,
        cookie_cipher: cookie_cipher(cfg.http().encrypt_cookies, &cfg.app().key)
            .map_err(|error| error.public_message)?,
        terminating,
        signed_urls,
        tenants,
        static_files,
        assets,
        security_headers: security_headers_from(cfg.security()),
        password_params: password_params_from(cfg.identity()),
        dummy_hash: Arc::new(std::sync::OnceLock::new()),
        password_policy: password_policy_from(&cfg.auth().password_policy),
        csrf: csrf_guard_from(cfg.security()),
    };

    // B-29: `keel shell` — interactive console over the booted graph. Same
    // production boot (pool, host, flag snapshot, graph gate); the REPL runs
    // under the principal you log in as, and no listener is opened.
    if shell_mode {
        let result = shell::run(state, &graph).await;
        telemetry_runtime.shutdown(std::time::Duration::from_secs(5));
        return result;
    }

    // Wave 3 Phase 6: `keel model:prune` — sweep every registered prunable
    // model through the store in `--chunk`-sized pages, print per-model
    // counts, exit (same production boot as `--shell`; no listener opens).
    // An empty registry reports "none" — the `schedule:list` posture.
    // `--tenant <uuid>` scopes the sweep; without it tenant-scoped models
    // match no rows (documented: sweep the tenants individually).
    if prune_mode {
        let registry = host
            .registry()
            .get::<keel_models::PruneRegistryRef>()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?
            .0
            .clone();
        let store = host
            .registry()
            .get::<keel_models::ModelStoreRef>()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
        let results = registry.sweep(&*store.0, prune_tenant, prune_chunk)?;
        if results.is_empty() {
            println!("keel model:prune: none (no prunable models registered)");
        } else {
            for (model, count) in &results {
                println!("keel model:prune: {model} purged {count} row(s)");
            }
        }
        telemetry_runtime.shutdown(std::time::Duration::from_secs(5));
        return Ok(());
    }

    // Wave 5 Phase 7: `keel call` — one-shot dispatch of a routes/console.toml
    // command through the CommandBus under the operator principal (same boot
    // as `--shell`; the JSON result owns stdout, failures go to stderr with a
    // non-zero exit so scripts can branch on capability denial).
    if call_mode {
        let result = call::run(call::CallRequest {
            state: &state,
            graph: &graph,
            name: &call_name,
            input: call_input.as_deref(),
            principal: call_principal.as_deref(),
            tenant: call_tenant.as_deref(),
        })
        .await;
        telemetry_runtime.shutdown(std::time::Duration::from_secs(5));
        if let Err(message) = result {
            eprintln!("keel call: {message}");
            std::process::exit(1);
        }
        return Ok(());
    }

    // Edge surface: platform routes + graph module routes + auth + uploads.
    let router = routes::router(state.clone())
        .merge(graph_routes::graph_router(state.clone(), &graph))
        .merge(auth_routes::router(state.clone()))
        .merge(upload_routes::router(state.clone()))
        .merge(telemetry_routes::router(state.clone()))
        .merge(notification_routes::router(state.clone()))
        .merge(broadcast_routes::router(state.clone()))
        .merge(batch_routes::router(state.clone()));
    let router = if realtime_enabled {
        let hub = host
            .registry()
            .get::<keel_realtime::BroadcastHub>()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
        // Wave 6 P7: the mounted transports are AUTHORIZED — every
        // connect runs the host-side channel-token authorizer (issue the
        // token at `POST /api/v1/broadcast/auth`), and `presence-*`
        // channels join/leave the roster `GET /api/v1/broadcast/members`
        // reads: one shared `state.presence` instance behind both.
        router.merge(keel_realtime::realtime_router_authorized(
            (*hub).clone(),
            state.presence.clone(),
            Arc::new(broadcast_routes::ChannelTokenAuthorizer {
                tokens: state.tokens.clone(),
                principals: state.principals.clone(),
            }),
        ))
    } else {
        router
    };
    // Wave 2 static assets: `public/` answers real files LAST (a router
    // fallback), and a miss — traversal included — renders the same
    // not-found path a graph route miss uses.
    let router = static_assets::with_static_files(router, &state);
    // Sentinel context propagation (S5): build the per-request TelemetryContext
    // from the inbound traceparent (or a fresh root) joined to the admission
    // request id; handlers read it from extensions (never hand-crafted).
    let router = router.layer(axum::middleware::from_fn(
        telemetry_routes::telemetry_context,
    ));

    // Session middleware runs INSIDE admission (after rate-limit), as planned.
    let router = if let Some(mw) = &state.sessions {
        (**mw).clone().apply(router)
    } else {
        router
    };

    // Admission pipeline (S2 item 25): request-id -> trace -> body-limit ->
    // CORS -> maintenance -> rate-limit -> router.
    let admission = keel_http::Admission {
        maintenance: keel_http::MaintenanceGuard::file(root),
        ..Default::default()
    };
    let app = admission.apply(router);
    // Wave 7 P7: CSRF is no longer an opt-in global layer (the `KEEL_CSRF`
    // env gate is gone — no flag, no env). `graph_router` attaches the
    // double-submit guard per web-plane route, default-on with the
    // deployment's `[security.csrf]` skip paths; the api plane attaches
    // nothing and stays token-authenticated. Rejections still render with
    // the request-id/trace envelope because admission ran outside the
    // router, and metrics below still counts them.
    // S10 metrics middleware sits OUTERMOST so admission/CSRF rejections are
    // counted alongside happy paths.
    let app = app.layer(axum::middleware::from_fn_with_state(
        state.clone(),
        routes::record_metrics,
    ));

    // Wave 2 edge: queued-cookie flush + terminating hooks. Applied OUTERMOST
    // so both also run for admission/CSRF rejections, and the hooks fire once
    // the response is final (post-response, metrics/close-span safe).
    let app = app.layer(axum::middleware::from_fn_with_state(
        state.edge_config(),
        edge::edge_mw,
    ));

    let listener = tokio::net::TcpListener::bind(&listen).await?;
    tracing::info!(%listen, "listening");

    let serve = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutdown signal received");
        })
        .await;
    // Drain telemetry before exit (S5 graceful shutdown: flush buffers + WAL
    // within the deadline); runs on serve errors too.
    telemetry_runtime.shutdown(std::time::Duration::from_secs(5));
    serve?;
    Ok(())
}

/// Wave 2 views engine config from the `[views]` section: relative template
/// roots and the cache dir resolve against the workspace root, an empty path
/// list falls back to the Laravel default (`resources/views`), and the engine
/// environment follows `app.env` unless `[views] env` pins it.
fn views_config(
    section: &keel_config::ViewsSection,
    root: &std::path::Path,
    env: &str,
) -> keel_views::ViewsConfig {
    let resolve = |raw: &str| {
        let path = std::path::PathBuf::from(raw);
        if path.is_absolute() {
            path
        } else {
            root.join(path)
        }
    };
    let mut paths: Vec<std::path::PathBuf> = section.paths.iter().map(|p| resolve(p)).collect();
    if paths.is_empty() {
        paths.push(root.join("resources/views"));
    }
    keel_views::ViewsConfig {
        paths,
        cache_dir: section.cache_dir.as_deref().map(resolve),
        env: section.env.clone().unwrap_or_else(|| env.to_string()),
    }
}

/// Wave 2 static assets: locate the Vite build manifest under `public/`.
/// Vite 4 writes `build/manifest.json`, Vite 5+ nests it at
/// `build/.vite/manifest.json`. No manifest yet is the normal state before
/// the first build (`@asset` then emits literal URLs); an *unreadable* one
/// is logged loudly and degrades the same way rather than failing boot.
fn load_asset_manifest(public: &std::path::Path) -> Option<Arc<keel_http::AssetManifest>> {
    for relative in ["build/manifest.json", "build/.vite/manifest.json"] {
        let path = public.join(relative);
        if !path.is_file() {
            continue;
        }
        return match keel_http::AssetManifest::from_path(&path) {
            Ok(manifest) => {
                tracing::info!(
                    path = %path.display(),
                    entries = manifest.len(),
                    "asset manifest loaded"
                );
                Some(Arc::new(manifest))
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    path = %path.display(),
                    "asset manifest unreadable; @asset falls back to literal URLs"
                );
                None
            }
        };
    }
    tracing::debug!(dir = %public.display(), "no Vite build manifest yet");
    None
}

/// Wave 2 asset URL helper in templates: `@asset("...")` resolves the key
/// through the Vite build manifest when it is known (a source key maps to
/// its hashed artifact) and otherwise emits the literal root-absolute URL —
/// Laravel's `asset()` helper as a view directive. The handler owns
/// escaping, so the URL always leaves the template HTML-safe.
fn register_asset_directive(
    engine: &keel_views::ViewsEngine,
    assets: Option<Arc<keel_http::AssetManifest>>,
) {
    engine.register_directive("asset", move |_name, arg, _ctx| {
        let raw = arg.unwrap_or_default().trim();
        let key = raw.trim_matches(&['"', '\''][..]).trim();
        let url = match assets.as_ref() {
            Some(manifest) => manifest.url(key),
            None => keel_http::asset_url(key),
        };
        Ok(keel_views::html_escape(&url))
    });
}

/// Wave 8 B-25 (B-24 index): every indexed package serves its `views/`
/// templates under its own namespace — `billing::welcome` resolves to
/// `<pkg>/views/welcome.html` (dots map to directories, the same rule the
/// app-side names use). No index (or an unusable one) installs no
/// resolver, so `pkg::` names keep failing closed exactly like an engine
/// without one; the boot config layer already refuses a malformed index,
/// fail-closed, before this runs.
fn register_package_view_namespaces(root: &std::path::Path, engine: &keel_views::ViewsEngine) {
    let Ok(index) = keel_registry::PackageIndex::load(root) else {
        return;
    };
    let roots: std::collections::HashMap<String, std::path::PathBuf> = index
        .package
        .iter()
        .map(|entry| {
            (
                entry.name.clone(),
                keel_registry::PackageIndex::entry_dir(root, entry).join("views"),
            )
        })
        .collect();
    if roots.is_empty() {
        return;
    }
    engine.set_resolver(move |ns, name| {
        let base = roots.get(ns)?;
        let rel = name.replace('.', "/");
        let file = if rel.ends_with(".html") {
            rel
        } else {
            format!("{rel}.html")
        };
        let candidate = base.join(file);
        candidate.is_file().then_some(candidate)
    });
}

/// Wave 2 queued-cookie encryption (opt-in via `[http] encrypt_cookies`).
/// The master key is derived from the application key and scoped to the
/// cookie context, so one derived key cannot open another context's
/// material. The web layer receives only the foundation `Sealer` port —
/// which cipher is in use is a security-class decision, not a web one.
fn cookie_cipher(
    enabled: bool,
    app_key: &str,
) -> Result<Option<Arc<keel_http::CookieCipher>>, keel_foundation::AppError> {
    if !enabled {
        return Ok(None);
    }
    let material = keel_http::derive_cookie_key(app_key);
    let sealer = keel_tokens::AesGcmSealer::from_secret(&keel_foundation::Secret::new(material))?;
    Ok(Some(Arc::new(keel_http::CookieCipher::new(
        Arc::new(sealer),
        "cookies.v1",
    ))))
}

/// Wave 7 P4 (`[security.headers]` → wire posture): the application layer
/// holds plain config data (it may not depend on the web class), so boot
/// resolves the section into the web layer's `SecurityHeaders` here — the
/// same conversion pattern as `ViewsSection` → `ViewsConfig`. The api plane
/// never consults the result; `graph_router` attaches it to web-plane
/// routes only.
pub fn security_headers_from(
    section: &keel_config::SecuritySection,
) -> keel_http::SecurityHeaders {
    keel_http::SecurityHeaders {
        enabled: section.headers.enabled,
        hsts_max_age_secs: section.headers.hsts_max_age_secs,
        hsts_include_subdomains: section.headers.hsts_include_subdomains,
        csp: section.headers.csp.clone(),
        referrer_policy: section.headers.referrer_policy.clone(),
    }
}

/// Wave 7 P5 (`[identity]` → verifier factors): the application layer holds
/// plain config data (it may not depend on the security class), so boot
/// resolves the section here — the same conversion pattern as
/// `security_headers_from`. Login compares a stored verifier's
/// PHC-embedded factors against the result and rehashes on drift; register
/// and reset-redeem hash directly with it.
pub fn password_params_from(
    section: &keel_config::IdentitySection,
) -> keel_identity::PasswordParams {
    keel_identity::PasswordParams {
        memory: section.memory,
        time: section.time,
        parallelism: section.parallelism,
    }
}

/// Wave 7 P6 (`[auth.password_policy]` → the composable + the flows): plain
/// config data in, the validation layer's policy out — the same conversion
/// pattern as `password_params_from`. `graph_routes` attaches the result to
/// every dispatch context (the Wave 4 `password` rule reads it) and
/// register/password-reset enforce the same policy directly.
pub fn password_policy_from(
    section: &keel_config::PasswordPolicySection,
) -> keel_validation::PasswordPolicy {
    keel_validation::PasswordPolicy {
        min_length: section.min_length,
        letters: section.letters,
        numbers: section.numbers,
        symbols: section.symbols,
        mixed_case: section.mixed_case,
    }
}

/// Wave 7 P7 (`[security.csrf]` → the web-plane guard): plain config data
/// in, the web layer's `CsrfGuard` out — the same conversion pattern as
/// `security_headers_from`. The guard itself is ALWAYS attached to
/// web-plane routes (default-on, no flag or env); this resolves only its
/// skip paths, so a deployment names receivers in config alone.
pub fn csrf_guard_from(section: &keel_config::SecuritySection) -> keel_http::CsrfGuard {
    keel_http::CsrfGuard {
        bypass_paths: section.csrf.skip_paths.clone(),
    }
}

/// Wave 7 P6: hard ceiling on `security.signed_urls.max_ttl_secs` — 30 days.
/// A configured bound above it refuses boot (19-PARITY "over-bound →
/// boot-refusing config error"): the TTL cap itself must stay sane, so a
/// misconfiguration can never mint links that outlive a key-rotation window.
pub const SIGNED_URL_TTL_CEILING_SECS: u64 = 2_592_000;

/// Wave 7 P6 (`[security.signed_urls]` → the mint bound): validate the
/// configured bound and hand it back for `SignedGate::with_max_ttl`. A
/// bound above [`SIGNED_URL_TTL_CEILING_SECS`] is a config error that
/// refuses boot — the helper exists so the refusal is unit-testable
/// without opening a listener.
pub fn signed_url_max_ttl_secs(
    section: &keel_config::SecuritySection,
) -> Result<u64, keel_foundation::AppError> {
    let secs = section.signed_urls.max_ttl_secs;
    if secs > SIGNED_URL_TTL_CEILING_SECS {
        return Err(keel_foundation::AppError::validation(format!(
            "security.signed_urls.max_ttl_secs={secs} exceeds the hard ceiling of {SIGNED_URL_TTL_CEILING_SECS} seconds"
        )));
    }
    Ok(secs)
}

/// Session cookie HMAC secret: explicit env wins; otherwise a per-boot
/// random value (in-memory sessions never survive a restart anyway; durable
/// pg sessions must set KEEL_SESSION_SECRET to stay valid across restarts).
fn session_secret() -> Vec<u8> {
    if let Ok(raw) = std::env::var("KEEL_SESSION_SECRET") {
        return raw.into_bytes();
    }
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(uuid::Uuid::now_v7().as_bytes());
    bytes.extend_from_slice(uuid::Uuid::now_v7().as_bytes());
    bytes
}

/// Wave 6 P2: build the named-disk registry from the `[storage]` section.
///
/// Declared `[storage.disks.<name>]` entries materialize through their
/// adapters (`local` -> `LocalDisk` at `root`, `s3` -> the in-memory
/// sandbox); with nothing declared the registry holds one synthetic
/// `local` disk at `fallback_root`, preserving the pre-disk dev/test
/// behavior. Failures refuse boot: a `local` disk without `root`, an `s3`
/// disk without `bucket`, or a `storage.default` naming no registered
/// disk. (Driver typos are already refused by the typed config parse.)
/// The runtime owns this mapping because the layer matrix keeps adapter
/// constructors out of the application config and service crates.
pub fn disk_registry(
    section: &keel_config::StorageConfig,
    fallback_root: &std::path::Path,
) -> Result<keel_storage::DiskRegistry, keel_foundation::AppError> {
    use keel_foundation::AppError;

    let mut registry = keel_storage::DiskRegistry::new(section.default.clone());
    if section.disks.is_empty() {
        let port: Arc<dyn keel_storage::StoragePort> =
            Arc::new(keel_storage_local::LocalDisk::new(fallback_root)?);
        registry.register(
            "local",
            keel_storage::DiskEntry {
                port,
                visibility: keel_storage::Visibility::Private,
                base_url: None,
            },
        )?;
    }
    for (name, disk) in &section.disks {
        let port: Arc<dyn keel_storage::StoragePort> = match disk.driver {
            keel_config::DiskDriver::Local => {
                let root = disk.root.as_deref().ok_or_else(|| {
                    AppError::validation(format!("storage disk {name:?} (local) needs a `root`"))
                })?;
                Arc::new(keel_storage_local::LocalDisk::new(root)?)
            }
            keel_config::DiskDriver::S3 => {
                let _bucket = disk.bucket.as_deref().ok_or_else(|| {
                    AppError::validation(format!("storage disk {name:?} (s3) needs a `bucket`"))
                })?;
                Arc::new(keel_storage_s3::S3Sandbox::new())
            }
        };
        let visibility = match disk.visibility {
            keel_config::DiskVisibility::Public => keel_storage::Visibility::Public,
            keel_config::DiskVisibility::Private => keel_storage::Visibility::Private,
        };
        registry.register(
            name,
            keel_storage::DiskEntry {
                port,
                visibility,
                base_url: disk.url.clone(),
            },
        )?;
    }
    registry.validate_default()?;
    Ok(registry)
}

// ------------------------------------------------------------ test harness

/// In-memory application state for the integration suite (S7 test plan) and
/// dev harnesses. Mirrors production wiring minus PostgreSQL and the
/// lifecycle host: same bus assembly, same stores, same auth model.
pub fn build_app_state(
    storage_root: &std::path::Path,
    sessions_enabled: bool,
    registration_open: bool,
) -> (
    routes::AppState,
    keel_registry::PlatformGraph,
    Arc<keel_mod_appointments::AppointmentStore>,
) {
    use keel_execution::{
        AuthorizationPort, Engine, InMemoryBackend, InMemoryIdempotency, OperationHandler,
    };
    use keel_policy::{AccessSnapshot, PolicyAuthz, PolicyEngine};

    let policy = Arc::new(PolicyEngine::new());
    let authz: Arc<dyn AuthorizationPort> = Arc::new(PolicyAuthz {
        engine: policy.clone(),
    });
    let engine = Arc::new(Engine::new(
        authz,
        Arc::new(InMemoryBackend::new()),
        Arc::new(InMemoryIdempotency::new()),
    ));

    let appt_backend = Arc::new(keel_models::InMemoryModelStore::new());
    let appt_store = Arc::new(keel_mod_appointments::AppointmentStore::new(
        appt_backend.clone(),
    ));

    // Wave 6 P3/P5/P6: the harness catalog (`<storage_root>/lang`, absent
    // = the empty default catalog) and a fresh in-memory inbox store, plus
    // the P6 producer hub sharing BOTH — the very store the HTTP feed
    // reads — so a book dispatch is observable end to end over HTTP. Views
    // for notification HTML ride the production `PlatformProvider` (this
    // harness's producer dispatches the Database channel only).
    let messages = Arc::new(
        keel_config::load_messages(storage_root, &keel_config::I18nSection::default())
            .expect("harness message catalog loads"),
    );
    let notification_store: Arc<dyn keel_notifications::NotificationStore> =
        Arc::new(keel_notifications::InMemoryNotificationStore::new());
    let notify_hub = Arc::new(
        keel_notifications::NotifyHub::with_catalog(messages.clone())
            .with_store(notification_store.clone()),
    );

    let mut bus = keel_command::CommandBus::new(engine);
    bus.register(Arc::new(host::EchoHandler::new()))
        .expect("demo handler registers");
    let bundle: Vec<Arc<dyn OperationHandler>> = vec![
        Arc::new(
            keel_mod_appointments::BookHandler::new(appt_store.clone())
                .with_notifier(Some(notify_hub.clone())),
        ),
        Arc::new(
            keel_mod_appointments::CancelHandler::new(appt_store.clone())
                .with_notifier(Some(notify_hub.clone())),
        ),
        Arc::new(keel_mod_appointments::ListHandler::new(appt_store.clone())),
        Arc::new(keel_mod_appointments::AvailabilityHandler::new(
            appt_store.clone(),
        )),
    ];
    for handler in bundle {
        bus.register(handler)
            .expect("appointments handlers register");
    }

    // Dev golden-path snapshot: demo.echo stays authorized without identity.
    policy.install_snapshot(AccessSnapshot {
        principal_id: PrincipalId::from_uuid(
            uuid::Uuid::parse_str(host::DEV_PRINCIPAL_UUID).expect("static uuid"),
        ),
        tenant_id: TenantId::from_uuid(
            uuid::Uuid::parse_str(host::DEV_TENANT_UUID).expect("static uuid"),
        ),
        membership_active: true,
        capabilities: ["demo.echo".to_string()].into_iter().collect(),
        scope_paths: ["LE1".to_string()].into_iter().collect(),
        authorization_revision: policy.revision(),
        feature_revision: keel_foundation::FeatureRevision(1),
        module_revision: keel_foundation::ModuleRevision(1),
        computed_at: time::OffsetDateTime::now_utc(),
    });

    // Graph from the real appointments manifest (compile-path parity with
    // `keel graph`, independent of process cwd).
    let manifest = keel_registry::ModuleManifest::from_toml(include_str!(
        "../../../applications/appointments/module.toml"
    ))
    .expect("appointments manifest parses");
    // Wave 8 P5 (B-21): the harness graph carries BOTH modules — routes,
    // capabilities (`oauth2.*`) and the granted-capability snapshots that
    // register installs then match production discovery, which compiles
    // every `applications/*/module.toml`.
    let oauth_manifest = keel_registry::ModuleManifest::from_toml(include_str!(
        "../../../applications/oauth2/module.toml"
    ))
    .expect("oauth2 manifest parses");
    let graph = keel_registry::PlatformGraph::compile(vec![manifest, oauth_manifest])
        .expect("graph compiles");
    let granted_capabilities: Vec<String> = graph
        .modules
        .iter()
        .flat_map(|m| m.capabilities.keys().map(|c| format!("{}.{}", m.name(), c)))
        .chain(
            notification_routes::FRAMEWORK_CAPABILITIES
                .iter()
                .map(|c| (*c).to_string()),
        )
        .collect();

    let tokens = Arc::new(keel_tokens::TokenService::new(Arc::new(
        keel_tokens::InMemoryTokenStore::new(),
    )));
    // Wave 8 P5 (B-21): the oauth2 handlers need the token service, so they
    // register onto the harness bus here (after `tokens`, before the bus is
    // moved into AppState). The client registry gets its own in-memory
    // backend — `oauth_clients` is global, never tenant keyed.
    let oauth_store = Arc::new(keel_mod_oauth2::OauthClientStore::new(Arc::new(
        keel_models::InMemoryModelStore::new(),
    )));
    for handler in keel_mod_oauth2::handlers(oauth_store, tokens.clone()) {
        bus.register(handler).expect("oauth2 handlers register");
    }
    let principals: Arc<dyn keel_identity::PrincipalStore> =
        Arc::new(keel_identity::InMemoryPrincipalStore::new());
    let credentials: Arc<dyn keel_identity::CredentialStore> =
        Arc::new(keel_identity::InMemoryCredentialStore::new());
    let sessions = if sessions_enabled {
        let secret = b"integration-cookie-secret-0123456789".to_vec();
        let manager = Arc::new(keel_session::SessionManager::new(
            keel_session::InMemorySessionStore::shared(),
            keel_session::SessionPolicy::default(),
            secret.clone(),
        ));
        Some(Arc::new(
            keel_http::SessionMiddleware::new(manager, secret).with_cookie_ttl_secs(28_800),
        ))
    } else {
        None
    };
    // Wave 6 P2: the harness fills the same disk registry the storage
    // provider builds — the default config's synthetic `local` disk rooted
    // at the harness storage root — so the upload edge and registry resolve
    // identically to production.
    let disks = disk_registry(&keel_config::StorageConfig::default(), storage_root)
        .expect("synthetic harness disk");
    let storage: Arc<dyn keel_storage::StoragePort> = disks
        .get(disks.default_name())
        .expect("synthetic local disk registered")
        .port
        .clone();

    // Wave 1 route binding registry: fresh per harness, with the
    // appointments reference resolver registered (production boot wires it
    // through the module provider + the route.bindings service instead).
    let route_bindings = Arc::new(keel_models::RouteBindingRegistry::new());
    route_bindings.register(Arc::new(
        keel_mod_appointments::AppointmentBindingResolver::new(appt_store.clone()),
    ));

    // Wave 4 Phase 4: model.probes — the harness registers the appointments
    // row probe directly (production wires it through the module provider +
    // the well-known service instead), so `unique`/`exists` rules run
    // against the same store the handlers write to.
    let probes = Arc::new(keel_models::ProbeRegistry::new());
    probes.register(Arc::new(keel_models::StoreProbe::new(
        keel_mod_appointments::appointment_model(),
        appt_backend,
    )));

    // Wave 2 views: the harness points the engine at <storage_root>/views so
    // tests can drop fixture templates, and keeps the compiled cache off disk
    // (each harness gets a fresh temp dir). `@asset` is registered against
    // the fixture build manifest when one exists under <storage_root>/public,
    // so the helper behaves in tests exactly as it does in production.
    let assets = load_asset_manifest(&storage_root.join("public"));
    let views_engine = keel_views::ViewsEngine::new(keel_views::ViewsConfig {
        paths: vec![storage_root.join("views")],
        cache_dir: None,
        env: "test".into(),
    });
    register_asset_directive(&views_engine, assets.clone());
    // Wave 8 B-25: the harness mirrors the production posture — an index
    // under the storage root registers package view namespaces too.
    register_package_view_namespaces(storage_root, &views_engine);
    let views = Some(Arc::new(views_engine));
    // Wave 2 static assets: the harness resolves <storage_root>/public —
    // tests opt into the fallback through `static_assets::with_static_files`
    // exactly as `run()` does for production.
    let static_files = Some(Arc::new(keel_http::StaticFiles::new(
        storage_root.join("public"),
    )));
    // Wave 2 error rendering: the harness shares the registry the same way
    // production does (services may register into it), with both transport
    // gates closed so tests opt in explicitly.
    let error_renders = Arc::new(keel_http::ErrorRenderRegistry::new());

    // Wave 2 routing edge: a fixed harness application key backs the signed
    // URL gate, and a fresh (empty) tenant registry tests can seed through
    // the public store API feeds the host gate's subdomain resolution.
    let signed_urls = Some(Arc::new(keel_http::SignedGate::new(
        b"harness-app-key-for-signed-urls".to_vec(),
        tokens.clone(),
    )));
    let tenants = Some(Arc::new(keel_tenancy::TenantService::new(Arc::new(
        keel_tenancy::InMemoryTenantStore::new(),
    ))));

    // Wave 6 P3/P5: the catalog and the in-memory inbox store are built
    // next to the producer hub at the top of this harness (one instance
    // shared by the dispatch and the feed).

    let state = routes::AppState {
        bus: Arc::new(bus),
        health: host::SystemHealth::new(false, "ok"),
        env: Environment::Test,
        flags: Vec::new(),
        tokens,
        sessions,
        principals,
        credentials,
        policy,
        granted_capabilities,
        default_tenant_id: Some(TenantId::from_uuid(
            uuid::Uuid::parse_str(host::DEV_TENANT_UUID).expect("static uuid"),
        )),
        registration_open,
        require_verified_email: false,
        confirm_timeout_secs: 900,
        jwt: None,
        body_limit: 1024 * 1024,
        storage,
        disks: Arc::new(disks),
        notifications: notification_store,
        channels: broadcast_routes::resolve_channels(&graph),
        presence: Arc::new(keel_realtime::PresenceHub::new()),
        // No pool in the builder: no durable store — the /metrics queue
        // series stays absent (tests inject one to prove the census).
        // Wave 6 P9: same posture for the batch ledger — tests inject a
        // ledger + admin store to prove the batch operations.
        queue: None,
        batches: None,
        route_inventory: graph_routes::route_inventory(&graph),
        middleware_catalog: keel_http::MiddlewareCatalog::builtin(),
        route_bindings,
        probes,
        throttle_buckets: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        route_plans: None,
        metrics: Arc::new(keel_metrics::MetricsRegistry::new(4096)),
        booted_at: std::time::Instant::now(),
        dashboard_enabled: true,
        // Sentinel (S5): an INERT runtime per harness — the incident plane,
        // status snapshot and capability gate all work without the worker
        // threads, so integration tests get production-shaped wiring without
        // spawning (and leaking) runtime threads per test case.
        telemetry: Some(
            keel_observability::ObservabilityRuntime::new(
                {
                    let mut tcfg = keel_observability::TelemetryConfig::default();
                    tcfg.wal.dir = storage_root.join("telemetry/wal");
                    tcfg.paths.incident_dir = storage_root.join("telemetry/control");
                    tcfg.paths.cost_file = storage_root.join("telemetry/cost.json");
                    tcfg.paths.health_dir = storage_root.join("telemetry/exporters");
                    tcfg
                },
                keel_observability::ServiceIdentity::new("keel-api-test", "test"),
                &[],
            )
            .expect("telemetry runtime constructs"),
        ),
        views,
        messages,
        error_renders,
        debug: false,
        problem_json: false,
        // Harness default: no cookie encryption, no terminating hooks. Both
        // are wired by `run()` (and layered by integration tests directly).
        cookie_cipher: None,
        terminating: Arc::new(crate::edge::TerminatingHooks::new()),
        signed_urls,
        tenants,
        static_files,
        assets,
        // Harness default: the P4 posture stays off (the switch is per
        // config); tests inject one to prove emission end to end.
        security_headers: keel_http::SecurityHeaders::default(),
        // Harness default: the argon2 baseline factors (the P5 drifted
        // fixture seeds its verifier through the stores directly).
        password_params: keel_identity::PasswordParams::default(),
        dummy_hash: Arc::new(std::sync::OnceLock::new()),
        // Harness default: the Laravel-posture policy — the P6 register/
        // reset enforcement tests exercise exactly the stock default.
        password_policy: keel_validation::PasswordPolicy::default(),
        // Harness default: the P7 posture — always-on for web-plane routes
        // with no skip paths; tests inject a bypass to prove it.
        csrf: keel_http::CsrfGuard::default(),
    };
    (state, graph, appt_store)
}

#[cfg(test)]
mod views_tests {
    use super::views_config;

    #[test]
    fn relative_paths_resolve_against_the_workspace_root() {
        let section = keel_config::ViewsSection {
            paths: vec!["resources/views".into(), "modules/blog/views".into()],
            cache_dir: Some("storage/framework/views".into()),
            env: None,
        };
        let cfg = views_config(&section, std::path::Path::new("/srv/app"), "dev");
        assert_eq!(
            cfg.paths,
            vec![
                std::path::PathBuf::from("/srv/app/resources/views"),
                std::path::PathBuf::from("/srv/app/modules/blog/views"),
            ]
        );
        assert_eq!(
            cfg.cache_dir,
            Some(std::path::PathBuf::from(
                "/srv/app/storage/framework/views"
            ))
        );
        assert_eq!(cfg.env, "dev", "env follows app.env when not pinned");
    }

    #[test]
    fn absolute_paths_and_pinned_env_win() {
        let abs = std::env::temp_dir().join("keel-abs-views");
        let section = keel_config::ViewsSection {
            paths: vec![abs.display().to_string()],
            cache_dir: None,
            env: Some("staging".into()),
        };
        let cfg = views_config(&section, std::path::Path::new("/srv/app"), "prod");
        assert_eq!(cfg.paths, vec![abs], "absolute roots are not re-rooted");
        assert!(cfg.cache_dir.is_none(), "None disables the compiled cache");
        assert_eq!(cfg.env, "staging");
    }

    #[test]
    fn empty_path_list_falls_back_to_the_laravel_default() {
        let section = keel_config::ViewsSection {
            paths: Vec::new(),
            cache_dir: None,
            env: None,
        };
        let cfg = views_config(&section, std::path::Path::new("/srv/app"), "dev");
        assert_eq!(
            cfg.paths,
            vec![std::path::PathBuf::from("/srv/app/resources/views")]
        );
    }
}

#[cfg(test)]
mod wave6_mail_tests {
    /// Wave 6 P1: the mail attachment cap mirrors the upload limit — both
    /// default to 8 MiB, so any attachment the upload edge accepts is
    /// always sendable (and a send never demands more than the edge
    /// stores). The two constants must not drift apart.
    #[test]
    fn mail_attachment_cap_matches_upload_limits() {
        assert_eq!(
            keel_notifications::DEFAULT_MAX_ATTACHMENT_BYTES as usize,
            keel_http::UploadLimits::default().max_file_bytes
        );
    }

    /// Wave 6 P5: notification HTML bodies render through the REAL Wave 2
    /// views engine — a fixture `views/notifications/<type>.html` receives
    /// the `to_database()` payload as context, and a template that cannot
    /// render fails the dispatch (fail-closed once a view port is wired).
    /// Wave 8 B-25: an indexed package registers its `views/` root under
    /// its own namespace (`billing::welcome`), unknown namespaces fail
    /// closed, and a package-less project installs no resolver at all
    /// (namespace lookups behave exactly like an unwired engine).
    #[test]
    fn package_view_namespaces_register_from_the_index() {
        let root = std::env::temp_dir().join(format!("keel-pkgviews-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(root.join("packages").join("billing").join("views"))
            .expect("view dirs");
        std::fs::write(
            root.join("packages").join("billing").join("views").join("welcome.html"),
            "pkg says hi",
        )
        .expect("fixture template");
        std::fs::write(
            root.join("packages.toml"),
            "[[package]]\nname = \"billing\"\nversion = \"0.1.0\"\npath = \"packages/billing\"\n",
        )
        .expect("index");

        let engine = keel_views::ViewsEngine::new(keel_views::ViewsConfig {
            paths: vec![],
            cache_dir: None,
            env: "test".into(),
        });
        crate::register_package_view_namespaces(&root, &engine);
        assert_eq!(
            engine
                .render("billing::welcome", serde_json::json!({}))
                .expect("package view resolves"),
            "pkg says hi"
        );
        // Dots map to directories under the package's view root.
        std::fs::create_dir_all(
            root.join("packages")
                .join("billing")
                .join("views")
                .join("admin"),
        )
        .expect("nested view dir");
        std::fs::write(
            root.join("packages")
                .join("billing")
                .join("views")
                .join("admin")
                .join("invoice.html"),
            "admin view",
        )
        .expect("nested template");
        assert_eq!(
            engine
                .render("billing::admin.invoice", serde_json::json!({}))
                .expect("dotted package view resolves"),
            "admin view"
        );
        // Unknown namespace -> VIEW-NOT-FOUND, never a panic.
        let err = engine
            .render("ghost::welcome", serde_json::json!({}))
            .expect_err("unknown namespace fails closed");
        assert_eq!(err.code, keel_views::engine::VIEW_NOT_FOUND);

        // No index -> no resolver installed -> namespaces stay closed.
        let bare = std::env::temp_dir().join(format!("keel-pkgviews-bare-{}", uuid::Uuid::now_v7()));
        let engine = keel_views::ViewsEngine::new(keel_views::ViewsConfig {
            paths: vec![],
            cache_dir: None,
            env: "test".into(),
        });
        crate::register_package_view_namespaces(&bare, &engine);
        let err = engine
            .render("billing::welcome", serde_json::json!({}))
            .expect_err("no index means no namespaces");
        assert_eq!(err.code, keel_views::engine::VIEW_NOT_FOUND);

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn notification_html_mail_renders_through_the_views_engine() {
        use keel_notifications::{
            Channel, Mailable, MailPort, NotificationContext, NotificationDef, NotifyHub,
            ResolvedMailAttachment,
        };

        let root = std::env::temp_dir().join(format!("keel-notif-views-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(root.join("notifications/appointment")).expect("view dirs");
        std::fs::write(
            root.join("notifications/appointment/booked.html"),
            "<p>Booked for {{ patient }} ({{ slots }} slot(s))</p>",
        )
        .expect("fixture template");
        let engine = std::sync::Arc::new(keel_views::ViewsEngine::new(keel_views::ViewsConfig {
            paths: vec![root.clone()],
            cache_dir: None,
            env: "test".into(),
        }));
        let mut hub = NotifyHub::empty().with_view_renderer(std::sync::Arc::new(
            crate::host::ViewsNotificationRenderer(engine),
        ));
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        hub.mail = Some(Box::new(LogMail {
            sent: log.clone(),
        }));
        let ctx = NotificationContext {
            principal_id: keel_foundation::PrincipalId::new(),
            tenant_id: None,
            locale: None,
        };

        struct Booked {
            patient: &'static str,
            slots: u32,
        }
        impl NotificationDef for Booked {
            fn type_key(&self) -> &str {
                "appointment.booked"
            }
            fn to_mail(&self) -> Mailable {
                Mailable {
                    to: vec![],
                    cc: vec![],
                    bcc: vec![],
                    from: None,
                    reply_to: None,
                    subject: format!("Booked for {}", self.patient),
                    text: Some("text body".into()),
                    html: None,
                    headers: vec![],
                    attachments: vec![],
                }
            }
            fn to_sms(&self) -> String {
                unreachable!("mail path only")
            }
            fn to_push(&self) -> serde_json::Value {
                unreachable!("mail path only")
            }
            fn to_database(&self) -> serde_json::Value {
                serde_json::json!({ "patient": self.patient, "slots": self.slots })
            }
        }

        hub.dispatch_notification(
            &Booked {
                patient: "Ada",
                slots: 2,
            },
            Channel::Email,
            Some("ada@example.com"),
            &ctx,
        )
        .await
        .expect("dispatches through the real engine");
        {
            let sent = log.lock().unwrap();
            assert_eq!(sent.len(), 1);
            assert_eq!(
                sent[0].html.as_deref(),
                Some("<p>Booked for Ada (2 slot(s))</p>"),
                "fixture template renders the database payload as context"
            );
            assert_eq!(sent[0].to, vec!["ada@example.com".to_string()]);
        }

        // A declared template with no file on disk refuses the dispatch —
        // no silent text-only fallback once the view port is wired.
        struct Ghost;
        impl NotificationDef for Ghost {
            fn type_key(&self) -> &str {
                "ghost.notice"
            }
            fn to_mail(&self) -> Mailable {
                Mailable {
                    to: vec![],
                    cc: vec![],
                    bcc: vec![],
                    from: None,
                    reply_to: None,
                    subject: "ghost".into(),
                    text: Some("text body".into()),
                    html: None,
                    headers: vec![],
                    attachments: vec![],
                }
            }
            fn to_sms(&self) -> String {
                unreachable!("mail path only")
            }
            fn to_push(&self) -> serde_json::Value {
                unreachable!("mail path only")
            }
            fn to_database(&self) -> serde_json::Value {
                serde_json::json!({})
            }
        }
        let err = hub
            .dispatch_notification(&Ghost, Channel::Email, Some("ops@example.com"), &ctx)
            .await
            .expect_err("missing template refuses");
        assert_eq!(err.code, keel_foundation::codes::INTERNAL);
        assert_eq!(log.lock().unwrap().len(), 1, "nothing was sent");

        struct LogMail {
            sent: std::sync::Arc<std::sync::Mutex<Vec<Mailable>>>,
        }
        impl MailPort for LogMail {
            fn send(
                &self,
                _msg: &keel_notifications::NotifyMessage,
                _subject: &str,
            ) -> Result<(), keel_foundation::AppError> {
                Ok(())
            }
            fn send_mailable(
                &self,
                mail: &Mailable,
                _attachments: &[ResolvedMailAttachment],
            ) -> Result<(), keel_foundation::AppError> {
                mail.validate()?;
                self.sent.lock().unwrap().push(mail.clone());
                Ok(())
            }
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// Wave 6 P6: the REAL localization chain end to end — the shipped
    /// `platform/lang` catalogs load through the boot loader, the real
    /// `i18n::resolve` picks `fr` from `Accept-Language` (the edge's
    /// resolution, unchanged), and the REAL producer def
    /// (`keel-mod-appointments::AppointmentBooked`) renders subject and
    /// text through the hub catalog with `{patient}` interpolated from
    /// the payload: FR bytes out of an FR request locale.
    #[tokio::test]
    async fn producer_mail_localizes_through_the_shipped_catalog_and_resolver() {
        use keel_notifications::{
            Channel, Mailable, MailPort, NotificationContext, NotifyHub, ResolvedMailAttachment,
        };

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let catalog = std::sync::Arc::new(
            keel_config::load_messages(&root, &keel_config::I18nSection::default())
                .expect("shipped lang files load"),
        );
        let locale = crate::i18n::resolve(&catalog, None, Some("fr"));
        assert_eq!(locale, "fr");

        struct RecordingMail {
            sent: std::sync::Arc<std::sync::Mutex<Vec<Mailable>>>,
        }
        impl MailPort for RecordingMail {
            fn send(
                &self,
                _msg: &keel_notifications::NotifyMessage,
                _subject: &str,
            ) -> Result<(), keel_foundation::AppError> {
                Ok(())
            }
            fn send_mailable(
                &self,
                mail: &Mailable,
                _attachments: &[ResolvedMailAttachment],
            ) -> Result<(), keel_foundation::AppError> {
                mail.validate()?;
                self.sent.lock().unwrap().push(mail.clone());
                Ok(())
            }
        }

        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut hub = NotifyHub::with_catalog(catalog);
        hub.mail = Some(Box::new(RecordingMail { sent: log.clone() }));

        let def = keel_mod_appointments::AppointmentBooked {
            patient: "Ada".into(),
            slot: "2026-09-01T10:00Z".into(),
            locale: Some(locale.clone()),
        };
        hub.dispatch_notification(
            &def,
            Channel::Email,
            Some("ada@example.com"),
            &NotificationContext {
                principal_id: keel_foundation::PrincipalId::new(),
                tenant_id: None,
                locale: Some(locale),
            },
        )
        .await
        .expect("dispatches");

        let sent = log.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].subject, "Rendez-vous réservé pour Ada");
        assert_eq!(
            sent[0].text.as_deref(),
            Some("Rendez-vous réservé pour Ada"),
            "the catalog key resolves to FR text with the patient interpolated"
        );
        assert_eq!(sent[0].to, vec!["ada@example.com".to_string()]);
    }

    /// Wave 6 P6: the queued-delivery seam end to end — a `queue_via` def
    /// renders at dispatch (the producer's catalog resolves the FR key;
    /// the hub carries NO mail port, so an accidental inline path would
    /// fail loudly) and enqueues through the runtime's port over the
    /// in-memory job store; the claim surface reads the pre-rendered
    /// envelope back, and the drain hub (no catalog — I/O only) delivers
    /// the FR mail.
    #[tokio::test]
    async fn queue_via_enqueues_a_drainable_envelope_round_trip() {
        use keel_jobs::JobStore as _;
        use keel_notifications::{
            Channel, Mailable, MailPort, NotificationContext, NotificationDef, NotifyHub,
            ResolvedMailAttachment,
        };

        // Enqueue side: the port over the in-memory job store + the
        // registered `notification.send` def (what serve() wires pool-gated).
        let jobs = std::sync::Arc::new(keel_jobs::InMemoryJobStore::new());
        let mut registry = keel_jobs::JobRegistry::new();
        registry
            .register(keel_jobs::JobDef::new(
                keel_notifications::NOTIFICATION_SEND_JOB,
            ))
            .expect("def registers");
        let port = crate::host::JobQueuePort::new(jobs.clone(), std::sync::Arc::new(registry));

        struct QueuedNotice;
        impl NotificationDef for QueuedNotice {
            fn type_key(&self) -> &str {
                "notice.queued"
            }
            fn to_mail(&self) -> Mailable {
                Mailable {
                    to: vec![],
                    cc: vec![],
                    bcc: vec![],
                    from: None,
                    reply_to: None,
                    subject: "notifications.appointment_booked".into(),
                    text: Some("notifications.appointment_booked".into()),
                    html: None,
                    headers: vec![],
                    attachments: vec![],
                }
            }
            fn to_sms(&self) -> String {
                unreachable!("mail channel only")
            }
            fn to_push(&self) -> serde_json::Value {
                unreachable!("mail channel only")
            }
            fn to_database(&self) -> serde_json::Value {
                serde_json::json!({ "patient": "Ada" })
            }
            fn queue_via(&self) -> Option<&str> {
                Some(keel_notifications::NOTIFICATION_SEND_JOB)
            }
        }

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let catalog = std::sync::Arc::new(
            keel_config::load_messages(&root, &keel_config::I18nSection::default())
                .expect("shipped lang files load"),
        );
        let producer_hub = NotifyHub::with_catalog(catalog).with_queue(Box::new(port));
        producer_hub
            .dispatch_notification(
                &QueuedNotice,
                Channel::Email,
                Some("ada@example.com"),
                &NotificationContext {
                    principal_id: keel_foundation::PrincipalId::new(),
                    tenant_id: None,
                    locale: Some("fr".into()),
                },
            )
            .await
            .expect("queued, never delivered inline");

        // The claim surface reads the durable job back …
        let job = jobs
            .claim("drain-worker")
            .expect("store claims")
            .expect("one job enqueued");
        assert_eq!(job.name, keel_notifications::NOTIFICATION_SEND_JOB);
        let envelope: keel_notifications::QueuedNotificationDelivery =
            serde_json::from_value(job.payload).expect("envelope payload");
        assert_eq!(
            envelope.v,
            keel_notifications::QueuedNotificationDelivery::VERSION
        );
        assert_eq!(envelope.type_key, "notice.queued");
        // … with the FR subject already baked in at enqueue.
        let pre_rendered = match &envelope.payload {
            keel_notifications::DeliveryPayload::Mail(mail) => mail.clone(),
            other => panic!("mail payload expected, got {other:?}"),
        };

        // Drain side: a fresh hub with NO catalog — only I/O.
        struct RecordingMail {
            sent: std::sync::Arc<std::sync::Mutex<Vec<Mailable>>>,
        }
        impl MailPort for RecordingMail {
            fn send(
                &self,
                _msg: &keel_notifications::NotifyMessage,
                _subject: &str,
            ) -> Result<(), keel_foundation::AppError> {
                Ok(())
            }
            fn send_mailable(
                &self,
                mail: &Mailable,
                _attachments: &[ResolvedMailAttachment],
            ) -> Result<(), keel_foundation::AppError> {
                mail.validate()?;
                self.sent.lock().unwrap().push(mail.clone());
                Ok(())
            }
        }
        assert_eq!(pre_rendered.subject, "Rendez-vous réservé pour Ada");
        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut drain_hub = NotifyHub::empty();
        drain_hub.mail = Some(Box::new(RecordingMail { sent: log.clone() }));
        drain_hub
            .deliver_queued(envelope)
            .await
            .expect("drain delivers");

        let sent = log.lock().unwrap();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].subject, "Rendez-vous réservé pour Ada");
        assert_eq!(sent[0].to, vec!["ada@example.com".to_string()]);
    }
}

#[cfg(test)]
mod wave6_disk_tests {
    use super::disk_registry;

    /// Wave 6 P2: an empty `[storage]` section yields exactly one synthetic
    /// `local` disk at the fallback root (dev/test parity with the
    /// pre-disk wiring), and objects round-trip through the port.
    #[test]
    fn synthetic_local_disk_fills_an_empty_storage_section() {
        let root = std::env::temp_dir().join(format!("keel-disks-synth-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).expect("temp root");
        let registry = disk_registry(&keel_config::StorageConfig::default(), &root).unwrap();
        assert_eq!(registry.default_name(), "local");
        assert_eq!(registry.names(), vec!["local"]);
        registry.validate_default().unwrap();
        let (name, entry) = registry.resolve(None).unwrap();
        assert_eq!(name, "local");
        assert!(matches!(
            entry.visibility,
            keel_storage::Visibility::Private
        ));
        let obj = entry.port.put("uploads", "a.txt", b"x".to_vec()).unwrap();
        assert_eq!(entry.port.get(&obj).unwrap(), b"x");
        std::fs::remove_dir_all(&root).ok();
    }

    /// Declared disks map drivers to adapters and carry visibility/url into
    /// the registry entry; malformed sections refuse boot (missing
    /// root/bucket, an unknown `storage.default`).
    #[test]
    fn declared_disks_map_to_adapters_and_bad_sections_refuse_boot() {
        let root = std::env::temp_dir().join(format!("keel-disks-decl-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).expect("temp root");
        let mut section = keel_config::StorageConfig {
            default: "cdn".into(),
            ..Default::default()
        };
        section.disks.insert(
            "assets".into(),
            keel_config::DiskConfig {
                driver: keel_config::DiskDriver::Local,
                root: Some("storage/app".into()),
                bucket: None,
                url: None,
                visibility: keel_config::DiskVisibility::Private,
            },
        );
        section.disks.insert(
            "cdn".into(),
            keel_config::DiskConfig {
                driver: keel_config::DiskDriver::S3,
                root: None,
                bucket: Some("keel".into()),
                url: Some("https://cdn.example.com".into()),
                visibility: keel_config::DiskVisibility::Public,
            },
        );
        let registry = disk_registry(&section, &root).unwrap();
        assert_eq!(registry.names(), vec!["assets", "cdn"]);
        registry.validate_default().unwrap();
        // Public issuance resolves through the `url` base (`{base}/{bucket}/{key}`).
        let obj = keel_storage::ObjectRef {
            bucket: "b".into(),
            key: "k/f.png".into(),
        };
        assert_eq!(
            registry.url_for("cdn", &obj, 30).unwrap(),
            "https://cdn.example.com/b/k/f.png"
        );

        // local without `root` refuses boot.
        let mut bad = keel_config::StorageConfig::default();
        bad.disks.insert(
            "d".into(),
            keel_config::DiskConfig {
                driver: keel_config::DiskDriver::Local,
                root: None,
                bucket: None,
                url: None,
                visibility: keel_config::DiskVisibility::Private,
            },
        );
        let err = disk_registry(&bad, &root).unwrap_err();
        assert!(err.public_message.contains("root"), "{}", err.public_message);

        // s3 without `bucket` refuses boot.
        let mut bad = keel_config::StorageConfig::default();
        bad.disks.insert(
            "d".into(),
            keel_config::DiskConfig {
                driver: keel_config::DiskDriver::S3,
                root: None,
                bucket: None,
                url: None,
                visibility: keel_config::DiskVisibility::Private,
            },
        );
        let err = disk_registry(&bad, &root).unwrap_err();
        assert!(
            err.public_message.contains("bucket"),
            "{}",
            err.public_message
        );

        // A `storage.default` naming no registered disk refuses boot.
        let mut bad = keel_config::StorageConfig {
            default: "nope".into(),
            ..Default::default()
        };
        bad.disks.insert(
            "d".into(),
            keel_config::DiskConfig {
                driver: keel_config::DiskDriver::S3,
                root: None,
                bucket: Some("b".into()),
                url: None,
                visibility: keel_config::DiskVisibility::Private,
            },
        );
        let err = disk_registry(&bad, &root).unwrap_err();
        assert!(err.public_message.contains("nope"), "{}", err.public_message);
        std::fs::remove_dir_all(&root).ok();
    }
}

#[cfg(test)]
mod wave7_signed_url_ttl_tests {
    use super::{signed_url_max_ttl_secs, SIGNED_URL_TTL_CEILING_SECS};

    /// Wave 7 P6 (19-PARITY "over-bound → boot-refusing config error"): a
    /// `security.signed_urls.max_ttl_secs` above the hard ceiling refuses
    /// boot; the default and the ceiling itself pass through to the gate.
    #[test]
    fn over_bound_signed_url_ttl_refuses_boot() {
        let mut security = keel_config::SecuritySection::default();
        assert_eq!(
            signed_url_max_ttl_secs(&security).expect("the default bound is in range"),
            604_800,
            "the default is the documented 7 days"
        );
        security.signed_urls.max_ttl_secs = SIGNED_URL_TTL_CEILING_SECS;
        signed_url_max_ttl_secs(&security).expect("the ceiling itself passes");
        security.signed_urls.max_ttl_secs = SIGNED_URL_TTL_CEILING_SECS + 1;
        let err = signed_url_max_ttl_secs(&security).expect_err("over-bound refuses boot");
        assert!(
            err.public_message.contains("max_ttl_secs"),
            "{}",
            err.public_message
        );
    }
}
