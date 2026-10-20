//! HTTP authentication flows (S7 C2/B-02): register / login / logout / me /
//! password reset, served on the platform plane. Login installs the principal's
//! access snapshot into the policy engine and issues a session cookie (browser)
//! or a hashed PAT (API client, `mode = "token"` or sessions disabled).
//!
//! Deliberate non-goals (report R5): JWT stays out of scope until a future
//! add-on; PAT + HMAC sessions are the v0.1 model.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use axum::extract::State;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use kelson_foundation::{AppError, FeatureRevision, ModuleRevision, PrincipalId, TenantId};
use kelson_http::{Envelope, ResolvedSession};
use kelson_identity::{hash_password_with, verify_credential, Principal, PrincipalStatus, Secret};
use kelson_policy::AccessSnapshot;
use serde::{Deserialize, Serialize};
use time::{Duration, OffsetDateTime};

use crate::routes::AppState;

const PAT_TTL: Duration = Duration::days(30);
const RESET_TTL: Duration = Duration::minutes(30);
/// B-23: email verification redemption window (email links need room).
const VERIFY_TTL: Duration = Duration::minutes(60);
const LOGIN_BURST: u32 = 5;
const LOGIN_REFILL_PER_SEC: f64 = 5.0 / 60.0;

// ------------------------------------------------------------ throttling

fn throttle_buckets() -> &'static Mutex<HashMap<String, kelson_http::middleware::BucketState>> {
    static BUCKETS: OnceLock<Mutex<HashMap<String, kelson_http::middleware::BucketState>>> =
        OnceLock::new();
    BUCKETS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Client IP for the credential throttle bucket. One definition, in the
/// transport crate, so login throttling and telemetry bucket the same way:
/// first `X-Forwarded-For` hop, else the `global` sentinel.
fn client_ip(headers: &HeaderMap) -> String {
    kelson_http::client_ip(headers)
}

/// Per credential-throttle bucket: IP + identifier (case-folded). Burst of 5,
/// refilling at 5/minute — brute force is expensive, legitimate retries work.
pub(crate) fn login_allowed(headers: &HeaderMap, identifier: &str) -> bool {
    let key = format!("{}|{}", client_ip(headers), identifier.to_lowercase());
    let mut buckets = throttle_buckets().lock().unwrap();
    let bucket = buckets
        .entry(key)
        .or_insert_with(|| kelson_http::middleware::BucketState {
            tokens: f64::from(LOGIN_BURST),
            updated_at: Instant::now(),
        });
    kelson_http::middleware::take_from_bucket(
        bucket,
        Instant::now(),
        LOGIN_BURST,
        LOGIN_REFILL_PER_SEC,
    )
}

// ------------------------------------------------------------ helpers

/// Wave 2 error rendering: the shared registry decides status/reportability,
/// `app.debug` adds redacted detail and per-code renderers contribute meta —
/// one choke point for every auth-plane error.
fn envelope(state: &AppState, err: &AppError) -> (StatusCode, Json<serde_json::Value>) {
    state.error_json(err)
}

fn unauthenticated() -> AppError {
    AppError::unauthenticated()
}

/// Equalized-timing dummy under the CURRENT `[identity]` factors: built once
/// per boot on first use, so an unknown principal or missing credential pays
/// the same argon2 work factor as a real converged-password check (docs/06
/// "login timing equalization" — it holds after a factor change too).
pub(crate) fn dummy_hash(state: &AppState) -> String {
    state
        .dummy_hash
        .get_or_init(|| {
            hash_password_with(
                &state.password_params,
                &Secret::new("kelson-dummy-password-for-timing"),
            )
            .expect("argon2 works")
        })
        .clone()
}

fn validate_email(email: &str) -> Result<String, AppError> {
    let email = email.trim().to_lowercase();
    let at = email.find('@').filter(|i| *i > 0);
    let dot = email.rfind('.');
    if email.len() > 254 || at.is_none() || dot.is_none_or(|d| d <= at.unwrap_or(0) + 1) {
        return Err(AppError::validation("email must be a valid address"));
    }
    Ok(email)
}

