//! HTTP edge for kelson-api (11-SPEC SS20 + S7 edge wiring): health/readiness,
//! meta endpoints and the dev demo route. Graph-driven module routes live in
//! `graph_routes.rs`; auth flows in `auth_routes.rs`; uploads in
//! `upload_routes.rs`. Everything returns the public Envelope shape.

use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use kelson_command::CommandBus;
use kelson_foundation::OperationId;
use kelson_http::Envelope;
use serde::Serialize;

use crate::graph_routes::RouteEntry;
use crate::host::{dev_context, SystemHealth};

#[derive(Clone)]
pub struct AppState {
    pub bus: Arc<CommandBus>,
    pub health: Arc<SystemHealth>,
    pub env: kelson_config::Environment,
    /// Feature flags enabled for the boot snapshot (S3 item 32 wiring).
    pub flags: Vec<String>,
    // --- S7 edge wiring -------------------------------------------------
    /// PAT issuance/verification (bearer auth + reset tokens).
    pub tokens: Arc<kelson_tokens::TokenService>,
    /// Session middleware posture when the `app.sessions` flag is on.
    pub sessions: Option<Arc<kelson_http::SessionMiddleware>>,
    pub principals: Arc<dyn kelson_identity::PrincipalStore>,
    pub credentials: Arc<dyn kelson_identity::CredentialStore>,
    /// Authz engine: login installs the access snapshot for a principal.
    pub policy: Arc<kelson_policy::PolicyEngine>,
    /// Capabilities granted to authenticated principals (module manifests).
    pub granted_capabilities: Vec<String>,
    /// Fallback tenant for principals without one (dev/test only).
    pub default_tenant_id: Option<kelson_foundation::TenantId>,
    pub registration_open: bool,
    /// B-23: gate module operations until the principal's email is verified.
    pub require_verified_email: bool,
    /// Wave 4 Phase 7: freshness window of a `POST /auth/confirm` stamp for
    /// `confirmed`-chained routes (`auth.confirm_timeout_secs`, default 900).
    pub confirm_timeout_secs: u64,
    /// B-22: opt-in HS256 JWT bearer interop (`app.jwt` flag). PAT + session
    /// stays the default auth model; this only verifies externally-issued tokens.
    pub jwt: Option<Arc<kelson_http::JwtHs256>>,
    /// Body ceiling matched to the admission pipeline.
    pub body_limit: usize,
    pub storage: Arc<dyn kelson_storage::StoragePort>,
    /// Wave 6 P2: named disk registry built from `[storage]` — the upload
    /// edge resolves `?disk=` through it (default = `storage.default`).
    pub disks: Arc<kelson_storage::DiskRegistry>,
    // --- Wave 6 P5 notifications -------------------------------------------
    /// Database-channel inbox store behind `GET /api/v1/notifications` and
    /// `POST /api/v1/notifications/read` (postgres at full boot,
    /// in-memory on degraded hosts — D-52). The notify hub dispatches
    /// rows through this very instance.
    pub notifications: Arc<dyn kelson_notifications::NotificationStore>,
    // --- Wave 6 P7 broadcasting -------------------------------------------
    /// Resolved broadcast channel authorization entries merged across
    /// module manifests (`routes/channels.toml`): channel name plus every
    /// capability string a caller's snapshot must carry. Consulted by
    /// `POST /api/v1/broadcast/auth`, `GET /api/v1/broadcast/members` and
    /// the subscription authorizer (resolve via
    /// `crate::broadcast_routes::resolve_channels`).
    pub channels: Vec<crate::broadcast_routes::ChannelAuthz>,
    /// Presence roster shared between the members endpoint and the
    /// authorized subscription path (join on connect, leave on
    /// disconnect) — ONE instance per boot.
    pub presence: Arc<kelson_realtime::PresenceHub>,
    // --- Wave 6 P8 queues ------------------------------------------------
    /// Pool-gated queue census behind the `/metrics` queue series (depth /
    /// age / finished throughput), snapshotted per scrape. `None` = no
    /// durable store (no pool, or a builder) — the series stays absent
    /// rather than reporting a lying zero (docs/19, 13-OPERATIONS).
    pub queue: Option<Arc<dyn kelson_jobs::JobStoreAdmin>>,
    // --- Wave 6 P9 batches ------------------------------------------------
    /// Pool-gated durable batch ledger (B-32) behind `GET /api/v1/batches/{id}`
    /// and its `cancel`/`retry` operations — membership written once at
    /// dispatch, per-member state read live from `queue`. `None` = no pool
    /// (or a builder): the endpoints fail closed (D-52) instead of reporting
    /// a batch the ledger never saw.
    pub batches: Option<Arc<dyn kelson_jobs::BatchLedger>>,
    // --- Wave 8 ADR 0009 ops dashboard ------------------------------------
    /// Registered schedule items behind `GET /_dashboard/ops` (ADR 0009).
    /// `None` today — registration is programmatic (docs/13), so the
    /// schedule section reports the honest note instead of a lie.
    pub schedule: Option<Arc<kelson_scheduling::ScheduleRegistry>>,
    /// Execution traces on the byte-identical file plane `kelson inspect`
    /// reads (ADR 0009): production wires `FileTraceStore::standard(root)`;
    /// `None` on a degraded boot (D-52) degrades the section inline. Rows
    /// are rendered from a whitelist — `input` never leaves the environment.
    pub traces: Option<Arc<dyn kelson_execution::TraceStore>>,
    pub route_inventory: Vec<RouteEntry>,
    // --- Wave 1 middleware model ------------------------------------------
    /// Named middleware catalog (middleware.toml or the builtin).
    pub middleware_catalog: kelson_http::MiddlewareCatalog,
    /// route.bindings service: model name -> tenant-scoped resolver.
    pub route_bindings: Arc<kelson_models::RouteBindingRegistry>,
    /// model.probes service (Wave 4 Phase 4): model name -> row probe for
    /// the `unique`/`exists` validation rules (fail-closed when missing).
    pub probes: Arc<kelson_models::ProbeRegistry>,
    /// Shared token-bucket state for throttle groups (one bucket per
    /// group+client across all routes).
    pub throttle_buckets:
        Arc<std::sync::Mutex<std::collections::HashMap<String, kelson_http::middleware::BucketState>>>,
    /// Route-cache plans (storage/framework/routes.json); None = live
    /// compile from the catalog at registration.
    pub route_plans: Option<Vec<kelson_registry::RoutePlan>>,
    // --- S10 observability -------------------------------------------------
    /// Process metrics registry (always recording; exposure is flag-gated).
    pub metrics: Arc<kelson_metrics::MetricsRegistry>,
    /// Boot instant feeding the uptime gauge.
    pub booted_at: std::time::Instant,
    /// Serve `/_dashboard` (dev/test default, or the `app.dashboard` flag).
    pub dashboard_enabled: bool,
    // --- S5 Sentinel -----------------------------------------------------
    /// Telemetry runtime handle (admin incident surface + status); `None`
    /// in harnesses that boot without the runtime.
    pub telemetry: Option<Arc<kelson_observability::ObservabilityRuntime>>,
    // --- Wave 2 views -----------------------------------------------------
    /// Compiled views engine (booted from the `[views]` section). `None` in
    /// harnesses that opt out: HTML routes and web-plane error views then
    /// degrade to JSON envelopes instead of failing the request.
    pub views: Option<Arc<kelson_views::ViewsEngine>>,
    // --- Wave 6 P3 i18n ----------------------------------------------------
    /// Boot-loaded message catalog (`lang/<locale>.toml`, `[i18n] paths`).
    /// Feeds the view `_locale`/`_lang` injections, localized
    /// `validation.<rule>` wording and notification string resolution;
    /// an empty catalog degrades every key to itself.
    pub messages: Arc<kelson_foundation::MessageCatalog>,
    // --- Wave 2 error rendering -------------------------------------------
    /// Typed error → status/view/reportable registry plus per-code custom
    /// renderers. Shared with the boot `ServiceRegistry`, so modules extend
    /// this very Arc from `Provider::init` (`ErrorRenderProvider`).
    pub error_renders: Arc<kelson_http::ErrorRenderRegistry>,
    /// `app.debug`: dev-only redacted detail on error bodies and views.
    pub debug: bool,
    /// `[http] problem_json`: RFC-7807 bodies for the api plane by default.
    pub problem_json: bool,
    // --- Wave 2 request/response layer ------------------------------------
    /// Queued-cookie value encryption (opt-in via `[http] encrypt_cookies`).
    /// `None` ships cookie values in the clear; `Some` seals every cookie
    /// queued through the cookie queue under a derived key.
    pub cookie_cipher: Option<Arc<kelson_http::CookieCipher>>,
    /// Post-response (terminating) callbacks, shared with the boot
    /// `ServiceRegistry` so module providers register into this very Arc
    /// from `Provider::init` (`TerminatingProvider`).
    pub terminating: Arc<crate::edge::TerminatingHooks>,
    // --- Wave 2 routing edge -----------------------------------------------
    /// Signed-URL gate behind the `signed` middleware builtin: HMAC + TTL
    /// verification and one-time `once` redemption through the token store.
    /// `None` without an application key — a route declaring `signed` then
    /// refuses every request (fail-closed, 500) instead of passing unsigned.
    pub signed_urls: Option<Arc<kelson_http::SignedGate>>,
    /// Control-plane tenant registry consulted by the host gate when a route
    /// declares `subdomain`: a matched subdomain resolves a record here and
    /// feeds it into tenant resolution (19-PARITY route host matching).
    pub tenants: Option<Arc<kelson_tenancy::TenantService>>,
    // --- Wave 2 static assets ----------------------------------------------
    /// `public/` file resolver behind the router fallback (19-PARITY
    /// "Static assets"): declared routes always win, a file that
    /// canonicalizes inside the root is served, anything else renders the
    /// not-found path. `None` leaves the router without a fallback.
    pub static_files: Option<Arc<kelson_http::StaticFiles>>,
    /// Vite build manifest (`public/build/manifest.json`) powering the
    /// `@asset("...")` URL helper in templates; `None` (no build yet)
    /// degrades to literal root-absolute URLs.
    pub assets: Option<Arc<kelson_http::AssetManifest>>,
    // --- Wave 7 P4 security headers ----------------------------------------
    /// Resolved `[security.headers]` posture: layered outermost on
    /// web-plane graph routes only (19-PARITY "Security-headers
    /// middleware"); the api plane never reads it. `enabled = false` (the
    /// default) attaches nothing at all.
    pub security_headers: kelson_http::SecurityHeaders,
    // --- Wave 7 P5 argon2 rehash-on-login -----------------------------------
    /// Resolved `[identity]` argon2 factors: register/reset hash with these,
    /// and a successful login rehashes a stored verifier whose embedded
    /// factors drift from them (19-PARITY "Argon2 rehash-on-login").
    pub password_params: kelson_identity::PasswordParams,
    /// The timing-equalization dummy for unknown principals / missing
    /// credentials, built once per boot on first use at the SAME factors so
    /// docs/06's equalization claim holds after a factor change too.
    pub dummy_hash: Arc<std::sync::OnceLock<String>>,
    // --- Wave 7 P6 password policy ----------------------------------------
    /// Resolved `[auth.password_policy]`: register/password-reset enforce it
    /// directly and `graph_routes` feeds it to every dispatch context, so
    /// the Wave 4 `password` rule composable reads the same configured
    /// posture (19-PARITY "`[auth.password_policy]` config feeding the
    /// `password` rule composable").
    pub password_policy: kelson_validation::PasswordPolicy,
    // --- Wave 7 P7 CSRF ----------------------------------------------------
    /// Resolved `[security.csrf]` posture: the double-submit guard is
    /// attached to web-plane graph routes ALWAYS (default-on — no flag or
    /// env; the section carries only skip paths), while the api plane
    /// attaches nothing and stays token-authenticated (19-PARITY "CSRF
    /// on-by-default for the web plane").
    pub csrf: kelson_http::CsrfGuard,
}

