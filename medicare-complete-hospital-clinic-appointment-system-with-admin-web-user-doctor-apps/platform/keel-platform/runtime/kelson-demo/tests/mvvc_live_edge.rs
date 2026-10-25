//! MVA-008 — the runtime's first live edge, proven end to end.
//!
//! The audit found the live spine (`kelson_views::live`) and the wire
//! (`kelson_views::LiveWire`) shipped as framework primitives with ZERO
//! application consumers: no runtime reference, no mounted transport, and
//! `app.realtime` disabled — so channel authorization stayed theoretical.
//!
//! This suite closes that at the runtime level. The one rule the edge
//! exists to prove is the last test below: a mounted channel
//! **authorizes, publishes a diff, and the client frame preserves state**
//! — the subscriber applies `{path, after}` entries to its local copy and
//! ends equal to the server's checked state.
//!
//! Every case is refused honestly on the negative lanes: no session to open
//! a session, no owner to drive one, no token to open the stream, and no
//! remembered state after the session is closed.

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use serde_json::{json, Value};
use tokio_stream::StreamExt;
use tower::ServiceExt;

struct Fixture {
    state: kelson_demo::routes::AppState,
    graph: kelson_registry::PlatformGraph,
}

fn fixture() -> Fixture {
    let dir = std::env::temp_dir().join(format!("kelson-mvvc-live-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).expect("fixture dir");
    let (state, graph, _store) = kelson_demo::build_app_state(&dir, true, true);
    let engine = state.views.as_ref().expect("views configured");
    kelson_demo::install_command_registry(engine, &graph);
    Fixture { state, graph }
}

/// Build the router with the realtime flag ON — the transports mount only
/// when this boot declared `app.realtime` (dev/test rule; prod opt-in).
/// The flag must be pushed BEFORE the router is built: `routes::router`
/// reads the set at mount time.
fn router(fx: &Fixture) -> axum::Router {
    let state = fx.state.clone();
    state
        .flags
        .write()
        .expect("flags cell is not poisoned")
        .push(String::from("app.realtime"));
    let router = kelson_demo::routes::router(state.clone())
        .merge(kelson_demo::graph_routes::graph_router(state.clone(), &fx.graph))
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

async fn post(router: &axum::Router, uri: &str, cookie: Option<&str>) -> Reply {
    let mut builder = Request::builder().method(Method::POST).uri(uri);
    if let Some(cookie) = cookie {
        builder = builder.header(header::COOKIE, cookie);
    }
    send(router, builder.body(Body::empty()).expect("request builds")).await
}

async fn delete(router: &axum::Router, uri: &str, cookie: Option<&str>) -> Reply {
    let mut builder = Request::builder().method(Method::DELETE).uri(uri);
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

/// The `data` payload of an SSE frame, as a JSON value.
fn envelope(reply: &Reply) -> Value {
    serde_json::from_str(&reply.body).unwrap_or_else(|e| panic!("{e}: {}", reply.body))
}

fn data(reply: &Reply) -> Value {
    let value = envelope(reply);
    assert_eq!(value["ok"], true, "{}", reply.body);
    value["data"].clone()
}

/// Apply a frame's `changes` to a local copy of the state — exactly what
/// a browser does. This is the frame-preservation half under test.
fn apply_changes(mut state: Value, changes: &[Value]) -> Value {
    for change in changes {
        let path = change["path"]
            .as_str()
            .unwrap_or_else(|| panic!("a change carries a path: {change}"));
        let after = change["after"].clone();
        let parts: Vec<&str> = path.split('.').collect();
        let mut cursor = &mut state;
        for (index, part) in parts.iter().enumerate() {
            if index + 1 == parts.len() {
                cursor[part] = after.clone();
            } else {
                cursor = &mut cursor[part];
            }
        }
    }
    state
}

// ---------------------------------------------------------------- the shell

#[tokio::test]
async fn the_live_shell_serves_its_bound_state() {
    let fx = fixture();
    let router = router(&fx);

    let reply = get(&router, "/mvc/live", None).await;

    assert_eq!(reply.status, StatusCode::OK, "{}", reply.body);
    assert!(
        reply.content_type.starts_with("text/html"),
        "{}",
        reply.content_type
    );
    // The shell renders through the SAME schema-checked render a live
    // re-render uses, and the binding's state is what the page shows.
    assert!(
        reply.body.contains(r#"<p class="waiting">0</p>"#),
        "the bound waiting state renders: {}",
        reply.body
    );
    assert!(
        reply
            .body
            .contains("<title>Waiting room · Kelson</title>"),
        "the live page titles itself: {}",
        reply.body
    );
    assert!(
        !reply.body.contains("{{"),
        "the template renders, it is not echoed: {}",
        reply.body
    );
}

// ------------------------------------------------------------ the ownership

#[tokio::test]
async fn opening_a_live_session_requires_a_session() {
    let fx = fixture();
    let router = router(&fx);

    let reply = post(&router, "/api/v1/live/sessions", None).await;

    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{}", reply.body);
    assert!(reply.body.contains(r#""ok":false"#), "{}", reply.body);
}

#[tokio::test]
async fn the_owner_opens_ticks_and_closes_a_session() {
    let fx = fixture();
    let router = router(&fx);
    let cookie = register(&router, "live-owner@example.test").await;

    let created = post(&router, "/api/v1/live/sessions", Some(&cookie)).await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.body);
    let created_data = data(&created);
    let id = created_data["id"].as_str().expect("a handle").to_string();
    assert!(
        created_data["channel"].as_str().is_some_and(|c| c.starts_with("global:")),
        "the session names its channel: {}",
        created_data
    );
    assert_eq!(created_data["revision"], 0, "{}", created_data);
    assert_eq!(created_data["state"]["waiting"], 0, "{}", created_data);

    // The owner's `change`: the state advances, the revision stamps it,
    // and the diff that travelled comes back with it.
    let ticked = post(&router, &format!("/api/v1/live/sessions/{id}/tick"), Some(&cookie)).await;
    assert_eq!(ticked.status, StatusCode::OK, "{}", ticked.body);
    let ticked_data = data(&ticked);
    assert_eq!(ticked_data["revision"], 1, "{}", ticked_data);
    assert_eq!(ticked_data["state"]["waiting"], 1, "{}", ticked_data);
    assert_eq!(
        ticked_data["changes"],
        json!([{ "path": "waiting", "after": 1 }]),
        "only the changed leaf travels: {}",
        ticked_data
    );

    // Closing retires the session: every later read is a 404 (stale),
    // never a remembered state.
    let closed = delete(&router, &format!("/api/v1/live/sessions/{id}"), Some(&cookie)).await;
    assert_eq!(closed.status, StatusCode::OK, "{}", closed.body);
    assert_eq!(data(&closed)["closed"], true, "{}", closed.body);

    let stale = post(&router, &format!("/api/v1/live/sessions/{id}/tick"), Some(&cookie)).await;
    assert_eq!(stale.status, StatusCode::NOT_FOUND, "{}", stale.body);
    assert!(stale.body.contains(r#""ok":false"#), "{}", stale.body);
}

#[tokio::test]
async fn a_non_owner_may_not_drive_the_session() {
    let fx = fixture();
    let router = router(&fx);
    let owner = register(&router, "live-owner@example.test").await;
    let stranger = register(&router, "live-stranger@example.test").await;

    let created = post(&router, "/api/v1/live/sessions", Some(&owner)).await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.body);
    let id = data(&created)["id"].as_str().expect("a handle").to_string();

    // The id is an opaque handle, not a credential: another principal may
    // neither drive nor subscribe it.
    let ticked = post(&router, &format!("/api/v1/live/sessions/{id}/tick"), Some(&stranger)).await;
    assert_eq!(ticked.status, StatusCode::FORBIDDEN, "{}", ticked.body);

    let subscribed = post(
        &router,
        &format!("/api/v1/live/sessions/{id}/subscribe"),
        Some(&stranger),
    )
    .await;
    assert_eq!(subscribed.status, StatusCode::FORBIDDEN, "{}", subscribed.body);
}

// ---------------------------------------------------------- the transport

#[tokio::test]
async fn the_stream_refuses_a_connect_without_a_channel_token() {
    let fx = fixture();
    let router = router(&fx);
    let cookie = register(&router, "live-stream@example.test").await;

    let created = post(&router, "/api/v1/live/sessions", Some(&cookie)).await;
    let created_data = data(&created);
    let channel = created_data["channel"].as_str().expect("a channel").to_string();

    let uri = format!("/realtime/sse?channel={}", urlencode(&channel));
    let reply = get(&router, &uri, None).await;

    // The transport is mounted (the flag was on) and it FAILS CLOSED: a
    // connect with no credential is refused by the host authorizer.
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{}", reply.body);
    assert!(reply.body.contains(r#""ok":false"#), "{}", reply.body);
}

#[tokio::test]
async fn a_channel_token_grants_exactly_its_channel() {
    let fx = fixture();
    let router = router(&fx);
    let cookie = register(&router, "live-bind@example.test").await;

    let created = post(&router, "/api/v1/live/sessions", Some(&cookie)).await;
    let created_data = data(&created);
    let id = created_data["id"].as_str().expect("a handle").to_string();
    let channel = created_data["channel"].as_str().expect("a channel").to_string();

    let subscribed = post(
        &router,
        &format!("/api/v1/live/sessions/{id}/subscribe"),
        Some(&cookie),
    )
    .await;
    assert_eq!(subscribed.status, StatusCode::OK, "{}", subscribed.body);
    let issued = data(&subscribed);
    let token = issued["token"].as_str().expect("a token").to_string();

    // The token is bound to ITS channel: presenting it against a channel
    // it was never issued for is the same uniform refusal as no token.
    let foreign = format!("/realtime/sse?channel={}&token={}", urlencode("global:elsewhere"), urlencode(&token));
    let reply = get(&router, &foreign, None).await;
    assert_eq!(reply.status, StatusCode::UNAUTHORIZED, "{}", reply.body);

    // And the right channel admits with the same credential.
    let right = format!(
        "/realtime/sse?channel={}&token={}",
        urlencode(&channel),
        urlencode(&token)
    );
    let mut builder = Request::builder().method(Method::GET).uri(&right);
    builder = builder.header(header::ACCEPT, "text/event-stream");
    let res = router
        .clone()
        .oneshot(builder.body(Body::empty()).expect("request builds"))
        .await
        .expect("request serves");
    assert_eq!(res.status(), StatusCode::OK);
    // Drop the open stream immediately (no publish yet in this case).
    drop(res);
}

/// THE rule this edge exists for: a mounted channel authorizes, publishes
/// a diff, and the client frame **preserves state** — the subscriber
/// applies the frame's `{path, after}` entries to its local copy and ends
/// equal to the server's checked state.
#[tokio::test]
async fn a_mounted_channel_authorizes_publishes_and_the_frame_preserves_state() {
    let fx = fixture();
    let router = router(&fx);
    let cookie = register(&router, "live-frame@example.test").await;

    let created = post(&router, "/api/v1/live/sessions", Some(&cookie)).await;
    assert_eq!(created.status, StatusCode::OK, "{}", created.body);
    let created_data = data(&created);
    let id = created_data["id"].as_str().expect("a handle").to_string();
    let channel = created_data["channel"].as_str().expect("a channel").to_string();

    let subscribed = post(
        &router,
        &format!("/api/v1/live/sessions/{id}/subscribe"),
        Some(&cookie),
    )
    .await;
    assert_eq!(subscribed.status, StatusCode::OK, "{}", subscribed.body);
    let token = data(&subscribed)["token"]
        .as_str()
        .expect("a token")
        .to_string();

    // Open the stream with the issued credential. The subscription joins
    // the hub inside the handler, so it is live by the time this returns.
    let uri = format!(
        "/realtime/sse?channel={}&token={}",
        urlencode(&channel),
        urlencode(&token)
    );
    let mut builder = Request::builder().method(Method::GET).uri(&uri);
    builder = builder.header(header::ACCEPT, "text/event-stream");
    let res = router
        .clone()
        .oneshot(builder.body(Body::empty()).expect("request builds"))
        .await
        .expect("request serves");
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "the mounted channel admits the authorized subscriber"
    );
    let mut stream = Box::pin(res.into_body().into_data_stream());

    // The channel is subscribed and SILENT: no frame before anything is
    // published (what an idle live page looks like).
    let idle = tokio::time::timeout(std::time::Duration::from_millis(250), stream.next()).await;
    assert!(idle.is_err(), "nothing publishes before the tick: {idle:?}");

    // The owner drives the session: state advances, diff publishes.
    let ticked = post(&router, &format!("/api/v1/live/sessions/{id}/tick"), Some(&cookie)).await;
    assert_eq!(ticked.status, StatusCode::OK, "{}", ticked.body);
    let server_state = data(&ticked)["state"].clone();
    assert_eq!(server_state, json!({ "waiting": 1 }), "{}", ticked.body);

    // The frame arrives on the mounted channel.
    let mut bytes: Vec<u8> = Vec::new();
    let frame = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !std::str::from_utf8(&bytes)
            .map(|text| text.contains("event: live.update"))
            .unwrap_or(false)
        {
            match stream.next().await {
                Some(Ok(chunk)) => bytes.extend_from_slice(&chunk),
                Some(Err(error)) => panic!("the stream errors: {error}"),
                None => panic!("the stream ended before the frame: {}", String::from_utf8_lossy(&bytes)),
            }
        }
    })
    .await;
    frame.expect("the published diff reaches the subscriber");

    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("event: live.update"),
        "the live event rides the frame: {text}"
    );
    let data_line = text
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .unwrap_or_else(|| panic!("a data line: {text}"));
    let event: Value =
        serde_json::from_str(data_line).unwrap_or_else(|e| panic!("{e}: {data_line}"));
    assert_eq!(event["event"], "live.update", "{}", event);
    assert_eq!(event["channel"], json!(channel), "{}", event);

    let payload = &event["payload"];
    assert_eq!(payload["session"], json!(id), "{}", payload);
    assert_eq!(payload["revision"], 1, "{}", payload);
    let changes = payload["changes"]
        .as_array()
        .unwrap_or_else(|| panic!("the frame carries changes: {payload}"));

    // FRAME PRESERVATION: applying the frame to an empty local copy lands
    // exactly on the server's checked state.
    let applied = apply_changes(json!({}), changes);
    assert_eq!(
        applied, server_state,
        "the subscriber's applied frame equals the server's state"
    );
    assert_eq!(applied["waiting"], 1, "{}", applied);
}
