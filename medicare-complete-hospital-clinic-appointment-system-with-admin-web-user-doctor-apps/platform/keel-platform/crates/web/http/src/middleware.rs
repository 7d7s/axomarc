//! Admission middleware pipeline (S2 item 25, L06 + L67).
//!
//! Fixed order (request flow):
//!   request-id -> trace -> body-limit -> CORS -> maintenance -> rate-limit -> catch-panic -> router
//!
//! axum runs the LAST-attached layer first, so Admission::apply attaches layers
//! in reverse. Rejections keep the public Envelope shape; CORS preflight is
//! answered before maintenance/rate-limit so preflights never get blocked.
//! The catch-panic boundary is the INNERMOST layer: a handler panic becomes a
//! redacted 500 envelope, and the outer layers still enrich that response.
//! Rate limiting is a hand-rolled token bucket (no dependency, D-62).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{header, HeaderMap, Request, Response, StatusCode};
use axum::middleware::Next;
use axum::response::IntoResponse;
use axum::Json;
use kelson_foundation::{codes, AppError, ErrorCategory, RequestId, Retryability, Severity, TraceId};
use serde::{Deserialize, Serialize};
use tower_http::catch_panic::CatchPanicLayer;

use crate::{Envelope, ErrorBody};

/// Paths that always bypass maintenance + rate limiting (liveness probes).
pub const ADMISSION_BYPASS: &[&str] = &["/healthz"];

// ---------------------------------------------------------------- request-id

fn request_id_from_headers(headers: &HeaderMap) -> RequestId {
    headers
        .get(header::HeaderName::from_static("x-request-id"))
        .and_then(|v| v.to_str().ok())
        .filter(|s| (1..=64).contains(&s.len()))
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .map(RequestId::from_uuid)
        .unwrap_or_else(RequestId::new)
}

pub(crate) async fn request_id_fn(mut req: Request<Body>, next: Next) -> Response<Body> {
    let id = request_id_from_headers(req.headers());
    req.extensions_mut().insert(id);
    let mut res = next.run(req).await;
    if let Ok(value) = id.to_string().parse() {
        res.headers_mut()
            .insert(header::HeaderName::from_static("x-request-id"), value);
    }
    res
}

// --------------------------------------------------------------------- trace

/// Parse the trace-id segment out of a W3C traceparent: 00-<32hex>-<16hex>-<2hex>.
pub fn trace_id_from_traceparent(value: &str) -> Option<TraceId> {
    let mut parts = value.split('-');
    let version = parts.next()?;
    let trace = parts.next()?;
    if version != "00" || trace.len() != 32 || !trace.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let bytes = hex_bytes(trace)?;
    Some(TraceId::from_uuid(uuid::Uuid::from_slice(&bytes).ok()?))
}

fn hex_bytes(s: &str) -> Option<[u8; 16]> {
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

pub(crate) async fn trace_fn(mut req: Request<Body>, next: Next) -> Response<Body> {
    let id = req
        .headers()
        .get("traceparent")
        .and_then(|v| v.to_str().ok())
        .and_then(trace_id_from_traceparent)
        .unwrap_or_default();
    req.extensions_mut().insert(id);
    let mut res = next.run(req).await;
    if let Ok(value) = id.to_string().parse() {
        res.headers_mut()
            .insert(header::HeaderName::from_static("x-trace-id"), value);
    }
    res
}

// ---------------------------------------------------------------- body limit

pub(crate) async fn body_limit_fn(
    State(limit): State<usize>,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    // Multipart uploads are bounded per-route (DefaultBodyLimit) and by the
    // upload pipeline's own caps; the JSON pre-check stays at limit.
    let is_multipart = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("multipart/form-data"));
    let too_big = !is_multipart
        && req
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<usize>().ok())
            .is_some_and(|len| len > limit);
    if too_big {
        let err = AppError::validation(format!("request body exceeds the {limit} byte limit"));
        return (
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(Envelope::<serde_json::Value>::error(&err)),
        )
            .into_response();
    }
    next.run(req).await
}

