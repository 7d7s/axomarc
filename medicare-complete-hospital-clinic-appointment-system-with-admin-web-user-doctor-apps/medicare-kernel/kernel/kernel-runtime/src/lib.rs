//! kernel-runtime: axum glue. Modules may depend on kernel-core + this crate
//! (sanctioned platform deps: axum/sqlx/serde — 10-GUIDELINES §2.3 note).
pub mod envelope;
pub mod services;
pub mod smart_router;

pub use envelope::*;
pub use services::*;
pub use smart_router::*;

pub use axum; // single axum version across the workspace
pub use axum::Router; // re-exported so modules build routers without a second axum

use std::sync::Arc;

use axum::extract::Extension;
use axum::http::StatusCode;
use axum::routing::get;
use axum::Json;
use kernel_core::KernelError;
use tower_http::trace::TraceLayer;

/// Router returned by each module. Paths are absolute (/api/v1/... or /platform/v1/...).
pub trait HttpModule: kernel_core::Module {
    fn router(&self, services: Arc<Services>) -> Router;
}

pub async fn healthz() -> &'static str {
    "ok"
}

pub async fn readyz(Extension(services): Extension<Arc<Services>>) -> (StatusCode, &'static str) {
    let ok = sqlx::query("SELECT 1")
        .execute(&services.pool)
        .await
        .is_ok();
    if ok {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "db unavailable")
    }
}

/// Stub used by module scaffolds until their task lands.
pub async fn not_implemented() -> (StatusCode, Json<Envelope<serde_json::Value>>) {
    let err = KernelError::Internal;
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(Envelope::<serde_json::Value>::error_with(
            &err,
            "not implemented yet (see 07-EXECUTION-PLAN task)",
        )),
    )
}

/// Compose the final app router: platform surface + every module's routes (02 §6).
pub fn build_router(modules: &[Arc<dyn HttpModule>], services: Arc<Services>) -> Router {
    let mut app = Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz));
    for m in modules {
        app = app.merge(m.router(services.clone()));
    }
    app.layer(Extension(services.clone())).layer(TraceLayer::new_for_http())
}