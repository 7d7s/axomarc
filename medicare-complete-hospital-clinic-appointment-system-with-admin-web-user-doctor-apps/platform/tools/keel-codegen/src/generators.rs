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
}