// --------------------------------------------------------------- maintenance

/// Content of the down file (storage/framework/down), Laravel-shaped.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenanceFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "time::serde::iso8601::option"
    )]
    pub started_at: Option<time::OffsetDateTime>,
}

impl Default for MaintenanceFile {
    fn default() -> Self {
        Self {
            retry_secs: None,
            secret: None,
            started_at: Some(time::OffsetDateTime::now_utc()),
        }
    }
}

/// Conventional location of the down marker, relative to a workspace root.
pub fn down_file_path(root: &Path) -> PathBuf {
    root.join("storage/framework/down")
}

pub fn read_down_file(root: &Path) -> Option<MaintenanceFile> {
    let raw = std::fs::read_to_string(down_file_path(root)).ok()?;
    serde_json::from_str(&raw)
        .ok()
        .or(Some(MaintenanceFile::default()))
}

pub fn write_down_file(root: &Path, file: &MaintenanceFile) -> Result<(), AppError> {
    let path = down_file_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::internal(format!("cannot create {}: {e}", parent.display())))?;
    }
    let raw = serde_json::to_string_pretty(file)
        .map_err(|e| AppError::internal(format!("down file: {e}")))?;
    std::fs::write(&path, raw)
        .map_err(|e| AppError::internal(format!("cannot write {}: {e}", path.display())))
}

pub fn remove_down_file(root: &Path) -> Result<(), AppError> {
    let path = down_file_path(root);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(AppError::internal(format!("cannot remove down file: {e}"))),
    }
}

/// Live maintenance posture: in-memory (tests) or file-backed (up/down CLI).
#[derive(Clone)]
pub enum MaintenanceGuard {
    Memory(Arc<RwLock<Option<MaintenanceFile>>>),
    File(PathBuf),
}

impl MaintenanceGuard {
    pub fn off() -> Self {
        Self::Memory(Arc::new(RwLock::new(None)))
    }

    pub fn file(root: impl Into<PathBuf>) -> Self {
        Self::File(root.into())
    }

    pub fn active(&self) -> Option<MaintenanceFile> {
        match self {
            Self::Memory(state) => state.read().ok()?.clone(),
            Self::File(root) => read_down_file(root),
        }
    }

    /// Memory-mode control (no-op for file-backed guards).
    pub fn set_memory(&self, file: Option<MaintenanceFile>) {
        if let Self::Memory(state) = self {
            if let Ok(mut guard) = state.write() {
                *guard = file;
            }
        }
    }
}

pub(crate) async fn maintenance_fn(
    State(guard): State<MaintenanceGuard>,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    let path = req.uri().path().to_string();
    if ADMISSION_BYPASS.iter().any(|p| *p == path) {
        return next.run(req).await;
    }
    let Some(file) = guard.active() else {
        return next.run(req).await;
    };
    let bypassed = file.secret.as_deref().is_some_and(|secret| {
        req.headers()
            .get(header::HeaderName::from_static("x-maintenance-secret"))
            .and_then(|v| v.to_str().ok())
            .is_some_and(|provided| provided == secret)
            || req
                .uri()
                .query()
                .is_some_and(|q| url_query_contains(q, "secret", secret))
    });
    if bypassed {
        return next.run(req).await;
    }
    let err = AppError::new(
        codes::MAINTENANCE,
        ErrorCategory::Internal,
        Severity::Low,
        Retryability::Retryable,
        "service is temporarily offline for maintenance",
    );
    let body = ErrorBody::from_app_error(&err);
    let mut res = (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(Envelope::<serde_json::Value> {
            ok: false,
            data: None,
            error: Some(body),
            meta: None,
        }),
    )
        .into_response();
    if let Some(retry) = file.retry_secs {
        if let Ok(value) = retry.to_string().parse() {
            res.headers_mut().insert(header::RETRY_AFTER, value);
        }
    }
    res
}

