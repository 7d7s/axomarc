//! `packages.toml` — the Wave 8 B-24 package registry index (docs/19).
//!
//! v1 is local-path only (zero-dep rule: git fetch stays out): the root
//! `packages.toml` lists `[[package]] { name, version, path }` entries and
//! every indexed package ships a `module.toml` manifest (the package v1
//! contract: manifest + optional workspace crate member). Discovery in
//! `graph_routes::discover_manifests` and `keel graph`/`keel check` merges
//! those manifests into the same fail-closed GRAPH compile as
//! `applications/` and `modules/` — an entry whose manifest is missing or
//! malformed refuses the compile instead of being skipped, and the index
//! participates in the route-cache pin (`manifests_hash`).
//!
//! Relative paths resolve against the workspace root; absolute paths are
//! used as-is. `keel pkg:add` maintains the file, `keel pkg:list` reports
//! each entry's manifest + [`MODULE_ABI`](crate::MODULE_ABI) compatibility.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ModuleManifest;

/// Root index file name.
pub const PACKAGES_FILE: &str = "packages.toml";

/// One indexed package: directory (relative to the workspace root unless
/// absolute) containing the package's `module.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageEntry {
    /// Package name — mirrors `[module].name` in the shipped manifest.
    pub name: String,
    /// Package version (informational for v1; defaults when hand-edited).
    #[serde(default = "default_version")]
    pub version: String,
    /// Package directory: root-relative or absolute.
    pub path: String,
}

fn default_version() -> String {
    "0.1.0".to_string()
}

/// The whole index: `[[package]]` entries in registration order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageIndex {
    #[serde(default)]
    pub package: Vec<PackageEntry>,
}

impl PackageIndex {
    /// Where the index lives for a workspace root.
    pub fn packages_path(root: &Path) -> PathBuf {
        root.join(PACKAGES_FILE)
    }

