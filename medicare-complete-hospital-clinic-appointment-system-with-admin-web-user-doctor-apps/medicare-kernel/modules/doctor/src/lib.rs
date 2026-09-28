//! Doctors, schedules, slots (M4-03..05).
//! Scaffold: endpoints return 501 until their execution-plan task lands.
use std::sync::Arc;

use axum::routing::{get, post};
use kernel_core::{ApiVersion, Module, ModuleManifest, RouteSpec};
use kernel_runtime::{not_implemented, HttpModule, Router, Services};

pub struct DoctorModule;

impl Module for DoctorModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "doctor",
            version: env!("CARGO_PKG_VERSION"),
            kernel_range: ">=0.1, <0.2",
            streams: &["doctors", "slots"],
            capabilities: &["doctor.read", "doctor.manage", "slot.read"],
            depends_on_streams: &[],
        }
    }

    fn routes(&self) -> Vec<RouteSpec> {
        vec![
            RouteSpec { module: "doctor", method: "GET", path: "/api/v1/doctors", capability: Some("doctor.read"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
            RouteSpec { module: "doctor", method: "GET", path: "/api/v1/slots", capability: Some("slot.read"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
        ]
    }
}

impl HttpModule for DoctorModule {
    fn router(&self, _services: Arc<Services>) -> Router {
        Router::new()
            .route("/api/v1/doctors", get(not_implemented))
            .route("/api/v1/slots", get(not_implemented))
    }
}