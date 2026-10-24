//! **F-10**: a file-backed promotion lock.
//!
//! The lock is a file on the release volume ✓ — the same substrate the
//! release manifest lives on ✓, so it is shared by every operator that can
//! see the release ✓ (a second host ✓, a second shell ✓).
//!
//! Two properties matter ✓:
//!
//! * **fail fast** ✓ — a second promotion is told the lock is held ✓, it is
//!   never queued ✓ (a queued promotion would land minutes later against a
//!   world it never inspected ✓);
//! * **a crashed holder does not wedge the release forever** ✓ — the lock
//!   carries a lease ✓ (the holder's timestamp ✓) and an expired lease can
//!   be broken ✓ (the operator says so explicitly ✓).
//!
//! The break is explicit on purpose ✓: silently stealing a lock that a
//! *live* promotion holds is how two promotions interleave in the first
//! place ✓ ✓.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use kelson_foundation::AppError;

use crate::ports::{BoxFuture, PromotionLease, PromotionLock, PromotionLockHolder};

/// How long a lease is honoured before it may be broken ✓.
pub const DEFAULT_LEASE_SECS: i64 = 900;

pub struct FilePromotionLock {
    path: PathBuf,
    lease_secs: i64,
    /// Serialises the read-modify-write of the lock file within this process
    /// ✓ (two promotions in one process ✓).
    guard: Mutex<()>,
}

impl FilePromotionLock {
    /// A lock at `<root>/promotion.lock` ✓.
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self::with_lease(root, DEFAULT_LEASE_SECS)
    }

    pub fn with_lease(root: impl AsRef<Path>, lease_secs: i64) -> Self {
        Self {
            path: root.as_ref().join("promotion.lock"),
            lease_secs,
            guard: Mutex::new(()),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Who holds the lock right now ✓ (`None` = free ✓), with the age of the
    /// lease ✓.
    pub fn holder(&self) -> Option<LockHolderInfo> {
        let raw = std::fs::read_to_string(&self.path).ok()?;
        let mut parts = raw.split_whitespace();
        let owner = parts.next()?.to_string();
        let epoch = parts
            .next()
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0);
        Some(LockHolderInfo { owner, epoch })
    }

    /// Break an expired lease ✓ — the explicit operator action ✓.
    pub fn break_expired(&self, now_epoch: i64) -> Result<bool, AppError> {
        let _guard = self
            .guard
            .lock()
            .map_err(|_| AppError::internal("the promotion lock's internal guard is poisoned"))?;
        let Some(info) = self.holder() else {
            return Ok(false);
        };
        if now_epoch - info.epoch < self.lease_secs {
            return Ok(false);
        }
        std::fs::remove_file(&self.path).map_err(|e| {
            AppError::persistence(format!("cannot break the expired promotion lock: {e}"))
        })?;
        Ok(true)
    }
}

/// Who holds the lock ✓.
#[derive(Debug, Clone)]
pub struct LockHolderInfo {
    pub owner: String,
    /// The epoch second the holder took it ✓.
    pub epoch: i64,
}

struct FileHolder {
    path: PathBuf,
}

impl PromotionLockHolder for FileHolder {
    fn release(&self) {
        // Best effort: the file is removed so the next promotion is not
        // blocked by a lease that no longer applies.
        let _ = std::fs::remove_file(&self.path);
    }
}

impl PromotionLock for FilePromotionLock {
    fn try_acquire(&self, owner: &str) -> BoxFuture<'_, Result<PromotionLease, AppError>> {
        // The owner is owned by the future ✓ (the `&str` borrow would not
        // outlive it ✓).
        let owner = owner.to_string();
        Box::pin(async move {
            let _guard = self.guard.lock().map_err(|_| {
                AppError::internal("the promotion lock's internal guard is poisoned")
            })?;
            let now = time::OffsetDateTime::now_utc().unix_timestamp();
            if let Some(info) = self.holder() {
                if now - info.epoch < self.lease_secs {
                    return Err(AppError::conflict(format!(
                        "a promotion is already running (held by {} since {})",
                        info.owner, info.epoch
                    )));
                }
            }
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    AppError::persistence(format!(
                        "cannot create the promotion lock's directory: {e}"
                    ))
                })?;
            }
            std::fs::write(&self.path, format!("{owner} {now}\n")).map_err(|e| {
                AppError::persistence(format!("cannot take the promotion lock: {e}"))
            })?;
            Ok(PromotionLease::new(Box::new(FileHolder {
                path: self.path.clone(),
            })))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> std::path::PathBuf {
        // A per-test unique directory ✓: two lock tests run concurrently ✓,
        // and one test's cleanup must never remove the other's lock file ✓
        // (the same class of race the storage adapter hit ✓).
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "kelson-promotion-lock-{}-{seq}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("the scratch dir exists");
        dir
    }

    /// **F-10**: the file lock is taken once, a second holder is told, and
    /// the lease releases when the guard drops ✓.
    #[tokio::test]
    async fn the_file_lock_admits_one_holder_and_releases_on_drop() {
        let dir = scratch();
        let lock = FilePromotionLock::new(&dir);

        let lease = lock
            .try_acquire("release-a")
            .await
            .expect("the lock is free");
        assert_eq!(lock.holder().map(|h| h.owner).as_deref(), Some("release-a"));

        // A second holder is told, not queued ✓.
        let err = match lock.try_acquire("release-b").await {
            Ok(_) => panic!("a second holder must be refused"),
            Err(e) => e,
        };
        assert!(
            err.public_message.contains("already running"),
            "the refusal says a promotion is running: {}",
            err.public_message
        );

        // Dropping the lease releases it ✓.
        drop(lease);
        assert!(lock.holder().is_none(), "the lock is free again");
        assert!(
            lock.try_acquire("release-b").await.is_ok(),
            "the next promotion may take it"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// **F-10**: a crashed holder does not wedge the release forever — an
    /// expired lease may be broken ✓, and a LIVE lease may not ✓.
    #[tokio::test]
    async fn an_expired_lease_can_be_broken_but_a_live_one_may_not() {
        let dir = scratch();
        let lock = FilePromotionLock::with_lease(&dir, 60);
        let _lease = lock
            .try_acquire("release-a")
            .await
            .expect("the lock is free");

        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        assert!(
            !lock.break_expired(now).expect("the break is readable"),
            "a live lease is not broken"
        );
        assert!(
            !lock.break_expired(now + 30).expect("the break is readable"),
            "a lease inside its window is not broken"
        );
        assert!(
            lock.break_expired(now + 61).expect("the break is readable"),
            "an expired lease is broken"
        );
        assert!(lock.holder().is_none(), "the lock is free after the break");

        std::fs::remove_dir_all(&dir).ok();
    }
}
