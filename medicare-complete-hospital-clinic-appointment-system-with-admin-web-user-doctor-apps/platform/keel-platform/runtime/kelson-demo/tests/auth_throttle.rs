//! MNC-004 / MNC-005: the credential throttle's client key.
//!
//! MNC-004: `X-Forwarded-For` was trusted unconditionally for the login
//! bucket, so a rotating header reset the brute-force budget on every
//! attempt. The key now walks the chain right-to-left through the
//! deployment's `TrustedProxies` only (the F-3 Phase 1 posture the admission
//! rate limiter already enforces): behind the proxy, injected hops sit left
//! of the proxy-appended client hop and never change the bucket; with no
//! trusted proxies declared the header is ignored entirely.
//!
//! MNC-005: `/auth/register` had NO throttle — one unauthenticated Argon2
//! hash per request (a free CPU-DoS amplifier). It now pays the same
//! per-email bucket as login.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

struct App {
    router: axum::Router,
}

/// Harness with an optional trusted-proxy set injected BEFORE the router is
/// built (the same `state.trusted_proxies` the live boot parses from
/// `[http].trusted_proxies`).
fn app_with_trusted_proxies(proxies: Option<&[&str]>) -> App {
    let dir = std::env::temp_dir().join(format!("kelson-auth-throttle-{}", uuid::Uuid::now_v7()));
    let (mut state, graph, _store) = kelson_demo::build_app_state(&dir, true, true);
    if let Some(proxies) = proxies {
        let specs: Vec<String> = proxies.iter().map(|s| s.to_string()).collect();
        state.trusted_proxies =
            kelson_http::TrustedProxies::parse(&specs).expect("trusted proxies parse");
    }
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
    App { router }
}

fn app() -> App {
    app_with_trusted_proxies(None)
}

fn login_req(identifier: &str, forwarded_for: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("/auth/login")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(xff) = forwarded_for {
        builder = builder.header("x-forwarded-for", xff);
    }
    builder
        .body(Body::from(
            serde_json::json!({ "identifier": identifier, "password": "wrong-password-1!" })
                .to_string(),
        ))
        .unwrap()
}

fn register_req(email: &str, forwarded_for: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri("/auth/register")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(xff) = forwarded_for {
        builder = builder.header("x-forwarded-for", xff);
    }
    builder
        .body(Body::from(
            serde_json::json!({
                "email": email,
                "password": "Correct-Horse-9!",
                "tenant": "dev",
            })
            .to_string(),
        ))
        .unwrap()
}

async fn status_and_code(app: &App, req: Request<Body>) -> (StatusCode, String) {
    let res = app.router.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or_default();
    let code = body["error"]["code"].as_str().unwrap_or("").to_string();
    (status, code)
}

// ------------------------------------------------------------ MNC-004

#[tokio::test]
async fn rotating_x_forwarded_for_does_not_reset_the_login_bucket() {
    let app = app();
    // No trusted proxies are declared, so the header is ignored entirely:
    // every attempt lands in the `global|<identifier>` bucket no matter how
    // the XFF header rotates. Five attempts pass through (unauthenticated);
    // the sixth is refused by the throttle.
    let identifier = format!("{}-xff@example.test", uuid::Uuid::now_v7());
    for i in 0..5 {
        let (status, code) = status_and_code(
            &app,
            login_req(&identifier, Some(&format!("203.0.113.{i}"))),
        )
        .await;
        assert_eq!(
            code, "AUTH_UNAUTHENTICATED",
            "attempt {i} should reach the credential check ({status})"
        );
    }
    let (status, code) = status_and_code(
        &app,
        login_req(&identifier, Some("203.0.113.99")),
    )
    .await;
    assert_eq!(
        code, "CAP_LIMIT_EXCEEDED",
        "a rotating XFF header must not reset the brute-force budget ({status})"
    );
}

#[tokio::test]
async fn an_attacker_behind_the_proxy_cannot_rotate_its_bucket_by_injecting_hops() {
    // The deployment terminates TLS at an ingress on 10.0.0.0/8.
    let app = app_with_trusted_proxies(Some(&["10.0.0.0/8"]));
    let identifier = format!("{}-inject@example.test", uuid::Uuid::now_v7());
    // The ingress appends the real client (203.0.113.9) as the rightmost hop;
    // the attacker's injected hops sit to its LEFT and must not matter.
    for i in 0..5 {
        let xff = format!("attacker-{i}, 203.0.113.9, 10.0.0.5");
        let (status, code) = status_and_code(&app, login_req(&identifier, Some(&xff))).await;
        assert_eq!(
            code, "AUTH_UNAUTHENTICATED",
            "attempt {i} should reach the credential check ({status})"
        );
    }
    let (status, code) =
        status_and_code(&app, login_req(&identifier, Some("attacker-99, 203.0.113.9, 10.0.0.5")))
            .await;
    assert_eq!(
        code, "CAP_LIMIT_EXCEEDED",
        "injected hops left of the real client must not reset the bucket ({status})"
    );
}

#[tokio::test]
async fn a_different_client_behind_the_proxy_keeps_its_own_budget() {
    let app = app_with_trusted_proxies(Some(&["10.0.0.0/8"]));
    let identifier = format!("{}-two-clients@example.test", uuid::Uuid::now_v7());
    // Client A exhausts its bucket (rightmost non-proxy hop = 203.0.113.9).
    for _ in 0..5 {
        let xff = "203.0.113.9, 10.0.0.5";
        let _ = status_and_code(&app, login_req(&identifier, Some(xff))).await;
    }
    let (status, code) =
        status_and_code(&app, login_req(&identifier, Some("203.0.113.9, 10.0.0.5"))).await;
    assert_eq!(code, "CAP_LIMIT_EXCEEDED", "client A is throttled ({status})");
    // Client B (a different observed peer) still has its own budget.
    let (status, code) =
        status_and_code(&app, login_req(&identifier, Some("203.0.113.10, 10.0.0.5"))).await;
    assert_eq!(
        code, "AUTH_UNAUTHENTICATED",
        "client B must not inherit client A's throttle ({status})"
    );
}

// ------------------------------------------------------------ MNC-005

#[tokio::test]
async fn registration_is_throttled_per_email() {
    let app = app();
    let email = format!("{}-reg@example.test", uuid::Uuid::now_v7());
    for i in 0..5 {
        let (status, code) = status_and_code(&app, register_req(&email, None)).await;
        assert!(
            code == "KSN_CONFLICT" || code == "AUTH_UNAUTHENTICATED" || status == StatusCode::OK,
            "attempt {i} should reach the credential work ({status}, {code})"
        );
    }
    let (status, code) = status_and_code(&app, register_req(&email, None)).await;
    assert_eq!(
        code, "CAP_LIMIT_EXCEEDED",
        "the sixth registration attempt must be throttled before any Argon2 work ({status})"
    );
}

#[tokio::test]
async fn a_throttled_registration_never_reaches_account_creation() {
    let app = app();
    let email = format!("{}-nocreate@example.test", uuid::Uuid::now_v7());
    // Exhaust the bucket with failing logins on the same identifier key
    // space, then prove a registration on that email is refused outright.
    for _ in 0..5 {
        let _ = status_and_code(&app, login_req(&email, None)).await;
    }
    let (status, code) = status_and_code(&app, register_req(&email, None)).await;
    assert_eq!(
        code, "CAP_LIMIT_EXCEEDED",
        "login and registration share the credential budget ({status})"
    );
}
