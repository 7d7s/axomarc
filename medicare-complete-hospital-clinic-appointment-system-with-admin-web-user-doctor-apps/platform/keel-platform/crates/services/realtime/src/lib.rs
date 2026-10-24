//! # kelson-realtime
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
use kelson_foundation::{AppError, TenantId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use tokio::sync::broadcast;
use tokio_stream::StreamExt;

/// Render the public error envelope without depending on kelson-http (Web
/// class is above Services in the layer matrix). Shape must stay in sync
/// with kelson_http::Envelope/ErrorBody.
fn error_response(err: &AppError, status: StatusCode) -> Response {
    let body = serde_json::json!({
        "ok": false,
        "error": {
            "code": err.code,
            "message": err.public_message,
            "retryable": err.retryable == kelson_foundation::Retryability::Retryable,
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

/// Identity of one subscriber on a `presence-*` channel (runtime-supplied:
/// the host derives it from the validated channel token). The roster stores
/// exactly this — no credentials ever reach the hub.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PresenceMember {
    pub id: String,
    pub display_name: String,
}

/// Wave 6 P7 presence roster (19-PARITY "Presence channels"): which
/// authenticated members sit on a `presence-<name>` channel right now.
/// Join/leave are transport-agnostic; `presence.join`/`presence.leave`
/// events broadcast through the SAME hub subscribers listen on. One instance
/// per boot is shared between the subscription path and the members
/// endpoint (`GET /api/v1/broadcast/members`).
#[derive(Clone, Default)]
pub struct PresenceHub {
    members: Arc<Mutex<HashMap<String, Vec<PresenceMember>>>>,
}

impl PresenceHub {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a member (idempotent per id within a channel). Returns
    /// `true` when the roster actually changed — the caller then broadcasts
    /// `presence.join`.
    pub fn join(&self, channel: &str, member: PresenceMember) -> bool {
        let mut map = self.members.lock().unwrap();
        let roster = map.entry(channel.to_string()).or_default();
        if roster.iter().any(|m| m.id == member.id) {
            return false;
        }
        roster.push(member);
        true
    }

    /// Remove a member; returns `true` when it was present — the caller
    /// then broadcasts `presence.leave`.
    pub fn leave(&self, channel: &str, member_id: &str) -> bool {
        let mut map = self.members.lock().unwrap();
        let Some(roster) = map.get_mut(channel) else {
            return false;
        };
        let before = roster.len();
        roster.retain(|m| m.id != member_id);
        let removed = roster.len() != before;
        if roster.is_empty() {
            map.remove(channel);
        }
        removed
    }

    /// Current roster (copy — the caller serializes the wire shape).
    pub fn members(&self, channel: &str) -> Vec<PresenceMember> {
        self.members
            .lock()
            .unwrap()
            .get(channel)
            .cloned()
            .unwrap_or_default()
    }

    pub fn member_count(&self, channel: &str) -> usize {
        self.members
            .lock()
            .unwrap()
            .get(channel)
            .map(Vec::len)
            .unwrap_or(0)
    }
}

/// Wave 6 P7: host-side per-connection authorization (19-PARITY
/// "Subscription authorization runs host-side per connection; a channel
/// token never grants more than its channel"). The runtime validates the
/// presented token against the requested channel and returns the
/// subscriber's identity for presence bookkeeping; any error refuses the
/// connect.
pub trait ChannelAuthorizer: Send + Sync {
    fn authorize(&self, channel: &str, token: Option<&str>) -> Result<PresenceMember, AppError>;
}

/// Shared transport state: the hub events publish through, the presence
/// roster, and — when the runtime composed it — the host-side authorizer.
/// `authorizer = None` is the raw transport primitive (the crate's own
/// tests exercise it); `serve()` always mounts
/// [`realtime_router_authorized`].
#[derive(Clone)]
pub struct RealtimeState {
    pub hub: BroadcastHub,
    pub presence: Arc<PresenceHub>,
    pub authorizer: Option<Arc<dyn ChannelAuthorizer>>,
}

/// Admit one connection: sanitize the channel FIRST (malformed names stay a
/// plain validation 400), then run the host authorizer when one is wired.
/// Returns the channel plus the subscriber identity (`None` = raw mode, or
/// a non-presence channel keeps the identity for the join decision).
fn transport_admit(
    state: &RealtimeState,
    query: &ChannelQuery,
) -> Result<(String, Option<PresenceMember>), AppError> {
    let channel = sanitize_channel(&query.channel)
        .ok_or_else(|| AppError::validation("missing or malformed channel"))?;
    let member = match &state.authorizer {
        Some(authorizer) => Some(authorizer.authorize(&channel, query.token.as_deref())?),
        None => None,
    };
    Ok((channel, member))
}

/// Transport status for an admission error: malformed channel = 400,
/// credential failures (missing/invalid/expired/mismatched token) = 401,
/// any other authorizer denial = 403.
fn admission_status(err: &AppError) -> StatusCode {
    match err.code.as_str() {
        kelson_foundation::codes::VALIDATION => StatusCode::BAD_REQUEST,
        kelson_foundation::codes::UNAUTHENTICATED => StatusCode::UNAUTHORIZED,
        _ => StatusCode::FORBIDDEN,
    }
}

/// Should this channel carry presence bookkeeping? (`presence-<name>`.)
fn is_presence_channel(channel: &str) -> bool {
    channel.starts_with("presence-")
}

/// Join a presence channel: register the member and broadcast
/// `presence.join` to everyone already on the channel. Returns the roster
/// change so callers can mirror the leave path.
fn presence_join(state: &RealtimeState, channel: &str, member: &PresenceMember) {
    if is_presence_channel(channel) && state.presence.join(channel, member.clone()) {
        state.hub.publish_value(
            channel,
            "presence.join",
            serde_json::json!({ "member": member }),
            OffsetDateTime::now_utc(),
        );
    }
}

/// Leave a presence channel (disconnect path): unregister + broadcast
/// `presence.leave`. Idempotent — a no-op when the member is gone.
fn presence_leave(state: &RealtimeState, channel: &str, member_id: &str, display_name: &str) {
    if is_presence_channel(channel) && state.presence.leave(channel, member_id) {
        state.hub.publish_value(
            channel,
            "presence.leave",
            serde_json::json!({ "member": { "id": member_id, "display_name": display_name } }),
            OffsetDateTime::now_utc(),
        );
    }
}

#[derive(Debug, Deserialize)]
pub struct ChannelQuery {
    #[serde(default)]
    pub channel: String,
    /// Wave 6 P7: channel token presented at connect (validated by the
    /// host-side authorizer when the runtime composed one).
    #[serde(default)]
    pub token: Option<String>,
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

/// SSE endpoint handler: `GET /realtime/sse?channel=...&token=...`.
/// Lagged frames are dropped (latest-wins semantics for realtime feeds).
/// Presence channels join before the stream is handed over and leave when
/// the response (and with it the stream) is dropped.
pub async fn sse_handler(
    State(state): State<RealtimeState>,
    Query(query): Query<ChannelQuery>,
) -> Response {
    let (channel, member) = match transport_admit(&state, &query) {
        Ok(admitted) => admitted,
        Err(err) => return error_response(&err, admission_status(&err)),
    };
    let joined = member
        .as_ref()
        .filter(|_| is_presence_channel(&channel))
        .cloned();
    if let Some(member) = member.as_ref() {
        presence_join(&state, &channel, member);
    }
    let stream = tokio_stream::wrappers::BroadcastStream::new(state.hub.subscribe(&channel))
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
    // A presence member's stream carries the leave-on-drop scope; a plain
    // subscriber streams as before.
    let stream: Box<
        dyn tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>> + Send + Unpin,
    > = match joined {
        Some(member) => Box::new(PresenceGuard {
            state,
            channel,
            member_id: member.id,
            display_name: member.display_name,
            inner: stream,
        }),
        None => Box::new(stream),
    };
    Sse::new(stream)
        .keep_alive(
            KeepAlive::new()
                .interval(Duration::from_secs(15))
                .text("keep-alive"),
        )
        .into_response()
}

/// Stream wrapper that leaves the presence roster when the SSE body is
/// dropped (client disconnect). Requires an `Unpin` inner stream — the
/// `BroadcastStream → filter_map → map` chain is.
struct PresenceGuard<S> {
    state: RealtimeState,
    channel: String,
    member_id: String,
    display_name: String,
    inner: S,
}

impl<S: tokio_stream::Stream + Unpin> tokio_stream::Stream for PresenceGuard<S> {
    type Item = S::Item;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        std::pin::Pin::new(&mut self.inner).poll_next(cx)
    }
}

impl<S> Drop for PresenceGuard<S> {
    fn drop(&mut self) {
        presence_leave(
            &self.state,
            &self.channel,
            &self.member_id,
            &self.display_name,
        );
    }
}

// ------------------------------------------------------------- WS transport

/// WebSocket endpoint handler: `GET /realtime/ws?channel=...&token=...`.
/// The same host-side admission as SSE; presence membership is scoped to
/// the upgraded session's lifetime.
pub async fn ws_handler(
    State(state): State<RealtimeState>,
    Query(query): Query<ChannelQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let (channel, member) = match transport_admit(&state, &query) {
        Ok(admitted) => admitted,
        Err(err) => return error_response(&err, admission_status(&err)),
    };
    let joined = member
        .as_ref()
        .filter(|_| is_presence_channel(&channel))
        .cloned();
    ws.on_upgrade(move |socket| async move {
        if let Some(member) = member.as_ref() {
            presence_join(&state, &channel, member);
        }
        ws_session(state.hub.clone(), channel.clone(), socket).await;
        if let Some(member) = joined.as_ref() {
            presence_leave(&state, &channel, &member.id, &member.display_name);
        }
    })
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

/// Wire both transports under one router prefix — the RAW transport
/// primitive with no host-side authorizer (the crate's own tests exercise
/// it; `serve()` mounts [`realtime_router_authorized`] instead).
pub fn realtime_router(hub: BroadcastHub) -> axum::Router {
    realtime_router_with_state(RealtimeState {
        hub,
        presence: Arc::new(PresenceHub::new()),
        authorizer: None,
    })
}

/// Wave 6 P7: authorized transport — every connect runs the host-side
/// authorizer (channel-token validation), and `presence-*` channels join/
/// leave the shared roster with `presence.join`/`presence.leave` events.
pub fn realtime_router_authorized(
    hub: BroadcastHub,
    presence: Arc<PresenceHub>,
    authorizer: Arc<dyn ChannelAuthorizer>,
) -> axum::Router {
    realtime_router_with_state(RealtimeState {
        hub,
        presence,
        authorizer: Some(authorizer),
    })
}

fn realtime_router_with_state(state: RealtimeState) -> axum::Router {
    axum::Router::new()
        .route("/realtime/sse", axum::routing::get(sse_handler))
        .route("/realtime/ws", axum::routing::get(ws_handler))
        .with_state(state)
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

    /// Wave 6 P7: the roster is idempotent per member id, drops empty
    /// channels, and never leaks anything but the runtime-supplied identity.
    #[test]
    fn presence_roster_join_leave_is_idempotent() {
        let presence = PresenceHub::new();
        let member = PresenceMember {
            id: "p1".into(),
            display_name: "ada@example.com".into(),
        };
        assert!(presence.join("presence-lobby", member.clone()));
        assert!(
            !presence.join("presence-lobby", member.clone()),
            "a duplicate join changes nothing"
        );
        assert_eq!(presence.member_count("presence-lobby"), 1);
        assert_eq!(presence.members("presence-lobby"), vec![member.clone()]);
        assert!(presence.leave("presence-lobby", "p1"));
        assert!(
            !presence.leave("presence-lobby", "p1"),
            "leave is idempotent"
        );
        assert_eq!(presence.member_count("presence-lobby"), 0);
        assert!(presence.members("presence-lobby").is_empty());
        assert!(presence.members("presence-elsewhere").is_empty());
    }

    /// Wave 6 P7: the authorized transport refuses connects without the
    /// host authorizer's blessing (401), joins a presence roster on connect
    /// (broadcasting `presence.join`), and leaves it when the response —
    /// and with it the stream — is dropped (broadcasting `presence.leave`).
    #[tokio::test]
    async fn authorized_transport_enforces_the_token_and_tracks_presence() {
        struct StubAuthorizer;
        impl ChannelAuthorizer for StubAuthorizer {
            fn authorize(
                &self,
                _channel: &str,
                token: Option<&str>,
            ) -> Result<PresenceMember, AppError> {
                match token {
                    Some("good") => Ok(PresenceMember {
                        id: "p1".into(),
                        display_name: "ada@example.com".into(),
                    }),
                    _ => Err(AppError::unauthenticated()),
                }
            }
        }

        let hub = BroadcastHub::new();
        let presence = Arc::new(PresenceHub::new());
        let app =
            realtime_router_authorized(hub.clone(), presence.clone(), Arc::new(StubAuthorizer));

        // No token and a wrong token both refuse the connect.
        for uri in [
            "/realtime/sse?channel=presence-lobby",
            "/realtime/sse?channel=presence-lobby&token=evil",
        ] {
            let res = tower::ServiceExt::oneshot(
                app.clone(),
                axum::http::Request::builder()
                    .uri(uri)
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
            assert_eq!(res.status(), 401, "{uri}");
        }
        assert_eq!(
            presence.member_count("presence-lobby"),
            0,
            "refused connects never join"
        );

        // A valid token joins the roster and broadcasts the join.
        let mut rx = hub.subscribe("presence-lobby");
        let res = tower::ServiceExt::oneshot(
            app.clone(),
            axum::http::Request::builder()
                .uri("/realtime/sse?channel=presence-lobby&token=good")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(res.status(), 200);
        let ct = res.headers().get("content-type").unwrap().to_str().unwrap();
        assert!(ct.starts_with("text/event-stream"), "got {ct}");
        assert_eq!(presence.member_count("presence-lobby"), 1);
        let join = rx.try_recv().expect("presence.join broadcast");
        assert_eq!(join.event, "presence.join");
        assert_eq!(join.payload["member"]["id"], "p1");

        // Dropping the response (client disconnect) leaves the roster.
        drop(res);
        assert_eq!(presence.member_count("presence-lobby"), 0);
        let leave = rx.try_recv().expect("presence.leave broadcast");
        assert_eq!(leave.event, "presence.leave");
        assert_eq!(leave.payload["member"]["id"], "p1");
    }
}