/// Wave 7 P6: register and password-reset enforce the configured
/// `[auth.password_policy]` (the same policy the `password` rule composable
/// reads), plus the hard 128-byte size cap the policy does not carry.
fn validate_password(
    password: &str,
    policy: &kelson_validation::PasswordPolicy,
) -> Result<(), AppError> {
    if password.len() > 128 {
        return Err(AppError::validation(
            "password must be 128 characters or fewer",
        ));
    }
    if let Some(failure) = policy.failure(password) {
        return Err(AppError::validation(format!("password {failure}")));
    }
    Ok(())
}

/// Tenant selector: explicit uuid, the literal `dev`, or the environment
/// default. Prod has no default (callers must be explicit).
pub(crate) fn resolve_tenant(state: &AppState, raw: Option<&str>) -> Result<TenantId, AppError> {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        None => state
            .default_tenant_id
            .ok_or_else(|| AppError::validation("tenant is required in this environment")),
        Some(value) => {
            if let Ok(id) = uuid::Uuid::parse_str(value) {
                return Ok(TenantId::from_uuid(id));
            }
            if value == "dev" {
                return state.default_tenant_id.ok_or_else(|| {
                    AppError::validation("tenant 'dev' is not configured in this environment")
                });
            }
            Err(AppError::validation("tenant must be a uuid or 'dev'"))
        }
    }
}

/// Install (or refresh) the principal's access snapshot so the policy engine
/// can authorize subsequent dispatches without re-hydration. Wave 1: the
/// field-level installer is shared with the graph edge's JWT hydration path
/// (EdgeAuth), which holds the policy engine + capability list directly.
pub(crate) fn install_access_snapshot(state: &AppState, principal: PrincipalId, tenant: TenantId) {
    install_snapshot_fields(
        &state.policy,
        &state.granted_capabilities,
        principal,
        tenant,
    );
}

/// Field-level snapshot installer (policy engine + granted capabilities).
pub(crate) fn install_snapshot_fields(
    policy: &kelson_policy::PolicyEngine,
    granted_capabilities: &[String],
    principal: PrincipalId,
    tenant: TenantId,
) {
    policy.install_snapshot(AccessSnapshot {
        principal_id: principal,
        tenant_id: tenant,
        membership_active: true,
        capabilities: granted_capabilities.iter().cloned().collect(),
        scope_paths: ["/".to_string()].into_iter().collect(),
        authorization_revision: policy.revision(),
        feature_revision: FeatureRevision(1),
        module_revision: ModuleRevision(1),
        computed_at: OffsetDateTime::now_utc(),
    });
}

#[derive(Debug, Serialize)]
struct AuthData {
    principal_id: String,
    tenant_id: String,
    mode: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
}
// ------------------------------------------------------------ endpoints

#[derive(Debug, Deserialize)]
struct RegisterBody {
    email: String,
    password: String,
    #[serde(default)]
    tenant: Option<String>,
    /// "token" also issues a PAT alongside the session.
    #[serde(default)]
    mode: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoginBody {
    /// Email identity (phone identities later).
    identifier: String,
    password: String,
    #[serde(default)]
    tenant: Option<String>,
    #[serde(default)]
    mode: Option<String>,
    /// Wave 4 Phase 7 remember-me (docs/19 L220): prolongs the issued
    /// cookie to `SessionPolicy::remember_ttl` (30 days) and records
    /// `remember: true` in the session bag.
    #[serde(default)]
    remember: bool,
}

#[derive(Debug, Deserialize)]
struct ResetRequestBody {
    email: String,
}

#[derive(Debug, Deserialize)]
struct ResetCompleteBody {
    token: String,
    password: String,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
        .route("/auth/confirm", post(confirm))
        .route("/auth/password-reset/request", post(reset_request))
        .route("/auth/password-reset/complete", post(reset_complete))
        .route(
            "/auth/email-verification/request",
            post(verification_request),
        )
        .route(
            "/auth/email-verification/confirm",
            post(verification_confirm),
        )
        .with_state(state)
}

fn principal_json(p: &Principal) -> serde_json::Value {
    serde_json::json!({
        "principal_id": p.id.to_string(),
        "email": p.email,
        "email_verified_at": p.email_verified_at.map(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default()
        }),
        "status": match p.status {
            PrincipalStatus::Active => "active",
            PrincipalStatus::Suspended => "suspended",
            PrincipalStatus::Deleted => "deleted",
        },
    })
}

