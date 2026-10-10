//! # kelson-analyzer
//! Static architecture guard (STAGE-0-SPEC §3, gate G07): parses every member
//! Cargo.toml, classifies crates by their topology folder, and enforces the
//! layer dependency matrix. Runs in `kelson check`, xtask, CI and pre-release.

use std::collections::BTreeMap;
use std::path::Path;

/// Topology classes (STAGE-0-SPEC §2/§3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    Foundation,
    Execution,
    Application,
    Web,
    Security,
    Data,
    Async,
    Services,
    Observability,
    Enterprise,
    Testing,
    Ai,
    Adapters,
    Runtime,
    Tools,
    Tests,
    Examples,
    Applications,
    /// `crates/framework`: the umbrella crate (`kelson-framework`) re-exporting
    /// the published framework surface as one dependency (PUBLISHING.md Wave 2
    /// step 3). Added by the distribution program — STAGE-0-SPEC §2 predates
    /// the umbrella and stays FROZEN (the `Extension` precedent): the addition
    /// is recorded in CHANGELOG + docs/19 delivery note, not by rewriting §2.
    Framework,
    /// Extension crates (`crates/extension/<name>`): sandboxed plugin runtimes
    /// hosted by the runtime (docs/19 Wave 9 B-30). Added by the Wave 9
    /// program — STAGE-0-SPEC §2 predates the class.
    Extension,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Foundation => "foundation",
            Class::Execution => "execution",
            Class::Application => "application",
            Class::Web => "web",
            Class::Security => "security",
            Class::Data => "data",
            Class::Async => "async",
            Class::Services => "services",
            Class::Observability => "observability",
            Class::Enterprise => "enterprise",
            Class::Testing => "testing",
            Class::Ai => "ai",
            Class::Adapters => "adapters",
            Class::Runtime => "runtime",
            Class::Tools => "tools",
            Class::Tests => "tests",
            Class::Examples => "examples",
            Class::Applications => "applications",
            Class::Framework => "framework",
            Class::Extension => "extension",
        }
    }

    /// Classify a workspace member by its relative path. `crates/<layer>/<crate>`
    /// uses the layer segment; everything else uses the first segment.
    pub fn from_member_path(rel: &str) -> Option<Self> {
        let mut segs = rel.split(['/', '\\']);
        let first = segs.next()?;
        match first {
            "crates" => match segs.next()? {
                "foundation" => Some(Class::Foundation),
                "execution" => Some(Class::Execution),
                "application" => Some(Class::Application),
                "web" => Some(Class::Web),
                "security" => Some(Class::Security),
                "data" => Some(Class::Data),
                "async" => Some(Class::Async),
                "services" => Some(Class::Services),
                "observability" => Some(Class::Observability),
                "enterprise" => Some(Class::Enterprise),
                "testing" => Some(Class::Testing),
                "ai" => Some(Class::Ai),
                "framework" => Some(Class::Framework),
                "extension" => Some(Class::Extension),
                _ => None,
            },
            "testing" => Some(Class::Testing),
            "adapters" => Some(Class::Adapters),
            "runtime" => Some(Class::Runtime),
            "tools" | "xtask" => Some(Class::Tools),
            "tests" => Some(Class::Tests),
            "examples" => Some(Class::Examples),
            "applications" => Some(Class::Applications),
            _ => None,
        }
    }

    /// Layer dependency matrix (STAGE-0-SPEC §3). Same-class deps always allowed.
    pub fn allowed_deps(self) -> &'static [Class] {
        use Class::*;
        match self {
            Foundation => &[Foundation],
            Security => &[Foundation, Security],
            Execution => &[Foundation, Security, Execution],
            Application => &[Foundation, Security, Execution, Application],
            // D-77 (recorded as D-73 before Sentinel renumbering): web may use framework services (upload edge -> object storage).
            Web => &[Foundation, Security, Execution, Application, Services, Web],
            Services => &[Foundation, Security, Execution, Application, Services],
            Async => &[Foundation, Security, Execution, Application, Async],
            Observability => &[Foundation, Observability],
            Enterprise => &[
                Foundation,
                Security,
                Execution,
                Application,
                Web,
                Services,
                Async,
                Observability,
                Enterprise,
            ],
            Testing => &[
                Foundation,
                Security,
                Execution,
                Application,
                Web,
                Services,
                Async,
                Observability,
                Data,
                Testing,
            ],
            Ai => &[
                Foundation,
                Security,
                Execution,
                Application,
                Web,
                Services,
                Async,
                Ai,
            ],
            Data => &[
                Foundation,
                Security,
                Execution,
                Application,
                Services,
                Async,
                Data,
            ],
            // Session adapters implement the security-class `SessionStore`
            // port: an adapter necessarily depends on the crate that defines
            // its port (direction adapter -> port owner stays acyclic; the
            // security layer never depends on adapters).
            Adapters => &[Foundation, Security, Services, Async, Adapters],
            // D-77 (recorded as D-73 before Sentinel renumbering): runtime hosts boot domain application modules.
            // Extension joins for Wave 9 B-30: the runtime host instantiates
            // and dispatches WASM plugin handlers.
            Runtime => &[
                Foundation,
                Security,
                Execution,
                Application,
                Web,
                Data,
                Async,
                Services,
                Observability,
                Enterprise,
                Adapters,
                Applications,
                Extension,
                Runtime,
            ],
            // STAGE-0-SPEC §3: tools -> anything except runtime binaries.
            // Adapters/tests/examples/applications stay excluded (stricter than
            // the floor; adapters are wired by runtime, not by platform tools).
            Tools => &[
                Foundation,
                Security,
                Execution,
                Application,
                Web,
                Services,
                Async,
                Observability,
                Data,
                Enterprise,
                Ai,
                Tools,
            ],
            Tests => &[Foundation, Application, Tools, Tests],
            Examples => &[
                Foundation,
                Security,
                Execution,
                Application,
                Web,
                Data,
                Async,
                Services,
                Observability,
                Ai,
                Adapters,
                Examples,
            ],
            Applications => &[
                Foundation,
                Security,
                Execution,
                Application,
                Web,
                Services,
                Async,
                Observability,
                Data,
                Enterprise,
                Adapters,
                Applications,
            ],
            // The umbrella re-exports the published framework surface, so it
            // spans every library layer a host or application module reaches.
            // It deliberately does NOT depend on Adapters/Runtime/Tools/Tests/
            // Examples/Applications: wiring, binaries and scaffolding are not
            // part of the surface an application imports.
            Framework => &[
                Foundation,
                Security,
                Execution,
                Application,
                Web,
                Services,
                Async,
                Observability,
                Data,
                Enterprise,
                Extension,
                Framework,
            ],
            // docs/19 Wave 9 B-30: the WASM plugin runtime hosts module
            // manifests (Application), builds ExecutionPlans and dispatches
            // through the OperationHandler trait (Execution), and reports
            // through Foundation errors. It must NOT reach Web/Security/Data
            // services: the sandbox has no host-side I/O surface by design.
            Extension => &[Foundation, Execution, Application, Extension],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Violation {
    /// Stable codes: DEP-CLASS, DEP-UNKNOWN-CRATE, DEP-UNKNOWN-MEMBER,
    /// DEP-MISSING-MANIFEST, DEP-NO-MEMBERS, ANALYZER-BUDGET.
    pub code: String,
    pub member: String,
    pub message: String,
}

pub struct CrateInfo {
    pub member: String,
    pub name: String,
    pub class: Class,
    /// (dependency crate name, is_dev_dependency)
    pub kelson_deps: Vec<(String, bool)>,
}

/// Extract `[workspace].members` from the root manifest.
pub fn parse_members(root_toml: &str) -> Result<Vec<String>, String> {
    let v: toml::Value = root_toml
        .parse()
        .map_err(|e| format!("root toml parse: {e}"))?;
    let members = v
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
        .ok_or_else(|| "no [workspace].members array".to_string())?;
    members
        .iter()
        .map(|m| {
            m.as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| "non-string member".to_string())
        })
        .collect()
}

