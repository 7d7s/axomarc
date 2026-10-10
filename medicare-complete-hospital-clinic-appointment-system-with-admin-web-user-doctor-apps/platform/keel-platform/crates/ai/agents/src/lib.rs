//! # kelson-ai-agents — agent runtime (item 33, L48)
//!
//! The execution path is fixed: AI Agent -> Tool -> Execution -> Capability ->
//! Scope -> Policy -> Command. An agent can never magically obtain elevated
//! authority: tool calls execute through [`Engine`] under the *caller's*
//! principal and context, so the same capability/scope/policy gates apply.
//! Denied tools are reported back to the model as tool errors — the runtime
//! never retries with a different principal.

use std::sync::Arc;

use kelson_ai::{AiClient, AiMessage, CompletionRequest, ModelTool};
use kelson_ai_tools::{ToolRegistry, ToolSpec};
use kelson_context::ExecutionContext;
use kelson_foundation::{codes, AppError, OperationId};
use kelson_execution::{Engine, IdempotencyKey, OperationHandler};
use serde::{Deserialize, Serialize};

/// Resolves the live handler for an operation id (wired by the runtime boot).
pub trait HandlerResolver: Send + Sync {
    fn resolve(&self, operation: &OperationId) -> Option<Arc<dyn OperationHandler>>;
}

/// One executed tool call (diagnostic chain for the agent run).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolRun {
    pub tool: String,
    pub operation: String,
    pub allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Final outcome of an agent run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentOutcome {
    pub answer: String,
    pub tool_runs: Vec<ToolRun>,
    pub steps: usize,
}

/// Agent loop runner.
pub struct AgentRuntime {
    ai: Arc<dyn AiClient>,
    tools: Arc<dyn ToolRegistry>,
    engine: Arc<Engine>,
    handlers: Arc<dyn HandlerResolver>,
    max_steps: usize,
}

impl AgentRuntime {
    pub fn new(
        ai: Arc<dyn AiClient>,
        tools: Arc<dyn ToolRegistry>,
        engine: Arc<Engine>,
        handlers: Arc<dyn HandlerResolver>,
    ) -> Self {
        Self {
            ai,
            tools,
            engine,
            handlers,
            max_steps: 8,
        }
    }

    pub fn with_max_steps(mut self, max_steps: usize) -> Self {
        self.max_steps = max_steps.max(1);
        self
    }

    /// Run the loop: model proposes, tools execute through the engine, results
    /// flow back until the model answers without further tool calls.
    pub async fn run(
        &self,
        ctx: Arc<ExecutionContext>,
        goal: &str,
    ) -> Result<AgentOutcome, AppError> {
        let model_tools: Vec<ModelTool> = self
            .tools
            .specs()
            .iter()
            .map(ToolSpec::model_tool)
            .collect();
        let mut messages = vec![
            AiMessage::system(
                "You are a platform agent. Use the provided tools to act; every tool call runs under the caller's permissions and cannot be elevated.",
            ),
            AiMessage::user(goal),
        ];
        let mut tool_runs: Vec<ToolRun> = Vec::new();

        for step in 1..=self.max_steps {
            let response = self.ai.complete(&CompletionRequest {
                messages: messages.clone(),
                tools: model_tools.clone(),
                temperature: 0.0,
            })?;
            let message = response.message;
            if message.tool_calls.is_empty() {
                return Ok(AgentOutcome {
                    answer: message.content,
                    tool_runs,
                    steps: step,
                });
            }
            messages.push(message.clone());

            for call in &message.tool_calls {
                let tool_result = self.execute_tool(ctx.clone(), call, &mut tool_runs).await;
                messages.push(AiMessage::tool_result(call.id.clone(), tool_result));
            }
        }

        Err(AppError::new(
            codes::INTERNAL,
            kelson_foundation::ErrorCategory::Internal,
            kelson_foundation::Severity::Medium,
            kelson_foundation::Retryability::NotRetryable,
            format!(
                "agent exceeded {} steps without a final answer",
                self.max_steps
            ),
        ))
    }

