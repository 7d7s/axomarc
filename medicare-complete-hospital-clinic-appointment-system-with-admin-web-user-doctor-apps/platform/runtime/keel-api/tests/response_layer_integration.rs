//! Wave 2 request/response layer (19-PARITY "Request/response layer"):
//! response kinds that reach the wire (`no_content`, `stream`), the
//! per-route `Cache-Control` policy and the ETag/conditional-GET (304)
//! exchange — all through the production router assembly, edge layer
//! included, so the middleware stack is exercised with real routes.
//!
//! `redirect` and `download` are covered at unit level in `graph_routes`
//! (their contracts key off the operation result, which no fixture
//! operation produces); queued cookies and terminating hooks are covered
//! end-to-end in `src/edge.rs`.

use axum::body::{Body, HttpBody};
use axum::http::{header, Method, Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

/// The real appointments manifest plus four Wave 2 routes: a 204, an NDJSON
/// stream, and two ETagged reads with different declared cache policies.
const EXTRA_ROUTES: &str = r#"
[[routes]]
method = "GET"
path = "/web/no-content"
operation = "availability"
capability = "availability"
plane = "web"
response = "no_content"
public = true

[[routes]]
method = "GET"
path = "/api/stream"
operation = "availability"
capability = "availability"
plane = "api"
response = "stream"
public = true

[[routes]]
method = "GET"
path = "/api/cached"
operation = "availability"
capability = "availability"
plane = "api"
response = "json"
public = true
cache = { policy = "public", max_age_secs = 60, stale_while_revalidate_secs = 30 }

[[routes]]
method = "GET"
path = "/api/cached-private"
operation = "availability"
capability = "availability"
plane = "api"
response = "json"
public = true
cache = { policy = "private", max_age_secs = 30 }

[[routes]]
method = "GET"
path = "/api/plain"
operation = "availability"
capability = "availability"
plane = "api"
response = "json"
public = true
"#;

fn router_for() -> axum::Router {
    router_with(|_| {})
}

/// `router_for` with a hook on the freshly built state, so a test can inject
/// a posture (Wave 7 P4 security headers) before the routes register.
fn router_with(prepare: impl FnOnce(&mut keel_api::routes::AppState)) -> axum::Router {
    let dir = std::env::temp_dir().join(format!("keel-response-it-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(dir.join("views")).expect("views fixture dir");
    let (state, _, _) = keel_api::build_app_state(&dir, true, true);
    let mut state = state;
    prepare(&mut state);
    let raw = format!(
        "{}{EXTRA_ROUTES}",
        include_str!("../../../applications/appointments/module.toml")
    );
    let manifest = keel_registry::ModuleManifest::from_toml(&raw).expect("fixture manifest parses");
    let graph =
        keel_registry::PlatformGraph::compile(vec![manifest]).expect("fixture graph compiles");

    let router = keel_api::routes::router(state.clone())
        .merge(keel_api::graph_routes::graph_router(state.clone(), &graph))
        .merge(keel_api::auth_routes::router(state.clone()));
    let router = state
        .sessions
        .as_ref()
        .expect("sessions enabled")
        .as_ref()
        .clone()
        .apply(router);
    // Production edge: queued-cookie flush + terminating hooks, outermost.
    router.layer(axum::middleware::from_fn_with_state(
        state.edge_config(),
        keel_api::edge::edge_mw,
    ))
}

struct Reply {
    status: StatusCode,
    content_type: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    /// True when the body arrived without a known length (streamed).
    streamed: bool,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

async fn send(router: &axum::Router, req: Request<Body>) -> Reply {
    let res = router.clone().oneshot(req).await.expect("request serves");
    let status = res.status();
    let headers: Vec<(String, String)> = res
        .headers()
        .iter()
        .map(|(key, value)| {
            (
                key.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let content_type = headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.clone())
        .unwrap_or_default();
    let streamed = res.body().size_hint().upper().is_none();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body reads");
    Reply {
        status,
        content_type,
        headers,
        body: bytes.to_vec(),
        streamed,
    }
}

async fn get(router: &axum::Router, uri: &str) -> Reply {
    send(
        router,
        Request::builder()
            .method(Method::GET)
            .uri(uri)
            .body(Body::empty())
            .expect("request builds"),
    )
    .await
}

async fn get_with(router: &axum::Router, uri: &str, if_none_match: &str) -> Reply {
    send(
        router,
        Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(header::IF_NONE_MATCH, if_none_match)
            .body(Body::empty())
            .expect("request builds"),
    )
    .await
}

// ------------------------------------------------------------- response kinds

#[tokio::test]
async fn no_content_routes_answer_204_without_a_body() {
    let router = router_for();
    let reply = get(&router, "/web/no-content").await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert!(reply.body.is_empty(), "{}", reply.text());
    assert!(
        reply.header("content-type").is_none(),
        "204 carries no representation"
    );
}

#[tokio::test]
async fn stream_routes_flush_ndjson_instead_of_an_envelope() {
    let router = router_for();
    let reply = get(&router, "/api/stream").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(reply.content_type, "application/x-ndjson");
    assert!(
        reply.streamed,
        "a streamed route has no known body length up front"
    );

    let text = reply.text();
    assert!(text.ends_with('\n'), "one record per line: {text:?}");
    let lines: Vec<&str> = text.lines().collect();
    assert!(!lines.is_empty(), "{text:?}");
    let first: Value = serde_json::from_str(lines[0]).expect("each line is JSON");
    assert!(
        first.get("ok").is_none() && first.get("data").is_none(),
        "NDJSON is the payload itself, never the Envelope: {first}"
    );
    assert!(
        first.get("date").is_some(),
        "the operation result is what streams: {first}"
    );
}

// ------------------------------------------------------------------- caching

#[tokio::test]
async fn declared_cache_policies_reach_the_wire_as_cache_control() {
    let router = router_for();

    let reply = get(&router, "/api/cached").await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(
        reply.header("cache-control"),
        Some("public, max-age=60, stale-while-revalidate=30"),
        "the route policy is rendered through the shared builder"
    );

    let reply = get(&router, "/api/cached-private").await;
    assert_eq!(reply.header("cache-control"), Some("private, max-age=30"));

    let reply = get(&router, "/api/plain").await;
    assert_eq!(
        reply.header("cache-control"),
        None,
        "no declared policy means no cache directive"
    );
    assert!(
        reply.header("etag").is_some(),
        "every buffered GET still gets a validator"
    );
}

#[tokio::test]
async fn conditional_get_short_circuits_with_304() {
    let router = router_for();

    let first = get(&router, "/api/cached").await;
    assert_eq!(first.status, StatusCode::OK);
    let etag = first.header("etag").expect("ETag advertised").to_string();
    assert!(etag.starts_with('"') && etag.ends_with('"'), "{etag}");
    let payload = first.text();

    let revisit = get_with(&router, "/api/cached", &etag).await;
    assert_eq!(revisit.status, StatusCode::NOT_MODIFIED);
    assert!(
        revisit.body.is_empty(),
        "304 never repeats the body: {:?}",
        revisit.text()
    );
    assert_eq!(revisit.header("etag"), Some(etag.as_str()));
    assert_eq!(
        revisit.header("cache-control"),
        Some("public, max-age=60, stale-while-revalidate=30"),
        "304 still carries the route policy"
    );

    // A stale validator keeps the full payload (and the same ETag).
    let stale = get_with(&router, "/api/cached", "\"not-this-one\"").await;
    assert_eq!(stale.status, StatusCode::OK);
    assert_eq!(stale.header("etag"), Some(etag.as_str()));
    assert_eq!(stale.text(), payload, "the payload is byte-identical");

    // `*` matches any current representation.
    let any = get_with(&router, "/api/cached", "*").await;
    assert_eq!(any.status, StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn an_unchanged_payload_keeps_its_validator_across_restarts_of_the_body() {
    let router = router_for();
    let one = get(&router, "/api/plain").await;
    let two = get(&router, "/api/plain").await;
    assert_eq!(
        one.header("etag"),
        two.header("etag"),
        "same bytes, same validator"
    );
    assert_ne!(one.header("etag"), None);
}

// ------------------------------------------- Wave 7 P4 security headers

/// A fully configured posture (every knob non-default) so each assertion
/// proves config → wire, not a coincidental default.
fn enabled_headers() -> keel_http::SecurityHeaders {
    keel_api::security_headers_from(&keel_config::SecuritySection {
        headers: keel_config::SecurityHeadersSection {
            enabled: true,
            hsts_max_age_secs: 7777,
            hsts_include_subdomains: true,
            csp: "default-src 'self' https://cdn.test".into(),
            referrer_policy: "no-referrer".into(),
        },
        ..Default::default()
    })
}

/// 19-PARITY "Security-headers middleware": a web-plane response carries
/// the configured posture, exactly as configured.
#[tokio::test]
async fn web_plane_responses_carry_the_configured_security_headers() {
    let router = router_with(|state| state.security_headers = enabled_headers());
    let reply = get(&router, "/web/no-content").await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert_eq!(reply.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(
        reply.header("strict-transport-security"),
        Some("max-age=7777; includeSubDomains")
    );
    assert_eq!(
        reply.header("content-security-policy"),
        Some("default-src 'self' https://cdn.test")
    );
    assert_eq!(reply.header("referrer-policy"), Some("no-referrer"));
}

/// The api plane stays untouched: same enabled posture, zero headers on an
/// api-plane answer.
#[tokio::test]
async fn api_plane_responses_stay_untouched_by_the_security_headers() {
    let router = router_with(|state| state.security_headers = enabled_headers());
    let reply = get(&router, "/api/plain").await;
    assert_eq!(reply.status, StatusCode::OK);
    for name in [
        "x-content-type-options",
        "strict-transport-security",
        "content-security-policy",
        "referrer-policy",
    ] {
        assert!(reply.header(name).is_none(), "{name} must not appear on the api plane");
    }
}

/// Default posture (the harness default, and `enabled = false`): even the
/// web plane emits nothing until a deployment turns the switch on.
#[tokio::test]
async fn the_security_headers_stay_off_until_a_deployment_turns_them_on() {
    let router = router_for();
    let reply = get(&router, "/web/no-content").await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    for name in [
        "x-content-type-options",
        "strict-transport-security",
        "content-security-policy",
        "referrer-policy",
    ] {
        assert!(reply.header(name).is_none(), "{name} must not appear while disabled");
    }
}
