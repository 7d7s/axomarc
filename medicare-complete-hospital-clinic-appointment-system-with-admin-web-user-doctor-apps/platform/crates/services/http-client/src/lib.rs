//! # keel-http-client
//! Outbound HTTP client port (blueprint L44, S3). The platform TALKS to the
//! outside world through this port so that timeout, retry and redaction
//! policy is central + testable instead of scattered per adapter. Inbound
//! traffic uses axum (keel-http); this crate never touches it.
//!
//! Behavior matrix (spec §31 external dependencies):
//! - hard timeout on every attempt (`KER_DEADLINE`-shaped external error),
//! - retries with exponential backoff on 429/5xx and connect errors only
//!   (4xx are caller bugs — never retried),
//! - bodies are never logged; configured headers are redacted in errors.

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use keel_foundation::{AppError, ErrorCategory, Retryability};
use serde::{Deserialize, Serialize};

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
    /// Retries AFTER the first attempt (so 2 = up to 3 attempts).
    pub max_retries: u32,
    /// Base backoff between retries: base * 2^(retry-1), capped at 10s.
    pub retry_base_ms: u64,
    /// Header names never rendered into errors or logs (authorization, keys).
    pub redact_headers: Vec<String>,
}

impl Default for HttpClientConfig {
    fn default() -> Self {
        Self {
            timeout_ms: 10_000,
            max_retries: 2,
            retry_base_ms: 250,
            redact_headers: vec!["authorization".into(), "x-api-key".into(), "cookie".into()],
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
}

/// The port. Implemented by `ReqwestHttpClient` (live) and `MockHttpClient`
/// (tests). Same failure shape as every other external dependency: timeout,
/// 429, duplicate-post risk are surfaced as `EXT_ERROR` with retryability.
pub trait HttpClientPort: Send + Sync {
    fn execute<'a>(&'a self, req: &'a HttpRequest)
        -> BoxFuture<'a, Result<HttpResponse, AppError>>;
}

/// Live client (reqwest, rustls). Cheap to clone (inner client is pooled).
#[derive(Clone)]
pub struct ReqwestHttpClient {
    client: reqwest::Client,
    config: HttpClientConfig,
}

impl ReqwestHttpClient {
    pub fn new(config: HttpClientConfig) -> Result<Self, AppError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms.max(1)))
            .use_rustls_tls()
            .build()
            .map_err(|e| AppError::internal(format!("http client build: {e}")))?;
        Ok(Self { client, config })
    }

    pub fn config(&self) -> &HttpClientConfig {
        &self.config
    }

    fn retry_after(&self, retry_index: u32) -> Duration {
        let exp = 2u64.saturating_pow(retry_index.saturating_sub(1).min(6));
        Duration::from_millis((self.config.retry_base_ms.saturating_mul(exp)).min(10_000))
    }

    async fn attempt(&self, req: &HttpRequest) -> Result<HttpResponse, AttemptError> {
        let method: reqwest::Method = req.method.parse().map_err(|_| {
            AttemptError::Call(AppError::validation(format!(
                "bad http method {}",
                req.method
            )))
        })?;
        let mut builder = self.client.request(method, &req.url);
        for (k, v) in &req.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        if !req.body.is_empty() {
            builder = builder.body(req.body.clone());
        }
        let response = builder.send().await.map_err(|e| {
            if e.is_timeout() {
                AttemptError::Timeout(format!("timeout after {}ms: {e}", self.config.timeout_ms))
            } else if e.is_connect() || e.is_request() {
                AttemptError::Connect(e.to_string())
            } else {
                AttemptError::Call(AppError::external("http", e.to_string()))
            }
        })?;
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
                match self.attempt(req).await {
                    Ok(response) if response.status == 429 || response.status >= 500 => {
                        if attempt_no > self.config.max_retries {
                            return Err(AppError::new(
                                keel_foundation::codes::EXTERNAL,
                                ErrorCategory::External,
                                keel_foundation::Severity::Medium,
                                Retryability::Retryable,
                                format!("external service http returned {}", response.status),
                            ));
                        }
                        tokio::time::sleep(self.retry_after(attempt_no)).await;
                    }
                    Ok(response) => return Ok(response),
                    Err(AttemptError::Call(err)) => return Err(err),
                    Err(AttemptError::Timeout(msg) | AttemptError::Connect(msg)) => {
                        if attempt_no > self.config.max_retries {
                            return Err(AppError::new(
                                keel_foundation::codes::EXTERNAL,
                                ErrorCategory::External,
                                keel_foundation::Severity::Medium,
                                Retryability::Retryable,
                                "external service http unreachable",
                            )
                            .with_internal(msg));
                        }
                        tokio::time::sleep(self.retry_after(attempt_no)).await;
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
        assert_eq!(err.code, keel_foundation::codes::NOT_IMPLEMENTED);
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
        assert_eq!(err.code, keel_foundation::codes::EXTERNAL);
    }

    #[test]
    fn live_client_builds_with_defaults() {
        let client = ReqwestHttpClient::new(HttpClientConfig::default()).unwrap();
        assert_eq!(client.config().max_retries, 2);
    }
}
