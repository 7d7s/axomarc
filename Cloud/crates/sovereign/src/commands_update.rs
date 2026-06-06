// F10: `sovereign update ...` subcommand handlers.
//
// The V0 binary supports four subcommands:
//
//   * `update check`     -- fetch the manifest, print the latest version
//   * `update apply`     -- fetch, verify, atomic-swap, record
//   * `update rollback`  -- restore the previous binary
//   * `update history`   -- list the last N records in `update_history`
//
// The HTTP manifest base defaults to
// `https://releases.sovereignruntime.dev`; the operator can override it
// with `--manifest` or `SOVEREIGN_UPDATE_MANIFEST`. The storage backing
// is the same `SqliteState` the other commands use; the path is the
// `default_db_path()` shared with `commands_deploy`.

use std::path::PathBuf;
use std::sync::Arc;

use serde::Serialize;
use sovereign_core::ports::update::{Sha256, UpdateChannel, UpdateError, UpdateRecord, Version};
use sovereign_core::ports::StoragePort;
use sovereign_core::state::no_runtime;
use sovereign_storage_sqlite::SqliteState;
use sovereign_update::{HttpUpdate, UpdatePort};
use tracing::warn;

use crate::commands::Dispatch;
use crate::commands_deploy::{default_backup_dir, default_db_path};
use crate::exit::AppExit;
use crate::output::{Envelope, Output};

const DEFAULT_MANIFEST_BASE: &str = "https://releases.sovereignruntime.dev";

/// `sovereign update check`. Returns 0 if the local binary is up to date
/// (or no manifest is configured), 2 if a newer version is available.
pub async fn run_check(out: &Output, channel: UpdateChannel, manifest: Option<&str>) -> Dispatch {
    let port = HttpUpdate::new(manifest.unwrap_or(DEFAULT_MANIFEST_BASE));
    let current = match port.current_version().await {
        Ok(v) => v,
        Err(e) => return err(out, &format!("cannot read current version: {e}")),
    };
    let latest = match port.latest(channel).await {
        Ok(r) => r,
        Err(UpdateError::Network(msg)) => {
            // Manifest endpoint is not reachable (offline, behind firewall,
            // first-boot). Treat that as "no update available" rather than
            // an error -- the operator might be air-gapped.
            let _ = out.warn(&format!(
                "manifest not reachable ({msg}); treating as 'no update available'"
            ));
            return Dispatch::Ok;
        }
        Err(e) => return err(out, &format!("manifest fetch failed: {e}")),
    };
    let latest_version = match Version::parse(&latest.version) {
        Ok(v) => v,
        Err(e) => {
            return err(
                out,
                &format!("manifest has invalid version '{}': {e}", latest.version),
            );
        }
    };
    let payload = CheckData {
        current: current.to_string(),
        latest: latest_version.to_string(),
        channel: channel.as_str().to_string(),
        released_at: latest.released_at.clone(),
        update_available: latest_version > current,
    };
    if out.format() == crate::output::Format::Text {
        if latest_version > current {
            let _ = out.warn(&format!(
                "{} -> {} available on channel '{}' (released {})",
                current, latest_version, channel, latest.released_at
            ));
        } else {
            let _ = out.ok(&format!(
                "up to date (current={} latest={} channel={})",
                current, latest_version, channel
            ));
        }
    } else {
        let env = Envelope::<CheckData>::ok(payload);
        let _ = out.success(&env);
    }
    if latest_version > current {
        Dispatch::Err(AppExit::Partial)
    } else {
        Dispatch::Ok
    }
}

/// `sovereign update apply`. Requires a storage-backed AppState so the
/// `update_history` row is written. `--no-swap` downloads + verifies
/// only.
pub async fn run_apply(
    out: &Output,
    channel: UpdateChannel,
    target: Option<&str>,
    manifest: Option<&str>,
    no_swap: bool,
) -> Dispatch {
    let manifest_base = manifest.unwrap_or(DEFAULT_MANIFEST_BASE).to_string();
    let port = HttpUpdate::new(&manifest_base);
    let target_triple = target
        .map(String::from)
        .unwrap_or_else(default_target_triple);
    let current = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return err(out, &format!("cannot read current_exe: {e}")),
    };
    if !no_swap && !is_writable(&current) {
        return err(
            out,
            "current binary is not writable; refusing to swap (run as root or with CAP_DAC_OVERRIDE)",
        );
    }
    let latest = match port.latest(channel).await {
        Ok(r) => r,
        Err(e) => return err(out, &format!("manifest fetch failed: {e}")),
    };
    let dest = std::env::temp_dir().join(format!(
        "sovereign-new-{}",
        Sha256::compute(latest.version.as_bytes())
    ));
    if let Err(e) = port.fetch(&latest, &target_triple, &dest).await {
        return err(out, &format!("download failed: {e}"));
    }
    let bytes = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    let _ = out.ok(&format!("downloaded {bytes} bytes to {}", dest.display()));
    if no_swap {
        let _ = out
            .ok("--no-swap set; not applying. binary is at the path above for manual inspection.");
        return Dispatch::Ok;
    }
    let storage = match build_storage().await {
        Ok(s) => s,
        Err(e) => return err(out, &e),
    };
    let backup_dir: PathBuf = default_backup_dir();
    if tokio::fs::metadata(&backup_dir).await.is_err() {
        if let Err(e) = tokio::fs::create_dir_all(&backup_dir).await {
            return err(
                out,
                &format!("cannot create backup dir {}: {e}", backup_dir.display()),
            );
        }
    }
    let record = match sovereign_core::use_cases::update::apply_update(
        &dest,
        &current,
        &backup_dir,
        storage,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => return err(out, &format!("apply failed: {e}")),
    };
    if out.format() == crate::output::Format::Text {
        let _ = out.ok(&format!(
            "applied {} (sha256={}, backup at {})",
            record.to_version,
            &record.sha256[..12],
            record.backup_path
        ));
    } else {
        let env = Envelope::<ApplyData>::ok(ApplyData {
            from_version: record.from_version.clone(),
            to_version: record.to_version.clone(),
            channel: record.channel.clone(),
            sha256: record.sha256.clone(),
            backup_path: record.backup_path.clone(),
        });
        let _ = out.success(&env);
    }
    Dispatch::Ok
}

