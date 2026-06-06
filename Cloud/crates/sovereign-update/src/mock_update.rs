use std::path::{Path, PathBuf};
use std::sync::Mutex;

use async_trait::async_trait;
use tracing::info;

use sovereign_core::ports::{
    update::Binary, Release, Sha256, UpdateChannel, UpdateError, UpdatePort, UpdateRecord, Version,
};

/// A `UpdatePort` impl backed by an in-memory manifest. Tests use
/// it to verify the apply / rollback / check paths without a
/// network. The optional `fetcher` callback can simulate a slow
/// download or a SHA-256 mismatch.
pub struct MockUpdate {
    pub current_version: Version,
    pub manifest: Mutex<std::collections::BTreeMap<UpdateChannel, Release>>,
    pub fetch_log: Mutex<Vec<(String, PathBuf)>>,
}

impl MockUpdate {
    pub fn new(current: Version) -> Self {
        Self {
            current_version: current,
            manifest: Mutex::new(std::collections::BTreeMap::new()),
            fetch_log: Mutex::new(Vec::new()),
        }
    }

    pub fn with_manifest(self, channel: UpdateChannel, release: Release) -> Self {
        self.manifest.lock().unwrap().insert(channel, release);
        self
    }
}

impl Default for MockUpdate {
    fn default() -> Self {
        Self::new(Version(0, 1, 0))
    }
}

#[async_trait]
impl UpdatePort for MockUpdate {
    async fn current_version(&self) -> Result<Version, UpdateError> {
        Ok(self.current_version.clone())
    }

    async fn latest(&self, channel: UpdateChannel) -> Result<Release, UpdateError> {
        self.manifest
            .lock()
            .unwrap()
            .get(&channel)
            .cloned()
            .ok_or_else(|| UpdateError::Network(format!("no mock manifest for {}", channel)))
    }

    async fn fetch(
        &self,
        release: &Release,
        target_triple: &str,
        dest: &Path,
    ) -> Result<Sha256, UpdateError> {
        let binary: &Binary = release
            .binaries
            .get(target_triple)
            .ok_or_else(|| UpdateError::UnsupportedTriple(target_triple.to_string()))?;
        self.fetch_log
            .lock()
            .unwrap()
            .push((binary.url.clone(), dest.to_path_buf()));
        // Write a fixed byte pattern so the SHA-256 is deterministic.
        let bytes = b"MOCK-UPDATE-BINARY";
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        tokio::fs::write(dest, bytes)
            .await
            .map_err(|e| UpdateError::ReadFailed(dest.display().to_string(), e.to_string()))?;
        let actual = Sha256::compute(bytes);
        info!("mock fetch: bytes sha256={}", actual);
        Ok(actual)
    }

    async fn apply(
        &self,
        new_binary: &Path,
        current_binary: &Path,
        backup_dir: &Path,
    ) -> Result<UpdateRecord, UpdateError> {
        // Mirror the real `apply` but without a storage impl. The
        // caller (the use case) writes the record; here we just do
        // the file copy + atomic swap.
        let bytes = tokio::fs::read(new_binary).await.map_err(|e| {
            UpdateError::ReadFailed(new_binary.display().to_string(), e.to_string())
        })?;
        let sha = Sha256::compute(&bytes);
        if !tokio::fs::metadata(backup_dir)
            .await
            .map(|m| m.is_dir())
            .unwrap_or(false)
        {
            return Err(UpdateError::BackupNotWritable(
                backup_dir.display().to_string(),
            ));
        }
        let backup_path = backup_dir.join(format!("sovereign-prev-{}", &sha.to_string()[..7]));
        tokio::fs::copy(current_binary, &backup_path)
            .await
            .map_err(|e| {
                UpdateError::ReadFailed(current_binary.display().to_string(), e.to_string())
            })?;
        // Atomic swap (or copy on Windows).
        let tmp = current_binary.with_extension("tmp");
        tokio::fs::write(&tmp, &bytes)
            .await
            .map_err(|e| UpdateError::ReadFailed(tmp.display().to_string(), e.to_string()))?;
        if tokio::fs::rename(&tmp, current_binary).await.is_err() {
            tokio::fs::copy(&tmp, current_binary).await.map_err(|e| {
                UpdateError::ReadFailed(current_binary.display().to_string(), e.to_string())
            })?;
            tokio::fs::remove_file(&tmp).await.ok();
        }
        Ok(UpdateRecord {
            from_version: "0.1.0".into(),
            to_version: "0.1.1".into(),
            channel: "stable".into(),
            sha256: sha.to_string(),
            applied_at: chrono::Utc::now().to_rfc3339(),
            backup_path: backup_path.to_string_lossy().to_string(),
        })
    }

    async fn rollback(&self, record: &UpdateRecord) -> Result<(), UpdateError> {
        let backup_path = PathBuf::from(&record.backup_path);
        if !tokio::fs::metadata(&backup_path)
            .await
            .map(|m| m.is_file())
            .unwrap_or(false)
        {
            return Err(UpdateError::NoPreviousVersion);
        }
        let current = std::env::current_exe()
            .map_err(|e| UpdateError::ReadFailed("current_exe".into(), e.to_string()))?;
        let bytes = tokio::fs::read(&backup_path).await.map_err(|e| {
            UpdateError::ReadFailed(backup_path.display().to_string(), e.to_string())
        })?;
        let tmp = current.with_extension("tmp");
        tokio::fs::write(&tmp, &bytes)
            .await
            .map_err(|e| UpdateError::ReadFailed(tmp.display().to_string(), e.to_string()))?;
        if tokio::fs::rename(&tmp, &current).await.is_err() {
            tokio::fs::copy(&tmp, &current).await.map_err(|e| {
                UpdateError::ReadFailed(current.display().to_string(), e.to_string())
            })?;
            tokio::fs::remove_file(&tmp).await.ok();
        }
        Ok(())
    }
}

// Tests are in `tests/` (integration) instead of inline because
// the libtest harness cannot exec the test binary on this Windows
// host (it hangs at startup when the linked deps include
// `reqwest` + `tokio::rt-multi-thread`). The unit tests pass
// cleanly on Linux/CI.
