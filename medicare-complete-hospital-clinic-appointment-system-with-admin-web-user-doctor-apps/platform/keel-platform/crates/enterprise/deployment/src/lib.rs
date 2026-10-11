//! # kelson-deployment
//! Deployment model (blueprint L66, STAGE-0-SPEC §12, S3). Releases are
//! described by an explicit manifest written at build time — what an
//! operator promotes is a named artifact, never "whatever is in target/".
//! The build matrix (which binaries, which profile) is documented in
//! RELEASE.md at the workspace root and encoded here for `kelson build`.

pub mod ports;
pub mod promote;
pub mod rollback;

#[cfg(test)]
mod fakes;

use std::path::{Path, PathBuf};

use kelson_foundation::AppError;
use serde::{Deserialize, Serialize};
use sha2::Digest;

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
    /// The pair migrations present in `database/migrations-pairs` when this
    /// release was built, in filename order.
    ///
    /// This is what lets `kelson upgrade` tell an operator which framework
    /// schema changes they owe after installing a newer release: the applied
    /// set lives in `keel_migrations`, the shipped set lives here, and the
    /// difference is the pending work. Manifests written before Phase S have
    /// no such field, hence the default — an old `release.json` still reads.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub migrations: Vec<ShippedMigration>,
}

/// One pair migration shipped with a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShippedMigration {
    /// Pair base filename `<ts>_<name>` — the primary key in `keel_migrations`.
    pub name: String,
    /// SHA-256 (hex) of the `.up.sql` body.
    ///
    /// `keel_migrations` records only the *name*, so a shipped migration edited
    /// after the release was built is otherwise invisible: the operator has
    /// "applied" it, but the bytes differ from what they applied. The hash
    /// makes that drift detectable.
    pub sha256: String,
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

/// Pair-migration directory, relative to the workspace root.
///
/// Mirrors `kelson_mysql::PAIRS_DIR` / `kelson_postgres::PAIRS_DIR`: those
/// crates own *running* the migrations, while this crate only enumerates them
/// so a release can record what it shipped. It deliberately depends on neither
/// driver, so the manifest survives being read where no database exists.
pub const PAIRS_DIR: &str = "database/migrations-pairs";

/// `<ts>_<name>` shape: numeric timestamp prefix, non-empty name. Same rule
/// the migration runners enforce, so a set the manifest calls shippable is one
/// `kelson migrate` would actually apply.
fn validate_pair_base(base: &str) -> Result<(), AppError> {
    let Some((ts, name)) = base.split_once('_') else {
        return Err(AppError::internal(format!(
            "shipped migration {base}: expected <ts>_<name>.up.sql (underscore-separated)"
        )));
    };
    if ts.is_empty() || !ts.bytes().all(|b| b.is_ascii_digit()) || name.is_empty() {
        return Err(AppError::internal(format!(
            "shipped migration {base}: expected a numeric <ts> prefix and a non-empty <name>"
        )));
    }
    Ok(())
}

/// Enumerate the pair migrations this tree ships, in filename order.
///
/// Follows the same discovery contract as the migration runners: every
/// `<ts>_<name>.up.sql` with a required sibling `.down.sql`, sorted by
/// filename; a missing directory means "none yet" (a project with no pair
/// migrations is normal). A malformed base name, a missing `.down.sql`, or an
/// orphan `.down.sql` is an error rather than a silently shorter manifest —
/// a manifest that quietly omits a migration would report a false all-clear
/// to an operator.
pub fn shipped_migrations(root: &Path) -> Result<Vec<ShippedMigration>, AppError> {
    let dir = root.join(PAIRS_DIR);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => {
            return Err(AppError::internal(format!(
                "shipped migrations: cannot read {}: {e}",
                dir.display()
            )))
        }
    };
    let mut up_files: Vec<PathBuf> = Vec::new();
    let mut down_bases: Vec<String> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| {
            AppError::internal(format!(
                "shipped migrations: cannot list {}: {e}",
                dir.display()
            ))
        })?;
        let path = entry.path();
        if !path.is_file() {
            continue; // subdirectories and the README are not migrations
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some(base) = name.strip_suffix(".up.sql") {
            validate_pair_base(base)?;
            up_files.push(path);
        } else if let Some(base) = name.strip_suffix(".down.sql") {
            down_bases.push(base.to_string());
        }
    }
    up_files.sort();
    let mut out = Vec::with_capacity(up_files.len());
    for up in up_files {
        let name = up.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        let base = name.strip_suffix(".up.sql").unwrap_or_default();
        if !up.with_file_name(format!("{base}.down.sql")).is_file() {
            return Err(AppError::internal(format!(
                "shipped migration {base}: missing .down.sql (a pair that cannot roll back never applies)"
            )));
        }
        let sql = std::fs::read_to_string(&up).map_err(|e| {
            AppError::internal(format!("shipped migration {base}: cannot read .up.sql: {e}"))
        })?;
        let digest = sha2::Sha256::digest(sql.as_bytes());
        out.push(ShippedMigration {
            name: base.to_string(),
            sha256: digest.iter().map(|b| format!("{b:02x}")).collect(),
        });
    }
    for base in &down_bases {
        if !out.iter().any(|m| &m.name == base) {
            return Err(AppError::internal(format!(
                "shipped migration {base}: orphan .down.sql without a matching .up.sql"
            )));
        }
    }
    Ok(out)
}

