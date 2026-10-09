//! Wave 2 routing edge (19-PARITY route dispatch + host matching + URL
//! resolution): `method = "ANY"` dispatch, automatic HEAD/OPTIONS
//! registration, `domain`/`subdomain` enforcement before the auth chain with
//! a matched subdomain feeding tenant resolution, and one-time signed-URL
//! redemption through the `signed` middleware builtin. In-memory backends;
//! see lib.rs::build_app_state.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use keel_foundation::{PrincipalId, TenantId, TenantRevision};
use keel_mod_appointments::{Appointment, AppointmentStatus};
use serde_json::Value;
use tower::ServiceExt;

/// The real appointments manifest plus the Wave 2 routing-edge routes: an
/// `ANY` route, a plain GET (HEAD/OPTIONS probes), a GET+explicit OPTIONS
/// pair, a `domain`-restricted route in both a public and an authenticated
/// variant (to prove the gate runs before auth), the `subdomain`-fed binding
/// route with an unscoped control, and a `signed` one-time link.
const EXTRA_ROUTES: &str = r#"
[[routes]]
method = "ANY"
path = "/wave2/any"
operation = "availability"
capability = "availability"
public = true

[[routes]]
method = "GET"
path = "/wave2/get"
operation = "availability"
capability = "availability"
public = true

[[routes]]
method = "GET"
path = "/wave2/explicit"
operation = "availability"
capability = "availability"
public = true

[[routes]]
method = "OPTIONS"
path = "/wave2/explicit"
operation = "availability"
capability = "availability"
public = true

[[routes]]
method = "GET"
path = "/wave2/domain"
operation = "availability"
capability = "availability"
domain = "api.example.com"
public = true

[[routes]]
method = "GET"
path = "/wave2/gated"
operation = "availability"
capability = "availability"
domain = "api.example.com"
public = false

[[routes]]
method = "GET"
path = "/wave2/hosted/{id}"
operation = "availability"
capability = "availability"
subdomain = "acme"
public = true
bindings = [{ param = "id", model = "Appointment" }]

[[routes]]
method = "GET"
path = "/wave2/default/{id}"
operation = "availability"
capability = "availability"
public = true
bindings = [{ param = "id", model = "Appointment" }]

[[routes]]
method = "GET"
path = "/wave2/signed"
operation = "availability"
capability = "availability"
public = true
middleware = ["signed"]

[[routes]]
method = "GET"
path = "/wave2/number/{value}"
operation = "availability"
capability = "availability"
public = true
constraints = [{ param = "value", kind = "number" }]

[[routes]]
method = "GET"
path = "/wave2/alpha/{value}"
operation = "availability"
capability = "availability"
public = true
constraints = [{ param = "value", kind = "alpha" }]

[[routes]]
method = "GET"
path = "/wave2/alpha-num/{value}"
operation = "availability"
capability = "availability"
public = true
constraints = [{ param = "value", kind = "alpha_num" }]

[[routes]]
method = "GET"
path = "/wave2/uuid/{value}"
operation = "availability"
capability = "availability"
public = true
constraints = [{ param = "value", kind = "uuid" }]

[[routes]]
method = "GET"
path = "/wave2/ulid/{value}"
operation = "availability"
capability = "availability"
public = true
constraints = [{ param = "value", kind = "ulid" }]

[[routes]]
method = "GET"
path = "/wave2/slug/{value}"
operation = "availability"
capability = "availability"
public = true
constraints = [{ param = "value", kind = "slug" }]

[[routes]]
method = "GET"
path = "/wave2/pattern/{value}"
operation = "availability"
capability = "availability"
public = true
constraints = [{ param = "value", kind = "pattern", pattern = "doc-[0-9]{4}" }]

[[routes]]
method = "GET"
path = "/wave2/constrained-gated/{value}"
operation = "availability"
capability = "availability"
public = false
constraints = [{ param = "value", kind = "number" }]
"#;

struct Fixture {
    state: keel_api::routes::AppState,
    graph: keel_registry::PlatformGraph,
    store: Arc<keel_mod_appointments::AppointmentStore>,
}

fn fixture() -> Fixture {
    let dir = std::env::temp_dir().join(format!("keel-route-edge-{}", uuid::Uuid::now_v7()));
    let (state, _, store) = keel_api::build_app_state(&dir, true, true);
    let raw = format!(
        "{}{EXTRA_ROUTES}",
        include_str!("../../../applications/appointments/module.toml")
    );
    let manifest = keel_registry::ModuleManifest::from_toml(&raw).expect("fixture manifest parses");
    let graph =
        keel_registry::PlatformGraph::compile(vec![manifest]).expect("fixture graph compiles");
    Fixture {
        state,
        graph,
        store,
    }
}

fn router_for(fx: &Fixture) -> axum::Router {
    let state = fx.state.clone();
    let router = keel_api::routes::router(state.clone())
        .merge(keel_api::graph_routes::graph_router(state.clone(), &fx.graph))
        .merge(keel_api::auth_routes::router(state.clone()));
    state
        .sessions
        .as_ref()
        .expect("sessions enabled")
        .as_ref()
        .clone()
        .apply(router)
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
    fn header(&self, name: header::HeaderName) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }
}

