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
    /// Sniffed pixel width (images only); omitted from JSON when unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Sniffed pixel height (images only); omitted from JSON when unknown.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

/// Pixel dimensions from an image header (Wave 4 Phase 3): the `dimensions`
/// validation rule needs width/height without pulling in a decoder
/// dependency, so only the header fields are read. PNG, JPEG (SOF), GIF,
/// WebP (VP8/VP8L/VP8X) and BMP layouts; anything else → `None` (the rule
/// then fails closed).
pub fn image_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    png_dimensions(bytes)
        .or_else(|| gif_dimensions(bytes))
        .or_else(|| jpeg_dimensions(bytes))
        .or_else(|| webp_dimensions(bytes))
        .or_else(|| bmp_dimensions(bytes))
}

fn png_dimensions(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || b[..8] != *b"\x89PNG\r\n\x1a\n" || b[12..16] != *b"IHDR" {
        return None;
    }
    Some((
        u32::from_be_bytes(b[16..20].try_into().ok()?),
        u32::from_be_bytes(b[20..24].try_into().ok()?),
    ))
}

fn gif_dimensions(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 10 || (b[..6] != *b"GIF87a" && b[..6] != *b"GIF89a") {
        return None;
    }
    Some((
        u32::from(u16::from_le_bytes(b[6..8].try_into().ok()?)),
        u32::from(u16::from_le_bytes(b[8..10].try_into().ok()?)),
    ))
}

fn jpeg_dimensions(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 4 || b[0] != 0xFF || b[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    while i + 9 < b.len() {
        if b[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = b[i + 1];
        if marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            i += 2;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([b[i + 2], b[i + 3]]));
        // SOF0-SOF15 minus the non-frame markers carry the frame size.
        let is_sof = (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC);
        if is_sof {
            let h = u16::from_be_bytes([b[i + 5], b[i + 6]]);
            let w = u16::from_be_bytes([b[i + 7], b[i + 8]]);
            return Some((u32::from(w), u32::from(h)));
        }
        i = i.checked_add(2 + len)?;
    }
    None
}

fn webp_dimensions(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 30 || b[..4] != *b"RIFF" || b[8..12] != *b"WEBP" {
        return None;
    }
    match &b[12..16] {
        b"VP8 " => Some((
            u32::from(u16::from_le_bytes([b[26], b[27]]) & 0x3FFF),
            u32::from(u16::from_le_bytes([b[28], b[29]]) & 0x3FFF),
        )),
        b"VP8L" => {
            let bits = u32::from_le_bytes([b[21], b[22], b[23], b[24]]);
            Some((
                (bits & 0x3FFF) + 1,
                ((bits >> 14) & 0x3FFF) + 1,
            ))
        }
        b"VP8X" => {
            let w = u32::from_le_bytes([b[24], b[25], b[26], 0]) & 0x00FF_FFFF;
            let h = u32::from_le_bytes([b[27], b[28], b[29], 0]) & 0x00FF_FFFF;
            Some((w + 1, h + 1))
        }
        _ => None,
    }
}

fn bmp_dimensions(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 26 || b[..2] != *b"BM" {
        return None;
    }
    let w = i32::from_le_bytes(b[18..22].try_into().ok()?);
    let h = i32::from_le_bytes(b[22..26].try_into().ok()?);
    if w <= 0 || h == 0 {
        return None;
    }
    Some((w.unsigned_abs(), h.unsigned_abs()))
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

/// Parse + validate + store every file part, RETAINING the text fields
/// (Wave 4 Phase 3). Graph routes use this so one multipart request can
/// carry both the operation input and its files: text fields come back as
/// (name, utf-8 value) pairs, file parts as [`StoredUpload`]s with sniffed
/// image dimensions. Zero file parts is valid here (unlike
/// [`ingest_multipart`]); every size/MIME limit still applies while reading.
pub async fn ingest_multipart_mixed(
    mut multipart: Multipart,
    limits: &UploadLimits,
    storage: &dyn StoragePort,
    bucket: &str,
    key_prefix: &str,
) -> Result<(Vec<(String, String)>, Vec<StoredUpload>), AppError> {
    let mut fields: Vec<(String, String)> = Vec::new();
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
                // Plain form field: bounded read, value retained as utf-8
                // (the graph edge coerces it into the JSON input).
                let mut bytes: Vec<u8> = Vec::new();
                while let Some(chunk) = field
                    .chunk()
                    .await
                    .map_err(|e| AppError::validation(format!("field {name}: {e}")))?
                {
                    if bytes.len() + chunk.len() > limits.max_field_bytes {
                        return Err(AppError::validation(format!(
                            "field {name} exceeds the {} byte limit",
                            limits.max_field_bytes
                        )));
                    }
                    bytes.extend_from_slice(&chunk);
                }
                let value = String::from_utf8(bytes)
                    .map_err(|_| AppError::validation(format!("field {name} must be utf-8")))?;
                fields.push((name, value));
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
                let (width, height) = image_dimensions(&bytes).unzip();
                let key = format!("{key_prefix}{}-{filename}", Uuid::now_v7());
                let object = storage.put(bucket, &key, bytes)?;
                stored.push(StoredUpload {
                    field: name,
                    filename,
                    content_type,
                    size,
                    object,
                    width,
                    height,
                });
            }
        }
    }
    Ok((fields, stored))
}

