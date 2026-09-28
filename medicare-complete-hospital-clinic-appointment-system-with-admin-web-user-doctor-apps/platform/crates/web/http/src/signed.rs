//! Signed URLs (S9): HMAC-SHA256 link signing with optional expiry, closing
//! the Laravel `signed URL / temporary signed URL` gap without a new
//! dependency (HMAC built over the existing sha2 digest).
//!
//! Wire format: `<path>?<existing-query>&expires=<unix>&signature=<hex>`.
//! The signature covers the full path+query string excluding the signature
//! parameter itself, so any tampering with the path, parameters or expiry
//! invalidates the link.

use std::sync::Arc;

use keel_foundation::{codes, AppError, ErrorCategory, PrincipalId, Retryability, Severity};
use keel_tokens::{TokenPurpose, TokenService};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};

const SIGNATURE_PARAM: &str = "signature";
const EXPIRES_PARAM: &str = "expires";

pub struct SignedUrls {
    secret: Vec<u8>,
}

impl SignedUrls {
    pub fn new(secret: Vec<u8>) -> Self {
        Self { secret }
    }

    /// Sign a path (with any existing query). `ttl = None` signs without an
    /// expiry (permanent until secret rotation).
    pub fn sign(&self, path: &str, ttl: Option<Duration>) -> String {
        let mut message = path.to_string();
        if let Some(ttl) = ttl {
            let expires = OffsetDateTime::now_utc() + ttl;
            let sep = if message.contains('?') { '&' } else { '?' };
            message = format!("{message}{sep}{EXPIRES_PARAM}={}", expires.unix_timestamp());
        }
        let tag = hex(&hmac_sha256(&self.secret, message.as_bytes()));
        let sep = if message.contains('?') { '&' } else { '?' };
        format!("{message}{sep}{SIGNATURE_PARAM}={tag}")
    }

    /// Verify a signed path+query. Ok when the signature is intact and the
    /// expiry (when present) has not passed.
    pub fn verify(&self, path_and_query: &str) -> Result<(), AppError> {
        let (body, signature) = split_signature(path_and_query)
            .ok_or_else(|| AppError::denied("signed url missing signature"))?;
        let expected = hmac_sha256(&self.secret, body.as_bytes());
        let provided = parse_hex(&signature)
            .ok_or_else(|| AppError::denied("signed url signature malformed"))?;
        if !constant_time_eq(&expected, &provided) {
            return Err(AppError::denied("signed url signature invalid"));
        }
        if let Some(expiry) = extract_expiry(&body) {
            let Some(expiry) = expiry else {
                return Err(AppError::denied("signed url expiry malformed"));
            };
            if OffsetDateTime::now_utc().unix_timestamp() > expiry {
                return Err(AppError::denied("signed url expired"));
            }
        }
        Ok(())
    }

    /// Boolean form of [`SignedUrls::verify`] for request guards: a signed
    /// path+query is accepted only when the HMAC matches and the expiry
    /// (when present) has not passed. The `Result` carries no detail worth
    /// leaking to a caller, so the reason is deliberately discarded here.
    pub fn has_valid_signature(&self, path_and_query: &str) -> bool {
        self.verify(path_and_query).is_ok()
    }
}

/// Query parameter carrying the single-use marker of a one-time signed URL.
const ONCE_PARAM: &str = "once";

/// Derive the signed-URL HMAC secret from the deployment's application key.
/// The derivation is scoped to the signed-URL context (as `derive_cookie_key`
/// is scoped to cookies), so one derived key cannot forge another context's
/// material and rotating the app key rotates every signed link.
pub fn derive_signed_key(app_key: &str) -> Vec<u8> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(app_key.as_bytes());
    hasher.update(b"\x00keel.signed-url.v1");
    hasher.finalize().to_vec()
}

