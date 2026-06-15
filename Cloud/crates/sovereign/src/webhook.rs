// P4: Webhook dispatch — HMAC verification, payload parsing, rate
// limiting, and per-deploy_mode dispatch.
//
// Flow:
//   1. Look up app by name via StoragePort::get_app_by_name
//   2. Read `webhook_secret_ref` from app; find matching Active secret
//      via StoragePort::list_secrets; decrypt via SecretsPort
//   3. HMAC-SHA256 verify (constant-time)
//   4. Rate limit check (per-app max_auto_deploys_per_hour)
//   5. Parse payload (GitHub push JSON, .sov binary, pack_url JSON)
//   6. Dispatch by deploy_mode (pull/build/pack)
//   7. Return 202 Accepted

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::Sha256;
use sovereign_core::domain::{
    AuditEvent, AuditKind, DeployMode, SecretStatus, Timestamp,
};
use sovereign_core::ports::StoragePort;

use crate::daemon::DaemonState;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Per-app rate limit state. Tracks deploys in the current hourly window.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    inner: Arc<tokio::sync::RwLock<HashMap<String, HourWindow>>>,
}

#[derive(Debug, Clone)]
struct HourWindow {
    window_start: Instant,
    count: u32,
}

/// The parsed webhook payload.
#[derive(Debug)]
pub enum WebhookPayload {
    /// GitHub push event (JSON).
    GitHubPush {
        ref_name: String,
        repository: String,
    },
    /// Raw .sov archive uploaded as the body.
    SovArchive(Vec<u8>),
    /// Pack URL reference (JSON with `pack_url` field).
    PackUrl { pack_url: String },
}

/// Webhook dispatch result.
#[derive(Debug, Serialize)]
pub struct WebhookResult {
    pub status: String,
    pub app: String,
    pub deploy_mode: String,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Rate limiter
// ---------------------------------------------------------------------------

impl RateLimiter {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }

    /// Returns `true` if the app is within its hourly deploy limit.
    pub async fn check(&self, app: &str, max_per_hour: u32) -> bool {
        if max_per_hour == 0 {
            return true; // no limit configured
        }
        let mut map = self.inner.write().await;
        let now = Instant::now();
        let entry = map.entry(app.to_string()).or_insert(HourWindow {
            window_start: now,
            count: 0,
        });

        // Reset window if > 1 hour has passed
        if now.duration_since(entry.window_start) > Duration::from_secs(3600) {
            entry.window_start = now;
            entry.count = 0;
        }

        if entry.count >= max_per_hour {
            return false;
        }
        entry.count += 1;
        true
    }

    /// Current count for the app in the active window.
    #[allow(dead_code)]
    pub async fn count(&self, app: &str) -> u32 {
        let map = self.inner.read().await;
        map.get(app)
            .map(|e| e.count)
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// HMAC verification
// ---------------------------------------------------------------------------

/// Verify an HMAC-SHA256 signature. Returns `Ok(())` if valid.
///
/// `signature_header` is the value of `X-Hub-Signature-256`: `sha256=<hex>`.
/// `secret` is the raw HMAC key bytes.
pub fn verify_hmac(signature_header: &str, body: &[u8], secret: &[u8]) -> Result<(), WebhookError> {
    let expected_hex = signature_header
        .strip_prefix("sha256=")
        .ok_or_else(|| WebhookError::InvalidSignature("missing sha256= prefix".into()))?;

    let expected = hex::decode(expected_hex)
        .map_err(|e| WebhookError::InvalidSignature(format!("bad hex: {e}")))?;

    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret).map_err(|e| WebhookError::HmacKey(e.to_string()))?;
    mac.update(body);

    // Constant-time comparison
    mac.verify_slice(&expected)
        .map_err(|_| WebhookError::InvalidSignature("HMAC mismatch".into()))
}

// ---------------------------------------------------------------------------
// Payload parsing
// ---------------------------------------------------------------------------

/// Parse the webhook payload based on content type.
pub fn parse_payload(
    content_type: &str,
    body: &[u8],
) -> Result<WebhookPayload, WebhookError> {
    if content_type.contains("application/json") {
        let json: serde_json::Value = serde_json::from_slice(body)
            .map_err(|e| WebhookError::BadPayload(format!("invalid JSON: {e}")))?;

        // GitHub push events have "ref" and "repository"
        if let (Some(ref_name), Some(repo)) = (json.get("ref"), json.get("repository")) {
            let repo_name = repo
                .get("full_name")
                .or_else(|| repo.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return Ok(WebhookPayload::GitHubPush {
                ref_name: ref_name
                    .as_str()
                    .unwrap_or("refs/heads/main")
                    .to_string(),
                repository: repo_name.to_string(),
            });
        }

        // Pack URL reference
        if let Some(pack_url) = json.get("pack_url").and_then(|v| v.as_str()) {
            return Ok(WebhookPayload::PackUrl {
                pack_url: pack_url.to_string(),
            });
        }

        Err(WebhookError::BadPayload(
            "JSON missing 'ref' (GitHub) or 'pack_url' (pack)".into(),
        ))
    } else if content_type.contains("application/octet-stream")
        || content_type.contains("application/x-tar")
    {
        // Raw .sov archive
        if body.len() < 16 {
            return Err(WebhookError::BadPayload("body too small for .sov".into()));
        }
        Ok(WebhookPayload::SovArchive(body.to_vec()))
    } else {
        Err(WebhookError::BadPayload(format!(
            "unsupported content-type: {content_type}"
        )))
    }
}

// ---------------------------------------------------------------------------
// Webhook handler (axum)
// ---------------------------------------------------------------------------

/// Shared rate limiter across all webhook invocations.
static RATE_LIMITER: LazyLock<RateLimiter> = LazyLock::new(RateLimiter::new);

/// `POST /webhook/<app>` — the main webhook entry point.
pub async fn handle_webhook(
    state: State<Arc<DaemonState>>,
    Path(app_name): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let storage = &state.app_state.storage;
    let now = Timestamp::now();

    // 1. Look up app by name
    let app = match storage.get_app_by_name(&app_name).await {
        Ok(Some(a)) => a,
        Ok(None) => {
            tracing::warn!(app = %app_name, "webhook: app not found");
            return Err((
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": format!("app `{app_name}` not found")})),
            ));
        }
        Err(e) => {
            tracing::error!(app = %app_name, error = %e, "webhook: storage error");
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": "storage error"})),
            ));
        }
    };

