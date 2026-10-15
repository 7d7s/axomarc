//! Operator module-lifecycle surface (docs/15-EXTENSION, Row 16 wiring):
//! session-authenticated operators degrade, recover or disable a tracked
//! module over HTTP. Mirrors the telemetry admin surface (`telemetry_routes`)
//! — session + capability gate — and defers to the registry's transition
//! rules: an invalid transition (recover a Ready module, disable twice) is a
//! 409 CONFLICT, an untracked module a 404, an unknown action a 422.
//!
//! Honest limit: this changes the module's *state*, surfaced by `/meta/boot`
//! and consulted by `/readyz`, but dispatch does not yet refuse a Disabled
//! module's routes — the registry keys providers by name (`module.appointments`)
//! while `RouteEntry.module` carries the manifest name (`appointments`), so a
//! dispatch-level gate needs that mapping first.

use axum::extract::State;
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use kelson_foundation::{AppError, PrincipalId, TenantId};
use kelson_http::{Envelope, ResolvedSession};
use serde::{Deserialize, Serialize};

use crate::routes::AppState;

/// Capability required to drive module lifecycle transitions. Not granted to
/// registered principals by default (same posture as `telemetry.incident.manage`):
/// an operator grants it through the access snapshot.
pub const MODULE_MANAGE_CAPABILITY: &str = "platform.modules.manage";

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/admin/modules", post(manage))
        .route("/admin/modules/activate", post(activate))
        .with_state(state)
}

