//! # keel-storage-local
//! Local-disk StoragePort adapter (S2 item 27, L38/L39). Objects live under
//! <root>/<bucket>/<key>. Every key is traversal-guarded: no absolute paths,
//! no backslashes, no dot-dot segments, and the resolved path must stay under
//! the root. Presigned URLs are not supported on a local disk.

use std::path::{Component, Path, PathBuf};

use keel_foundation::AppError;
use keel_storage::{ObjectRef, ObjectWriter, StoragePort};

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

    /// Wave 6 P2: resolve a directory prefix under the root (shared guard +
    /// defense-in-depth containment, same posture as [`Self::resolve`]).
    fn dir_path(&self, prefix: &str) -> Result<PathBuf, AppError> {
        keel_storage::validate_dir_prefix(prefix)?;
        let path = self.root.join(prefix.trim_end_matches('/'));
        if !path.starts_with(&self.root) {
            return Err(AppError::validation(
                "storage prefix escapes the disk root",
            ));
        }
        Ok(path)
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

    /// Wave 6 P2: walk the tree under the prefix and return the objects
    /// (`bucket/key` from the on-disk layout, `/`-separated on every
    /// platform, sorted). A missing directory lists as empty.
    fn list(&self, prefix: &str) -> Result<Vec<ObjectRef>, AppError> {
        let dir = self.dir_path(prefix)?;
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        let mut stack = vec![dir];
        while let Some(dir) = stack.pop() {
            let entries = std::fs::read_dir(&dir)
                .map_err(|e| AppError::persistence(format!("cannot list {}: {e}", dir.display())))?;
            for entry in entries {
                let entry = entry.map_err(|e| {
                    AppError::persistence(format!("cannot list {}: {e}", dir.display()))
                })?;
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                let rel = path
                    .strip_prefix(&self.root)
                    .expect("listed path is under the disk root");
                let key = rel.to_string_lossy().replace('\\', "/");
                let Some((bucket, key)) = key.split_once('/') else {
                    continue; // files directly under the root are not objects
                };
                if bucket.is_empty() || key.is_empty() {
                    continue;
                }
                out.push(ObjectRef {
                    bucket: bucket.to_string(),
                    key: key.to_string(),
                });
            }
        }
        out.sort_by(|a, b| (&a.bucket, &a.key).cmp(&(&b.bucket, &b.key)));
        Ok(out)
    }

    /// Wave 6 P2: create the directory (idempotent through
    /// `create_dir_all`).
    fn make_dir(&self, prefix: &str) -> Result<(), AppError> {
        let dir = self.dir_path(prefix)?;
        std::fs::create_dir_all(&dir)
            .map_err(|e| AppError::persistence(format!("cannot create {}: {e}", dir.display())))?;
        Ok(())
    }

    /// Wave 6 P2: remove the tree under the prefix; a missing directory is
    /// success (idempotent).
    fn delete_dir(&self, prefix: &str) -> Result<(), AppError> {
        let dir = self.dir_path(prefix)?;
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(AppError::persistence(format!(
                "cannot delete {}: {e}",
                dir.display()
            ))),
        }
    }

    /// Wave 6 P10 (HTTP client sink): stream straight to the target file —
    /// chunks are written as they arrive (a large download never buffers in
    /// memory), and a session dropped before [`ObjectWriter::finish`]
    /// removes the partial file, so an aborted download never publishes a
    /// truncated object under the final key.
    fn writer<'a>(
        &'a self,
        bucket: &str,
        key: &str,
    ) -> Result<Box<dyn ObjectWriter + 'a>, AppError> {
        let path = self.resolve(bucket, key)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AppError::persistence(format!("cannot create {}: {e}", parent.display()))
            })?;
        }
        let file = std::fs::File::create(&path).map_err(|e| {
            AppError::persistence(format!("cannot create {}: {e}", path.display()))
        })?;
        Ok(Box::new(FileWriter {
            file,
            path,
            bucket: bucket.to_string(),
            key: key.to_string(),
            committed: false,
        }))
    }
}

/// Wave 6 P10: streaming write session for [`LocalDisk`]. Chunks land on
/// the file immediately; `finish` commits the object, while a Drop without
/// finish removes the partial (write failures abort the same way).
struct FileWriter {
    file: std::fs::File,
    path: PathBuf,
    bucket: String,
    key: String,
    committed: bool,
}

