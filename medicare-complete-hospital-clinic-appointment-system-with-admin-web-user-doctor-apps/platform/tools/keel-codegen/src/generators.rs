//! Generators (item 30): make:<kind> scaffold templates. In-code templates —
//! deterministic, reviewable, versioned with the platform (11-SPEC §8).
//! Generated files are starting points: valid Rust skeletons that modules
//! wire into their own mod tree.

/// All generator kinds (keel make <kind> <name>).
pub const GENERATOR_KINDS: &[&str] = &[
    "migration",
    "seeder",
    "factory",
    "job",
    "event",
    "listener",
    "command",
    "query",
    "policy",
    "workflow",
    "model",
    "test",
    "resource",
    "request",
    "controller",
    // Wave 5 Phase 9 (docs/19): 15 -> 27 kinds.
    "cast",
    "channel",
    "component",
    "exception",
    "mail",
    "notification",
    "observer",
    "rule",
    "scope",
    "job-middleware",
    "middleware",
    "view",
];

fn validate_snake(name: &str) -> Result<(), String> {
    if name.is_empty()
        || !name
            .chars()
            .next()
            .map(|c| c.is_ascii_lowercase())
            .unwrap_or(false)
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        return Err(format!(
            "name must be snake_case [a-z][a-z0-9_]*: got {name:?}"
        ));
    }
    Ok(())
}

fn pascal(name: &str) -> String {
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect()
}

fn substitute(template: &str, module: &str, name: &str) -> String {
    template
        .replace("__MODULE__", module)
        .replace("__NAME__", name)
        .replace("__TYPE__", &pascal(name))
}

/// Generate one scaffold file: returns (path relative to workspace root, content).
/// Module-scoped kinds land under applications/<module>/src/<area>/.
pub fn generate(kind: &str, module: &str, name: &str) -> Result<(String, String), String> {
    if !GENERATOR_KINDS.contains(&kind) {
        return Err(format!(
            "unknown generator kind {kind:?}; expected one of: {}",
            GENERATOR_KINDS.join(", ")
        ));
    }
    validate_snake(name)?;
    let (path, template): (String, &str) = match kind {
        "migration" => (
            format!("database/migrations/{name}.sql"),
            MIGRATION_TEMPLATE,
        ),
        "seeder" => (
            format!("applications/{module}/src/seeders/{name}_seeder.rs"),
            SEEDER_TEMPLATE,
        ),
        "factory" => (
            format!("applications/{module}/src/factories/{name}_factory.rs"),
            FACTORY_TEMPLATE,
        ),
        "job" => (
            format!("applications/{module}/src/jobs/{name}_job.rs"),
            JOB_TEMPLATE,
        ),
        "event" => (
            format!("applications/{module}/src/events/{name}.rs"),
            EVENT_TEMPLATE,
        ),
        "listener" => (
            format!("applications/{module}/src/listeners/{name}_listener.rs"),
            LISTENER_TEMPLATE,
        ),
        "command" => (
            format!("applications/{module}/src/commands/{name}_command.rs"),
            COMMAND_TEMPLATE,
        ),
        "query" => (
            format!("applications/{module}/src/queries/{name}_query.rs"),
            QUERY_TEMPLATE,
        ),
        "policy" => (
            format!("applications/{module}/src/policies/{name}_policy.rs"),
            POLICY_TEMPLATE,
        ),
        "workflow" => (
            format!("applications/{module}/src/workflows/{name}.workflow.toml"),
            WORKFLOW_TEMPLATE,
        ),
        "model" => (
            format!("applications/{module}/src/models/{name}.rs"),
            MODEL_TEMPLATE,
        ),
        "test" => (
            format!("applications/{module}/tests/{name}_test.rs"),
            TEST_TEMPLATE,
        ),
        "resource" => (
            format!("applications/{module}/src/resources/{name}_resource.rs"),
            RESOURCE_TEMPLATE,
        ),
        "request" => (
            format!("applications/{module}/src/requests/{name}_request.rs"),
            REQUEST_TEMPLATE,
        ),
        "controller" => (
            format!("applications/{module}/src/controllers/{name}_controller.rs"),
            CONTROLLER_TEMPLATE,
        ),
        "cast" => (
            format!("applications/{module}/src/casts/{name}_cast.rs"),
            CAST_TEMPLATE,
        ),
        "channel" => (
            format!("applications/{module}/src/channels/{name}_channel.rs"),
            CHANNEL_TEMPLATE,
        ),
        "component" => (
            format!("applications/{module}/resources/views/components/{name}.html"),
            COMPONENT_TEMPLATE,
        ),
        "exception" => (
            format!("applications/{module}/src/exceptions/{name}_exception.rs"),
            EXCEPTION_TEMPLATE,
        ),
        "mail" => (
            format!("applications/{module}/src/mail/{name}_mail.rs"),
            MAIL_TEMPLATE,
        ),
        "notification" => (
            format!("applications/{module}/src/notifications/{name}_notification.rs"),
            NOTIFICATION_TEMPLATE,
        ),
        "observer" => (
            format!("applications/{module}/src/observers/{name}_observer.rs"),
            OBSERVER_TEMPLATE,
        ),
        "rule" => (
            format!("applications/{module}/src/rules/{name}_rule.rs"),
            RULE_TEMPLATE,
        ),
        "scope" => (
            format!("applications/{module}/src/scopes/{name}_scope.rs"),
            SCOPE_TEMPLATE,
        ),
        "job-middleware" => (
            format!("applications/{module}/src/jobs/{name}_middleware.rs"),
            JOB_MIDDLEWARE_TEMPLATE,
        ),
        "middleware" => (
            format!("applications/{module}/src/middleware/{name}_middleware.rs"),
            MIDDLEWARE_TEMPLATE,
        ),
        "view" => (
            format!("applications/{module}/resources/views/{name}.html"),
            VIEW_TEMPLATE,
        ),
        _ => unreachable!("kind validated above"),
    };
    Ok((path, substitute(template, module, name)))
}