/// Session + operator capability gate (module lifecycle control is never public — same shape as the telemetry incident gate).
fn authorize(state: &AppState, principal: PrincipalId, tenant: Option<TenantId>) -> Result<(), AppError> {
    let Some(tenant) = tenant else {
        return Err(AppError::denied(
            "module control requires a tenant session",
        ));
    };
    let Some(snapshot) = state.policy.snapshot(principal, tenant) else {
        return Err(AppError::denied("no active access snapshot"));
    };
    if !snapshot.capabilities.contains(MODULE_MANAGE_CAPABILITY) {
        return Err(AppError::denied(format!(
            "missing module control capability {MODULE_MANAGE_CAPABILITY}"
        )));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct ManageRequest {
    /// Tracked module name (as keyed by the registry, e.g. `config`).
    name: String,
    /// `degrade`, `recover` or `disable`.
    action: String,
}

#[derive(Debug, Serialize)]
struct ManageBody {
    module: String,
    action: String,
    state: kelson_lifecycle::ModuleState,
}

async fn manage(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<axum::extract::Extension<ResolvedSession>>,
    Json(request): Json<ManageRequest>,
) -> axum::response::Response {
    let Some(session) = session else {
        return state.error_response(&AppError::unauthenticated(), &headers, None);
    };
    if let Err(err) = authorize(&state, session.session.principal, session.session.tenant) {
        return state.error_response(&err, &headers, None);
    }

    let mut modules = state.modules.write().unwrap();
    if modules.state(&request.name).is_none() {
        return state.error_response(
            &AppError::not_found(format!("module {}", request.name)),
            &headers,
            None,
        );
    }
    let transition = match request.action.as_str() {
        "degrade" => modules.degrade(&request.name),
        "recover" => modules.recover(&request.name),
        "disable" => modules.disable(&request.name),
        other => {
            return state.error_response(
                &AppError::validation(format!(
                    "unknown action {other} (want degrade|recover|disable)"
                )),
                &headers,
                None,
            );
        }
    };
    match transition {
        Ok(()) => {
            let current = modules.state(&request.name).expect("tracked above");
            Json(Envelope::ok(ManageBody {
                module: request.name,
                action: request.action,
                state: current,
            }))
            .into_response()
        }
        // Invalid transition (e.g. recover a Ready module, or a terminal
        // Disabled one): the registry's rules refuse it — 409 CONFLICT.
        Err(message) => state.error_response(&AppError::conflict(message), &headers, None),
    }
}

// ---------------------------------------------------------------------------
// Module activation (Row 21, Stage C)
// ---------------------------------------------------------------------------
//
// Everything below sits past the end of the `manage` citation
// (`module_routes.rs:69-118`), so a new item here costs the ledger nothing.
// This is the control-plane edge `ARCHITECTURE-CONTRACT.md` describes: staging
// is side-effect-free, the compare-and-swap is on the generation the operator
// staged against, and the commit is a single publication of one complete
// aggregate. It is a separate route from `POST /admin/modules` because that
// one drives *lifecycle state* through the registry's transition rules, and
// this one publishes *code* — the two errors differ, and so must the surface.

#[derive(Debug, Deserialize)]
struct ActivateRequest {
    /// The module as `packages.toml` indexes it (`[[wasm]].name`) — the key
    /// boot discovers under, not the manifest name a route entry carries.
    name: String,
    /// The generation this request staged against. It is published only while
    /// it is still the generation serving.
    expected_generation: u64,
}

#[derive(Debug, Serialize)]
struct ActivateBody {
    module: String,
    /// Exactly one past `expected_generation`, or the request was refused and
    /// no number changed at all.
    generation: u64,
    /// The content digest now live — moved by every accepted activation.
    active_content_digest: String,
}

async fn activate(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    session: Option<axum::extract::Extension<ResolvedSession>>,
    Json(request): Json<ActivateRequest>,
) -> axum::response::Response {
    // Session cookie first, then bearer PAT — the same resolution every other
    // non-graph edge uses, so an operator script holding a token reaches the
    // module surface the way the console does.
    let (principal, tenant) = match crate::graph_routes::resolve_identity(
        &state,
        &headers,
        session.as_deref(),
    ) {
        Ok(resolved) => resolved,
        Err(err) => return state.error_response(&err, &headers, None),
    };
    if let Err(err) = authorize(&state, principal, tenant) {
        return state.error_response(&err, &headers, None);
    }
    match publish_activation(&state, &request) {
        Ok(body) => Json(Envelope::ok(body)).into_response(),
        Err(err) => state.error_response(&err, &headers, None),
    }
}

/// Stage, reserve, publish and record one module's activation.
///
/// The whole sequence runs under [`crate::routes::DispatchBus::activation_guard`],
/// which is what makes the compare-and-swap mean something: no other activation
/// can land between this one reading the serving generation and publishing its
/// handlers, so the number reserved is the number published against.
///
/// Failure ordering is the point of the shape. Staging reads only — a refused
/// prepare, an absent module, an unpinned artifact — leaves the serving
/// generation untouched. The reserve is a compare-and-swap, so an operator who
/// staged against a generation that has since moved is refused with nothing
/// published. Only after both succeed does the single write land, and the
/// status surface records what that write made live.
fn publish_activation(
    state: &AppState,
    request: &ActivateRequest,
) -> Result<ActivateBody, AppError> {
    let _guard = state.bus.activation_guard();
    let root =
        std::env::current_dir().map_err(|e| AppError::internal(format!("resolve root: {e}")))?;

    // --- Stage: read-only, nothing serving changes yet --------------------
    let serving = state.runtime.generation();
    if request.expected_generation != serving {
        return Err(AppError::conflict(format!(
            "{} staged against generation {}, which is no longer serving",
            request.name, request.expected_generation
        )));
    }
    let index = kelson_registry::PackageIndex::load(&root)
        .map_err(|e| AppError::internal(format!("package index: {e}")))?;
    let entry = index
        .wasm
        .iter()
        .find(|entry| entry.name == request.name)
        .ok_or_else(|| AppError::not_found(format!("wasm module {}", request.name)))?;
    // The same pin gate boot enforces: a `kelson promote` pin that refuses a
    // boot must not be bypassable by activating the unpinned bytes instead.
    index
        .verify_pinned_boot(&root)
        .map_err(|e| AppError::conflict(format!("module pins: {e}")))?;
    // Discovery goes through the shared gate, so a sidecar boot would refuse
    // is a sidecar this edge refuses too — the two cannot drift.
    let sidecar = index
        .wasm_manifest(&root, entry)
        .map_err(AppError::validation)?;
    let artifact = kelson_registry::PackageIndex::resolve_path(&root, &entry.path);
    let bytes = std::fs::read(&artifact)
        .map_err(|e| AppError::validation(format!("read {}: {e}", artifact.display())))?;

    // Prepare: `WasmHost` here is this activation's own, and preparing one
    // touches nothing the serving generation resolves — the same reason the
    // boot path can afford to refuse here without a half-installed module.
    let host = kelson_wasm::WasmHost::new().map_err(|e| AppError::internal(format!("{e}")))?;
    let plugin = std::sync::Arc::new(host.prepare(&bytes, &sidecar).map_err(|e| {
        AppError::validation(format!("wasm plugin {}: prepare: {e}", request.name))
    })?);
    plugin
        .commit()
        .map_err(|e| AppError::internal(format!("wasm plugin {}: commit: {e}", request.name)))?;
    let handlers = kelson_wasm::handlers(plugin)
        .map_err(|e| AppError::internal(format!("wasm plugin {}: handlers: {e}", request.name)))?;

    // --- Reserve: the compare-and-swap ------------------------------------
    // Refusal here rewrites neither generation nor digest, exactly like a
    // refused config reload. The prepared plugin is dropped, never published.
    let generation = state
        .runtime
        .note_module_activation(
            request.expected_generation,
            &crate::runtime_status::artifacts_digest(&root),
        )
        .map_err(AppError::conflict)?;

    // --- Publish: one write, one complete aggregate for this module --------
    state.bus.publish_module(&request.name, handlers);

    // The response quotes the same surface the proof reads, so an operator and
    // the verification harness are looking at one number rather than two.
    let reported = state.runtime.body(
        "kelson-demo",
        env!("CARGO_PKG_VERSION"),
        state.modules.read().unwrap().snapshot(),
    );
    Ok(ActivateBody {
        module: request.name.clone(),
        generation,
        active_content_digest: reported["active_content_digest"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    })
}
