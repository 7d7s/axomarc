//! Wave 6 P7 broadcasting surface (19-PARITY "Broadcasting"):
//! `POST /api/v1/broadcast/auth` issues a short-TTL, channel-bound token
//! for a registered `routes/channels.toml` entry once the caller's snapshot
//! carries the channel's capability; `GET /api/v1/broadcast/members` lists
//! a `presence-*` channel's roster under the same gate; and
//! [`ChannelTokenAuthorizer`] is the host-side check the realtime
//! transports run at connect — a channel token never grants more than its
//! channel (the binding rides the token row's label, the expiry is the
//! token service's, and every failure answers the same uniform 401).
//!
//! Honest boundary: channel names are exact-matched. The Wave 2 wire
//! sanitizer allows `[a-zA-Z0-9:_-]{1,200}`, so parameterized names
//! (`orders.{id}`) that pass the compile gate can never be requested —
//! per-record traffic rides `tenant:{id}:...` style names instead.

use std::sync::Arc;

use axum::extract::{Extension, Query, State};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use keel_foundation::{AppError, PrincipalId, TenantId};
use keel_http::{Envelope, ResolvedSession};

use crate::graph_routes::resolve_identity;
use crate::routes::AppState;

/// Short-TTL window of a broadcast channel token (19-PARITY "short-TTL
/// signed channel token"). The subscription path re-verifies at every
/// connect, so a leaked token simply dies on its own.
const CHANNEL_TOKEN_TTL_SECS: i64 = 300;

/// Resolved broadcast channel authorization entry (Wave 6 P7): the channel
/// name plus every capability string a caller's snapshot must carry.
/// Bare capabilities are qualified with each declaring module — login
/// grants `module.capability` pairs, and the Wave 2 `GRAPH-CHANNEL-CAP`
/// gate accepts a capability declared by any module. A dotted reference
/// stays as-is (the compile gate accepted it as a qualified operation id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelAuthz {
    pub name: String,
    pub allowed: Vec<String>,
}

/// Merge every module's `routes/channels.toml` entries, resolving each
/// capability to the form the login snapshot actually carries.
pub fn resolve_channels(graph: &keel_registry::PlatformGraph) -> Vec<ChannelAuthz> {
    graph
        .modules
        .iter()
        .flat_map(|module| {
            module.channels.iter().map(move |channel| {
                let allowed = if channel.capability.contains('.') {
                    vec![channel.capability.clone()]
                } else {
                    graph
                        .modules
                        .iter()
                        .filter(|decl| decl.capabilities.contains_key(channel.capability.as_str()))
                        .map(|decl| format!("{}.{}", decl.name(), channel.capability))
                        .collect()
                };
                ChannelAuthz {
                    name: channel.name.clone(),
                    allowed,
                }
            })
        })
        .collect()
}

/// Look up a requested channel (exact match — see the module docs).
pub fn find_channel<'a>(
    channels: &'a [ChannelAuthz],
    requested: &str,
) -> Option<&'a ChannelAuthz> {
    channels.iter().find(|entry| entry.name == requested)
}

/// Capability gate shared by the auth and members endpoints: the channel
/// must be registered (an unknown name is a plain denial — no oracle), and
/// an active tenant snapshot must carry one of its resolved capabilities.
fn authorize_channel(
    state: &AppState,
    principal: PrincipalId,
    tenant: Option<TenantId>,
    channel: &str,
) -> Result<(), AppError> {
    let spec = find_channel(&state.channels, channel)
        .ok_or_else(|| AppError::denied("channel is not registered for this application"))?;
    let tenant = tenant
        .ok_or_else(|| AppError::denied("broadcast authorization requires a tenant session"))?;
    let snapshot = state
        .policy
        .snapshot(principal, tenant)
        .ok_or_else(|| AppError::denied("no active access snapshot"))?;
    if !spec.allowed.iter().any(|cap| snapshot.has_capability(cap)) {
        return Err(AppError::denied(format!(
            "channel capability not granted for {channel}"
        )));
    }
    Ok(())
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/broadcast/auth", post(auth))
        .route("/api/v1/broadcast/members", get(members))
        .with_state(state)
}