    async fn execute_tool(
        &self,
        ctx: Arc<ExecutionContext>,
        call: &kelson_ai::ToolCall,
        tool_runs: &mut Vec<ToolRun>,
    ) -> String {
        let Some(spec) = self.tools.get(&call.name) else {
            tool_runs.push(ToolRun {
                tool: call.name.clone(),
                operation: String::new(),
                allowed: false,
                error: Some("unknown tool".into()),
            });
            return serde_json::json!({ "error": format!("unknown tool: {}", call.name), "code": codes::NOT_FOUND }).to_string();
        };
        let Some(handler) = self.handlers.resolve(&spec.operation) else {
            tool_runs.push(ToolRun {
                tool: call.name.clone(),
                operation: spec.operation.to_string(),
                allowed: false,
                error: Some("no handler registered".into()),
            });
            return serde_json::json!({ "error": format!("no handler for operation {}", spec.operation), "code": codes::NOT_IMPLEMENTED })
                .to_string();
        };

        // Deterministic per run+call: same goal replayed is idempotent.
        let idempotency = IdempotencyKey(format!("agent-{}-{}", ctx.execution_id, call.id));
        match self
            .engine
            .execute(ctx, &*handler, call.arguments.clone(), Some(idempotency))
            .await
        {
            Ok(outcome) => {
                tool_runs.push(ToolRun {
                    tool: call.name.clone(),
                    operation: spec.operation.to_string(),
                    allowed: true,
                    error: None,
                });
                outcome.result.to_string()
            }
            Err(error) => {
                tool_runs.push(ToolRun {
                    tool: call.name.clone(),
                    operation: spec.operation.to_string(),
                    allowed: false,
                    error: Some(error.public_message.clone()),
                });
                serde_json::json!({ "error": error.public_message, "code": error.code }).to_string()
            }
        }
    }
}