fn url_query_contains(query: &str, key: &str, expected: &str) -> bool {
    query.split('&').any(|pair| {
        let mut kv = pair.splitn(2, '=');
        kv.next() == Some(key) && kv.next() == Some(expected)
    })
}

// ---------------------------------------------------------------- rate limit

/// Hand-rolled token bucket state (one per client key).
#[derive(Debug, Clone)]
pub struct BucketState {
    pub tokens: f64,
    pub updated_at: Instant,
}

/// Pure token-bucket step: refill by elapsed time, take one token if available.
pub fn take_from_bucket(
    state: &mut BucketState,
    now: Instant,
    capacity: u32,
    refill_per_sec: f64,
) -> bool {
    let elapsed = now
        .saturating_duration_since(state.updated_at)
        .as_secs_f64();
    let tokens = (state.tokens + elapsed * refill_per_sec).min(capacity as f64);
    state.updated_at = now;
    if tokens >= 1.0 {
        state.tokens = tokens - 1.0;
        true
    } else {
        state.tokens = tokens;
        false
    }
}

/// Rate-limit posture: burst capacity + steady refill per second.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub capacity: u32,
    pub refill_per_sec: f64,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            capacity: 120,
            refill_per_sec: 2.0,
        }
    }
}

/// Client key: explicit header first, then bearer credential (S7 B-09
/// per-principal keying without a store lookup at the admission edge), then
/// forwarded-for, then one global bucket.
pub fn client_key(headers: &HeaderMap) -> String {
    if let Some(key) = headers
        .get(header::HeaderName::from_static("x-rate-key"))
        .and_then(|v| v.to_str().ok())
    {
        if !key.is_empty() && key.len() <= 128 {
            return key.to_string();
        }
    }
    if let Some(auth) = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
    {
        if let Some(token) = auth
            .strip_prefix("Bearer ")
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            use sha2::{Digest, Sha256};
            let digest = Sha256::digest(token.as_bytes());
            let mut key = String::with_capacity(23);
            key.push_str("bearer:");
            for b in &digest[..8] {
                key.push_str(&format!("{b:02x}"));
            }
            return key;
        }
    }
    if let Some(fwd) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
        if let Some(first) = fwd.split(',').next() {
            let ip = first.trim();
            if !ip.is_empty() {
                return ip.to_string();
            }
        }
    }
    "global".into()
}

#[derive(Clone)]
pub(crate) struct RateLimitState {
    config: RateLimitConfig,
    buckets: Arc<Mutex<HashMap<String, BucketState>>>,
}

pub(crate) async fn rate_limit_fn(
    State(state): State<RateLimitState>,
    req: Request<Body>,
    next: Next,
) -> Response<Body> {
    let path = req.uri().path().to_string();
    if ADMISSION_BYPASS.iter().any(|p| *p == path) {
        return next.run(req).await;
    }
    let key = client_key(req.headers());
    let allowed = {
        let mut buckets = state.buckets.lock().unwrap();
        let bucket = buckets.entry(key).or_insert_with(|| BucketState {
            tokens: state.config.capacity as f64,
            updated_at: Instant::now(),
        });
        take_from_bucket(
            bucket,
            Instant::now(),
            state.config.capacity,
            state.config.refill_per_sec,
        )
    };
    if !allowed {
        let err = AppError::cap_limit("rate limit exceeded; retry shortly");
        let mut res = (
            StatusCode::TOO_MANY_REQUESTS,
            Json(Envelope::<serde_json::Value>::error(&err)),
        )
            .into_response();
        res.headers_mut()
            .insert(header::RETRY_AFTER, "1".parse().unwrap());
        return res;
    }
    next.run(req).await
}

// ------------------------------------------------------------------ pipeline

