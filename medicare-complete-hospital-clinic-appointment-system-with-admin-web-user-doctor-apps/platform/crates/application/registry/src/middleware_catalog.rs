//! Named middleware catalog (Wave 1, Laravel `$middlewareGroups` parity).
//!
//! Pure resolution half — the axum chain runtime lives in
//! `keel_http::middleware_catalog`. Contract:
//! - `middleware.toml` at the workspace discovery root (same root as
//!   `applications/`) declares `[aliases]` (name -> builtin id), `[groups]`
//!   (name -> ordered alias list) and `[throttle.<group>]` bucket configs.
//!   Absent file = the builtin catalog: aliases `auth`/`verified`/
//!   `throttle:api`, group `api = ["throttle:api", "auth", "verified"]`.
//! - Builtins: `auth` (installs the resolved principal), `verified` (B-23
//!   email gate), `confirmed` (Wave 4 Phase 7 password-confirm window),
//!   `throttle:<group>` (token bucket keyed by group + client).
//! - Tenant-plane graph routes get the implicit `api` group unless
//!   `public = true` (which strips `auth` + `verified` only). Explicit route
//!   `middleware = [...]` entries resolve alias-first, then group (flattened
//!   in place), and run AFTER the implicit chain. First entry = outermost.
//! - Modules cannot define custom middleware in v1 — the catalog is
//!   framework-owned (documented follow-up; Wave 5 ships a `middleware`
//!   codegen kind for builtin registration).

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Stable violation code reported when a route references an unknown
/// middleware alias/group (fail-closed boot gate, Wave 1).
pub const GRAPH_MW_UNKNOWN: &str = "GRAPH-MW-UNKNOWN";

/// One middleware resolution failure (boot-refusing).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MwViolation {
    pub code: String,
    pub message: String,
}

/// Named builtin middlewares. Serialized into the route-cache artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BuiltinMw {
    Auth,
    Verified,
    /// Wave 4 Phase 7 password-confirm (docs/19 L221): admit only a session
    /// stamped by `POST /auth/confirm` within `auth.confirm_timeout_secs`;
    /// 403 `CONFIRMATION_REQUIRED` otherwise.
    Confirmed,
    /// Wave 2 signed-URL resolution (19-PARITY "URL generation"): verify the
    /// HMAC over the request path+query before auth runs, and consume the
    /// `once` marker of a one-time signed URL through the token store.
    Signed,
    Throttle { group: String },
}

/// Parse a builtin id: "auth" | "verified" | "confirmed" | "signed" |
/// "throttle:<group>".
pub fn parse_builtin_id(id: &str) -> Result<BuiltinMw, String> {
    match id {
        "auth" => Ok(BuiltinMw::Auth),
        "verified" => Ok(BuiltinMw::Verified),
        "confirmed" => Ok(BuiltinMw::Confirmed),
        "signed" => Ok(BuiltinMw::Signed),
        other => {
            if let Some(group) = other.strip_prefix("throttle:") {
                if group.is_empty() {
                    return Err("throttle requires a group: throttle:<group>".into());
                }
                Ok(BuiltinMw::Throttle {
                    group: group.to_string(),
                })
            } else {
                Err(format!(
                    "unknown middleware builtin {id:?} (expected auth, verified, confirmed, signed, throttle:<group>)"
                ))
            }
        }
    }
}

/// Token-bucket posture for one throttle group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThrottleGroupConfig {
    pub capacity: u32,
    pub refill_per_sec: f64,
}

impl Default for ThrottleGroupConfig {
    fn default() -> Self {
        Self {
            capacity: 120,
            refill_per_sec: 2.0,
        }
    }
}

/// middleware.toml file shape (all sections optional; missing file = builtin).
#[derive(Debug, Deserialize)]
struct MiddlewareFile {
    #[serde(default)]
    aliases: BTreeMap<String, String>,
    #[serde(default)]
    groups: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    throttle: BTreeMap<String, ThrottleGroupConfig>,
}

/// Aliases + groups + throttle configs. Framework-owned in v1.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MiddlewareCatalog {
    aliases: BTreeMap<String, String>,
    groups: BTreeMap<String, Vec<String>>,
    throttle: BTreeMap<String, ThrottleGroupConfig>,
}

