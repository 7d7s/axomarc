//! Operator module-lifecycle surface (docs/15-EXTENSION, Row 16 wiring):
//! session-authenticated operators degrade, recover or disable a tracked
//! module over HTTP. Mirrors the telemetry admin surface (`telemetry_routes`)
//! — session + capability gate — and defers to the registry's transition
//! rules: an invalid transition (recover a Ready module, disable twice) is a
//! 409 CONFLICT, an untracked module a 404, an unknown action a 422.
//!
//! Honest limit: this changes the module's *state*, surfaced by `/meta/boot`
//! and consulted by `/readyz`, but dispatch does not yet refuse a Disabled
//! module's routes — the registry keys providers by name (`module.appointments`)
//! while `RouteEntry.module` carries the manifest name (`appointments`), so a
//! dispatch-level gate needs that mapping first.

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use kelson_foundation::AppError;
use kelson_http::{Envelope, ResolvedSession};
use serde::{Deserialize, Serialize};

use crate::routes::AppState;

/// Capability required to drive module lifecycle transitions. Not granted to
/// registered principals by default (same posture as `telemetry.incident.manage`):
/// an operator grants it through the access snapshot.
pub const MODULE_MANAGE_CAPABILITY: &str = "platform.modules.manage";

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/admin/modules", post(manage))
        .with_state(state)
}

/// Session + operator capability gate (module lifecycle control is never
/// public — same shape as the telemetry incident gate).
fn authorize(state: &AppState, session: &ResolvedSession) -> Result<(), AppError> {
    let Some(tenant) = session.session.tenant else {
        return Err(AppError::denied(
            "module control requires a tenant session",
        ));
    };
    let Some(snapshot) = state.policy.snapshot(session.session.principal, tenant) else {
        return Err(AppError::denied("no active access snapshot"));
    };
    if !snapshot.capabilities.contains(MODULE_MANAGE_CAPABILITY) {
        return Err(AppError::denied(format!(
            "missing module control capability {MODULE_MANAGE_CAPABILITY}"
        )));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct ManageRequest {
    /// Tracked module name (as keyed by the registry, e.g. `config`).
    name: String,
    /// `degrade`, `recover` or `disable`.
    action: String,
}

#[derive(Debug, Serialize)]
struct ManageBody {
    module: String,
    action: String,
    state: kelson_lifecycle::ModuleState,
}

async fn manage(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<axum::extract::Extension<ResolvedSession>>,
    Json(request): Json<ManageRequest>,
) -> axum::response::Response {
    let Some(session) = session else {
        return state.error_response(&AppError::unauthenticated(), &headers, None);
    };
    if let Err(err) = authorize(&state, &session) {
        return state.error_response(&err, &headers, None);
    }

    let mut modules = state.modules.write().unwrap();
    if modules.state(&request.name).is_none() {
        return state.error_response(
            &AppError::not_found(format!("module {}", request.name)),
            &headers,
            None,
        );
    }
    let transition = match request.action.as_str() {
        "degrade" => modules.degrade(&request.name),
        "recover" => modules.recover(&request.name),
        "disable" => modules.disable(&request.name),
        other => {
            return state.error_response(
                &AppError::validation(format!(
                    "unknown action {other} (want degrade|recover|disable)"
                )),
                &headers,
                None,
            );
        }
    };
    match transition {
        Ok(()) => {
            let current = modules.state(&request.name).expect("tracked above");
            Json(Envelope::ok(ManageBody {
                module: request.name,
                action: request.action,
                state: current,
            }))
            .into_response()
        }
        // Invalid transition (e.g. recover a Ready module, or a terminal
        // Disabled one): the registry's rules refuse it — 409 CONFLICT.
        Err(message) => state.error_response(&AppError::conflict(message), &headers, None),
    }
}
