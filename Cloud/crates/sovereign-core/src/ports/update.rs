use std::path::Path;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The update channel. V0 supports `Stable` only; `Rc` and `Nightly`
/// are reserved for V1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum UpdateChannel {
    Stable,
    Rc,
    Nightly,
}

impl UpdateChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            UpdateChannel::Stable => "stable",
            UpdateChannel::Rc => "rc",
            UpdateChannel::Nightly => "nightly",
        }
    }
}

impl std::fmt::Display for UpdateChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A semver-ish version. V0 uses `MAJOR.MINOR.PATCH`; pre-release
/// tags (`-rc1`, `-nightly.20251201`) are parsed but not compared
/// strictly — `0.1.0-rc1` < `0.1.0`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    pub fn parse(s: &str) -> Result<Self, UpdateError> {
        let s = s.trim().trim_start_matches('v');
        // Allow `0.1.0-rc1` by stripping the suffix.
        let numeric = s.split('-').next().unwrap_or(s);
        let mut parts = numeric.split('.');
        let major = parts
            .next()
            .and_then(|p| p.parse().ok())
            .ok_or_else(|| UpdateError::BadVersion(s.into()))?;
        let minor = parts
            .next()
            .and_then(|p| p.parse().ok())
            .ok_or_else(|| UpdateError::BadVersion(s.into()))?;
        let patch = parts
            .next()
            .and_then(|p| p.parse().ok())
            .ok_or_else(|| UpdateError::BadVersion(s.into()))?;
        Ok(Version(major, minor, patch))
    }

    pub fn as_str(&self) -> String {
        format!("{}.{}.{}", self.0, self.0, self.2)
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// A SHA-256 digest, hex-encoded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sha256(pub String);

impl Sha256 {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        use std::fmt::Write;
        let mut s = String::with_capacity(64);
        for b in bytes {
            let _ = write!(&mut s, "{:02x}", b);
        }
        Sha256(s)
    }

    pub fn compute(bytes: &[u8]) -> Self {
        use sha2::{Digest, Sha256 as Sha256Hasher};
        let mut hasher = Sha256Hasher::new();
        hasher.update(bytes);
        let digest = hasher.finalize();
        Sha256::from_bytes(&digest)
    }

    pub fn is_hex64(&self) -> bool {
        self.0.len() == 64 && self.0.chars().all(|c| c.is_ascii_hexdigit())
    }
}

impl std::fmt::Display for Sha256 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A binary's location and checksum in a release manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binary {
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

/// The full release manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub channel: String,
    pub released_at: String,
    pub binaries: std::collections::BTreeMap<String, Binary>,
}

/// An entry in the `update_history` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateRecord {
    pub from_version: String,
    pub to_version: String,
    pub channel: String,
    pub sha256: String,
    pub applied_at: String,
    pub backup_path: String,
}

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("bad version string: {0}")]
    BadVersion(String),
    #[error("checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("binary at {0} is not a regular file")]
    NotAFile(String),
    #[error("could not read binary at {0}: {1}")]
    ReadFailed(String, String),
    #[error("backup directory is not writable: {0}")]
    BackupNotWritable(String),
    #[error("no previous version to roll back to")]
    NoPreviousVersion,
    #[error("network error: {0}")]
    Network(String),
    #[error("manifest parse error: {0}")]
    ManifestParse(String),
    #[error("unsupported target triple: {0}")]
    UnsupportedTriple(String),
}

/// The abstract interface for fetching and applying updates. The
/// in-tree `HttpUpdate` implementation is the V0 default; tests
/// inject a `MockUpdate` so they do not need a network.
#[async_trait]
pub trait UpdatePort: Send + Sync {
    /// The version of the currently running binary.
    async fn current_version(&self) -> Result<Version, UpdateError>;

    /// The latest release available for the given channel.
    async fn latest(&self, channel: UpdateChannel) -> Result<Release, UpdateError>;

    /// Fetch the binary for `target_triple` from `release` to `dest`.
    /// Returns the SHA-256 of the bytes that were written.
    async fn fetch(
        &self,
        release: &Release,
        target_triple: &str,
        dest: &Path,
    ) -> Result<Sha256, UpdateError>;

    /// Apply the binary at `new_binary` to the running installation.
    /// `current_binary` is the path of the running binary (the one
    /// we are updating in place). `backup_dir` is the directory the
    /// old binary is copied to. Returns the `UpdateRecord` that was
    /// written to the database.
    async fn apply(
        &self,
        new_binary: &Path,
        current_binary: &Path,
        backup_dir: &Path,
        release: &Release,
    ) -> Result<UpdateRecord, UpdateError>;

    /// Roll back to the previous binary per the given `UpdateRecord`.
    async fn rollback(&self, record: &UpdateRecord) -> Result<(), UpdateError>;
}