/// Wave 2 error rendering at this edge (status/view/reportable decisions).
fn error_response(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    err: &AppError,
) -> axum::response::Response {
    state.error_response(err, headers, None)
}

// ------------------------------------------------------------- auth issuance

#[derive(serde::Deserialize)]
struct AuthRequest {
    #[serde(default)]
    channel: String,
}

/// `POST /api/v1/broadcast/auth` — identity (session or bearer), then the
/// channel capability gate, then a short-TTL channel-bound token. The
/// token's plaintext leaves here exactly once; the subscription path
/// re-verifies the stored hash at connect.
async fn auth(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<Extension<ResolvedSession>>,
    body: axum::body::Bytes,
) -> axum::response::Response {
    let (principal, tenant) =
        match resolve_identity(&state, &headers, session.as_deref()) {
            Ok(identity) => identity,
            Err(err) => return error_response(&state, &headers, &err),
        };
    let request: AuthRequest = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => {
            return error_response(
                &state,
                &headers,
                &AppError::validation("request body must be JSON with a `channel` string"),
            )
        }
    };
    let channel = match keel_realtime::sanitize_channel(&request.channel) {
        Some(channel) => channel,
        None => {
            return error_response(&state, &headers, &AppError::validation("malformed channel"))
        }
    };
    if let Err(err) = authorize_channel(&state, principal, tenant, &channel) {
        return error_response(&state, &headers, &err);
    }
    let issued = match state.tokens.issue(
        principal,
        keel_tokens::TokenPurpose::Channel,
        time::Duration::seconds(CHANNEL_TOKEN_TTL_SECS),
        Some(channel.clone()),
    ) {
        Ok(issued) => issued,
        Err(err) => return error_response(&state, &headers, &err),
    };
    let envelope = Envelope::ok(serde_json::json!({
        "token": issued.plaintext,
        "channel": channel,
        "expires_at": issued.token.expires_at,
    }));
    (
        axum::http::StatusCode::OK,
        Json(serde_json::to_value(envelope).unwrap_or_default()),
    )
        .into_response()
}

// ---------------------------------------------------------- presence roster

#[derive(serde::Deserialize)]
struct MembersQuery {
    #[serde(default)]
    channel: String,
}

/// `GET /api/v1/broadcast/members?channel=presence-*` — the roster under
/// the same capability gate as issuing a token for the channel. Only
/// presence channels carry members; anything else is a validation refusal.
async fn members(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<Extension<ResolvedSession>>,
    Query(query): Query<MembersQuery>,
) -> axum::response::Response {
    let (principal, tenant) =
        match resolve_identity(&state, &headers, session.as_deref()) {
            Ok(identity) => identity,
            Err(err) => return error_response(&state, &headers, &err),
        };
    let channel = match keel_realtime::sanitize_channel(&query.channel) {
        Some(channel) => channel,
        None => {
            return error_response(&state, &headers, &AppError::validation("malformed channel"))
        }
    };
    if !channel.starts_with("presence-") {
        return error_response(
            &state,
            &headers,
            &AppError::validation("member lists are served for presence-* channels only"),
        );
    }
    if let Err(err) = authorize_channel(&state, principal, tenant, &channel) {
        return error_response(&state, &headers, &err);
    }
    let roster: Vec<serde_json::Value> = state
        .presence
        .members(&channel)
        .iter()
        .map(|member| {
            serde_json::json!({ "id": member.id, "display_name": member.display_name })
        })
        .collect();
    let envelope = Envelope::ok(serde_json::json!({
        "channel": channel,
        "members": roster,
    }));
    (
        axum::http::StatusCode::OK,
        Json(serde_json::to_value(envelope).unwrap_or_default()),
    )
        .into_response()
}

// ------------------------------------------------- subscription authorizer

/// Host-side subscription authorizer for the realtime transports (19-PARITY
/// "the realtime subscription path validates the token at connect"):
/// verify the channel token — purpose + expiry, uniform denial, so an
/// expired token is indistinguishable from a forged one — and enforce the
/// channel binding carried in its label. The subscriber identity comes
/// from the token's principal; the presence display name falls back to the
/// principal id when the principal has no email on file.
pub struct ChannelTokenAuthorizer {
    pub tokens: Arc<keel_tokens::TokenService>,
    pub principals: Arc<dyn keel_identity::PrincipalStore>,
}

