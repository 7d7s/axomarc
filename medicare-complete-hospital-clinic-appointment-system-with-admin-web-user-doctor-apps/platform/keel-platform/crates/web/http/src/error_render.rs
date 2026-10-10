//! # kelson-http error rendering (19-PARITY "Error rendering")
//! Typed `AppError` → HTTP status mapping registry with an extensible
//! per-code override table, the `reportable`/`renderable` split, per-code
//! custom renderers (modules register them from `Provider::init`) and the
//! RFC-7807 `application/problem+json` body builder for the api plane.
//!
//! The default table is `crate::status_for_error`, so behaviour is unchanged
//! until a module registers an override. Registration takes `&self` (interior
//! mutability): the registry is shared as an `Arc`, inserted into the boot
//! `ServiceRegistry`, and read by the dispatch edge on every error.

use std::collections::BTreeMap;
use std::sync::{PoisonError, RwLock};

use axum::http::StatusCode;
use kelson_foundation::{AppError, ErrorCategory};

use crate::status_for_error;
use crate::ErrorBody;

/// Content type of an RFC-7807 problem response.
pub const PROBLEM_JSON: &str = "application/problem+json";

/// Per-code custom renderer (the `Provider::init` registration entry point).
/// Returns extra wire detail merged into the envelope's `error.meta` (and,
/// for `application/problem+json`, the problem's extensions); `None`
/// contributes nothing.
pub type ErrorRenderer =
    std::sync::Arc<dyn Fn(&AppError) -> Option<serde_json::Value> + Send + Sync>;

/// What the transport edge does with one error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorRenderPlan {
    /// HTTP status: the per-code override when registered, else the category
    /// default (`status_for_error`).
    pub status: StatusCode,
    /// The error carries a render path — its own status, public message and
    /// meta reach the client. `false` = report-only: the mapped status is
    /// preserved but the client gets the generic internal body.
    pub renderable: bool,
    /// The error flows to observability (server-side faults report; client
    /// mistakes such as 404/422 are not logged as incidents).
    pub reportable: bool,
    /// Error view rendered for the code on the web plane; `None` = the edge
    /// falls back to the status-driven `errors/<status>` candidates.
    pub view: Option<String>,
}

/// Server-side faults are worth reporting; client mistakes are not.
fn default_reportable(err: &AppError) -> bool {
    matches!(
        err.category,
        ErrorCategory::Internal | ErrorCategory::Persistence | ErrorCategory::External
    )
}

#[derive(Default)]
struct RegistryInner {
    statuses: BTreeMap<String, StatusCode>,
    renderable: BTreeMap<String, bool>,
    reportable: BTreeMap<String, bool>,
    views: BTreeMap<String, String>,
    renderers: BTreeMap<String, ErrorRenderer>,
}

/// Wave 2 error-render registry: one instance per deployment, shared through
/// the `ServiceRegistry` so modules can extend it from `Provider::init`
/// while the edge reads the same map at request time.
#[derive(Default)]
pub struct ErrorRenderRegistry {
    inner: RwLock<RegistryInner>,
}

