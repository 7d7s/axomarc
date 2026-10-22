//! Graph-driven HTTP edge (S7 C1/B-01, Wave 1 middleware model): compiles
//! PlatformGraph from all applications/*/module.toml manifests and registers
//! one axum route per RouteSpec with its fully-resolved middleware chain
//! attached at registration time. Declared module routes become executable
//! HTTP endpoints dispatched through the CommandBus.
//!
//! Dispatch contract (Wave 1):
//! - identity resolution lives in the route's middleware chain (session ->
//!   bearer PAT -> opt-in JWT via the auth stage); dispatch consumes the
//!   installed ResolvedPrincipal extension
//! - anonymous reach is only possible on public routes whose handler plan
//!   does not require authorization (401 envelopes otherwise)
//! - declared {param} model bindings resolve tenant-scoped through the
//!   route.bindings registry and are injected as _bindings.<param> AFTER
//!   the path/query/body merge (bodies cannot forge them); misses are 404
//! - GET routes parse page/per_page (clamped) and inject _page
//! - manifest [[validation]] rules run collect-all over the merged input
//!   before dispatch (422 envelope with fields meta)
//! - deprecated routes emit a Deprecation: true header + envelope meta
//! - path/query params merge into the JSON body (body wins on conflict)
//! - the Idempotency-Key header feeds the engine's idempotency gate
//! - every response is the public Envelope; unknown routes fall through 404

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{FromRequest, FromRequestParts, Path as AxumPath, State};
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{MethodFilter, MethodRouter};
use axum::{Json, Router};
use kelson_context::{ExecutionContext, PrincipalContext, ScopeSnapshot, TenantContext};
use kelson_foundation::{
    AuthorizationRevision, ExecutionProfileKind, OperationId, PrincipalId, SecurityPlane, TenantId,
};
use kelson_execution::IdempotencyKey;
use kelson_http::{
    AuthVia, ChainRuntime, EdgeAuthPort, Envelope, Guard, ResolvedPrincipal, ResolvedSession,
};
use kelson_registry::{
    BindingSpec, CachePolicy, CacheSpec, ModuleManifest, PlatformGraph, RoutePlan, RouteSpec,
    ResponseKind, RoutePlane,
};
use time::OffsetDateTime;

use crate::routes::AppState;

// ------------------------------------------------------------- graph compile

/// Discover module manifests under `applications/` and `modules/`, plus every
/// package indexed by the root `packages.toml` (Wave 8 B-24) and the Wave 2
/// root route files. The implementation is shared
/// (`kelson_registry::discover_manifests`) so boot, the console and
/// `tests/architecture` cannot drift apart — the private copies are how the
/// console missed the byte-identical collapse added here in 26a653a
/// (P0 finding F3). Kept as a wrapper because existing callers and tests use
/// `graph_routes::discover_manifests`.
pub fn discover_manifests(root: &Path) -> Result<Vec<ModuleManifest>, String> {
    kelson_registry::discover_manifests(root)
}

/// Shared Wave 2 route-file discovery (`kelson_registry::discover_route_files`).
pub fn discover_route_files(root: &Path) -> Result<Vec<ModuleManifest>, String> {
    kelson_registry::discover_route_files(root)
}

/// Compile the platform graph from a workspace root.
pub fn compile_graph(root: &Path) -> Result<PlatformGraph, String> {
    let manifests = discover_manifests(root)?;
    PlatformGraph::compile(manifests).map_err(|violations| {
        violations
            .iter()
            .map(|v| format!("{}: {}", v.code, v.message))
            .collect::<Vec<_>>()
            .join("; ")
    })
}

// --------------------------------------------------------------- route cache

/// Boot gate (Wave 1 route cache): absent artifact -> Ok(None) (live
/// compile); present artifact must match the current manifest + middleware
/// content and the current module ABI, else boot is refused with a pointer
/// to kelson route:cache. Corrupt artifacts refuse boot too (fail closed,
/// strictly stricter than Laravel: the graph always recompiles).
pub fn route_cache_check(root: &Path) -> Result<Option<Vec<RoutePlan>>, String> {
    let path = kelson_registry::route_cache_path(root);
    let Some(artifact) = kelson_registry::RouteCacheArtifact::load(&path)? else {
        return Ok(None);
    };
    if !artifact.matches(root)? {
        return Err(format!(
            "route cache {} is stale (manifest or middleware.toml drift, or module ABI change); run kelson route:cache to rebuild or kelson route:clear to drop it",
            path.display()
        ));
    }
    Ok(Some(artifact.plans))
}

// ------------------------------------------------------------- route registry

/// Per-route dispatch context captured at registration time.
#[derive(Clone)]
pub struct RouteDispatch {
    pub operation: OperationId,
    pub profile: ExecutionProfileKind,
    pub plane: SecurityPlane,
    /// Declared public = anonymously reachable (implicit auth/verified
    /// stripped from the chain).
    pub public: bool,
    /// Emits a Deprecation: true header + meta.deprecated on success (L72).
    pub deprecated: bool,
    /// GET routes parse page/per_page and inject _page (Wave 1 pagination).
    pub inject_page: bool,
    /// JSON:API type name (Wave 8 ADR 0007): `Some` on expanded resource
    /// routes (`resource = "<name>"`), derived from the route name's
    /// `{basename}.{op}` prefix — the declared resource name, stable across
    /// path renames.
    pub resource_type: Option<String>,
    /// Path-parameter -> model bindings resolved before dispatch.
    pub bindings: Vec<BindingSpec>,
    /// Compiled manifest validation for this route's operation (collect-all).
    pub validator: Option<Arc<kelson_validation::Validator>>,
    /// Route plane (Wave 2): `web` renders error views instead of JSON
    /// envelopes (unless the request explicitly expects JSON).
    pub route_plane: RoutePlane,
    /// Response wire kind (Wave 2): json (default) | html (rendered view) |
    /// redirect (Location + flash) | download/file (StoragePort object) |
    /// stream (chunked NDJSON) | no_content (204).
    pub response: ResponseKind,
    /// Dot-path view name for `response = "html"` routes (`pages.welcome`).
    pub view: Option<String>,
    /// True for GET-registered routes: they carry ETags and answer
    /// conditional GETs with 304 (Wave 2 HTTP caching).
    pub is_get: bool,
    /// Per-route `Cache-Control` policy (Wave 2 HTTP caching); absent means
    /// the route emits no explicit cache directive.
    pub cache: Option<CacheSpec>,
}

/// Manifest {param} syntax is axum 0.8 capture syntax verbatim — pass
/// through unchanged (axum rejects the old :param form).
fn axum_path(spec_path: &str) -> String {
    spec_path.to_string()
}

/// The JSON:API `type` for an expanded resource route (ADR 0007). Registry
/// expansion names every verb `{basename}.{op}` where the basename is the
/// declared `resource = "<name>"` (or the route's own `name`), so the
/// prefix before the last dot is the resource name — stable across path
/// renames. Falls back to the first literal path segment.
fn jsonapi_type_name(spec: &RouteSpec) -> String {
    spec.name
        .as_deref()
        .and_then(|name| name.rsplit_once('.'))
        .map(|(prefix, _)| prefix)
        .filter(|prefix| !prefix.is_empty())
        .or_else(|| {
            spec.path
                .trim_start_matches('/')
                .split('/')
                .find(|segment| !segment.is_empty() && !segment.starts_with('{'))
        })
        .unwrap_or("resource")
        .to_string()
}

/// The wire methods a manifest `method` answers (19-PARITY route dispatch):
/// `GET` gains an automatic `HEAD`, and `ANY` is `Route::any`'s set — which
/// includes `OPTIONS`, so an `ANY` route answers the OPTIONS probe itself.
/// Unknown methods have no entry and are skipped at registration (the boot
/// gates refuse them outright in production).
fn declared_methods(method: &str) -> Option<Vec<&'static str>> {
    Some(match method.to_ascii_uppercase().as_str() {
        "GET" => vec!["GET", "HEAD"],
        "POST" => vec!["POST"],
        "PUT" => vec!["PUT"],
        "PATCH" => vec!["PATCH"],
        "DELETE" => vec!["DELETE"],
        "HEAD" => vec!["HEAD"],
        "OPTIONS" => vec!["OPTIONS"],
        "ANY" => vec![
            "GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS",
        ],
        _ => return None,
    })
}

/// One already-uppercased method token -> its axum filter bit.
fn filter_for(method: &str) -> Option<MethodFilter> {
    Some(match method {
        "GET" => MethodFilter::GET,
        "POST" => MethodFilter::POST,
        "PUT" => MethodFilter::PUT,
        "PATCH" => MethodFilter::PATCH,
        "DELETE" => MethodFilter::DELETE,
        "HEAD" => MethodFilter::HEAD,
        "OPTIONS" => MethodFilter::OPTIONS,
        _ => return None,
    })
}

/// Union of the filters for a set of declared methods.
fn filter_from(methods: &[&'static str]) -> Option<MethodFilter> {
    let mut iter = methods.iter();
    let mut filter = filter_for(iter.next()?)?;
    for method in iter {
        filter = filter.or(filter_for(method)?);
    }
    Some(filter)
}

/// One manifest method -> (declared wire methods, axum filter). The filter
/// is derived from the declaration, so the Allow list the OPTIONS probe
/// serves can never drift from what the route actually matches.
fn route_methods(method: &str) -> Option<(Vec<&'static str>, MethodFilter)> {
    let declared = declared_methods(method)?;
    let filter = filter_from(&declared)?;
    Some((declared, filter))
}

/// Registered route inventory (for kelson route:list-style introspection).
#[derive(Debug, Clone, serde::Serialize)]
pub struct RouteEntry {
    pub module: String,
    pub method: String,
    pub path: String,
    pub operation: String,
    pub capability: Option<String>,
}

/// Find the cached dispatch plan for one manifest route (route-cache hit).
fn cached_plan<'a>(plans: &'a [RoutePlan], module: &str, spec: &RouteSpec) -> Option<&'a RoutePlan> {
    let method = spec.method.to_ascii_uppercase();
    plans.iter().find(|p| {
        p.module == module && p.method == method && p.path == spec.path && p.operation == spec.operation
    })
}

/// Wave 4 Phase 4: adapt the `model.probes` registry (kelson-models) behind
/// the foundation's `DbProbeLookup`, so the validator evaluates `unique`/
/// `exists` without the data layer leaking downward. An unregistered model
/// yields `None` → the rule fails closed with a 503, never a pass.
struct EdgeProbes<'a>(&'a kelson_models::ProbeRegistry);

impl kelson_validation::DbProbeLookup for EdgeProbes<'_> {
    fn probe(&self, model: &str) -> Option<Arc<dyn kelson_validation::DbProbe>> {
        struct Adapter(Arc<dyn kelson_models::ModelProbe>);
        impl kelson_validation::DbProbe for Adapter {
            fn has_row(
                &self,
                tenant: Option<kelson_foundation::TenantId>,
                column: &str,
                value: &str,
                wheres: &[(String, String)],
                ignore_id: Option<&str>,
            ) -> Result<bool, String> {
                self.0.has_row(tenant, column, value, wheres, ignore_id)
            }
        }
        self.0
            .get(model)
            .map(|probe| Arc::new(Adapter(probe)) as Arc<dyn kelson_validation::DbProbe>)
    }
}

/// Compile manifest validation entries into one collect-all Validator.
/// None when the operation declares no rules. Rule parse errors fail the
/// route registration (the GRAPH-VALIDATION-RULE boot gate rejects them in
/// production; the live path degrades with a warning). Phase 4 also wires
/// the entry's `custom` rule (evaluated through the context lookup) and the
/// `messages`/`attribute` overrides (per-field message templates).
fn compile_validator(
    entries: &[kelson_registry::ValidationSpec],
) -> Result<Option<Arc<kelson_validation::Validator>>, String> {
    if entries.is_empty() {
        return Ok(None);
    }
    let mut validator = kelson_validation::Validator::new();
    for entry in entries {
        let mut rules = kelson_validation::parse_rules(&entry.rules)
            .map_err(|errs| format!("field {}: {}", entry.field, errs.join("; ")))?;
        if let Some(name) = &entry.custom {
            rules.push(kelson_validation::Rule::Custom(name.clone()));
        }
        validator = validator.field_messages(
            entry.field.clone(),
            rules,
            entry.messages.clone(),
            entry.attribute.clone(),
        );
    }
    Ok(Some(Arc::new(validator)))
}