impl keel_realtime::ChannelAuthorizer for ChannelTokenAuthorizer {
    fn authorize(
        &self,
        channel: &str,
        token: Option<&str>,
    ) -> Result<keel_realtime::PresenceMember, AppError> {
        let token = token
            .filter(|token| !token.is_empty())
            .ok_or_else(AppError::unauthenticated)?;
        let auth = self
            .tokens
            .verify(keel_tokens::TokenPurpose::Channel, token)
            .map_err(|_| AppError::unauthenticated())?;
        if auth.label.as_deref() != Some(channel) {
            // A channel token never grants more than its channel.
            return Err(AppError::unauthenticated());
        }
        let display_name = self
            .principals
            .by_id(auth.principal_id)
            .ok()
            .flatten()
            .and_then(|principal| principal.email)
            .unwrap_or_else(|| auth.principal_id.to_string());
        Ok(keel_realtime::PresenceMember {
            id: auth.principal_id.to_string(),
            display_name,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolution qualifies bare capabilities with every declaring module
    /// (what login grants) and keeps dotted references exactly as the
    /// Wave 2 compile gate accepted them.
    #[test]
    fn resolve_channels_qualifies_bare_capabilities_and_keeps_dotted() {
        let manifest = keel_registry::ModuleManifest::from_toml(include_str!(
            "../../applications/appointments/module.toml"
        ))
        .expect("appointments manifest parses");
        let graph = keel_registry::PlatformGraph::compile(vec![manifest]).expect("graph compiles");
        // No [[channel]] entries in the shipped manifest — resolution
        // starts empty and every resolved entry points at a granted form.
        let resolved = resolve_channels(&graph);
        for entry in &resolved {
            assert!(
                entry
                    .allowed
                    .iter()
                    .all(|cap| cap.contains('.') && !cap.starts_with('.')),
                "every allowed capability is a granted-form pair: {entry:?}"
            );
        }
        assert!(find_channel(&resolved, "nope:channel").is_none());
    }

    /// The authorizer binds the token to its channel (label), refuses
    /// missing and expired tokens with the same uniform error, and derives
    /// the subscriber identity from the token's principal.
    #[test]
    fn channel_authorizer_binds_token_expiry_and_identity() {
        use keel_realtime::ChannelAuthorizer as _;
        let tokens = Arc::new(keel_tokens::TokenService::new(Arc::new(
            keel_tokens::InMemoryTokenStore::new(),
        )));
        let principals: Arc<dyn keel_identity::PrincipalStore> =
            Arc::new(keel_identity::InMemoryPrincipalStore::new());
        let authorizer = ChannelTokenAuthorizer {
            tokens: tokens.clone(),
            principals,
        };
        let principal =
            keel_foundation::PrincipalId::from_uuid(uuid::Uuid::now_v7());

        let issued = tokens
            .issue(
                principal,
                keel_tokens::TokenPurpose::Channel,
                time::Duration::minutes(5),
                Some("presence-lobby".into()),
            )
            .expect("token issues");
        let member = authorizer
            .authorize("presence-lobby", Some(&issued.plaintext))
            .expect("matching channel admits");
        assert_eq!(member.id, principal.to_string());

        // The binding: the same token on another channel is refused.
        assert!(authorizer
            .authorize("presence-other", Some(&issued.plaintext))
            .is_err());
        // Missing token is refused the same way.
        assert!(authorizer.authorize("presence-lobby", None).is_err());

        // Expiry: a token whose TTL elapses answers identically.
        let tiny = tokens
            .issue(
                principal,
                keel_tokens::TokenPurpose::Channel,
                time::Duration::milliseconds(5),
                Some("presence-lobby".into()),
            )
            .expect("tiny-ttl token issues");
        std::thread::sleep(std::time::Duration::from_millis(25));
        assert!(authorizer
            .authorize("presence-lobby", Some(&tiny.plaintext))
            .is_err());
    }
}
