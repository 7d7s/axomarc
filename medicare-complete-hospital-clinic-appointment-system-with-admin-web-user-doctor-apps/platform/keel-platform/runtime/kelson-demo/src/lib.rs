//! kelson-demo: HTTP edge host (11-SPEC SS20, S7 edge wiring). Binary entry is
//! `main.rs`; this lib exposes the routing modules and boot function so the
//! integration suite can drive the exact production wiring.

pub mod auth_routes;
pub mod broadcast_routes;
pub mod call;
pub mod dashboard;
pub mod edge;
pub mod graph_routes;
pub mod host;
pub mod hot_reload;
pub mod i18n;
pub mod notification_routes;
pub mod batch_routes;
pub mod routes;
pub mod runtime_status;
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
use kelson_config::{Environment, PlatformConfig};
use kelson_foundation::{PrincipalId, TenantId};
use kelson_lifecycle::ApplicationHost;

/// Wave 9 B-30: load every WASM plugin indexed by the root `packages.toml`
/// `[[wasm]]` entries. Runs BEFORE `host.boot()` so compile, link, ABI,
/// capability and manifest-drift failures refuse the boot outright
/// (fail-closed) instead of surfacing at first dispatch. `Ok(None)` = no
/// plugins indexed — the default path loads nothing and changes nothing.
pub fn load_wasm_plugins(
    root: &std::path::Path,
) -> Result<Option<kelson_wasm::WasmHandlerBundle>, String> {
    let index = kelson_registry::PackageIndex::load(root)?;
    if index.wasm.is_empty() {
        return Ok(None);
    }
    // Wave M1: the module pins are enforced BEFORE a single guest is
    // instantiated. Without this the pin file was write-only — `kelson
    // promote` wrote it and `kelson rollback` restored it, but nothing that
    // actually executed read it, so a pin naming 0.3.1 while 0.4.0 sat
    // installed was silently ignored and the unpinned bytes ran. Refusing
    // here (fail-closed, same shape as the other discovery gates) is what
    // makes a pin mean something.
    let pinned = index.verify_pinned_boot(root).map_err(|e| {
        format!("module pins: {e} — refusing to boot rather than running unpinned WASM modules")
    })?;
    if pinned > 0 {
        tracing::info!(pins = pinned, "module pins verified against the package index");
    }
    let host = kelson_wasm::WasmHost::new().map_err(|e| e.to_string())?;
    let mut handlers: Vec<Arc<dyn kelson_execution::OperationHandler>> = Vec::new();
    for entry in &index.wasm {
        // Shared discovery rules (sidecar parses, name/version agree,
        // binary exists, capabilities present with mappable op profiles) —
        // the exact call `kelson graph`/`kelson check` and the console make, so
        // what boots here was what `kelson check` reviewed.
        let sidecar = index.wasm_manifest(root, entry)?;
        let binary = kelson_registry::PackageIndex::resolve_path(root, &entry.path);
        let bytes = std::fs::read(&binary)
            .map_err(|e| format!("wasm plugin {}: {}: {e}", entry.name, binary.display()))?;
        // ADR 0019 §6 — prepare and commit are deliberately separate acts.
        // `prepare` compiles, links, checks the ABI and drift-checks with NO
        // dispatch registration, so a plugin failing any of those refuses here
        // while whatever generation is already serving stays untouched. The
        // host then commits, and only the host: the guest never registers
        // itself, the same asymmetry as the sidecar-verified capability set.
        // A failure between the two leaves the plugin `prepared`, which
        // `WasmPlugin::call` refuses to dispatch.
        let plugin = Arc::new(
            host.prepare(&bytes, &sidecar)
                .map_err(|e| format!("wasm plugin {}: {e}", entry.name))?,
        );
        handlers.extend(
            kelson_wasm::handlers(Arc::clone(&plugin))
                .map_err(|e| format!("wasm plugin {}: {e}", entry.name))?,
        );
        plugin
            .commit()
            .map_err(|e| format!("wasm plugin {}: {e}", entry.name))?;
        tracing::info!(plugin = %entry.name, handlers = handlers.len(), "wasm plugin loaded");
    }
    Ok(Some(kelson_wasm::WasmHandlerBundle(handlers)))
}

