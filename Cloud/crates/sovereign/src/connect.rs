// Shared connect helpers. Extracted from commands_deploy.rs and
// commands_rollback.rs so the daemon, deploy, rollback, and any
// future long-running process share the same wiring.

use std::sync::Arc;

use sovereign_backup::FileBackupSink;
use sovereign_core::ports::{BackupSink, ProxyPort, SecretsPort, StoragePort};
use sovereign_proxy_caddy::CaddyProxy;
use sovereign_secrets_age::AgeSecrets;
use sovereign_storage_sqlite::SqliteState;

/// Default data-dir path. Linux: `~/.local/share/sovereign/`,
/// macOS: `~/Library/Application Support/sovereign/`,
/// Windows: `%APPDATA%\sovereign\`.
pub fn default_data_dir() -> std::path::PathBuf {
    if let Some(dir) = dirs_data() {
        dir.join("sovereign")
    } else {
        std::path::PathBuf::from(".")
    }
}

/// Default SQLite database path (`<data_dir>/sovereign.db`).
pub fn default_db_path() -> std::path::PathBuf {
    default_data_dir().join("sovereign.db")
}

/// Default backup directory (next to the DB).
pub fn default_backup_dir() -> std::path::PathBuf {
    default_data_dir().join("backups")
}

/// Open or create the SQLite storage. Returns `None` on failure so
/// callers can surface a clean error.
#[allow(dead_code)]
pub async fn connect_storage() -> Option<Arc<dyn StoragePort>> {
    let db_path = default_db_path();
    match SqliteState::open(&db_path).await {
        Ok(s) => Some(Arc::new(s) as Arc<dyn StoragePort>),
        Err(e) => {
            tracing::error!(path = %db_path.display(), error = %e, "cannot open storage");
            None
        }
    }
}

/// Connect to the local Caddy admin API. Returns `None` if Caddy is
/// not reachable — callers fall back to the loopback placeholder URL.
pub async fn connect_proxy() -> Option<Arc<dyn ProxyPort>> {
    match CaddyProxy::connect_from_env().await {
        Ok(p) => {
            tracing::info!("caddy admin API reachable; routes will be wired");
            Some(Arc::new(p) as Arc<dyn ProxyPort>)
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "caddy admin API not reachable; deploy URL will be the loopback placeholder."
            );
            None
        }
    }
}

/// Open the age master key for secret decryption/encryption.
/// Returns `None` if the passphrase is missing or the key is not
/// provisioned — the caller opts out of env injection silently.
pub fn connect_secrets() -> Option<Arc<dyn SecretsPort>> {
    let passphrase = match crate::commands_login::read_passphrase_or_prompt() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "no passphrase available; secrets will NOT be injected. Set SOVEREIGN_PASSPHRASE or run `sovereign login` first."
            );
            return None;
        }
    };
    let secrets = AgeSecrets::with_default_path(passphrase);
    match secrets.encrypt(b"") {
        Ok(_) => Some(Arc::new(secrets) as Arc<dyn SecretsPort>),
        Err(e) => {
            tracing::warn!(
                error = %e,
                "master key not available; secrets will NOT be injected into the container."
            );
            None
        }
    }
}

/// Create the filesystem backup sink. Always succeeds — a missing
/// directory becomes a runtime error on the first `backup create`.
pub fn connect_backup() -> Option<Arc<dyn BackupSink>> {
    let dir = default_backup_dir();
    Some(Arc::new(FileBackupSink::new(dir)))
}

// --- platform helpers -------------------------------------------------------

#[cfg(unix)]
fn dirs_data() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| {
                let mut p = std::path::PathBuf::from(h);
                p.push(".local/share");
                p
            })
        })
}

#[cfg(windows)]
fn dirs_data() -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(std::path::PathBuf::from)
}