impl AppState {
    /// Edge-layer configuration (queued cookies + terminating hooks) for
    /// `crate::edge::edge_mw`, applied last in `run()` so it wraps every
    /// route including the merged module/auth surfaces.
    pub fn edge_config(&self) -> crate::edge::EdgeConfig {
        crate::edge::EdgeConfig::new(Arc::clone(&self.terminating))
            .with_cipher(self.cookie_cipher.clone())
    }

    /// Wave 2 error rendering (the choke point every JSON edge goes through):
    /// the registry decides status, render path and reportability; reportable
    /// errors flow to observability here; `app.debug` adds redacted internal
    /// detail; per-code custom renderer output lands in `error.meta`.
    /// Report-only codes (`renderable = false`) keep the mapped status but
    /// serve the generic internal body.
    pub fn error_envelope(
        &self,
        err: &kelson_foundation::AppError,
    ) -> (axum::http::StatusCode, Envelope<serde_json::Value>) {
        let plan = self.error_renders.plan(err);
        if plan.reportable {
            tracing::warn!(
                target: "kelson::error_render",
                code = %err.code,
                status = plan.status.as_u16(),
                category = ?err.category,
                message = %err.public_message,
                "reportable error surfaced to observability"
            );
        }
        let envelope = if plan.renderable {
            let mut envelope = Envelope::<serde_json::Value>::error(err);
            let mut error = envelope
                .error
                .take()
                .unwrap_or_else(|| kelson_http::ErrorBody::from_app_error(err));
            if let Some(detail) = self.error_renders.detail(err) {
                error = error.with_detail(detail);
            }
            if self.debug {
                if let Some(detail) = debug_detail(err) {
                    error = error.with_debug(detail);
                }
            }
            envelope.error = Some(error);
            envelope
        } else {
            Envelope::<serde_json::Value>::error(&kelson_foundation::AppError::internal(
                err.internal.clone().unwrap_or_default(),
            ))
        };
        (plan.status, envelope)
    }

