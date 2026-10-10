//! Architecture gate (11-SPEC §13 G07): this test fails CI if any crate in the
//! workspace violates the dependency matrix, the external-dependency budget, or
//! if a module manifest cannot compile into the platform graph.

use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn dependency_matrix_has_zero_violations() {
    let root = workspace_root();
    let violations = kelson_analyzer::analyze_workspace(&root);
    let report: Vec<String> = violations
        .iter()
        .map(|v| format!("{} {}: {}", v.code, v.member, v.message))
        .collect();
    assert!(
        violations.is_empty(),
        "architecture violations:\n{}",
        report.join("\n")
    );
}

#[test]
fn every_member_belongs_to_a_topology_class() {
    let root = workspace_root();
    let raw = std::fs::read_to_string(root.join("Cargo.toml")).expect("root manifest");
    let members = kelson_analyzer::parse_members(&raw).expect("members parse");
    assert!(!members.is_empty(), "workspace has no members");
    for m in &members {
        assert!(
            kelson_analyzer::Class::from_member_path(m).is_some(),
            "member {m} is outside the topology (core/security/framework/adapters/runtime/toolchain/validation/examples/applications)"
        );
    }
}

#[test]
fn platform_graph_compiles_from_manifests() {
    let root = workspace_root();
    let mut manifests = Vec::new();
    for base in ["applications", "modules"] {
        let dir = root.join(base);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let manifest_path = entry.path().join("module.toml");
            if manifest_path.exists() {
                let raw = std::fs::read_to_string(&manifest_path).expect("manifest read");
                manifests
                    .push(kelson_registry::ModuleManifest::from_toml(&raw).expect("manifest parse"));
            }
        }
    }
    let graph = kelson_registry::PlatformGraph::compile(manifests)
        .unwrap_or_else(|v| panic!("graph violations: {v:?}"));
    // Foundation stage: graph must at least compile; modules arrive later.
    let _ = graph.modules.len();
}
