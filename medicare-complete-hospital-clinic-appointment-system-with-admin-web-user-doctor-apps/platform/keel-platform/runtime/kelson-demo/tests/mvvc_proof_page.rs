//! MVA-004 — the first live MVVC page, wired end to end in the runtime.
//!
//! One appointments page serves **bound** HTML: the manifest route declares
//! both the view and its view model (`view` + `view_model`, MVA-005), the boot
//! registers the `BookAppointmentVm` binding and refuses to start if the
//! declaration points at nothing, and the template reads only through the
//! `@vm` binder. This suite proves the whole chain at the runtime level —
//! `@vm` binds, `@command` posts, `@csrf` mints, and a command POST round-trips
//! back into the bound page — against the REAL shipped template and the REAL
//! compiled graph, not a fixture copy.
//!
//! MVA-006 is checked here too: the shipped `resources/views/errors/*.html`
//! templates are rendered straight from the app-facing root, so the error
//! surface is proven present and parseable, not merely copied.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

struct Fixture {
    state: kelson_demo::routes::AppState,
    graph: kelson_registry::PlatformGraph,
}

/// Boot the harness, drop the REAL shipped book page under the fixture root,
/// and install the command registry (production boot does this; the raw
/// harness does not) so `@command` can bind. The graph the harness returns is
/// the compiled real manifest, so the `/appointments/book` lane is live.
fn fixture() -> Fixture {
    let dir = std::env::temp_dir().join(format!("kelson-mvvc-proof-{}", uuid::Uuid::now_v7()));
    let pages = dir.join("views").join("pages");
    std::fs::create_dir_all(&pages).expect("views fixture dir");
    std::fs::write(
        pages.join("book-appointment.html"),
        include_str!("../../../resources/views/pages/book-appointment.html"),
    )
    .expect("shipped book page copies into the fixture");

    let (state, graph, _store) = kelson_demo::build_app_state(&dir, true, true);
    let engine = state.views.as_ref().expect("views configured");
    kelson_demo::install_command_registry(engine, &graph);
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

async fn get(router: &axum::Router, uri: &str, cookie: Option<&str>) -> Reply {
    let mut builder = Request::builder().method(Method::GET).uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    send(router, builder.body(Body::empty()).expect("request builds")).await
}

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

/// POST a booking through the declared command endpoint with the caller's
/// session and the `x-csrf-token` the rendered page minted.
async fn book_with_token(
    router: &axum::Router,
    cookie: &str,
    token: &str,
    patient: &str,
    slot: &str,
) -> Reply {
    let body = serde_json::json!({ "patient": patient, "slot": slot });
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/appointments")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, cookie)
        .header("x-csrf-token", token)
        .body(Body::from(body.to_string()))
        .expect("request builds");
    send(router, req).await
}

/// Pull the first rendered `_token` value out of the page (`@csrf` / the
/// `@command` form both emit one; they carry the same session token).
fn extracted_token(body: &str) -> Option<String> {
    let after = body.split("name=\"_token\" value=\"").nth(1)?;
    let value = after.split('"').next()?;
    (!value.is_empty()).then(|| value.to_string())
}

// ------------------------------------------------------------- the bound page

#[tokio::test]
async fn the_book_page_serves_bound_view_model_html() {
    let fx = fixture();
    let router = router(&fx);
    let reply = get(&router, "/appointments/book?date=2026-11-15", None).await;

    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert!(
        reply.content_type.starts_with("text/html"),
        "{}",
        reply.content_type
    );
    // `@vm` binds the declared model: the operation result reaches the view
    // through the `vm` namespace, not as loose context keys.
    assert!(
        reply.body.contains(r#"<p class="date">2026-11-15</p>"#),
        "vm.date must render: {}",
        reply.body
    );
    assert!(
        reply.body.contains(r#"<p class="booked">0</p>"#),
        "vm.appointments_booked must render: {}",
        reply.body
    );
    assert!(
        !reply.body.contains("{{"),
        "the template renders, it is not echoed: {}",
        reply.body
    );
    // `@command` renders a CSRF form — for signed-in callers only. A guest
    // has no session to mint a token from, the framework refuses a tokenless
    // command form, and the template gates the lane behind `@auth`, so the
    // guest sees the sign-in hint and NO form.
    assert!(
        !reply.body.contains(
            r#"<form method="POST" action="/api/v1/appointments" class="kelson-command">"#
        ),
        "a guest sees no command form (no token to carry): {}",
        reply.body
    );
    assert!(
        reply.body.contains(r#"<p class="sign-in">"#),
        "the @auth @else branch serves the sign-in hint: {}",
        reply.body
    );
    assert!(
        reply.body.contains(r#"<span class="guest">guest</span>"#),
        "the @guest branch renders: {}",
        reply.body
    );
}

#[tokio::test]
async fn the_boot_verifies_the_declared_binding_on_the_real_graph() {
    let fx = fixture();
    let engine = fx.state.views.as_ref().expect("views configured");
    // The real compiled graph declares BookAppointmentVm on /appointments/book
    // and the app registered that binding — so the boot gate passes.
    kelson_demo::verify_declared_view_models(engine, &fx.graph)
        .expect("the shipped declaration resolves to a registered model");
}

#[tokio::test]
async fn a_declaration_pointing_at_nothing_refuses_the_boot() {
    let fx = fixture();
    let engine = fx.state.views.as_ref().expect("views configured");
    // The same view bound to a model the app never registered: a manifest
    // that names GhostVm must not boot.
    let raw = format!(
        "{}{}",
        include_str!("../../../applications/appointments/module.toml"),
        r#"
[[routes]]
method = "GET"
path = "/appointments/ghost"
operation = "availability"
capability = "availability"
name = "ghost-page"
plane = "web"
response = "html"
view = "pages.book-appointment"
view_model = "GhostVm"
public = true
"#
    );
    let manifest = kelson_registry::ModuleManifest::from_toml(&raw).expect("ghost manifest parses");
    let graph =
        kelson_registry::PlatformGraph::compile(vec![manifest]).expect("ghost graph compiles");
    let err = kelson_demo::verify_declared_view_models(engine, &graph)
        .expect_err("an unbound declared model refuses the boot");
    let msg = err.to_string();
    assert!(
        msg.contains("GhostVm"),
        "the refusal names the model: {msg}"
    );
}

#[tokio::test]
async fn an_authenticated_page_mints_a_csrf_token_the_command_post_carries() {
    let fx = fixture();
    let router = router(&fx);
    let email = format!("mvvc-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&router, &email).await;

    let page = get(&router, "/appointments/book?date=2026-12-01", Some(&cookie)).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    // Comment prose in the template mentions "guest" in BOTH branches, so
    // the discriminator is markup, not the word: the auth side must carry
    // the command form and must not render the guest marker.
    assert!(
        page.body.contains(r#"class="kelson-command""#),
        "the @auth branch renders the command form for a signed-in caller: {}",
        page.body
    );
    assert!(
        !page.body.contains(r#"<span class="guest">"#),
        "an authed caller is not a guest: {}",
        page.body
    );
    let token = extracted_token(&page.body).expect("the page mints a CSRF token");

    // The command POST carries the minted token and lands.
    let booked = book_with_token(&router, &cookie, &token, "Proof Patient", "2026-12-31").await;
    assert_eq!(booked.status, StatusCode::OK, "{}", booked.body);

    // The bound page now reflects the booking — the read model refreshed.
    let after = get(&router, "/appointments/book?date=2026-12-01", Some(&cookie)).await;
    assert!(
        after.body.contains(r#"<p class="booked">1</p>"#),
        "the bound count reflects the round-trip: {}",
        after.body
    );
}

#[tokio::test]
async fn the_booking_round_trips_from_a_fresh_guest_render() {
    // Proves the declared command endpoint needs a session: an
    // unauthenticated POST (no cookie) to the tenant-plane book route is a
    // 401 envelope, so the page's form only works once `@auth` supplies a
    // session and `@csrf` mints a token for it.
    let fx = fixture();
    let router = router(&fx);
    let body = serde_json::json!({ "patient": "No Session", "slot": "2026-12-31" });
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/appointments")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("request builds");
    let reply = send(&router, req).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{}", reply.body);
    let body: Value = serde_json::from_str(&reply.body).expect("envelope json");
    assert_eq!(body["ok"], Value::Bool(false));
}

// ------------------------------------------------------- MVA-006 error views

/// The shipped app-facing root must actually resolve and render its error
/// templates — not merely exist on disk. Build an engine pointed straight at
/// `resources/views` and render each error page.
#[test]
fn the_shipped_error_views_render_from_the_app_root() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/views");
    let engine = kelson_views::ViewsEngine::new(kelson_views::ViewsConfig {
        paths: vec![root],
        cache_dir: None,
        env: "dev".into(),
    });
    assert!(engine.missing_roots().is_empty(), "the app root exists");

    for (name, ctx) in [
        (
            "errors.404",
            serde_json::json!({
                "status": 404, "title": "Not Found",
                "message": "The page you want is not here.", "path": "/appointments/ghost",
            }),
        ),
        (
            "errors.500",
            serde_json::json!({
                "status": 500, "title": "Server Error",
                "message": "Something went wrong.", "path": "/appointments/book",
            }),
        ),
        (
            "errors.maintenance",
            serde_json::json!({
                "status": 503, "title": "Down for maintenance",
                "message": "We will be back shortly.", "path": "/",
            }),
        ),
    ] {
        let html = engine
            .render(name, ctx)
            .unwrap_or_else(|e| panic!("{name} must render: {e}"));
        assert!(
            !html.contains("{{"),
            "{name}: template echoed, not rendered: {html}"
        );
        assert!(html.contains("<!doctype html>"), "{name}: {html}");
    }

    // The starter placeholders were substituted at ship time: the brand is
    // real, the `__PROJECT_NAME__` token is gone.
    let not_found = engine
        .render(
            "errors.404",
            serde_json::json!({
                "status": 404, "title": "Not Found",
                "message": "gone", "path": "/x",
            }),
        )
        .expect("404 renders");
    assert!(
        not_found.contains("Kelson"),
        "the brand landed: {not_found}"
    );
    assert!(
        !not_found.contains("__PROJECT"),
        "no unsubstituted placeholder: {not_found}"
    );
}

// ------------------------------------------------------- E1 build-time lint

/// E1/C07: the reference app's own bound page must lint clean against the
/// REAL registry. `view:cache` and `kelson check` run the binding-free half
/// (root reads) because the host-side registry does not exist in a tool
/// process (C17); this suite is where the declared-path half runs, with the
/// same `BookAppointmentVm` the boot installs.
#[test]
fn the_shipped_bound_page_passes_the_build_time_lint() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../resources/views");
    let engine = kelson_views::ViewsEngine::new(kelson_views::ViewsConfig {
        paths: vec![root],
        cache_dir: None,
        env: "dev".into(),
    });
    kelson_demo::install_view_registry(&engine);
    let linted = engine
        .lint_all()
        .unwrap_or_else(|e| panic!("every shipped template must lint clean: {e}"));
    assert!(linted >= 1, "the book page is discovered");
    assert!(
        engine
            .lint_roots_all()
            .expect("the binding-free half is green")
            >= 1,
        "the book page is discovered by the cache-time lint"
    );
}
