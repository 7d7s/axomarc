//! # keel-storage-s3
//! S3-compatible storage (MinIO self-host per D-19). Foundation level: in-memory
//! sandbox implementing StoragePort. The real signed-HTTP client lands with the
//! document/export milestone.

use std::collections::HashMap;
use std::sync::Mutex;

use keel_foundation::AppError;
use keel_storage::{ObjectRef, StoragePort};

#[derive(Default)]
pub struct S3Sandbox {
    objects: Mutex<HashMap<(String, String), Vec<u8>>>,
}

impl S3Sandbox {
    pub fn new() -> Self {
        Self::default()
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
}
