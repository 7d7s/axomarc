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

pub mod wrapped;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use age::{
    decrypt as age_decrypt, encrypt as age_encrypt, secrecy::ExposeSecret, x25519::Identity,
    x25519::Recipient,
};
use anyhow::Context;
use secrecy::SecretString;
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
    /// The passphrase used to derive the AEAD key that wraps the
    /// on-disk age secret key. V0.5+: the master key is NEVER
    /// stored in plaintext on disk. V0 (pre-0.5) used a bare
    /// `AGE-SECRET-KEY-1...` Bech32 file; V0.5 refuses to load
    /// those (see `load_or_generate_wrapped` for the migration
    /// error message).
    passphrase: SecretString,
}

impl std::fmt::Debug for AgeSecrets {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgeSecrets")
            .field("master_key_path", &self.master_key_path)
            .finish_non_exhaustive()
    }
}

impl AgeSecrets {
    /// Build an `AgeSecrets` that will load (or generate) the
    /// passphrase-wrapped master key at `master_key_path`. The
    /// file is created if missing; the dir is NOT created
    /// (caller's responsibility).
    pub fn new(master_key_path: impl Into<PathBuf>, passphrase: SecretString) -> Self {
        Self {
            master_key_path: master_key_path.into(),
            identity: Arc::new(OnceCell::new()),
            passphrase,
        }
    }

