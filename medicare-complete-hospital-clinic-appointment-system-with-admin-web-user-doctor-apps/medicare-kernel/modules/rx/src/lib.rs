//! Prescriptions + medicine master (M5-07/08).
//! Scaffold: endpoints return 501 until their execution-plan task lands.
use std::sync::Arc;

use axum::routing::{get, post};
use kernel_core::{ApiVersion, Module, ModuleManifest, RouteSpec};
use kernel_runtime::{not_implemented, HttpModule, Router, Services};

pub struct RxModule;

impl Module for RxModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "rx",
            version: env!("CARGO_PKG_VERSION"),
            kernel_range: ">=0.1, <0.2",
            streams: &["prescriptions"],
            capabilities: &["rx.read", "rx.write", "rx.transition"],
            depends_on_streams: &[],
        }
    }

    fn routes(&self) -> Vec<RouteSpec> {
        vec![
            RouteSpec { module: "rx", method: "POST", path: "/api/v1/prescriptions", capability: Some("rx.write"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
            RouteSpec { module: "rx", method: "GET", path: "/api/v1/medicines", capability: Some("rx.read"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
        ]
    }
}

impl HttpModule for RxModule {
    fn router(&self, _services: Arc<Services>) -> Router {
        Router::new()
            .route("/api/v1/prescriptions", post(not_implemented))
            .route("/api/v1/medicines", get(not_implemented))
    }
}