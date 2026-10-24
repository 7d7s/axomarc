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
  @csrf
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
    state: kelson_demo::routes::AppState,
    graph: kelson_registry::PlatformGraph,
}

/// Boot the harness with fixture templates under `<dir>/views`. With
/// `with_error_view = false` the engine has no `errors/404` page, so the
/// error path must fall back to the JSON envelope.
fn fixture(with_error_view: bool) -> Fixture {
    let dir = std::env::temp_dir().join(format!("kelson-views-it-{}", uuid::Uuid::now_v7()));
    let pages = dir.join("views").join("pages");
    std::fs::create_dir_all(&pages).expect("views fixture dir");
    std::fs::write(pages.join("availability.html"), AVAILABILITY_VIEW).expect("view writes");
    if with_error_view {
        let errors = dir.join("views").join("errors");
        std::fs::create_dir_all(&errors).expect("errors fixture dir");
        std::fs::write(errors.join("404.html"), ERROR_404_VIEW).expect("error view writes");
    }
    let (state, _, _) = kelson_demo::build_app_state(&dir, true, true);
    let raw = format!(
        "{}{EXTRA_ROUTES}",
        include_str!("../../../applications/appointments/module.toml")
    );
    let manifest =
        kelson_registry::ModuleManifest::from_toml(&raw).expect("fixture manifest parses");
    let graph =
        kelson_registry::PlatformGraph::compile(vec![manifest]).expect("fixture graph compiles");
    Fixture { state, graph }
}