    /// Build an `AgeSecrets` with the canonical V0 default path
    /// (`/var/lib/sovereign/master.key`), unless the operator has
    /// set `SOVEREIGN_MASTER_KEY_PATH` in the environment. The env
    /// override is what `sovereign init`/`sovereign login`/`sovereign
    /// deploy` all consult; the hardcoded path remains the default
    /// for V0 back-compat.
    pub fn with_default_path(passphrase: SecretString) -> Self {
        let path = std::env::var_os("SOVEREIGN_MASTER_KEY_PATH")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(DEFAULT_MASTER_KEY_PATH));
        Self::new(path, passphrase)
    }

    /// Build an `AgeSecrets` from a pre-loaded identity. No file
    /// I/O happens; the identity is used directly for both
    /// `encrypt` and `decrypt`. Used by the V0.1.0 → V0.5
    /// migration flow to read V0.1.0-encrypted ciphertexts
    /// without going through the file-load path.
    pub fn with_identity(master_key_path: impl Into<PathBuf>, identity: Arc<Identity>) -> Self {
        Self {
            master_key_path: master_key_path.into(),
            identity: Arc::new(OnceCell::from(identity)),
            // The passphrase is unused for in-memory identities;
            // we still store an empty one so the field is
            // always populated.
            passphrase: SecretString::new(String::new().into_boxed_str()),
        }
    }

    /// Force-load (or generate) the master key now, returning the
    /// in-memory identity. The first caller does the file I/O +
    /// Argon2id KDF (~300 ms on a CX22); concurrent callers get
    /// the same `Arc<Identity>` from the cache.
    pub async fn ensure_loaded(&self) -> Result<Arc<Identity>, AppError> {
        self.identity
            .get_or_try_init(|| async {
                let id = load_or_generate_wrapped(&self.master_key_path, &self.passphrase)
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

/// Load the wrapped master identity from `path`, or generate +
/// save a new one if the file is missing. On a present file in
/// the pre-V0.5 bare Bech32 format, returns a clear migration
/// error (we do NOT silently overwrite or upgrade a key the
/// operator may have planted).
async fn load_or_generate_wrapped(
    path: &Path,
    passphrase: &SecretString,
) -> anyhow::Result<Identity> {
    if path.exists() {
        load_existing_wrapped(path, passphrase).await
    } else {
        generate_and_save_wrapped(path, passphrase).await
    }
}

/// Read a V1 wrapped key file, derive the AEAD key from
/// `passphrase`, decrypt, and parse the Bech32 identity. On
/// pre-V0.5 bare Bech32 format, return a clear migration error.
async fn load_existing_wrapped(path: &Path, passphrase: &SecretString) -> anyhow::Result<Identity> {
    let bytes = tokio::fs::read(path)
        .await
        .with_context(|| format!("read master key at {}", path.display()))?;
    if !wrapped::is_wrapped_v1(&bytes) {
        anyhow::bail!(
            "master key at {} is in the pre-V0.5 (bare Bech32) format. V0.5+ \
             refuses to read it because the key is stored without a passphrase. \
             To migrate, run V0.5 once with `sovereign login --migrate` (which \
             re-encrypts every existing secret under a new wrapped key) OR \
             keep using a V0.1.0 binary until you're ready to re-encrypt.",
            path.display()
        );
    }
    let plaintext = wrapped::unwrap(passphrase, &bytes)
        .map_err(|e| anyhow::anyhow!("cannot unwrap master key at {}: {e}", path.display()))?;
    let identity: Identity = plaintext.trim().parse().map_err(|e| {
        anyhow::anyhow!(
            "unwrapped master key at {} is not a valid age X25519 identity (parse error: {e}; corrupt file or wrong passphrase)",
            path.display()
        )
    })?;
    info!(path = %path.display(), "master key unwrapped (V1, Argon2id)");
    Ok(identity)
}

/// Generate a new X25519 identity, wrap it with Argon2id +
/// XChaCha20-Poly1305 under `passphrase`, and save to `path`
/// with 0600 permissions.
async fn generate_and_save_wrapped(
    path: &Path,
    passphrase: &SecretString,
) -> anyhow::Result<Identity> {
    let identity = Identity::generate();
    let bech32 = identity.to_string();
    let plaintext = format!("{}\n", bech32.expose_secret());
    let wrapped_bytes = wrapped::wrap(passphrase, &plaintext)
        .map_err(|e| anyhow::anyhow!("wrap master key with Argon2id: {e}"))?;
    write_secret(path, &wrapped_bytes).await?;
    info!(
        path = %path.display(),
        "master key generated (V1 wrapped, mode 0600). \
         KEEP THIS FILE AND REMEMBER THE PASSPHRASE — \
         losing either means losing all secrets."
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

/// Read a V0.1.0 (pre-V0.5) bare Bech32 master key file and
/// return the in-memory identity. The V0.1.0 file is just a
/// single line `AGE-SECRET-KEY-1...` (no passphrase, no header).
///
/// This function exists ONLY for the V0.1.0 → V0.5 migration
/// flow. It will be kept around as long as there are operators
/// still running v0.1.0 binaries; it can be removed once the
/// last known v0.1.0 install has been upgraded.
pub async fn read_bare_bech32_identity(path: &Path) -> anyhow::Result<Identity> {
    let s = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("read V0.1.0 master key at {}", path.display()))?;
    let s = s.trim();
    let identity: Identity = s.parse().map_err(|e| {
        anyhow::anyhow!(
            "V0.1.0 master key at {} is not a valid age X25519 identity (parse error: {e})",
            path.display()
        )
    })?;
    Ok(identity)
}

/// Generate a fresh X25519 identity, wrap it with Argon2id under
/// `passphrase`, and save to `path` with 0600 permissions.
/// Returns the in-memory identity. The path may be the same as
/// an existing V0.1.0 file; the caller is responsible for the
/// atomic rename.
pub async fn generate_wrapped_master_key(
    path: &Path,
    passphrase: &SecretString,
) -> anyhow::Result<Identity> {
    let identity = Identity::generate();
    let bech32 = identity.to_string();
    let plaintext = format!("{}\n", bech32.expose_secret());
    let wrapped_bytes = wrapped::wrap(passphrase, &plaintext)
        .map_err(|e| anyhow::anyhow!("wrap master key with Argon2id: {e}"))?;
    write_secret(path, &wrapped_bytes).await?;
    info!(
        path = %path.display(),
        "wrote V0.5 wrapped master key (Argon2id, mode 0600)"
    );
    Ok(identity)
}

/// Atomically replace `target` with `source`. On Unix, that's
/// `rename(source, target)`. On Windows, the destination file
/// can't be replaced if it's open for reading by another
/// process, so we fall back to `copy + remove`.
pub async fn atomic_replace(source: &Path, target: &Path) -> anyhow::Result<()> {
    if let Err(e) = tokio::fs::rename(source, target).await {
        // Windows returns an error if the target is open. We try
        // copy+remove as a fallback. This is not strictly atomic
        // but is correct in the "no other process is reading
        // the file" case (which is our case for the master key).
        tracing::warn!(
            error = %e,
            "atomic rename failed; falling back to copy + remove"
        );
        tokio::fs::copy(source, target)
            .await
            .with_context(|| format!("copy {} -> {}", source.display(), target.display()))?;
        tokio::fs::remove_file(source)
            .await
            .with_context(|| format!("remove {}", source.display()))?;
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

    fn test_passphrase() -> SecretString {
        SecretString::new(
            "test-passphrase-please-do-not-use-in-prod"
                .to_string()
                .into(),
        )
    }

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
        let s1 = AgeSecrets::new(path.clone(), test_passphrase());
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
        let s2 = AgeSecrets::new(path.clone(), test_passphrase());
        let id2 = s2.ensure_loaded().await.expect("load 2");
        let pub2 = id2.to_public().to_string();
        assert_eq!(pub1, pub2, "master key must be stable across reloads");
    }

    #[tokio::test]
    async fn wrong_passphrase_fails_to_unwrap_existing_key() {
        let (_dir, path) = temp_master_key();
        // Generate with passphrase A.
        let s1 = AgeSecrets::new(path.clone(), test_passphrase());
        s1.ensure_loaded().await.expect("gen A");
        // Try to load with passphrase B.
        let wrong = SecretString::new("different-passphrase".to_string().into());
        let s2 = AgeSecrets::new(path.clone(), wrong);
        let result = s2.ensure_loaded().await;
        let msg = match result {
            Ok(_) => panic!("wrong passphrase must fail; got Ok"),
            Err(e) => e.to_string(),
        };
        assert!(
            msg.contains("wrong passphrase")
                || msg.contains("cannot unwrap")
                || msg.contains("age X25519"),
            "expected clear error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn rejects_pre_v05_bare_bech32_format() {
        // Write a V0 bare Bech32 file directly to the path, then
        // try to load with V0.5. The migration error should fire.
        let (_dir, path) = temp_master_key();
        // A throwaway (but valid) V0 Bech32 identity.
        let bare = Identity::generate().to_string();
        let bech32 = bare.expose_secret();
        std::fs::write(&path, format!("{bech32}\n").as_bytes()).expect("write");
        let s = AgeSecrets::new(path.clone(), test_passphrase());
        let result = s.ensure_loaded().await;
        let msg = match result {
            Ok(_) => panic!("expected V0-format rejection"),
            Err(e) => e.to_string(),
        };
        assert!(
            msg.contains("pre-V0.5") || msg.contains("bare Bech32"),
            "expected V0 migration error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn encrypt_decrypt_roundtrip() {
        let (_dir, path) = temp_master_key();
        let secrets = AgeSecrets::new(path, test_passphrase());
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
        let secrets = AgeSecrets::new(path, test_passphrase());
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
        // Garbage that IS long enough to look like a wrapped key
        // header (so the bare-Bech32 error doesn't fire) but
        // fails the magic check.
        std::fs::write(&path, b"not a bech32 age identity\nGARBAGE").expect("write garbage");
        let secrets = AgeSecrets::new(path, test_passphrase());
        let result = secrets.ensure_loaded().await;
        let msg = match result {
            Ok(_) => panic!("expected an error, got Ok"),
            Err(e) => e.to_string(),
        };
        assert!(
            msg.contains("master key") || msg.contains("V1"),
            "expected a clear error about the master key, got: {msg}"
        );
    }

    #[tokio::test]
    async fn rejects_missing_parent_dir() {
        // Write to a path whose parent does not exist.
        let dir = TempDir::new().expect("tempdir");
        let path = dir.path().join("nope").join("master.key");
        let secrets = AgeSecrets::new(path, test_passphrase());
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
        let secrets = AgeSecrets::new(path, test_passphrase());
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
        let secrets = AgeSecrets::new("/some/path/master.key", test_passphrase());
        assert_eq!(
            secrets.master_key_path(),
            PathBuf::from("/some/path/master.key")
        );
    }

    #[tokio::test]
    async fn with_identity_skips_file_io() {
        // `with_identity` should give us an AgeSecrets that
        // encrypts + decrypts with the in-memory identity, no
        // file I/O required.
        let (_dir, path) = temp_master_key();
        let pp = test_passphrase();
        // First create a real wrapped master key so we have a
        // known identity to use.
        let s_gen = AgeSecrets::new(path.clone(), pp.clone());
        let id = s_gen.ensure_loaded().await.expect("gen");
        // Now build a second AgeSecrets with the in-memory
        // identity, pointing at a path that does NOT exist.
        let fake_path = std::path::PathBuf::from("/nonexistent/never/written/master.key");
        let s_mem = AgeSecrets::with_identity(fake_path, id.clone());
        let plaintext = b"postgres://user:secret@db:5432/x";
        let ct = s_mem.encrypt(plaintext).expect("encrypt via mem identity");
        let pt = s_mem.decrypt(&ct).expect("decrypt via mem identity");
        assert_eq!(pt, plaintext);
    }

    #[tokio::test]
    async fn read_bare_bech32_identity_parses_v0_file() {
        let (_dir, path) = temp_master_key();
        // A V0.1.0 file is just a bare Bech32 string on a line.
        // Use `Identity::generate()` to get a real one, write it
        // as V0.1.0 format, then parse it back.
        let v0 = Identity::generate();
        let v0_bech = v0.to_string();
        let v0_plain = format!("{}\n", v0_bech.expose_secret());
        std::fs::write(&path, v0_plain.as_bytes()).expect("write v0");

        let parsed = read_bare_bech32_identity(&path).await.expect("read");
        assert_eq!(
            parsed.to_public().to_string(),
            v0.to_public().to_string(),
            "parsed identity must match the one we wrote"
        );
    }

    #[tokio::test]
    async fn read_bare_bech32_rejects_garbage() {
        let (_dir, path) = temp_master_key();
        std::fs::write(&path, b"not a bech32 string\n").expect("write");
        let result = read_bare_bech32_identity(&path).await;
        let err = result.err().expect("expected Err");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("not a valid age X25519") || msg.contains("V0.1.0"),
            "expected clear error, got: {msg}"
        );
    }

    #[tokio::test]
    async fn generate_wrapped_master_key_writes_v1_file() {
        let (_dir, path) = temp_master_key();
        let id = generate_wrapped_master_key(&path, &test_passphrase())
            .await
            .expect("gen");
        // File should exist and be V1 wrapped.
        let bytes = tokio::fs::read(&path).await.expect("read");
        assert!(wrapped::is_wrapped_v1(&bytes));
        // The identity should be unwrappable with the same passphrase.
        let unwrapped = wrapped::unwrap(&test_passphrase(), &bytes).expect("unwrap");
        let parsed: Identity = unwrapped.trim().parse().expect("parse");
        assert_eq!(
            parsed.to_public().to_string(),
            id.to_public().to_string(),
            "generated identity must match what's in the file"
        );
    }
}
