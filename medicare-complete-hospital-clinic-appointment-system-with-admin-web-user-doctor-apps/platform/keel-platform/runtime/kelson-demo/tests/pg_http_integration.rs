//! Row 19 (tenant Pg adapter): HTTP dispatch over the PostgreSQL model
//! store. Production's boot binds the appointments handlers to
//! `PgModelStore` when the driver pool is up and seeds the dev tenant (the
//! FK target for domain writes); this suite drives that same wiring end to
//! end over the edge — book → persist → list → cross-tenant invisible →
//! cancel → conflict — with the established pg-suite posture: no
//! DATABASE_URL (or an unreachable database) is a graceful skip, and every
//! identifier is a per-run uuid so a shared compose database stays clean.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

struct App {
    router: axum::Router,
}

/// Mirror of the production pool bootstrap (lib.rs `run`): connect, apply
/// the compiled-in migrations (a missing pairs directory = "none yet"), and
/// seed the dev tenant the auth flows assign — appointments rows FK it.
async fn pg_pool() -> Option<kelson_postgres::PgPool> {
    let url = match std::env::var("DATABASE_URL") {
        Ok(url) if !url.is_empty() => url,
        _ => {
            eprintln!("pg-http: DATABASE_URL not set; skipping");
            return None;
        }
    };
    match kelson_postgres::create_pool(&url, 4).await {
        Ok(pool) => {
            kelson_postgres::run_migrations(&pool, std::path::Path::new("."))
                .await
                .expect("migrations apply");
            let dev = uuid::Uuid::parse_str(kelson_demo::host::DEV_TENANT_UUID)
                .expect("static dev tenant uuid");
            kelson_postgres::seed_tenant(&pool, dev, "dev", "Development Tenant")
                .await
                .expect("dev tenant seeds");
            Some(pool)
        }
        Err(e) => {
            eprintln!("pg-http: cannot connect ({e}); skipping");
            None
        }
    }
}

