//! # keel-tokens
//! Hashed single-use TTL tokens (S2 item 21, L24): API tokens, password reset,
//! email/phone verification. Plaintext is revealed exactly once at issue time;
//! only its sha256 hash is ever persisted. sha2 is a D-62-justified addition
//! (see DEPENDENCY-POLICY.md).
//!
//! Purposes are disjoint: a token issued for one purpose never verifies under
//! another. Reset/verify tokens are single-use (consumed on redemption); API
//! tokens stay valid until expiry or revocation.

pub mod reset;
pub mod seal;

pub use seal::{AesGcmSealer, ALG_AES256_GCM, KEY_LEN, NONCE_LEN};

use std::sync::{Arc, Mutex};

use keel_foundation::{AppError, PrincipalId};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenPurpose {
    /// Long-lived API access token.
    Api,
    /// Password reset redemption (single-use).
    Reset,
    /// Contact verification (single-use).
    Verify,
    /// OAuth2 authorization-code redemption (single-use; B-21).
    OauthCode,
    /// OAuth2 refresh-token rotation (single-use per rotation; B-21).
    OauthRefresh,
    /// One-time signed URL redemption (Wave 2, single-use): the link carries
    /// the plaintext marker, this row records its hash so the second
    /// redemption fails.
    SignedUrl,
}

impl TokenPurpose {
    pub fn as_str(&self) -> &'static str {
        match self {
            TokenPurpose::Api => "api",
            TokenPurpose::Reset => "reset",
            TokenPurpose::Verify => "verify",
            TokenPurpose::OauthCode => "oauth_code",
            TokenPurpose::OauthRefresh => "oauth_refresh",
            TokenPurpose::SignedUrl => "signed_url",
        }
    }

    /// Only single-use purposes are consumed on redemption.
    pub fn single_use(&self) -> bool {
        matches!(
            self,
            TokenPurpose::Reset
                | TokenPurpose::Verify
                | TokenPurpose::OauthCode
                | TokenPurpose::OauthRefresh
                | TokenPurpose::SignedUrl
        )
    }
}

/// Persistent token row. The plaintext never exists here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthToken {
    pub id: Uuid,
    pub principal_id: PrincipalId,
    pub purpose: TokenPurpose,
    /// sha256 hex of the plaintext.
    pub token_hash: String,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(with = "time::serde::iso8601")]
    pub expires_at: OffsetDateTime,
    #[serde(default, with = "time::serde::iso8601::option")]
    pub consumed_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
}

impl AuthToken {
    pub fn expired(&self, now: OffsetDateTime) -> bool {
        now >= self.expires_at
    }

    pub fn consumed(&self) -> bool {
        self.consumed_at.is_some()
    }

    pub fn active(&self, now: OffsetDateTime) -> bool {
        !self.expired(now) && !self.consumed()
    }
}

/// sha256 hex digest of a token plaintext.
pub fn sha256_hex(plaintext: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(plaintext.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// 32 random bytes rendered as URL-safe base64 (no padding), 43 chars.
pub fn generate_plaintext() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64_url_encode(&bytes)
}

/// URL-safe base64 without padding (RFC 4648 sect. 5).
pub fn base64_url_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            out.push(ALPHABET[((triple >> 6) & 0x3F) as usize] as char);
        }
        if chunk.len() > 2 {
            out.push(ALPHABET[(triple & 0x3F) as usize] as char);
        }
    }
    out
}

// --- PKCE (RFC 7636, B-21): S256 only; the plain method is deliberately
// --- unsupported (a verifier-less exchange can never satisfy a challenge). ---

/// PKCE S256 challenge: BASE64URL(SHA256(ascii(code_verifier))).
pub fn pkce_s256_challenge(code_verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(code_verifier.as_bytes());
    base64_url_encode(&hasher.finalize())
}

/// RFC 7636 sect. 4.1: verifier is 43..=128 chars of
/// `[A-Z] / [a-z] / [0-9] / "-" / "." / "_" / "~"`.
pub fn valid_code_verifier(code_verifier: &str) -> bool {
    (43..=128).contains(&code_verifier.len())
        && code_verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'))
}

/// Issue result: plaintext shown once, persisted row carries only the hash.
#[derive(Debug, Clone)]
pub struct IssuedToken {
    pub plaintext: String,
    pub token: AuthToken,
}

/// Storage port. Implementations: InMemoryTokenStore (here), PgTokenStore
/// (keel-postgres) on the auth_tokens table.
pub trait TokenStore: Send + Sync {
    fn insert(&self, token: AuthToken) -> Result<(), AppError>;
    fn find_by_hash(
        &self,
        purpose: TokenPurpose,
        hash: &str,
    ) -> Result<Option<AuthToken>, AppError>;
    /// Atomically mark consumed; returns false if already consumed (or gone).
    fn mark_consumed(&self, id: Uuid, at: OffsetDateTime) -> Result<bool, AppError>;
    fn revoke(&self, id: Uuid) -> Result<(), AppError>;
    fn list_for(&self, principal: PrincipalId) -> Result<Vec<AuthToken>, AppError>;
}

