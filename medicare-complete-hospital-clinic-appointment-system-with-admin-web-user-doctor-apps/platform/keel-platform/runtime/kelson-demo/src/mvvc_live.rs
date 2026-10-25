//! MVA-008 — the runtime's first live edge: a session, a channel, and
//! the authorized transport that carries its diffs to a browser.
//!
//! The spine (`kelson_views::live`) and the wire
//! (`kelson_views::LiveWire`, feature `live-transport`) shipped as
//! framework primitives with zero application consumers. This module
//! hosts the first one at the runtime level, end to end:
//!
//! * `POST /api/v1/live/sessions` opens a session on the LIVE-declared
//!   view (`pages.mvc-live`, `ViewBinding::live()` — opt-in per view),
//!   owned by the caller's principal.
//! * `POST .../{id}/tick` is the owner's `change` — it advances the
//!   view model and publishes the diff on `global:live-<id>`.
//! * `POST .../{id}/subscribe` issues the short-TTL channel token bound
//!   to exactly that channel; the mounted transports
//!   (`/realtime/sse`, `/realtime/ws`, gated by the `app.realtime`
//!   flag) re-verify it at connect through the SAME
//!   [`ChannelTokenAuthorizer`](crate::broadcast_routes::ChannelTokenAuthorizer)
//!   the broadcast surface uses.
//! * `DELETE .../{id}` closes the session — the registry then refuses
//!   the id everywhere (stale, never remembered state).
//!
//! The one rule this edge exists to prove: a mounted channel
//! authorizes, publishes a diff, and the client frame preserves state —
//! the subscriber applies `{path: after}` entries to its local copy and
//! ends equal to the server's checked state.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Extension, Path, State};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use kelson_foundation::AppError;
use kelson_http::{Envelope, ResolvedSession};
use serde_json::{json, Value};

use crate::app_vm::WaitingRoomVm;
use crate::graph_routes::resolve_identity;
use crate::routes::AppState;
use kelson_views::ViewModel;

/// One hosted session: the registry id, the typed handle the owner's
/// `change` runs against, and who may drive it.
struct LiveEntry {
    id: kelson_views::LiveId,
    session: Arc<kelson_views::LiveSession<WaitingRoomVm>>,
    owner: kelson_foundation::PrincipalId,
}

/// The live edge this boot hosts: one hub, one session registry, one
/// wire, and the typed entries keyed by their public handle (`live-0`).
///
/// The typed map exists because `LiveNode` is object-safe for the
/// transport but has no `change` — the application keeps the typed
/// session, the registry serves the erased one.
pub struct LiveEdge {
    /// The hub the wire publishes on and the transports subscribe to
    /// (cloned into the authorized transport router at mount time).
    pub hub: kelson_realtime::BroadcastHub,
    /// The registry that answers "is this id still hosted?" (stale
    /// after close — every read refused).
    pub sessions: Arc<kelson_views::LiveSessions>,
    /// The wire: render → diff → publish (`live.update`).
    pub wire: kelson_views::LiveWire,
    entries: Mutex<HashMap<String, LiveEntry>>,
}

