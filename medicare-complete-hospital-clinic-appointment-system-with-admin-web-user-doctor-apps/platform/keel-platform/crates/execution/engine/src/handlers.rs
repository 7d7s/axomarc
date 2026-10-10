//! Multi-module handler aggregation (B-21, groundwork for B-24). The
//! lifecycle registry is typed (one slot per concrete type), so module
//! providers cannot each register their own handler bundle under the same
//! type. Instead every module appends its handlers to this shared registry
//! during init; the host assembles the CommandBus from the aggregate once
//! all module providers have registered (init order becomes irrelevant).

use std::sync::{Arc, Mutex};

use crate::engine::OperationHandler;

/// Append-only aggregate of operation handlers. Interior-mutable so module
/// providers can register during init in any (phase, name) order.
#[derive(Default)]
pub struct HandlerRegistry {
    handlers: Mutex<Vec<Arc<dyn OperationHandler>>>,
}

impl HandlerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one handler.
    pub fn register(&self, handler: Arc<dyn OperationHandler>) {
        self.handlers.lock().unwrap().push(handler);
    }

    /// Append a module's full handler bundle.
    pub fn register_all(&self, bundle: Vec<Arc<dyn OperationHandler>>) {
        self.handlers.lock().unwrap().extend(bundle);
    }

    /// Snapshot in registration order (host bus assembly consumes this).
    pub fn handlers(&self) -> Vec<Arc<dyn OperationHandler>> {
        self.handlers.lock().unwrap().clone()
    }

    pub fn len(&self) -> usize {
        self.handlers.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.handlers.lock().unwrap().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::unit::ExecutionUnit;
    use crate::{BoxFuture, ExecutionPlan};
    use kelson_context::ExecutionContext;
    use kelson_foundation::{AppError, ExecutionProfileKind, OperationId};

    struct Stub;

    impl OperationHandler for Stub {
        fn plan(&self) -> &ExecutionPlan {
            static PLAN: std::sync::OnceLock<ExecutionPlan> = std::sync::OnceLock::new();
            PLAN.get_or_init(|| {
                ExecutionPlan::new(
                    OperationId::from_static("stub.op"),
                    ExecutionProfileKind::Read,
                )
            })
        }

        fn handle(
            &self,
            _ctx: Arc<ExecutionContext>,
            _input: serde_json::Value,
            _unit: &mut dyn ExecutionUnit,
        ) -> BoxFuture<'static, Result<serde_json::Value, AppError>> {
            Box::pin(async move { Ok(serde_json::Value::Null) })
        }
    }

    #[test]
    fn registry_aggregates_across_providers() {
        let registry = HandlerRegistry::new();
        assert!(registry.is_empty());
        registry.register(Arc::new(Stub));
        registry.register_all(vec![Arc::new(Stub), Arc::new(Stub)]);
        assert_eq!(registry.len(), 3);
        assert_eq!(registry.handlers().len(), 3);
    }
}
