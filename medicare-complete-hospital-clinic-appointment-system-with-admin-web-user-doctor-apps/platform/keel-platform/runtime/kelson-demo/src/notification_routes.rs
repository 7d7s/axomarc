//! Wave 6 P5 notifications surface (19-PARITY "Notifications v2"):
//! session/bearer-authenticated inbox endpoints over the database channel —
//! `GET /api/v1/notifications` (paginated feed + unread count meta) and
//! `POST /api/v1/notifications/read` (mark ids read, capability-gated).
//!
//! Both handlers read through the boot's `NotificationStore` (postgres at
//! full boot, in-memory on degraded hosts — D-52) and are principal-scoped
//! by construction: the store interface only ever accepts the caller's own
//! principal, so one identity can never read or mark another's rows.

use axum::extract::{Extension, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use kelson_foundation::{AppError, PrincipalId, TenantId};
use kelson_http::{Envelope, ResolvedSession};
use kelson_notifications::NotificationRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::graph_routes::resolve_identity;
use crate::routes::AppState;

/// Wave 6 P5: framework (non-module) capabilities granted to authenticated
/// principals at login alongside the graph's `module.capability` pairs.
/// Marking your own notifications read is self-service (Laravel parity: the
/// route is behind `auth`), but 19-PARITY asks for a capability gate — so
/// the check exists, is testable and stays revocable per-snapshot, while
/// the default grant keeps the endpoint usable in production.
/// Wave 6 P9 extends the set with the batch-ledger operations
/// (`batches.status` / `batches.cancel` / `batches.retry`) — the same
/// posture: granted at login, checked per request, revocable per snapshot.
pub(crate) const FRAMEWORK_CAPABILITIES: &[&str] = &[
    "notifications.mark_read",
    "batches.status",
    "batches.cancel",
    "batches.retry",
];

/// Ceiling on one mark-read request (ids ride in a single UPDATE).
const MAX_READ_IDS: usize = 500;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/notifications", get(list))
        .route("/api/v1/notifications/read", post(mark_read))
        .with_state(state)
}

// ------------------------------------------------------------- wire shapes

/// Public row shape: no principal/tenant columns — the feed is
/// self-scoped, and the envelope already implies the identity.
#[derive(serde::Serialize)]
struct NotificationWire<'a> {
    id: String,
    #[serde(rename = "type")]
    kind: &'a str,
    data: &'a serde_json::Value,
    #[serde(with = "time::serde::iso8601::option")]
    read_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::iso8601")]
    created_at: OffsetDateTime,
}

fn wire<'a>(row: &'a NotificationRow) -> NotificationWire<'a> {
    NotificationWire {
        id: row.id.to_string(),
        kind: &row.kind,
        data: &row.data,
        read_at: row.read_at,
        created_at: row.created_at,
    }
}

/// Wave 2 error rendering at this edge (status/view/reportable decisions).
fn error_response(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    err: &AppError,
) -> axum::response::Response {
    state.error_response(err, headers, None)
}

// -------------------------------------------------------------------- list

async fn list(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<Extension<ResolvedSession>>,
    uri: axum::http::Uri,
) -> axum::response::Response {
    let (principal, _tenant) = match resolve_identity(&state, &headers, session.as_deref()) {
        Ok(identity) => identity,
        Err(err) => return error_response(&state, &headers, &err),
    };

    let pairs = crate::graph_routes::query_pairs(uri.query());
    let pair_refs: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let page = match kelson_http::PageQuery::from_pairs(pair_refs) {
        Ok(page) => page,
        Err(err) => return error_response(&state, &headers, &err),
    };

    let feed = match state
        .notifications
        .list(principal, page.offset(), page.limit())
        .await
    {
        Ok(feed) => feed,
        Err(err) => return error_response(&state, &headers, &err),
    };

    let rows: Vec<serde_json::Value> = feed
        .rows
        .iter()
        .map(|row| serde_json::to_value(wire(row)).unwrap_or_default())
        .collect();
    // Stable `Page::meta` block (page/per_page/total/has_more) extended with
    // the unread count the spec ships as meta.
    let meta_page = kelson_http::Page::new(rows, page.page, page.per_page, Some(feed.total));
    let mut meta = meta_page.meta();
    meta["unread"] = serde_json::json!(feed.unread);
    let envelope = Envelope::ok(meta_page.items).with_meta(meta);
    (
        axum::http::StatusCode::OK,
        Json(serde_json::to_value(envelope).unwrap_or_default()),
    )
        .into_response()
}

// ---------------------------------------------------------------- mark read

#[derive(serde::Deserialize)]
struct ReadRequest {
    #[serde(default)]
    ids: Vec<String>,
}

/// Capability gate for read-marking: an active tenant snapshot that carries
/// `notifications.mark_read` (granted at login — see
/// [`FRAMEWORK_CAPABILITIES`]). Anonymous callers never reach it.
fn authorize_mark_read(
    state: &AppState,
    principal: PrincipalId,
    tenant: Option<TenantId>,
) -> Result<(), AppError> {
    let tenant = tenant
        .ok_or_else(|| AppError::denied("notification read-marking requires a tenant session"))?;
    let snapshot = state
        .policy
        .snapshot(principal, tenant)
        .ok_or_else(|| AppError::denied("no active access snapshot"))?;
    if !snapshot.capabilities.contains(FRAMEWORK_CAPABILITIES[0]) {
        return Err(AppError::denied(format!(
            "missing capability {}",
            FRAMEWORK_CAPABILITIES[0]
        )));
    }
    Ok(())
}

async fn mark_read(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<Extension<ResolvedSession>>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let (principal, tenant) = match resolve_identity(&state, &headers, session.as_deref()) {
        Ok(identity) => identity,
        Err(err) => return error_response(&state, &headers, &err),
    };
    if let Err(err) = authorize_mark_read(&state, principal, tenant) {
        return error_response(&state, &headers, &err);
    }

    let request: ReadRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            return error_response(
                &state,
                &headers,
                &AppError::validation("request body must be JSON with an `ids` array"),
            )
        }
    };
    if request.ids.is_empty() {
        return error_response(
            &state,
            &headers,
            &AppError::validation("ids must contain at least one notification id"),
        );
    }
    if request.ids.len() > MAX_READ_IDS {
        return error_response(
            &state,
            &headers,
            &AppError::validation(format!(
                "ids accepts at most {MAX_READ_IDS} entries per request"
            )),
        );
    }
    let mut ids = Vec::with_capacity(request.ids.len());
    for raw in &request.ids {
        match Uuid::parse_str(raw) {
            Ok(id) => ids.push(id),
            Err(_) => {
                return error_response(
                    &state,
                    &headers,
                    &AppError::validation(format!("unknown notification id {raw:?}")),
                )
            }
        }
    }

    // Principal-scoped by construction: the store never accepts another
    // identity's rows, so foreign ids simply report 0 changed.
    match state.notifications.mark_read(principal, ids).await {
        Ok(updated) => (
            axum::http::StatusCode::OK,
            Json(
                serde_json::to_value(Envelope::ok(serde_json::json!({ "updated": updated })))
                    .unwrap_or_default(),
            ),
        )
            .into_response(),
        Err(err) => error_response(&state, &headers, &err),
    }
}