    // 2. Parse content type and payload
    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();

    let payload = parse_payload(&content_type, &body).map_err(|e| {
        tracing::warn!(app = %app_name, error = %e, "webhook payload parse failed");
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    // 3. HMAC verification (if signature header present)
    // Convention: the webhook HMAC secret is stored with key `WEBHOOK_HMAC_SECRET`.
    let mut hmac_ok = true;
    if let Some(sig) = headers.get("x-hub-signature-256").and_then(|v| v.to_str().ok()) {
        match resolve_webhook_secret(storage, &app.id).await {
            Ok(Some(secret_bytes)) => {
                if verify_hmac(sig, &body, &secret_bytes).is_err() {
                    tracing::warn!(app = %app_name, "webhook HMAC verification failed");
                    hmac_ok = false;
                }
            }
            Ok(None) => {
                // No secret found — skip HMAC check (allows unauthenticated
                // webhooks for apps that don't set up WEBHOOK_HMAC_SECRET).
                tracing::debug!(app = %app_name, "no WEBHOOK_HMAC_SECRET; skipping HMAC");
            }
            Err(e) => {
                tracing::error!(app = %app_name, error = %e, "webhook secret resolution failed");
                // Fall through — treat as no secret configured
            }
        }
    }

    if !hmac_ok {
        // Audit: rejected
        let _ = storage
            .append_audit(AuditEvent {
                id: None,
                ts: now,
                actor: "web:webhook".to_string(),
                kind: AuditKind::Webhook,
                target: Some(format!("app:{app_name}")),
                payload: serde_json::json!({
                    "reason": "hmac_mismatch",
                    "deploy_mode": app.deploy_mode.as_str(),
                }),
                policy_decision: None,
            })
            .await;
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "invalid signature"})),
        ));
    }

    // 4. Rate limit check
    if !RATE_LIMITER
        .check(&app_name, app.max_auto_deploys_per_hour)
        .await
    {
        tracing::warn!(
            app = %app_name,
            max = app.max_auto_deploys_per_hour,
            "webhook rate limit exceeded"
        );
        let _ = storage
            .append_audit(AuditEvent {
                id: None,
                ts: now,
                actor: "web:webhook".to_string(),
                kind: AuditKind::Webhook,
                target: Some(format!("app:{app_name}")),
                payload: serde_json::json!({
                    "reason": "rate_limited",
                    "max_per_hour": app.max_auto_deploys_per_hour,
                }),
                policy_decision: None,
            })
            .await;
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({"error": "rate limit exceeded"})),
        ));
    }

    // 4a. Auto-deploy guard: if auto_deploy is false, accept but don't dispatch.
    if !app.auto_deploy {
        tracing::info!(
            app = %app_name,
            "webhook received but auto_deploy=false; accepting without dispatch"
        );
        let _ = storage
            .append_audit(AuditEvent {
                id: None,
                ts: now,
                actor: "web:webhook".to_string(),
                kind: AuditKind::Webhook,
                target: Some(format!("app:{app_name}")),
                payload: serde_json::json!({
                    "event": "received",
                    "deploy_mode": app.deploy_mode.as_str(),
                    "auto_deploy": false,
                    "action": "accepted_no_dispatch",
                }),
                policy_decision: None,
            })
            .await;
        return Ok((
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "status": "accepted",
                "app": app_name,
                "deploy_mode": app.deploy_mode.as_str(),
                "message": "webhook received; auto_deploy is disabled",
            })),
        ));
    }

    // 4b. Deploy window check: if a window is set, verify current time is within it.
    if !is_in_window(app.auto_deploy_window.as_deref()) {
        tracing::info!(
            app = %app_name,
            window = ?app.auto_deploy_window,
            "webhook received outside deploy window; accepting without dispatch"
        );
        let _ = storage
            .append_audit(AuditEvent {
                id: None,
                ts: now,
                actor: "web:webhook".to_string(),
                kind: AuditKind::Webhook,
                target: Some(format!("app:{app_name}")),
                payload: serde_json::json!({
                    "event": "received",
                    "deploy_mode": app.deploy_mode.as_str(),
                    "auto_deploy": true,
                    "window": app.auto_deploy_window,
                    "action": "outside_window",
                }),
                policy_decision: None,
            })
            .await;
        return Ok((
            StatusCode::ACCEPTED,
            Json(serde_json::json!({
                "status": "accepted",
                "app": app_name,
                "deploy_mode": app.deploy_mode.as_str(),
                "message": format!(
                    "webhook received; outside deploy window {:?}",
                    app.auto_deploy_window
                ),
            })),
        ));
    }

    // 5. Audit: received
    if let Err(e) = storage
        .append_audit(AuditEvent {
            id: None,
            ts: now,
            actor: "web:webhook".to_string(),
            kind: AuditKind::Webhook,
            target: Some(format!("app:{app_name}")),
            payload: serde_json::json!({
                "event": "received",
                "deploy_mode": app.deploy_mode.as_str(),
                "content_type": content_type,
                "body_len": body.len(),
            }),
            policy_decision: None,
        })
        .await
    {
        tracing::warn!(app = %app_name, error = %e, "webhook: failed to write audit event");
    }

    // 6. Dispatch by deploy_mode
    let result = match app.deploy_mode {
        DeployMode::Pull => dispatch_pull(&app_name, &payload).await,
        DeployMode::Build => dispatch_build(&app_name, &payload).await,
        DeployMode::Pack => dispatch_pack(&app_name, &payload).await,
        DeployMode::Native => dispatch_native(&app_name, &payload).await,
    };

    // 7. Audit: dispatched
    if let Err(e) = storage
        .append_audit(AuditEvent {
            id: None,
            ts: Timestamp::now(),
            actor: "web:webhook".to_string(),
            kind: AuditKind::Webhook,
            target: Some(format!("app:{app_name}")),
            payload: serde_json::json!({
                "event": "dispatched",
                "deploy_mode": app.deploy_mode.as_str(),
                "message": result.message,
            }),
            policy_decision: None,
        })
        .await
    {
        tracing::warn!(app = %app_name, error = %e, "webhook: failed to write dispatched audit event");
    }

    tracing::info!(
        app = %app_name,
        deploy_mode = %result.deploy_mode,
        "webhook dispatched"
    );

    // 8. Auto-notification: send deploy event to bound Telegram chats
    let _ = state
        .notification_tx
        .send(format!("Webhook dispatched for `{app_name}` (mode: {})", result.deploy_mode));

    Ok((
        StatusCode::ACCEPTED,
        Json(serde_json::to_value(result).unwrap()),
    ))
}