/// Complete the credential handshake shared by register + login: snapshot
/// install, then session cookie and/or PAT issuance. `remember` (login only)
/// prolongs the cookie to the policy's remember window and records the flag
/// in the session bag (Wave 4 Phase 7).
fn finish_login(
    state: &AppState,
    principal: PrincipalId,
    tenant: TenantId,
    want_token: bool,
    remember: bool,
    incoming: Option<&ResolvedSession>,
) -> Result<(AuthData, Option<axum::http::HeaderValue>), AppError> {
    install_access_snapshot(state, principal, tenant);
    // Session-fixation defense: any session the client presented BEFORE
    // authenticating (possibly attacker-planted) is destroyed before the
    // fresh authenticated session id is issued.
    if let Some(mw) = &state.sessions {
        if let Some(resolved) = incoming {
            let _ = mw.manager.destroy(&resolved.session.id);
        }
    }
    let mut data = AuthData {
        principal_id: principal.to_string(),
        tenant_id: tenant.to_string(),
        mode: "session",
        token: None,
    };
    let mut cookie = None;
    if let Some(mw) = &state.sessions {
        let session = mw
            .manager
            .start(principal, Some(tenant), mw.now())?;
        if remember {
            // Bag first: the cookie value is id-only, so ordering is free.
            mw.manager
                .put(&session.id, "remember", serde_json::json!(true))?;
            cookie = Some(mw.issue_cookie_with_max_age(
                &session,
                mw.manager.policy().remember_ttl,
            )?);
        } else {
            cookie = Some(mw.issue_cookie(&session)?);
        }
        if want_token {
            let issued = state.tokens.issue(
                principal,
                kelson_tokens::TokenPurpose::Api,
                PAT_TTL,
                Some(format!("tenant:{}", tenant)),
            )?;
            data.mode = "session+token";
            data.token = Some(issued.plaintext);
        }
    } else {
        let issued = state.tokens.issue(
            principal,
            kelson_tokens::TokenPurpose::Api,
            PAT_TTL,
            Some(format!("tenant:{}", tenant)),
        )?;
        data.mode = "token";
        data.token = Some(issued.plaintext);
    }
    Ok((data, cookie))
}

