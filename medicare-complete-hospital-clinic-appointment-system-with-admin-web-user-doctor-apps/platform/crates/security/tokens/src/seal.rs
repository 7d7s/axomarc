//! AES-256-GCM at-rest sealer (blueprint L27, S3 item 41).
//!
//! Concrete [`Sealer`] implementation for encryption at rest: AES-256-GCM
//! (authenticated encryption), 96-bit random nonce per seal, algorithm and
//! key identity recorded in the [`Envelope`]. aes-gcm is a RustCrypto crate;
//! justified in DEPENDENCY-POLICY.md alongside sha2 for the security class.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use kelson_foundation::{codes, AppError, Envelope, ErrorCategory, Retryability, Sealer, Secret, Severity};
use rand::RngCore;

/// Algorithm identifier recorded in [`Envelope::alg`].
pub const ALG_AES256_GCM: &str = "AES-256-GCM";
/// Master key length: 32 bytes (256 bits).
pub const KEY_LEN: usize = 32;
/// Per-seal nonce length: 96 bits (NIST-recommended for GCM).
pub const NONCE_LEN: usize = 12;

/// AES-256-GCM sealer holding one master key.
pub struct AesGcmSealer {
    key: [u8; KEY_LEN],
}

impl AesGcmSealer {
    pub fn new(key: [u8; KEY_LEN]) -> Self {
        Self { key }
    }

    /// Build from a [`Secret`] holding exactly 32 bytes.
    pub fn from_secret(key: &Secret) -> Result<Self, AppError> {
        let raw = key.expose().as_bytes();
        if raw.len() != KEY_LEN {
            return Err(AppError::validation(format!(
                "AES-256-GCM master key must be exactly {KEY_LEN} bytes (got {})",
                raw.len()
            )));
        }
        let mut key = [0u8; KEY_LEN];
        key.copy_from_slice(raw);
        Ok(Self { key })
    }

    fn cipher(&self) -> Aes256Gcm {
        Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&self.key))
    }
}

impl Drop for AesGcmSealer {
    /// Best-effort wipe of the master-key copy held by this sealer.
    fn drop(&mut self) {
        for byte in self.key.iter_mut() {
            // SAFETY: exclusive reference to our own key copy.
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
    }
}

impl Sealer for AesGcmSealer {
    fn seal(&self, key_id: &str, secret: &Secret) -> Result<Envelope, AppError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        rand::thread_rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = self
            .cipher()
            .encrypt(nonce, secret.expose().as_bytes())
            .map_err(|_| {
                AppError::new(
                    codes::INTERNAL,
                    ErrorCategory::Internal,
                    Severity::Medium,
                    Retryability::Retryable,
                    "seal failed",
                )
            })?;
        Ok(Envelope::new(
            ALG_AES256_GCM,
            key_id,
            nonce_bytes.to_vec(),
            ciphertext,
        ))
    }

    fn open(&self, envelope: &Envelope) -> Result<Secret, AppError> {
        if envelope.alg != ALG_AES256_GCM {
            return Err(AppError::validation(format!(
                "unsupported envelope alg {}",
                envelope.alg
            )));
        }
        if envelope.nonce.len() != NONCE_LEN {
            return Err(AppError::validation(format!(
                "envelope nonce must be {NONCE_LEN} bytes"
            )));
        }
        let nonce = Nonce::from_slice(&envelope.nonce);
        let plaintext = self
            .cipher()
            .decrypt(nonce, envelope.ciphertext.as_slice())
            .map_err(|_| {
                AppError::new(
                    codes::ELEVATION,
                    ErrorCategory::Authorization,
                    Severity::High,
                    Retryability::NotRetryable,
                    "envelope authentication failed",
                )
            })?;
        Secret::from_bytes(plaintext)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sealer() -> AesGcmSealer {
        AesGcmSealer::new([7u8; KEY_LEN])
    }

    #[test]
    fn seal_open_roundtrip() {
        let s = sealer();
        let secret = Secret::new("db-password-xyz");
        let env = s.seal("master-2026", &secret).unwrap();
        assert_eq!(env.alg, ALG_AES256_GCM);
        assert_eq!(env.key_id, "master-2026");
        assert_eq!(env.nonce.len(), NONCE_LEN);
        assert_ne!(env.ciphertext.as_slice(), secret.expose().as_bytes());
        assert_eq!(s.open(&env).unwrap(), secret);
    }

    #[test]
    fn fresh_nonce_per_seal() {
        let s = sealer();
        let secret = Secret::new("same-input");
        let a = s.seal("k", &secret).unwrap();
        let b = s.seal("k", &secret).unwrap();
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn tampered_ciphertext_rejected() {
        let s = sealer();
        let mut env = s.seal("k", &Secret::new("val")).unwrap();
        env.ciphertext[0] ^= 0x01;
        let err = s.open(&env).unwrap_err();
        assert_eq!(err.code, codes::ELEVATION);
    }

    #[test]
    fn wrong_key_rejected() {
        let a = sealer();
        let b = AesGcmSealer::new([9u8; KEY_LEN]);
        let env = a.seal("k", &Secret::new("val")).unwrap();
        assert!(b.open(&env).is_err());
    }

    #[test]
    fn key_len_validation() {
        assert!(AesGcmSealer::from_secret(&Secret::new("tooshort")).is_err());
        assert!(
            AesGcmSealer::from_secret(&Secret::new("01234567890123456789012345678901")).is_ok()
        );
    }

    #[test]
    fn bad_nonce_and_alg_rejected() {
        let s = sealer();
        let mut env = s.seal("k", &Secret::new("v")).unwrap();
        env.nonce.pop();
        assert!(s.open(&env).is_err());
        env.nonce.push(0);
        env.alg = "OTHER".into();
        assert!(s.open(&env).is_err());
    }

    #[test]
    fn unicode_roundtrip() {
        let s = sealer();
        let secret = Secret::new("multi-script-value-ключ-رمز");
        let env = s.seal("k", &secret).unwrap();
        assert_eq!(s.open(&env).unwrap(), secret);
    }
}
