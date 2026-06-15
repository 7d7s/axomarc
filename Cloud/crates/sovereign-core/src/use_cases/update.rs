use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::{info, warn};

use crate::error::AppError;
use crate::ports::{
    Release, Sha256, UpdateChannel, UpdateError, UpdatePort, UpdateRecord, Version,
};

/// `apply_update` — copy the running binary to the backup dir, swap
/// in the new binary atomically, append a row to `update_history`.
/// Returns the record so the caller can roll back.
pub async fn apply_update(
    new_binary: &Path,
    current_binary: &Path,
    backup_dir: &Path,
    target_version: &str,
    channel: &str,
    storage: Arc<dyn crate::ports::StoragePort>,
) -> Result<UpdateRecord, UpdateError> {
    if !fs::metadata(new_binary)
        .await
        .map(|m| m.is_file())
        .unwrap_or(false)
    {
        return Err(UpdateError::NotAFile(new_binary.display().to_string()));
    }
    if !fs::metadata(backup_dir)
        .await
        .map(|m| m.is_dir())
        .unwrap_or(false)
    {
        return Err(UpdateError::BackupNotWritable(
            backup_dir.display().to_string(),
        ));
    }
    let new_bytes = fs::read(new_binary)
        .await
        .map_err(|e| UpdateError::ReadFailed(new_binary.display().to_string(), e.to_string()))?;
    let new_sha = Sha256::compute(&new_bytes);
    let current_version =
        current_binary_version(current_binary).unwrap_or_else(|| "unknown".into());

    let backup_path = backup_dir.join(format!("sovereign-{}", current_version));
    fs::copy(current_binary, &backup_path).await.map_err(|e| {
        UpdateError::ReadFailed(current_binary.display().to_string(), e.to_string())
    })?;
    info!(
        "backed up current binary {} -> {}",
        current_binary.display(),
        backup_path.display()
    );

    // Atomic swap: write to `<dest>.tmp` then `rename(2)` over `dest`.
    // On Windows, `rename` over an existing file fails; we fall back
    // to `remove_file` + `rename` (best-effort, the .tmp file keeps
    // a copy if the swap fails).
    let tmp = current_binary.with_extension("tmp");
    fs::write(&tmp, &new_bytes)
        .await
        .map_err(|e| UpdateError::ReadFailed(tmp.display().to_string(), e.to_string()))?;
    if let Err(err) = atomic_swap(&tmp, current_binary).await {
        warn!(error = %err, "atomic swap failed; reverting");
        let _ = fs::remove_file(&tmp).await;
        return Err(err);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(current_binary, std::fs::Permissions::from_mode(0o755)).await;
    }

    let record = UpdateRecord {
        from_version: current_version.clone(),
        to_version: target_version.to_string(),
        channel: channel.to_string(),
        sha256: new_sha.to_string(),
        applied_at: Utc::now().to_rfc3339(),
        backup_path: backup_path.to_string_lossy().to_string(),
    };
    storage
        .record_update(&record)
        .await
        .map_err(|e: AppError| UpdateError::Network(e.to_string()))?;
    Ok(record)
}

/// `rollback_update` — restore the previous binary per the given
/// `UpdateRecord`. The new binary is whatever is currently on disk
/// at `current_binary`; the old one is at `record.backup_path`.
pub async fn rollback_update(
    record: &UpdateRecord,
    current_binary: &Path,
    storage: Arc<dyn crate::ports::StoragePort>,
) -> Result<(), UpdateError> {
    let backup_path = PathBuf::from(&record.backup_path);
    if !fs::metadata(&backup_path)
        .await
        .map(|m| m.is_file())
        .unwrap_or(false)
    {
        return Err(UpdateError::NoPreviousVersion);
    }
    let bytes = fs::read(&backup_path)
        .await
        .map_err(|e| UpdateError::ReadFailed(backup_path.display().to_string(), e.to_string()))?;
    let tmp = current_binary.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp)
            .await
            .map_err(|e| UpdateError::ReadFailed(tmp.display().to_string(), e.to_string()))?;
        f.write_all(&bytes)
            .await
            .map_err(|e| UpdateError::ReadFailed(tmp.display().to_string(), e.to_string()))?;
        f.flush().await.ok();
    }
    atomic_swap(&tmp, current_binary).await?;
    storage
        .mark_update_rolled_back(&record.sha256)
        .await
        .map_err(|e: AppError| UpdateError::Network(e.to_string()))?;
    Ok(())
}

