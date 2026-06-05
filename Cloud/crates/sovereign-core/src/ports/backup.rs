// `BackupSink` — the port the use cases know about for storing and
// retrieving SQLite snapshots. The V0 implementation is a filesystem
// directory (`/var/lib/sovereign/backups/`); V1 will gain an S3
// adapter. The snapshot itself is produced by `StoragePort::vacuum_into`
// — this port only deals with the bytes on disk, not with the
// database engine. See `docs/phase-00-mvp.md` F8a.

use async_trait::async_trait;

use crate::domain::BackupId;
use crate::error::AppError;

/// The single state-mutation port for backup persistence. One
/// implementation per storage backend; the V0 default is the
/// filesystem (`FileBackupSink` in `sovereign-backup`).
#[async_trait]
pub trait BackupSink: Send + Sync {
    /// Read the bytes of a snapshot from the sink. The returned
    /// `Vec<u8>` is the full SQLite database file (clean, defragmented)
    /// as produced by `VACUUM INTO`. Implementations should error
    /// (not return an empty `Vec`) if the snapshot is missing — that
    /// is a fatal corruption signal.
    async fn read(&self, id: BackupId) -> Result<Vec<u8>, AppError>;

    /// List the IDs of every snapshot currently held in the sink.
    /// Order is undefined; the caller can sort. The doctor and CLI
    /// commands rely on this to cross-check against the `backup` table.
    async fn list(&self) -> Result<Vec<BackupId>, AppError>;

    /// On-disk size in bytes of a single snapshot. Cheaper than
    /// `read()` when the caller only wants the size (e.g. for the
    /// doctor `backup_size_sane` check).
    async fn size(&self, id: BackupId) -> Result<u64, AppError>;

    /// Remove a snapshot. Returns `Ok(())` if the file is already gone
    /// (idempotent — `sovereign backup restore` may delete an old
    /// snapshot after a successful overwrite).
    async fn delete(&self, id: BackupId) -> Result<(), AppError>;

    /// Where a snapshot of `id` is stored. Used by `sovereign backup
    /// list` to print the location and by the doctor to check that the
    /// path is inside the configured base directory.
    fn location(&self, id: BackupId) -> String;
}