impl LiveEdge {
    /// One edge over the boot's views engine (the SAME engine the
    /// request path renders through — a live re-render goes through the
    /// schema-checked render, never a looser one).
    pub fn new(engine: Arc<kelson_views::ViewsEngine>) -> Self {
        let sessions = Arc::new(kelson_views::LiveSessions::new());
        let hub = kelson_realtime::BroadcastHub::new();
        let wire = kelson_views::LiveWire::new(hub.clone(), engine, sessions.clone());
        Self {
            hub,
            sessions,
            wire,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Open a session on the live view for `owner`. Refuses (fail
    /// closed) when the view's binding did not declare itself live or
    /// the registry never installed the model.
    pub fn open(
        &self,
        engine: &kelson_views::ViewsEngine,
        owner: kelson_foundation::PrincipalId,
    ) -> Result<String, AppError> {
        let session = engine
            .live_session(WaitingRoomVm::VIEW, WaitingRoomVm { waiting: 0 })
            .map_err(|error| {
                AppError::internal(format!("the live view refuses a session: {error}"))
            })?;
        let session = Arc::new(session);
        let id = self.sessions.push(session.clone());
        let handle = id.to_string();
        self.entries.lock().unwrap().insert(
            handle.clone(),
            LiveEntry {
                id,
                session,
                owner,
            },
        );
        Ok(handle)
    }

    /// Resolve a public handle: unknown or closed → 404 (the stale
    /// refusal — a closed session never serves its state again).
    fn resolve(
        &self,
        handle: &str,
    ) -> Result<(kelson_views::LiveId, Arc<kelson_views::LiveSession<WaitingRoomVm>>), AppError> {
        self.entries
            .lock()
            .unwrap()
            .get(handle)
            .map(|entry| (entry.id, entry.session.clone()))
            .ok_or_else(|| AppError::not_found(format!("live session {handle}")))
    }

    /// The owner gate: only the principal that opened the session may
    /// drive or subscribe it (the id is an opaque handle, not a
    /// credential).
    fn owner_of(&self, handle: &str) -> Result<kelson_foundation::PrincipalId, AppError> {
        self.entries
            .lock()
            .unwrap()
            .get(handle)
            .map(|entry| entry.owner)
            .ok_or_else(|| AppError::not_found(format!("live session {handle}")))
    }

    /// Check ownership for the caller, mapping mismatches to 403.
    fn require_owner(
        &self,
        handle: &str,
        principal: kelson_foundation::PrincipalId,
    ) -> Result<(), AppError> {
        if self.owner_of(handle)? != principal {
            return Err(AppError::denied(
                "only the session's owner may drive or subscribe it",
            ));
        }
        Ok(())
    }

    /// The owner's `change`: advance the state, then publish the diff.
    /// Returns the revision, the checked state and the changes that
    /// travelled — exactly what a subscriber's frame carries.
    pub fn tick(
        &self,
        handle: &str,
    ) -> Result<(u64, Value, Vec<kelson_views::DiffEntry>), AppError> {
        let (id, session) = self.resolve(handle)?;
        session.change(|vm| vm.waiting = vm.waiting.saturating_add(1));
        let changes = self
            .wire
            .publish(&id, session.as_ref())
            .map_err(|error| AppError::internal(format!("the live wire refused: {error}")))?
            .unwrap_or_default();
        let state = session
            .state_value()
            .map_err(|error| AppError::internal(format!("the live state is unreadable: {error}")))?;
        Ok((session.revision(), state, changes))
    }

    /// The channel a handle's diffs travel on (`global:live-<id>`).
    pub fn channel(&self, handle: &str) -> Result<String, AppError> {
        let (id, _) = self.resolve(handle)?;
        Ok(self.wire.channel(&id))
    }

    /// The current revision + checked state (the create/snapshot view).
    pub fn snapshot(&self, handle: &str) -> Result<(u64, Value), AppError> {
        let (_, session) = self.resolve(handle)?;
        let state = session
            .state_value()
            .map_err(|error| AppError::internal(format!("the live state is unreadable: {error}")))?;
        Ok((session.revision(), state))
    }

    /// Close a session: the handle goes stale everywhere — the next
    /// tick/subscribe/snapshot is a 404, never remembered state.
    pub fn close(&self, handle: &str) -> Result<(), AppError> {
        let (id, _) = self.resolve(handle)?;
        self.entries.lock().unwrap().remove(handle);
        self.sessions.close(&id);
        Ok(())
    }
}

// ----------------------------------------------------------------- helpers

fn error_response(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    error: &AppError,
) -> axum::response::Response {
    state.error_response(error, headers, None)
}

/// The edge this boot hosts — or the honest refusal as an error (a boot
/// without views has no engine to re-render a live state through).
fn edge_of(
    state: &AppState,
) -> Result<(Arc<kelson_views::ViewsEngine>, Arc<LiveEdge>), AppError> {
    let engine = state
        .views
        .clone()
        .ok_or_else(|| AppError::internal("views are not configured"))?;
    let edge = state.live.clone().ok_or_else(|| {
        AppError::internal("the live edge is not installed in this boot")
    })?;
    Ok((engine, edge))
}

// ------------------------------------------------------------------ routes

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/mvc/live", get(page))
        .route("/api/v1/live/sessions", post(create))
        .route("/api/v1/live/sessions/{id}", delete(close_session))
        .route("/api/v1/live/sessions/{id}/tick", post(tick))
        .route(
            "/api/v1/live/sessions/{id}/subscribe",
            post(subscribe),
        )
}

/// `GET /mvc/live` — the bound shell: rendered through the SAME
/// schema-checked render a live re-render uses, with the page's own
/// head. The browser's script then opens (or joins) the session.
pub async fn page(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<Extension<ResolvedSession>>,
) -> axum::response::Response {
    let (engine, _) = match edge_of(&state) {
        Ok(pair) => pair,
        Err(error) => return error_response(&state, &headers, &error),
    };
    let mut context = json!({});
    kelson_foundation::ViewContext::localized(
        &state.messages,
        &crate::i18n::resolve(
            &state.messages,
            crate::i18n::session_locale(session.as_ref().map(|s| &s.0)),
            None,
            headers
                .get(axum::http::header::ACCEPT_LANGUAGE)
                .and_then(|v| v.to_str().ok()),
        ),
    )
    .with_head(
        kelson_foundation::HeadBuilder::new()
            .title("Waiting room · Kelson")
            .meta("description", "A live waiting-room counter")
            .build(),
    )
    .apply(&mut context);
    match engine.render_bound(WaitingRoomVm::VIEW, context, &WaitingRoomVm { waiting: 0 }) {
        Ok(html) => axum::response::Html(html).into_response(),
        Err(error) => error_response(
            &state,
            &headers,
            &AppError::internal(format!("the live page refused to render: {error}")),
        ),
    }
}

/// `POST /api/v1/live/sessions` — open a session owned by the caller.
async fn create(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<Extension<ResolvedSession>>,
) -> axum::response::Response {
    let (principal, _) = match resolve_identity(&state, &headers, session.as_deref()) {
        Ok(identity) => identity,
        Err(error) => return error_response(&state, &headers, &error),
    };
    let (engine, edge) = match edge_of(&state) {
        Ok(pair) => pair,
        Err(error) => return error_response(&state, &headers, &error),
    };
    let handle = match edge.open(&engine, principal) {
        Ok(handle) => handle,
        Err(error) => return error_response(&state, &headers, &error),
    };
    let channel = match edge.channel(&handle) {
        Ok(channel) => channel,
        Err(error) => return error_response(&state, &headers, &error),
    };
    let (revision, state_value) = match edge.snapshot(&handle) {
        Ok(snapshot) => snapshot,
        Err(error) => return error_response(&state, &headers, &error),
    };
    Json(serde_json::to_value(Envelope::ok(json!({
        "id": handle,
        "channel": channel,
        "revision": revision,
        "state": state_value,
    })))
    .unwrap_or_default())
    .into_response()
}

/// `POST /api/v1/live/sessions/{id}/tick` — the owner's `change` +
/// publish. Non-owners 403; a closed handle 404s (stale).
async fn tick(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<Extension<ResolvedSession>>,
    Path(handle): Path<String>,
) -> axum::response::Response {
    let (principal, _) = match resolve_identity(&state, &headers, session.as_deref()) {
        Ok(identity) => identity,
        Err(error) => return error_response(&state, &headers, &error),
    };
    let (_, edge) = match edge_of(&state) {
        Ok(pair) => pair,
        Err(error) => return error_response(&state, &headers, &error),
    };
    if let Err(error) = edge.require_owner(&handle, principal) {
        return error_response(&state, &headers, &error);
    }
    match edge.tick(&handle) {
        Ok((revision, state_value, changes)) => Json(
            serde_json::to_value(Envelope::ok(json!({
                "id": handle,
                "revision": revision,
                "state": state_value,
                "changes": changes,
            })))
            .unwrap_or_default(),
        )
        .into_response(),
        Err(error) => error_response(&state, &headers, &error),
    }
}

/// `POST /api/v1/live/sessions/{id}/subscribe` — issue the short-TTL
/// channel token bound to exactly this session's channel. The token's
/// plaintext leaves once; `/realtime/sse` re-verifies it at connect.
async fn subscribe(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<Extension<ResolvedSession>>,
    Path(handle): Path<String>,
) -> axum::response::Response {
    let (principal, _) = match resolve_identity(&state, &headers, session.as_deref()) {
        Ok(identity) => identity,
        Err(error) => return error_response(&state, &headers, &error),
    };
    let (_, edge) = match edge_of(&state) {
        Ok(pair) => pair,
        Err(error) => return error_response(&state, &headers, &error),
    };
    if let Err(error) = edge.require_owner(&handle, principal) {
        return error_response(&state, &headers, &error);
    }
    let channel = match edge.channel(&handle) {
        Ok(channel) => channel,
        Err(error) => return error_response(&state, &headers, &error),
    };
    let issued = state.tokens.issue(
        principal,
        kelson_tokens::TokenPurpose::Channel,
        time::Duration::seconds(crate::broadcast_routes::CHANNEL_TOKEN_TTL_SECS),
        Some(channel.clone()),
    );
    match issued {
        Ok(issued) => Json(
            serde_json::to_value(Envelope::ok(json!({
                "token": issued.plaintext,
                "channel": channel,
                "expires_at": issued.token.expires_at,
            })))
            .unwrap_or_default(),
        )
        .into_response(),
        Err(error) => error_response(&state, &headers, &error),
    }
}

/// `DELETE /api/v1/live/sessions/{id}` — close it; the handle is stale
/// from here on.
async fn close_session(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<Extension<ResolvedSession>>,
    Path(handle): Path<String>,
) -> axum::response::Response {
    let (principal, _) = match resolve_identity(&state, &headers, session.as_deref()) {
        Ok(identity) => identity,
        Err(error) => return error_response(&state, &headers, &error),
    };
    let (_, edge) = match edge_of(&state) {
        Ok(pair) => pair,
        Err(error) => return error_response(&state, &headers, &error),
    };
    if let Err(error) = edge.require_owner(&handle, principal) {
        return error_response(&state, &headers, &error);
    }
    match edge.close(&handle) {
        Ok(()) => Json(
            serde_json::to_value(Envelope::ok(json!({ "id": handle, "closed": true })))
                .unwrap_or_default(),
        )
        .into_response(),
        Err(error) => error_response(&state, &headers, &error),
    }
}
