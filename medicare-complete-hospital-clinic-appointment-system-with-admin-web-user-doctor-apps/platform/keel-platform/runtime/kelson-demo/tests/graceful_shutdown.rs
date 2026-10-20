//! Graceful shutdown: a stop **drains** the in-flight requests to zero and
//! drops no connection mid-response.
//!
//! The backlog item was "a graceful-shutdown test in `kelson-demo` (stop
//! drains in-flight to zero, no dropped connections)" — a production-safety
//! property that had no evidence: the drain was wired (`axum::serve(...).
//! with_graceful_shutdown(...)`) but never exercised.
//!
//! The test drives a **raw socket** rather than a client library, because
//! the property under test is exactly what happens on the wire: a complete
//! response versus a connection dropped half-way through.
//!
//! The stop is triggered with a oneshot rather than a real signal (a test
//! cannot portably signal itself), so `serve_with_shutdown` is the seam both
//! the production boot and this test use — one implementation of the drain.

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// A minimal HTTP/1.1 GET over a raw socket: the assertion is about the wire.
async fn get(addr: std::net::SocketAddr, path: &str) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(addr).await?;
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).await?;
    Ok(String::from_utf8_lossy(&raw).to_string())
}

/// The stop drains the in-flight work: the slow request that was already
/// running when the stop arrived still gets its **full** response, the server
/// future resolves, and the listener is closed (a request after the stop is
/// refused, not queued).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_drains_in_flight_requests_and_drops_nothing() {
    let app = axum::Router::new().route(
        "/slow",
        axum::routing::get(|| async {
            tokio::time::sleep(Duration::from_millis(700)).await;
            "drained"
        }),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the listener binds");
    let addr = listener.local_addr().expect("the address");

    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        kelson_demo::serve_with_shutdown(app, listener, async {
            let _ = stop_rx.await;
        })
        .await
    });

    // Fire the slow request, then let it get IN FLIGHT before the stop.
    let request = tokio::spawn(async move { get(addr, "/slow").await });
    tokio::time::sleep(Duration::from_millis(150)).await;

    // The stop arrives while the request is still running.
    let stopped_at = std::time::Instant::now();
    stop_tx.send(()).expect("the stop fires");

    let response = request
        .await
        .expect("the request task joins")
        .expect("the request completes");
    assert!(
        response.starts_with("HTTP/1.1 200 OK"),
        "the in-flight request still gets a complete response, not a dropped connection: {response:?}"
    );
    assert!(
        response.ends_with("drained"),
        "the body is complete: {response:?}"
    );
    // The server WAITED for the in-flight request: the response lands well
    // after the stop, not before it. Without the drain the connection would
    // have been cut at the stop instead.
    assert!(
        stopped_at.elapsed() >= Duration::from_millis(400),
        "the drain waited for the in-flight request ({}ms after the stop)",
        stopped_at.elapsed().as_millis()
    );

    // The serve future resolves once the drain is done.
    let served = tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("the server stops within the deadline")
        .expect("the server task joins");
    assert!(served.is_ok(), "the serve ends cleanly: {served:?}");

    // The listener is closed: a request after the stop is REFUSED, not
    // accepted and left hanging.
    let after = get(addr, "/slow").await;
    assert!(
        after.is_err(),
        "the listener is closed after the drain — a late request is refused, not queued: {after:?}"
    );
}

/// A stop with **nothing in flight** returns immediately — the drain does not
/// wait for a timeout it does not need.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_with_nothing_in_flight_returns_at_once() {
    let app = axum::Router::new().route(
        "/fast",
        axum::routing::get(|| async { "ok" }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the listener binds");
    let addr = listener.local_addr().expect("the address");

    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        kelson_demo::serve_with_shutdown(app, listener, async {
            let _ = stop_rx.await;
        })
        .await
    });

    // The server answers a normal request first.
    let response = get(addr, "/fast").await.expect("the request completes");
    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response:?}");

    stop_tx.send(()).expect("the stop fires");
    let started = std::time::Instant::now();
    let served = tokio::time::timeout(Duration::from_secs(10), server)
        .await
        .expect("the server stops within the deadline")
        .expect("the server task joins");
    assert!(served.is_ok());
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "an idle stop does not wait out a drain timeout"
    );
}
