//! Org lattice: nodes, memberships, roles, grants (M2 tasks).
//! Scaffold: endpoints return 501 until their execution-plan task lands.
use std::sync::Arc;

use axum::routing::{get, post};
use kernel_core::{ApiVersion, Module, ModuleManifest, RouteSpec};
use kernel_runtime::{not_implemented, HttpModule, Router, Services};

pub struct OrgModule;

impl Module for OrgModule {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            id: "org",
            version: env!("CARGO_PKG_VERSION"),
            kernel_range: ">=0.1, <0.2",
            streams: &["org"],
            capabilities: &["org.read", "org.write", "user.manage", "role.manage", "grant.manage"],
            depends_on_streams: &[],
        }
    }

    fn routes(&self) -> Vec<RouteSpec> {
        vec![
            RouteSpec { module: "org", method: "GET", path: "/api/v1/org/nodes", capability: Some("org.read"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
            RouteSpec { module: "org", method: "POST", path: "/api/v1/users", capability: Some("user.manage"), min_api_version: ApiVersion::V1, notes: "scaffold — see 07-EXECUTION-PLAN" },
        ]
    }
}

impl HttpModule for OrgModule {
    fn router(&self, _services: Arc<Services>) -> Router {
        Router::new()
            .route("/api/v1/org/nodes", get(not_implemented))
            .route("/api/v1/users", post(not_implemented))
    }
}