//! Read projections + standard reports (M7-05, D-41).
//! Scaffold: endpoints return 501 until their execution-plan task lands.
use std::sync::Arc;

use axum::routing::{get, post};
use kernel_core::{ApiVersion, Module, ModuleManifest, RouteSpec};
use kernel_runtime::{not_implemented, HttpModule, Router, Services};

pub struct ReportsModule;

impl Module for ReportsModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "reports",
            version: env!("CARGO_PKG_VERSION"),
            kernel_range: ">=0.1, <0.2",
            streams: &[],
            capabilities: &["reports.read"],
            depends_on_streams: &["appointments", "invoices", "stock"],
        }
    }

    fn routes(&self) -> Vec<RouteSpec> {
        vec![
            RouteSpec { module: "reports", method: "GET", path: "/api/v1/reports/appointments", capability: Some("reports.read"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
        ]
    }
}

impl HttpModule for ReportsModule {
    fn router(&self, _services: Arc<Services>) -> Router {
        Router::new()
            .route("/api/v1/reports/appointments", get(not_implemented))
    }
}