/// Build a Router<()> of all graph routes, attaching each route's resolved
/// middleware chain at registration time (Wave 1 middleware model). Skips
/// (with a warning) any route whose method/operation id is malformed or
/// whose middleware chain fails to resolve rather than failing boot here —
/// the boot gates (GRAPH-MW-UNKNOWN, kelson check) refuse those in production.
pub fn graph_router(state: AppState, graph: &PlatformGraph) -> Router {
    // One shared edge-auth port + one shared bucket map: a named throttle
    // group is ONE limiter across all routes (Laravel semantics).
    let edge_auth: Arc<dyn EdgeAuthPort> = Arc::new(EdgeAuth {
        tokens: state.tokens.clone(),
        principals: state.principals.clone(),
        jwt: state.jwt.clone(),
        policy: state.policy.clone(),
        granted_capabilities: state.granted_capabilities.clone(),
        default_tenant_id: state.default_tenant_id,
    });
    let throttle = state.middleware_catalog.throttle_configs();
    let buckets = state.throttle_buckets.clone();

    let mut router = Router::new();

    // Wave 2 route dispatch: collect the Allow set per path first so every
    // GET path gets exactly one auto OPTIONS probe (204 + Allow, listing
    // every method served there). Paths that already declare OPTIONS — an
    // explicit route or `method = "ANY"` — answer it through dispatch and
    // are excluded from the probe list.
    let mut allow: BTreeMap<String, BTreeSet<&'static str>> = BTreeMap::new();
    let mut explicit_options: BTreeSet<String> = BTreeSet::new();
    let mut probe_paths: BTreeSet<String> = BTreeSet::new();
    let mut fallback_route: Option<MethodRouter> = None;
    for module in &graph.modules {
        for spec in &module.routes {
            let Some(methods) = declared_methods(&spec.method) else {
                continue;
            };
            let path = axum_path(&spec.path);
            allow
                .entry(path.clone())
                .or_default()
                .extend(methods.iter().copied());
            if methods.contains(&"OPTIONS") {
                explicit_options.insert(path);
            } else if methods.contains(&"GET") {
                probe_paths.insert(path);
            }
        }
    }
    probe_paths.retain(|path| !explicit_options.contains(path));

    for module in &graph.modules {
        let plane = module.module.plane.to_security_plane();
        let tenant_plane = module.module.plane == kelson_registry::ModulePlane::Tenant;
        for spec in &module.routes {
            let Some((declared, filter)) = route_methods(&spec.method) else {
                tracing::warn!(module = module.name(), method = %spec.method, path = %spec.path, "graph route skipped: unsupported method");
                continue;
            };
            let operation_id = if spec.from_route_file { spec.operation.clone() } else { format!("{}.{}", module.name(), spec.operation) };
            let Ok(operation) = OperationId::parse(&operation_id) else {
                tracing::warn!(operation = %operation_id, "graph route skipped: invalid operation id");
                continue;
            };
            // Route-cache hit = register from the pinned artifact; miss =
            // live resolution against the catalog (today's behavior).
            // Phase 3 (§4.2): plans live behind a swappable cell — read the
            // current revision once per route; a concurrent hot reload
            // swaps the `Arc`, never mutating what this build is reading.
            let route_plans = state.route_plans.read().unwrap();
            let cached = route_plans
                .as_deref()
                .and_then(|plans| cached_plan(plans, module.name(), spec));

            let chain = match cached {
                Some(plan) => plan.chain.clone(),
                None => match state.middleware_catalog.resolve_chain(
                    tenant_plane,
                    spec.public,
                    &spec.middleware,
                    &spec.excluded_middleware,
                ) {
                    Ok(chain) => chain,
                    Err(v) => {
                        tracing::warn!(module = module.name(), method = %spec.method, path = %spec.path, error = %v.message, "graph route skipped: middleware chain rejected");
                        continue;
                    }
                },
            };

            let validation_entries: Vec<kelson_registry::ValidationSpec> = match cached {
                Some(plan) => plan.validation.clone(),
                None => {
                    // A route-file route carries no rules of its own; inherit the declaring module's.
                    let owner = spec.from_route_file.then(|| graph.operations.get(&operation_id)).flatten().and_then(|o| graph.modules.iter().find(|m| m.name() == o.as_str()));
                    owner.into_iter().chain(std::iter::once(module)).flat_map(|m| m.validation.iter())
                        .filter(|s| s.operation == spec.operation || spec.operation.ends_with(&format!(".{}", s.operation))).cloned().collect()
                }
            };
            let validator = match compile_validator(&validation_entries) {
                Ok(validator) => validator,
                Err(error) => {
                    tracing::warn!(module = module.name(), operation = %operation_id, error = %error, "graph route registered without manifest validation (rules failed to compile)");
                    None
                }
            };

            // Route-cache hit carries the same constraints the manifest
            // resolves (route-cache parity with chain/bindings); they gate
            // route matching, not dispatch.
            let constraints = cached
                .map(|plan| plan.constraints.clone())
                .unwrap_or_else(|| spec.resolved_constraints.clone());

            let profile = module.capabilities.get(&spec.operation).map(|c| c.profile)
                .unwrap_or_else(|| graph.operations.get(&operation_id)
                    .and_then(|owner| graph.modules.iter().find(|m| m.name() == owner.as_str()))
                    .and_then(|m| spec.operation.strip_prefix(&format!("{}.", m.name())).and_then(|cap| m.capabilities.get(cap)))
                    .map(|c| c.profile).unwrap_or(ExecutionProfileKind::Command));
            let dispatch = Arc::new(RouteDispatch {
                operation,
                profile,
                plane,
                public: spec.public,
                deprecated: spec.deprecated,
                inject_page: spec.method.eq_ignore_ascii_case("get"),
                resource_type: spec.from_resource.then(|| jsonapi_type_name(spec)),
                bindings: cached
                    .map(|plan| plan.bindings.clone())
                    .unwrap_or_else(|| spec.bindings.clone()),
                validator,
                // Wave 2: plane + wire kind drive the edge (HTML render, web
                // error views). Invalid plane tokens are already refused by
                // the GRAPH-PLANE boot gate, so the fallback is `api`.
                route_plane: spec
                    .plane
                    .as_deref()
                    .and_then(|token| RoutePlane::from_token(token).ok())
                    .unwrap_or_default(),
                response: spec.response,
                view: spec.view.clone(),
                is_get: spec.method.eq_ignore_ascii_case("get"),
                cache: spec.cache,
            });

            let runtime = Arc::new(ChainRuntime {
                chain,
                auth: edge_auth.clone(),
                require_verified_email: state.require_verified_email,
                confirm_timeout_secs: state.confirm_timeout_secs,
                throttle: throttle.clone(),
                buckets: buckets.clone(),
                signed_urls: state
                    .signed_urls
                    .clone()
                    .map(|gate| gate as Arc<dyn kelson_http::SignedUrlPort>),
            });

            let path = axum_path(&spec.path);
            // Wave 7 P4 (19-PARITY "Security-headers middleware"): the
            // deployment's `[security.headers]` posture, captured before the
            // route gate consumes `dispatch`. Web-plane routes only — an
            // api-plane response stays byte-identical — and only while the
            // switch is on, so the default posture attaches nothing at all.
            let security_headers = (dispatch.route_plane == RoutePlane::Web
                && state.security_headers.enabled)
                .then(|| state.security_headers.clone());
            // Wave 7 P7 (19-PARITY "CSRF on-by-default for the web plane"):
            // the double-submit guard, captured with the posture before the
            // route gate consumes `dispatch`. Web-plane routes only — an
            // api-plane request stays token-authenticated and never sees
            // the layer — and always on (no flag or env); the deployment's
            // `[security.csrf]` skip paths ride the guard itself.
            let csrf = (dispatch.route_plane == RoutePlane::Web).then(|| state.csrf.clone());
            let mut route: MethodRouter = axum::routing::on(filter, graph_dispatch)
                .with_state((state.clone(), dispatch.clone()))
                .layer(axum::middleware::from_fn_with_state(
                    runtime,
                    kelson_http::chain_middleware,
                ));
            // Wave 2 route gate: host matching + parameter constraints run
            // BEFORE auth — the layer sits outside the chain, so a request
            // the route does not match never reaches the identity stage
            // (404, the way Laravel route domains and `where*` behave).
            if spec.domain.is_some() || spec.subdomain.is_some() || !constraints.is_empty() {
                route = route.layer(axum::middleware::from_fn_with_state(
                    (state.clone(), RouteGate::new(dispatch, spec, constraints)),
                    route_gate,
                ));
            }
            // Wave 2 HEAD auto-registration: strip outermost so a chain
            // short-circuit (401/404, envelope or error view) is stripped
            // exactly like a handled GET body.
            if declared.contains(&"HEAD") {
                route = route.layer(axum::middleware::from_fn(strip_head_body));
            }
            // Wave 7 P7: attached BEFORE the security-headers layer, so the
            // headers wrap the guard — a CSRF 403 still carries the
            // deployment posture while the rejection itself (403, envelope
            // or web error view) renders exactly like any other route
            // answer. Safe requests bootstrap the `kelson_csrf` cookie here;
            // state-changing ones must echo it.
            if let Some(guard) = csrf {
                route = guard.apply_route(route);
            }
            // Wave 7 P4: attached last, so it is outermost within the route —
            // the handler's views, error views, chain short-circuits and
            // route-gate rejections all carry the posture. Responses
            // short-circuited before routing (rate limit, maintenance) stay
            // outside the route by design; they are edge posture, not
            // web-plane answers.
            if let Some(headers) = security_headers {
                route = route.layer(axum::middleware::from_fn_with_state(
                    headers,
                    kelson_http::security_headers_fn,
                ));
            }
            if spec.fallback { if fallback_route.is_some() { panic!("module {}: route {} {} is a second fallback route; the merged edge router attaches at most one", module.name(), spec.method, spec.path); } fallback_route = Some(route); } else { router = router.route(&path, route); }
            tracing::info!(method = %spec.method, path = %spec.path, operation = %operation_id, "graph route registered");
        }
    }

    // One auto OPTIONS probe per GET path: 204 + Allow, listing every
    // method registered on that path (19-PARITY "GET routes auto-register
    // HEAD + OPTIONS"). Registered last, after every declared method has
    // contributed to the Allow set.
    for path in probe_paths {
        let Some(methods) = allow.get(&path) else {
            continue;
        };
        let mut served = methods.clone();
        served.insert("OPTIONS");
        let allow_header = served.into_iter().collect::<Vec<_>>().join(", ");
        tracing::info!(path = %path, allow = %allow_header, "graph route registered (auto OPTIONS probe)");
        let probe: MethodRouter = axum::routing::options(options_probe).with_state(allow_header);
        router = router.route(&path, probe);
    }
    // Module-lifecycle availability (docs/15-EXTENSION, dispatch wiring):
    // the refusal for a tracked module that is not serving is graph-router
    // posture — these are the very routes the gate guards — so pin its
    // status once here. The render registry is interior-mutable and shared
    // with the boot ServiceRegistry, so the production boot and every
    // harness build register the same pin, and a hot-reload rebuild
    // re-pins idempotently. Without it the ResourceExhausted category
    // would serve 429, telling the caller it was rate limited when our own
    // module was the thing that went offline — the same reasoning that
    // pins the WASM availability codes at both boot sites.
    state.error_renders.set_status(
        kelson_foundation::codes::MODULE_NOT_SERVING,
        StatusCode::SERVICE_UNAVAILABLE,
    );
    if let Some(fb) = fallback_route { router.fallback_service(fb) } else { router }
}

// ------------------------------------------- Wave 2 routing edge (19-PARITY)

/// Body ceiling when computing a HEAD answer's `Content-Length` from the GET
/// body it mirrors: a body larger than this (a stream, a download) is
/// reported without a length instead of being buffered to the end.
const HEAD_BODY_LIMIT: usize = 8 * 1024 * 1024;

/// Auto OPTIONS answer (19-PARITY "GET routes auto-register HEAD +
/// OPTIONS"): 204 with an `Allow` header enumerating every method the path
/// serves. Registered once per path, after the declared routes, so the set
/// is complete.
async fn options_probe(State(allow): State<String>) -> axum::response::Response {
    let mut response = axum::response::Response::new(Body::empty());
    *response.status_mut() = StatusCode::NO_CONTENT;
    if let Ok(value) = header::HeaderValue::from_str(&allow) {
        response.headers_mut().insert(header::ALLOW, value);
    }
    response
}

/// HEAD answers carry the GET status and headers with no body (RFC 9110
/// §9.3.2), reporting the length the GET would have served when the body is
/// known. Layered outermost on every HEAD-capable route so a chain
/// short-circuit (401/404, envelope or error view) strips too.
async fn strip_head_body(
    req: Request<Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let is_head = req.method() == Method::HEAD;
    let response = next.run(req).await;
    if !is_head {
        return response;
    }
    let (parts, body) = response.into_parts();
    let bytes = axum::body::to_bytes(body, HEAD_BODY_LIMIT).await.ok();
    let mut parts = parts;
    // 204 must not carry a Content-Length (RFC 9110 §8.6).
    if let (Some(bytes), false) = (&bytes, parts.status == StatusCode::NO_CONTENT) {
        if let Ok(value) = header::HeaderValue::from_str(&bytes.len().to_string()) {
            parts.headers.insert(header::CONTENT_LENGTH, value);
        }
    }
    axum::response::Response::from_parts(parts, Body::empty())
}

/// Wave 2 route gate (19-PARITY routing): the "does this route match?"
/// predicates — `domain`/`subdomain` host matching and resolved parameter
/// constraints — enforced BEFORE the auth chain. The gate layers outside
/// `chain_middleware`, so a wrong host or a rejected capture never reaches
/// the identity stage: the route simply does not exist for that request
/// (404, the way Laravel's route domains and `where*` constraints behave).
/// A matched `subdomain` also resolves a tenant record from the registry
/// and carries it to dispatch as [`HostTenant`] (multi-tenant web hosting).
#[derive(Clone)]
struct RouteGate {
    dispatch: Arc<RouteDispatch>,
    domain: Option<String>,
    subdomain: Option<String>,
    constraints: Vec<kelson_registry::ResolvedConstraint>,
}

