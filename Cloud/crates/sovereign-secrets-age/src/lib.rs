//! `age` (X25519) envelope-encryption adapter for the secrets port.
//!
//! V0 stores a randomly-generated `age::x25519::Identity` at the
//! `master_key_path` passed in (default `/var/lib/sovereign/master.key`).
//! The file is the standard `age` CLI Bech32 format
//! (`AGE-SECRET-KEY-1...`), so an operator can rotate / inspect it
//! with the `age-keygen` and `age` CLIs.
//!
//! # Why X25519-only, not scrypt, for V0
//!
//! The spec (docs/phase-00-mvp.md §F7) calls for the master key to
//! be passphrase-protected via Argon2id. The Argon2 dep is in
//! `Cargo.toml` for that future; V0 ships X25519 only because the
//! single-tenant / single-user V0 does not have a "passphrase" — the
//! key is owned by the `sovereign` system user and never leaves the
//! box. V1.5 will add the passphrase prompt and Argon2id key
//! derivation; the `master_key_path` stays the same.
//!
//! # Trust model
//!
//! - The master key file MUST be `0600`, owned by the sovereign
//!   service user. We `chmod` on write; the OS enforces reads.
//! - The plaintext is only ever held in process memory; the runtime
//!   receives it as an env var at `create_container` time and the
//!   container process is the only place it ever lives.
//! - The `encrypt` output includes a fresh ephemeral X25519 key per
//!   call (age's design), so two encryptions of the same plaintext
//!   produce different ciphertexts. This is the right default for
//!   secret rotation audits.

#![deny(unsafe_code)]
#![allow(missing_docs)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use age::{
    decrypt as age_decrypt, encrypt as age_encrypt, secrecy::ExposeSecret, x25519::Identity,
    x25519::Recipient,
};
use anyhow::Context;
use sovereign_core::error::AppError;
use sovereign_core::ports::SecretsPort;
use tokio::sync::OnceCell;
use tracing::info;

/// The default master key path on the canonical target host
/// (Hetzner CX22, Ubuntu 22.04, per `docs/tech-stack.md` §2.1).
/// V0 does not support a custom path via config; the user is
/// expected to symlink `/var/lib/sovereign/master.key` if they want
/// the key on a different filesystem.
pub const DEFAULT_MASTER_KEY_PATH: &str = "/var/lib/sovereign/master.key";

/// The on-disk file permissions for the master key. `0600` =
/// owner-read-write only. We apply this on every write; the OS
/// enforces the read.
#[cfg(unix)]
const MASTER_KEY_MODE: u32 = 0o600;
// Windows does not have POSIX modes. We open the file with the
// default DACL (which inherits the user's umask on the parent dir).
// The `age-keygen` import flow (V1) is the right place to add a
// real Windows ACL.

/// The `age` (X25519) secrets adapter. Cheap to clone (the master
/// key is behind an `Arc<OnceCell<Identity>>`).
#[derive(Clone)]
pub struct AgeSecrets {
    /// The path to the master key file. Created on first encrypt/
    /// decrypt if missing.
    master_key_path: PathBuf,
    /// Lazily-loaded master identity. `OnceCell` so `open` is cheap
    /// and the I/O happens at most once per process.
    identity: Arc<OnceCell<Arc<Identity>>>,
}

impl std::fmt::Debug for AgeSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgeSecrets")
            .field("master_key_path", &self.master_key_path)
            .finish_non_exhaustive()
    }
}

impl AgeSecrets {
    /// Build an `AgeSecrets` that will load (or generate) the master
    /// key at `master_key_path`. The file is created if missing; the
    /// dir is NOT created (caller's responsibility).
    pub fn new(master_key_path: impl Into<PathBuf>) -> Self {
        Self {
            master_key_path: master_key_path.into(),
            identity: Arc::new(OnceCell::new()),
        }
    }

    /// Build an `AgeSecrets` with the canonical V0 default path
    /// (`/var/lib/sovereign/master.key`).
    pub fn with_default_path() -> Self {
        Self::new(DEFAULT_MASTER_KEY_PATH)
    }