/// CORS posture for the admission pipeline (dev default: permissive).
#[derive(Debug, Clone)]
pub struct CorsConfig {
    pub allow_origins: Vec<String>,
    pub allow_methods: Vec<String>,
    pub allow_headers: Vec<String>,
    pub max_age_secs: u64,
    /// Allow credentials (cookies, auth headers). Requires explicit origins
    /// (not `*`) — `*` + credentials is browser-invalid and tower-http
    /// refuses that combination at layer construction.
    pub allow_credentials: bool,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allow_origins: vec!["*".into()],
            allow_methods: vec![
                "GET".into(),
                "POST".into(),
                "PUT".into(),
                "PATCH".into(),
                "DELETE".into(),
                "OPTIONS".into(),
            ],
            allow_headers: vec![
                "content-type".into(),
                "authorization".into(),
                "x-request-id".into(),
                "x-rate-key".into(),
                "x-maintenance-secret".into(),
                "x-tenant".into(),
            ],
            max_age_secs: 3600,
            allow_credentials: false,
        }
    }
}

impl CorsConfig {
    fn layer(&self) -> tower_http::cors::CorsLayer {
        use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, Any, CorsLayer};
        // Fail fast at boot: `*` + credentials is browser-invalid, and
        // tower-http would only refuse it at request time (as a panic in the
        // request path). A deployment must learn about it at startup.
        assert!(
            !(self.allow_credentials && self.allow_origins.iter().any(|o| o == "*")),
            "CORS: allow_credentials requires explicit origins, not the `*` wildcard"
        );
        let origins: AllowOrigin = if self.allow_origins.iter().any(|o| o == "*") {
            Any.into()
        } else {
            let list: Vec<axum::http::HeaderValue> = self
                .allow_origins
                .iter()
                .filter_map(|o| o.parse().ok())
                .collect();
            AllowOrigin::list(list)
        };
        let methods: Vec<axum::http::Method> = self
            .allow_methods
            .iter()
            .filter_map(|m| m.parse().ok())
            .collect();
        let headers: Vec<header::HeaderName> = self
            .allow_headers
            .iter()
            .filter_map(|h| h.parse().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(AllowMethods::list(methods))
            .allow_headers(AllowHeaders::list(headers))
            .max_age(std::time::Duration::from_secs(self.max_age_secs))
            .allow_credentials(self.allow_credentials)
    }
}

/// Admission posture. Cheap to clone (everything is Arc-shared).
#[derive(Clone)]
pub struct Admission {
    pub body_limit: usize,
    pub cors: CorsConfig,
    pub maintenance: MaintenanceGuard,
    pub rate_limit: RateLimitConfig,
}

impl Default for Admission {
    fn default() -> Self {
        Self {
            body_limit: 1024 * 1024,
            cors: CorsConfig::default(),
            maintenance: MaintenanceGuard::off(),
            rate_limit: RateLimitConfig::default(),
        }
    }
}