    /// Read the index. An absent file is an empty index; a malformed or
    /// unreadable one fails closed (a typo in the index never silently
    /// drops packages from the graph).
    pub fn load(root: &Path) -> Result<Self, String> {
        let path = Self::packages_path(root);
        match std::fs::read_to_string(&path) {
            Ok(raw) => toml::from_str(&raw)
                .map_err(|e| format!("{}: package index parse failed: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    /// Resolve an entry's package directory (root-relative unless absolute).
    pub fn entry_dir(root: &Path, entry: &PackageEntry) -> PathBuf {
        let path = Path::new(&entry.path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        }
    }

    /// Read one entry's `module.toml` (the package v1 contract). Missing or
    /// malformed manifests are errors with the entry's context attached.
    pub fn entry_manifest(
        &self,
        root: &Path,
        entry: &PackageEntry,
    ) -> Result<ModuleManifest, String> {
        let path = Self::entry_dir(root, entry).join("module.toml");
        let raw = std::fs::read_to_string(&path).map_err(|e| {
            format!("package {}: {}: {e}", entry.name, path.display())
        })?;
        ModuleManifest::from_toml(&raw)
            .map_err(|v| format!("package {}: {}: {}", entry.name, path.display(), v.message))
    }

    /// Every indexed package's manifest, in index order — fail-closed
    /// (discovery feeds this straight into graph compile).
    pub fn manifests(&self, root: &Path) -> Result<Vec<ModuleManifest>, String> {
        self.package
            .iter()
            .map(|entry| self.entry_manifest(root, entry))
            .collect()
    }

    /// True when the name is already indexed (`keel pkg:add` duplicate gate).
    pub fn contains_name(&self, name: &str) -> bool {
        self.package.iter().any(|e| e.name == name)
    }

    /// True when the resolved directory is already indexed (same package
    /// added twice under two names is refused).
    pub fn contains_dir(&self, root: &Path, dir: &Path) -> bool {
        self.package
            .iter()
            .any(|e| Self::entry_dir(root, e) == dir)
    }

    /// Append an entry (registration order is the merge/display order).
    pub fn append(&mut self, entry: PackageEntry) {
        self.package.push(entry);
    }

    /// Write the index back (creates `packages.toml` with a header).
    pub fn save(&self, root: &Path) -> Result<(), String> {
        let path = Self::packages_path(root);
        let body = toml::to_string_pretty(self)
            .map_err(|e| format!("{}: package index serialize failed: {e}", path.display()))?;
        let raw = format!(
            "# Package index (Wave 8 B-24, docs/19): local-path packages merged\n\
             # into graph compile (keel graph/check/boot) and boot config\n\
             # defaults ([config] in the package manifest). Maintained by\n\
             # keel pkg:add / pkg:list.\n\
             {body}"
        );
        std::fs::write(&path, raw).map_err(|e| format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "keel-pkg-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_manifest(dir: &Path, name: &str, extra: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("module.toml"),
            format!("[module]\nname = \"{name}\"\nversion = \"0.2.0\"\n{extra}"),
        )
        .unwrap();
    }

    #[test]
    fn absent_index_is_empty_and_a_typo_fails_closed() {
        let root = temp_root("load");
        let index = PackageIndex::load(&root).expect("absent index is empty");
        assert!(index.package.is_empty());

        std::fs::write(root.join(PACKAGES_FILE), "packag = []\n").unwrap();
        let err = PackageIndex::load(&root).unwrap_err();
        assert!(err.contains("package index parse failed"), "{err}");

        std::fs::write(
            root.join(PACKAGES_FILE),
            "[[package]]\nname = \"x\"\npath = \"x\"\nbogus = 1\n",
        )
        .unwrap();
        let err = PackageIndex::load(&root).unwrap_err();
        assert!(err.contains(PACKAGES_FILE), "{err}");
    }

    #[test]
    fn append_save_reload_round_trips() {
        let root = temp_root("roundtrip");
        let mut index = PackageIndex::load(&root).unwrap();
        index.append(PackageEntry {
            name: "billing".into(),
            version: "0.3.0".into(),
            path: "packages/billing".into(),
        });
        index.save(&root).unwrap();

        let reloaded = PackageIndex::load(&root).unwrap();
        assert_eq!(reloaded, index);
        assert!(reloaded.contains_name("billing"));
        assert!(!reloaded.contains_name("other"));
        assert!(reloaded.contains_dir(&root, &root.join("packages/billing")));
        assert!(!reloaded.contains_dir(&root, &root.join("packages/other")));
    }

    #[test]
    fn manifests_merge_fail_closed_with_entry_context() {
        let root = temp_root("manifests");
        write_manifest(&root.join("packages/billing"), "billing", "");

        let mut index = PackageIndex::load(&root).unwrap();
        index.append(PackageEntry {
            name: "billing".into(),
            version: "0.2.0".into(),
            path: "packages/billing".into(),
        });
        let manifests = index.manifests(&root).expect("indexed manifest loads");
        assert_eq!(manifests.len(), 1);
        assert_eq!(manifests[0].name(), "billing");

        // Missing manifest: refused, naming the package.
        index.append(PackageEntry {
            name: "ghost".into(),
            version: "0.1.0".into(),
            path: "packages/ghost".into(),
        });
        let err = index.manifests(&root).unwrap_err();
        assert!(err.contains("package ghost"), "{err}");

        // Malformed manifest: refused too (never skipped silently).
        index.package.pop();
        write_manifest(&root.join("packages/broken"), "broken", "this is not toml ==");
        index.append(PackageEntry {
            name: "broken".into(),
            version: "0.1.0".into(),
            path: "packages/broken".into(),
        });
        let err = index.manifests(&root).unwrap_err();
        assert!(err.contains("package broken"), "{err}");
    }

    #[test]
    fn entry_dir_resolves_relative_and_absolute_paths() {
        let root = temp_root("paths");
        let relative = PackageEntry {
            name: "r".into(),
            version: "1".into(),
            path: "packages/r".into(),
        };
        assert_eq!(
            PackageIndex::entry_dir(&root, &relative),
            root.join("packages/r")
        );
        let absolute = PackageEntry {
            name: "a".into(),
            version: "1".into(),
            path: root.join("elsewhere").display().to_string(),
        };
        assert_eq!(
            PackageIndex::entry_dir(&root, &absolute),
            root.join("elsewhere")
        );
    }
}