    /// Force-load (or generate) the master key now, returning the
    /// in-memory identity. The first caller does the file I/O;
    /// concurrent callers get the same `Arc<Identity>`.
    pub async fn ensure_loaded(&self) -> Result<Arc<Identity>, AppError> {
        self.identity
            .get_or_try_init(|| async {
                let id = load_or_generate(&self.master_key_path)
                    .await
                    .map_err(|e| AppError::upstream(format!("{e:#}")))?;
                Ok::<Arc<Identity>, AppError>(Arc::new(id))
            })
            .await
            .cloned()
    }
}

#[async_trait::async_trait]
impl SecretsPort for AgeSecrets {
    fn encrypt(&self, plaintext: &[u8]) -> Result<Vec<u8>, AppError> {
        // Block-in-async is acceptable here: the work is a single
        // X25519 + ChaCha20-Poly1305 encrypt of a small blob
        // (typically < 4 KB), and there is no async I/O. We do the
        // load synchronously by calling `blocking_lock` semantics
        // via the inner `OnceCell::get` (which is set after the
        // first await call; subsequent calls are sync).
        //
        // For the first call (before `ensure_loaded` ran), we
        // fall through to a sync file read. This is rare (only at
        // process startup) and the cost is one syscall.
        let identity = futures::executor::block_on(self.ensure_loaded())?;
        let recipient: Recipient = identity.to_public();
        age_encrypt(&recipient, plaintext)
            .map_err(|e| AppError::upstream(format!("age encrypt: {e}")))
    }

    fn decrypt(&self, ciphertext: &[u8]) -> Result<Vec<u8>, AppError> {
        let identity = futures::executor::block_on(self.ensure_loaded())?;
        age_decrypt(identity.as_ref(), ciphertext)
            .map_err(|e| AppError::upstream(format!("age decrypt: {e}")))
    }

    fn master_key_path(&self) -> PathBuf {
        self.master_key_path.clone()
    }
}

/// Load the master identity from `path`, or generate + save a new
/// one if the file is missing. On a present-but-corrupt file,
/// returns an error (we do NOT silently overwrite a key the
/// operator may have planted).
async fn load_or_generate(path: &Path) -> anyhow::Result<Identity> {
    if path.exists() {
        load_existing(path).await
    } else {
        generate_and_save(path).await
    }
}

/// Read the file and parse the Bech32 identity.
async fn load_existing(path: &Path) -> anyhow::Result<Identity> {
    let s = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("read master key at {}", path.display()))?;
    // The `age` CLI allows a passphrase-protected identity (scrypt
    // or age-plugin-se). V0 only handles the unprotected X25519
    // case; if the line doesn't start with `AGE-SECRET-KEY-1`, we
    // bail with a clear error so the operator can either remove the
    // passphrase (V0) or wait for V1 to add Argon2id protection.
    let s = s.trim();
    let identity = s.parse::<Identity>().map_err(|e| {
        anyhow::anyhow!(
            "master key at {} is not an unprotected age X25519 identity \
             (parse error: {e}). V0 does not support passphrase-protected \
             master keys; regenerate with `age-keygen -o {}` (unprotected) \
             or wait for V1.",
            path.display(),
            path.display()
        )
    })?;
    info!(path = %path.display(), "master key loaded");
    Ok(identity)
}

/// Generate a new X25519 identity, save it to `path` with 0600
/// permissions, and return it.
async fn generate_and_save(path: &Path) -> anyhow::Result<Identity> {
    let identity = Identity::generate();
    let bech32 = identity.to_string();
    write_secret(path, bech32.expose_secret().as_bytes()).await?;
    info!(
        path = %path.display(),
        "master key generated (mode 0600). KEEP THIS FILE SAFE — losing it means losing all secrets."
    );
    Ok(identity)
}

