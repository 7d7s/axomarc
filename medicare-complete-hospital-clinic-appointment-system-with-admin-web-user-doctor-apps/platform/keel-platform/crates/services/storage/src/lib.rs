//! # kelson-storage
//! Storage port (11-SPEC §45). The domain never touches S3/MinIO directly;
//! adapters implement this port.

use kelson_foundation::AppError;
#[cfg(feature = "tenant-ports")]
use kelson_foundation::TenantId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Maximum directory-prefix length (mirrors the object-key bound).
pub const MAX_PREFIX_LEN: usize = 1024;

/// Wave 6 P2: shared directory-prefix guard for [`StoragePort`]'s directory
/// ops. One trailing `/` is allowed (directories are addressed like paths);
/// everything else follows the object-key rules — relative, no dot/empty
/// segments, bounded length.
pub fn validate_dir_prefix(prefix: &str) -> Result<(), AppError> {
    let core = prefix.strip_suffix('/').unwrap_or(prefix);
    if core.is_empty() || core.len() > MAX_PREFIX_LEN {
        return Err(AppError::validation(format!(
            "storage directory prefix must be 1..={MAX_PREFIX_LEN} characters"
        )));
    }
    if core.contains('\\') || core.contains(':') {
        return Err(AppError::validation(
            "storage directory prefix must not contain backslashes or colons",
        ));
    }
    if core.starts_with('/') || core.starts_with('~') {
        return Err(AppError::validation(
            "storage directory prefix must be relative",
        ));
    }
    for segment in core.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(AppError::validation(
                "storage directory prefix has an empty or relative segment",
            ));
        }
    }
    Ok(())
}

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
    /// by `max_bytes` (see `kelson-storage-local`, which reads at most
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

    /// Wave 6 P2 (directory ops): every object under a prefix, sorted by
    /// `bucket/key` where `bucket/key` starts with the prefix at a `/`
    /// boundary. The default is `NOT_IMPLEMENTED` — adapters that can walk
    /// their backing store override it (`kelson-storage-local`,
    /// `kelson-storage-s3`).
    fn list(&self, _prefix: &str) -> Result<Vec<ObjectRef>, AppError> {
        Err(AppError::not_implemented("list"))
    }

    /// Wave 6 P2 (directory ops): create a directory marker under the
    /// prefix (idempotent). Default: `NOT_IMPLEMENTED` (see [`Self::list`]).
    fn make_dir(&self, _prefix: &str) -> Result<(), AppError> {
        Err(AppError::not_implemented("make_dir"))
    }

    /// Wave 6 P2 (directory ops): delete every object under the prefix,
    /// recursively; a missing directory is success (idempotent). Default:
    /// `NOT_IMPLEMENTED` (see [`Self::list`]).
    fn delete_dir(&self, _prefix: &str) -> Result<(), AppError> {
        Err(AppError::not_implemented("delete_dir"))
    }

    /// Wave 6 P10 (HTTP client sink): open a chunked write session for a
    /// large object. The default buffers every chunk and performs one
    /// `put` on [`ObjectWriter::finish`] — adapters that own a real sink
    /// (see `kelson-storage-local`) override it to write incrementally, so
    /// a multi-hundred-megabyte download never has to live in memory at
    /// once. A session that is dropped without `finish` commits nothing.
    fn writer<'a>(
        &'a self,
        bucket: &str,
        key: &str,
    ) -> Result<Box<dyn ObjectWriter + 'a>, AppError> {
        Ok(Box::new(BufferedWriter {
            port: self,
            bucket: bucket.to_string(),
            key: key.to_string(),
            buf: Vec::new(),
        }))
    }
}

/// Wave 6 P10: one chunked write session opened by [`StoragePort::writer`].
/// `finish` is the only way a well-behaved sink commits the object;
/// dropping the writer aborts it (partial data is discarded, never
/// published under the target key).
pub trait ObjectWriter: Send {
    /// Append a chunk to the object under construction.
    fn write(&mut self, chunk: &[u8]) -> Result<(), AppError>;
    /// Commit the object and return its reference.
    fn finish(self: Box<Self>) -> Result<ObjectRef, AppError>;
}

