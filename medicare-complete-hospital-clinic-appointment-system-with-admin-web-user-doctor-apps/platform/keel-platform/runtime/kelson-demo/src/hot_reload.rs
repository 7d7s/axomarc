//! Phase 3 hot reload (target-arch §4.1–§4.3): the pieces of the API edge
//! that swap WITHOUT a restart, wired as `kelson_config::ConfigReloader`
//! subscribers of the `ConfigRuntime` `run()` boots.
//!
//! * [`build_graph_bundle`] assembles the GRAPH segment (module routes +
//!   the `public/` static fallback) — exactly what `run()` used to inline.
//!   It now rides behind [`SwappableGraphRoutes`] (the router's fallback):
//!   a reload rebuilds the bundle and swaps the `Arc`, so in-flight
//!   requests finish on the bundle they started with and new requests get
//!   the new one (§4.2: "in-flight requests complete on old manifest; new
//!   requests use new manifest").
//! * [`RouteReloader`] re-checks the route-cache artifact with the SAME
//!   fail-closed gate boot uses, writes fresh plans into
//!   `state.route_plans`, and swaps the bundle. A stale/corrupt artifact
//!   keeps the current bundle and warns (a running edge must keep serving;
//!   the fix is `kelson route:cache`, whose rewrite re-arms the artifact
//!   mtime probe — the artifact is its own trigger, no config edit needed).
//! * [`FlagsReloader`] re-resolves the ambient flag set (§4.3) behind
//!   `state.flags`. Boot-STRUCTURAL flags (sessions / realtime / jwt /
//!   dashboard — wired into the server at boot) stay boot-frozen on
//!   purpose: they are wiring, not policy.
//!
//! Trigger wiring (`run()`): a poller task calls `ConfigRuntime::poll()`
//! (file mtime) and the artifact probe every 500 ms; both triggers funnel
//! through the same refresh path, so each change swaps exactly once.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

use axum::Router;
use kelson_config::{ConfigReloader, ConfigSnapshot};

use crate::graph_routes;
use crate::routes::AppState;
use crate::static_assets;

/// The GRAPH segment: module routes + the `public/` fallback, assembled in
/// the production order (graph routes first, static files as ITS fallback).
pub fn build_graph_bundle(state: &AppState, graph: &kelson_registry::PlatformGraph) -> Router {
    static_assets::with_static_files(graph_routes::graph_router(state.clone(), graph), state)
}

/// Swappable cell: writer = the reload path, readers = the serving path.
/// `Arc<RwLock<Arc<Router>>>` — the outer lock serializes writes, the
/// inner `Arc` is what requests clone for the duration of their call.
pub type BundleCell = Arc<RwLock<Arc<Router>>>;

/// Wrap a freshly built bundle into a cell (boot path).
pub fn new_bundle(bundle: Router) -> BundleCell {
    Arc::new(RwLock::new(Arc::new(bundle)))
}

/// Router fallback dispatching into the CURRENT bundle. Each call clones
/// the `Arc` BEFORE awaiting, so a swap mid-request cannot pull the rug:
/// the in-flight request finishes on the bundle it loaded (§4.2).
#[derive(Clone)]
pub struct SwappableGraphRoutes {
    pub cell: BundleCell,
}

impl tower::Service<axum::extract::Request> for SwappableGraphRoutes {
    type Response = axum::response::Response;
    type Error = std::convert::Infallible;
    type Future = std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Self::Response, Self::Error>> + Send>,
    >;

    fn poll_ready(
        &mut self,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: axum::extract::Request) -> Self::Future {
        // Snapshot the CURRENT bundle (axum `Router::clone` is cheap — it
        // is what `serve` itself does per connection) and dispatch against
        // it: a swap after this point cannot affect the in-flight call.
        let bundle = self.cell.read().unwrap().as_ref().clone();
        Box::pin(async move { tower::ServiceExt::oneshot(bundle, req).await })
    }
}

/// Route-cache subscriber (§4.2). See the module docs for the swap and
/// fail-closed semantics.
#[derive(Clone)]
pub struct RouteReloader {
    cell: BundleCell,
    state: AppState,
    graph: Arc<kelson_registry::PlatformGraph>,
    root: PathBuf,
    /// Artifact mtime stamp, advanced on EVERY refresh (success OR
    /// refusal): a stale artifact must not hot-loop, and the operator's
    /// `route:cache` rewrite moves mtime, which re-arms the probe.
    artifact_stamp: Arc<Mutex<Option<SystemTime>>>,
}