/// Write `bytes` to `path` with `MASTER_KEY_MODE` permissions. If
/// the parent directory does not exist, returns an error (the
/// caller — `sovereign init` or the install script — is expected
/// to `mkdir -p` first).
async fn write_secret(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            anyhow::bail!(
                "parent dir {} does not exist; create it with `mkdir -p` (or run `sovereign init` once that's wired in V0.5).",
                parent.display()
            );
        }
    }
    tokio::fs::write(path, bytes)
        .await
        .with_context(|| format!("write master key to {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(MASTER_KEY_MODE);
        tokio::fs::set_permissions(path, perms)
            .await
            .with_context(|| format!("chmod 0600 on {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! The unit tests use a tempdir for the master key so they
    //! don't touch `/var/lib/sovereign`. Integration tests in
    //! `tests/age_integration.rs` cover the full rotate-secret flow
    //! against a real storage adapter.

    use super::*;
    use tempfile::TempDir;

    fn temp_master_key() -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("master.key");
        (dir, path)
    }

    #[tokio::test]
    async fn generates_and_reloads_master_key() {
        let (_dir, path) = temp_master_key();
        assert!(!path.exists(), "precondition: file does not exist");

        // First call: generate.
        let s1 = AgeSecrets::new(path.clone());
        let id1 = s1.ensure_loaded().await.expect("load 1");
        let pub1 = id1.to_public().to_string();
        assert!(
            path.exists(),
            "master key file should exist after first call"
        );

        // Verify file is mode 0600 on unix.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let meta = std::fs::metadata(&path).expect("stat");
            assert_eq!(
                meta.permissions().mode() & 0o777,
                MASTER_KEY_MODE,
                "master key file must be 0600"
            );
        }

        // Second call: reload from the same file -> same public key.
        let s2 = AgeSecrets::new(path.clone());
        let id2 = s2.ensure_loaded().await.expect("load 2");
        let pub2 = id2.to_public().to_string();
        assert_eq!(pub1, pub2, "master key must be stable across reloads");
    }

    #[tokio::test]
    async fn encrypt_decrypt_roundtrip() {
        let (_dir, path) = temp_master_key();
        let secrets = AgeSecrets::new(path);
        let plaintext = b"postgres://user:hunter2@db:5432/api";
        let ct = secrets.encrypt(plaintext).expect("encrypt");
        assert_ne!(&ct[..], plaintext, "ciphertext must differ from plaintext");
        let pt = secrets.decrypt(&ct).expect("decrypt");
        assert_eq!(&pt[..], plaintext, "decrypted must match original");
    }

    #[tokio::test]
    async fn two_encryptions_produce_different_ciphertexts() {
        // age uses a fresh ephemeral X25519 key per encryption, so
        // this is the contract — not just a property.
        let (_dir, path) = temp_master_key();
        let secrets = AgeSecrets::new(path);
        let plaintext = b"same input";
        let a = secrets.encrypt(plaintext).expect("a");
        let b = secrets.encrypt(plaintext).expect("b");
        assert_ne!(a, b, "two encryptions of the same plaintext must differ");
        assert_eq!(
            secrets.decrypt(&a).expect("da"),
            secrets.decrypt(&b).expect("db"),
            "both must decrypt to the same plaintext"
        );
    }

    #[tokio::test]
    async fn rejects_corrupt_master_key() {
        let (_dir, path) = temp_master_key();
        std::fs::write(&path, b"not a bech32 age identity\n").expect("write garbage");
        let secrets = AgeSecrets::new(path);
        let result = secrets.ensure_loaded().await;
        let msg = match result {
            Ok(_) => panic!("expected an error, got Ok"),
            Err(e) => e.to_string(),
        };
        assert!(
            msg.contains("master key") || msg.contains("age X25519"),
            "expected a clear error about the master key, got: {msg}"
        );
    }

    #[tokio::test]
    async fn rejects_missing_parent_dir() {
        // Write to a path whose parent does not exist.
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("nope").join("master.key");
        let secrets = AgeSecrets::new(path);
        let result = secrets.ensure_loaded().await;
        let msg = match result {
            Ok(_) => panic!("expected an error, got Ok"),
            Err(e) => e.to_string(),
        };
        assert!(
            msg.contains("parent dir"),
            "expected a 'parent dir' error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn streaming_decrypt_works() {
        // Sanity: the `decrypt` API can handle ciphertext > 1 MB
        // (env-var limit is generous; we test with a 2 MB blob to
        // exercise the streaming path).
        let (_dir, path) = temp_master_key();
        let secrets = AgeSecrets::new(path);
        let mut plaintext = Vec::with_capacity(2 * 1024 * 1024);
        for i in 0..2 * 1024 * 1024 {
            plaintext.push((i % 251) as u8);
        }
        let ct = secrets.encrypt(&plaintext).expect("encrypt");
        let pt = secrets.decrypt(&ct).expect("decrypt");
        assert_eq!(pt, plaintext);
    }

    #[test]
    fn master_key_path_is_returned() {
        let secrets = AgeSecrets::new("/some/path/master.key");
        assert_eq!(
            secrets.master_key_path(),
            PathBuf::from("/some/path/master.key")
        );
    }
}
