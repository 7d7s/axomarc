//! Patients, family, vitals, files (M4-06..08). PHI classes per 05-SECURITY §5.
//! Scaffold: endpoints return 501 until their execution-plan task lands.
use std::sync::Arc;

use axum::routing::{get, post};
use kernel_core::{ApiVersion, Module, ModuleManifest, RouteSpec};
use kernel_runtime::{not_implemented, HttpModule, Router, Services};

pub struct PatientModule;

impl Module for PatientModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "patient",
            version: env!("CARGO_PKG_VERSION"),
            kernel_range: ">=0.1, <0.2",
            streams: &["patients", "vitals"],
            capabilities: &["patient.read", "patient.write", "patient.phi.read", "patient.files.read", "patient.files.write"],
            depends_on_streams: &[],
        }
    }

    fn routes(&self) -> Vec<RouteSpec> {
        vec![
            RouteSpec { module: "patient", method: "GET", path: "/api/v1/patients", capability: Some("patient.read"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
            RouteSpec { module: "patient", method: "POST", path: "/api/v1/patients/{id}/vitals", capability: Some("patient.write"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
        ]
    }
}

impl HttpModule for PatientModule {
    fn router(&self, _services: Arc<Services>) -> Router {
        Router::new()
            .route("/api/v1/patients", get(not_implemented))
            .route("/api/v1/patients/{id}/vitals", post(not_implemented))
    }
}