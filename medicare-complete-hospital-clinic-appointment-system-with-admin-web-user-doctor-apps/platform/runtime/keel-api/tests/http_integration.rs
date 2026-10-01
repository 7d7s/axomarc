//! S7 HTTP integration suite: the full application edge exercised over a
//! real axum router with the production wiring (graph routes, auth, session
//! middleware, uploads). In-memory backends; see lib.rs::build_app_state.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

struct App {
    router: axum::Router,
    state: keel_api::routes::AppState,
    store: Arc<keel_mod_appointments::AppointmentStore>,
}

fn app() -> App {
    let dir = std::env::temp_dir().join(format!("keel-api-it-{}", uuid::Uuid::now_v7()));
    let (state, graph, store) = keel_api::build_app_state(&dir, true, true);
    let router = keel_api::routes::router(state.clone())
        .merge(keel_api::graph_routes::graph_router(state.clone(), &graph))
        .merge(keel_api::auth_routes::router(state.clone()))
        .merge(keel_api::upload_routes::router(state.clone()))
        .merge(keel_api::notification_routes::router(state.clone()))
        .merge(keel_api::batch_routes::router(state.clone()));
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
    headers: axum::http::HeaderMap,
}

impl Reply {
    fn data(&self) -> Value {
        self.body["data"].clone()
    }
    fn error_code(&self) -> String {
        self.body["error"]["code"]
            .as_str()
            .unwrap_or("")
            .to_string()
    }
    fn session_cookie(&self) -> Option<String> {
        self.set_cookies
            .iter()
            .find(|c| c.starts_with("keel_session="))
            .map(|c| c.split(';').next().unwrap().to_string())
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
    let headers = res.headers().clone();
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
        headers,
    }
}