async fn register(
    State(state): State<AppState>,
    session: Option<axum::extract::Extension<ResolvedSession>>,
    Json(body): Json<RegisterBody>,
) -> impl IntoResponse {
    if !state.registration_open {
        let err = AppError::denied("registration is disabled in this environment");
        return envelope(&state, &err).into_response();
    }
    let result = (|| -> Result<(AuthData, Option<axum::http::HeaderValue>), AppError> {
        let email = validate_email(&body.email)?;
        validate_password(&body.password, &state.password_policy)?;
        if state.principals.by_email(&email)?.is_some() {
            return Err(AppError::conflict(
                "an account with this email already exists",
            ));
        }
        let now = OffsetDateTime::now_utc();
        let principal = state.principals.create(Principal {
            id: PrincipalId::new(),
            status: PrincipalStatus::Active,
            email: Some(email.clone()),
            phone: None,
            email_verified_at: None,
            created_at: now,
        })?;
        let hash = hash_password_with(&state.password_params, &Secret::new(&body.password))?;
        state.credentials.create(kelson_identity::Credential {
            id: uuid::Uuid::now_v7(),
            principal_id: principal.id,
            kind: kelson_identity::CredentialKind::PasswordArgon2,
            secret_hash: Secret::new(hash),
            created_at: now,
            expires_at: None,
        })?;
        let tenant = resolve_tenant(&state, body.tenant.as_deref())?;
        finish_login(
            &state,
            principal.id,
            tenant,
            body.mode.as_deref() == Some("token"),
            false,
            session.as_deref(),
        )
    })();
    match result {
        Ok((data, cookie)) => {
            let mut res = (
                StatusCode::OK,
                Json(serde_json::to_value(Envelope::ok(data)).unwrap_or_default()),
            )
                .into_response();
            if let Some(cookie) = cookie {
                res.headers_mut().append(header::SET_COOKIE, cookie);
            }
            res
        }
        Err(err) => envelope(&state, &err).into_response(),
    }
}

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    session: Option<axum::extract::Extension<ResolvedSession>>,
    Json(body): Json<LoginBody>,
) -> impl IntoResponse {
    let identifier = body.identifier.trim().to_lowercase();
    if !login_allowed(&headers, &identifier) {
        let err = AppError::cap_limit("too many login attempts; retry later");
        return envelope(&state, &err).into_response();
    }
    let result = (|| -> Result<(AuthData, Option<axum::http::HeaderValue>), AppError> {
        let principal = match state.principals.by_email(&identifier)? {
            Some(p) => p,
            None => {
                // Equalized timing: verify against a dummy hash so unknown
                // principals do not answer faster than bad passwords.
                let _ = kelson_identity::verify_password(&Secret::new(&body.password), &dummy_hash(&state));
                return Err(unauthenticated());
            }
        };
        let credential = match state.credentials.password_for(principal.id)? {
            Some(c) => c,
            None => {
                let _ = kelson_identity::verify_password(&Secret::new(&body.password), &dummy_hash(&state));
                return Err(unauthenticated());
            }
        };
        if !verify_credential(&credential, &Secret::new(&body.password))? {
            return Err(unauthenticated());
        }
        if !principal.is_valid() {
            return Err(AppError::denied("principal is suspended"));
        }
        let tenant = resolve_tenant(&state, body.tenant.as_deref())?;
        // Wave 7 P5 — argon2 rehash-on-login: the stored verifier's factors
        // are embedded in its PHC string; when they drift from the current
        // `[identity]` config, this successful login re-hashes the password
        // and swaps the verifier. Best effort on purpose: an upgrade hiccup
        // never blocks an authenticated login — the next login retries the
        // same healing step.
        if let Some(stored) = kelson_identity::password_params_of(credential.secret_hash.expose()) {
            if stored != state.password_params {
                let healed = hash_password_with(&state.password_params, &Secret::new(&body.password))
                    .map(Secret::new)
                    .and_then(|fresh| state.credentials.replace_password_hash(principal.id, fresh));
                if let Err(err) = healed {
                    tracing::warn!(
                        %err,
                        "argon2 rehash-on-login failed; the verifier upgrades on the next login"
                    );
                }
            }
        }
        finish_login(
            &state,
            principal.id,
            tenant,
            body.mode.as_deref() == Some("token"),
            body.remember,
            session.as_deref(),
        )
    })();
    match result {
        Ok((data, cookie)) => {
            let mut res = (
                StatusCode::OK,
                Json(serde_json::to_value(Envelope::ok(data)).unwrap_or_default()),
            )
                .into_response();
            if let Some(cookie) = cookie {
                res.headers_mut().append(header::SET_COOKIE, cookie);
            }
            res
        }
        Err(err) => envelope(&state, &err).into_response(),
    }
}

async fn logout(
    State(state): State<AppState>,
    headers: HeaderMap,
    session: Option<axum::extract::Extension<ResolvedSession>>,
) -> impl IntoResponse {
    let mut cleared = None;
    if let Some(resolved) = session {
        if let Some(mw) = &state.sessions {
            let _ = mw.manager.destroy(&resolved.session.id);
            cleared = mw.clear_cookie().ok();
        }
    }
    if let Some(token) = crate::graph_routes::bearer_token(&headers) {
        if let Ok(auth_token) = state.tokens.verify(kelson_tokens::TokenPurpose::Api, token) {
            let _ = state.tokens.revoke(auth_token.id);
        }
    }
    if cleared.is_none() && state.sessions.is_some() {
        cleared = state
            .sessions
            .as_ref()
            .and_then(|mw| mw.clear_cookie().ok());
    }
    let mut res = (
        StatusCode::OK,
        Json(
            serde_json::to_value(Envelope::ok(serde_json::json!({ "logged_out": true })))
                .unwrap_or_default(),
        ),
    )
        .into_response();
    if let Some(cookie) = cleared {
        res.headers_mut().append(header::SET_COOKIE, cookie);
    }
    res
}

