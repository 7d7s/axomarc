//! # keel-storage-local
//! Local-disk StoragePort adapter (S2 item 27, L38/L39). Objects live under
//! <root>/<bucket>/<key>. Every key is traversal-guarded: no absolute paths,
//! no backslashes, no dot-dot segments, and the resolved path must stay under
//! the root. Presigned URLs are not supported on a local disk.

use std::path::{Component, Path, PathBuf};

use keel_foundation::AppError;
use keel_storage::{ObjectRef, StoragePort};

/// Maximum object key length (defensive bound, not a storage limit).
pub const MAX_KEY_LEN: usize = 1024;

pub struct LocalDisk {
    root: PathBuf,
}

impl LocalDisk {
    /// Create/attach a local disk rooted at dir (created if missing).
    pub fn new(root: impl Into<PathBuf>) -> Result<Self, AppError> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|e| {
            AppError::persistence(format!(
                "cannot create storage root {}: {e}",
                root.display()
            ))
        })?;
        Ok(Self {
            root: std::fs::canonicalize(&root).map_err(|e| {
                AppError::persistence(format!(
                    "cannot resolve storage root {}: {e}",
                    root.display()
                ))
            })?,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Validate + resolve an object path. The resolved path must stay inside
    /// the disk root (defense in depth on top of the component checks).
    fn resolve(&self, bucket: &str, key: &str) -> Result<PathBuf, AppError> {
        validate_segment("bucket", bucket)?;
        validate_key(key)?;
        for part in Path::new(key).components() {
            if !matches!(part, Component::Normal(_)) {
                return Err(AppError::validation("storage key must be a relative path"));
            }
        }
        let candidate = self.root.join(bucket).join(key);
        if !candidate.starts_with(&self.root) {
            return Err(AppError::validation("storage key escapes the disk root"));
        }
        Ok(candidate)
    }
}

fn validate_segment(kind: &str, value: &str) -> Result<(), AppError> {
    let ok = !value.is_empty()
        && value.len() <= 64
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !ok {
        return Err(AppError::validation(format!(
            "invalid {kind} {value:?}: expected [a-z0-9_-] (max 64)"
        )));
    }
    Ok(())
}

fn validate_key(key: &str) -> Result<(), AppError> {
    if key.is_empty() || key.len() > MAX_KEY_LEN {
        return Err(AppError::validation(format!(
            "storage key must be 1..={MAX_KEY_LEN} characters"
        )));
    }
    if key.contains('\\') || key.contains(':') {
        return Err(AppError::validation(
            "storage key must not contain backslashes or colons",
        ));
    }
    if key.starts_with('/') || key.starts_with('~') {
        return Err(AppError::validation("storage key must be relative"));
    }
    for segment in key.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(AppError::validation(
                "storage key has an empty or relative segment",
            ));
        }
    }
    Ok(())
}