impl RouteReloader {
    pub fn new(
        cell: BundleCell,
        state: AppState,
        graph: Arc<kelson_registry::PlatformGraph>,
        root: PathBuf,
    ) -> Self {
        let reloader = Self {
            cell,
            state,
            graph,
            root,
            artifact_stamp: Arc::new(Mutex::new(None)),
        };
        reloader.mark_artifact(); // boot baseline: this artifact is current
        reloader
    }

    /// Baseline the artifact mtime probe.
    pub fn mark_artifact(&self) {
        *self.artifact_stamp.lock().unwrap() = artifact_mtime(&self.root);
    }

    /// True when `storage/framework/routes.json` changed since the last
    /// probe (the `kelson route:cache` trigger — independent of config).
    pub fn artifact_changed(&self) -> bool {
        let mtime = artifact_mtime(&self.root);
        let mut stamp = self.artifact_stamp.lock().unwrap();
        let changed = mtime != *stamp;
        *stamp = mtime;
        changed
    }

    /// Re-check the artifact and swap the bundle. `Ok(None)` (artifact
    /// cleared) rebuilds on the live compile path — `route:clear` semantics.
    /// A stale/corrupt artifact keeps the current bundle and warns.
    pub fn refresh(&self) {
        self.mark_artifact();
        match graph_routes::route_cache_check(&self.root) {
            Ok(plans) => {
                *self.state.route_plans.write().unwrap() = plans.map(Arc::new);
                let bundle = Arc::new(build_graph_bundle(&self.state, &self.graph));
                *self.cell.write().unwrap() = bundle;
                tracing::info!(root = %self.root.display(), "route bundle hot-swapped");
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    "route reload refused: keeping the current bundle (run kelson route:cache)"
                );
            }
        }
    }
}

impl ConfigReloader for RouteReloader {
    fn on_reload(&self, snapshot: &ConfigSnapshot) {
        tracing::debug!(
            version = snapshot.version,
            hash = %snapshot.hash,
            "config revision accepted: refreshing route bundle"
        );
        self.refresh();
    }
}

fn artifact_mtime(root: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(kelson_registry::route_cache_path(root))
        .and_then(|m| m.modified())
        .ok()
}

/// Ambient flag-set subscriber (§4.3): re-resolves the boot input against
/// the store (FileFlagStore reads fresh per call, so stage edits land too)
/// and swaps `state.flags`. Handlers observe the new set at their next
/// read; a reader that already cloned the old `Vec` keeps it (§4.2
/// semantics apply to this swap too).
#[derive(Clone)]
pub struct FlagsReloader {
    service: Arc<kelson_feature_flags::FlagService>,
    environment: String,
    capabilities: BTreeSet<String>,
    actor: kelson_foundation::PrincipalId,
    state: AppState,
}

impl FlagsReloader {
    pub fn new(
        service: Arc<kelson_feature_flags::FlagService>,
        environment: String,
        actor: kelson_foundation::PrincipalId,
        state: AppState,
    ) -> Self {
        Self {
            service,
            environment,
            capabilities: BTreeSet::new(),
            actor,
            state,
        }
    }

    /// Re-resolve and swap; returns the fresh set (logging + tests).
    pub fn refresh(&self) -> Vec<String> {
        let input = kelson_feature_flags::FlagInput {
            environment: &self.environment,
            tenant: None,
            region: None,
            capabilities: &self.capabilities,
            actor: self.actor,
            internal_actor: true,
        };
        match self.service.snapshot(&input) {
            Ok(snapshot) => {
                let enabled: Vec<String> = snapshot.flags.iter().cloned().collect();
                *self.state.flags.write().unwrap() = enabled.clone();
                enabled
            }
            Err(error) => {
                tracing::warn!(%error, "flag refresh refused: keeping the current set");
                self.state.flags.read().unwrap().clone()
            }
        }
    }
}