/// The production-shaped edge: the same merged router the in-memory suite
/// builds, but with the appointments handlers bound to the injected
/// PostgreSQL backend (the `PersistenceProvider` posture).
fn app_over_pg(store: kelson_postgres::PgModelStore) -> App {
    let dir = std::env::temp_dir().join(format!("kelson-demo-pg-http-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("temp root");
    let (state, graph, _store) =
        kelson_demo::build_app_state_with_backend(&dir, true, true, Arc::new(store));
    let router = kelson_demo::routes::router(state.clone())
        .merge(kelson_demo::graph_routes::graph_router(
            state.clone(),
            &graph,
        ))
        .merge(kelson_demo::auth_routes::router(state.clone()))
        .merge(kelson_demo::upload_routes::router(state.clone()))
        .merge(kelson_demo::notification_routes::router(state.clone()))
        .merge(kelson_demo::batch_routes::router(state.clone()));
    let router = state
        .sessions
        .as_ref()
        .expect("sessions enabled")
        .as_ref()
        .clone()
        .apply(router);
    App { router }
}

#[derive(Debug)]
struct Reply {
    status: StatusCode,
    body: Value,
    set_cookies: Vec<String>,
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
            .find(|c| c.starts_with("kelson_session="))
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

async fn register(app: &App, email: &str, tenant: &str) -> Reply {
    let body =
        serde_json::json!({ "email": email, "password": "Correct-horse-99", "tenant": tenant });
    send(app, json_req(Method::POST, "/auth/register", body, None)).await
}

fn listed_ids(list: &Reply) -> Vec<String> {
    list.data()["appointments"]
        .as_array()
        .expect("rows")
        .iter()
        .filter_map(|row| row["id"].as_str().map(str::to_string))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_dispatch_serves_over_the_pg_model_store() {
    let Some(pool) = pg_pool().await else { return };
    let app = app_over_pg(kelson_postgres::PgModelStore::new(pool.clone()));

    // Tenant `dev` books through the graph route; the row lands in
    // PostgreSQL (the FK target was seeded above).
    let email_a = format!("pg-http-a-{}@clinic.test", uuid::Uuid::now_v7());
    let reg_a = register(&app, &email_a, "dev").await;
    assert_eq!(reg_a.status, StatusCode::OK, "register: {}", reg_a.body);
    let cookie_a = reg_a.session_cookie().expect("session cookie on register");
    let book = send(
        &app,
        json_req(
            Method::POST,
            "/api/v1/appointments",
            serde_json::json!({
                "patient": format!("Pg. Patient {}", uuid::Uuid::now_v7()),
                "slot": "2026-09-01T10:00Z",
            }),
            Some(&cookie_a),
        ),
    )
    .await;
    assert_eq!(book.status, StatusCode::OK, "book: {}", book.body);
    let id = book.data()["id"]
        .as_str()
        .expect("appointment id")
        .to_string();
    assert_eq!(book.data()["status"], "booked");

    // Round trip: the list route reads the very row back through PG.
    let list = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments",
            Value::Null,
            Some(&cookie_a),
        ),
    )
    .await;
    assert_eq!(list.status, StatusCode::OK, "list: {}", list.body);
    let ids = listed_ids(&list);
    assert!(
        ids.contains(&id),
        "the booked row survived the fabric: {ids:?}"
    );

    // Tenant isolation holds in the fabric: a principal of another tenant
    // neither lists nor reaches the row (the binding miss is a 404).
    let tenant_b = uuid::Uuid::now_v7().to_string();
    let reg_b = register(
        &app,
        &format!("pg-http-b-{}@clinic.test", uuid::Uuid::now_v7()),
        &tenant_b,
    )
    .await;
    assert_eq!(reg_b.status, StatusCode::OK, "register b: {}", reg_b.body);
    let cookie_b = reg_b.session_cookie().expect("session cookie on register");
    let list_b = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments",
            Value::Null,
            Some(&cookie_b),
        ),
    )
    .await;
    assert_eq!(list_b.status, StatusCode::OK, "list b: {}", list_b.body);
    assert_eq!(listed_ids(&list_b).len(), 0, "{list_b:?}");
    let cross = send(
        &app,
        json_req(
            Method::DELETE,
            &format!("/api/v1/appointments/{id}"),
            Value::Null,
            Some(&cookie_b),
        ),
    )
    .await;
    assert_eq!(
        cross.status,
        StatusCode::NOT_FOUND,
        "cross-tenant: {}",
        cross.body
    );

    // Cancel lands as the soft delete over PG; the double cancel is the
    // same conflict envelope the in-memory suite pins.
    let cancel = send(
        &app,
        json_req(
            Method::DELETE,
            &format!("/api/v1/appointments/{id}"),
            Value::Null,
            Some(&cookie_a),
        ),
    )
    .await;
    assert_eq!(cancel.status, StatusCode::OK, "cancel: {}", cancel.body);
    assert_eq!(cancel.data()["status"], "cancelled");
    let again = send(
        &app,
        json_req(
            Method::DELETE,
            &format!("/api/v1/appointments/{id}"),
            Value::Null,
            Some(&cookie_a),
        ),
    )
    .await;
    assert_eq!(
        again.status,
        StatusCode::CONFLICT,
        "double cancel: {}",
        again.body
    );
    assert_eq!(again.error_code(), "KSN_CONFLICT");

    // The veil holds over PG too: the cancelled row leaves the list.
    let after = send(
        &app,
        json_req(
            Method::GET,
            "/api/v1/appointments",
            Value::Null,
            Some(&cookie_a),
        ),
    )
    .await;
    assert_eq!(after.status, StatusCode::OK, "after: {}", after.body);
    assert!(
        !listed_ids(&after).contains(&id),
        "the cancelled row is veiled: {after:?}"
    );
}
