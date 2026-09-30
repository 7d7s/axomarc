//! Upload edge (S7 C5/B-06): bounded multipart intake over the platform
//! plane. Authenticated via session or bearer PAT; files stream into the
//! StoragePort under uploads/<principal uuid>.

use axum::extract::multipart::MultipartRejection;
use axum::extract::{DefaultBodyLimit, Multipart, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use keel_http::{Envelope, ResolvedSession, UploadLimits};

use crate::routes::AppState;

/// Upload route body ceiling (admission pre-check skips multipart; the
/// per-route DefaultBodyLimit + per-file caps keep this bounded).
pub const UPLOAD_BODY_LIMIT: usize = 32 * 1024 * 1024;

/// Wave 6 P2: `?disk=<name>` selects the storage disk. Absent =
/// `storage.default` through the registry; an unknown name is a 422 (never
/// a silent fallback to another disk).
#[derive(Debug, Default, serde::Deserialize)]
struct UploadQuery {
    #[serde(default)]
    disk: Option<String>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/uploads", post(upload))
        .layer(DefaultBodyLimit::max(UPLOAD_BODY_LIMIT))
        .with_state(state)
}

async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    session: Option<axum::extract::Extension<ResolvedSession>>,
    Query(params): Query<UploadQuery>,
    multipart: Result<Multipart, MultipartRejection>,
) -> impl IntoResponse {
    let identity = match crate::graph_routes::resolve_identity(&state, &headers, session.as_deref())
    {
        Ok((principal, _tenant)) => principal,
        Err(err) => {
            return state.error_json(&err).into_response();
        }
    };
    let multipart = match multipart {
        Ok(mp) => mp,
        Err(err) => {
            let err = keel_foundation::AppError::validation(format!("multipart request required: {err}"));
            return state.error_json(&err).into_response();
        }
    };
    let limits = UploadLimits::default();
    let prefix = format!("{}/", identity.as_uuid());
    // Wave 6 P2: resolve the requested disk before any bytes are read —
    // the registry owns the visibility/port policy per disk.
    let (disk_name, entry) = match state.disks.resolve(params.disk.as_deref()) {
        Ok(resolved) => resolved,
        Err(err) => return state.error_json(&err).into_response(),
    };
    match keel_http::ingest_multipart(
        multipart,
        &limits,
        &*entry.port,
        disk_name,
        "uploads",
        &prefix,
    )
    .await
    {
        Ok(stored) => (
            StatusCode::OK,
            Json(
                serde_json::to_value(Envelope::ok(serde_json::json!({ "files": stored })))
                    .unwrap_or_default(),
            ),
        )
            .into_response(),
        Err(err) => state.error_json(&err).into_response(),
    }
}