impl ObjectWriter for FileWriter {
    fn write(&mut self, chunk: &[u8]) -> Result<(), AppError> {
        use std::io::Write as _;
        self.file.write_all(chunk).map_err(|e| {
            AppError::persistence(format!("cannot write {}: {e}", self.path.display()))
        })
    }

    fn finish(mut self: Box<Self>) -> Result<ObjectRef, AppError> {
        self.committed = true;
        Ok(ObjectRef {
            bucket: std::mem::take(&mut self.bucket),
            key: std::mem::take(&mut self.key),
        })
    }
}

impl Drop for FileWriter {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_file(&self.path);
        }
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

    /// Wave 6 P2: list/make/delete over the tree — boundary-scoped
    /// listings, `/`-separated keys on every platform, idempotent deletes,
    /// and the shared traversal guard.
    #[test]
    fn directory_ops_list_make_delete() {
        let (disk, dir) = disk();
        disk.put("uploads", "abc/one.png", vec![1]).unwrap();
        disk.put("uploads", "abc/nested/two.png", vec![2]).unwrap();
        disk.put("uploads", "other/three.png", vec![3]).unwrap();
        disk.put("exports", "abc/four.csv", vec![4]).unwrap();

        // Prefix boundary: `uploads/` never leaks the exports bucket, and
        // `abc/` only matches at the `/` boundary (sorted, nested included).
        let uploads = disk.list("uploads/").unwrap();
        assert_eq!(uploads.len(), 3);
        assert!(uploads.iter().all(|o| o.bucket == "uploads"));
        let abc = disk.list("uploads/abc/").unwrap();
        assert_eq!(
            abc.iter().map(|o| o.key.as_str()).collect::<Vec<_>>(),
            vec!["abc/nested/two.png", "abc/one.png"]
        );

        // Missing directories are empty/success, not errors.
        assert!(disk.list("uploads/gone/").unwrap().is_empty());
        disk.delete_dir("uploads/gone/").unwrap();

        disk.make_dir("uploads/fresh/").unwrap();
        assert!(dir.join("uploads/fresh").is_dir());

        disk.delete_dir("uploads/abc/").unwrap();
        assert!(disk.list("uploads/abc/").unwrap().is_empty());
        assert_eq!(disk.list("uploads/").unwrap().len(), 1); // other/ only
        assert!(dir.join("uploads/other").exists(), "siblings survive");

        // The shared prefix guard refuses traversal before any path work.
        assert!(disk.list("../").is_err());
        assert!(disk.make_dir("a/../b").is_err());
        assert!(disk.delete_dir("/abs/").is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    /// Wave 6 P10: the streaming writer commits exactly the chunks that
    /// were written, readable through the normal `get` path.
    #[test]
    fn writer_streams_chunks_and_finish_commits() {
        let (disk, dir) = disk();
        {
            let mut writer = disk.writer("exports", "reports/big.bin").unwrap();
            writer.write(b"a,b,c\n").unwrap();
            writer.write(b"1,2,3\n").unwrap();
            writer.write(b"4,5,6\n").unwrap();
            let obj = writer.finish().unwrap();
            assert_eq!(obj.bucket, "exports");
            assert_eq!(obj.key, "reports/big.bin");
        }
        let obj = ObjectRef {
            bucket: "exports".into(),
            key: "reports/big.bin".into(),
        };
        assert_eq!(disk.get(&obj).unwrap(), b"a,b,c\n1,2,3\n4,5,6\n");
        assert!(dir.join("exports/reports/big.bin").exists());
        std::fs::remove_dir_all(dir).ok();
    }

    /// Wave 6 P10: a session dropped before `finish` removes the partial —
    /// an aborted download never publishes a truncated object.
    #[test]
    fn aborted_writer_session_removes_the_partial_file() {
        let (disk, dir) = disk();
        {
            let mut writer = disk.writer("exports", "half.bin").unwrap();
            writer.write(b"partial").unwrap();
            // Dropped without finish.
        }
        assert!(
            !dir.join("exports/half.bin").exists(),
            "partial file must be removed"
        );
        std::fs::remove_dir_all(dir).ok();
    }
}
