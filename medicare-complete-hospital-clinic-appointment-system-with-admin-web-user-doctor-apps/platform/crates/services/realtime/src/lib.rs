//! # keel-realtime
//! Broadcasting + realtime transports (blueprint L42/L43, S3). The hub is
//! the contract: application code publishes `RealtimeEvent`s to named
//! channels; transports (WebSocket / SSE) are interchangeable subscribers.
//! Single-node today (in-process broadcast channels); a redis adapter later
//! implements the same hub surface for multi-node.
//!
//! Channel naming is a convention enforced by helpers, not free strings:
//! `tenant:{tenant_id}:{topic}` for tenant traffic, `global:{topic}` for
//! platform broadcasts. Authorization of who may subscribe is the runtime's
//! job (it decides which channels to expose on which routes).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use keel_foundation::{AppError, TenantId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tokio::sync::broadcast;
use tokio_stream::StreamExt;

/// Render the public error envelope without depending on keel-http (Web
/// class is above Services in the layer matrix). Shape must stay in sync
/// with keel_http::Envelope/ErrorBody.
fn error_response(err: &AppError, status: StatusCode) -> Response {
    let body = serde_json::json!({
        "ok": false,
        "error": {
            "code": err.code,
            "message": err.public_message,
            "retryable": err.retryable == keel_foundation::Retryability::Retryable,
        },
    });
    (status, axum::Json(body)).into_response()
}

/// Channel fan-out capacity before slow consumers start dropping (lagged).
pub const CHANNEL_CAPACITY: usize = 256;

/// One broadcast event. Payload must be wire-safe JSON (no PHI by policy).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealtimeEvent {
    pub channel: String,
    /// Event name inside the channel (`appointment.updated`, ...).
    pub event: String,
    pub payload: serde_json::Value,
    #[serde(with = "time::serde::iso8601")]
    pub at: OffsetDateTime,
}

/// Canonical tenant channel name.
pub fn tenant_channel(tenant: TenantId, topic: &str) -> String {
    format!("tenant:{tenant}:{topic}")
}

/// Canonical platform-wide channel name.
pub fn global_channel(topic: &str) -> String {
    format!("global:{topic}")
}

/// In-process broadcast hub. Clone-cheap (everything Arc-shared).
#[derive(Clone, Default)]
pub struct BroadcastHub {
    channels: Arc<Mutex<HashMap<String, broadcast::Sender<RealtimeEvent>>>>,
}

impl BroadcastHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Subscribe to a channel (created lazily on first interest).
    pub fn subscribe(&self, channel: &str) -> broadcast::Receiver<RealtimeEvent> {
        self.sender(channel).subscribe()
    }

    /// Publish to a channel; returns how many live receivers got it.
    pub fn publish(&self, event: RealtimeEvent) -> usize {
        self.sender(&event.channel).send(event).unwrap_or_default()
    }

    /// Publish helper with the clock injected.
    pub fn publish_value(
        &self,
        channel: &str,
        event: &str,
        payload: serde_json::Value,
        now: OffsetDateTime,
    ) -> usize {
        self.publish(RealtimeEvent {
            channel: channel.to_string(),
            event: event.to_string(),
            payload,
            at: now,
        })
    }

    pub fn channel_count(&self) -> usize {
        self.channels.lock().unwrap().len()
    }

    fn sender(&self, channel: &str) -> broadcast::Sender<RealtimeEvent> {
        let mut map = self.channels.lock().unwrap();
        map.entry(channel.to_string())
            .or_insert_with(|| broadcast::channel(CHANNEL_CAPACITY).0)
            .clone()
    }
}

#[derive(Debug, Deserialize)]
pub struct ChannelQuery {
    #[serde(default)]
    pub channel: String,
}

/// Channels are `[a-zA-Z0-9:_-]{1,200}` — no path/HTTP smuggling surface.
pub fn sanitize_channel(raw: &str) -> Option<String> {
    let t = raw.trim();
    if t.is_empty() || t.len() > 200 {
        return None;
    }
    if t.chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-'))
    {
        Some(t.to_string())
    } else {
        None
    }
}

// ------------------------------------------------------------ SSE transport

