//! `packages.toml` — the Wave 8 B-24 package registry index (docs/19).
//!
//! v1 is local-path only (zero-dep rule: git fetch stays out): the root
//! `packages.toml` lists `[[package]] { name, version, path }` entries and
//! every indexed package ships a `module.toml` manifest (the package v1
//! contract: manifest + optional workspace crate member). Discovery in
//! `graph_routes::discover_manifests` and `kelson graph`/`kelson check` merges
//! those manifests into the same fail-closed GRAPH compile as
//! `applications/` and `modules/` — an entry whose manifest is missing or
//! malformed refuses the compile instead of being skipped, and the index
//! participates in the route-cache pin (`manifests_hash`).
//!
//! Relative paths resolve against the workspace root; absolute paths are
//! used as-is. `kelson pkg:add` maintains the file, `kelson pkg:list` reports
//! each entry's manifest + [`MODULE_ABI`](crate::MODULE_ABI) compatibility.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::ModuleManifest;

/// Root index file name.
pub const PACKAGES_FILE: &str = "packages.toml";

/// Where the WASM module version pins live (`storage/framework/module-pins.json`,
/// §7.1 step 3). A flat JSON object mapping module name -> pinned version.
///
/// This constant is canonical here, in the registry, because the rule that
/// consumes it is a boot rule: `PackageIndex::verify_pins_against_index`
/// refuses to load plugins whose installed version disagrees with the pin.
/// The console's promote/rollback tooling imports it rather than restating
/// the path, so there is one string to change and one place that defines
/// what the file means.
pub const MODULE_PINS_PATH: &str = "storage/framework/module-pins.json";

/// The pin file's decoded shape: module name -> pinned version. Named so
/// call sites read as intent (`pins: ModulePinMap`) rather than as an
/// incidental `BTreeMap`.
pub type ModulePinMap = BTreeMap<String, String>;

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

/// One indexed WASM plugin (Wave 9 B-30, docs/19): the compiled binary plus
/// the reviewed sidecar manifest. The sidecar is the same shape as a
/// `module.toml`, and at boot the guest's own `kelson_manifest()` JSON must
/// normalize to the IDENTICAL document (drift refuses the load) — so the
/// index entry and the reviewed TOML are the two halves of the trust
/// story: what was reviewed is what runs. Paths resolve like package
/// paths (root-relative unless absolute).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WasmModuleEntry {
    /// Plugin name — must equal `[module].name` in the sidecar.
    pub name: String,
    /// Plugin version — must equal `[module].version` in the sidecar.
    pub version: String,
    /// Compiled binary path (`.wasm`, or `.wat` for checked-in fixtures).
    pub path: String,
    /// Sidecar manifest path (the `module.toml`-equivalent TOML).
    pub manifest: String,
}