    /// `(status, envelope body)` tuple for the JSON edges.
    pub fn error_json(
        &self,
        err: &kelson_foundation::AppError,
    ) -> (axum::http::StatusCode, Json<serde_json::Value>) {
        let (status, envelope) = self.error_envelope(err);
        (
            status,
            Json(serde_json::to_value(envelope).unwrap_or_default()),
        )
    }

    /// Wave 2 transport decision for an api-plane (or JSON-negotiating)
    /// error: RFC-7807 `application/problem+json` when the request's `Accept`
    /// header or the deployment default `[http] problem_json` asks for it,
    /// the Envelope otherwise. `instance` becomes the problem's `instance`.
    pub fn error_response(
        &self,
        err: &kelson_foundation::AppError,
        headers: &axum::http::HeaderMap,
        instance: Option<&str>,
    ) -> axum::response::Response {
        let (status, envelope) = self.error_envelope(err);
        let accept = headers
            .get(axum::http::header::ACCEPT)
            .and_then(|v| v.to_str().ok());
        if kelson_http::wants_problem(accept, self.problem_json) {
            let error = envelope
                .error
                .clone()
                .unwrap_or_else(|| kelson_http::ErrorBody::from_app_error(err));
            return self.problem_response(status, &error, instance);
        }
        (
            status,
            Json(serde_json::to_value(envelope).unwrap_or_default()),
        )
            .into_response()
    }

