//! Secret handling (blueprint L27, S3 item 41).
//!
//! - [`Secret`]: redacting carrier for in-memory secrets. Both `Debug` and
//!   `Display` render a fixed redaction marker; plaintext is reachable only
//!   through the explicit, grep-able [`Secret::expose`] / [`Secret::into_string`].
//! - [`Envelope`]: serializable at-rest shape (alg, key id, nonce, ciphertext).
//! - [`Sealer`]: the encryption port. Foundation defines the contract; the
//!   security class ships concrete ciphers (AES-GCM in kelson-tokens). Core
//!   intentionally adds no crypto externals for this.

use serde::{Deserialize, Serialize};

use crate::{codes, AppError, ErrorCategory, Retryability, Severity};

const REDACTED: &str = "[REDACTED]";

/// Redacting in-memory secret carrier (L27).
///
/// Stores the value as bytes so [`Drop`] can do a best-effort wipe of the
/// backing allocation. Construction validates UTF-8 once; [`Secret::expose`]
/// therefore never fails.
#[derive(Clone)]
pub struct Secret(Vec<u8>);

impl Secret {
    /// Wrap a UTF-8 secret value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into().into_bytes())
    }

    /// Wrap raw secret bytes; rejects non-UTF-8 input.
    pub fn from_bytes(bytes: Vec<u8>) -> Result<Self, AppError> {
        String::from_utf8(bytes)
            .map(|s| Self(s.into_bytes()))
            .map_err(|_| AppError::validation("secret must be valid UTF-8"))
    }

    /// Explicit opt-in to read the plaintext. Call sites using this are the
    /// audit surface for secret exposure — keep them few and reviewed.
    pub fn expose(&self) -> &str {
        std::str::from_utf8(&self.0).expect("validated at construction")
    }

    /// Consume and return the plaintext.
    pub fn into_string(mut self) -> String {
        let bytes = std::mem::take(&mut self.0);
        // SAFETY: UTF-8 validated at construction. The sensitive buffer is
        // swapped out, so the Drop wipe touches an empty vec only.
        unsafe { String::from_utf8_unchecked(bytes) }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Constant-time (no early exit) equality for verification checks.
    /// Leaks only the length, which equality must reveal anyway.
    pub fn eq_ct(&self, other: &Secret) -> bool {
        let a = &self.0;
        let b = &other.0;
        if a.len() != b.len() {
            return false;
        }
        let mut diff = 0u8;
        for (x, y) in a.iter().zip(b.iter()) {
            diff |= x ^ y;
        }
        diff == 0
    }
}

impl Drop for Secret {
    /// Best-effort wipe of the carrier buffer. No defense against copies the
    /// caller may already hold; the contract covers this allocation only.
    fn drop(&mut self) {
        for byte in self.0.iter_mut() {
            // SAFETY: write_volatile through an exclusive reference to our own
            // allocation; the write has no side effects beyond zeroing.
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Secret({REDACTED})")
    }
}

impl std::fmt::Display for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(REDACTED)
    }
}

impl From<String> for Secret {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for Secret {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl std::cmp::PartialEq for Secret {
    fn eq(&self, other: &Self) -> bool {
        self.eq_ct(other)
    }
}

impl std::cmp::Eq for Secret {}

/// Serializable sealed form of a secret at rest (L27).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    /// Cipher identifier, e.g. `AES-256-GCM`.
    pub alg: String,
    /// Master key identity that sealed this envelope (rotation support).
    pub key_id: String,
    /// Nonce/IV used for sealing.
    pub nonce: Vec<u8>,
    /// Ciphertext, including any authentication tag.
    pub ciphertext: Vec<u8>,
}

impl Envelope {
    pub fn new(
        alg: impl Into<String>,
        key_id: impl Into<String>,
        nonce: Vec<u8>,
        ciphertext: Vec<u8>,
    ) -> Self {
        Self {
            alg: alg.into(),
            key_id: key_id.into(),
            nonce,
            ciphertext,
        }
    }
}

/// At-rest encryption port (L27). Foundation owns the contract; concrete
/// ciphers live in the security class so core stays external-free.
pub trait Sealer: Send + Sync {
    /// Encrypt `secret` under `key_id`. Each call uses a fresh nonce.
    fn seal(&self, key_id: &str, secret: &Secret) -> Result<Envelope, AppError>;