/// Validate that an agent-produced tool call targets a registered tool;
/// wiring-time check for static manifests (MCP generation uses it too).
pub fn known_tool(registry: &dyn ToolRegistry, name: &str) -> bool {
    registry.get(name).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kelson_ai::{StubAiClient, ToolCall};
    use kelson_ai_tools::StaticToolRegistry;
    use kelson_context::{PrincipalContext, ScopeSnapshot, TenantContext};
    use kelson_foundation::{AuthorizationRevision, ExecutionProfileKind, PrincipalId, TenantId};
    use kelson_execution::{
        BoxFuture, ExecutionUnit, InMemoryBackend, InMemoryIdempotency, MutationRecord,
        PlanCompiler, PlanIntent,
    };
    use kelson_policy::{AccessSnapshot, PolicyAuthz, PolicyEngine};
    use std::collections::BTreeMap;

    struct NoteCreate {
        plan: kelson_execution::ExecutionPlan,
    }
    impl NoteCreate {
        fn new() -> Self {
            let intent = PlanIntent::new(
                OperationId::from_static("notes.create"),
                ExecutionProfileKind::Command,
            )
            .with_capability("notes.create");
            Self {
                plan: PlanCompiler::compile(&intent),
            }
        }
    }
    impl OperationHandler for NoteCreate {
        fn plan(&self) -> &kelson_execution::ExecutionPlan {
            &self.plan
        }
        fn handle(
            &self,
            _ctx: Arc<ExecutionContext>,
            input: serde_json::Value,
            unit: &mut dyn ExecutionUnit,
        ) -> BoxFuture<'static, Result<serde_json::Value, AppError>> {
            unit.record_mutation(MutationRecord {
                resource_type: "note".into(),
                resource_id: Some(uuid::Uuid::now_v7()),
                resource_version: 1,
                before: None,
                after: input,
                stream: None,
            });
            Box::pin(async move { Ok(serde_json::json!({ "ok": true })) })
        }
    }

    struct BillingCharge {
        plan: kelson_execution::ExecutionPlan,
    }
    impl BillingCharge {
        fn new() -> Self {
            let intent = PlanIntent::new(
                OperationId::from_static("billing.charge"),
                ExecutionProfileKind::Command,
            )
            .with_capability("billing.charge");
            Self {
                plan: PlanCompiler::compile(&intent),
            }
        }
    }
    impl OperationHandler for BillingCharge {
        fn plan(&self) -> &kelson_execution::ExecutionPlan {
            &self.plan
        }
        fn handle(
            &self,
            _ctx: Arc<ExecutionContext>,
            _input: serde_json::Value,
            unit: &mut dyn ExecutionUnit,
        ) -> BoxFuture<'static, Result<serde_json::Value, AppError>> {
            unit.record_mutation(MutationRecord {
                resource_type: "charge".into(),
                resource_id: Some(uuid::Uuid::now_v7()),
                resource_version: 1,
                before: None,
                after: serde_json::json!({}),
                stream: None,
            });
            Box::pin(async move { Ok(serde_json::json!({ "charged": true })) })
        }
    }

    #[derive(Default)]
    struct Handlers {
        map: BTreeMap<String, Arc<dyn OperationHandler>>,
    }
    impl HandlerResolver for Handlers {
        fn resolve(&self, operation: &OperationId) -> Option<Arc<dyn OperationHandler>> {
            self.map.get(operation.as_str()).cloned()
        }
    }

    fn fixture(
        allowed_capability: Option<&str>,
    ) -> (
        Arc<Engine>,
        Arc<InMemoryBackend>,
        AgentRuntime,
        Arc<StubAiClient>,
        PrincipalId,
        TenantId,
    ) {
        let principal = PrincipalId::new();
        let tenant = TenantId::new();
        let policies = Arc::new(PolicyEngine::new());
        policies.install_snapshot(AccessSnapshot {
            principal_id: principal,
            tenant_id: tenant,
            membership_active: true,
            capabilities: allowed_capability
                .into_iter()
                .map(|c| c.to_string())
                .collect(),
            scope_paths: ["/".to_string()].into_iter().collect(),
            authorization_revision: policies.revision(),
            feature_revision: kelson_foundation::FeatureRevision(1),
            module_revision: kelson_foundation::ModuleRevision(1),
            computed_at: time::OffsetDateTime::now_utc(),
        });
        let backend = Arc::new(InMemoryBackend::new());
        let engine = Arc::new(Engine::new(
            Arc::new(PolicyAuthz { engine: policies }),
            backend.clone(),
            Arc::new(InMemoryIdempotency::new()),
        ));
        let handlers = Arc::new(Handlers {
            map: [
                (
                    "notes.create".to_string(),
                    Arc::new(NoteCreate::new()) as Arc<dyn OperationHandler>,
                ),
                (
                    "billing.charge".to_string(),
                    Arc::new(BillingCharge::new()) as Arc<dyn OperationHandler>,
                ),
            ]
            .into_iter()
            .collect(),
        });
        let tools = Arc::new(StaticToolRegistry::with_tools(vec![
            kelson_ai_tools::ToolSpec::new(
                "notes__create",
                "Create a note",
                OperationId::from_static("notes.create"),
            )
            .with_capability("notes.create"),
            kelson_ai_tools::ToolSpec::new(
                "billing__charge",
                "Charge billing",
                OperationId::from_static("billing.charge"),
            )
            .with_capability("billing.charge"),
        ]));
        let stub = Arc::new(StubAiClient::new());
        let runtime = AgentRuntime::new(stub.clone(), tools, engine.clone(), handlers);
        (engine, backend, runtime, stub, principal, tenant)
    }

    fn ctx(principal: PrincipalId, tenant: TenantId) -> Arc<ExecutionContext> {
        ExecutionContext::builder(
            ExecutionProfileKind::Command,
            time::OffsetDateTime::now_utc(),
        )
        .principal(PrincipalContext {
            id: principal,
            display: Some("agent-tester".into()),
        })
        .tenant(TenantContext {
            id: tenant,
            slug: "acme".into(),
            plan: "standard".into(),
        })
        .scope(ScopeSnapshot {
            nodes: vec![],
            revision: AuthorizationRevision(1),
            computed_at: time::OffsetDateTime::now_utc(),
        })
        .build()
        .unwrap()
    }

    #[tokio::test]
    async fn agent_tool_call_executes_through_engine() {
        let (_engine, backend, runtime, stub, principal, tenant) = fixture(Some("notes.create"));
        let context = ctx(principal, tenant);
        stub.push_turn(AiMessage::tool_calls(vec![ToolCall {
            id: "call-1".into(),
            name: "notes__create".into(),
            arguments: serde_json::json!({ "body": "from agent" }),
        }]));
        stub.push_turn(AiMessage::assistant("note created"));
        let outcome = runtime.run(context, "create a note").await.unwrap();
        assert_eq!(outcome.answer, "note created");
        assert_eq!(outcome.tool_runs.len(), 1);
        assert!(
            outcome.tool_runs[0].allowed,
            "tool was denied: {:?}",
            outcome.tool_runs[0].error
        );
        assert_eq!(backend.stones().len(), 1);
    }

    #[tokio::test]
    async fn agent_cannot_exceed_its_capabilities() {
        // Principal only holds notes.create — billing.charge must be denied,
        // surfaced to the model as a tool error, never retried elevated.
        let (_engine, backend, runtime, stub, principal, tenant) = fixture(Some("notes.create"));
        let context = ctx(principal, tenant);
        stub.push_turn(AiMessage::tool_calls(vec![ToolCall {
            id: "call-1".into(),
            name: "billing__charge".into(),
            arguments: serde_json::json!({}),
        }]));
        stub.push_turn(AiMessage::assistant("cannot charge: no permission"));
        let outcome = runtime.run(context, "charge the customer").await.unwrap();
        assert_eq!(outcome.tool_runs.len(), 1);
        assert!(!outcome.tool_runs[0].allowed);
        assert_eq!(backend.stones().len(), 0, "denied tool writes nothing");
        assert!(outcome.answer.contains("no permission"));
    }

    #[tokio::test]
    async fn unknown_tool_reports_error_without_panicking() {
        let (_engine, _backend, runtime, stub, principal, tenant) = fixture(Some("notes.create"));
        let context = ctx(principal, tenant);
        stub.push_turn(AiMessage::tool_calls(vec![ToolCall {
            id: "call-1".into(),
            name: "does_not_exist".into(),
            arguments: serde_json::json!({}),
        }]));
        stub.push_turn(AiMessage::assistant("tool missing"));
        let outcome = runtime.run(context, "try a ghost tool").await.unwrap();
        assert!(!outcome.tool_runs[0].allowed);
        assert_eq!(outcome.tool_runs[0].error.as_deref(), Some("unknown tool"));
    }

    #[tokio::test]
    async fn runaway_agent_hits_step_budget() {
        let (_engine, _backend, runtime, stub, principal, tenant) = fixture(Some("notes.create"));
        let context = ctx(principal, tenant);
        for i in 0..8 {
            stub.push_turn(AiMessage::tool_calls(vec![ToolCall {
                id: format!("call-{i}"),
                name: "notes__create".into(),
                arguments: serde_json::json!({ "body": i }),
            }]));
        }
        let error = runtime
            .with_max_steps(3)
            .run(context, "loop forever")
            .await
            .unwrap_err();
        assert_eq!(error.code, codes::INTERNAL);
        assert!(error.public_message.contains("exceeded"));
    }
}