    /// One RFC-7807 problem response: the status comes from the registry and
    /// `Content-Type` is replaced explicitly so the body never ships as
    /// `application/json`.
    pub fn problem_response(
        &self,
        status: axum::http::StatusCode,
        body: &kelson_http::ErrorBody,
        instance: Option<&str>,
    ) -> axum::response::Response {
        let mut response = Json(kelson_http::problem_json(status, body, instance)).into_response();
        *response.status_mut() = status;
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static(kelson_http::PROBLEM_JSON),
        );
        response
    }
}

/// `app.debug` detail for one error: the internal context run through the
/// logging redaction list (kernel default sensitive fields) plus the
/// correlation id. Callers gate this on `app.debug`; secrets never render.
pub fn debug_detail(err: &kelson_foundation::AppError) -> Option<serde_json::Value> {
    let mut detail = serde_json::Map::new();
    if let Some(internal) = &err.internal {
        let fields: Vec<String> = kelson_observability::DEFAULT_REDACTIONS
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        detail.insert(
            "internal".into(),
            serde_json::Value::String(kelson_observability::redact_str(internal, &fields)),
        );
    }
    if let Some(correlation) = err.correlation {
        detail.insert(
            "correlation".into(),
            serde_json::json!(correlation.to_string()),
        );
    }
    if detail.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(detail))
    }
}

