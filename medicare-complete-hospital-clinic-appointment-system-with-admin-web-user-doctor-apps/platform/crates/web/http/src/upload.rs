//! Multipart upload pipeline (S7 C5/B-06): bounded multipart parsing behind a
//! transport-level contract, streaming into any `StoragePort`.
//!
//! Hardening posture (report SS8):
//! - max parts / max field size / max file size are enforced WHILE reading
//!   (an oversized part aborts the request, it is never buffered whole)
//! - MIME allowlist hook (empty = allow all)
//! - filenames are sanitized to a safe basename (no path components, no
//!   control chars, no leading dot) and merged into a collision-proof key
//! - storage keys are namespaced by a caller-supplied prefix

use axum::extract::Multipart;
use keel_foundation::AppError;
use keel_storage::{ObjectRef, StoragePort};
use uuid::Uuid;

/// Upload limits. All bounds are hard; reading stops at the first violation.
#[derive(Debug, Clone)]
pub struct UploadLimits {
    /// Maximum number of multipart parts (files + fields combined).
    pub max_parts: usize,
    /// Maximum size of a non-file form field.
    pub max_field_bytes: usize,
    /// Maximum size of a single uploaded file.
    pub max_file_bytes: usize,
    /// Allowed content types for file parts; empty = allow all.
    pub allowed_mime: Vec<String>,
}

impl Default for UploadLimits {
    fn default() -> Self {
        Self {
            max_parts: 8,
            max_field_bytes: 64 * 1024,
            max_file_bytes: 8 * 1024 * 1024,
            allowed_mime: Vec::new(),
        }
    }
}

/// One accepted file, stored through the port.
#[derive(Debug, Clone, serde::Serialize)]
pub struct StoredUpload {
    pub field: String,
    pub filename: String,
    pub content_type: String,
    pub size: usize,
    pub object: ObjectRef,
}

/// Sanitize an untrusted upload filename down to a safe basename:
/// last path segment only, ASCII alnum + space + `._-`, no leading dot,
/// max 128 chars.
pub fn sanitize_filename(raw: &str) -> String {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or("");
    let cleaned: String = last
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ' '))
        .take(128)
        .collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim().to_string();
    if cleaned.is_empty() {
        "upload.bin".into()
    } else {
        cleaned
    }
}

/// Parse + validate + store every file part. Returns the stored file list.
/// Non-file fields are consumed and size-checked but not retained.
pub async fn ingest_multipart(
    mut multipart: Multipart,
    limits: &UploadLimits,
    storage: &dyn StoragePort,
    bucket: &str,
    key_prefix: &str,
) -> Result<Vec<StoredUpload>, AppError> {
    let mut stored: Vec<StoredUpload> = Vec::new();
    let mut parts = 0usize;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::validation(format!("malformed multipart body: {e}")))?
    {
        parts += 1;
        if parts > limits.max_parts {
            return Err(AppError::cap_limit(format!(
                "too many multipart parts (max {})",
                limits.max_parts
            )));
        }
        let name = field.name().unwrap_or("").to_string();
        let file_name = field.file_name().map(|f| f.to_string());
        let content_type = field
            .content_type()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "application/octet-stream".into());

        match file_name {
            None => {
                // Plain form field: bounded read, value discarded.
                let mut size = 0usize;
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|e| AppError::validation(format!("field {name}: {e}")))?
                {
                    size += chunk.len();
                    if size > limits.max_field_bytes {
                        return Err(AppError::validation(format!(
                            "field {name} exceeds the {} byte limit",
                            limits.max_field_bytes
                        )));
                    }
                }
            }
            Some(raw_name) => {
                if !limits.allowed_mime.is_empty()
                    && !limits
                        .allowed_mime
                        .iter()
                        .any(|m| m.eq_ignore_ascii_case(&content_type))
                {
                    return Err(AppError::validation(format!(
                        "content type {content_type} is not allowed for uploads"
                    )));
                }
                let filename = sanitize_filename(&raw_name);
                let mut bytes: Vec<u8> = Vec::new();
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|e| AppError::validation(format!("upload {filename}: {e}")))?
                {
                    if bytes.len() + chunk.len() > limits.max_file_bytes {
                        return Err(AppError::cap_limit(format!(
                            "upload {filename} exceeds the {} byte limit",
                            limits.max_file_bytes
                        )));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                let size = bytes.len();
                let key = format!("{key_prefix}{}-{filename}", Uuid::now_v7());
                let object = storage.put(bucket, &key, bytes)?;
                stored.push(StoredUpload {
                    field: name,
                    filename,
                    content_type,
                    size,
                    object,
                });
            }
        }
    }
    if stored.is_empty() {
        return Err(AppError::validation(
            "no file parts found in the multipart body",
        ));
    }
    Ok(stored)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_traversal_and_garbage() {
        assert_eq!(sanitize_filename("../etc/passwd"), "passwd");
        assert_eq!(sanitize_filename("a\\b\\report.pdf"), "report.pdf");
        assert_eq!(sanitize_filename("../../x"), "x");
        assert_eq!(sanitize_filename(".hidden"), "hidden");
        assert_eq!(sanitize_filename("bad/name<script>.txt"), "namescript.txt");
        assert_eq!(sanitize_filename(""), "upload.bin");
        assert_eq!(sanitize_filename("///"), "upload.bin");
        let long = "a".repeat(300);
        assert_eq!(sanitize_filename(&long).len(), 128);
    }

    #[test]
    fn limits_default_is_bounded() {
        let limits = UploadLimits::default();
        assert_eq!(limits.max_parts, 8);
        assert_eq!(limits.max_file_bytes, 8 * 1024 * 1024);
        assert!(limits.allowed_mime.is_empty());
    }
}
#[cfg(test)]
mod multipart_tests {
    use super::*;
    use axum::body::Body;
    use axum::extract::FromRequest;
    use axum::http::{header, Request};

