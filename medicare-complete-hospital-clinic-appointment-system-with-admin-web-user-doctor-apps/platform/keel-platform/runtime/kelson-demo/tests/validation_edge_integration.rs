//! `kelson` validation edge integration.
//! Wave 4 Phase 4 validation edge (19-PARITY database rules + per-field
//! messages + custom rules): the manifest's `unique`/`exists` grammar runs
//! against the `model.probes` registry (default column = final field segment,
//! explicit `col` token redirects the value, unregistered model → 503
//! `DB_ERROR`), wildcards expand into array elements / object keys,
//! `messages`/`attribute` override the built-in text, and `custom = "<name>"`
//! evaluates through the process registry. In-memory backends; see
//! lib.rs::build_app_state.
//!
//! Fixture shape: the REAL appointments manifest plus appended `[[validation]]`
//! entries on the real `book` route (TOML arrays append). Every appended field
//! is `_`-prefixed — the GRAPH-VALIDATION-FIELD shape gate skips that
//! namespace (it belongs to edge-injected names like `_bindings`), so the
//! declared `inputs` shape of `book` stays untouched. Validation runs BEFORE
//! dispatch, so a passing rule set is observed as the real 200 booking while
//! any failure is the 422 envelope (503 on the fatal probe channel).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

/// Extra `[[validation]]` entries appended to the demo manifest (see the
/// module doc for why the fields are `_`-prefixed). The custom rule is
/// registered by `app()` BEFORE graph compile so GRAPH-RULE-UNKNOWN sees it.
const EXTRA_VALIDATION: &str = r#"
# Default column = final field segment (slot), from the nested path.
[[validation]]
operation = "book"
field = "_x.slot"
rules = ["exists:Appointment"]

# Explicit column token: the VALUE of `_t` is matched against slot.
[[validation]]
operation = "book"
field = "_t"
rules = ["exists:Appointment,slot"]

# Unregistered model: the probe lookup has nowhere to answer -> 503.
[[validation]]
operation = "book"
field = "_ghost"
rules = ["exists:GhostModel"]

# Wildcard: expands per array index / object key of the merged input.
[[validation]]
operation = "book"
field = "_items.*.name"
rules = ["required"]

# Per-field message override: the required failure renders the template
# with {attribute} substituted.
[[validation]]
operation = "book"
field = "_code"
rules = ["required"]
attribute = "Access code"
messages = { required = "the {attribute} is mandatory" }

# Module custom rule - registered by app() before graph compile.
[[validation]]
operation = "book"
field = "_target"
rules = []
custom = "wave4_phase4_edge_rule"
"#;

struct App {
    router: axum::Router,
    state: kelson_demo::routes::AppState,

    store: Arc<kelson_appointments_contract::AppointmentStore>,
}

fn app() -> App {
    // The custom rule under test: anything but "ALLOW" fails the field.
    // (Registration replaces by name, so repeated app() calls are safe.)
    struct EdgeRule;
    impl kelson_validation::CustomRule for EdgeRule {
        fn check(&self, _field: &str, value: &Value, _input: &Value) -> Option<String> {
            (value.as_str() != Some("ALLOW")).then(|| "must be ALLOW".to_string())
        }
    }
    kelson_validation::register_custom_rule("wave4_phase4_edge_rule", Arc::new(EdgeRule));

    let dir = std::env::temp_dir().join(format!("kelson-p4-edge-{}", uuid::Uuid::now_v7()));
    let (state, _, store) = kelson_demo::build_app_state(&dir, true, true);
    let raw = format!(
        "{}{EXTRA_VALIDATION}",
        include_str!("../../../applications/appointments/module.toml")
    );
    let manifest =
        kelson_registry::ModuleManifest::from_toml(&raw).expect("fixture manifest parses");
    let graph =
        kelson_registry::PlatformGraph::compile(vec![manifest]).expect("fixture graph compiles");
    let router = kelson_demo::routes::router(state.clone())
        .merge(kelson_demo::graph_routes::graph_router(
            state.clone(),
            &graph,
        ))
        .merge(kelson_demo::auth_routes::router(state.clone()));
    let router = state
        .sessions
        .as_ref()
        .expect("sessions enabled")
        .as_ref()
        .clone()
        .apply(router);
    App {
        router,
        state,
        store,
    }
}

struct Reply {
    status: StatusCode,
    body: Value,
    set_cookies: Vec<String>,
}