/// Default [`StoragePort::writer`] implementation: accumulates the chunks
/// and issues a single `put` on `finish`, so the port stays implementable
/// with its core four methods. Generic over `P` (not `dyn StoragePort`)
/// so the default compiles for `Self: ?Sized` — `P = dyn StoragePort`
/// works through the same path.
struct BufferedWriter<'a, P: StoragePort + ?Sized> {
    port: &'a P,
    bucket: String,
    key: String,
    buf: Vec<u8>,
}

impl<P: StoragePort + ?Sized> ObjectWriter for BufferedWriter<'_, P> {
    fn write(&mut self, chunk: &[u8]) -> Result<(), AppError> {
        self.buf.extend_from_slice(chunk);
        Ok(())
    }

    fn finish(self: Box<Self>) -> Result<ObjectRef, AppError> {
        let session = *self;
        session.port.put(&session.bucket, &session.key, session.buf)
    }
}

// --------------------------------------------- tenant-scoped port (target §3.1)

/// Tenant-scoped storage port: every call carries `TenantId` (§3.1) and every
/// object lives under `tenant/{id}/…` inside its bucket (§3.3's scheme), so
/// isolation is structural — a forged or buggy key cannot address another
/// tenant's objects. [`put`](Self::put) returns the **logical** reference
/// (namespace never leaks to callers); directory ops keep the composite
/// `bucket/key` prefix contract of [`StoragePort`], scoped by the adapter.
/// The physical port stays the platform layer (break-glass, mirroring the
/// `kelson_platform` database role).
#[cfg(feature = "tenant-ports")]
pub trait TenantStoragePort: Send + Sync {
    fn put(
        &self,
        tenant: TenantId,
        bucket: &str,
        key: &str,
        bytes: Vec<u8>,
    ) -> Result<ObjectRef, AppError>;
    fn get(&self, tenant: TenantId, obj: &ObjectRef) -> Result<Vec<u8>, AppError>;
    fn delete(&self, tenant: TenantId, obj: &ObjectRef) -> Result<(), AppError>;
    fn presigned_url(
        &self,
        tenant: TenantId,
        obj: &ObjectRef,
        ttl_secs: u64,
    ) -> Result<String, AppError>;

    /// Same capped-read contract as [`StoragePort::get_capped`], scoped.
    fn get_capped(
        &self,
        tenant: TenantId,
        obj: &ObjectRef,
        max_bytes: u64,
    ) -> Result<Vec<u8>, AppError> {
        let bytes = self.get(tenant, obj)?;
        if bytes.len() as u64 > max_bytes {
            return Err(AppError::cap_limit(format!(
                "{}/{} exceeds the {max_bytes} byte cap",
                obj.bucket, obj.key
            )));
        }
        Ok(bytes)
    }

    /// Every object under a tenant-scoped prefix; default
    /// `NOT_IMPLEMENTED`, exactly like [`StoragePort::list`].
    fn list(&self, _tenant: TenantId, _prefix: &str) -> Result<Vec<ObjectRef>, AppError> {
        Err(AppError::not_implemented("list"))
    }

    /// Tenant-scoped directory marker; default `NOT_IMPLEMENTED`
    /// (mirrors [`StoragePort::make_dir`]).
    fn make_dir(&self, _tenant: TenantId, _prefix: &str) -> Result<(), AppError> {
        Err(AppError::not_implemented("make_dir"))
    }

    /// Tenant-scoped recursive delete; default `NOT_IMPLEMENTED`
    /// (mirrors [`StoragePort::delete_dir`]).
    fn delete_dir(&self, _tenant: TenantId, _prefix: &str) -> Result<(), AppError> {
        Err(AppError::not_implemented("delete_dir"))
    }
}

/// §3.3 transition adapter (`TenantShardedAdapter`): wraps ANY physical
/// [`StoragePort`] and rewrites every key into the tenant's namespace —
/// memory/local/S3 adapters gain isolation without being rewritten.
#[cfg(feature = "tenant-ports")]
pub struct TenantStorageShard {
    inner: Arc<dyn StoragePort>,
}

#[cfg(feature = "tenant-ports")]
impl TenantStorageShard {
    pub fn new(inner: Arc<dyn StoragePort>) -> Self {
        Self { inner }
    }

