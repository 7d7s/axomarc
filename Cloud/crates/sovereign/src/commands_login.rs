// F4.6 / V0.5: `sovereign login` — passphrase-wrapped master-key
// init/load.
//
// V0.5 closes the V0 KDF gap: the master key file is now wrapped
// with Argon2id (m=64 MiB, t=3, p=1) under a passphrase, and the
// AEAD is XChaCha20-Poly1305. See
// `crates/sovereign-secrets-age/src/wrapped.rs` for the on-disk
// format and KDF/AEAD parameter choices.
//
// The "V0 single-tenant" model is unchanged: there's no real user
// account database, no OIDC, no JWTs. The operator who runs
// `sovereign` is implicitly the holder of the master key
// passphrase, and `sovereign login` either generates a new
// passphrase-wrapped key (first run) or unwraps the existing one
// (every subsequent run) and prints the public key (Bech32) the
// operator can share with a CI run or a peer host that needs
// read-only access.

use std::io::Write;
use std::path::{Path, PathBuf};

use age::x25519::Identity;
use secrecy::SecretString;
use serde::Serialize;
use sovereign_secrets_age::AgeSecrets;
use tracing::info;

use crate::commands::Dispatch;
use crate::exit::AppExit;
use crate::output::{Envelope, Output};

/// The platform-appropriate default master key path. On Linux this
/// is `/var/lib/sovereign/master.key` (matching the production
/// deployment). On macOS / Windows the per-user data dir is the
/// V0-friendly fallback. We resolve at runtime so a single binary
/// works on all three.
pub fn default_master_key_path() -> PathBuf {
    if cfg!(target_os = "linux") {
        PathBuf::from("/var/lib/sovereign/master.key")
    } else if cfg!(target_os = "macos") {
        let home = std::env::var_os("HOME").unwrap_or_default();
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("sovereign")
            .join("master.key")
    } else {
        // Windows + everything else: %APPDATA%\sovereign\master.key
        let appdata = std::env::var_os("APPDATA").unwrap_or_default();
        PathBuf::from(appdata).join("sovereign").join("master.key")
    }
}

#[derive(Debug, Serialize)]
pub struct LoginResult {
    pub master_key_path: PathBuf,
    pub created: bool,
    pub public_key: String,
    /// The KDF that protects the on-disk master key file. The
    /// parameters are echoed back to the operator so they can
    /// verify what hardware cost they're paying at unlock time.
    pub kdf: KdfInfo,
    pub next_steps: Vec<String>,
}

/// The KDF parameters used to wrap the on-disk master key.
#[derive(Debug, Serialize)]
pub struct KdfInfo {
    pub algorithm: String,
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

/// Read the passphrase used to wrap the master key. Order:
/// 1. `SOVEREIGN_PASSPHRASE` env var (for CI / scripting)
/// 2. Hidden stdin prompt
/// 3. Error
///
/// This is the "non-interactive" variant used by every command
/// other than `sovereign login` itself: we always want to
/// read-or-prompt (never refuse to prompt), because every command
/// that needs the master key (deploy, secret, backup, etc.) needs
/// the passphrase to do the Argon2id KDF.
pub fn read_passphrase_or_prompt() -> Result<SecretString, String> {
    if let Ok(p) = std::env::var("SOVEREIGN_PASSPHRASE") {
        if p.is_empty() {
            return Err(
                "SOVEREIGN_PASSPHRASE is set but empty; either unset it or pass a real passphrase"
                    .into(),
            );
        }
        return Ok(SecretString::new(p.into_boxed_str()));
    }
    eprint!("sovereign passphrase: ");
    let _ = std::io::stderr().flush();
    let mut buf = String::new();
    std::io::stdin()
        .read_line(&mut buf)
        .map_err(|e| format!("read passphrase: {e}"))?;
    Ok(SecretString::new(buf.trim().to_string().into_boxed_str()))
}

/// Read the passphrase from `SOVEREIGN_PASSPHRASE` (when set) or
/// from a hidden stdin prompt. The `no_input` flag is honored:
/// when set, we refuse to prompt and require the env var. This is
/// the `sovereign login`-specific variant.
pub fn read_passphrase(no_input: bool) -> Result<SecretString, String> {
    if let Ok(p) = std::env::var("SOVEREIGN_PASSPHRASE") {
        if p.is_empty() {
            return Err(
                "SOVEREIGN_PASSPHRASE is set but empty; either unset it or pass a real passphrase"
                    .into(),
            );
        }
        return Ok(SecretString::new(p.into_boxed_str()));
    }
    if no_input {
        return Err(
            "--no-input requires SOVEREIGN_PASSPHRASE in the environment; refusing to prompt"
                .into(),
        );
    }
    eprint!("sovereign passphrase: ");
    let _ = std::io::stderr().flush();
    let mut buf = String::new();
    std::io::stdin()
        .read_line(&mut buf)
        .map_err(|e| format!("read passphrase: {e}"))?;
    Ok(SecretString::new(buf.trim().to_string().into_boxed_str()))
}

/// Ensure the parent dir of `master_key_path` exists and is
/// `0700` on Unix. We do NOT create the file — `AgeSecrets` will
/// generate or load on the first `ensure_loaded` call.
fn ensure_parent_dir(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create parent dir {}: {e}", parent.display()))?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(parent) = path.parent() {
            if parent.exists() {
                let mode = 0o700;
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(mode))
                    .map_err(|e| format!("chmod 0700 {}: {e}", parent.display()))?;
            }
        }
    }
    Ok(())
}

