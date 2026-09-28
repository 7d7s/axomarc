//! Invoices, double-entry ledger, payments, refunds, reconciliation (M6).
//! Scaffold: endpoints return 501 until their execution-plan task lands.
use std::sync::Arc;

use axum::routing::{get, post};
use kernel_core::{ApiVersion, Module, ModuleManifest, RouteSpec};
use kernel_runtime::{not_implemented, HttpModule, Router, Services};

pub struct BillingModule;

impl Module for BillingModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "billing",
            version: env!("CARGO_PKG_VERSION"),
            kernel_range: ">=0.1, <0.2",
            streams: &["invoices", "payments", "ledger"],
            capabilities: &["invoice.write", "invoice.finalize", "invoice.post", "invoice.reverse", "ledger.read", "payment.create", "payment.capture", "refund.request", "recon.read"],
            depends_on_streams: &[],
        }
    }

    fn routes(&self) -> Vec<RouteSpec> {
        vec![
            RouteSpec { module: "billing", method: "POST", path: "/api/v1/invoices", capability: Some("invoice.write"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
            RouteSpec { module: "billing", method: "GET", path: "/api/v1/ledger", capability: Some("ledger.read"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
        ]
    }
}

impl HttpModule for BillingModule {
    fn router(&self, _services: Arc<Services>) -> Router {
        Router::new()
            .route("/api/v1/invoices", post(not_implemented))
            .route("/api/v1/ledger", get(not_implemented))
    }
}