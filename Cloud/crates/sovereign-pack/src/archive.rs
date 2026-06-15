// Archive create/extract for .sov format (tar.zst).
//
// The archive is a zstd-compressed tar stream. Files are written in
// a deterministic order: manifest.json first, checksums.sha256 last.

use crate::error::PackError;
use crate::manifest::SovManifest;
use crate::PackFile;

use sha2::{Digest, Sha256};

/// Create a .sov archive from a manifest and files.
pub fn create_archive(
    manifest: &SovManifest,
    files: &[PackFile],
    output: &std::path::Path,
) -> Result<(), PackError> {
    let manifest_json = serde_json::to_string_pretty(manifest).map_err(PackError::JsonParse)?;

    let mut checksums = std::collections::BTreeMap::new();

    let out_file = std::fs::File::create(output).map_err(PackError::Io)?;
    let encoder = zstd::Encoder::new(out_file, 3).map_err(PackError::ZstdEncode)?;
    let mut archive = tar::Builder::new(encoder);

    // 1. Write manifest.json first
    {
        let manifest_bytes = manifest_json.as_bytes();
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest_bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();

        archive
            .append_data(&mut header, "manifest.json", manifest_bytes)
            .map_err(PackError::TarEntry)?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut &manifest_bytes[..], &mut hasher).map_err(PackError::Io)?;
        let hash = format!("{:x}", hasher.finalize());
        checksums.insert("manifest.json".to_string(), hash);
    }

    // 2. Write each user file
    for file in files {
        let source = std::fs::File::open(&file.path).map_err(PackError::Io)?;
        let metadata = source.metadata().map_err(PackError::Io)?;

        let mut header = tar::Header::new_gnu();
        header.set_size(metadata.len());
        header.set_mode(0o644);
        header.set_cksum();

        archive
            .append_data(&mut header, &file.name, source)
            .map_err(PackError::TarEntry)?;

        // Compute checksum
        let mut hasher = Sha256::new();
        let mut f = std::fs::File::open(&file.path).map_err(PackError::Io)?;
        std::io::copy(&mut f, &mut hasher).map_err(PackError::Io)?;
        let hash = format!("{:x}", hasher.finalize());
        checksums.insert(file.name.clone(), hash);
    }

    // 3. Write checksums.sha256 last
    {
        let checksum_content: String = checksums
            .iter()
            .map(|(name, hash)| format!("{hash}  {name}\n"))
            .collect();

        let checksum_bytes = checksum_content.as_bytes();
        let mut header = tar::Header::new_gnu();
        header.set_size(checksum_bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();

        archive
            .append_data(&mut header, "checksums.sha256", checksum_bytes)
            .map_err(PackError::TarEntry)?;
    }

    // Finish the archive
    archive
        .into_inner()
        .map_err(PackError::ZstdEncode)?
        .finish()
        .map_err(PackError::ZstdEncode)?;

    Ok(())
}