/// Signed-URL resolution port (Wave 2, 19-PARITY "URL generation"). The
/// chain runtime holds this behind a trait, mirroring [`crate::EdgeAuthPort`]:
/// the web crate executes the `signed` builtin without owning the token store.
pub trait SignedUrlPort: Send + Sync {
    /// Resolve a request's path+query: the HMAC must be intact and the TTL
    /// (when present) unexpired, and a carried `once` marker must be redeemed
    /// for its first time. The first call wins; every later call fails with a
    /// 404, so a one-time link cannot be replayed.
    fn resolve(&self, path_and_query: &str) -> Result<(), AppError>;
}

/// The signer behind the `signed` middleware: HMAC verification plus the
/// one-time redemption record, bound to the deployment's token store.
pub struct SignedGate {
    urls: SignedUrls,
    tokens: Arc<TokenService>,
}

impl SignedGate {
    pub fn new(secret: Vec<u8>, tokens: Arc<TokenService>) -> Self {
        Self {
            urls: SignedUrls::new(secret),
            tokens,
        }
    }

    /// Mint a one-time signed URL (19-PARITY "one-time redemption variant").
    /// A single-use marker is recorded in the token store as a hash only and
    /// travels inside the URL, under the signature together with `expires` —
    /// so the marker can be neither stripped, swapped, nor replayed without
    /// invalidating the HMAC.
    pub fn sign_once(
        &self,
        principal: PrincipalId,
        path: &str,
        ttl: Duration,
    ) -> Result<String, AppError> {
        let issued = self
            .tokens
            .issue(principal, TokenPurpose::SignedUrl, ttl, None)?;
        let sep = if path.contains('?') { '&' } else { '?' };
        let marked = format!("{path}{sep}{ONCE_PARAM}={}", issued.plaintext);
        Ok(self.urls.sign(&marked, Some(ttl)))
    }

    /// Verify signature + TTL, then consume the `once` marker when the URL
    /// carries one. An ordinary (multi-use) signed URL has no marker and
    /// simply verifies.
    pub fn redeem(&self, path_and_query: &str) -> Result<(), AppError> {
        self.urls.verify(path_and_query)?;
        let Some(marker) = extract_param(path_and_query, ONCE_PARAM) else {
            return Ok(());
        };
        self.tokens
            .consume(TokenPurpose::SignedUrl, &marker)
            .map(|_| ())
            .map_err(|_| {
                // Replay, unknown or expired marker: the link is spent. The
                // NotFound category keeps it a 404 (410/404 per the Wave 2
                // test plan) and lets a web plane render `errors.404`.
                AppError::new(
                    codes::NOT_FOUND,
                    ErrorCategory::NotFound,
                    Severity::Low,
                    Retryability::NotRetryable,
                    "signed url has already been redeemed",
                )
            })
    }
}

impl SignedUrlPort for SignedGate {
    fn resolve(&self, path_and_query: &str) -> Result<(), AppError> {
        self.redeem(path_and_query)
    }
}

/// Read `key=value` out of a `path?query` string. The signature is hex (it
/// can never contain `&`/`=` pairs of its own), so scanning the raw string
/// is safe; a marker appended after the signature invalidates the HMAC
/// first and never reaches this point.
fn extract_param(path_and_query: &str, key: &str) -> Option<String> {
    let (_, query) = path_and_query.split_once('?')?;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| v.to_string())
    })
}

/// Separate `signature=...` from the rest of the path+query.
fn split_signature(path_and_query: &str) -> Option<(String, String)> {
    let (path, query) = match path_and_query.split_once('?') {
        Some((p, q)) => (p, q),
        None => return None,
    };
    let mut kept: Vec<&str> = Vec::new();
    let mut signature: Option<String> = None;
    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            if key == SIGNATURE_PARAM {
                signature = Some(value.to_string());
                continue;
            }
        }
        kept.push(pair);
    }
    let signature = signature?;
    let body = if kept.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{}", kept.join("&"))
    };
    Some((body, signature))
}