const MIGRATION_TEMPLATE: &str = r##"-- migration: __NAME__
CREATE TABLE IF NOT EXISTS __NAME__ (
    id uuid PRIMARY KEY,
    created_at timestamptz NOT NULL DEFAULT now()
);
"##;

const SEEDER_TEMPLATE: &str = r##"//! Seeder: __NAME__. Register in the module seeder registry; keep it
//! idempotent (guarded inserts / ON CONFLICT DO NOTHING).

use keel_postgres::{BoxFuture, PgPool, SeedReport};

pub struct __TYPE__Seeder;

impl keel_postgres::Seeder for __TYPE__Seeder {
    fn name(&self) -> &str { "__NAME__" }

    fn run<'a>(&'a self, _pool: &'a PgPool) -> BoxFuture<'a, Result<SeedReport, keel_foundation::AppError>> {
        Box::pin(async move { Ok(SeedReport::default()) })
    }
}
"##;

const FACTORY_TEMPLATE: &str = r##"//! Factory: __TYPE__ fixtures (keel-testkit Factory trait). Defaults must
//! build; overrides stay explicit at the call site.

use std::sync::atomic::{AtomicU64, Ordering};

use keel_testkit::Factory;

/// Monotonic per-process sequence: generated defaults stay unique and
/// deterministic (no wall-clock dependence in fixture values).
static __TYPE__FACTORY_SEQ: AtomicU64 = AtomicU64::new(1);

/// The fixture product. Extend with real domain fields as the model lands;
/// keep every field derivable from defaults so `make()` stays zero-config.
#[derive(Debug, Clone)]
pub struct __TYPE__ {
    pub id: uuid::Uuid,
    pub label: String,
}

/// Builder with explicit overrides; unset fields get deterministic defaults.
#[derive(Debug, Clone, Default)]
pub struct __TYPE__Factory {
    pub id: Option<uuid::Uuid>,
    pub label: Option<String>,
}

impl Factory for __TYPE__Factory {
    type Item = __TYPE__;

    fn build(self) -> __TYPE__ {
        let seq = __TYPE__FACTORY_SEQ.fetch_add(1, Ordering::Relaxed);
        __TYPE__ {
            id: self.id.unwrap_or_else(uuid::Uuid::now_v7),
            label: self.label.unwrap_or_else(|| format!("__NAME__-{seq}")),
        }
    }
}
"##;

