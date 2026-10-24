//! Wave 6 P9 batch operations surface (19-PARITY "Queues advanced" bullet
//! 304): capability-gated `batch.status` / `batch.cancel` / `batch.retry`
//! over the durable batch ledger (B-32) —
//!   * `GET  /api/v1/batches/{id}`       progress counts (live job-store state)
//!   * `POST /api/v1/batches/{id}/cancel`  pending members → dead (P8 `kill`)
//!   * `POST /api/v1/batches/{id}/retry`   failed/dead members → pending (`requeue`)
//!
//! The same [`BatchOps`] service backs the console's `queue:retry-batch`,
//! so the HTTP edge and the CLI agree by construction. Every handler is
//! identity-resolved (session or bearer) and gated on a framework capability
//! from [`crate::notification_routes::FRAMEWORK_CAPABILITIES`] (granted at
//! login, revocable per snapshot). Without a pool there is no ledger and no
//! admin store — the endpoints answer `KSN_NOT_IMPLEMENTED` (D-52
//! fail-closed) rather than reporting batches they cannot see.

use std::sync::Arc;

use axum::extract::{Extension, Path, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use kelson_foundation::{AppError, PrincipalId, TenantId};
use kelson_http::{Envelope, ResolvedSession};
use kelson_jobs::{BatchLedger, BatchOps, JobStoreAdmin};
use uuid::Uuid;

use crate::graph_routes::resolve_identity;
use crate::routes::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/batches/{id}", get(status))
        .route("/api/v1/batches/{id}/cancel", post(cancel))
        .route("/api/v1/batches/{id}/retry", post(retry))
        .with_state(state)
}

/// Wave 2 error rendering at this edge (status/view/reportable decisions).
fn error_response(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    err: &AppError,
) -> axum::response::Response {
    state.error_response(err, headers, None)
}

/// Capability gate: an active tenant snapshot that carries `capability`
/// (granted at login — see [`crate::notification_routes::FRAMEWORK_CAPABILITIES`]).
/// Anonymous callers never reach it; a snapshot without the capability is
/// `AUTH_DENIED`.
fn authorize(
    state: &AppState,
    principal: PrincipalId,
    tenant: Option<TenantId>,
    capability: &str,
) -> Result<(), AppError> {
    let tenant =
        tenant.ok_or_else(|| AppError::denied("batch operations require a tenant session"))?;
    let snapshot = state
        .policy
        .snapshot(principal, tenant)
        .ok_or_else(|| AppError::denied("no active access snapshot"))?;
    if !snapshot.capabilities.contains(capability) {
        return Err(AppError::denied(format!("missing capability {capability}")));
    }
    Ok(())
}

/// The gate's payload: the parsed batch id plus the ledger/store pair the
/// operation runs on.
type BatchGate = (Uuid, Arc<dyn BatchLedger>, Arc<dyn JobStoreAdmin>);

/// Shared gate for all three operations: parse the path id (422 on a
/// malformed uuid, before any authorization leaks whether it exists),
/// resolve identity, check the capability, then pair the ledger with the
/// admin store the member states are read from.
fn prepare(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    session: Option<&ResolvedSession>,
    id: &str,
    capability: &str,
) -> Result<BatchGate, AppError> {
    let batch =
        Uuid::parse_str(id).map_err(|e| AppError::validation(format!("invalid batch id: {e}")))?;
    let (principal, tenant) = resolve_identity(state, headers, session)?;
    authorize(state, principal, tenant, capability)?;
    let ledger = state
        .batches
        .clone()
        .ok_or_else(|| AppError::not_implemented("batch ledger (no database pool)"))?;
    let store = state
        .queue
        .clone()
        .ok_or_else(|| AppError::not_implemented("job store admin (no database pool)"))?;
    Ok((batch, ledger, store))
}

/// `batch.status`: progress counts over every member (total / pending /
/// running / succeeded / failed / dead / missing + a finished flag).
async fn status(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<Extension<ResolvedSession>>,
    Path(id): Path<String>,
) -> axum::response::Response {
    let (batch, ledger, store) =
        match prepare(&state, &headers, session.as_deref(), &id, "batches.status") {
            Ok(prepared) => prepared,
            Err(err) => return error_response(&state, &headers, &err),
        };
    match BatchOps::new(ledger.as_ref(), store.as_ref()).status(batch) {
        Ok(progress) => (
            axum::http::StatusCode::OK,
            Json(serde_json::to_value(Envelope::ok(progress)).unwrap_or_default()),
        )
            .into_response(),
        Err(err) => error_response(&state, &headers, &err),
    }
}

/// `batch.cancel`: pending members go dead; running members keep their
/// in-flight claim and terminal members are left alone.
async fn cancel(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<Extension<ResolvedSession>>,
    Path(id): Path<String>,
) -> axum::response::Response {
    let (batch, ledger, store) =
        match prepare(&state, &headers, session.as_deref(), &id, "batches.cancel") {
            Ok(prepared) => prepared,
            Err(err) => return error_response(&state, &headers, &err),
        };
    match BatchOps::new(ledger.as_ref(), store.as_ref()).cancel(batch) {
        Ok(outcome) => (
            axum::http::StatusCode::OK,
            Json(serde_json::to_value(Envelope::ok(outcome)).unwrap_or_default()),
        )
            .into_response(),
        Err(err) => error_response(&state, &headers, &err),
    }
}

/// `batch.retry`: failed/dead members go back to pending — the operation
/// `queue:retry-batch` runs through the same service.
async fn retry(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<Extension<ResolvedSession>>,
    Path(id): Path<String>,
) -> axum::response::Response {
    let (batch, ledger, store) =
        match prepare(&state, &headers, session.as_deref(), &id, "batches.retry") {
            Ok(prepared) => prepared,
            Err(err) => return error_response(&state, &headers, &err),
        };
    match BatchOps::new(ledger.as_ref(), store.as_ref()).retry(batch) {
        Ok(outcome) => (
            axum::http::StatusCode::OK,
            Json(serde_json::to_value(Envelope::ok(outcome)).unwrap_or_default()),
        )
            .into_response(),
        Err(err) => error_response(&state, &headers, &err),
    }
}