    /// Namespace an object key: `tenant/{id}/{key}`. The guard mirrors the
    /// adapters' key rules (relative, no traversal, bounded) so a crafted
    /// `../` key can never escape the namespace on adapters that validate
    /// less strictly.
    fn k(tenant: TenantId, key: &str) -> Result<String, AppError> {
        if key.is_empty() {
            return Err(AppError::validation("storage key must not be empty"));
        }
        let scoped = format!("tenant/{tenant}/{key}");
        validate_dir_prefix(&scoped)?;
        Ok(scoped)
    }

    /// Namespace a composite `bucket/key` directory prefix: the tenant
    /// segment is inserted after the bucket (the directory contract is
    /// composite — `uploads/2026/` → `uploads/tenant/{id}/2026/`).
    fn scoped_prefix(tenant: TenantId, prefix: &str) -> Result<String, AppError> {
        validate_dir_prefix(prefix)?;
        let (bucket, rest) = prefix.split_once('/').unwrap_or((prefix, ""));
        if rest.is_empty() {
            Ok(format!("{bucket}/tenant/{tenant}"))
        } else {
            Ok(format!("{bucket}/tenant/{tenant}/{rest}"))
        }
    }

    /// Strip the tenant segment from a physical key on the way out, so
    /// callers only ever see logical references.
    fn logical(tenant: TenantId, obj: ObjectRef) -> Result<ObjectRef, AppError> {
        let head = format!("tenant/{tenant}/");
        let key = obj.key.strip_prefix(head.as_str()).ok_or_else(|| {
            AppError::persistence(format!(
                "storage adapter returned a key outside the tenant namespace: {}",
                obj.key
            ))
        })?;
        Ok(ObjectRef {
            bucket: obj.bucket,
            key: key.to_string(),
        })
    }
}

#[cfg(feature = "tenant-ports")]
impl TenantStoragePort for TenantStorageShard {
    fn put(
        &self,
        tenant: TenantId,
        bucket: &str,
        key: &str,
        bytes: Vec<u8>,
    ) -> Result<ObjectRef, AppError> {
        self.inner.put(bucket, &Self::k(tenant, key)?, bytes)?;
        // Return the LOGICAL reference — round-trips through get/delete as-is.
        Ok(ObjectRef {
            bucket: bucket.to_string(),
            key: key.to_string(),
        })
    }

    fn get(&self, tenant: TenantId, obj: &ObjectRef) -> Result<Vec<u8>, AppError> {
        self.inner.get(&ObjectRef {
            bucket: obj.bucket.clone(),
            key: Self::k(tenant, &obj.key)?,
        })
    }

    fn delete(&self, tenant: TenantId, obj: &ObjectRef) -> Result<(), AppError> {
        self.inner.delete(&ObjectRef {
            bucket: obj.bucket.clone(),
            key: Self::k(tenant, &obj.key)?,
        })
    }

    fn presigned_url(
        &self,
        tenant: TenantId,
        obj: &ObjectRef,
        ttl_secs: u64,
    ) -> Result<String, AppError> {
        self.inner.presigned_url(
            &ObjectRef {
                bucket: obj.bucket.clone(),
                key: Self::k(tenant, &obj.key)?,
            },
            ttl_secs,
        )
    }

    fn list(&self, tenant: TenantId, prefix: &str) -> Result<Vec<ObjectRef>, AppError> {
        let listed = self.inner.list(&Self::scoped_prefix(tenant, prefix)?)?;
        listed
            .into_iter()
            .map(|obj| Self::logical(tenant, obj))
            .collect()
    }

    fn make_dir(&self, tenant: TenantId, prefix: &str) -> Result<(), AppError> {
        self.inner.make_dir(&Self::scoped_prefix(tenant, prefix)?)
    }

    fn delete_dir(&self, tenant: TenantId, prefix: &str) -> Result<(), AppError> {
        self.inner.delete_dir(&Self::scoped_prefix(tenant, prefix)?)
    }
}

/// Wave 6 P2: disk visibility. Private objects are served only through
/// signed URLs; public ones resolve through the disk's `url` base (or the
/// `storage:link` path for local disks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Public,
    Private,
}

/// One registered disk: its adapter plus the URL-issuance policy.
pub struct DiskEntry {
    pub port: Arc<dyn StoragePort>,
    pub visibility: Visibility,
    /// Public base URL (`[storage.disks.<name>].url`); required to issue a
    /// public URL, ignored for private disks.
    pub base_url: Option<String>,
}