/// `sovereign update rollback`. Reads the most recent `update_history`
/// row that has not been rolled back, then swaps back to the binary
/// at `record.backup_path`.
pub async fn run_rollback(out: &Output, _manifest: Option<&str>) -> Dispatch {
    let storage = match build_storage().await {
        Ok(s) => s,
        Err(e) => return err(out, &e),
    };
    let last = match storage.last_update().await {
        Ok(Some(r)) => r,
        Ok(None) => return err(out, "no previous update to roll back to"),
        Err(e) => return err(out, &format!("cannot read update history: {e}")),
    };
    let current = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => return err(out, &format!("cannot read current_exe: {e}")),
    };
    if let Err(e) =
        sovereign_core::use_cases::update::rollback_update(&last, &current, storage.clone()).await
    {
        return err(out, &format!("rollback failed: {e}"));
    }
    if let Err(e) = storage.mark_update_rolled_back(&last.sha256).await {
        warn!("rollback succeeded but cannot stamp update_history: {e}");
    }
    if out.format() == crate::output::Format::Text {
        let _ = out.ok(&format!(
            "rolled back to {} (sha256={})",
            last.from_version,
            &last.sha256[..12]
        ));
    } else {
        let env = Envelope::<RollbackData>::ok(RollbackData {
            from_version: last.from_version.clone(),
            to_version: last.to_version.clone(),
            sha256: last.sha256.clone(),
        });
        let _ = out.success(&env);
    }
    Dispatch::Ok
}

/// `sovereign update history`. Newest first.
pub async fn run_history(out: &Output, limit: u32) -> Dispatch {
    let storage = match build_storage().await {
        Ok(s) => s,
        Err(e) => return err(out, &e),
    };
    let rows = match storage.list_updates(limit).await {
        Ok(v) => v,
        Err(e) => return err(out, &format!("cannot read update history: {e}")),
    };
    if out.format() == crate::output::Format::Text {
        if rows.is_empty() {
            let _ = out.ok("no updates recorded");
        } else {
            for r in &rows {
                let _ = out.ok(&format!(
                    "{} -> {}  channel={}  sha256={}  applied_at={}",
                    r.from_version,
                    r.to_version,
                    r.channel,
                    &r.sha256[..12],
                    r.applied_at
                ));
            }
        }
    } else {
        let env = Envelope::<Vec<UpdateRecord>>::ok(rows);
        let _ = out.success(&env);
    }
    Dispatch::Ok
}

// --- helpers ---------------------------------------------------------------

/// Open the storage impl at `default_db_path()`. Returns a `String` error
/// so the caller can `format!` it into the user-facing message.
async fn build_storage() -> Result<Arc<dyn StoragePort>, String> {
    let db_path = default_db_path();
    SqliteState::open(&db_path)
        .await
        .map(|s| Arc::new(s) as Arc<dyn StoragePort>)
        .map_err(|e| {
            format!(
                "cannot open data dir at {}: {e}. Run `sovereign init` first?",
                db_path.display()
            )
        })
}

fn default_target_triple() -> String {
    // `cargo` doesn't expose the target triple at runtime; we
    // construct a best-effort string. Operators can override with
    // --target. The manifest is expected to have a key for this.
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    let env_part = if cfg!(target_env = "musl") {
        "musl"
    } else if cfg!(target_env = "gnu") {
        "gnu"
    } else {
        "unknown"
    };
    format!("{arch}-{os}-{env_part}")
}

fn is_writable(path: &std::path::Path) -> bool {
    std::fs::metadata(path)
        .map(|m| !m.permissions().readonly())
        .unwrap_or(false)
}

fn err(out: &Output, msg: &str) -> Dispatch {
    let _ = out.err(&format!("update: {msg}"));
    Dispatch::Err(AppExit::Generic)
}

// Silence the unused-import lint for things only used in JSON envelope
// paths. Cheap to keep them all around; the compiler strips them.
#[allow(dead_code)]
fn _ensure_no_runtime_in_scope() -> Arc<dyn sovereign_core::ports::RuntimePort> {
    no_runtime()
}

// --- envelope payloads -----------------------------------------------------

#[derive(Serialize)]
struct CheckData {
    current: String,
    latest: String,
    channel: String,
    released_at: String,
    update_available: bool,
}

#[derive(Serialize)]
struct ApplyData {
    from_version: String,
    to_version: String,
    channel: String,
    sha256: String,
    backup_path: String,
}

#[derive(Serialize)]
struct RollbackData {
    from_version: String,
    to_version: String,
    sha256: String,
}