impl StoragePort for LocalDisk {
    fn put(&self, bucket: &str, key: &str, bytes: Vec<u8>) -> Result<ObjectRef, AppError> {
        let path = self.resolve(bucket, key)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AppError::persistence(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        std::fs::write(&path, &bytes)
            .map_err(|e| AppError::persistence(format!("cannot write {}: {e}", path.display())))?;
        Ok(ObjectRef {
            bucket: bucket.to_string(),
            key: key.to_string(),
        })
    }

    fn get(&self, obj: &ObjectRef) -> Result<Vec<u8>, AppError> {
        let path = self.resolve(&obj.bucket, &obj.key)?;
        match std::fs::read(&path) {
            Ok(bytes) => Ok(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(AppError::not_found(format!("{}/{}", obj.bucket, obj.key)))
            }
            Err(e) => Err(AppError::persistence(format!(
                "cannot read {}: {e}",
                path.display()
            ))),
        }
    }

    fn delete(&self, obj: &ObjectRef) -> Result<(), AppError> {
        let path = self.resolve(&obj.bucket, &obj.key)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(AppError::not_found(format!("{}/{}", obj.bucket, obj.key)))
            }
            Err(e) => Err(AppError::persistence(format!(
                "cannot delete {}: {e}",
                path.display()
            ))),
        }
    }

    fn presigned_url(&self, _obj: &ObjectRef, _ttl_secs: u64) -> Result<String, AppError> {
        Err(AppError::validation(
            "local disk does not support presigned URLs (use an S3-compatible adapter)",
        ))
    }

    /// Wave 6 P1: capped read that stops early — at most `max_bytes + 1`
    /// bytes are pulled from disk even when the object is far larger, then
    /// `CAP_LIMIT_EXCEEDED` is raised (no silent truncation).
    fn get_capped(&self, obj: &ObjectRef, max_bytes: u64) -> Result<Vec<u8>, AppError> {
        use std::io::Read;
        let path = self.resolve(&obj.bucket, &obj.key)?;
        let file = std::fs::File::open(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AppError::not_found(format!("{}/{}", obj.bucket, obj.key))
            } else {
                AppError::persistence(format!("cannot read {}: {e}", path.display()))
            }
        })?;
        let mut buf = Vec::new();
        file.take(max_bytes.saturating_add(1))
            .read_to_end(&mut buf)
            .map_err(|e| AppError::persistence(format!("cannot read {}: {e}", path.display())))?;
        if buf.len() as u64 > max_bytes {
            return Err(AppError::cap_limit(format!(
                "{}/{} exceeds the {max_bytes} byte cap",
                obj.bucket, obj.key
            )));
        }
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn disk() -> (LocalDisk, PathBuf) {
        let dir = std::env::temp_dir().join(format!("keel-localdisk-{}", uuid_like()));
        (LocalDisk::new(&dir).unwrap(), dir)
    }

    /// Unique per-call directory suffix. A bare nanosecond clock is not
    /// unique across concurrently starting test threads (Windows may hand
    /// out identical timestamps), which let one test's `remove_dir_all`
    /// wipe another test's shared directory — the atomic sequence closes
    /// that race.
    fn uuid_like() -> String {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::time::{SystemTime, UNIX_EPOCH};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("{nanos}-{}", SEQ.fetch_add(1, Ordering::Relaxed))
    }

    #[test]
    fn roundtrip_put_get_delete() {
        let (disk, dir) = disk();
        let r = disk
            .put("exports", "reports/2026/q3.csv", b"a,b,c".to_vec())
            .unwrap();
        assert_eq!(r.bucket, "exports");
        assert_eq!(disk.get(&r).unwrap(), b"a,b,c");
        disk.delete(&r).unwrap();
        assert!(disk.get(&r).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn traversal_is_rejected() {
        let (disk, dir) = disk();
        for key in [
            "../etc/passwd",
            "a/../../b",
            "/abs/path",
            "a\\b",
            "a:zone",
            "~user/file",
            "a//b",
            "a/./b",
        ] {
            assert!(
                disk.put("exports", key, vec![1]).is_err(),
                "key accepted: {key}"
            );
        }
        assert!(disk.put("BadBucket", "k", vec![1]).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn nested_key_layout() {
        let (disk, dir) = disk();
        disk.put("docs", "tenant/1/invoice.pdf", vec![9]).unwrap();
        assert!(dir.join("docs/tenant/1/invoice.pdf").exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn presigned_unsupported() {
        let (disk, dir) = disk();
        let obj = ObjectRef {
            bucket: "exports".into(),
            key: "x.txt".into(),
        };
        assert!(disk.presigned_url(&obj, 60).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    /// Wave 6 P1: `get_capped` streams under the cap and refuses over the cap
    /// with `CAP_LIMIT_EXCEEDED` (missing objects stay `NOT_FOUND`).
    #[test]
    fn get_capped_enforces_the_byte_cap() {
        let (disk, dir) = disk();
        let obj = disk.put("docs", "big.bin", vec![9u8; 4096]).unwrap();
        assert_eq!(disk.get_capped(&obj, 4096).unwrap().len(), 4096);
        let over = disk.get_capped(&obj, 100).unwrap_err();
        assert_eq!(over.code, keel_foundation::codes::CAP_LIMIT);
        let missing = ObjectRef {
            bucket: "docs".into(),
            key: "nope.bin".into(),
        };
        let err = disk.get_capped(&missing, 100).unwrap_err();
        assert_eq!(err.code, keel_foundation::codes::NOT_FOUND);
        std::fs::remove_dir_all(dir).ok();
    }
}
