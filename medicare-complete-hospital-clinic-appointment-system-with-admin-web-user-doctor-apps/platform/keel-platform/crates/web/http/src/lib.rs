//! # keel-http
//! MVVM presentation adapter (11-SPEC §6/§20). Transport ONLY:
//! `Controller → RequestViewModel → Command/Query → Execution → Result →
//!  ResponseViewModel → Resource → wire`. The domain never knows HTTP; this crate
//! maps `AppError` to transport concerns at the edge.

pub mod assets;
pub mod cache;
pub mod cookie;
pub mod csrf;
pub mod error_render;
pub mod flash;
pub mod jsonapi;
pub mod jwt;
pub mod middleware;
pub mod middleware_catalog;
pub mod pagination;
pub mod redirect;
pub mod request;
pub mod resource;
pub mod security_headers;
pub mod session_mw;
pub mod signed;
pub mod static_files;
pub mod upload;
pub mod viewmodel;

pub use assets::{asset_url, AssetEntry, AssetManifest};
pub use cache::{etag_for, if_none_match_hits, CacheControlBuilder};
pub use cookie::{derive_cookie_key, Cookie, CookieCipher, CookieJar, CookieQueue, SameSite};
pub use csrf::{session_csrf_token, CsrfGuard, CSRF_COOKIE, CSRF_HEADER};
pub use error_render::{
    problem_json, wants_problem, ErrorRenderPlan, ErrorRenderRegistry, ErrorRenderer, PROBLEM_JSON,
};
pub use flash::{flash_put, flash_take};
pub use jwt::{JwtClaims, JwtHs256};
pub use middleware_catalog::{
    chain_middleware, parse_builtin_id, AuthVia, BuiltinMw, ChainRuntime, EdgeAuthPort,
    MiddlewareCatalog, MwViolation, ResolvedPrincipal, ThrottleGroupConfig, GRAPH_MW_UNKNOWN,
};
pub use middleware::{
    down_file_path, read_down_file, remove_down_file, trace_id_from_traceparent, write_down_file,
    Admission, CorsConfig, MaintenanceFile, MaintenanceGuard, RateLimitConfig, ADMISSION_BYPASS,
};
pub use pagination::{Page, PageQuery, DEFAULT_PER_PAGE, MAX_PER_PAGE};
pub use redirect::Redirect;
pub use request::{accepts, bearer_token, client_ip, expects_json, fingerprint};
pub use resource::{Fields, Resource, ResourceCollection};
pub use security_headers::{security_headers_fn, SecurityHeaders};
pub use session_mw::{
    parse_session_cookie, session_cookie_value, ResolvedSession, SessionMiddleware, SESSION_COOKIE,
};
pub use signed::{derive_signed_key, SignedGate, SignedUrlPort, SignedUrls};
pub use static_files::{is_fingerprinted, StaticAsset, StaticFiles};
pub use viewmodel::{TypedResource, Validated};
pub use upload::{
    image_dimensions, ingest_multipart, ingest_multipart_mixed, sanitize_filename, StoredUpload,
    UploadLimits,
};

use axum::http::StatusCode;
use keel_foundation::{AppError, ErrorCategory};
use serde::{Deserialize, Serialize};

/// Success envelope (public API shape).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope<T> {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

impl<T> Envelope<T> {
    pub fn ok(data: T) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
            meta: None,
        }
    }

    pub fn with_meta(mut self, meta: serde_json::Value) -> Self {
        self.meta = Some(meta);
        self
    }
}

/// Wire error body — public fields only (never `internal`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
    /// Wave 2 `app.debug` detail: redacted internal context attached by the
    /// edge. `None` unless `app.debug = true`, so it never serializes by
    /// default (and never carries an unredacted secret).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<serde_json::Value>,
}

impl ErrorBody {
    pub fn from_app_error(err: &AppError) -> Self {
        Self {
            code: err.code.clone(),
            message: err.public_message.clone(),
            retryable: err.retryable == keel_foundation::Retryability::Retryable,
            meta: if err.meta.is_empty() {
                None
            } else {
                Some(serde_json::Value::Object(err.meta.clone()))
            },
            debug: None,
        }
    }