async fn send(router: &axum::Router, req: Request<Body>) -> Reply {
    let res = router.clone().oneshot(req).await.expect("request serves");
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body reads");
    Reply {
        status,
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

fn request(method: Method, uri: &str) -> Request<Body> {
    request_from(method, uri, None)
}

fn request_from(method: Method, uri: &str, host: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(host) = host {
        builder = builder.header(header::HOST, host);
    }
    builder.body(Body::empty()).expect("request builds")
}

// ------------------------------------------------------------- method dispatch

#[tokio::test]
async fn any_route_answers_every_declared_verb() {
    let fx = fixture();
    let router = router_for(&fx);
    for method in [
        Method::GET,
        Method::POST,
        Method::PUT,
        Method::PATCH,
        Method::DELETE,
        Method::OPTIONS,
    ] {
        let label = method.to_string();
        let reply = send(&router, request(method, "/wave2/any")).await;
        assert_eq!(reply.status, StatusCode::OK, "{label} on ANY");
    }
}

#[tokio::test]
async fn get_route_answers_head_without_a_body() {
    let fx = fixture();
    let router = router_for(&fx);

    let get = send(&router, request(Method::GET, "/wave2/get")).await;
    assert_eq!(get.status, StatusCode::OK);
    assert!(!get.body.is_empty(), "GET carries the envelope");

    let head = send(&router, request(Method::HEAD, "/wave2/get")).await;
    assert_eq!(head.status, StatusCode::OK, "GET auto-registers HEAD");
    assert!(head.body.is_empty(), "HEAD answers with no body");
    assert_eq!(
        head.header(header::CONTENT_LENGTH).as_deref(),
        Some(get.body.len().to_string().as_str()),
        "HEAD reports the length the GET would have served"
    );
}

#[tokio::test]
async fn get_route_answers_the_auto_options_probe() {
    let fx = fixture();
    let router = router_for(&fx);
    let reply = send(&router, request(Method::OPTIONS, "/wave2/get")).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT);
    assert!(reply.body.is_empty());
    assert_eq!(
        reply.header(header::ALLOW).as_deref(),
        Some("GET, HEAD, OPTIONS"),
        "Allow lists every method the path serves"
    );

    // An explicit OPTIONS route answers itself instead of the probe.
    let reply = send(&router, request(Method::OPTIONS, "/wave2/explicit")).await;
    assert_eq!(reply.status, StatusCode::OK, "explicit OPTIONS wins");
    assert_eq!(reply.json()["ok"], Value::Bool(true), "it dispatches");
}

/// A declared path with the WRONG verb is a 405 carrying `Allow` — the path
/// exists, the method does not, and the Allow list is the same declared set
/// the auto-OPTIONS probe serves. An undeclared path stays a 404 miss.
#[tokio::test]
async fn verb_mismatch_is_405_with_allow() {
    let fx = fixture();
    let router = router_for(&fx);

    let reply = send(&router, request(Method::POST, "/wave2/get")).await;
    assert_eq!(
        reply.status,
        StatusCode::METHOD_NOT_ALLOWED,
        "POST to a GET path refuses: {}",
        reply.body
    );
    // axum emits `GET,HEAD,OPTIONS` (no OWS); our probe emits `GET, HEAD,
    // OPTIONS` — same set either way, so compare tokens.
    let allow = reply.header(header::ALLOW).unwrap_or_default();
    let tokens: Vec<&str> = allow.split(',').map(str::trim).collect();
    assert_eq!(
        tokens,
        vec!["GET", "HEAD", "OPTIONS"],
        "405 Allow mirrors the declared set (got {allow:?})"
    );

    // The same path still answers its declared verbs...
    assert_eq!(
        send(&router, request(Method::GET, "/wave2/get")).await.status,
        StatusCode::OK
    );
    // ...while an undeclared path is a miss, never a 405.
    assert_eq!(
        send(&router, request(Method::POST, "/wave2/nowhere"))
            .await
            .status,
        StatusCode::NOT_FOUND
    );
}

// ------------------------------------------------------------- host matching

#[tokio::test]
async fn domain_gate_matches_before_the_auth_chain() {
    let fx = fixture();
    let router = router_for(&fx);

    // Wrong host on the public route: the route does not exist for it.
    let reply = send(&router, request_from(Method::GET, "/wave2/domain", Some("portal.example.com")))
        .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    // Right host: the handler runs.
    let reply =
        send(&router, request_from(Method::GET, "/wave2/domain", Some("api.example.com"))).await;
    assert_eq!(reply.status, StatusCode::OK);

    // Authenticated variant: the host gate short-circuits BEFORE `auth`, so
    // a wrong host is 404 (no route) rather than 401 (no identity).
    let reply = send(&router, request_from(Method::GET, "/wave2/gated", Some("portal.example.com")))
        .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    let reply =
        send(&router, request_from(Method::GET, "/wave2/gated", Some("api.example.com"))).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "auth runs on match");
}