#[derive(Serialize)]
pub struct HealthBody {
    pub ok: bool,
    pub degraded: bool,
    pub reason: String,
}

#[derive(Serialize)]
pub struct FlagsBody {
    pub enabled: Vec<String>,
}

#[derive(Serialize)]
pub struct BootBody {
    pub name: &'static str,
    pub version: &'static str,
    pub services: Vec<String>,
}

#[derive(Serialize)]
pub struct RoutesBody {
    pub routes: Vec<RouteEntry>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/readyz", get(readyz))
        .route("/meta/boot", get(boot_meta))
        .route("/meta/flags", get(flags_meta))
        .route("/meta/routes", get(routes_meta))
        .route("/metrics", get(metrics_text))
        .route("/_dashboard", get(dashboard))
        // Wave 8 ADR 0009: the operational half — same `dashboard_enabled`
        // gate, shared identity resolution, views-or-JSON rendering.
        .route("/_dashboard/ops", get(crate::dashboard::ops_dashboard))
        .route("/api/v1/demo/echo", post(demo_echo))
        .with_state(state)
}

// ----------------------------------------------------------- S10 endpoints

/// Prometheus text exposition (S10 B-17). Recording runs always; the route
/// only answers when the `app.metrics` flag is enabled, so the endpoint is
/// provable in tests without flipping global state.
async fn metrics_text(State(s): State<AppState>) -> axum::response::Response {
    if !s.flags.iter().any(|f| f == "app.metrics") {
        let err =
            kelson_foundation::AppError::not_found("metrics exposition disabled (enable flag app.metrics)");
        return s.error_json(&err).into_response();
    }
    s.metrics.set_gauge(
        "kelson_uptime_seconds",
        std::collections::BTreeMap::new(),
        s.booted_at.elapsed().as_secs_f64(),
    );
    s.metrics.set_gauge(
        "kelson_degraded",
        std::collections::BTreeMap::new(),
        if s.health.is_degraded() { 1.0 } else { 0.0 },
    );
    s.metrics.set_gauge(
        "kelson_routes_declared",
        std::collections::BTreeMap::new(),
        s.route_inventory.len() as f64,
    );
    // Wave 6 P8: the queue census (docs/19: queue depth/age/throughput
    // counters on /metrics) — snapshotted at scrape from the pool-gated
    // store. No durable store = the series stays absent rather than
    // reporting a lying zero.
    if let Some(queue) = &s.queue {
        if let Ok(snap) = queue.snapshot() {
            for (status, count) in [
                ("pending", snap.pending),
                ("running", snap.running),
                ("dead", snap.dead),
                ("succeeded", snap.succeeded),
            ] {
                let mut labels = std::collections::BTreeMap::new();
                labels.insert("status".to_string(), status.to_string());
                s.metrics
                    .set_gauge("kelson_queue_depth", labels, count as f64);
            }
            let oldest = snap
                .oldest_pending
                .map(|at| {
                    (time::OffsetDateTime::now_utc() - at)
                        .whole_seconds()
                        .max(0) as f64
                })
                .unwrap_or(0.0);
            s.metrics.set_gauge(
                "kelson_queue_oldest_age_seconds",
                std::collections::BTreeMap::new(),
                oldest,
            );
            // Throughput: the cumulative finished census — worker-side
            // finishes land in the SAME store, so both planes surface here.
            // A prune/clear can lower it; it is a store census, not an
            // event counter (13-OPERATIONS).
            for (outcome, count) in [("succeeded", snap.succeeded), ("dead", snap.dead)] {
                let mut labels = std::collections::BTreeMap::new();
                labels.insert("outcome".to_string(), outcome.to_string());
                s.metrics
                    .set_gauge("kelson_queue_finished", labels, count as f64);
            }
        }
    }
    let body = kelson_metrics::render_prometheus(&s.metrics.snapshot());
    (
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        body,
    )
        .into_response()
}

