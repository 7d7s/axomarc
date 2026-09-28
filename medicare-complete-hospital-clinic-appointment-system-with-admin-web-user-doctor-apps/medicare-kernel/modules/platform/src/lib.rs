//! Platform plane: tenants, plans, break-glass, support mode, export/erase (M7-07, D-21/22).
//! Scaffold: endpoints return 501 until their execution-plan task lands.
use std::sync::Arc;

use axum::routing::{get, post};
use kernel_core::{ApiVersion, Module, ModuleManifest, RouteSpec};
use kernel_runtime::{not_implemented, HttpModule, Router, Services};

pub struct PlatformModule;

impl Module for PlatformModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "platform",
            version: env!("CARGO_PKG_VERSION"),
            kernel_range: ">=0.1, <0.2",
            streams: &["tenants", "platform.audit", "metering"],
            capabilities: &["platform.tenant.manage", "platform.security", "platform.sre", "platform.release", "platform.support"],
            depends_on_streams: &[],
        }
    }

    fn routes(&self) -> Vec<RouteSpec> {
        vec![
            RouteSpec { module: "platform", method: "GET", path: "/platform/v1/tenants", capability: Some("platform.tenant.manage"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
            RouteSpec { module: "platform", method: "POST", path: "/platform/v1/tenants/{id}/support-session", capability: Some("platform.support"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
            RouteSpec { module: "platform", method: "GET", path: "/platform/v1/tenants/{id}/export", capability: Some("platform.support"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
            RouteSpec { module: "platform", method: "POST", path: "/platform/v1/break-glass", capability: Some("platform.security"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
        ]
    }
}

impl HttpModule for PlatformModule {
    fn router(&self, _services: Arc<Services>) -> Router {
        Router::new()
            .route("/platform/v1/tenants", get(not_implemented))
            .route("/platform/v1/tenants/{id}/support-session", post(not_implemented))
            .route("/platform/v1/tenants/{id}/export", get(not_implemented))
            .route("/platform/v1/break-glass", post(not_implemented))
    }
}