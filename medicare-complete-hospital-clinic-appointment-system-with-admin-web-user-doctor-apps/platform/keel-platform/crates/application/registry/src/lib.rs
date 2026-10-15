//! # kelson-registry
//! Module manifests + PlatformGraph compilation (G11/G12). Only a valid graph may
//! produce a release artifact. Violations carry stable codes for `kelson check`.

use std::collections::{BTreeMap, BTreeSet};

use kelson_foundation::{ExecutionProfileKind, SecurityPlane};
use serde::{Deserialize, Serialize};

pub mod config_cache;
pub mod constraint;
pub mod middleware_catalog;
pub mod packages;
pub mod route_plan;

pub use constraint::{
    constraint_matches, ConstraintKind, ConstraintPattern, ConstraintSpec, PatternElement,
    PatternTerm, Repeat, ResolvedConstraint, GRAPH_CONSTRAINT_KIND, GRAPH_CONSTRAINT_PARAM,
};
pub use middleware_catalog::{
    middleware_violations, parse_builtin_id, BuiltinMw, MiddlewareCatalog, MwViolation,
    ThrottleGroupConfig, GRAPH_MW_UNKNOWN,
};
pub use config_cache::{
    config_cache_check, config_cache_path, config_hash, ConfigCacheArtifact, CONFIG_CACHE_ABI,
    CONFIG_CACHE_CODE,
};
pub use packages::{
    ModulePinMap, PackageEntry, PackageIndex, WasmModuleEntry, MODULE_PINS_PATH, PACKAGES_FILE,
};pub use route_plan::{
    manifests_hash, resolve_route_plans, route_cache_path, RouteCacheArtifact, RoutePlan,
};

/// Module ABI version for the third-party extension contract (S10 B-18).
/// Manifests pin the ABI they were written against; the graph gate rejects
/// mismatched modules so a framework upgrade never silently changes module
/// semantics. Bump only when the manifest schema or provider contract breaks.
pub const MODULE_ABI: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub enum MaturityLevel {
    #[default]
    L0,
    L1,
    L2,
    L3,
    L4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Requirement {
    #[default]
    Required,
    Optional,
    Forbidden,
}

/// One declared operation of a module (= permission catalog entry).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CapabilitySpec {
    pub profile: ExecutionProfileKind,
    #[serde(default)]
    pub delta: Requirement,
    #[serde(default)]
    pub audit: Requirement,
    #[serde(default)]
    pub idempotency: Requirement,
    /// Declared operation input shape (Wave 4 Phase 4): the
    /// GRAPH-VALIDATION-FIELD gate checks each `[[validation]]` field's
    /// first path segment against it. Empty (default) = shape undeclared →
    /// the gate skips, per docs/19's "when the shape is declared".
    #[serde(default)]
    pub inputs: Vec<String>,
}

/// One `{param}` path binding resolved at the HTTP edge (Wave 1 model
/// binding): the captured path parameter is looked up in the module's
/// registered model resolver (tenant-scoped ModelStore::find) and injected
/// into the operation input as `_bindings.<param>`. Misses are 404 envelopes.
/// `with_trashed` (Wave 3 Phase 6) additionally resolves soft-deleted rows
/// through the resolver's `resolve_trashed` hook — fail-closed by default
/// when the resolver does not implement it.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BindingSpec {
    pub param: String,
    pub model: String,
    #[serde(default)]
    pub with_trashed: bool,
}

/// Wire response kind for a route. JSON envelopes are the default (S9
/// SPA-first); every other kind is an explicit opt-in per route.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ResponseKind {
    #[default]
    Json,
    Html,
    /// Redirect to a `Location` taken from the operation result (a string
    /// target, or `{ to, status?, flash? }`); `flash` entries are stored
    /// for exactly one subsequent request.
    Redirect,
    /// Stream a stored object (`file` is an accepted alias): the result
    /// names a `StoragePort` object and is served with an attachment
    /// disposition.
    #[serde(alias = "file")]
    Download,
    /// Chunked body: the result's elements are flushed one NDJSON line at
    /// a time instead of one buffered payload.
    Stream,
    /// 204 with no body at all.
    #[serde(rename = "no_content")]
    NoContent,
}

/// Cache policy for a route (Wave 2). A closed set so a manifest can never
/// emit an unparseable `Cache-Control`; an unknown token fails at TOML parse
/// exactly like an unknown `plane`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CachePolicy {
    /// Never store a copy (default: caching is opt-in per route).
    #[default]
    NoStore,
    /// Store, but revalidate before every reuse.
    NoCache,
    /// Store, but only in a private cache (browser), never a shared one.
    Private,
    /// Store in any cache (CDN-friendly reads).
    Public,
}

/// Per-route `Cache-Control` policy: a policy plus the optional lifetimes
/// that only make sense for it. Emitted by the edge through its
/// `CacheControl` builder, never as a raw manifest string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CacheSpec {
    pub policy: CachePolicy,
    /// `max-age=<secs>` (and `s-max-age` for shared caches).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_age_secs: Option<u64>,
    /// `stale-while-revalidate=<secs>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_while_revalidate_secs: Option<u64>,
}

/// Route plane (Wave 2). Closed set enforced by the `GRAPH-PLANE` gate
/// (collect-all, boot-refusing). `api` (default) answers JSON envelopes;
/// `web` renders views + web error pages and participates in session/CSRF
/// posture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RoutePlane {
    #[default]
    Api,
    Web,
}

impl RoutePlane {
    /// Parse a manifest plane token. Unknown tokens are a `GRAPH-PLANE`
    /// violation at graph compile (fail closed).
    pub fn from_token(token: &str) -> Result<Self, String> {
        match token {
            "api" => Ok(Self::Api),
            "web" => Ok(Self::Web),
            other => Err(format!("unknown route plane {other:?} (expected api or web)")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Api => "api",
            Self::Web => "web",
        }
    }
}

/// Route group (Wave 2): declared as `[[route_groups]]` in module manifests
/// or root route files. Member routes set `group = "<name>"` and inherit
/// prefix + middleware + plane; route-level fields win on conflict (group
/// middleware runs OUTERMOST, before the route's own entries). Unknown
/// group references fail the `GRAPH-GROUP-UNKNOWN` gate (collect-all).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteGroupSpec {
    pub name: String,
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub middleware: Vec<String>,
    /// Plane applied to member routes that do not set their own.
    #[serde(default)]
    pub plane: Option<String>,
}

/// Console route (Wave 2, `routes/console.toml`): `kelson call <name>`
/// (Wave 5) executes the referenced module operation under the caller's
/// principal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsoleCommandSpec {
    pub name: String,
    /// Qualified operation id (`module.operation`).
    pub operation: String,
}

/// Broadcast channel authorization entry (Wave 2, `routes/channels.toml`);
/// consumed by Wave 6 broadcast auth. Unknown capabilities fail the
/// `GRAPH-CHANNEL-CAP` gate (collect-all).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelSpec {
    pub name: String,
    pub capability: String,
}

/// Root route file (Wave 2, mirroring Laravel's `routes/*.php`):
/// `routes/web.toml` + `routes/api.toml` carry `[[route]]` entries merged
/// into the graph with the plane forced by file; `routes/console.toml`
/// carries `[[command]]`; `routes/channels.toml` carries `[[channel]]`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RouteFileManifest {
    /// Source path relative to the discovery root (diagnostics only).
    #[serde(default)]
    pub source: String,
    /// Plane forced onto every route in this file ("api" or "web").
    #[serde(default)]
    pub plane: String,
    #[serde(default, rename = "route")]
    pub routes: Vec<RouteSpec>,
    #[serde(default)]
    pub route_groups: Vec<RouteGroupSpec>,
    #[serde(default, rename = "command")]
    pub commands: Vec<ConsoleCommandSpec>,
    #[serde(default, rename = "channel")]
    pub channels: Vec<ChannelSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RouteSpec {
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub operation: String,
    pub capability: Option<String>,
    #[serde(default = "default_version")]
    pub version: String,
    /// Optional route name (S9): enables URL generation and `kelson route:list`
    /// names. Unique within a module (GRAPH-DUP-ROUTENAME).
    #[serde(default)]
    pub name: Option<String>,
    /// Laravel-style resource expansion input (Wave 1): `resource = "notes"`
    /// expands at parse time into index/show/store/update(PUT+PATCH)/destroy
    /// route specs. Mutually exclusive with explicit method/operation.
    #[serde(default)]
    pub resource: Option<String>,
    /// Restrict resource expansion to these verbs (operation names).
    #[serde(default)]
    pub only: Vec<String>,
    /// Exclude these verbs from resource expansion.
    #[serde(default)]
    pub except: Vec<String>,
    /// Named middleware chain entries (aliases, `throttle:<group>`, or group
    /// names) resolved by the middleware catalog; run innermost, in order.
    #[serde(default)]
    pub middleware: Vec<String>,
    /// True = reachable anonymously (implicit auth/verified stripped).
    /// False (default) = the implicit `api` group guards the route.
    #[serde(default)]
    pub public: bool,
    /// Emits a `Deprecation: true` response header + envelope meta (L72).
    #[serde(default)]
    pub deprecated: bool,
    /// Response wire kind: json (default) or html (Wave 3 views).
    #[serde(default)]
    pub response: ResponseKind,
    /// Path-parameter -> model bindings resolved at the edge (Wave 1).
    #[serde(default)]
    pub bindings: Vec<BindingSpec>,
    /// Route plane (Wave 2): "api" (default) or "web"; the `GRAPH-PLANE`
    /// gate enforces the closed set. Root route files force their plane.
    #[serde(default)]
    pub plane: Option<String>,
    /// Named route group (Wave 2): inherits prefix/middleware/plane from the
    /// matching `[[route_groups]]` entry (`GRAPH-GROUP-UNKNOWN` gate).
    #[serde(default)]
    pub group: Option<String>,
    /// Parameter constraints (Wave 2, Laravel `where*` parity): closed kinds
    /// and a pattern mini-grammar; mismatch -> the route does not match ->
    /// 404. Gates: `GRAPH-CONSTRAINT-PARAM` / `GRAPH-CONSTRAINT-KIND`.
    #[serde(default)]
    pub constraints: Vec<ConstraintSpec>,
    /// Singleton expansion input (Wave 2, Laravel `Route::singleton`):
    /// expands to show/update(PUT+PATCH)/destroy WITHOUT `{id}`. Mutually
    /// exclusive with resource/method/operation.
    #[serde(default)]
    pub singleton: bool,
    /// Fallback route (Wave 2): matches any path no other route matches on
    /// this plane. Max one per plane (`GRAPH-FALLBACK-DUP` gate).
    #[serde(default)]
    pub fallback: bool,
    /// withoutMiddleware equivalent (Wave 2): entries removed from the
    /// resolved chain. Every name must be part of the route's resolved
    /// chain, otherwise `GRAPH-MW-UNKNOWN`.
    #[serde(default)]
    pub excluded_middleware: Vec<String>,
    /// View template name for `response = "html"` routes (Wave 2 views).
    #[serde(default)]
    pub view: Option<String>,
    /// Exact host match (Wave 2): the route only matches this Host header.
    /// Mutually exclusive with `subdomain`.
    #[serde(default)]
    pub domain: Option<String>,
    /// Subdomain match (Wave 2): the first Host label must equal this slug
    /// and drives tenant resolution. Mutually exclusive with `domain`.
    #[serde(default)]
    pub subdomain: Option<String>,
    /// Per-route cache policy (Wave 2, 19-PARITY "HTTP caching"): the
    /// `Cache-Control` directives the edge emits for this route. Absent
    /// means no explicit cache policy (the route still gets ETags).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache: Option<CacheSpec>,
    /// Internal: true for specs produced by resource/singleton expansion.
    /// Not part of the serialized contract (skipped both directions).
    #[serde(skip)]
    pub from_resource: bool,
    /// Internal: true for specs merged from root route files (Wave 2).
    #[serde(skip)]
    pub from_route_file: bool,
    /// Internal: constraints resolved by the graph compile pass (Wave 2).
    /// Not part of the serialized contract.
    #[serde(skip)]
    pub resolved_constraints: Vec<ResolvedConstraint>,
}