/// Token lifecycle service over a TokenStore.
pub struct TokenService {
    store: Arc<dyn TokenStore>,
}

impl TokenService {
    pub fn new(store: Arc<dyn TokenStore>) -> Self {
        Self { store }
    }

    /// Issue a new token. Plaintext leaves this function exactly once.
    pub fn issue(
        &self,
        principal: PrincipalId,
        purpose: TokenPurpose,
        ttl: Duration,
        label: Option<String>,
    ) -> Result<IssuedToken, AppError> {
        if ttl <= Duration::ZERO {
            return Err(AppError::validation("token ttl must be positive"));
        }
        let now = OffsetDateTime::now_utc();
        let plaintext = generate_plaintext();
        let token = AuthToken {
            id: Uuid::now_v7(),
            principal_id: principal,
            purpose,
            token_hash: sha256_hex(&plaintext),
            label,
            expires_at: now + ttl,
            consumed_at: None,
            created_at: now,
        };
        self.store.insert(token.clone())?;
        Ok(IssuedToken { plaintext, token })
    }

    /// Verify without consuming. Uniform error for missing/expired/consumed:
    /// callers cannot distinguish those cases (no oracle).
    pub fn verify(&self, purpose: TokenPurpose, plaintext: &str) -> Result<AuthToken, AppError> {
        let token = self
            .store
            .find_by_hash(purpose, &sha256_hex(plaintext))?
            .ok_or_else(|| AppError::denied("token is not valid"))?;
        if !token.active(OffsetDateTime::now_utc()) {
            return Err(AppError::denied("token is not valid"));
        }
        Ok(token)
    }

    /// Redeem a single-use token (Reset/Verify). API tokens are verified only.
    pub fn consume(&self, purpose: TokenPurpose, plaintext: &str) -> Result<AuthToken, AppError> {
        let token = self.verify(purpose, plaintext)?;
        if purpose.single_use() {
            let consumed = self
                .store
                .mark_consumed(token.id, OffsetDateTime::now_utc())?;
            if !consumed {
                return Err(AppError::denied("token is not valid"));
            }
        }
        Ok(token)
    }

    pub fn revoke(&self, id: Uuid) -> Result<(), AppError> {
        self.store.revoke(id)
    }

    pub fn list(&self, principal: PrincipalId) -> Result<Vec<AuthToken>, AppError> {
        self.store.list_for(principal)
    }
}

/// Degraded-boot/test store (D-52). Not durable.
#[derive(Debug, Default)]
pub struct InMemoryTokenStore {
    tokens: Mutex<Vec<AuthToken>>,
}

impl InMemoryTokenStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl TokenStore for InMemoryTokenStore {
    fn insert(&self, token: AuthToken) -> Result<(), AppError> {
        self.tokens.lock().unwrap().push(token);
        Ok(())
    }

    fn find_by_hash(
        &self,
        purpose: TokenPurpose,
        hash: &str,
    ) -> Result<Option<AuthToken>, AppError> {
        Ok(self
            .tokens
            .lock()
            .unwrap()
            .iter()
            .find(|t| t.purpose == purpose && t.token_hash == hash)
            .cloned())
    }

