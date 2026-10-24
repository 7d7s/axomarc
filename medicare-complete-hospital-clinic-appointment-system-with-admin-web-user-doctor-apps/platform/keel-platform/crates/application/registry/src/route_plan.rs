//! Route plan resolution + route-cache artifact (Wave 1).
//!
//! `resolve_route_plans` turns the compiled PlatformGraph + middleware catalog
//! into one fully-resolved dispatch plan per route (chain, bindings,
//! validation, flags). The plan list is the content of the deployment cache
//! artifact `storage/framework/routes.json`, pinned to a content hash over
//! every module manifest + `middleware.toml`. Boot semantics (strictly
//! stricter than Laravel): the graph always recompiles; a present artifact with
//! a mismatching hash refuses boot ("run `kelson route:cache`").

use std::path::{Path, PathBuf};

use kelson_foundation::{ExecutionProfileKind, SecurityPlane};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::middleware_catalog::{BuiltinMw, MiddlewareCatalog};
use crate::{
    BindingSpec, GraphViolation, ModulePlane, PlatformGraph, ResolvedConstraint, ResponseKind,
    RoutePlane, ValidationSpec, MODULE_ABI,
};

/// Fully-resolved dispatch plan for one route (route-cache artifact content).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutePlan {
    pub module: String,
    pub method: String,
    pub path: String,
    /// Module-local operation name.
    pub operation: String,
    /// Qualified operation id (`module.operation`).
    pub qualified: String,
    pub profile: ExecutionProfileKind,
    pub plane: SecurityPlane,
    /// Route plane (Wave 2): api (default) answers JSON envelopes; web
    /// renders views + web error pages.
    #[serde(default)]
    pub route_plane: RoutePlane,
    /// Named route group the route belongs to, if any (Wave 2).
    #[serde(default)]
    pub group: Option<String>,
    public: bool,
    pub deprecated: bool,
    pub response: ResponseKind,
    /// Resolved middleware chain (first = outermost).
    pub chain: Vec<BuiltinMw>,
    pub bindings: Vec<BindingSpec>,
    /// Manifest validation specs for this route's operation (full spec,
    /// Wave 4 Phase 4: the cache pin carries `custom`/`messages`/`attribute`
    /// too — cache-hit and live-compiled routes must validate identically).
    pub validation: Vec<ValidationSpec>,
    pub name: Option<String>,
    /// GET routes parse `page`/`per_page` and inject `_page` (Wave 1 pagination).
    pub inject_page: bool,
    /// Resolved parameter constraints (Wave 2); mismatch at the edge means
    /// the route does not match (404).
    #[serde(default)]
    pub constraints: Vec<ResolvedConstraint>,
    /// Fallback route (Wave 2): matches any unmatched path on its plane.
    #[serde(default)]
    pub fallback: bool,
    /// View template name for `response = "html"` routes (Wave 2).
    #[serde(default)]
    pub view: Option<String>,
    /// The view model the `view` is bound to (MVA-005): declared in the
    /// manifest, verified against the registered binding at boot, and the
    /// route edge renders schema-checked when it is set.
    #[serde(default)]
    pub view_model: Option<String>,
    /// Exact Host header match (Wave 2).
    #[serde(default)]
    pub domain: Option<String>,
    /// First Host label match driving tenant resolution (Wave 2).
    #[serde(default)]
    pub subdomain: Option<String>,
}