impl ConfigReloader for FlagsReloader {
    fn on_reload(&self, snapshot: &ConfigSnapshot) {
        let enabled = self.refresh();
        tracing::info!(
            version = snapshot.version,
            flags = ?enabled,
            "feature flags re-resolved"
        );
    }
}

// ----------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use tower::ServiceExt;

    fn fixture() -> (AppState, Arc<kelson_registry::PlatformGraph>, PathBuf) {
        let dir = std::env::temp_dir().join(format!("kelson-demo-hot-{}", uuid::Uuid::now_v7()));
        let (state, graph, _store) = crate::build_app_state(&dir, true, true);
        (state, Arc::new(graph), dir)
    }

    fn boot_actor() -> kelson_foundation::PrincipalId {
        kelson_foundation::PrincipalId::from_uuid(
            uuid::Uuid::parse_str(crate::host::BOOT_ACTOR_UUID).expect("static uuid"),
        )
    }

    fn flag_service(root: &std::path::Path) -> Arc<kelson_feature_flags::FlagService> {
        Arc::new(kelson_feature_flags::FlagService::new(
            Arc::new(kelson_feature_flags::platform_registry()),
            Arc::new(kelson_feature_flags::FileFlagStore::new(
                root.join("flags.json"),
            )),
        ))
    }

    /// One request/response envelope against an explicit bundle snapshot
    /// (the `Router` clone mirrors exactly what an in-flight request
    /// holds — same shape `SwappableGraphRoutes::call` takes).
    ///
    /// Demo-only: every caller drives a route from the appointments manifest.
    #[cfg(feature = "demo")]
    async fn envelope_of(bundle: Router, method: Method, path: &str) -> (StatusCode, String) {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .expect("request builds");
        let res = bundle.oneshot(req).await.expect("infallible dispatch");
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 64 * 1024)
            .await
            .expect("body reads");
        let code = serde_json::from_slice::<serde_json::Value>(&bytes)
            .ok()
            .and_then(|v| v["error"]["code"].as_str().map(str::to_string))
            .unwrap_or_default();
        (status, code)
    }

    #[cfg(feature = "demo")]
    async fn envelope_via(cell: &BundleCell, method: Method, path: &str) -> (StatusCode, String) {
        let bundle = cell.read().unwrap().as_ref().clone();
        envelope_of(bundle, method, path).await
    }

    /// What the dispatch-level auth produces for anon callers (the graph's
    /// non-public routes enforce in dispatch, independent of the chain).
    #[cfg(feature = "demo")]
    fn auth_envelope() -> (StatusCode, String) {
        (StatusCode::UNAUTHORIZED, "AUTH_UNAUTHENTICATED".to_string())
    }

    /// Artifact helper: plans resolved from the harness graph, optionally
    /// mutated, pinned the way `kelson route:cache` pins them.
    fn write_artifact(
        root: &std::path::Path,
        graph: &kelson_registry::PlatformGraph,
        mutate: impl FnOnce(&mut Vec<kelson_registry::RoutePlan>),
    ) {
        let mut plans =
            kelson_registry::resolve_route_plans(graph, &kelson_http::MiddlewareCatalog::builtin())
                .expect("plans resolve");
        mutate(&mut plans);
        kelson_registry::RouteCacheArtifact {
            module_abi: kelson_registry::MODULE_ABI,
            manifest_hash: kelson_registry::manifests_hash(root).expect("hash computes"),
            plans,
        }
        .write(&kelson_registry::route_cache_path(root))
        .expect("artifact writes");
    }

    /// §9/§10: route cache hot swap — a fresh artifact lands via
    /// `kelson route:cache`, the subscriber swaps the bundle in <50ms with
    /// NO restart, the new request reads the new chain, and the previous
    /// bundle (what an in-flight request holds) is untouched.
    // The fixture route (`/api/v1/appointments/availability`) and its
    // anon-tolerant handler come from the demo appointments module, so the
    // sanity baseline only exists with `--features demo`.
    #[cfg(feature = "demo")]
    #[tokio::test]
    async fn route_bundle_hot_swaps_under_50ms_in_flight_keeps_old_bundle() {
        let (state, graph, dir) = fixture();
        let cell = new_bundle(build_graph_bundle(&state, &graph));

        // Baseline: /api/v1/appointments/availability is declared
        // `public = true` ("strips the implicit auth/verified stages") with
        // a registered anon-tolerant handler — anon passes the edge.
        let before = envelope_via(&cell, Method::GET, "/api/v1/appointments/availability").await;
        assert_ne!(
            before,
            auth_envelope(),
            "sanity: the public route is reachable anonymously"
        );
        // Exactly what an in-flight request holds across the swap.
        let inflight = cell.read().unwrap().as_ref().clone();

        // A fresh artifact whose plan for this route GAINS the auth
        // middleware — a stack change only the artifact can express (the
        // manifest still declares the route public; the raw cached chain
        // is what the middleware stage runs).
        write_artifact(&dir, &graph, |plans| {
            let target = plans
                .iter_mut()
                .find(|p| p.path == "/api/v1/appointments/availability")
                .expect("availability plan present");
            target.chain = vec![kelson_registry::BuiltinMw::Auth];
        });

        let reloader = RouteReloader::new(cell.clone(), state.clone(), graph.clone(), dir.clone());
        let started = std::time::Instant::now();
        reloader.refresh();
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "route bundle swap must stay under 50ms, took {elapsed:?}"
        );

        // The state cell now carries the ARTIFACT's plans (the only input
        // registration reads — graph_routes' `cached_plan` path), so the
        // rebuilt bundle is provably built from them.
        {
            let loaded = state.route_plans.read().unwrap();
            let loaded = loaded.as_ref().expect("plans swapped onto state");
            let target = loaded
                .iter()
                .find(|p| p.path == "/api/v1/appointments/availability")
                .expect("artifact plan present");
            assert!(
                matches!(target.chain.as_slice(), [kelson_registry::BuiltinMw::Auth]),
                "the artifact's chain is what registration saw"
            );
        }

        // New requests: the swapped bundle applies the artifact's stack —
        // the auth middleware now gates the formerly-public route.
        let after = envelope_via(&cell, Method::GET, "/api/v1/appointments/availability").await;
        assert_eq!(after, auth_envelope(), "new requests observe the new plan");
        assert_ne!(after, before, "response changed with the plan");

        // In-flight: still the old bundle, still the old behavior.
        let inflight_envelope =
            envelope_of(inflight, Method::GET, "/api/v1/appointments/availability").await;
        assert_eq!(
            inflight_envelope, before,
            "in-flight bundle keeps serving the stack it loaded"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A stale artifact is refused exactly like boot refuses it — but a
    /// running edge keeps its current bundle instead of going down.
    #[tokio::test]
    async fn stale_artifact_keeps_current_bundle_and_warns() {
        let (state, graph, dir) = fixture();
        let cell = new_bundle(build_graph_bundle(&state, &graph));
        let inflight = cell.read().unwrap().as_ref().clone();

        write_artifact(&dir, &graph, |plans| {
            plans.iter_mut().for_each(|p| p.chain.clear());
        });
        // Drift: a manifest change invalidates the pinned hash.
        let app_dir = dir.join("applications").join("probe");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(app_dir.join("module.toml"), "[module]\nname = \"probe\"\n").unwrap();

        let reloader = RouteReloader::new(cell.clone(), state.clone(), graph.clone(), dir.clone());
        let cell_before = Arc::as_ptr(&*cell.read().unwrap());
        reloader.refresh();
        assert_eq!(
            Arc::as_ptr(&*cell.read().unwrap()),
            cell_before,
            "stale artifact never swaps the bundle (fail-closed)"
        );
        // The in-flight bundle still answers.
        let status = {
            let req = Request::builder()
                .method(Method::GET)
                .uri("/api/v1/appointments")
                .body(Body::empty())
                .expect("request builds");
            inflight.oneshot(req).await.expect("dispatch").status()
        };
        assert_eq!(status, StatusCode::UNAUTHORIZED, "old bundle still serves");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The §10 DoD chain: `config:reload` (file edit + poll/reload) swaps
    /// BOTH the route bundle and the ambient flag set in <50ms, no
    /// restart, subscribers notified once.
    #[tokio::test]
    async fn config_reload_swaps_route_bundle_and_flags_under_50ms() {
        let (state, graph, dir) = fixture();
        let cfg_path = dir.join("config.toml");
        std::fs::write(&cfg_path, "[app]\nname = \"hot-reload\" \nenv = \"dev\"\n\n[limits]\ndefault_monthly_operations = 100000\n")
            .unwrap();
        let source = kelson_config::FileConfigSource::new(&cfg_path);
        let config = kelson_config::PlatformConfig::load(Some(&cfg_path)).expect("probe config loads");
        let runtime = kelson_config::ConfigRuntime::boot(source, config).expect("boot hashes");

        let cell = new_bundle(build_graph_bundle(&state, &graph));
        runtime.subscribe(RouteReloader::new(
            cell.clone(),
            state.clone(),
            graph.clone(),
            dir.clone(),
        ));
        runtime.subscribe(FlagsReloader::new(
            flag_service(&dir),
            "test".to_string(),
            boot_actor(),
            state.clone(),
        ));

        // Baseline: app.metrics resolves DISABLED (opt-in platform flag).
        let baseline_flags = state.flags.read().unwrap().clone();
        assert!(
            !baseline_flags.iter().any(|f| f == "app.metrics"),
            "baseline: app.metrics off ({baseline_flags:?})"
        );
        let bundle_before = Arc::as_ptr(&*cell.read().unwrap());
        let inflight = cell.read().unwrap().as_ref().clone();

        // Change: stage app.metrics to full AND touch the config file
        // (the §4.1 file-mtime trigger; limits key is env-overlay-proof).
        flag_service(&dir)
            .set_stage("app.metrics", kelson_feature_flags::RolloutStage::Percent100)
            .expect("stage writes");
        std::fs::write(
            &cfg_path,
            "[app]\nname = \"hot-reload\" \nenv = \"dev\"\n\n[limits]\ndefault_monthly_operations = 42\n",
        )
        .unwrap();

        let started = std::time::Instant::now();
        let swapped = runtime.poll().expect("poll observes the mtime change");
        let elapsed = started.elapsed();
        let swapped = swapped.expect("config changed -> swap");
        assert_eq!(swapped.version, 2, "config revision bumped");
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "config reload + both subscriber swaps must stay under 50ms, took {elapsed:?}"
        );

        // Flag set swapped (§4.3).
        assert!(
            state.flags.read().unwrap().iter().any(|f| f == "app.metrics"),
            "flags re-resolved after reload"
        );
        // Route bundle swapped (§4.2) — new Arc behind the cell...
        assert_ne!(
            Arc::as_ptr(&*cell.read().unwrap()),
            bundle_before,
            "route bundle swapped"
        );
        // ...while the in-flight snapshot still serves the old bundle.
        let status = {
            let req = Request::builder()
                .method(Method::GET)
                .uri("/api/v1/appointments")
                .body(Body::empty())
                .expect("request builds");
            inflight.oneshot(req).await.expect("dispatch").status()
        };
        assert_eq!(status, StatusCode::UNAUTHORIZED, "in-flight unaffected");
        // Quiet on the next poll: unchanged config + unchanged artifact.
        assert!(
            runtime.poll().expect("poll quiet").is_none(),
            "no churn when nothing changed"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// `kelson route:cache` lands WITHOUT a config edit: the artifact mtime
    /// probe is an independent trigger.
    #[tokio::test]
    async fn artifact_rewrite_swaps_without_a_config_edit() {
        let (state, graph, dir) = fixture();
        let cell = new_bundle(build_graph_bundle(&state, &graph));
        let reloader = RouteReloader::new(cell.clone(), state.clone(), graph.clone(), dir.clone());
        assert!(!reloader.artifact_changed(), "baseline is quiet");

        write_artifact(&dir, &graph, |_| {});
        assert!(
            reloader.artifact_changed(),
            "route:cache rewrite is its own trigger"
        );
        reloader.refresh();
        assert!(!reloader.artifact_changed(), "stamp current after refresh");
        // Bundle swapped onto the artifact path (plans now pinned).
        assert!(state.route_plans.read().unwrap().is_some(), "plans loaded");
        reloader.refresh(); // idempotent: same artifact, still fine
        assert!(state.route_plans.read().unwrap().is_some());

        std::fs::remove_dir_all(&dir).ok();
    }
}