pub async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let cfg_path = std::env::var("KEEL_CONFIG").unwrap_or_else(|_| "config/dev.toml".into());
    // B-29: interactive console mode (`kelson shell` spawns `kelson-demo --shell`).
    let shell_mode = std::env::args().any(|arg| arg == "--shell");
    // Wave 3 Phase 6: maintenance sweep (`kelson model:prune` spawns
    // `kelson-demo --prune [--chunk N] [--tenant T]`) — boots the graph, runs
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
    // Wave 5 Phase 7: one-shot console-route dispatch (`kelson call <name> [json]`
    // spawns `kelson-demo --call <name> [--input <json>] [--principal <uuid>]
    // [--tenant <uuid>]`) — boots the graph, dispatches through the CommandBus
    // under the operator principal, prints the JSON result and exits.
    let call_mode = args.iter().any(|arg| arg == "--call");
    let call_name = flag_value("--call").unwrap_or_default();
    let call_input = flag_value("--input");
    let call_principal = flag_value("--principal");
    let call_tenant = flag_value("--tenant");
    // Phase 3 (target-arch §4.1): keep the pre-freeze config — the
    // ConfigRuntime subscribed below boots from this exact layered load as
    // version 1 (the frozen view stays the compatibility handle).
    let platform_cfg = PlatformConfig::load(Some(std::path::Path::new(&cfg_path)))?;
    let cfg = platform_cfg.clone().freeze();
    let env = cfg.app().env.clone();
    let listen = cfg.app().listen.clone();

    // Sentinel (S5): the telemetry runtime replaces S1 `init_logging` as
    // the single logging init point. Layering: compiled defaults <-
    // `[telemetry]` section <- env overlay; the S1 env conventions stay
    // honored (RUST_LOG, KEEL_LOG_FILE, environment-driven console format).
    let mut telemetry_config =
        kelson_observability::TelemetryConfig::from_section(cfg.telemetry());
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
            kelson_observability::ExporterConfig {
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
    // impossible by type — see kelson_config::FrozenConfig).

    // Feature flags at boot (S3 item 32): decide which opt-in platform
    // services start. The boot input uses a synthetic internal actor.
    let root = std::env::current_dir()?;
    // Config cache (Wave 4 Phase 5): a present artifact must pin the exact
    // effective config boot just derived (file + environment overlay),
    // else boot is refused — drift-refuse semantics identical to the route
    // cache below.
    kelson_registry::config_cache_check(
        &root,
        &kelson_registry::config_hash(&cfg.cache_json()),
    )
    .map_err(Box::<dyn std::error::Error>::from)?;
    // Resolve telemetry storage paths against the workspace root, then
    // construct + start the runtime BEFORE the lifecycle host boots so
    // boot-time events flow through it (S1 parity); the provider boot()
    // below is idempotent.
    telemetry_config.rebase(&root);
    let telemetry_runtime = kelson_observability::ObservabilityRuntime::new(
        telemetry_config,
        kelson_observability::ServiceIdentity::new("kelson-demo", env.as_str()),
        &[],
    )
    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    telemetry_runtime
        .start()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    let flags_registry = Arc::new(kelson_feature_flags::platform_registry());
    let flags_store: Arc<dyn kelson_feature_flags::FlagStore> = Arc::new(
        kelson_feature_flags::FileFlagStore::new(kelson_feature_flags::default_flags_path(&root)),
    );
    let boot_caps = std::collections::BTreeSet::new();
    let boot_input = kelson_feature_flags::FlagInput {
        environment: env.as_str(),
        tenant: None,
        region: None,
        capabilities: &boot_caps,
        actor: PrincipalId::from_uuid(
            uuid::Uuid::parse_str(host::BOOT_ACTOR_UUID).expect("static uuid"),
        ),
        internal_actor: true,
    };
    let flag_service = Arc::new(kelson_feature_flags::FlagService::new(
        flags_registry.clone(),
        flags_store.clone(),
    ));
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
            Some(std::sync::Arc::new(kelson_http::JwtHs256::new(secret)))
        }
    } else {
        None
    };

    // Wave 9 B-31: `[database].driver` selects the probe + fabric at boot.
    // `postgres` (the default) keeps today's path byte-identical; `mysql`
    // swaps in the kelson-mysql pool, runner and seed. Degraded boot (D-52)
    // is unchanged: unreachable + not required = in-memory stores.
    let pool: Option<kelson_mysql::DbPool> = match cfg.database() {
        Some(db) => {
            let probe = match cfg.database_driver() {
                kelson_config::DatabaseDriver::Postgres => tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    kelson_postgres::create_pool(&db.url, db.max_connections),
                )
                .await
                .map(|r| r.map(kelson_mysql::DbPool::Postgres)),
                kelson_config::DatabaseDriver::MySql => tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    kelson_mysql::create_pool(&db.url, db.max_connections),
                )
                .await
                .map(|r| r.map(kelson_mysql::DbPool::MySql)),
            };
            match probe {
                Ok(Ok(pool)) => {
                    if db.run_migrations {
                        match &pool {
                            kelson_mysql::DbPool::Postgres(p) => {
                                kelson_postgres::run_migrations(p, &root).await?;
                            }
                            kelson_mysql::DbPool::MySql(p) => {
                                kelson_mysql::run_migrations(p, &root).await?;
                            }
                        }
                    }
                    // Dev bootstrap: register/login assign the fixed dev tenant, so a fresh
                    // database needs its tenants row to exist (FK target for domain writes).
                    // Idempotent guarded insert keeps this re-runnable; failure only warns.
                    if matches!(env, Environment::Dev | Environment::Test) {
                        if let Ok(dev_tenant) = uuid::Uuid::parse_str(host::DEV_TENANT_UUID) {
                            let seeded = match &pool {
                                kelson_mysql::DbPool::Postgres(p) => {
                                    kelson_postgres::seed_tenant(
                                        p,
                                        dev_tenant,
                                        "dev",
                                        "Development Tenant",
                                    )
                                    .await
                                }
                                kelson_mysql::DbPool::MySql(p) => {
                                    kelson_mysql::seed_tenant(
                                        p,
                                        dev_tenant,
                                        "dev",
                                        "Development Tenant",
                                    )
                                    .await
                                }
                            };
                            if let Err(e) = seeded {
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
    // pointer to `kelson route:cache`; absent artifact = live compile.
    let route_plans = graph_routes::route_cache_check(&root)
        .map_err(|e| format!("route cache rejected: {e}"))?;
    // Wave 1 middleware model: middleware.toml at the discovery root (absent
    // file = builtin catalog); malformed files refuse boot (fail closed).
    let middleware_catalog = kelson_http::MiddlewareCatalog::load(&root)
        .map_err(|e| format!("middleware catalog rejected: {e}"))?;
    // Wave 6 P3: the message catalog (`lang/<locale>.toml` at the discovery
    // root, `[i18n] paths`) — absent roots are empty catalogs, malformed
    // files or invalid locales refuse boot (the middleware.toml posture).
    let messages = Arc::new(
        kelson_config::load_messages(&root, cfg.i18n())
            .map_err(|e| format!("message catalog rejected: {}", e.public_message))?,
    );

    let health = SystemHealth::new(
        pool.is_none(),
        if pool.is_none() {
            // B-31: driver-aware message; the postgres spelling (and the
            // default path) stays byte-identical to today.
            match cfg.database_driver() {
                kelson_config::DatabaseDriver::MySql => "mysql unreachable or not configured",
                kelson_config::DatabaseDriver::Postgres => "postgres unreachable or not configured",
            }
        } else {
            "ok"
        },
    );
    let secret = session_secret();
    let session_store: Arc<dyn kelson_session::SessionStore> = match &pool {
        Some(kelson_mysql::DbPool::Postgres(pool)) => {
            Arc::new(kelson_postgres::PgSessionStore::new(pool.clone()))
        }
        Some(kelson_mysql::DbPool::MySql(pool)) => {
            Arc::new(kelson_mysql::MySqlSessionStore::new(pool.clone()))
        }
        None => kelson_session::InMemorySessionStore::shared(),
    };
    // Wave 6 P5: the database-channel inbox store — durable driver when
    // reachable, in-memory otherwise (D-52, the session-store posture).
    // Built before the providers so the notify hub and the HTTP feed share
    // ONE instance.
    let notification_store: Arc<dyn kelson_notifications::NotificationStore> = match &pool {
        Some(kelson_mysql::DbPool::Postgres(pool)) => {
            Arc::new(kelson_postgres::PgNotificationStore::new(pool.clone()))
        }
        Some(kelson_mysql::DbPool::MySql(pool)) => {
            Arc::new(kelson_mysql::MySqlNotificationStore::new(pool.clone()))
        }
        None => Arc::new(kelson_notifications::InMemoryNotificationStore::new()),
    };
    // Wave 6 P6: the pool-gated enqueue seam — the driver-selected job store
    // + the registered `notification.send` def, so `queue_via` defs build a
    // durable job at dispatch (the worker drains it). No pool = no port =
    // `queue_via` fail-closed (`not_implemented`), the D-52 posture.
    // Wave 6 P8: the SAME store instance doubles as the `/metrics` queue
    // census view (admin trait) — depth/age/finished, snapshot per scrape.
    // Wave 6 P9: and as the batch-operations store next to the durable
    // batch ledger (B-32) the batch endpoints read.
    let (notification_queue, queue_stats, batch_ledger) = match &pool {
        Some(driver) => {
            // The def registration is driver-independent; each arm builds the
            // ONE concrete store so it can be coerced to both `JobStore` and
            // `JobStoreAdmin` from the same instance.
            let registry = || {
                let mut registry = kelson_jobs::JobRegistry::new();
                registry
                    .register(kelson_jobs::JobDef::new(
                        kelson_notifications::NOTIFICATION_SEND_JOB,
                    ))
                    .expect("notification.send def registers");
                Arc::new(registry)
            };
            match driver {
                kelson_mysql::DbPool::Postgres(pool) => {
                    let store = Arc::new(kelson_postgres::PgJobStore::new(pool.clone()));
                    (
                        Some((store.clone() as Arc<dyn kelson_jobs::JobStore>, registry())),
                        Some(store as Arc<dyn kelson_jobs::JobStoreAdmin>),
                        Some(
                            Arc::new(kelson_postgres::PgBatchLedger::new(pool.clone()))
                                as Arc<dyn kelson_jobs::BatchLedger>,
                        ),
                    )
                }
                kelson_mysql::DbPool::MySql(pool) => {
                    let store = Arc::new(kelson_mysql::MySqlJobStore::new(pool.clone()));
                    (
                        Some((store.clone() as Arc<dyn kelson_jobs::JobStore>, registry())),
                        Some(store as Arc<dyn kelson_jobs::JobStoreAdmin>),
                        Some(
                            Arc::new(kelson_mysql::MySqlBatchLedger::new(pool.clone()))
                                as Arc<dyn kelson_jobs::BatchLedger>,
                        ),
                    )
                }
            }
        }
        None => (None, None, None),
    };

    let mut host = ApplicationHost::new();
    // Wave 2 error rendering: one registry per boot, published to every
    // module provider (Foundation phase) and read by the dispatch edge.
    let error_renders = Arc::new(kelson_http::ErrorRenderRegistry::new());
    // Wave 8 ADR 0007: content-negotiation refusals are protocol-fixed at
    // HTTP 406 (the Validation category default would be 422).
    error_renders.set_status(
        kelson_foundation::codes::NOT_ACCEPTABLE,
        axum::http::StatusCode::NOT_ACCEPTABLE,
    );
    // Wave 9 B-30: fuel exhaustion is a resource failure that behaves like
    // a timeout — pinned to 503 SERVICE UNAVAILABLE (the ResourceExhausted
    // category default would be 429 rate-limit, which misattributes a
    // plugin bug to the caller).
    error_renders.set_status(
        kelson_foundation::codes::WASM_FUEL,
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
    );
    // Wave M2: saturation and wall-clock expiry are the same kind of
    // failure — a sandbox budget of our own plugin ran out — so they take
    // the same status. Without these pins both would serve 429, telling the
    // client it was rate limited when the plugin's slot or clock was the
    // thing that ran out.
    error_renders.set_status(
        kelson_foundation::codes::WASM_SATURATED,
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
    );
    error_renders.set_status(
        kelson_foundation::codes::WASM_DEADLINE,
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
    );
    // Wave M2: lifecycle refusal (draining/drained plugin) is a service
    // availability signal — the plugin exists but is retiring. Same 503 posture
    // as fuel/deadline/saturation: a sandbox budget of our own plugin ran out.
    error_renders.set_status(
        kelson_foundation::codes::WASM_LIFECYCLE,
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
    );
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
        kelson_observability::ObservabilityProvider::from_runtime(telemetry_runtime.clone()),
    ));
    // S10 B-12: optional Redis cache. REDIS_URL present + reachable -> redis
    // backs the cache port; otherwise the port stays in-memory (D-52 parity).
    let redis_cache = match std::env::var("REDIS_URL") {
        Ok(url) => match kelson_cache_redis::RedisCache::connect(&url) {
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
    let metrics = Arc::new(kelson_metrics::MetricsRegistry::new(4096));
    let booted_at = std::time::Instant::now();

    // Wave 2 views: engine built from the `[views]` section. Roots and the
    // compiled cache resolve against the workspace root, so the same templates
    // are served regardless of the cwd the host was started from. The
    // `@asset("...")` URL helper is registered here (once per engine) so
    // templates see production behaviour in every harness too. Built BEFORE
    // the providers (Wave 6 P5): the platform phase wires the engine into
    // the notify hub for notification HTML mail bodies.
    let assets = load_asset_manifest(&root.join("public"));
    let mut views_cfg = views_config(cfg.views(), &root, env.as_str());
    // Wave 8 ADR 0009: the platform-shipped view root (dashboard/ops.html
    // + partials) rides LAST, so an app's own `dashboard/ops.html` in any
    // `[views] paths` entry wins. Appended here rather than in
    // `views_config`, whose output the views tests pin exactly.
    views_cfg.paths.push(crate::dashboard::builtin_views_root());
    let views_engine = kelson_views::ViewsEngine::new(views_cfg);
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
    #[cfg(feature = "demo")]
    host.add_provider(Box::new(kelson_mod_appointments::ModuleProvider::new()));
    // Wave 8 P5 (B-21): the OAuth2 module registers its own handler bundle
    // (distinct newtype slot) BEFORE the bus assembly below.
    #[cfg(feature = "demo")]
    host.add_provider(Box::new(kelson_mod_oauth2::ModuleProvider::new()));
    // Wave 9 B-30: WASM plugins load BEFORE boot (fail-closed: compile,
    // link, ABI, capability and manifest-drift failures refuse the boot
    // outright). The provider then only publishes the bundle for the bus
    // assembly below — dependency edges keep the assembly strictly after it.
    let wasm_bundle = load_wasm_plugins(&root)?;
    host.add_provider(Box::new(host::WasmModulesProvider {
        bundle: wasm_bundle,
    }));
    host.add_provider(Box::new(DemoModuleProvider {
        dev_snapshot: matches!(env, Environment::Dev | Environment::Test),
    }));
    host.boot()
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    *health.services.lock().unwrap() = host.registry().service_names();
    tracing::info!(services = ?host.registry().service_names(), "kelson-demo booted");

    // Platform graph compile is a BOOT GATE (parity with `kelson check`): a
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
        .get::<kelson_command::CommandBus>()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    let tokens = host
        .registry()
        .get::<kelson_tokens::TokenService>()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    let policy = host
        .registry()
        .get::<kelson_policy::PolicyEngine>()
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
            .get::<kelson_session::SessionManager>()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
        Some(Arc::new(
            kelson_http::SessionMiddleware::new(manager, secret.clone())
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
        .get::<kelson_models::RouteBindingsRef>()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?
        .0
        .clone();
    // Wave 4 Phase 4: model.probes — row probes for the `unique`/`exists`
    // validation rules (modules register one per model during boot).
    let probes = host
        .registry()
        .get::<kelson_models::ProbeRegistryRef>()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?
        .0
        .clone();
    let mut gate_violations = kelson_registry::middleware_violations(&graph, &middleware_catalog);
    gate_violations.extend(kelson_registry::binding_resolver_violations(
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
    let static_files = Some(Arc::new(kelson_http::StaticFiles::new(
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
            kelson_http::SignedGate::new(
                kelson_http::derive_signed_key(&cfg.app().key),
                tokens.clone(),
            )
            .with_max_ttl(signed_ttl_bound),
        ))
    };

    // Wave 2 subdomain tenancy (19-PARITY route host matching): the host
    // gate resolves a subdomain's tenant here and feeds it into tenant
    // resolution. The registry is the in-memory control-plane port until
    // the kelson-postgres `tenants` adapter lands; Dev/Test get the seeded
    // dev tenant the database bootstrap writes, so `subdomain = "dev"`
    // resolves out of the box. Unknown slugs fall back to the default
    // tenant (the host match itself still applies).
    let tenant_store = Arc::new(kelson_tenancy::InMemoryTenantStore::new());
    if matches!(env, Environment::Dev | Environment::Test) {
        if let Ok(dev_id) = uuid::Uuid::parse_str(host::DEV_TENANT_UUID) {
            use kelson_tenancy::TenantStore as _;
            let seeded = tenant_store.upsert(kelson_tenancy::TenantRecord {
                id: TenantId::from_uuid(dev_id),
                slug: "dev".into(),
                name: "Development Tenant".into(),
                plan: "standard".into(),
                status: kelson_tenancy::TenantStatus::Active,
                region: "in".into(),
                capabilities: std::collections::BTreeSet::new(),
                created_at: time::OffsetDateTime::now_utc(),
                revision: kelson_foundation::TenantRevision(1),
            });
            if let Err(error) = seeded {
                tracing::warn!(%error, "dev tenant not registered for subdomain resolution");
            }
        }
    }
    let tenants = Some(Arc::new(kelson_tenancy::TenantService::new(tenant_store)));

    // Wave 7 P5: refuse boot on out-of-range `[identity]` factors — a typo
    // must not surface halfway through the first register or login.
    password_params_from(cfg.identity())
        .to_argon2()
        .map_err(|error| error.public_message)?;

    let state = routes::AppState {
        bus: Arc::new(routes::DispatchBus::new(bus)),
        health: health.clone(), modules: host.modules_shared(),
        env,
        // Phase 3 (§4.3): shared cell — the FlagsReloader subscriber swaps
        // the ambient set in place; readers take a cheap read lock.
        flags: Arc::new(std::sync::RwLock::new(enabled_flags)),
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
        storage, disks,
        notifications: notification_store,
        mail: crate::mail::port_from(cfg.mail()).map_err(|error| error.public_message)?,
        uploads: crate::upload_routes::UploadPolicy::from_config(cfg.files()),
        // Row 20: identity + generation + digest for `/__runtime/status`.
        runtime: crate::runtime_status::boot_status(&root, &platform_cfg),
        channels: broadcast_routes::resolve_channels(&graph),
        presence: Arc::new(kelson_realtime::PresenceHub::new()),
        queue: queue_stats,
        batches: batch_ledger,
        // Wave 8 ADR 0009: schedules register programmatically (the worker
        // process owns the registry — docs/13), so the API holds none; the
        // traces handle points at the SAME file plane the engine saves to
        // and `kelson inspect` reads (`storage/framework/traces`).
        schedule: None,
        traces: Some(Arc::new(kelson_execution::FileTraceStore::standard(&root))),
        route_inventory,
        middleware_catalog,
        route_bindings,
        probes,
        throttle_buckets: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        // Phase 3 (§4.2): hot-swappable plans cell — written by the
        // RouteReloader subscriber, read once per bundle build.
        route_plans: Arc::new(std::sync::RwLock::new(route_plans.map(Arc::new))),
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

    // B-29: `kelson shell` — interactive console over the booted graph. Same
    // production boot (pool, host, flag snapshot, graph gate); the REPL runs
    // under the principal you log in as, and no listener is opened.
    if shell_mode {
        let result = shell::run(state, &graph).await;
        telemetry_runtime.shutdown(std::time::Duration::from_secs(5));
        return result;
    }

    // Wave 3 Phase 6: `kelson model:prune` — sweep every registered prunable
    // model through the store in `--chunk`-sized pages, print per-model
    // counts, exit (same production boot as `--shell`; no listener opens).
    // An empty registry reports "none" — the `schedule:list` posture.
    // `--tenant <uuid>` scopes the sweep; without it tenant-scoped models
    // match no rows (documented: sweep the tenants individually).
    if prune_mode {
        let registry = host
            .registry()
            .get::<kelson_models::PruneRegistryRef>()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?
            .0.clone();
        let store = host
            .registry()
            .get::<kelson_models::ModelStoreRef>()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
        let results = registry.sweep(&*store.0, prune_tenant, prune_chunk)?;
        if results.is_empty() {
            println!("kelson model:prune: none (no prunable models registered)");
        } else {
            for (model, count) in &results {
                println!("kelson model:prune: {model} purged {count} row(s)");
            }
        }
        telemetry_runtime.shutdown(std::time::Duration::from_secs(5));
        return Ok(());
    }

    // Wave 5 Phase 7: `kelson call` — one-shot dispatch of a routes/console.toml
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
            eprintln!("kelson call: {message}");
            std::process::exit(1);
        }
        return Ok(());
    }

    // Edge surface: platform routes + auth + uploads. The GRAPH segment
    // (module routes + the `public/` static fallback) moves into a
    // swappable bundle behind the router's fallback: Phase 3 hot reload
    // (target-arch §4.2) rebuilds the bundle from a fresh route-cache
    // artifact and swaps the `Arc`, so a route/config change lands without
    // a restart while in-flight requests keep the bundle they loaded. The
    // merged platform/auth/realtime sets stay path-disjoint from graph
    // routes (axum's merge panic guaranteed that in the previous flat
    // layout), and the fallback still runs only after every static route
    // misses — precedence and the not-found path are unchanged.
    let graph = Arc::new(graph);
    let bundle_cell = hot_reload::new_bundle(hot_reload::build_graph_bundle(&state, &graph));
    let router = routes::router(state.clone())
        .merge(auth_routes::router(state.clone()))
        .merge(upload_routes::router(state.clone()))
        .merge(telemetry_routes::router(state.clone()))
        .merge(routes::module_routes::router(state.clone()))
        .merge(notification_routes::router(state.clone()))
        .merge(broadcast_routes::router(state.clone()))
        .merge(batch_routes::router(state.clone()));
    let router = if realtime_enabled {
        let hub = host
            .registry()
            .get::<kelson_realtime::BroadcastHub>()
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
        // Wave 6 P7: the mounted transports are AUTHORIZED — every
        // connect runs the host-side channel-token authorizer (issue the
        // token at `POST /api/v1/broadcast/auth`), and `presence-*`
        // channels join/leave the roster `GET /api/v1/broadcast/members`
        // reads: one shared `state.presence` instance behind both.
        router.merge(kelson_realtime::realtime_router_authorized(
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
    // Graph routes + static assets serve THROUGH the cell: the static
    // fallback moved INSIDE the bundle (graph first, static as ITS
    // fallback — the same relative position as before), and this outer
    // fallback dispatches into the bundle's current revision.
    let router = router.fallback_service(hot_reload::SwappableGraphRoutes {
        cell: bundle_cell.clone(),
    });
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
    let admission = kelson_http::Admission {
        maintenance: kelson_http::MaintenanceGuard::file(&root),
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

    // Phase 3 (target-arch §4.1/§4.2/§4.3): boot the ConfigRuntime over
    // the SAME layered load (this is config version 1), subscribe the
    // route bundle and the ambient flag set, and start the trigger
    // poller: config file mtime every 500ms PLUS the route artifact's own
    // mtime, so a `kelson route:cache` rewrite lands with no config edit.
    // Every swap is Arc-level behind a read lock — no restart, in-flight requests unaffected.
    let config_rt = kelson_config::ConfigRuntime::boot(
        kelson_config::FileConfigSource::new(&cfg_path),
        platform_cfg,
    )?;
    let route_reloader = hot_reload::RouteReloader::new(
        bundle_cell,
        state.clone(),
        graph,
        root.clone(),
    );
    config_rt.subscribe(route_reloader.clone());
    config_rt.subscribe(hot_reload::FlagsReloader::new(
        flag_service,
        state.env.as_str().to_string(),
        PrincipalId::from_uuid(uuid::Uuid::parse_str(host::BOOT_ACTOR_UUID).expect("static uuid")),
        state.clone(),
    ));
    config_rt.subscribe(crate::runtime_status::StatusReloader::new(state.runtime.clone()));
    // Row 16 wiring: the poller is the config module's runtime health check.
    // It shares the same module registry as the host (via `state`), so a
    // refused reload degrades `config` and a reload that lands recovers it.
    let module_states = state.modules.clone();
    let poller = tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
        loop {
            tick.tick().await;
            match config_rt.poll() {
                Err(error) => {
                    tracing::warn!(%error, "config reload refused: keeping the current revision");
                    note_config_poll(&module_states, ConfigPoll::Refused);
                }
                Ok(reloaded) => {
                    note_config_poll(
                        &module_states,
                        if reloaded.is_some() {
                            ConfigPoll::Reloaded
                        } else {
                            ConfigPoll::Unchanged
                        },
                    );
                    if route_reloader.artifact_changed() {
                        route_reloader.refresh();
                    }
                }
            }
        }
    });

    let listener = tokio::net::TcpListener::bind(&listen).await?;
    tracing::info!(%listen, "listening");

    let serve = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutdown signal received");
        })
        .await;
    // Stop the config/artifact poller (Phase 3) before draining telemetry.
    poller.abort();
    // Drain telemetry before exit (S5 graceful shutdown: flush buffers + WAL
    // within the deadline); runs on serve errors too.
    telemetry_runtime.shutdown(std::time::Duration::from_secs(5));
    serve?;
    Ok(())
}

/// Outcome of one config poll, for the module-lifecycle reaction (Row 16).
/// `Refused` = the file changed but failed to load (the runtime keeps the
/// last-good revision); `Reloaded` = a new revision landed; `Unchanged` =
/// nothing to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigPoll {
    Unchanged,
    Reloaded,
    Refused,
}

/// The config module's runtime health check (Row 16 wiring): a refused
/// reload degrades `config` (it serves the last-good revision but cannot
/// pick up new configuration — reduced function, docs/15-EXTENSION DEGRADED);
/// a reload that lands recovers it. `Unchanged` is a no-op, a module that is
/// not tracked (a harness boot without a host) is left alone, and repeated
/// polls settle on the current state rather than erroring on an invalid
/// transition.
fn note_config_poll(
    modules: &Arc<std::sync::RwLock<kelson_lifecycle::ModuleRegistry>>,
    outcome: ConfigPoll,
) {
    const CONFIG_MODULE: &str = "config";
    let mut modules = modules.write().unwrap();
    match outcome {
        ConfigPoll::Refused => {
            if modules.state(CONFIG_MODULE) == Some(kelson_lifecycle::ModuleState::Ready) {
                match modules.degrade(CONFIG_MODULE) {
                    Ok(()) => {
                        tracing::warn!(module = CONFIG_MODULE, "config module degraded: reload refused")
                    }
                    Err(error) => {
                        tracing::debug!(%error, module = CONFIG_MODULE, "config module degrade skipped")
                    }
                }
            }
        }
        ConfigPoll::Reloaded => {
            if modules.state(CONFIG_MODULE) == Some(kelson_lifecycle::ModuleState::Degraded) {
                match modules.recover(CONFIG_MODULE) {
                    Ok(()) => {
                        tracing::info!(module = CONFIG_MODULE, "config module recovered: reload landed")
                    }
                    Err(error) => {
                        tracing::debug!(%error, module = CONFIG_MODULE, "config module recover skipped")
                    }
                }
            }
        }
        ConfigPoll::Unchanged => {}
    }
}

/// Wave 2 views engine config from the `[views]` section: relative template
/// roots and the cache dir resolve against the workspace root, an empty path
/// list falls back to the Laravel default (`resources/views`), and the engine
/// environment follows `app.env` unless `[views] env` pins it.
fn views_config(
    section: &kelson_config::ViewsSection,
    root: &std::path::Path,
    env: &str,
) -> kelson_views::ViewsConfig {
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
    kelson_views::ViewsConfig {
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
fn load_asset_manifest(public: &std::path::Path) -> Option<Arc<kelson_http::AssetManifest>> {
    for relative in ["build/manifest.json", "build/.vite/manifest.json"] {
        let path = public.join(relative);
        if !path.is_file() {
            continue;
        }
        return match kelson_http::AssetManifest::from_path(&path) {
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
    engine: &kelson_views::ViewsEngine,
    assets: Option<Arc<kelson_http::AssetManifest>>,
) {
    engine.register_directive("asset", move |_name, arg, _ctx| {
        let raw = arg.unwrap_or_default().trim();
        let key = raw.trim_matches(&['"', '\''][..]).trim();
        let url = match assets.as_ref() {
            Some(manifest) => manifest.url(key),
            None => kelson_http::asset_url(key),
        };
        Ok(kelson_views::html_escape(&url))
    });
}

/// Wave 8 B-25 (B-24 index): every indexed package serves its `views/`
/// templates under its own namespace — `billing::welcome` resolves to
/// `<pkg>/views/welcome.html` (dots map to directories, the same rule the
/// app-side names use). No index (or an unusable one) installs no
/// resolver, so `pkg::` names keep failing closed exactly like an engine
/// without one; the boot config layer already refuses a malformed index,
/// fail-closed, before this runs.
fn register_package_view_namespaces(root: &std::path::Path, engine: &kelson_views::ViewsEngine) {
    let Ok(index) = kelson_registry::PackageIndex::load(root) else {
        return;
    };
    let roots: std::collections::HashMap<String, std::path::PathBuf> = index
        .package
        .iter()
        .map(|entry| {
            (
                entry.name.clone(),
                kelson_registry::PackageIndex::entry_dir(root, entry).join("views"),
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
) -> Result<Option<Arc<kelson_http::CookieCipher>>, kelson_foundation::AppError> {
    if !enabled {
        return Ok(None);
    }
    let material = kelson_http::derive_cookie_key(app_key);
    let sealer = kelson_tokens::AesGcmSealer::from_secret(&kelson_foundation::Secret::new(material))?;
    Ok(Some(Arc::new(kelson_http::CookieCipher::new(
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
    section: &kelson_config::SecuritySection,
) -> kelson_http::SecurityHeaders {
    kelson_http::SecurityHeaders {
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
    section: &kelson_config::IdentitySection,
) -> kelson_identity::PasswordParams {
    kelson_identity::PasswordParams {
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
    section: &kelson_config::PasswordPolicySection,
) -> kelson_validation::PasswordPolicy {
    kelson_validation::PasswordPolicy {
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
/// skip paths and the Secure cookie flag, so a deployment names receivers in config alone.
pub fn csrf_guard_from(section: &kelson_config::SecuritySection) -> kelson_http::CsrfGuard {
    kelson_http::CsrfGuard {
        bypass_paths: section.csrf.skip_paths.clone(),
        secure_cookie: section.csrf.secure_cookie,
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
    section: &kelson_config::SecuritySection,
) -> Result<u64, kelson_foundation::AppError> {
    let secs = section.signed_urls.max_ttl_secs;
    if secs > SIGNED_URL_TTL_CEILING_SECS {
        return Err(kelson_foundation::AppError::validation(format!(
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
    section: &kelson_config::StorageConfig,
    fallback_root: &std::path::Path,
) -> Result<kelson_storage::DiskRegistry, kelson_foundation::AppError> {
    use kelson_foundation::AppError;

    let mut registry = kelson_storage::DiskRegistry::new(section.default.clone());
    if section.disks.is_empty() {
        let port: Arc<dyn kelson_storage::StoragePort> =
            Arc::new(kelson_storage_local::LocalDisk::new(fallback_root)?);
        registry.register(
            "local",
            kelson_storage::DiskEntry {
                port,
                visibility: kelson_storage::Visibility::Private,
                base_url: None,
            },
        )?;
    }
    for (name, disk) in &section.disks {
        let port: Arc<dyn kelson_storage::StoragePort> = match disk.driver {
            kelson_config::DiskDriver::Local => {
                let root = disk.root.as_deref().ok_or_else(|| {
                    AppError::validation(format!("storage disk {name:?} (local) needs a `root`"))
                })?;
                Arc::new(kelson_storage_local::LocalDisk::new(root)?)
            }
            kelson_config::DiskDriver::S3 => {
                let _bucket = disk.bucket.as_deref().ok_or_else(|| {
                    AppError::validation(format!("storage disk {name:?} (s3) needs a `bucket`"))
                })?;
                Arc::new(kelson_storage_s3::S3Sandbox::new())
            }
        };
        let visibility = match disk.visibility {
            kelson_config::DiskVisibility::Public => kelson_storage::Visibility::Public,
            kelson_config::DiskVisibility::Private => kelson_storage::Visibility::Private,
        };
        registry.register(
            name,
            kelson_storage::DiskEntry {
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

// The appointment store is a demo-module type. Behind `demo` the harness
// hands back the real store; without it the harness still boots and hands
// back an ordinary in-memory model store of the same shape, so every
// demo-only registration below compiles out instead of hard-wiring the
// unpublished module crates into the default build.
#[cfg(feature = "demo")]
type HarnessAppStore = Arc<kelson_mod_appointments::AppointmentStore>;
#[cfg(not(feature = "demo"))]
type HarnessAppStore = Arc<kelson_models::InMemoryModelStore>;

/// Harness application state for the integration suite (S7 test plan) and
/// dev harnesses. Mirrors production wiring minus the lifecycle host: same
/// bus, stores and auth model; appointments default to the in-memory store.
fn build_app_state_inner(
    storage_root: &std::path::Path,
    sessions_enabled: bool,
    registration_open: bool,
    _appt_backend: std::sync::Arc<dyn kelson_models::ModelStore>,
) -> (
    routes::AppState,
    kelson_registry::PlatformGraph,
    HarnessAppStore,
) {
    use kelson_execution::{
        AuthorizationPort, Engine, InMemoryBackend, InMemoryIdempotency, OperationHandler,
    };
    use kelson_policy::{AccessSnapshot, PolicyAuthz, PolicyEngine};

    let policy = Arc::new(PolicyEngine::new());
    let authz: Arc<dyn AuthorizationPort> = Arc::new(PolicyAuthz {
        engine: policy.clone(),
    });
    let engine = Arc::new(Engine::new(
        authz,
        Arc::new(InMemoryBackend::new()),
        Arc::new(InMemoryIdempotency::new()),
    ));

    #[cfg(feature = "demo")]
    let appt_backend = _appt_backend;
    #[cfg(feature = "demo")]
    let appt_store =
        Arc::new(kelson_mod_appointments::AppointmentStore::new(appt_backend.clone()));
    // No `demo`: a live (empty) store handle so the third tuple element of
    // the signature below stays real; every demo-only registration is cfg'd
    // out, so nothing ever reads a model out of it.
    #[cfg(not(feature = "demo"))]
    let appt_store: HarnessAppStore = kelson_models::InMemoryModelStore::new();

    // Wave 6 P3/P5/P6: the harness catalog (`<storage_root>/lang`, absent
    // = the empty default catalog) and a fresh in-memory inbox store, plus
    // the P6 producer hub sharing BOTH — the very store the HTTP feed
    // reads — so a book dispatch is observable end to end over HTTP. Views
    // for notification HTML ride the production `PlatformProvider` (this
    // harness's producer dispatches the Database channel only).
    let messages = Arc::new(
        kelson_config::load_messages(storage_root, &kelson_config::I18nSection::default())
            .expect("harness message catalog loads"),
    );
    let notification_store: Arc<dyn kelson_notifications::NotificationStore> =
        Arc::new(kelson_notifications::InMemoryNotificationStore::new());
    #[cfg(feature = "demo")]
    let notify_hub = Arc::new(
        kelson_notifications::NotifyHub::with_catalog(messages.clone())
            .with_store(notification_store.clone()),
    );

    let mut bus = kelson_command::CommandBus::new(engine);
    bus.register(Arc::new(host::EchoHandler::new()))
        .expect("demo handler registers");
    #[cfg(feature = "demo")]
    let bundle: Vec<Arc<dyn OperationHandler>> = vec![
        Arc::new(
            kelson_mod_appointments::BookHandler::new(appt_store.clone())
                .with_notifier(Some(notify_hub.clone())),
        ),
        Arc::new(
            kelson_mod_appointments::CancelHandler::new(appt_store.clone())
                .with_notifier(Some(notify_hub.clone())),
        ),
        Arc::new(kelson_mod_appointments::ListHandler::new(appt_store.clone())),
        Arc::new(kelson_mod_appointments::AvailabilityHandler::new(
            appt_store.clone(),
        )),
    ];
    #[cfg(not(feature = "demo"))]
    let bundle: Vec<Arc<dyn OperationHandler>> = Vec::new();
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
        feature_revision: kelson_foundation::FeatureRevision(1),
        module_revision: kelson_foundation::ModuleRevision(1),
        computed_at: time::OffsetDateTime::now_utc(),
    });

    // Graph from the real appointments manifest (compile-path parity with
    // `kelson graph`, independent of process cwd).
    let manifest = kelson_registry::ModuleManifest::from_toml(include_str!(
        "../../../applications/appointments/module.toml"
    ))
    .expect("appointments manifest parses");
    // Wave 8 P5 (B-21): the harness graph carries BOTH modules — routes,
    // capabilities (`oauth2.*`) and the granted-capability snapshots that
    // register installs then match production discovery, which compiles
    // every `applications/*/module.toml`.
    let oauth_manifest = kelson_registry::ModuleManifest::from_toml(include_str!(
        "../../../applications/oauth2/module.toml"
    ))
    .expect("oauth2 manifest parses");
    let graph = kelson_registry::PlatformGraph::compile(vec![manifest, oauth_manifest])
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

    let tokens = Arc::new(kelson_tokens::TokenService::new(Arc::new(
        kelson_tokens::InMemoryTokenStore::new(),
    )));
    // Wave 8 P5 (B-21): the oauth2 handlers need the token service, so they
    // register onto the harness bus here (after `tokens`, before the bus is
    // moved into AppState). The client registry gets its own in-memory
    // backend — `oauth_clients` is global, never tenant keyed.
    #[cfg(feature = "demo")]
    let oauth_store = Arc::new(kelson_mod_oauth2::OauthClientStore::new(Arc::new(
        kelson_models::InMemoryModelStore::new(),
    )));
    #[cfg(feature = "demo")]
    for handler in kelson_mod_oauth2::handlers(oauth_store, tokens.clone()) {
        bus.register(handler).expect("oauth2 handlers register");
    }
    let principals: Arc<dyn kelson_identity::PrincipalStore> =
        Arc::new(kelson_identity::InMemoryPrincipalStore::new());
    let credentials: Arc<dyn kelson_identity::CredentialStore> =
        Arc::new(kelson_identity::InMemoryCredentialStore::new());
    let sessions = if sessions_enabled {
        let secret = b"integration-cookie-secret-0123456789".to_vec();
        let manager = Arc::new(kelson_session::SessionManager::new(
            kelson_session::InMemorySessionStore::shared(),
            kelson_session::SessionPolicy::default(),
            secret.clone(),
        ));
        Some(Arc::new(
            kelson_http::SessionMiddleware::new(manager, secret).with_cookie_ttl_secs(28_800),
        ))
    } else {
        None
    };
    // Wave 6 P2: the harness fills the same disk registry the storage
    // provider builds — the default config's synthetic `local` disk rooted
    // at the harness storage root — so the upload edge and registry resolve
    // identically to production.
    let disks = disk_registry(&kelson_config::StorageConfig::default(), storage_root)
        .expect("synthetic harness disk");
    let storage: Arc<dyn kelson_storage::StoragePort> = disks
        .get(disks.default_name())
        .expect("synthetic local disk registered")
        .port
        .clone();

    // Wave 1 route binding registry: fresh per harness, with the
    // appointments reference resolver registered (production boot wires it
    // through the module provider + the route.bindings service instead).
    let route_bindings = Arc::new(kelson_models::RouteBindingRegistry::new());
    #[cfg(feature = "demo")]
    route_bindings.register(Arc::new(
        kelson_mod_appointments::AppointmentBindingResolver::new(appt_store.clone()),
    ));

    // Wave 4 Phase 4: model.probes — the harness registers the appointments
    // row probe directly (production wires it through the module provider +
    // the well-known service instead), so `unique`/`exists` rules run
    // against the same store the handlers write to.
    let probes = Arc::new(kelson_models::ProbeRegistry::new());
    #[cfg(feature = "demo")]
    probes.register(Arc::new(kelson_models::StoreProbe::new(
        kelson_mod_appointments::appointment_model(),
        appt_backend,
    )));

    // Wave 2 views: the harness points the engine at <storage_root>/views so
    // tests can drop fixture templates, and keeps the compiled cache off disk
    // (each harness gets a fresh temp dir). `@asset` is registered against
    // the fixture build manifest when one exists under <storage_root>/public,
    // so the helper behaves in tests exactly as it does in production.
    let assets = load_asset_manifest(&storage_root.join("public"));
    let views_engine = kelson_views::ViewsEngine::new(kelson_views::ViewsConfig {
        // ADR 0009: the platform-shipped root rides LAST, so a fixture
        // under <storage_root>/views overrides it exactly like a production
        // app's `[views] paths` entry overrides the shipped page.
        paths: vec![
            storage_root.join("views"),
            crate::dashboard::builtin_views_root(),
        ],
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
    let static_files = Some(Arc::new(kelson_http::StaticFiles::new(
        storage_root.join("public"),
    )));
    // Wave 2 error rendering: the harness shares the registry the same way
    // production does (services may register into it), with both transport
    // gates closed so tests opt in explicitly.
    let error_renders = Arc::new(kelson_http::ErrorRenderRegistry::new());
    // Wave 8 ADR 0007: the harness pins the same protocol-fixed 406 for
    // content-negotiation refusals production serves.
    error_renders.set_status(
        kelson_foundation::codes::NOT_ACCEPTABLE,
        axum::http::StatusCode::NOT_ACCEPTABLE,
    );
    // Wave 9 B-30: the harness pins the same 503 for wasm fuel exhaustion,
    // so `error_envelope` serves the production status for KSN_WASM_FUEL.
    error_renders.set_status(
        kelson_foundation::codes::WASM_FUEL,
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
    );
    // Wave M2: the harness pins the production 503 for the two new sandbox
    // budgets too, so `error_envelope` serves the same status production
    // does for KSN_WASM_SATURATED and KSN_WASM_DEADLINE.
    error_renders.set_status(
        kelson_foundation::codes::WASM_SATURATED,
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
    );
    error_renders.set_status(
        kelson_foundation::codes::WASM_DEADLINE,
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
    );
    // Wave M2: lifecycle refusal (draining/drained plugin) is a service
    // availability signal — the plugin exists but is retiring. Same 503 posture
    // as fuel/deadline/saturation: a sandbox budget of our own plugin ran out.
    error_renders.set_status(
        kelson_foundation::codes::WASM_LIFECYCLE,
        axum::http::StatusCode::SERVICE_UNAVAILABLE,
    );

    // Wave 2 routing edge: a fixed harness application key backs the signed
    // URL gate, and a fresh (empty) tenant registry tests can seed through
    // the public store API feeds the host gate's subdomain resolution.
    let signed_urls = Some(Arc::new(kelson_http::SignedGate::new(
        b"harness-app-key-for-signed-urls".to_vec(),
        tokens.clone(),
    )));
    let tenants = Some(Arc::new(kelson_tenancy::TenantService::new(Arc::new(
        kelson_tenancy::InMemoryTenantStore::new(),
    ))));

    // Wave 6 P3/P5: the catalog and the in-memory inbox store are built
    // next to the producer hub at the top of this harness (one instance
    // shared by the dispatch and the feed).

    let state = routes::AppState {
        bus: Arc::new(routes::DispatchBus::new(Arc::new(bus))),
        health: host::SystemHealth::new(false, "ok"),
        // Row 16 wiring: the harness boots without a host, so the module
        // registry starts empty — `/readyz` stays ready and `/meta/boot`
        // reports an empty map; tests seed states directly when they need
        // one (e.g. the module-admin integration tests).
        modules: Arc::new(std::sync::RwLock::new(kelson_lifecycle::ModuleRegistry::new())),
        env: Environment::Test,
        flags: Arc::new(std::sync::RwLock::new(Vec::new())),
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
        disks: Arc::new(disks), notifications: notification_store,
        mail: None,
        uploads: Default::default(),
        // Row 20: no config file or disk manifests in the harness: empty digest components.
        runtime: Arc::new(crate::runtime_status::RuntimeStatus::at_boot("", "", "")),
        channels: broadcast_routes::resolve_channels(&graph),
        presence: Arc::new(kelson_realtime::PresenceHub::new()),
        // No pool in the builder: no durable store — the /metrics queue
        // series stays absent (tests inject one to prove the census).
        // Wave 6 P9: same posture for the batch ledger — tests inject a
        // ledger + admin store to prove the batch operations.
        queue: None, batches: None,
        // Wave 8 ADR 0009 (D-52 posture, like `queue`): no registry is
        // registered and no trace plane is attached in the harness — the
        // ops dashboard's sections degrade inline; tests inject handles.
        schedule: None, traces: None,
        route_inventory: graph_routes::route_inventory(&graph),
        middleware_catalog: kelson_http::MiddlewareCatalog::builtin(),
        route_bindings, probes,
        throttle_buckets: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        route_plans: Arc::new(std::sync::RwLock::new(None)),
        metrics: Arc::new(kelson_metrics::MetricsRegistry::new(4096)),
        booted_at: std::time::Instant::now(),
        dashboard_enabled: true,
        // Sentinel (S5): an INERT runtime per harness — the incident plane,
        // status snapshot and capability gate all work without the worker
        // threads, so integration tests get production-shaped wiring without
        // spawning (and leaking) runtime threads per test case.
        telemetry: Some(
            kelson_observability::ObservabilityRuntime::new(
                {
                    let mut tcfg = kelson_observability::TelemetryConfig::default();
                    tcfg.wal.dir = storage_root.join("telemetry/wal");
                    tcfg.paths.incident_dir = storage_root.join("telemetry/control");
                    tcfg.paths.cost_file = storage_root.join("telemetry/cost.json");
                    tcfg.paths.health_dir = storage_root.join("telemetry/exporters");
                    tcfg
                },
                kelson_observability::ServiceIdentity::new("kelson-demo-test", "test"),
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
        security_headers: kelson_http::SecurityHeaders::default(),
        // Harness default: the argon2 baseline factors (the P5 drifted
        // fixture seeds its verifier through the stores directly).
        password_params: kelson_identity::PasswordParams::default(),
        dummy_hash: Arc::new(std::sync::OnceLock::new()),
        // Harness default: the Laravel-posture policy — the P6 register/
        // reset enforcement tests exercise exactly the stock default.
        password_policy: kelson_validation::PasswordPolicy::default(),
        // Harness default: the P7 posture — always-on for web-plane routes
        // with no skip paths; tests inject a bypass to prove it.
        csrf: kelson_http::CsrfGuard::default(),
    };
    (state, graph, appt_store)
}

/// Wave 9 B-30 facet suite: discovery + guest ABI gates, bus dispatch
/// roundtrip, authz + capability denials, fuel -> 503, loader refusals.
/// Rides the crate's existing lib test binary (suite count stays 150).
#[cfg(test)]
mod wasm_tests;

#[cfg(test)]
mod views_tests {
    use super::views_config;

    #[test]
    fn relative_paths_resolve_against_the_workspace_root() {
        let section = kelson_config::ViewsSection {
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
        let abs = std::env::temp_dir().join("kelson-abs-views");
        let section = kelson_config::ViewsSection {
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
        let section = kelson_config::ViewsSection {
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
            kelson_notifications::DEFAULT_MAX_ATTACHMENT_BYTES as usize,
            kelson_http::UploadLimits::default().max_file_bytes
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
        let root = std::env::temp_dir().join(format!("kelson-pkgviews-{}", uuid::Uuid::now_v7()));
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

        let engine = kelson_views::ViewsEngine::new(kelson_views::ViewsConfig {
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
        assert_eq!(err.code, kelson_views::engine::VIEW_NOT_FOUND);

        // No index -> no resolver installed -> namespaces stay closed.
        let bare = std::env::temp_dir().join(format!("kelson-pkgviews-bare-{}", uuid::Uuid::now_v7()));
        let engine = kelson_views::ViewsEngine::new(kelson_views::ViewsConfig {
            paths: vec![],
            cache_dir: None,
            env: "test".into(),
        });
        crate::register_package_view_namespaces(&bare, &engine);
        let err = engine
            .render("billing::welcome", serde_json::json!({}))
            .expect_err("no index means no namespaces");
        assert_eq!(err.code, kelson_views::engine::VIEW_NOT_FOUND);

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn notification_html_mail_renders_through_the_views_engine() {
        use kelson_notifications::{
            Channel, Mailable, MailPort, NotificationContext, NotificationDef, NotifyHub,
            ResolvedMailAttachment,
        };

        let root = std::env::temp_dir().join(format!("kelson-notif-views-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(root.join("notifications/appointment")).expect("view dirs");
        std::fs::write(
            root.join("notifications/appointment/booked.html"),
            "<p>Booked for {{ patient }} ({{ slots }} slot(s))</p>",
        )
        .expect("fixture template");
        let engine = std::sync::Arc::new(kelson_views::ViewsEngine::new(kelson_views::ViewsConfig {
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
            principal_id: kelson_foundation::PrincipalId::new(),
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
        assert_eq!(err.code, kelson_foundation::codes::INTERNAL);
        assert_eq!(log.lock().unwrap().len(), 1, "nothing was sent");

        struct LogMail {
            sent: std::sync::Arc<std::sync::Mutex<Vec<Mailable>>>,
        }
        impl MailPort for LogMail {
            fn send(
                &self,
                _msg: &kelson_notifications::NotifyMessage,
                _subject: &str,
            ) -> Result<(), kelson_foundation::AppError> {
                Ok(())
            }
            fn send_mailable(
                &self,
                mail: &Mailable,
                _attachments: &[ResolvedMailAttachment],
            ) -> Result<(), kelson_foundation::AppError> {
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
    /// (`kelson-mod-appointments::AppointmentBooked`) renders subject and
    /// text through the hub catalog with `{patient}` interpolated from
    /// the payload: FR bytes out of an FR request locale.
    #[cfg(feature = "demo")]
    #[tokio::test]
    async fn producer_mail_localizes_through_the_shipped_catalog_and_resolver() {
        use kelson_notifications::{
            Channel, Mailable, MailPort, NotificationContext, NotifyHub, ResolvedMailAttachment,
        };

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let catalog = std::sync::Arc::new(
            kelson_config::load_messages(&root, &kelson_config::I18nSection::default())
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
                _msg: &kelson_notifications::NotifyMessage,
                _subject: &str,
            ) -> Result<(), kelson_foundation::AppError> {
                Ok(())
            }
            fn send_mailable(
                &self,
                mail: &Mailable,
                _attachments: &[ResolvedMailAttachment],
            ) -> Result<(), kelson_foundation::AppError> {
                mail.validate()?;
                self.sent.lock().unwrap().push(mail.clone());
                Ok(())
            }
        }

        let log = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut hub = NotifyHub::with_catalog(catalog);
        hub.mail = Some(Box::new(RecordingMail { sent: log.clone() }));

        let def = kelson_mod_appointments::AppointmentBooked {
            patient: "Ada".into(),
            slot: "2026-09-01T10:00Z".into(),
            locale: Some(locale.clone()),
        };
        hub.dispatch_notification(
            &def,
            Channel::Email,
            Some("ada@example.com"),
            &NotificationContext {
                principal_id: kelson_foundation::PrincipalId::new(),
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
        use kelson_jobs::JobStore as _;
        use kelson_notifications::{
            Channel, Mailable, MailPort, NotificationContext, NotificationDef, NotifyHub,
            ResolvedMailAttachment,
        };

        // Enqueue side: the port over the in-memory job store + the
        // registered `notification.send` def (what serve() wires pool-gated).
        let jobs = std::sync::Arc::new(kelson_jobs::InMemoryJobStore::new());
        let mut registry = kelson_jobs::JobRegistry::new();
        registry
            .register(kelson_jobs::JobDef::new(
                kelson_notifications::NOTIFICATION_SEND_JOB,
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
                Some(kelson_notifications::NOTIFICATION_SEND_JOB)
            }
        }

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let catalog = std::sync::Arc::new(
            kelson_config::load_messages(&root, &kelson_config::I18nSection::default())
                .expect("shipped lang files load"),
        );
        let producer_hub = NotifyHub::with_catalog(catalog).with_queue(Box::new(port));
        producer_hub
            .dispatch_notification(
                &QueuedNotice,
                Channel::Email,
                Some("ada@example.com"),
                &NotificationContext {
                    principal_id: kelson_foundation::PrincipalId::new(),
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
        assert_eq!(job.name, kelson_notifications::NOTIFICATION_SEND_JOB);
        let envelope: kelson_notifications::QueuedNotificationDelivery =
            serde_json::from_value(job.payload).expect("envelope payload");
        assert_eq!(
            envelope.v,
            kelson_notifications::QueuedNotificationDelivery::VERSION
        );
        assert_eq!(envelope.type_key, "notice.queued");
        // … with the FR subject already baked in at enqueue.
        let pre_rendered = match &envelope.payload {
            kelson_notifications::DeliveryPayload::Mail(mail) => mail.clone(),
            other => panic!("mail payload expected, got {other:?}"),
        };

        // Drain side: a fresh hub with NO catalog — only I/O.
        struct RecordingMail {
            sent: std::sync::Arc<std::sync::Mutex<Vec<Mailable>>>,
        }
        impl MailPort for RecordingMail {
            fn send(
                &self,
                _msg: &kelson_notifications::NotifyMessage,
                _subject: &str,
            ) -> Result<(), kelson_foundation::AppError> {
                Ok(())
            }
            fn send_mailable(
                &self,
                mail: &Mailable,
                _attachments: &[ResolvedMailAttachment],
            ) -> Result<(), kelson_foundation::AppError> {
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
        let root = std::env::temp_dir().join(format!("kelson-disks-synth-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).expect("temp root");
        let registry = disk_registry(&kelson_config::StorageConfig::default(), &root).unwrap();
        assert_eq!(registry.default_name(), "local");
        assert_eq!(registry.names(), vec!["local"]);
        registry.validate_default().unwrap();
        let (name, entry) = registry.resolve(None).unwrap();
        assert_eq!(name, "local");
        assert!(matches!(
            entry.visibility,
            kelson_storage::Visibility::Private
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
        let root = std::env::temp_dir().join(format!("kelson-disks-decl-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).expect("temp root");
        let mut section = kelson_config::StorageConfig {
            default: "cdn".into(),
            ..Default::default()
        };
        section.disks.insert(
            "assets".into(),
            kelson_config::DiskConfig {
                driver: kelson_config::DiskDriver::Local,
                root: Some("storage/app".into()),
                bucket: None,
                url: None,
                visibility: kelson_config::DiskVisibility::Private,
            },
        );
        section.disks.insert(
            "cdn".into(),
            kelson_config::DiskConfig {
                driver: kelson_config::DiskDriver::S3,
                root: None,
                bucket: Some("kelson".into()),
                url: Some("https://cdn.example.com".into()),
                visibility: kelson_config::DiskVisibility::Public,
            },
        );
        let registry = disk_registry(&section, &root).unwrap();
        assert_eq!(registry.names(), vec!["assets", "cdn"]);
        registry.validate_default().unwrap();
        // Public issuance resolves through the `url` base (`{base}/{bucket}/{key}`).
        let obj = kelson_storage::ObjectRef {
            bucket: "b".into(),
            key: "k/f.png".into(),
        };
        assert_eq!(
            registry.url_for("cdn", &obj, 30).unwrap(),
            "https://cdn.example.com/b/k/f.png"
        );

        // local without `root` refuses boot.
        let mut bad = kelson_config::StorageConfig::default();
        bad.disks.insert(
            "d".into(),
            kelson_config::DiskConfig {
                driver: kelson_config::DiskDriver::Local,
                root: None,
                bucket: None,
                url: None,
                visibility: kelson_config::DiskVisibility::Private,
            },
        );
        let err = disk_registry(&bad, &root).unwrap_err();
        assert!(err.public_message.contains("root"), "{}", err.public_message);

        // s3 without `bucket` refuses boot.
        let mut bad = kelson_config::StorageConfig::default();
        bad.disks.insert(
            "d".into(),
            kelson_config::DiskConfig {
                driver: kelson_config::DiskDriver::S3,
                root: None,
                bucket: None,
                url: None,
                visibility: kelson_config::DiskVisibility::Private,
            },
        );
        let err = disk_registry(&bad, &root).unwrap_err();
        assert!(
            err.public_message.contains("bucket"),
            "{}",
            err.public_message
        );

        // A `storage.default` naming no registered disk refuses boot.
        let mut bad = kelson_config::StorageConfig {
            default: "nope".into(),
            ..Default::default()
        };
        bad.disks.insert(
            "d".into(),
            kelson_config::DiskConfig {
                driver: kelson_config::DiskDriver::S3,
                root: None,
                bucket: Some("b".into()),
                url: None,
                visibility: kelson_config::DiskVisibility::Private,
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
        let mut security = kelson_config::SecuritySection::default();
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

// ------------------------------------------- Row 16: config-poller lifecycle

#[cfg(test)]
mod config_poll_tests {
    use super::{note_config_poll, ConfigPoll};
    use kelson_lifecycle::{ModuleRegistry, ModuleState};
    use std::sync::Arc;

    /// A registry with one module tracked at Ready (what the host leaves
    /// after a successful boot).
    fn tracked_ready(name: &str) -> Arc<std::sync::RwLock<ModuleRegistry>> {
        let mut registry = ModuleRegistry::new();
        registry.discover(name);
        registry.advance(name, ModuleState::Validated).unwrap();
        registry.advance(name, ModuleState::Registered).unwrap();
        registry.advance(name, ModuleState::Initialized).unwrap();
        registry.advance(name, ModuleState::Ready).unwrap();
        Arc::new(std::sync::RwLock::new(registry))
    }

    #[test]
    fn refused_reload_degrades_config_and_a_landed_reload_recovers_it() {
        let modules = tracked_ready("config");
        note_config_poll(&modules, ConfigPoll::Refused);
        assert_eq!(
            modules.read().unwrap().state("config"),
            Some(ModuleState::Degraded)
        );
        // A second refusal stays degraded (no invalid-transition error).
        note_config_poll(&modules, ConfigPoll::Refused);
        assert_eq!(
            modules.read().unwrap().state("config"),
            Some(ModuleState::Degraded)
        );
        // Unchanged is a no-op: the revision is still refused.
        note_config_poll(&modules, ConfigPoll::Unchanged);
        assert_eq!(
            modules.read().unwrap().state("config"),
            Some(ModuleState::Degraded)
        );
        // A reload that lands recovers the module.
        note_config_poll(&modules, ConfigPoll::Reloaded);
        assert_eq!(
            modules.read().unwrap().state("config"),
            Some(ModuleState::Ready)
        );
        // Reload while ready is a no-op (no invalid recover).
        note_config_poll(&modules, ConfigPoll::Reloaded);
        assert_eq!(
            modules.read().unwrap().state("config"),
            Some(ModuleState::Ready)
        );
    }

    #[test]
    fn untracked_modules_are_left_alone() {
        let modules = tracked_ready("other");
        // A refused reload with no `config` module: nothing to degrade, and
        // the tracked module is untouched.
        note_config_poll(&modules, ConfigPoll::Refused);
        assert_eq!(modules.read().unwrap().state("config"), None);
        assert_eq!(
            modules.read().unwrap().state("other"),
            Some(ModuleState::Ready)
        );
    }
}

// Row 19 (auth mail): the `[mail]` failover chain behind the notifications
// `MailPort`, wired into `AppState::mail` at boot (harness leaves it `None`).
mod mail;

/// Harness boot with the appointments handlers bound to the caller's model
/// store (Row 19): the HTTP-over-PostgreSQL e2e seam — production's
/// `PersistenceProvider` binds the same handlers to `PgModelStore` when the
/// driver pool is up. Every other suite keeps the in-memory backend through
/// `build_app_state` below.
#[cfg(feature = "demo")]
pub fn build_app_state_with_backend(
    storage_root: &std::path::Path,
    sessions_enabled: bool,
    registration_open: bool,
    appt_backend: std::sync::Arc<dyn kelson_models::ModelStore>,
) -> (
    routes::AppState,
    kelson_registry::PlatformGraph,
    HarnessAppStore,
) {
    build_app_state_inner(storage_root, sessions_enabled, registration_open, appt_backend)
}

/// The harness boot every suite builds on: appointments over a fresh
/// in-memory model store (S7 test plan).
pub fn build_app_state(
    storage_root: &std::path::Path,
    sessions_enabled: bool,
    registration_open: bool,
) -> (
    routes::AppState,
    kelson_registry::PlatformGraph,
    HarnessAppStore,
) {
    build_app_state_inner(
        storage_root,
        sessions_enabled,
        registration_open,
        // The harness default is the in-memory store every suite pins.
        std::sync::Arc::new(kelson_models::InMemoryModelStore::new()),
    )
}