const JOB_TEMPLATE: &str = r##"//! Durable job: __NAME__. Register a JobDef in the job registry and this
//! handler in the worker registry (module needs the keel-jobs dependency).

use keel_foundation::AppError;
use keel_jobs::{BoxFuture, DurableJob, JobHandler};

pub struct __TYPE__Job;

impl JobHandler for __TYPE__Job {
    fn name(&self) -> &str { "__NAME__" }

    fn handle<'a>(&'a self, job: &'a DurableJob) -> BoxFuture<'a, Result<(), AppError>> {
        Box::pin(async move {
            tracing::info!(job_id = %job.id, "__NAME__ executed");
            Ok(())
        })
    }
}
"##;

const EVENT_TEMPLATE: &str = r##"//! Event: __NAME__. Declare it in module.toml [[events]] and queue it from
//! an execution unit (post-commit dispatch, D-53).

pub const EVENT_NAME: &str = "__NAME__";

/// Payload shape for __NAME__.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct __TYPE__Event {
    pub id: uuid::Uuid,
}
"##;

const LISTENER_TEMPLATE: &str = r##"//! Listener: reacts to events through the ListenerRegistry. Handlers must
//! be idempotent (at-least-once delivery).

use keel_foundation::AppError;
use keel_event::{BoxFuture, ListenerHandler, OutboxEvent};

pub struct __TYPE__Listener;

impl ListenerHandler for __TYPE__Listener {
    fn handle(&self, event: &OutboxEvent) -> BoxFuture<'_, Result<(), AppError>> {
        Box::pin(async move {
            tracing::info!(event = %event.event, "__NAME__ handled");
            Ok(())
        })
    }
}
"##;

const COMMAND_TEMPLATE: &str = r##"//! Command operation: __NAME__ (Interactive profile). Register an
//! OperationHandler on the module command bus.

pub const OPERATION: &str = "__NAME__";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct __TYPE__Input {
    pub id: uuid::Uuid,
}
"##;

const QUERY_TEMPLATE: &str = r##"//! Query operation: __NAME__ (Read profile — mutations are rejected by the
//! lineage guard).

pub const OPERATION: &str = "__NAME__";

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct __TYPE__Filter {
    pub page: u32,
    pub per_page: u32,
}
"##;

const POLICY_TEMPLATE: &str = r##"//! Policy: __TYPE__. Decision logic for the policy engine — pure data in,
//! decision out, no I/O.

pub struct __TYPE__Policy;

impl __TYPE__Policy {
    pub fn new() -> Self { Self }
}

impl Default for __TYPE__Policy {
    fn default() -> Self { Self::new() }
}
"##;

const WORKFLOW_TEMPLATE: &str = r##"# Workflow: __NAME__ (durable multi-step orchestration).
# Reference it from module.toml [[workflows]].

[workflow]
name = "__NAME__"
version = "1"

[[steps]]
name = "start"
operation = "__NAME__.start"
"##;

const MODEL_TEMPLATE: &str = r##"//! Model: __TYPE__ domain type + persistence row shape.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct __TYPE__ {
    pub id: Uuid,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
}
"##;

const TEST_TEMPLATE: &str = r##"//! Test: __NAME__ — uses keel-testkit (TestApp, fakes, factories).

#[tokio::test]
async fn __NAME___smoke() {
    // let app = TestApp::new(my_router()).acting_as("principal-test");
    // app.get("/api/v1/...").send().await.assert_status(200);
}
"##;

const RESOURCE_TEMPLATE: &str = r##"//! Resource: API representation of __TYPE__ (keel-http Resource trait).

/// The source type the resource renders.
pub struct __TYPE__;

pub struct __TYPE__Resource;

impl keel_http::Resource for __TYPE__Resource {
    type Source = __TYPE__;

