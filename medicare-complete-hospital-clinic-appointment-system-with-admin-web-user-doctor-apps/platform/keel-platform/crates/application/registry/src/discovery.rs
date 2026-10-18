//! One discovery path for every caller: `kelson graph`/`check`/`route:cache`/
//! `optimize` (the console), `kelson-demo` boot, and the `tests/architecture`
//! gate.
//!
//! History worth keeping: the collapse rule below is the fix from 26a653a
//! ("Stop discovery from declaring an indexed module twice"). `packages.toml`
//! indexes a package by pointing at the `module.toml` the layout walk already
//! read (`manifest = "applications/appointments/module.toml"`), so the same
//! file arrived twice and the graph's (correct) duplicate gate refused with
//! `GRAPH-DUP-MODULE: module appointments declared more than once` plus a dup
//! line for every operation, event and route. That fix lived only in the
//! demo's private copy, so the console — which kept its own, never-updated
//! copy — went on refusing `kelson graph`/`route:cache`/`optimize` on the
//! shipped workspace (P0 finding F3). This module is now the single
//! implementation; the console and demo delegate to it.

use std::path::Path;

use crate::{ModuleManifest, PackageIndex, RouteFileManifest};

/// Discover module manifests under `applications/` and `modules/` (the layout
/// `kelson graph` documents), plus every package indexed by the root
/// `packages.toml` (Wave 8 B-24), plus the Wave 2 root route files
/// (`routes/*.toml` as synthetic modules), sorted by module name.
///
/// An entry whose manifest is missing or malformed refuses discovery
/// (fail-closed) — packages never silently disappear from the graph.
pub fn discover_manifests(root: &Path) -> Result<Vec<ModuleManifest>, String> {
    let mut out = Vec::new();
    for base in ["applications", "modules"] {
        let dir = root.join(base);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let manifest_path = entry.path().join("module.toml");
            if manifest_path.exists() {
                let raw = std::fs::read_to_string(&manifest_path)
                    .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
                let manifest = ModuleManifest::from_toml(&raw)
                    .map_err(|v| format!("{}: {}", manifest_path.display(), v.message))?;
                out.push(manifest);
            }
        }
    }
    // An indexed package normally LIVES in the layout walked above:
    // `packages.toml` records `manifest = "applications/appointments/module.toml"`
    // and the `applications/` loop just read that same file. Two reads of one
    // file are one declaration, and handing both to the graph made its
    // duplicate gate fire on the discovery order rather than on a second
    // module — see the module docs for the 26a653a history.
    //
    // Only byte-identical declarations collapse. A name the index resolves to
    // *different* content is a real second declaration, so both survive and
    // the gate still reports it — deduplication must not become a way to hide
    // drift.
    for manifest in PackageIndex::load(root)?.manifests(root)? {
        let canon = serde_json::to_string(&manifest).ok();
        let already_declared = out.iter().any(|seen| {
            seen.name() == manifest.name()
                && match (&canon, serde_json::to_string(seen).ok()) {
                    (Some(a), Some(b)) => a.as_str() == b.as_str(),
                    _ => false,
                }
        });
        if !already_declared {
            out.push(manifest);
        }
    }
    out.extend(discover_route_files(root)?);
    out.sort_by(|a, b| a.name().cmp(b.name()));
    Ok(out)
}