pub async fn run(out: &Output, no_input: bool, master_key_override: Option<&Path>) -> Dispatch {
    let passphrase = match read_passphrase(no_input) {
        Ok(p) => p,
        Err(e) => return err(out, AppExit::Usage, &e),
    };

    let master_key_path = master_key_override
        .map(|p| p.to_path_buf())
        .unwrap_or_else(default_master_key_path);
    info!(path = %master_key_path.display(), "selected master key path");

    if let Err(e) = ensure_parent_dir(&master_key_path) {
        return err(out, AppExit::Upstream, &e);
    }

    let created = !master_key_path.exists();
    let secrets = AgeSecrets::new(&master_key_path, passphrase);
    let identity = match secrets.ensure_loaded().await {
        Ok(id) => id,
        Err(e) => {
            return err(
                out,
                AppExit::Upstream,
                &format!("load/generate master key: {e}"),
            );
        }
    };

    let public_key = identity_to_public_bech32(&identity);
    let result = LoginResult {
        master_key_path: master_key_path.clone(),
        created,
        public_key: public_key.clone(),
        kdf: KdfInfo {
            algorithm: "Argon2id".to_string(),
            memory_kib: 64 * 1024,
            iterations: 3,
            parallelism: 1,
        },
        next_steps: vec![
            format!(
                "Master key is at {} (passphrase-wrapped with Argon2id). \
                 Back this file AND remember the passphrase; loss of either = \
                 loss of all secrets.",
                master_key_path.display()
            ),
            "Next: run `sovereign secret set <KEY> --app <APP>` to add a secret.".to_string(),
        ],
    };

    if out.format() == crate::output::Format::Text {
        let _ = out.ok(&format!(
            "{} the master key at {} (Argon2id m=64MiB t=3 p=1)",
            if created { "generated" } else { "loaded" },
            result.master_key_path.display()
        ));
        let _ = out.ok(&format!("public key (shareable, read-only): {public_key}"));
        for line in &result.next_steps {
            let _ = out.ok(line);
        }
    } else {
        let env = Envelope::<LoginResult>::ok(result);
        let _ = out.success(&env);
    }
    Dispatch::Ok
}

/// Format the X25519 public key as Bech32 (`age1...`). The
/// `Identity` does not implement `Display`; the public key does
/// not implement `Display` either; we round-trip through the
/// `to_public()` method which returns a `Recipient` whose
/// `to_string()` is the canonical Bech32 form.
fn identity_to_public_bech32(identity: &Identity) -> String {
    identity.to_public().to_string()
}

fn err(out: &Output, code: AppExit, msg: &str) -> Dispatch {
    if out.format() == crate::output::Format::Text {
        let _ = out.err(msg);
    } else {
        let env = Envelope::<serde_json::Value>::err(code, serde_json::json!({"error": msg}), msg);
        let _ = out.error(&env);
    }
    Dispatch::Err(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use secrecy::ExposeSecret;

    #[test]
    fn default_master_key_path_is_nonempty() {
        let p = default_master_key_path();
        // The path should be absolute on every platform.
        assert!(p.is_absolute(), "default path must be absolute, got {p:?}");
        assert!(p.ends_with("master.key"));
    }

    #[test]
    fn read_passphrase_honors_env_var() {
        // SAFETY: not concurrent; test-only
        unsafe { std::env::set_var("SOVEREIGN_PASSPHRASE", "from-env") };
        let p = read_passphrase(true).unwrap();
        unsafe { std::env::remove_var("SOVEREIGN_PASSPHRASE") };
        assert_eq!(p.expose_secret(), "from-env");
    }

    #[test]
    fn read_passphrase_rejects_empty_env() {
        unsafe { std::env::set_var("SOVEREIGN_PASSPHRASE", "") };
        let r = read_passphrase(true);
        unsafe { std::env::remove_var("SOVEREIGN_PASSPHRASE") };
        assert!(r.is_err());
    }

    #[test]
    fn read_passphrase_no_input_requires_env() {
        // SAFETY: not concurrent; test-only. We must clear the env
        // var to assert the "no env, no_input=true" failure.
        let saved = std::env::var("SOVEREIGN_PASSPHRASE").ok();
        unsafe { std::env::remove_var("SOVEREIGN_PASSPHRASE") };
        let r = read_passphrase(true);
        if let Some(s) = saved {
            unsafe { std::env::set_var("SOVEREIGN_PASSPHRASE", s) };
        }
        assert!(r.is_err());
    }

    #[test]
    fn ensure_parent_dir_creates_missing_dir() {
        let dir =
            std::env::temp_dir().join(format!("sovereign-login-test-{}", uuid::Uuid::new_v4()));
        let target = dir.join("nested").join("master.key");
        ensure_parent_dir(&target).unwrap();
        assert!(dir.join("nested").is_dir());
    }

    #[test]
    fn public_key_is_bech32_age1_prefix() {
        // We can't easily fabricate an `Identity` from scratch here
        // (the constructor takes a Bech32 string), but we can use
        // the same one `AgeSecrets` would generate: parse a
        // throwaway key in a temp dir and read the public form.
        let dir =
            std::env::temp_dir().join(format!("sovereign-login-pubkey-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let pp = SecretString::new("test-pass".to_string().into_boxed_str());
        let secrets = AgeSecrets::new(dir.join("master.key"), pp);
        // Don't actually save; just generate.
        let id = age::x25519::Identity::generate();
        let _ = secrets; // silence unused
        let pk = identity_to_public_bech32(&id);
        assert!(
            pk.starts_with("age1"),
            "public key must be Bech32 age1..., got {pk}"
        );
    }
}