async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
    session: Option<axum::extract::Extension<ResolvedSession>>,
) -> impl IntoResponse {
    let result = (|| -> Result<serde_json::Value, AppError> {
        let (principal_id, tenant_id) =
            crate::graph_routes::resolve_identity(&state, &headers, session.as_deref())?;
        let principal = state
            .principals
            .by_id(principal_id)?
            .ok_or_else(unauthenticated)?;
        // Wave 4 Phase 7: report the remember-me bag flag (false without a
        // session — bearer-only callers never have one).
        let remember = session
            .as_deref()
            .map(|resolved| resolved.session.remembered())
            .unwrap_or(false);
        Ok(serde_json::json!({
            "principal": principal_json(&principal),
            "tenant_id": tenant_id.map(|t| t.to_string()),
            "remember": remember,
        }))
    })();
    match result {
        Ok(data) => (
            StatusCode::OK,
            Json(serde_json::to_value(Envelope::ok(data)).unwrap_or_default()),
        )
            .into_response(),
        Err(err) => envelope(&state, &err).into_response(),
    }
}

// ------------------------------------------- password-confirm (Wave 4 P7)

#[derive(Debug, Deserialize)]
struct ConfirmBody {
    password: String,
}

/// Wave 4 Phase 7 (docs/19 L221): re-verify the CURRENT password and stamp
/// `confirmed_at` into the session bag; `confirmed`-chained routes then
/// admit for `auth.confirm_timeout_secs` (default 15 min). Session cookie
/// required — a bearer-only principal has no bag to stamp, so the gate stays
/// fail-closed for them.
async fn confirm(
    State(state): State<AppState>,
    headers: HeaderMap,
    session: Option<axum::extract::Extension<ResolvedSession>>,
    Json(body): Json<ConfirmBody>,
) -> impl IntoResponse {
    let result = (|| -> Result<serde_json::Value, AppError> {
        let resolved = session.ok_or_else(unauthenticated)?;
        let mw = state.sessions.as_ref().ok_or_else(unauthenticated)?;
        let principal_id = resolved.session.principal;
        // Brute force: the same credential throttle as login, keyed per
        // principal (this endpoint only re-verifies known sessions).
        if !login_allowed(&headers, &principal_id.to_string()) {
            return Err(AppError::cap_limit(
                "too many confirmation attempts; retry later",
            ));
        }
        let credential = state
            .credentials
            .password_for(principal_id)?
            .ok_or_else(unauthenticated)?;
        if !verify_credential(&credential, &Secret::new(&body.password))? {
            return Err(unauthenticated());
        }
        let now = OffsetDateTime::now_utc();
        mw.manager.put(
            &resolved.session.id,
            "confirmed_at",
            serde_json::json!(
                now.format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default()
            ),
        )?;
        Ok(serde_json::json!({ "confirmed": true }))
    })();
    match result {
        Ok(data) => (
            StatusCode::OK,
            Json(serde_json::to_value(Envelope::ok(data)).unwrap_or_default()),
        )
            .into_response(),
        Err(err) => envelope(&state, &err).into_response(),
    }
}

#[derive(Debug, Serialize)]
struct ResetRequestData {
    sent: bool,
    /// Dev/test only (non-prod): the plaintext — the mail channel delivers
    /// the same token (Row 19).
    #[serde(skip_serializing_if = "Option::is_none")]
    dev_token: Option<String>,
}

