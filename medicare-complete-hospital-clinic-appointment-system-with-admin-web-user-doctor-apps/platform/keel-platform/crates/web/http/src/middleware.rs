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

/// Rate-limit posture: burst capacity + steady refill per second, plus the
/// bucket-map bounds (F-3 Phase 2).
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    pub capacity: u32,
    pub refill_per_sec: f64,
    /// Idle buckets are swept after this long (F-3 Phase 2). `0` disables the
    /// TTL sweep (the map is then bounded by `max_buckets` alone).
    pub bucket_ttl_secs: u64,
    /// Hard cap on tracked client buckets (F-3 Phase 2). When the map is full
    /// the oldest bucket is evicted before a new one is inserted.
    pub max_buckets: usize,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            capacity: 120,
            refill_per_sec: 2.0,
            bucket_ttl_secs: 900,
            max_buckets: 100_000,
        }
    }
}

/// A CIDR range of trusted proxies (F-3 Phase 1). Parsed once at boot; an
/// unparsable range refuses the boot (fail closed) instead of silently
/// widening the trust boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedProxyRange {
    addr: [u8; 16],
    prefix: u8,
    /// `true` for an IPv4 range (the address is stored in the first 4 bytes).
    v4: bool,
}

impl TrustedProxyRange {
    /// Parse `10.0.0.0/8`, `192.168.1.0/24`, `fd00::/8` or a bare address
    /// (treated as a host route, `/32` or `/128`).
    pub fn parse(spec: &str) -> Result<Self, String> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err("empty trusted-proxy range".into());
        }
        let (addr_part, prefix_part) = match spec.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (spec, None),
        };
        let (addr, v4) = parse_ip(addr_part)?;
        let max_prefix = if v4 { 32u8 } else { 128u8 };
        let prefix = match prefix_part {
            None => max_prefix,
            Some(p) => {
                let n: u8 = p
                    .parse()
                    .map_err(|_| format!("trusted-proxy prefix is not a number: {spec}"))?;
                if n > max_prefix {
                    return Err(format!("trusted-proxy prefix out of range: {spec}"));
                }
                n
            }
        };
        Ok(Self { addr, prefix, v4 })
    }

    /// Does `ip` fall inside this range?
    pub fn contains(&self, ip: &str) -> bool {
        let Ok((other, other_v4)) = parse_ip(ip) else {
            return false;
        };
        if other_v4 != self.v4 {
            return false;
        }
        let full = (self.prefix / 8) as usize;
        let rem = self.prefix % 8;
        if full > 0 && self.addr[..full] != other[..full] {
            return false;
        }
        if rem == 0 {
            return true;
        }
        let mask = 0xffu8 << (8 - rem);
        (self.addr[full] & mask) == (other[full] & mask)
    }
}

/// Parse an IPv4 or IPv6 literal into 16 bytes (IPv4 in the first 4).
fn parse_ip(text: &str) -> Result<([u8; 16], bool), String> {
    let text = text.trim();
    if text.contains(':') {
        // IPv6: full form only (8 groups) plus the common `::` compression.
        let (head, tail) = match text.split_once("::") {
            Some((h, t)) => (h, Some(t)),
            None => (text, None),
        };
        let head_groups: Vec<&str> = head.split(':').filter(|s| !s.is_empty()).collect();
        let tail_groups: Vec<&str> = tail
            .map(|t| t.split(':').filter(|s| !s.is_empty()).collect())
            .unwrap_or_default();
        let total = head_groups.len() + tail_groups.len();
        if total > 8 || (tail.is_none() && total != 8) {
            return Err(format!("not an IPv6 address: {text}"));
        }
        let mut groups: Vec<u16> = Vec::with_capacity(8);
        for g in &head_groups {
            groups.push(parse_hex_group(g)?);
        }
        if tail.is_some() {
            groups.resize(8 - tail_groups.len(), 0);
            for g in &tail_groups {
                groups.push(parse_hex_group(g)?);
            }
        }
        let mut out = [0u8; 16];
        for (i, g) in groups.iter().enumerate() {
            out[i * 2] = (g >> 8) as u8;
            out[i * 2 + 1] = (g & 0xff) as u8;
        }
        return Ok((out, false));
    }
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() != 4 {
        return Err(format!("not an IPv4 address: {text}"));
    }
    let mut out = [0u8; 16];
    for (i, p) in parts.iter().enumerate() {
        let n: u8 = p
            .parse()
            .map_err(|_| format!("not an IPv4 address: {text}"))?;
        if n > 255 {
            return Err(format!("not an IPv4 address: {text}"));
        }
        out[i] = n;
    }
    Ok((out, true))
}

