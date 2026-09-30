//! # keel-storage
//! Storage port (11-SPEC §45). The domain never touches S3/MinIO directly;
//! adapters implement this port.

use keel_foundation::AppError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageClass {
    Document,
    Image,
    Export,
    Archive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObjectRef {
    pub bucket: String,
    pub key: String,
}

pub trait StoragePort: Send + Sync {
    fn put(&self, bucket: &str, key: &str, bytes: Vec<u8>) -> Result<ObjectRef, AppError>;
    fn get(&self, obj: &ObjectRef) -> Result<Vec<u8>, AppError>;
    fn delete(&self, obj: &ObjectRef) -> Result<(), AppError>;
    fn presigned_url(&self, obj: &ObjectRef, ttl_secs: u64) -> Result<String, AppError>;

    /// Wave 6 P1 (mail attachments): read an object under a hard byte cap.
    /// Over the cap is [`AppError::cap_limit`] (`CAP_LIMIT_EXCEEDED`) — never a
    /// silent truncation. The default buffers the object and then checks the
    /// size; adapters that can stop reading early override it to bound memory
    /// by `max_bytes` (see `keel-storage-local`, which reads at most
    /// `max_bytes + 1` bytes).
    fn get_capped(&self, obj: &ObjectRef, max_bytes: u64) -> Result<Vec<u8>, AppError> {
        let bytes = self.get(obj)?;
        if bytes.len() as u64 > max_bytes {
            return Err(AppError::cap_limit(format!(
                "{}/{} exceeds the {max_bytes} byte cap",
                obj.bucket, obj.key
            )));
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-memory port that only implements `get` — exercises the default
    /// `get_capped` (every adapter without an override gets this behavior).
    struct Blob(Vec<u8>);

    impl StoragePort for Blob {
        fn put(&self, _bucket: &str, _key: &str, _bytes: Vec<u8>) -> Result<ObjectRef, AppError> {
            Err(AppError::not_implemented("test put"))
        }
        fn get(&self, _obj: &ObjectRef) -> Result<Vec<u8>, AppError> {
            Ok(self.0.clone())
        }
        fn delete(&self, _obj: &ObjectRef) -> Result<(), AppError> {
            Ok(())
        }
        fn presigned_url(&self, _obj: &ObjectRef, _ttl_secs: u64) -> Result<String, AppError> {
            Err(AppError::not_implemented("test presign"))
        }
    }

    #[test]
    fn default_get_capped_enforces_the_byte_cap() {
        let port = Blob(vec![7u8; 64]);
        let obj = ObjectRef {
            bucket: "m".into(),
            key: "invoice.pdf".into(),
        };
        // Exactly at the cap passes; one byte under it fails with the cap code.
        assert_eq!(port.get_capped(&obj, 64).unwrap().len(), 64);
        let err = port.get_capped(&obj, 63).unwrap_err();
        assert_eq!(err.code, keel_foundation::codes::CAP_LIMIT);
    }
}