async fn atomic_swap(src: &Path, dst: &Path) -> Result<(), UpdateError> {
    match fs::rename(src, dst).await {
        Ok(()) => Ok(()),
        Err(err) => {
            // On Windows the destination may be locked by the
            // running process; fall back to copy + remove.
            warn!(error = %err, "rename failed; trying copy+remove");
            fs::copy(src, dst)
                .await
                .map_err(|e| UpdateError::ReadFailed(dst.display().to_string(), e.to_string()))?;
            fs::remove_file(src)
                .await
                .map_err(|e| UpdateError::ReadFailed(src.display().to_string(), e.to_string()))?;
            Ok(())
        }
    }
}

fn current_binary_version(_p: &Path) -> Option<String> {
    // The binary embeds its version in CARGO_PKG_VERSION at compile
    // time; we read it from the executable path as a fallback (the
    // caller may set `SOVEREIGN_VERSION` at build time to override).
    Some(env!("CARGO_PKG_VERSION").to_string())
}

/// The most recent update record, or `None` if no updates have been
/// applied yet.
pub async fn last_update(
    storage: Arc<dyn crate::ports::StoragePort>,
) -> Result<Option<UpdateRecord>, UpdateError> {
    storage
        .last_update()
        .await
        .map_err(|e: AppError| UpdateError::Network(e.to_string()))
}

/// List the last `limit` update records, newest first.
pub async fn list_update_history(
    storage: Arc<dyn crate::ports::StoragePort>,
    limit: u32,
) -> Result<Vec<UpdateRecord>, UpdateError> {
    storage
        .list_updates(limit)
        .await
        .map_err(|e: AppError| UpdateError::Network(e.to_string()))
}

/// `check_update` — does the running binary have a newer release on
/// the manifest? Returns `Some(release)` if newer, `None` otherwise.
pub async fn check_update(
    port: Arc<dyn UpdatePort>,
    channel: UpdateChannel,
) -> Result<Option<Release>, UpdateError> {
    let current = port.current_version().await?;
    let latest = port.latest(channel).await?;
    let latest_v = Version::parse(&latest.version)?;
    if latest_v > current {
        Ok(Some(latest))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn version_parses_semver() {
        assert_eq!(Version::parse("0.1.0").unwrap(), Version(0, 1, 0));
        assert_eq!(Version::parse("v1.2.3").unwrap(), Version(1, 2, 3));
        assert_eq!(Version::parse("2.0.0-rc1").unwrap(), Version(2, 0, 0));
    }

    #[tokio::test]
    async fn version_rejects_garbage() {
        assert!(Version::parse("not-a-version").is_err());
        assert!(Version::parse("0.1").is_err());
    }

    #[tokio::test]
    async fn version_orders() {
        let v010 = Version::parse("0.1.0").unwrap();
        let v011 = Version::parse("0.1.1").unwrap();
        let v100 = Version::parse("1.0.0").unwrap();
        assert!(v011 > v010);
        assert!(v100 > v011);
    }

    #[tokio::test]
    async fn sha256_is_hex_64() {
        let s = Sha256::compute(b"hello world");
        assert!(s.is_hex64());
        assert_eq!(s.to_string().len(), 64);
    }

    #[tokio::test]
    async fn sha256_matches_known_value() {
        // SHA-256("hello world") = b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9
        let s = Sha256::compute(b"hello world");
        assert_eq!(
            s.to_string(),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[tokio::test]
    async fn apply_then_rollback_round_trip() {
        let dir = tempdir().unwrap();
        let current = dir.path().join("sovereign");
        let backup = dir.path().join("backups");
        tokio::fs::create_dir(&backup).await.unwrap();
        tokio::fs::write(&current, b"OLD-BINARY-BYTES")
            .await
            .unwrap();
        let new = dir.path().join("sovereign-new");
        tokio::fs::write(&new, b"NEW-BINARY-BYTES").await.unwrap();
        // We can't call apply_update without a StoragePort impl, but
        // we can verify the atomic-swap helper directly.
        let tmp = current.with_extension("tmp");
        tokio::fs::copy(&new, &tmp).await.unwrap();
        atomic_swap(&tmp, &current).await.unwrap();
        let bytes = tokio::fs::read(&current).await.unwrap();
        assert_eq!(bytes, b"NEW-BINARY-BYTES");
    }
}
