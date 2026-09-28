//! Clinic structure: cities, clinics, departments, specializations, services (M4-01/02).
//! Scaffold: endpoints return 501 until their execution-plan task lands.
use std::sync::Arc;

use axum::routing::{get, post};
use kernel_core::{ApiVersion, Module, ModuleManifest, RouteSpec};
use kernel_runtime::{not_implemented, HttpModule, Router, Services};

pub struct ClinicModule;

impl Module for ClinicModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "clinic",
            version: env!("CARGO_PKG_VERSION"),
            kernel_range: ">=0.1, <0.2",
            streams: &["clinic"],
            capabilities: &["clinic.meta.read", "clinic.meta.write", "clinic.manage"],
            depends_on_streams: &[],
        }
    }

    fn routes(&self) -> Vec<RouteSpec> {
        vec![
            RouteSpec { module: "clinic", method: "GET", path: "/api/v1/clinics", capability: Some("clinic.meta.read"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
            RouteSpec { module: "clinic", method: "GET", path: "/api/v1/public/doctors", capability: None, min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
        ]
    }
}

impl HttpModule for ClinicModule {
    fn router(&self, _services: Arc<Services>) -> Router {
        Router::new()
            .route("/api/v1/clinics", get(not_implemented))
            .route("/api/v1/public/doctors", get(not_implemented))
    }
}