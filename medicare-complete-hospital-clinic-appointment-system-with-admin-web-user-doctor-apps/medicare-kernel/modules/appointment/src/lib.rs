//! Appointment module (Tier A). Spec: docs/rust/04-API-CONTRACTS §8.
//! Anatomy follows 10-STRUCTURE-AND-GUIDELINES §2.2 (D-43).
pub mod events;
pub mod http;
pub mod models;
pub mod policies;
pub mod repositories;
pub mod services;
pub mod workflows;

use std::sync::Arc;

use axum::routing::{get, post};
use kernel_core::{ApiVersion, Module, ModuleManifest, RouteSpec};
use kernel_runtime::{HttpModule, Router, Services};

pub struct AppointmentModule;

impl Module for AppointmentModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "appointment",
            version: env!("CARGO_PKG_VERSION"),
            kernel_range: ">=0.1, <0.2",
            streams: &["appointments", "queue"],
            capabilities: policies::ALL,
            depends_on_streams: &[],
        }
    }

    fn routes(&self) -> Vec<RouteSpec> {
        vec![
            RouteSpec { module: "appointment", method: "POST", path: "/api/v1/appointments", capability: Some(policies::WRITE), min_api_version: ApiVersion::V1, notes: "idemotent booking (APT-02)" },
            RouteSpec { module: "appointment", method: "GET", path: "/api/v1/appointments/{id}", capability: Some(policies::READ), min_api_version: ApiVersion::V1, notes: "includes allowed_events" },
            RouteSpec { module: "appointment", method: "POST", path: "/api/v1/appointments/{id}/transition", capability: Some(policies::TRANSITION), min_api_version: ApiVersion::V1, notes: "workflow-validated" },
        ]
    }
}

impl HttpModule for AppointmentModule {
    fn router(&self, _services: Arc<Services>) -> Router {
        Router::new()
            .route("/api/v1/appointments", post(http::controllers::create))
            .route("/api/v1/appointments/{id}", get(http::controllers::get))
            .route("/api/v1/appointments/{id}/transition", post(http::controllers::transition))
    }
}