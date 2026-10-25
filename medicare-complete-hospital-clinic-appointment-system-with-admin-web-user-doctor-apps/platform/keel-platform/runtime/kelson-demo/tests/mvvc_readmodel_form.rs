//! MVA-007 — the runtime's first read-model page and first bridged form.
//!
//! The audit found zero application consumers for three shipped MVVC
//! layers: no `ReadModelDef` anywhere, no `[[validation]]` spec ever
//! becoming a form, and the route edge building one head with one
//! hard-coded identity. This suite proves the three consumers at the
//! runtime level, against the REAL shipped declarations:
//!
//! * a `ReadModelDef` registered and projected by a `ReadModelStore`,
//!   rendered by a page that carries its staleness surface — and that
//!   REFUSES honestly when the boot installed no store.
//! * the appointments manifest's `book` specs parsed into a `BridgedForm`
//!   that renders, validates and round-trips per-field errors.
//! * the route edge authoring a per-page head from the view the route
//!   declared, instead of one title riding every page.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use tower::ServiceExt;

struct Fixture {
    state: kelson_demo::routes::AppState,
    graph: kelson_registry::PlatformGraph,
    store: std::sync::Arc<kelson_appointments_contract::AppointmentStore>,
}

/// Boot the harness with a fixture root that carries a book page reading
/// the reserved `_head` key — the head-authoring lane (`/appointments/book`
/// renders through the SAME `page_head` the MVA-007 pages use).
fn fixture() -> Fixture {
    let dir = std::env::temp_dir().join(format!("kelson-mvvc-readmodel-{}", uuid::Uuid::now_v7()));
    let pages = dir.join("views").join("pages");
    std::fs::create_dir_all(&pages).expect("views fixture dir");
    std::fs::write(
        pages.join("book-appointment.html"),
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>{{ _head.title }}</title>
</head>
<body>
  @vm
  <main class="book-appointment">
    <p class="date">{{ vm.date }}</p>
    <p class="booked">{{ vm.appointments_booked }}</p>
  </main>
</body>
</html>
"#,
    )
    .expect("fixture book page copies in");

    let (state, graph, store) = kelson_demo::build_app_state(&dir, true, true);
    let engine = state.views.as_ref().expect("views configured");
    kelson_demo::install_command_registry(engine, &graph);
    Fixture {
        state,
        graph,
        store,
    }
}

fn router_with(state: kelson_demo::routes::AppState, graph: &kelson_registry::PlatformGraph) -> axum::Router {
    let router = kelson_demo::routes::router(state.clone())
        .merge(kelson_demo::graph_routes::graph_router(state.clone(), graph))
        .merge(kelson_demo::auth_routes::router(state.clone()));
    state
        .sessions
        .as_ref()
        .expect("sessions enabled")
        .as_ref()
        .clone()
        .apply(router)
}

