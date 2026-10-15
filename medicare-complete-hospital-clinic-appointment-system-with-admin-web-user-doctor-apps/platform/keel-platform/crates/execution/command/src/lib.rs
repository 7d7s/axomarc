//! # kelson-command
//! The command bus: handler registry + dispatch into the execution engine.
//! Authorization, idempotency, transaction, delta, audit all happen in the engine —
//! handlers stay small and business-pure.

use std::collections::HashMap;
use std::sync::Arc;

pub mod typed;
pub use typed::{Json, TypedHandleFn, TypedHandler, TypedInput, TypedOutput};

use kelson_context::ExecutionContext;
use kelson_foundation::{AppError, OperationId};
use kelson_execution::{Engine, IdempotencyKey, OperationHandler, OperationOutcome};
use serde_json::Value;

pub struct CommandBus {
    engine: Arc<Engine>,
    handlers: HashMap<OperationId, Arc<dyn OperationHandler>>,
}

impl CommandBus {
    pub fn new(engine: Arc<Engine>) -> Self {
        Self {
            engine,
            handlers: HashMap::new(),
        }
    }

    pub fn register(&mut self, handler: Arc<dyn OperationHandler>) -> Result<(), AppError> {
        let op = handler.plan().operation.clone();
        if self.handlers.insert(op.clone(), handler).is_some() {
            return Err(AppError::internal(format!(
                "command handler registered twice: {op}"
            )));
        }
        Ok(())
    }

    pub fn operations(&self) -> Vec<OperationId> {
        let mut ops: Vec<OperationId> = self.handlers.keys().cloned().collect();
        ops.sort();
        ops
    }

    pub fn handler(&self, operation: &OperationId) -> Option<Arc<dyn OperationHandler>> {
        self.handlers.get(operation).cloned()
    }

    pub async fn dispatch(
        &self,
        ctx: Arc<ExecutionContext>,
        operation: &OperationId,
        input: Value,
        key: Option<IdempotencyKey>,
    ) -> Result<OperationOutcome, AppError> {
        let handler = self
            .handlers
            .get(operation)
            .ok_or_else(|| AppError::not_found(format!("operation {operation}")))?;
        self.engine.execute(ctx, handler.as_ref(), input, key).await
    }

    /// Run a caller-supplied handler through this bus's execution pipeline.
    ///
    /// A live module activation swaps *which* handler serves an operation, but
    /// never *how* it executes: authorization, idempotency, transaction, delta
    /// and audit all still run in the engine, through exactly the path a
    /// boot-time handler takes. The handler registry itself stays boot-frozen
    /// and never runtime-populated — the caller resolves the current
    /// generation and hands the pinned `Arc` in here.
    pub async fn dispatch_handler(
        &self,
        handler: &Arc<dyn OperationHandler>,
        ctx: Arc<ExecutionContext>,
        input: Value,
        key: Option<IdempotencyKey>,
    ) -> Result<OperationOutcome, AppError> {
        self.engine.execute(ctx, handler.as_ref(), input, key).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kelson_execution::{
        AuthorizationPort, AuthzDecision, AuthzRequest, BoxFuture, ExecutionPlan, ExecutionUnit,
        InMemoryBackend, InMemoryIdempotency, MutationRecord,
    };

    struct AllowAll;
    impl AuthorizationPort for AllowAll {
        fn authorize(&self, _ctx: &ExecutionContext, _request: &AuthzRequest) -> AuthzDecision {
            AuthzDecision::allow("test")
        }
    }

    struct H {
        plan: ExecutionPlan,
    }
    impl OperationHandler for H {
        fn plan(&self) -> &ExecutionPlan {
            &self.plan
        }
        fn handle(
            &self,
            _ctx: Arc<ExecutionContext>,
            _input: Value,
            unit: &mut dyn ExecutionUnit,
        ) -> BoxFuture<'static, Result<Value, AppError>> {
            unit.record_mutation(MutationRecord {
                resource_type: "x".into(),
                resource_id: None,
                resource_version: 1,
                before: None,
                after: Value::Null,
                stream: None,
            });
            Box::pin(async move { Ok(Value::Null) })
        }
    }

    fn handler() -> Arc<dyn OperationHandler> {
        Arc::new(H {
            plan: ExecutionPlan::new(
                OperationId::from_static("demo.op"),
                kelson_foundation::ExecutionProfileKind::Command,
            ),
        })
    }

    #[test]
    fn duplicate_registration_rejected() {
        let engine = Arc::new(Engine::new(
            Arc::new(AllowAll),
            Arc::new(InMemoryBackend::new()),
            Arc::new(InMemoryIdempotency::new()),
        ));
        let mut bus = CommandBus::new(engine);
        bus.register(handler()).unwrap();
        assert!(bus.register(handler()).is_err());
        assert_eq!(bus.operations().len(), 1);
        assert!(bus.handler(&OperationId::from_static("demo.op")).is_some());
    }
}
