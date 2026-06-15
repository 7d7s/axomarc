// SovManifest — the metadata inside every .sov archive.
//
// The manifest is always stored as the first file in the archive.
// It describes the app, runtime mode, and deployment parameters.

use serde::{Deserialize, Serialize};

/// The runtime mode for a deployed app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeKind {
    /// App runs inside a Docker/OCI container.
    Container,
    /// App runs as a native binary via systemd.
    Native,
}

impl std::fmt::Display for RuntimeKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Container => write!(f, "container"),
            Self::Native => write!(f, "native"),
        }
    }
}

impl std::str::FromStr for RuntimeKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "container" => Ok(Self::Container),
            "native" => Ok(Self::Native),
            other => Err(format!(
                "invalid runtime `{other}` (expected `container` or `native`)"
            )),
        }
    }
}

/// The manifest stored inside every `.sov` archive.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SovManifest {
    /// App name. DNS-label safe.
    pub app: String,
    /// Semantic version string.
    pub version: String,
    /// Runtime mode (container or native).
    pub runtime: RuntimeKind,
    /// Docker/OCI image reference. Required if runtime=container.
    pub image_ref: Option<String>,
    /// Path to binary inside archive. Required if runtime=native.
    pub binary_path: Option<String>,
    /// systemd ExecStart template. Required if runtime=native.
    /// Supports `{port}` substitution.
    pub systemd_exec: Option<String>,
    /// HTTP health check path (e.g. `/health`).
    pub health_path: Option<String>,
    /// Port the app listens on.
    pub port: u16,
    /// Environment variables.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    /// Archive creation timestamp (unix seconds).
    pub created_at: i64,
    /// Who built this archive (e.g. "ci", "local").
    pub built_by: String,
}

impl SovManifest {
    /// Validate the manifest fields. Returns a list of errors (empty = valid).
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();

        if self.app.is_empty() {
            errors.push("`app` is required and must be non-empty".into());
        } else if !is_dns_label_safe(&self.app) {
            errors.push(format!("`app` must be DNS-label safe; got `{}`", self.app));
        }

        if self.version.is_empty() {
            errors.push("`version` is required and must be non-empty".into());
        }

        match self.runtime {
            RuntimeKind::Container => {
                if self.image_ref.is_none() {
                    errors.push("`image_ref` is required when runtime=container".into());
                }
            }
            RuntimeKind::Native => {
                if self.binary_path.is_none() {
                    errors.push("`binary_path` is required when runtime=native".into());
                }
                if self.systemd_exec.is_none() {
                    errors.push("`systemd_exec` is required when runtime=native".into());
                }
            }
        }

        if self.port == 0 {
            errors.push("`port` must be in 1-65535; got 0".into());
        }

        errors
    }
}

fn is_dns_label_safe(s: &str) -> bool {
    if s.is_empty() || s.len() > 63 {
        return false;
    }
    if s.starts_with('-') || s.ends_with('-') {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Read the manifest from a raw .sov archive (tar.zst).
pub fn read_manifest_from_archive(data: &[u8]) -> Result<SovManifest, crate::PackError> {
    let cursor = std::io::Cursor::new(data);
    let decoder = zstd::Decoder::new(cursor).map_err(crate::PackError::ZstdDecode)?;
    let mut archive = tar::Archive::new(decoder);

    for entry in archive.entries().map_err(crate::PackError::TarRead)? {
        let mut entry = entry.map_err(crate::PackError::TarEntry)?;
        let path = entry.path().map_err(crate::PackError::TarEntry)?;
        if path.to_string_lossy() == "manifest.json" {
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut buf).map_err(crate::PackError::Io)?;
            let manifest: SovManifest =
                serde_json::from_slice(&buf).map_err(crate::PackError::JsonParse)?;
            return Ok(manifest);
        }
    }

    Err(crate::PackError::MissingManifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_manifest() -> SovManifest {
        SovManifest {
            app: "my-app".into(),
            version: "1.0.0".into(),
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
    fn valid_manifest_passes_validation() {
        let m = valid_manifest();
        assert!(m.validate().is_empty());
    }

    #[test]
    fn missing_app_name_fails() {
        let mut m = valid_manifest();
        m.app.clear();
        assert!(m.validate().iter().any(|e| e.contains("`app`")));
    }

    #[test]
    fn invalid_app_name_fails() {
        let mut m = valid_manifest();
        m.app = "MyApp".into();
        assert!(m.validate().iter().any(|e| e.contains("DNS-label safe")));
    }

    #[test]
    fn container_needs_image_ref() {
        let mut m = valid_manifest();
        m.image_ref = None;
        assert!(m.validate().iter().any(|e| e.contains("image_ref")));
    }

    #[test]
    fn native_needs_binary_and_exec() {
        let m = SovManifest {
            runtime: RuntimeKind::Native,
            binary_path: None,
            systemd_exec: None,
            ..valid_manifest()
        };
        let errors = m.validate();
        assert!(errors.iter().any(|e| e.contains("binary_path")));
        assert!(errors.iter().any(|e| e.contains("systemd_exec")));
    }

    #[test]
    fn native_with_fields_passes() {
        let m = SovManifest {
            runtime: RuntimeKind::Native,
            image_ref: None,
            binary_path: Some("bin/myapp".into()),
            systemd_exec: Some("/opt/sovereign/apps/myapp/bin/myapp --port {port}".into()),
            ..valid_manifest()
        };
        assert!(m.validate().is_empty());
    }

    #[test]
    fn zero_port_fails() {
        let mut m = valid_manifest();
        m.port = 0;
        assert!(m.validate().iter().any(|e| e.contains("port")));
    }

    #[test]
    fn serde_roundtrip() {
        let m = valid_manifest();
        let json = serde_json::to_string(&m).unwrap();
        let back: SovManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }

    #[test]
    fn runtime_kind_display() {
        assert_eq!(RuntimeKind::Container.to_string(), "container");
        assert_eq!(RuntimeKind::Native.to_string(), "native");
    }

    #[test]
    fn runtime_kind_from_str() {
        assert_eq!(
            "container".parse::<RuntimeKind>().unwrap(),
            RuntimeKind::Container
        );
        assert_eq!(
            "native".parse::<RuntimeKind>().unwrap(),
            RuntimeKind::Native
        );
        assert!("invalid".parse::<RuntimeKind>().is_err());
    }
}