/// Row 19: deliver a token mail through the edge's configured mail port
/// (`AppState::mail`, the `[mail]` failover chain built at boot). SMTP is
/// blocking, so the send runs on the blocking pool and the handler awaits
/// it — the sync mailer posture: the response never precedes delivery, so
/// a failed send surfaces to the caller's retry instead of being lost.
/// Delivery failure never changes the response: the request stays
/// enumeration-safe (a transport error must not reveal whether the account
/// exists). With no port configured the token simply is not delivered —
/// dev/test keep the `dev_token` surface.
async fn deliver_token_mail(state: &AppState, to: &str, subject: &str, body: &str) {
    let Some(port) = state.mail.clone() else {
        tracing::debug!(target: "auth::mail", %to, "no mail port configured; token not delivered");
        return;
    };
    let msg = kelson_notifications::NotifyMessage {
        channel: kelson_notifications::Channel::Email,
        to: to.to_string(),
        template: body.to_string(),
        vars: std::collections::BTreeMap::new(),
        tenant_id: state.default_tenant_id,
        locale: None,
    };
    let subject = subject.to_string();
    match tokio::task::spawn_blocking(move || port.send(&msg, &subject)).await {
        Ok(Ok(())) => tracing::info!(target: "auth::mail", %to, "token mail delivered"),
        Ok(Err(error)) => tracing::warn!(target: "auth::mail", %to, %error, "token mail delivery failed"),
        Err(error) => tracing::warn!(target: "auth::mail", %to, %error, "token mail delivery task failed"),
    }
}

async fn reset_request(
    State(state): State<AppState>,
    Json(body): Json<ResetRequestBody>,
) -> impl IntoResponse {
    let email = body.email.trim().to_lowercase();
    let mut dev_token = None;
    // Enumeration-safe: always report sent, even for unknown emails.
    if let Ok(Some(principal)) = state.principals.by_email(&email) {
        if let Ok(issued) = state.tokens.issue(
            principal.id,
            kelson_tokens::TokenPurpose::Reset,
            RESET_TTL,
            None,
        ) {
            let body_text = format!(
                "Someone requested a password reset for this address.\n\n\
                 Reset token (single use, valid {} minutes):\n\n{}\n\n\
                 If you did not request this, ignore this message.",
                RESET_TTL.whole_minutes(),
                issued.plaintext
            );
            deliver_token_mail(&state, &email, "Reset your password", &body_text).await;
            if !state.env.is_prod() && !matches!(state.env, kelson_config::Environment::Staging) {
                dev_token = Some(issued.plaintext);
            }
        }
    }
    (
        StatusCode::OK,
        Json(
            serde_json::to_value(Envelope::ok(ResetRequestData {
                sent: true,
                dev_token,
            }))
            .unwrap_or_default(),
        ),
    )
}

/// Close every credential that was issued under the OLD password, after a
/// successful reset.
///
/// Two kinds of credential can outlive a password change, and both are a
/// takeover path when the reset was prompted by a compromise:
///
/// - the **API tokens** (a bearer credential — it never asks for the
///   password again), and
/// - the **sessions** (the cookie is valid until it expires).
///
/// Both are revoked here. The revocation is best-effort per kind: a store
/// that cannot enumerate (the trait's default) reports 0, and the reset still
/// succeeds — the password change itself is the security boundary, and the
/// revocation closes the extra doors.
fn revoke_credentials_after_reset(state: &AppState, principal: kelson_foundation::PrincipalId) {
    // The API tokens: every token the principal holds is revoked.
    match state.tokens.list(principal) {
        Ok(tokens) => {
            for token in tokens {
                if let Err(err) = state.tokens.revoke(token.id) {
                    tracing::warn!(
                        error = %err.public_message,
                        "a token could not be revoked after the password reset"
                    );
                }
            }
        }
        Err(err) => tracing::warn!(
            error = %err.public_message,
            "the principal's tokens could not be listed after the password reset"
        ),
    }
    // The sessions: every session row the principal holds is deleted.
    if let Some(sessions) = &state.sessions {
        match sessions
            .manager
            .store()
            .delete_principal_sessions(principal)
        {
            Ok(n) if n > 0 => tracing::info!(
                principal = %principal,
                sessions = n,
                "the sessions issued under the old password were revoked"
            ),
            Ok(_) => {}
            Err(err) => tracing::warn!(
                error = %err.public_message,
                "the principal's sessions could not be revoked after the password reset"
            ),
        }
    }
}