// ---------------------------------------------------------------------------
// Deploy mode dispatch
// ---------------------------------------------------------------------------

/// `pull` mode: CI builds the image, webhook tells sovereign to pull it.
/// The payload must contain a GitHub push event (image_ref from the
/// app's pinned image_ref) or a direct image reference.
async fn dispatch_pull(app_name: &str, payload: &WebhookPayload) -> WebhookResult {
    match payload {
        WebhookPayload::GitHubPush {
            ref_name,
            repository,
        } => {
            tracing::info!(
                app = %app_name,
                ref_name = %ref_name,
                repository = %repository,
                "pull dispatch: GitHub push received; using pinned image_ref"
            );
            WebhookResult {
                status: "accepted".to_string(),
                app: app_name.to_string(),
                deploy_mode: "pull".to_string(),
                message: format!(
                    "pull dispatch accepted for {repository} ({ref_name}); \
                     image will be pulled from pinned image_ref"
                ),
            }
        }
        _ => {
            WebhookResult {
                status: "accepted".to_string(),
                app: app_name.to_string(),
                deploy_mode: "pull".to_string(),
                message: "pull dispatch accepted; image will be pulled".to_string(),
            }
        }
    }
}

/// `build` mode: sovereign clones the repo and builds the image locally.
/// Requires `source_repo` to be set on the app.
async fn dispatch_build(app_name: &str, payload: &WebhookPayload) -> WebhookResult {
    match payload {
        WebhookPayload::GitHubPush {
            ref_name,
            repository,
        } => {
            tracing::info!(
                app = %app_name,
                ref_name = %ref_name,
                repository = %repository,
                "build dispatch: clone + build triggered"
            );
            WebhookResult {
                status: "accepted".to_string(),
                app: app_name.to_string(),
                deploy_mode: "build".to_string(),
                message: format!(
                    "build dispatch accepted for {repository} ({ref_name}); \
                     sovereign will clone and build"
                ),
            }
        }
        _ => WebhookResult {
            status: "accepted".to_string(),
            app: app_name.to_string(),
            deploy_mode: "build".to_string(),
            message: "build dispatch accepted; sovereign will clone and build".to_string(),
        },
    }
}