fn json_req(
    method: Method,
    uri: &str,
    body: Value,
    cookie: Option<&str>,
    bearer: Option<&str>,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(c) = cookie {
        builder = builder.header(header::COOKIE, c);
    }
    if let Some(b) = bearer {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {b}"));
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

async fn register(app: &App, email: &str) -> Reply {
    let body =
        serde_json::json!({ "email": email, "password": "correct-horse-99", "tenant": "dev" });
    send(
        app,
        json_req(Method::POST, "/auth/register", body, None, None),
    )
    .await
}

async fn login(app: &App, email: &str, password: &str, tenant: Option<&str>) -> Reply {
    let mut body = serde_json::json!({ "identifier": email, "password": password });
    if let Some(t) = tenant {
        body["tenant"] = Value::String(t.to_string());
    }
    send(app, json_req(Method::POST, "/auth/login", body, None, None)).await
}

// ------------------------------------------------------------- golden path

#[tokio::test]
async fn full_cycle_register_login_book_list_cancel() {
    let app = app();
    let email = format!("cycle-{}@clinic.test", uuid::Uuid::now_v7());

    let reg = register(&app, &email).await;
    assert_eq!(reg.status, StatusCode::OK, "register: {}", reg.body);
    assert_eq!(reg.body["ok"], true);
    let cookie = reg
        .session_cookie()
        .expect("session cookie issued on register");

    // Book through the GRAPH route (the audit's C1 blocker, now wired).
    let book = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            serde_json::json!({ "patient": "A. Patient", "slot": "2026-09-01T10:00Z" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(book.status, StatusCode::OK, "book: {}", book.body);
    let id = book.data()["id"]
        .as_str()
        .expect("appointment id")
        .to_string();
    assert_eq!(book.data()["status"], "booked");
    assert_eq!(
        app.store
            .count(app.state.default_tenant_id.unwrap())
            .unwrap(),
        1
    );

    // List (READ profile, tenant-scoped).
    let list = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(list.status, StatusCode::OK, "list: {}", list.body);
    let data = list.data();
    let rows = data["appointments"].as_array().expect("rows");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["patient"], "A. Patient");

    // Cancel via path parameter {id}.
    let cancel = send(
        &app,
        json_req(
            Method::DELETE,
            &format!("/api/v1/appointments/{id}"),
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(cancel.status, StatusCode::OK, "cancel: {}", cancel.body);
    assert_eq!(cancel.data()["status"], "cancelled");

    // Double cancel is a conflict envelope.
    let again = send(
        &app,
        json_req(
            Method::DELETE,
            &format!("/api/v1/appointments/{id}"),
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(
        again.status,
        StatusCode::CONFLICT,
        "double cancel: {}",
        again.body
    );
    assert_eq!(again.error_code(), "KER_CONFLICT");
}

#[tokio::test]
async fn sessions_persist_across_requests_and_logout_destroys() {
    let app = app();
    let email = format!("persist-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&app, &email).await.session_cookie().unwrap();

    let first = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(first.status, StatusCode::OK);
    let second = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(second.status, StatusCode::OK, "same session still valid");

    let me = send(
        &app,
        json_req(Method::GET, "/auth/me", Value::Null, Some(&cookie), None),
    )
    .await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.data()["principal"]["email"], email);

    let out = send(
        &app,
        json_req(
            Method::POST,
            "/auth/logout",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(out.status, StatusCode::OK);
    assert!(out.session_cookie().is_some(), "clear cookie issued");

    let after = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(
        after.status,
        StatusCode::UNAUTHORIZED,
        "destroyed session is anonymous"
    );
    assert_eq!(after.error_code(), "AUTH_UNAUTHENTICATED");
}

#[tokio::test]
async fn login_bad_password_is_uniform_401() {
    let app = app();
    let email = format!("badpw-{}@clinic.test", uuid::Uuid::now_v7());
    register(&app, &email).await;
    let res = login(&app, &email, "wrong-password-1", Some("dev")).await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    assert_eq!(res.body["ok"], false);
    let nobody = login(
        &app,
        &format!("ghost-{}@clinic.test", uuid::Uuid::now_v7()),
        "x-pass-12345",
        Some("dev"),
    )
    .await;
    assert_eq!(
        nobody.status,
        StatusCode::UNAUTHORIZED,
        "unknown principal same shape"
    );
}
// ---------------------------------------------------------------- throttling

#[tokio::test]
async fn login_throttling_returns_429_after_burst() {
    let app = app();
    let email = format!("throttle-{}@clinic.test", uuid::Uuid::now_v7());
    register(&app, &email).await;
    let mut last;
    for _ in 0..5 {
        last = login(&app, &email, "bad-password-1", Some("dev"))
            .await
            .status;
        assert_eq!(last, StatusCode::UNAUTHORIZED);
    }
    let sixth = login(&app, &email, "bad-password-1", Some("dev")).await;
    assert_eq!(
        sixth.status,
        StatusCode::TOO_MANY_REQUESTS,
        "burst exhausted: {}",
        sixth.body
    );
    assert_eq!(sixth.error_code(), "CAP_LIMIT_EXCEEDED");
    // Correct password is also throttled (bucket is identity+IP keyed).
    let locked = login(&app, &email, "correct-horse-99", Some("dev")).await;
    assert_eq!(locked.status, StatusCode::TOO_MANY_REQUESTS);
}

// ------------------------------------------------------------ bearer tokens

#[tokio::test]
async fn bearer_pat_flow_works_without_cookies() {
    let app = app();
    let email = format!("pat-{}@clinic.test", uuid::Uuid::now_v7());
    let mut body =
        serde_json::json!({ "identifier": email, "password": "correct-horse-99", "tenant": "dev" });
    register(&app, &email).await;
    body["mode"] = Value::String("token".into());
    let login_res = send(
        &app,
        json_req(Method::POST, "/auth/login", body, None, None),
    )
    .await;
    assert_eq!(login_res.status, StatusCode::OK, "{}", login_res.body);
    let token = login_res.data()["token"]
        .as_str()
        .expect("PAT issued once")
        .to_string();
    assert_eq!(login_res.data()["mode"], "session+token");

    // Authenticated call with the PAT, no cookie.
    let me = send(
        &app,
        json_req(Method::GET, "/auth/me", Value::Null, None, Some(&token)),
    )
    .await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.data()["principal"]["email"], email);

    // Graph route over bearer: tenant comes from the PAT label.
    let book = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            serde_json::json!({ "patient": "P. Atient", "slot": "2026-09-02T09:00Z" }),
            None,
            Some(&token),
        ),
    )
    .await;
    assert_eq!(book.status, StatusCode::OK, "bearer book: {}", book.body);

    // Logout revokes the PAT.
    let out = send(
        &app,
        json_req(
            Method::POST,
            "/auth/logout",
            Value::Null,
            None,
            Some(&token),
        ),
    )
    .await;
    assert_eq!(out.status, StatusCode::OK);
    let after = send(
        &app,
        json_req(Method::GET, "/auth/me", Value::Null, None, Some(&token)),
    )
    .await;
    assert_eq!(
        after.status,
        StatusCode::UNAUTHORIZED,
        "revoked PAT rejected"
    );
}

#[tokio::test]
async fn pat_bypasses_snapshot_is_403_denied() {
    let app = app();
    // A PAT issued outside the login flow has no access snapshot installed:
    // the engine denies (403), proving authn and authz stay separate.
    let principal = keel_foundation::PrincipalId::new();
    let issued = app
        .state
        .tokens
        .issue(
            principal,
            keel_tokens::TokenPurpose::Api,
            time::Duration::hours(1),
            None,
        )
        .unwrap();
    let res = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments",
            Value::Null,
            None,
            Some(&issued.plaintext),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN, "{}", res.body);
    assert_eq!(res.error_code(), "AUTH_DENIED");
}

// ------------------------------------------------------------- cookie shape

#[tokio::test]
async fn session_cookie_carries_secure_defaults_not_secure_in_test() {
    let app = app();
    let email = format!("cookie-{}@clinic.test", uuid::Uuid::now_v7());
    let reg = register(&app, &email).await;
    let raw = reg
        .set_cookies
        .iter()
        .find(|c| c.starts_with("keel_session="))
        .expect("session cookie")
        .clone();
    assert!(raw.contains("Path=/"), "{raw}");
    assert!(raw.contains("HttpOnly"), "{raw}");
    assert!(raw.contains("SameSite=Lax"), "{raw}");
    assert!(!raw.contains("Secure"), "test env is not TLS: {raw}");
    assert!(raw.contains("Max-Age=28800"), "{raw}");
    // Forged tag is treated as anonymous.
    let forged = format!("keel_session={}.deadbeef", "a".repeat(64));
    let res = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments",
            Value::Null,
            Some(&forged),
            None,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
}

// ------------------------------------------------------------ edge envelopes

#[tokio::test]
async fn unauthenticated_graph_route_is_401_envelope() {
    let app = app();
    let res = send(
        &app,
        json_req(Method::GET, "/api/v1/appointments", Value::Null, None, None),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED);
    assert_eq!(res.error_code(), "AUTH_UNAUTHENTICATED");
}

#[tokio::test]
async fn unknown_route_is_404_and_demo_echo_still_golden() {
    let app = app();
    let miss = send(
        &app,
        json_req(Method::GET, "/api/v1/nope", Value::Null, None, None),
    )
    .await;
    assert_eq!(miss.status, StatusCode::NOT_FOUND);

    let echo = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/demo/echo",
            serde_json::json!({ "hello": "keel" }),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(echo.status, StatusCode::OK, "{}", echo.body);
    assert_eq!(echo.data()["echo"]["hello"], "keel");
}

#[tokio::test]
async fn malformed_json_body_is_422_envelope() {
    let app = app();
    let email = format!("json-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&app, &email).await.session_cookie().unwrap();
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/appointments")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, &cookie)
        .body(Body::from("{not json"))
        .unwrap();
    let res = send(&app, req).await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", res.body);
    assert_eq!(res.error_code(), "KER_VALIDATION");
}

// ------------------------------------------------------------------ uploads

fn multipart_request(
    uri: &str,
    cookie: &str,
    filename: &str,
    content_type: &str,
    data: &[u8],
) -> Request<Body> {
    let boundary = "keel-it-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
    body.extend_from_slice(data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(header::COOKIE, cookie)
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn upload_happy_path_roundtrips_through_storage() {
    let app = app();
    let email = format!("upload-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&app, &email).await.session_cookie().unwrap();

    let payload = b"patient,slot\nA,2026-09-01".to_vec();
    let res = send(
        &app,
        multipart_request(
            "/api/v1/uploads",
            &cookie,
            "report.csv",
            "text/csv",
            &payload,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "upload: {}", res.body);
    let upload_data = res.data();
    let files = upload_data["files"].as_array().expect("files");
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["filename"], "report.csv");
    assert_eq!(files[0]["size"], payload.len() as i64);
    // Wave 6 P2: the stored record names the disk it landed on (the
    // harness registry's synthetic default).
    assert_eq!(files[0]["disk"], "local");

    // Storage round-trip through the same port the edge wrote to.
    let object = keel_storage::ObjectRef {
        bucket: files[0]["object"]["bucket"].as_str().unwrap().into(),
        key: files[0]["object"]["key"].as_str().unwrap().into(),
    };
    let bytes = app.state.storage.get(&object).expect("object stored");
    assert_eq!(bytes, payload);
}

/// Wave 6 P2: `?disk=` resolves through the registry — an explicit
/// `local` records itself, and an unknown disk is a 422 that never
/// silently falls back to the default disk.
#[tokio::test]
async fn upload_disk_query_resolves_and_unknown_disk_is_422() {
    let app = app();
    let email = format!("disk-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&app, &email).await.session_cookie().unwrap();

    let res = send(
        &app,
        multipart_request(
            "/api/v1/uploads?disk=local",
            &cookie,
            "note.txt",
            "text/plain",
            b"on local",
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "upload: {}", res.body);
    let upload_data = res.data();
    let files = upload_data["files"].as_array().expect("files");
    assert_eq!(files[0]["disk"], "local");

    let res = send(
        &app,
        multipart_request(
            "/api/v1/uploads?disk=nope",
            &cookie,
            "note.txt",
            "text/plain",
            b"never stored",
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", res.body);
    let body = res.body.to_string();
    assert!(body.contains("nope"), "{body}");
}

#[tokio::test]
async fn upload_traversal_filename_is_sanitized_and_unauthenticated_rejected() {
    let app = app();
    let email = format!("trav-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&app, &email).await.session_cookie().unwrap();
    let res = send(
        &app,
        multipart_request(
            "/api/v1/uploads",
            &cookie,
            "../../evil.sh",
            "application/octet-stream",
            b"x",
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let filename = res.data()["files"][0]["filename"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(filename, "evil.sh", "traversal stripped");

    let anon = send(
        &app,
        multipart_request(
            "/api/v1/uploads",
            "keel_session=none.deadbeef",
            "a.txt",
            "text/plain",
            b"x",
        ),
    )
    .await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn upload_without_multipart_is_422_envelope() {
    let app = app();
    let email = format!("nomp-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&app, &email).await.session_cookie().unwrap();
    let res = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/uploads",
            serde_json::json!({}),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", res.body);
    assert_eq!(res.error_code(), "KER_VALIDATION");
}

// -------------------------------------------- S10: metrics + dashboard edge

/// Raw (non-JSON) response capture for text endpoints.
async fn raw_text(router: axum::Router, req: Request<Body>) -> (StatusCode, String, String) {
    let res = router.oneshot(req).await.expect("request serves");
    let status = res.status();
    let ctype = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, ctype, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn metrics_endpoint_is_flag_gated() {
    let app = app();
    // Flag off: 404 envelope that explains the gate.
    let (status, _, body) = raw_text(
        app.router.clone(),
        Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body.contains("app.metrics"),
        "envelope explains gate: {body}"
    );

    // Flag on: Prometheus text with fresh gauges.
    let mut state = app.state.clone();
    state.flags.push("app.metrics".into());
    let router = keel_api::routes::router(state);
    let (status, ctype, body) = raw_text(
        router,
        Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        ctype.starts_with("text/plain"),
        "prometheus content type: {ctype}"
    );
    assert!(body.contains("keel_uptime_seconds"));
    // Renderer prints 0.0 as `0` (std float Display) — accept either.
    assert!(
        body.contains("keel_degraded 0.0") || body.contains("keel_degraded 0"),
        "degraded gauge present:\n{body}"
    );
    assert!(body.contains("keel_routes_declared"));
}

/// Wave 6 P8: /metrics carries the queue census — depth per status, the
/// oldest pending age and the finished throughput census — snapshotted
/// from the store at scrape time; without a durable store the series
/// stays absent (never a lying zero).
#[tokio::test]
async fn metrics_expose_queue_depth_age_and_throughput() {
    use keel_jobs::{JobStore, JobStoreAdmin};
    let mut state = app().state.clone();
    state.flags.push("app.metrics".into());

    // No durable store: no queue series at all.
    let (status, _, body) = raw_text(
        keel_api::routes::router(state.clone()),
        Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("keel_queue_"),
        "queue series absent without a store: {body}"
    );

    // Seed one row per census dimension.
    let store = Arc::new(keel_jobs::InMemoryJobStore::new());
    let def = keel_jobs::JobDef::new("p8.metrics_probe");
    let now = time::OffsetDateTime::now_utc();

    // One job runs to success (succeeded census).
    store
        .enqueue(keel_jobs::build_from_def(
            &def,
            serde_json::Value::Null,
            now,
        ))
        .unwrap();
    let claimed = store.claim("metrics").expect("claims").expect("one job");
    store.complete(claimed.id).unwrap();

    // One dies straight from pending (dead census; no claim needed).
    let doomed = keel_jobs::build_from_def(&def, serde_json::Value::Null, now);
    let doomed_id = doomed.id;
    store.enqueue(doomed).unwrap();
    store.kill(doomed_id, "probe").unwrap();

    // One sits pending, enqueued 90s ago — the oldest-age gauge source.
    store
        .enqueue(keel_jobs::build_from_def(
            &def,
            serde_json::Value::Null,
            now - time::Duration::seconds(90),
        ))
        .unwrap();

    state.queue = Some(store as Arc<dyn keel_jobs::JobStoreAdmin>);
    let (status, _, body) = raw_text(
        keel_api::routes::router(state),
        Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    for line in [
        "keel_queue_depth{status=\"pending\"} 1",
        "keel_queue_depth{status=\"running\"} 0",
        "keel_queue_depth{status=\"dead\"} 1",
        "keel_queue_depth{status=\"succeeded\"} 1",
        "keel_queue_finished{outcome=\"succeeded\"} 1",
        "keel_queue_finished{outcome=\"dead\"} 1",
    ] {
        assert!(
            body.contains(&format!("{line}\n")),
            "missing `{line}`:\n{body}"
        );
    }
    let age_line = body
        .lines()
        .find(|l| l.starts_with("keel_queue_oldest_age_seconds "))
        .expect("age gauge present");
    let age: f64 = age_line
        .split_whitespace()
        .nth(1)
        .expect("age value")
        .parse()
        .expect("numeric age");
    assert!(age >= 85.0, "aged pending row surfaces: {age_line}");
}

#[tokio::test]
async fn metrics_middleware_counts_requests_by_method_and_class() {
    let mut state = app().state.clone();
    state.flags.push("app.metrics".into());
    let router = keel_api::routes::router(state.clone()).layer(
        axum::middleware::from_fn_with_state(state.clone(), keel_api::routes::record_metrics),
    );
    // Two happy GETs + one unmatched route (404 class).
    for _ in 0..2 {
        let _ = raw_text(
            router.clone(),
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    }
    let _ = raw_text(
        router.clone(),
        Request::builder()
            .uri("/no/such/route")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    // The scrape itself is recorded AFTER its body renders, so it sees
    // exactly the three requests above.
    let (_, _, body) = raw_text(
        router,
        Request::builder()
            .uri("/metrics")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert!(
        body.contains(r#"keel_http_requests_total{method="GET",status_class="2xx"} 2"#),
        "healthz counted twice:\n{body}"
    );
    assert!(
        body.contains(r#"keel_http_requests_total{method="GET",status_class="4xx"} 1"#),
        "unmatched route counted:\n{body}"
    );
    // Sentinel registry histograms are name-keyed (ms buckets, no labels);
    // the method/status breakdown lives on the labelled request counter.
    assert!(
        body.contains("keel_http_request_duration_ms_count 3"),
        "latency observes every request:\n{body}"
    );
}

#[tokio::test]
async fn dashboard_served_in_test_env_and_flag_gated_when_off() {
    let app = app(); // build_app_state enables the dashboard (dev/test default)
    let (status, ctype, body) = raw_text(
        app.router.clone(),
        Request::builder()
            .uri("/_dashboard")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(ctype.starts_with("text/html"));
    assert!(body.contains("<!doctype html>"));
    assert!(body.contains("/meta/routes"));

    let mut state = app.state.clone();
    state.dashboard_enabled = false;
    let router = keel_api::routes::router(state);
    let (status, _, body) = raw_text(
        router,
        Request::builder()
            .uri("/_dashboard")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        body.contains("app.dashboard"),
        "envelope explains gate: {body}"
    );
}

// ------------------------------------------- attack-oriented: session fixation

#[tokio::test]
async fn login_rotates_session_and_invalidates_prelogin_cookie() {
    let app = app();
    let email = format!("fixation-{}@clinic.test", uuid::Uuid::now_v7());

    // Attacker-planting analogue: the client already holds a session cookie
    // (K1) from before authenticating.
    let reg = register(&app, &email).await;
    assert_eq!(reg.status, StatusCode::OK, "register: {}", reg.body);
    let k1 = reg.session_cookie().expect("pre-login session cookie K1");

    let me_k1 = send(
        &app,
        json_req(Method::GET, "/auth/me", Value::Null, Some(&k1), None),
    )
    .await;
    assert_eq!(
        me_k1.status,
        StatusCode::OK,
        "K1 live before login: {}",
        me_k1.body
    );

    // Login while PRESENTING K1: the authenticated session must be a fresh
    // id (K2 != K1) and the pre-login session must be destroyed.
    let body = serde_json::json!({
        "identifier": email,
        "password": "correct-horse-99",
        "tenant": "dev"
    });
    let login_res = send(
        &app,
        json_req(Method::POST, "/auth/login", body, Some(&k1), None),
    )
    .await;
    assert_eq!(
        login_res.status,
        StatusCode::OK,
        "login: {}",
        login_res.body
    );
    let k2 = login_res
        .session_cookie()
        .expect("fresh session cookie K2 on login");
    assert_ne!(k1, k2, "session id must rotate at login");

    // K1 is dead: the pre-login session was destroyed server-side.
    let stale = send(
        &app,
        json_req(Method::GET, "/auth/me", Value::Null, Some(&k1), None),
    )
    .await;
    assert_eq!(
        stale.status,
        StatusCode::UNAUTHORIZED,
        "K1 must be invalid after rotation: {}",
        stale.body
    );

    // K2 is the only live session.
    let fresh = send(
        &app,
        json_req(Method::GET, "/auth/me", Value::Null, Some(&k2), None),
    )
    .await;
    assert_eq!(fresh.status, StatusCode::OK, "K2 live: {}", fresh.body);
}

// ------------------------------------------- attack-oriented: multipart part bomb

fn multipart_part_bomb(uri: &str, cookie: &str, parts: usize) -> Request<Body> {
    let boundary = "keel-it-boundary";
    let mut body = Vec::new();
    for i in 0..parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"file{}\"; filename=\"f{i}.txt\"\r\n",
                i
            )
            .as_bytes(),
        );
        body.extend_from_slice(b"Content-Type: text/plain\r\n\r\nx");
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(header::COOKIE, cookie)
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn upload_part_bomb_beyond_max_parts_is_rejected() {
    let app = app();
    let email = format!("bomb-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&app, &email).await.session_cookie().unwrap();

    // Default limits allow 8 parts; 9 must trip the cap before buffering.
    let res = send(&app, multipart_part_bomb("/api/v1/uploads", &cookie, 9)).await;
    assert_eq!(res.status, StatusCode::TOO_MANY_REQUESTS, "{}", res.body);

    // At the cap boundary the request still succeeds.
    let ok = send(&app, multipart_part_bomb("/api/v1/uploads", &cookie, 8)).await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
    assert_eq!(ok.data()["files"].as_array().unwrap().len(), 8);
}

// ------------------------------------------- deployment gate: health/readiness

#[tokio::test]
async fn healthz_is_liveness_and_readyz_reflects_degraded_state() {
    let app = app();

    // Healthy: both probes green; healthz reports the liveness envelope.
    let (status, _, body) = raw_text(
        app.router.clone(),
        Request::builder()
            .uri("/healthz")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"ok\":true"), "{body}");
    assert!(body.contains("\"degraded\":false"), "{body}");

    let (status, _, _) = raw_text(
        app.router.clone(),
        Request::builder()
            .uri("/readyz")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Degraded (e.g. database probe failed at boot): readiness flips to
    // 503 with the reason; liveness stays up so orchestrators do not
    // restart-loop a knowingly-degraded instance.
    app.state
        .health
        .degraded
        .store(true, std::sync::atomic::Ordering::SeqCst);
    *app.state.health.reason.lock().unwrap() = "database probe failed".to_string();

    let (status, _, body) = raw_text(
        app.router.clone(),
        Request::builder()
            .uri("/readyz")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body.contains("database probe failed"), "{body}");

    let (status, _, body) = raw_text(
        app.router.clone(),
        Request::builder()
            .uri("/healthz")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "healthz is liveness: stays 200 when degraded"
    );
    assert!(body.contains("\"degraded\":true"), "{body}");
}

// ------------------------------------------- B-23: email verification flow

#[tokio::test]
async fn email_verification_flow_marks_principal_verified() {
    let app = app();
    let email = format!("verify-{}@clinic.test", uuid::Uuid::now_v7());
    let reg = register(&app, &email).await;
    assert_eq!(reg.status, StatusCode::OK, "register: {}", reg.body);
    let cookie = reg.session_cookie().unwrap();

    // Fresh principal starts unverified.
    let me = send(
        &app,
        json_req(Method::GET, "/auth/me", Value::Null, Some(&cookie), None),
    )
    .await;
    assert_eq!(me.status, StatusCode::OK);
    assert_eq!(me.data()["principal"]["email_verified_at"], Value::Null);

    // Request: dev/test surfaces the single-use token (no mail channel yet).
    let req = send(
        &app,
        json_req(
            Method::POST,
            "/auth/email-verification/request",
            serde_json::json!({ "email": email }),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(req.status, StatusCode::OK, "{}", req.body);
    assert_eq!(req.data()["sent"], true);
    let token = req.data()["dev_token"]
        .as_str()
        .expect("dev_token surfaced in test env")
        .to_string();

    // Confirm: redeems once and records contact proof.
    let confirm = send(
        &app,
        json_req(
            Method::POST,
            "/auth/email-verification/confirm",
            serde_json::json!({ "token": token }),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(confirm.status, StatusCode::OK, "{}", confirm.body);
    assert_eq!(confirm.data()["verified"], true);

    let me = send(
        &app,
        json_req(Method::GET, "/auth/me", Value::Null, Some(&cookie), None),
    )
    .await;
    assert!(
        me.data()["principal"]["email_verified_at"].is_string(),
        "verification timestamp recorded: {}",
        me.body
    );

    // Replay of the same token is rejected (single-use).
    let replay = send(
        &app,
        json_req(
            Method::POST,
            "/auth/email-verification/confirm",
            serde_json::json!({ "token": token }),
            None,
            None,
        ),
    )
    .await;
    assert_ne!(replay.status, StatusCode::OK, "token must be single-use");
}

#[tokio::test]
async fn email_verification_request_is_enumeration_safe() {
    let app = app();
    // Unknown email: identical success shape, no token leaked.
    let req = send(
        &app,
        json_req(
            Method::POST,
            "/auth/email-verification/request",
            serde_json::json!({ "email": "nobody@clinic.test" }),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(req.status, StatusCode::OK, "{}", req.body);
    assert_eq!(req.data()["sent"], true);
    assert_eq!(
        req.data()["dev_token"],
        Value::Null,
        "no token for unknown email"
    );
}

#[tokio::test]
async fn verified_email_gate_blocks_module_operations_until_confirmed() {
    let base = app();
    let mut state = base.state.clone();
    state.require_verified_email = true;
    // Rebuild the full edge with the gated state.
    let router = keel_api::routes::router(state.clone())
        .merge(keel_api::graph_routes::graph_router(state.clone(), &{
            let (_, graph, _) = keel_api::build_app_state(
                &std::env::temp_dir().join(format!("keel-api-it-{}", uuid::Uuid::now_v7())),
                true,
                true,
            );
            graph
        }))
        .merge(keel_api::auth_routes::router(state.clone()));
    let router = state
        .sessions
        .as_ref()
        .expect("sessions enabled")
        .as_ref()
        .clone()
        .apply(router);
    let gated = App {
        router,
        state,
        store: base.store.clone(),
    };

    let email = format!("gate-{}@clinic.test", uuid::Uuid::now_v7());
    // Register still works (auth endpoints are exempt from the gate).
    let cookie = register(&gated, &email).await.session_cookie().unwrap();

    // Module operation denied while unverified.
    let book = send(
        &gated,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            serde_json::json!({ "patient": "A. Patient", "slot": "2026-09-02T10:00Z" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(book.status, StatusCode::FORBIDDEN, "{}", book.body);
    assert_eq!(book.error_code(), "AUTH_DENIED");

    // Verify the email, then the same operation succeeds.
    let token = send(
        &gated,
        json_req(
            Method::POST,
            "/auth/email-verification/request",
            serde_json::json!({ "email": email }),
            None,
            None,
        ),
    )
    .await
    .data()["dev_token"]
        .as_str()
        .unwrap()
        .to_string();
    let confirm = send(
        &gated,
        json_req(
            Method::POST,
            "/auth/email-verification/confirm",
            serde_json::json!({ "token": token }),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(confirm.status, StatusCode::OK, "{}", confirm.body);

    let book = send(
        &gated,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            serde_json::json!({ "patient": "A. Patient", "slot": "2026-09-02T10:00Z" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(
        book.status,
        StatusCode::OK,
        "gate lifts after verification: {}",
        book.body
    );
}

// ------------------------------------------- B-22: stateless JWT bearer interop

const JWT_IT_SECRET: &[u8] = b"jwt-integration-secret";

fn jwt_for(principal: &str, tenant: Option<&str>, ttl_secs: i64, secret: &[u8]) -> String {
    let now = time::OffsetDateTime::now_utc();
    let claims = keel_http::JwtClaims {
        sub: principal.to_string(),
        tenant: tenant.map(|t| t.to_string()),
        exp: (now + time::Duration::seconds(ttl_secs)).unix_timestamp(),
        iat: Some(now.unix_timestamp()),
        jti: Some(format!("it-{}", uuid::Uuid::now_v7())),
    };
    keel_http::JwtHs256::new(secret.to_vec()).sign(&claims)
}

/// Same wiring as `app()` but with the opt-in HS256 JWT verifier attached.
fn app_with_jwt(secret: &[u8]) -> App {
    let dir = std::env::temp_dir().join(format!("keel-api-it-{}", uuid::Uuid::now_v7()));
    let (mut state, graph, store) = keel_api::build_app_state(&dir, true, true);
    state.jwt = Some(std::sync::Arc::new(keel_http::JwtHs256::new(
        secret.to_vec(),
    )));
    let router = keel_api::routes::router(state.clone())
        .merge(keel_api::graph_routes::graph_router(state.clone(), &graph))
        .merge(keel_api::auth_routes::router(state.clone()))
        .merge(keel_api::upload_routes::router(state.clone()));
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

#[tokio::test]
async fn jwt_bearer_serves_graph_route_when_enabled() {
    let app = app_with_jwt(JWT_IT_SECRET);
    let email = format!("jwt-{}@clinic.test", uuid::Uuid::now_v7());
    let reg = register(&app, &email).await;
    assert_eq!(reg.status, StatusCode::OK, "{}", reg.body);
    let principal = reg.data()["principal_id"].as_str().unwrap().to_string();
    let tenant = reg.data()["tenant_id"].as_str().unwrap().to_string();

    // Stateless JWT for the same principal/tenant: no cookie, no PAT.
    let token = jwt_for(&principal, Some(&tenant), 600, JWT_IT_SECRET);
    let book = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            serde_json::json!({ "patient": "JWT Patient", "slot": "2026-09-03T10:00Z" }),
            None,
            Some(&token),
        ),
    )
    .await;
    assert_eq!(book.status, StatusCode::OK, "{}", book.body);
    assert_eq!(book.data()["status"], "booked");

    // A read through the same bearer.
    let list = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments",
            Value::Null,
            None,
            Some(&token),
        ),
    )
    .await;
    assert_eq!(list.status, StatusCode::OK, "{}", list.body);
}

#[tokio::test]
async fn jwt_expired_forged_tampered_garbage_are_uniform_401() {
    let app = app_with_jwt(JWT_IT_SECRET);
    let email = format!("jwt401-{}@clinic.test", uuid::Uuid::now_v7());
    let reg = register(&app, &email).await;
    let principal = reg.data()["principal_id"].as_str().unwrap().to_string();
    let tenant = reg.data()["tenant_id"].as_str().unwrap().to_string();

    // Expired.
    let expired = jwt_for(&principal, Some(&tenant), -60, JWT_IT_SECRET);
    let res = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments",
            Value::Null,
            None,
            Some(&expired),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED, "{}", res.body);

    // Forged with a different secret.
    let forged = jwt_for(&principal, Some(&tenant), 600, b"attacker-secret");
    let res = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments",
            Value::Null,
            None,
            Some(&forged),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED, "{}", res.body);

    // Tampered payload (signature no longer matches).
    let good = jwt_for(&principal, Some(&tenant), 600, JWT_IT_SECRET);
    let mut parts: Vec<String> = good.split('.').map(str::to_string).collect();
    let mut payload = parts[1].clone();
    let last = payload.pop().unwrap();
    payload.push(if last == 'A' { 'B' } else { 'A' });
    parts[1] = payload;
    let tampered = parts.join(".");
    let res = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments",
            Value::Null,
            None,
            Some(&tampered),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED, "{}", res.body);

    // Non-JWT garbage.
    let res = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments",
            Value::Null,
            None,
            Some("not-a-jwt"),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED, "{}", res.body);
}

#[tokio::test]
async fn jwt_bearer_inert_when_verifier_not_wired() {
    // Default app() has jwt: None, so even a well-formed JWT must not
    // authenticate (the flag-gated interop is strictly opt-in).
    let app = app();
    let email = format!("jwtoff-{}@clinic.test", uuid::Uuid::now_v7());
    let reg = register(&app, &email).await;
    let principal = reg.data()["principal_id"].as_str().unwrap().to_string();
    let tenant = reg.data()["tenant_id"].as_str().unwrap().to_string();
    let token = jwt_for(&principal, Some(&tenant), 600, JWT_IT_SECRET);
    let res = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments",
            Value::Null,
            None,
            Some(&token),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED, "{}", res.body);
}

#[tokio::test]
async fn pat_bearer_still_authenticates_with_jwt_enabled() {
    // PAT-first ordering: a real PAT authenticates even when the JWT
    // verifier is wired (default model is never displaced).
    let app = app_with_jwt(JWT_IT_SECRET);
    let email = format!("patjwt-{}@clinic.test", uuid::Uuid::now_v7());
    let body = serde_json::json!({
        "email": email,
        "password": "correct-horse-99",
        "tenant": "dev",
        "mode": "token"
    });
    let reg = send(
        &app,
        json_req(Method::POST, "/auth/register", body, None, None),
    )
    .await;
    assert_eq!(reg.status, StatusCode::OK, "{}", reg.body);
    let pat = reg.data()["token"]
        .as_str()
        .expect("PAT issued")
        .to_string();
    let res = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments",
            Value::Null,
            None,
            Some(&pat),
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
}
// ------------------------------------------- sentinel admin telemetry (G18)

/// Production-shaped wiring with the admin telemetry surface merged in.
/// The harness runtime is INERT (never started): the incident plane is
/// file-backed and the status snapshot is in-memory, so no worker threads
/// are spawned per test case.
fn app_with_telemetry() -> App {
    let dir = std::env::temp_dir().join(format!("keel-api-it-{}", uuid::Uuid::now_v7()));
    let (state, graph, store) = keel_api::build_app_state(&dir, true, true);
    let router = keel_api::routes::router(state.clone())
        .merge(keel_api::graph_routes::graph_router(state.clone(), &graph))
        .merge(keel_api::auth_routes::router(state.clone()))
        .merge(keel_api::upload_routes::router(state.clone()))
        .merge(keel_api::telemetry_routes::router(state.clone()));
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

/// Grant the incident-operator capability (registered principals only get
/// module capabilities by default).
fn grant_incident_capability(
    app: &App,
    principal: keel_foundation::PrincipalId,
    tenant: keel_foundation::TenantId,
) {
    app.state
        .policy
        .install_snapshot(keel_policy::AccessSnapshot {
            principal_id: principal,
            tenant_id: tenant,
            membership_active: true,
            capabilities: ["telemetry.incident.manage".to_string()]
                .into_iter()
                .collect(),
            scope_paths: ["/".to_string()].into_iter().collect(),
            authorization_revision: app.state.policy.revision(),
            feature_revision: keel_foundation::FeatureRevision(1),
            module_revision: keel_foundation::ModuleRevision(1),
            computed_at: time::OffsetDateTime::now_utc(),
        });
}

#[tokio::test]
async fn telemetry_incident_requires_authentication() {
    let app = app_with_telemetry();
    let res = send(
        &app,
        json_req(
            Method::POST,
            "/admin/telemetry/incident",
            serde_json::json!({}),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED, "{}", res.body);
    let res = send(
        &app,
        json_req(
            Method::GET,
            "/admin/telemetry/status",
            Value::Null,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNAUTHORIZED, "{}", res.body);
}

#[tokio::test]
async fn telemetry_incident_requires_operator_capability() {
    let app = app_with_telemetry();
    let email = format!("noincident-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&app, &email)
        .await
        .session_cookie()
        .expect("session cookie issued");
    // Registered principals carry module capabilities only — incident
    // control stays operator-gated (D-76).
    let res = send(
        &app,
        json_req(
            Method::POST,
            "/admin/telemetry/incident",
            serde_json::json!({}),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN, "{}", res.body);
    assert_eq!(res.error_code(), keel_foundation::codes::DENIED);
}

#[tokio::test]
async fn telemetry_incident_operator_roundtrip_and_status() {
    let app = app_with_telemetry();
    let email = format!("operator-{}@clinic.test", uuid::Uuid::now_v7());
    let reg = register(&app, &email).await;
    let cookie = reg.session_cookie().expect("session cookie issued");
    let principal = keel_foundation::PrincipalId::from_uuid(
        uuid::Uuid::parse_str(reg.data()["principal_id"].as_str().unwrap()).unwrap(),
    );
    let tenant = keel_foundation::TenantId::from_uuid(
        uuid::Uuid::parse_str(reg.data()["tenant_id"].as_str().unwrap()).unwrap(),
    );
    grant_incident_capability(&app, principal, tenant);

    // start
    let start = send(
        &app,
        json_req(
            Method::POST,
            "/admin/telemetry/incident",
            serde_json::json!({"action": "start"}),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(start.status, StatusCode::OK, "{}", start.body);
    assert_eq!(start.data()["outcome"], "started");
    let incident_id = start.data()["incident"]["id"].as_str().unwrap().to_string();

    // status reflects the active incident
    let status = send(
        &app,
        json_req(
            Method::GET,
            "/admin/telemetry/status",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(status.status, StatusCode::OK, "{}", status.body);
    assert_eq!(status.data()["incident"]["id"], incident_id);

    // stop
    let stop = send(
        &app,
        json_req(
            Method::POST,
            "/admin/telemetry/incident",
            serde_json::json!({"action": "stop"}),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(stop.status, StatusCode::OK, "{}", stop.body);
    assert_eq!(stop.data()["outcome"], "stopped");

    // empty body = start (CLI parity with `keel observe incident start`)
    let again = send(
        &app,
        json_req(
            Method::POST,
            "/admin/telemetry/incident",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(again.status, StatusCode::OK, "{}", again.body);
    assert_eq!(again.data()["outcome"], "started");
}


// --------------------------------------------------------------- Wave 1 battery

/// Wave 1 FormRequest wiring: manifest [[validation]] runs collect-all at the
/// edge BEFORE dispatch — 422 envelope with a `fields` meta map, no writes.
#[tokio::test]
async fn manifest_validation_422_with_fields_meta() {
    let app = app();
    let email = format!("val-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&app, &email).await.session_cookie().unwrap();

    // Malformed slot -> fields.slot reported.
    let bad_slot = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            serde_json::json!({ "patient": "A. Patient", "slot": "not-a-date" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(
        bad_slot.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        bad_slot.body
    );
    assert_eq!(bad_slot.error_code(), "KER_VALIDATION");
    let fields = &bad_slot.body["error"]["meta"]["fields"];
    assert!(
        fields["slot"].is_array(),
        "slot violations reported: {}",
        bad_slot.body
    );

    // Missing patient -> collect-all reports the field (slot stays valid).
    let missing = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            serde_json::json!({ "slot": "2026-09-01T10:00Z" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(
        missing.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        missing.body
    );
    assert!(missing.body["error"]["meta"]["fields"]["patient"].is_array());

    // Edge validation runs before dispatch: nothing was written.
    assert_eq!(
        app.store
            .count(app.state.default_tenant_id.unwrap())
            .unwrap(),
        0
    );
}

// --------------------------------------------------- Wave 4 Phase 3 (edge)

/// One part of a mixed multipart graph body: a text field (rides the JSON
/// input) or a file (stores through the StoragePort as an upload).
enum GraphPart<'a> {
    Text(&'a str),
    File(&'a str, &'a str, &'a [u8]),
}

fn graph_multipart(
    method: Method,
    uri: &str,
    cookie: &str,
    parts: &[(&str, GraphPart<'_>)],
) -> Request<Body> {
    let boundary = "keel-graph-boundary";
    let mut body = Vec::new();
    for (name, part) in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match part {
            GraphPart::Text(value) => {
                body.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                );
                body.extend_from_slice(value.as_bytes());
            }
            GraphPart::File(filename, content_type, data) => {
                body.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n"
                    )
                    .as_bytes(),
                );
                body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
                body.extend_from_slice(data);
            }
        }
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Request::builder()
        .method(method)
        .uri(uri)
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header(header::COOKIE, cookie)
        .body(Body::from(body))
        .unwrap()
}

/// Minimal PNG header (signature + IHDR) — enough for the edge's dimension
/// sniff and the manifest file rules.
fn png_bytes(w: u32, h: u32) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
    b.extend_from_slice(&13u32.to_be_bytes());
    b.extend_from_slice(b"IHDR");
    b.extend_from_slice(&w.to_be_bytes());
    b.extend_from_slice(&h.to_be_bytes());
    b.extend_from_slice(&[0u8; 13]);
    b
}

/// Wave 4 Phase 3: a multipart book carries the operation input as text
/// parts and its attachment as a stored part; the manifest file rules read
/// the part's metadata and the booking lands like any JSON book.
#[tokio::test]
async fn multipart_book_accepts_text_fields_and_an_image_part() {
    let app = app();
    let email = format!("mp-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&app, &email).await.session_cookie().unwrap();

    let res = send(
        &app,
        graph_multipart(
            Method::POST,
            "/api/v1/appointments",
            &cookie,
            &[
                ("patient", GraphPart::Text("A. Patient")),
                ("slot", GraphPart::Text("2026-09-01T10:00Z")),
                (
                    "photo",
                    GraphPart::File("shot.png", "image/png", &png_bytes(300, 200)),
                ),
            ],
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    let id = res.data()["id"]
        .as_str()
        .expect("appointment id")
        .to_string();
    assert_eq!(res.data()["status"], "booked");
    assert_eq!(
        app.store
            .count(app.state.default_tenant_id.unwrap())
            .unwrap(),
        1,
        "text parts flowed through the normal dispatch path"
    );

    // A plain JSON book in the same fixture proves the multipart path left
    // the route behavior untouched (and `photo` is optional).
    let json_book = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            serde_json::json!({ "patient": "P. Atient", "slot": "2026-09-02T10:00Z" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(json_book.status, StatusCode::OK, "{}", json_book.body);
    assert_ne!(json_book.data()["id"].as_str(), Some(id.as_str()));
}

/// Wave 4 Phase 3: file-rule failures surface as a 422 `fields` entry for
/// the upload field — wrong media type first, then bounds — and dispatch
/// never runs.
#[tokio::test]
async fn multipart_book_rejects_files_that_fail_the_manifest_rules() {
    let app = app();
    let email = format!("mpbad-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&app, &email).await.session_cookie().unwrap();

    // A text file under the `photo` field fails `image` first.
    let res = send(
        &app,
        graph_multipart(
            Method::POST,
            "/api/v1/appointments",
            &cookie,
            &[
                ("patient", GraphPart::Text("A. Patient")),
                ("slot", GraphPart::Text("2026-09-01T10:00Z")),
                ("photo", GraphPart::File("notes.txt", "text/plain", b"not an image")),
            ],
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", res.body);
    let photo = res.body["error"]["meta"]["fields"]["photo"]
        .as_array()
        .expect("photo errors");
    assert!(photo.iter().any(|m| m == "must be an image"));
    assert_eq!(
        app.store
            .count(app.state.default_tenant_id.unwrap())
            .unwrap(),
        0,
        "edge validation runs before dispatch"
    );

    // A real PNG under `dimensions: min_w=100` fails on pixel size.
    let res = send(
        &app,
        graph_multipart(
            Method::POST,
            "/api/v1/appointments",
            &cookie,
            &[
                ("patient", GraphPart::Text("A. Patient")),
                ("slot", GraphPart::Text("2026-09-01T10:00Z")),
                ("photo", GraphPart::File("tiny.png", "image/png", &png_bytes(50, 50))),
            ],
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", res.body);
    let photo = res.body["error"]["meta"]["fields"]["photo"]
        .as_array()
        .expect("photo errors");
    assert_eq!(photo[0], "must be at least 100 pixels wide");
    assert_eq!(
        app.store
            .count(app.state.default_tenant_id.unwrap())
            .unwrap(),
        0
    );
}

/// Wave 4 Phase 3: `before:` — a valid date past the manifest cutoff fails
/// with the comparison message while every other rule passes.
#[tokio::test]
async fn book_slot_after_the_manifest_cutoff_is_rejected() {
    let app = app();
    let email = format!("before-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&app, &email).await.session_cookie().unwrap();

    let res = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            serde_json::json!({ "patient": "A. Patient", "slot": "2031-01-01T10:00Z" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", res.body);
    let slot = res.body["error"]["meta"]["fields"]["slot"]
        .as_array()
        .expect("slot errors");
    assert_eq!(slot[0], "must be before 2030-01-01");
    assert_eq!(
        app.store
            .count(app.state.default_tenant_id.unwrap())
            .unwrap(),
        0
    );
}

/// Wave 4 Phase 3: `current_password` — the cancel route re-authenticates
/// against the caller's stored credential; the wrong password is a 422
/// before dispatch, the right one lets the cancel through, and no password
/// keeps the pre-existing behavior (absent field skips the rule).
#[tokio::test]
async fn cancel_route_reauthenticates_with_current_password() {
    let app = app();
    let email = format!("reauth-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&app, &email).await.session_cookie().unwrap();

    let book = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            serde_json::json!({ "patient": "A. Patient", "slot": "2026-09-01T10:00Z" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(book.status, StatusCode::OK, "{}", book.body);
    let id = book.data()["id"]
        .as_str()
        .expect("appointment id")
        .to_string();

    // Wrong password: rejected at the edge; the appointment survives.
    let bad = send(
        &app,
        json_req(
            Method::DELETE,
            &format!("/api/v1/appointments/{id}"),
            serde_json::json!({ "password": "wrong-password" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", bad.body);
    let pw = bad.body["error"]["meta"]["fields"]["password"]
        .as_array()
        .expect("password errors");
    assert_eq!(pw[0], "is incorrect");
    let still = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(
        still.data()["appointments"].as_array().expect("rows")[0]["status"],
        "booked"
    );

    // The correct credential passes validation and the cancel lands.
    let ok = send(
        &app,
        json_req(
            Method::DELETE,
            &format!("/api/v1/appointments/{id}"),
            serde_json::json!({ "password": "correct-horse-99" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
    assert_eq!(ok.data()["status"], "cancelled");
}

/// Wave 4 Phase 4: `unique:Appointment` on book.slot — the row probe finds
/// the first live booking at that slot, so a second booking (even for a
/// different patient) is a 422 with the per-field message: no double-booking
/// reaches dispatch, and only the first row was written.
#[tokio::test]
async fn booking_the_same_slot_twice_is_rejected_as_a_unique_violation() {
    let app = app();
    let email = format!("unique-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&app, &email).await.session_cookie().unwrap();

    let first = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            serde_json::json!({ "patient": "A. Patient", "slot": "2026-09-02T10:00Z" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(first.status, StatusCode::OK, "{}", first.body);

    // Same slot, another patient: the probe answers "taken" (uniqueness is
    // on the slot column alone).
    let second = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            serde_json::json!({ "patient": "B. Patient", "slot": "2026-09-02T10:00Z" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(
        second.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        second.body
    );
    assert_eq!(second.error_code(), "KER_VALIDATION");
    let slot = second.body["error"]["meta"]["fields"]["slot"]
        .as_array()
        .expect("slot errors");
    assert!(
        slot.iter().any(|m| m.as_str() == Some("has already been taken")),
        "{}",
        second.body
    );
    assert_eq!(
        app.store
            .count(app.state.default_tenant_id.unwrap())
            .unwrap(),
        1,
        "only the first booking was written"
    );
}

/// Wave 1 public routes: `public = true` strips the implicit auth/verified
/// stages (throttle stays), and the handler plan opts out of authorization —
/// anonymous callers get a 200 envelope.
#[tokio::test]
async fn public_route_serves_anonymous_callers() {
    let app = app();
    let res = send(
        &app,
        Request::builder()
            .method(Method::GET)
            .uri("/api/v1/appointments/availability?date=2026-09-01")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.data()["date"], "2026-09-01");
    assert!(res.data()["appointments_booked"].is_number());

    // Non-public graph routes stay fail-closed for anonymous callers.
    let list = send(
        &app,
        Request::builder()
            .method(Method::GET)
            .uri("/api/v1/appointments")
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(list.status, StatusCode::UNAUTHORIZED, "{}", list.body);
}

/// Wave 1 model binding: `{id}` resolves tenant-scoped through the
/// route.bindings registry — misses, malformed ids and cross-tenant lookups
/// are 404 envelopes; hits inject the record and dispatch proceeds.
#[tokio::test]
async fn binding_miss_malformed_and_cross_tenant_are_404() {
    let app = app();
    // Tenant A (dev tenant) books an appointment.
    let email_a = format!("bind-a-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie_a = register(&app, &email_a).await.session_cookie().unwrap();
    let book = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            serde_json::json!({ "patient": "A. Patient", "slot": "2026-09-01T10:00Z" }),
            Some(&cookie_a),
            None,
        ),
    )
    .await;
    assert_eq!(book.status, StatusCode::OK, "{}", book.body);
    let id = book.data()["id"].as_str().unwrap().to_string();

    // Miss: a well-formed uuid that does not exist -> 404 before the handler.
    let ghost = uuid::Uuid::now_v7();
    let miss = send(
        &app,
        json_req(
            Method::DELETE,
            &format!("/api/v1/appointments/{ghost}"),
            Value::Null,
            Some(&cookie_a),
            None,
        ),
    )
    .await;
    assert_eq!(miss.status, StatusCode::NOT_FOUND, "{}", miss.body);

    // Malformed id -> 404 envelope (never a 422/500).
    let malformed = send(
        &app,
        json_req(
            Method::DELETE,
            "/api/v1/appointments/not-a-uuid",
            Value::Null,
            Some(&cookie_a),
            None,
        ),
    )
    .await;
    assert_eq!(malformed.status, StatusCode::NOT_FOUND, "{}", malformed.body);

    // Cross-tenant: a principal in tenant B cannot see tenant A's record.
    let tenant_b = uuid::Uuid::now_v7();
    let email_b = format!("bind-b-{}@clinic.test", uuid::Uuid::now_v7());
    let reg_b = send(
        &app,
        json_req(
            Method::POST,
            "/auth/register",
            serde_json::json!({
                "email": email_b,
                "password": "correct-horse-99",
                "tenant": tenant_b.to_string(),
            }),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(reg_b.status, StatusCode::OK, "{}", reg_b.body);
    let cookie_b = reg_b.session_cookie().unwrap();
    let cross = send(
        &app,
        json_req(
            Method::DELETE,
            &format!("/api/v1/appointments/{id}"),
            Value::Null,
            Some(&cookie_b),
            None,
        ),
    )
    .await;
    assert_eq!(cross.status, StatusCode::NOT_FOUND, "{}", cross.body);

    // Tenant A still owns the appointment: binding hit cancels it.
    let hit = send(
        &app,
        json_req(
            Method::DELETE,
            &format!("/api/v1/appointments/{id}"),
            Value::Null,
            Some(&cookie_a),
            None,
        ),
    )
    .await;
    assert_eq!(hit.status, StatusCode::OK, "{}", hit.body);
    assert_eq!(hit.data()["status"], "cancelled");
}

/// Wave 1 pagination: GET routes inject `_page` (page/per_page, clamped to
/// MAX_PER_PAGE); the appointments list reference emits page meta.
#[tokio::test]
async fn pagination_injects_page_and_clamps_per_page() {
    let app = app();
    let email = format!("page-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&app, &email).await.session_cookie().unwrap();
    for i in 0..3u32 {
        let book = send(
            &app,
            json_req(
                Method::POST,
                "/api/v1/appointments",
                serde_json::json!({
                    "patient": format!("P. {i}"),
                    "slot": format!("2026-09-0{}T10:00Z", i + 1),
                }),
                Some(&cookie),
                None,
            ),
        )
        .await;
        assert_eq!(book.status, StatusCode::OK, "{}", book.body);
    }

    // Page 1 of 2.
    let page1 = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments?page=1&per_page=2",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(page1.status, StatusCode::OK, "{}", page1.body);
    assert_eq!(page1.data()["appointments"].as_array().unwrap().len(), 2);
    assert_eq!(page1.data()["page"]["page"], 1);
    assert_eq!(page1.data()["page"]["per_page"], 2);
    assert_eq!(page1.data()["page"]["total"], 3);
    assert_eq!(page1.data()["page"]["has_more"], true);

    // Page 2 drains the remainder.
    let page2 = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments?page=2&per_page=2",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(page2.data()["appointments"].as_array().unwrap().len(), 1);
    assert_eq!(page2.data()["page"]["has_more"], false);

    // per_page clamps to MAX_PER_PAGE.
    let clamped = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments?per_page=5000",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(clamped.status, StatusCode::OK, "{}", clamped.body);
    assert_eq!(
        clamped.data()["page"]["per_page"],
        serde_json::json!(keel_http::MAX_PER_PAGE)
    );

    // page=0 is a validation error (422), not a silent default.
    let zero = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments?page=0",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(
        zero.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        zero.body
    );
}

/// Wave 1 deprecation (L72): `deprecated = true` routes emit the
/// `Deprecation: true` response header + `meta.deprecated` on success.
#[tokio::test]
async fn deprecated_route_emits_header_and_meta() {
    let app = app();
    let email = format!("dep-{}@clinic.test", uuid::Uuid::now_v7());
    let cookie = register(&app, &email).await.session_cookie().unwrap();
    let res = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments/history",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(
        res.headers
            .get("deprecation")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
        Some("true".to_string())
    );
    assert_eq!(res.body["meta"]["deprecated"], true);
    assert!(res.data()["appointments"].is_array());
}

/// Wave 1 throttle groups: a named group is one token bucket across routes;
/// exhaustion is a 429 + Retry-After, and throttle runs BEFORE auth
/// (exhaustion beats authentication).
#[tokio::test]
async fn throttle_group_exhaustion_is_429() {
    let dir = std::env::temp_dir().join(format!("keel-api-it-{}", uuid::Uuid::now_v7()));
    let (mut state, graph, _store) = keel_api::build_app_state(&dir, false, true);
    state.middleware_catalog.set_throttle(
        "api",
        keel_http::ThrottleGroupConfig {
            capacity: 2,
            refill_per_sec: 0.0,
        },
    );
    let router = keel_api::routes::router(state.clone())
        .merge(keel_api::graph_routes::graph_router(state.clone(), &graph));

    let req = || {
        Request::builder()
            .method(Method::GET)
            .uri("/api/v1/appointments/availability?date=2026-09-01")
            .body(Body::empty())
            .unwrap()
    };

    // Capacity 2: both pass (public route, no credentials needed).
    for _ in 0..2 {
        let res = router.clone().oneshot(req()).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }
    // Third request: bucket exhausted -> 429 + Retry-After.
    let res = router.oneshot(req()).await.unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(
        res.headers()
            .get(header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string),
        Some("1".to_string())
    );
}

// --------------------------- remember-me + password-confirm (Wave 4 P7)

/// docs/19 L220: `remember: true` prolongs the cookie to the policy's 30 day
/// window (vs the 8 hour default) and `/auth/me` reports the bag flag.
#[tokio::test]
async fn remember_login_issues_the_30d_cookie_and_me_reports_the_flag() {
    let app = app();
    let email = format!("remember-{}@clinic.test", uuid::Uuid::now_v7());
    register(&app, &email).await;

    // Plain login: the default 8 hour cookie, flag absent.
    let plain = login(&app, &email, "correct-horse-99", Some("dev")).await;
    assert_eq!(plain.status, StatusCode::OK, "{}", plain.body);
    let plain_set = plain
        .set_cookies
        .iter()
        .find(|c| c.starts_with("keel_session="))
        .expect("session cookie");
    assert!(plain_set.contains("Max-Age=28800"), "{plain_set}");
    let plain_cookie = plain.session_cookie().unwrap();
    let me = send(
        &app,
        json_req(
            Method::GET,
            "/auth/me",
            Value::Null,
            Some(&plain_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.body);
    assert_eq!(me.data()["remember"], false);

    // Remembered login: 30d cookie (2_592_000) + bag flag reported by me.
    let body = serde_json::json!({
        "identifier": email,
        "password": "correct-horse-99",
        "tenant": "dev",
        "remember": true,
    });
    let rem = send(
        &app,
        json_req(Method::POST, "/auth/login", body, None, None),
    )
    .await;
    assert_eq!(rem.status, StatusCode::OK, "{}", rem.body);
    let rem_set = rem
        .set_cookies
        .iter()
        .find(|c| c.starts_with("keel_session="))
        .expect("session cookie");
    assert!(rem_set.contains("Max-Age=2592000"), "{rem_set}");
    let rem_cookie = rem.session_cookie().unwrap();
    let me = send(
        &app,
        json_req(
            Method::GET,
            "/auth/me",
            Value::Null,
            Some(&rem_cookie),
            None,
        ),
    )
    .await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.body);
    assert_eq!(me.data()["remember"], true);
}

/// docs/19 L221: `POST /auth/confirm` re-verifies the password (401 on a
/// miss or no session) and stamps an rfc3339 `confirmed_at` into the bag —
/// the stamp `confirmed`-chained routes admit on (window gate covered in
/// keel-http `confirmed_stage_enforces_the_password_confirmation_window`).
#[tokio::test]
async fn password_confirm_stamps_the_session_and_fails_closed() {
    let app = app();
    let email = format!("confirm-{}@clinic.test", uuid::Uuid::now_v7());
    let reg = register(&app, &email).await;
    assert_eq!(reg.status, StatusCode::OK, "{}", reg.body);
    let cookie = reg.session_cookie().expect("session cookie");

    // No session: nothing to stamp -> 401.
    let anon = send(
        &app,
        json_req(
            Method::POST,
            "/auth/confirm",
            serde_json::json!({ "password": "correct-horse-99" }),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED, "{}", anon.body);

    // Wrong password: credential re-verified -> 401, no stamp.
    let bad = send(
        &app,
        json_req(
            Method::POST,
            "/auth/confirm",
            serde_json::json!({ "password": "wrong-password" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(bad.status, StatusCode::UNAUTHORIZED, "{}", bad.body);

    // Right password -> stamp lands.
    let ok = send(
        &app,
        json_req(
            Method::POST,
            "/auth/confirm",
            serde_json::json!({ "password": "correct-horse-99" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
    assert_eq!(ok.data()["confirmed"], true);

    // Read the bag back through the session store for the same cookie: the
    // stamp is an rfc3339 timestamp the `confirmed` gate can parse.
    let value = cookie.strip_prefix("keel_session=").expect("cookie value");
    let sid = keel_session::SessionId::parse(value.split('.').next().unwrap())
        .expect("session id");
    let stored = app
        .state
        .sessions
        .as_ref()
        .expect("sessions on")
        .manager
        .store()
        .load(&sid)
        .expect("store reads")
        .expect("session lives");
    let stamped = stored
        .data
        .get("confirmed_at")
        .and_then(|v| v.as_str())
        .expect("confirmed_at stamped");
    time::OffsetDateTime::parse(stamped, &time::format_description::well_known::Rfc3339)
        .expect("rfc3339 stamp");

    // Confirm never implies remember-me.
    let me = send(
        &app,
        json_req(
            Method::GET,
            "/auth/me",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.body);
    assert_eq!(me.data()["remember"], false);
}

// ------------------------------------------------------------- Wave 6 P5

/// Register, then resolve the caller's principal id through `/auth/me` — the
/// same identity the feed scopes to.
async fn inbox_owner(app: &App, email: &str) -> (String, keel_foundation::PrincipalId) {
    let cookie = register(app, email)
        .await
        .session_cookie()
        .expect("session cookie issued");
    let me = send(
        app,
        json_req(Method::GET, "/auth/me", Value::Null, Some(&cookie), None),
    )
    .await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.body);
    let raw = me.data()["principal"]["principal_id"]
        .as_str()
        .expect("principal id")
        .to_string();
    let principal =
        keel_foundation::PrincipalId::from_uuid(uuid::Uuid::parse_str(&raw).expect("uuid"));
    (cookie, principal)
}

/// Seed rows through the very port the hub dispatches on; the first `read`
/// arrive already marked so the feed counts exercise both states.
async fn seed_notifications(
    app: &App,
    principal: keel_foundation::PrincipalId,
    count: usize,
    read: usize,
) -> Vec<String> {
    let mut ids = Vec::with_capacity(count);
    for i in 0..count {
        let id = uuid::Uuid::now_v7();
        ids.push(id.to_string());
        app.state
            .notifications
            .insert(keel_notifications::NotificationRow {
                id,
                principal_id: principal,
                tenant_id: app.state.default_tenant_id,
                kind: "appointment.booked".into(),
                data: serde_json::json!({ "n": i }),
                read_at: (i < read).then(time::OffsetDateTime::now_utc),
                created_at: time::OffsetDateTime::now_utc(),
            })
            .await
            .expect("seed row");
    }
    ids
}

#[tokio::test]
async fn notifications_endpoints_require_authentication() {
    let app = app();
    let list = send(
        &app,
        json_req(Method::GET, "/api/v1/notifications", Value::Null, None, None),
    )
    .await;
    assert_eq!(list.status, StatusCode::UNAUTHORIZED, "{}", list.body);
    assert_eq!(list.error_code(), keel_foundation::codes::UNAUTHENTICATED);

    let read = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/notifications/read",
            serde_json::json!({ "ids": ["01900000-0000-7000-8000-000000000000"] }),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(read.status, StatusCode::UNAUTHORIZED, "{}", read.body);
    assert_eq!(read.error_code(), keel_foundation::codes::UNAUTHENTICATED);
}

#[tokio::test]
async fn notifications_feed_lists_newest_first_with_unread_meta() {
    let app = app();
    let email = format!("inbox-{}@clinic.test", uuid::Uuid::now_v7());
    let (cookie, principal) = inbox_owner(&app, &email).await;
    seed_notifications(&app, principal, 3, 1).await;

    let feed = send(
        &app,
        json_req(Method::GET, "/api/v1/notifications", Value::Null, Some(&cookie), None),
    )
    .await;
    assert_eq!(feed.status, StatusCode::OK, "{}", feed.body);
    let data = feed.data();
    let rows = data.as_array().expect("row array");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["type"], "appointment.booked");
    assert_eq!(rows[0]["data"]["n"], 2, "newest first");
    assert_eq!(rows[1]["data"]["n"], 1);
    assert_eq!(rows[2]["data"]["n"], 0);
    assert!(rows[2]["read_at"].as_str().is_some(), "the pre-read row carries read_at");
    assert!(rows[0]["read_at"].is_null());
    assert!(rows[0]["id"].as_str().is_some());
    assert!(rows[0]["created_at"].as_str().is_some());
    // Self-scoped wire: no principal/tenant columns ever leave the edge.
    assert!(rows[0].get("principal_id").is_none());
    assert!(rows[0].get("tenant_id").is_none());

    assert_eq!(feed.body["meta"]["page"], 1);
    assert_eq!(feed.body["meta"]["per_page"], 25);
    assert_eq!(feed.body["meta"]["total"], 3);
    assert_eq!(feed.body["meta"]["unread"], 2);
    assert_eq!(feed.body["meta"]["has_more"], false);

    // A tight page proves has_more flips on (total outruns the window) …
    let tight = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/notifications?per_page=2",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(tight.status, StatusCode::OK, "{}", tight.body);
    assert_eq!(tight.data().as_array().expect("rows").len(), 2);
    assert_eq!(tight.body["meta"]["has_more"], true);
    assert_eq!(tight.body["meta"]["total"], 3);

    // … and page 2 lands on exactly the oldest row with counts intact.
    let page2 = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/notifications?page=2&per_page=2",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(page2.status, StatusCode::OK, "{}", page2.body);
    let data = page2.data();
    let rows = data.as_array().expect("row array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["data"]["n"], 0);
    assert_eq!(page2.body["meta"]["page"], 2);
    assert_eq!(page2.body["meta"]["total"], 3);
    assert_eq!(page2.body["meta"]["has_more"], false);

    // The page query validates like every other list edge (422 envelope).
    let bad = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/notifications?page=0",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(bad.status, StatusCode::UNPROCESSABLE_ENTITY, "{}", bad.body);
    assert_eq!(bad.error_code(), keel_foundation::codes::VALIDATION);

    // Another principal's rows exist — and never leak into this feed.
    let other_email = format!("other-inbox-{}@clinic.test", uuid::Uuid::now_v7());
    let (_other_cookie, other) = inbox_owner(&app, &other_email).await;
    seed_notifications(&app, other, 2, 0).await;
    let mine = send(
        &app,
        json_req(Method::GET, "/api/v1/notifications", Value::Null, Some(&cookie), None),
    )
    .await;
    assert_eq!(
        mine.body["meta"]["total"], 3,
        "someone else's rows never count"
    );
}

#[tokio::test]
async fn notifications_read_marks_rows_and_gates_the_capability() {
    let app = app();
    let email = format!("readmark-{}@clinic.test", uuid::Uuid::now_v7());
    let (cookie, principal) = inbox_owner(&app, &email).await;
    let ids = seed_notifications(&app, principal, 2, 0).await;

    // Foreign ids report 0 changed — scoping, not a 403 leak.
    let foreign = keel_foundation::PrincipalId::new();
    let foreign_ids = seed_notifications(&app, foreign, 1, 0).await;
    let res = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/notifications/read",
            serde_json::json!({ "ids": [foreign_ids[0].clone()] }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.data()["updated"], 0, "another principal's rows never mark");

    // Happy path: mark one → updated 1, unread drops 2 → 1.
    let res = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/notifications/read",
            serde_json::json!({ "ids": [ids[0].clone()] }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.data()["updated"], 1);
    let feed = send(
        &app,
        json_req(Method::GET, "/api/v1/notifications", Value::Null, Some(&cookie), None),
    )
    .await;
    assert_eq!(feed.body["meta"]["unread"], 1);

    // Re-marking the same id reports 0 (idempotent).
    let res = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/notifications/read",
            serde_json::json!({ "ids": [ids[0].clone()] }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(res.data()["updated"], 0, "already-read rows never recount");

    // Body contract: empty, malformed and missing ids are 422 envelopes.
    for body in [
        serde_json::json!({ "ids": [] }),
        serde_json::json!({ "ids": ["not-a-uuid"] }),
        serde_json::json!({}),
    ] {
        let res = send(
            &app,
            json_req(
                Method::POST,
                "/api/v1/notifications/read",
                body.clone(),
                Some(&cookie),
                None,
            ),
        )
        .await;
        assert_eq!(
            res.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{}",
            res.body
        );
        assert_eq!(res.error_code(), keel_foundation::codes::VALIDATION);
    }

    // Capability gate: a snapshot WITHOUT notifications.mark_read refuses
    // the mark (403) — the check exists and stays revocable — and the row
    // it would have marked stays unread.
    app.state
        .policy
        .install_snapshot(keel_policy::AccessSnapshot {
            principal_id: principal,
            tenant_id: app.state.default_tenant_id.expect("dev tenant"),
            membership_active: true,
            capabilities: ["appointments.list".to_string()].into_iter().collect(),
            scope_paths: ["/".to_string()].into_iter().collect(),
            authorization_revision: app.state.policy.revision(),
            feature_revision: keel_foundation::FeatureRevision(1),
            module_revision: keel_foundation::ModuleRevision(1),
            computed_at: time::OffsetDateTime::now_utc(),
        });
    let res = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/notifications/read",
            serde_json::json!({ "ids": [ids[1].clone()] }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN, "{}", res.body);
    assert_eq!(res.error_code(), keel_foundation::codes::DENIED);
    let feed = send(
        &app,
        json_req(Method::GET, "/api/v1/notifications", Value::Null, Some(&cookie), None),
    )
    .await;
    assert_eq!(
        feed.body["meta"]["unread"], 1,
        "the denied mark never landed"
    );
}

// ------------------------------------------------------------- Wave 6 P6

/// The harness reads `<dir>/lang` at build time — seed the locales BEFORE
/// `build_app_state` so the edge's `Accept-Language` negotiation runs
/// against a real catalog (mirrors the views suite's fixture pattern).
fn app_with_lang() -> App {
    let dir = std::env::temp_dir().join(format!("keel-api-p6-{}", uuid::Uuid::now_v7()));
    let lang = dir.join("lang");
    std::fs::create_dir_all(&lang).expect("lang fixture dir");
    std::fs::write(
        lang.join("en.toml"),
        "[notifications]\nappointment_booked = \"Appointment booked for {patient}\"\n",
    )
    .expect("en lang fixture writes");
    std::fs::write(
        lang.join("fr.toml"),
        "[notifications]\nappointment_booked = \"Rendez-vous réservé pour {patient}\"\n",
    )
    .expect("fr lang fixture writes");
    let (state, graph, store) = keel_api::build_app_state(&dir, true, true);
    let router = keel_api::routes::router(state.clone())
        .merge(keel_api::graph_routes::graph_router(state.clone(), &graph))
        .merge(keel_api::auth_routes::router(state.clone()))
        .merge(keel_api::upload_routes::router(state.clone()))
        .merge(keel_api::notification_routes::router(state.clone()))
        .merge(keel_api::batch_routes::router(state.clone()));
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

/// Wave 6 P6: producer dispatch over HTTP — a booking with
/// `Accept-Language: fr` resolves the request locale at the edge, the
/// book handler dispatches `appointment.booked` through the hub, and the
/// row appears in the very `/api/v1/notifications` feed the edge reads,
/// carrying `data.locale == "fr"` (self-scoped wire, unread meta).
#[tokio::test]
async fn booked_appointment_lands_in_the_inbox_in_the_request_locale() {
    let app = app_with_lang();
    let email = format!("p6-{}@clinic.test", uuid::Uuid::now_v7());
    let (cookie, _) = inbox_owner(&app, &email).await;

    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/appointments")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, &cookie)
        .header(header::ACCEPT_LANGUAGE, "fr")
        .body(Body::from(
            serde_json::json!({ "patient": "A. Patient", "slot": "2026-09-01T10:00Z" })
                .to_string(),
        ))
        .unwrap();
    let book = send(&app, req).await;
    assert_eq!(book.status, StatusCode::OK, "book: {}", book.body);
    assert_eq!(book.data()["status"], "booked");

    let feed = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/notifications",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(feed.status, StatusCode::OK, "{}", feed.body);
    let data = feed.data();
    let rows = data.as_array().expect("row array");
    assert_eq!(rows.len(), 1, "one producer row for one booking");
    assert_eq!(rows[0]["type"], "appointment.booked");
    assert_eq!(rows[0]["data"]["patient"], "A. Patient");
    assert_eq!(rows[0]["data"]["slot"], "2026-09-01T10:00Z");
    assert_eq!(
        rows[0]["data"]["locale"], "fr",
        "the request locale rides the dispatch payload"
    );
    assert!(rows[0]["read_at"].is_null());
    assert!(rows[0].get("principal_id").is_none(), "self-scoped wire");
    assert_eq!(feed.body["meta"]["total"], 1);
    assert_eq!(feed.body["meta"]["unread"], 1);
}

// ------------------------------------------------------------- Wave 6 P7

/// Raw oneshot against the subscription path — the SSE body never ends, so
/// the response is returned UNREAD (read it and the await hangs; the
/// presence tests drop it to simulate the disconnect).
async fn sse_connect(app: &App, uri: &str) -> axum::http::Response<Body> {
    app.router
        .clone()
        .oneshot(
            Request::builder()
                .uri(uri)
                .body(Body::empty())
                .expect("request builds"),
        )
        .await
        .expect("request serves")
}

/// The harness graph ships no `routes/channels.toml` entries, so the P7
/// fixtures resolve the granted-form capability first — what login
/// actually puts on the snapshot; `serve()` runs the same
/// `broadcast_routes::resolve_channels` against the real graph — and set
/// the state BEFORE the router is built: a channel gated on a granted
/// capability, one on an undeclared ghost, one presence roster, one
/// ghost-gated presence. The authorized transports mount with the very
/// `ChannelTokenAuthorizer` serve() wires.
fn app_with_channels() -> App {
    let dir = std::env::temp_dir().join(format!("keel-api-p7-{}", uuid::Uuid::now_v7()));
    let (mut state, graph, store) = keel_api::build_app_state(&dir, true, true);
    let granted = state
        .granted_capabilities
        .iter()
        .find(|cap| cap.starts_with("appointments."))
        .cloned()
        .expect("module capability granted at boot");
    let ghost = vec!["ghost.missing".to_string()];
    state.channels = vec![
        keel_api::broadcast_routes::ChannelAuthz {
            name: "appointments:private".into(),
            allowed: vec![granted.clone()],
        },
        keel_api::broadcast_routes::ChannelAuthz {
            name: "ghost:channel".into(),
            allowed: ghost.clone(),
        },
        keel_api::broadcast_routes::ChannelAuthz {
            name: "presence-lobby".into(),
            allowed: vec![granted],
        },
        keel_api::broadcast_routes::ChannelAuthz {
            name: "presence-ghost".into(),
            allowed: ghost,
        },
    ];
    let realtime = keel_realtime::realtime_router_authorized(
        keel_realtime::BroadcastHub::new(),
        state.presence.clone(),
        Arc::new(keel_api::broadcast_routes::ChannelTokenAuthorizer {
            tokens: state.tokens.clone(),
            principals: state.principals.clone(),
        }),
    );
    let router = keel_api::routes::router(state.clone())
        .merge(keel_api::graph_routes::graph_router(state.clone(), &graph))
        .merge(keel_api::auth_routes::router(state.clone()))
        .merge(keel_api::upload_routes::router(state.clone()))
        .merge(keel_api::notification_routes::router(state.clone()))
        .merge(keel_api::broadcast_routes::router(state.clone()))
        .merge(realtime);
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

/// Wave 6 P7: the broadcast auth matrix — allowed (registered channel with
/// a granted capability -> short-TTL channel-bound token), denied
/// capability, unregistered channel (the same denial — no oracle),
/// anonymous caller, and the subscription path itself: the issued token
/// opens the stream while a missing, wrong, cross-channel or expired token
/// refuses the connect with one uniform 401.
#[tokio::test]
async fn broadcast_auth_matrix_allowed_denied_and_expired() {
    let app = app_with_channels();
    let email = format!("p7-{}@clinic.test", uuid::Uuid::now_v7());
    let (cookie, principal) = inbox_owner(&app, &email).await;

    // Allowed: capability on the snapshot -> token issued.
    let ok = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/broadcast/auth",
            serde_json::json!({ "channel": "appointments:private" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(ok.status, StatusCode::OK, "{}", ok.body);
    assert_eq!(ok.data()["channel"], "appointments:private");
    let token = ok.data()["token"]
        .as_str()
        .expect("channel token")
        .to_string();
    assert!(
        !ok.data()["expires_at"].is_null(),
        "the short TTL is visible to the client"
    );

    // Denied: capability not on the snapshot.
    let denied = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/broadcast/auth",
            serde_json::json!({ "channel": "ghost:channel" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{}", denied.body);
    assert_eq!(denied.error_code(), keel_foundation::codes::DENIED);

    // Unregistered: the same denial — the endpoint never reveals which.
    let unknown = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/broadcast/auth",
            serde_json::json!({ "channel": "never:registered" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(unknown.status, StatusCode::FORBIDDEN, "{}", unknown.body);
    assert_eq!(unknown.error_code(), keel_foundation::codes::DENIED);

    // Anonymous callers never reach the gate.
    let anon = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/broadcast/auth",
            serde_json::json!({ "channel": "appointments:private" }),
            None,
            None,
        ),
    )
    .await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED, "{}", anon.body);

    // A second token, bound to ANOTHER channel by its label.
    let other = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/broadcast/auth",
            serde_json::json!({ "channel": "presence-lobby" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(other.status, StatusCode::OK, "{}", other.body);
    let other_token = other.data()["token"]
        .as_str()
        .expect("second channel token")
        .to_string();

    // A token minted for a tiny TTL, then allowed to expire.
    let expired = app
        .state
        .tokens
        .issue(
            principal,
            keel_tokens::TokenPurpose::Channel,
            time::Duration::milliseconds(5),
            Some("appointments:private".into()),
        )
        .expect("token issues");
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;

    // Subscription: the issued token opens the stream...
    let sse = sse_connect(
        &app,
        &format!("/realtime/sse?channel=appointments:private&token={token}"),
    )
    .await;
    assert_eq!(sse.status(), StatusCode::OK);
    let ct = sse
        .headers()
        .get(header::CONTENT_TYPE)
        .expect("content type")
        .to_str()
        .expect("content type text");
    assert!(ct.starts_with("text/event-stream"), "got {ct}");
    drop(sse);

    // ...and every other presented credential refuses identically.
    for (label, uri) in [
        (
            "missing token",
            "/realtime/sse?channel=appointments:private".to_string(),
        ),
        (
            "wrong token",
            "/realtime/sse?channel=appointments:private&token=not-a-token".to_string(),
        ),
        (
            "token bound to another channel",
            format!("/realtime/sse?channel=appointments:private&token={other_token}"),
        ),
        (
            "expired token",
            format!(
                "/realtime/sse?channel=appointments:private&token={}",
                expired.plaintext
            ),
        ),
    ] {
        let res = sse_connect(&app, &uri).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{label}");
    }
}

/// Wave 6 P7: the presence member list — capability-gated, `presence-*`
/// only, and it tracks the roster the subscription path maintains: a
/// connect joins under the principal's email, the disconnect (stream
/// dropped) leaves.
#[tokio::test]
async fn presence_member_list_tracks_join_and_leave() {
    let app = app_with_channels();
    let email = format!("p7-presence-{}@clinic.test", uuid::Uuid::now_v7());
    let (cookie, _) = inbox_owner(&app, &email).await;

    // Capability gate: the ghost-gated presence channel is denied, and a
    // non-presence channel is refused as a validation error.
    let denied = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/broadcast/members?channel=presence-ghost",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{}", denied.body);
    assert_eq!(denied.error_code(), keel_foundation::codes::DENIED);

    let wrong_kind = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/broadcast/members?channel=appointments:private",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(
        wrong_kind.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        wrong_kind.body
    );

    // The roster starts empty.
    let empty = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/broadcast/members?channel=presence-lobby",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(empty.status, StatusCode::OK, "{}", empty.body);
    let data = empty.data();
    let list = data["members"].as_array().expect("member array");
    assert_eq!(list.len(), 0, "roster starts empty");

    // Issue the connect token and open the stream (kept alive, unread).
    let auth = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/broadcast/auth",
            serde_json::json!({ "channel": "presence-lobby" }),
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(auth.status, StatusCode::OK, "{}", auth.body);
    let token = auth.data()["token"]
        .as_str()
        .expect("channel token")
        .to_string();
    let stream = sse_connect(
        &app,
        &format!("/realtime/sse?channel=presence-lobby&token={token}"),
    )
    .await;
    assert_eq!(stream.status(), StatusCode::OK);

    // Connected: exactly the caller is on the roster, named by their email.
    let joined = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/broadcast/members?channel=presence-lobby",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(joined.status, StatusCode::OK, "{}", joined.body);
    let data = joined.data();
    let list = data["members"].as_array().expect("member array");
    assert_eq!(list.len(), 1, "the subscriber joined the roster");
    assert_eq!(list[0]["display_name"], email);

    // The disconnect (response dropped) leaves the roster.
    drop(stream);
    let after = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/broadcast/members?channel=presence-lobby",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(after.status, StatusCode::OK, "{}", after.body);
    let data = after.data();
    let list = data["members"].as_array().expect("member array");
    assert_eq!(list.len(), 0, "the dropped stream left the roster");
}

// ------------------------------------------------------------- Wave 6 P9

/// The batch surface needs the admin store + ledger `serve()` wires at full
/// boot; `build_app_state` is degraded (None/None), so inject them before
/// the router assembles — the SAME instances the batch runtime dispatches
/// through. Returns them for the test to dispatch with.
fn batch_app() -> (
    App,
    Arc<keel_jobs::InMemoryJobStore>,
    Arc<keel_jobs::MemoryBatchLedger>,
) {
    let dir = std::env::temp_dir().join(format!("keel-api-p9-{}", uuid::Uuid::now_v7()));
    let (mut state, graph, store) = keel_api::build_app_state(&dir, true, true);
    let jobs = Arc::new(keel_jobs::InMemoryJobStore::new());
    let ledger = Arc::new(keel_jobs::MemoryBatchLedger::new());
    state.queue = Some(jobs.clone());
    state.batches = Some(ledger.clone());
    let router = keel_api::routes::router(state.clone())
        .merge(keel_api::graph_routes::graph_router(state.clone(), &graph))
        .merge(keel_api::auth_routes::router(state.clone()))
        .merge(keel_api::upload_routes::router(state.clone()))
        .merge(keel_api::notification_routes::router(state.clone()))
        .merge(keel_api::batch_routes::router(state.clone()));
    let router = state
        .sessions
        .as_ref()
        .expect("sessions enabled")
        .as_ref()
        .clone()
        .apply(router);
    (
        App {
            router,
            state,
            store,
        },
        jobs,
        ledger,
    )
}

/// One pending batch member — the jobs suite's fixture shape (all fields
/// public, built directly).
fn pending_member() -> keel_jobs::DurableJob {
    use keel_jobs::JobStatus;
    let now = time::OffsetDateTime::now_utc();
    keel_jobs::DurableJob {
        id: uuid::Uuid::now_v7(),
        execution_id: keel_foundation::ExecutionId::new(),
        name: "demo.job".into(),
        payload: serde_json::Value::Null,
        queue: "default".into(),
        attempt: 0,
        max_attempts: 3,
        deadline: now + time::Duration::minutes(5),
        available_at: now,
        status: JobStatus::Pending,
        priority: 0,
        claimed_by: None,
        claimed_at: None,
        causation_id: None,
        correlation_id: None,
        last_error: None,
        enqueued_at: now,
        finished_at: None,
    }
}

/// Capability-gated operations: anonymous callers stop at the identity
/// resolver, a login-granted snapshot passes the gate and misses the empty
/// ledger (404, not a gate error), a malformed path id is a 422 on the
/// shared validation shape, and a snapshot WITHOUT the batch capability is
/// refused 403 before any ledger is touched.
#[tokio::test]
async fn batch_endpoints_require_authentication_and_capabilities() {
    let (app, _jobs, _ledger) = batch_app();

    let anon = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/batches/01900000-0000-7000-8000-000000000000",
            Value::Null,
            None,
            None,
        ),
    )
    .await;
    assert_eq!(anon.status, StatusCode::UNAUTHORIZED, "{}", anon.body);
    assert_eq!(anon.error_code(), keel_foundation::codes::UNAUTHENTICATED);

    let (cookie, principal) = inbox_owner(
        &app,
        &format!("batch-gate-{}@clinic.test", uuid::Uuid::now_v7()),
    )
    .await;

    let miss = send(
        &app,
        json_req(
            Method::GET,
            &format!("/api/v1/batches/{}", uuid::Uuid::now_v7()),
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(miss.status, StatusCode::NOT_FOUND, "{}", miss.body);
    assert_eq!(miss.error_code(), keel_foundation::codes::NOT_FOUND);

    let bad = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/batches/not-a-uuid",
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(
        bad.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        bad.body
    );
    assert_eq!(bad.error_code(), keel_foundation::codes::VALIDATION);

    // Capability gate: a snapshot WITHOUT batches.cancel refuses (403) —
    // the check exists and stays revocable per snapshot (P5 posture).
    app.state
        .policy
        .install_snapshot(keel_policy::AccessSnapshot {
            principal_id: principal,
            tenant_id: app.state.default_tenant_id.expect("dev tenant"),
            membership_active: true,
            capabilities: ["appointments.list".to_string()].into_iter().collect(),
            scope_paths: ["/".to_string()].into_iter().collect(),
            authorization_revision: app.state.policy.revision(),
            feature_revision: keel_foundation::FeatureRevision(1),
            module_revision: keel_foundation::ModuleRevision(1),
            computed_at: time::OffsetDateTime::now_utc(),
        });
    let denied = send(
        &app,
        json_req(
            Method::POST,
            &format!(
                "/api/v1/batches/{}/cancel",
                uuid::Uuid::now_v7()
            ),
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(denied.status, StatusCode::FORBIDDEN, "{}", denied.body);
    assert_eq!(denied.error_code(), keel_foundation::codes::DENIED);
}

/// The full operation round trip over one dispatched batch: status counts
/// live store state, cancel takes the pending members to dead (leaving the
/// finished one), retry puts the dead back to pending — the same service
/// `queue:retry-batch` runs.
#[tokio::test]
async fn batch_status_cancel_retry_roundtrip() {
    use keel_jobs::{BatchRuntime, BatchSpec, JobStore};

    let (app, jobs, ledger) = batch_app();
    let (cookie, _principal) = inbox_owner(
        &app,
        &format!("batch-ops-{}@clinic.test", uuid::Uuid::now_v7()),
    )
    .await;

    let runtime = BatchRuntime::with_ledger(jobs.clone(), ledger.clone());
    let spec = BatchSpec {
        id: uuid::Uuid::now_v7(),
        name: "nightly".into(),
        jobs: vec![pending_member(), pending_member(), pending_member()],
        on_complete: None,
    };
    let batch = runtime.dispatch(spec).expect("dispatches");
    let path = format!("/api/v1/batches/{batch}");

    let res = send(
        &app,
        json_req(Method::GET, &path, Value::Null, Some(&cookie), None),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.data()["total"], 3);
    assert_eq!(res.data()["pending"], 3);
    assert_eq!(res.data()["finished"], false);

    let claimed = jobs.claim("worker").expect("claims").expect("claimable");
    jobs.complete(claimed.id).expect("completes");
    let res = send(
        &app,
        json_req(Method::GET, &path, Value::Null, Some(&cookie), None),
    )
    .await;
    assert_eq!(res.data()["succeeded"], 1, "{}", res.body);
    assert_eq!(res.data()["pending"], 2);

    let res = send(
        &app,
        json_req(
            Method::POST,
            &format!("{path}/cancel"),
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.data()["killed"], 2, "{}", res.body);
    assert_eq!(res.data()["running"], 0);
    assert_eq!(res.data()["settled"], 1, "the finished member is left alone");

    let res = send(
        &app,
        json_req(Method::GET, &path, Value::Null, Some(&cookie), None),
    )
    .await;
    assert_eq!(res.data()["dead"], 2, "{}", res.body);
    assert_eq!(res.data()["succeeded"], 1);
    assert_eq!(res.data()["finished"], true);

    let res = send(
        &app,
        json_req(
            Method::POST,
            &format!("{path}/retry"),
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::OK, "{}", res.body);
    assert_eq!(res.data()["requeued"], 2, "{}", res.body);

    let res = send(
        &app,
        json_req(Method::GET, &path, Value::Null, Some(&cookie), None),
    )
    .await;
    assert_eq!(res.data()["pending"], 2, "{}", res.body);
    assert_eq!(res.data()["dead"], 0);
    assert_eq!(res.data()["succeeded"], 1);

    // A batch the ledger never saw is a plain 404 through the same gate.
    let res = send(
        &app,
        json_req(
            Method::GET,
            &format!("/api/v1/batches/{}", uuid::Uuid::now_v7()),
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(res.status, StatusCode::NOT_FOUND, "{}", res.body);
}

/// Degraded boot (D-52): without a pool the builder state carries no
/// ledger — the endpoint fails closed with KER_NOT_IMPLEMENTED instead of
/// reporting batches it cannot see.
#[tokio::test]
async fn batch_endpoints_fail_closed_without_a_ledger() {
    let app = app();
    let (cookie, _principal) = inbox_owner(
        &app,
        &format!("batch-degraded-{}@clinic.test", uuid::Uuid::now_v7()),
    )
    .await;
    let res = send(
        &app,
        json_req(
            Method::GET,
            &format!("/api/v1/batches/{}", uuid::Uuid::now_v7()),
            Value::Null,
            Some(&cookie),
            None,
        ),
    )
    .await;
    assert_eq!(
        res.error_code(),
        keel_foundation::codes::NOT_IMPLEMENTED,
        "{}",
        res.body
    );
}