/// Wave 6 P2: named disk registry — resolves a configured disk name to its
/// [`StoragePort`] and owns the visibility policy for URL issuance. The
/// runtime builders (kelson-demo, kelson-worker) fill it from `[storage]`; the
/// registry itself stays adapter-agnostic per the layer matrix.
pub struct DiskRegistry {
    disks: BTreeMap<String, DiskEntry>,
    default: String,
}

/// Ports are not `Debug`; show the shape (default + registered names)
/// instead so `unwrap_err`/asserts on builder results stay usable.
impl std::fmt::Debug for DiskRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskRegistry")
            .field("default", &self.default)
            .field("disks", &self.names())
            .finish()
    }
}

impl DiskRegistry {
    pub fn new(default: impl Into<String>) -> Self {
        Self {
            disks: BTreeMap::new(),
            default: default.into(),
        }
    }

    pub fn register(&mut self, name: &str, entry: DiskEntry) -> Result<(), AppError> {
        if name.is_empty() {
            return Err(AppError::validation("storage disk name must not be empty"));
        }
        if self.disks.contains_key(name) {
            return Err(AppError::conflict(format!(
                "storage disk {name:?} is already registered"
            )));
        }
        self.disks.insert(name.to_string(), entry);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Result<&DiskEntry, AppError> {
        self.disks.get(name).ok_or_else(|| {
            let configured = self.names().join(", ");
            AppError::validation(format!(
                "unknown storage disk {name:?} (configured: {})",
                if configured.is_empty() {
                    "none".into()
                } else {
                    configured
                }
            ))
        })
    }

    /// Resolve a request-supplied disk name against the default
    /// (`storage.default` when the caller names none).
    pub fn resolve<'a>(
        &'a self,
        requested: Option<&'a str>,
    ) -> Result<(&'a str, &'a DiskEntry), AppError> {
        let name = requested.unwrap_or(self.default.as_str());
        let entry = self.get(name)?;
        Ok((name, entry))
    }

    pub fn default_name(&self) -> &str {
        &self.default
    }

    pub fn names(&self) -> Vec<&str> {
        self.disks.keys().map(String::as_str).collect()
    }

    /// Boot check: the default disk must be registered (call once the
    /// registry is filled — an unknown `storage.default` refuses boot).
    pub fn validate_default(&self) -> Result<(), AppError> {
        self.get(self.default_name()).map(|_| ())
    }

    /// Wave 6 P2: visibility policy for URL issuance. Private objects only
    /// ever get a signed URL from the port; public objects resolve through
    /// the disk's `url` base as `{base}/{bucket}/{key}`.
    pub fn url_for(&self, disk: &str, obj: &ObjectRef, ttl_secs: u64) -> Result<String, AppError> {
        let entry = self.get(disk)?;
        match entry.visibility {
            Visibility::Private => entry.port.presigned_url(obj, ttl_secs),
            Visibility::Public => {
                let base = entry.base_url.as_deref().ok_or_else(|| {
                    AppError::validation(format!(
                        "public storage disk {disk:?} needs a `url` base for issuance"
                    ))
                })?;
                Ok(format!(
                    "{}/{}/{}",
                    base.trim_end_matches('/'),
                    obj.bucket,
                    obj.key
                ))
            }
        }
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
        assert_eq!(err.code, kelson_foundation::codes::CAP_LIMIT);
    }