async fn reset_complete(
    State(state): State<AppState>,
    Json(body): Json<ResetCompleteBody>,
) -> impl IntoResponse {
    let result = (|| -> Result<serde_json::Value, AppError> {
        // Wave 7 P6: the policy gates before anything is spent — a weak
        // password never consumes the reset token (re-requesting is the
        // user's only recovery step otherwise).
        validate_password(&body.password, &state.password_policy)?;
        let token = state
            .tokens
            .consume(kelson_tokens::TokenPurpose::Reset, &body.token)?;
        let hash = hash_password_with(&state.password_params, &Secret::new(&body.password))?;
        state
            .credentials
            .replace_password_hash(token.principal_id, Secret::new(hash))?;
        // A reset must CLOSE the old credentials, not just change the
        // password: whoever holds a session cookie or an API token issued
        // under the old password would stay signed in after the owner
        // recovered the account. The API tokens go first (a bearer token
        // survives the password change by itself), then the sessions.
        revoke_credentials_after_reset(&state, token.principal_id);
        Ok(serde_json::json!({ "changed": true }))
    })();
    match result {
        Ok(data) => (
            StatusCode::OK,
            Json(serde_json::to_value(Envelope::ok(data)).unwrap_or_default()),
        )
            .into_response(),
        Err(err) => envelope(&state, &err).into_response(),
    }
}

// ----------------------------------------------------- email verification (B-23)

#[derive(Debug, Deserialize)]
struct VerifyRequestBody {
    email: String,
}

#[derive(Debug, Deserialize)]
struct VerifyConfirmBody {
    token: String,
}

#[derive(Debug, Serialize)]
struct VerifyRequestData {
    sent: bool,
    /// Dev/test only (non-prod): the plaintext — the mail channel delivers
    /// the same token (Row 19).
    #[serde(skip_serializing_if = "Option::is_none")]
    dev_token: Option<String>,
}

/// B-23 step 1: issue a single-use email verification token and deliver it
/// through the configured mail port (Row 19); dev/test also surfaces it
/// directly. Enumeration-safe: always reports sent.
async fn verification_request(
    State(state): State<AppState>,
    Json(body): Json<VerifyRequestBody>,
) -> impl IntoResponse {
    let email = body.email.trim().to_lowercase();
    let mut dev_token = None;
    if let Ok(Some(principal)) = state.principals.by_email(&email) {
        if let Ok(issued) = state.tokens.issue(
            principal.id,
            kelson_tokens::TokenPurpose::Verify,
            VERIFY_TTL,
            None,
        ) {
            let body_text = format!(
                "Confirm this address is yours.\n\n\
                 Verification token (single use, valid {} minutes):\n\n{}",
                VERIFY_TTL.whole_minutes(),
                issued.plaintext
            );
            deliver_token_mail(&state, &email, "Verify your email address", &body_text).await;
            if !state.env.is_prod() && !matches!(state.env, kelson_config::Environment::Staging) {
                dev_token = Some(issued.plaintext);
            }
        }
    }
    (
        StatusCode::OK,
        Json(
            serde_json::to_value(Envelope::ok(VerifyRequestData {
                sent: true,
                dev_token,
            }))
            .unwrap_or_default(),
        ),
    )
}

/// B-23 step 2: redeem the token once and record contact proof on the
/// principal. Fails on unknown/expired/already-used tokens.
async fn verification_confirm(
    State(state): State<AppState>,
    Json(body): Json<VerifyConfirmBody>,
) -> impl IntoResponse {
    let result = (|| -> Result<serde_json::Value, AppError> {
        let token = state
            .tokens
            .consume(kelson_tokens::TokenPurpose::Verify, &body.token)?;
        state.principals.mark_email_verified(token.principal_id)?;
        Ok(serde_json::json!({ "verified": true }))
    })();
    match result {
        Ok(data) => (
            StatusCode::OK,
            Json(serde_json::to_value(Envelope::ok(data)).unwrap_or_default()),
        )
            .into_response(),
        Err(err) => envelope(&state, &err).into_response(),
    }
}
