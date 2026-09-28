//! Upload edge (S7 C5/B-06): bounded multipart intake over the platform
//! plane. Authenticated via session or bearer PAT; files stream into the
//! StoragePort under uploads/<principal uuid>.

use axum::extract::multipart::MultipartRejection;
use axum::extract::{DefaultBodyLimit, Multipart, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use keel_http::{Envelope, ResolvedSession, UploadLimits};

use crate::routes::AppState;

/// Upload route body ceiling (admission pre-check skips multipart; the
/// per-route DefaultBodyLimit + per-file caps keep this bounded).
pub const UPLOAD_BODY_LIMIT: usize = 32 * 1024 * 1024;

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
    match keel_http::ingest_multipart(multipart, &limits, &*state.storage, "uploads", &prefix).await
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