fn router(fx: &Fixture) -> axum::Router {
    let state = fx.state.clone();
    let router = kelson_demo::routes::router(state.clone())
        .merge(kelson_demo::graph_routes::graph_router(
            state.clone(),
            &fx.graph,
        ))
        .merge(kelson_demo::auth_routes::router(state.clone()));
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
        serde_json::json!({ "email": email, "password": "Correct-horse-99", "tenant": "dev" });
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
        .find(|c| c.starts_with("kelson_session="))
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

/// MVVC (ADR 0021): the request's CSRF token reaches the view, so a
/// form the view renders can carry the token the session issued. A guest
/// renders an empty `@csrf` field.
#[tokio::test]
async fn the_csrf_token_reaches_the_view() {
    let fx = fixture(true);
    let router = router(&fx);
    let email = format!("csrf-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&router, &email).await;

    let reply = get_with_cookie(&router, "/web/appointments", &cookie).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert!(
        reply.body.contains("name=\"_token\""),
        "the view received the CSRF token: {}",
        reply.body
    );

    // No session -> no token -> the field renders empty.
    let anon = get(&router, "/web/availability", None).await;
    assert_eq!(anon.status, StatusCode::OK, "{}", anon.body);
    assert!(
        anon.body.contains("name=\"_token\"") && anon.body.contains("value=\"\""),
        "a guest carries no CSRF token: {}",
        anon.body
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
    assert_eq!(body["error"]["code"].as_str(), Some("KSN_NOT_FOUND"));
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
    assert_eq!(body["error"]["code"].as_str(), Some("KSN_NOT_FOUND"));
}

#[tokio::test]
async fn html_route_without_an_engine_degrades_to_a_private_500_envelope() {
    let mut fx = fixture(true);
    fx.state.views = None; // harness opt-out path
    let router = router(&fx);
    let reply = get(&router, "/web/availability?date=2026-09-01", None).await;

    assert_eq!(
        reply.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "{}",
        reply.body
    );
    assert!(
        reply.content_type.contains("application/json"),
        "{}",
        reply.content_type
    );
    let body = reply.json();
    assert_eq!(body["ok"], Value::Bool(false));
    assert_eq!(body["error"]["code"].as_str(), Some("KSN_INTERNAL"));
    // The public message never leaks the internal detail.
    assert!(!reply.body.contains("views are not configured"));
}

// ------------------------------------------------------- i18n (Wave 6 P3)

/// Wave 6 P3: the availability page localized — `_locale` is the negotiated
/// locale injected at the edge, and `@lang` / the `trans` filter resolve
/// through the catalog loaded from the fixture `<temp>/lang` directory.
const I18N_AVAILABILITY_VIEW: &str = r#"<!doctype html>
<html lang="{{ _locale }}">
<head><meta charset="utf-8"><title>Availability</title></head>
<body>
  <p class="locale">{{ _locale }}</p>
  <p class="msg">{{ "validation.string" | trans }}</p>
  <p class="lang">@lang("notifications.appointment_booked")</p>
  <p class="date">{{ date }}</p>
</body>
</html>
"#;

/// Same graph as `fixture`, plus locale files dropped BEFORE the app builds
/// (`build_app_state` reads `<dir>/lang`, absent = empty catalog) and a view
/// that consumes the locale injections.
fn i18n_fixture() -> Fixture {
    let dir = std::env::temp_dir().join(format!("kelson-i18n-it-{}", uuid::Uuid::now_v7()));
    let pages = dir.join("views").join("pages");
    std::fs::create_dir_all(&pages).expect("views fixture dir");
    std::fs::write(pages.join("availability.html"), I18N_AVAILABILITY_VIEW).expect("view writes");
    let lang = dir.join("lang");
    std::fs::create_dir_all(&lang).expect("lang fixture dir");
    std::fs::write(
        lang.join("en.toml"),
        "[validation]\nrequired = \"{attribute} is required\"\nstring = \"must be a string\"\n\n[notifications]\nappointment_booked = \"Appointment booked for {patient}\"\n",
    )
    .expect("en lang fixture writes");
    std::fs::write(
        lang.join("fr.toml"),
        "[validation]\nrequired = \"{attribute} est requis\"\nstring = \"doit être une chaîne\"\n\n[notifications]\nappointment_booked = \"Rendez-vous réservé pour {patient}\"\n",
    )
    .expect("fr lang fixture writes");
    let (state, _, _) = kelson_demo::build_app_state(&dir, true, true);
    let raw = format!(
        "{}{EXTRA_ROUTES}",
        include_str!("../../../applications/appointments/module.toml")
    );
    let manifest =
        kelson_registry::ModuleManifest::from_toml(&raw).expect("fixture manifest parses");
    let graph =
        kelson_registry::PlatformGraph::compile(vec![manifest]).expect("fixture graph compiles");
    Fixture { state, graph }
}

/// GET with an `Accept-Language` header (the negotiation input).
async fn get_lang(router: &axum::Router, uri: &str, accept_language: &str) -> Reply {
    let req = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::ACCEPT_LANGUAGE, accept_language)
        .body(Body::empty())
        .expect("request builds");
    send(router, req).await
}

/// Wave 6 P3: `Accept-Language` reaches the view layer — `_locale` carries
/// the negotiated locale and both lookup forms resolve through it; an
/// unsupported language and no header at all fall back to the default.
#[tokio::test]
async fn accept_language_negotiates_the_view_locale_and_messages() {
    let fx = i18n_fixture();
    let router = router(&fx);

    let fr = get_lang(
        &router,
        "/web/availability?date=2026-09-01",
        "fr-CA, fr;q=0.9, en;q=0.5",
    )
    .await;
    assert_eq!(fr.status, StatusCode::OK, "{}", fr.body);
    assert!(fr.body.contains(r#"<html lang="fr">"#), "{}", fr.body);
    assert!(fr.body.contains("doit être une chaîne"), "{}", fr.body);
    assert!(
        fr.body.contains("Rendez-vous réservé pour {patient}"),
        "{}",
        fr.body
    );

    // No preference → configured default (en), same keys resolved.
    let en = get(&router, "/web/availability?date=2026-09-01", None).await;
    assert_eq!(en.status, StatusCode::OK, "{}", en.body);
    assert!(en.body.contains(r#"<html lang="en">"#), "{}", en.body);
    assert!(en.body.contains("must be a string"), "{}", en.body);

    // Unsupported language → default, never a broken render.
    let de = get_lang(&router, "/web/availability?date=2026-09-01", "de").await;
    assert_eq!(de.status, StatusCode::OK, "{}", de.body);
    assert!(de.body.contains(r#"<html lang="en">"#), "{}", de.body);
}

/// Wave 6 P3: edge validation wording follows the negotiated locale — the
/// manifest's `required` failure renders the catalog's `validation.required`
/// for `Accept-Language: fr` and the English default without it (the 422
/// `fields` wire shape itself is unchanged).
#[tokio::test]
async fn validation_messages_follow_the_negotiated_locale() {
    let fx = i18n_fixture();
    let router = router(&fx);
    let email = format!("i18n-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&router, &email).await;

    let post = |accept_language: Option<&str>| {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/appointments")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::COOKIE, cookie.as_str());
        if let Some(lang) = accept_language {
            builder = builder.header(header::ACCEPT_LANGUAGE, lang);
        }
        builder
            .body(Body::from(
                serde_json::json!({ "slot": "2026-09-01T10:00Z" }).to_string(),
            ))
            .expect("request builds")
    };

    let fr = send(&router, post(Some("fr"))).await;
    assert_eq!(fr.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", fr.body);
    assert_eq!(fr.json()["error"]["code"].as_str(), Some("KSN_VALIDATION"));
    assert_eq!(
        fr.json()["error"]["meta"]["fields"]["patient"][0].as_str(),
        Some("patient est requis"),
        "{}",
        fr.body
    );

    // No header → the English default, byte-identical to the built-in.
    let en = send(&router, post(None)).await;
    assert_eq!(en.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", en.body);
    assert_eq!(
        en.json()["error"]["meta"]["fields"]["patient"][0].as_str(),
        Some("patient is required"),
        "{}",
        en.body
    );
}
