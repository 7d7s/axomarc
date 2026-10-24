//! # kelson-http-client
//! Outbound HTTP client port (blueprint L44, S3). The platform TALKS to the
//! outside world through this port so that timeout, retry and redaction
//! policy is central + testable instead of scattered per adapter. Inbound
//! traffic uses axum (kelson-http); this crate never touches it.
//!
//! Behavior matrix (spec §31 external dependencies):
//! - hard timeout on every attempt (`KSN_DEADLINE`-shaped external error),
//! - retries with exponential backoff on 429/5xx and connect errors only
//!   (4xx are caller bugs — never retried),
//! - bodies are never logged; configured headers are redacted in errors.
//!
//! Wave 6 P10 (parity: `retry { times, base_ms, max_ms, on }`, timeout,
//! pool, tap, fake, sink): `retry_max_ms`/`retry_on` configure the
//! schedule and the retried conditions, `HttpRequest::timeout_ms`
//! overrides the client timeout per request, [`execute_pool`] fans out
//! over a bounded tokio join-set, [`HttpTap`] observes every attempt with
//! redacted headers, [`HttpFake`] asserts recorded requests in tests, and
//! `ReqwestHttpClient::sink_to` streams a response body into a
//! `kelson_storage::StoragePort` writer without buffering it whole.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use kelson_foundation::{AppError, ErrorCategory, Retryability};
use serde::{Deserialize, Serialize};

pub mod egress;
pub use egress::EgressPolicy;

/// Boxed future used across the object-safe port surface.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Outbound request value (wire-neutral; the adapter maps it).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(default, with = "serde_bytes_b64")]
    pub body: Vec<u8>,
    /// Wave 6 P10: per-request timeout override (milliseconds). `None`
    /// inherits the client's `timeout_ms`. Serde-default and skipped when
    /// absent, so pre-P10 payloads stay byte-identical on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

impl HttpRequest {
    pub fn json(method: &str, url: &str, body: &impl Serialize) -> Result<Self, AppError> {
        let mut headers = std::collections::BTreeMap::new();
        headers.insert("content-type".into(), "application/json".into());
        Ok(Self {
            method: method.to_string(),
            url: url.to_string(),
            headers,
            body: serde_json::to_vec(body)
                .map_err(|e| AppError::internal(format!("json encode: {e}")))?,
            timeout_ms: None,
        })
    }
}

mod serde_bytes_b64 {
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], ser: S) -> Result<S::Ok, S::Error> {
        if bytes.is_empty() {
            ser.serialize_str("")
        } else {
            // Keep payloads out of accidental logs: serialize as byte length marker.
            ser.serialize_str(&format!("<{} bytes>", bytes.len()))
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<Vec<u8>, D::Error> {
        // Wire requests are built in code; deserialization is lossless only
        // for the empty marker. Full body transport is not a use case.
        let _ = String::deserialize(de)?;
        Ok(Vec::new())
    }
}

/// Outbound response value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpResponse {
    pub status: u16,
    #[serde(default)]
    pub headers: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub body: Vec<u8>,
}

impl HttpResponse {
    pub fn succeeded(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn body_json<T: serde::de::DeserializeOwned>(&self) -> Result<T, AppError> {
        serde_json::from_slice(&self.body)
            .map_err(|e| AppError::external("http", format!("invalid json response: {e}")))
    }
}

/// Client posture.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HttpClientConfig {
    /// Per-attempt timeout.
    pub timeout_ms: u64,
    /// Retries AFTER the first attempt (so 2 = up to 3 attempts) — the
    /// `times` knob of the parity spec's `retry { times, base_ms, ... }`.
    pub max_retries: u32,
    /// Base backoff between retries: base * 2^(retry-1) (`base_ms`).
    pub retry_base_ms: u64,
    /// Wave 6 P10: backoff ceiling in milliseconds (`max_ms`); the
    /// exponential schedule stops growing here. Was hard-coded 10s.
    #[serde(default = "default_retry_max_ms")]
    pub retry_max_ms: u64,
    /// Wave 6 P10: which failures are retried (`on`) — any of `"429"`,
    /// `"5xx"`, `"timeout"`, `"connect"`. The default keeps the original
    /// posture (all four); an unknown condition refuses client build.
    #[serde(default = "default_retry_on")]
    pub retry_on: Vec<String>,
    /// Header names never rendered into errors or logs (authorization, keys).
    pub redact_headers: Vec<String>,
    /// **F-23**: the egress policy — what the client may reach. The default
    /// is permissive (no behaviour change); a deployment opts into the
    /// refusal of private targets.
    #[serde(default)]
    pub egress: EgressPolicy,
}

fn default_retry_max_ms() -> u64 {
    10_000
}

fn default_retry_on() -> Vec<String> {
    ["429", "5xx", "timeout", "connect"]
        .iter()
        .map(|c| (*c).to_string())
        .collect()
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 10_000,
            max_retries: 2,
            retry_base_ms: 250,
            retry_max_ms: default_retry_max_ms(),
            retry_on: default_retry_on(),
            redact_headers: vec!["authorization".into(), "x-api-key".into(), "cookie".into()],
            egress: EgressPolicy::default(),
        }
    }
}

impl HttpClientConfig {
    /// Render headers with configured secrets replaced by [REDACTED].
    pub fn redact(
        &self,
        headers: &std::collections::BTreeMap<String, String>,
    ) -> std::collections::BTreeMap<String, String> {
        headers
            .iter()
            .map(|(k, v)| {
                let redact = self
                    .redact_headers
                    .iter()
                    .any(|r| r.eq_ignore_ascii_case(k));
                (
                    k.clone(),
                    if redact {
                        "[REDACTED]".to_string()
                    } else {
                        v.clone()
                    },
                )
            })
            .collect()
    }

    /// Wave 6 P10: whether `condition` ("429" | "5xx" | "timeout" |
    /// "connect") is in the `retry_on` list — the `on` selector of the
    /// parity spec's `retry { ..., on: [5xx, timeout] }`.
    pub fn retries(&self, condition: &str) -> bool {
        self.retry_on.iter().any(|c| c == condition)
    }
}

/// The port. Implemented by `ReqwestHttpClient` (live) and `MockHttpClient`
/// (tests). Same failure shape as every other external dependency: timeout,
/// 429, duplicate-post risk are surfaced as `EXT_ERROR` with retryability.
pub trait HttpClientPort: Send + Sync {
    fn execute<'a>(&'a self, req: &'a HttpRequest)
        -> BoxFuture<'a, Result<HttpResponse, AppError>>;
}

