//! # kelson-deployment
//! Deployment model (blueprint L66, STAGE-0-SPEC §12, S3). Releases are
//! described by an explicit manifest written at build time — what an
//! operator promotes is a named artifact, never "whatever is in target/".
//! The build matrix (which binaries, which profile) is documented in
//! RELEASE.md at the workspace root and encoded here for `kelson build`.

use std::path::{Path, PathBuf};

use kelson_foundation::AppError;
use serde::{Deserialize, Serialize};

/// Workspace-relative runtime binaries produced by `kelson build`.
pub const RELEASE_BINARIES: &[&str] = &["kelson-demo", "kelson-worker", "kelson"];

/// Build profile selection (§12: dev/test/release postures).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildProfile {
    Dev,
    Release,
}

impl BuildProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            BuildProfile::Dev => "dev",
            BuildProfile::Release => "release",
        }
    }

    pub fn parse(raw: &str) -> Result<Self, AppError> {
        match raw {
            "dev" | "debug" => Ok(BuildProfile::Dev),
            "release" | "prod" => Ok(BuildProfile::Release),
            other => Err(AppError::validation(format!(
                "unknown build profile {other:?}"
            ))),
        }
    }

    pub fn cargo_flag(self) -> &'static [&'static str] {
        match self {
            BuildProfile::Dev => &[],
            BuildProfile::Release => &["--release"],
        }
    }
}

/// One built artifact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub name: String,
    pub version: String,
    pub profile: BuildProfile,
}

/// Release manifest (storage/framework/release.json). Written by `kelson build`,
/// read by operators and runtime self-reporting (`/meta/boot` style endpoints).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseManifest {
    /// App version (workspace package version).
    pub version: String,
    pub profile: BuildProfile,
    pub artifacts: Vec<Artifact>,
    /// Source control identity when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_dirty: Option<bool>,
    #[serde(with = "time::serde::rfc3339")]
    pub built_at: time::OffsetDateTime,
    pub rustc: String,
}

/// Promotion lifecycle. Transitions are one-directional except rollback:
/// Built -> Staged -> Deployed; any state -> RolledBack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromotionState {
    Built,
    Staged,
    Deployed,
    RolledBack,
}

impl PromotionState {
    pub fn advance(self) -> Result<Self, AppError> {
        match self {
            PromotionState::Built => Ok(PromotionState::Staged),
            PromotionState::Staged => Ok(PromotionState::Deployed),
            PromotionState::Deployed => Err(AppError::conflict("already deployed; rollback first")),
            PromotionState::RolledBack => Err(AppError::conflict(
                "rolled-back releases cannot advance; rebuild",
            )),
        }
    }

    pub fn rollback_from(self) -> Result<PromotionState, AppError> {
        match self {
            PromotionState::Built => Err(AppError::conflict("nothing deployed to roll back")),
            PromotionState::Staged | PromotionState::Deployed => Ok(PromotionState::RolledBack),
            PromotionState::RolledBack => Err(AppError::conflict("already rolled back")),
        }
    }
}

/// Standard manifest location under the workspace root.
pub fn manifest_path(root: &Path) -> PathBuf {
    root.join("storage/framework/release.json")
}

pub fn write_manifest(root: &Path, manifest: &ReleaseManifest) -> Result<PathBuf, AppError> {
    let path = manifest_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::internal(format!("release dir: {e}")))?;
    }
    let raw =
        serde_json::to_string_pretty(manifest).map_err(|e| AppError::internal(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, raw).map_err(|e| AppError::internal(format!("release write: {e}")))?;
    std::fs::rename(&tmp, &path).map_err(|e| AppError::internal(format!("release rename: {e}")))?;
    Ok(path)
}

pub fn read_manifest(root: &Path) -> Result<Option<ReleaseManifest>, AppError> {
    let path = manifest_path(root);
    match std::fs::read_to_string(&path) {
        Ok(raw) => {
            let manifest = serde_json::from_str(&raw)
                .map_err(|e| AppError::internal(format!("release manifest corrupt: {e}")))?;
            Ok(Some(manifest))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AppError::internal(format!(
            "release manifest unreadable: {e}"
        ))),
    }
}

/// Git identity for the manifest (best-effort; None outside a repo).
pub fn git_identity(root: &Path) -> (Option<String>, Option<bool>) {
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| !o.stdout.is_empty());
    (commit, dirty)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_manifest() -> ReleaseManifest {
        ReleaseManifest {
            version: "0.1.0".into(),
            profile: BuildProfile::Release,
            artifacts: RELEASE_BINARIES
                .iter()
                .map(|name| Artifact {
                    name: (*name).to_string(),
                    version: "0.1.0".into(),
                    profile: BuildProfile::Release,
                })
                .collect(),
            git_commit: Some("abc123".into()),
            git_dirty: Some(false),
            built_at: time::OffsetDateTime::now_utc(),
            rustc: "1.85.0".into(),
        }
    }

    #[test]
    fn profile_parse_rules() {
        assert_eq!(BuildProfile::parse("dev").unwrap(), BuildProfile::Dev);
        assert_eq!(
            BuildProfile::parse("release").unwrap(),
            BuildProfile::Release
        );
        assert!(BuildProfile::parse("yolo").is_err());
        assert_eq!(BuildProfile::Release.cargo_flag(), &["--release"]);
        assert!(BuildProfile::Dev.cargo_flag().is_empty());
    }

    #[test]
    fn promotion_state_machine() {
        let mut state = PromotionState::Built;
        state = state.advance().unwrap();
        assert_eq!(state, PromotionState::Staged);
        state = state.advance().unwrap();
        assert_eq!(state, PromotionState::Deployed);
        assert!(state.advance().is_err());
        state = state.rollback_from().unwrap();
        assert_eq!(state, PromotionState::RolledBack);
        assert!(state.rollback_from().is_err());
        assert!(state.advance().is_err());
        assert!(PromotionState::Built.rollback_from().is_err());
    }

    #[test]
    fn manifest_roundtrip() {
        let root = std::env::temp_dir().join(format!("kelson-deploy-test-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let written = write_manifest(&root, &sample_manifest()).unwrap();
        assert!(written.exists());
        let read = read_manifest(&root).unwrap().unwrap();
        assert_eq!(read.version, "0.1.0");
        assert_eq!(read.artifacts.len(), RELEASE_BINARIES.len());
        assert_eq!(read.git_commit.as_deref(), Some("abc123"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn missing_manifest_is_none_not_error() {
        let root = std::env::temp_dir().join(format!("kelson-deploy-none-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        assert!(read_manifest(&root).unwrap().is_none());
        std::fs::remove_dir_all(&root).ok();
    }
}
