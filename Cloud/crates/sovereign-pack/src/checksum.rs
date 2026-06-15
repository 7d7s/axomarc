// Checksum verification for .sov archives.
//
// Reads checksums.sha256 from the archive and verifies every other file
// against its stored hash.

use crate::error::PackError;
use sha2::Digest;

/// Validate the checksums of all files in a .sov archive.
pub fn validate_archive(data: &[u8]) -> Result<(), PackError> {
    let cursor = std::io::Cursor::new(data);
    let decoder = zstd::Decoder::new(cursor).map_err(PackError::ZstdDecode)?;
    let mut archive = tar::Archive::new(decoder);

    let mut checksum_data = None;
    let mut file_hashes: Vec<(String, String)> = Vec::new();

    for entry in archive.entries().map_err(PackError::TarRead)? {
        let mut entry = entry.map_err(PackError::TarEntry)?;
        let path = entry
            .path()
            .map_err(PackError::TarEntry)?
            .to_string_lossy()
            .to_string();

        if path == "checksums.sha256" {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut buf).map_err(PackError::Io)?;
            checksum_data = Some(buf);
        } else {
            // Compute checksum of the entry data
            let mut hasher = sha2::Sha256::new();
            std::io::copy(&mut entry, &mut hasher).map_err(PackError::Io)?;
            let hash = format!("{:x}", hasher.finalize());
            file_hashes.push((path, hash));
        }
    }

    let checksum_bytes = checksum_data.ok_or(PackError::MissingChecksumFile)?;
    let expected = parse_checksums(&checksum_bytes)?;

    let mut errors = Vec::new();

    for (name, actual_hash) in &file_hashes {
        match expected.get(name) {
            Some(expected_hash) => {
                if expected_hash != actual_hash {
                    errors.push(format!(
                        "`{name}`: expected {expected_hash}, got {actual_hash}"
                    ));
                }
            }
            None => {
                errors.push(format!("`{name}`: not listed in checksums.sha256"));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        let count = errors.len();
        let error_str = errors.join("; ");
        Err(PackError::ValidationFailed {
            count,
            errors: error_str,
        })
    }
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
    use crate::archive::create_archive;
    use crate::manifest::{RuntimeKind, SovManifest};
    use crate::PackFile;
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
    fn valid_archive_passes() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = test_manifest();

        let dummy_path = dir.path().join("dummy.txt");
        let mut f = std::fs::File::create(&dummy_path).unwrap();
        f.write_all(b"test data").unwrap();
        drop(f);

        let files = vec![PackFile {
            path: dummy_path,
            name: "dummy.txt".into(),
        }];

        let output = dir.path().join("test.sov");
        create_archive(&manifest, &files, &output).unwrap();

        let data = std::fs::read(&output).unwrap();
        validate_archive(&data).unwrap();
    }

    #[test]
    fn missing_checksum_file_fails() {
        // Create an archive without checksums.sha256 manually
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("no-checksum.sov");

        let out_file = std::fs::File::create(&output).unwrap();
        let encoder = zstd::Encoder::new(out_file, 3).unwrap();
        let mut archive = tar::Builder::new(encoder);

        let manifest = test_manifest();
        let manifest_json = serde_json::to_string_pretty(&manifest).unwrap();
        let mut header = tar::Header::new_gnu();
        header.set_size(manifest_json.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive
            .append_data(&mut header, "manifest.json", manifest_json.as_bytes())
            .unwrap();

        let inner = archive.into_inner().unwrap();
        inner.finish().unwrap();

        let data = std::fs::read(&output).unwrap();
        let result = validate_archive(&data);
        assert!(matches!(result, Err(PackError::MissingChecksumFile)));
    }
}