/// Wave 6 P10: observability hook. Every request view handed to a tap has
/// its headers passed through [`HttpClientConfig::redact`] first, so taps
/// can log the shape safely — the `req.body` bytes are NOT redacted and
/// fall under this crate's "bodies are never logged" rule: a tap must
/// never print them. All hooks default to doing nothing.
pub trait HttpTap: Send + Sync {
    /// One attempt is about to be sent.
    fn on_request(&self, _req: &HttpRequest) {}
    /// A response arrived (before any retry decision). `sink_to` fires it
    /// with an empty `body` — the sink never materializes the payload.
    fn on_response(&self, _req: &HttpRequest, _res: &HttpResponse, _attempt: u32) {}
    /// This attempt failed and the backoff schedule will run.
    fn on_retry(&self, _req: &HttpRequest, _reason: &str, _attempt: u32) {}
    /// Terminal failure — the call is returning this error.
    fn on_error(&self, _req: &HttpRequest, _err: &AppError, _attempt: u32) {}
}

/// Live client (reqwest, rustls). Cheap to clone (inner client is pooled).
#[derive(Clone)]
pub struct ReqwestHttpClient {
    client: reqwest::Client,
    config: HttpClientConfig,
    tap: Option<std::sync::Arc<dyn HttpTap>>,
}

