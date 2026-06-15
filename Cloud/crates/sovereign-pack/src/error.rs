// Error types for sovereign-pack.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PackError {
    #[error("missing manifest.json in archive")]
    MissingManifest,

    #[error("invalid manifest: {0}")]
    InvalidManifest(String),

    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("zstd decode error: {0}")]
    ZstdDecode(std::io::Error),

    #[error("tar read error: {0}")]
    TarRead(std::io::Error),

    #[error("tar entry error: {0}")]
    TarEntry(std::io::Error),

    #[error("checksum mismatch for `{file}`: expected {expected}, got {actual}")]
    ChecksumMismatch {
        file: String,
        expected: String,
        actual: String,
    },

    #[error("checksum file parse error: {0}")]
    ChecksumParse(String),

    #[error("checksums.sha256 missing from archive")]
    MissingChecksumFile,

    #[error("archive validation failed: {count} errors: {errors}")]
    ValidationFailed { count: usize, errors: String },

    #[error("path traversal detected in archive entry: {0}")]
    PathTraversal(String),

    #[error("symlink detected in archive entry: {0}")]
    SymlinkDetected(String),

    #[error("tar zstd encode error: {0}")]
    ZstdEncode(std::io::Error),
}