    fn from_source(_source: &Self::Source) -> serde_json::Value {
        // Conditional fields: `.when(condition, key, value)` omits the key
        // entirely when the condition is false (Laravel `when` semantics);
        // `.when_some(key, option)` omits on None. Render lists through
        // `keel_http::ResourceCollection` (pagination meta passes through).
        keel_http::Fields::new()
            .field("type", "__NAME__")
            .into_value()
    }
}
"##;

const REQUEST_TEMPLATE: &str = r##"//! Request: validated input contract for __NAME__. Rules run through
//! keel-validation before the operation sees the payload.

use keel_validation::{Rule, Validator};

pub fn __NAME___rules() -> Validator {
    Validator::new().field("id", vec![Rule::Required])
}
"##;

const CONTROLLER_TEMPLATE: &str = r##"//! Controller: HTTP edge for __NAME__. Maps request -> operation dispatch
//! -> envelope. Keep transport concerns out of the domain.

pub async fn handle() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({ "operation": "__NAME__" }))
}
"##;

const CAST_TEMPLATE: &str = r##"//! Cast: __NAME__ — wire value <-> stored value (keel_models::Cast).
//! Register the cast on the model's field; transforms are pure JSON in,
//! JSON out — no I/O, no wall-clock, no ambient state.

use keel_models::Cast;

pub struct __TYPE__Cast;

impl Cast for __TYPE__Cast {
    fn to_db(&self, wire: &serde_json::Value) -> Result<serde_json::Value, keel_foundation::AppError> {
        Ok(wire.clone())
    }

    fn from_db(&self, stored: &serde_json::Value) -> Result<serde_json::Value, keel_foundation::AppError> {
        Ok(stored.clone())
    }
}
"##;

const CHANNEL_TEMPLATE: &str = r##"//! Broadcast channel: __NAME__ — capability-gated subscription metadata.
//! The authoritative registration lives in routes/channels.toml (merged
//! into the graph with the plane forced by file); boot refuses a channel
//! whose capability is undeclared with GRAPH-CHANNEL-CAP:
//!
//! [[channel]]
//! name = "__NAME__"
//! capability = "__NAME__"
//!
//! Declare the capability in this module manifest's [capabilities]:
//! `__NAME__ = { profile = "READ", inputs = [] }`. A dotted capability must
//! name a qualified operation instead (the gate checks operations, not
//! capabilities, for dotted values).

/// Channel name clients subscribe to (matches routes/channels.toml).
pub const CHANNEL: &str = "__NAME__";

/// Graph capability gating subscriptions (see the manifest note above).
pub const CAPABILITY: &str = "__NAME__";

/// Module-side registration payload for tooling and tests.
pub fn channel() -> (&'static str, &'static str) {
    (CHANNEL, CAPABILITY)
}
"##;

const COMPONENT_TEMPLATE: &str = r##"<!-- Component: __NAME__ — include from a view with
     @include("components.__NAME__", ["title" => heading]).
     @props documents the expected data keys (documentation-only; a missing
     key echoes as empty). -->
@props(["title"])
<div class="__NAME__">
    <h2>{{ title }}</h2>
</div>
"##;

const EXCEPTION_TEMPLATE: &str = r##"//! Exception: __TYPE__Error — typed failure for __NAME__ that converts
//! into the platform error envelope (keel_foundation::AppError). The edge
//! renders the envelope; HTTP error pages resolve from the views error map
//! by status (errors.404 -> errors/404.html).

use keel_foundation::{AppError, ErrorCategory, Retryability, Severity};

/// Stable machine code for __NAME__ failures (never renumber, only append).
pub const __NAME___CODE: &str = "MODULE___NAME__";

#[derive(Debug, Clone)]
pub struct __TYPE__Error {
    pub reason: String,
}

impl std::fmt::Display for __TYPE__Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason)
    }
}

impl From<__TYPE__Error> for AppError {
    fn from(err: __TYPE__Error) -> Self {
        AppError::new(
            __NAME___CODE,
            ErrorCategory::Domain,
            Severity::Medium,
            Retryability::NotRetryable,
            err.reason,
        )
    }
}
"##;