impl ReqwestHttpClient {
    pub fn new(config: HttpClientConfig) -> Result<Self, AppError> {
        for condition in &config.retry_on {
            if !matches!(condition.as_str(), "429" | "5xx" | "timeout" | "connect") {
                return Err(AppError::validation(format!(
                    "http retry_on: unknown condition {condition:?} (expected any of 429, 5xx, timeout, connect)"
                )));
            }
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms.max(1)))
            .use_rustls_tls()
            // **F-8**: the PINNING resolver. reqwest asks it for every name,
            // so the address the client connects to is the address the policy
            // checked — a DNS answer that changes between the check and the
            // connect (a rebind) cannot slip through. Every redirect hop
            // resolves again, so every hop is checked.
            .dns_resolver(std::sync::Arc::new(egress::PolicyResolver::new(
                config.egress.clone(),
            )))
            .build()
            .map_err(|e| AppError::internal(format!("http client build: {e}")))?;
        Ok(Self {
            client,
            config,
            tap: None,
        })
    }

    pub fn config(&self) -> &HttpClientConfig {
        &self.config
    }

    /// Wave 6 P10: attach an observability tap (chainable) — the hooks
    /// receive redacted header views of every attempt.
    pub fn with_tap(mut self, tap: std::sync::Arc<dyn HttpTap>) -> Self {
        self.tap = Some(tap);
        self
    }

    /// Exponential wait before `retry_index` (1-based):
    /// base * 2^(index-1) capped at `retry_max_ms`. Public so the schedule
    /// is assertable without a live server.
    pub fn retry_after(&self, retry_index: u32) -> Duration {
        let exp = 2u64.saturating_pow(retry_index.saturating_sub(1).min(6));
        Duration::from_millis(
            self.config
                .retry_base_ms
                .saturating_mul(exp)
                .min(self.config.retry_max_ms),
        )
    }

    /// Whether a response status class is retried under `retry_on` (429
    /// and 5xx only — 4xx stay caller bugs that are never retried).
    fn status_retryable(&self, status: u16) -> bool {
        match status {
            429 => self.config.retries("429"),
            other => other >= 500 && self.config.retries("5xx"),
        }
    }

    /// Terminal `EXT_ERROR` for a retryable status the policy gave up on
    /// (shared shape between `execute` and `sink_to`).
    fn status_error(&self, status: u16) -> AppError {
        AppError::new(
            kelson_foundation::codes::EXTERNAL,
            ErrorCategory::External,
            kelson_foundation::Severity::Medium,
            Retryability::Retryable,
            format!("external service http returned {status}"),
        )
    }

    /// Terminal `EXT_ERROR` for a transport-class failure (timeout/
    /// connect). `Some(err)` = fail now: `max_retries` is exhausted or
    /// the condition is not in `retry_on`; `None` = back off and retry.
    fn transport_failure(&self, condition: &str, msg: &str, attempt_no: u32) -> Option<AppError> {
        if attempt_no > self.config.max_retries || !self.config.retries(condition) {
            Some(
                AppError::new(
                    kelson_foundation::codes::EXTERNAL,
                    ErrorCategory::External,
                    kelson_foundation::Severity::Medium,
                    Retryability::Retryable,
                    "external service http unreachable",
                )
                .with_internal(msg),
            )
        } else {
            None
        }
    }

    /// The request as a tap sees it: headers through `redact` (the body
    /// stays untouched — see [`HttpTap`]'s never-log rule).
    fn tap_request(&self, req: &HttpRequest) -> HttpRequest {
        let mut seen = req.clone();
        seen.headers = self.config.redact(&req.headers);
        seen
    }

    /// Build + send one attempt (status/headers known, body still unread —
    /// `execute` buffers it, `sink_to` streams it). The per-request
    /// `timeout_ms` override wins over the client default.
    async fn send(&self, req: &HttpRequest) -> Result<reqwest::Response, AttemptError> {
        // **F-23**: the egress policy runs BEFORE the request — a refused
        // target never leaves the process. With the default (permissive)
        // policy this costs nothing: no lookup, no check.
        if let Err(err) = self.check_egress(&req.url).await {
            return Err(AttemptError::Call(err));
        }
        let method: reqwest::Method = req.method.parse().map_err(|_| {
            AttemptError::Call(AppError::validation(format!(
                "bad http method {}",
                req.method
            )))
        })?;
        let mut builder = self.client.request(method, &req.url);
        if let Some(ms) = req.timeout_ms {
            builder = builder.timeout(Duration::from_millis(ms.max(1)));
        }
        for (k, v) in &req.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        if !req.body.is_empty() {
            builder = builder.body(req.body.clone());
        }
        builder.send().await.map_err(|e| {
            if e.is_timeout() {
                AttemptError::Timeout(format!(
                    "timeout after {}ms: {e}",
                    req.timeout_ms.unwrap_or(self.config.timeout_ms)
                ))
            } else if e.is_connect() || e.is_request() {
                AttemptError::Connect(e.to_string())
            } else {
                AttemptError::Call(AppError::external("http", e.to_string()))
            }
        })
    }

    /// **F-23**: the egress check. Only runs when the policy opts in, so the
    /// default path is unchanged. A literal IP is checked without a lookup;
    /// a host is resolved and every address is checked.
    async fn check_egress(&self, url: &str) -> Result<(), AppError> {
        let policy = &self.config.egress;
        if !policy.refuse_private_targets {
            return Ok(());
        }
        let parsed = reqwest::Url::parse(url)
            .map_err(|e| AppError::validation(format!("http url is not parseable: {e}")))?;
        let host = parsed
            .host_str()
            .ok_or_else(|| AppError::validation("http url has no host"))?;
        if policy.allows_host(host) {
            return Ok(());
        }
        // A literal address needs no lookup.
        if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            if EgressPolicy::refuses_address(&ip) {
                return Err(EgressPolicy::refusal(host, &ip.to_string()));
            }
            return Ok(());
        }
        let port = parsed.port_or_known_default().unwrap_or(443);
        let addresses = egress::resolve_host(host, port).await?;
        for ip in &addresses {
            if EgressPolicy::refuses_address(ip) {
                return Err(EgressPolicy::refusal(host, &ip.to_string()));
            }
        }
        Ok(())
    }
    async fn attempt(&self, req: &HttpRequest) -> Result<HttpResponse, AttemptError> {
        let response = self.send(req).await?;
        let status = response.status().as_u16();
        let mut headers = std::collections::BTreeMap::new();
        for (name, value) in response.headers().iter() {
            if let Ok(v) = value.to_str() {
                headers.insert(name.as_str().to_string(), v.to_string());
            }
        }
        let body = response
            .bytes()
            .await
            .map_err(|e| AttemptError::Call(AppError::external("http", format!("body read: {e}"))))?
            .to_vec();
        Ok(HttpResponse {
            status,
            headers,
            body,
        })
    }

    /// Wave 6 P10 (parity: response sink `sink_to(disk, path)`): stream a
    /// successful response body into `storage` without ever holding the
    /// whole payload — chunks go straight through
    /// [`kelson_storage::StoragePort::writer`], and the local-disk adapter
    /// overrides the default buffered writer with direct file writes.
    ///
    /// Policy: `retry_on`/`max_retries` and the tap hooks apply while the
    /// status is decided; once streaming starts the download is NOT
    /// retried — a mid-stream failure aborts the write session and the
    /// writer's drop semantics discard the partial object. Non-2xx
    /// answers are `EXT_ERROR`: 4xx are `NotRetryable` (an error page is
    /// never sunk over the target key; a capped preview lands in
    /// `internal`), exhausted 429/5xx keep the `Retryable` shape
    /// `execute` produces. Callers pass the port of the target disk
    /// (`DiskRegistry::get(disk)?.port`) plus the object coordinates.
    pub async fn sink_to(
        &self,
        req: &HttpRequest,
        storage: &dyn kelson_storage::StoragePort,
        bucket: &str,
        key: &str,
    ) -> Result<kelson_storage::ObjectRef, AppError> {
        let mut attempt_no: u32 = 0;
        loop {
            attempt_no += 1;
            let seen = self.tap_request(req);
            if let Some(tap) = &self.tap {
                tap.on_request(&seen);
            }
            let mut response = match self.send(req).await {
                Ok(response) => response,
                Err(failure) => {
                    let (condition, msg) = match &failure {
                        AttemptError::Timeout(msg) => ("timeout", msg),
                        AttemptError::Connect(msg) => ("connect", msg),
                        AttemptError::Call(err) => {
                            if let Some(tap) = &self.tap {
                                tap.on_error(&seen, err, attempt_no);
                            }
                            return Err(err.clone());
                        }
                    };
                    match self.transport_failure(condition, msg, attempt_no) {
                        Some(err) => {
                            if let Some(tap) = &self.tap {
                                tap.on_error(&seen, &err, attempt_no);
                            }
                            return Err(err);
                        }
                        None => {
                            if let Some(tap) = &self.tap {
                                tap.on_retry(&seen, condition, attempt_no);
                            }
                            tokio::time::sleep(self.retry_after(attempt_no)).await;
                            continue;
                        }
                    }
                }
            };
            let status = response.status().as_u16();
            let mut headers = std::collections::BTreeMap::new();
            for (name, value) in response.headers().iter() {
                if let Ok(v) = value.to_str() {
                    headers.insert(name.as_str().to_string(), v.to_string());
                }
            }
            if let Some(tap) = &self.tap {
                // Empty body on purpose: the sink never materializes it.
                tap.on_response(
                    &seen,
                    &HttpResponse {
                        status,
                        headers,
                        body: Vec::new(),
                    },
                    attempt_no,
                );
            }
            if self.status_retryable(status) {
                if attempt_no > self.config.max_retries {
                    let err = self.status_error(status);
                    if let Some(tap) = &self.tap {
                        tap.on_error(&seen, &err, attempt_no);
                    }
                    return Err(err);
                }
                if let Some(tap) = &self.tap {
                    tap.on_retry(&seen, &format!("status {status}"), attempt_no);
                }
                tokio::time::sleep(self.retry_after(attempt_no)).await;
                continue;
            }
            if !(200..300).contains(&status) {
                let mut preview = Vec::new();
                while preview.len() < 2048 {
                    match response.chunk().await {
                        Ok(Some(chunk)) => {
                            let take = (2048 - preview.len()).min(chunk.len());
                            preview.extend_from_slice(&chunk[..take]);
                        }
                        Ok(None) | Err(_) => break,
                    }
                }
                let retryability = if (400..500).contains(&status) {
                    Retryability::NotRetryable
                } else {
                    Retryability::Retryable
                };
                let err = AppError::new(
                    kelson_foundation::codes::EXTERNAL,
                    ErrorCategory::External,
                    kelson_foundation::Severity::Medium,
                    retryability,
                    format!("external service http returned {status}"),
                )
                .with_internal(format!(
                    "sink to {bucket}/{key} aborted: {}",
                    String::from_utf8_lossy(&preview)
                ));
                if let Some(tap) = &self.tap {
                    tap.on_error(&seen, &err, attempt_no);
                }
                return Err(err);
            }
            let mut writer = storage.writer(bucket, key)?;
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => writer.write(&chunk)?,
                    Ok(None) => break,
                    Err(e) => return Err(AppError::external("http", format!("body read: {e}"))),
                }
            }
            return writer.finish();
        }
    }
}

enum AttemptError {
    Timeout(String),
    Connect(String),
    /// Non-retryable caller/configuration failure.
    Call(AppError),
}