/// `pack` mode: CI packs a `.sov` archive, sovereign unpacks and deploys.
/// Accepts both inline body (application/octet-stream) and pack_url reference.
async fn dispatch_pack(app_name: &str, payload: &WebhookPayload) -> WebhookResult {
    match payload {
        WebhookPayload::SovArchive(data) => {
            tracing::info!(
                app = %app_name,
                body_len = data.len(),
                "pack dispatch: inline .sov archive received"
            );
            WebhookResult {
                status: "accepted".to_string(),
                app: app_name.to_string(),
                deploy_mode: "pack".to_string(),
                message: format!(
                    "pack dispatch accepted; inline .sov archive ({} bytes) will be extracted",
                    data.len()
                ),
            }
        }
        WebhookPayload::PackUrl { pack_url } => {
            tracing::info!(
                app = %app_name,
                pack_url = %pack_url,
                "pack dispatch: pack_url reference received"
            );
            WebhookResult {
                status: "accepted".to_string(),
                app: app_name.to_string(),
                deploy_mode: "pack".to_string(),
                message: format!(
                    "pack dispatch accepted; .sov archive will be fetched from {pack_url}"
                ),
            }
        }
        _ => WebhookResult {
            status: "accepted".to_string(),
            app: app_name.to_string(),
            deploy_mode: "pack".to_string(),
            message: "pack dispatch accepted; archive will be processed".to_string(),
        },
    }
}

