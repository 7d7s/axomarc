//! Stock (movement-only), procurement PO workflows (M6-07/08).
//! Scaffold: endpoints return 501 until their execution-plan task lands.
use std::sync::Arc;

use axum::routing::{get, post};
use kernel_core::{ApiVersion, Module, ModuleManifest, RouteSpec};
use kernel_runtime::{not_implemented, HttpModule, Router, Services};

pub struct InventoryModule;

impl Module for InventoryModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "inventory",
            version: env!("CARGO_PKG_VERSION"),
            kernel_range: ">=0.1, <0.2",
            streams: &["stock", "procurement"],
            capabilities: &["stock.read", "stock.adjust", "procure.request", "procure.approve", "procure.receive"],
            depends_on_streams: &[],
        }
    }

    fn routes(&self) -> Vec<RouteSpec> {
        vec![
            RouteSpec { module: "inventory", method: "GET", path: "/api/v1/stock", capability: Some("stock.read"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
            RouteSpec { module: "inventory", method: "POST", path: "/api/v1/purchase-requests", capability: Some("procure.request"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
        ]
    }
}

impl HttpModule for InventoryModule {
    fn router(&self, _services: Arc<Services>) -> Router {
        Router::new()
            .route("/api/v1/stock", get(not_implemented))
            .route("/api/v1/purchase-requests", post(not_implemented))
    }
}