#[tokio::test]
async fn subdomain_gate_404s_the_wrong_host_and_feeds_the_tenant() {
    use keel_tenancy::{TenantRecord, TenantStatus};

    let fx = fixture();
    let router = router_for(&fx);

    // Register the acme tenant + an appointment that belongs to it.
    let acme = TenantId::from_uuid(uuid::Uuid::now_v7());
    fx.state
        .tenants
        .as_ref()
        .expect("tenant registry wired")
        .store()
        .upsert(TenantRecord {
            id: acme,
            slug: "acme".into(),
            name: "Acme Inc".into(),
            plan: "standard".into(),
            status: TenantStatus::Active,
            region: "in".into(),
            capabilities: std::collections::BTreeSet::new(),
            created_at: time::OffsetDateTime::now_utc(),
            revision: TenantRevision(1),
        })
        .expect("tenant registers");
    let id = uuid::Uuid::now_v7();
    fx.store
        .insert(Appointment {
            id,
            tenant: acme,
            patient: "Ada".into(),
            slot: "09:00".into(),
            status: AppointmentStatus::Booked,
            version: 1,
        })
        .expect("appointment seeds");

    // Wrong host: the route does not match at all.
    let reply = send(
        &router,
        request_from(Method::GET, &format!("/wave2/hosted/{id}"), Some("globex.example.com")),
    )
    .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);

    // Right host: the subdomain resolves the acme tenant and feeds tenant
    // resolution, so the tenant-scoped binding finds the appointment.
    let reply = send(
        &router,
        request_from(Method::GET, &format!("/wave2/hosted/{id}"), Some("acme.example.com")),
    )
    .await;
    assert_eq!(reply.status, StatusCode::OK);

    // Control: the same appointment under the configured default tenant is
    // a miss (404) — the hit above came from the host-fed tenant alone.
    let reply = send(&router, request(Method::GET, &format!("/wave2/default/{id}"))).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
}

// ------------------------------------------------------------- signed URLs

#[tokio::test]
async fn one_time_signed_link_redempts_exactly_once() {
    let fx = fixture();
    let router = router_for(&fx);

    let link = fx
        .state
        .signed_urls
        .as_ref()
        .expect("signed gate wired")
        .sign_once(
            PrincipalId::new(),
            "/wave2/signed",
            time::Duration::minutes(5),
        )
        .expect("one-time link mints");

    // First redemption passes the `signed` gate and reaches the handler.
    let reply = send(&router, request(Method::GET, &link)).await;
    assert_eq!(reply.status, StatusCode::OK);

    // The very same link is spent: 404 with the rendered error code.
    let reply = send(&router, request(Method::GET, &link)).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND);
    assert_eq!(reply.json()["error"]["code"], "KER_NOT_FOUND");

    // An unsigned visit never reaches the handler either.
    let reply = send(&router, request(Method::GET, "/wave2/signed")).await;
    assert_eq!(reply.status, StatusCode::FORBIDDEN);
}

// ------------------------------------------------------------- constraints

#[tokio::test]
async fn constraint_mismatch_is_a_route_miss_for_every_kind() {
    let fx = fixture();
    let router = router_for(&fx);

    // (route segment, capture the kind accepts, capture it rejects)
    let cases: [(&str, &str, &str); 7] = [
        ("number", "42", "a1"),
        ("alpha", "abc", "a1"),
        ("alpha-num", "a1b2", "a-1"),
        ("uuid", "0197a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b", "not-a-uuid"),
        ("ulid", "01ARZ3NDEKTSV4RRFFQ69G5FAV", "too-short"),
        ("slug", "acme-2", "ACME"),
        ("pattern", "doc-0123", "doc-abcd"),
    ];
    for (segment, accepted, rejected) in cases {
        let ok = send(
            &router,
            request(Method::GET, &format!("/wave2/{segment}/{accepted}")),
        )
        .await;
        assert_eq!(
            ok.status,
            StatusCode::OK,
            "{segment} accepts {accepted}: {}",
            ok.body
        );

        let miss = send(
            &router,
            request(Method::GET, &format!("/wave2/{segment}/{rejected}")),
        )
        .await;
        assert_eq!(
            miss.status,
            StatusCode::NOT_FOUND,
            "{segment} rejects {rejected}: {}",
            miss.body
        );
        assert_eq!(miss.json()["error"]["code"], "KER_NOT_FOUND");
    }

    // Constraints gate route MATCHING, so they run before the auth chain:
    // a rejected capture is 404 (no route), never 401 (no identity).
    let miss = send(
        &router,
        request(Method::GET, "/wave2/constrained-gated/not-a-number"),
    )
    .await;
    assert_eq!(miss.status, StatusCode::NOT_FOUND);
    let unauthorized = send(
        &router,
        request(Method::GET, "/wave2/constrained-gated/42"),
    )
    .await;
    assert_eq!(unauthorized.status, StatusCode::UNAUTHORIZED);
}