    #[derive(Default)]
    struct MemStore {
        objects: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
    }

    impl StoragePort for MemStore {
        fn put(&self, bucket: &str, key: &str, bytes: Vec<u8>) -> Result<ObjectRef, AppError> {
            self.objects
                .lock()
                .unwrap()
                .insert(format!("{bucket}/{key}"), bytes);
            Ok(ObjectRef {
                bucket: bucket.into(),
                key: key.into(),
            })
        }
        fn get(&self, obj: &ObjectRef) -> Result<Vec<u8>, AppError> {
            self.objects
                .lock()
                .unwrap()
                .get(&format!("{}/{}", obj.bucket, obj.key))
                .cloned()
                .ok_or_else(|| AppError::not_found(format!("{}/{}", obj.bucket, obj.key)))
        }
        fn delete(&self, obj: &ObjectRef) -> Result<(), AppError> {
            self.objects
                .lock()
                .unwrap()
                .remove(&format!("{}/{}", obj.bucket, obj.key));
            Ok(())
        }
        fn presigned_url(&self, _obj: &ObjectRef, _ttl: u64) -> Result<String, AppError> {
            Err(AppError::validation("no presign on memory store"))
        }
    }

    fn multipart(
        body_parts: Vec<(&'static str, Option<&'static str>, &'static str, Vec<u8>)>,
    ) -> Request<Body> {
        let boundary = "keel-boundary";
        let mut body = Vec::new();
        for (name, filename, content_type, data) in body_parts {
            body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
            match filename {
                Some(fname) => body.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{fname}\"\r\n"
                    )
                    .as_bytes(),
                ),
                None => body.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n").as_bytes(),
                ),
            }
            body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
            body.extend_from_slice(&data);
            body.extend_from_slice(b"\r\n");
        }
        body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        Request::builder()
            .method("POST")
            .uri("/upload")
            .header(
                header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body))
            .unwrap()
    }

    async fn run(
        req: Request<Body>,
        limits: &UploadLimits,
        allowed: Vec<String>,
    ) -> Result<Vec<StoredUpload>, AppError> {
        let mp = Multipart::from_request(req, &())
            .await
            .expect("multipart parses");
        let store = MemStore::default();
        let mut limits = limits.clone();
        limits.allowed_mime = allowed;
        ingest_multipart(mp, &limits, &store, "uploads", "pfx/")
            .await
            .inspect(|stored| {
                // Round-trip proof: every stored object is retrievable.
                for s in stored {
                    assert!(store.get(&s.object).is_ok());
                }
            })
    }

    #[tokio::test]
    async fn files_and_fields_accepted() {
        let req = multipart(vec![
            ("note", None, "text/plain", b"hello".to_vec()),
            ("file", Some("a b.txt"), "text/plain", b"data!".to_vec()),
        ]);
        let stored = run(req, &UploadLimits::default(), vec![]).await.unwrap();
        assert_eq!(stored.len(), 1, "fields are consumed, files stored");
        assert_eq!(stored[0].field, "file");
        assert_eq!(stored[0].filename, "a b.txt");
        assert_eq!(stored[0].size, 5);
        assert!(stored[0].object.key.starts_with("pfx/"));
    }

    #[tokio::test]
    async fn oversized_file_rejected_mid_stream() {
        let limits = UploadLimits {
            max_file_bytes: 16,
            ..UploadLimits::default()
        };
        let req = multipart(vec![(
            "file",
            Some("big.bin"),
            "application/octet-stream",
            vec![0u8; 64],
        )]);
        let err = run(req, &limits, vec![]).await.unwrap_err();
        assert_eq!(err.category, keel_foundation::ErrorCategory::ResourceExhausted);
    }

    #[tokio::test]
    async fn mime_allowlist_enforced() {
        let req = multipart(vec![(
            "file",
            Some("x.exe"),
            "application/x-msdownload",
            vec![1, 2],
        )]);
        let err = run(req, &UploadLimits::default(), vec!["text/plain".into()])
            .await
            .unwrap_err();
        assert_eq!(err.category, keel_foundation::ErrorCategory::Validation);
        let ok = multipart(vec![("file", Some("x.txt"), "text/plain", vec![1, 2])]);
        assert!(run(ok, &UploadLimits::default(), vec!["text/plain".into()])
            .await
            .is_ok());
    }

    #[tokio::test]
    async fn max_parts_enforced() {
        let limits = UploadLimits {
            max_parts: 1,
            ..UploadLimits::default()
        };
        let req = multipart(vec![
            ("a", Some("1.txt"), "text/plain", vec![1]),
            ("b", Some("2.txt"), "text/plain", vec![2]),
        ]);
        let err = run(req, &limits, vec![]).await.unwrap_err();
        assert_eq!(err.category, keel_foundation::ErrorCategory::ResourceExhausted);
    }

    #[tokio::test]
    async fn no_file_parts_rejected() {
        let req = multipart(vec![("note", None, "text/plain", b"only a field".to_vec())]);
        let err = run(req, &UploadLimits::default(), vec![])
            .await
            .unwrap_err();
        assert_eq!(err.category, keel_foundation::ErrorCategory::Validation);
    }
}
