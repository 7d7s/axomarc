//! Notification templates, channel config, sends (M7-02).
//! Scaffold: endpoints return 501 until their execution-plan task lands.
use std::sync::Arc;

use axum::routing::{get, post};
use kernel_core::{ApiVersion, Module, ModuleManifest, RouteSpec};
use kernel_runtime::{not_implemented, HttpModule, Router, Services};

pub struct NotifyCfgModule;

impl Module for NotifyCfgModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "notifycfg",
            version: env!("CARGO_PKG_VERSION"),
            kernel_range: ">=0.1, <0.2",
            streams: &["notifications"],
            capabilities: &["notify.manage", "notify.read"],
            depends_on_streams: &[],
        }
    }

    fn routes(&self) -> Vec<RouteSpec> {
        vec![
            RouteSpec { module: "notifycfg", method: "GET", path: "/api/v1/notifications/templates", capability: Some("notify.read"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
        ]
    }
}

impl HttpModule for NotifyCfgModule {
    fn router(&self, _services: Arc<Services>) -> Router {
        Router::new()
            .route("/api/v1/notifications/templates", get(not_implemented))
    }
}