/// SSE endpoint handler: `GET /realtime/sse?channel=...`. Lagged frames are
/// dropped (latest-wins semantics for realtime feeds).
pub async fn sse_handler(
    State(hub): State<BroadcastHub>,
    Query(query): Query<ChannelQuery>,
) -> Response {
    let Some(channel) = sanitize_channel(&query.channel) else {
        return error_response(
            &AppError::validation("missing or malformed channel"),
            StatusCode::BAD_REQUEST,
        );
    };
    let stream = tokio_stream::wrappers::BroadcastStream::new(hub.subscribe(&channel))
        .filter_map(|result| result.ok())
        .map(|event| {
            let name = event.event.clone();
            Ok::<_, std::convert::Infallible>(
                Event::default()
                    .event(name)
                    .json_data(event)
                    .expect("event serializes"),
            )
        });
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

// ------------------------------------------------------------- WS transport

/// WebSocket endpoint handler: `GET /realtime/ws?channel=...`.
pub async fn ws_handler(
    State(hub): State<BroadcastHub>,
    Query(query): Query<ChannelQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(channel) = sanitize_channel(&query.channel) else {
        return error_response(
            &AppError::validation("missing or malformed channel"),
            StatusCode::BAD_REQUEST,
        );
    };
    ws.on_upgrade(move |socket| ws_session(hub, channel, socket))
}

async fn ws_session(hub: BroadcastHub, channel: String, mut socket: WebSocket) {
    let mut receiver = hub.subscribe(&channel);
    loop {
        tokio::select! {
            incoming = receiver.recv() => match incoming {
                Ok(event) => {
                    let Ok(wire) = serde_json::to_string(&event) else { continue };
                    if socket.send(Message::Text(wire.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => break,
            },
            frame = socket.recv() => match frame {
                None => break,
                Some(Err(_)) => break,
                Some(Ok(Message::Close(_))) => break,
                Some(Ok(_)) => {}
            },
        }
    }
}

/// Wire both transports under one router prefix (runtime composes auth).
pub fn realtime_router(hub: BroadcastHub) -> axum::Router {
    axum::Router::new()
        .route("/realtime/sse", axum::routing::get(sse_handler))
        .route("/realtime/ws", axum::routing::get(ws_handler))
        .with_state(hub)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_naming_helpers() {
        let tenant = TenantId::new();
        assert_eq!(
            tenant_channel(tenant, "appointments"),
            format!("tenant:{tenant}:appointments")
        );
        assert_eq!(global_channel("maintenance"), "global:maintenance");
    }

    #[tokio::test]
    async fn hub_publish_reaches_subscribers_only_on_that_channel() {
        let hub = BroadcastHub::new();
        let mut rx_a = hub.subscribe("tenant:t1:appointments");
        let mut rx_b = hub.subscribe("global:maintenance");

        let delivered = hub.publish_value(
            "tenant:t1:appointments",
            "created",
            serde_json::json!({"id": 1}),
            OffsetDateTime::now_utc(),
        );
        assert_eq!(delivered, 1);
        let event = rx_a.recv().await.unwrap();
        assert_eq!(event.event, "created");
        assert!(rx_b.try_recv().is_err(), "other channels stay silent");
    }

    #[tokio::test]
    async fn publish_with_no_subscribers_is_zero() {
        let hub = BroadcastHub::new();
        let delivered = hub.publish_value(
            "global:x",
            "ping",
            serde_json::json!(null),
            OffsetDateTime::now_utc(),
        );
        assert_eq!(delivered, 0);
    }

    #[test]
    fn channel_sanitizer_rules() {
        assert!(sanitize_channel("tenant:abc:appointments").is_some());
        assert!(sanitize_channel("global_maintenance").is_some());
        assert!(sanitize_channel("").is_none());
        assert!(sanitize_channel("bad channel").is_none());
        assert!(sanitize_channel("a/b").is_none());
        assert!(sanitize_channel(&"x".repeat(201)).is_none());
    }

    #[tokio::test]
    async fn sse_endpoint_rejects_bad_channel_and_serves_stream() {
        let hub = BroadcastHub::new();
        let app = realtime_router(hub.clone());

        let res = tower::ServiceExt::oneshot(
            app.clone(),
            axum::http::Request::builder()
                .uri("/realtime/sse?channel=")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(res.status(), 400);

        let res = tower::ServiceExt::oneshot(
            app,
            axum::http::Request::builder()
                .uri("/realtime/sse?channel=global:ops")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(res.status(), 200);
        let ct = res.headers().get("content-type").unwrap().to_str().unwrap();
        assert!(ct.starts_with("text/event-stream"), "got {ct}");

        let mut rx = hub.subscribe("global:ops");
        hub.publish_value(
            "global:ops",
            "deploy",
            serde_json::json!({"ok": true}),
            OffsetDateTime::now_utc(),
        );
        assert_eq!(rx.recv().await.unwrap().event, "deploy");
    }

    #[tokio::test]
    async fn ws_route_without_upgrade_handshake_is_rejected() {
        let app = realtime_router(BroadcastHub::new());
        let res = tower::ServiceExt::oneshot(
            app,
            axum::http::Request::builder()
                .uri("/realtime/ws?channel=global:ops")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(
            res.status(),
            400,
            "plain GET without websocket handshake cannot upgrade"
        );
    }

    #[tokio::test]
    async fn ws_roundtrip_over_real_socket() {
        let hub = BroadcastHub::new();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_hub = hub.clone();
        tokio::spawn(async move {
            axum::serve(listener, realtime_router(server_hub))
                .await
                .unwrap();
        });

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let req = format!(
            "GET /realtime/ws?channel=global:ops HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(req.as_bytes()).await.unwrap();

        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        let handshake = String::from_utf8_lossy(&buf[..n]);
        assert!(
            handshake.starts_with("HTTP/1.1 101"),
            "handshake was: {handshake}"
        );

        // Give the upgraded session a beat to subscribe before publishing.
        tokio::time::sleep(Duration::from_millis(100)).await;
        hub.publish_value(
            "global:ops",
            "deploy",
            serde_json::json!({"ok": 1}),
            OffsetDateTime::now_utc(),
        );

        let mut frame = vec![0u8; 4096];
        let n = stream.read(&mut frame).await.unwrap();
        assert!(n > 2);
        assert_eq!(frame[0] & 0x0f, 0x1, "text frame opcode");
        let len = (frame[1] & 0x7f) as usize;
        assert!(len <= 125, "small payload fits one frame");
        let payload = String::from_utf8_lossy(&frame[2..2 + len]);
        assert!(payload.contains("deploy"), "frame payload: {payload}");
    }
}
