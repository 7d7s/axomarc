//! Wave 2 error rendering (19-PARITY "Error rendering"): the typed status
//! registry, the `reportable`/`renderable` split, per-code renderer output,
//! the `app.debug` detail gate (redacted through the logging list) and the
//! RFC-7807 `application/problem+json` option for the api plane. Fixture
//! templates live under `<temp>/views`, exactly like the views suite.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

/// The real appointments manifest plus three Wave 2 routes: an api-plane
/// twin (binding miss -> 404), the web-plane twin (error view path), and an
/// HTML route whose view template is deliberately absent (500 path).
const EXTRA_ROUTES: &str = r#"
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
path = "/web/broken"
operation = "availability"
capability = "availability"
plane = "web"
response = "html"
view = "pages.missing"
public = true
"#;

const ERROR_404_VIEW: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>{{ status }} {{ title }}</title></head>
<body>
  <main class="error-404">
    <h1>{{ status }}</h1>
    <p class="message">{{ message }}</p>
    <p class="path">{{ path }}</p>
  </main>
</body>
</html>
"#;

const ERROR_500_VIEW: &str = r#"<!doctype html>
<html lang="en">
<head><meta charset="utf-8"><title>{{ status }} {{ title }}</title></head>
<body>
  <main class="error-500">
    <h1>{{ status }}</h1>
    <p class="message">{{ message }}</p>
    <pre class="debug">{{ debug.internal }}</pre>
  </main>
</body>
</html>
"#;

struct Fixture {
    state: keel_api::routes::AppState,
    graph: keel_registry::PlatformGraph,
}

/// Boot the harness with `errors/404.html` and `errors/500.html` wired into
/// the engine. Tests flip the two Wave 2 gates (`state.debug`,
/// `state.problem_json`), register registry overrides, or drop `state.views`
/// to exercise the degrade path.
fn fixture() -> Fixture {
    let dir = std::env::temp_dir().join(format!("keel-error-it-{}", uuid::Uuid::now_v7()));
    let errors = dir.join("views").join("errors");
    std::fs::create_dir_all(&errors).expect("error fixture dir");
    std::fs::write(errors.join("404.html"), ERROR_404_VIEW).expect("404 view writes");
    std::fs::write(errors.join("500.html"), ERROR_500_VIEW).expect("500 view writes");
    let (state, _, _) = keel_api::build_app_state(&dir, true, true);
    let raw = format!(
        "{}{EXTRA_ROUTES}",
        include_str!("../../applications/appointments/module.toml")
    );
    let manifest = keel_registry::ModuleManifest::from_toml(&raw).expect("fixture manifest parses");
    let graph =
        keel_registry::PlatformGraph::compile(vec![manifest]).expect("fixture graph compiles");
    Fixture { state, graph }
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
    content_type: String,
    body: String,
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
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("body reads");
    Reply {
        status,
        content_type,
        body: String::from_utf8_lossy(&bytes).into_owned(),
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

// ------------------------------------------------------------- rfc-7807

#[tokio::test]
async fn api_plane_serves_rfc7807_when_the_deployment_opts_in() {
    let mut fx = fixture();
    fx.state.problem_json = true;
    let router = router_for(&fx);

    let reply = get(&router, "/api/availability/not-a-uuid", None).await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND, "{}", reply.body);
    assert!(
        reply.content_type.contains("application/problem+json"),
        "{}",
        reply.content_type
    );

    let problem = reply.json();
    assert_eq!(problem["type"], json!("urn:keel:error:KER_NOT_FOUND"));
    assert_eq!(problem["title"], json!("Not Found"));
    assert_eq!(problem["status"], json!(404));
    assert_eq!(problem["code"], json!("KER_NOT_FOUND"));
    assert_eq!(
        problem["instance"],
        json!("/api/availability/not-a-uuid"),
        "the request path becomes the problem instance"
    );
    assert!(
        problem["detail"].is_string(),
        "RFC-7807 detail is required: {}",
        problem
    );
    // The Envelope vocabulary never leaks into the problem body.
    assert!(problem.get("ok").is_none(), "{}", problem);
    assert!(problem.get("error").is_none(), "{}", problem);
}

#[tokio::test]
async fn explicit_accept_header_beats_the_deployment_default() {
    // Opt-in on, but the client explicitly wants the Envelope.
    let mut fx = fixture();
    fx.state.problem_json = true;
    let router = router_for(&fx);
    let reply = get(
        &router,
        "/api/availability/not-a-uuid",
        Some("application/json"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND, "{}", reply.body);
    assert!(
        reply.content_type.contains("application/json")
            && !reply.content_type.contains("problem"),
        "{}",
        reply.content_type
    );
    assert_eq!(reply.json()["ok"], json!(false));
    assert_eq!(reply.json()["error"]["code"], json!("KER_NOT_FOUND"));

    // Opt-in off, but the client explicitly asks for the problem form.
    let fx = fixture();
    let router = router_for(&fx);
    let reply = get(
        &router,
        "/api/availability/not-a-uuid",
        Some("application/problem+json"),
    )
    .await;
    assert_eq!(reply.status, StatusCode::NOT_FOUND, "{}", reply.body);
    assert!(
        reply.content_type.contains("application/problem+json"),
        "{}",
        reply.content_type
    );
    assert_eq!(reply.json()["status"], json!(404));
}

#[tokio::test]
async fn web_plane_html_clients_keep_the_error_view_under_the_problem_opt_in() {
    let mut fx = fixture();
    fx.state.problem_json = true;
    let router = router_for(&fx);

    // The deployment default must not turn browsers away from the view.
    let browser = get(&router, "/web/availability/not-a-uuid", None).await;
    assert_eq!(browser.status, StatusCode::NOT_FOUND, "{}", browser.body);
    assert!(
        browser.content_type.starts_with("text/html"),
        "{}",
        browser.content_type
    );
    assert!(
        browser.body.contains(r#"class="error-404""#),
        "{}",
        browser.body
    );

    // An explicit machine client on the same web route gets the problem.
    let machine = get(
        &router,
        "/web/availability/not-a-uuid",
        Some("application/problem+json"),
    )
    .await;
    assert_eq!(machine.status, StatusCode::NOT_FOUND, "{}", machine.body);
    assert!(
        machine.content_type.contains("application/problem+json"),
        "{}",
        machine.content_type
    );
    assert_eq!(machine.json()["status"], json!(404));
}

// ---------------------------------------------------------- app.debug gate

#[tokio::test]
async fn app_debug_gate_hides_internal_detail_when_closed() {
    // No engine: the request degrades to the JSON body instead of failing.
    let mut fx = fixture();
    fx.state.views = None;
    let router = router_for(&fx);

    let reply = get(&router, "/web/broken", None).await;
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR, "{}", reply.body);
    let body = reply.json();
    assert_eq!(body["ok"], json!(false));
    assert_eq!(body["error"]["code"], json!("KER_INTERNAL"));
    assert!(
        body["error"].get("debug").is_none(),
        "debug detail never renders when app.debug is off: {}",
        body
    );
    assert!(
        !reply.body.contains("views are not configured"),
        "internal context must not leak: {}",
        reply.body
    );
}

#[tokio::test]
async fn app_debug_gate_renders_redacted_internal_detail_when_open() {
    let mut fx = fixture();
    fx.state.views = None;
    fx.state.debug = true;
    let router = router_for(&fx);

    let reply = get(&router, "/web/broken", None).await;
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR, "{}", reply.body);
    let debug = &reply.json()["error"]["debug"];
    assert_eq!(debug["internal"], json!("views are not configured"), "{}", debug);
}

#[tokio::test]
async fn app_debug_detail_reaches_the_error_view_too() {
    // Closed: the 500 page renders with no internal context.
    let fx = fixture();
    let router = router_for(&fx);
    let reply = get(&router, "/web/broken", None).await;
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR, "{}", reply.body);
    assert!(
        reply.content_type.starts_with("text/html"),
        "{}",
        reply.content_type
    );
    assert!(
        reply.body.contains(r#"class="error-500""#),
        "{}",
        reply.body
    );
    assert!(
        !reply.body.contains("view pages.missing failed"),
        "internal context must not reach the page: {}",
        reply.body
    );

    // Open: the same page carries the redacted detail.
    let mut fx = fixture();
    fx.state.debug = true;
    let router = router_for(&fx);
    let reply = get(&router, "/web/broken", None).await;
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR, "{}", reply.body);
    assert!(
        reply.body.contains("view pages.missing failed"),
        "{}",
        reply.body
    );
}

