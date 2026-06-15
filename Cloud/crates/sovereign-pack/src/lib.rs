// sovereign-pack — .sov archive create/inspect/validate
//
// The .sov archive is the universal deploy artifact. CI builds it,
// sovereign unpacks it. No build logic needed on the VPS.
//
// Format: tar.zst containing:
//   manifest.json  (required, first file)
//   image.tar      (present if runtime=container)
//   binary         (present if runtime=native)
//   config/        (optional config overrides)
//   checksums.sha256 (required, last file)

pub mod archive;
pub mod checksum;
pub mod error;
pub mod manifest;

pub use error::PackError;
pub use manifest::{RuntimeKind, SovManifest};

use std::path::Path;

/// A file to include in the archive.
pub struct PackFile {
    /// Path to the source file on disk.
    pub path: std::path::PathBuf,
    /// Archive-relative name (e.g. "image.tar", "binary").
    pub name: String,
}

/// Create a `.sov` archive from a manifest and files.
pub fn create(manifest: &SovManifest, files: &[PackFile], output: &Path) -> Result<(), PackError> {
    archive::create_archive(manifest, files, output)
}

/// Extract a `.sov` archive to a directory. Returns the parsed manifest.
pub fn extract(archive_data: &[u8], dest: &Path) -> Result<SovManifest, PackError> {
    archive::extract_archive(archive_data, dest)
}

/// Read the manifest from a `.sov` archive without extracting.
pub fn inspect(archive_data: &[u8]) -> Result<SovManifest, PackError> {
    manifest::read_manifest_from_archive(archive_data)
}

/// Validate checksums and manifest schema of a `.sov` archive.
pub fn validate(archive_data: &[u8]) -> Result<(), PackError> {
    checksum::validate_archive(archive_data)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn test_manifest() -> SovManifest {
        SovManifest {
            app: "test-app".into(),
            version: "0.1.0".into(),
            runtime: RuntimeKind::Container,
            image_ref: Some("alpine:3.20".into()),
            binary_path: None,
            systemd_exec: None,
            health_path: Some("/health".into()),
            port: 8080,
            env: [("RUST_LOG".into(), "info".into())].into_iter().collect(),
            created_at: chrono::Utc::now().timestamp(),
            built_by: "test".into(),
        }
    }

    #[test]
    fn create_and_inspect_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = test_manifest();

        // Create a dummy file to include
        let dummy_path = dir.path().join("dummy.txt");
        std::fs::write(&dummy_path, b"hello world").unwrap();

        let files = vec![PackFile {
            path: dummy_path,
            name: "dummy.txt".into(),
        }];

        let output = dir.path().join("test.sov");
        create(&manifest, &files, &output).unwrap();

        let data = std::fs::read(&output).unwrap();
        let restored = inspect(&data).unwrap();
        assert_eq!(restored.app, "test-app");
        assert_eq!(restored.version, "0.1.0");
        assert_eq!(restored.port, 8080);
    }

    #[test]
    fn validate_valid_archive() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = test_manifest();

        let dummy_path = dir.path().join("dummy.txt");
        std::fs::write(&dummy_path, b"hello world").unwrap();

        let files = vec![PackFile {
            path: dummy_path,
            name: "dummy.txt".into(),
        }];

        let output = dir.path().join("test.sov");
        create(&manifest, &files, &output).unwrap();

        let data = std::fs::read(&output).unwrap();
        validate(&data).unwrap();
    }
}
