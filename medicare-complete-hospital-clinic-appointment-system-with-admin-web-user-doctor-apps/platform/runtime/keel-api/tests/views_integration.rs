//! Wave 2 views edge integration: `response = "html"` routes render a view
//! with the operation result as the template context, web-plane errors render
//! `errors.404`-style pages, and JSON clients / the api plane keep the
//! Envelope shape. Fixture templates live under `<temp>/views` — the harness
//! points the engine there (see `lib.rs::build_app_state`).

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

/// The real appointments manifest plus four Wave 2 routes: an anonymous HTML
/// page, the same page behind a model binding (the 404 path), an
/// api-plane twin used to prove the plane split, and an authenticated
/// HTML page used to prove the `_auth` view convention.
const EXTRA_ROUTES: &str = r#"
[[routes]]
method = "GET"
path = "/web/availability"
operation = "availability"
capability = "availability"
plane = "web"
response = "html"
view = "pages.availability"
public = true

[[routes]]
method = "GET"
path = "/web/availability/{id}"
operation = "availability"
capability = "availability"
plane = "web"
response = "html"
view = "pages.availability"
public = true
bindings = [{ param = "id", model = "Appointment" }]

[[routes]]
method = "GET"
path = "/api/availability/{id}"
operation = "availability"
capability = "availability"
plane = "api"
response = "json"
public = true
bindings = [{ param = "id", model = "Appointment" }]

[[routes]]
method = "GET"
path = "/web/appointments"
operation = "list"
capability = "list"
plane = "web"
response = "html"
view = "pages.availability"
"#;

const AVAILABILITY_VIEW: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>Availability</title></head>
<body>
  <h1>Availability</h1>
  <p class="date">{{ date }}</p>
  <p class="booked">{{ appointments_booked }}</p>
  @auth<span class="who">{{ _auth }}</span>@endauth
  @guest<span class="guest">guest</span>@endguest
</body>
</html>
"#;

const ERROR_404_VIEW: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>{{ status }} {{ title }}</title></head>
<body>
  <main class="error-404">
    <h1>{{ status }}</h1>
    <p class="title">{{ title }}</p>
    <p class="message">{{ message }}</p>
    <p class="path">{{ path }}</p>
  </main>
</body>
</html>
"#;

struct Fixture {
    state: keel_api::routes::AppState,
    graph: keel_registry::PlatformGraph,
}

/// Boot the harness with fixture templates under `<dir>/views`. With
/// `with_error_view = false` the engine has no `errors/404` page, so the
/// error path must fall back to the JSON envelope.
fn fixture(with_error_view: bool) -> Fixture {
    let dir = std::env::temp_dir().join(format!("keel-views-it-{}", uuid::Uuid::now_v7()));
    let pages = dir.join("views").join("pages");
    std::fs::create_dir_all(&pages).expect("views fixture dir");
    std::fs::write(pages.join("availability.html"), AVAILABILITY_VIEW).expect("view writes");
    if with_error_view {
        let errors = dir.join("views").join("errors");
        std::fs::create_dir_all(&errors).expect("errors fixture dir");
        std::fs::write(errors.join("404.html"), ERROR_404_VIEW).expect("error view writes");
    }
    let (state, _, _) = keel_api::build_app_state(&dir, true, true);
    let raw = format!(
        "{}{EXTRA_ROUTES}",
        include_str!("../../../applications/appointments/module.toml")
    );
    let manifest = keel_registry::ModuleManifest::from_toml(&raw).expect("fixture manifest parses");
    let graph =
        keel_registry::PlatformGraph::compile(vec![manifest]).expect("fixture graph compiles");
    Fixture { state, graph }
}