impl HttpClientPort for ReqwestHttpClient {
    fn execute<'a>(
        &'a self,
        req: &'a HttpRequest,
    ) -> BoxFuture<'a, Result<HttpResponse, AppError>> {
        Box::pin(async move {
            let mut attempt_no: u32 = 0;
            loop {
                attempt_no += 1;
                let seen = self.tap_request(req);
                if let Some(tap) = &self.tap {
                    tap.on_request(&seen);
                }
                match self.attempt(req).await {
                    Ok(response) => {
                        if let Some(tap) = &self.tap {
                            tap.on_response(&seen, &response, attempt_no);
                        }
                        // 429/5xx are intrinsically external failures (4xx
                        // stay `Ok` for the caller); `retry_on` only gates
                        // whether that failure is retried or fails fast.
                        if !(response.status == 429 || response.status >= 500) {
                            return Ok(response);
                        }
                        if attempt_no > self.config.max_retries
                            || !self.status_retryable(response.status)
                        {
                            let err = self.status_error(response.status);
                            if let Some(tap) = &self.tap {
                                tap.on_error(&seen, &err, attempt_no);
                            }
                            return Err(err);
                        }
                        if let Some(tap) = &self.tap {
                            tap.on_retry(&seen, &format!("status {}", response.status), attempt_no);
                        }
                        tokio::time::sleep(self.retry_after(attempt_no)).await;
                    }
                    Err(failure) => {
                        let (condition, msg) = match &failure {
                            AttemptError::Timeout(msg) => ("timeout", msg),
                            AttemptError::Connect(msg) => ("connect", msg),
                            AttemptError::Call(err) => {
                                if let Some(tap) = &self.tap {
                                    tap.on_error(&seen, err, attempt_no);
                                }
                                return Err(err.clone());
                            }
                        };
                        match self.transport_failure(condition, msg, attempt_no) {
                            Some(err) => {
                                if let Some(tap) = &self.tap {
                                    tap.on_error(&seen, &err, attempt_no);
                                }
                                return Err(err);
                            }
                            None => {
                                if let Some(tap) = &self.tap {
                                    tap.on_retry(&seen, condition, attempt_no);
                                }
                                tokio::time::sleep(self.retry_after(attempt_no)).await;
                            }
                        }
                    }
                }
            }
        })
    }
}

/// Test double: returns queued responses in order (or the default).
#[derive(Default)]
pub struct MockHttpClient {
    queued: std::sync::Mutex<std::collections::VecDeque<Result<HttpResponse, AppError>>>,
    default: std::sync::Mutex<Option<HttpResponse>>,
    pub calls: std::sync::Mutex<Vec<HttpRequest>>,
}

impl MockHttpClient {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(&self, response: HttpResponse) -> &Self {
        self.queued.lock().unwrap().push_back(Ok(response));
        self
    }

    pub fn enqueue_error(&self, err: AppError) -> &Self {
        self.queued.lock().unwrap().push_back(Err(err));
        self
    }

    pub fn set_default(&self, response: HttpResponse) -> &Self {
        *self.default.lock().unwrap() = Some(response);
        self
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    /// Wave 6 P10: responses still queued (never consumed).
    pub fn pending_count(&self) -> usize {
        self.queued.lock().unwrap().len()
    }
}

impl HttpClientPort for MockHttpClient {
    fn execute<'a>(
        &'a self,
        req: &'a HttpRequest,
    ) -> BoxFuture<'a, Result<HttpResponse, AppError>> {
        let response = {
            self.calls.lock().unwrap().push(req.clone());
            match self.queued.lock().unwrap().pop_front() {
                Some(item) => item,
                None => self
                    .default
                    .lock()
                    .unwrap()
                    .clone()
                    .map(Ok)
                    .unwrap_or_else(|| {
                        Err(AppError::not_implemented(
                            "mock http client has no queued response",
                        ))
                    }),
            }
        };
        Box::pin(async move { response })
    }
}

/// Wave 6 P10 (parity: concurrency pool): run `requests` through `client`
/// with at most `limit` in flight — a bounded tokio join-set refills as
/// tasks finish. Results come back in input order (one slot per request;
/// a panicked/cancelled task yields `KSN_INTERNAL` for its slot).
/// `limit = 0` clamps to 1.
pub async fn execute_pool<C>(
    client: C,
    requests: Vec<HttpRequest>,
    limit: usize,
) -> Vec<Result<HttpResponse, AppError>>
where
    C: HttpClientPort + Clone + Send + Sync + 'static,
{
    use tokio::task::JoinSet;

    let limit = limit.max(1);
    let mut results: Vec<Option<Result<HttpResponse, AppError>>> =
        requests.iter().map(|_| None).collect();
    let mut pending = requests.into_iter().enumerate();
    let mut running: JoinSet<(usize, Result<HttpResponse, AppError>)> = JoinSet::new();
    let mut aborted: Option<AppError> = None;
    loop {
        while running.len() < limit {
            match pending.next() {
                Some((index, request)) => {
                    let client = client.clone();
                    running.spawn(async move { (index, client.execute(&request).await) });
                }
                None => break,
            }
        }
        if running.is_empty() {
            break;
        }
        match running.join_next().await {
            Some(Ok((index, result))) => results[index] = Some(result),
            Some(Err(join_err)) => {
                aborted = Some(AppError::internal(format!(
                    "http pool task failed: {join_err}"
                )));
            }
            None => break,
        }
    }
    results
        .into_iter()
        .map(|slot| {
            slot.unwrap_or_else(|| {
                Err(aborted
                    .clone()
                    .unwrap_or_else(|| AppError::internal("http pool task dropped")))
            })
        })
        .collect()
}

/// Wave 6 P10 (parity: `HttpFake`): assertion-oriented wrapper over the
/// [`MockHttpClient`] recording — request count / method / url / body
/// checks whose failure messages show what was actually recorded. Wave 7
/// wires the assertions into the shared testkit.
#[derive(Default)]
pub struct HttpFake {
    mock: MockHttpClient,
}

impl HttpFake {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(&self, response: HttpResponse) -> &Self {
        self.mock.enqueue(response);
        self
    }

    pub fn enqueue_error(&self, err: AppError) -> &Self {
        self.mock.enqueue_error(err);
        self
    }

    pub fn set_default(&self, response: HttpResponse) -> &Self {
        self.mock.set_default(response);
        self
    }

    /// Number of requests recorded so far.
    pub fn request_count(&self) -> usize {
        self.mock.call_count()
    }

