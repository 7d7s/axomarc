//! Backup adapter. V0 ships the filesystem sink; V1 will add the
//! encrypted S3 sink and Litestream-style WAL streaming. The
//! snapshot itself (a clean, defragmented copy of the SQLite
//! database) is produced by `StoragePort::vacuum_into`; this crate
//! only deals with storing and reading the resulting bytes.
//!
//! See `docs/phase-00-mvp.md` F8a for the full spec.

#![deny(unsafe_code)]
#![allow(missing_docs)]

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use sovereign_core::domain::BackupId;
use sovereign_core::error::AppError;
use sovereign_core::ports::BackupSink;
use tracing::{info, instrument, warn};

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Filesystem backend: one `<id>.db` file per snapshot under a base
/// directory. V0 default. On a fresh install the base directory is
/// `/var/lib/sovereign/backups/`.
#[derive(Debug, Clone)]
pub struct FileBackupSink {
    base_dir: PathBuf,
}

impl FileBackupSink {
    /// Construct a sink rooted at `base_dir`. The directory is created
    /// (recursively) on the first write; the constructor itself does
    /// not touch the filesystem so the doctor check can probe
    /// `base_dir` without side effects.
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// The on-disk path a snapshot of `id` would occupy.
    pub fn path_for(&self, id: BackupId) -> PathBuf {
        self.base_dir.join(format!("{id}.db"))
    }

    async fn ensure_base(&self) -> Result<(), AppError> {
        if self.base_dir.as_os_str().is_empty() {
            return Err(AppError::Storage("backup base_dir is empty".into()));
        }
        tokio::fs::create_dir_all(&self.base_dir)
            .await
            .map_err(|e| {
                AppError::Storage(format!(
                    "create backup dir {}: {e}",
                    self.base_dir.display()
                ))
            })?;
        Ok(())
    }
}

#[async_trait]
impl BackupSink for FileBackupSink {
    #[instrument(skip(self))]
    async fn read(&self, id: BackupId) -> Result<Vec<u8>, AppError> {
        let path = self.path_for(id);
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(AppError::Storage(format!(
                "backup {id} not found at {}",
                path.display()
            ))),
            Err(e) => Err(AppError::Storage(format!(
                "read backup {id} from {}: {e}",
                path.display()
            ))),
        }
    }

    #[instrument(skip(self))]
    async fn list(&self) -> Result<Vec<BackupId>, AppError> {
        if !self.base_dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut rd = tokio::fs::read_dir(&self.base_dir)
            .await
            .map_err(|e| AppError::Storage(format!("read_dir {}: {e}", self.base_dir.display())))?;
        while let Some(entry) = rd
            .next_entry()
            .await
            .map_err(|e| AppError::Storage(format!("iter {}: {e}", self.base_dir.display())))?
        {
            let name = match entry.file_name().into_string() {
                Ok(s) => s,
                Err(_) => continue, // non-UTF8 entry, skip
            };
            if !name.ends_with(".db") {
                continue;
            }
            let id_str = name.trim_end_matches(".db");
            if id_str.len() != 36 {
                warn!(file = %name, "skipping non-BackupId file in backup dir");
                continue;
            }
            // Avoid a hard dep on the Uuid type by parsing defensively.
            match uuid::Uuid::parse_str(id_str) {
                Ok(u) => out.push(BackupId(u)),
                Err(_) => {
                    warn!(file = %name, "skipping non-BackupId file in backup dir");
                }
            }
        }
        Ok(out)
    }

    #[instrument(skip(self))]
    async fn size(&self, id: BackupId) -> Result<u64, AppError> {
        let path = self.path_for(id);
        let meta = tokio::fs::metadata(&path).await.map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppError::Storage(format!("backup {id} not found at {}", path.display()))
            } else {
                AppError::Storage(format!("stat backup {id}: {e}"))
            }
        })?;
        Ok(meta.len())
    }

    #[instrument(skip(self))]
    async fn delete(&self, id: BackupId) -> Result<(), AppError> {
        let path = self.path_for(id);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => {
                info!(id = %id, path = %path.display(), "backup deleted");
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Idempotent — operator may rerun restore after a partial failure.
                Ok(())
            }
            Err(e) => Err(AppError::Storage(format!(
                "delete backup {id} at {}: {e}",
                path.display()
            ))),
        }
    }

    fn location(&self, id: BackupId) -> String {
        self.path_for(id).display().to_string()
    }
}

impl FileBackupSink {
    /// `true` iff `path` is the path this sink would use for any
    /// snapshot. Used by the doctor `backup_dir_sane` check to make
    /// sure `restore --to` is not pointing outside the sink.
    pub fn contains(&self, path: &Path) -> bool {
        path.starts_with(&self.base_dir)
    }

    /// `true` iff the base directory is on a writable filesystem.
    /// The doctor uses this for the `backup_dir_writable` check.
    pub async fn is_writable(&self) -> bool {
        match self.ensure_base().await {
            Ok(()) => {
                let probe = self.base_dir.join(".sovereign-write-probe");
                if tokio::fs::write(&probe, b"ok").await.is_err() {
                    return false;
                }
                tokio::fs::remove_file(&probe).await.is_ok()
            }
            Err(_) => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id() -> BackupId {
        BackupId(uuid::Uuid::new_v4())
    }

    #[tokio::test]
    async fn read_missing_errors_not_panics() {
        let dir = tempfile::tempdir().unwrap();
        let sink = FileBackupSink::new(dir.path());
        let res = sink.read(id()).await;
        assert!(matches!(res, Err(AppError::Storage(_))));
    }

    #[tokio::test]
    async fn delete_missing_is_ok() {
        let dir = tempfile::tempdir().unwrap();
        let sink = FileBackupSink::new(dir.path());
        assert!(sink.delete(id()).await.is_ok());
    }

    #[tokio::test]
    async fn size_missing_errors() {
        let dir = tempfile::tempdir().unwrap();
        let sink = FileBackupSink::new(dir.path());
        let res = sink.size(id()).await;
        assert!(matches!(res, Err(AppError::Storage(_))));
    }

    #[tokio::test]
    async fn list_empty_dir_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let sink = FileBackupSink::new(dir.path());
        let ids = sink.list().await.unwrap();
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn list_skips_non_uuid_files() {
        let dir = tempfile::tempdir().unwrap();
        let sink = FileBackupSink::new(dir.path());
        sink.ensure_base().await.unwrap();
        // Garbage file — should be skipped, not panic.
        std::fs::write(dir.path().join("not-a-uuid.db"), b"x").unwrap();
        std::fs::write(dir.path().join("README"), b"x").unwrap();
        let ids = sink.list().await.unwrap();
        assert!(ids.is_empty());
    }

    #[tokio::test]
    async fn is_writable_succeeds_for_tempdir() {
        let dir = tempfile::tempdir().unwrap();
        let sink = FileBackupSink::new(dir.path());
        assert!(sink.is_writable().await);
    }

    #[tokio::test]
    async fn location_format_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let sink = FileBackupSink::new(dir.path());
        let i = id();
        let loc = sink.location(i);
        // Must round-trip the suffix and the .db extension.
        assert!(loc.ends_with(&format!("{i}.db")));
        assert!(loc.contains(dir.path().to_str().unwrap()));
    }
}
