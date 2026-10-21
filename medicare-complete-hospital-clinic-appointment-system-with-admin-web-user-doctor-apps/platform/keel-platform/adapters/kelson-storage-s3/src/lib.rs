//! # kelson-storage-s3
//! S3-compatible storage (MinIO self-host per D-19). Foundation level: in-memory
//! sandbox implementing StoragePort. The real signed-HTTP client lands with the
//! document/export milestone.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use kelson_foundation::AppError;
use kelson_storage::{ObjectRef, StoragePort};

#[derive(Default)]
pub struct S3Sandbox {
    objects: Mutex<HashMap<(String, String), Vec<u8>>>,
    /// Wave 6 P2: directory markers. S3 has no real directories, so
    /// `make_dir` records the prefix here (S3-style virtual dirs) while
    /// `list` keeps returning objects only.
    dirs: Mutex<HashSet<String>>,
}

impl S3Sandbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// Wave 6 P2: whether a directory marker exists (normalized to the
    /// prefix without its trailing `/`).
    pub fn has_dir(&self, prefix: &str) -> bool {
        self.dirs
            .lock()
            .unwrap()
            .contains(prefix.trim_end_matches('/'))
    }

    /// Boundary-matched prefix test: `core` must be a leading segment of
    /// `bucket/key` followed by `/` (`uploads` matches `uploads/a` but not
    /// `uploads2/a`).
    fn under(full: &str, core: &str) -> bool {
        full.len() > core.len() && full.starts_with(core) && full.as_bytes()[core.len()] == b'/'
    }
}

impl StoragePort for S3Sandbox {
    fn put(&self, bucket: &str, key: &str, bytes: Vec<u8>) -> Result<ObjectRef, AppError> {
        self.objects
            .lock()
            .unwrap()
            .insert((bucket.to_string(), key.to_string()), bytes);
        Ok(ObjectRef {
            bucket: bucket.to_string(),
            key: key.to_string(),
        })
    }

    fn get(&self, obj: &ObjectRef) -> Result<Vec<u8>, AppError> {
        self.objects
            .lock()
            .unwrap()
            .get(&(obj.bucket.clone(), obj.key.clone()))
            .cloned()
            .ok_or_else(|| AppError::not_found(format!("object {}/{}", obj.bucket, obj.key)))
    }

    fn delete(&self, obj: &ObjectRef) -> Result<(), AppError> {
        self.objects
            .lock()
            .unwrap()
            .remove(&(obj.bucket.clone(), obj.key.clone()));
        Ok(())
    }

    fn presigned_url(&self, obj: &ObjectRef, ttl_secs: u64) -> Result<String, AppError> {
        Ok(format!(
            "https://minio.local/{}/{key}?expires={ttl_secs}",
            obj.bucket,
            key = obj.key
        ))
    }

    /// Wave 6 P2: objects whose `bucket/key` sits under the prefix at a
    /// `/` boundary, sorted (directory markers are not objects).
    fn list(&self, prefix: &str) -> Result<Vec<ObjectRef>, AppError> {
        kelson_storage::validate_dir_prefix(prefix)?;
        let core = prefix.trim_end_matches('/');
        let mut out: Vec<ObjectRef> = self
            .objects
            .lock()
            .unwrap()
            .keys()
            .filter(|(bucket, key)| Self::under(&format!("{bucket}/{key}"), core))
            .map(|(bucket, key)| ObjectRef {
                bucket: bucket.clone(),
                key: key.clone(),
            })
            .collect();
        out.sort_by(|a, b| (&a.bucket, &a.key).cmp(&(&b.bucket, &b.key)));
        Ok(out)
    }

    /// Wave 6 P2: record a virtual directory marker (S3 has no real dirs).
    fn make_dir(&self, prefix: &str) -> Result<(), AppError> {
        kelson_storage::validate_dir_prefix(prefix)?;
        self.dirs
            .lock()
            .unwrap()
            .insert(prefix.trim_end_matches('/').to_string());
        Ok(())
    }