#[test]
fn debug_detail_redacts_secrets_through_the_logging_list() {
    let err = keel_foundation::AppError::internal(
        "login failed password=hunter2 api_key=sk-live-1 token=abc123",
    )
    .with_correlation(keel_foundation::CorrelationId::new());

    let detail = keel_api::routes::debug_detail(&err).expect("internal detail renders");
    let text = detail.to_string();
    assert!(!text.contains("hunter2"), "{text}");
    assert!(!text.contains("sk-live-1"), "{text}");
    assert!(!text.contains("abc123"), "{text}");
    assert!(text.contains("[REDACTED]"), "{text}");
    assert!(detail["correlation"].is_string(), "{text}");
}

// -------------------------------------------------------------- registry

#[tokio::test]
async fn registry_overrides_and_renderer_output_reach_the_wire() {
    let fx = fixture();
    fx.state
        .error_renders
        .set_status("KER_NOT_FOUND", StatusCode::GONE);
    fx.state.error_renders.set_view("KER_NOT_FOUND", "errors.404");
    fx.state
        .error_renders
        .register("KER_NOT_FOUND", std::sync::Arc::new(|err: &keel_foundation::AppError| {
            Some(json!({ "docs": format!("https://docs.test/{}", err.code) }))
        }));
    let router = router_for(&fx);

    let reply = get(&router, "/api/availability/not-a-uuid", None).await;
    assert_eq!(reply.status, StatusCode::GONE, "{}", reply.body);
    let body = reply.json();
    assert_eq!(body["error"]["code"], json!("KER_NOT_FOUND"));
    assert_eq!(
        body["error"]["meta"]["docs"],
        json!("https://docs.test/KER_NOT_FOUND"),
        "renderer output lands in error.meta: {}",
        body
    );
}

#[tokio::test]
async fn report_only_codes_keep_the_status_but_serve_the_generic_body() {
    let fx = fixture();
    fx.state.error_renders.set_renderable("KER_NOT_FOUND", false);
    let router = router_for(&fx);

    let reply = get(&router, "/api/availability/not-a-uuid", None).await;
    assert_eq!(
        reply.status,
        StatusCode::NOT_FOUND,
        "the mapped status is preserved: {}",
        reply.body
    );
    let body = reply.json();
    assert_eq!(body["error"]["code"], json!("KER_INTERNAL"));
    assert_eq!(body["error"]["message"], json!("internal error"));
    assert!(body["error"].get("debug").is_none(), "{}", body);
}
