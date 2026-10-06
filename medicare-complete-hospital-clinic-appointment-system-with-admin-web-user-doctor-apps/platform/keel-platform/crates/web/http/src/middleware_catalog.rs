//! Named middleware chain runtime (Wave 1, Laravel `$middlewareGroups`
//! parity).
//!
//! The pure catalog half (aliases, groups, throttle configs, chain
//! resolution, `middleware.toml` loading, the `GRAPH-MW-UNKNOWN` gate) lives
//! in `keel_registry::middleware_catalog` so the boot gates sit with the
//! other GRAPH-* gates. This module is the axum runtime that executes a
//! resolved chain per route:
//! - `auth` (session extension -> bearer PAT/JWT via [`EdgeAuthPort`],
//!   installs [`ResolvedPrincipal`], else 401 envelope)
//! - `verified` (B-23 email gate, 403 `AUTH_DENIED` when the config requires
//!   it)
//! - `confirmed` (Wave 4 Phase 7 password-confirm: a session stamped by
//!   `POST /auth/confirm` within `confirm_timeout_secs`, else 403
//!   `CONFIRMATION_REQUIRED` — fail-closed for bearer-only principals and
//!   missing/stale stamps)
//! - `signed` (Wave 2 signed URLs: HMAC + TTL over the request path+query,
//!   one-time `once` markers consumed through the token store; 403 on a bad
//!   signature, 404 on a replayed one-time link)
//! - `throttle:<group>` (token bucket keyed by group + client key;
//!   429 + Retry-After)
//!
//! The global admission pipeline order is frozen; per-route chains run
//! strictly inside it (attached per MethodRouter at registration time).
//! Modules cannot define custom middleware in v1 — the catalog is
//! framework-owned (documented follow-up).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, HeaderMap, Request, Response};
use axum::middleware::Next;
use axum::response::IntoResponse;
use keel_foundation::{codes, AppError, ErrorCategory, PrincipalId, Retryability, Severity, TenantId};

pub use keel_registry::middleware_catalog::{
    middleware_violations, parse_builtin_id, BuiltinMw, MiddlewareCatalog, MwViolation,
    ThrottleGroupConfig, GRAPH_MW_UNKNOWN,
};

use crate::error_response;
use crate::middleware::{client_key, take_from_bucket, BucketState};
use crate::session_mw::ResolvedSession;

// ------------------------------------------------------------------ runtime

/// How the principal was authenticated at the edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthVia {
    Session,
    Bearer,
    Jwt,
}

/// Installed by the `auth` stage; consumed by the graph dispatcher.
#[derive(Debug, Clone)]
pub struct ResolvedPrincipal {
    pub principal: PrincipalId,
    pub tenant: Option<TenantId>,
    pub via: AuthVia,
}

/// Transport-port for bearer resolution + B-23 verification checks. The API
/// runtime implements this over TokenService/PrincipalStore/JWT; keel-http
/// stays decoupled from the concrete stores.
pub trait EdgeAuthPort: Send + Sync {
    /// Resolve a bearer credential (hashed PAT first, opt-in JWT interop
    /// second). None = unauthenticated (uniform 401 at the edge).
    fn resolve_bearer(&self, token: &str) -> Option<ResolvedPrincipal>;
    /// B-23: has the principal proven contact (verified email)?
    fn email_verified(&self, principal: PrincipalId) -> bool;
}

/// Per-route chain state. Buckets are shared across routes so a named
/// throttle group is one limiter (Laravel semantics).
pub struct ChainRuntime {
    pub chain: Vec<BuiltinMw>,
    pub auth: Arc<dyn EdgeAuthPort>,
    pub require_verified_email: bool,
    /// Wave 4 Phase 7: freshness window of a `POST /auth/confirm` stamp
    /// (`auth.confirm_timeout_secs`, default 900). Zero = zero-width —
    /// fail-closed, only a same-second confirmation passes.
    pub confirm_timeout_secs: u64,
    pub throttle: BTreeMap<String, ThrottleGroupConfig>,
    pub buckets: Arc<Mutex<HashMap<String, BucketState>>>,
    /// Wave 2 signed-URL resolution: present when the deployment has an
    /// application key. A route declaring `signed` without it is refused
    /// (500, fail-closed) — an unsigned link must never pass silently.
    pub signed_urls: Option<Arc<dyn crate::SignedUrlPort>>,
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|t| !t.is_empty())
}