/// Wave 2 root route files (mirroring Laravel's `routes/*.php`): merge
/// `routes/web.toml` + `routes/api.toml` routes (plane forced by file),
/// `routes/console.toml` commands and `routes/channels.toml` channels into
/// synthetic module manifests. Absent files are fine. Route-file operations
/// must reference module-registered operations (GRAPH-ROUTEFILE-OP gate);
/// channel capabilities must be declared somewhere (GRAPH-CHANNEL-CAP).
pub fn discover_route_files(root: &Path) -> Result<Vec<ModuleManifest>, String> {
    let mut out = Vec::new();
    for (file, plane, module_name) in [
        ("web.toml", Some("web"), "routes_web"),
        ("api.toml", Some("api"), "routes_api"),
        ("console.toml", None, "routes_console"),
        ("channels.toml", None, "routes_channels"),
    ] {
        let path = root.join("routes").join(file);
        if !path.exists() {
            continue;
        }
        let raw =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let manifest = RouteFileManifest::from_toml(&raw, &format!("routes/{file}"), plane)
            .map_err(|v| format!("{}: {}", path.display(), v.message))?;
        out.push(manifest.into_module(module_name));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique temp root per test (Windows-safe: no forward-slash asserts).
    fn fresh_temp(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kelson-registry-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    fn app_module(root: &Path, name: &str, version: &str) -> std::path::PathBuf {
        let dir = root.join("applications").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("module.toml"),
            format!("[module]\nname = \"{name}\"\nversion = \"{version}\"\napi = 1\n"),
        )
        .unwrap();
        dir
    }

    /// P0 finding F3: the `kelson pkg:add applications/alpha` shape — the
    /// index points at the file the layout walk just read — is ONE
    /// declaration, and the graph gate must accept it.
    #[test]
    fn an_indexed_module_the_layout_already_walked_is_one_declaration() {
        let root = fresh_temp("indexed-overlap");
        app_module(&root, "alpha", "0.1.0");
        std::fs::write(
            root.join("packages.toml"),
            "[[package]]\nname = \"alpha\"\nversion = \"0.1.0\"\npath = \"applications/alpha\"\n",
        )
        .unwrap();

        let manifests = discover_manifests(&root).expect("discovers");
        let names: Vec<&str> = manifests.iter().map(|m| m.name()).collect();
        assert_eq!(names, vec!["alpha"], "one file read twice is one declaration");
        crate::PlatformGraph::compile(manifests).expect("one declaration compiles");

        std::fs::remove_dir_all(&root).ok();
    }

    /// The collapse must not hide drift: the index resolving the same name to
    /// different content is a real second declaration and still refuses.
    #[test]
    fn a_name_the_index_resolves_to_different_content_stays_a_duplicate() {
        let root = fresh_temp("indexed-drift");
        app_module(&root, "alpha", "0.1.0");
        let drifted = root.join("packages").join("alpha");
        std::fs::create_dir_all(&drifted).unwrap();
        std::fs::write(
            drifted.join("module.toml"),
            "[module]\nname = \"alpha\"\nversion = \"9.9.9\"\napi = 1\n",
        )
        .unwrap();
        std::fs::write(
            root.join("packages.toml"),
            "[[package]]\nname = \"alpha\"\nversion = \"9.9.9\"\npath = \"packages/alpha\"\n",
        )
        .unwrap();

        let manifests = discover_manifests(&root).expect("discovers");
        assert_eq!(
            manifests.iter().filter(|m| m.name() == "alpha").count(),
            2,
            "same name from different content stays two declarations"
        );
        let violations =
            crate::PlatformGraph::compile(manifests).expect_err("the gate reports the drift");
        let codes: Vec<&str> = violations.iter().map(|v| v.code.as_str()).collect();
        assert!(codes.contains(&"GRAPH-DUP-MODULE"), "{codes:?}");

        std::fs::remove_dir_all(&root).ok();
    }

    /// Packages never silently vanish: a missing indexed manifest refuses
    /// discovery with the entry's context attached.
    #[test]
    fn a_missing_indexed_manifest_refuses_with_the_entry_context() {
        let root = fresh_temp("indexed-missing");
        std::fs::write(
            root.join("packages.toml"),
            "[[package]]\nname = \"billing\"\nversion = \"0.1.0\"\npath = \"packages/billing\"\n",
        )
        .unwrap();

        let err = discover_manifests(&root).expect_err("missing manifest refuses");
        assert!(err.contains("package billing"), "{err}");

        std::fs::remove_dir_all(&root).ok();
    }

    /// Root route files merge as synthetic modules (`routes_console` here).
    #[test]
    fn root_route_files_merge_as_synthetic_modules() {
        let root = fresh_temp("route-files");
        std::fs::create_dir_all(root.join("routes")).unwrap();
        std::fs::write(
            root.join("routes").join("console.toml"),
            "[[command]]\nname = \"book\"\noperation = \"alpha.book\"\n",
        )
        .unwrap();

        let manifests = discover_manifests(&root).expect("discovers");
        let names: Vec<&str> = manifests.iter().map(|m| m.name()).collect();
        assert_eq!(names, vec!["routes_console"]);

        std::fs::remove_dir_all(&root).ok();
    }
}