/// The whole index: `[[package]]` entries in registration order, plus
/// `[[wasm]]` plugin entries (Wave 9 B-30, additive).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageIndex {
    #[serde(default)]
    pub package: Vec<PackageEntry>,
    /// Indexed WASM plugins. `skip_serializing_if` keeps a pure package
    /// index byte-identical to its pre-B-30 output (no bare `wasm = []`
    /// key can land inside the last `[[package]]` table and trip
    /// `deny_unknown_fields` on reload).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub wasm: Vec<WasmModuleEntry>,
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

    /// Every indexed manifest, in index order — packages first, then WASM
    /// plugin sidecars — fail-closed (discovery feeds this straight into
    /// graph compile; both discovery copies — kelson-demo and kelson-console —
    /// call it, so WASM plugins reach `kelson graph`/`kelson check`/boot with
    /// zero per-caller wiring).
    pub fn manifests(&self, root: &Path) -> Result<Vec<ModuleManifest>, String> {
        let mut out: Vec<ModuleManifest> = self
            .package
            .iter()
            .map(|entry| self.entry_manifest(root, entry))
            .collect::<Result<_, _>>()?;
        out.extend(self.wasm_manifests(root)?);
        Ok(out)
    }

    /// Resolve an index path (root-relative unless absolute).
    pub fn resolve_path(root: &Path, path: &str) -> PathBuf {
        let path = Path::new(path);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        }
    }

    /// One WASM plugin's sidecar manifest (Wave 9 B-30). The shared
    /// discovery rule — run by graph discovery (console + boot) and mirrored
    /// by the boot-time loader, so every caller sees the same contract:
    /// * the sidecar must exist and parse (`GRAPH-MANIFEST` on garbage),
    /// * `[module].name` / `[module].version` must match the index entry
    ///   (a stale entry never silently describes a different plugin),
    /// * the binary must exist,
    /// * at least one capability must be declared (nothing to register =
    ///   refuse instead of dead-weighting boot),
    /// * every sidecar profile loads (Phase 3 §5.3): the op profile maps
    ///   to a guest capability profile at handler build
    ///   (`WasmCapabilityProfile::from_op_profile`), and the write
    ///   imports enforce it per call — discovery no longer refuses
    ///   write profiles.
    pub fn wasm_manifest(
        &self,
        root: &Path,
        entry: &WasmModuleEntry,
    ) -> Result<ModuleManifest, String> {
        let manifest_path = Self::resolve_path(root, &entry.manifest);
        let binary_path = Self::resolve_path(root, &entry.path);
        let raw = std::fs::read_to_string(&manifest_path).map_err(|e| {
            format!("wasm plugin {}: {}: {e}", entry.name, manifest_path.display())
        })?;
        let manifest = ModuleManifest::from_toml(&raw).map_err(|v| {
            format!(
                "wasm plugin {}: {}: {}",
                entry.name,
                manifest_path.display(),
                v.message
            )
        })?;
        if manifest.name() != entry.name {
            return Err(format!(
                "wasm plugin {}: sidecar {} declares module name {:?}, index says {:?}",
                entry.name,
                manifest_path.display(),
                manifest.name(),
                entry.name
            ));
        }
        if manifest.module.version != entry.version {
            return Err(format!(
                "wasm plugin {}: sidecar {} declares version {:?}, index says {:?}",
                entry.name,
                manifest_path.display(),
                manifest.module.version,
                entry.version
            ));
        }
        if !binary_path.exists() {
            return Err(format!(
                "wasm plugin {}: binary {} does not exist",
                entry.name,
                binary_path.display()
            ));
        }
        if manifest.capabilities.is_empty() {
            return Err(format!(
                "wasm plugin {}: sidecar {} declares no capabilities",
                entry.name,
                manifest_path.display()
            ));
        }
        // Phase 3 §5.3 (KEEL-007): every sidecar profile loads — the op
        // profile maps to a guest capability profile at handler build
        // (`WasmCapabilityProfile::from_op_profile`: Read/Report → readonly
        // imports, Command-family → +capability_write, System → full set).
        // The guest can never exceed its sidecar: declared actions gate
        // every import, and the profile gates the write imports per call.
        Ok(manifest)
    }

    /// Every indexed plugin's sidecar manifest — fail-closed.
    pub fn wasm_manifests(&self, root: &Path) -> Result<Vec<ModuleManifest>, String> {
        self.wasm
            .iter()
            .map(|entry| self.wasm_manifest(root, entry))
            .collect()
    }

    /// The WASM module version pins (§7.1 step 3), as name -> pinned version.
    ///
    /// An absent pin file is NOT an error and yields an empty map: pinning is
    /// opt-in, and a workspace that has never run `kelson promote` boots
    /// exactly as it did before pins existed. A pin file that exists but is
    /// unreadable or is not a JSON object of string -> string IS an error —
    /// an operator who wrote pins and mistyped one must not have them
    /// silently ignored (fail-closed, not fail-open).
    ///
    /// The format is the one `ModulePinAdapter` writes: a flat JSON object.
    pub fn module_pins(root: &Path) -> Result<ModulePinMap, String> {
        let path = root.join(MODULE_PINS_PATH);
        if !path.exists() {
            return Ok(BTreeMap::new());
        }
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| format!("module pins {}: {e}", path.display()))?;
        serde_json::from_str(&raw)
            .map_err(|e| format!("module pins {}: invalid JSON: {e}", path.display()))
    }

    /// Enforce the pins against what is installed — fail-closed.
    ///
    /// Every pinned module must be installed, and the installed version must
    /// equal the pinned version. This is the check that makes a pin mean
    /// something: without it the pin file is write-only, and a rollback
    /// restores a version field that nothing consults.
    ///
    /// Rules:
    /// * absent pin file -> nothing to enforce, `Ok` (see [`Self::module_pins`]);
    /// * a pinned module that is not indexed as a WASM plugin -> refuse;
    /// * a pinned module whose installed version differs -> refuse.
    ///
    /// Pins name WASM plugins specifically. A name that is installed as a
    /// `[[package]]` entry but not a `[[wasm]]` entry is refused, because the
    /// pin file is written from the WASM index and a pin naming anything else
    /// is a misconfiguration that would otherwise never be noticed.
    ///
    /// Returns the number of pins verified, so a caller can report
    /// "N pins verified" rather than a bare success.
    pub fn verify_pins_against_index(&self, pins: &ModulePinMap) -> Result<usize, String> {
        let installed: BTreeMap<&str, &str> = self
            .wasm
            .iter()
            .map(|w| (w.name.as_str(), w.version.as_str()))
            .collect();
        for (name, version) in pins {
            match installed.get(name.as_str()) {
                None => {
                    return Err(format!(
                        "module pin `{name}` -> {version}: no WASM plugin named `{name}` is installed \
                         (index lists {})",
                        installed.len()
                    ))
                }
                Some(&got) if got != version => {
                    return Err(format!(
                        "module pin `{name}` -> {version}: installed version is {got} \
                         (the pin and the installed bytes disagree)"
                    ))
                }
                Some(_) => {}
            }
        }
        Ok(pins.len())
    }

    /// Read the pins and enforce them in one step — the call boot and the
    /// console make. `Ok(0)` means no pins are declared.
    pub fn verify_pinned_boot(&self, root: &Path) -> Result<usize, String> {
        let pins = Self::module_pins(root)?;
        self.verify_pins_against_index(&pins)
    }

    /// True when the name is already indexed (`kelson pkg:add` duplicate
    /// gate; WASM plugins occupy the same module-name namespace).
    pub fn contains_name(&self, name: &str) -> bool {
        self.package.iter().any(|e| e.name == name) || self.wasm.iter().any(|e| e.name == name)
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
             # into graph compile (kelson graph/check/boot) and boot config\n\
             # defaults ([config] in the package manifest). Maintained by\n\
             # kelson pkg:add / pkg:list.\n\
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
            "kelson-pkg-{tag}-{}-{}",
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

    // ------------------------------------------------ Wave 9 B-30 (wasm)

    /// Write a plugin binary + sidecar under `plugins/<name>/` and return
    /// its index entry.
    fn write_plugin(
        root: &Path,
        name: &str,
        version: &str,
        capability: &str,
        profile: &str,
    ) -> WasmModuleEntry {
        let dir = root.join("plugins").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{name}.wasm")), b"\0asmfixture").unwrap();
        std::fs::write(
            dir.join("module.toml"),
            format!(
                "[module]\nname = \"{name}\"\nversion = \"{version}\"\n\n\
                 [capabilities.{capability}]\nprofile = \"{profile}\"\n"
            ),
        )
        .unwrap();
        WasmModuleEntry {
            name: name.into(),
            version: version.into(),
            path: format!("plugins/{name}/{name}.wasm"),
            manifest: format!("plugins/{name}/module.toml"),
        }
    }

    #[test]
    fn wasm_sidecars_discover_with_full_context_refusals() {
        let root = temp_root("wasm");
        let entry = write_plugin(&root, "echo", "1.2.0", "echo", "READ");

        let mut index = PackageIndex::load(&root).unwrap();
        index.wasm.push(entry.clone());
        let manifests = index.manifests(&root).expect("plugin sidecar discovers");
        assert_eq!(manifests.len(), 1, "sidecar joins the manifest merge");
        assert_eq!(manifests[0].name(), "echo");
        assert_eq!(manifests[0].module.version, "1.2.0");
        assert!(manifests[0].capabilities.contains_key("echo"));
        assert!(index.contains_name("echo"), "plugin names are indexed too");

        // Index/sidecar name disagreement: refused, naming the plugin.
        let mut bad = entry.clone();
        bad.name = "renamed".into();
        index.wasm[0] = bad;
        let err = index.manifests(&root).unwrap_err();
        assert!(err.contains("wasm plugin renamed"), "{err}");

        // Version disagreement: refused (a stale entry never silently
        // describes a different plugin).
        let mut bad = entry.clone();
        bad.version = "9.9.9".into();
        index.wasm[0] = bad;
        let err = index.manifests(&root).unwrap_err();
        assert!(err.contains("9.9.9"), "{err}");

        // Missing binary: refused.
        let mut bad = entry.clone();
        bad.path = "plugins/echo/missing.wasm".into();
        index.wasm[0] = bad;
        let err = index.manifests(&root).unwrap_err();
        assert!(err.contains("does not exist"), "{err}");

        // Missing sidecar: refused with plugin context.
        let mut bad = entry.clone();
        bad.manifest = "plugins/echo/ghost.toml".into();
        index.wasm[0] = bad;
        let err = index.manifests(&root).unwrap_err();
        assert!(err.contains("wasm plugin echo"), "{err}");

        // A non-READ capability: DISCOVERED under Phase 3 §5.3 (the op
        // profile maps to a guest capability profile at handler build;
        // discovery no longer refuses write profiles).
        index.wasm[0] = write_plugin(&root, "mutate", "0.1.0", "write_all", "COMMAND");
        let manifests = index.manifests(&root).expect("command sidecar discovers");
        assert_eq!(manifests.len(), 1);
        assert_eq!(
            manifests[0].capabilities["write_all"].profile,
            kelson_foundation::ExecutionProfileKind::Command
        );

        // A plugin declaring nothing: refused (nothing to register).
        let dir = root.join("plugins/idle");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("idle.wasm"), b"\0asmfixture").unwrap();
        std::fs::write(
            dir.join("module.toml"),
            "[module]\nname = \"idle\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        index.wasm[0] = WasmModuleEntry {
            name: "idle".into(),
            version: "0.1.0".into(),
            path: "plugins/idle/idle.wasm".into(),
            manifest: "plugins/idle/module.toml".into(),
        };
        let err = index.manifests(&root).unwrap_err();
        assert!(err.contains("no capabilities"), "{err}");
    }

    #[test]
    fn save_omits_the_wasm_key_when_empty_and_round_trips_plugins() {
        let root = temp_root("wasm-save");
        write_manifest(&root.join("packages/billing"), "billing", "");
        let mut index = PackageIndex::load(&root).unwrap();
        index.append(PackageEntry {
            name: "billing".into(),
            version: "0.1.0".into(),
            path: "packages/billing".into(),
        });
        index.save(&root).unwrap();
        let raw = std::fs::read_to_string(PackageIndex::packages_path(&root)).unwrap();
        assert!(
            !raw.contains("[[wasm]]") && !raw.contains("\nwasm"),
            "an empty wasm list emits no key at all:\n{raw}"
        );
        let reloaded = PackageIndex::load(&root).unwrap();
        assert_eq!(reloaded, index);

        // With a plugin: [[wasm]] survives save -> load and the sidecars
        // still discover from the reloaded index.
        index
            .wasm
            .push(write_plugin(&root, "echo", "1.0.0", "echo", "READ"));
        index.save(&root).unwrap();
        let raw = std::fs::read_to_string(PackageIndex::packages_path(&root)).unwrap();
        assert!(raw.contains("[[wasm]]"), "{raw}");
        let reloaded = PackageIndex::load(&root).unwrap();
        assert_eq!(reloaded, index);
        assert!(reloaded.manifests(&root).is_ok());
    }

    // ------------------------------------------------------ Wave M1: pins

    fn write_pins(root: &Path, json: &str) {
        let dir = root.join("storage").join("framework");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("module-pins.json"), json).unwrap();
    }

    #[test]
    fn absent_pin_file_yields_no_pins_rather_than_refusing_boot() {
        let root = temp_root("pins-absent");
        let mut index = PackageIndex::default();
        index
            .wasm
            .push(write_plugin(&root, "echo", "1.0.0", "echo", "READ"));
        // No pin file at all: pinning is opt-in, so this is Ok(0) and NOT an
        // error. Every workspace that has never run `kelson promote` keeps
        // booting exactly as before.
        assert_eq!(PackageIndex::module_pins(&root).unwrap().len(), 0);
        assert_eq!(index.verify_pinned_boot(&root).unwrap(), 0);
    }

    #[test]
    fn a_pin_matching_the_index_verifies() {
        let root = temp_root("pins-match");
        let mut index = PackageIndex::default();
        index
            .wasm
            .push(write_plugin(&root, "echo", "1.0.0", "echo", "READ"));
        write_pins(&root, r#"{"echo": "1.0.0"}"#);
        assert_eq!(index.verify_pinned_boot(&root).unwrap(), 1);
    }

    #[test]
    fn a_pin_naming_a_different_installed_version_refuses() {
        let root = temp_root("pins-drift");
        let mut index = PackageIndex::default();
        index
            .wasm
            .push(write_plugin(&root, "echo", "0.4.0", "echo", "READ"));
        // The exact defect this gate exists for: the pin says 0.3.1, the
        // installed bytes are 0.4.0, and before Wave M1 nothing read the pin
        // so 0.4.0 simply ran.
        write_pins(&root, r#"{"echo": "0.3.1"}"#);
        let err = index.verify_pinned_boot(&root).unwrap_err();
        assert!(err.contains("0.3.1"), "{err}");
        assert!(err.contains("0.4.0"), "{err}");
    }

    #[test]
    fn a_pin_naming_an_uninstalled_module_refuses() {
        let root = temp_root("pins-unknown");
        let mut index = PackageIndex::default();
        index
            .wasm
            .push(write_plugin(&root, "echo", "1.0.0", "echo", "READ"));
        write_pins(&root, r#"{"ghost": "9.9.9"}"#);
        let err = index.verify_pinned_boot(&root).unwrap_err();
        assert!(err.contains("ghost"), "{err}");
    }

    #[test]
    fn a_malformed_pin_file_refuses_rather_than_being_ignored() {
        let root = temp_root("pins-malformed");
        let mut index = PackageIndex::default();
        index
            .wasm
            .push(write_plugin(&root, "echo", "1.0.0", "echo", "READ"));
        // Fail-closed: an operator who wrote pins and mistyped one must not
        // have them silently skipped.
        write_pins(&root, "{not json");
        let err = index.verify_pinned_boot(&root).unwrap_err();
        assert!(err.contains("module pins"), "{err}");
    }

    #[test]
    fn a_pin_naming_a_package_entry_instead_of_a_plugin_refuses() {
        let root = temp_root("pins-package");
        let mut index = PackageIndex::default();
        index.append(PackageEntry {
            name: "billing".into(),
            version: "0.2.0".into(),
            path: "packages/billing".into(),
        });
        // Installed as a [[package]], never as a [[wasm]]: pins are written
        // from the WASM index, so a pin naming anything else is a
        // misconfiguration that would otherwise go unnoticed forever.
        write_pins(&root, r#"{"billing": "0.2.0"}"#);
        let err = index.verify_pinned_boot(&root).unwrap_err();
        assert!(err.contains("billing"), "{err}");
        assert!(err.contains("WASM plugin"), "{err}");
    }

    #[test]
    fn pins_verify_count_is_reported_so_a_caller_can_say_how_many() {
        let root = temp_root("pins-count");
        let mut index = PackageIndex::default();
        index
            .wasm
            .push(write_plugin(&root, "echo", "1.0.0", "echo", "READ"));
        index
            .wasm
            .push(write_plugin(&root, "fib", "2.0.0", "fib", "READ"));
        write_pins(&root, r#"{"echo": "1.0.0", "fib": "2.0.0"}"#);
        assert_eq!(index.verify_pinned_boot(&root).unwrap(), 2);
    }
}