fn parse_hex_group(text: &str) -> Result<u16, String> {
    if text.is_empty() || text.len() > 4 {
        return Err(format!("not an IPv6 group: {text}"));
    }
    u16::from_str_radix(text, 16).map_err(|_| format!("not an IPv6 group: {text}"))
}

/// The set of proxy ranges whose `X-Forwarded-For` hops may be trusted
/// (F-3 Phase 1). An empty set trusts nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustedProxies {
    ranges: Vec<TrustedProxyRange>,
}

impl TrustedProxies {
    /// Parse a configured list of CIDR ranges; an unparsable entry refuses the
    /// boot (fail closed).
    pub fn parse(specs: &[String]) -> Result<Self, String> {
        let mut ranges = Vec::with_capacity(specs.len());
        for spec in specs {
            ranges.push(TrustedProxyRange::parse(spec)?);
        }
        Ok(Self { ranges })
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }

    /// Is `ip` inside one of the trusted ranges?
    pub fn contains(&self, ip: &str) -> bool {
        self.ranges.iter().any(|r| r.contains(ip))
    }
}

/// Client key: explicit header first, then bearer credential (S7 B-09
/// per-principal keying without a store lookup at the admission edge), then
/// the forwarded-for chain, then one global bucket.
///
/// The forwarded-for chain is only honoured when the deployment declares
/// trusted proxies (F-3 Phase 1): the chain is walked right-to-left and the
/// first hop that is **not** a trusted proxy is the client. With no trusted
/// proxies configured the header is ignored entirely (a client-supplied header
/// must never decide its own rate-limit bucket).
pub fn client_key(headers: &HeaderMap) -> String {
    client_key_with_proxies(headers, &TrustedProxies::default())
}

/// Client key with an explicit trusted-proxy set (F-3 Phase 1).
pub fn client_key_with_proxies(headers: &HeaderMap, trusted: &TrustedProxies) -> String {
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
    if !trusted.is_empty() {
        if let Some(fwd) = headers.get("x-forwarded-for").and_then(|v| v.to_str().ok()) {
            // Walk right-to-left: skip the hops that our own proxies added, and
            // take the first hop that is not one of them. With an untrusted
            // chain (a client-supplied header) the right-most hop is the
            // client, so the key stays attacker-controlled only when the whole
            // chain is inside the trusted ranges.
            for hop in fwd.split(',').rev() {
                let ip = hop.trim();
                if ip.is_empty() {
                    continue;
                }
                if !trusted.contains(ip) {
                    return ip.to_string();
                }
            }
        }
    }
    "global".into()
}

#[derive(Clone)]
pub(crate) struct RateLimitState {
    config: RateLimitConfig,
    buckets: Arc<Mutex<HashMap<String, BucketState>>>,
    /// The trusted-proxy set the client key is derived against (F-3 Phase 1).
    trusted: TrustedProxies,
}

