//! The **guard** port: who is this request?
//!
//! The atlas's LF-049 asks for the guard (the request/session/token
//! *strategy*) to be **separate from the provider** (the user retrieval). This
//! module is that separation:
//!
//! - a [`Guard`] answers "which credential does this request carry, and who
//!   does it belong to", and
//! - the **provider** (`kelson_identity`'s `PrincipalStore` /
//!   `CredentialStore`) answers "what does that principal look like, and is
//!   this password correct".
//!
//! Keeping them apart is what lets a deployment change the *strategy* (the
//! session cookie, a personal access token, an externally issued JWT) without
//! touching the identity stores — and what makes the **failure response
//! consistent**: a guard that finds a credential it cannot trust always says
//! the same thing, and never leaks which part failed.
//!
//! The framework does not depend on the token crate: a bearer guard delegates
//! the credential check to a host-supplied [`BearerVerifier`], so the concrete
//! "PAT first, JWT fallback" order stays the host's policy.

use std::sync::Arc;

use axum::http::HeaderMap;
use kelson_foundation::{AppError, PrincipalId, TenantId};

use crate::cookie::CookieJar;
use crate::request::bearer_token;
use crate::session_mw::ResolvedSession;

/// **How** a request's identity was proven.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardStrategy {
    /// The session cookie (the session store).
    Session,
    /// A bearer credential (a personal access token).
    Token,
    /// A personal access token carried in a **cookie** (Sanctum's SPA mode).
    ///
    /// The cookie is a credential like any other, so it is NOT stateless
    /// bearer auth: it is only honoured together with the CSRF double-submit
    /// proof ✓ (the framework's CSRF layer ✓), which a bearer header does not
    /// need (a header cannot be sent cross-site by a browser ✓).
    TokenCookie,
    /// An externally issued JWT (the opt-in HS256 interop).
    Jwt,
    /// No credential was presented.
    Anonymous,
}

/// The identity a guard resolved for a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuardedPrincipal {
    pub principal_id: PrincipalId,
    pub tenant_id: Option<TenantId>,
    /// The strategy that proved it — so an audit row can say *how* the
    /// request authenticated, not just *who* it was.
    pub strategy: GuardStrategy,
    /// The **abilities** the credential carries (Sanctum's `abilities`).
    ///
    /// `None` = the credential carries no narrowing (a session acts as the
    /// whole principal ✓); `Some(list)` = the credential may exercise only
    /// those abilities ✓ — the host narrows the principal's capabilities with
    /// them ✓.
    pub abilities: Option<Vec<String>>,
}

/// The guard port: resolve a request's identity.
///
/// The contract every guard honours:
///
/// - no credential → `Ok(None)` (anonymous — the caller decides whether that
///   is allowed; an anonymous request is not an error until a route needs an
///   identity);
/// - a credential that cannot be trusted → `Err` (the caller renders the one
///   unauthorized answer — the same one for "unknown", "expired" and
///   "revoked", so the failure never says which);
/// - a valid credential → `Ok(Some(..))` with the strategy that proved it.
pub trait Guard: Send + Sync {
    fn resolve(
        &self,
        headers: &HeaderMap,
        session: Option<&ResolvedSession>,
    ) -> Result<Option<GuardedPrincipal>, AppError>;
}

/// What a bearer guard needs from the host: the check for one bearer
/// credential.
///
/// The host owns the *order* and the *meaning* (the PAT check, the JWT
/// fallback); the framework owns the plumbing.
pub trait BearerVerifier: Send + Sync {
    /// Verify one bearer credential. `Ok(None)` = not a valid credential (the
    /// guard then tries the next strategy).
    fn verify_bearer(&self, token: &str) -> Result<Option<GuardedPrincipal>, AppError>;
}

/// A guard that resolves a **bearer** credential through a host-supplied
/// verifier.
///
/// The session strategy is handled by the session middleware (which resolves
/// the session before the route runs ✓), so this guard covers the credential
/// that arrives in the `Authorization` header ✓.
pub struct BearerGuard<V: BearerVerifier> {
    verifier: Arc<V>,
    /// The strategy the verifier proves (the host says whether it checked a
    /// personal access token or a JWT ✓).
    strategy: GuardStrategy,
}