/// Execute a resolved middleware chain, then the handler. Stages run in
/// order (first = outermost); any stage may short-circuit with an envelope.
pub async fn chain_middleware(
    State(rt): State<Arc<ChainRuntime>>,
    mut req: Request<Body>,
    next: Next,
) -> Response<Body> {
    for mw in &rt.chain {
        match mw {
            BuiltinMw::Throttle { group } => {
                let cfg = rt
                    .throttle
                    .get(group)
                    .cloned()
                    .unwrap_or_default();
                let key = format!("{group}|{}", client_key(req.headers()));
                let allowed = {
                    let mut buckets = rt.buckets.lock().unwrap();
                    let bucket = buckets.entry(key).or_insert_with(|| BucketState {
                        tokens: cfg.capacity as f64,
                        updated_at: Instant::now(),
                    });
                    take_from_bucket(bucket, Instant::now(), cfg.capacity, cfg.refill_per_sec)
                };
                if !allowed {
                    let err = AppError::cap_limit(format!(
                        "throttle:{group} rate limit exceeded; retry shortly"
                    ));
                    let mut res = error_response(&err).into_response();
                    if let Ok(value) = "1".parse() {
                        res.headers_mut().insert(header::RETRY_AFTER, value);
                    }
                    return res;
                }
            }
            BuiltinMw::Auth => {
                if req.extensions().get::<ResolvedPrincipal>().is_some() {
                    continue;
                }
                let resolved = if let Some(session) = req.extensions().get::<ResolvedSession>() {
                    Some(ResolvedPrincipal {
                        principal: session.session.principal,
                        tenant: session.session.tenant,
                        via: AuthVia::Session,
                    })
                } else {
                    bearer_token(req.headers())
                        .and_then(|token| rt.auth.resolve_bearer(token))
                };
                match resolved {
                    Some(principal) => {
                        req.extensions_mut().insert(principal);
                    }
                    None => {
                        let err = AppError::unauthenticated();
                        return error_response(&err).into_response();
                    }
                }
            }
            BuiltinMw::Verified => {
                if !rt.require_verified_email {
                    continue;
                }
                let Some(resolved) = req.extensions().get::<ResolvedPrincipal>().cloned() else {
                    let err = AppError::unauthenticated();
                    return error_response(&err).into_response();
                };
                if !rt.auth.email_verified(resolved.principal) {
                    let err = AppError::new(
                        codes::DENIED,
                        ErrorCategory::Authorization,
                        Severity::Low,
                        Retryability::NotRetryable,
                        "email must be verified before using module operations",
                    );
                    return error_response(&err).into_response();
                }
            }
            BuiltinMw::Confirmed => {
                // Must be authenticated; the chain normally runs `auth` first.
                if req.extensions().get::<ResolvedPrincipal>().is_none() {
                    let err = AppError::unauthenticated();
                    return error_response(&err).into_response();
                }
                // Freshness is session-bag state: bearer-only principals and
                // missing/stale stamps all fail closed with 403.
                let fresh = req
                    .extensions()
                    .get::<ResolvedSession>()
                    .and_then(|rs| rs.session.data.get("confirmed_at"))
                    .and_then(|v| v.as_str())
                    .and_then(|raw| {
                        time::OffsetDateTime::parse(
                            raw,
                            &time::format_description::well_known::Rfc3339,
                        )
                        .ok()
                    })
                    .map(|stamped| {
                        let age = time::OffsetDateTime::now_utc() - stamped;
                        age.whole_seconds() <= rt.confirm_timeout_secs as i64
                    })
                    .unwrap_or(false);
                if !fresh {
                    let err = AppError::new(
                        codes::CONFIRMATION_REQUIRED,
                        ErrorCategory::Authorization,
                        Severity::Low,
                        Retryability::NotRetryable,
                        "password confirmation is required or has expired",
                    );
                    return error_response(&err).into_response();
                }
            }
            BuiltinMw::Signed => {
                let Some(port) = &rt.signed_urls else {
                    // Deployment misconfiguration, not a client error: a
                    // signed link must never pass an unconfigured verifier.
                    let err = AppError::internal(
                        "signed url verification is not configured (application key missing)",
                    );
                    return error_response(&err).into_response();
                };
                let target = req
                    .uri()
                    .path_and_query()
                    .map(|pq| pq.as_str().to_string())
                    .unwrap_or_else(|| req.uri().path().to_string());
                if let Err(err) = port.resolve(&target) {
                    return error_response(&err).into_response();
                }
            }
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::routing::get;
    use axum::Router;
    use keel_session::{InMemorySessionStore, SessionManager, SessionPolicy};
    use time::OffsetDateTime;
    use tower::ServiceExt;

    struct FakeAuth {
        verified: bool,
    }

    impl EdgeAuthPort for FakeAuth {
        fn resolve_bearer(&self, token: &str) -> Option<ResolvedPrincipal> {
            (token == "good-pat").then(|| ResolvedPrincipal {
                principal: PrincipalId::new(),
                tenant: Some(TenantId::new()),
                via: AuthVia::Bearer,
            })
        }
        fn email_verified(&self, _principal: PrincipalId) -> bool {
            self.verified
        }
    }

    fn runtime(
        chain: Vec<BuiltinMw>,
        require_verified_email: bool,
        throttle: &[(&str, ThrottleGroupConfig)],
    ) -> Arc<ChainRuntime> {
        Arc::new(ChainRuntime {
            chain,
            auth: Arc::new(FakeAuth { verified: true }),
            require_verified_email,
            confirm_timeout_secs: 900,
            throttle: throttle
                .iter()
                .map(|(g, c)| (g.to_string(), c.clone()))
                .collect(),
            buckets: Arc::new(Mutex::new(HashMap::new())),
            signed_urls: None,
        })
    }

    /// Chain runtime with a signed-URL resolver attached (Wave 2 `signed`).
    fn signed_runtime(chain: Vec<BuiltinMw>, port: Arc<dyn crate::SignedUrlPort>) -> Arc<ChainRuntime> {
        Arc::new(ChainRuntime {
            chain,
            auth: Arc::new(FakeAuth { verified: true }),
            require_verified_email: false,
            confirm_timeout_secs: 900,
            throttle: BTreeMap::new(),
            buckets: Arc::new(Mutex::new(HashMap::new())),
            signed_urls: Some(port),
        })
    }

    fn app(rt: Arc<ChainRuntime>) -> Router {
        Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(rt, chain_middleware))
    }

    fn get_req() -> Request<Body> {
        Request::builder().uri("/").body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn auth_stage_rejects_anonymous_and_accepts_bearer() {
        let rt = runtime(vec![BuiltinMw::Auth], false, &[]);
        let res = app(rt).oneshot(get_req()).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        let rt = runtime(vec![BuiltinMw::Auth], false, &[]);
        let mut req = get_req();
        req.headers_mut()
            .insert(header::AUTHORIZATION, "Bearer good-pat".parse().unwrap());
        let res = app(rt).oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn auth_stage_accepts_session_extension() {
        let manager = SessionManager::new(
            InMemorySessionStore::shared(),
            SessionPolicy::default(),
            b"unit-secret",
        );
        let session = manager
            .start(PrincipalId::new(), Some(TenantId::new()), OffsetDateTime::now_utc())
            .unwrap();
        let rt = runtime(vec![BuiltinMw::Auth], false, &[]);
        let router = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(
                rt,
                chain_middleware,
            ))
            .layer(axum::middleware::from_fn(move |mut req: Request<Body>, next: Next| {
                let session = session.clone();
                async move {
                    req.extensions_mut().insert(ResolvedSession { session });
                    next.run(req).await
                }
            }));
        let res = router.oneshot(get_req()).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn verified_stage_gates_unverified_principals() {
        let chain = vec![BuiltinMw::Auth, BuiltinMw::Verified];
        let rt = Arc::new(ChainRuntime {
            chain: chain.clone(),
            auth: Arc::new(FakeAuth { verified: false }),
            require_verified_email: true,
            confirm_timeout_secs: 900,
            throttle: BTreeMap::new(),
            buckets: Arc::new(Mutex::new(HashMap::new())),
            signed_urls: None,
        });
        let mut req = get_req();
        req.headers_mut()
            .insert(header::AUTHORIZATION, "Bearer good-pat".parse().unwrap());
        let res = app(rt).oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // Same request, verified principal -> passes.
        let rt = Arc::new(ChainRuntime {
            chain,
            auth: Arc::new(FakeAuth { verified: true }),
            require_verified_email: true,
            confirm_timeout_secs: 900,
            throttle: BTreeMap::new(),
            buckets: Arc::new(Mutex::new(HashMap::new())),
            signed_urls: None,
        });
        let mut req = get_req();
        req.headers_mut()
            .insert(header::AUTHORIZATION, "Bearer good-pat".parse().unwrap());
        let res = app(rt).oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // Gate disabled -> unverified passes.
        let rt = Arc::new(ChainRuntime {
            chain: vec![BuiltinMw::Auth, BuiltinMw::Verified],
            auth: Arc::new(FakeAuth { verified: false }),
            require_verified_email: false,
            confirm_timeout_secs: 900,
            throttle: BTreeMap::new(),
            buckets: Arc::new(Mutex::new(HashMap::new())),
            signed_urls: None,
        });
        let mut req = get_req();
        req.headers_mut()
            .insert(header::AUTHORIZATION, "Bearer good-pat".parse().unwrap());
        let res = app(rt).oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    /// Wave 4 Phase 7 (docs/19 L221): `confirmed` admits only a session
    /// stamped by `POST /auth/confirm` inside the window. Anonymous -> 401;
    /// bearer-only, unstamped, and stale stamps -> 403 CONFIRMATION_REQUIRED.
    #[tokio::test]
    async fn confirmed_stage_enforces_the_password_confirmation_window() {
        let rt = Arc::new(ChainRuntime {
            chain: vec![BuiltinMw::Auth, BuiltinMw::Confirmed],
            auth: Arc::new(FakeAuth { verified: true }),
            require_verified_email: false,
            confirm_timeout_secs: 900,
            throttle: BTreeMap::new(),
            buckets: Arc::new(Mutex::new(HashMap::new())),
            signed_urls: None,
        });

        // Anonymous -> the auth stage short-circuits with 401.
        let res = app(rt.clone()).oneshot(get_req()).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

        // Bearer-authenticated but no session bag -> fail-closed 403.
        let mut req = get_req();
        req.headers_mut()
            .insert(header::AUTHORIZATION, "Bearer good-pat".parse().unwrap());
        let res = app(rt.clone()).oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        assert_eq!(error_code(res).await, "CONFIRMATION_REQUIRED");

        // Session stamped inside the window -> passes.
        let now = OffsetDateTime::now_utc();
        let mut req = get_req();
        req.extensions_mut().insert(ResolvedSession {
            session: session_stamped(now),
        });
        let res = app(rt.clone()).oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // Session stamped before the window (901s > 900s) -> 403.
        let mut req = get_req();
        req.extensions_mut().insert(ResolvedSession {
            session: session_stamped(now - time::Duration::seconds(901)),
        });
        let res = app(rt.clone()).oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        assert_eq!(error_code(res).await, "CONFIRMATION_REQUIRED");
    }

    /// A live session whose bag carries `confirmed_at` at the given instant.
    fn session_stamped(at: OffsetDateTime) -> keel_session::Session {
        let manager = SessionManager::new(
            InMemorySessionStore::shared(),
            SessionPolicy::default(),
            b"unit-secret",
        );
        let mut session = manager
            .start(PrincipalId::new(), Some(TenantId::new()), at)
            .unwrap();
        session.data.insert(
            "confirmed_at".into(),
            serde_json::json!(
                at.format(&time::format_description::well_known::Rfc3339)
                    .expect("rfc3339")
            ),
        );
        session
    }

    async fn error_code(res: axum::response::Response) -> String {
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        body["error"]["code"].as_str().unwrap_or("").to_string()
    }

    #[tokio::test]
    async fn throttle_stage_exhausts_and_runs_before_auth() {
        let cfg = ThrottleGroupConfig {
            capacity: 2,
            refill_per_sec: 0.0,
        };
        // Chain with throttle FIRST and no credentials: exhaustion must be a
        // 429, not a 401 (ordering proof).
        let rt = runtime(
            vec![
                BuiltinMw::Throttle { group: "api".into() },
                BuiltinMw::Auth,
            ],
            false,
            &[("api", cfg)],
        );
        let router = app(rt);
        for _ in 0..2 {
            let res = router.clone().oneshot(get_req()).await.unwrap();
            assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        }
        let res = router.oneshot(get_req()).await.unwrap();
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(res.headers().get(header::RETRY_AFTER).unwrap(), "1");
    }

    #[tokio::test]
    async fn throttle_groups_have_isolated_buckets() {
        let cfg = ThrottleGroupConfig {
            capacity: 1,
            refill_per_sec: 0.0,
        };
        let rt = runtime(
            vec![
                BuiltinMw::Throttle { group: "a".into() },
                BuiltinMw::Throttle { group: "b".into() },
                BuiltinMw::Auth,
            ],
            false,
            &[("a", cfg.clone()), ("b", cfg)],
        );
        let router = app(rt);
        // Both groups let the first request through to auth (401, not 429).
        let res = router.clone().oneshot(get_req()).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        // Second request: group a is exhausted.
        let res = router.oneshot(get_req()).await.unwrap();
        assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    // ------------------------------------ Wave 2 `signed` (19-PARITY URLs)

    const SIGNED_SECRET: &[u8] = b"unit-signed-url-secret-0123456789";

    fn gate() -> Arc<crate::SignedGate> {
        let tokens = Arc::new(keel_tokens::TokenService::new(Arc::new(
            keel_tokens::InMemoryTokenStore::new(),
        )));
        Arc::new(crate::SignedGate::new(SIGNED_SECRET.to_vec(), tokens))
    }

    fn uri_req(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn signed_stage_redempts_a_one_time_link_exactly_once() {
        let gate = gate();
        let link = gate
            .sign_once(
                PrincipalId::new(),
                "/?campaign=welcome",
                time::Duration::minutes(5),
            )
            .expect("one-time link mints");

        // First redemption reaches the handler.
        let rt = signed_runtime(vec![BuiltinMw::Signed], gate.clone());
        let res = app(rt).oneshot(uri_req(&link)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // Replay of the very same URL is spent: 404 (410/404 per plan).
        let rt = signed_runtime(vec![BuiltinMw::Signed], gate);
        let res = app(rt).oneshot(uri_req(&link)).await.unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let envelope: serde_json::Value = serde_json::from_slice(&body).expect("error envelope");
        assert_eq!(envelope["ok"], false);
        assert_eq!(envelope["error"]["code"], "KER_NOT_FOUND");
    }

    #[tokio::test]
    async fn signed_stage_rejects_tampered_and_unconfigured_links() {
        // Multi-use signed URL: verifies, and a flipped parameter fails.
        let urls = crate::SignedUrls::new(SIGNED_SECRET.to_vec());
        let link = urls.sign("/?x=1", Some(time::Duration::minutes(5)));
        let rt = signed_runtime(vec![BuiltinMw::Signed], gate());
        let res = app(rt).oneshot(uri_req(&link)).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        let tampered = link.replace("x=1", "x=2");
        let rt = signed_runtime(vec![BuiltinMw::Signed], gate());
        let res = app(rt).oneshot(uri_req(&tampered)).await.unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // A route declaring `signed` with no verifier configured fails closed.
        let rt = runtime(vec![BuiltinMw::Signed], false, &[]);
        let res = app(rt).oneshot(uri_req(&link)).await.unwrap();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
