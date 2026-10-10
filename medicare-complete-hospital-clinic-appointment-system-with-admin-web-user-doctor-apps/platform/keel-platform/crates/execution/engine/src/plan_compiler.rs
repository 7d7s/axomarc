//! Plan compiler (S1): declared intent → fully-resolved `ExecutionPlan`.
//!
//! This is the single construction point for execution semantics (STAGE-0-SPEC
//! §8). REST, CLI, jobs, webhooks, workflows and AI tools all compile through
//! here; the engine consumes plans verbatim and never re-derives semantics at
//! execution time.

use kelson_foundation::{
    AppError, ExecutionProfile, ExecutionProfileKind, OperationId, RetryPolicy, SecurityPlane,
    StreamRef,
};

use crate::plan::{
    AuditPlan, AuthorizationPlan, ConcurrencyPlan, ConsistencyModel, ConsistencyPlan, DeltaPlan,
    ExecutionPlan, IdempotencyPlan, TransactionPlan,
};

/// Declared intent for one operation: what the developer asks for. Everything
/// else (semantics) is resolved deterministically from the execution profile.
#[derive(Debug, Clone)]
pub struct PlanIntent {
    pub operation: OperationId,
    pub kind: ExecutionProfileKind,
    /// Capability required to execute (`module.operation` or module-local name).
    pub capability: Option<String>,
    pub plane: SecurityPlane,
    pub stream: Option<StreamRef>,
    /// Lock-key template for Serialized ops (`{tenant}`/`{operation}` expanded at runtime).
    pub serialized_key: Option<String>,
    /// Operation-level retry override; defaults come from the profile.
    pub retry: Option<RetryPolicy>,
    /// Operation-level timeout override (seconds); defaults come from the profile.
    pub timeout_secs: Option<u64>,
}

impl PlanIntent {
    pub fn new(operation: OperationId, kind: ExecutionProfileKind) -> Self {
        Self {
            operation,
            kind,
            capability: None,
            plane: SecurityPlane::Tenant,
            stream: None,
            serialized_key: None,
            retry: None,
            timeout_secs: None,
        }
    }

    pub fn with_capability(mut self, capability: impl Into<String>) -> Self {
        self.capability = Some(capability.into());
        self
    }
    pub fn with_plane(mut self, plane: SecurityPlane) -> Self {
        self.plane = plane;
        self
    }
    pub fn with_stream(mut self, stream: StreamRef) -> Self {
        self.stream = Some(stream);
        self
    }
    pub fn with_serialized_key(mut self, key: impl Into<String>) -> Self {
        self.serialized_key = Some(key.into());
        self
    }
    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = Some(retry);
        self
    }
    pub fn with_timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_secs = Some(secs);
        self
    }
}

/// Stateless, deterministic compiler: identical intent ⇒ identical plan.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlanCompiler;

impl PlanCompiler {
    /// Compile intent into a fully-resolved plan. All framework semantics are
    /// fixed here from profile resolution — never re-derived at runtime.
    pub fn compile(intent: &PlanIntent) -> ExecutionPlan {
        let profile = ExecutionProfile::resolve(intent.kind);
        let consistency = if profile.is_readonly() {
            ConsistencyModel::Eventual
        } else {
            ConsistencyModel::Strong
        };
        ExecutionPlan {
            operation: intent.operation.clone(),
            plane: intent.plane,
            authorization: AuthorizationPlan {
                capability: intent.capability.clone(),
                required: true,
            },
            transaction: TransactionPlan {
                required: profile.transaction_required,
            },
            concurrency: ConcurrencyPlan {
                strategy: profile.concurrency,
                serialized_key: intent.serialized_key.clone(),
            },
            consistency: ConsistencyPlan { model: consistency },
            idempotency: IdempotencyPlan {
                required: profile.idempotency_required,
            },
            retry: intent.retry.unwrap_or(profile.retry),
            delta: DeltaPlan {
                required: profile.delta_required,
                stream: intent.stream.clone(),
            },
            audit: AuditPlan {
                required: profile.audit_required,
            },
            timeout_secs: intent.timeout_secs.unwrap_or(profile.timeout_secs),
            profile,
        }
    }

    /// Compile + plan self-consistency validation (G06).
    pub fn compile_checked(intent: &PlanIntent) -> Result<ExecutionPlan, AppError> {
        let plan = Self::compile(intent);
        plan.validate()?;
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kelson_foundation::ConcurrencyStrategy;

    fn op() -> OperationId {
        OperationId::from_static("demo.echo")
    }

    #[test]
    fn compile_is_deterministic_and_complete() {
        let intent = PlanIntent::new(op(), ExecutionProfileKind::Financial)
            .with_capability("demo.echo")
            .with_serialized_key("close:{operation}");
        let a = PlanCompiler::compile(&intent);
        let b = PlanCompiler::compile(&intent);
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::to_value(&b).unwrap()
        );
        // Financial profile semantics are fully resolved on the plan.
        assert!(a.transaction.required);
        assert_eq!(a.concurrency.strategy, ConcurrencyStrategy::Serialized);
        assert!(a.idempotency.required && a.delta.required && a.audit.required);
        assert_eq!(a.retry.max_attempts, 0);
        assert_eq!(a.timeout_secs, 30);
        assert_eq!(a.consistency.model, ConsistencyModel::Strong);
    }

    #[test]
    fn read_profile_is_eventual_and_lean() {
        let plan = PlanCompiler::compile(&PlanIntent::new(op(), ExecutionProfileKind::Read));
        assert!(!plan.transaction.required);
        assert!(!plan.delta.required);
        assert_eq!(plan.consistency.model, ConsistencyModel::Eventual);
        plan.validate().expect("read plan validates");
    }

    #[test]
    fn serialized_without_transaction_is_rejected() {
        let mut plan = PlanCompiler::compile(&PlanIntent::new(op(), ExecutionProfileKind::Command));
        plan.concurrency.strategy = ConcurrencyStrategy::Serialized;
        plan.transaction.required = false;
        assert!(plan.validate().is_err());
    }

    #[test]
    fn overrides_win_over_profile_defaults() {
        let intent = PlanIntent::new(op(), ExecutionProfileKind::Command)
            .with_timeout_secs(99)
            .with_retry(RetryPolicy {
                max_attempts: 5,
                retryable_only: true,
            });
        let plan = PlanCompiler::compile(&intent);
        assert_eq!(plan.timeout_secs, 99);
        assert_eq!(plan.retry.max_attempts, 5);
    }
}