impl<V: BearerVerifier> BearerGuard<V> {
    pub fn new(verifier: Arc<V>, strategy: GuardStrategy) -> Self {
        Self { verifier, strategy }
    }
}

impl<V: BearerVerifier> Guard for BearerGuard<V> {
    fn resolve(
        &self,
        headers: &HeaderMap,
        session: Option<&ResolvedSession>,
    ) -> Result<Option<GuardedPrincipal>, AppError> {
        // A session (already resolved by the middleware) wins: the browser
        // sends it on every request.
        if let Some(resolved) = session {
            return Ok(Some(GuardedPrincipal {
                principal_id: resolved.session.principal,
                tenant_id: resolved.session.tenant,
                strategy: GuardStrategy::Session,
                abilities: None,
            }));
        }
        let Some(token) = bearer_token(headers) else {
            // No credential at all is NOT an error here: the route decides
            // whether it needs an identity.
            return Ok(None);
        };
        match self.verifier.verify_bearer(token)? {
            Some(principal) => Ok(Some(principal)),
            // A credential that does not verify is an authentication failure —
            // one answer, for every reason.
            None => Err(AppError::unauthenticated()),
        }
    }
}

/// A guard that resolves a personal access token carried in a **cookie**
/// (Sanctum's SPA mode).
///
/// The cookie is a credential like any other, so it is NOT stateless bearer
/// auth: it is only honoured together with the CSRF double-submit proof ✓
/// (a cookie is sent cross-site by a browser ✓ — a bearer header is not ✓).
/// The host therefore supplies the CSRF check ✓ and the framework owns the
/// cookie plumbing ✓.
pub struct CookieTokenGuard<V: BearerVerifier> {
    verifier: Arc<V>,
    /// The cookie the token is carried in (Sanctum's cookie name ✓).
    cookie_name: String,
}

impl<V: BearerVerifier> CookieTokenGuard<V> {
    pub fn new(verifier: Arc<V>, cookie_name: impl Into<String>) -> Self {
        Self {
            verifier,
            cookie_name: cookie_name.into(),
        }
    }

    /// The token in the named cookie, if the request carries one.
    pub fn token_from(&self, headers: &HeaderMap) -> Option<String> {
        let jar = CookieJar::from_headers(headers);
        jar.get(&self.cookie_name).map(str::to_string)
    }
}

impl<V: BearerVerifier> Guard for CookieTokenGuard<V> {
    fn resolve(
        &self,
        headers: &HeaderMap,
        session: Option<&ResolvedSession>,
    ) -> Result<Option<GuardedPrincipal>, AppError> {
        if let Some(resolved) = session {
            return Ok(Some(GuardedPrincipal {
                principal_id: resolved.session.principal,
                tenant_id: resolved.session.tenant,
                strategy: GuardStrategy::Session,
                abilities: None,
            }));
        }
        let Some(token) = self.token_from(headers) else {
            // No cookie-carried credential: anonymous (the route decides ✓).
            return Ok(None);
        };
        match self.verifier.verify_bearer(&token)? {
            Some(principal) => Ok(Some(GuardedPrincipal {
                strategy: GuardStrategy::TokenCookie,
                ..principal
            })),
            None => Err(AppError::unauthenticated()),
        }
    }
}

/// The strategy a guard reports for a resolved principal — the host's
/// helper when it builds one by hand (the demo's own resolution paths ✓).
pub fn guarded(
    principal_id: PrincipalId,
    tenant_id: Option<TenantId>,
    strategy: GuardStrategy,
) -> GuardedPrincipal {
    GuardedPrincipal {
        principal_id,
        tenant_id,
        strategy,
        abilities: None,
    }
}