fn router(fx: &Fixture) -> axum::Router {
    router_with(fx.state.clone(), &fx.graph)
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

/// POST `application/x-www-form-urlencoded` back at a page.
async fn post_form(router: &axum::Router, uri: &str, cookie: Option<&str>, body: String) -> Reply {
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    send(router, builder.body(Body::from(body)).expect("request builds")).await
}

/// Pull the rendered `_token` value out of the form page.
fn extracted_token(body: &str) -> Option<String> {
    let after = body.split("name=\"_token\" value=\"").nth(1)?;
    let value = after.split('"').next()?;
    (!value.is_empty()).then(|| value.to_string())
}

fn urlencode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn form_body(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

/// Seed one booked appointment into the tenant the boot reports for.
fn seed(fx: &Fixture, patient: &str, slot: &str) {
    let tenant = fx.state.default_tenant_id.expect("a default tenant");
    fx.store
        .insert(kelson_appointments_contract::Appointment {
            id: uuid::Uuid::now_v7(),
            tenant,
            patient: patient.to_string(),
            slot: slot.to_string(),
            status: kelson_appointments_contract::AppointmentStatus::Booked,
            version: 1,
        })
        .expect("seed appointment");
}

// ------------------------------------------------------- the read-model page

#[tokio::test]
async fn the_read_model_page_projects_booked_appointments() {
    let fx = fixture();
    seed(&fx, "A. Patient", "2026-11-15T10:00:00Z");
    let router = router(&fx);

    let reply = get(&router, "/mvc/reports/daily", None).await;

    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert!(
        reply.content_type.starts_with("text/html"),
        "{}",
        reply.content_type
    );
    // The page renders the PROJECTION — the row the store materialised —
    // under the read model's own name, not a fabricated row.
    assert!(
        reply.body.contains("appointments_daily"),
        "the declared read model names itself: {}",
        reply.body
    );
    assert!(
        reply.body.contains("A. Patient"),
        "the projected patient reaches the page: {}",
        reply.body
    );
    assert!(
        reply.body.contains("2026-11-15T10:00:00Z"),
        "the projected slot reaches the page: {}",
        reply.body
    );
    // The staleness surface rides with the data: a refresh just ran, so the
    // page reports Fresh rather than a state it never measured.
    assert!(
        reply.body.contains("Fresh"),
        "the staleness surface reports the just-run refresh: {}",
        reply.body
    );
    // Per-page head authoring: this page authors its OWN identity.
    assert!(
        reply
            .body
            .contains("<title>Appointments daily report · Kelson</title>"),
        "the page titles itself: {}",
        reply.body
    );
    assert!(
        !reply.body.contains("{{"),
        "the template renders, it is not echoed: {}",
        reply.body
    );
}

#[tokio::test]
async fn the_daily_page_degrades_honestly_without_a_store() {
    let fx = fixture();
    seed(&fx, "A. Patient", "2026-11-15T10:00:00Z");

    // A boot that registered the def but installed no projection store
    // (no model backend): the page must REFUSE, never render rows it never
    // materialised.
    let mut state = fx.state.clone();
    state.read_models = std::sync::Arc::new(kelson_models::ReadModelRegistry::new());
    let router = router_with(state, &fx.graph);

    let reply = get(&router, "/mvc/reports/daily", None).await;

    assert_eq!(
        reply.status,
        StatusCode::INTERNAL_SERVER_ERROR,
        "{}",
        reply.body
    );
    assert!(
        reply.body.contains(r#""ok":false"#),
        "the refusal is an honest error envelope: {}",
        reply.body
    );
    assert!(
        !reply.body.contains("A. Patient"),
        "a storeless boot never fabricates the projection: {}",
        reply.body
    );
}

// ------------------------------------------------------------ the form bridge

/// The form is built from the SHIPPED manifest's `[[validation]]` specs —
/// a spec change changes the form with no code edit here.
#[test]
fn the_book_form_is_built_from_the_shipped_validation_spec() {
    let form = kelson_demo::mvvc_pages::book_form();

    let names: Vec<&str> = form.fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["patient", "slot", "photo"],
        "the manifest's `book` field list is the form's field list"
    );
    assert_eq!(form.action, "/mvc/book-form");
    assert_eq!(form.method, "POST");

    let patient = &form.fields[0];
    assert!(patient.required, "the spec's `required` drives the field");
    assert!(
        patient.validation_rules.contains(&kelson_views::ValidationRule::Required),
        "{:?}",
        patient.validation_rules
    );
    assert!(
        patient
            .validation_rules
            .contains(&kelson_views::ValidationRule::MaxLength(120)),
        "`max:120` maps to the typed rule: {:?}",
        patient.validation_rules
    );
    // `unique:Appointment` / `before:` are wire-plane rules: they ride as
    // named custom rules so the declaration stays visible on the field.
    let slot = &form.fields[1];
    assert_eq!(slot.field_type, "date", "the spec's `date` rule picks the control");
    assert!(
        slot
            .validation_rules
            .iter()
            .any(|r| matches!(r, kelson_views::ValidationRule::Custom(name) if name == "unique:Appointment")),
        "`unique:Appointment` rides as a named custom rule: {:?}",
        slot.validation_rules
    );

    let photo = &form.fields[2];
    assert!(!photo.required, "`photo` is optional in the spec");
    assert_eq!(
        photo.field_type, "file",
        "the attachment rules pick the file control"
    );
}

#[tokio::test]
async fn the_book_form_renders_from_the_shipped_validation_spec() {
    let fx = fixture();
    let router = router(&fx);
    let cookie = register(&router, "form-render@example.test").await;

    let reply = get(&router, "/mvc/book-form", Some(&cookie)).await;

    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert!(
        reply.content_type.starts_with("text/html"),
        "{}",
        reply.content_type
    );
    for field in ["patient", "slot", "photo"] {
        assert!(
            reply.body.contains(&format!("name=\"{field}\"")),
            "the spec's {field} field renders: {}",
            reply.body
        );
    }
    assert!(
        reply.body.contains(r#"name="_token""#),
        "the session's token rides the form: {}",
        reply.body
    );
    assert!(
        reply.body.contains(r#"type="file""#),
        "the attachment spec picks the file control: {}",
        reply.body
    );
    assert!(
        reply
            .body
            .contains("<title>Book an appointment · Kelson</title>"),
        "the form page titles itself: {}",
        reply.body
    );
    assert!(
        !reply.body.contains("{{"),
        "the template renders, it is not echoed: {}",
        reply.body
    );
}

#[tokio::test]
async fn a_form_without_a_session_is_refused() {
    let fx = fixture();
    let router = router(&fx);

    // A guest's GET renders no token, and the POST lane is session-bound:
    // the refusal is the uniform 401, never a half-validated submission.
    let reply = post_form(&router, "/mvc/book-form", None, form_body(&[])).await;

    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{}", reply.body);
    assert!(
        reply.body.contains(r#""ok":false"#),
        "{}",
        reply.body
    );
}

#[tokio::test]
async fn a_form_submission_with_a_wrong_token_is_refused() {
    let fx = fixture();
    let router = router(&fx);
    let cookie = register(&router, "form-token@example.test").await;

    let reply = post_form(
        &router,
        "/mvc/book-form",
        Some(&cookie),
        form_body(&[
            ("_token", "not-the-minted-token"),
            ("patient", "A. Patient"),
            ("slot", "2026-11-15"),
        ]),
    )
    .await;

    assert_eq!(reply.status, StatusCode::FORBIDDEN, "{}", reply.body);
    assert!(
        reply.body.contains(r#""ok":false"#),
        "{}",
        reply.body
    );
}

#[tokio::test]
async fn an_invalid_form_submission_round_trips_field_errors() {
    let fx = fixture();
    let router = router(&fx);
    let cookie = register(&router, "form-invalid@example.test").await;

    let page = get(&router, "/mvc/book-form", Some(&cookie)).await;
    let token = extracted_token(&page.body).expect("the form mints a token");

    let reply = post_form(
        &router,
        "/mvc/book-form",
        Some(&cookie),
        form_body(&[
            ("_token", &token),
            ("patient", ""),
            ("slot", "not-a-date"),
            ("photo", ""),
        ]),
    )
    .await;

    // A validation failure re-renders the form — it does not 422 the page
    // the browser is standing on.
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert!(
        reply.body.contains("Patient is required"),
        "the declared rule's message reaches the field: {}",
        reply.body
    );
    assert!(
        reply.body.contains(r#"value="not-a-date""#),
        "the submitted value round-trips back onto the field: {}",
        reply.body
    );
    assert!(
        reply.body.contains("name=\"_token\" value=\"") && reply.body.contains(&token),
        "the session token survives the round trip: {}",
        reply.body
    );
    assert!(
        !reply.body.contains("accepted"),
        "nothing was accepted: {}",
        reply.body
    );
}

#[tokio::test]
async fn a_valid_form_submission_accepts_the_round_trip() {
    let fx = fixture();
    let router = router(&fx);
    let cookie = register(&router, "form-valid@example.test").await;

    let page = get(&router, "/mvc/book-form", Some(&cookie)).await;
    let token = extracted_token(&page.body).expect("the form mints a token");

    let reply = post_form(
        &router,
        "/mvc/book-form",
        Some(&cookie),
        form_body(&[
            ("_token", &token),
            ("patient", "A. Patient"),
            ("slot", "2026-11-15"),
            ("photo", ""),
        ]),
    )
    .await;

    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert!(
        reply.body.contains(r#"class="accepted""#),
        "the accepted branch renders: {}",
        reply.body
    );
    assert!(
        reply.body.contains("A. Patient"),
        "the accepted values echo back: {}",
        reply.body
    );
    assert!(
        !reply.body.contains("is required"),
        "no field errored: {}",
        reply.body
    );
}

// ------------------------------------------------------ the route edge's head

#[tokio::test]
async fn the_route_edge_authors_a_per_page_head() {
    let fx = fixture();
    let router = router(&fx);

    // The route declared `pages.book-appointment`; the head derives from
    // that declaration, so the edge can never ship another page's title.
    let reply = get(&router, "/appointments/book?date=2026-11-15", None).await;

    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert!(
        reply
            .body
            .contains("<title>Book Appointment · Kelson</title>"),
        "the declared view titles the page: {}",
        reply.body
    );
}
