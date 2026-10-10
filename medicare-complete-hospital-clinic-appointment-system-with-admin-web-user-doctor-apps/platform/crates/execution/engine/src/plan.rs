//! ExecutionPlan: developers declare intent; the plan compiler resolves semantics
//! (S1). The engine consumes compiled plans verbatim and never re-derives them.

use kelson_foundation::{
    AppError, ConcurrencyStrategy, ExecutionProfile, ExecutionProfileKind, OperationId,
    RetryPolicy, SecurityPlane, StreamRef,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizationPlan {
    /// Capability required, e.g. `appointment.book`. None = authenticated-only ops.
    pub capability: Option<String>,
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionPlan {
    pub required: bool,
}

/// Concurrency semantics resolved by the plan compiler (S1). Optimistic = CAS at
/// the data layer; Pessimistic = row locks at the data layer; Serialized =
/// centralized sequencing acquired by the engine before the unit runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConcurrencyPlan {
    pub strategy: ConcurrencyStrategy,
    /// Lock-key template for Serialized strategy. `{tenant}` / `{operation}` are
    /// expanded by the runtime before acquisition.
    pub serialized_key: Option<String>,
}

/// Consistency guarantee for this execution (S1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsistencyModel {
    /// One transaction; reads observe own writes (kernel default for mutations).
    Strong,
    /// Committed-state reads only; no write-ordering guarantees (read profiles).
    Eventual,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ConsistencyPlan {
    pub model: ConsistencyModel,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdempotencyPlan {
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaPlan {
    pub required: bool,
    /// Default stream for mutations of this operation (per-resource overrides allowed).
    pub stream: Option<StreamRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditPlan {
    pub required: bool,
}

/// Typed, validated execution semantics for one operation (11-SPEC §2).
/// Produced by the plan compiler; the engine consumes it verbatim (STAGE-0-SPEC §8).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionPlan {
    pub operation: OperationId,
    pub plane: SecurityPlane,
    pub profile: ExecutionProfile,
    pub authorization: AuthorizationPlan,
    pub transaction: TransactionPlan,
    pub concurrency: ConcurrencyPlan,
    pub consistency: ConsistencyPlan,
    pub idempotency: IdempotencyPlan,
    pub retry: RetryPolicy,
    pub delta: DeltaPlan,
    pub audit: AuditPlan,
    /// Operation-level budget (seconds). Backends derive statement timeouts from it.
    pub timeout_secs: u64,
}

impl ExecutionPlan {
    /// Compatibility constructor: kernel-default compilation of the intent.
    pub fn new(operation: OperationId, kind: ExecutionProfileKind) -> Self {
        crate::plan_compiler::PlanCompiler::compile(&crate::plan_compiler::PlanIntent::new(
            operation, kind,
        ))
    }

    pub fn with_capability(mut self, capability: impl Into<String>) -> Self {
        self.authorization.capability = Some(capability.into());
        self
    }

    pub fn with_plane(mut self, plane: SecurityPlane) -> Self {
        self.plane = plane;
        self
    }

    pub fn with_stream(mut self, stream: StreamRef) -> Self {
        self.delta.stream = Some(stream);
        self
    }

    pub fn with_timeout_secs(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }

    pub fn with_retry(mut self, retry: RetryPolicy) -> Self {
        self.retry = retry;
        self
    }

    pub fn with_serialized_key(mut self, key: impl Into<String>) -> Self {
        self.concurrency.serialized_key = Some(key.into());
        self
    }

    /// Plan self-consistency (G06).
    pub fn validate(&self) -> Result<(), AppError> {
        if self.profile.delta_required && !self.delta.required {
            return Err(AppError::internal(format!(
                "plan {}: profile {:?} requires delta but delta plan disabled",
                self.operation, self.profile.kind
            )));
        }
        if self.profile.transaction_required && !self.transaction.required {
            return Err(AppError::internal(format!(
                "plan {}: profile {:?} requires a transaction",
                self.operation, self.profile.kind
            )));
        }
        if self.profile.is_readonly() && self.delta.required {
            return Err(AppError::internal(format!(
                "plan {}: read profiles cannot declare delta",
                self.operation
            )));
        }
        if self.timeout_secs == 0 {
            return Err(AppError::internal(format!(
                "plan {}: timeout must be positive",
                self.operation
            )));
        }
        if self.concurrency.strategy == kelson_foundation::ConcurrencyStrategy::Serialized
            && !self.transaction.required
        {
            return Err(AppError::internal(format!(
                "plan {}: serialized concurrency requires a transaction",
                self.operation
            )));
        }
        Ok(())
    }
}