/// Horizon-lite dashboard (S10 B-20): static page over the JSON meta/metrics
/// surface. Gated by `dashboard_enabled` (dev/test default; prod needs the
/// `app.dashboard` flag).
async fn dashboard(State(s): State<AppState>) -> axum::response::Response {
    if !s.dashboard_enabled {
        let err = kelson_foundation::AppError::not_found("dashboard disabled (enable flag app.dashboard)");
        return s.error_json(&err).into_response();
    }
    (
        [("content-type", "text/html; charset=utf-8")],
        crate::dashboard::DASHBOARD_HTML,
    )
        .into_response()
}

/// Metrics middleware (S10): counts every request and observes latency.
/// Attached OUTSIDE admission in lib.rs so rate-limit/maintenance/CSRF
/// rejections are counted too.
pub async fn record_metrics(
    State(s): State<AppState>,
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let method = req.method().to_string();
    let start = std::time::Instant::now();
    let response = next.run(req).await;
    let status_class = format!("{}xx", response.status().as_u16() / 100);
    let mut labels = std::collections::BTreeMap::new();
    labels.insert("method".to_string(), method);
    labels.insert("status_class".to_string(), status_class);
    s.metrics.inc("kelson_http_requests_total", labels, 1.0);
    // Sentinel registry histograms are name-keyed (ms unit) — per-method
    // breakdown stays on the labelled request counter.
    s.metrics.observe_ms(
        "kelson_http_request_duration_ms",
        start.elapsed().as_secs_f64() * 1000.0,
    );
    response
}

fn health_body(h: &SystemHealth) -> HealthBody {
    HealthBody {
        ok: !h.is_degraded(),
        degraded: h.is_degraded(),
        reason: h.reason.lock().unwrap().clone(),
    }
}

async fn healthz(State(s): State<AppState>) -> Json<Envelope<HealthBody>> {
    Json(Envelope::ok(HealthBody {
        ok: true,
        ..health_body(&s.health)
    }))
}

async fn readyz(State(s): State<AppState>) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    if s.health.is_degraded() {
        let err = kelson_foundation::AppError::new(
            kelson_foundation::codes::BOOT,
            kelson_foundation::ErrorCategory::Internal,
            kelson_foundation::Severity::Medium,
            kelson_foundation::Retryability::Retryable,
            format!("degraded: {}", s.health.reason.lock().unwrap().clone()),
        );
        (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::to_value(Envelope::<serde_json::Value>::error(&err)).unwrap()),
        )
    } else {
        (
            axum::http::StatusCode::OK,
            Json(serde_json::to_value(Envelope::ok(health_body(&s.health))).unwrap()),
        )
    }
}

async fn boot_meta(State(s): State<AppState>) -> Json<Envelope<BootBody>> {
    Json(Envelope::ok(BootBody {
        name: "kelson-api",
        version: env!("CARGO_PKG_VERSION"),
        services: s.health.services.lock().unwrap().clone(),
    }))
}

async fn flags_meta(State(s): State<AppState>) -> Json<Envelope<FlagsBody>> {
    Json(Envelope::ok(FlagsBody {
        enabled: s.flags.clone(),
    }))
}

async fn routes_meta(State(s): State<AppState>) -> Json<Envelope<RoutesBody>> {
    Json(Envelope::ok(RoutesBody {
        routes: s.route_inventory.clone(),
    }))
}

/// Dev/test-only golden path: dispatch demo.echo through the full engine pipeline.
async fn demo_echo(
    State(s): State<AppState>,
    Json(input): Json<serde_json::Value>,
) -> (axum::http::StatusCode, Json<serde_json::Value>) {
    if s.env.is_prod() || matches!(s.env, kelson_config::Environment::Staging) {
        let err = kelson_foundation::AppError::denied("demo routes are disabled outside dev/test");
        return s.error_json(&err);
    }
    let op = OperationId::from_static("demo.echo");
    match s
        .bus
        .dispatch(dev_context().expect("dev context builds"), &op, input, None)
        .await
    {
        Ok(outcome) => (
            axum::http::StatusCode::OK,
            Json(serde_json::to_value(Envelope::ok(outcome.result)).unwrap()),
        ),
        Err(err) => s.error_json(&err),
    }
}