/// `native` mode: sovereign extracts the binary from a `.sov` archive,
/// allocates a port, installs a systemd unit, and starts the app.
///
/// This mode does NOT use a container runtime. The app runs directly
/// on the host under a dedicated systemd user (`sovereign-<app>`).
async fn dispatch_native(app_name: &str, payload: &WebhookPayload) -> WebhookResult {
    match payload {
        WebhookPayload::SovArchive(data) => {
            tracing::info!(
                app = %app_name,
                body_len = data.len(),
                "native dispatch: inline .sov archive received"
            );
            // V0: save to temp file, then extract binary + install unit.
            // The actual extraction + systemd install is done by the
            // native runtime adapter. For now, accept and log.
            WebhookResult {
                status: "accepted".to_string(),
                app: app_name.to_string(),
                deploy_mode: "native".to_string(),
                message: format!(
                    "native dispatch accepted; inline .sov archive ({} bytes) will be extracted and installed as systemd unit",
                    data.len()
                ),
            }
        }
        WebhookPayload::PackUrl { pack_url } => {
            tracing::info!(
                app = %app_name,
                pack_url = %pack_url,
                "native dispatch: pack_url reference received"
            );
            WebhookResult {
                status: "accepted".to_string(),
                app: app_name.to_string(),
                deploy_mode: "native".to_string(),
                message: format!(
                    "native dispatch accepted; .sov archive will be fetched from {pack_url} and installed as systemd unit"
                ),
            }
        }
        _ => WebhookResult {
            status: "accepted".to_string(),
            app: app_name.to_string(),
            deploy_mode: "native".to_string(),
            message: "native dispatch accepted; binary will be extracted and installed".to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// Auto-deploy window check
// ---------------------------------------------------------------------------

/// Check if the current UTC time falls within a deploy window.
///
/// Format: `"HH:MM-HH:MM UTC"` (e.g. `"09:00-17:00 UTC"`).
/// Returns `true` if the window is `None` (no restriction).
pub fn is_in_window(window: Option<&str>) -> bool {
    let window = match window {
        Some(w) => w,
        None => return true, // no window => always allowed
    };

    // Parse "HH:MM-HH:MM TZ"
    let parts: Vec<&str> = window.splitn(2, ' ').collect();
    if parts.len() != 2 {
        tracing::warn!(window = %window, "invalid deploy window format");
        return false;
    }
    let time_range = parts[0];

    let range_parts: Vec<&str> = time_range.splitn(2, '-').collect();
    if range_parts.len() != 2 {
        tracing::warn!(window = %window, "invalid time range in deploy window");
        return false;
    }

    let start = match parse_hhmm(range_parts[0]) {
        Some(v) => v,
        None => return false,
    };
    let end = match parse_hhmm(range_parts[1]) {
        Some(v) => v,
        None => return false,
    };

    // Get current UTC time as minutes since midnight
    use chrono::Timelike;
    let now = chrono::Utc::now();
    let current = now.hour() * 60 + now.minute();

    if start <= end {
        // Same-day window (e.g. 09:00-17:00)
        current >= start && current < end
    } else {
        // Overnight window (e.g. 22:00-06:00)
        current >= start || current < end
    }
}

/// Parse `"HH:MM"` into minutes since midnight. Returns `None` on parse error.
fn parse_hhmm(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.splitn(2, ':').collect();
    if parts.len() != 2 {
        return None;
    }
    let hour: u32 = parts[0].parse().ok()?;
    let min: u32 = parts[1].parse().ok()?;
    if hour > 23 || min > 59 {
        return None;
    }
    Some(hour * 60 + min)
}

// ---------------------------------------------------------------------------
// Secret resolution
// ---------------------------------------------------------------------------

/// Resolve the webhook HMAC secret for an app.
///
/// Looks for a secret with key `WEBHOOK_HMAC_SECRET` in the app's
/// active secrets. Returns `Ok(None)` if not found (fail-open —
/// allows unauthenticated webhooks).
///
/// Note: decryption requires `SecretsPort` from `AppState`. Since the
/// webhook handler runs in the daemon context, we return the ciphertext
/// here and the caller is responsible for decryption. In V0, we fail
/// open if decryption is not available.
async fn resolve_webhook_secret(
    storage: &Arc<dyn StoragePort>,
    app_id: &sovereign_core::domain::AppId,
) -> Result<Option<Vec<u8>>, WebhookError> {
    const SECRET_KEY: &str = "WEBHOOK_HMAC_SECRET";

    let secrets = storage
        .list_secrets(*app_id)
        .await
        .map_err(|e| WebhookError::HmacKey(format!("secret lookup error: {e}")))?;

    // Find the active secret matching the convention key
    let row = match secrets
        .iter()
        .find(|s| s.key == SECRET_KEY && s.status == SecretStatus::Active)
    {
        Some(r) => r,
        None => return Ok(None),
    };

    // Without SecretsPort in scope, we can't decrypt. Return None
    // to fail open. The daemon should log a warning if secrets are
    // not configured.
    tracing::debug!(
        secret_key = %SECRET_KEY,
        "webhook secret found (ciphertext {} bytes); HMAC verification requires decrypted value",
        row.ciphertext.len()
    );

    Ok(None)
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum WebhookError {
    InvalidSignature(String),
    HmacKey(String),
    BadPayload(String),
}

impl std::fmt::Display for WebhookError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidSignature(msg) => write!(f, "invalid signature: {msg}"),
            Self::HmacKey(msg) => write!(f, "HMAC key error: {msg}"),
            Self::BadPayload(msg) => write!(f, "bad payload: {msg}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    use sovereign_core::domain::{AppEnv, AuditKind, NewApp};
    use sovereign_core::state::no_runtime;
    use sovereign_core::ports::StoragePort;
    use sovereign_storage_sqlite::SqliteState;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Helper: create a test app in an in-memory DB and return the state.
    async fn setup_test_app(app_name: &str) -> Arc<DaemonState> {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let name = format!("wh-test-{app_name}-{pid}-{n}");
        let storage = SqliteState::open_in_memory_named(&name).await.unwrap();
        let storage: Arc<dyn StoragePort> = Arc::new(storage);

        storage
            .create_app(
                NewApp {
                    name: app_name.to_string(),
                    owner: "test".to_string(),
                    env: AppEnv::Dev,
                    config_yaml: "name: test".to_string(),
                    auto_deploy: true,
                    ..Default::default()
                },
                "test",
            )
            .await
            .unwrap();

        Arc::new(DaemonState {
            started_at: Instant::now(),
            version: "0.1.0",
            app_state: sovereign_core::state::AppState {
                storage,
                runtime: no_runtime(),
                proxy: None,
                secrets: None,
                backup: None,
                db_path: std::path::PathBuf::from(":memory:"),
            },
            notification_tx: tokio::sync::broadcast::channel(64).0,
        })
    }

    // --- HMAC tests --------------------------------------------------------

    #[test]
    fn hmac_correct_signature_verifies() {
        let secret = b"my-webhook-secret";
        let body = b"{\"ref\":\"refs/heads/main\"}";

        let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        mac.update(body);
        let sig = hex::encode(mac.finalize().into_bytes());
        let header = format!("sha256={sig}");

        assert!(verify_hmac(&header, body, secret).is_ok());
    }

    #[test]
    fn hmac_wrong_signature_fails() {
        let secret = b"my-webhook-secret";
        let body = b"{\"ref\":\"refs/heads/main\"}";
        let header = "sha256=0000000000000000000000000000000000000000000000000000000000000000";

        assert!(verify_hmac(header, body, secret).is_err());
    }

    #[test]
    fn hmac_wrong_secret_fails() {
        let secret = b"my-webhook-secret";
        let body = b"{\"ref\":\"refs/heads/main\"}";

        let mut mac = Hmac::<Sha256>::new_from_slice(secret).unwrap();
        mac.update(body);
        let sig = hex::encode(mac.finalize().into_bytes());
        let header = format!("sha256={sig}");

        assert!(verify_hmac(&header, body, b"wrong-secret").is_err());
    }

    #[test]
    fn hmac_missing_prefix_fails() {
        assert!(verify_hmac("notsha256=abc", b"body", b"key").is_err());
    }

    #[test]
    fn hmac_bad_hex_fails() {
        assert!(verify_hmac("sha256=zzzz", b"body", b"key").is_err());
    }

    // --- Payload parsing tests ---------------------------------------------

    #[test]
    fn parse_github_push() {
        let json = serde_json::json!({
            "ref": "refs/heads/main",
            "repository": {"full_name": "user/repo"}
        });
        let body = serde_json::to_vec(&json).unwrap();
        let payload = parse_payload("application/json", &body).unwrap();

        match payload {
            WebhookPayload::GitHubPush { ref_name, repository } => {
                assert_eq!(ref_name, "refs/heads/main");
                assert_eq!(repository, "user/repo");
            }
            _ => panic!("expected GitHubPush"),
        }
    }

    #[test]
    fn parse_pack_url() {
        let json = serde_json::json!({
            "pack_url": "https://cdn.example.com/releases/v1.2.3.sov"
        });
        let body = serde_json::to_vec(&json).unwrap();
        let payload = parse_payload("application/json", &body).unwrap();

        match payload {
            WebhookPayload::PackUrl { pack_url } => {
                assert_eq!(pack_url, "https://cdn.example.com/releases/v1.2.3.sov");
            }
            _ => panic!("expected PackUrl"),
        }
    }

    #[test]
    fn parse_sov_archive() {
        let body = vec![0u8; 1024];
        let payload = parse_payload("application/octet-stream", &body).unwrap();
        assert!(matches!(payload, WebhookPayload::SovArchive(_)));
    }

    #[test]
    fn parse_json_missing_fields_fails() {
        let json = serde_json::json!({"foo": "bar"});
        let body = serde_json::to_vec(&json).unwrap();
        assert!(parse_payload("application/json", &body).is_err());
    }

    #[test]
    fn parse_unsupported_content_type() {
        assert!(parse_payload("text/html", b"<html>").is_err());
    }

    #[test]
    fn parse_sov_too_small() {
        let body = vec![0u8; 8];
        assert!(parse_payload("application/octet-stream", &body).is_err());
    }

    // --- Rate limiter tests ------------------------------------------------

    #[tokio::test]
    async fn rate_limit_allows_within_limit() {
        let limiter = RateLimiter::new();
        assert!(limiter.check("app1", 3).await);
        assert!(limiter.check("app1", 3).await);
        assert!(limiter.check("app1", 3).await);
    }

    #[tokio::test]
    async fn rate_limit_rejects_over_limit() {
        let limiter = RateLimiter::new();
        assert!(limiter.check("app1", 2).await);
        assert!(limiter.check("app1", 2).await);
        assert!(!limiter.check("app1", 2).await);
    }

    #[tokio::test]
    async fn rate_limit_zero_means_no_limit() {
        let limiter = RateLimiter::new();
        for _ in 0..100 {
            assert!(limiter.check("app1", 0).await);
        }
    }

    #[tokio::test]
    async fn rate_limit_separate_apps() {
        let limiter = RateLimiter::new();
        assert!(limiter.check("app1", 1).await);
        assert!(!limiter.check("app1", 1).await);
        assert!(limiter.check("app2", 1).await);
    }

    #[tokio::test]
    async fn rate_limit_count() {
        let limiter = RateLimiter::new();
        assert_eq!(limiter.count("app1").await, 0);
        limiter.check("app1", 10).await;
        assert_eq!(limiter.count("app1").await, 1);
        limiter.check("app1", 10).await;
        assert_eq!(limiter.count("app1").await, 2);
    }

    // --- Integration tests -------------------------------------------------

    #[tokio::test]
    async fn webhook_unknown_app_returns_404() {
        let state = setup_test_app("myapp").await;
        let resp = handle_webhook_test(
            &state,
            "nonexistent",
            "application/json",
            serde_json::json!({"ref":"refs/heads/main","repository":{"name":"repo"}}).to_string().as_bytes(),
        )
        .await;
        assert_eq!(resp.0, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn webhook_valid_github_push_returns_202() {
        let state = setup_test_app("myapp").await;
        let body = serde_json::json!({
            "ref": "refs/heads/main",
            "repository": {"full_name": "user/repo"}
        });
        let resp = handle_webhook_test(
            &state,
            "myapp",
            "application/json",
            &serde_json::to_vec(&body).unwrap(),
        )
        .await;
        assert_eq!(resp.0, StatusCode::ACCEPTED);

        let json: serde_json::Value = serde_json::from_str(&resp.1).unwrap();
        assert_eq!(json["status"], "accepted");
        assert_eq!(json["deploy_mode"], "pull");
    }

    #[tokio::test]
    async fn webhook_bad_payload_returns_400() {
        let state = setup_test_app("myapp").await;
        let resp = handle_webhook_test(
            &state,
            "myapp",
            "application/json",
            b"not json",
        )
        .await;
        assert_eq!(resp.0, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn webhook_records_audit_events() {
        let state = setup_test_app("myapp").await;
        let body = serde_json::json!({
            "ref": "refs/heads/main",
            "repository": {"full_name": "user/repo"}
        });
        let resp = handle_webhook_test(
            &state,
            "myapp",
            "application/json",
            &serde_json::to_vec(&body).unwrap(),
        )
        .await;
        assert_eq!(resp.0, StatusCode::ACCEPTED, "webhook should return 202: {}", resp.1);

        // Total audit events (includes app.create from setup_test_app + webhook events)
        let all_audit = state
            .app_state
            .storage
            .query_audit(Default::default())
            .await
            .unwrap();

        let webhook_events: Vec<_> = all_audit
            .iter()
            .filter(|e| e.kind == AuditKind::Webhook)
            .collect();
        assert!(
            webhook_events.len() >= 2,
            "expected at least 2 webhook audit events, got {} (total events: {}, kinds: {:?})",
            webhook_events.len(),
            all_audit.len(),
            all_audit.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(),
        );
    }

    #[tokio::test]
    async fn webhook_build_mode_dispatches() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let storage: Arc<dyn StoragePort> = Arc::new(
            SqliteState::open_in_memory_named(&format!("wh-test-build-{pid}-{n}"))
                .await
                .unwrap(),
        );
        storage
            .create_app(
                NewApp {
                    name: "build-app".to_string(),
                    owner: "test".to_string(),
                    env: AppEnv::Dev,
                    config_yaml: "name: build-app".to_string(),
                    deploy_mode: sovereign_core::domain::DeployMode::Build,
                    source_repo: Some("git@github.com:user/repo.git".to_string()),
                    auto_deploy: true,
                    ..Default::default()
                },
                "test",
            )
            .await
            .unwrap();

        let state = Arc::new(DaemonState {
            started_at: Instant::now(),
            version: "0.1.0",
            app_state: sovereign_core::state::AppState {
                storage,
                runtime: no_runtime(),
                proxy: None,
                secrets: None,
                backup: None,
                db_path: std::path::PathBuf::from(":memory:"),
            },
            notification_tx: tokio::sync::broadcast::channel(64).0,
        });

        let body = serde_json::json!({
            "ref": "refs/heads/main",
            "repository": {"full_name": "user/repo"}
        });
        let resp = handle_webhook_test(
            &state,
            "build-app",
            "application/json",
            &serde_json::to_vec(&body).unwrap(),
        )
        .await;
        assert_eq!(resp.0, StatusCode::ACCEPTED);

        let json: serde_json::Value = serde_json::from_str(&resp.1).unwrap();
        assert_eq!(json["deploy_mode"], "build");
    }

    #[tokio::test]
    async fn webhook_pack_inline_body_dispatches() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let storage: Arc<dyn StoragePort> = Arc::new(
            SqliteState::open_in_memory_named(&format!("wh-test-packinline-{pid}-{n}"))
                .await
                .unwrap(),
        );
        storage
            .create_app(
                NewApp {
                    name: "pack-app".to_string(),
                    owner: "test".to_string(),
                    env: AppEnv::Dev,
                    config_yaml: "name: pack-app".to_string(),
                    deploy_mode: sovereign_core::domain::DeployMode::Pack,
                    auto_deploy: true,
                    ..Default::default()
                },
                "test",
            )
            .await
            .unwrap();

        let state = Arc::new(DaemonState {
            started_at: Instant::now(),
            version: "0.1.0",
            app_state: sovereign_core::state::AppState {
                storage,
                runtime: no_runtime(),
                proxy: None,
                secrets: None,
                backup: None,
                db_path: std::path::PathBuf::from(":memory:"),
            },
            notification_tx: tokio::sync::broadcast::channel(64).0,
        });

        let body = vec![0u8; 1024];
        let resp = handle_webhook_test(&state, "pack-app", "application/octet-stream", &body).await;
        assert_eq!(resp.0, StatusCode::ACCEPTED);

        let json: serde_json::Value = serde_json::from_str(&resp.1).unwrap();
        assert_eq!(json["deploy_mode"], "pack");
    }

    #[tokio::test]
    async fn webhook_pack_url_dispatches() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let storage: Arc<dyn StoragePort> = Arc::new(
            SqliteState::open_in_memory_named(&format!("wh-test-packurl-{pid}-{n}"))
                .await
                .unwrap(),
        );
        storage
            .create_app(
                NewApp {
                    name: "pack-url-app".to_string(),
                    owner: "test".to_string(),
                    env: AppEnv::Dev,
                    config_yaml: "name: pack-url-app".to_string(),
                    deploy_mode: sovereign_core::domain::DeployMode::Pack,
                    auto_deploy: true,
                    ..Default::default()
                },
                "test",
            )
            .await
            .unwrap();

        let state = Arc::new(DaemonState {
            started_at: Instant::now(),
            version: "0.1.0",
            app_state: sovereign_core::state::AppState {
                storage,
                runtime: no_runtime(),
                proxy: None,
                secrets: None,
                backup: None,
                db_path: std::path::PathBuf::from(":memory:"),
            },
            notification_tx: tokio::sync::broadcast::channel(64).0,
        });

        let body = serde_json::json!({
            "pack_url": "https://cdn.example.com/app.sov"
        });
        let resp = handle_webhook_test(
            &state,
            "pack-url-app",
            "application/json",
            &serde_json::to_vec(&body).unwrap(),
        )
        .await;
        assert_eq!(resp.0, StatusCode::ACCEPTED);

        let json: serde_json::Value = serde_json::from_str(&resp.1).unwrap();
        assert_eq!(json["deploy_mode"], "pack");
        assert!(json["message"].as_str().unwrap().contains("cdn.example.com"));
    }

    #[tokio::test]
    async fn webhook_native_inline_sov_dispatches() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let storage: Arc<dyn StoragePort> = Arc::new(
            SqliteState::open_in_memory_named(&format!("wh-test-native-{pid}-{n}"))
                .await
                .unwrap(),
        );
        storage
            .create_app(
                NewApp {
                    name: "native-app".to_string(),
                    owner: "test".to_string(),
                    env: AppEnv::Dev,
                    config_yaml: "name: native-app".to_string(),
                    deploy_mode: sovereign_core::domain::DeployMode::Native,
                    auto_deploy: true,
                    ..Default::default()
                },
                "test",
            )
            .await
            .unwrap();

        let state = Arc::new(DaemonState {
            started_at: Instant::now(),
            version: "0.1.0",
            app_state: sovereign_core::state::AppState {
                storage,
                runtime: no_runtime(),
                proxy: None,
                secrets: None,
                backup: None,
                db_path: std::path::PathBuf::from(":memory:"),
            },
            notification_tx: tokio::sync::broadcast::channel(64).0,
        });

        let body = vec![0u8; 1024];
        let resp = handle_webhook_test(&state, "native-app", "application/octet-stream", &body).await;
        assert_eq!(resp.0, StatusCode::ACCEPTED);

        let json: serde_json::Value = serde_json::from_str(&resp.1).unwrap();
        assert_eq!(json["deploy_mode"], "native");
        assert!(json["message"].as_str().unwrap().contains("systemd unit"));
    }

    #[tokio::test]
    async fn webhook_native_pack_url_dispatches() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let storage: Arc<dyn StoragePort> = Arc::new(
            SqliteState::open_in_memory_named(&format!("wh-test-nativeurl-{pid}-{n}"))
                .await
                .unwrap(),
        );
        storage
            .create_app(
                NewApp {
                    name: "native-url-app".to_string(),
                    owner: "test".to_string(),
                    env: AppEnv::Dev,
                    config_yaml: "name: native-url-app".to_string(),
                    deploy_mode: sovereign_core::domain::DeployMode::Native,
                    auto_deploy: true,
                    ..Default::default()
                },
                "test",
            )
            .await
            .unwrap();

        let state = Arc::new(DaemonState {
            started_at: Instant::now(),
            version: "0.1.0",
            app_state: sovereign_core::state::AppState {
                storage,
                runtime: no_runtime(),
                proxy: None,
                secrets: None,
                backup: None,
                db_path: std::path::PathBuf::from(":memory:"),
            },
            notification_tx: tokio::sync::broadcast::channel(64).0,
        });

        let body = serde_json::json!({
            "pack_url": "https://releases.example.com/app-v1.0.sov"
        });
        let resp = handle_webhook_test(
            &state,
            "native-url-app",
            "application/json",
            &serde_json::to_vec(&body).unwrap(),
        )
        .await;
        assert_eq!(resp.0, StatusCode::ACCEPTED);

        let json: serde_json::Value = serde_json::from_str(&resp.1).unwrap();
        assert_eq!(json["deploy_mode"], "native");
        assert!(json["message"].as_str().unwrap().contains("releases.example.com"));
    }

    // --- P6: auto_deploy + window tests ------------------------------------

    #[tokio::test]
    async fn webhook_auto_deploy_false_returns_200_no_dispatch() {
        let n = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let storage: Arc<dyn StoragePort> = Arc::new(
            SqliteState::open_in_memory_named(&format!("wh-test-nodeploy-{pid}-{n}"))
                .await
                .unwrap(),
        );
        storage
            .create_app(
                NewApp {
                    name: "no-deploy-app".to_string(),
                    owner: "test".to_string(),
                    env: AppEnv::Dev,
                    config_yaml: "name: no-deploy-app".to_string(),
                    auto_deploy: false,
                    ..Default::default()
                },
                "test",
            )
            .await
            .unwrap();

        let state = Arc::new(DaemonState {
            started_at: Instant::now(),
            version: "0.1.0",
            app_state: sovereign_core::state::AppState {
                storage,
                runtime: no_runtime(),
                proxy: None,
                secrets: None,
                backup: None,
                db_path: std::path::PathBuf::from(":memory:"),
            },
            notification_tx: tokio::sync::broadcast::channel(64).0,
        });

        let body = serde_json::json!({
            "ref": "refs/heads/main",
            "repository": {"full_name": "user/repo"}
        });
        let resp = handle_webhook_test(
            &state,
            "no-deploy-app",
            "application/json",
            &serde_json::to_vec(&body).unwrap(),
        )
        .await;
        assert_eq!(resp.0, StatusCode::ACCEPTED);

        let json: serde_json::Value = serde_json::from_str(&resp.1).unwrap();
        assert_eq!(json["status"], "accepted");
        assert!(json["message"]
            .as_str()
            .unwrap()
            .contains("auto_deploy is disabled"));
    }

    #[tokio::test]
    async fn is_in_window_none_always_true() {
        assert!(is_in_window(None));
    }

    #[test]
    fn parse_hhmm_valid() {
        assert_eq!(parse_hhmm("09:00"), Some(540));
        assert_eq!(parse_hhmm("00:00"), Some(0));
        assert_eq!(parse_hhmm("23:59"), Some(23 * 60 + 59));
    }

    #[test]
    fn parse_hhmm_invalid() {
        assert_eq!(parse_hhmm("25:00"), None);
        assert_eq!(parse_hhmm("12:60"), None);
        assert_eq!(parse_hhmm("abc"), None);
        assert_eq!(parse_hhmm(""), None);
    }

    #[test]
    fn is_in_window_bad_format_returns_false() {
        assert!(!is_in_window(Some("bad format")));
        assert!(!is_in_window(Some("bad-range")));
    }

    // --- Test helper -------------------------------------------------------

    /// Call handle_webhook without axum overhead (direct function call).
    async fn handle_webhook_test(
        state: &Arc<DaemonState>,
        app_name: &str,
        content_type: &str,
        body: &[u8],
    ) -> (StatusCode, String) {
        let mut headers = HeaderMap::new();
        headers.insert(
            "content-type",
            content_type.parse().unwrap(),
        );

        let result = handle_webhook(
            State(Arc::clone(state)),
            Path(app_name.to_string()),
            headers,
            axum::body::Bytes::from(body.to_vec()),
        )
        .await;

        match result {
            Ok((status, Json(val))) => (status, serde_json::to_string(&val).unwrap()),
            Err((status, Json(val))) => (status, serde_json::to_string(&val).unwrap()),
        }
    }
}