impl RouteGate {
    fn new(
        dispatch: Arc<RouteDispatch>,
        spec: &RouteSpec,
        constraints: Vec<kelson_registry::ResolvedConstraint>,
    ) -> Self {
        Self {
            dispatch,
            domain: non_empty(spec.domain.as_deref()),
            subdomain: non_empty(spec.subdomain.as_deref())
                .map(|label| label.to_ascii_lowercase()),
            constraints,
        }
    }
}

fn non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Lowercased host from the Host header with any `:port` stripped. The
/// authority may come as a comma list when a proxy appended its own, so only
/// the first entry counts; a missing header answers empty (no route matches).
fn host_label(headers: &HeaderMap) -> String {
    let Some(raw) = headers.get(header::HOST).and_then(|value| value.to_str().ok()) else {
        return String::new();
    };
    let authority = raw.split(',').next().unwrap_or(raw).trim();
    let host = match authority.rsplit_once(':') {
        Some((head, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => authority,
    };
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// Exact host match, or a `*.example.com` wildcard covering any subdomain
/// of that suffix (the apex itself does not match, as with DNS wildcards).
fn domain_matches(host: &str, expected: &str) -> bool {
    let host = host.to_ascii_lowercase();
    let expected = expected.trim_end_matches('.').to_ascii_lowercase();
    match expected.strip_prefix("*.") {
        Some(suffix) => host
            .strip_suffix(&format!(".{suffix}"))
            .is_some_and(|prefix| !prefix.is_empty() && !prefix.ends_with('.')),
        None => host == expected,
    }
}

/// The first DNS label is the declared subdomain (`acme.example.com` ->
/// `acme`), and the host must actually carry a parent domain.
fn subdomain_matches(host: &str, expected: &str) -> bool {
    let mut labels = host.split('.');
    let Some(label) = labels.next() else {
        return false;
    };
    !label.is_empty() && label.eq_ignore_ascii_case(expected) && labels.next().is_some()
}

/// Tenant resolved from a route's `subdomain` by the host gate, carried to
/// dispatch as a request extension where it feeds tenant resolution:
/// the principal's own tenant wins, then this, then the configured default.
#[derive(Debug, Clone)]
pub struct HostTenant(pub TenantContext);

async fn route_gate(
    State((state, gate)): State<(AppState, RouteGate)>,
    mut req: Request<Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let host = host_label(req.headers());
    if let Some(expected) = gate.domain.as_deref() {
        if !domain_matches(&host, expected) {
            return route_miss(&state, &gate, req.headers(), req.uri().path());
        }
    }
    if let Some(expected) = gate.subdomain.as_deref() {
        if !subdomain_matches(&host, expected) {
            return route_miss(&state, &gate, req.headers(), req.uri().path());
        }
        // Matched subdomain -> tenant lookup feeding tenant resolution. An
        // unknown slug keeps the existing resolution (the host still had to
        // match): nothing is silently promoted to another tenant's data.
        if let Some(tenants) = state.tenants.as_ref() {
            match tenants.resolve(&kelson_tenancy::TenantHint::Slug(expected.to_string())) {
                Ok(record) => {
                    let context = tenants.context(&record);
                    tracing::debug!(
                        subdomain = %expected,
                        tenant = %context.slug,
                        "host tenant feeds tenant resolution"
                    );
                    req.extensions_mut().insert(HostTenant(context));
                }
                Err(error) => tracing::debug!(
                    subdomain = %expected,
                    error = %error.public_message,
                    "subdomain has no tenant record; tenant resolution falls back"
                ),
            }
        }
    }
    if !gate.constraints.is_empty() {
        // Constraints are part of route MATCHING (Laravel `where*`), so they
        // belong here rather than in the handler: a capture the constraint
        // rejects means no route, before any identity work. The captures are
        // read through axum's own `Path` extractor, so values are decoded
        // exactly as dispatch sees them (no second path parser at the edge).
        let (mut parts, body) = req.into_parts();
        let captured =
            match AxumPath::<Vec<(String, String)>>::from_request_parts(&mut parts, &()).await {
                Ok(AxumPath(captured)) => captured,
                Err(error) => {
                    // An already-matched route always carries its captures;
                    // if they cannot be read, refuse rather than dispatch a
                    // constraint nobody checked.
                    tracing::warn!(?error, "route captures unreadable; treating as a route miss");
                    return route_miss(&state, &gate, &parts.headers, parts.uri.path());
                }
            };
        for constraint in &gate.constraints {
            let Some(value) = captured
                .iter()
                .find(|(key, _)| key == &constraint.param)
                .map(|(_, value)| value.as_str())
            else {
                // GRAPH-CONSTRAINT-PARAM refuses boot when a constrained
                // param is absent from the path, so this is unreachable in
                // a booted graph; nothing to check against.
                continue;
            };
            if !kelson_registry::constraint_matches(constraint, value) {
                tracing::debug!(
                    param = %constraint.param,
                    kind = %constraint.kind.as_str(),
                    "route constraint rejects the capture"
                );
                return route_miss(&state, &gate, &parts.headers, parts.uri.path());
            }
        }
        req = Request::from_parts(parts, body);
    }
    next.run(req).await
}

/// Route miss: the request does not match this entry (host or capture), so
/// the route does not exist for it — 404 through the usual render path
/// (web-plane error views included), never 403.
fn route_miss(
    state: &AppState,
    gate: &RouteGate,
    headers: &HeaderMap,
    path: &str,
) -> axum::response::Response {
    dispatch_error(
        state,
        &gate.dispatch,
        headers,
        path,
        &kelson_foundation::AppError::not_found("route"),
    )
}

/// Render a path that matched NO route — the static fallback's miss case
/// (traversal attempts, unknown URLs, files that do not exist). A synthetic
/// web-plane dispatch sends browsers to the `errors/404` view and machine
/// clients to the Envelope/problem, exactly like a graph route that resolves
/// to nothing, so an unmatched path is one error shape rather than axum's
/// bare text.
pub(crate) fn not_found_response(
    state: &AppState,
    headers: &HeaderMap,
    path: &str,
) -> axum::response::Response {
    let dispatch = RouteDispatch {
        operation: OperationId::from_static("http.not_found"),
        profile: ExecutionProfileKind::Read,
        plane: SecurityPlane::Edge,
        public: true,
        deprecated: false,
        inject_page: false,
        resource_type: None,
        bindings: Vec::new(),
        validator: None,
        route_plane: RoutePlane::Web,
        response: ResponseKind::Json,
        view: None,
        is_get: true,
        cache: None,
    };
    dispatch_error(
        state,
        &dispatch,
        headers,
        path,
        &kelson_foundation::AppError::not_found("route"),
    )
}

/// Inventory of graph routes (registration-independent, for meta endpoints).
pub fn route_inventory(graph: &PlatformGraph) -> Vec<RouteEntry> {
    let mut out = Vec::new();
    for module in &graph.modules {
        for spec in &module.routes {
            out.push(RouteEntry {
                module: module.name().to_string(),
                method: spec.method.to_ascii_uppercase(),
                path: spec.path.clone(),
                operation: if spec.from_route_file { spec.operation.clone() } else { format!("{}.{}", module.name(), spec.operation) },
                capability: spec.capability.clone(),
            });
        }
    }
    out
}

// ------------------------------------------------------------------ edge auth

/// Edge bearer resolution shared by every graph route chain (Wave 1): hashed
/// PAT first (the default auth model), opt-in HS256 JWT interop second.
/// PATs keep their login-time snapshot semantics (no install here — the
/// pat_bypasses_snapshot contract); a stateless JWT never went through
/// login in this process, so its access snapshot is hydrated at resolution
/// time (idempotent install).
pub(crate) struct EdgeAuth {
    pub(crate) tokens: Arc<kelson_tokens::TokenService>,
    pub(crate) principals: Arc<dyn kelson_identity::PrincipalStore>,
    pub(crate) jwt: Option<Arc<kelson_http::JwtHs256>>,
    pub(crate) policy: Arc<kelson_policy::PolicyEngine>,
    pub(crate) granted_capabilities: Vec<String>,
    pub(crate) default_tenant_id: Option<TenantId>,
}

impl EdgeAuthPort for EdgeAuth {
    fn resolve_bearer(&self, token: &str) -> Option<ResolvedPrincipal> {
        match self.tokens.verify(kelson_tokens::TokenPurpose::Api, token) {
            Ok(auth_token) => Some(ResolvedPrincipal {
                principal: auth_token.principal_id,
                tenant: tenant_from_label(&auth_token.label),
                via: AuthVia::Bearer,
                // The PAT's narrowing: the abilities it was issued with
                // (F-7). An empty list means no narrowing was granted,
                // so the credential acts as the principal (the old
                // behaviour) rather than as nothing.
                abilities: (!auth_token.abilities.is_empty())
                    .then(|| auth_token.abilities.clone()),
            }),
            Err(_) => {
                let (principal, tenant) = resolve_jwt_token(self.jwt.as_deref(), token)?;
                let tenant = tenant.or(self.default_tenant_id);
                if let Some(tenant) = tenant {
                    crate::auth_routes::install_snapshot_fields(
                        &self.policy,
                        &self.granted_capabilities,
                        principal,
                        tenant,
                    );
                }
                Some(ResolvedPrincipal {
                    principal,
                    tenant,
                    via: AuthVia::Jwt,
                    abilities: None,
                })
            }
        }
    }

    fn email_verified(&self, principal: PrincipalId) -> bool {
        matches!(
            self.principals.by_id(principal),
            Ok(Some(p)) if p.email_verified_at.is_some()
        )
    }
}

/// B-22: resolve a bearer token through the opt-in HS256 JWT interop
/// verifier. Returns None when the verifier is not wired (app.jwt off) or
/// verification fails; callers collapse that to a uniform 401. The tenant
/// claim is optional; absent tenants fall through to the caller's
/// default-tenant logic.
pub(crate) fn resolve_jwt_token(
    jwt: Option<&kelson_http::JwtHs256>,
    token: &str,
) -> Option<(PrincipalId, Option<TenantId>)> {
    let claims = jwt?.verify(token).ok()?;
    let principal = uuid::Uuid::parse_str(&claims.sub)
        .ok()
        .map(PrincipalId::from_uuid)?;
    let tenant = claims
        .tenant
        .as_deref()
        .and_then(|raw| uuid::Uuid::parse_str(raw).ok())
        .map(TenantId::from_uuid);
    Some((principal, tenant))
}

// ---------------------------------------------------------------- dispatch

/// Query parser: a=1&b=2 -> ordered pairs. Tolerant: malformed pairs are
/// skipped (never a request failure).
pub(crate) fn query_pairs(query: Option<&str>) -> Vec<(String, String)> {
    let Some(query) = query else {
        return Vec::new();
    };
    query
        .split('&')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            if key.is_empty() {
                return None;
            }
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

/// `application/x-www-form-urlencoded` value decoding (HTML form encoding,
/// required by RFC 6749 for token-request bodies): `%XX` bytes decode and
/// `+` means space. Malformed escapes stay literal and invalid UTF-8 falls
/// back to lossy — decoding never fails a request (same tolerance posture
/// as [`query_pairs`]).
pub(crate) fn form_decode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Merge path + query params into the JSON input object (body wins).
fn merge_params(
    mut input: serde_json::Value,
    params: Vec<(String, String)>,
    query: Vec<(String, String)>,
) -> serde_json::Value {
    if !input.is_object() {
        input = serde_json::json!({});
    }
    let obj = input.as_object_mut().expect("checked object");
    for (key, value) in query.into_iter().chain(params) {
        if !obj.contains_key(&key) {
            obj.insert(key, serde_json::Value::String(value));
        }
    }
    input
}

/// PAT labels carry the tenant: tenant:<uuid>.
fn tenant_from_label(label: &Option<String>) -> Option<TenantId> {
    let raw = label.as_ref()?.strip_prefix("tenant:")?;
    uuid::Uuid::parse_str(raw).ok().map(TenantId::from_uuid)
}

/// Bearer credential for the dispatch edge. Delegates to the shared request
/// helper so the api plane, uploads and `/auth/me` read `Authorization` the
/// same way (case-insensitive scheme, trimmed, non-empty).
pub(crate) fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    kelson_http::bearer_token(headers)
}

/// The host's **bearer verifier** for the framework's guard port: the PAT
/// first (the default model), then the opt-in stateless HS256 JWT fallback.
///
/// The *order* is the host's policy; the framework owns the plumbing (a
/// credential that does not verify is one uniform refusal ✓).
pub(crate) struct DemoBearerVerifier {
    tokens: Arc<kelson_tokens::TokenService>,
    jwt: Option<Arc<kelson_http::JwtHs256>>,
}

impl kelson_http::BearerVerifier for DemoBearerVerifier {
    fn verify_bearer(
        &self,
        token: &str,
    ) -> Result<Option<kelson_http::GuardedPrincipal>, kelson_foundation::AppError> {
        match self.tokens.verify(kelson_tokens::TokenPurpose::Api, token) {
            Ok(auth_token) => Ok(Some(kelson_http::guarded_with_abilities(
                auth_token.principal_id,
                tenant_from_label(&auth_token.label),
                kelson_http::GuardStrategy::Token,
                auth_token.abilities.clone(),
            ))),
            Err(_) => Ok(resolve_jwt_token(self.jwt.as_deref(), token).map(
                |(principal, tenant)| {
                    kelson_http::guarded(principal, tenant, kelson_http::GuardStrategy::Jwt)
                },
            )),
        }
    }
}

/// The host's **guard**: the session first, then a bearer credential, then a
/// personal access token carried in a **cookie** (Sanctum's SPA mode).
///
/// LF-049's separation made concrete — the guard answers "who is this
/// request and how was it proven" (and with which **abilities** ✓), while the
/// provider (the identity stores) answers "what is this principal" ✓.
pub(crate) fn guard(
    state: &AppState,
) -> kelson_http::BearerGuard<DemoBearerVerifier> {
    kelson_http::BearerGuard::new(
        Arc::new(DemoBearerVerifier {
            tokens: state.tokens.clone(),
            jwt: state.jwt.clone(),
        }),
        kelson_http::GuardStrategy::Token,
    )
}

/// The cookie a SPA client carries its personal access token in.
pub(crate) const TOKEN_COOKIE: &str = "kelson_token";

/// The host's **cookie guard**: a personal access token in a cookie (the SPA
/// mode). It is NOT stateless bearer auth — a cookie is sent cross-site by a
/// browser, so the CSRF double-submit proof still applies ✓.
pub(crate) fn cookie_guard(
    state: &AppState,
) -> kelson_http::CookieTokenGuard<DemoBearerVerifier> {
    kelson_http::CookieTokenGuard::new(
        Arc::new(DemoBearerVerifier {
            tokens: state.tokens.clone(),
            jwt: state.jwt.clone(),
        }),
        TOKEN_COOKIE,
    )
}

/// Shared identity resolution for non-graph endpoints (uploads, /auth/me):
/// the session first, then a bearer PAT, then the SPA token cookie. Used by
/// upload_routes/auth_routes.
pub(crate) fn resolve_identity(
    state: &AppState,
    headers: &HeaderMap,
    session: Option<&ResolvedSession>,
) -> Result<(PrincipalId, Option<TenantId>), kelson_foundation::AppError> {
    if let Some(resolved) = guard(state).resolve(headers, session)? {
        return Ok((resolved.principal_id, resolved.tenant_id));
    }
    if let Some(resolved) = cookie_guard(state).resolve(headers, session)? {
        return Ok((resolved.principal_id, resolved.tenant_id));
    }
    // No credential at all: the route decides whether it needs an identity;
    // the shared answer is the uniform 401.
    Err(kelson_foundation::AppError::unauthenticated())
}

/// Wave 2 request intent (the `expects_json` half of the web-plane contract):
/// only an explicit `Accept: application/json` keeps the JSON envelope on a
/// web route. Browsers, `curl` without an Accept header and anything else get
/// the HTML error view. Delegates to the shared request helper so every edge
/// agrees on what "JSON client" means (wildcards never count, `q=0` never
/// counts).
fn expects_json(headers: &HeaderMap) -> bool {
    kelson_http::expects_json(headers)
}

/// Wave 2 error views: the registry-pinned view for the error code first,
/// then `errors.404`, `errors.500` and `errors.maintenance` (503), falling
/// back to the generic `errors.500` when the specific page is absent. The
/// context carries public fields — status, canonical reason, the error's
/// public message, its code and the request path — plus redacted internal
/// detail when `app.debug` is on. Returns `None` when no engine is wired or
/// no candidate renders, so the caller falls back to the JSON body.
fn error_view_response(
    state: &AppState,
    status: StatusCode,
    err: &kelson_foundation::AppError,
    path: &str,
    pinned: Option<&str>,
    locale: &str,
) -> Option<axum::response::Response> {
    let engine = state.views.as_ref()?;
    let by_status = match status {
        StatusCode::NOT_FOUND => "errors.404",
        StatusCode::SERVICE_UNAVAILABLE => "errors.maintenance",
        _ => "errors.500",
    };
    let name = match pinned.filter(|n| !n.is_empty()) {
        Some(p) if engine.exists(p) => p.to_string(),
        _ if engine.exists(by_status) => by_status.to_string(),
        _ if engine.exists("errors.500") => "errors.500".to_string(),
        _ => return None,
    };
    let mut context = serde_json::json!({
        "status": status.as_u16(),
        "title": status.canonical_reason().unwrap_or("Error"),
        "message": err.public_message,
        "code": err.code,
        "path": path,
    });
    // Wave 6 P3: the same locale injections as success views so error
    // pages can use `@lang` / the `trans` filter too.
    if let Some(obj) = context.as_object_mut() {
        obj.insert(
            "_locale".into(),
            serde_json::Value::String(locale.to_string()),
        );
        obj.insert(
            "_lang".into(),
            serde_json::to_value(state.messages.flat(locale)).unwrap_or_default(),
        );
    }
    if state.debug {
        if let Some(debug) = crate::routes::debug_detail(err) {
            if let Some(obj) = context.as_object_mut() {
                obj.insert("debug".into(), debug);
            }
        }
    }
    let html = engine.render(&name, context).ok()?;
    Some(
        (status, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response(),
    )
}

/// Wave 2 error rendering at the dispatch edge. The registry plan (status,
/// render path, reportability) runs exactly once through
/// `AppState::error_envelope`, which also logs reportable errors and attaches
/// `app.debug` detail plus per-code renderer output. Web-plane HTML clients
/// then get the error view; the api plane and machine clients get the
/// Envelope, or RFC-7807 `application/problem+json` when negotiated
/// (fail closed to the Envelope everywhere else).
fn dispatch_error(
    state: &AppState,
    dispatch: &RouteDispatch,
    headers: &HeaderMap,
    path: &str,
    err: &kelson_foundation::AppError,
) -> axum::response::Response {
    let (status, envelope) = state.error_envelope(err);
    let accept = headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok());

    if dispatch.route_plane == RoutePlane::Web
        && !expects_json(headers)
        && !kelson_http::wants_problem(accept, false)
    {
        // Wave 6 P3: error views carry the same locale injections as
        // success views; no session rides this path, so resolution is
        // Accept-Language -> configured default.
        let locale = crate::i18n::resolve(
            &state.messages,
            None,
            None,
            headers
                .get(header::ACCEPT_LANGUAGE)
                .and_then(|v| v.to_str().ok()),
        );
        let pinned = state.error_renders.plan(err).view;
        if let Some(response) =
            error_view_response(state, status, err, path, pinned.as_deref(), &locale)
        {
            return response;
        }
        return (
            status,
            Json(serde_json::to_value(envelope).unwrap_or_default()),
        )
            .into_response();
    }

    if kelson_http::wants_problem(accept, state.problem_json) {
        let error = envelope
            .error
            .clone()
            .unwrap_or_else(|| kelson_http::ErrorBody::from_app_error(err));
        return state.problem_response(status, &error, Some(path));
    }
    (
        status,
        Json(serde_json::to_value(envelope).unwrap_or_default()),
    )
        .into_response()
}

/// Wave 2 HTML route: render `route.view` with the operation result as the
/// template context (an object result IS the context; scalars and arrays nest
/// under `data`). Seeded with the views crate's documented edge conventions:
/// `_errors` starts empty so `@error` renders nothing, and `_auth` carries the
/// principal id when one is installed. A missing engine, a missing `view`, a
/// missing template or a parse error is a 500 through the normal error path —
/// never a half-rendered page.
fn html_response(
    state: &AppState,
    dispatch: &RouteDispatch,
    headers: &HeaderMap,
    path: &str,
    locale: &str,
    authed: Option<String>,
    result: serde_json::Value,
) -> (axum::response::Response, Option<Vec<u8>>) {
    let fail = |err: kelson_foundation::AppError| {
        (dispatch_error(state, dispatch, headers, path, &err), None)
    };
    let Some(engine) = state.views.as_ref() else {
        return fail(kelson_foundation::AppError::internal("views are not configured"));
    };
    let Some(name) = dispatch.view.as_deref().filter(|n| !n.is_empty()) else {
        return fail(kelson_foundation::AppError::internal(
            "route declares response = html but no view",
        ));
    };
    let mut context = if result.is_object() {
        result
    } else {
        serde_json::json!({ "data": result })
    };
    if let Some(obj) = context.as_object_mut() {
        obj.entry("_errors")
            .or_insert_with(|| serde_json::json!({}));
        // Wave 6 P3: resolved locale + chain-merged messages, so `@lang`
        // and the `trans` filter resolve in-request (exact locale overlay
        // over the language part over the configured default).
        obj.insert(
            "_locale".into(),
            serde_json::Value::String(locale.to_string()),
        );
        obj.insert(
            "_lang".into(),
            serde_json::to_value(state.messages.flat(locale)).unwrap_or_default(),
        );
        if let Some(who) = authed {
            obj.insert("_auth".into(), serde_json::Value::String(who));
        }
    }
    match engine.render(name, context) {
        Ok(html) => {
            // The caching step hashes exactly these bytes, so hand them back
            // instead of re-reading the body off the transport later.
            let bytes = html.as_bytes().to_vec();
            let response = (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response();
            (response, Some(bytes))
        }
        Err(view) => fail(kelson_foundation::AppError::internal(format!(
            "view {name} failed: {}",
            view.message
        ))),
    }
}

// ---------------------------------------------------- Wave 2 response kinds

/// The resolved caller a route renders for: the session, the tenant and
/// the view authorisation the middleware produced for this request.
///
/// They travel together because a renderer needs all of them or none of
/// them - and one value keeps the renderer's signature honest.
struct RenderCaller {
    session: Option<ResolvedSession>,
    tenant: Option<TenantId>,
    view_auth: Option<String>,
}

/// Render a successful operation result according to the route's declared
/// response kind (19-PARITY "Response kinds"). Returns the response plus
/// the exact bytes it was built from (when buffered) so the caching step
/// can hash what was served without re-reading it from the transport.
fn render_success(
    state: &AppState,
    dispatch: &RouteDispatch,
    headers: &HeaderMap,
    request_target: &str,
    caller: RenderCaller,
    result: serde_json::Value,
) -> Result<(axum::response::Response, Option<Vec<u8>>), kelson_foundation::AppError> {
    use kelson_foundation::AppError;
    let RenderCaller {
        session,
        tenant,
        view_auth,
    } = caller;
    match dispatch.response {
        ResponseKind::Html => {
            // The route was registered on the path, never on the query —
            // `request_target` is that path with the query re-appended.
            let path = match request_target.split_once('?') {
                Some((path, _)) => path,
                None => request_target,
            };
            // Wave 6 P3: same pure resolution the dispatcher ran for
            // validation/context (session + tenant + headers ⇒ identical outcome).
            let locale = crate::i18n::resolve(
                &state.messages,
                crate::i18n::session_locale(session.as_ref()),
                state.tenant_locale(tenant).as_deref(),
                headers
                    .get(header::ACCEPT_LANGUAGE)
                    .and_then(|v| v.to_str().ok()),
            );            Ok(html_response(
                state, dispatch, headers, path, &locale, view_auth, result,
            ))
        }
        ResponseKind::NoContent => Ok((StatusCode::NO_CONTENT.into_response(), None)),
        ResponseKind::Redirect => Ok((redirect_response(state, session, result)?, None)),
        ResponseKind::Download => Ok((download_response(state, result)?, None)),
        ResponseKind::Stream => Ok((stream_response(result)?, None)),
        ResponseKind::Json => {
            // Wave 8 ADR 0007: a resource route whose Accept negotiation
            // matches answers with a JSON:API document; a matched entry
            // with media-type parameters refuses with a 406 through the
            // standard error path. Absent/q=0 falls through to today's
            // bytes (byte-identical default).
            if let Some(type_name) = &dispatch.resource_type {
                let accept = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok());
                match kelson_http::jsonapi::negotiate(accept) {
                    kelson_http::jsonapi::Negotiation::UnsupportedParameters => {
                        return Err(AppError::not_acceptable());
                    }
                    kelson_http::jsonapi::Negotiation::Matched => {
                        let split = kelson_http::jsonapi::split(&result);
                        let query = request_target
                            .split_once('?')
                            .map(|(_, query)| query)
                            .filter(|query| !query.is_empty());
                        let mut document = kelson_http::jsonapi::render_document(
                            type_name,
                            &split,
                            query,
                            request_target,
                        );
                        if dispatch.deprecated {
                            // Envelope parity: `meta.deprecated` rides the
                            // document the way it rides the envelope.
                            if let Some(document) = document.as_object_mut() {
                                let meta = document
                                    .entry("meta")
                                    .or_insert_with(|| serde_json::json!({}));
                                if let Some(meta) = meta.as_object_mut() {
                                    meta.insert(
                                        "deprecated".to_string(),
                                        serde_json::json!(true),
                                    );
                                }
                            }
                        }
                        let bytes = serde_json::to_vec(&document).map_err(|_| {
                            AppError::internal("JSON:API document could not be serialized")
                        })?;
                        let response = (
                            StatusCode::OK,
                            [(header::CONTENT_TYPE, kelson_http::jsonapi::MEDIA_TYPE)],
                            Body::from(bytes.clone()),
                        )
                            .into_response();
                        return Ok((response, Some(bytes)));
                    }
                    kelson_http::jsonapi::Negotiation::Absent => {}
                }
            }
            let mut envelope = Envelope::ok(result);
            if dispatch.deprecated {
                envelope = envelope.with_meta(serde_json::json!({ "deprecated": true }));
            }
            let bytes = serde_json::to_vec(&envelope)
                .map_err(|_| AppError::internal("envelope could not be serialized"))?;
            let response = (
                StatusCode::OK,
                [(header::CONTENT_TYPE, "application/json")],
                Body::from(bytes.clone()),
            )
                .into_response();
            Ok((response, Some(bytes)))
        }
    }
}

/// `response = "redirect"`: the operation result names the target — a bare
/// URL string (302) or `{ to, status?, flash? }`. `flash` entries land in
/// the caller's session and survive exactly one request.
fn redirect_response(
    state: &AppState,
    session: Option<ResolvedSession>,
    result: serde_json::Value,
) -> Result<axum::response::Response, kelson_foundation::AppError> {
    use kelson_foundation::AppError;
    let (target, status, flash) = match &result {
        serde_json::Value::String(to) => (to.clone(), StatusCode::FOUND, None),
        serde_json::Value::Object(map) => {
            let to = map
                .get("to")
                .and_then(|v| v.as_str())
                .ok_or_else(|| AppError::validation("redirect result is missing `to`"))?;
            let status = match map.get("status").and_then(|v| v.as_u64()) {
                Some(raw) => u16::try_from(raw)
                    .ok()
                    .and_then(|raw| StatusCode::from_u16(raw).ok())
                    .filter(|status| status.is_redirection())
                    .ok_or_else(|| {
                        AppError::validation("redirect status must be a 3xx code")
                    })?,
                None => StatusCode::FOUND,
            };
            let flash = map.get("flash").and_then(|v| v.as_object()).cloned();
            (to.to_string(), status, flash)
        }
        _ => {
            return Err(AppError::validation(
                "redirect result must be a URL string or an object with `to`",
            ))
        }
    };
    if target.is_empty() {
        return Err(AppError::validation("redirect target must not be empty"));
    }
    // A Location is a header value: CR/LF would be response splitting.
    let location = header::HeaderValue::from_str(&target)
        .map_err(|_| AppError::validation("redirect target must be a valid URL"))?;
    if let Some(flash) = flash {
        store_flash(state, session, &flash);
    }
    let mut response = axum::response::Response::new(Body::empty());
    *response.status_mut() = status;
    response.headers_mut().insert(header::LOCATION, location);
    Ok(response)
}

/// Copy `flash` entries into the caller's session (one-request lifetime).
/// Without a session there is nowhere durable to put them, so they are
/// dropped loudly rather than silently pretending to be flash data.
fn store_flash(
    state: &AppState,
    session: Option<ResolvedSession>,
    flash: &serde_json::Map<String, serde_json::Value>,
) {
    let (Some(middleware), Some(session)) = (state.sessions.as_ref(), session) else {
        tracing::warn!(
            target: "kelson::redirect",
            "redirect flash dropped: no session on this request"
        );
        return;
    };
    for (key, value) in flash {
        if let Err(error) = kelson_http::flash_put(
            &middleware.manager,
            &session.session.id,
            key,
            value.clone(),
        ) {
            tracing::warn!(
                target: "kelson::redirect",
                key = %key,
                error = %error.public_message,
                "flash value dropped"
            );
        }
    }
}

/// `response = "download"` (`file` alias): the result names a stored
/// object — `{ bucket, key, filename?, content_type? }` — and is served
/// from the StoragePort with an attachment disposition. No key, no bucket
/// access: the operation decides what the caller may take.
fn download_response(
    state: &AppState,
    result: serde_json::Value,
) -> Result<axum::response::Response, kelson_foundation::AppError> {
    use kelson_foundation::AppError;
    let map = result.as_object().ok_or_else(|| {
        AppError::validation("download result must be an object with `key`")
    })?;
    let bucket = map
        .get("bucket")
        .and_then(|v| v.as_str())
        .unwrap_or("uploads");
    let key = map
        .get("key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| AppError::validation("download result is missing `key`"))?;
    if key.is_empty() {
        return Err(AppError::validation("download key must not be empty"));
    }
    let object = kelson_storage::ObjectRef {
        bucket: bucket.to_string(),
        key: key.to_string(),
    };
    let bytes = state.storage.get(&object)?;

    let fallback_name = key.rsplit('/').next().unwrap_or(key);
    let filename = map
        .get("filename")
        .and_then(|v| v.as_str())
        .unwrap_or(fallback_name);
    let filename = kelson_http::sanitize_filename(filename);
    if filename.is_empty() {
        return Err(AppError::validation("download filename must not be empty"));
    }
    let content_type = map
        .get("content_type")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| content_type_for(&filename));
    let disposition = format!("attachment; filename=\"{filename}\"");

    let mut response = Body::from(bytes).into_response();
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_str(&content_type)
            .map_err(|_| AppError::validation("content_type is not a valid header value"))?,
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_str(&disposition)
            .map_err(|_| AppError::validation("filename is not a valid header value"))?,
    );
    Ok(response)
}

/// Content type for a downloaded object, by filename extension. Anything
/// unknown is served as `application/octet-stream` — never guessed from
/// content sniffing, and never `text/html` (that would be an XSS footgun
/// for a user-supplied file).
fn content_type_for(filename: &str) -> String {
    let extension = filename.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match extension.as_str() {
        "txt" | "log" => "text/plain; charset=utf-8",
        "html" | "htm" => "text/html; charset=utf-8",
        "json" => "application/json",
        "csv" => "text/csv; charset=utf-8",
        "md" => "text/markdown; charset=utf-8",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// `response = "stream"`: a chunked NDJSON body. An array result flushes
/// one line per element, a `{"chunks": [...]}` object one line per chunk,
/// anything else a single line — each line is its own HTTP chunk, so a
/// client sees records as they are produced instead of one buffered blob.
fn stream_response(
    result: serde_json::Value,
) -> Result<axum::response::Response, kelson_foundation::AppError> {
    let items: Vec<&serde_json::Value> = match &result {
        serde_json::Value::Array(items) => items.iter().collect(),
        serde_json::Value::Object(map) => match map.get("chunks") {
            Some(serde_json::Value::Array(chunks)) => chunks.iter().collect(),
            _ => vec![&result],
        },
        _ => vec![&result],
    };
    let lines: Vec<Result<axum::body::Bytes, std::convert::Infallible>> = items
        .into_iter()
        .map(|item| {
            let mut bytes = serde_json::to_vec(item).unwrap_or_default();
            bytes.push(b'\n');
            Ok(axum::body::Bytes::from(bytes))
        })
        .collect();
    let mut response = Body::from_stream(tokio_stream::iter(lines)).into_response();
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/x-ndjson"),
    );
    Ok(response)
}

// --------------------------------------------------- Wave 2 HTTP caching

/// Render a route's `CacheSpec` as a `Cache-Control` header value through
/// the shared builder (directives are never hand-concatenated at the edge).
fn cache_control_value(spec: &CacheSpec) -> String {
    let builder = match spec.policy {
        CachePolicy::NoStore => kelson_http::CacheControlBuilder::new().no_store(),
        CachePolicy::NoCache => kelson_http::CacheControlBuilder::new().no_cache(),
        CachePolicy::Private => kelson_http::CacheControlBuilder::new().private(),
        CachePolicy::Public => kelson_http::CacheControlBuilder::new().public(),
    };
    let builder = match spec.max_age_secs {
        Some(secs) => builder.max_age(secs),
        None => builder,
    };
    match spec.stale_while_revalidate_secs {
        Some(secs) => builder.stale_while_revalidate(secs),
        None => builder,
    }
    .build()
}

/// Per-route caching (19-PARITY "HTTP caching"): emit the route's
/// `Cache-Control`, add an ETag over the bytes actually served and answer
/// a matching `If-None-Match` with 304 + no body. GET routes only; only
/// buffered kinds (json/html) are ETagged — a streamed or stored-object
/// body is not hashed twice for a validator.
fn apply_caching(
    dispatch: &RouteDispatch,
    request_headers: &HeaderMap,
    response: axum::response::Response,
    buffer: Option<&[u8]>,
) -> axum::response::Response {
    let cache_control = dispatch.cache.map(|spec| cache_control_value(&spec));
    let mut response = response;
    if let Some(value) = &cache_control {
        if let Ok(value) = header::HeaderValue::from_str(value) {
            response.headers_mut().insert(header::CACHE_CONTROL, value);
        }
    }
    if !dispatch.is_get {
        return response;
    }
    let Some(bytes) = buffer else {
        return response;
    };
    let etag = kelson_http::etag_for(bytes);
    let conditional = request_headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| kelson_http::if_none_match_hits(value, &etag));

    if conditional {
        // 304: validators and policy only, never a body.
        let mut not_modified = axum::response::Response::new(Body::empty());
        *not_modified.status_mut() = StatusCode::NOT_MODIFIED;
        if let Ok(value) = header::HeaderValue::from_str(&etag) {
            not_modified.headers_mut().insert(header::ETAG, value);
        }
        if let Some(value) = &cache_control {
            if let Ok(value) = header::HeaderValue::from_str(value) {
                not_modified
                    .headers_mut()
                    .insert(header::CACHE_CONTROL, value);
            }
        }
        return not_modified;
    }
    if let Ok(value) = header::HeaderValue::from_str(&etag) {
        response.headers_mut().insert(header::ETAG, value);
    }
    response
}

/// The module-lifecycle registry key for one operation (docs/15-EXTENSION
/// dispatch wiring). The registry is keyed by *provider* name
/// (`module.appointments`) while an operation id names its module by
/// *manifest* (`{module.name()}.{operation}`, built at registration), so the
/// operation's first segment prefixed with `module.` is the key — the naming
/// every manifest-shipping provider follows (`applications/appointments`,
/// `applications/oauth2`). A dotless or dotted-module operation yields a key
/// no provider registers, which the gate treats as untracked (dispatch
/// proceeds, the safe failure mode).
fn registry_key_for(operation: &OperationId) -> String {
    let module = operation.as_str().split('.').next().unwrap_or_default();
    format!("module.{module}")
}

/// Generic dispatcher for one graph route (Wave 1 slim edge). Identity,
/// tenant resolution and the email-verified gate are handled by the route's
/// middleware chain before this runs; dispatch consumes the installed
/// ResolvedPrincipal extension. Anonymous reach only happens on public
/// routes, and only when the handler plan does not require authorization.
async fn graph_dispatch(
    State((state, dispatch)): State<(AppState, Arc<RouteDispatch>)>,
    headers: HeaderMap,
    AxumPath(path_params): AxumPath<Vec<(String, String)>>,
    req: Request<Body>,
) -> axum::response::Response {
    // Admission inserts RequestId/TraceId extensions; degrade gracefully when
    // tests hit the router without the admission pipeline.
    let request_id = req.extensions().get::<kelson_foundation::RequestId>().copied();
    let trace_id = req
        .extensions()
        .get::<kelson_foundation::TraceId>()
        .copied()
        .unwrap_or_else(kelson_foundation::TraceId::new);
    let resolved = req.extensions().get::<ResolvedPrincipal>().cloned();
    // **F-7**: the credential's narrowing, captured before the principal is
    // moved out of `resolved` ✓. An empty list means no narrowing was
    // granted ✓ — the credential then acts as the principal ✓ (the old
    // behaviour ✓), rather than as nothing ✓.
    let credential_abilities = resolved
        .as_ref()
        .and_then(|r| r.abilities.clone())
        .filter(|list| !list.is_empty());
    // Wave 2: the host gate resolves a route's `subdomain` into a tenant
    // record before auth; it feeds resolution below the principal's own
    // tenant (identity wins, so a session can never be promoted into
    // another tenant's context) and above the configured default.
    let host_tenant = req.extensions().get::<HostTenant>().cloned();
    let host_tenant_id = host_tenant.as_ref().map(|host| host.0.id);
    // Wave 2: redirect routes carry flash data onto the caller's session.
    let resolved_session = req.extensions().get::<ResolvedSession>().cloned();
    // Wave 2: `_auth` view convention + error-view path context.
    let authenticated = resolved.is_some();

    let uri = req.uri().clone();
    let path = uri.path().to_string();
    // The full request target (path?query) — JSON:API links.self and
    // pagination rewriting read it verbatim (ADR 0007).
    let request_target = match uri.query() {
        Some(query) => format!("{path}?{query}"),
        None => path.clone(),
    };
    // Wave 8 ADR 0008 (Precognition): validate-only dispatch. The header
    // name matches ASCII case-insensitively (HeaderMap); the mode activates
    // only for the value `true` (trimmed, ASCII case-insensitive) — any
    // other value is treated as absent and never fails a request. Graph
    // routes only; platform routes never read it.
    let precognition = headers
        .get("precognition")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("true"));
    // Module-lifecycle admission (docs/15-EXTENSION dispatch wiring): a
    // tracked module that is not serving refuses its own routes here —
    // before body intake (an offline module stores no uploads and burns no
    // validation) and before dispatch's own principal resolution. Note the
    // ordering at large: the auth chain layers OUTSIDE dispatch, so an
    // anonymous caller on a non-public route still meets the chain's 401
    // first — its contract — while public routes and authenticated
    // callers reach this refusal. Module availability itself is public
    // posture anyway: `/readyz` publishes the same states. Degraded keeps
    // serving: availability over consistency, the same posture the
    // readiness edge documents. A module the registry does not know — any
    // harness boot, or a provider that skips the `module.<name>` naming
    // convention — dispatches exactly as before; the gate only ever fires
    // for a module the lifecycle is actually tracking.
    let registry_key = registry_key_for(&dispatch.operation);
    let not_serving = state
        .modules
        .read()
        .unwrap()
        .state(&registry_key)
        .is_some_and(|module_state| !module_state.is_serving());
    if not_serving {
        let err = kelson_foundation::AppError::new(
            kelson_foundation::codes::MODULE_NOT_SERVING,
            kelson_foundation::ErrorCategory::ResourceExhausted,
            kelson_foundation::Severity::Low,
            kelson_foundation::Retryability::Retryable,
            format!("{registry_key} is not serving"),
        );
        return dispatch_error(&state, &dispatch, &headers, &path, &err);
    }
    let method = req.method().clone();
    let body = match axum::body::to_bytes(req.into_body(), state.body_limit).await {
        Ok(bytes) => bytes,
        Err(_) => {
            let err = kelson_foundation::AppError::validation("request body too large");
            return dispatch_error(&state, &dispatch, &headers, &path, &err);
        }
    };

    // Resolve the principal: the auth stage installed one, or the route is
    // public and the handler plan tolerates anonymous callers.
    let (principal, tenant) = match resolved {
        Some(r) => {
            let tenant = r.tenant.or(host_tenant_id).or(state.default_tenant_id);
            let Some(tenant) = tenant else {
                let err = kelson_foundation::AppError::unauthenticated();
                return dispatch_error(&state, &dispatch, &headers, &path, &err);
            };
            (r.principal, tenant)
        }
        None => {
            let required = state
                .bus
                .handler(&dispatch.operation)
                .map(|h| h.plan().authorization.required)
                .unwrap_or(true);
            if required {
                let err = kelson_foundation::AppError::unauthenticated();
                return dispatch_error(&state, &dispatch, &headers, &path, &err);
            }
            let Some(tenant) = host_tenant_id.or(state.default_tenant_id) else {
                let err = kelson_foundation::AppError::unauthenticated();
                return dispatch_error(&state, &dispatch, &headers, &path, &err);
            };
            (crate::host::anonymous_principal(), tenant)
        }
    };

    // Body intake (Wave 4 Phase 3): JSON for the plain path; mixed multipart
    // when the request carries files — text fields become the input object
    // and file parts store through the StoragePort as `UploadMeta` for the
    // file rules. Empty body -> {} ; malformed -> 422 envelope.
    let is_multipart = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("multipart/form-data"));
    // Wave 8 ADR 0008: multipart intake stores upload parts BEFORE the
    // validator runs, so a precognition probe cannot be allowed to parse
    // the body — fail closed with the documented refusal instead (JSON and
    // form-urlencoded bodies intake normally below).
    if precognition && is_multipart {
        let err = kelson_foundation::AppError::validation(
            "precognition does not accept multipart bodies",
        );
        return dispatch_error(&state, &dispatch, &headers, &path, &err);
    }
    // Wave 8 P5 (B-21): RFC 6749 sec 4.1.3 / sec 4.1.2 — the OAuth2 token
    // and revoke endpoints MUST accept `application/x-www-form-urlencoded`
    // bodies (that is the wire format RFC 8252 native apps and every
    // conforming client library send). Distinct from the query string,
    // which has never percent-decoded: form bodies decode (`+` and `%XX`,
    // the HTML form encoding) and values stay STRINGS (`$_POST` posture).
    let is_form = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("application/x-www-form-urlencoded"));
    let (input, uploads): (serde_json::Value, Vec<kelson_validation::UploadMeta>) = if is_multipart
    {
        let content_type = headers[header::CONTENT_TYPE]
            .to_str()
            .unwrap_or_default()
            .to_string();
        let multipart_req = Request::builder()
            .method(method)
            .uri(uri.clone())
            .header(header::CONTENT_TYPE, content_type)
            .body(Body::from(body))
            .expect("multipart request rebuilds");
        let multipart = match axum::extract::Multipart::from_request(multipart_req, &()).await {
            Ok(mp) => mp,
            Err(err) => {
                let err = kelson_foundation::AppError::validation(format!(
                    "multipart request required: {err}"
                ));
                return dispatch_error(&state, &dispatch, &headers, &path, &err);
            }
        };
        let prefix = format!("{}/", principal.as_uuid());
        let (text_fields, stored) = match kelson_http::ingest_multipart_mixed(
            multipart,
            &kelson_http::UploadLimits::default(),
            &*state.storage,
            state.disks.default_name(),
            "uploads",
            &prefix,
        )
        .await
        {
            Ok(v) => v,
            Err(err) => return dispatch_error(&state, &dispatch, &headers, &path, &err),
        };
        let mut obj = serde_json::Map::new();
        for (key, raw) in text_fields {
            // Form fields arrive as strings; JSON-shaped payloads parse back
            // into typed values so rules and handlers see the same input
            // they would from a JSON body (non-JSON text stays a string).
            let value =
                serde_json::from_str(&raw).unwrap_or(serde_json::Value::String(raw));
            obj.insert(key, value);
        }
        let metas: Vec<kelson_validation::UploadMeta> = stored
            .into_iter()
            .map(|s| kelson_validation::UploadMeta {
                field: s.field,
                filename: s.filename,
                content_type: s.content_type,
                size: s.size as u64,
                width: s.width,
                height: s.height,
            })
            .collect();
        (serde_json::Value::Object(obj), metas)
    } else if is_form {
        let raw = match std::str::from_utf8(&body) {
            Ok(raw) => raw,
            Err(_) => {
                let err = kelson_foundation::AppError::validation("form body must be UTF-8");
                return dispatch_error(&state, &dispatch, &headers, &path, &err);
            }
        };
        let mut obj = serde_json::Map::new();
        for (key, value) in query_pairs(Some(raw)) {
            obj.insert(key, serde_json::Value::String(form_decode(&value)));
        }
        (serde_json::Value::Object(obj), Vec::new())
    } else if body.is_empty() {
        (serde_json::json!({}), Vec::new())
    } else {
        match serde_json::from_slice(&body) {
            Ok(value) => (value, Vec::new()),
            Err(_) => {
                let err = kelson_foundation::AppError::validation("request body must be valid JSON");
                return dispatch_error(&state, &dispatch, &headers, &path, &err);
            }
        }
    };

    let query = query_pairs(uri.query());
    let mut input = merge_params(input, path_params.clone(), query.clone());

    // Wave 1 pagination: GET routes parse page/per_page (clamped to
    // MAX_PER_PAGE) and inject _page. Parse errors are 422 envelopes.
    if dispatch.inject_page {
        let pairs: Vec<(&str, &str)> = query
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        match kelson_http::PageQuery::from_pairs(pairs) {
            Ok(page) => {
                if let Some(obj) = input.as_object_mut() {
                    // The cursor rides along with the page numbers: a module
                    // that reads `_page.cursor` can page by keyset (the
                    // opaque, signed anchor) instead of by offset.
                    obj.insert(
                        "_page".into(),
                        serde_json::json!({
                            "page": page.page,
                            "per_page": page.per_page,
                            "cursor": page.cursor,
                        }),
                    );
                }
            }
            Err(err) => return dispatch_error(&state, &dispatch, &headers, &path, &err),
        }
    }

    // Wave 1 model binding: each declared {param} binding resolves through
    // the route.bindings registry (tenant-scoped find). Resolved records are
    // injected AFTER the merge so bodies cannot forge _bindings.<param>.
    // Misses, malformed ids and missing resolvers are 404 envelopes.
    if !dispatch.bindings.is_empty() {
        let obj = match input.as_object_mut() {
            Some(obj) => obj,
            None => {
                let err = kelson_foundation::AppError::not_found("binding target not found");
                return dispatch_error(&state, &dispatch, &headers, &path, &err);
            }
        };
        let mut bindings_map = serde_json::Map::new();
        for binding in &dispatch.bindings {
            let raw = path_params
                .iter()
                .find(|(k, _)| k == &binding.param)
                .map(|(_, v)| v.clone());
            let Some(raw) = raw else {
                let err = kelson_foundation::AppError::not_found(format!(
                    "route parameter {} is missing",
                    binding.param
                ));
                return dispatch_error(&state, &dispatch, &headers, &path, &err);
            };
            let Ok(id) = uuid::Uuid::parse_str(&raw) else {
                let err = kelson_foundation::AppError::not_found(format!(
                    "{} {} not found",
                    binding.model, raw
                ));
                return dispatch_error(&state, &dispatch, &headers, &path, &err);
            };
            let Some(resolver) = state.route_bindings.get(&binding.model) else {
                let err = kelson_foundation::AppError::not_found(format!(
                    "{} {} not found",
                    binding.model, raw
                ));
                return dispatch_error(&state, &dispatch, &headers, &path, &err);
            };
            // Wave 3 Phase 6: `with_trashed` bindings resolve soft-deleted
            // rows through the resolver's trashed-aware hook (fail-closed
            // default when the resolver does not implement it).
            let resolved = kelson_models::resolve_binding(
                resolver.as_ref(),
                binding.with_trashed,
                Some(tenant),
                id,
            );
            match resolved {
                Ok(Some(record)) => {
                    bindings_map.insert(binding.param.clone(), serde_json::Value::Object(record));
                }
                Ok(None) | Err(_) => {
                    let err = kelson_foundation::AppError::not_found(format!(
                        "{} {} not found",
                        binding.model, raw
                    ));
                    return dispatch_error(&state, &dispatch, &headers, &path, &err);
                }
            }
        }
        obj.insert(
            "_bindings".into(),
            serde_json::Value::Object(bindings_map),
        );
    }

    // Wave 1 FormRequest wiring: manifest validation runs collect-all over
    // the merged input BEFORE dispatch (422 envelope with fields meta).
    // Wave 4 Phase 3: the context carries the principal, the credential
    // verifier (`current_password`) and any multipart upload parts (`file`
    // family) — all read-only, all fail-closed when absent.
    // Wave 4 Phase 4: plus the request tenant + row probes (`unique`/
    // `exists`, probe failure → 503 via the fatal channel) and the
    // process-global custom-rule registry (`custom = "<name>"`).
    // Wave 6 P3: resolve the request locale once — session preference ->
    // the tenant's locale (LF-078) -> Accept-Language (first supported,
    // quality-ordered) -> configured default — and carry it through
    // validation wording, the execution context, and the view
    // `_locale`/`_lang` injections below.
    let locale = crate::i18n::resolve(
        &state.messages,
        crate::i18n::session_locale(resolved_session.as_ref()),
        state.tenant_locale(Some(tenant)).as_deref(),
        headers
            .get(header::ACCEPT_LANGUAGE)
            .and_then(|v| v.to_str().ok()),
    );

    if let Some(validator) = &dispatch.validator {
        let credentials = state.credentials.clone();
        let for_principal = principal;
        let verifier = move |candidate: &str| -> bool {
            match credentials.password_for(for_principal) {
                Ok(Some(credential)) => kelson_identity::verify_credential(
                    &credential,
                    &kelson_identity::Secret::new(candidate),
                )
                .unwrap_or(false),
                _ => false,
            }
        };
        let probes = EdgeProbes(&state.probes);
        let mut ctx = kelson_validation::ValidationContext::new(&input)
            .with_principal(principal)
            .with_verifier(&verifier)
            .with_tenant(Some(tenant))
            .with_db_probes(&probes)
            .with_custom_rules(&kelson_validation::GLOBAL_CUSTOM_RULES)
            .with_messages(&state.messages, &locale)
            // Wave 7 P6: the `password` rule reads the configured
            // `[auth.password_policy]` — the same policy register/reset
            // enforce — so every dispatch context sees one posture.
            .with_password_policy(state.password_policy);
        if !uploads.is_empty() {
            ctx = ctx.with_uploads(&uploads);
        }
        if let Err(report) = validator.validate_with(&ctx) {
            let err = report.into_app_error();
            return dispatch_error(&state, &dispatch, &headers, &path, &err);
        }
    }

    // Wave 8 ADR 0008: validation passed (or the route declares nothing to
    // validate — a vacuous pass), so a precognition probe answers HERE.
    // Side-effect freedom is structural: the idempotency-key read, the
    // `ExecutionContext`, `CommandBus::dispatch` and every mutation/audit/
    // outbox/job/token/trace path below this line do not exist for the
    // probe. The 204 carries no body and no special headers.
    if precognition {
        return StatusCode::NO_CONTENT.into_response();
    }

    let idempotency = headers
        .get(header::HeaderName::from_static("idempotency-key"))
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty() && v.len() <= 128)
        .map(IdempotencyKey);

    let now = OffsetDateTime::now_utc();
    // Views edge convention: `_auth` is the principal id when an auth stage
    // installed one; anonymous callers stay guests (@guest renders).
    let view_auth = authenticated.then(|| principal.to_string());
    let mut builder = ExecutionContext::builder(dispatch.profile, now)
        .plane(dispatch.plane)
        .trace_id(trace_id)
        .principal(PrincipalContext {
            id: principal,
            display: None,
        })
        // Wave 6 P3: the resolved request locale rides the context; the
        // timezone stays the bootstrap default so business-day semantics
        // never change with language negotiation.
        .locale(kelson_foundation::Locale {
            lang: locale.clone(),
            timezone: kelson_foundation::Locale::default().timezone,
        });
    // **F-7**: the credential's narrowing rides the context, so the
    // authorization boundary can confine a personal access token to the
    // abilities it was issued with.
    if let Some(abilities) = credential_abilities.clone() {
        builder = builder.with_credential_abilities(abilities);
    }
    if let Some(request_id) = request_id {
        builder = builder.request_id(request_id);
    }
    if dispatch.plane == SecurityPlane::Tenant {
        // A host-fed tenant contributes its real slug and plan (the slug
        // the caller's hostname resolved to); anything else keeps the
        // bootstrap defaults.
        let (slug, plan) = match host_tenant.as_ref() {
            Some(host) if host.0.id == tenant => (host.0.slug.clone(), host.0.plan.clone()),
            _ => ("default".into(), "standard".into()),
        };
        builder = builder
            .tenant(TenantContext {
                id: tenant,
                slug,
                plan,
            })
            .scope(ScopeSnapshot {
                nodes: vec![],
                revision: AuthorizationRevision(1),
                computed_at: now,
            });
    }
    let ctx = match builder.build() {
        Ok(ctx) => ctx,
        Err(err) => return dispatch_error(&state, &dispatch, &headers, &path, &err),
    };

    match state
        .bus
        .dispatch(ctx, &dispatch.operation, input, idempotency)
        .await
    {
        Ok(outcome) => {
            let (response, buffer) = match render_success(
                &state,
                &dispatch,
                &headers,
                &request_target,
                RenderCaller {
                    session: resolved_session,
                    tenant: Some(tenant),
                    view_auth,
                },
                outcome.result,
            ) {
                Ok(rendered) => rendered,
                Err(err) => return dispatch_error(&state, &dispatch, &headers, &path, &err),
            };
            // Cache-Control + ETag/304 first so a 304 still carries the
            // route's policy and the Deprecation notice.
            let mut response = apply_caching(&dispatch, &headers, response, buffer.as_deref());
            if dispatch.deprecated {
                response.headers_mut().insert(
                    header::HeaderName::from_static("deprecation"),
                    header::HeaderValue::from_static("true"),
                );
            }
            response
        }
        Err(err) => dispatch_error(&state, &dispatch, &headers, &path, &err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::HttpBody;

    #[test]
    fn axum_path_converts_braces() {
        assert_eq!(
            axum_path("/api/v1/appointments/{id}"),
            "/api/v1/appointments/{id}"
        );
        assert_eq!(axum_path("/a/{x}/b/{y}"), "/a/{x}/b/{y}");
        assert_eq!(axum_path("/plain"), "/plain");
    }

    #[test]
    fn registry_key_follows_provider_naming() {
        // The operation id carries its manifest module as the first segment
        // (`{module.name()}.{operation}`, built at registration); the
        // lifecycle registry keys the provider `module.<name>`.
        assert_eq!(
            registry_key_for(&OperationId::from_static("appointments.book")),
            "module.appointments"
        );
        // A dotless operation maps to a key no provider registers — the
        // gate treats it as untracked and dispatch proceeds.
        assert_eq!(
            registry_key_for(&OperationId::from_static("solo")),
            "module.solo"
        );
    }

    #[test]
    fn method_filter_mapping() {
        // GET answers HEAD too (auto-registration); ANY is Route::any's set.
        let (declared, filter) = route_methods("get").expect("get");
        assert_eq!(declared, vec!["GET", "HEAD"]);
        assert_eq!(
            filter,
            MethodFilter::GET.or(MethodFilter::HEAD),
            "the filter is the union of the declared methods"
        );
        let (declared, _) = route_methods("ANY").expect("any");
        assert_eq!(
            declared,
            vec!["GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"]
        );
        assert!(route_methods("POST").is_some());
        assert!(route_methods("TRACE").is_none());
        assert!(route_methods("nope").is_none());
    }

    #[test]
    fn host_label_strips_port_and_lowercases() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::HOST,
            header::HeaderValue::from_static("API.Example.COM:8080"),
        );
        assert_eq!(host_label(&headers), "api.example.com");
        assert_eq!(host_label(&HeaderMap::new()), "", "no Host -> no match");
    }

    #[test]
    fn host_matching_rules() {
        assert!(domain_matches("api.example.com", "api.example.com"));
        assert!(domain_matches("api.example.com", "API.Example.COM"));
        assert!(!domain_matches("api.example.com", "portal.example.com"));
        // Wildcards cover any subdomain but never the apex itself.
        assert!(domain_matches("a.example.com", "*.example.com"));
        assert!(domain_matches("a.b.example.com", "*.example.com"));
        assert!(!domain_matches("example.com", "*.example.com"));
        assert!(!domain_matches("notexample.com", "*.example.com"));

        assert!(subdomain_matches("acme.example.com", "acme"));
        assert!(subdomain_matches("ACME.example.com", "acme"));
        assert!(!subdomain_matches("acme.example.com", "globex"));
        assert!(
            !subdomain_matches("acme", "acme"),
            "a bare host carries no parent domain"
        );
        assert!(!subdomain_matches("", "acme"));
    }

    #[test]
    fn merge_params_body_wins() {
        let input = serde_json::json!({ "id": "from-body" });
        let merged = merge_params(
            input,
            vec![
                ("id".into(), "from-path".into()),
                ("extra".into(), "p".into()),
            ],
            vec![("q".into(), "1".into())],
        );
        assert_eq!(merged["id"], "from-body");
        assert_eq!(merged["extra"], "p");
        assert_eq!(merged["q"], "1");
        // Non-object bodies are replaced, not wrapped.
        let merged = merge_params(
            serde_json::json!([1, 2]),
            vec![("a".into(), "b".into())],
            vec![],
        );
        assert_eq!(merged["a"], "b");
    }

    #[test]
    fn tenant_label_parsing() {
        let id = uuid::Uuid::now_v7();
        assert_eq!(
            tenant_from_label(&Some(format!("tenant:{id}"))).map(|t| *t.as_uuid()),
            Some(id)
        );
        assert_eq!(
            tenant_from_label(&Some("tenant:not-a-uuid".to_string())),
            None
        );
        assert_eq!(tenant_from_label(&Some("ci".to_string())), None);
        assert_eq!(tenant_from_label(&None), None);
    }

    #[test]
    fn query_pairs_skip_garbage() {
        let pairs = query_pairs(Some("a=1&=x&b&c=3=4"));
        assert_eq!(
            pairs,
            vec![("a".into(), "1".into()), ("c".into(), "3=4".into())]
        );
        assert!(query_pairs(None).is_empty());
    }

    #[test]
    fn form_decode_applies_form_urlencoding() {
        // Wave 8 P5 (B-21): OAuth2 form bodies decode per HTML form rules.
        assert_eq!(form_decode("plain"), "plain");
        assert_eq!(form_decode("a%20b"), "a b");
        assert_eq!(form_decode("a+b"), "a b");
        assert_eq!(
            form_decode("https%3A%2F%2Fclient.example%2Fcb"),
            "https://client.example/cb"
        );
        assert_eq!(form_decode("%41%42"), "AB");
        // Tolerance: a malformed escape stays literal instead of failing.
        assert_eq!(form_decode("100%"), "100%");
        assert_eq!(form_decode("%zz"), "%zz");
    }

    // ------------------------------------------------------- route cache pin

    const ALPHA_MANIFEST: &str = r#"
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
"#;

    fn manifest_dir(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kelson-demo-routecache-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let app_dir = dir.join("applications").join("alpha");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(app_dir.join("module.toml"), ALPHA_MANIFEST).unwrap();
        dir
    }

    #[test]
    fn route_cache_pin_accepts_match_rejects_drift_and_clears() {
        let dir = manifest_dir("pin");
        let graph = compile_graph(&dir).expect("graph compiles");
        let catalog = kelson_http::MiddlewareCatalog::builtin();
        let plans = kelson_registry::resolve_route_plans(&graph, &catalog).expect("plans resolve");
        let artifact = kelson_registry::RouteCacheArtifact {
            module_abi: kelson_registry::MODULE_ABI,
            manifest_hash: kelson_registry::manifests_hash(&dir).expect("hash computes"),
            plans,
        };
        artifact
            .write(&kelson_registry::route_cache_path(&dir))
            .expect("artifact writes");

        // Matching artifact -> boot proceeds from the cache.
        let cached = route_cache_check(&dir).expect("cache accepted").expect("plans present");
        assert_eq!(cached.len(), 1);

        // Manifest drift -> boot refused with a pointer to route:cache.
        let manifest_path = dir.join("applications").join("alpha").join("module.toml");
        let raw = std::fs::read_to_string(&manifest_path).unwrap();
        std::fs::write(&manifest_path, format!("{raw}\n# drift\n")).unwrap();
        let err = route_cache_check(&dir).expect_err("stale cache refuses boot");
        assert!(err.contains("route:cache"), "{err}");

        // route:clear semantics -> absent artifact means live compile again.
        std::fs::remove_file(kelson_registry::route_cache_path(&dir)).unwrap();
        assert!(route_cache_check(&dir).expect("absent ok").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_route_cache_refuses_boot() {
        let dir = manifest_dir("corrupt");
        let path = kelson_registry::route_cache_path(&dir);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();
        assert!(route_cache_check(&dir).is_err(), "corrupt artifact is a boot error");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn package_discovery_merges_indexed_manifests_and_fails_closed() {
        let dir = manifest_dir("pkgdisc");
        // No index: applications only.
        let found = discover_manifests(&dir).expect("discovers");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name(), "alpha");

        // Index a package manifest -> merged into the same fail-closed
        // discovery that feeds graph compile (Wave 8 B-24).
        std::fs::create_dir_all(dir.join("packages").join("billing")).unwrap();
        std::fs::write(
            dir.join("packages").join("billing").join("module.toml"),
            "[module]\nname = \"billing\"\nversion = \"0.1.0\"\napi = 1\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("packages.toml"),
            "[[package]]\nname = \"billing\"\nversion = \"0.1.0\"\npath = \"packages/billing\"\n",
        )
        .unwrap();
        let found = discover_manifests(&dir).expect("discovers packages");
        let names: Vec<&str> = found.iter().map(|m| m.name()).collect();
        assert_eq!(names, vec!["alpha", "billing"]);

        // A missing package manifest refuses discovery — packages never
        // silently vanish from the graph.
        std::fs::remove_file(dir.join("packages").join("billing").join("module.toml")).unwrap();
        let err = discover_manifests(&dir).expect_err("missing manifest refuses");
        assert!(err.contains("package billing"), "{err}");

        std::fs::remove_dir_all(&dir).ok();
    }

    // ------------------------------------------------------------- edge auth

    #[test]
    fn edge_auth_resolves_pat_then_jwt_and_installs_snapshot() {
        let tokens = Arc::new(kelson_tokens::TokenService::new(Arc::new(
            kelson_tokens::InMemoryTokenStore::new(),
        )));
        let principals: Arc<dyn kelson_identity::PrincipalStore> =
            Arc::new(kelson_identity::InMemoryPrincipalStore::new());
        let jwt = kelson_http::JwtHs256::new(b"edge-auth-unit-secret-0123456789".to_vec());
        let policy = Arc::new(kelson_policy::PolicyEngine::new());
        let auth = EdgeAuth {
            tokens: tokens.clone(),
            principals,
            jwt: Some(Arc::new(jwt.clone())),
            policy: policy.clone(),
            granted_capabilities: vec!["appointments.list".into()],
            default_tenant_id: None,
        };

        let principal = PrincipalId::new();
        let tenant = TenantId::new();

        // PAT: resolves with the tenant label; NO snapshot install (login-time
        // snapshot semantics, pat_bypasses_snapshot contract).
        let issued = tokens
            .issue(
                principal,
                kelson_tokens::TokenPurpose::Api,
                time::Duration::days(30),
                Some(format!("tenant:{tenant}")),
            )
            .expect("pat issues");
        let resolved = auth.resolve_bearer(&issued.plaintext).expect("pat resolves");
        assert_eq!(resolved.principal, principal);
        assert_eq!(resolved.tenant, Some(tenant));
        assert_eq!(resolved.via, AuthVia::Bearer);
        assert!(
            policy.snapshot(principal, tenant).is_none(),
            "PAT resolution must not install a snapshot"
        );

        // JWT: resolves and hydrates the access snapshot at resolution time.
        let claims = kelson_http::JwtClaims {
            sub: principal.to_string(),
            tenant: Some(tenant.to_string()),
            exp: (OffsetDateTime::now_utc() + time::Duration::hours(1)).unix_timestamp(),
            iat: None,
            jti: None,
        };
        let token = jwt.sign(&claims);
        let resolved = auth.resolve_bearer(&token).expect("jwt resolves");
        assert_eq!(resolved.principal, principal);
        assert_eq!(resolved.tenant, Some(tenant));
        assert_eq!(resolved.via, AuthVia::Jwt);
        assert!(
            policy.snapshot(principal, tenant).is_some(),
            "JWT resolution hydrates the access snapshot"
        );

        // Garbage: uniform None (the edge maps it to 401).
        assert!(auth.resolve_bearer("garbage-token").is_none());
    }

    // ------------------------------------------ Wave 2 response kinds (unit)

    /// State only — the response-kind helpers take `&AppState`, not a graph.
    fn response_state(tag: &str) -> crate::routes::AppState {
        let dir = std::env::temp_dir().join(format!("kelson-resp-{tag}-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let (state, _, _) = crate::build_app_state(&dir, true, true);
        state
    }

    fn dispatch(response: ResponseKind, is_get: bool, cache: Option<CacheSpec>) -> RouteDispatch {
        RouteDispatch {
            operation: OperationId::from_static("test.op"),
            profile: ExecutionProfileKind::Command,
            plane: SecurityPlane::Tenant,
            public: true,
            deprecated: false,
            inject_page: is_get,
            resource_type: None,
            bindings: Vec::new(),
            validator: None,
            route_plane: RoutePlane::Api,
            response,
            view: None,
            is_get,
            cache,
        }
    }

    fn header_of(response: &axum::response::Response, name: header::HeaderName) -> Option<String> {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    }

    #[test]
    fn redirect_targets_come_from_the_operation_result() {
        let state = response_state("redirect");
        // A bare URL string redirects with the default 302 Found.
        let response =
            redirect_response(&state, None, serde_json::json!("/appointments")).expect("target");
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(
            header_of(&response, header::LOCATION).as_deref(),
            Some("/appointments")
        );

        // The object form carries the target plus an explicit 3xx status.
        let response = redirect_response(
            &state,
            None,
            serde_json::json!({ "to": "https://portal.test/dashboard", "status": 303 }),
        )
        .expect("target");
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            header_of(&response, header::LOCATION).as_deref(),
            Some("https://portal.test/dashboard")
        );
        // A redirect body is empty: nothing to render, nothing to cache.
        assert!(response.body().size_hint().exact() == Some(0));
    }

    #[test]
    fn redirect_refuses_non_3xx_statuses_and_header_injection() {
        let state = response_state("redirect-reject");
        let err = redirect_response(&state, None, serde_json::json!({ "to": "/x", "status": 200 }))
            .expect_err("200 is not a redirect");
        assert!(err.public_message.contains("3xx"), "{}", err.public_message);
        let err = redirect_response(&state, None, serde_json::json!({ "to": "/x", "status": 999 }))
            .expect_err("999 is not a status code");
        assert!(err.public_message.contains("3xx"), "{}", err.public_message);
        let err = redirect_response(
            &state,
            None,
            serde_json::json!({ "to": "https://evil.test/\r\nSet-Cookie: pwn=1" }),
        )
        .expect_err("CRLF would split the response");
        assert!(err.public_message.contains("valid URL"), "{}", err.public_message);
        let err = redirect_response(&state, None, serde_json::json!({ "target": "/x" }))
            .expect_err("`to` is the key");
        assert!(err.public_message.contains("`to`"), "{}", err.public_message);
        let err = redirect_response(&state, None, serde_json::json!(""))
            .expect_err("empty target");
        assert!(err.public_message.contains("empty"), "{}", err.public_message);
    }

    #[tokio::test]
    async fn redirect_flash_survives_exactly_one_next_request() {
        let state = response_state("flash");
        let middleware = state.sessions.as_ref().expect("sessions enabled");
        let session = middleware
            .manager
            .start(PrincipalId::new(), None, OffsetDateTime::now_utc())
            .expect("session starts");

        let response = redirect_response(
            &state,
            Some(ResolvedSession {
                session: session.clone(),
            }),
            serde_json::json!({
                "to": "/appointments",
                "flash": { "status": "booked", "seats": 2 }
            }),
        )
        .expect("redirect builds");
        assert_eq!(response.status(), StatusCode::FOUND);
        assert_eq!(
            header_of(&response, header::LOCATION).as_deref(),
            Some("/appointments")
        );

        assert_eq!(
            kelson_http::flash_take(&middleware.manager, &session.id, "status").expect("read"),
            Some(serde_json::json!("booked")),
            "flash is readable on the very next request"
        );
        assert_eq!(
            kelson_http::flash_take(&middleware.manager, &session.id, "status").expect("read"),
            None,
            "and gone after one read"
        );
        assert_eq!(
            kelson_http::flash_take(&middleware.manager, &session.id, "seats").expect("read"),
            Some(serde_json::json!(2))
        );
    }

    #[tokio::test]
    async fn download_serves_a_stored_object_as_an_attachment() {
        let state = response_state("download");
        state
            .storage
            .put("exports", "reports/appointments.csv", b"slot,patient\n09:00,ada\n".to_vec())
            .expect("object stores");

        let response = download_response(
            &state,
            serde_json::json!({ "bucket": "exports", "key": "reports/appointments.csv" }),
        )
        .expect("download builds");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header_of(&response, header::CONTENT_TYPE).as_deref(),
            Some("text/csv; charset=utf-8"),
            "content type follows the filename extension"
        );
        assert_eq!(
            header_of(&response, header::CONTENT_DISPOSITION).as_deref(),
            Some("attachment; filename=\"appointments.csv\""),
            "the filename falls back to the last key segment"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        assert_eq!(&body[..], b"slot,patient\n09:00,ada\n");

        // Explicit content_type and filename win over the defaults.
        let response = download_response(
            &state,
            serde_json::json!({
                "bucket": "exports",
                "key": "reports/appointments.csv",
                "filename": "slots.data",
                "content_type": "application/x-custom"
            }),
        )
        .expect("download builds");
        assert_eq!(
            header_of(&response, header::CONTENT_TYPE).as_deref(),
            Some("application/x-custom")
        );
        assert_eq!(
            header_of(&response, header::CONTENT_DISPOSITION).as_deref(),
            Some("attachment; filename=\"slots.data\"")
        );

        // Missing key: a download route never guesses what to hand over.
        let err = download_response(&state, serde_json::json!({ "bucket": "exports" }))
            .expect_err("no key");
        assert!(err.public_message.contains("`key`"), "{}", err.public_message);
        // Unknown object: the storage error surfaces as the route's error.
        let err = download_response(
            &state,
            serde_json::json!({ "bucket": "exports", "key": "absent.csv" }),
        )
        .expect_err("object is absent");
        assert_eq!(
            kelson_http::status_for_error(&err),
            StatusCode::NOT_FOUND,
            "{}",
            err.public_message
        );
    }

    #[tokio::test]
    async fn stream_flushes_one_ndjson_line_per_element() {
        let response = stream_response(serde_json::json!([
            { "slot": "09:00" },
            { "slot": "10:00" }
        ]))
        .expect("stream builds");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            header_of(&response, header::CONTENT_TYPE).as_deref(),
            Some("application/x-ndjson"),
            "NDJSON, not the Envelope"
        );
        assert!(
            response.body().size_hint().upper().is_none(),
            "a streamed body has no known length"
        );
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let text = String::from_utf8(body.to_vec()).expect("utf8");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "{text}");
        for line in lines {
            serde_json::from_str::<serde_json::Value>(line).expect("each line is JSON");
        }
        assert!(text.ends_with('\n'), "every record is its own line");

        // The `chunks` envelope streams element by element too.
        let response =
            stream_response(serde_json::json!({ "chunks": ["a", "b", "c"] })).expect("builds");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        assert_eq!(body.iter().filter(|byte| **byte == b'\n').count(), 3);

        // Anything else is a single record (availability-style objects).
        let response = stream_response(serde_json::json!({ "date": "2026-01-05" })).expect("builds");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        let text = String::from_utf8(body.to_vec()).expect("utf8");
        assert_eq!(text.lines().count(), 1, "{text}");
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(text.trim_end()).ok(),
            Some(serde_json::json!({ "date": "2026-01-05" }))
        );
    }

    // ------------------------------------------- Wave 2 HTTP caching (unit)

    #[test]
    fn cache_control_renders_a_canonical_directive_list() {
        let spec = |policy, max_age, swr| CacheSpec {
            policy,
            max_age_secs: max_age,
            stale_while_revalidate_secs: swr,
        };
        assert_eq!(
            cache_control_value(&spec(CachePolicy::Public, Some(60), Some(30))),
            "public, max-age=60, stale-while-revalidate=30"
        );
        assert_eq!(
            cache_control_value(&spec(CachePolicy::Private, Some(120), None)),
            "private, max-age=120"
        );
        assert_eq!(cache_control_value(&spec(CachePolicy::NoStore, None, None)), "no-store");
        assert_eq!(cache_control_value(&spec(CachePolicy::NoCache, None, None)), "no-cache");
    }

    #[tokio::test]
    async fn conditional_get_short_circuits_with_304_for_get_routes() {
        let body = b"{\"ok\":true}";
        let policy = Some(CacheSpec {
            policy: CachePolicy::Public,
            max_age_secs: Some(60),
            stale_while_revalidate_secs: None,
        });
        let route = dispatch(ResponseKind::Json, true, policy);

        // First visit: 200 with a strong ETag and the declared policy.
        let response = apply_caching(
            &route,
            &HeaderMap::new(),
            (StatusCode::OK, "payload").into_response(),
            Some(body),
        );
        assert_eq!(response.status(), StatusCode::OK);
        let etag = header_of(&response, header::ETAG).expect("ETag advertised");
        assert!(etag.starts_with('\"') && etag.ends_with('\"'), "{etag}");
        assert_eq!(etag, kelson_http::etag_for(body), "ETag covers the served bytes");
        assert_eq!(
            header_of(&response, header::CACHE_CONTROL).as_deref(),
            Some("public, max-age=60")
        );

        // Revisit with that validator: 304, validators only, no body.
        let mut revisit = HeaderMap::new();
        revisit.insert(
            header::IF_NONE_MATCH,
            header::HeaderValue::from_str(&etag).expect("etag is a header value"),
        );
        let response = apply_caching(
            &route,
            &revisit,
            (StatusCode::OK, "payload").into_response(),
            Some(body),
        );
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(
            header_of(&response, header::ETAG).as_deref(),
            Some(etag.as_str()),
            "304 repeats the validator"
        );
        assert_eq!(
            header_of(&response, header::CACHE_CONTROL).as_deref(),
            Some("public, max-age=60"),
            "304 still carries the policy"
        );
        let payload = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads");
        assert!(payload.is_empty(), "304 never repeats the body");

        // A stale validator still gets the full payload.
        let mut stale = HeaderMap::new();
        stale.insert(
            header::IF_NONE_MATCH,
            header::HeaderValue::from_static("\"other\""),
        );
        let response = apply_caching(
            &route,
            &stale,
            (StatusCode::OK, "payload").into_response(),
            Some(body),
        );
        assert_eq!(response.status(), StatusCode::OK);
        assert!(header_of(&response, header::ETAG).is_some());

        // `*` matches any current representation.
        let mut star = HeaderMap::new();
        star.insert(header::IF_NONE_MATCH, header::HeaderValue::from_static("*"));
        let response = apply_caching(
            &route,
            &star,
            (StatusCode::OK, "payload").into_response(),
            Some(body),
        );
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    }

    #[test]
    fn non_get_routes_and_bufferless_kinds_skip_the_validator() {
        let policy = Some(CacheSpec {
            policy: CachePolicy::NoStore,
            max_age_secs: None,
            stale_while_revalidate_secs: None,
        });

        // A POST route keeps its declared policy but never advertises an ETag.
        let post = dispatch(ResponseKind::Json, false, policy);
        let response = apply_caching(
            &post,
            &HeaderMap::new(),
            (StatusCode::OK, "payload").into_response(),
            Some(b"body"),
        );
        assert!(header_of(&response, header::ETAG).is_none(), "POST is not revalidated");
        assert_eq!(
            header_of(&response, header::CACHE_CONTROL).as_deref(),
            Some("no-store")
        );

        // Streamed/stored-object responses carry the policy without being
        // hashed twice for a validator.
        let stream = dispatch(ResponseKind::Stream, true, policy);
        let response = apply_caching(
            &stream,
            &HeaderMap::new(),
            (StatusCode::OK, "payload").into_response(),
            None,
        );
        assert!(header_of(&response, header::ETAG).is_none());
        assert_eq!(
            header_of(&response, header::CACHE_CONTROL).as_deref(),
            Some("no-store")
        );

        // No declared policy: no Cache-Control, but GET bodies still get an
        // ETag so a client can revalidate.
        let plain = dispatch(ResponseKind::Json, true, None);
        let response = apply_caching(
            &plain,
            &HeaderMap::new(),
            (StatusCode::OK, "payload").into_response(),
            Some(b"body"),
        );
        assert!(header_of(&response, header::CACHE_CONTROL).is_none());
        assert!(header_of(&response, header::ETAG).is_some());
    }

    /// GRAPH-ROUTE-METHOD compiles against `kelson_registry::ROUTE_METHODS`;
    /// the edge must accept exactly that set — a verb the boot gate allows
    /// but registration skips (or the reverse) would leave the warn-and-skip
    /// path live and a declared route silently unregistered.
    #[test]
    fn declared_methods_mirror_the_registry_route_gate() {
        for verb in kelson_registry::ROUTE_METHODS {
            assert!(
                declared_methods(verb).is_some(),
                "registry verb {verb} must register at the edge"
            );
        }
        for verb in ["PURGE", "TRACE", "CONNECT", "", "get "] {
            assert!(
                declared_methods(verb).is_none(),
                "edge skips {verb:?}, so the boot gate must refuse it"
            );
        }
    }
}