impl RateLimitState {
    /// Take one token for `key`, sweeping idle buckets and evicting the oldest
    /// bucket when the map is over its cap (F-3 Phase 2).
    fn take(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut buckets = self.buckets.lock().unwrap();
        // TTL sweep: a bucket nobody touched for `bucket_ttl_secs` is dropped.
        // It would have refilled to full capacity anyway, so dropping it only
        // resets the clock — the client keeps its (full) allowance.
        if self.config.bucket_ttl_secs > 0 {
            let ttl = std::time::Duration::from_secs(self.config.bucket_ttl_secs);
            buckets.retain(|_, b| now.duration_since(b.updated_at) < ttl);
        }
        // Hard cap: evict the oldest bucket so the map cannot grow without
        // bound (distinct client keys are attacker-controlled).
        if self.config.max_buckets > 0 && buckets.len() >= self.config.max_buckets {
            if let Some(oldest) = buckets
                .iter()
                .min_by_key(|(_, b)| b.updated_at)
                .map(|(k, _)| k.clone())
            {
                buckets.remove(&oldest);
            }
        }
        let bucket = buckets.entry(key.to_string()).or_insert_with(|| BucketState {
            tokens: self.config.capacity as f64,
            updated_at: now,
        });
        take_from_bucket(
            bucket,
            now,
            self.config.capacity,
            self.config.refill_per_sec,
        )
    }
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
    let key = client_key_with_proxies(req.headers(), &state.trusted);
    let allowed = state.take(&key);
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
    /// Proxy ranges whose `X-Forwarded-For` hops may be trusted when the
    /// client key is derived (F-3 Phase 1). Empty = trust nothing.
    pub trusted_proxies: TrustedProxies,
}

impl Default for Admission {
    fn default() -> Self {
        Self {
            body_limit: 1024 * 1024,
            cors: CorsConfig::default(),
            maintenance: MaintenanceGuard::off(),
            rate_limit: RateLimitConfig::default(),
            trusted_proxies: TrustedProxies::default(),
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
            trusted: self.trusted_proxies.clone(),
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
                ..RateLimitConfig::default()
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
        // Without trusted proxies the forwarded-for header is ignored (F-3
        // Phase 1): a client-supplied header must not pick its own bucket.
        headers.insert("x-forwarded-for", "10.0.0.1, 10.0.0.2".parse().unwrap());
        assert_eq!(client_key(&headers), "global");
        // The explicit key still wins.
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

    // ——— F-3: rate limiter ———

    /// F-3 Phase 1+2 are fixed (trusted-proxy walk, bounded bucket map). What
    /// remains is the **shared store**: the buckets live in the process, so a
    /// multi-instance deployment multiplies the effective limit by the instance
    /// count. This repro records that gap; the fix is a `RateLimitStore` trait
    /// with a Redis backend (design in `LF-011-TO-LF-027-FINDINGS-FIXES.md` §F-3).
    #[test]
    #[ignore = "repro F-3: the buckets are process-local; a shared store is still missing"]
    fn rate_limiter_buckets_are_process_local() {
        // The state is an in-process Arc<Mutex<HashMap<..>>> — two app
        // instances would each keep their own buckets.
        let state = RateLimitState {
            config: RateLimitConfig::default(),
            buckets: Arc::new(Mutex::new(HashMap::new())),
            trusted: TrustedProxies::default(),
        };
        state.take("client-a");
        assert_eq!(state.buckets.lock().unwrap().len(), 1);
        // A second "instance" (a second state) starts empty — the two do not
        // share their buckets.
        let other = RateLimitState {
            config: RateLimitConfig::default(),
            buckets: Arc::new(Mutex::new(HashMap::new())),
            trusted: TrustedProxies::default(),
        };
        other.take("client-a");
        assert_eq!(other.buckets.lock().unwrap().len(), 1);
        assert_eq!(
            state.buckets.lock().unwrap().len(),
            1,
            "each instance keeps its own bucket for the same client"
        );
    }

    // ——— F-3 Phase 1: trusted proxies ———

    /// With no trusted proxies configured, the `X-Forwarded-For` header is
    /// ignored entirely — a client-supplied header must never decide its own
    /// rate-limit bucket.
    #[test]
    fn client_key_ignores_forwarded_for_without_trusted_proxies() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "192.168.1.100, 10.0.0.5".parse().unwrap());
        assert_eq!(
            client_key_with_proxies(&headers, &TrustedProxies::default()),
            "global",
            "an untrusted forwarded-for header decides nothing"
        );
    }

