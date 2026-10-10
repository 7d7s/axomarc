//! my-app-api: application composition root (the `your-app` workspace).
//!
//! Thin binary: boot the kernel services (config, telemetry, tenant-guarded
//! database pools), compose THIS application's module routers, serve.
//!
//! Today the module list is compile-time (`mod_patients::router` below); the
//! target architecture replaces that merge with dynamic
//! `ModuleLoader::discover("modules/")` — drop a module in, reload, no
//! kernel rebuild. Everything else here (config freeze, telemetry runtime,
//! RLS pools, security posture) is the real boot path, unchanged.

use std::sync::Arc;

use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use kelson_framework::kelson_config::Environment;
use kelson_framework::kelson_observability::{
    ObservabilityRuntime, ServiceIdentity, TelemetryConfig,
};
use kelson_framework::kelson_http;
use kelson_framework::{CsrfGuard, PlatformConfig, SecurityHeaders, TenantGuardedPool, TenantId};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Config: layered load, then FROZEN (no post-boot mutation by type).
    let cfg_path = std::env::var("KEEL_CONFIG").unwrap_or_else(|_| "config/dev.toml".into());
    let cfg = PlatformConfig::load(Some(std::path::Path::new(&cfg_path)))?.freeze();

    // Telemetry: compiled defaults <- [telemetry] <- env overlay (RUST_LOG).
    let mut telemetry_config = TelemetryConfig::from_section(cfg.telemetry());
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
    telemetry_config.apply_env();
    let root = std::env::current_dir()?;
    telemetry_config.rebase(&root);
    let telemetry = ObservabilityRuntime::new(
        telemetry_config,
        ServiceIdentity::new("my-app-api", cfg.app().env.as_str()),
        &[],
    )?;
    telemetry.start()?;

    // Database: tenant pool (kelson_tenant, RLS enforced) + platform pool
    // (kelson_platform, break-glass). Separate URLs in production; one URL in
    // dev via [database].url.
    let db = cfg.database().ok_or("config [database] section required")?;
    let tenant_url = std::env::var("KEEL_TENANT_DATABASE_URL")
        .unwrap_or_else(|_| db.url.clone());
    let platform_url = std::env::var("KEEL_PLATFORM_DATABASE_URL")
        .unwrap_or_else(|_| db.url.clone());
    let pool = Arc::new(
        TenantGuardedPool::new(&tenant_url, &platform_url, db.max_connections).await?,
    );

    // This application's modules (compile-time list until ModuleLoader lands).
    let modules = mod_patients::router(pool.clone())
        .layer(middleware::from_fn(tenant_middleware));

    // Security posture: headers stamp every module response; the CSRF
    // double-submit guard wraps unsafe methods (GET /healthz stays clean).
    let security = &cfg.security();
    let headers = SecurityHeaders {
        enabled: security.headers.enabled,
        hsts_max_age_secs: security.headers.hsts_max_age_secs,
        hsts_include_subdomains: security.headers.hsts_include_subdomains,
        csp: security.headers.csp.clone(),
        referrer_policy: security.headers.referrer_policy.clone(),
    };
    let csrf = CsrfGuard {
        bypass_paths: security.csrf.skip_paths.clone(),
        secure_cookie: security.csrf.secure_cookie,
    };

    let router = Router::new()
        .route("/healthz", get(kelson_http::healthz))
        .merge(modules)
        .layer(middleware::from_fn_with_state(
            headers,
            kelson_http::security_headers_fn,
        ));
    let router = csrf.apply(router);

    let addr: std::net::SocketAddr = cfg.app().listen.parse()?;
    tracing::info!("my-app-api listening on {addr}");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router.into_make_service()).await?;

    // Drain telemetry before exit (flush buffers within the deadline).
    telemetry.shutdown(std::time::Duration::from_secs(5));
    Ok(())
}

/// Resolve the request tenant from the `x-tenant-id` header and expose it
/// as an extension for module handlers. Missing/invalid → 400 (fail closed;
/// production swaps this for the JWT/session identity stage).
async fn tenant_middleware(mut req: Request, next: Next) -> Response {
    let tenant = req
        .headers()
        .get("x-tenant-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| uuid::Uuid::parse_str(raw).ok())
        .map(TenantId::from_uuid);
    match tenant {
        Some(t) => {
            req.extensions_mut().insert(t);
            next.run(req).await
        }
        None => (
            StatusCode::BAD_REQUEST,
            "missing or invalid x-tenant-id header",
        )
            .into_response(),
    }
}