impl Reply {
    fn data(&self) -> Value {
        self.body["data"].clone()
    }
    fn session_cookie(&self) -> Option<String> {
        self.set_cookies
            .iter()
            .find(|c| c.starts_with("kelson_session="))
            .map(|c| c.split(';').next().unwrap().to_string())
    }
    /// The 422 envelope's `error.meta.fields` map (field path -> messages).
    fn fields(&self) -> Value {
        self.body["error"]["meta"]["fields"].clone()
    }
}

async fn send(app: &App, req: Request<Body>) -> Reply {
    let res = app
        .router
        .clone()
        .oneshot(req)
        .await
        .expect("request serves");
    let status = res.status();
    let set_cookies: Vec<String> = res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .map(str::to_string)
        .collect();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Reply {
        status,
        body,
        set_cookies,
    }
}

fn json_req(method: Method, uri: &str, body: Value, cookie: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    if body.is_null() {
        builder.body(Body::empty()).unwrap()
    } else {
        builder
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }
}

async fn register(app: &App, email: &str) -> String {
    let res = send(
        app,
        json_req(
            Method::POST,
            "/auth/register",
            json!({ "email": email, "password": "Correct-horse-99", "tenant": "dev" }),
            None,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    res.session_cookie().expect("session cookie")
}

/// Seed a live appointment row the probe rules can find (same tenant the
/// authenticated caller books into).
fn seed_slot(app: &App, slot: &str) {
    app.store
        .insert(kelson_appointments_contract::Appointment {
            id: uuid::Uuid::now_v7(),
            tenant: app.state.default_tenant_id.expect("default tenant"),
            patient: "Probe Patient".into(),
            slot: slot.into(),
            status: kelson_appointments_contract::AppointmentStatus::Booked,
            version: 1,
        })
        .expect("seeded row inserts");
}

fn assert_field_message(reply: &Reply, field: &str, message: &str) {
    let fields = reply.fields();
    let violations = fields[field]
        .as_array()
        .unwrap_or_else(|| panic!("{field} violations reported: {}", reply.body));
    assert!(
        violations.iter().any(|m| m.as_str() == Some(message)),
        "{field}: expected {message:?} in {violations:?} ({})",
        reply.body
    );
}

// ------------------------------------------------- exists: default column

/// `exists:Appointment` with no column token matches the final segment of
/// the validated field path (`_x.slot` → `slot`): a live row answers the
/// probe (the booking passes), an absent row is a per-field 422.
#[tokio::test]
async fn exists_rule_matches_the_default_column_of_the_field_path() {
    let app = app();
    let cookie = register(&app, &format!("p4col-{}@clinic.test", uuid::Uuid::now_v7())).await;
    seed_slot(&app, "2026-09-06T11:00Z");

    // Row exists under the default column (slot == nested value).
    let hit = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            json!({
                "patient": "A. Patient",
                "slot": "2026-09-06T12:00Z",
                "_code": "ok",
                "_x": { "slot": "2026-09-06T11:00Z" },
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(hit.status, StatusCode::OK, "{}", hit.body);
    assert_eq!(hit.data()["status"], "booked");

    // No row carries that slot: the probe answers "not found" per-field.
    let miss = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            json!({
                "patient": "B. Patient",
                "slot": "2026-09-06T13:00Z",
                "_code": "ok",
                "_x": { "slot": "2026-09-06T14:00Z" },
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(
        miss.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        miss.body
    );
    assert_field_message(&miss, "_x.slot", "is invalid");
}

// ------------------------------------------------- exists: column token

/// `exists:Appointment,slot` redirects the VALUE of the validated field
/// (`_t`, which no model column matches) into the slot column: without the
/// token the probe would fail closed on the nonexistent `_t` column (503),
/// with it the same value flips between pass and per-field 422.
#[tokio::test]
async fn exists_rule_honors_the_explicit_column_token() {
    let app = app();
    let cookie = register(&app, &format!("p4tok-{}@clinic.test", uuid::Uuid::now_v7())).await;
    seed_slot(&app, "2026-09-07T11:00Z");

    let hit = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            json!({
                "patient": "A. Patient",
                "slot": "2026-09-07T12:00Z",
                "_code": "ok",
                "_t": "2026-09-07T11:00Z",
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(hit.status, StatusCode::OK, "{}", hit.body);

    let miss = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            json!({
                "patient": "B. Patient",
                "slot": "2026-09-07T13:00Z",
                "_code": "ok",
                "_t": "nope",
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(
        miss.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        miss.body
    );
    assert_field_message(&miss, "_t", "is invalid");
}

// ------------------------------------------------- unknown model: 503

/// A probe rule whose model has no registered row probe fails closed: the
/// fatal channel aborts validation into a 503 DB_ERROR envelope — never a
/// field-level pass (docs/19: DB errors never pass validation).
#[tokio::test]
async fn exists_rule_against_an_unregistered_model_fails_closed_503() {
    let app = app();
    let cookie = register(
        &app,
        &format!("p4fatal-{}@clinic.test", uuid::Uuid::now_v7()),
    )
    .await;

    let res = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            json!({
                "patient": "A. Patient",
                "slot": "2026-09-08T10:00Z",
                "_ghost": "x",
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::SERVICE_UNAVAILABLE, "{}", res.body);
    assert_eq!(res.body["error"]["code"], "DB_ERROR");
}

// ------------------------------------------------- wildcards

/// `_items.*.name` expands per element of the merged input: only the failing
/// concrete path (`_items.0.name`) appears in the 422 fields meta, healthy
/// elements never surface, object keys expand the same way, and a fully
/// healthy array passes into the booking.
#[tokio::test]
async fn wildcard_fields_expand_per_element() {
    let app = app();
    let cookie = register(
        &app,
        &format!("p4wild-{}@clinic.test", uuid::Uuid::now_v7()),
    )
    .await;

    let mixed = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            json!({
                "patient": "A. Patient",
                "slot": "2026-09-09T10:00Z",
                "_code": "ok",
                "_items": [{ "name": "" }, { "name": "ok" }],
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(
        mixed.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        mixed.body
    );
    let fields = mixed.fields();
    assert!(
        fields.get("_items.0.name").is_some(),
        "failing element reported: {}",
        mixed.body
    );
    assert!(
        fields.get("_items.1.name").is_none(),
        "healthy element never surfaces: {}",
        mixed.body
    );

    // Object keys expand the same way (concrete paths, not the pattern).
    let keyed = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            json!({
                "patient": "B. Patient",
                "slot": "2026-09-09T11:00Z",
                "_code": "ok",
                "_items": { "first": { "name": "" } },
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(
        keyed.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        keyed.body
    );
    assert!(keyed.fields().get("_items.first.name").is_some());

    // Every element healthy -> validation passes (the booking lands).
    let pass = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            json!({
                "patient": "C. Patient",
                "slot": "2026-09-09T12:00Z",
                "_code": "ok",
                "_items": [{ "name": "ok" }],
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(pass.status, StatusCode::OK, "{}", pass.body);
}

// ------------------------------------------------- messages + attribute

/// The `messages.required` override replaces the built-in text and
/// substitutes `{attribute}` with the declared display name; collect-all
/// reports only the failing field.
#[tokio::test]
async fn message_overrides_render_the_attribute_placeholder() {
    let app = app();
    let cookie = register(&app, &format!("p4msg-{}@clinic.test", uuid::Uuid::now_v7())).await;

    let res = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            json!({ "patient": "A. Patient", "slot": "2026-09-10T10:00Z" }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", res.body);
    assert_field_message(&res, "_code", "the Access code is mandatory");
    let fields = res.fields();
    assert_eq!(
        fields.as_object().map(|o| o.len()),
        Some(1),
        "collect-all reports only the failing field: {fields}"
    );
}

// ------------------------------------------------- custom rules

/// `custom = "<name>"` evaluates the registered rule: a rejection is a
/// per-field 422 with the rule's message, an accepted value passes through
/// into the booking.
#[tokio::test]
async fn registered_custom_rule_rejects_and_accepts_values() {
    let app = app();
    let cookie = register(
        &app,
        &format!("p4custom-{}@clinic.test", uuid::Uuid::now_v7()),
    )
    .await;

    let rejected = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            json!({
                "patient": "A. Patient",
                "slot": "2026-09-11T10:00Z",
                "_code": "ok",
                "_target": "DENY",
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(
        rejected.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        rejected.body
    );
    assert_field_message(&rejected, "_target", "must be ALLOW");

    let accepted = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            json!({
                "patient": "B. Patient",
                "slot": "2026-09-11T11:00Z",
                "_code": "ok",
                "_target": "ALLOW",
            }),
            Some(&cookie),
        ),
    )
    .await;
    assert_eq!(accepted.status, StatusCode::OK, "{}", accepted.body);
}