    /// With trusted proxies, the chain is walked right-to-left and the first
    /// hop that is not a trusted proxy is the client.
    #[test]
    fn client_key_walks_the_chain_past_trusted_proxies() {
        let trusted = TrustedProxies::parse(&["10.0.0.0/8".to_string()]).expect("parses");
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            "203.0.113.7, 10.0.0.5, 10.0.0.9".parse().unwrap(),
        );
        assert_eq!(
            client_key_with_proxies(&headers, &trusted),
            "203.0.113.7",
            "the first untrusted hop (right-to-left) is the client"
        );
    }

    /// An explicit key and a bearer credential still outrank the chain.
    #[test]
    fn client_key_explicit_and_bearer_beat_the_chain() {
        let trusted = TrustedProxies::parse(&["10.0.0.0/8".to_string()]).expect("parses");
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "203.0.113.7, 10.0.0.5".parse().unwrap());
        headers.insert("x-rate-key", "api-client-9".parse().unwrap());
        assert_eq!(client_key_with_proxies(&headers, &trusted), "api-client-9");
        headers.remove("x-rate-key");
        headers.insert(
            header::AUTHORIZATION,
            "Bearer pat-secret-token".parse().unwrap(),
        );
        assert!(client_key_with_proxies(&headers, &trusted).starts_with("bearer:"));
    }

    /// The CIDR parser handles IPv4 and IPv6 ranges and refuses garbage.
    #[test]
    fn trusted_proxy_ranges_parse_and_match() {
        let v4 = TrustedProxyRange::parse("10.0.0.0/8").expect("parses");
        assert!(v4.contains("10.1.2.3"));
        assert!(!v4.contains("11.1.2.3"));
        let v4_24 = TrustedProxyRange::parse("192.168.1.0/24").expect("parses");
        assert!(v4_24.contains("192.168.1.255"));
        assert!(!v4_24.contains("192.168.2.1"));
        let host = TrustedProxyRange::parse("203.0.113.7").expect("parses");
        assert!(host.contains("203.0.113.7"));
        assert!(!host.contains("203.0.113.8"));
        let v6 = TrustedProxyRange::parse("fd00::/8").expect("parses");
        assert!(v6.contains("fd00::1"));
        assert!(!v6.contains("fe80::1"));
        assert!(TrustedProxyRange::parse("10.0.0.0/33").is_err());
        assert!(TrustedProxyRange::parse("not-an-ip").is_err());
        assert!(TrustedProxyRange::parse("").is_err());
    }

    // ——— F-3 Phase 2: bucket eviction ———

    /// The bucket map is bounded: inserting more distinct keys than the cap
    /// evicts the oldest instead of growing without limit.
    #[test]
    fn rate_limit_buckets_are_capped() {
        let state = RateLimitState {
            config: RateLimitConfig {
                capacity: 10,
                refill_per_sec: 1.0,
                bucket_ttl_secs: 0,
                max_buckets: 4,
            },
            buckets: Arc::new(Mutex::new(HashMap::new())),
            trusted: TrustedProxies::default(),
        };
        for i in 0..50 {
            state.take(&format!("client-{i}"));
        }
        let len = state.buckets.lock().unwrap().len();
        assert!(len <= 4, "the bucket map stays capped: {len}");
    }

    /// Idle buckets are swept by the TTL, so a burst of one-off client keys
    /// does not keep the map populated forever.
    #[test]
    fn rate_limit_buckets_are_swept_by_ttl() {
        let state = RateLimitState {
            config: RateLimitConfig {
                capacity: 10,
                refill_per_sec: 1.0,
                bucket_ttl_secs: 1,
                max_buckets: 0,
            },
            buckets: Arc::new(Mutex::new(HashMap::new())),
            trusted: TrustedProxies::default(),
        };
        state.take("client-a");
        assert_eq!(state.buckets.lock().unwrap().len(), 1);
        // Age the bucket past the TTL, then take a different key: the sweep
        // drops the idle one.
        {
            let mut buckets = state.buckets.lock().unwrap();
            let b = buckets.get_mut("client-a").expect("bucket");
            b.updated_at = Instant::now() - std::time::Duration::from_secs(5);
        }
        state.take("client-b");
        let len = state.buckets.lock().unwrap().len();
        assert_eq!(len, 1, "the idle bucket was swept");
    }
}