/// Resource expansion table: (operation, method, path suffix). `update`
/// intentionally expands to two specs (PUT + PATCH) sharing one operation,
/// matching Laravel apiResource semantics.
const RESOURCE_VERBS: &[(&str, &str, &str)] = &[
    ("index", "GET", ""),
    ("show", "GET", "/{id}"),
    ("store", "POST", ""),
    ("update", "PUT", "/{id}"),
    ("update", "PATCH", "/{id}"),
    ("destroy", "DELETE", "/{id}"),
];

/// Singleton expansion table (Wave 2, Laravel `Route::singleton`): the
/// show/update/destroy verbs WITHOUT `{id}` — one instance per path.
const SINGLETON_VERBS: &[(&str, &str, &str)] = &[
    ("show", "GET", ""),
    ("update", "PUT", ""),
    ("update", "PATCH", ""),
    ("destroy", "DELETE", ""),
];

/// Expand `resource = "..."` route entries into concrete RouteSpecs. Runs at
/// parse time so every downstream consumer (graph compile, route:list,
/// OpenAPI, HTTP edge) sees one flat route list. Non-resource entries are
/// validated to carry method/path/operation.
fn expand_route_list(
    owner: &str,
    routes: Vec<RouteSpec>,
) -> Result<Vec<RouteSpec>, GraphViolation> {
    let mut expanded: Vec<RouteSpec> = Vec::new();
    for route in routes {
        let Some(resource) = &route.resource else {
            if route.method.is_empty() || route.path.is_empty() || route.operation.is_empty() {
                return Err(GraphViolation {
                    code: "GRAPH-MANIFEST".into(),
                    message: format!(
                        "{owner}: every [[routes]] entry needs method/path/operation or a resource"
                    ),
                });
            }
            if route.singleton {
                return Err(GraphViolation {
                    code: "GRAPH-MANIFEST".into(),
                    message: format!(
                        "{owner}: singleton = true requires a resource path (expands without {{id}})"
                    ),
                });
            }
            expanded.push(route);
            continue;
        };
        if !route.method.is_empty() || !route.operation.is_empty() {
            return Err(GraphViolation {
                code: "GRAPH-MANIFEST".into(),
                message: format!(
                    "{owner}: resource route {resource:?} must not set method/operation"
                ),
            });
        }
        let base = if resource.starts_with('/') {
            resource.clone()
        } else {
            format!("/{resource}")
        };
        let basename = route.name.clone().unwrap_or_else(|| {
            base.rsplit('/')
                .find(|s| !s.is_empty())
                .unwrap_or("resource")
                .to_string()
        });
        let verbs: &[(&str, &str, &str)] = if route.singleton {
            SINGLETON_VERBS
        } else {
            RESOURCE_VERBS
        };
        for (op, method, suffix) in verbs {
            if !route.only.is_empty() && !route.only.iter().any(|o| o == op) {
                continue;
            }
            if route.except.iter().any(|o| o == op) {
                continue;
            }
            expanded.push(RouteSpec {
                method: (*method).to_string(),
                path: format!("{base}{suffix}"),
                operation: (*op).to_string(),
                capability: Some((*op).to_string()),
                version: route.version.clone(),
                name: Some(format!("{basename}.{op}")),
                resource: None,
                only: Vec::new(),
                except: Vec::new(),
                middleware: route.middleware.clone(),
                public: route.public,
                deprecated: route.deprecated,
                response: route.response,
                bindings: route.bindings.clone(),
                plane: route.plane.clone(),
                group: route.group.clone(),
                constraints: route.constraints.clone(),
                singleton: false,
                fallback: false,
                excluded_middleware: route.excluded_middleware.clone(),
                view: route.view.clone(),
                domain: route.domain.clone(),
                subdomain: route.subdomain.clone(),
                cache: route.cache,
                from_resource: true,
                from_route_file: route.from_route_file,
                resolved_constraints: Vec::new(),
            });
        }
    }
    Ok(expanded)
}

/// Group application (Wave 2): member routes inherit prefix + middleware +
/// plane from their `[[route_groups]]` entry; route-level fields win on
/// conflict (group middleware runs OUTERMOST, before the route's own
/// entries). Unknown group references stay set for the `GRAPH-GROUP-UNKNOWN`
/// compile gate (fail closed). Duplicate group names are rejected at parse.
fn apply_groups(
    owner: &str,
    groups: &[RouteGroupSpec],
    routes: &mut [RouteSpec],
) -> Result<(), GraphViolation> {
    if groups.is_empty() {
        return Ok(());
    }
    let mut by_name: BTreeMap<&str, &RouteGroupSpec> = BTreeMap::new();
    for g in groups {
        if by_name.insert(g.name.as_str(), g).is_some() {
            return Err(GraphViolation {
                code: "GRAPH-MANIFEST".into(),
                message: format!("{owner}: duplicate route group {:?}", g.name),
            });
        }
    }
    for r in routes.iter_mut() {
        let Some(gname) = r.group.clone() else {
            continue;
        };
        let Some(g) = by_name.get(gname.as_str()) else {
            continue; // GRAPH-GROUP-UNKNOWN reported at graph compile
        };
        let prefix = normalize_prefix(&g.prefix);
        if !prefix.is_empty() {
            r.path = format!("{prefix}{}", r.path);
        }
        if !g.middleware.is_empty() {
            let mut merged = g.middleware.clone();
            merged.extend(std::mem::take(&mut r.middleware));
            r.middleware = merged;
        }
        if r.plane.is_none() {
            r.plane = g.plane.clone();
        }
    }
    Ok(())
}

fn normalize_prefix(raw: &str) -> String {
    let trimmed = raw.trim_end_matches('/');
    if trimmed.is_empty() {
        return String::new();
    }
    if trimmed.starts_with('/') {
        trimmed.to_string()
    } else {
        format!("/{trimmed}")
    }
}

/// Expand `resource`/`singleton` route entries into concrete RouteSpecs,
/// then apply route groups. Runs at parse time so every downstream consumer
/// (graph compile, route:list, OpenAPI, HTTP edge) sees one flat route list.
fn expand_resources(manifest: &mut ModuleManifest) -> Result<(), GraphViolation> {
    let owner = manifest.name().to_string();
    let expanded = expand_route_list(&owner, std::mem::take(&mut manifest.routes))?;
    manifest.routes = expanded;
    apply_groups(&owner, &manifest.route_groups, &mut manifest.routes)?;
    Ok(())
}

