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
    pub throttle: BTreeMap<String, ThrottleGroupConfig>,
    pub buckets: Arc<Mutex<HashMap<String, BucketState>>>,
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
            throttle: throttle
                .iter()
                .map(|(g, c)| (g.to_string(), c.clone()))
                .collect(),
            buckets: Arc::new(Mutex::new(HashMap::new())),
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
            throttle: BTreeMap::new(),
            buckets: Arc::new(Mutex::new(HashMap::new())),
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
            throttle: BTreeMap::new(),
            buckets: Arc::new(Mutex::new(HashMap::new())),
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
            throttle: BTreeMap::new(),
            buckets: Arc::new(Mutex::new(HashMap::new())),
        });
        let mut req = get_req();
        req.headers_mut()
            .insert(header::AUTHORIZATION, "Bearer good-pat".parse().unwrap());
        let res = app(rt).oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
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
}