    /// Merge per-code custom renderer output (Wave 2 registry) into `meta`.
    /// An object extends the map; anything else nests under `detail`.
    pub fn with_detail(mut self, detail: serde_json::Value) -> Self {
        let mut meta = match self.meta.take() {
            Some(serde_json::Value::Object(map)) => map,
            Some(other) => {
                let mut map = serde_json::Map::new();
                map.insert("value".into(), other);
                map
            }
            None => serde_json::Map::new(),
        };
        match detail {
            serde_json::Value::Object(map) => meta.extend(map),
            other => {
                meta.insert("detail".into(), other);
            }
        }
        self.meta = Some(serde_json::Value::Object(meta));
        self
    }

    /// Attach `app.debug` detail (redacted by the edge before it gets here).
    pub fn with_debug(mut self, debug: serde_json::Value) -> Self {
        self.debug = Some(debug);
        self
    }
}

impl<T> Envelope<T> {
    pub fn error(err: &AppError) -> Envelope<serde_json::Value> {
        Envelope::<serde_json::Value> {
            ok: false,
            data: None,
            error: Some(ErrorBody::from_app_error(err)),
            meta: None,
        }
    }
}

/// Map AppError → HTTP status. Transport-only knowledge (domain stays clean).
pub fn status_for_error(err: &AppError) -> StatusCode {
    match err.category {
        ErrorCategory::Validation => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCategory::Authentication => StatusCode::UNAUTHORIZED,
        ErrorCategory::Authorization => StatusCode::FORBIDDEN,
        ErrorCategory::NotFound => StatusCode::NOT_FOUND,
        ErrorCategory::Conflict => StatusCode::CONFLICT,
        ErrorCategory::Workflow => StatusCode::CONFLICT,
        ErrorCategory::Domain => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorCategory::Persistence => StatusCode::SERVICE_UNAVAILABLE,
        ErrorCategory::External => StatusCode::BAD_GATEWAY,
        ErrorCategory::ResourceExhausted => StatusCode::TOO_MANY_REQUESTS,
        ErrorCategory::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Render `(status, envelope)` from an AppError.
pub fn error_response(err: &AppError) -> (StatusCode, axum::Json<Envelope<serde_json::Value>>) {
    (
        status_for_error(err),
        axum::Json(Envelope::<serde_json::Value>::error(err)),
    )
}

/// Request ViewModel: transport contract → validated business input (Form-Request role).
pub trait RequestViewModel: Sized {
    type Input;
    fn validate(self) -> Result<Self::Input, AppError>;
}

/// Response ViewModel: business result → wire representation.
pub trait ResponseViewModel: Sized + Serialize {
    type Source;
    fn from_source(source: &Self::Source) -> Self;
}

/// Health endpoints (transport-level, no DB).
pub async fn healthz() -> &'static str {
    "ok"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_status_mapping() {
        let e = AppError::denied("no");
        assert_eq!(status_for_error(&e), StatusCode::FORBIDDEN);
        let e = AppError::validation("bad");
        assert_eq!(status_for_error(&e), StatusCode::UNPROCESSABLE_ENTITY);
        let e = AppError::cap_limit("limit");
        assert_eq!(status_for_error(&e), StatusCode::TOO_MANY_REQUESTS);
    }

    /// Wave 3 Phase 6: an optimistic-lock conflict (`STALE_RECORD`,
    /// Conflict category) and a generic conflict both map to 409 at the
    /// HTTP edge — no error_render change needed.
    #[test]
    fn conflict_category_maps_to_409() {
        let e = AppError::stale_record("Ledger 1 was modified concurrently");
        assert_eq!(e.code, keel_foundation::codes::STALE_RECORD);
        assert_eq!(e.category, ErrorCategory::Conflict);
        assert_eq!(status_for_error(&e), StatusCode::CONFLICT);
        let e = AppError::conflict("already taken");
        assert_eq!(status_for_error(&e), StatusCode::CONFLICT);
    }

    #[test]
    fn error_body_never_leaks_internal() {
        let e = AppError::internal("secret stack trace").with_meta("op", "x");
        let body = ErrorBody::from_app_error(&e);
        let wire = serde_json::to_string(&body).unwrap();
        assert!(!wire.contains("secret stack trace"));
        assert!(wire.contains("KER_INTERNAL"));
    }
}