fn default_version() -> String {
    "v1".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSpec {
    pub name: String,
    #[serde(default = "default_stream")]
    pub stream: String,
}

fn default_stream() -> String {
    "data".into()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkflowRef {
    pub name: String,
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModuleDependencies {
    /// Contracts-only dependencies (never another module's internals, §9).
    #[serde(default)]
    pub contracts: Vec<String>,
}

fn default_plane() -> ModulePlane {
    ModulePlane::Tenant
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModulePlane {
    Edge,
    Platform,
    Tenant,
}

impl ModulePlane {
    pub fn to_security_plane(self) -> SecurityPlane {
        match self {
            ModulePlane::Edge => SecurityPlane::Edge,
            ModulePlane::Platform => SecurityPlane::Platform,
            ModulePlane::Tenant => SecurityPlane::Tenant,
        }
    }
}

/// Manifest-declared request validation (Wave 1 FormRequest wiring): rules
/// bound to one field of one operation's merged input. The HTTP edge compiles
/// these into a collect-all Validator per operation and enforces them before
/// CommandBus dispatch (422 envelope with `fields` meta). Rule grammar lives
/// in kelson_validation::Rule::from_token; the graph gate GRAPH-VALIDATION-RULE
/// rejects unparseable rules at boot (fail closed). Wave 4 Phase 4 adds the
/// `custom` rule reference, the `messages`/`attribute` overrides and (via
/// `CapabilitySpec::inputs`) the GRAPH-VALIDATION-FIELD shape declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ValidationSpec {
    pub operation: String,
    pub field: String,
    pub rules: Vec<String>,
    /// Module-registered custom rule referenced by name (Wave 4 Phase 4):
    /// evaluated through the process registry at request time; an unknown
    /// name is a GRAPH-RULE-UNKNOWN boot violation.
    #[serde(default)]
    pub custom: Option<String>,
    /// Display name substituted into `messages` templates' `{attribute}`
    /// placeholder (Phase 4); absent = the field path itself.
    #[serde(default)]
    pub attribute: Option<String>,
    /// Message overrides keyed by rule name (Phase 4): a template replaces
    /// the rule's built-in message; an unknown key is a
    /// GRAPH-VALIDATION-RULE boot violation (a typo would silently never
    /// fire otherwise).
    #[serde(default)]
    pub messages: BTreeMap<String, String>,
}
/// module.toml — the immutable contract shape of a module (11-SPEC §7).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleManifest {
    pub module: ModuleHeader,
    /// Guest ABI generation this artifact is reviewed against (ADR 0019 §2).
    /// Additive: absent means ABI v1, so every shipped fixture and
    /// `your-app/modules/patients` keeps working untouched.
    ///
    /// This is a REVIEW gate, never a detection source. ADR 0019 §2 is
    /// explicit that "the artifact shape decides; nothing is inferred from a
    /// manifest field that could drift" — so `WasmHost::prepare` measures the
    /// binary's layer instead of reading this, and only uses the key to refuse
    /// a component that was never declared (or a module declared as `2`).
    /// It is cleared before the guest/sidecar drift comparison for the same
    /// reason `budget` is: it is host-side review state a guest cannot
    /// meaningfully declare, and a field that detection ignores must not be a
    /// field that can fail the drift check.
    ///
    /// `Some(1)` = core module exporting `kelson_manifest`/`kelson_alloc`/
    /// `kelson_call`; `Some(2)` = Component Model component exporting
    /// `kelson:plugin/dispatch`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub abi: Option<u8>,
    #[serde(default)]
    pub capabilities: BTreeMap<String, CapabilitySpec>,
    #[serde(default)]
    pub dependencies: ModuleDependencies,
    #[serde(default)]
    pub routes: Vec<RouteSpec>,
    #[serde(default)]
    pub events: Vec<EventSpec>,
    #[serde(default)]
    pub workflows: Vec<WorkflowRef>,
    /// Manifest-declared request validation (Wave 1 FormRequest wiring):
    /// rules bound to fields of an operation's merged input.
    #[serde(default)]
    pub validation: Vec<ValidationSpec>,
    /// Route groups (Wave 2): member routes set `group = "<name>"` and
    /// inherit prefix + middleware + plane.
    #[serde(default)]
    pub route_groups: Vec<RouteGroupSpec>,
    /// Console commands (Wave 2): merged from `routes/console.toml` via
    /// [RouteFileManifest::into_module]; `kelson call <name>` (Wave 5)
    /// executes the referenced operation. Unknown operations fail the
    /// GRAPH-ROUTEFILE-OP gate.
    #[serde(default)]
    pub console_commands: Vec<ConsoleCommandSpec>,
    /// Broadcast channel authorization entries (Wave 2): merged from
    /// `routes/channels.toml` via [RouteFileManifest::into_module];
    /// consumed by Wave 6 broadcast auth. Unknown capabilities fail the
    /// GRAPH-CHANNEL-CAP gate.
    #[serde(default)]
    pub channels: Vec<ChannelSpec>,
    /// URL-generation defaults (Wave 2): registered defaults fill every
    /// `{param}` `resolve_url` is not given explicitly (Laravel's
    /// `URL::defaults`). Additive — absent section keeps
    /// [`MODULE_ABI`].
    #[serde(default)]
    pub url: UrlSection,
    /// Boot config defaults (Wave 8 B-24): a `[config]` TOML table a
    /// package ships with its manifest; merged UNDER the app config file
    /// at boot (file wins per key) for packages listed in the root
    /// `packages.toml` index. Additive — absent section keeps
    /// [`MODULE_ABI`].
    #[serde(default)]
    pub config: Option<toml::Value>,
    /// Per-plugin execution budgets (Wave M2). Additive — absent section
    /// keeps [`MODULE_ABI`] and every plugin keeps the host defaults.
    ///
    /// Deliberately named `[budget]`, not `[resources]`: the manifest
    /// format already uses "resources" for Laravel-style route resources
    /// (`expand_resources`, `[resources]` route specs), and one word
    /// meaning two things in one file is how misconfiguration gets written.
    #[serde(default)]
    pub budget: ModuleBudget,
}

/// `[budget]` manifest section (Wave M2): a plugin's execution ceilings.
///
/// **Tighten-only.** Every field may only LOWER a host limit, never raise
/// it. A manifest that asks for more than the host allows is a load
/// refusal, not a negotiation — see [`ModuleBudget::validate_against`].
/// That direction is the whole point: a third-party plugin cannot ship a
/// manifest that widens the sandbox an operator configured, so the host's
/// numbers are the ceiling and the manifest is free to be stricter.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModuleBudget {
    /// Per-call linear-memory ceiling in MiB. Must be `<=` the host's
    /// memory cap. Absent = the host default (16 MiB).
    #[serde(default)]
    pub max_memory_mb: Option<u64>,
    /// Per-call instruction budget (fuel). Must be `<=` the host's fuel
    /// cap. Absent = the host default (10M).
    #[serde(default)]
    pub max_fuel: Option<u64>,
    /// Maximum simultaneous in-flight calls into this plugin. Exceeding it
    /// refuses the call with `KSN_WASM_SATURATED` before a guest is
    /// instantiated. Absent = unlimited (the host default).
    #[serde(default)]
    pub max_concurrency: Option<u32>,
    /// Wall-clock deadline per call, in milliseconds, enforced by wasmtime
    /// epoch interruption. Absent = no wall-clock deadline (fuel remains
    /// the bound). Zero is rejected: a zero-millisecond deadline would
    /// refuse every call, which is never what an operator meant.
    #[serde(default)]
    pub execution_timeout_ms: Option<u64>,
}

impl ModuleBudget {
    /// Enforce tighten-only against the host's ceilings. Called at load, so
    /// an over-declared budget is a boot refusal rather than a silent raise.
    ///
    /// `host_memory_bytes` and `host_fuel` are the host's configured caps.
    /// Returns every violation at once, so an operator fixes their manifest
    /// in one pass rather than one field per run.
    pub fn validate_against(
        &self,
        host_memory_bytes: usize,
        host_fuel: u64,
    ) -> Result<(), Vec<String>> {
        let mut out = Vec::new();
        if let Some(mb) = self.max_memory_mb {
            let requested = mb.saturating_mul(1024 * 1024);
            if requested > host_memory_bytes as u64 {
                out.push(format!(
                    "budget.max_memory_mb = {mb} ({requested} bytes) exceeds the host cap \
                     {host_memory_bytes} bytes — a budget may only LOWER a host limit"
                ));
            }
        }
        if let Some(fuel) = self.max_fuel {
            if fuel > host_fuel {
                out.push(format!(
                    "budget.max_fuel = {fuel} exceeds the host cap {host_fuel} — \
                     a budget may only LOWER a host limit"
                ));
            }
        }
        if let Some(0) = self.execution_timeout_ms {
            out.push(
                "budget.execution_timeout_ms = 0 would refuse every call — \
                 omit the field for no deadline"
                    .to_string(),
            );
        }
        if out.is_empty() {
            Ok(())
        } else {
            Err(out)
        }
    }

    /// The effective memory cap in bytes: the budget's if set, else the
    /// host's. Callers must have run [`Self::validate_against`] first, so
    /// the budget can only ever be the tighter number.
    pub fn effective_memory_bytes(&self, host_memory_bytes: usize) -> usize {
        match self.max_memory_mb {
            Some(mb) => (mb.saturating_mul(1024 * 1024)).min(host_memory_bytes as u64) as usize,
            None => host_memory_bytes,
        }
    }

    /// The effective fuel: the budget's if set, else the host's.
    pub fn effective_fuel(&self, host_fuel: u64) -> u64 {
        match self.max_fuel {
            Some(fuel) => fuel.min(host_fuel),
            None => host_fuel,
        }
    }
}

/// `[url]` manifest section (Wave 2): default parameter values for URL
/// generation. Scope is the declaring module's own routes; a value the
/// caller passes explicitly always wins over a registered default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UrlSection {
    /// `{param} -> default value`.
    #[serde(default)]
    pub defaults: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleHeader {
    pub name: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub maturity: MaturityLevel,
    #[serde(default = "default_plane")]
    pub plane: ModulePlane,
    #[serde(default)]
    pub description: String,
    /// Module ABI this manifest targets (required).
    pub api: u32,
}

impl ModuleManifest {
    pub fn from_toml(raw: &str) -> Result<Self, GraphViolation> {
        let mut manifest: ModuleManifest = toml::from_str(raw).map_err(|e| GraphViolation {
            code: "GRAPH-MANIFEST".into(),
            message: format!("manifest parse failed: {e}"),
        })?;
        expand_resources(&mut manifest)?;
        Ok(manifest)
    }

    /// Parse a guest-supplied `kelson_manifest()` JSON document (Wave 9 B-30,
    /// docs/19). Normalization mirrors [`ModuleManifest::from_toml`]
    /// (resource expansion runs) so the drift comparison against the
    /// reviewed sidecar compares like with like. A malformed document is
    /// `GRAPH-MANIFEST` — the plugin load refuses, fail closed.
    pub fn from_json(raw: &str) -> Result<Self, GraphViolation> {
        let mut manifest: ModuleManifest = serde_json::from_str(raw).map_err(|e| GraphViolation {
            code: "GRAPH-MANIFEST".into(),
            message: format!("manifest parse failed: {e}"),
        })?;
        expand_resources(&mut manifest)?;
        Ok(manifest)
    }

    pub fn name(&self) -> &str {
        &self.module.name
    }
}

impl RouteFileManifest {
    /// Parse a root route file (Wave 2). `forced_plane` overrides any
    /// file-level `plane`: discovery passes Some("web") for
    /// routes/web.toml, Some("api") for routes/api.toml and None for
    /// console/channels. A non-empty plane is forced onto every route in
    /// the file (Laravel's routes/web.php vs routes/api.php split). Route
    /// entries run resource/singleton expansion + group application exactly
    /// like module manifests; the synthetic module owner name is used in
    /// diagnostics.
    pub fn from_toml(
        raw: &str,
        source: &str,
        forced_plane: Option<&str>,
    ) -> Result<Self, GraphViolation> {
        let mut file: RouteFileManifest = toml::from_str(raw).map_err(|e| GraphViolation {
            code: "GRAPH-MANIFEST".into(),
            message: format!("{source}: route file parse failed: {e}"),
        })?;
        file.source = source.to_string();
        if let Some(plane) = forced_plane {
            file.plane = plane.to_string();
        }
        let owner = format!("route file {source}");
        let mut routes = std::mem::take(&mut file.routes);
        for r in routes.iter_mut() {
            if !file.plane.is_empty() {
                r.plane = Some(file.plane.clone());
            }
            r.from_route_file = true;
        }
        routes = expand_route_list(&owner, routes)?;
        apply_groups(&owner, &file.route_groups, &mut routes)?;
        file.routes = routes;
        Ok(file)
    }

    /// Merge this file's routes into a module manifest: route-file specs
    /// keep their forced plane and are prefixed with the owning module name
    /// so qualified operations resolve (the GRAPH-ROUTE-OP gate stays
    /// meaningful — the operation must be declared by the owning module).
    pub fn into_module(self, module_name: &str) -> ModuleManifest {
        ModuleManifest {
            module: ModuleHeader {
                name: module_name.to_string(),
                version: default_version(),
                maturity: MaturityLevel::default(),
                plane: ModulePlane::Tenant,
                description: format!("Root route file {}", self.source),
                api: MODULE_ABI,
            },
            // A root route file is not a guest artifact, so it declares no
            // guest ABI: absent means v1 (ADR 0019 §2).
            abi: None,
            capabilities: BTreeMap::new(),
            dependencies: ModuleDependencies::default(),
            routes: self.routes,
            events: Vec::new(),
            workflows: Vec::new(),
            validation: Vec::new(),
            route_groups: self.route_groups,
            console_commands: self.commands,
            channels: self.channels,
            // Root route files generate URLs for their own routes only;
            // module `[url] defaults` never leaks across the file boundary.
            url: UrlSection::default(),
            // Root route files ship no boot config defaults; only indexed
            // packages feed `[config]` into the boot merge (Wave 8 B-24).
            config: None,
            // A root route file is not a WASM binary, so it has no sandbox to
            // tighten: the budget is the empty one, which resolves to exactly
            // the host ceilings. Declaring anything here would imply a wall
            // clock or concurrency bound that nothing enforces.
            budget: ModuleBudget::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GraphViolation {
    /// Stable code: GRAPH-DUP-MODULE, GRAPH-DUP-OP, GRAPH-DUP-ROUTE, GRAPH-MISSING-DEP,
    /// GRAPH-CYCLE, GRAPH-ROUTE-OP, GRAPH-CAP-MISSING, GRAPH-MANIFEST, plus Wave 2:
    /// GRAPH-PLANE, GRAPH-GROUP-UNKNOWN, GRAPH-CONSTRAINT-PARAM, GRAPH-CONSTRAINT-KIND,
    /// GRAPH-FALLBACK-DUP, GRAPH-ROUTEFILE-OP, GRAPH-CHANNEL-CAP, plus Wave 4:
    /// GRAPH-VERSION-SEMVER, plus the P0 dispatch gates GRAPH-ROUTE-METHOD and
    /// GRAPH-ROUTE-OPID (an edge-skippable route refuses the compile).
    pub code: String,
    pub message: String,
}

/// Wave 4 Phase 8: a `v<N>` version token (`v1`, `v12`, ...). Lowercase `v`
/// followed by one or more ASCII digits — `V1`, `1`, `v1.2`, and `v` alone
/// are all rejected by GRAPH-VERSION-SEMVER. Path segments that pass this
/// check are version-looking and must match the route's declared version.
fn is_version_token(segment: &str) -> bool {
    segment
        .strip_prefix('v')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
}

/// The wire methods a manifest `method` may declare — the canonical set the
/// HTTP edge registers (`graph_routes::declared_methods` in kelson-demo
/// uppercases the manifest token and matches this same list). Anything else
/// is refused by GRAPH-ROUTE-METHOD at compile, so the edge's warn-and-skip
/// registration path — a declared route silently becoming a runtime 404 —
/// can never fire for a compiled graph.
pub const ROUTE_METHODS: &[&str] = &[
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "ANY",
];

/// Compiled, frozen platform graph. Runtime serves lookups against it; it never
/// re-discovers what exists.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PlatformGraph {
    pub modules: Vec<ModuleManifest>,
    /// operation id -> owning module
    pub operations: BTreeMap<String, String>,
    /// event name -> owning module
    pub events: BTreeMap<String, String>,
}

impl PlatformGraph {
    /// Compile + validate. Returns the graph or ALL violations (no fail-fast).
    pub fn compile(mut modules: Vec<ModuleManifest>) -> Result<Self, Vec<GraphViolation>> {
        let mut violations = Vec::new();
        let mut seen_modules = BTreeSet::new();
        let mut operations: BTreeMap<String, String> = BTreeMap::new();
        let mut events: BTreeMap<String, String> = BTreeMap::new();
        let mut routes_seen: BTreeMap<String, String> = BTreeMap::new();

        for m in &modules {
            if m.module.api != MODULE_ABI {
                violations.push(GraphViolation {
                    code: "GRAPH-MANIFEST-ABI".into(),
                    message: format!(
                        "module {} targets module ABI {} but this framework speaks ABI {}",
                        m.name(),
                        m.module.api,
                        MODULE_ABI
                    ),
                });
            }
            if !seen_modules.insert(m.name().to_string()) {
                violations.push(GraphViolation {
                    code: "GRAPH-DUP-MODULE".into(),
                    message: format!("module {} declared more than once", m.name()),
                });
            }
            for op in m.capabilities.keys() {
                let qualified = format!("{}.{}", m.name(), op);
                if let Some(prev) = operations.insert(qualified.clone(), m.name().to_string()) {
                    violations.push(GraphViolation {
                        code: "GRAPH-DUP-OP".into(),
                        message: format!(
                            "operation {qualified} declared by {prev} and {}",
                            m.name()
                        ),
                    });
                }
            }
            for e in &m.events {
                if let Some(prev) = events.insert(e.name.clone(), m.name().to_string()) {
                    violations.push(GraphViolation {
                        code: "GRAPH-DUP-EVENT".into(),
                        message: format!("event {} declared by {prev} and {}", e.name, m.name()),
                    });
                }
            }
            let mut route_names: BTreeMap<String, (String, String, String)> = BTreeMap::new();
            for r in &m.routes {
                // P0 dispatch gates: the HTTP edge registers a route only when
                // its method and `{module}.{operation}` id parse (graph_router
                // skips the rest with a warning). Refuse them here instead, so
                // a route declared in a manifest can never silently degrade
                // into a runtime 404 miss — the boot graph holds only routes
                // that fully register (fail closed; `kelson graph`/`kelson
                // check` report the same codes).
                if !ROUTE_METHODS.contains(&r.method.to_ascii_uppercase().as_str()) {
                    violations.push(GraphViolation {
                        code: "GRAPH-ROUTE-METHOD".into(),
                        message: format!(
                            "module {}: route {} {} declares method {:?}, which the HTTP edge does not serve (allowed: {})",
                            m.name(),
                            r.method,
                            r.path,
                            r.method,
                            ROUTE_METHODS.join("/")
                        ),
                    });
                }
                let operation_id = format!("{}.{}", m.name(), r.operation);
                if kelson_foundation::OperationId::parse(&operation_id).is_err() {
                    violations.push(GraphViolation {
                        code: "GRAPH-ROUTE-OPID".into(),
                        message: format!(
                            "module {}: route {} {} maps to operation {operation_id:?}, which is not a `module.operation` id of lowercase segments — registration would skip it",
                            m.name(),
                            r.method,
                            r.path
                        ),
                    });
                }
                let key = format!("{} {}", r.method.to_uppercase(), r.path);
                if let Some(name) = &r.name {
                    let method = r.method.to_ascii_uppercase();
                    if let Some((prev_path, prev_op, prev_method)) = route_names.insert(
                        name.clone(),
                        (r.path.clone(), r.operation.clone(), method.clone()),
                    ) {
                        // Laravel parity: resource/singleton expansion registers the
                        // update verb for BOTH PUT and PATCH under one shared route
                        // name (route("notes.update") resolves either). That shared
                        // name is legal only for expansion-generated specs targeting
                        // the same path + operation across the PUT/PATCH alias pair.
                        let alias_pair = r.from_resource
                            && prev_path == r.path
                            && prev_op == r.operation
                            && ((prev_method == "PUT" && method == "PATCH")
                                || (prev_method == "PATCH" && method == "PUT"));
                        if !alias_pair {
                            violations.push(GraphViolation {
                                code: "GRAPH-DUP-ROUTENAME".into(),
                                message: format!(
                                    "module {}: route name {name:?} used by both {prev_path} and {}",
                                    m.name(), r.path
                                ),
                            });
                        }
                    }
                }
                if let Some(prev) = routes_seen.insert(key.clone(), m.name().to_string()) {
                    violations.push(GraphViolation {
                        code: "GRAPH-DUP-ROUTE".into(),
                        message: format!("route {key} declared by {prev} and {}", m.name()),
                    });
                }
                // Route-file routes (Wave 2) never reference the synthetic
                // module's (empty) capabilities: the GRAPH-ROUTEFILE-OP pass
                // below checks them against the graph-wide operations map,
                // once every module's operations are registered.
                if r.from_route_file {
                    continue;
                }
                let op_qualified = if r.operation.contains('.') {
                    r.operation.clone()
                } else {
                    format!("{}.{}", m.name(), r.operation)
                };
                if !m.capabilities.contains_key(r.operation.as_str())
                    && !m
                        .capabilities
                        .contains_key(op_qualified.trim_start_matches(&format!("{}.", m.name())))
                {
                    // route must reference an operation this module declares
                    if !m.capabilities.keys().any(|cap| {
                        let q = format!("{}.{}", m.name(), cap);
                        q == op_qualified || cap == &r.operation
                    }) {
                        violations.push(GraphViolation {
                            code: if r.from_resource {
                                "GRAPH-RESOURCE-OP".into()
                            } else {
                                "GRAPH-ROUTE-OP".into()
                            },
                            message: format!(
                                "module {}: route {key} references undeclared operation {}",
                                m.name(),
                                r.operation
                            ),
                        });
                    }
                }
                if let Some(cap) = &r.capability {
                    if !m.capabilities.contains_key(cap.as_str()) {
                        violations.push(GraphViolation {
                            code: if r.from_resource {
                                "GRAPH-RESOURCE-OP".into()
                            } else {
                                "GRAPH-CAP-MISSING".into()
                            },
                            message: format!(
                                "module {}: route {key} requires undeclared capability {cap}",
                                m.name()
                            ),
                        });
                    }
                }
            }
        }

        // Wave 2 gate GRAPH-ROUTEFILE-OP: root route-file routes must
        // reference operations a real module registers (qualified
        // module.operation against the graph-wide operations map; synthetic
        // route-file modules declare no capabilities of their own). Runs
        // after every module's capabilities are registered. Collect-all,
        // boot-refusing.
        for m in &modules {
            for r in &m.routes {
                if !r.from_route_file {
                    continue;
                }
                let ok = r.operation.contains('.') && operations.contains_key(&r.operation);
                if !ok {
                    violations.push(GraphViolation {
                        code: "GRAPH-ROUTEFILE-OP".into(),
                        message: format!(
                            "module {}: route file route {} {} references undeclared operation {} (expected a qualified module.operation registered by a module)",
                            m.name(),
                            r.method.to_uppercase(),
                            r.path,
                            r.operation
                        ),
                    });
                }
            }
        }

        // Wave 4 Phase 8 gate GRAPH-VERSION-SEMVER (docs/19 L223): every
        // route's `version` parses as `v<N>` (lowercase `v` + digits), and a
        // version-looking path segment (e.g. `/api/v1/...`) must agree with
        // the declared version. The `deprecated` flag rides separately (the
        // Deprecation header/meta, route:list columns, and OpenAPI marker).
        // Collect-all, boot-refusing.
        for m in &modules {
            for r in &m.routes {
                if !is_version_token(&r.version) {
                    violations.push(GraphViolation {
                        code: "GRAPH-VERSION-SEMVER".into(),
                        message: format!(
                            "module {}: route {} {} declares version {:?} which must parse as v<N> (e.g. v1)",
                            m.name(),
                            r.method.to_uppercase(),
                            r.path,
                            r.version
                        ),
                    });
                    continue;
                }
                for segment in r.path.split('/') {
                    if segment != r.version && is_version_token(segment) {
                        violations.push(GraphViolation {
                            code: "GRAPH-VERSION-SEMVER".into(),
                            message: format!(
                                "module {}: route {} {} declares version {} but its path carries conflicting version segment {segment}",
                                m.name(),
                                r.method.to_uppercase(),
                                r.path,
                                r.version
                            ),
                        });
                    }
                }
            }
        }

        // Wave 1 gates: bindings must reference real path params; manifest
        // validation specs must reference declared operations and parseable
        // rule grammar (fail closed, collect-all).
        for m in &modules {
            for r in &m.routes {
                for b in &r.bindings {
                    if !r.path.contains(&format!("{{{}}}", b.param)) {
                        violations.push(GraphViolation {
                            code: "GRAPH-BINDING-PARAM".into(),
                            message: format!(
                                "module {}: route {} {} binds unknown path parameter {{{}}}",
                                m.name(),
                                r.method,
                                r.path,
                                b.param
                            ),
                        });
                    }
                }
            }
            for spec in &m.validation {
                let Some(cap) = m.capabilities.get(spec.operation.as_str()) else {
                    violations.push(GraphViolation {
                        code: "GRAPH-VALIDATION-OP".into(),
                        message: format!(
                            "module {}: [[validation]] references undeclared operation {}",
                            m.name(),
                            spec.operation
                        ),
                    });
                    continue;
                };
                if let Err(errs) = kelson_validation::parse_rules(&spec.rules) {
                    violations.push(GraphViolation {
                        code: "GRAPH-VALIDATION-RULE".into(),
                        message: format!(
                            "module {}: validation for {}.{}: {}",
                            m.name(),
                            spec.operation,
                            spec.field,
                            errs.join("; ")
                        ),
                    });
                }
                // Wave 4 Phase 4 gates (fail closed, collect-all): an
                // unregistered `custom` name (GRAPH-RULE-UNKNOWN), an unknown
                // `messages` key (a typo would silently never fire), and a
                // field outside the declared input shape
                // (GRAPH-VALIDATION-FIELD).
                if let Some(name) = &spec.custom {
                    if !kelson_validation::custom_rule_names().iter().any(|n| n == name) {
                        violations.push(GraphViolation {
                            code: "GRAPH-RULE-UNKNOWN".into(),
                            message: format!(
                                "module {}: validation for {}.{} references unregistered custom rule {} (register it before graph compile)",
                                m.name(),
                                spec.operation,
                                spec.field,
                                name
                            ),
                        });
                    }
                }
                for key in spec.messages.keys() {
                    if !kelson_validation::is_known_rule_name(key) {
                        violations.push(GraphViolation {
                            code: "GRAPH-VALIDATION-RULE".into(),
                            message: format!(
                                "module {}: validation for {}.{}: messages key {key} is not a known rule name",
                                m.name(),
                                spec.operation,
                                spec.field
                            ),
                        });
                    }
                }
                // Declared shape only: an undeclared (empty) input list skips
                // the gate, and the `_`-prefixed injection namespace
                // (_bindings/_auth/_page) is edge-injected, never part of
                // the declared operation input.
                if !cap.inputs.is_empty() {
                    let first = spec.field.split('.').next().unwrap_or(&spec.field);
                    if !first.starts_with('_') && !cap.inputs.iter().any(|i| i == first) {
                        violations.push(GraphViolation {
                            code: "GRAPH-VALIDATION-FIELD".into(),
                            message: format!(
                                "module {}: validation field {} on operation {} is not in the declared input shape (inputs: {})",
                                m.name(),
                                spec.field,
                                spec.operation,
                                cap.inputs.join(", ")
                            ),
                        });
                    }
                }
            }
        }

        // Wave 2 gates: planes, groups, constraints, fallbacks (fail-closed,
        // collect-all, boot-refusing).
        let mut fallbacks_by_plane: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
        for m in modules.iter_mut() {
            let module_name = m.name().to_string();
            for g in &m.route_groups {
                if let Some(p) = &g.plane {
                    if let Err(e) = RoutePlane::from_token(p) {
                        violations.push(GraphViolation {
                            code: "GRAPH-PLANE".into(),
                            message: format!("module {module_name}: route group {}: {e}", g.name),
                        });
                    }
                }
            }
            let group_names: BTreeSet<&str> =
                m.route_groups.iter().map(|g| g.name.as_str()).collect();
            for r in m.routes.iter_mut() {
                let key = format!("{} {}", r.method.to_uppercase(), r.path);
                if let Some(p) = &r.plane {
                    if let Err(e) = RoutePlane::from_token(p) {
                        violations.push(GraphViolation {
                            code: "GRAPH-PLANE".into(),
                            message: format!("module {module_name}: route {key}: {e}"),
                        });
                    }
                }
                if let Some(g) = &r.group {
                    if !group_names.contains(g.as_str()) {
                        violations.push(GraphViolation {
                            code: "GRAPH-GROUP-UNKNOWN".into(),
                            message: format!(
                                "module {module_name}: route {key} references undeclared group {g:?}"
                            ),
                        });
                    }
                }
                r.resolved_constraints.clear();
                for c in &r.constraints {
                    if !r.path.contains(&format!("{{{}}}", c.param)) {
                        violations.push(GraphViolation {
                            code: GRAPH_CONSTRAINT_PARAM.into(),
                            message: format!(
                                "module {module_name}: route {key} constrains unknown path parameter {{{}}}",
                                c.param
                            ),
                        });
                    }
                    match c.resolve() {
                        Ok(resolved) => r.resolved_constraints.push(resolved),
                        Err(e) => violations.push(GraphViolation {
                            code: GRAPH_CONSTRAINT_KIND.into(),
                            message: format!("module {module_name}: route {key}: {e}"),
                        }),
                    }
                }
                if r.fallback {
                    let plane = r.plane.clone().unwrap_or_else(|| "api".into());
                    fallbacks_by_plane
                        .entry(plane)
                        .or_default()
                        .push((module_name.clone(), key));
                }
            }
        }
        for (plane, entries) in &fallbacks_by_plane {
            if entries.len() > 1 {
                let first = format!("{} {}", entries[0].0, entries[0].1);
                for (module, key) in entries.iter().skip(1) {
                    violations.push(GraphViolation {
                        code: "GRAPH-FALLBACK-DUP".into(),
                        message: format!(
                            "module {module}: route {key} is a duplicate fallback on the {plane:?} plane (first: {first}) — max one fallback route per plane"
                        ),
                    });
                }
            }
        }

        // Wave 2 gates: console commands (routes/console.toml) must resolve
        // to registered operations; broadcast channels (routes/channels.toml)
        // must reference capabilities some module declares. Collect-all,
        // boot-refusing.
        for m in &modules {
            for cmd in &m.console_commands {
                let ok = cmd.operation.contains('.') && operations.contains_key(&cmd.operation);
                if !ok {
                    violations.push(GraphViolation {
                        code: "GRAPH-ROUTEFILE-OP".into(),
                        message: format!(
                            "module {}: console command {:?} references undeclared operation {} (expected a qualified module.operation)",
                            m.name(),
                            cmd.name,
                            cmd.operation
                        ),
                    });
                }
            }
            for ch in &m.channels {
                let declared = if ch.capability.contains('.') {
                    operations.contains_key(&ch.capability)
                } else {
                    modules
                        .iter()
                        .any(|mm| mm.capabilities.contains_key(ch.capability.as_str()))
                };
                if !declared {
                    violations.push(GraphViolation {
                        code: "GRAPH-CHANNEL-CAP".into(),
                        message: format!(
                            "module {}: broadcast channel {:?} requires undeclared capability {}",
                            m.name(),
                            ch.name,
                            ch.capability
                        ),
                    });
                }
            }
        }

        // dependency existence + cycles
        let names: BTreeSet<String> = modules.iter().map(|m| m.name().to_string()).collect();
        for m in &modules {
            for dep in &m.dependencies.contracts {
                if !names.contains(dep) {
                    violations.push(GraphViolation {
                        code: "GRAPH-MISSING-DEP".into(),
                        message: format!("module {} depends on unknown module {dep}", m.name()),
                    });
                }
            }
        }
        if let Some(cycle) = find_cycle(&modules) {
            violations.push(GraphViolation {
                code: "GRAPH-CYCLE".into(),
                message: format!("module dependency cycle: {}", cycle.join(" -> ")),
            });
        }

        if violations.is_empty() {
            Ok(Self {
                modules,
                operations,
                events,
            })
        } else {
            Err(violations)
        }
    }

    pub fn module(&self, name: &str) -> Option<&ModuleManifest> {
        self.modules.iter().find(|m| m.name() == name)
    }

    /// Find a route by its declared name within a module (S9 URL generation).
    pub fn route_named(&self, module_name: &str, route_name: &str) -> Option<&RouteSpec> {
        self.module(module_name)
            .and_then(|m| m.routes.iter().find(|r| r.name.as_deref() == Some(route_name)))
    }

    /// Generate a concrete path for a named route (S9 B-07). Every `{param}`
    /// placeholder must be supplied; extra parameters are appended as a
    /// sorted query string (Laravel-style extras). Unknown module/route name
    /// or a missing parameter is a graph violation. Values are percent-encoded.
    ///
    /// Wave 2 URL defaults: placeholders the caller omits fall back to the
    /// module's registered `[url] defaults` (`URL::defaults`); an explicit
    /// value always wins. Defaults only ever fill path placeholders — they
    /// are never appended as query parameters.
    pub fn resolve_url(
        &self,
        module_name: &str,
        route_name: &str,
        params: &BTreeMap<String, String>,
    ) -> Result<String, GraphViolation> {
        let module = self.module(module_name).ok_or_else(|| GraphViolation {
            code: "GRAPH-ROUTE-NAME".into(),
            message: format!("unknown module {module_name:?}"),
        })?;
        let route = module
            .routes
            .iter()
            .find(|r| r.name.as_deref() == Some(route_name))
            .ok_or_else(|| GraphViolation {
                code: "GRAPH-ROUTE-NAME".into(),
                message: format!("module {module_name} has no route named {route_name:?}",),
            })?;
        let mut path = route.path.clone();
        let mut used: BTreeSet<String> = BTreeSet::new();
        for (key, value) in params {
            let placeholder = format!("{{{key}}}");
            if path.contains(&placeholder) {
                path = path.replace(&placeholder, &url_encode(value));
                used.insert(key.clone());
            }
        }
        // Registered defaults fill the leftovers (Wave 2 URL defaults).
        for (key, value) in &module.url.defaults {
            let placeholder = format!("{{{key}}}");
            if path.contains(&placeholder) {
                path = path.replace(&placeholder, &url_encode(value));
            }
        }
        if let Some(start) = path.find('{') {
            let end = path[start..].find('}').map(|i| start + i);
            let missing = end
                .map(|e| path[start + 1..e].to_string())
                .unwrap_or_else(|| path[start + 1..].to_string());
            return Err(GraphViolation {
                code: "GRAPH-ROUTE-NAME".into(),
                message: format!(
                    "route {route_name:?} of module {module_name} is missing parameter {missing:?}"
                ),
            });
        }
        let extras: Vec<String> = params
            .iter()
            .filter(|(key, _)| !used.contains(*key))
            .map(|(key, value)| format!("{}={}", url_encode(key), url_encode(value)))
            .collect();
        if extras.is_empty() {
            return Ok(path);
        }
        let sep = if path.contains('?') { '&' } else { '?' };
        Ok(format!("{path}{sep}{}", extras.join("&")))
    }
}

/// Conservative percent-encoding for URL generation (RFC 3986 unreserved set).
fn url_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn find_cycle(modules: &[ModuleManifest]) -> Option<Vec<String>> {
    fn visit(
        node: &str,
        modules: &BTreeMap<String, Vec<String>>,
        stack: &mut Vec<String>,
        seen: &mut BTreeSet<String>,
    ) -> Option<Vec<String>> {
        if stack.contains(&node.to_string()) {
            let mut cycle = stack[stack.iter().position(|n| n == node).unwrap()..].to_vec();
            cycle.push(node.to_string());
            return Some(cycle);
        }
        if seen.contains(node) {
            return None;
        }
        seen.insert(node.to_string());
        stack.push(node.to_string());
        for dep in modules.get(node).cloned().unwrap_or_default() {
            if let Some(c) = visit(&dep, modules, stack, seen) {
                return Some(c);
            }
        }
        stack.pop();
        None
    }

    let map: BTreeMap<String, Vec<String>> = modules
        .iter()
        .map(|m| (m.name().to_string(), m.dependencies.contracts.clone()))
        .collect();
    let mut seen = BTreeSet::new();
    let mut stack = Vec::new();
    for name in map.keys() {
        if let Some(c) = visit(name, &map, &mut stack, &mut seen) {
            return Some(c);
        }
    }
    None
}

/// Fail-closed boot gate (Wave 1): every route binding's model must have a
/// registered resolver in the well-known `route.bindings` service (modules
/// register resolvers in `Provider::init`). Collect-all, boot-refusing —
/// same reporting shape as the other GRAPH-* gates.
pub fn binding_resolver_violations(
    graph: &PlatformGraph,
    registered_models: &BTreeSet<String>,
) -> Vec<GraphViolation> {
    let mut out = Vec::new();
    for m in &graph.modules {
        for r in &m.routes {
            for b in &r.bindings {
                if !registered_models.contains(&b.model) {
                    out.push(GraphViolation {
                        code: "GRAPH-BINDING-RESOLVER".into(),
                        message: format!(
                            "module {}: route {} {} binds model {} but no resolver is registered in route.bindings",
                            m.name(),
                            r.method,
                            r.path,
                            b.model
                        ),
                    });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(name: &str, extra: &str) -> ModuleManifest {
        let toml = format!(
            r#"
[module]
name = "{name}"
version = "0.1.0"
api = 1

[capabilities]
"create" = {{ profile = "COMMAND" }}

{extra}
"#
        );
        ModuleManifest::from_toml(&toml).expect("manifest should parse")
    }

    #[test]
    fn valid_graph_compiles() {
        let graph =
            PlatformGraph::compile(vec![manifest("alpha", ""), manifest("beta", "")]).unwrap();
        assert_eq!(graph.modules.len(), 2);
        assert!(graph.operations.contains_key("alpha.create"));
    }

    /// Wave 3 Phase 6: `BindingSpec::with_trashed` is opt-in — absent from
    /// existing manifests (defaults false) and settable in both formats.
    #[test]
    fn binding_spec_with_trashed_is_opt_in() {
        #[derive(serde::Deserialize)]
        struct Wrap {
            binding: BindingSpec,
        }
        let bare: Wrap =
            toml::from_str(r#"binding = { param = "id", model = "Thing" }"#).expect("parses");
        assert!(!bare.binding.with_trashed, "absent flag defaults to false");

        let opted: Wrap =
            toml::from_str(r#"binding = { param = "id", model = "Thing", with_trashed = true }"#)
                .expect("parses");
        assert!(opted.binding.with_trashed);

        let json: Wrap =
            serde_json::from_str(r#"{"binding":{"param":"id","model":"Thing","with_trashed":true}}"#)
                .expect("parses");
        assert!(json.binding.with_trashed);
        let json_bare: Wrap = serde_json::from_str(r#"{"binding":{"param":"id","model":"Thing"}}"#)
            .expect("parses");
        assert!(!json_bare.binding.with_trashed);
    }

    #[test]
    fn duplicate_routes_detected() {
        let routes = r#"
[[routes]]
method = "POST"
path = "/api/v1/things"
operation = "create"
"#;
        let graph =
            PlatformGraph::compile(vec![manifest("alpha", routes), manifest("beta", routes)]);
        let v = graph.unwrap_err();
        assert!(v.iter().any(|x| x.code == "GRAPH-DUP-ROUTE"));
    }

    #[test]
    fn dependency_cycle_detected() {
        let ab = r#"
[dependencies]
contracts = ["beta"]
"#;
        let ba = r#"
[dependencies]
contracts = ["alpha"]
"#;
        let graph = PlatformGraph::compile(vec![manifest("alpha", ab), manifest("beta", ba)]);
        let v = graph.unwrap_err();
        assert!(v.iter().any(|x| x.code == "GRAPH-CYCLE"));
    }

    #[test]
    fn missing_dependency_detected() {
        let deps = r#"
[dependencies]
contracts = ["ghost"]
"#;
        let graph = PlatformGraph::compile(vec![manifest("alpha", deps)]);
        let v = graph.unwrap_err();
        assert!(v.iter().any(|x| x.code == "GRAPH-MISSING-DEP"));
    }

    #[test]
    fn route_to_undeclared_operation_detected() {
        let routes = r#"
[[routes]]
method = "POST"
path = "/api/v1/things"
operation = "nonexistent"
"#;
        let graph = PlatformGraph::compile(vec![manifest("alpha", routes)]);
        let v = graph.unwrap_err();
        assert!(v.iter().any(|x| x.code == "GRAPH-ROUTE-OP"));
    }

    fn named_routes_manifest() -> ModuleManifest {
        let routes = r#"
[[routes]]
method = "GET"
path = "/api/v1/things/{id}"
operation = "create"
name = "things.show"

[[routes]]
method = "GET"
path = "/api/v1/things"
operation = "create"
name = "things.index"
"#;
        manifest("alpha", routes)
    }

    #[test]
    fn duplicate_route_name_detected() {
        let routes = r#"
[[routes]]
method = "GET"
path = "/a"
operation = "create"
name = "same"

[[routes]]
method = "GET"
path = "/b"
operation = "create"
name = "same"
"#;
        let graph = PlatformGraph::compile(vec![manifest("alpha", routes)]);
        let v = graph.unwrap_err();
        assert!(v.iter().any(|x| x.code == "GRAPH-DUP-ROUTENAME"));
    }

    #[test]
    fn resolve_url_substitutes_and_encodes_parameters() {
        let graph = PlatformGraph::compile(vec![named_routes_manifest()]).unwrap();
        let mut params = BTreeMap::new();
        params.insert("id".into(), "weird id&x".into());
        let url = graph.resolve_url("alpha", "things.show", &params).unwrap();
        assert_eq!(url, "/api/v1/things/weird%20id%26x");
        assert_eq!(
            graph.resolve_url("alpha", "things.index", &BTreeMap::new()).unwrap(),
            "/api/v1/things"
        );
    }

    #[test]
    fn resolve_url_extra_params_become_query_string() {
        let graph = PlatformGraph::compile(vec![named_routes_manifest()]).unwrap();
        let mut params = BTreeMap::new();
        params.insert("id".into(), "42".into());
        params.insert("page".into(), "2".into());
        let url = graph.resolve_url("alpha", "things.show", &params).unwrap();
        assert_eq!(url, "/api/v1/things/42?page=2");
    }

    #[test]
    fn resolve_url_rejects_unknown_route_and_missing_params() {
        let graph = PlatformGraph::compile(vec![named_routes_manifest()]).unwrap();
        let err = graph
            .resolve_url("alpha", "ghost", &BTreeMap::new())
            .unwrap_err();
        assert_eq!(err.code, "GRAPH-ROUTE-NAME");
        let err = graph
            .resolve_url("alpha", "things.show", &BTreeMap::new())
            .unwrap_err();
        assert_eq!(err.code, "GRAPH-ROUTE-NAME");
        assert!(err.message.contains("id"));
    }

    /// Wave 2 URL defaults (Laravel `URL::defaults`): a registered default
    /// fills an omitted path parameter, an explicit value beats it, and a
    /// default never leaks into the query string of another route.
    #[test]
    fn resolve_url_falls_back_to_registered_url_defaults() {
        let toml = r#"
[module]
name = "alpha"
version = "0.1.0"
api = 1

[capabilities]
"create" = { profile = "COMMAND" }

[url]
defaults = { locale = "en" }

[[routes]]
method = "GET"
path = "/api/v1/{locale}/things/{id}"
operation = "create"
name = "things.show"

[[routes]]
method = "GET"
path = "/api/v1/things"
operation = "create"
name = "things.index"
"#;
        let graph =
            PlatformGraph::compile(vec![ModuleManifest::from_toml(toml).expect("manifest parses")])
                .expect("graph compiles");
        assert_eq!(
            graph.module("alpha").expect("alpha").url.defaults["locale"],
            "en"
        );

        // Omitted placeholder falls back to the registered default.
        let mut params = BTreeMap::new();
        params.insert("id".into(), "7".into());
        assert_eq!(
            graph.resolve_url("alpha", "things.show", &params).unwrap(),
            "/api/v1/en/things/7"
        );

        // The explicit value wins over the registered default.
        params.insert("locale".into(), "fr".into());
        assert_eq!(
            graph.resolve_url("alpha", "things.show", &params).unwrap(),
            "/api/v1/fr/things/7"
        );

        // Defaults fill path placeholders only — never an extra query param.
        assert_eq!(
            graph
                .resolve_url("alpha", "things.index", &BTreeMap::new())
                .unwrap(),
            "/api/v1/things"
        );
    }

    /// A placeholder with no supplied value and no registered default stays a
    /// graph violation (defaults relax generation, they do not hide bugs).
    #[test]
    fn resolve_url_still_requires_params_that_have_no_default() {
        let toml = r#"
[module]
name = "alpha"
version = "0.1.0"
api = 1

[capabilities]
"create" = { profile = "COMMAND" }

[url]
defaults = { locale = "en" }

[[routes]]
method = "GET"
path = "/api/v1/{locale}/things/{id}"
operation = "create"
name = "things.show"
"#;
        let graph =
            PlatformGraph::compile(vec![ModuleManifest::from_toml(toml).expect("manifest parses")])
                .expect("graph compiles");
        let err = graph
            .resolve_url("alpha", "things.show", &BTreeMap::new())
            .unwrap_err();
        assert_eq!(err.code, "GRAPH-ROUTE-NAME");
        assert!(err.message.contains("id"), "{:?}", err.message);
    }

    // ------------------------------------------------ S10 B-18: module ABI

    #[test]
    fn manifest_with_explicit_api_compiles() {
        let m = manifest("abiexplicit", "");
        assert_eq!(m.module.api, MODULE_ABI);
        PlatformGraph::compile(vec![m]).expect("explicit ABI compiles");
    }

    #[test]
    fn manifest_without_api_fails_to_parse() {
        let toml = r#"
[module]
name = "noapi"
version = "0.1.0"

[capabilities]
"create" = { profile = "COMMAND" }
"#;
        let err = ModuleManifest::from_toml(toml).expect_err("missing api must fail");
        assert!(err.message.contains("missing field `api`") || err.message.contains("api"));
    }

    #[test]
    fn mismatched_module_abi_is_rejected_by_the_gate() {
        let toml = format!(
            r#"
[module]
name = "futuremod"
version = "9.9.9"
api = {}

[capabilities]
"create" = {{ profile = "COMMAND" }}
"#,
            MODULE_ABI + 1
        );
        let m = ModuleManifest::from_toml(&toml).expect("parses");
        let violations = PlatformGraph::compile(vec![m]).unwrap_err();
        assert!(
            violations.iter().any(|v| v.code == "GRAPH-MANIFEST-ABI"),
            "ABI mismatch must fail the graph gate: {violations:?}"
        );
    }

    // ------------------------------------------------ Wave 1: resources

    fn resource_manifest(extra_caps: &str, route_extra: &str) -> ModuleManifest {
        let toml = format!(
            r#"
[module]
name = "notes"
version = "0.1.0"
api = 1

[capabilities]
"index" = {{ profile = "READ" }}
"show" = {{ profile = "READ" }}
"store" = {{ profile = "COMMAND" }}
"update" = {{ profile = "COMMAND" }}
"destroy" = {{ profile = "COMMAND" }}
{extra_caps}

[[routes]]
resource = "notes"
{route_extra}
"#
        );
        ModuleManifest::from_toml(&toml).expect("resource manifest parses")
    }

    #[test]
    fn resource_expands_into_six_route_specs() {
        let m = resource_manifest("", "");
        assert_eq!(m.routes.len(), 6, "update expands to PUT + PATCH");
        let pairs: Vec<(String, String)> = m
            .routes
            .iter()
            .map(|r| (r.method.clone(), r.path.clone()))
            .collect();
        assert!(pairs.contains(&("GET".into(), "/notes".into())));
        assert!(pairs.contains(&("GET".into(), "/notes/{id}".into())));
        assert!(pairs.contains(&("POST".into(), "/notes".into())));
        assert!(pairs.contains(&("PUT".into(), "/notes/{id}".into())));
        assert!(pairs.contains(&("PATCH".into(), "/notes/{id}".into())));
        assert!(pairs.contains(&("DELETE".into(), "/notes/{id}".into())));
        let names: Vec<&str> = m.routes.iter().map(|r| r.name.as_deref().unwrap()).collect();
        assert!(names.contains(&"notes.index"));
        assert!(names.contains(&"notes.destroy"));
        // update shares one operation across two specs
        let updates: Vec<&RouteSpec> =
            m.routes.iter().filter(|r| r.operation == "update").collect();
        assert_eq!(updates.len(), 2);
        assert!(updates.iter().all(|r| r.from_resource));
    }

    #[test]
    fn resource_only_and_except_filter_verbs() {
        let m = resource_manifest("", "only = [\"index\", \"show\"]");
        assert_eq!(m.routes.len(), 2);
        let m = resource_manifest("", "except = [\"destroy\", \"update\"]");
        assert_eq!(m.routes.len(), 3, "update excluded removes both PUT+PATCH");
        assert!(!m.routes.iter().any(|r| r.operation == "update"));
    }

    // ------------------------------------------------ Wave 2: singleton

    fn singleton_manifest(route_extra: &str) -> ModuleManifest {
        let toml = format!(
            r#"
[module]
name = "profile"
version = "0.1.0"
api = 1

[capabilities]
"show" = {{ profile = "READ" }}
"update" = {{ profile = "COMMAND" }}
"destroy" = {{ profile = "COMMAND" }}

[[routes]]
singleton = true
resource = "profile"
{route_extra}
"#
        );
        ModuleManifest::from_toml(&toml).expect("singleton manifest parses")
    }

    #[test]
    fn singleton_expands_without_id_into_four_route_specs() {
        let m = singleton_manifest("");
        assert_eq!(
            m.routes.len(),
            4,
            "show + update (PUT/PATCH) + destroy"
        );
        let pairs: Vec<(String, String)> = m
            .routes
            .iter()
            .map(|r| (r.method.clone(), r.path.clone()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                ("GET".into(), "/profile".into()),
                ("PUT".into(), "/profile".into()),
                ("PATCH".into(), "/profile".into()),
                ("DELETE".into(), "/profile".into()),
            ],
            "one instance per path — never an {{id}} segment"
        );
        let names: Vec<&str> = m.routes.iter().map(|r| r.name.as_deref().unwrap()).collect();
        assert_eq!(
            names,
            vec![
                "profile.show",
                "profile.update",
                "profile.update",
                "profile.destroy"
            ],
            "the update alias pair shares one route name"
        );
        // The expanded set still clears the graph gates (DUP-ROUTENAME alias,
        // GRAPH-RESOURCE-OP capabilities).
        PlatformGraph::compile(vec![m]).expect("singleton manifest compiles");
    }

    #[test]
    fn singleton_only_and_except_filter_verbs() {
        let m = singleton_manifest("only = [\"show\"]");
        assert_eq!(m.routes.len(), 1);
        assert_eq!(m.routes[0].method, "GET");
        let m = singleton_manifest("except = [\"destroy\", \"update\"]");
        assert_eq!(m.routes.len(), 1, "update excluded removes PUT + PATCH");
    }

    #[test]
    fn singleton_without_a_resource_path_is_rejected() {
        let toml = r#"
[module]
name = "profile"
version = "0.1.0"
api = 1

[capabilities]
"show" = { profile = "READ" }

[[routes]]
singleton = true
method = "GET"
path = "/profile"
operation = "show"
"#;
        let err = ModuleManifest::from_toml(toml).unwrap_err();
        assert_eq!(err.code, "GRAPH-MANIFEST");
        assert!(err.message.contains("singleton"), "{}", err.message);
    }

    #[test]
    fn resource_missing_capability_fails_with_resource_gate() {
        let toml = r#"
[module]
name = "notes"
version = "0.1.0"
api = 1

[capabilities]
"index" = { profile = "READ" }

[[routes]]
resource = "notes"
"#;
        let m = ModuleManifest::from_toml(toml).expect("parses");
        let v = PlatformGraph::compile(vec![m]).unwrap_err();
        assert!(
            v.iter().any(|x| x.code == "GRAPH-RESOURCE-OP"),
            "resource verb without capability must fail closed: {v:?}"
        );
        assert!(!v.iter().any(|x| x.code == "GRAPH-CAP-MISSING"));
    }

    #[test]
    fn resource_with_explicit_method_is_rejected() {
        let toml = r#"
[module]
name = "notes"
version = "0.1.0"
api = 1

[capabilities]
"index" = { profile = "READ" }

[[routes]]
resource = "notes"
method = "GET"
"#;
        let err = ModuleManifest::from_toml(toml).unwrap_err();
        assert_eq!(err.code, "GRAPH-MANIFEST");
    }

    #[test]
    fn explicit_route_missing_operation_is_rejected() {
        let toml = r#"
[module]
name = "notes"
version = "0.1.0"
api = 1

[capabilities]
"index" = { profile = "READ" }

[[routes]]
method = "GET"
path = "/notes"
"#;
        let err = ModuleManifest::from_toml(toml).unwrap_err();
        assert_eq!(err.code, "GRAPH-MANIFEST");
    }

    #[test]
    fn full_resource_manifest_compiles_through_the_graph() {
        // Regression: resource expansion emits PUT + PATCH for update under one
        // shared route name (notes.update). The DUP-ROUTENAME gate must allow
        // exactly that alias pair, so a full resource manifest compiles.
        let m = resource_manifest("", "");
        let g = PlatformGraph::compile(vec![m])
            .expect("PUT+PATCH update pair shares one route name (Laravel semantics)");
        assert_eq!(g.modules[0].routes.len(), 6);
    }

    // ------------------------------------------------ Wave 1: validation specs

    #[test]
    fn validation_specs_compile_and_gate_unknown_ops() {
        let toml = r#"
[module]
name = "alpha"
version = "0.1.0"
api = 1

[capabilities]
"create" = { profile = "COMMAND" }

[[routes]]
method = "POST"
path = "/api/v1/things"
operation = "create"

[[validation]]
operation = "create"
field = "patient"
rules = ["required", "max:500"]
"#;
        let m = ModuleManifest::from_toml(toml).expect("parses");
        PlatformGraph::compile(vec![m]).expect("valid spec compiles");

        let bad = r#"
[module]
name = "alpha"
version = "0.1.0"
api = 1

[capabilities]
"create" = { profile = "COMMAND" }

[[validation]]
operation = "ghost"
field = "x"
rules = ["required"]
"#;
        let m = ModuleManifest::from_toml(bad).expect("parses");
        let v = PlatformGraph::compile(vec![m]).unwrap_err();
        assert!(v.iter().any(|x| x.code == "GRAPH-VALIDATION-OP"));
    }

    #[test]
    fn validation_rule_grammar_is_fail_closed() {
        let toml = r#"
[module]
name = "alpha"
version = "0.1.0"
api = 1

[capabilities]
"create" = { profile = "COMMAND" }

[[validation]]
operation = "create"
field = "kind"
rules = ["one_of:self,family", "bogus"]

[[validation]]
operation = "create"
field = "age"
rules = ["range:1"]
"#;
        let m = ModuleManifest::from_toml(toml).expect("parses");
        let v = PlatformGraph::compile(vec![m]).unwrap_err();
        let rule_errors: Vec<&GraphViolation> = v
            .iter()
            .filter(|x| x.code == "GRAPH-VALIDATION-RULE")
            .collect();
        assert_eq!(rule_errors.len(), 2, "collect-all reports both specs");
    }

    // ------------------------------------------- Wave 4 Phase 4: new gates

    #[test]
    fn validation_field_gate_checks_the_declared_input_shape() {
        // Declared shape covers the field (and its wildcard first segment).
        let good = r#"
[module]
name = "alpha"
version = "0.1.0"
api = 1

[capabilities]
"create" = { profile = "COMMAND", inputs = ["patient", "tags"] }

[[validation]]
operation = "create"
field = "patient"
rules = ["required"]

[[validation]]
operation = "create"
field = "tags.*.name"
rules = ["required"]
"#;
        let m = ModuleManifest::from_toml(good).expect("parses");
        PlatformGraph::compile(vec![m]).expect("declared fields compile");

        // A field outside the declared shape is a boot violation.
        let bad = r#"
[module]
name = "alpha"
version = "0.1.0"
api = 1

[capabilities]
"create" = { profile = "COMMAND", inputs = ["patient"] }

[[validation]]
operation = "create"
field = "ghost"
rules = ["required"]
"#;
        let m = ModuleManifest::from_toml(bad).expect("parses");
        let v = PlatformGraph::compile(vec![m]).unwrap_err();
        assert!(
            v.iter().any(|x| x.code == "GRAPH-VALIDATION-FIELD"),
            "{v:?}"
        );

        // Undeclared shape (inputs absent/empty) → the gate skips.
        let undeclared = r#"
[module]
name = "alpha"
version = "0.1.0"
api = 1

[capabilities]
"create" = { profile = "COMMAND" }

[[validation]]
operation = "create"
field = "ghost"
rules = ["required"]
"#;
        let m = ModuleManifest::from_toml(undeclared).expect("parses");
        PlatformGraph::compile(vec![m]).expect("undeclared shape skips the gate");

        // The `_`-prefixed injection namespace is never checked against the
        // declared input (the edge injects _bindings/_auth itself).
        let injected = r#"
[module]
name = "alpha"
version = "0.1.0"
api = 1

[capabilities]
"create" = { profile = "COMMAND", inputs = ["patient"] }

[[validation]]
operation = "create"
field = "_bindings.id"
rules = ["required"]
"#;
        let m = ModuleManifest::from_toml(injected).expect("parses");
        PlatformGraph::compile(vec![m]).expect("injected fields skip the shape gate");
    }

    #[test]
    fn unknown_custom_rule_names_are_refused() {
        struct Noop;
        impl kelson_validation::CustomRule for Noop {
            fn check(
                &self,
                _field: &str,
                _value: &serde_json::Value,
                _input: &serde_json::Value,
            ) -> Option<String> {
                None
            }
        }

        let toml = r#"
[module]
name = "alpha"
version = "0.1.0"
api = 1

[capabilities]
"create" = { profile = "COMMAND" }

[[validation]]
operation = "create"
field = "code"
rules = ["required"]
custom = "unregistered_phase4_rule"
"#;
        let m = ModuleManifest::from_toml(toml).expect("parses");
        let v = PlatformGraph::compile(vec![m]).unwrap_err();
        assert!(
            v.iter().any(|x| x.code == "GRAPH-RULE-UNKNOWN"),
            "{v:?}"
        );

        // Registered names pass (modules register before graph compile).
        kelson_validation::register_custom_rule("registered_phase4_rule", std::sync::Arc::new(Noop));
        let toml = toml.replace("unregistered_phase4_rule", "registered_phase4_rule");
        let m = ModuleManifest::from_toml(&toml).expect("parses");
        PlatformGraph::compile(vec![m]).expect("registered custom rule compiles");
    }

    #[test]
    fn messages_keys_must_be_known_rule_names() {
        let good = r#"
[module]
name = "alpha"
version = "0.1.0"
api = 1

[capabilities]
"create" = { profile = "COMMAND" }

[[validation]]
operation = "create"
field = "patient"
rules = ["required"]
messages = { required = "the {attribute} is mandatory" }
attribute = "Patient name"
"#;
        let m = ModuleManifest::from_toml(good).expect("parses");
        PlatformGraph::compile(vec![m]).expect("known messages key compiles");

        let bad = r#"
[module]
name = "alpha"
version = "0.1.0"
api = 1

[capabilities]
"create" = { profile = "COMMAND" }

[[validation]]
operation = "create"
field = "patient"
rules = ["required"]
messages = { requird = "typo never fires" }
"#;
        let m = ModuleManifest::from_toml(bad).expect("parses");
        let v = PlatformGraph::compile(vec![m]).unwrap_err();
        assert!(
            v.iter().any(|x| x.code == "GRAPH-VALIDATION-RULE"
                && x.message.contains("requird")),
            "{v:?}"
        );
    }

    // ------------------------------------------------ Wave 1: bindings

    #[test]
    fn binding_param_must_exist_in_path() {
        let toml = r#"
[module]
name = "alpha"
version = "0.1.0"
api = 1

[capabilities]
"create" = { profile = "COMMAND" }

[[routes]]
method = "GET"
path = "/api/v1/things"
operation = "create"

[[routes.bindings]]
param = "id"
model = "Thing"
"#;
        let m = ModuleManifest::from_toml(toml).expect("parses");
        let v = PlatformGraph::compile(vec![m]).unwrap_err();
        assert!(v.iter().any(|x| x.code == "GRAPH-BINDING-PARAM"));
    }

    #[test]
    fn binding_on_real_param_compiles() {
        let toml = r#"
[module]
name = "alpha"
version = "0.1.0"
api = 1

[capabilities]
"create" = { profile = "COMMAND" }

[[routes]]
method = "GET"
path = "/api/v1/things/{id}"
operation = "create"

[[routes.bindings]]
param = "id"
model = "Thing"
"#;
        let m = ModuleManifest::from_toml(toml).expect("parses");
        PlatformGraph::compile(vec![m]).expect("valid binding compiles");
    }

    // ------------------------------------------------ Wave 1: additive defaults

    #[test]
    fn existing_manifests_keep_defaults() {
        let m = manifest("plain", "");
        let r = RouteSpec {
            method: "GET".into(),
            path: "/x".into(),
            operation: "create".into(),
            ..Default::default()
        };
        assert!(!r.public && !r.deprecated && !r.from_resource);
        assert_eq!(r.response, ResponseKind::Json);
        assert!(r.middleware.is_empty() && r.bindings.is_empty());
        drop(m);
    }

    // ------------------------------------- Wave 2: response kinds + caching

    #[test]
    fn response_kinds_and_cache_policies_parse_from_manifest_tokens() {
        let m = manifest(
            "kinds",
            r#"
[[routes]]
method = "GET"
path = "/a"
operation = "create"
response = "no_content"

[[routes]]
method = "GET"
path = "/b"
operation = "create"
response = "file"

[[routes]]
method = "GET"
path = "/c"
operation = "create"
response = "redirect"

[[routes]]
method = "GET"
path = "/d"
operation = "create"
response = "stream"

[[routes]]
method = "GET"
path = "/e"
operation = "create"
response = "download"
cache = { policy = "public", max_age_secs = 60, stale_while_revalidate_secs = 30 }

[[routes]]
method = "POST"
path = "/f"
operation = "create"
cache = { policy = "no_store" }
"#,
        );
        let route = |path: &str| {
            m.routes
                .iter()
                .find(|r| r.path == path)
                .unwrap_or_else(|| panic!("route {path} declared"))
        };
        assert_eq!(route("/a").response, ResponseKind::NoContent);
        assert_eq!(
            route("/b").response,
            ResponseKind::Download,
            "`file` is an accepted alias for `download`"
        );
        assert_eq!(route("/c").response, ResponseKind::Redirect);
        assert_eq!(route("/d").response, ResponseKind::Stream);
        assert_eq!(route("/e").response, ResponseKind::Download);
        assert_eq!(route("/a").cache, None, "absent policy stays absent");
        assert_eq!(
            route("/e").cache,
            Some(CacheSpec {
                policy: CachePolicy::Public,
                max_age_secs: Some(60),
                stale_while_revalidate_secs: Some(30),
            })
        );
        assert_eq!(
            route("/f").cache.map(|spec| spec.policy),
            Some(CachePolicy::NoStore)
        );
        drop(m);
    }

    #[test]
    fn unknown_response_kinds_and_cache_policies_are_refused_at_parse() {
        let shape = |extra: &str| {
            format!(
                r#"
[module]
name = "kinds"
version = "0.1.0"
api = 1

[capabilities]
"create" = {{ profile = "COMMAND" }}

[[routes]]
method = "GET"
path = "/a"
operation = "create"
{extra}
"#
            )
        };
        let err = ModuleManifest::from_toml(&shape("response = \"jsonish\""))
            .expect_err("an unknown wire kind is not a route");
        assert_eq!(err.code, "GRAPH-MANIFEST");
        assert!(
            err.message.contains("jsonish"),
            "the token is named in the parse error: {}",
            err.message
        );
        let err = ModuleManifest::from_toml(&shape("cache = { policy = \"sometimes\" }"))
            .expect_err("cache policies are a closed set");
        assert_eq!(err.code, "GRAPH-MANIFEST");
        assert!(
            err.message.contains("sometimes"),
            "the token is named in the parse error: {}",
            err.message
        );
    }

    // ------------------------------------------------ Wave 2: graph gates

    #[test]
    fn plane_gate_rejects_unknown_plane_tokens() {
        let routes = r#"
[[routes]]
method = "GET"
path = "/api/v1/things"
operation = "create"
plane = "admin"

[[route_groups]]
name = "g"
plane = "intranet"
"#;
        let v = PlatformGraph::compile(vec![manifest("alpha", routes)]).unwrap_err();
        let plane_violations: Vec<_> = v.iter().filter(|x| x.code == "GRAPH-PLANE").collect();
        // Collect-all: BOTH the route plane and the group plane are reported.
        assert_eq!(plane_violations.len(), 2, "{v:?}");
        assert!(plane_violations.iter().any(|x| x.message.contains("admin")));
        assert!(plane_violations.iter().any(|x| x.message.contains("intranet")));
    }

    #[test]
    fn valid_planes_compile_and_default_is_api() {
        let routes = r#"
[[routes]]
method = "GET"
path = "/api/v1/things"
operation = "create"
plane = "web"
"#;
        let graph = PlatformGraph::compile(vec![manifest("alpha", routes)]).unwrap();
        assert_eq!(
            graph.modules[0].routes[0].plane.as_deref(),
            Some("web")
        );
    }

    #[test]
    fn group_unknown_gate_collects_all() {
        let routes = r#"
[[routes]]
method = "GET"
path = "/api/v1/things"
operation = "create"
group = "ghost"

[[routes]]
method = "POST"
path = "/api/v1/things"
operation = "create"
group = "phantom"
"#;
        let v = PlatformGraph::compile(vec![manifest("alpha", routes)]).unwrap_err();
        let group_violations: Vec<_> =
            v.iter().filter(|x| x.code == "GRAPH-GROUP-UNKNOWN").collect();
        assert_eq!(group_violations.len(), 2, "{v:?}");
    }

    #[test]
    fn declared_group_applies_and_compiles() {
        let routes = r#"
[[route_groups]]
name = "admin"
prefix = "/admin"
plane = "web"

[[routes]]
method = "GET"
path = "/things"
operation = "create"
group = "admin"
"#;
        let graph = PlatformGraph::compile(vec![manifest("alpha", routes)]).unwrap();
        let r = &graph.modules[0].routes[0];
        assert_eq!(r.path, "/admin/things");
        assert_eq!(r.plane.as_deref(), Some("web"));
    }

    #[test]
    fn constraint_param_gate_rejects_param_not_in_path() {
        let routes = r#"
[[routes]]
method = "GET"
path = "/api/v1/things"
operation = "create"
constraints = [{ param = "id", kind = "number" }]
"#;
        let v = PlatformGraph::compile(vec![manifest("alpha", routes)]).unwrap_err();
        assert!(v.iter().any(|x| x.code == "GRAPH-CONSTRAINT-PARAM"), "{v:?}");
    }

    #[test]
    fn constraint_kind_gate_collects_all_bad_specs() {
        let routes = r#"
[[routes]]
method = "GET"
path = "/api/v1/things/{id}/{ref}"
operation = "create"
constraints = [
  { param = "id", kind = "regex" },
  { param = "ref", kind = "pattern" },
]
"#;
        let v = PlatformGraph::compile(vec![manifest("alpha", routes)]).unwrap_err();
        // Collect-all: unknown kind AND pattern-without-pattern are both
        // GRAPH-CONSTRAINT-KIND violations in one boot refusal.
        let kind_violations: Vec<_> =
            v.iter().filter(|x| x.code == "GRAPH-CONSTRAINT-KIND").collect();
        assert_eq!(kind_violations.len(), 2, "{v:?}");
    }

    #[test]
    fn valid_constraints_resolve_onto_the_route() {
        let routes = r#"
[[routes]]
method = "GET"
path = "/api/v1/things/{id}/{slug}"
operation = "create"
constraints = [
  { param = "id", kind = "uuid" },
  { param = "slug", kind = "pattern", pattern = "doc-[0-9]{4}" },
]
"#;
        let graph = PlatformGraph::compile(vec![manifest("alpha", routes)]).unwrap();
        let r = &graph.modules[0].routes[0];
        assert_eq!(r.resolved_constraints.len(), 2);
        assert_eq!(r.resolved_constraints[0].kind, ConstraintKind::Uuid);
        assert!(r.resolved_constraints[1].pattern.is_some());
    }

    #[test]
    fn fallback_dup_gate_allows_one_per_plane() {
        let routes = r#"
[[routes]]
method = "GET"
path = "/catch-api"
operation = "create"
fallback = true

[[routes]]
method = "GET"
path = "/catch-web"
operation = "create"
fallback = true
plane = "web"
"#;
        // One fallback per plane -> compiles.
        PlatformGraph::compile(vec![manifest("alpha", routes)]).expect("one per plane compiles");
    }

    #[test]
    fn fallback_dup_gate_rejects_two_on_one_plane() {
        let routes = r#"
[[routes]]
method = "GET"
path = "/catch-one"
operation = "create"
fallback = true
plane = "web"

[[routes]]
method = "GET"
path = "/catch-two"
operation = "create"
fallback = true
plane = "web"
"#;
        let v = PlatformGraph::compile(vec![manifest("alpha", routes)]).unwrap_err();
        assert!(v.iter().any(|x| x.code == "GRAPH-FALLBACK-DUP"), "{v:?}");
    }

    #[test]
    fn routefile_op_gate_requires_registered_module_operations() {
        let file = RouteFileManifest::from_toml(
            r#"
[[route]]
method = "GET"
path = "/web/ok"
operation = "alpha.create"

[[route]]
method = "GET"
path = "/web/bad"
operation = "alpha.missing"

[[route]]
method = "GET"
path = "/web/unqualified"
operation = "create"
"#,
            "routes/web.toml",
            Some("web"),
        )
        .expect("route file parses");
        let v = PlatformGraph::compile(vec![
            manifest("alpha", ""),
            file.into_module("routes_web"),
        ])
        .unwrap_err();
        let rf: Vec<_> = v.iter().filter(|x| x.code == "GRAPH-ROUTEFILE-OP").collect();
        // Collect-all: both the unknown AND the unqualified op are reported;
        // the valid alpha.create route passes.
        assert_eq!(rf.len(), 2, "{v:?}");
    }

    #[test]
    fn routefile_routes_with_registered_ops_compile() {
        let file = RouteFileManifest::from_toml(
            r#"
[[route]]
method = "GET"
path = "/web/things"
operation = "alpha.create"
"#,
            "routes/web.toml",
            Some("web"),
        )
        .expect("route file parses");
        let graph = PlatformGraph::compile(vec![
            manifest("alpha", ""),
            file.into_module("routes_web"),
        ])
        .expect("compiles");
        let route_file_module = graph.module("routes_web").unwrap();
        assert_eq!(route_file_module.routes[0].plane.as_deref(), Some("web"));
        assert!(route_file_module.routes[0].from_route_file);
    }

    #[test]
    fn console_commands_and_channels_gate() {
        let file = RouteFileManifest::from_toml(
            r#"
[[command]]
name = "things:sync"
operation = "alpha.create"

[[command]]
name = "things:broken"
operation = "alpha.missing"

[[channel]]
name = "things.{id}"
capability = "create"

[[channel]]
name = "ghost.{id}"
capability = "ghost.cap"
"#,
            "routes/console.toml",
            None,
        )
        .expect("route file parses");
        let v = PlatformGraph::compile(vec![
            manifest("alpha", ""),
            file.into_module("routes_console"),
        ])
        .unwrap_err();
        assert!(v.iter().any(|x| x.code == "GRAPH-ROUTEFILE-OP"), "{v:?}");
        assert!(v.iter().any(|x| x.code == "GRAPH-CHANNEL-CAP"), "{v:?}");
    }

    /// Wave 4 Phase 8 (docs/19 L223): GRAPH-VERSION-SEMVER — every route's
    /// `version` parses as `v<N>`, and a version-looking path segment must
    /// agree with it. The matching v2 route and the segment-less routes pass;
    /// the unparseable token and the mismatched segment are both reported
    /// (collect-all).
    #[test]
    fn graph_version_semver_gate_rejects_bad_tokens_and_segment_mismatches() {
        let routes = r#"
[[routes]]
method = "GET"
path = "/api/v2/alpha"
operation = "create"
version = "v2"

[[routes]]
method = "POST"
path = "/beta"
operation = "create"
version = "1.0"

[[routes]]
method = "GET"
path = "/api/v2/gamma"
operation = "create"
version = "v1"

[[routes]]
method = "DELETE"
path = "/delta"
operation = "create"
"#;
        let v: Vec<_> = PlatformGraph::compile(vec![manifest("alpha", routes)])
            .expect_err("two version violations refuse the graph")
            .into_iter()
            .filter(|x| x.code == "GRAPH-VERSION-SEMVER")
            .collect();
        assert_eq!(v.len(), 2, "{v:?}");
        assert!(
            v.iter().any(|x| x.message.contains("1.0")),
            "bad token reported: {v:?}"
        );
        assert!(
            v.iter()
                .any(|x| x.message.contains("conflicting version segment")),
            "segment mismatch reported: {v:?}"
        );

        // Routes whose versions parse and agree compile clean.
        let ok = r#"
[[routes]]
method = "GET"
path = "/api/v2/alpha"
operation = "create"
version = "v2"

[[routes]]
method = "GET"
path = "/notes"
operation = "create"
"#;
        PlatformGraph::compile(vec![manifest("alpha", ok)]).expect("clean versions compile");
    }

    #[test]
    fn wave2_gates_collect_all_in_one_refusal() {
        let routes = r#"
[[routes]]
method = "GET"
path = "/api/v1/things/{id}"
operation = "create"
plane = "admin"
group = "ghost"
constraints = [{ param = "missing", kind = "number" }]
"#;
        let v = PlatformGraph::compile(vec![manifest("alpha", routes)]).unwrap_err();
        assert!(v.iter().any(|x| x.code == "GRAPH-PLANE"), "{v:?}");
        assert!(v.iter().any(|x| x.code == "GRAPH-GROUP-UNKNOWN"), "{v:?}");
        assert!(v.iter().any(|x| x.code == "GRAPH-CONSTRAINT-PARAM"), "{v:?}");
    }

    #[test]
    fn p0_route_shape_gates_refuse_edge_skippable_routes() {
        // An unsupported method and an unparseable `{module}.{operation}` id
        // are exactly the two routes the HTTP edge skips at registration
        // (warn-only). The compile refuses both, collect-all, so a boot graph
        // can never hold a route that would silently never register.
        let routes = r#"
[[routes]]
method = "PURGE"
path = "/api/v1/things"
operation = "create"

[[routes]]
method = "GET"
path = "/api/v1/other"
operation = "DoThing"
"#;
        let v = PlatformGraph::compile(vec![manifest("alpha", routes)]).unwrap_err();
        assert!(
            v.iter().any(|x| x.code == "GRAPH-ROUTE-METHOD" && x.message.contains("PURGE")),
            "{v:?}"
        );
        assert!(
            v.iter()
                .any(|x| x.code == "GRAPH-ROUTE-OPID" && x.message.contains("alpha.DoThing")),
            "{v:?}"
        );
    }

    #[test]
    fn p0_route_shape_gate_accepts_every_registered_verb() {
        // The edge uppercases the manifest token before matching, so a
        // lowercase declaration registers — and must compile.
        let routes = r#"
[[routes]]
method = "get"
path = "/api/v1/things"
operation = "create"
"#;
        PlatformGraph::compile(vec![manifest("alpha", routes)]).expect("registered verbs compile");
    }
}