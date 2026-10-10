//! Stateless HS256 JWT bearer interop (B-22, report R5). Opt-in behind the
//! `app.jwt` feature flag; hashed PAT + HMAC session stays the default auth
//! model. This is an *interop verifier*: it accepts externally-issued HS256
//! bearer tokens so stateless clients can call Kelson without a PAT exchange.
//!
//! Security posture:
//! - Only `alg = HS256` is accepted. `none` and every other algorithm are
//!   rejected before any crypto runs (no algorithm-confusion oracle).
//! - The signature is recomputed over the exact received `header.payload`
//!   ASCII substring (RFC 7515 signing input) — never over a re-encoding —
//!   and compared with the crate's constant-time `constant_time_eq`.
//! - `exp` is mandatory and enforced; malformed/expired/bad-signature tokens
//!   all collapse to `AppError::unauthenticated()` (401, no distinguishing
//!   error), matching the edge policy for invalid bearer credentials.
//! - HMAC-SHA256 reuses the single tested `signed::hmac_sha256` (sha2-based,
//!   RFC 2104) — no second HMAC implementation, no new crate.

use kelson_foundation::AppError;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// JWT claims carried by an interop bearer token. Unknown claims are
/// tolerated and ignored; these are the ones the edge acts on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JwtClaims {
    /// Subject: the principal id (UUID string). Required.
    pub sub: String,
    /// Tenant id (UUID string) when the token is tenant-scoped.
    #[serde(default)]
    pub tenant: Option<String>,
    /// Expiry as unix seconds. Required and enforced.
    pub exp: i64,
    /// Issued-at as unix seconds. Informational.
    #[serde(default)]
    pub iat: Option<i64>,
    /// Unique token id (revocation/replay hygiene). Informational here.
    #[serde(default)]
    pub jti: Option<String>,
}

/// HS256 signer/verifier bound to one shared secret.
#[derive(Clone)]
pub struct JwtHs256 {
    secret: Vec<u8>,
}

impl JwtHs256 {
    pub fn new(secret: Vec<u8>) -> Self {
        Self { secret }
    }

    /// Issue a compact JWS (header.payload.signature). Used by tests and any
    /// future token-exchange surface; not wired to an endpoint by default.
    pub fn sign(&self, claims: &JwtClaims) -> String {
        let header = serde_json::json!({ "alg": "HS256", "typ": "JWT" });
        let header_b64 = b64url_encode(header.to_string().as_bytes());
        let payload_b64 = b64url_encode(&serde_json::to_vec(claims).expect("claims serialize"));
        let signing_input = format!("{header_b64}.{payload_b64}");
        let tag = crate::signed::hmac_sha256(&self.secret, signing_input.as_bytes());
        format!("{signing_input}.{}", b64url_encode(&tag))
    }

    /// Verify a compact JWS and return its claims. Every failure mode maps to
    /// `AppError::unauthenticated()` so the edge answers a uniform 401.
    pub fn verify(&self, token: &str) -> Result<JwtClaims, AppError> {
        let parts: Vec<&str> = token.split('.').collect();
        if parts.len() != 3 {
            return Err(AppError::unauthenticated());
        }
        let [header_b64, payload_b64, sig_b64] = [parts[0], parts[1], parts[2]];

        // Header must decode and declare exactly HS256. Reject alg=none and
        // any other algorithm before touching the signature.
        let header_bytes = b64url_decode(header_b64).ok_or_else(AppError::unauthenticated)?;
        let header: serde_json::Value =
            serde_json::from_slice(&header_bytes).map_err(|_| AppError::unauthenticated())?;
        if header.get("alg").and_then(|v| v.as_str()) != Some("HS256") {
            return Err(AppError::unauthenticated());
        }

        // Recompute over the received signing input (never re-encoded).
        let signing_input = format!("{header_b64}.{payload_b64}");
        let expected = crate::signed::hmac_sha256(&self.secret, signing_input.as_bytes());
        let provided = b64url_decode(sig_b64).ok_or_else(AppError::unauthenticated)?;
        if !crate::signed::constant_time_eq(&expected, &provided) {
            return Err(AppError::unauthenticated());
        }

        let payload_bytes = b64url_decode(payload_b64).ok_or_else(AppError::unauthenticated)?;
        let claims: JwtClaims =
            serde_json::from_slice(&payload_bytes).map_err(|_| AppError::unauthenticated())?;

        let now = OffsetDateTime::now_utc().unix_timestamp();
        if claims.exp <= now {
            return Err(AppError::unauthenticated());
        }
        Ok(claims)
    }
}

// ------------------------------------------------------------- base64url