fn extract_expiry(body: &str) -> Option<Option<i64>> {
    let query = body.split_once('?').map(|(_, q)| q)?;
    for pair in query.split('&') {
        if let Some((key, value)) = pair.split_once('=') {
            if key == EXPIRES_PARAM {
                return Some(value.parse::<i64>().ok());
            }
        }
    }
    None
}

/// HMAC-SHA256 (RFC 2104) over the sha2 digest — avoids a new hmac crate.
/// `pub(crate)` so the JWT interop verifier (jwt.rs) reuses this exact,
/// tested implementation instead of growing a second HMAC.
pub(crate) fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut block_key = [0u8; BLOCK];
    if key.len() > BLOCK {
        let digest = Sha256::digest(key);
        block_key[..32].copy_from_slice(&digest);
    } else {
        block_key[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= block_key[i];
        opad[i] ^= block_key[i];
    }
    let mut inner = Sha256::new();
    inner.update(ipad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(opad);
    outer.update(inner_digest);
    outer.finalize().into()
}

/// Compare two byte strings without timing side channels. Unequal lengths
/// short-circuit (lengths of our own tags are public); equal-length inputs
/// always scan every byte.
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..a.len() {
        diff |= a[i] ^ b[i];
    }
    diff == 0
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

fn parse_hex(raw: &str) -> Option<Vec<u8>> {
    if raw.len() % 2 != 0 {
        return None;
    }
    (0..raw.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&raw[i..i + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_foundation::ErrorCategory;

    fn signer() -> SignedUrls {
        SignedUrls::new(b"signed-url-test-secret".to_vec())
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let s = signer();
        let url = s.sign("/invites/123", None);
        assert!(url.contains("signature="));
        s.verify(&url).unwrap();
    }

    #[test]
    fn preserves_existing_query() {
        let s = signer();
        let url = s.sign("/invites/123?role=doctor", None);
        assert!(url.starts_with("/invites/123?role=doctor&signature="));
        s.verify(&url).unwrap();
    }

    #[test]
    fn tampered_path_or_params_fail() {
        let s = signer();
        let url = s.sign("/invites/123", None);
        assert!(s
            .verify(&url.replace("/invites/123", "/invites/999"))
            .is_err());
        let with_query = s.sign("/invites/123?role=doctor", None);
        assert!(s
            .verify(&with_query.replace("role=doctor", "role=admin"))
            .is_err());
    }

    #[test]
    fn tampered_signature_fails() {
        let s = signer();
        let url = s.sign("/invites/123", None);
        let mut tampered = url.clone();
        let len = tampered.len();
        let last = tampered.drain(len - 1..).next().unwrap();
        tampered.push(if last == '0' { '1' } else { '0' });
        assert!(s.verify(&tampered).is_err());
    }

    #[test]
    fn expiry_enforced() {
        let s = signer();
        let fresh = s.sign("/invites/123", Some(Duration::minutes(10)));
        s.verify(&fresh).unwrap();
        assert!(fresh.contains("expires="));
        let stale = fresh.replace(
            &format!(
                "expires={}",
                (OffsetDateTime::now_utc() + Duration::minutes(10)).unix_timestamp()
            ),
            &format!(
                "expires={}",
                (OffsetDateTime::now_utc() - Duration::minutes(1)).unix_timestamp()
            ),
        );
        // signature now mismatches (expiry is signed over) — still rejected
        assert!(s.verify(&stale).is_err());
        // hand-craft an expired but correctly-signed url to prove the time check
        let expired_body = format!(
            "/x?expires={}",
            OffsetDateTime::now_utc().unix_timestamp() - 5
        );
        let tag = hex(&hmac_sha256(&s.secret, expired_body.as_bytes()));
        let expired_url = format!("{expired_body}&signature={tag}");
        let err = s.verify(&expired_url).unwrap_err();
        assert_eq!(err.category, ErrorCategory::Authorization);
    }

    #[test]
    fn missing_signature_rejected() {
        assert!(signer().verify("/invites/123?expires=1").is_err());
    }

    #[test]
    fn has_valid_signature_is_the_boolean_guard() {
        let s = signer();
        let url = s.sign("/exports/1", Some(Duration::minutes(5)));
        assert!(s.has_valid_signature(&url));
        assert!(!s.has_valid_signature("/exports/1"));
        assert!(!s.has_valid_signature("/exports/1?signature=deadbeef"));
    }

    // ------------------------- Wave 2 one-time signed URLs (19-PARITY URL gen)

    fn gate() -> SignedGate {
        let tokens = Arc::new(keel_tokens::TokenService::new(Arc::new(
            keel_tokens::InMemoryTokenStore::new(),
        )));
        SignedGate::new(b"signed-url-test-secret".to_vec(), tokens)
    }

    #[test]
    fn one_time_url_redeems_exactly_once() {
        let g = gate();
        let url = g
            .sign_once(
                keel_foundation::PrincipalId::new(),
                "/downloads/1?scope=csv",
                Duration::minutes(10),
            )
            .expect("one-time url mints");
        assert!(url.contains("once="), "{url}");
        assert!(url.contains("expires="), "{url}");

        // Redemption records through the token store; the first call wins.
        g.redeem(&url).expect("first redemption succeeds");
        let err = g.redeem(&url).expect_err("replay must be refused");
        assert_eq!(err.category, ErrorCategory::NotFound);
        assert_eq!(err.code, codes::NOT_FOUND);
        assert!(err.public_message.contains("already been redeemed"));
    }

    #[test]
    fn one_time_marker_is_bound_to_the_signature() {
        let g = gate();
        let url = g
            .sign_once(
                keel_foundation::PrincipalId::new(),
                "/downloads/1",
                Duration::minutes(10),
            )
            .expect("one-time url mints");

        // Stripping the marker changes the signed body -> refused as forged.
        let marker = url
            .split_once('?')
            .expect("query part")
            .1
            .split('&')
            .find(|pair| pair.starts_with("once="))
            .expect("marker present")
            .to_string();
        let stripped = url.replace(&format!("{marker}&"), "");
        let err = g.redeem(&stripped).expect_err("marker cannot be stripped");
        assert_eq!(err.category, ErrorCategory::Authorization);

        // Appending a foreign marker after the signature fails the HMAC first.
        let foreign = format!("{url}&once=attacker-supplied");
        let err = g.redeem(&foreign).expect_err("foreign marker refused");
        assert_eq!(err.category, ErrorCategory::Authorization);
    }

    #[test]
    fn multi_use_signed_urls_keep_verifying() {
        let g = gate();
        let url = g.urls.sign("/plain", Some(Duration::minutes(5)));
        g.redeem(&url).expect("first use");
        g.redeem(&url).expect("ordinary signed urls are multi-use");
    }
}

// constant_time_eq is re-used by CSRF double-submit and session-cookie tag
// checks; pin its semantics so a future "optimisation" cannot reintroduce
// a timing or correctness hole.
#[cfg(test)]
mod constant_time_tests {
    use super::constant_time_eq;

    #[test]
    fn equal_inputs_match() {
        assert!(constant_time_eq(b"keel-tag-value", b"keel-tag-value"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn unequal_inputs_reject_at_every_position() {
        let base = b"0123456789abcdef";
        for i in 0..base.len() {
            let mut flipped = base.to_vec();
            flipped[i] ^= 0x01;
            assert!(!constant_time_eq(base, &flipped), "diff at {i} accepted");
        }
    }

    #[test]
    fn length_mismatch_rejects() {
        assert!(!constant_time_eq(b"short", b"longer-value"));
        assert!(!constant_time_eq(b"longer-value", b"short"));
        assert!(!constant_time_eq(b"", b"x"));
    }
}