    /// Wave 6 P2: drop the marker and every object under the prefix
    /// (idempotent — a missing directory succeeds).
    fn delete_dir(&self, prefix: &str) -> Result<(), AppError> {
        kelson_storage::validate_dir_prefix(prefix)?;
        let core = prefix.trim_end_matches('/');
        let scoped = format!("{core}/");
        self.dirs
            .lock()
            .unwrap()
            .retain(|dir| dir != core && !dir.starts_with(&scoped));
        self.objects
            .lock()
            .unwrap()
            .retain(|(bucket, key), _| !Self::under(&format!("{bucket}/{key}"), core));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let s = S3Sandbox::new();
        let r = s.put("docs", "a/b.pdf", vec![1, 2, 3]).unwrap();
        assert_eq!(s.get(&r).unwrap(), vec![1, 2, 3]);
        s.delete(&r).unwrap();
        assert!(s.get(&r).is_err());
    }

    /// **LF-072**: a temporary URL is scoped to ONE object and carries the
    /// requested expiry — two different objects never share a URL, and the
    /// TTL is in the URL.
    ///
    /// Scope note: this sandbox's URL is a **stub** (no signature — the real
    /// signed-HTTP client is not written yet), so the case proves the
    /// *contract shape* the port promises, not a real signature.
    #[test]
    fn a_temporary_url_is_scoped_to_one_object_and_carries_the_expiry() {
        let s = S3Sandbox::new();
        let one = s.put("docs", "a/one.pdf", vec![1]).unwrap();
        let two = s.put("docs", "a/two.pdf", vec![2]).unwrap();

        let url_one = s.presigned_url(&one, 60).unwrap();
        let url_two = s.presigned_url(&two, 60).unwrap();
        assert_ne!(url_one, url_two, "two objects never share a URL");
        assert!(
            url_one.contains("one.pdf") && url_one.contains("docs"),
            "the URL is scoped to its bucket and key: {url_one}"
        );
        assert!(
            url_one.contains("60"),
            "the URL carries the requested expiry: {url_one}"
        );
        let short = s.presigned_url(&one, 30).unwrap();
        assert_ne!(
            short, url_one,
            "a shorter lease is a different URL — the expiry is not ignored"
        );
    }

    /// Wave 6 P2: virtual directory semantics — boundary-matched sorted
    /// listings, marker directories, scoped idempotent deletes.
    #[test]
    fn directory_ops_use_boundary_matching_and_markers() {
        let s = S3Sandbox::new();
        s.put("uploads", "abc/one.png", vec![1]).unwrap();
        s.put("uploads", "abc/two.png", vec![2]).unwrap();
        s.put("uploads", "abcd/three.png", vec![3]).unwrap(); // not under abc/
        s.put("exports", "abc/four.csv", vec![4]).unwrap();

        // `uploads` matches only at the `/` boundary (uploads2-style names
        // and other buckets stay out).
        let listed = s.list("uploads/").unwrap();
        assert_eq!(listed.len(), 3);
        let abc = s.list("uploads/abc/").unwrap();
        assert_eq!(
            abc.iter().map(|o| o.key.as_str()).collect::<Vec<_>>(),
            vec!["abc/one.png", "abc/two.png"]
        );
        assert!(s.list("uploads/missing/").unwrap().is_empty());

        // Markers are recorded separately from objects.
        s.make_dir("uploads/fresh/").unwrap();
        assert!(s.has_dir("uploads/fresh/"));
        assert!(s.list("uploads/fresh/").unwrap().is_empty());

        // Delete is scoped: abc/ goes, abcd/ and other buckets survive.
        s.delete_dir("uploads/abc/").unwrap();
        assert!(s.list("uploads/abc/").unwrap().is_empty());
        assert_eq!(s.list("uploads/").unwrap().len(), 1);
        assert_eq!(s.list("uploads/").unwrap()[0].key, "abcd/three.png");
        assert_eq!(s.list("exports/").unwrap().len(), 1);
        s.delete_dir("uploads/fresh/").unwrap();
        assert!(!s.has_dir("uploads/fresh/"));
        s.delete_dir("uploads/gone/").unwrap(); // idempotent

        // The shared prefix guard refuses traversal.
        assert!(s.list("../").is_err());
        assert!(s.make_dir("a/../b").is_err());
    }
}
