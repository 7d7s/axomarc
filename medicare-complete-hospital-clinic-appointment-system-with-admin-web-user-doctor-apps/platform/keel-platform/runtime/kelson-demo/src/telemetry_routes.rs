//! Admin telemetry surface (Sentinel S5, D-76): session-authenticated
//! operators control incident mode over HTTP and read runtime status. The
//! file control plane under `storage/framework/telemetry/control/` stays
//! the single source of truth — handlers go through the runtime's incident
//! plane, so CLI, HTTP and the pollers all agree.
//!
//! Also hosts the request-context middleware: every request gets a
//! `TelemetryContext` extension built from the inbound W3C `traceparent`
//! (or a fresh trace root) joined with the admission request id. Handlers
//! read it from extensions; causality is never hand-crafted.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use kelson_foundation::AppError;
use kelson_http::{Envelope, ResolvedSession};
use kelson_observability::{IncidentState, ObservabilityRuntime};
use serde::{Deserialize, Serialize};

use crate::routes::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/admin/telemetry/incident", post(incident_control))
        .route("/admin/telemetry/status", get(status))
        .with_state(state)
}

// ------------------------------------------------------------- resolution

fn runtime(state: &AppState) -> Result<Arc<ObservabilityRuntime>, AppError> {
    state
        .telemetry
        .clone()
        .ok_or_else(|| AppError::internal("telemetry runtime not wired in this host"))
}

/// Session + operator capability gate (incident control is never public).
fn authorize(
    state: &AppState,
    session: &ResolvedSession,
    runtime: &ObservabilityRuntime,
) -> Result<(), AppError> {
    let capability = runtime.config().incident_capability.clone();
    let Some(tenant) = session.session.tenant else {
        return Err(AppError::denied(
            "telemetry admin requires a tenant session",
        ));
    };
    let Some(snapshot) = state.policy.snapshot(session.session.principal, tenant) else {
        return Err(AppError::denied("no active access snapshot"));
    };
    if !snapshot.capabilities.contains(&capability) {
        return Err(AppError::denied(format!(
            "missing operator capability {capability}"
        )));
    }
    Ok(())
}

/// Wave 2 error rendering: registry status/reportability + `app.debug`
/// detail, with the deployment's RFC-7807 opt-in honoured (the request's
/// `Accept` wins when headers are forwarded).
fn error_response(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    err: &AppError,
) -> axum::response::Response {
    state.error_response(err, headers, None)
}

// --------------------------------------------------------------- incident

#[derive(Debug, Deserialize)]
struct IncidentRequest {
    /// `start` (default for an empty body) or `stop`.
    #[serde(default = "default_action")]
    action: String,
    /// Control flags recorded on the incident (start only), e.g.
    /// `{"debug_retention": "true"}`.
    #[serde(default)]
    flags: BTreeMap<String, String>,
}

fn default_action() -> String {
    "start".into()
}

#[derive(Debug, Serialize)]
struct IncidentBody {
    outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    incident: Option<IncidentState>,
}

async fn incident_control(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<axum::extract::Extension<ResolvedSession>>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let Some(session) = session else {
        return error_response(&state, &headers, &AppError::unauthenticated());
    };
    let Ok(runtime) = runtime(&state) else {
        return error_response(
            &state,
            &headers,
            &AppError::internal("telemetry runtime not wired in this host"),
        );
    };
    if let Err(err) = authorize(&state, &session, &runtime) {
        return error_response(&state, &headers, &err);
    }

    // Empty body -> start (parity with `kelson observe incident start`).
    let request: IncidentRequest = if body.is_empty() {
        IncidentRequest {
            action: default_action(),
            flags: BTreeMap::new(),
        }
    } else {
        match serde_json::from_slice(&body) {
            Ok(request) => request,
            Err(_) => {
                return error_response(
                    &state,
                    &headers,
                    &AppError::validation("request body must be valid JSON"),
                );
            }
        }
    };

    let operator = format!(
        "principal:{} via /admin/telemetry/incident",
        session.session.principal
    );
    let plane = runtime.incident_plane();
    match request.action.as_str() {
        "start" => {
            let active_id = plane.read().map(|active| active.id);
            let incident = match plane.start_with_flags(Some(&operator), request.flags) {
                Ok(incident) => incident,
                Err(_) => {
                    return error_response(
                        &state,
                        &headers,
                        &AppError::internal("incident control plane write failed"),
                    );
                }
            };
            let outcome = if active_id == Some(incident.id) {
                "already_active"
            } else {
                runtime.info(
                    "incident started via admin endpoint",
                    kelson_observability::fields!(
                        "incident_id" => incident.id.to_string(),
                        "operator" => operator
                    ),
                );
                "started"
            };
            (
                axum::http::StatusCode::OK,
                Json(
                    serde_json::to_value(Envelope::ok(IncidentBody {
                        outcome: outcome.into(),
                        incident: Some(incident),
                    }))
                    .unwrap(),
                ),
            )
                .into_response()
        }
        "stop" => match plane.stop() {
            Ok(Some(incident)) => {
                runtime.info(
                    "incident stopped via admin endpoint",
                    kelson_observability::fields!(
                        "incident_id" => incident.id.to_string(),
                        "operator" => operator
                    ),
                );
                (
                    axum::http::StatusCode::OK,
                    Json(
                        serde_json::to_value(Envelope::ok(IncidentBody {
                            outcome: "stopped".into(),
                            incident: Some(incident),
                        }))
                        .unwrap(),
                    ),
                )
                    .into_response()
            }
            Ok(None) => (
                axum::http::StatusCode::OK,
                Json(
                    serde_json::to_value(Envelope::ok(IncidentBody {
                        outcome: "none_active".into(),
                        incident: None,
                    }))
                    .unwrap(),
                ),
            )
                .into_response(),
            Err(_) => error_response(
                &state,
                &headers,
                &AppError::internal("incident control plane write failed"),
            ),
        },
        other => error_response(
            &state,
            &headers,
            &AppError::validation(format!("unknown action {other:?} (expected start|stop)")),
        ),
    }
}