    /// The recorded request at `index` (panics naming the recorded set
    /// when out of range).
    pub fn request(&self, index: usize) -> HttpRequest {
        let calls = self.mock.calls.lock().unwrap();
        calls.get(index).cloned().unwrap_or_else(|| {
            panic!(
                "HttpFake: no request at index {index} (recorded {}): {}",
                calls.len(),
                recorded_summary(&calls)
            )
        })
    }

    /// Assert how many requests reached the fake (`assert_request_count`
    /// is the Wave 7 testkit name — the testkit fake delegates here).
    pub fn assert_request_count(&self, expected: usize) {
        let calls = self.mock.calls.lock().unwrap();
        assert_eq!(
            calls.len(),
            expected,
            "HttpFake: expected {expected} request(s), recorded {}: {}",
            calls.len(),
            recorded_summary(&calls)
        );
    }

    /// Assert at least one recorded request satisfies `matches` (the
    /// Wave 7 `assert_sent(|req| ...)` shape — inspect method/url/headers/
    /// body inside the closure).
    pub fn assert_sent(&self, matches: impl Fn(&HttpRequest) -> bool) {
        let calls = self.mock.calls.lock().unwrap();
        assert!(
            calls.iter().any(matches),
            "HttpFake: no recorded request matched the predicate (recorded {}): {}",
            calls.len(),
            recorded_summary(&calls)
        );
    }

    /// Assert every queued response was consumed.
    pub fn assert_no_pending(&self) {
        let pending = self.mock.pending_count();
        assert_eq!(
            pending, 0,
            "HttpFake: {pending} queued response(s) were never consumed"
        );
    }

    /// Assert the method and URL of request `index`.
    pub fn assert_request(&self, index: usize, method: &str, url: &str) {
        let calls = self.mock.calls.lock().unwrap();
        let Some(recorded) = calls.get(index) else {
            panic!(
                "HttpFake: no request at index {index} (recorded {}): {}",
                calls.len(),
                recorded_summary(&calls)
            );
        };
        assert_eq!(
            (recorded.method.as_str(), recorded.url.as_str()),
            (method, url),
            "HttpFake: request {index} mismatch (recorded: {})",
            recorded_summary(&calls)
        );
    }

    /// Assert the JSON body of request `index` equals `expected`.
    pub fn assert_body_json(&self, index: usize, expected: serde_json::Value) {
        let calls = self.mock.calls.lock().unwrap();
        let Some(recorded) = calls.get(index) else {
            panic!(
                "HttpFake: no request at index {index} (recorded {}): {}",
                calls.len(),
                recorded_summary(&calls)
            );
        };
        let actual: serde_json::Value =
            serde_json::from_slice(&recorded.body).unwrap_or_else(|e| {
                panic!(
                    "HttpFake: request {index} body is not JSON ({e}; {} bytes)",
                    recorded.body.len()
                )
            });
        assert_eq!(actual, expected, "HttpFake: request {index} body mismatch");
    }
}

/// Compact `method url` listing used in every HttpFake assertion message.
fn recorded_summary(calls: &[HttpRequest]) -> String {
    calls
        .iter()
        .map(|r| format!("{} {}", r.method, r.url))
        .collect::<Vec<_>>()
        .join(", ")
}