impl MiddlewareCatalog {
    /// The builtin catalog used when `middleware.toml` is absent: group `api`
    /// = throttle:api -> auth -> verified (first = outermost).
    pub fn builtin() -> Self {
        let mut aliases = BTreeMap::new();
        aliases.insert("auth".into(), "auth".into());
        aliases.insert("verified".into(), "verified".into());
        // Wave 4 Phase 7: password-confirm gate (19-PARITY "password-confirm
        // flow"); opt-in per route, never part of the implicit `api` group.
        aliases.insert("confirmed".into(), "confirmed".into());
        // Wave 2: signed-URL gate (19-PARITY "URL generation").
        aliases.insert("signed".into(), "signed".into());
        aliases.insert("throttle:api".into(), "throttle:api".into());
        let mut groups = BTreeMap::new();
        groups.insert(
            "api".into(),
            vec![
                "throttle:api".into(),
                "auth".into(),
                "verified".into(),
            ],
        );
        let mut throttle = BTreeMap::new();
        throttle.insert("api".into(), ThrottleGroupConfig::default());
        Self {
            aliases,
            groups,
            throttle,
        }
    }

    /// Load `middleware.toml` from the discovery root; absent file = builtin
    /// catalog. Malformed files and unknown references are load errors
    /// (fail-closed: the runtime refuses boot on them).
    pub fn load(root: &Path) -> Result<Self, String> {
        let path = root.join("middleware.toml");
        if !path.exists() {
            return Ok(Self::builtin());
        }
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        Self::from_toml(&raw).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// Parse a `middleware.toml` document over the builtin catalog.
    pub fn from_toml(raw: &str) -> Result<Self, String> {
        let file: MiddlewareFile = toml::from_str(raw).map_err(|e| e.to_string())?;
        let mut catalog = Self::builtin();
        for (name, target) in file.aliases {
            parse_builtin_id(&target)
                .map_err(|e| format!("middleware.toml alias {name:?}: {e}"))?;
            catalog.aliases.insert(name, target);
        }
        for (name, entries) in file.groups {
            for entry in &entries {
                if !catalog.aliases.contains_key(entry) {
                    return Err(format!(
                        "middleware.toml group {name:?} references unknown alias {entry:?}"
                    ));
                }
            }
            catalog.groups.insert(name, entries);
        }
        for (name, cfg) in file.throttle {
            catalog.throttle.insert(name, cfg);
        }
        Ok(catalog)
    }

    pub fn throttle_config(&self, group: &str) -> ThrottleGroupConfig {
        self.throttle.get(group).cloned().unwrap_or_default()
    }

    /// All throttle group configs (chain-runtime state).
    pub fn throttle_configs(&self) -> BTreeMap<String, ThrottleGroupConfig> {
        self.throttle.clone()
    }

    /// Override/add one throttle group config (harnesses and tests).
    pub fn set_throttle(&mut self, group: impl Into<String>, cfg: ThrottleGroupConfig) {
        self.throttle.insert(group.into(), cfg);
    }

    pub fn group_names(&self) -> Vec<&str> {
        self.groups.keys().map(String::as_str).collect()
    }

    /// Resolve one chain entry: alias -> builtin; bare builtin syntax
    /// (`throttle:<group>`, `auth`, `verified`) resolves directly; group name
    /// flattens to its alias list (one level, validated at load).
    fn resolve_entry(&self, entry: &str) -> Result<Vec<BuiltinMw>, MwViolation> {
        let violation = |message: String| MwViolation {
            code: GRAPH_MW_UNKNOWN.into(),
            message,
        };
        if let Some(target) = self.aliases.get(entry) {
            return parse_builtin_id(target)
                .map(|mw| vec![mw])
                .map_err(|e| violation(format!("alias {entry:?}: {e}")));
        }
        if let Ok(mw) = parse_builtin_id(entry) {
            return Ok(vec![mw]);
        }
        if let Some(entries) = self.groups.get(entry) {
            let mut out = Vec::new();
            for inner in entries {
                let target = self.aliases.get(inner).ok_or_else(|| {
                    violation(format!(
                        "group {entry:?} entry {inner:?} is not a known alias"
                    ))
                })?;
                out.push(
                    parse_builtin_id(target)
                        .map_err(|e| violation(format!("alias {inner:?}: {e}")))?,
                );
            }
            return Ok(out);
        }
        Err(violation(format!(
            "no middleware alias, builtin or group named {entry:?}"
        )))
    }

    /// Full ordered chain for one route. Tenant-plane routes get the implicit
    /// `api` group first; `public = true` strips `auth` + `verified` from the
    /// implicit chain only (explicit entries are the module's call).
    ///
    /// Wave 2 withoutMiddleware parity: `excluded` entries resolve through
    /// the same catalog (alias, builtin syntax, or group flattened to its
    /// builtins) and every matching builtin is removed from the chain. Each
    /// exclusion must resolve AND be present in the resolved chain,
    /// otherwise `GRAPH-MW-UNKNOWN` (fail closed).
    pub fn resolve_chain(
        &self,
        tenant_plane: bool,
        public: bool,
        route_entries: &[String],
        excluded: &[String],
    ) -> Result<Vec<BuiltinMw>, MwViolation> {
        let mut chain = Vec::new();
        if tenant_plane && self.groups.contains_key("api") {
            for mw in self.resolve_entry("api")? {
                if public && matches!(mw, BuiltinMw::Auth | BuiltinMw::Verified) {
                    continue;
                }
                chain.push(mw);
            }
        }
        for entry in route_entries {
            chain.extend(self.resolve_entry(entry)?);
        }
        for entry in excluded {
            let resolved = self.resolve_entry(entry)?;
            for mw in &resolved {
                let before = chain.len();
                chain.retain(|c| c != mw);
                if chain.len() == before {
                    return Err(MwViolation {
                        code: GRAPH_MW_UNKNOWN.into(),
                        message: format!(
                            "excluded middleware {entry:?} is not part of the route's resolved chain"
                        ),
                    });
                }
            }
        }
        Ok(chain)
    }

    /// Boot gate helper: validate one route's middleware entries (used by
    /// keel-api boot and `keel check` before any route is registered). The
    /// route's method/path only shape the violation message.
    pub fn validate_route(
        &self,
        module: &str,
        route: &crate::RouteSpec,
        tenant_plane: bool,
    ) -> Result<(), MwViolation> {
        self.resolve_chain(
            tenant_plane,
            route.public,
            &route.middleware,
            &route.excluded_middleware,
        )
        .map(|_| ())
        .map_err(|mut v| {
            v.message = format!(
                "module {module}: route {} {}: {}",
                route.method, route.path, v.message
            );
            v
        })
    }
}

/// Boot gate (Wave 1): validate every route's middleware entries in the
/// graph against the catalog, collecting ALL violations (fail-closed, same
/// reporting shape as the other GRAPH-* gates).
pub fn middleware_violations(
    graph: &crate::PlatformGraph,
    catalog: &MiddlewareCatalog,
) -> Vec<crate::GraphViolation> {
    let mut out = Vec::new();
    for m in &graph.modules {
        let tenant_plane = m.module.plane == crate::ModulePlane::Tenant;
        for r in &m.routes {
            if let Err(v) = catalog.validate_route(m.name(), r, tenant_plane) {
                out.push(crate::GraphViolation {
                    code: v.code,
                    message: v.message,
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_implicit_chain_for_tenant_routes() {
        let catalog = MiddlewareCatalog::builtin();
        let chain = catalog.resolve_chain(true, false, &[], &[]).unwrap();
        assert_eq!(
            chain,
            vec![
                BuiltinMw::Throttle { group: "api".into() },
                BuiltinMw::Auth,
                BuiltinMw::Verified,
            ]
        );
        // Non-tenant planes get no implicit chain.
        assert!(catalog.resolve_chain(false, false, &[], &[]).unwrap().is_empty());
    }

    #[test]
    fn public_strips_auth_and_verified_keeps_throttle() {
        let catalog = MiddlewareCatalog::builtin();
        let chain = catalog.resolve_chain(true, true, &[], &[]).unwrap();
        assert_eq!(
            chain,
            vec![BuiltinMw::Throttle { group: "api".into() }]
        );
    }

    #[test]
    fn explicit_entries_flatten_groups_after_implicit() {
        let mut catalog = MiddlewareCatalog::builtin();
        // Alias must exist before a group may reference it (load-time rule).
        catalog
            .aliases
            .insert("throttle:ops".into(), "throttle:ops".into());
        catalog
            .groups
            .insert("ops".into(), vec!["auth".into(), "throttle:ops".into()]);
        let entries = vec!["ops".into(), "verified".into()];
        let chain = catalog.resolve_chain(true, false, &entries, &[]).unwrap();
        assert_eq!(
            chain,
            vec![
                BuiltinMw::Throttle { group: "api".into() },
                BuiltinMw::Auth,
                BuiltinMw::Verified,
                BuiltinMw::Auth,
                BuiltinMw::Throttle { group: "ops".into() },
                BuiltinMw::Verified,
            ]
        );
    }

    #[test]
    fn unknown_entry_is_graph_mw_unknown() {
        let catalog = MiddlewareCatalog::builtin();
        let err = catalog
            .resolve_chain(true, false, &["nope".into()], &[])
            .unwrap_err();
        assert_eq!(err.code, GRAPH_MW_UNKNOWN);
        assert!(err.message.contains("nope"));
    }

    /// Wave 4 Phase 7: `confirmed` is a builtin alias — resolvable per route
    /// — but never part of the implicit `api` group (opt-in only).
    #[test]
    fn confirmed_alias_resolves_but_stays_out_of_the_implicit_group() {
        let catalog = MiddlewareCatalog::builtin();
        assert_eq!(
            parse_builtin_id("confirmed").expect("parses"),
            BuiltinMw::Confirmed
        );
        let chain = catalog
            .resolve_chain(true, false, &["confirmed".into()], &[])
            .unwrap();
        assert_eq!(
            chain,
            vec![
                BuiltinMw::Throttle { group: "api".into() },
                BuiltinMw::Auth,
                BuiltinMw::Verified,
                BuiltinMw::Confirmed,
            ],
            "confirmed rides after the implicit group"
        );
        let implicit = catalog.resolve_chain(true, false, &[], &[]).unwrap();
        assert!(
            !implicit.contains(&BuiltinMw::Confirmed),
            "the default api group never gates confirmation"
        );
    }

    #[test]
    fn middleware_toml_loads_and_rejects_bad_references() {
        static DIR_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "keel-mw-reg-{}-{}",
            std::process::id(),
            DIR_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("middleware.toml"),
            r#"
[aliases]
"limit" = "throttle:heavy"

[groups]
api = ["throttle:api", "auth", "verified"]
write = ["limit", "auth"]

[throttle.heavy]
capacity = 5
refill_per_sec = 0.5
"#,
        )
        .unwrap();
        let catalog = MiddlewareCatalog::load(&dir).unwrap();
        let chain = catalog
            .resolve_chain(true, false, &["write".into()], &[])
            .unwrap();
        assert!(chain.contains(&BuiltinMw::Throttle {
            group: "heavy".into()
        }));
        assert_eq!(catalog.throttle_config("heavy").capacity, 5);

        // Group referencing an unknown alias fails the load (fail closed).
        std::fs::write(
            dir.join("middleware.toml"),
            "[groups]\napi = [\"ghost\"]\n",
        )
        .unwrap();
        assert!(MiddlewareCatalog::load(&dir).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn excluded_middleware_removes_entries_from_the_chain() {
        let catalog = MiddlewareCatalog::builtin();
        // Drop the implicit verified gate from a tenant route.
        let chain = catalog
            .resolve_chain(true, false, &[], &["verified".into()])
            .unwrap();
        assert_eq!(
            chain,
            vec![
                BuiltinMw::Throttle { group: "api".into() },
                BuiltinMw::Auth,
            ]
        );
    }

    #[test]
    fn excluded_middleware_flattens_groups() {
        let catalog = MiddlewareCatalog::builtin();
        // Excluding the whole implicit group removes all three builtins.
        let chain = catalog
            .resolve_chain(true, false, &[], &["api".into()])
            .unwrap();
        assert!(chain.is_empty());
    }

    #[test]
    fn excluded_middleware_not_in_chain_is_graph_mw_unknown() {
        let catalog = MiddlewareCatalog::builtin();
        // auth is already stripped by public = true -> exclusion is dangling.
        let err = catalog
            .resolve_chain(true, true, &[], &["auth".into()])
            .unwrap_err();
        assert_eq!(err.code, GRAPH_MW_UNKNOWN);
        assert!(err.message.contains("auth"));
        // Unknown exclusion names fail the same gate.
        let err = catalog
            .resolve_chain(true, false, &[], &["ghost".into()])
            .unwrap_err();
        assert_eq!(err.code, GRAPH_MW_UNKNOWN);
    }

    #[test]
    fn set_throttle_overrides_group_config() {
        let mut catalog = MiddlewareCatalog::builtin();
        catalog.set_throttle(
            "api",
            ThrottleGroupConfig {
                capacity: 3,
                refill_per_sec: 0.0,
            },
        );
        assert_eq!(catalog.throttle_config("api").capacity, 3);
        assert_eq!(catalog.throttle_configs().len(), 1);
    }
}