// ----------------------------------------------------------------- status

async fn status(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<axum::extract::Extension<ResolvedSession>>,
) -> axum::response::Response {
    let Some(session) = session else {
        return error_response(&state, &headers, &AppError::unauthenticated());
    };
    let Ok(runtime) = runtime(&state) else {
        return error_response(
            &state,
            &headers,
            &AppError::internal("telemetry runtime not wired in this host"),
        );
    };
    if let Err(err) = authorize(&state, &session, &runtime) {
        return error_response(&state, &headers, &err);
    }
    // The file control plane is the source of truth (D-76): report it
    // directly instead of the poller-cached copy (up to 2 s stale, and
    // absent entirely on hosts whose runtime has not started).
    let mut status = runtime.status();
    status.incident = runtime.incident_plane().read();
    (
        axum::http::StatusCode::OK,
        Json(serde_json::to_value(Envelope::ok(status)).unwrap()),
    )
        .into_response()
}

// -------------------------------------------------------- context middleware

/// Pure context construction (unit-testable): continue the inbound trace
/// when a valid `traceparent` arrives (our request span becomes a child of
/// the remote span), else start a fresh root; join the admission request id.
fn context_for(
    headers: &axum::http::HeaderMap,
    request_id: Option<kelson_foundation::RequestId>,
) -> kelson_foundation::TelemetryContext {
    let inbound = headers
        .get("traceparent")
        .and_then(|v| v.to_str().ok())
        .and_then(kelson_observability::parse_traceparent);
    let mut ctx = match inbound {
        Some(parent) => {
            let mut ctx = kelson_foundation::TelemetryContext::with_trace(parent.trace_id);
            ctx.parent_span_id = Some(parent.parent_span_id);
            ctx.span_id = Some(kelson_foundation::SpanId::new());
            ctx
        }
        None => kelson_foundation::TelemetryContext::root(),
    };
    if let Some(request_id) = request_id {
        ctx = ctx.with_request(request_id);
    }
    ctx
}

/// Axum middleware: insert the per-request `TelemetryContext` extension.
/// Runs inside admission, so the request-id extension already exists.
pub async fn telemetry_context(
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let request_id = req
        .extensions()
        .get::<kelson_foundation::RequestId>()
        .copied();
    let ctx = context_for(req.headers(), request_id);
    req.extensions_mut().insert(ctx);
    next.run(req).await
}

// ------------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continues_inbound_trace() {
        let trace = kelson_foundation::TraceId::new();
        // W3C span ids are 8 bytes: the round-trip zero-fills the low half,
        // so model a remote span accordingly.
        let mut span_bytes = [0u8; 16];
        span_bytes[..8].copy_from_slice(&uuid::Uuid::now_v7().as_bytes()[..8]);
        let remote_span = kelson_foundation::SpanId::from_uuid(uuid::Uuid::from_bytes(span_bytes));
        let header = kelson_observability::make_traceparent(trace, remote_span, true);
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("traceparent", header.parse().unwrap());

        let ctx = context_for(&headers, None);
        assert_eq!(ctx.trace_id, trace);
        assert_eq!(ctx.parent_span_id, Some(remote_span));
        assert!(ctx.span_id.is_some(), "request span id assigned");
        assert_ne!(ctx.span_id, Some(remote_span));
    }

    #[test]
    fn malformed_or_missing_header_starts_fresh_trace() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert("traceparent", "garbage".parse().unwrap());
        let ctx = context_for(&headers, None);
        assert!(ctx.parent_span_id.is_none());
        assert!(ctx.span_id.is_none());
    }

    #[test]
    fn joins_admission_request_id() {
        let headers = axum::http::HeaderMap::new();
        let request_id = kelson_foundation::RequestId::new();
        let ctx = context_for(&headers, Some(request_id));
        assert_eq!(ctx.request_id, Some(request_id));
    }
}
