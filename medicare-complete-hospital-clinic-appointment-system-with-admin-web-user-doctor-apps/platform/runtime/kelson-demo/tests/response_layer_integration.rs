#![cfg(feature = "demo")]
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

[[routes]]
method = "POST"
path = "/web/submit"
operation = "availability"
capability = "availability"
plane = "web"
response = "json"
public = true

[[routes]]
method = "POST"
path = "/api/submit"
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
fn router_with(prepare: impl FnOnce(&mut kelson_demo::routes::AppState)) -> axum::Router {
    let dir = std::env::temp_dir().join(format!("kelson-response-it-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(dir.join("views")).expect("views fixture dir");
    let (state, _, _) = kelson_demo::build_app_state(&dir, true, true);
    let mut state = state;
    prepare(&mut state);
    let raw = format!(
        "{}{EXTRA_ROUTES}",
        include_str!("../../../applications/appointments/module.toml")
    );
    let manifest = kelson_registry::ModuleManifest::from_toml(&raw).expect("fixture manifest parses");
    let graph =
        kelson_registry::PlatformGraph::compile(vec![manifest]).expect("fixture graph compiles");

    let router = kelson_demo::routes::router(state.clone())
        .merge(kelson_demo::graph_routes::graph_router(state.clone(), &graph))
        .merge(kelson_demo::auth_routes::router(state.clone()));
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
        kelson_demo::edge::edge_mw,
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

async fn post(router: &axum::Router, uri: &str) -> Reply {
    send(
        router,
        Request::builder()
            .method(Method::POST)
            .uri(uri)
            .body(Body::empty())
            .expect("request builds"),
    )
    .await
}

/// POST with the double-submit pair: the `kelson_csrf` cookie value echoed
/// in the `X-CSRF-TOKEN` header — exactly what a browser does after the
/// GET that rendered the form.
async fn post_with_csrf(router: &axum::Router, uri: &str, token: &str) -> Reply {
    send(
        router,
        Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header(header::COOKIE, format!("{}={token}", kelson_http::CSRF_COOKIE))
            .header(kelson_http::CSRF_HEADER, token)
            .body(Body::empty())
            .expect("request builds"),
    )
    .await
}

/// Pull the bootstrap token out of whichever `Set-Cookie` carries it —
/// the response may also carry a session or queued cookie, and insertion
/// order across layers is not part of the contract.
fn csrf_token(reply: &Reply) -> String {
    reply
        .headers
        .iter()
        .filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie"))
        .filter_map(|(_, value)| value.strip_prefix("kelson_csrf="))
        .filter_map(|rest| rest.split(';').next())
        .find(|token| !token.is_empty())
        .expect("kelson_csrf bootstrap cookie on a safe web-plane response")
        .to_string()
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
fn enabled_headers() -> kelson_http::SecurityHeaders {
    kelson_demo::security_headers_from(&kelson_config::SecuritySection {
        headers: kelson_config::SecurityHeadersSection {
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

// ------------------------------------------- Wave 7 P7 web-plane CSRF

/// The public message of a refused double-submit (built inline by
/// `csrf_fn`, rendered in the standard envelope).
const CSRF_DENIED: &str = "CSRF token missing or mismatched";

/// 19-PARITY "CSRF on-by-default for the web plane": a state-changing
/// web-plane request without the double-submit pair is refused — no flag,
/// no env; this is the default posture.
#[tokio::test]
async fn web_plane_state_changing_requests_are_csrf_checked_by_default() {
    let router = router_for();
    let reply = post(&router, "/web/submit").await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
    let body = reply.text();
    assert!(body.contains(CSRF_DENIED), "{body}");
}

/// The roundtrip (deferred from Wave 2 to this item): the safe GET
/// bootstraps the `kelson_csrf` cookie, the POST echoes cookie + header and
/// sails through the guard to the handler.
#[tokio::test]
async fn the_web_plane_csrf_roundtrip_bootstraps_from_a_safe_request() {
    let router = router_for();
    let boot = get(&router, "/web/no-content").await;
    assert_eq!(boot.status, StatusCode::NO_CONTENT);
    let token = csrf_token(&boot);

    let reply = post_with_csrf(&router, "/web/submit", &token).await;
    let body = reply.text();
    assert!(!body.contains(CSRF_DENIED), "the pair passes the guard: {body}");
    assert_ne!(reply.status, StatusCode::FORBIDDEN, "{body}");
}

/// The api plane stays untouched: the same state-changing request, no
/// guard anywhere near it — token-authenticated routes answer on their own
/// terms (19-PARITY "api plane stays token-authenticated, unchanged").
#[tokio::test]
async fn api_plane_state_changing_requests_stay_free_of_the_csrf_guard() {
    let router = router_for();
    let reply = post(&router, "/api/submit").await;
    let body = reply.text();
    assert!(!body.contains(CSRF_DENIED), "the api plane never runs the guard: {body}");
    assert_ne!(reply.status, StatusCode::FORBIDDEN, "{body}");
}

/// `[security.csrf].skip_paths` carries a declared receiver through —
/// config-only escape hatch, no flag or env (19-PARITY "skip-paths
/// config only").
#[tokio::test]
async fn csrf_skip_paths_carry_a_declared_receiver_through() {
    let router = router_with(|state| {
        state.csrf = kelson_http::CsrfGuard {
            bypass_paths: vec!["/web/submit".into()],
            secure_cookie: false,
        };
    });
    let reply = post(&router, "/web/submit").await;
    let body = reply.text();
    assert!(!body.contains(CSRF_DENIED), "the skip path passes unsigned: {body}");
    assert_ne!(reply.status, StatusCode::FORBIDDEN, "{body}");
}

// ------------------------------------------- Wave 8 ADR 0007 JSON:API

use std::sync::Arc;

use kelson_context::ExecutionContext;
use kelson_execution::{BoxFuture, ExecutionPlan, ExecutionUnit, OperationHandler};
use kelson_foundation::{AppError, ExecutionProfileKind, OperationId};

/// A standalone resource module: `resource = "notes"` expands into the
/// CRUD verbs (only `index`/`show` here), and one plain route proves the
/// header is ignored outside resource routes (ADR 0007 activation).
const NOTES_MODULE: &str = r#"[module]
name = "notes"
api = 1
version = "0.1.0"
maturity = "L0"
plane = "tenant"
description = "JSON:API negotiation fixture"

[capabilities]
index = { profile = "READ", inputs = ["page"] }
show = { profile = "READ", inputs = [] }

[dependencies]
contracts = []

[[routes]]
resource = "notes"
name = "notes"
only = ["index", "show"]
plane = "api"
response = "json"
public = true

[[routes]]
method = "GET"
path = "/plain"
operation = "index"
capability = "index"
plane = "api"
response = "json"
public = true
"#;

/// The canonical list result the `notes.index` stub returns: wrap-shaped
/// body + reserved `included` + page meta, exactly what a resource handler
/// renders through `ResourceCollection`/`from_page`.
fn index_result() -> Value {
    serde_json::json!({
        "data": [
            {
                "id": "n-1",
                "title": "first note",
                "body": "hello",
                "relationships": { "author": { "data": { "type": "people", "id": "p-1" } } }
            },
            { "id": 2, "title": "second note", "body": "world" }
        ],
        "included": [
            { "type": "people", "id": "p-1", "name": "Ada" },
            { "type": "people", "id": "p-1", "name": "dupe" },
            { "id": "orphan", "name": "no type" }
        ],
        "meta": { "page": 1, "per_page": 2, "total": 3, "has_more": true }
    })
}

/// The canonical item result the `notes.show` stub returns — its own `type`
/// member must never land in `attributes`.
fn show_result() -> Value {
    serde_json::json!({
        "data": {
            "id": "n-1",
            "type": "note",
            "title": "first note",
            "body": "hello"
        }
    })
}

/// Fixture operation: plan + fixed result (public/anonymous read).
struct StubOperation {
    plan: ExecutionPlan,
    result: Value,
}

impl OperationHandler for StubOperation {
    fn plan(&self) -> &ExecutionPlan {
        &self.plan
    }

    fn handle(
        &self,
        _ctx: Arc<ExecutionContext>,
        _input: Value,
        _unit: &mut dyn ExecutionUnit,
    ) -> BoxFuture<'static, Result<Value, AppError>> {
        let result = self.result.clone();
        Box::pin(async move { Ok(result) })
    }
}

fn stub(operation: &'static str, result: Value) -> Arc<StubOperation> {
    let mut plan = ExecutionPlan::new(OperationId::from_static(operation), ExecutionProfileKind::Read);
    // Public fixture route: the HTTP edge admits anonymous callers only
    // when the plan says authorization is not required.
    plan.authorization.required = false;
    Arc::new(StubOperation { plan, result })
}

/// The production router assembly over the notes fixture: stub operations
/// registered on the bus before any clone, then the same layers the other
/// suites in this file mount (sessions + edge).
fn jsonapi_router() -> axum::Router {
    let dir = std::env::temp_dir().join(format!("kelson-jsonapi-it-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(dir.join("views")).expect("views fixture dir");
    let (mut state, _, _) = kelson_demo::build_app_state(&dir, true, true);
    let bus = Arc::get_mut(&mut state.bus).expect("sole bus handle");
    bus.register(stub("notes.index", index_result()))
        .expect("index stub registers");
    bus.register(stub("notes.show", show_result()))
        .expect("show stub registers");

    let manifest =
        kelson_registry::ModuleManifest::from_toml(NOTES_MODULE).expect("notes manifest parses");
    let graph =
        kelson_registry::PlatformGraph::compile(vec![manifest]).expect("notes graph compiles");

    let router = kelson_demo::routes::router(state.clone())
        .merge(kelson_demo::graph_routes::graph_router(state.clone(), &graph))
        .merge(kelson_demo::auth_routes::router(state.clone()));
    let router = state
        .sessions
        .as_ref()
        .expect("sessions enabled")
        .as_ref()
        .clone()
        .apply(router);
    router.layer(axum::middleware::from_fn_with_state(
        state.edge_config(),
        kelson_demo::edge::edge_mw,
    ))
}

async fn get_accept(router: &axum::Router, uri: &str, accept: &str) -> Reply {
    send(
        router,
        Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header(header::ACCEPT, accept)
            .body(Body::empty())
            .expect("request builds"),
    )
    .await
}

#[tokio::test]
async fn jsonapi_negotiation_renders_the_compound_document() {
    let router = jsonapi_router();
    let reply = get_accept(&router, "/notes?page=1&per_page=2", "application/vnd.api+json").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.content_type, "application/vnd.api+json");

    let document: Value = serde_json::from_slice(&reply.body).expect("JSON:API document parses");
    assert!(document.get("ok").is_none(), "never the envelope: {document}");

    let data = document["data"].as_array().expect("collection primary data");
    assert_eq!(data.len(), 2);
    assert_eq!(data[0]["type"], "notes");
    assert_eq!(data[0]["id"], "n-1");
    assert_eq!(data[1]["id"], "2", "non-scalar ids lift as canonical strings");
    assert_eq!(data[0]["attributes"]["title"], "first note");
    assert!(data[0]["attributes"].get("id").is_none());
    assert!(data[0]["attributes"].get("type").is_none());
    assert!(data[0]["attributes"].get("relationships").is_none());
    assert!(data[0]["attributes"].get("included").is_none());
    assert_eq!(data[0]["relationships"]["author"]["data"]["type"], "people");
    assert!(
        data[1].get("relationships").is_none(),
        "an emptied relationships object is omitted"
    );

    let included = document["included"].as_array().expect("compound includes");
    assert_eq!(included.len(), 1, "dedupe by (type, id), drop bad entries");
    assert_eq!(included[0]["type"], "people");
    assert_eq!(included[0]["id"], "p-1");
    assert_eq!(included[0]["attributes"]["name"], "Ada");
    assert!(included[0].get("links").is_none(), "no self on included");

    assert_eq!(document["meta"]["page"], 1);
    assert_eq!(document["meta"]["has_more"], true);
    assert_eq!(document["links"]["self"], "/notes?page=1&per_page=2");
    assert_eq!(document["links"]["first"], "/notes?per_page=2&page=1");
    assert_eq!(document["links"]["next"], "/notes?per_page=2&page=2");
    assert_eq!(document["links"]["last"], "/notes?per_page=2&page=2");
    assert!(document["links"].get("prev").is_none(), "no prev at page 1");
}

#[tokio::test]
async fn jsonapi_item_documents_carry_self_links_and_strip_reserved_members() {
    let router = jsonapi_router();
    let reply = get_accept(&router, "/notes/n-1", "application/vnd.api+json").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());

    let document: Value = serde_json::from_slice(&reply.body).expect("JSON:API document parses");
    assert_eq!(document["data"]["type"], "notes");
    assert_eq!(document["data"]["id"], "n-1");
    assert_eq!(document["data"]["links"]["self"], "/notes/n-1");
    assert_eq!(document["links"]["self"], "/notes/n-1");
    assert_eq!(document["data"]["attributes"]["title"], "first note");
    assert!(
        document["data"]["attributes"].get("type").is_none(),
        "the body's reserved type never lands in attributes"
    );
    assert_eq!(
        document["links"].as_object().expect("links").len(),
        1,
        "no page meta -> self only"
    );
}

#[tokio::test]
async fn jsonapi_negotiation_absent_falls_back_to_the_default_envelope() {
    let router = jsonapi_router();
    let expected =
        serde_json::to_vec(&kelson_http::Envelope::ok(index_result())).expect("envelope serializes");

    // No Accept header: today's bytes, byte for byte.
    let reply = get(&router, "/notes?page=1&per_page=2").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.content_type, "application/json");
    assert_eq!(reply.body, expected, "the default wire is byte-identical");

    // An Accept that does not ask for JSON:API - same bytes.
    let reply = get_accept(&router, "/notes?page=1&per_page=2", "application/json").await;
    assert_eq!(reply.content_type, "application/json");
    assert_eq!(reply.body, expected);

    // q=0 explicitly refuses the media type - same bytes.
    let reply = get_accept(
        &router,
        "/notes?page=1&per_page=2",
        "application/vnd.api+json;q=0",
    )
    .await;
    assert_eq!(reply.content_type, "application/json");
    assert_eq!(reply.body, expected);

    // Non-resource routes ignore the header entirely (ADR 0007 activation).
    let reply = get_accept(&router, "/plain", "application/vnd.api+json").await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.content_type, "application/json");
    let body: Value = serde_json::from_slice(&reply.body).expect("envelope parses");
    assert_eq!(body["ok"], true);
    assert!(
        body["data"]["data"].is_array(),
        "the resource wrap stays: {body}"
    );
}

#[tokio::test]
async fn jsonapi_media_type_parameters_are_refused_with_406() {
    let router = jsonapi_router();
    let reply = get_accept(&router, "/notes", "application/vnd.api+json;ext[]=foo").await;
    assert_eq!(reply.status, StatusCode::NOT_ACCEPTABLE, "{}", reply.text());
    assert_eq!(reply.content_type, "application/json");

    let body: Value = serde_json::from_slice(&reply.body).expect("refusal parses");
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"]["code"], "KSN_NOT_ACCEPTABLE");
    assert_eq!(body["error"]["message"], "unsupported media type parameters");
    assert_eq!(body["error"]["retryable"], false);
    assert!(
        body.get("data").is_none(),
        "the standard envelope, not a JSON:API document: {body}"
    );
}

#[tokio::test]
async fn jsonapi_sparse_fieldsets_filter_attributes_at_the_http_edge() {
    let router = jsonapi_router();
    let reply = get_accept(
        &router,
        "/notes?fields[notes]=title,author",
        "application/vnd.api+json",
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());

    let document: Value = serde_json::from_slice(&reply.body).expect("JSON:API document parses");
    let data = &document["data"][0];
    assert_eq!(data["id"], "n-1", "id/type survive a fieldset");
    assert_eq!(data["type"], "notes");
    assert_eq!(data["attributes"]["title"], "first note");
    assert!(
        data["attributes"].get("body").is_none(),
        "unlisted attribute filtered"
    );
    assert!(
        data["relationships"].get("author").is_some(),
        "listed relationship kept"
    );
}