const MAIL_TEMPLATE: &str = r##"//! Mailable: __TYPE__Mail — the module's outbound mail definition for
//! __NAME__. Wave 6 lands the framework Mailable + the [mail.mailers]
//! failover chain (queued as the `mail.send` job with view-rendered
//! bodies); this definition compiles standalone and maps 1:1 onto it.

#[derive(Debug, Clone)]
pub struct __TYPE__Mail {
    pub to: String,
    pub subject: String,
    pub body_text: String,
}

impl __TYPE__Mail {
    pub fn new(to: impl Into<String>) -> Self {
        Self {
            to: to.into(),
            subject: format!("__NAME__ update"),
            body_text: format!("Hello from __NAME__."),
        }
    }

    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }
}
"##;

const NOTIFICATION_TEMPLATE: &str = r##"//! Notification: __TYPE__Notification with per-channel renderers.
//! Wave 6 ships the framework NotificationDef (fan-out across mail, sms,
//! push and database channels with locale fallback); these renderers
//! compile standalone and map 1:1 onto those channels.

use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct __TYPE__Notification {
    pub data: Value,
}

impl __TYPE__Notification {
    pub fn new(data: Value) -> Self {
        Self { data }
    }

    /// Mail channel: (subject, plain-text body).
    pub fn to_mail(&self) -> (String, String) {
        (format!("__NAME__ update"), self.data.to_string())
    }

    /// SMS channel: one short segment (keep it under 160 chars).
    pub fn to_sms(&self) -> String {
        format!("__NAME__: {}", self.data)
    }

    /// Push channel: title + payload.
    pub fn to_push(&self) -> Value {
        json!({ "title": "__NAME__ update", "data": self.data })
    }

    /// Database channel: the persisted inbox row payload.
    pub fn to_database(&self) -> Value {
        json!({ "type": "__NAME__", "data": self.data })
    }
}
"##;

const OBSERVER_TEMPLATE: &str = r##"//! Observer: __NAME__ — reacts to model events (keel_models::ModelObserver).
//! Register it where the runtime is built:
//! `.with_observer(Arc::new(__TYPE__Observer))`. Handlers run on the write
//! path: keep them fast and idempotent.

use std::sync::Arc;

use keel_models::{ModelEvent, ModelObserver};

pub struct __TYPE__Observer;

impl ModelObserver for __TYPE__Observer {
    fn on_model_event(&self, event: &ModelEvent) {
        tracing::info!(model = %event.model, "observed __NAME__ model event");
    }
}

/// Arc-wrapped instance ready for `ModelRuntime::with_observer`.
pub fn observer() -> Arc<dyn ModelObserver> {
    Arc::new(__TYPE__Observer)
}
"##;

const RULE_TEMPLATE: &str = r##"//! Custom validation rule: __NAME__ — register it at module boot and
//! reference it from validation manifests with `custom = "__NAME__"`
//! (Wave 4 Phase 4 surface; an unregistered name fails the field with
//! "custom rule __NAME__ is not registered").

use std::sync::Arc;

use keel_validation::{register_custom_rule, CustomRule};

pub struct __TYPE__Rule;

impl CustomRule for __TYPE__Rule {
    fn check(
        &self,
        field: &str,
        value: &serde_json::Value,
        _input: &serde_json::Value,
    ) -> Option<String> {
        // Some(message) FAILS the field; None passes. Start permissive and
        // tighten here (e.g. reject values outside an allowed set).
        let _ = (field, value);
        None
    }
}

/// Registration is safe at boot: re-registering replaces the entry.
pub fn register() {
    register_custom_rule("__NAME__", Arc::new(__TYPE__Rule));
}
"##;

const SCOPE_TEMPLATE: &str = r##"//! Model scope: __NAME__ — a named query filter over the keel_models
//! FieldFilter list. Register it on the runtime with
//! `.with_scope("__NAME__", __NAME___scope)`; named scopes and the global
//! scope compose on reads.

use keel_models::FieldFilter;

pub fn __NAME___scope(filters: Vec<FieldFilter>) -> Vec<FieldFilter> {
    // Narrow the read here (e.g. drop archived rows); returning the input
    // unchanged keeps every other filter untouched.
    filters
}
"##;