impl HttpClientPort for HttpFake {
    fn execute<'a>(
        &'a self,
        req: &'a HttpRequest,
    ) -> BoxFuture<'a, Result<HttpResponse, AppError>> {
        self.mock.execute(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(status: u16) -> HttpResponse {
        HttpResponse {
            status,
            headers: Default::default(),
            body: b"{}".to_vec(),
        }
    }

    #[test]
    fn header_redaction_is_case_insensitive() {
        let config = HttpClientConfig::default();
        let mut headers = std::collections::BTreeMap::new();
        headers.insert("Authorization".into(), "Bearer secret".into());
        headers.insert("x-trace-id".into(), "abc".into());
        let redacted = config.redact(&headers);
        assert_eq!(redacted.get("Authorization").unwrap(), "[REDACTED]");
        assert_eq!(redacted.get("x-trace-id").unwrap(), "abc");
    }

    #[test]
    fn json_request_builds_and_body_is_not_serialized() {
        let req = HttpRequest::json(
            "POST",
            "https://example.com/hook",
            &serde_json::json!({"a": 1}),
        )
        .unwrap();
        assert_eq!(req.headers.get("content-type").unwrap(), "application/json");
        let wire = serde_json::to_string(&req).unwrap();
        assert!(
            !wire.contains("\"a\""),
            "payload must not appear in serialized request: {wire}"
        );
        assert!(wire.contains("bytes>"));
    }

    #[tokio::test]
    async fn mock_client_queues_and_records_calls() {
        let mock = MockHttpClient::new();
        mock.enqueue(response(201));
        let req = HttpRequest::json("POST", "https://example.com", &()).unwrap();
        let res = mock.execute(&req).await.unwrap();
        assert_eq!(res.status, 201);
        assert!(res.succeeded());
        assert_eq!(mock.call_count(), 1);
        let err = mock.execute(&req).await.unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::NOT_IMPLEMENTED);
    }

    #[tokio::test]
    async fn response_json_parsing() {
        let res = HttpResponse {
            status: 200,
            headers: Default::default(),
            body: br#"{"ok":true}"#.to_vec(),
        };
        let parsed: serde_json::Value = res.body_json().unwrap();
        assert_eq!(parsed["ok"], true);
        let bad = HttpResponse {
            status: 200,
            headers: Default::default(),
            body: b"nope".to_vec(),
        };
        let err: AppError = bad.body_json::<serde_json::Value>().unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::EXTERNAL);
    }

    #[test]
    fn live_client_builds_with_defaults() {
        let client = ReqwestHttpClient::new(HttpClientConfig::default()).unwrap();
        assert_eq!(client.config().max_retries, 2);
    }

    // ---- Wave 6 P10 helpers ---------------------------------------------

    use std::sync::atomic::Ordering;

    /// Fast backoff so live retry tests finish in milliseconds.
    fn fast_config() -> HttpClientConfig {
        HttpClientConfig {
            timeout_ms: 5_000,
            retry_base_ms: 5,
            retry_max_ms: 20,
            ..HttpClientConfig::default()
        }
    }

    /// Wave 6 P10: scripted loopback HTTP/1.1 server for live tests — one
    /// scripted turn per accepted connection, in order, then the fallback
    /// repeats; every connection gets its own std thread. Every response
    /// says `connection: close`, so one connection == one client attempt
    /// and `hits` counts attempts.
    struct Loopback {
        addr: std::net::SocketAddr,
        hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[derive(Clone)]
    enum Turn {
        Reply(u16, String),
        /// Sleep `u64` ms, then reply (timeout tests).
        Slow(u16, String, u64),
    }

    impl Loopback {
        fn spawn(turns: Vec<Turn>, fallback: Turn) -> Self {
            use std::io::{Read, Write as _};

            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let hits_server = hits.clone();
            let turns = std::sync::Arc::new(std::sync::Mutex::new(
                turns
                    .into_iter()
                    .collect::<std::collections::VecDeque<Turn>>(),
            ));
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { break };
                    let hits = hits_server.clone();
                    let turns = turns.clone();
                    let fallback = fallback.clone();
                    std::thread::spawn(move || {
                        hits.fetch_add(1, Ordering::SeqCst);
                        // Drain the request head; the blank line may split
                        // across reads, so scan the accumulated buffer.
                        let mut head: Vec<u8> = Vec::new();
                        let mut buf = [0u8; 4096];
                        loop {
                            match stream.read(&mut buf) {
                                Ok(0) | Err(_) => break,
                                Ok(n) => {
                                    head.extend_from_slice(&buf[..n]);
                                    let done = head.windows(4).any(|w| w == b"\r\n\r\n");
                                    if done || head.len() > 65_536 {
                                        break;
                                    }
                                }
                            }
                        }
                        let turn = turns.lock().unwrap().pop_front().unwrap_or(fallback);
                        let (status, body, delay) = match turn {
                            Turn::Reply(status, body) => (status, body, 0),
                            Turn::Slow(status, body, delay) => (status, body, delay),
                        };
                        if delay > 0 {
                            std::thread::sleep(std::time::Duration::from_millis(delay));
                        }
                        let reason = match status {
                            200 => "OK",
                            404 => "Not Found",
                            429 => "Too Many Requests",
                            500 => "Internal Server Error",
                            503 => "Service Unavailable",
                            _ => "Status",
                        };
                        let head = format!(
                            "HTTP/1.1 {status} {reason}\r\ncontent-length: {}\r\ncontent-type: application/json\r\nconnection: close\r\n\r\n",
                            body.len()
                        );
                        let _ = stream.write_all(head.as_bytes());
                        let _ = stream.write_all(body.as_bytes());
                        let _ = stream.flush();
                    });
                }
            });
            Self { addr, hits }
        }

        fn url(&self, path: &str) -> String {
            format!("http://{}{}", self.addr, path)
        }

        fn hit_count(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }
    }

    /// Tap double: records every hook invocation for assertions.
    #[derive(Default)]
    struct RecordingTap {
        requests: std::sync::Mutex<Vec<HttpRequest>>,
        responses: std::sync::Mutex<Vec<u16>>,
        retries: std::sync::Mutex<Vec<String>>,
        errors: std::sync::Mutex<Vec<AppError>>,
    }

    impl HttpTap for RecordingTap {
        fn on_request(&self, req: &HttpRequest) {
            self.requests.lock().unwrap().push(req.clone());
        }
        fn on_response(&self, _req: &HttpRequest, res: &HttpResponse, _attempt: u32) {
            self.responses.lock().unwrap().push(res.status);
        }
        fn on_retry(&self, _req: &HttpRequest, reason: &str, _attempt: u32) {
            self.retries.lock().unwrap().push(reason.to_string());
        }
        fn on_error(&self, _req: &HttpRequest, err: &AppError, _attempt: u32) {
            self.errors.lock().unwrap().push(err.clone());
        }
    }

    /// Live client wrapper that measures how many `execute` calls overlap —
    /// the client-side proof that `execute_pool` honors its bound.
    #[derive(Clone)]
    struct Gauge {
        inner: ReqwestHttpClient,
        current: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        peak: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl HttpClientPort for Gauge {
        fn execute<'a>(
            &'a self,
            req: &'a HttpRequest,
        ) -> BoxFuture<'a, Result<HttpResponse, AppError>> {
            let current = self.current.clone();
            let peak = self.peak.clone();
            Box::pin(async move {
                let now = current.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                let result = self.inner.execute(req).await;
                current.fetch_sub(1, Ordering::SeqCst);
                result
            })
        }
    }

    /// In-memory storage port double for the sink tests (records commits).
    #[derive(Default)]
    struct MemStore(std::sync::Mutex<std::collections::HashMap<(String, String), Vec<u8>>>);

    impl kelson_storage::StoragePort for MemStore {
        fn put(
            &self,
            bucket: &str,
            key: &str,
            bytes: Vec<u8>,
        ) -> Result<kelson_storage::ObjectRef, AppError> {
            self.0
                .lock()
                .unwrap()
                .insert((bucket.to_string(), key.to_string()), bytes);
            Ok(kelson_storage::ObjectRef {
                bucket: bucket.to_string(),
                key: key.to_string(),
            })
        }
        fn get(&self, obj: &kelson_storage::ObjectRef) -> Result<Vec<u8>, AppError> {
            self.0
                .lock()
                .unwrap()
                .get(&(obj.bucket.clone(), obj.key.clone()))
                .cloned()
                .ok_or_else(|| AppError::not_implemented("missing stored object"))
        }
        fn delete(&self, obj: &kelson_storage::ObjectRef) -> Result<(), AppError> {
            self.0
                .lock()
                .unwrap()
                .remove(&(obj.bucket.clone(), obj.key.clone()));
            Ok(())
        }
        fn presigned_url(
            &self,
            _obj: &kelson_storage::ObjectRef,
            _ttl_secs: u64,
        ) -> Result<String, AppError> {
            Err(AppError::not_implemented("presign"))
        }
    }

    // ---- Wave 6 P10 tests -----------------------------------------------

    /// The retry schedule is base * 2^(i-1), capped at `retry_max_ms`
    /// (the cap was a hard-coded 10s before P10).
    #[test]
    fn retry_schedule_is_exponential_and_capped() {
        let client = ReqwestHttpClient::new(HttpClientConfig {
            retry_base_ms: 100,
            retry_max_ms: 350,
            ..HttpClientConfig::default()
        })
        .unwrap();
        assert_eq!(client.retry_after(1), Duration::from_millis(100));
        assert_eq!(client.retry_after(2), Duration::from_millis(200));
        assert_eq!(client.retry_after(3), Duration::from_millis(350));
        assert_eq!(client.retry_after(9), Duration::from_millis(350));
    }

    /// An unknown `retry_on` condition refuses client build instead of
    /// silently never retrying.
    #[test]
    fn unknown_retry_condition_refuses_client_build() {
        let config = HttpClientConfig {
            retry_on: vec!["5xx".into(), "sometimes".into()],
            ..HttpClientConfig::default()
        };
        let err = match ReqwestHttpClient::new(config) {
            Ok(_) => panic!("unknown retry_on condition must refuse the build"),
            Err(err) => err,
        };
        assert_eq!(err.code, kelson_foundation::codes::VALIDATION);
        assert!(
            err.public_message.contains("sometimes"),
            "{}",
            err.public_message
        );
    }

    /// New config/request fields are serde-default additive, and a request
    /// without a timeout override stays off the wire byte-for-byte.
    #[test]
    fn retry_and_timeout_fields_deserialize_additively() {
        let config: HttpClientConfig = serde_json::from_value(serde_json::json!({
            "timeout_ms": 1000,
            "max_retries": 1,
            "retry_base_ms": 10,
            "redact_headers": ["authorization"]
        }))
        .unwrap();
        assert_eq!(config.retry_max_ms, 10_000);
        assert_eq!(config.retry_on, HttpClientConfig::default().retry_on);

        let original = HttpRequest::json("GET", "https://example.com/x", &()).unwrap();
        let wire = serde_json::to_string(&original).unwrap();
        assert!(
            !wire.contains("timeout_ms"),
            "absent override stays off the wire: {wire}"
        );
        let decoded: HttpRequest = serde_json::from_str(&wire).unwrap();
        assert_eq!(decoded.timeout_ms, None);

        let mut overridden = original.clone();
        overridden.timeout_ms = Some(250);
        let decoded: HttpRequest =
            serde_json::from_str(&serde_json::to_string(&overridden).unwrap()).unwrap();
        assert_eq!(decoded.timeout_ms, Some(250));
    }

    /// A 500 is retried per the default `retry_on` and the second attempt
    /// succeeds.
    #[tokio::test]
    async fn live_retry_recovers_after_5xx() {
        let server = Loopback::spawn(
            vec![Turn::Reply(500, "{}".into())],
            Turn::Reply(200, r#"{"ok":true}"#.into()),
        );
        let client = ReqwestHttpClient::new(fast_config()).unwrap();
        let req = HttpRequest::json("GET", &server.url("/hook"), &()).unwrap();
        let res = client.execute(&req).await.unwrap();
        assert_eq!(res.status, 200);
        assert_eq!(server.hit_count(), 2, "exactly one retry after the 500");
    }

    /// Excluding `5xx` from `retry_on` fails on the first response —
    /// one attempt, no backoff.
    #[tokio::test]
    async fn retry_off_for_5xx_fails_on_the_first_response() {
        let server = Loopback::spawn(vec![], Turn::Reply(503, r#"{"nope":true}"#.into()));
        let client = ReqwestHttpClient::new(HttpClientConfig {
            retry_on: vec!["429".into(), "timeout".into(), "connect".into()],
            ..fast_config()
        })
        .unwrap();
        let req = HttpRequest::json("GET", &server.url("/hook"), &()).unwrap();
        let err = client.execute(&req).await.unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::EXTERNAL);
        assert!(err.public_message.contains("503"), "{}", err.public_message);
        assert_eq!(server.hit_count(), 1, "5xx must not retry when excluded");
    }

    /// `HttpRequest::timeout_ms` beats the client default: the 100ms
    /// override fires long before the server's 700ms sleep finishes.
    #[tokio::test]
    async fn per_request_timeout_override_beats_the_client_default() {
        let server = Loopback::spawn(vec![], Turn::Slow(200, "{}".into(), 700));
        let client = ReqwestHttpClient::new(HttpClientConfig {
            max_retries: 0,
            ..fast_config()
        })
        .unwrap();
        let mut req = HttpRequest::json("GET", &server.url("/slow"), &()).unwrap();
        req.timeout_ms = Some(100);
        let started = std::time::Instant::now();
        let err = client.execute(&req).await.unwrap_err();
        let elapsed = started.elapsed();
        assert_eq!(err.code, kelson_foundation::codes::EXTERNAL);
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "the 100ms override must fire, not the 700ms server sleep ({elapsed:?})"
        );
        assert_eq!(server.hit_count(), 1, "timeouts retry; max_retries = 0");
    }

    /// Taps see one redacted view per attempt: `[REDACTED]` headers, the
    /// retry reason, and the final 200 — never the raw secret.
    #[tokio::test]
    async fn tap_sees_redacted_headers_across_retries() {
        let server = Loopback::spawn(
            vec![Turn::Reply(500, "{}".into())],
            Turn::Reply(200, r#"{"ok":true}"#.into()),
        );
        let tap = std::sync::Arc::new(RecordingTap::default());
        let client = ReqwestHttpClient::new(fast_config())
            .unwrap()
            .with_tap(tap.clone());
        let mut req = HttpRequest::json("GET", &server.url("/hook"), &()).unwrap();
        req.headers
            .insert("authorization".into(), "Bearer supersecret".into());
        let res = client.execute(&req).await.unwrap();
        assert_eq!(res.status, 200);
        assert_eq!(server.hit_count(), 2);

        let requests = tap.requests.lock().unwrap();
        assert_eq!(requests.len(), 2, "one on_request per attempt");
        for seen in requests.iter() {
            assert_eq!(seen.headers.get("authorization").unwrap(), "[REDACTED]");
            assert!(
                !seen.headers.values().any(|v| v.contains("supersecret")),
                "raw secret leaked into the tap view"
            );
        }
        drop(requests);
        assert_eq!(*tap.responses.lock().unwrap(), vec![500, 200]);
        assert_eq!(*tap.retries.lock().unwrap(), vec!["status 500".to_string()]);
        assert!(tap.errors.lock().unwrap().is_empty());
    }

    /// The pool holds exactly `limit` requests in flight, returns results
    /// in input order, and issues one attempt per request.
    #[tokio::test]
    async fn execute_pool_bounds_concurrency_and_preserves_order() {
        let server = Loopback::spawn(vec![], Turn::Slow(200, r#"{"n":1}"#.into(), 250));
        let gauge = Gauge {
            inner: ReqwestHttpClient::new(fast_config()).unwrap(),
            current: Default::default(),
            peak: Default::default(),
        };
        let requests: Vec<HttpRequest> = (0..6)
            .map(|i| HttpRequest::json("GET", &server.url(&format!("/item/{i}")), &()).unwrap())
            .collect();
        let results = execute_pool(gauge.clone(), requests, 2).await;
        assert_eq!(results.len(), 6);
        for (index, result) in results.iter().enumerate() {
            assert_eq!(result.as_ref().unwrap().status, 200, "slot {index}");
        }
        assert_eq!(
            gauge.peak.load(Ordering::SeqCst),
            2,
            "the join-set must hold exactly the limit in flight"
        );
        assert_eq!(server.hit_count(), 6);
    }

    /// HttpFake asserts count / method / url / body over the recording.
    #[tokio::test]
    async fn http_fake_asserts_count_method_url_and_body() {
        let fake = HttpFake::new();
        fake.enqueue(response(201));
        let req = HttpRequest::json(
            "POST",
            "https://example.com/hooks/x",
            &serde_json::json!({"a": 1}),
        )
        .unwrap();
        let res = fake.execute(&req).await.unwrap();
        assert_eq!(res.status, 201);
        fake.assert_request_count(1);
        fake.assert_request(0, "POST", "https://example.com/hooks/x");
        fake.assert_body_json(0, serde_json::json!({"a": 1}));
        fake.assert_sent(|r| r.method == "POST" && r.url.ends_with("/hooks/x"));
        fake.assert_no_pending();
        assert_eq!(fake.request_count(), 1);
        assert_eq!(fake.request(0).url, req.url);
    }

    /// Assertion failures name the mismatch instead of a bare bool.
    #[test]
    #[should_panic(expected = "expected 2 request(s)")]
    fn http_fake_assert_failures_name_the_mismatch() {
        let fake = HttpFake::new();
        fake.assert_request_count(2);
    }

    /// `sink_to` streams a large body into storage — the committed object
    /// equals the payload, coordinates included.
    #[tokio::test]
    async fn sink_to_streams_the_body_into_storage() {
        let body = "x".repeat(100 * 1024);
        let server = Loopback::spawn(vec![], Turn::Reply(200, body.clone()));
        let client = ReqwestHttpClient::new(fast_config()).unwrap();
        let store = MemStore::default();
        let req = HttpRequest::json("GET", &server.url("/export.csv"), &()).unwrap();
        let obj = client
            .sink_to(&req, &store, "exports", "big.csv")
            .await
            .unwrap();
        assert_eq!(obj.bucket, "exports");
        assert_eq!(obj.key, "big.csv");
        let written = store.0.lock().unwrap();
        assert_eq!(written.len(), 1, "exactly one commit");
        let stored = written
            .get(&("exports".to_string(), "big.csv".to_string()))
            .expect("object committed under the requested coordinates");
        assert_eq!(stored.as_slice(), body.as_bytes());
    }

    /// A 404 answer is an error, not a file: nothing is committed and the
    /// caller-bug status is never retried.
    #[tokio::test]
    async fn sink_to_refuses_error_pages() {
        let server = Loopback::spawn(vec![], Turn::Reply(404, "not here".into()));
        let client = ReqwestHttpClient::new(fast_config()).unwrap();
        let store = MemStore::default();
        let req = HttpRequest::json("GET", &server.url("/missing.csv"), &()).unwrap();
        let err = client
            .sink_to(&req, &store, "exports", "missing.csv")
            .await
            .unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::EXTERNAL);
        assert!(err.public_message.contains("404"), "{}", err.public_message);
        assert!(
            store.0.lock().unwrap().is_empty(),
            "error pages must never be sunk over the target key"
        );
        assert_eq!(server.hit_count(), 1, "404 is a caller bug — never retried");
    }

    /// **F-23 — the client refuses a private target when the egress policy
    /// opts in.**
    ///
    /// The client used to fetch a loopback/private address like any other,
    /// so a URL an untrusted party supplies (the reachable path is the
    /// webhook adapter, which calls a tenant-registered target) could reach
    /// the host's internal network. The policy resolves the target and
    /// refuses it — before the request leaves the process.
    ///
    /// The default policy is permissive: no deployment changes behaviour
    /// (proved by `the_default_policy_reaches_a_private_target`).
    #[tokio::test]
    async fn the_client_refuses_a_loopback_target() {
        let server = Loopback::spawn(vec![], Turn::Reply(200, r#"{"ok":true}"#.into()));
        let mut config = fast_config();
        config.egress.refuse_private_targets = true;
        let client = ReqwestHttpClient::new(config).unwrap();
        let req = HttpRequest::json("GET", &server.url("/internal"), &()).unwrap();
        let err = client.execute(&req).await.unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::EXTERNAL);
        assert!(
            err.public_message.contains("egress"),
            "the refusal names the policy: {}",
            err.public_message
        );
        assert!(
            !err.public_message.contains("127.0.0.1"),
            "the public message never carries the address: {}",
            err.public_message
        );
        assert_eq!(server.hit_count(), 0, "a refused target is never contacted");
    }

    /// **F-23** — the DEFAULT policy is permissive: an existing deployment
    /// that calls an internal service keeps working, and the check costs
    /// nothing (no lookup, no refusal).
    #[tokio::test]
    async fn the_default_policy_reaches_a_private_target() {
        let server = Loopback::spawn(vec![], Turn::Reply(200, r#"{"ok":true}"#.into()));
        let client = ReqwestHttpClient::new(fast_config()).unwrap();
        let req = HttpRequest::json("GET", &server.url("/internal"), &()).unwrap();
        let res = client
            .execute(&req)
            .await
            .expect("the default policy is permissive");
        assert_eq!(res.status, 200);
        assert_eq!(server.hit_count(), 1);
    }

    /// **F-23** — the operator's allowlist is the escape hatch: a private
    /// host on the list is reachable even when the policy refuses private
    /// targets.
    #[tokio::test]
    async fn an_allowed_host_reaches_a_private_target() {
        let server = Loopback::spawn(vec![], Turn::Reply(200, r#"{"ok":true}"#.into()));
        let host = server
            .url("/internal")
            .trim_start_matches("http://")
            .split(':')
            .next()
            .unwrap_or("127.0.0.1")
            .to_string();
        let mut config = fast_config();
        config.egress.refuse_private_targets = true;
        config.egress.allowed_hosts = vec![host];
        let client = ReqwestHttpClient::new(config).unwrap();
        let req = HttpRequest::json("GET", &server.url("/internal"), &()).unwrap();
        let res = client
            .execute(&req)
            .await
            .expect("an allowed host is reachable");
        assert_eq!(res.status, 200);
    }
}
