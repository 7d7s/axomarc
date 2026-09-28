//! HTTP harness: serves an axum router in-process through tower oneshot.
//! No network, no runtime binary — the router is built by the caller from
//! platform primitives.

use axum::body::{to_bytes, Body, Bytes};
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::Router;
use serde::de::DeserializeOwned;
use serde::Serialize;
use tower::ServiceExt;

/// Actor header applied by TestApp::acting_as. Module test wiring maps it
/// into whichever identity mechanism it uses (real sessions land in S3).
pub const ACTOR_HEADER: &str = "x-keel-actor";
/// Tenant header applied by TestApp::tenant.
pub const TENANT_HEADER: &str = "x-keel-tenant";

/// In-process HTTP harness around one axum router.
pub struct TestApp {
    router: Router,
    actor: Option<String>,
    tenant: Option<String>,
    default_headers: Vec<(String, String)>,
}

impl TestApp {
    pub fn new(router: Router) -> Self {
        Self {
            router,
            actor: None,
            tenant: None,
            default_headers: Vec::new(),
        }
    }

    /// Every subsequent request acts as this principal.
    pub fn acting_as(mut self, principal: impl Into<String>) -> Self {
        self.actor = Some(principal.into());
        self
    }

    /// Every subsequent request runs inside this tenant scope.
    pub fn tenant(mut self, tenant: impl Into<String>) -> Self {
        self.tenant = Some(tenant.into());
        self
    }

    /// Extra default header applied to every request.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.default_headers.push((name.into(), value.into()));
        self
    }

    pub fn router(&self) -> &Router {
        &self.router
    }

    pub fn get(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::GET, path)
    }

    pub fn post(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::POST, path)
    }

    pub fn put(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::PUT, path)
    }

    pub fn patch(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::PATCH, path)
    }

    pub fn delete(&self, path: &str) -> TestRequest<'_> {
        self.request(Method::DELETE, path)
    }

    pub fn request(&self, method: Method, path: &str) -> TestRequest<'_> {
        TestRequest {
            app: self,
            method,
            path: path.to_string(),
            headers: HeaderMap::new(),
            body: Vec::new(),
            content_type: None,
        }
    }
}

/// One request under construction.
pub struct TestRequest<'a> {
    app: &'a TestApp,
    method: Method,
    path: String,
    headers: HeaderMap,
    body: Vec<u8>,
    content_type: Option<&'static str>,
}

impl<'a> TestRequest<'a> {
    pub fn header(mut self, name: impl AsRef<str>, value: impl AsRef<str>) -> Self {
        let name = axum::http::HeaderName::try_from(name.as_ref()).expect("valid header name");
        let value = axum::http::HeaderValue::try_from(value.as_ref()).expect("valid header value");
        self.headers.insert(name, value);
        self
    }

    pub fn json<T: Serialize>(mut self, value: &T) -> Self {
        self.body = serde_json::to_vec(value).expect("json serializes");
        self.content_type = Some("application/json");
        self
    }

    pub fn body(mut self, bytes: impl Into<Vec<u8>>) -> Self {
        self.body = bytes.into();
        self
    }

    /// Serve the request through the router (no network).
    pub async fn send(self) -> TestResponse {
        let mut builder = Request::builder()
            .method(self.method.clone())
            .uri(&self.path);
        if let Some(actor) = &self.app.actor {
            builder = builder.header(ACTOR_HEADER, actor.as_str());
        }
        if let Some(tenant) = &self.app.tenant {
            builder = builder.header(TENANT_HEADER, tenant.as_str());
        }
        for (name, value) in &self.app.default_headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        if let Some(content_type) = self.content_type {
            builder = builder.header(header::CONTENT_TYPE, content_type);
        }
        let mut request = builder.body(Body::from(self.body)).expect("request builds");
        let headers = request.headers_mut();
        for (name, value) in self.headers.iter() {
            headers.insert(name.clone(), value.clone());
        }
        let response = self
            .app
            .router
            .clone()
            .oneshot(request)
            .await
            .expect("router serves the request");
        let status = response.status();
        let headers = response.headers().clone();
        let body = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body collects");
        TestResponse {
            status,
            headers,
            body,
        }
    }
}

/// One served response with assertion ergonomics.
pub struct TestResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
}

impl TestResponse {
    pub fn status(&self) -> u16 {
        self.status.as_u16()
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }

    pub fn body_bytes(&self) -> &[u8] {
        &self.body
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json_value(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!("response body is not JSON: {e}; body: {}", self.body_text())
        })
    }

    pub fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| {
            panic!(
                "response body does not deserialize: {e}; body: {}",
                self.body_text()
            )
        })
    }

    pub fn assert_status(self, expected: u16) -> Self {
        assert_eq!(
            self.status.as_u16(),
            expected,
            "unexpected status; body: {}",
            self.body_text()
        );
        self
    }

    pub fn assert_header(self, name: &str, expected: &str) -> Self {
        let actual = self
            .header(name)
            .unwrap_or_else(|| panic!("header {name} missing; body: {}", self.body_text()));
        assert_eq!(actual, expected, "header {name} mismatch");
        self
    }

    /// Exact JSON equality.
    pub fn assert_json_eq(self, expected: serde_json::Value) -> Self {
        let actual = self.json_value();
        assert_eq!(actual, expected, "json mismatch");
        self
    }

    /// Subset match: every key/value in expected must appear in the actual
    /// payload (objects checked recursively, arrays positionally).
    pub fn assert_json_contains(self, expected: serde_json::Value) -> Self {
        let actual = self.json_value();
        assert!(
            json_contains(&actual, &expected),
            "json subset mismatch; actual: {actual}, expected subset: {expected}"
        );
        self
    }
}

fn json_contains(actual: &serde_json::Value, expected: &serde_json::Value) -> bool {
    match expected {
        serde_json::Value::Object(map) => match actual {
            serde_json::Value::Object(actual_map) => map.iter().all(|(key, value)| {
                actual_map
                    .get(key)
                    .map(|actual_value| json_contains(actual_value, value))
                    .unwrap_or(false)
            }),
            _ => false,
        },
        serde_json::Value::Array(items) => match actual {
            serde_json::Value::Array(actual_items) => {
                items.len() == actual_items.len()
                    && items
                        .iter()
                        .zip(actual_items)
                        .all(|(expected_item, actual_item)| {
                            json_contains(actual_item, expected_item)
                        })
            }
            _ => false,
        },
        other => actual == other,
    }
}