    /// Wave 6 P2: adapters that cannot walk a backing store report
    /// `NOT_IMPLEMENTED` for the directory ops (the port stays implementable
    /// with the core four methods).
    #[test]
    fn directory_ops_default_to_not_implemented() {
        let port = Blob(vec![]);
        let err = port.list("uploads/").unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::NOT_IMPLEMENTED);
        assert!(
            err.public_message.contains("list"),
            "{}",
            err.public_message
        );
        assert_eq!(
            port.make_dir("uploads/").unwrap_err().code,
            kelson_foundation::codes::NOT_IMPLEMENTED
        );
        assert_eq!(
            port.delete_dir("uploads/").unwrap_err().code,
            kelson_foundation::codes::NOT_IMPLEMENTED
        );
    }

    /// Wave 6 P2: the shared prefix guard accepts path-like directory
    /// addresses and refuses traversal (mirrors the object-key rules).
    #[test]
    fn validate_dir_prefix_guards_traversal() {
        for ok in ["uploads/", "uploads/abc/", "a/b/c", "tenant_1"] {
            validate_dir_prefix(ok).unwrap_or_else(|e| panic!("{ok}: {}", e.public_message));
        }
        for bad in [
            "", "/", "/abs/", "a/../b", "a\\b", "a:z", "~user", "a//b", "..", "a/./b",
        ] {
            assert!(
                validate_dir_prefix(bad).is_err(),
                "prefix accepted: {bad:?}"
            );
        }
    }

    /// Presign-capable fake for the registry policy tests.
    struct Signed;

    impl StoragePort for Signed {
        fn put(&self, _bucket: &str, _key: &str, _bytes: Vec<u8>) -> Result<ObjectRef, AppError> {
            Err(AppError::not_implemented("test put"))
        }
        fn get(&self, _obj: &ObjectRef) -> Result<Vec<u8>, AppError> {
            Ok(Vec::new())
        }
        fn delete(&self, _obj: &ObjectRef) -> Result<(), AppError> {
            Ok(())
        }
        fn presigned_url(&self, obj: &ObjectRef, ttl_secs: u64) -> Result<String, AppError> {
            Ok(format!("signed://{}/{}?t={ttl_secs}", obj.bucket, obj.key))
        }
    }

    fn obj() -> ObjectRef {
        ObjectRef {
            bucket: "uploads".into(),
            key: "2026/q3.png".into(),
        }
    }

    /// Wave 6 P2: name resolution (default vs explicit), duplicate/unknown
    /// refusals, and the visibility policy — private ports only ever sign,
    /// public ones resolve through the `url` base.
    #[test]
    fn registry_resolves_disks_and_enforces_visibility() {
        let mut registry = DiskRegistry::new("private");
        registry
            .register(
                "private",
                DiskEntry {
                    port: Arc::new(Signed),
                    visibility: Visibility::Private,
                    base_url: None,
                },
            )
            .unwrap();
        registry
            .register(
                "cdn",
                DiskEntry {
                    port: Arc::new(Signed),
                    visibility: Visibility::Public,
                    base_url: Some("https://cdn.example.com/".into()),
                },
            )
            .unwrap();
        registry.validate_default().unwrap();

        // Request-supplied name vs default.
        let (name, _) = registry.resolve(None).unwrap();
        assert_eq!(name, "private");
        let (name, _) = registry.resolve(Some("cdn")).unwrap();
        assert_eq!(name, "cdn");
        assert_eq!(registry.default_name(), "private");
        assert_eq!(registry.names(), vec!["cdn", "private"]); // sorted

        // Private -> signed by the port; public -> base + bucket/key.
        assert_eq!(
            registry.url_for("private", &obj(), 60).unwrap(),
            "signed://uploads/2026/q3.png?t=60"
        );
        assert_eq!(
            registry.url_for("cdn", &obj(), 60).unwrap(),
            "https://cdn.example.com/uploads/2026/q3.png"
        );

        // Unknown disk names the miss and lists what exists.
        let err = registry.url_for("nope", &obj(), 60).unwrap_err();
        assert!(
            err.public_message.contains("nope"),
            "{}",
            err.public_message
        );
        assert!(err.public_message.contains("cdn"), "{}", err.public_message);

        // Public without a base is a config gap surfaced at issuance.
        registry
            .register(
                "bare",
                DiskEntry {
                    port: Arc::new(Signed),
                    visibility: Visibility::Public,
                    base_url: None,
                },
            )
            .unwrap();
        let err = registry.url_for("bare", &obj(), 60).unwrap_err();
        assert!(err.public_message.contains("url"), "{}", err.public_message);

        // Duplicates refuse; an unregistered default fails the boot check.
        let dup = registry
            .register(
                "cdn",
                DiskEntry {
                    port: Arc::new(Signed),
                    visibility: Visibility::Public,
                    base_url: None,
                },
            )
            .unwrap_err();
        assert_eq!(dup.code, kelson_foundation::codes::CONFLICT);

        let mut empty_name = DiskRegistry::new("");
        let err = empty_name
            .register(
                "",
                DiskEntry {
                    port: Arc::new(Signed),
                    visibility: Visibility::Private,
                    base_url: None,
                },
            )
            .unwrap_err();
        assert!(
            err.public_message.contains("empty"),
            "{}",
            err.public_message
        );
        assert!(empty_name.validate_default().is_err());
    }

    /// Wave 6 P10: a port that records every `put` (the default writer's
    /// only commit path).
    #[derive(Default)]
    struct Recorder(std::sync::Mutex<Vec<(String, String, Vec<u8>)>>);

    impl StoragePort for Recorder {
        fn put(&self, bucket: &str, key: &str, bytes: Vec<u8>) -> Result<ObjectRef, AppError> {
            self.0
                .lock()
                .unwrap()
                .push((bucket.to_string(), key.to_string(), bytes));
            Ok(ObjectRef {
                bucket: bucket.to_string(),
                key: key.to_string(),
            })
        }
        fn get(&self, _obj: &ObjectRef) -> Result<Vec<u8>, AppError> {
            Err(AppError::not_implemented("test get"))
        }
        fn delete(&self, _obj: &ObjectRef) -> Result<(), AppError> {
            Ok(())
        }
        fn presigned_url(&self, _obj: &ObjectRef, _ttl_secs: u64) -> Result<String, AppError> {
            Err(AppError::not_implemented("test presign"))
        }
    }

    /// The default writer buffers chunks in order and commits exactly one
    /// `put` on `finish`.
    #[test]
    fn default_writer_buffers_and_commits_on_finish() {
        let port = Recorder::default();
        {
            let mut writer = port.writer("exports", "big.bin").unwrap();
            writer.write(b"chunk-1").unwrap();
            writer.write(b"chunk-2").unwrap();
            let obj = writer.finish().unwrap();
            assert_eq!(obj.bucket, "exports");
            assert_eq!(obj.key, "big.bin");
        }
        let written = port.0.lock().unwrap();
        assert_eq!(written.len(), 1, "finish is the single commit point");
        assert_eq!(written[0].0, "exports");
        assert_eq!(written[0].1, "big.bin");
        assert_eq!(written[0].2, b"chunk-1chunk-2");
    }

    /// An aborted session (drop without `finish`) publishes nothing — the
    /// sink's honesty guarantee for mid-stream failures.
    #[test]
    fn aborted_writer_session_commits_nothing() {
        let port = Recorder::default();
        {
            let mut writer = port.writer("exports", "half.bin").unwrap();
            writer.write(b"partial").unwrap();
            // Dropped without finish.
        }
        assert!(
            port.0.lock().unwrap().is_empty(),
            "an unfinished session must not reach put"
        );
    }

    // ------------------------------------------- tenant isolation probes (DoD)

    /// Map-backed fake with the full surface (incl. directory ops) — the
    /// isolation probes' substrate (Services must not depend on Adapters).
    #[cfg(feature = "tenant-ports")]
    #[derive(Default)]
    struct MapStore(std::sync::Mutex<BTreeMap<String, Vec<u8>>>); // "bucket/key"

    #[cfg(feature = "tenant-ports")]
    impl StoragePort for MapStore {
        fn put(&self, bucket: &str, key: &str, bytes: Vec<u8>) -> Result<ObjectRef, AppError> {
            self.0
                .lock()
                .unwrap()
                .insert(format!("{bucket}/{key}"), bytes);
            Ok(ObjectRef {
                bucket: bucket.to_string(),
                key: key.to_string(),
            })
        }
        fn get(&self, obj: &ObjectRef) -> Result<Vec<u8>, AppError> {
            self.0
                .lock()
                .unwrap()
                .get(&format!("{}/{}", obj.bucket, obj.key))
                .cloned()
                .ok_or_else(|| AppError::not_found("object"))
        }
        fn delete(&self, obj: &ObjectRef) -> Result<(), AppError> {
            self.0
                .lock()
                .unwrap()
                .remove(&format!("{}/{}", obj.bucket, obj.key));
            Ok(()) // idempotent, like the real adapters
        }
        fn presigned_url(&self, obj: &ObjectRef, ttl_secs: u64) -> Result<String, AppError> {
            Ok(format!("signed://{}/{}?t={ttl_secs}", obj.bucket, obj.key))
        }
        fn list(&self, prefix: &str) -> Result<Vec<ObjectRef>, AppError> {
            let map = self.0.lock().unwrap();
            let mut out: Vec<ObjectRef> = map
                .keys()
                .filter(|k| k.starts_with(prefix))
                .filter_map(|k| {
                    let (bucket, key) = k.split_once('/')?;
                    Some(ObjectRef {
                        bucket: bucket.to_string(),
                        key: key.to_string(),
                    })
                })
                .collect();
            out.sort_by(|a, b| (&a.bucket, &a.key).cmp(&(&b.bucket, &b.key)));
            Ok(out)
        }
        fn make_dir(&self, _prefix: &str) -> Result<(), AppError> {
            Ok(())
        }
        fn delete_dir(&self, prefix: &str) -> Result<(), AppError> {
            // Faithful to the real adapters: every physical key under the
            // prefix goes. This was a no-op, which meant NO test in this
            // crate could observe delete_dir at all — a broken
            // `scoped_prefix` would have passed unnoticed (Phase 5 pentest).
            self.0.lock().unwrap().retain(|k, _| !k.starts_with(prefix));
            Ok(())
        }
    }

    #[cfg(feature = "tenant-ports")]
    fn shard() -> TenantStorageShard {
        TenantStorageShard::new(Arc::new(MapStore::default()))
    }

    /// Cross-tenant read/write/delete probe over object storage.
    #[cfg(feature = "tenant-ports")]
    #[test]
    fn tenant_isolation_storage_objects_do_not_cross_tenants() {
        let shard = shard();
        let a = TenantId::new();
        let b = TenantId::new();

        let ref_a = shard
            .put(a, "uploads", "2026/report.pdf", b"A".to_vec())
            .unwrap();
        // put answers with the LOGICAL reference — the namespace stays internal.
        assert_eq!(ref_a.key, "2026/report.pdf");
        assert_eq!(shard.get(a, &ref_a).unwrap(), b"A");

        let err = shard.get(b, &ref_a).unwrap_err();
        assert_eq!(
            err.code,
            kelson_foundation::codes::NOT_FOUND,
            "cross-tenant read must miss"
        );

        // B writes the same logical key — A still reads its own bytes.
        shard
            .put(b, "uploads", "2026/report.pdf", b"B".to_vec())
            .unwrap();
        assert_eq!(shard.get(a, &ref_a).unwrap(), b"A");
        assert_eq!(shard.get(b, &ref_a).unwrap(), b"B");

        // B deleting A's logical reference only empties B's namespace.
        shard.delete(b, &ref_a).unwrap();
        assert_eq!(
            shard.get(a, &ref_a).unwrap(),
            b"A",
            "cross-tenant delete must not erase"
        );
        assert_eq!(
            shard.get(b, &ref_a).unwrap_err().code,
            kelson_foundation::codes::NOT_FOUND,
            "B's delete emptied B's own namespace"
        );
    }

    /// Directory listing returns logical keys scoped to the calling tenant.
    #[cfg(feature = "tenant-ports")]
    #[test]
    fn tenant_isolation_storage_listing_is_namespaced() {
        let shard = shard();
        let a = TenantId::new();
        let b = TenantId::new();

        shard.put(a, "uploads", "x/1.png", vec![1]).unwrap();
        shard.put(a, "uploads", "y/2.png", vec![2]).unwrap();
        shard.put(b, "uploads", "x/9.png", vec![9]).unwrap();

        let list_a: Vec<String> = shard
            .list(a, "uploads/")
            .unwrap()
            .into_iter()
            .map(|o| o.key)
            .collect();
        assert_eq!(list_a, vec!["x/1.png".to_string(), "y/2.png".to_string()]);

        let list_b: Vec<String> = shard
            .list(b, "uploads/")
            .unwrap()
            .into_iter()
            .map(|o| o.key)
            .collect();
        assert_eq!(list_b, vec!["x/9.png".to_string()]);

        // Sub-prefix listing stays inside the tenant too.
        let sub: Vec<String> = shard
            .list(a, "uploads/x/")
            .unwrap()
            .into_iter()
            .map(|o| o.key)
            .collect();
        assert_eq!(sub, vec!["x/1.png".to_string()]);
    }

    /// Traversal keys are refused at the shard — no crafted key can reach
    /// outside the tenant namespace, regardless of adapter strictness.
    #[cfg(feature = "tenant-ports")]
    #[test]
    fn tenant_isolation_storage_traversal_keys_are_refused() {
        let shard = shard();
        let a = TenantId::new();
        for bad in ["../other/x", "a/../b", "/abs", "", "a//b", ".."] {
            let err = match shard.put(a, "uploads", bad, vec![1]) {
                Ok(_) => panic!("key {bad:?} must be refused"),
                Err(e) => e,
            };
            assert_eq!(
                err.code,
                kelson_foundation::codes::VALIDATION,
                "key {bad:?} must be refused"
            );
        }
    }

    /// Presigned URLs point at the tenant's own namespace only.
    #[cfg(feature = "tenant-ports")]
    #[test]
    fn tenant_isolation_storage_presign_stays_inside_namespace() {
        let shard = shard();
        let a = TenantId::new();
        let b = TenantId::new();

        let obj = shard.put(a, "uploads", "photo.png", vec![1]).unwrap();
        let url = shard.presigned_url(a, &obj, 60).unwrap();
        assert!(
            url.contains(&format!("tenant/{a}/")),
            "presigned URL must address the tenant namespace: {url}"
        );
        assert!(
            !url.contains(&format!("tenant/{b}/")),
            "presigned URL must not name another tenant"
        );
    }

    // -------------------------------------- pentest (Phase 5, §9 box 2)

    /// Penetration test: hostile PREFIXES (not just keys — the traversal
    /// probe above only covers `put`) and the most destructive operation the
    /// port exposes, `delete_dir`.
    ///
    /// `delete_dir` is the blast-radius case: if the tenant segment were
    /// dropped or misplaced for one call, a single tenant could erase every
    /// tenant's objects in a bucket. It must be scoped in BOTH directions —
    /// refuses to walk out, and cannot reach sideways.
    #[cfg(feature = "tenant-ports")]
    #[test]
    fn pentest_storage_hostile_prefixes_and_delete_dir_stay_inside_the_tenant() {
        let shard = shard();
        let a = TenantId::new();
        let b = TenantId::new();

        shard.put(a, "uploads", "x/1.png", vec![1]).unwrap();
        shard.put(a, "uploads", "y/2.png", vec![2]).unwrap();
        shard.put(b, "uploads", "x/9.png", vec![9]).unwrap();
        shard.put(b, "reports", "q3.pdf", vec![9]).unwrap();

        // (1) Hostile prefixes on the READ side: every one is refused rather
        //     than silently listing another namespace.
        for bad in [
            "uploads/../../",
            "uploads/../",
            "../uploads/",
            "/uploads/",
            "uploads//",
        ] {
            let err = match shard.list(a, bad) {
                Ok(hits) => panic!("prefix {bad:?} must be refused, listed {hits:?}"),
                Err(e) => e,
            };
            assert_eq!(
                err.code,
                kelson_foundation::codes::VALIDATION,
                "prefix {bad:?} must be refused"
            );
        }

        // (2) A wipes its own bucket directory. B's objects in the SAME
        //     bucket survive — the tenant segment is part of the path the
        //     delete addresses.
        shard.delete_dir(a, "uploads/").unwrap();
        assert!(
            shard.list(a, "uploads/").unwrap().is_empty(),
            "A's uploads are gone"
        );
        assert_eq!(
            shard
                .list(b, "uploads/")
                .unwrap()
                .into_iter()
                .map(|o| o.key)
                .collect::<Vec<_>>(),
            vec!["x/9.png".to_string()],
            "B's objects in the same bucket survive A's delete_dir"
        );
        assert_eq!(
            shard.list(b, "reports/").unwrap().len(),
            1,
            "B's other directory untouched"
        );

        // (3) A hostile delete_dir is refused outright, not executed loosely.
        let err = shard.delete_dir(a, "uploads/../").unwrap_err();
        assert_eq!(err.code, kelson_foundation::codes::VALIDATION);
        assert_eq!(
            shard
                .list(b, "uploads/")
                .unwrap()
                .into_iter()
                .map(|o| o.key)
                .collect::<Vec<_>>(),
            vec!["x/9.png".to_string()],
            "B still intact after the refused delete"
        );
    }
}