/// The strategy a guard reports for a resolved principal **with abilities**
/// (a personal access token that may exercise only what it was granted ✓).
pub fn guarded_with_abilities(
    principal_id: PrincipalId,
    tenant_id: Option<TenantId>,
    strategy: GuardStrategy,
    abilities: Vec<String>,
) -> GuardedPrincipal {
    GuardedPrincipal {
        principal_id,
        tenant_id,
        strategy,
        abilities: Some(abilities),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header;
    use kelson_foundation::PrincipalId;

    struct Fixed(Option<GuardedPrincipal>);

    impl BearerVerifier for Fixed {
        fn verify_bearer(&self, _token: &str) -> Result<Option<GuardedPrincipal>, AppError> {
            Ok(self.0.clone())
        }
    }

    fn headers_with_bearer(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}").parse().unwrap(),
        );
        headers
    }

    #[test]
    fn a_request_without_a_credential_is_anonymous_not_an_error() {
        let guard = BearerGuard::new(Arc::new(Fixed(None)), GuardStrategy::Token);
        let resolved = guard
            .resolve(&HeaderMap::new(), None)
            .expect("an anonymous request is not an error");
        assert!(
            resolved.is_none(),
            "no credential means anonymous — the route decides"
        );
    }

    #[test]
    fn a_valid_bearer_resolves_with_its_strategy() {
        let expected = guarded(PrincipalId::new(), Some(TenantId::new()), GuardStrategy::Token);
        let guard = BearerGuard::new(Arc::new(Fixed(Some(expected.clone()))), GuardStrategy::Token);
        let resolved = guard
            .resolve(&headers_with_bearer("pat-value"), None)
            .expect("the credential verifies");
        assert_eq!(resolved, Some(expected));
        assert_eq!(resolved.unwrap().strategy, GuardStrategy::Token);
    }

    #[test]
    fn a_credential_that_does_not_verify_is_one_uniform_refusal() {
        let guard = BearerGuard::new(Arc::new(Fixed(None)), GuardStrategy::Token);
        let err = guard
            .resolve(&headers_with_bearer("not-a-token"), None)
            .expect_err("an unverifiable credential is refused");
        assert_eq!(
            err.public_message,
            AppError::unauthenticated().public_message,
            "every failure reads the same — no oracle"
        );
    }

    #[test]
    fn a_malformed_authorization_header_is_not_a_credential() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Basic abc".parse().unwrap());
        let guard = BearerGuard::new(Arc::new(Fixed(None)), GuardStrategy::Token);
        let resolved = guard
            .resolve(&headers, None)
            .expect("a non-bearer header is not a credential");
        assert!(resolved.is_none(), "a non-bearer header is anonymous");
    }

    #[test]
    fn a_cookie_carried_token_resolves_with_the_spa_strategy() {
        let verified = guarded_with_abilities(
            PrincipalId::new(),
            Some(TenantId::new()),
            GuardStrategy::Token,
            vec!["appointments:read".into()],
        );
        let expected = verified.clone();
        let guard = CookieTokenGuard::new(Arc::new(Fixed(Some(verified))), "kelson_token");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            "kelson_session=abc; kelson_token=pat-value".parse().unwrap(),
        );
        let resolved = guard
            .resolve(&headers, None)
            .expect("the cookie credential verifies")
            .expect("the cookie carried a credential");
        assert_eq!(
            resolved.principal_id, expected.principal_id,
            "the same principal is resolved"
        );
        assert_eq!(resolved.abilities, expected.abilities, "the abilities travel");
        assert_eq!(
            resolved.strategy,
            GuardStrategy::TokenCookie,
            "a cookie-carried token is the SPA strategy, not a bearer one"
        );
    }

    #[test]
    fn a_request_without_the_token_cookie_is_anonymous() {
        let guard = CookieTokenGuard::new(Arc::new(Fixed(None)), "kelson_token");
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, "other=abc".parse().unwrap());
        let resolved = guard
            .resolve(&headers, None)
            .expect("no cookie-carried credential is not an error");
        assert!(resolved.is_none(), "the SPA cookie is absent -> anonymous");
    }

    #[test]
    fn an_unverifiable_cookie_token_is_one_uniform_refusal() {
        let guard = CookieTokenGuard::new(Arc::new(Fixed(None)), "kelson_token");
        let mut headers = HeaderMap::new();
        headers.insert(header::COOKIE, "kelson_token=not-a-token".parse().unwrap());
        let err = guard
            .resolve(&headers, None)
            .expect_err("an unverifiable cookie credential is refused");
        assert_eq!(
            err.public_message,
            AppError::unauthenticated().public_message,
            "the refusal is the same one the bearer guard gives"
        );
    }
}
