//! Session middleware (S7 C6): binds `keel-session` to the HTTP edge.
//!
//! Request flow: parse the `keel_session` cookie (session id + HMAC-style tag
//! over the server secret) -> `SessionManager::touch` -> insert a
//! `ResolvedSession` extension for downstream handlers. Unknown/forged/expired
//! cookies are treated as anonymous (never an error); the login/logout edge
//! owns Set-Cookie issuance and clearing.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderValue, Request, Response};
use axum::middleware::Next;
use keel_session::{Session, SessionId, SessionManager};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::cookie::{Cookie, CookieJar, SameSite};

/// Browser session cookie name.
pub const SESSION_COOKIE: &str = "keel_session";

fn hex(bytes: &[u8]) -> String {
    const NIBBLES: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(NIBBLES[(b >> 4) as usize] as char);
        out.push(NIBBLES[(b & 0xf) as usize] as char);
    }
    out
}

/// HMAC-style tag over the session id: useless to forge without the secret.
fn tag(session_id: &str, secret: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret);
    hasher.update(b"|keel-session-v1|");
    hasher.update(session_id.as_bytes());
    hex(&hasher.finalize())
}

/// Wire value of the session cookie: `<id>.<tag>`.
pub fn session_cookie_value(session: &Session, secret: &[u8]) -> String {
    format!(
        "{}.{}",
        session.id.as_str(),
        tag(session.id.as_str(), secret)
    )
}

/// Parse + verify a session cookie value. The tag comparison is
/// constant-time so a forged tag cannot be byte-guessed via timing.
pub fn parse_session_cookie(raw: &str, secret: &[u8]) -> Option<SessionId> {
    let (id, provided) = raw.split_once('.')?;
    let expected = tag(id, secret);
    if !crate::signed::constant_time_eq(expected.as_bytes(), provided.as_bytes()) {
        return None;
    }
    SessionId::parse(id).ok()
}

/// Request extension: the session resolved by the middleware (if any).
#[derive(Debug, Clone)]
pub struct ResolvedSession {
    pub session: Session,
}

/// Session edge posture. `secure` controls the Secure flag on issued cookies
/// (prod); `cookie_ttl_secs` bounds Max-Age (0 = session cookie, no Max-Age).
#[derive(Clone)]
pub struct SessionMiddleware {
    pub manager: Arc<SessionManager>,
    pub secret: Vec<u8>,
    pub secure: bool,
    pub cookie_ttl_secs: u64,
}

impl SessionMiddleware {
    pub fn new(manager: Arc<SessionManager>, secret: impl AsRef<[u8]>) -> Self {
        Self {
            manager,
            secret: secret.as_ref().to_vec(),
            secure: false,
            cookie_ttl_secs: 0,
        }
    }

    pub fn with_secure(mut self, secure: bool) -> Self {
        self.secure = secure;
        self
    }

    pub fn with_cookie_ttl_secs(mut self, secs: u64) -> Self {
        self.cookie_ttl_secs = secs;
        self
    }

    /// Attach to a router. Runs INSIDE Admission (request-id/trace exist) and
    /// BEFORE handlers; CSRF stays as its own layer.
    pub fn apply(self, router: axum::Router) -> axum::Router {
        router.layer(axum::middleware::from_fn_with_state(self, session_fn))
    }

    /// Set-Cookie header for a fresh/rotated session (login, regenerate).
    pub fn issue_cookie(&self, session: &Session) -> Result<HeaderValue, keel_foundation::AppError> {
        self.cookie_header(session, None)
    }

    /// Wave 4 Phase 7 remember-me (docs/19 L220): same cookie with an
    /// explicit `Max-Age` — login passes `SessionPolicy::remember_ttl` (the
    /// 30 day window) instead of the middleware's default cookie TTL (8h).
    pub fn issue_cookie_with_max_age(
        &self,
        session: &Session,
        max_age_secs: u64,
    ) -> Result<HeaderValue, keel_foundation::AppError> {
        self.cookie_header(session, Some(max_age_secs))
    }

    fn cookie_header(
        &self,
        session: &Session,
        max_age_secs: Option<u64>,
    ) -> Result<HeaderValue, keel_foundation::AppError> {
        let mut cookie = Cookie::new(SESSION_COOKIE, session_cookie_value(session, &self.secret))?
            .same_site(SameSite::Lax)
            .secure(self.secure);
        let ttl = max_age_secs.unwrap_or(self.cookie_ttl_secs);
        if ttl > 0 {
            cookie = cookie.max_age_secs(ttl as i64);
        }
        cookie.header_value()
    }

    /// Set-Cookie header that clears the session cookie (logout).
    pub fn clear_cookie(&self) -> Result<HeaderValue, keel_foundation::AppError> {
        Cookie::expire(SESSION_COOKIE, self.secure)?.header_value()
    }
}