/// Resolve every route's dispatch plan against the catalog. Unknown
/// middleware entries collect ALL `GRAPH-MW-UNKNOWN` violations (fail-closed,
/// same reporting shape as the graph gates).
pub fn resolve_route_plans(
    graph: &PlatformGraph,
    catalog: &MiddlewareCatalog,
) -> Result<Vec<RoutePlan>, Vec<GraphViolation>> {
    let mut plans = Vec::new();
    let mut violations = Vec::new();
    for m in &graph.modules {
        let plane = m.module.plane.to_security_plane();
        let tenant_plane = m.module.plane == ModulePlane::Tenant;
        for r in &m.routes {
            let chain = match catalog.resolve_chain(
                tenant_plane,
                r.public,
                &r.middleware,
                &r.excluded_middleware,
            ) {
                Ok(chain) => chain,
                Err(v) => {
                    violations.push(GraphViolation {
                        code: v.code,
                        message: format!(
                            "module {}: route {} {}: {}",
                            m.name(),
                            r.method,
                            r.path,
                            v.message
                        ),
                    });
                    continue;
                }
            };
            let qualified = if r.operation.contains('.') {
                r.operation.clone()
            } else {
                format!("{}.{}", m.name(), r.operation)
            };
            let profile = m
                .capabilities
                .get(&r.operation)
                .map(|c| c.profile)
                .unwrap_or(ExecutionProfileKind::Command);
            let validation: Vec<ValidationSpec> = m
                .validation
                .iter()
                .filter(|s| s.operation == r.operation)
                .cloned()
                .collect();
            let route_plane = r
                .plane
                .as_deref()
                .and_then(|p| RoutePlane::from_token(p).ok())
                .unwrap_or_default();
            // MVA-005: `view_model` without a `view` is a declaration of
            // nothing — the pair is the contract (fail-closed, collect-all).
            if r.view_model
                .as_deref()
                .is_some_and(|s| !s.trim().is_empty())
                && r.view.as_deref().is_none_or(|v| v.trim().is_empty())
            {
                violations.push(GraphViolation {
                    code: "GRAPH-VIEW-MODEL".into(),
                    message: format!(
                        "module {}: route {} {} declares view_model {:?} with no view to bind it to",
                        m.name(),
                        r.method,
                        r.path,
                        r.view_model
                    ),
                });
                continue;
            }
            plans.push(RoutePlan {
                module: m.name().to_string(),
                method: r.method.to_ascii_uppercase(),
                path: r.path.clone(),
                operation: r.operation.clone(),
                qualified,
                profile,
                plane,
                route_plane,
                group: r.group.clone(),
                public: r.public,
                deprecated: r.deprecated,
                response: r.response,
                chain,
                bindings: r.bindings.clone(),
                validation,
                name: r.name.clone(),
                inject_page: r.method.eq_ignore_ascii_case("get"),
                constraints: r.resolved_constraints.clone(),
                fallback: r.fallback,
                view: r.view.clone(),
                view_model: r.view_model.clone(),
                domain: r.domain.clone(),
                subdomain: r.subdomain.clone(),
            });
        }
    }
    if violations.is_empty() {
        Ok(plans)
    } else {
        Err(violations)
    }
}

/// Deployment cache artifact: resolved plans pinned to a manifest hash +
/// module ABI (hash-mismatch-refuse boot, Wave 1).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteCacheArtifact {
    pub module_abi: u32,
    pub manifest_hash: String,
    pub plans: Vec<RoutePlan>,
}