/// The difference between the migrations a release recorded at build time and
/// the ones the tree ships now.
///
/// Note what this can and cannot see: a migration *added* or *removed* since
/// the build is ordinary churn, but a migration whose **bytes changed** is the
/// dangerous case, because `keel_migrations` records only the name — the
/// database believes it has already applied that migration and will not run it
/// again. That is what [`Self::changed`] exists to surface.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MigrationDrift {
    /// Present now, absent from the manifest (added after `kelson build`).
    pub added: Vec<String>,
    /// In the manifest, gone from the tree (deleted or renamed).
    pub removed: Vec<String>,
    /// Same name, different `.up.sql` content.
    pub changed: Vec<String>,
}

impl MigrationDrift {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// Diff a release's recorded migration set against the tree's current one.
///
/// Both sides are filename-ordered; the result is ordered too, so the report
/// an operator reads is stable between runs.
pub fn migration_drift(
    released: &[ShippedMigration],
    current: &[ShippedMigration],
) -> MigrationDrift {
    let released: std::collections::BTreeMap<&str, &str> = released
        .iter()
        .map(|m| (m.name.as_str(), m.sha256.as_str()))
        .collect();
    let current: std::collections::BTreeMap<&str, &str> = current
        .iter()
        .map(|m| (m.name.as_str(), m.sha256.as_str()))
        .collect();
    let mut drift = MigrationDrift::default();
    for (name, hash) in &current {
        match released.get(name) {
            None => drift.added.push((*name).to_string()),
            Some(previous) if previous != hash => drift.changed.push((*name).to_string()),
            Some(_) => {}
        }
    }
    for name in released.keys() {
        if !current.contains_key(name) {
            drift.removed.push((*name).to_string());
        }
    }
    drift
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
            migrations: vec![ShippedMigration {
                name: "20260101000000_create_widgets".into(),
                sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                    .into(),
            }],
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

    /// Write a complete `<ts>_<name>` pair under `root`.
    fn write_pair(root: &Path, name: &str, up: &str, down: &str) {
        let dir = root.join(PAIRS_DIR);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.up.sql")), up).unwrap();
        std::fs::write(dir.join(format!("{name}.down.sql")), down).unwrap();
    }

    #[test]
    fn missing_pairs_directory_ships_nothing() {
        let root = std::env::temp_dir().join(format!("kelson-ship-none-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        assert!(shipped_migrations(&root).unwrap().is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn shipped_migrations_are_filename_ordered_and_hashed() {
        let root = std::env::temp_dir().join(format!("kelson-ship-ord-{}", uuid::Uuid::now_v7()));
        // Written out of order on purpose: discovery must sort, not trust the
        // directory listing.
        write_pair(&root, "20260202000000_second", "SELECT 2;", "SELECT 0;");
        write_pair(&root, "20260101000000_first", "", "SELECT 0;");
        let shipped = shipped_migrations(&root).unwrap();
        let names: Vec<&str> = shipped.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            names,
            ["20260101000000_first", "20260202000000_second"],
            "filename order, matching keel_migrations bookkeeping"
        );
        // The empty file pins the digest against a known SHA-256 constant.
        assert_eq!(
            shipped[0].sha256,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(shipped[1].sha256.len(), 64);
        std::fs::remove_dir_all(&root).ok();
    }

    /// The hash exists to catch the case `keel_migrations` cannot: a shipped
    /// migration edited after the release was built. The operator has applied
    /// it, so the name matches — only the bytes moved.
    #[test]
    fn a_migration_edited_after_the_release_changes_its_hash() {
        let root = std::env::temp_dir().join(format!("kelson-ship-drift-{}", uuid::Uuid::now_v7()));
        write_pair(&root, "20260101000000_widgets", "CREATE TABLE t (a int);", "DROP TABLE t;");
        let released = shipped_migrations(&root).unwrap();
        write_pair(
            &root,
            "20260101000000_widgets",
            "CREATE TABLE t (a bigint);",
            "DROP TABLE t;",
        );
        let edited = shipped_migrations(&root).unwrap();
        assert_eq!(released[0].name, edited[0].name, "the name alone would not notice");
        assert_ne!(
            released[0].sha256, edited[0].sha256,
            "the content hash does"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn incomplete_pairs_are_refused_not_skipped() {
        let root = std::env::temp_dir().join(format!("kelson-ship-bad-{}", uuid::Uuid::now_v7()));
        let dir = root.join(PAIRS_DIR);
        std::fs::create_dir_all(&dir).unwrap();

        // An up file with no down: the pair could never roll back.
        std::fs::write(dir.join("20260101000000_orphan_up.up.sql"), "SELECT 1;").unwrap();
        assert!(
            shipped_migrations(&root).is_err(),
            "a missing .down.sql must fail closed"
        );
        std::fs::remove_file(dir.join("20260101000000_orphan_up.up.sql")).unwrap();

        // A down file with no up: same, from the other side.
        std::fs::write(dir.join("20260101000000_orphan_down.down.sql"), "SELECT 0;").unwrap();
        assert!(
            shipped_migrations(&root).is_err(),
            "an orphan .down.sql must fail closed"
        );
        std::fs::remove_file(dir.join("20260101000000_orphan_down.down.sql")).unwrap();

        // A malformed base name.
        std::fs::write(dir.join("notimestamp.up.sql"), "SELECT 1;").unwrap();
        std::fs::write(dir.join("notimestamp.down.sql"), "SELECT 0;").unwrap();
        assert!(
            shipped_migrations(&root).is_err(),
            "a non <ts>_<name> base must fail closed"
        );
        std::fs::remove_dir_all(&root).ok();
    }

    /// A `release.json` written before Phase S carries no `migrations` key at
    /// all. It must still read, as an empty set — an operator mid-upgrade
    /// should not have their manifest rejected by a newer console.
    #[test]
    fn a_manifest_without_the_migrations_field_still_reads() {
        let root = std::env::temp_dir().join(format!("kelson-ship-legacy-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let mut legacy = serde_json::to_value(sample_manifest()).unwrap();
        legacy
            .as_object_mut()
            .unwrap()
            .remove("migrations")
            .expect("fixture carries the field");
        let path = manifest_path(&root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();
        let read = read_manifest(&root).unwrap().expect("legacy manifest reads");
        assert!(read.migrations.is_empty(), "defaults to no shipped migrations");
        std::fs::remove_dir_all(&root).ok();
    }

    fn ship(name: &str, sha256: &str) -> ShippedMigration {
        ShippedMigration {
            name: name.into(),
            sha256: sha256.into(),
        }
    }

    #[test]
    fn drift_is_empty_when_the_release_matches_the_tree() {
        let set = vec![ship("20260101000000_a", "aa"), ship("20260102000000_b", "bb")];
        let drift = migration_drift(&set, &set.clone());
        assert!(drift.is_empty());
    }

    #[test]
    fn drift_classifies_added_removed_and_changed_by_name() {
        let released = vec![ship("20260101000000_kept", "aa"), ship("20260102000000_gone", "bb")];
        let current = vec![ship("20260101000000_kept", "aa"), ship("20260103000000_new", "cc")];
        let drift = migration_drift(&released, &current);
        assert_eq!(drift.added, ["20260103000000_new"]);
        assert_eq!(drift.removed, ["20260102000000_gone"]);
        assert!(drift.changed.is_empty());
        assert!(!drift.is_empty());
    }

    /// The case this whole feature exists for: the name is unchanged, so
    /// `keel_migrations` reports the migration as applied and `kelson migrate`
    /// will never run it again — yet the bytes the release shipped are not the
    /// bytes in the tree. Only the hash can see it.
    #[test]
    fn an_edit_that_keeps_the_name_is_reported_as_changed_not_added() {
        let released = vec![ship("20260101000000_widgets", "aaa")];
        let current = vec![ship("20260101000000_widgets", "bbb")];
        let drift = migration_drift(&released, &current);
        assert_eq!(drift.changed, ["20260101000000_widgets"]);
        assert!(drift.added.is_empty(), "the name did not change");
        assert!(drift.removed.is_empty(), "so it is neither added nor removed");
    }

    /// The report must be stable between runs, so an operator diffing two
    /// outputs is not chasing reordering noise.
    #[test]
    fn drift_output_is_ordered_regardless_of_input_order() {
        let released = vec![ship("20260202000000_z", "z"), ship("20260101000000_a", "a")];
        let current = vec![
            ship("20260303000000_m", "m"),
            ship("20260101000000_a", "a2"),
            ship("20260202000000_z", "z"),
        ];
        let drift = migration_drift(&released, &current);
        assert_eq!(drift.added, ["20260303000000_m"]);
        assert_eq!(drift.changed, ["20260101000000_a"]);
        assert!(drift.removed.is_empty());
    }
}