/// Parse + validate + store every file part. Returns the stored file list.
/// Non-file fields are consumed and size-checked but not retained.
pub async fn ingest_multipart(
    multipart: Multipart,
    limits: &UploadLimits,
    storage: &dyn StoragePort,
    bucket: &str,
    key_prefix: &str,
) -> Result<Vec<StoredUpload>, AppError> {
    let (_fields, stored) =
        ingest_multipart_mixed(multipart, limits, storage, bucket, key_prefix).await?;
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

    fn png_header(w: u32, h: u32) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        b.extend_from_slice(&13u32.to_be_bytes()); // IHDR length
        b.extend_from_slice(b"IHDR");
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&[0u8; 13]);
        b
    }

    #[test]
    fn image_dimensions_reads_headers_without_a_decoder() {
        // PNG: fixed IHDR offsets.
        assert_eq!(image_dimensions(&png_header(300, 200)), Some((300, 200)));
        // GIF: 16-bit little-endian logical screen size.
        let mut gif = b"GIF89a".to_vec();
        gif.extend_from_slice(&320u16.to_le_bytes());
        gif.extend_from_slice(&240u16.to_le_bytes());
        assert_eq!(image_dimensions(&gif), Some((320, 240)));
        // JPEG: walk segments to the SOF frame header (height then width).
        let mut jpg = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08];
        jpg.extend_from_slice(&100u16.to_be_bytes());
        jpg.extend_from_slice(&200u16.to_be_bytes());
        jpg.extend_from_slice(&[0x01, 0x11, 0x00]);
        assert_eq!(image_dimensions(&jpg), Some((200, 100)));
        // WebP lossless: 14-bit-minus-one pairs packed from byte 21.
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&24u32.to_le_bytes());
        webp.extend_from_slice(b"WEBP");
        webp.extend_from_slice(b"VP8L");
        webp.extend_from_slice(&10u32.to_le_bytes());
        webp.push(0x2F);
        webp.extend_from_slice(&(299u32 | (199u32 << 14)).to_le_bytes());
        webp.extend_from_slice(&[0u8; 6]);
        assert_eq!(image_dimensions(&webp), Some((300, 200)));
        // BMP: DIB header width/height (height may be top-down negative).
        let mut bmp = vec![0u8; 26];
        bmp[0] = b'B';
        bmp[1] = b'M';
        bmp[18..22].copy_from_slice(&640i32.to_le_bytes());
        bmp[22..26].copy_from_slice(&(-480i32).to_le_bytes());
        assert_eq!(image_dimensions(&bmp), Some((640, 480)));
        // Anything else is unknown — the dimensions rule fails closed.
        assert_eq!(image_dimensions(b"not an image"), None);
        assert_eq!(image_dimensions(b""), None);
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
    async fn mixed_ingest_retains_text_and_sniffs_image_dimensions() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&13u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&300u32.to_be_bytes());
        png.extend_from_slice(&200u32.to_be_bytes());
        png.extend_from_slice(&[0u8; 13]);
        let req = multipart(vec![
            ("patient", None, "text/plain", b"A. Patient".to_vec()),
            ("slot", None, "text/plain", b"2026-09-01T10:00Z".to_vec()),
            ("photo", Some("shot.png"), "image/png", png),
        ]);
        let mp = Multipart::from_request(req, &()).await.expect("parses");
        let store = MemStore::default();
        let (fields, stored) =
            ingest_multipart_mixed(mp, &UploadLimits::default(), &store, "uploads", "pfx/")
                .await
                .unwrap();
        assert_eq!(
            fields,
            vec![
                ("patient".to_string(), "A. Patient".to_string()),
                ("slot".to_string(), "2026-09-01T10:00Z".to_string()),
            ],
            "text parts are retained for the graph input"
        );
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].field, "photo");
        assert_eq!((stored[0].width, stored[0].height), (Some(300), Some(200)));
        assert!(store.get(&stored[0].object).is_ok());
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