const JOB_MIDDLEWARE_TEMPLATE: &str = r##"//! Job middleware: __NAME__ — wraps durable-job execution (pass-through
//! logging by default). Wrap `next` where the worker dispatches the job;
//! delivery is at-least-once, so anything the middleware does must be
//! idempotent.

use keel_foundation::AppError;
use keel_jobs::{BoxFuture, DurableJob};

pub struct __TYPE__JobMiddleware;

impl __TYPE__JobMiddleware {
    pub fn handle<'a>(
        &'a self,
        job: &'a DurableJob,
        next: BoxFuture<'a, Result<(), AppError>>,
    ) -> BoxFuture<'a, Result<(), AppError>> {
        Box::pin(async move {
            tracing::debug!(job_id = %job.id, "__NAME__ middleware: before");
            let result = next.await;
            tracing::debug!(job_id = %job.id, "__NAME__ middleware: after");
            result
        })
    }
}
"##;

const MIDDLEWARE_TEMPLATE: &str = r##"//! Builtin-style middleware: __NAME__ — an admission layer the module
//! layers onto its own router with
//! `axum::middleware::from_fn(__NAME___middleware)`. The platform admission
//! chain stays framework-owned: middleware.toml aliases/groups resolve over
//! the builtin ids (auth, verified, confirmed, signed, throttle:*), and an
//! unknown id refuses boot with GRAPH-MW-UNKNOWN.

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;

pub async fn __NAME___middleware(request: Request, next: Next) -> Response {
    tracing::debug!(path = %request.uri().path(), "__NAME__ middleware: before");
    let response = next.run(request).await;
    tracing::debug!("__NAME__ middleware: after");
    response
}
"##;

const VIEW_TEMPLATE: &str = r##"<!-- View: __NAME__ — dot name "__MODULE__.__name__". View names resolve as
     dot paths under the configured view roots ([views].paths; empty falls
     back to resources/views), so list applications/__MODULE__/resources/views
     to render this file directly; cross-file references use
     @include("path.to.template"). -->
<section class="__NAME__">
    <h1>__NAME__</h1>