pub(crate) async fn session_fn(
    State(mw): State<SessionMiddleware>,
    mut req: Request<Body>,
    next: Next,
) -> Response<Body> {
    let jar = CookieJar::from_headers(req.headers());
    if let Some(raw) = jar.get(SESSION_COOKIE) {
        if let Some(id) = parse_session_cookie(raw, &mw.secret) {
            let now = OffsetDateTime::now_utc();
            if let Ok(Some(session)) = mw.manager.touch(&id, now) {
                req.extensions_mut().insert(ResolvedSession { session });
            }
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Extension;
    use axum::routing::get;
    use axum::Router;
    use keel_foundation::PrincipalId;
    use keel_session::{InMemorySessionStore, SessionPolicy};
    use tower::ServiceExt;

    fn app() -> (Router, Arc<SessionManager>) {
        let manager = Arc::new(SessionManager::new(
            InMemorySessionStore::shared(),
            SessionPolicy::default(),
            b"unit-secret",
        ));
        let mw = SessionMiddleware::new(manager.clone(), b"cookie-secret");
        let router = Router::new().route(
            "/whoami",
            get(|Extension(rs): Extension<ResolvedSession>| async move {
                rs.session.principal.to_string()
            }),
        );
        (mw.apply(router), manager)
    }

    fn get_req(cookie: Option<String>) -> Request<Body> {
        let mut builder = axum::http::Request::builder().uri("/whoami");
        if let Some(c) = cookie {
            builder = builder.header(axum::http::header::COOKIE, c);
        }
        builder.body(Body::empty()).unwrap()
    }

    #[tokio::test]
    async fn no_cookie_is_anonymous() {
        let (router, _) = app();
        // No ResolvedSession extension -> handler rejects with 500 (missing
        // extension). The middleware must NOT error: it simply does not insert.
        let res = router.oneshot(get_req(None)).await.unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn valid_cookie_resolves_session() {
        let (router, manager) = app();
        let session = manager
            .start(PrincipalId::new(), None, OffsetDateTime::now_utc())
            .unwrap();
        let value = session_cookie_value(&session, b"cookie-secret");
        let res = router
            .oneshot(get_req(Some(format!("keel_session={value}"))))
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
    }

    #[tokio::test]
    async fn forged_tag_is_anonymous() {
        let (router, manager) = app();
        let session = manager
            .start(PrincipalId::new(), None, OffsetDateTime::now_utc())
            .unwrap();
        let forged = format!("{}.deadbeef", session.id.as_str());
        let res = router
            .oneshot(get_req(Some(format!("keel_session={forged}"))))
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn destroyed_session_is_anonymous() {
        let (router, manager) = app();
        let session = manager
            .start(PrincipalId::new(), None, OffsetDateTime::now_utc())
            .unwrap();
        let value = session_cookie_value(&session, b"cookie-secret");
        manager.destroy(&session.id).unwrap();
        let res = router
            .oneshot(get_req(Some(format!("keel_session={value}"))))
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn cookie_value_roundtrip_and_rejects() {
        let manager = SessionManager::new(
            InMemorySessionStore::shared(),
            SessionPolicy::default(),
            b"unit-secret",
        );
        let session = manager
            .start(PrincipalId::new(), None, OffsetDateTime::now_utc())
            .unwrap();
        let value = session_cookie_value(&session, b"cookie-secret");
        assert_eq!(
            parse_session_cookie(&value, b"cookie-secret"),
            Some(session.id.clone())
        );
        assert_eq!(parse_session_cookie(&value, b"other-secret"), None);
        assert_eq!(parse_session_cookie("garbage", b"cookie-cookie"), None);
        assert_eq!(parse_session_cookie("zz.deadbeef", b"x"), None);
    }

    #[test]
    fn issue_and_clear_cookies_carry_flags() {
        let manager = Arc::new(SessionManager::new(
            InMemorySessionStore::shared(),
            SessionPolicy::default(),
            b"unit-secret",
        ));
        let session = manager
            .start(PrincipalId::new(), None, OffsetDateTime::now_utc())
            .unwrap();
        let mw = SessionMiddleware::new(manager, b"cookie-secret")
            .with_secure(true)
            .with_cookie_ttl_secs(3600);
        let issued = mw
            .issue_cookie(&session)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(issued.starts_with("keel_session="));
        assert!(issued.contains("HttpOnly"));
        assert!(issued.contains("Secure"));
        assert!(issued.contains("SameSite=Lax"));
        assert!(issued.contains("Max-Age=3600"));
        let cleared = mw.clear_cookie().unwrap().to_str().unwrap().to_string();
        assert!(cleared.contains("Max-Age=0"));
    }

    /// Wave 4 Phase 7: the remembered cookie carries the policy's 30 day
    /// window while the default login keeps the 8 hour cookie TTL.
    #[test]
    fn remember_cookie_uses_policy_remember_ttl() {
        let manager = Arc::new(SessionManager::new(
            InMemorySessionStore::shared(),
            SessionPolicy::default(),
            b"unit-secret",
        ));
        let session = manager
            .start(PrincipalId::new(), None, OffsetDateTime::now_utc())
            .unwrap();
        let mw = SessionMiddleware::new(manager.clone(), b"cookie-secret")
            .with_cookie_ttl_secs(28_800);
        assert_eq!(manager.policy().remember_ttl, 2_592_000);
        let plain = mw
            .issue_cookie(&session)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(plain.contains("Max-Age=28800"), "{plain}");
        let remembered = mw
            .issue_cookie_with_max_age(&session, manager.policy().remember_ttl)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        assert!(remembered.contains("Max-Age=2592000"), "{remembered}");
        // Same session value: only the TTL differs.
        assert!(
            remembered.starts_with(&plain[..plain.find(';').unwrap()]),
            "cookie value identical"
        );
    }
}