fn router(fx: &Fixture) -> axum::Router {
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
    content_type: String,
    body: String,
    set_cookies: Vec<String>,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

async fn send(router: &axum::Router, req: Request<Body>) -> Reply {
    let res = router.clone().oneshot(req).await.expect("request serves");
    let status = res.status();
    let content_type = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let set_cookies: Vec<String> = res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(str::to_string)
        .collect();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body reads");
    Reply {
        status,
        content_type,
        body: String::from_utf8_lossy(&bytes).into_owned(),
        set_cookies,
    }
}

/// GET helper; `accept` is the raw Accept header when the test plays an API
/// client instead of a browser.
async fn get(router: &axum::Router, uri: &str, accept: Option<&str>) -> Reply {
    let mut builder = Request::builder().method(Method::GET).uri(uri);
    if let Some(accept) = accept {
        builder = builder.header(header::ACCEPT, accept);
    }
    send(router, builder.body(Body::empty()).expect("request builds")).await
}

/// Register a fresh principal and return its session cookie (production
/// wiring: auth routes + session middleware are part of the router).
async fn register(router: &axum::Router, email: &str) -> String {
    let body =
        serde_json::json!({ "email": email, "password": "correct-horse-99", "tenant": "dev" });
    let req = Request::builder()
        .method(Method::POST)
        .uri("/auth/register")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request builds");
    let reply = send(router, req).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    reply
        .set_cookies
        .iter()
        .find(|c| c.starts_with("keel_session="))
        .map(|c| c.split(';').next().expect("cookie pair").to_string())
        .expect("session cookie issued")
}

async fn get_with_cookie(router: &axum::Router, uri: &str, cookie: &str) -> Reply {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .expect("request builds");
    send(router, req).await
}

// ------------------------------------------------------------- html routes

#[tokio::test]
async fn html_route_renders_the_view_with_the_operation_result_as_context() {
    let fx = fixture(true);
    let router = router(&fx);
    let reply = get(&router, "/web/availability?date=2026-09-01", None).await;

    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert!(
        reply.content_type.starts_with("text/html"),
        "{}",
        reply.content_type
    );
    // The operation result IS the template context.
    assert!(
        reply.body.contains(r#"<p class="date">2026-09-01</p>"#),
        "result must reach the view: {}",
        reply.body
    );
    assert!(
        !reply.body.contains("{{"),
        "template must be rendered, not echoed: {}",
        reply.body
    );
    // Edge convention: an anonymous caller is a guest.
    assert!(
        reply.body.contains(r#"<span class="guest">guest</span>"#),
        "{}",
        reply.body
    );
    assert!(!reply.body.contains(r#"class="who""#), "{}", reply.body);
}

#[tokio::test]
async fn authenticated_html_route_carries_the_principal_as_auth_context() {
    let fx = fixture(true);
    let router = router(&fx);
    let email = format!("views-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&router, &email).await;

    let reply = get_with_cookie(&router, "/web/appointments", &cookie).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert!(
        reply.body.contains(r#"<span class="who""#),
        "_auth must carry the principal: {}",
        reply.body
    );
    assert!(!reply.body.contains("guest"), "{}", reply.body);

    // Same page without a session is a 401, and with no 401 error view the
    // edge falls back to the envelope instead of failing the render.
    let anon = get(&router, "/web/appointments", None).await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED, "{}", anon.body);
    assert!(
        anon.content_type.contains("application/json"),
        "{}",
        anon.content_type
    );
}

// ------------------------------------------------------------- error views

#[tokio::test]
async fn web_plane_404_renders_the_error_view_for_html_clients() {
    let fx = fixture(true);
    let router = router(&fx);
    let reply = get(&router, "/web/availability/not-a-uuid", None).await;

    assert_eq!(reply.status, StatusCode::NOT_FOUND, "{}", reply.body);
    assert!(
        reply.content_type.starts_with("text/html"),
        "{}",
        reply.content_type
    );
    assert!(
        reply.body.contains(r#"class="error-404""#),
        "{}",
        reply.body
    );
    assert!(reply.body.contains("<h1>404</h1>"), "{}", reply.body);
    // Public message only — internals never render.
    assert!(!reply.body.contains("internal"), "{}", reply.body);
}

#[tokio::test]
async fn web_plane_json_clients_keep_the_envelope() {
    let fx = fixture(true);
    let router = router(&fx);
    let reply = get(
        &router,
        "/web/availability/not-a-uuid",
        Some("application/json"),
    )
    .await;

    assert_eq!(reply.status, StatusCode::NOT_FOUND, "{}", reply.body);
    assert!(
        reply.content_type.contains("application/json"),
        "{}",
        reply.content_type
    );
    let body = reply.json();
    assert_eq!(body["ok"], Value::Bool(false));
    assert_eq!(body["error"]["code"].as_str(), Some("KER_NOT_FOUND"));
}

#[tokio::test]
async fn api_plane_errors_stay_on_the_envelope_even_when_error_views_exist() {
    let fx = fixture(true);
    let router = router(&fx);
    let reply = get(&router, "/api/availability/not-a-uuid", None).await;

    assert_eq!(reply.status, StatusCode::NOT_FOUND, "{}", reply.body);
    assert!(
        reply.content_type.contains("application/json"),
        "{}",
        reply.content_type
    );
    assert!(
        !reply.body.contains("error-404"),
        "the api plane must not render views: {}",
        reply.body
    );
    assert_eq!(reply.json()["ok"], Value::Bool(false));
}

#[tokio::test]
async fn missing_error_view_falls_back_to_the_envelope() {
    let fx = fixture(false);
    let router = router(&fx);
    let reply = get(&router, "/web/availability/not-a-uuid", None).await;

    assert_eq!(reply.status, StatusCode::NOT_FOUND, "{}", reply.body);
    assert!(
        reply.content_type.contains("application/json"),
        "{}",
        reply.content_type
    );
    let body = reply.json();
    assert_eq!(body["ok"], Value::Bool(false));
    assert_eq!(body["error"]["code"].as_str(), Some("KER_NOT_FOUND"));
}

#[tokio::test]
async fn html_route_without_an_engine_degrades_to_a_private_500_envelope() {
    let mut fx = fixture(true);
    fx.state.views = None; // harness opt-out path
    let router = router(&fx);
    let reply = get(&router, "/web/availability?date=2026-09-01", None).await;

    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR, "{}", reply.body);
    assert!(
        reply.content_type.contains("application/json"),
        "{}",
        reply.content_type
    );
    let body = reply.json();
    assert_eq!(body["ok"], Value::Bool(false));
    assert_eq!(body["error"]["code"].as_str(), Some("KER_INTERNAL"));
    // The public message never leaks the internal detail.
    assert!(!reply.body.contains("views are not configured"));
}
