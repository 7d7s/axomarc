// The secrets port. Implemented by `sovereign-secrets-age` (age X25519
// + scrypt; optional Argon2id for passphrase-derived keys in V1+).
//
// The port exposes only the cryptographic primitives, not the storage.
// The storage port (`StoragePort::put_secret` / `get_secret`) carries
// the ciphertext blob; the use case pulls the ciphertext out of
// storage and hands it to the secrets port for decryption. This split
// is what makes the rotation story tractable: `rotate_secret` writes
// new ciphertext to the same row, the master key is unchanged.
//
// Per `docs/phase-00-mvp.md` §F7:
//   - master key at /var/lib/sovereign/master.key, 0600, owned by the
//     sovereign service user
//   - V0 uses a randomly-generated X25519 identity; the file is the
//     `AGE-SECRET-KEY-1...` Bech32 string. The `Argon2id` dep is in
//     `Cargo.toml` for the V1 passphrase-protected master.
//   - secrets are injected into the container at `create_container`
//     time; the runtime never sees the plaintext at rest
//   - secret values are NEVER logged, NEVER passed on argv
//
// All methods return `AppError::Upstream` on crypto/IO errors and
// `AppError::Validation` on bad inputs. The HTTP layer maps
// `Upstream` to 502 and `Validation` to 400.

use async_trait::async_trait;

use crate::error::AppError;

/// The secrets port. Cheap to clone (the master key is behind an
/// `Arc`); the canonical instance lives in the composition root.
#[async_trait]
pub trait SecretsPort: Send + Sync {
    /// Encrypt `plaintext` and return the age-format ciphertext blob
    /// (the same wire format `age::Encryptor::with_recipients` emits).
    /// The blob is self-describing (it carries its own ephemeral
    /// public key + MAC), so the secret store does not need a
    /// separate "key id" column.
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, AppError>;

    /// Decrypt a ciphertext blob produced by [`encrypt`](Self::encrypt)
    /// or any other age X25519 recipient. Returns the original
    /// plaintext.
    fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, AppError>;

    /// The path the master key was loaded from (or generated to).
    /// Surfaced in the doctor check ("master key present at X")
    /// and the install log.
    fn master_key_path(&self) -> std::path::PathBuf;
}

#[cfg(test)]
mod tests {
    // Trait is the contract; no tests at the port level (the
    // `sovereign-secrets-age` crate has the real tests).
}