impl RouteCacheArtifact {
    /// Write the artifact (parent dirs created).
    pub fn write(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Load the artifact; `Ok(None)` when absent. Malformed content is an
    /// error (fail-closed: a corrupt cache refuses boot).
    pub fn load(path: &Path) -> Result<Option<Self>, String> {
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let artifact: Self =
            serde_json::from_str(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Some(artifact))
    }

    /// True when this artifact matches the current workspace content.
    pub fn matches(&self, root: &Path) -> Result<bool, String> {
        Ok(self.module_abi == MODULE_ABI && self.manifest_hash == manifests_hash(root)?)
    }
}

/// Canonical route-cache artifact location under a workspace root.
pub fn route_cache_path(root: &Path) -> PathBuf {
    root.join("storage").join("framework").join("routes.json")
}

/// Content hash over every module manifest + `middleware.toml` (the cache
/// pin). Deterministic: sorted paths, content-separated.
pub fn manifests_hash(root: &Path) -> Result<String, String> {
    let mut hasher = Sha256::new();
    for base in ["applications", "modules"] {
        let dir = root.join(base);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path().join("module.toml"))
            .filter(|p| p.exists())
            .collect();
        paths.sort();
        for path in paths {
            let raw =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            hasher.update(path.display().to_string().as_bytes());
            hasher.update(b"\0");
            hasher.update(raw.as_bytes());
            hasher.update(b"\0");
        }
    }
    let mw = root.join("middleware.toml");
    if mw.exists() {
        let raw = std::fs::read_to_string(&mw).map_err(|e| format!("{}: {e}", mw.display()))?;
        hasher.update(b"middleware.toml\0");
        hasher.update(raw.as_bytes());
        hasher.update(b"\0");
    }
    // Wave 8 (B-24): the package index pins too — adding, re-pointing or
    // editing an entry invalidates a cached route set, and an index that
    // no longer parses (or an entry whose manifest vanished) refuses the
    // hash instead of silently serving the old routes.
    let index = crate::PackageIndex::load(root)?;
    if let Ok(raw) = std::fs::read_to_string(crate::PackageIndex::packages_path(root)) {
        hasher.update(b"packages.toml\0");
        hasher.update(raw.as_bytes());
        hasher.update(b"\0");
    }
    for entry in &index.package {
        let path = crate::PackageIndex::entry_dir(root, entry).join("module.toml");
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| format!("package {}: {}: {e}", entry.name, path.display()))?;
        hasher.update(entry.path.as_bytes());
        hasher.update(b"\0package-manifest\0");
        hasher.update(raw.as_bytes());
        hasher.update(b"\0");
    }
    // Wave 9 (B-30): WASM plugins pin too — sidecar manifest AND compiled
    // binary bytes, so editing either (a code change re-links routes just
    // like an operation rename would) invalidates the cache, and an
    // unreadable file refuses the hash instead of serving stale routes.
    for entry in &index.wasm {
        let sidecar = crate::PackageIndex::resolve_path(root, &entry.manifest);
        let raw = std::fs::read(&sidecar)
            .map_err(|e| format!("wasm plugin {}: {}: {e}", entry.name, sidecar.display()))?;
        hasher.update(b"wasm-sidecar\0");
        hasher.update(entry.name.as_bytes());
        hasher.update(b"\0");
        hasher.update(&raw);
        hasher.update(b"\0");
        let binary = crate::PackageIndex::resolve_path(root, &entry.path);
        let raw = std::fs::read(&binary)
            .map_err(|e| format!("wasm plugin {}: {}: {e}", entry.name, binary.display()))?;
        hasher.update(b"wasm-binary\0");
        hasher.update(entry.name.as_bytes());
        hasher.update(b"\0");
        hasher.update(&raw);
        hasher.update(b"\0");
    }
    // Wave 2: root route files participate in the cache pin too.
    for name in ["api.toml", "channels.toml", "console.toml", "web.toml"] {
        let path = root.join("routes").join(name);
        if path.exists() {
            let raw =
                std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            hasher.update(format!("routes/{name}\0").as_bytes());
            hasher.update(raw.as_bytes());
            hasher.update(b"\0");
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModuleManifest;

    fn manifest(extra: &str) -> ModuleManifest {
        let toml = format!(
            r#"
[module]
name = "alpha"
version = "0.1.0"
api = 1

[capabilities]
"index" = {{ profile = "READ" }}
"create" = {{ profile = "COMMAND" }}

[[routes]]
method = "GET"
path = "/api/v1/things"
operation = "index"
middleware = ["throttle:api"]

[[routes]]
method = "POST"
path = "/api/v1/things"
operation = "create"
deprecated = true

[[validation]]
operation = "create"
field = "name"
rules = ["required", "max:10"]
{extra}
"#
        );
        ModuleManifest::from_toml(&toml).expect("manifest parses")
    }

    #[test]
    fn plans_resolve_chains_validation_and_flags() {
        let graph = PlatformGraph::compile(vec![manifest("")]).expect("graph compiles");
        let catalog = MiddlewareCatalog::builtin();
        let plans = resolve_route_plans(&graph, &catalog).expect("plans resolve");
        assert_eq!(plans.len(), 2);

        let get = plans.iter().find(|p| p.method == "GET").unwrap();
        assert_eq!(get.qualified, "alpha.index");
        assert!(get.inject_page);
        // Implicit api group first, explicit entry appended (outermost-first).
        assert_eq!(
            get.chain,
            vec![
                BuiltinMw::Throttle {
                    group: "api".into()
                },
                BuiltinMw::Auth,
                BuiltinMw::Verified,
                BuiltinMw::Throttle {
                    group: "api".into()
                },
            ]
        );
        assert!(get.validation.is_empty());

        let post = plans.iter().find(|p| p.method == "POST").unwrap();
        assert!(post.deprecated);
        assert!(!post.inject_page);
        assert_eq!(
            post.validation,
            vec![ValidationSpec {
                operation: "create".into(),
                field: "name".into(),
                rules: vec!["required".into(), "max:10".into()],
                custom: None,
                attribute: None,
                messages: Default::default(),
            }]
        );
    }

    #[test]
    fn plans_carry_wave2_fields() {
        let m = manifest(
            r#"
[[routes]]
method = "GET"
path = "/web/things/{id}"
operation = "index"
plane = "web"
response = "html"
view = "things/show"
view_model = "ThingShowVm"
fallback = false
domain = "app.example.com"
constraints = [{ param = "id", kind = "number" }]
"#,
        );
        let graph = PlatformGraph::compile(vec![m]).expect("graph compiles");
        let plans =
            resolve_route_plans(&graph, &MiddlewareCatalog::builtin()).expect("plans resolve");
        let web = plans
            .iter()
            .find(|p| p.path == "/web/things/{id}")
            .expect("web route planned");
        assert_eq!(web.route_plane, RoutePlane::Web);
        assert_eq!(web.view.as_deref(), Some("things/show"));
        assert_eq!(web.view_model.as_deref(), Some("ThingShowVm"));
        assert_eq!(web.domain.as_deref(), Some("app.example.com"));
        assert!(web.subdomain.is_none());
        assert!(!web.fallback);
        assert_eq!(web.constraints.len(), 1);
        assert_eq!(web.constraints[0].param, "id");
        // Existing api-plane routes default to RoutePlane::Api.
        let api = plans.iter().find(|p| p.method == "POST").unwrap();
        assert_eq!(api.route_plane, RoutePlane::Api);
        assert!(api.constraints.is_empty());
    }

    #[test]
    fn view_model_without_a_view_is_refused() {
        // MVA-005: the contract is the PAIR — a model declared with no
        // view to bind it to refuses the compile (fail-closed, collect-all).
        let m = manifest(
            r#"
[[routes]]
method = "GET"
path = "/web/things"
operation = "index"
plane = "web"
response = "html"
view_model = "ThingIndexVm"
"#,
        );
        let graph = PlatformGraph::compile(vec![m]).expect("graph compiles");
        let errs = resolve_route_plans(&graph, &MiddlewareCatalog::builtin())
            .expect_err("the split pair refuses");
        assert!(
            errs.iter().any(|v| v.code == "GRAPH-VIEW-MODEL"),
            "{errs:?}"
        );
    }

    #[test]
    fn route_files_participate_in_the_manifest_hash() {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kelson-routefiles-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(dir.join("routes")).unwrap();
        let base = manifests_hash(&dir).unwrap();

        // Adding a root route file changes the pin.
        std::fs::write(
            dir.join("routes").join("web.toml"),
            "# routes
",
        )
        .unwrap();
        let with_web = manifests_hash(&dir).unwrap();
        assert_ne!(base, with_web);

        // Deterministic for identical content.
        assert_eq!(with_web, manifests_hash(&dir).unwrap());

        // Drift in the route file changes the pin again.
        std::fs::write(
            dir.join("routes").join("web.toml"),
            "# routes v2
",
        )
        .unwrap();
        assert_ne!(with_web, manifests_hash(&dir).unwrap());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Row 9: the **no-rebuild proof**.
    ///
    /// A CONFIG change must not invalidate the compiled route artifact: the
    /// routes are compiled from the module manifests + `middleware.toml`, not
    /// from the config's values, so editing the config (a limit, a flag, the
    /// app name) must leave the artifact VALID — no rebuild, no restart.
    ///
    /// The converse is pinned too: a MANIFEST change DOES invalidate the
    /// artifact (a rebuild is required), so the property is not vacuous.
    #[test]
    fn a_config_change_does_not_invalidate_the_compiled_routes() {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "kelson-norebuild-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let module_dir = root.join("applications").join("alpha");
        std::fs::create_dir_all(&module_dir).unwrap();
        std::fs::create_dir_all(root.join("config")).unwrap();
        let manifest_path = module_dir.join("module.toml");
        let manifest_toml = r#"
[module]
name = "alpha"
version = "0.1.0"
api = 1

[capabilities]
"index" = { profile = "READ" }

[[routes]]
method = "GET"
path = "/api/v1/things"
operation = "index"
"#;
        std::fs::write(&manifest_path, manifest_toml).unwrap();
        std::fs::write(
            root.join("config").join("dev.toml"),
            "[app]\nname = \"before\"\n",
        )
        .unwrap();

        // The compiled artifact, pinned to the current kernel digest.
        let digest = manifests_hash(&root).unwrap();
        let artifact = RouteCacheArtifact {
            module_abi: MODULE_ABI,
            manifest_hash: digest.clone(),
            plans: Vec::new(),
        };
        assert!(
            artifact.matches(&root).unwrap(),
            "the artifact matches the workspace it was built from"
        );

        // A CONFIG change: the app name, a limit, a feature flag…
        std::fs::write(
            root.join("config").join("dev.toml"),
            "[app]\nname = \"after\"\n\n[limits]\ndefault_monthly_operations = 999\n",
        )
        .unwrap();
        assert_eq!(
            manifests_hash(&root).unwrap(),
            digest,
            "a CONFIG change does not touch the kernel digest"
        );
        assert!(
            artifact.matches(&root).unwrap(),
            "the compiled routes are still VALID after a config change — NO REBUILD, no restart"
        );

        // The converse: a MANIFEST change DOES require a rebuild.
        std::fs::write(
            &manifest_path,
            manifest_toml.replace("version = \"0.1.0\"", "version = \"0.2.0\""),
        )
        .unwrap();
        assert_ne!(
            manifests_hash(&root).unwrap(),
            digest,
            "a manifest change moves the kernel digest"
        );
        assert!(
            !artifact.matches(&root).unwrap(),
            "a manifest change invalidates the artifact — a rebuild IS required"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn packages_participate_in_the_manifest_hash() {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kelson-pkghash-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("packages/billing")).unwrap();
        let base = manifests_hash(&dir).unwrap();

        // Creating the index changes the pin (B-24: a cached route set
        // never survives a package registration). The manifest ships with
        // the index — an entry whose manifest is missing fails the hash
        // (fail-closed, asserted below).
        std::fs::write(
            dir.join("packages/billing").join("module.toml"),
            "[module]\nname = \"billing\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        let index_raw =
            "[[package]]\nname = \"billing\"\nversion = \"0.1.0\"\npath = \"packages/billing\"\n";
        std::fs::write(dir.join("packages.toml"), index_raw).unwrap();
        let with_index = manifests_hash(&dir).unwrap();
        assert_ne!(base, with_index);

        // Manifest content drift invalidates the cache too.
        std::fs::write(
            dir.join("packages/billing").join("module.toml"),
            "[module]\nname = \"billing\"\nversion = \"0.1.1\"\n",
        )
        .unwrap();
        let with_manifest = manifests_hash(&dir).unwrap();
        assert_ne!(with_index, with_manifest);
        assert_eq!(with_manifest, manifests_hash(&dir).unwrap());

        // An index entry whose manifest vanished refuses the hash
        // (fail-closed — never a silent stale-serve).
        std::fs::remove_file(dir.join("packages/billing").join("module.toml")).unwrap();
        let err = manifests_hash(&dir).unwrap_err();
        assert!(err.contains("package billing"), "{err}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn wasm_plugins_participate_in_the_manifest_hash() {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kelson-wasmhash-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("plugins/echo")).unwrap();
        std::fs::write(
            dir.join("plugins/echo/module.toml"),
            "[module]\nname = \"echo\"\nversion = \"0.1.0\"\n\n[capabilities.echo]\nprofile = \"READ\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("plugins/echo/echo.wasm"), b"\0asm-v1").unwrap();
        let base = manifests_hash(&dir).unwrap();

        // Registering the plugin pins it (index bytes + both file reads).
        std::fs::write(
            dir.join("packages.toml"),
            "[[wasm]]\nname = \"echo\"\nversion = \"0.1.0\"\npath = \"plugins/echo/echo.wasm\"\nmanifest = \"plugins/echo/module.toml\"\n",
        )
        .unwrap();
        let with_index = manifests_hash(&dir).unwrap();
        assert_ne!(base, with_index);

        // Binary bytes invalidate the pin (B-30: code edits re-pin routes).
        std::fs::write(dir.join("plugins/echo/echo.wasm"), b"\0asm-v2").unwrap();
        let with_binary = manifests_hash(&dir).unwrap();
        assert_ne!(with_index, with_binary);
        assert_eq!(with_binary, manifests_hash(&dir).unwrap(), "deterministic");

        // Sidecar edits invalidate the pin too.
        std::fs::write(
            dir.join("plugins/echo/module.toml"),
            "[module]\nname = \"echo\"\nversion = \"0.1.0\"\n\n[capabilities.scan]\nprofile = \"READ\"\n",
        )
        .unwrap();
        let with_sidecar = manifests_hash(&dir).unwrap();
        assert_ne!(with_binary, with_sidecar);

        // A vanished binary refuses the hash (fail-closed, never stale-serve).
        std::fs::remove_file(dir.join("plugins/echo/echo.wasm")).unwrap();
        let err = manifests_hash(&dir).unwrap_err();
        assert!(err.contains("wasm plugin echo"), "{err}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_middleware_collects_all_violations() {
        let m = manifest(
            r#"
[[routes]]
method = "DELETE"
path = "/api/v1/things/{id}"
operation = "index"
middleware = ["ghost", "phantom"]
"#,
        );
        let graph = PlatformGraph::compile(vec![m]).expect("graph compiles");
        let catalog = MiddlewareCatalog::builtin();
        let errs = resolve_route_plans(&graph, &catalog).unwrap_err();
        // The route-level resolution reports the first unknown entry; the
        // gate reports one violation per offending route (collect-all).
        assert!(!errs.is_empty());
        assert!(errs
            .iter()
            .all(|v| v.code == crate::middleware_catalog::GRAPH_MW_UNKNOWN));
    }

    #[test]
    fn artifact_roundtrip_and_hash_pin() {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kelson-routecache-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let app_dir = dir.join("applications").join("alpha");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("module.toml"),
            r#"
[module]
name = "alpha"
version = "0.1.0"
api = 1

[capabilities]
"index" = { profile = "READ" }

[[routes]]
method = "GET"
path = "/x"
operation = "index"
"#,
        )
        .unwrap();

        let hash1 = manifests_hash(&dir).unwrap();
        assert_eq!(hash1, manifests_hash(&dir).unwrap(), "deterministic");

        let graph = PlatformGraph::compile(vec![ModuleManifest::from_toml(
            &std::fs::read_to_string(app_dir.join("module.toml")).unwrap(),
        )
        .unwrap()])
        .unwrap();
        let plans = resolve_route_plans(&graph, &MiddlewareCatalog::builtin()).unwrap();
        let artifact = RouteCacheArtifact {
            module_abi: MODULE_ABI,
            manifest_hash: hash1.clone(),
            plans,
        };
        let path = route_cache_path(&dir);
        artifact.write(&path).unwrap();
        let loaded = RouteCacheArtifact::load(&path).unwrap().expect("present");
        assert_eq!(loaded.manifest_hash, hash1);
        assert_eq!(loaded.plans.len(), 1);
        assert!(loaded.matches(&dir).unwrap());

        // Manifest drift changes the hash -> artifact no longer matches.
        std::fs::write(
            app_dir.join("module.toml"),
            std::fs::read_to_string(app_dir.join("module.toml")).unwrap() + "\n# drift\n",
        )
        .unwrap();
        assert!(!loaded.matches(&dir).unwrap());

        // middleware.toml participates in the pin too.
        let hash2 = manifests_hash(&dir).unwrap();
        std::fs::write(dir.join("middleware.toml"), "[aliases]\n").unwrap();
        assert_ne!(hash2, manifests_hash(&dir).unwrap());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn absent_artifact_loads_none_and_malformed_fails() {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kelson-routecache2-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = route_cache_path(&dir);
        assert!(RouteCacheArtifact::load(&path).unwrap().is_none());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert!(RouteCacheArtifact::load(&path).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