    fn mark_consumed(&self, id: Uuid, at: OffsetDateTime) -> Result<bool, AppError> {
        let mut tokens = self.tokens.lock().unwrap();
        if let Some(token) = tokens.iter_mut().find(|t| t.id == id) {
            if token.consumed_at.is_none() {
                token.consumed_at = Some(at);
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn revoke(&self, id: Uuid) -> Result<(), AppError> {
        let mut tokens = self.tokens.lock().unwrap();
        let before = tokens.len();
        tokens.retain(|t| t.id != id);
        if tokens.len() == before {
            return Err(AppError::not_found(format!("token {id}")));
        }
        Ok(())
    }

    fn list_for(&self, principal: PrincipalId) -> Result<Vec<AuthToken>, AppError> {
        Ok(self
            .tokens
            .lock()
            .unwrap()
            .iter()
            .filter(|t| t.principal_id == principal)
            .cloned()
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service() -> TokenService {
        TokenService::new(Arc::new(InMemoryTokenStore::new()))
    }

    #[test]
    fn plaintext_never_persisted() {
        let svc = service();
        let principal = PrincipalId::new();
        let issued = svc
            .issue(principal, TokenPurpose::Api, Duration::hours(1), None)
            .unwrap();
        assert_ne!(issued.token.token_hash, issued.plaintext);
        assert_eq!(issued.token.token_hash, sha256_hex(&issued.plaintext));
        assert!(!issued.plaintext.is_empty());
        assert_eq!(issued.plaintext.len(), 43, "32 bytes -> 43 base64url chars");
    }

    #[test]
    fn issue_verify_consume_and_single_use() {
        let svc = service();
        let principal = PrincipalId::new();
        let issued = svc
            .issue(principal, TokenPurpose::Reset, Duration::minutes(30), None)
            .unwrap();

        let found = svc.verify(TokenPurpose::Reset, &issued.plaintext).unwrap();
        assert_eq!(found.principal_id, principal);

        let consumed = svc.consume(TokenPurpose::Reset, &issued.plaintext).unwrap();
        assert_eq!(consumed.id, issued.token.id);
        assert!(
            svc.consume(TokenPurpose::Reset, &issued.plaintext).is_err(),
            "second redemption fails"
        );
        assert!(
            svc.verify(TokenPurpose::Reset, &issued.plaintext).is_err(),
            "consumed token no longer verifies"
        );
    }

    #[test]
    fn api_tokens_are_not_consumed() {
        let svc = service();
        let issued = svc
            .issue(
                PrincipalId::new(),
                TokenPurpose::Api,
                Duration::hours(8),
                Some("ci".to_string()),
            )
            .unwrap();
        assert!(svc.consume(TokenPurpose::Api, &issued.plaintext).is_ok());
        assert!(
            svc.consume(TokenPurpose::Api, &issued.plaintext).is_ok(),
            "api token stays valid"
        );
    }

    #[test]
    fn purposes_are_disjoint() {
        let svc = service();
        let issued = svc
            .issue(
                PrincipalId::new(),
                TokenPurpose::Api,
                Duration::hours(1),
                None,
            )
            .unwrap();
        assert!(svc.verify(TokenPurpose::Reset, &issued.plaintext).is_err());
        assert!(svc.verify(TokenPurpose::Verify, &issued.plaintext).is_err());
    }

    #[test]
    fn expiry_enforced() {
        let svc = service();
        let issued = svc
            .issue(
                PrincipalId::new(),
                TokenPurpose::Verify,
                Duration::seconds(1),
                None,
            )
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        assert!(svc.verify(TokenPurpose::Verify, &issued.plaintext).is_err());
    }

    #[test]
    fn revoke_and_list() {
        let svc = service();
        let principal = PrincipalId::new();
        let t1 = svc
            .issue(
                principal,
                TokenPurpose::Api,
                Duration::hours(1),
                Some("a".to_string()),
            )
            .unwrap();
        let _t2 = svc
            .issue(
                principal,
                TokenPurpose::Api,
                Duration::hours(1),
                Some("b".to_string()),
            )
            .unwrap();
        assert_eq!(svc.list(principal).unwrap().len(), 2);
        svc.revoke(t1.token.id).unwrap();
        assert_eq!(svc.list(principal).unwrap().len(), 1);
        assert!(svc.verify(TokenPurpose::Api, &t1.plaintext).is_err());
        assert!(svc.revoke(Uuid::now_v7()).is_err());
    }

    #[test]
    fn ttl_must_be_positive() {
        let svc = service();
        assert!(svc
            .issue(PrincipalId::new(), TokenPurpose::Api, Duration::ZERO, None)
            .is_err());
    }

    #[test]
    fn uniform_denied_errors() {
        let svc = service();
        let missing = svc
            .verify(TokenPurpose::Api, "not-a-real-token")
            .unwrap_err();
        assert_eq!(missing.category, keel_foundation::ErrorCategory::Authorization);
    }

    #[test]
    fn oauth_purposes_are_single_use_and_disjoint() {
        let svc = service();
        let principal = PrincipalId::new();
        let code = svc
            .issue(
                principal,
                TokenPurpose::OauthCode,
                Duration::minutes(10),
                None,
            )
            .unwrap();
        assert!(TokenPurpose::OauthCode.single_use());
        assert!(TokenPurpose::OauthRefresh.single_use());
        assert_eq!(TokenPurpose::OauthCode.as_str(), "oauth_code");
        assert_eq!(TokenPurpose::OauthRefresh.as_str(), "oauth_refresh");
        let consumed = svc
            .consume(TokenPurpose::OauthCode, &code.plaintext)
            .unwrap();
        assert_eq!(consumed.principal_id, principal);
        assert!(
            svc.consume(TokenPurpose::OauthCode, &code.plaintext).is_err(),
            "authorization code is single-use"
        );
        // Purposes stay disjoint across the new variants.
        assert!(svc.verify(TokenPurpose::Api, &code.plaintext).is_err());
        assert!(svc
            .verify(TokenPurpose::OauthRefresh, &code.plaintext)
            .is_err());
    }

    #[test]
    fn pkce_s256_matches_the_rfc7636_vector() {
        // RFC 7636 appendix B test vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            pkce_s256_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert!(valid_code_verifier(verifier));
        assert!(!valid_code_verifier("too-short"));
        assert!(!valid_code_verifier(&"a".repeat(42)));
        assert!(!valid_code_verifier(&"a".repeat(129)));
        assert!(!valid_code_verifier(&format!("{}$", "a".repeat(43))));
    }
}