fn kelson_deps(manifest: &toml::Value, table: &str) -> Vec<(String, bool)> {
    let dev = table == "dev-dependencies";
    manifest
        .get(table)
        .and_then(|d| d.as_table())
        .map(|deps| {
            deps.keys()
                .filter(|k| k.starts_with("kelson-"))
                .map(|k| (k.clone(), dev))
                .collect()
        })
        .unwrap_or_default()
}

/// Analyze a workspace root. Returns ALL violations (no fail-fast).
pub fn analyze_workspace(root: &Path) -> Vec<Violation> {
    let mut violations = Vec::new();
    let root_toml = match std::fs::read_to_string(root.join("Cargo.toml")) {
        Ok(s) => s,
        Err(e) => {
            violations.push(Violation {
                code: "DEP-MISSING-MANIFEST".into(),
                member: ".".into(),
                message: format!("cannot read root Cargo.toml: {e}"),
            });
            return violations;
        }
    };
    let members = match parse_members(&root_toml) {
        Ok(m) => m,
        Err(e) => {
            violations.push(Violation {
                code: "DEP-NO-MEMBERS".into(),
                member: ".".into(),
                message: e,
            });
            return violations;
        }
    };

    let mut crates = Vec::new();
    for member in &members {
        let manifest_path = root.join(member).join("Cargo.toml");
        let raw = match std::fs::read_to_string(&manifest_path) {
            Ok(s) => s,
            Err(e) => {
                violations.push(Violation {
                    code: "DEP-MISSING-MANIFEST".into(),
                    member: member.clone(),
                    message: format!("cannot read {}: {e}", manifest_path.display()),
                });
                continue;
            }
        };
        let manifest: toml::Value = match raw.parse() {
            Ok(v) => v,
            Err(e) => {
                violations.push(Violation {
                    code: "DEP-MISSING-MANIFEST".into(),
                    member: member.clone(),
                    message: format!("manifest parse: {e}"),
                });
                continue;
            }
        };
        let Some(class) = Class::from_member_path(member) else {
            violations.push(Violation {
                code: "DEP-UNKNOWN-MEMBER".into(),
                member: member.clone(),
                message: "member path is not a topology class (STAGE-0-SPEC §2)".into(),
            });
            continue;
        };
        let name = manifest
            .get("package")
            .and_then(|p| p.get("name"))
            .and_then(|n| n.as_str())
            .unwrap_or_default()
            .to_string();
        let mut deps = kelson_deps(&manifest, "dependencies");
        deps.extend(kelson_deps(&manifest, "dev-dependencies"));
        crates.push(CrateInfo {
            member: member.clone(),
            name,
            class,
            kelson_deps: deps,
        });
    }

    let by_name: BTreeMap<&str, &CrateInfo> = crates.iter().map(|c| (c.name.as_str(), c)).collect();
    for c in &crates {
        for (dep, dev) in &c.kelson_deps {
            let Some(info) = by_name.get(dep.as_str()) else {
                violations.push(Violation {
                    code: "DEP-UNKNOWN-CRATE".into(),
                    member: c.member.clone(),
                    message: format!("depends on unknown workspace crate {dep}"),
                });
                continue;
            };
            if !c.class.allowed_deps().contains(&info.class) {
                violations.push(Violation {
                    code: "DEP-CLASS".into(),
                    member: c.member.clone(),
                    message: format!(
                        "{} crate {} depends on {} crate {dep}{}",
                        c.class.as_str(),
                        c.name,
                        info.class.as_str(),
                        if *dev { " (dev-dependency)" } else { "" }
                    ),
                });
            }
        }
    }

    // External-dependency budget (DEPENDENCY-POLICY v2, D-62): cap 25 direct externals.
    let externals = root_toml
        .parse::<toml::Value>()
        .ok()
        .and_then(|v| {
            v.get("workspace")?
                .get("dependencies")?
                .as_table()
                .map(|t| t.iter().filter(|(_, v)| v.get("path").is_none()).count())
        })
        .unwrap_or(0);
    if externals > 25 {
        violations.push(Violation {
            code: "ANALYZER-BUDGET".into(),
            member: ".".into(),
            message: format!(
                "{externals} direct external dependencies exceed the policy-v2 cap of 25 (D-62)"
            ),
        });
    }
    violations
}
