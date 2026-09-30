//! keel-api: HTTP edge host (11-SPEC SS20, S7 edge wiring). Binary entry is
//! `main.rs`; this lib exposes the routing modules and boot function so the
//! integration suite can drive the exact production wiring.

pub mod auth_routes;
pub mod call;
pub mod dashboard;
pub mod edge;
pub mod graph_routes;
pub mod host;
pub mod i18n;
pub mod notification_routes;
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
        Some(Arc::new(keel_http::SignedGate::new(
            keel_http::derive_signed_key(&cfg.app().key),
            tokens.clone(),
        )))
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
        .merge(notification_routes::router(state.clone()));
    let router = if realtime_enabled {
        let hub = host
            .registry()
            .get::<keel_realtime::BroadcastHub>()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
        router.merge(keel_realtime::realtime_router((*hub).clone()))
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
    // Opt-in CSRF double-submit guard (L26): attached AFTER Admission so the
    // request-id/trace envelope context exists when rejections render.
    let csrf_enabled = std::env::var("KEEL_CSRF")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let app = if csrf_enabled {
        keel_http::CsrfGuard::default().apply(app)
    } else {
        app
    };
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
    let mut bus = keel_command::CommandBus::new(engine);
    bus.register(Arc::new(host::EchoHandler::new()))
        .expect("demo handler registers");
    let bundle: Vec<Arc<dyn OperationHandler>> = vec![
        Arc::new(keel_mod_appointments::BookHandler::new(appt_store.clone())),
        Arc::new(keel_mod_appointments::CancelHandler::new(
            appt_store.clone(),
        )),
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
    let graph = keel_registry::PlatformGraph::compile(vec![manifest]).expect("graph compiles");
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

    // Wave 6 P3: harness catalog from `<storage_root>/lang` (absent = the
    // empty default catalog), so integration tests can drop fixture locale
    // files exactly like fixture views.
    let messages = Arc::new(
        keel_config::load_messages(storage_root, &keel_config::I18nSection::default())
            .expect("harness message catalog loads"),
    );

    // Wave 6 P5: fresh in-memory inbox store per harness (the D-52 backend
    // only — tests seed rows directly or through the hub dispatch).
    let notification_store: Arc<dyn keel_notifications::NotificationStore> =
        Arc::new(keel_notifications::InMemoryNotificationStore::new());

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
