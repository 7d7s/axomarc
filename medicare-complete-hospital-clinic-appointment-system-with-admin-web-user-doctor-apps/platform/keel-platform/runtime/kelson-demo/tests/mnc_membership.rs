//! MNC-001 — the membership trust boundary.
//!
//! A verified credential is an IDENTITY proof, not a tenant entry right.
//! These cases pin the edge at the real HTTP surface: register GRANTS the
//! membership edge it names, login DEMANDS an ACTIVE edge (cross-tenant and
//! revoked edges answer the uniform 401 and mint no session), and nothing
//! installs an access snapshot the membership store has never seen.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use kelson_foundation::{MembershipId, PrincipalId};
use kelson_identity::{Membership, MembershipStatus, MembershipStore};
use serde_json::Value;
use tower::ServiceExt;

fn app() -> (kelson_demo::routes::AppState, axum::Router) {
    let dir = std::env::temp_dir().join(format!("kelson-mnc001-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let (state, _graph, _store) = kelson_demo::build_app_state(&dir, true, true);
    let router = kelson_demo::routes::router(state.clone())
        .merge(kelson_demo::auth_routes::router(state.clone()));
    let router = state
        .sessions
        .as_ref()
        .expect("sessions enabled")
        .as_ref()
        .clone()
        .apply(router);
    (state, router)
}

async fn post(router: &axum::Router, uri: &str, body: Value) -> (StatusCode, Value, Vec<String>) {
    let req = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request builds");
    let res = router.clone().oneshot(req).await.expect("request serves");
    let status = res.status();
    let cookies: Vec<String> = res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(str::to_string)
        .collect();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body reads");
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json, cookies)
}

fn register_body(email: &str, tenant: &str) -> Value {
    serde_json::json!({ "email": email, "password": "Correct-horse-99", "tenant": tenant })
}

fn login_body(identifier: &str, tenant: Option<&str>) -> Value {
    let mut body = serde_json::json!({ "identifier": identifier, "password": "Correct-horse-99" });
    if let Some(tenant) = tenant {
        body["tenant"] = Value::String(tenant.to_string());
    }
    body
}

/// The golden edge: register grants, login lands — on the SAME tenant.
#[tokio::test]
async fn register_grants_and_login_on_the_same_tenant_lands() {
    let (_state, router) = app();
    let email = format!("mnc-home-{}@clinic.test", uuid::Uuid::now_v7());
    let (status, _body, cookies) =
        post(&router, "/auth/register", register_body(&email, "dev")).await;
    assert_eq!(status, StatusCode::OK, "register: {status}");
    assert!(
        cookies.iter().any(|c| c.starts_with("kelson_session=")),
        "register mints a session"
    );

    let (status, body, cookies) =
        post(&router, "/auth/login", login_body(&email, Some("dev"))).await;
    assert_eq!(status, StatusCode::OK, "same-tenant login: {body}");
    assert!(
        cookies.iter().any(|c| c.starts_with("kelson_session=")),
        "the home tenant issues a fresh session"
    );
}

/// MNC-001 core: any uuid in `body.tenant` used to walk straight in. With a
/// valid credential for tenant A, a login naming another tenant answers the
/// uniform 401 and mints NOTHING.
#[tokio::test]
async fn a_credential_is_not_a_tenant_entry() {
    let (_state, router) = app();
    let email = format!("mnc-cross-{}@clinic.test", uuid::Uuid::now_v7());
    let (status, _, _) = post(&router, "/auth/register", register_body(&email, "dev")).await;
    assert_eq!(status, StatusCode::OK);

    let foreign = uuid::Uuid::now_v7().to_string();
    let (status, body, cookies) =
        post(&router, "/auth/login", login_body(&email, Some(&foreign))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "AUTH_UNAUTHENTICATED", "{body}");
    assert!(
        !cookies.iter().any(|c| c.starts_with("kelson_session=")),
        "a refused cross-tenant login mints no session"
    );
}

/// The lifecycle edge: an existing member whose edge was REVOKED (or left in
/// INVITED state) no longer enters — the row exists but is not ACTIVE.
#[tokio::test]
async fn a_suspended_membership_refuses_login() {
    let (state, router) = app();
    let email = format!("mnc-revoked-{}@clinic.test", uuid::Uuid::now_v7());
    let (status, body, _) = post(&router, "/auth/register", register_body(&email, "dev")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let principal = PrincipalId::from_uuid(
        uuid::Uuid::parse_str(body["data"]["principal_id"].as_str().expect("principal id"))
            .expect("uuid"),
    );
    let tenant = state.default_tenant_id.expect("dev fallback tenant");

    let edge = state
        .memberships
        .find(principal, tenant)
        .expect("store answers")
        .expect("register granted the edge");
    state
        .memberships
        .update_status(edge.id, MembershipStatus::Revoked)
        .expect("revoke");

    let (status, body, cookies) =
        post(&router, "/auth/login", login_body(&email, Some("dev"))).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{body}");
    assert!(
        !cookies.iter().any(|c| c.starts_with("kelson_session=")),
        "a revoked edge mints no session"
    );
}

/// The store itself, at the port: a grant for a pair is unique, and the
/// gate reads ACTIVE status, not row existence.
#[tokio::test]
async fn the_store_grants_once_and_gates_on_active_status() {
    let store = kelson_identity::InMemoryMembershipStore::new();
    let principal = PrincipalId::new();
    let tenant = kelson_foundation::TenantId::new();
    let edge = Membership {
        id: MembershipId::new(),
        principal_id: principal,
        tenant_id: tenant,
        status: MembershipStatus::Invited,
        joined_at: time::OffsetDateTime::now_utc(),
    };
    store.grant(edge.clone()).expect("grants");
    assert!(
        store
            .active_for(principal, tenant)
            .expect("answers")
            .is_none(),
        "an INVITED edge is not entry"
    );
    store
        .update_status(edge.id, MembershipStatus::Active)
        .expect("activates");
    assert!(store
        .active_for(principal, tenant)
        .expect("answers")
        .is_some());
    assert!(
        store.grant(edge).is_err(),
        "one edge per (principal, tenant) pair"
    );
}