impl ErrorRenderRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, RegistryInner> {
        // A panic inside a renderer must not take down later requests: the
        // poisoned maps are still structurally sound.
        self.inner.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, RegistryInner> {
        self.inner.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// Override the status served for an error code (the mapping registry).
    pub fn set_status(&self, code: &str, status: StatusCode) {
        self.write().statuses.insert(code.to_string(), status);
    }

    /// `false` marks a code report-only (generic body, mapped status kept).
    pub fn set_renderable(&self, code: &str, renderable: bool) {
        self.write().renderable.insert(code.to_string(), renderable);
    }

    /// Force the reportable classification for a code.
    pub fn set_reportable(&self, code: &str, reportable: bool) {
        self.write().reportable.insert(code.to_string(), reportable);
    }

    /// Pin the error view rendered for a code on the web plane
    /// (`errors/404` style dot path).
    pub fn set_view(&self, code: &str, view: impl Into<String>) {
        self.write().views.insert(code.to_string(), view.into());
    }

    /// Register a custom renderer for a code (modules: `Provider::init`).
    pub fn register(&self, code: &str, renderer: ErrorRenderer) {
        self.write().renderers.insert(code.to_string(), renderer);
    }

    /// Registry-mapped status for an error.
    pub fn status_for(&self, err: &AppError) -> StatusCode {
        self.read()
            .statuses
            .get(&err.code)
            .copied()
            .unwrap_or_else(|| status_for_error(err))
    }

    /// The full render decision for an error.
    pub fn plan(&self, err: &AppError) -> ErrorRenderPlan {
        let inner = self.read();
        ErrorRenderPlan {
            status: inner
                .statuses
                .get(&err.code)
                .copied()
                .unwrap_or_else(|| status_for_error(err)),
            renderable: inner.renderable.get(&err.code).copied().unwrap_or(true),
            reportable: inner
                .reportable
                .get(&err.code)
                .copied()
                .unwrap_or_else(|| default_reportable(err)),
            view: inner.views.get(&err.code).cloned(),
        }
    }

    /// Custom renderer output for a code, when one registered.
    pub fn detail(&self, err: &AppError) -> Option<serde_json::Value> {
        let inner = self.read();
        inner.renderers.get(&err.code).and_then(|render| render(err))
    }
}

/// Choose the api-plane error representation. An explicit `Accept` header
/// always wins: `application/problem+json` selects RFC-7807,
/// `application/json` selects the Envelope. Otherwise the deployment default
/// (`[http] problem_json`) applies.
pub fn wants_problem(accept: Option<&str>, deployment_default: bool) -> bool {
    match accept {
        Some(a) if a.contains(PROBLEM_JSON) => true,
        Some(a) if a.contains("application/json") => false,
        _ => deployment_default,
    }
}

/// RFC-7807 problem body for one wire error. `type` is a URN derived from the
/// platform error code so clients can branch exactly as they do on the
/// Envelope's `error.code`; `debug` (present only under `app.debug`) carries
/// the redacted internal context the edge attached.
pub fn problem_json(
    status: StatusCode,
    body: &ErrorBody,
    instance: Option<&str>,
) -> serde_json::Value {
    let mut problem = serde_json::Map::new();
    problem.insert(
        "type".into(),
        serde_json::Value::String(format!("urn:kelson:error:{}", body.code)),
    );
    problem.insert(
        "title".into(),
        serde_json::Value::String(
            status
                .canonical_reason()
                .unwrap_or("Error")
                .to_string(),
        ),
    );
    problem.insert("status".into(), serde_json::json!(status.as_u16()));
    problem.insert(
        "detail".into(),
        serde_json::Value::String(body.message.clone()),
    );
    if let Some(instance) = instance {
        problem.insert("instance".into(), serde_json::json!(instance));
    }
    problem.insert("code".into(), serde_json::json!(body.code));
    problem.insert("retryable".into(), serde_json::json!(body.retryable));
    if let Some(meta) = &body.meta {
        problem.insert("meta".into(), meta.clone());
    }
    if let Some(debug) = &body.debug {
        problem.insert("debug".into(), debug.clone());
    }
    serde_json::Value::Object(problem)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Arc;

    #[test]
    fn defaults_match_the_category_table_and_split() {
        let reg = ErrorRenderRegistry::new();

        let internal = AppError::internal("boom");
        let plan = reg.plan(&internal);
        assert_eq!(plan.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(plan.reportable, "server faults report");
        assert!(plan.renderable);
        assert!(plan.view.is_none());

        let missing = AppError::not_found("Appointment");
        let plan = reg.plan(&missing);
        assert_eq!(plan.status, StatusCode::NOT_FOUND);
        assert!(!plan.reportable, "client mistakes do not report");

        let validation = AppError::validation("bad");
        assert_eq!(reg.status_for(&validation), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(!reg.plan(&validation).reportable);
    }

    #[test]
    fn per_code_overrides_win_over_the_default_table() {
        let reg = ErrorRenderRegistry::new();
        reg.set_status("KSN_NOT_FOUND", StatusCode::GONE);
        reg.set_reportable("KSN_NOT_FOUND", true);
        reg.set_renderable("KSN_INTERNAL", false);
        reg.set_view("KSN_NOT_FOUND", "errors.410");

        let missing = AppError::not_found("Appointment");
        let plan = reg.plan(&missing);
        assert_eq!(plan.status, StatusCode::GONE);
        assert!(plan.reportable);
        assert_eq!(plan.view.as_deref(), Some("errors.410"));

        let internal = AppError::internal("secret detail");
        assert!(!reg.plan(&internal).renderable);
        // Unrelated codes keep the default table.
        assert_eq!(
            reg.status_for(&AppError::validation("bad")),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[test]
    fn custom_renderers_contribute_detail() {
        let reg = ErrorRenderRegistry::new();
        assert!(reg.detail(&AppError::not_found("x")).is_none());

        reg.register("KSN_NOT_FOUND", {
            let renderer: ErrorRenderer = Arc::new(|err| {
                Some(json!({ "resource": err.code, "docs": "https://example.test/404" }))
            });
            renderer
        });
        let detail = reg.detail(&AppError::not_found("Appointment")).unwrap();
        assert_eq!(detail["docs"], json!("https://example.test/404"));
    }

    #[test]
    fn wants_problem_prefers_an_explicit_accept() {
        // Deployment default off (the Envelope stays the platform default).
        assert!(!wants_problem(None, false));
        assert!(!wants_problem(Some("*/*"), false));
        assert!(!wants_problem(Some("application/json"), false));
        assert!(wants_problem(Some(PROBLEM_JSON), false));

        // Deployment opt-in flips the default but never an explicit choice.
        assert!(wants_problem(None, true));
        assert!(wants_problem(Some("*/*"), true));
        assert!(!wants_problem(Some("application/json"), true));
        // Any explicit mention of `application/json` keeps the Envelope, even
        // at a lower q-value than a neighbouring HTML preference.
        assert!(!wants_problem(Some("text/html, application/json;q=0.9"), true));
        assert!(wants_problem(Some("text/html, application/problem+json;q=0.9"), true));
    }

    #[test]
    fn problem_body_is_rfc7807_shaped_with_platform_extensions() {
        let status = StatusCode::NOT_FOUND;
        let err = AppError::not_found("Appointment").with_meta("lookup", "tenant-scoped");
        let body = ErrorBody::from_app_error(&err).with_detail(json!({ "resource": "appointment" }));

        let problem = problem_json(status, &body, Some("/api/v1/appointments/x"));
        assert_eq!(problem["type"], json!("urn:kelson:error:KSN_NOT_FOUND"));
        assert_eq!(problem["title"], json!("Not Found"));
        assert_eq!(problem["status"], json!(404));
        assert_eq!(problem["detail"], json!(body.message));
        assert_eq!(problem["instance"], json!("/api/v1/appointments/x"));
        // Extensions keep the Envelope vocabulary clients already branch on.
        assert_eq!(problem["code"], json!("KSN_NOT_FOUND"));
        assert_eq!(problem["meta"]["lookup"], json!("tenant-scoped"));
        assert_eq!(problem["meta"]["resource"], json!("appointment"));
        assert!(problem.get("debug").is_none(), "debug never renders unasked");
    }

    #[test]
    fn reportable_flag_can_be_forced_both_ways() {
        let reg = ErrorRenderRegistry::new();
        reg.set_reportable("KSN_NOT_FOUND", true);
        assert!(reg.plan(&AppError::not_found("x")).reportable);
        reg.set_reportable("KSN_INTERNAL", false);
        assert!(!reg.plan(&AppError::internal("x")).reportable);
    }

    #[test]
    fn retryable_flag_lands_on_the_problem_body() {
        let mut body = ErrorBody::from_app_error(&AppError::cap_limit("slow down"));
        body.retryable = true;
        let problem = problem_json(StatusCode::TOO_MANY_REQUESTS, &body, None);
        assert_eq!(problem["retryable"], json!(true));
        assert!(problem.get("instance").is_none());
        assert_eq!(problem["status"], json!(429));
    }
}