/// URL-safe base64 without padding (RFC 4648 §5), matching kelson-tokens.
fn b64url_encode(bytes: &[u8]) -> String {
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

fn b64url_val(c: u8) -> Option<u32> {
    match c {
        b'A'..=b'Z' => Some((c - b'A') as u32),
        b'a'..=b'z' => Some((c - b'a') as u32 + 26),
        b'0'..=b'9' => Some((c - b'0') as u32 + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    }
}

/// Decode URL-safe base64 (no padding accepted). Per RFC 7515 any non-zero
/// leftover bits in the final group are ignored (interop-tolerant); invalid
/// characters or padding reject.
fn b64url_decode(s: &str) -> Option<Vec<u8>> {
    let bytes = s.as_bytes();
    if bytes.contains(&b'=') {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4 + 1);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for &b in bytes {
        let v = b64url_val(b)?;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    fn signer() -> JwtHs256 {
        JwtHs256::new(b"jwt-interop-test-secret".to_vec())
    }

    fn claims(ttl: Duration) -> JwtClaims {
        let now = OffsetDateTime::now_utc();
        JwtClaims {
            sub: "11111111-1111-7111-8111-111111111111".into(),
            tenant: Some("22222222-2222-7222-8222-222222222222".into()),
            exp: (now + ttl).unix_timestamp(),
            iat: Some(now.unix_timestamp()),
            jti: Some("jti-1".into()),
        }
    }

    #[test]
    fn sign_and_verify_roundtrip() {
        let s = signer();
        let token = s.sign(&claims(Duration::minutes(10)));
        let got = s.verify(&token).unwrap();
        assert_eq!(got.sub, "11111111-1111-7111-8111-111111111111");
        assert_eq!(
            got.tenant.as_deref(),
            Some("22222222-2222-7222-8222-222222222222")
        );
    }

    #[test]
    fn expired_token_rejected() {
        let s = signer();
        let token = s.sign(&claims(Duration::minutes(-1)));
        assert!(s.verify(&token).is_err());
    }

    #[test]
    fn wrong_secret_rejected() {
        let token = signer().sign(&claims(Duration::minutes(10)));
        let other = JwtHs256::new(b"a-different-secret".to_vec());
        assert!(other.verify(&token).is_err());
    }

    #[test]
    fn tampered_payload_rejected() {
        let s = signer();
        let token = s.sign(&claims(Duration::minutes(10)));
        let mut parts: Vec<String> = token.split('.').map(str::to_string).collect();
        // Flip a character in the payload segment.
        let mut payload = parts[1].clone();
        let last = payload.pop().unwrap();
        payload.push(if last == 'A' { 'B' } else { 'A' });
        parts[1] = payload;
        assert!(s.verify(&parts.join(".")).is_err());
    }

    #[test]
    fn alg_none_rejected() {
        let header = b64url_encode(br#"{"alg":"none","typ":"JWT"}"#);
        let payload = b64url_encode(
            serde_json::to_vec(&claims(Duration::minutes(10)))
                .unwrap()
                .as_slice(),
        );
        let forged = format!("{header}.{payload}.");
        assert!(signer().verify(&forged).is_err());
    }

    #[test]
    fn alg_substitution_rejected() {
        let s = signer();
        let token = s.sign(&claims(Duration::minutes(10)));
        let mut parts: Vec<String> = token.split('.').map(str::to_string).collect();
        // Swap the header for an RS256 declaration; signature is now over a
        // different header, so verification must fail.
        parts[0] = b64url_encode(br#"{"alg":"RS256","typ":"JWT"}"#);
        assert!(s.verify(&parts.join(".")).is_err());
    }

    #[test]
    fn malformed_shapes_rejected() {
        let s = signer();
        assert!(s.verify("").is_err());
        assert!(s.verify("not-a-jwt").is_err());
        assert!(s.verify("a.b").is_err());
        assert!(s.verify("a.b.c.d").is_err());
        assert!(s.verify("!!!.@@@.###").is_err());
    }

    #[test]
    fn padding_in_signature_rejected() {
        let s = signer();
        let token = s.sign(&claims(Duration::minutes(10)));
        assert!(s.verify(&format!("{token}=")).is_err());
    }

    #[test]
    fn base64url_roundtrip() {
        for input in [
            b"".to_vec(),
            b"a".to_vec(),
            b"ab".to_vec(),
            b"abc".to_vec(),
            (0u8..=255).collect(),
        ] {
            let encoded = b64url_encode(&input);
            assert!(!encoded.contains('='));
            let decoded = b64url_decode(&encoded).unwrap();
            assert_eq!(decoded, input, "roundtrip len={}", input.len());
        }
    }
}