impl Admission {
    /// Wrap a router with the fixed admission order (item 25).
    ///
    /// The panic boundary is the INNERMOST layer: a panic in a handler (or in
    /// the router itself) is caught there and turned into a redacted 500
    /// envelope, and the outer layers (request-id, trace, body-limit, CORS,
    /// maintenance, rate-limit) still run their post-processing on that
    /// response — so the caught 500 keeps its `x-request-id` / `x-trace-id`
    /// headers and the CORS posture.
    pub fn apply(self, router: axum::Router) -> axum::Router {
        let rate_state = RateLimitState {
            config: self.rate_limit.clone(),
            buckets: Arc::new(Mutex::new(HashMap::new())),
        };
        router
            // Panic boundary (innermost): catches a handler panic and answers a
            // redacted 500 envelope. The panic payload never reaches the client.
            .layer(CatchPanicLayer::custom(|err: Box<dyn std::any::Any + Send>| {
                // Log the panic server-side (never the payload to the client).
                let msg = err
                    .downcast_ref::<String>()
                    .map(|s| s.as_str())
                    .or_else(|| err.downcast_ref::<&str>().copied())
                    .unwrap_or("internal server error");
                tracing::error!(panic = %msg, "handler panicked; answering 500");
                let app_err = AppError::internal("internal server error");
                let env = Envelope::<()>::error(&app_err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(env),
                )
                    .into_response()
            }))
            .layer(axum::middleware::from_fn_with_state(
                rate_state,
                rate_limit_fn,
            ))
            .layer(axum::middleware::from_fn_with_state(
                self.maintenance.clone(),
                maintenance_fn,
            ))
            .layer(self.cors.layer())
            .layer(DefaultBodyLimit::max(self.body_limit))
            .layer(axum::middleware::from_fn_with_state(
                self.body_limit,
                body_limit_fn,
            ))
            .layer(axum::middleware::from_fn(trace_fn))
            .layer(axum::middleware::from_fn(request_id_fn))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    fn app(admission: Admission) -> Router {
        let router = Router::new()
            .route("/", get(|| async { "root" }))
            .route("/healthz", get(|| async { "ok" }));
        admission.apply(router)
    }

    fn req(method: &str, uri: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap()
    }

    async fn body_string(body: Body) -> String {
        let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn request_id_generated_and_echoed() {
        let res = app(Admission::default())
            .oneshot(req("GET", "/"))
            .await
            .unwrap();
        assert!(res.headers().contains_key("x-request-id"));
        assert!(res.headers().contains_key("x-trace-id"));
    }

    #[tokio::test]
    async fn provided_request_id_reused() {
        let id = uuid::Uuid::now_v7().to_string();
        let mut r = req("GET", "/");
        r.headers_mut().insert("x-request-id", id.parse().unwrap());
        let res = app(Admission::default()).oneshot(r).await.unwrap();
        assert_eq!(
            res.headers().get("x-request-id").unwrap().to_str().unwrap(),
            id
        );
    }

    #[tokio::test]
    async fn traceparent_parsed() {
        let raw = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let id = trace_id_from_traceparent(raw).expect("parses");
        let expected =
            uuid::Uuid::from_slice(&hex_bytes("4bf92f3577b34da6a3ce929d0e0e4736").unwrap())
                .unwrap();
        assert_eq!(id.to_string(), expected.to_string());
        assert!(trace_id_from_traceparent("00-short").is_none());
        assert!(trace_id_from_traceparent(
            "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"
        )
        .is_none());
    }

    #[tokio::test]
    async fn body_limit_rejects_oversized_content_length() {
        let mut r = Request::builder()
            .method("POST")
            .uri("/")
            .body(Body::empty())
            .unwrap();
        r.headers_mut()
            .insert(header::CONTENT_LENGTH, "99999999".parse().unwrap());
        let admission = Admission {
            body_limit: 1024,
            ..Admission::default()
        };
        let res = app(admission).oneshot(r).await.unwrap();
        assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body = body_string(res.into_body()).await;
        assert!(body.contains("KSN_VALIDATION"));
    }

    #[tokio::test]
    async fn maintenance_blocks_with_retry_after_and_secret_bypass() {
        let guard = MaintenanceGuard::off();
        guard.set_memory(Some(MaintenanceFile {
            retry_secs: Some(30),
            secret: Some("open-sesame".into()),
            started_at: None,
        }));
        let admission = Admission {
            maintenance: guard.clone(),
            ..Admission::default()
        };

        let res = app(admission.clone())
            .oneshot(req("GET", "/"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(res.headers().get("retry-after").unwrap(), "30");
        let body = body_string(res.into_body()).await;
        assert!(body.contains("KSN_MAINTENANCE"));

        let res = app(admission.clone())
            .oneshot(req("GET", "/healthz"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let mut r = req("GET", "/");
        r.headers_mut()
            .insert("x-maintenance-secret", "open-sesame".parse().unwrap());
        let res = app(admission).oneshot(r).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn rate_limit_exhaustion_returns_429() {
        let admission = Admission {
            rate_limit: RateLimitConfig {
                capacity: 2,
                refill_per_sec: 0.0,
            },
            ..Admission::default()
        };
        let router = app(admission);
        let call = |router: Router| {
            let mut r = req("GET", "/");
            r.headers_mut()
                .insert("x-rate-key", "client-a".parse().unwrap());
            router.oneshot(r)
        };
        let r1 = call(router.clone()).await.unwrap();
        assert_eq!(r1.status(), StatusCode::OK);
        let r2 = call(router.clone()).await.unwrap();
        assert_eq!(r2.status(), StatusCode::OK);
        let r3 = call(router.clone()).await.unwrap();
        assert_eq!(r3.status(), StatusCode::TOO_MANY_REQUESTS);
        let rh = router.oneshot(req("GET", "/healthz")).await.unwrap();
        assert_eq!(rh.status(), StatusCode::OK);
    }

    #[test]
    fn token_bucket_math() {
        let t0 = Instant::now();
        let mut bucket = BucketState {
            tokens: 1.0,
            updated_at: t0,
        };
        assert!(take_from_bucket(&mut bucket, t0, 2, 1.0));
        assert!(!take_from_bucket(&mut bucket, t0, 2, 1.0));
        assert!(take_from_bucket(
            &mut bucket,
            t0 + std::time::Duration::from_secs(1),
            2,
            1.0
        ));
    }

    #[test]
    fn client_key_extraction() {
        let mut headers = HeaderMap::new();
        assert_eq!(client_key(&headers), "global");
        headers.insert("x-forwarded-for", "10.0.0.1, 10.0.0.2".parse().unwrap());
        assert_eq!(client_key(&headers), "10.0.0.1");
        headers.insert("x-rate-key", "api-client-9".parse().unwrap());
        assert_eq!(client_key(&headers), "api-client-9");
    }

    #[test]
    fn client_key_prefers_bearer_over_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "10.0.0.1".parse().unwrap());
        headers.insert(
            header::AUTHORIZATION,
            "Bearer pat-secret-token".parse().unwrap(),
        );
        let key = client_key(&headers);
        assert!(key.starts_with("bearer:"), "bearer key wins: {key}");
        assert_eq!(key.len(), 23);
        // Same credential -> same bucket regardless of IP.
        headers.insert("x-forwarded-for", "10.9.9.9".parse().unwrap());
        assert_eq!(client_key(&headers), key);
        // Different credential -> different bucket.
        headers.insert(header::AUTHORIZATION, "Bearer other-token".parse().unwrap());
        assert_ne!(client_key(&headers), key);
    }

    #[test]
    fn down_file_roundtrip() {
        let dir = std::env::temp_dir().join(format!("kelson-down-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(read_down_file(&dir).is_none());
        let file = MaintenanceFile {
            retry_secs: Some(60),
            secret: None,
            started_at: None,
        };
        write_down_file(&dir, &file).unwrap();
        let read = read_down_file(&dir).unwrap();
        assert_eq!(read.retry_secs, Some(60));
        remove_down_file(&dir).unwrap();
        assert!(read_down_file(&dir).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    // ——— F-2: panic boundary ———

    /// A panicking handler answers a redacted 500 envelope through the whole
    /// admission chain — the request-id and trace-id headers still land on the
    /// caught response, and the panic payload never reaches the client.
    #[tokio::test]
    async fn panicking_handler_answers_a_500_envelope() {
        let router = Router::new().route("/boom", get(|| async {
            panic!("handler exploded: super-secret-token-42");
            "never reached"
        }));
        let res = Admission::default()
            .apply(router)
            .oneshot(req("GET", "/boom"))
            .await
            .expect("request serves");
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert!(
            res.headers().contains_key("x-request-id"),
            "the caught 500 still carries the admission request id"
        );
        assert!(
            res.headers().contains_key("x-trace-id"),
            "the caught 500 still carries the admission trace id"
        );
        let body = body_string(res.into_body()).await;
        assert!(body.contains("KSN_INTERNAL"), "envelope: {body}");
        assert!(
            !body.contains("handler exploded") && !body.contains("super-secret-token-42"),
            "the panic payload must never reach the client: {body}"
        );
    }

    // ——— F-1: CORS allowlist ———

    /// The default CORS posture is a permissive wildcard (dev default) and the
    /// platform config can now override it with an allowlist.
    #[test]
    fn cors_default_is_a_permissive_wildcard() {
        let cors = CorsConfig::default();
        assert_eq!(cors.allow_origins, vec!["*".to_string()]);
        assert!(!cors.allow_credentials, "wildcard + credentials is invalid");
    }

    /// An allowlist denies an origin that is not on it.
    #[tokio::test]
    async fn cors_allowlist_rejects_disallowed_origin() {
        let router = Router::new().route("/", get(|| async { "ok" }));
        let admission = Admission {
            cors: CorsConfig {
                allow_origins: vec!["https://app.example.com".into()],
                ..CorsConfig::default()
            },
            ..Admission::default()
        };
        let mut r = req("GET", "/");
        r.headers_mut()
            .insert("origin", "https://evil.com".parse().unwrap());
        let res = admission.apply(router).oneshot(r).await.unwrap();
        assert!(!res
            .headers()
            .contains_key("access-control-allow-origin"));
    }

    /// An allowlist echoes the allowed origin.
    #[tokio::test]
    async fn cors_allowlist_allows_configured_origin() {
        let router = Router::new().route("/", get(|| async { "ok" }));
        let admission = Admission {
            cors: CorsConfig {
                allow_origins: vec!["https://app.example.com".into()],
                ..CorsConfig::default()
            },
            ..Admission::default()
        };
        let mut r = req("GET", "/");
        r.headers_mut()
            .insert("origin", "https://app.example.com".parse().unwrap());
        let res = admission.apply(router).oneshot(r).await.unwrap();
        assert_eq!(
            res.headers()
                .get("access-control-allow-origin")
                .unwrap()
                .to_str()
                .unwrap(),
            "https://app.example.com"
        );
        assert!(res.headers().contains_key("vary"), "Vary: Origin is set");
    }

    /// The wildcard posture still answers `*` and never sets credentials.
    #[tokio::test]
    async fn cors_wildcard_without_credentials() {
        let router = Router::new().route("/", get(|| async { "ok" }));
        let mut r = req("GET", "/");
        r.headers_mut()
            .insert("origin", "https://any-origin.com".parse().unwrap());
        let res = Admission::default().apply(router).oneshot(r).await.unwrap();
        assert_eq!(
            res.headers()
                .get("access-control-allow-origin")
                .unwrap()
                .to_str()
                .unwrap(),
            "*"
        );
        assert!(!res
            .headers()
            .contains_key("access-control-allow-credentials"));
    }

    /// The deployment learns at boot, not at request time, that the
    /// browser-invalid `*` + credentials combination is refused.
    #[test]
    #[should_panic(expected = "allow_credentials requires explicit origins")]
    fn cors_wildcard_with_credentials_is_refused() {
        let cors = CorsConfig {
            allow_origins: vec!["*".into()],
            allow_credentials: true,
            ..CorsConfig::default()
        };
        let _ = cors.layer();
    }

    // ——— F-3: rate limiter gaps (recorded, fix pending) ———

    /// Repro F-3: the client key trusts the first `X-Forwarded-For` hop with no
    /// trusted-proxy configuration, the bucket map has no eviction, and the
    /// buckets are process-local. Asserts the current behaviour; the fix adds
    /// `[http.trusted_proxies]` + bucket eviction.
    #[test]
    #[ignore = "repro F-3: first XFF hop trusted, no eviction, no shared store"]
    fn rate_limiter_gaps_first_xff_trusted_unbounded_no_shared() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "192.168.1.100, 10.0.0.5".parse().unwrap());
        assert_eq!(
            client_key(&headers),
            "192.168.1.100",
            "first XFF hop is trusted without a proxy-trust config"
        );
    }
}