</section>
"##;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_kind_and_name() {
        assert!(generate("bogus", "demo", "thing").is_err());
        assert!(generate("job", "demo", "Bad-Name").is_err());
        let (path, _) = generate("job", "demo", "send_mail").unwrap();
        assert_eq!(path, "applications/demo/src/jobs/send_mail_job.rs");
    }

    #[test]
    fn all_kinds_produce_substituted_content() {
        for kind in GENERATOR_KINDS {
            let (path, content) = generate(kind, "demo", "sample_thing").unwrap();
            assert!(!path.is_empty());
            assert!(!content.is_empty(), "empty template for {kind}");
            assert!(
                !content.contains("__NAME__"),
                "unsubstituted name in {kind}"
            );
            assert!(
                !content.contains("__MODULE__"),
                "unsubstituted module in {kind}"
            );
        }
    }

    #[test]
    fn pascal_converts_snake_case() {
        assert_eq!(pascal("send_mail"), "SendMail");
        assert_eq!(pascal("report"), "Report");
    }

    /// Production gate (report §6): generated scaffold may never ship
    /// placeholder macros — every template must build as written.
    #[test]
    fn generated_code_contains_no_placeholder_macros() {
        for kind in GENERATOR_KINDS {
            let (_, content) = generate(kind, "demo", "sample_thing").unwrap();
            assert!(
                !content.contains("todo!("),
                "placeholder todo! in {kind} template"
            );
            assert!(
                !content.contains("unimplemented!("),
                "placeholder unimplemented! in {kind} template"
            );
        }
    }

    #[test]
    fn factory_build_is_real_and_deterministic() {
        let (_, content) = generate("factory", "demo", "widget").unwrap();
        assert!(content.contains("fn build(self) -> Widget"));
        assert!(content.contains("WidgetFACTORY_SEQ.fetch_add"));
        assert!(content.contains("pub struct WidgetFactory"));
    }

    /// Wave 3 Phase 7: the resource scaffold implements the REAL trait. The
    /// old template wrote `impl keel_http::Resource<Source>` against the
    /// viewmodel STRUCT — a scaffold that could never build (the doctest on
    /// `keel_http::Resource` mirrors this shape as the compile proof).
    #[test]
    fn resource_template_emits_the_trait_impl() {
        let (path, content) = generate("resource", "demo", "sample_thing").unwrap();
        assert_eq!(
            path,
            "applications/demo/src/resources/sample_thing_resource.rs"
        );
        assert!(content.contains("impl keel_http::Resource for SampleThingResource"));
        assert!(content.contains("type Source = SampleThing;"));
        assert!(content.contains(r#".field("type", "sample_thing")"#));
        // The struct-as-trait bug must not come back.
        assert!(!content.contains("impl keel_http::Resource<"));
        // The impl compiles standalone: `from_source` is infallible Value
        // rendering with no todo!/unimplemented! (covered by the loop test).
        assert!(content.contains("keel_http::Fields::new()"));
    }

    /// Wave 5 Phase 9 (docs/19): kinds 15 -> 27 — every Wave 5 kind is
    /// discoverable and resolves to its documented module-scoped path with
    /// substituted, placeholder-free content.
    #[test]
    fn wave5_adds_twelve_generator_kinds() {
        assert_eq!(GENERATOR_KINDS.len(), 27, "15 existing + 12 Wave 5 kinds");
        for kind in [
            "cast",
            "channel",
            "component",
            "exception",
            "mail",
            "notification",
            "observer",
            "rule",
            "scope",
            "job-middleware",
            "middleware",
            "view",
        ] {
            assert!(GENERATOR_KINDS.contains(&kind), "{kind} missing");
            let (path, content) = generate(kind, "demo", "sample_thing").unwrap();
            assert!(path.starts_with("applications/demo/"), "{kind}: {path}");
            assert!(!content.contains("__NAME__"), "unsubstituted name in {kind}");
            assert!(!content.contains("__MODULE__"), "unsubstituted module in {kind}");
            assert!(!content.contains("__TYPE__"), "unsubstituted type in {kind}");
            assert!(!content.contains("todo!("), "placeholder in {kind}");
            assert!(!content.contains("unimplemented!("), "placeholder in {kind}");
        }
    }

    /// Wave 5 Phase 9: the new templates point at REAL platform surface —
    /// the traits and APIs they claim (Cast, ModelObserver, CustomRule,
    /// FieldFilter scopes, the axum middleware shape, routes/channels.toml)
    /// rather than invented ones; the Wave 6-pending definitions (mail,
    /// notification) stay standalone and say so.
    #[test]
    fn wave5_templates_reference_real_platform_surface() {
        let trait_refs = [
            ("cast", "impl Cast for SampleThingCast"),
            ("observer", "impl ModelObserver for SampleThingObserver"),
            ("rule", "register_custom_rule(\"sample_thing\""),
            ("scope", "pub fn sample_thing_scope(filters: Vec<FieldFilter>)"),
            (
                "middleware",
                "pub async fn sample_thing_middleware(request: Request, next: Next)",
            ),
            ("channel", "routes/channels.toml"),
            ("exception", "impl From<SampleThingError> for AppError"),
            ("job-middleware", "keel_jobs::{BoxFuture, DurableJob}"),
        ];
        for (kind, needle) in trait_refs {
            let (_, content) = generate(kind, "demo", "sample_thing").unwrap();
            assert!(content.contains(needle), "{kind} missing {needle:?}");
        }
        for kind in ["mail", "notification"] {
            let (_, content) = generate(kind, "demo", "sample_thing").unwrap();
            assert!(content.contains("Wave 6"), "{kind} documents its Wave 6 landing");
        }
        // Template files land under the module's view root.
        let (path, content) = generate("view", "demo", "sample_thing").unwrap();
        assert_eq!(
            path,
            "applications/demo/resources/views/sample_thing.html"
        );
        assert!(content.contains("@include"), "view documents include resolution");
        let (path, content) = generate("component", "demo", "sample_thing").unwrap();
        assert_eq!(
            path,
            "applications/demo/resources/views/components/sample_thing.html"
        );
        assert!(content.contains("@props"), "component documents its props");
    }
}