/// Extract a .sov archive to a directory. Returns the parsed manifest.
pub fn extract_archive(data: &[u8], dest: &std::path::Path) -> Result<SovManifest, PackError> {
    // First pass: extract all files and compute checksums
    let cursor = std::io::Cursor::new(data);
    let decoder = zstd::Decoder::new(cursor).map_err(PackError::ZstdDecode)?;
    let mut archive = tar::Archive::new(decoder);

    let mut manifest_data = None;
    let mut checksum_data = None;
    let mut extracted_files = Vec::new();

    for entry in archive.entries().map_err(PackError::TarRead)? {
        let mut entry = entry.map_err(PackError::TarEntry)?;
        let path = entry
            .path()
            .map_err(PackError::TarEntry)?
            .to_string_lossy()
            .to_string();

        // Reject path traversal
        if path.contains("..") || path.starts_with('/') {
            return Err(PackError::PathTraversal(path));
        }

        // Reject symlinks
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() {
            return Err(PackError::SymlinkDetected(path));
        }

        if path == "manifest.json" {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut buf).map_err(PackError::Io)?;
            manifest_data = Some(buf);
        } else if path == "checksums.sha256" {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut buf).map_err(PackError::Io)?;
            checksum_data = Some(buf);
        } else {
            // Extract to dest
            let out_path = dest.join(&path);
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent).map_err(PackError::Io)?;
            }
            let mut out_file = std::fs::File::create(&out_path).map_err(PackError::Io)?;
            std::io::copy(&mut entry, &mut out_file).map_err(PackError::Io)?;

            // Compute checksum
            let mut hasher = Sha256::new();
            let mut f = std::fs::File::open(&out_path).map_err(PackError::Io)?;
            std::io::copy(&mut f, &mut hasher).map_err(PackError::Io)?;
            let hash = format!("{:x}", hasher.finalize());
            extracted_files.push((path, hash));
        }
    }

    // Parse manifest
    let manifest_bytes = manifest_data.ok_or(PackError::MissingManifest)?;
    let manifest: SovManifest =
        serde_json::from_slice(&manifest_bytes).map_err(PackError::JsonParse)?;

    // Validate checksums if present
    if let Some(checksum_bytes) = checksum_data {
        let expected = parse_checksums(&checksum_bytes)?;
        for (name, actual_hash) in &extracted_files {
            if let Some(expected_hash) = expected.get(name) {
                if expected_hash != actual_hash {
                    return Err(PackError::ChecksumMismatch {
                        file: name.clone(),
                        expected: expected_hash.clone(),
                        actual: actual_hash.clone(),
                    });
                }
            }
        }
        // Verify manifest checksum too
        let mut hasher = Sha256::new();
        hasher.update(&manifest_bytes);
        let manifest_hash = format!("{:x}", hasher.finalize());
        if let Some(expected_hash) = expected.get("manifest.json") {
            if expected_hash != &manifest_hash {
                return Err(PackError::ChecksumMismatch {
                    file: "manifest.json".into(),
                    expected: expected_hash.clone(),
                    actual: manifest_hash,
                });
            }
        }
    }

    Ok(manifest)
}

/// Parse a checksums.sha256 file. Format: "<hex>  <filename>\n"
fn parse_checksums(data: &[u8]) -> Result<std::collections::HashMap<String, String>, PackError> {
    let content = std::str::from_utf8(data).map_err(|e| PackError::ChecksumParse(e.to_string()))?;
    let mut result = std::collections::HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(2, "  ").collect();
        if parts.len() != 2 {
            return Err(PackError::ChecksumParse(format!("invalid line: `{line}`")));
        }
        result.insert(parts[1].to_string(), parts[0].to_string());
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::RuntimeKind;
    use std::io::Write;

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
            env: std::collections::HashMap::new(),
            created_at: 1717785600,
            built_by: "test".into(),
        }
    }

    #[test]
    fn create_and_extract_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = test_manifest();

        let dummy_path = dir.path().join("dummy.txt");
        let mut f = std::fs::File::create(&dummy_path).unwrap();
        f.write_all(b"hello world").unwrap();
        drop(f);

        let files = vec![PackFile {
            path: dummy_path,
            name: "dummy.txt".into(),
        }];

        let output = dir.path().join("test.sov");
        create_archive(&manifest, &files, &output).unwrap();

        let data = std::fs::read(&output).unwrap();
        let extract_dir = dir.path().join("extracted");
        std::fs::create_dir(&extract_dir).unwrap();

        let restored = extract_archive(&data, &extract_dir).unwrap();
        assert_eq!(restored.app, "test-app");

        let extracted_file = extract_dir.join("dummy.txt");
        let content = std::fs::read_to_string(extracted_file).unwrap();
        assert_eq!(content, "hello world");
    }

    #[test]
    fn rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = test_manifest();

        let dummy_path = dir.path().join("dummy.txt");
        std::fs::write(&dummy_path, b"evil").unwrap();

        let files = vec![PackFile {
            path: dummy_path,
            name: "../../etc/passwd".into(),
        }];

        let output = dir.path().join("evil.sov");
        let result = create_archive(&manifest, &files, &output);
        assert!(result.is_err(), "archive creation should reject traversal paths");
    }
}