    /// Decrypt an envelope back into an in-memory [`Secret`].
    fn open(&self, envelope: &Envelope) -> Result<Secret, AppError>;
}

/// Open helper that rejects envelopes sealed for another key identity.
pub fn open_for_key(
    sealer: &dyn Sealer,
    envelope: &Envelope,
    key_id: &str,
) -> Result<Secret, AppError> {
    if envelope.key_id != key_id {
        return Err(AppError::new(
            codes::ELEVATION,
            ErrorCategory::Authorization,
            Severity::High,
            Retryability::NotRetryable,
            "envelope sealed under a different key identity",
        ));
    }
    sealer.open(envelope)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_and_display_never_leak() {
        let secret = Secret::new("hunter2-super-secret");
        let dbg = format!("{secret:?}");
        let disp = format!("{secret}");
        assert!(!dbg.contains("hunter2"));
        assert!(!disp.contains("hunter2"));
        assert_eq!(disp, "[REDACTED]");
        assert!(dbg.contains("[REDACTED]"));
    }

    #[test]
    fn expose_and_consume_roundtrip() {
        let secret = Secret::new("abc-123");
        assert_eq!(secret.expose(), "abc-123");
        assert!(!secret.is_empty());
        assert_eq!(secret.into_string(), "abc-123");
        assert!(Secret::new("").is_empty());
    }

    #[test]
    fn from_bytes_rejects_non_utf8() {
        assert!(Secret::from_bytes(vec![0xff, 0xfe, 0xfd]).is_err());
        assert_eq!(Secret::from_bytes(b"ok".to_vec()).unwrap().expose(), "ok");
    }

    #[test]
    fn constant_time_equality() {
        let a = Secret::new("same-value");
        let b = Secret::new("same-value");
        let c = Secret::new("same-valu3");
        let d = Secret::new("shorter");
        assert_eq!(a, b);
        assert!(a.eq_ct(&b));
        assert_ne!(a, c);
        assert!(!a.eq_ct(&c));
        assert!(!a.eq_ct(&d));
    }

    #[test]
    fn envelope_serde_roundtrip() {
        let env = Envelope::new("TEST-ALG", "k1", vec![1, 2, 3], vec![9, 9, 9, 9]);
        let raw = serde_json::to_string(&env).unwrap();
        let back: Envelope = serde_json::from_str(&raw).unwrap();
        assert_eq!(env, back);
    }

    /// Toy content-authenticating sealer for the port contract: reverses the
    /// bytes and appends a checksum; rejects tampered ciphertext on open.
    struct ReverseSealer;

    impl Sealer for ReverseSealer {
        fn seal(&self, key_id: &str, secret: &Secret) -> Result<Envelope, AppError> {
            let mut data = secret.expose().as_bytes().to_vec();
            let sum = data.iter().fold(0u8, |acc, b| acc.wrapping_add(*b));
            data.reverse();
            data.push(sum);
            Ok(Envelope::new("TEST", key_id, vec![], data))
        }
        fn open(&self, envelope: &Envelope) -> Result<Secret, AppError> {
            if envelope.alg != "TEST" {
                return Err(AppError::validation("wrong alg"));
            }
            if envelope.ciphertext.is_empty() {
                return Err(AppError::new(
                    codes::ELEVATION,
                    ErrorCategory::Authorization,
                    Severity::High,
                    Retryability::NotRetryable,
                    "envelope truncated",
                ));
            }
            let sum = envelope.ciphertext[envelope.ciphertext.len() - 1];
            let mut data = envelope.ciphertext[..envelope.ciphertext.len() - 1].to_vec();
            let check = data.iter().fold(0u8, |acc, b| acc.wrapping_add(*b));
            if check != sum {
                return Err(AppError::new(
                    codes::ELEVATION,
                    ErrorCategory::Authorization,
                    Severity::High,
                    Retryability::NotRetryable,
                    "envelope failed authenticity check",
                ));
            }
            data.reverse();
            Secret::from_bytes(data)
        }
    }

    #[test]
    fn sealer_port_roundtrip_and_tamper_rejection() {
        let sealer = ReverseSealer;
        let secret = Secret::new("payload-value");
        let mut env = sealer.seal("k1", &secret).unwrap();
        assert_eq!(sealer.open(&env).unwrap(), secret);
        env.ciphertext[0] ^= 0xff;
        assert!(sealer.open(&env).is_err());
    }

    #[test]
    fn open_for_key_rejects_wrong_identity() {
        let sealer = ReverseSealer;
        let env = sealer.seal("k1", &Secret::new("v")).unwrap();
        assert!(open_for_key(&sealer, &env, "k1").is_ok());
        let err = open_for_key(&sealer, &env, "k2").unwrap_err();
        assert_eq!(err.code, codes::ELEVATION);
    }
}
