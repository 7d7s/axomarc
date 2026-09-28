//! medicare-api: single binary (D-01). Boot: config → pool → migrate → seed →
//! compose modules → serve. Design: docs/rust/02-SYSTEM-DESIGN.md §4.
mod config;
mod modules;

use std::sync::Arc;

use kernel_core::TenantId;
use kernel_runtime::{build_router, Services};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let cfg = config::load()?;
    tracing::info!(deploy_mode = %cfg.deploy.mode, boot = "medicare-api");

    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| cfg.database.url.clone());
    let app = match kernel_db::create_pool(&db_url, cfg.database.max_connections).await {
        Ok(pool) => {
            kernel_db::run_migrations(&pool).await?;
            let demo_tenant = uuid::Uuid::parse_str(&cfg.demo.tenant_id)?;
            if let Err(e) = kernel_db::seed_tenant(&pool, demo_tenant, "demo", "Demo Tenant").await {
                tracing::warn!(error = %e, "demo tenant seed skipped");
            }
            let services = Arc::new(Services {
                pool,
                capability: kernel_capability::Engine::new(),
                demo_tenant_id: TenantId::from(demo_tenant),
            });
            build_router(&modules::all_modules(), services)
        }
        Err(e) => {
            tracing::warn!(error = %e, "database unavailable — degraded boot (/healthz only)");
            axum::Router::new().route("/healthz", axum::routing::get(kernel_runtime::healthz))
        }
    };

    let listener = tokio::net::TcpListener::bind(&cfg.server.bind).await?;
    tracing::info!(bind = %cfg.server.bind, "listening");
    axum::serve(listener, app).await?;
    Ok(())
}