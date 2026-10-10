//! Work units + mutation/audit/outbox records + backend port.
//! Design: the backend keeps ownership of its concrete unit for the whole
//! `run()` scope — no cross-boundary downcasts, no lifetime erasure problems.

use std::future::Future;
use std::pin::Pin;

use kelson_foundation::{AppError, StreamRef, TenantId};
use serde_json::Value;
use uuid::Uuid;

use crate::plan::ExecutionPlan;
use kelson_context::ExecutionContext;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One state transition recorded by a handler. Becomes a DeltaStone at commit.
#[derive(Debug, Clone)]
pub struct MutationRecord {
    pub resource_type: String,
    pub resource_id: Option<Uuid>,
    pub resource_version: i64,
    pub before: Option<Value>,
    pub after: Value,
    /// Stream override; falls back to `plan.delta.stream`.
    pub stream: Option<StreamRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    Success,
    Denied,
    Failed,
}

/// Accountability record (who/what/when/where/authority/result). Separate from delta.
#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub action: String,
    pub resource: String,
    pub authority: String,
    pub outcome: AuditOutcome,
    pub detail: Value,
}

/// Event intent, committed atomically with state, dispatched only after commit (D-53).
#[derive(Debug, Clone)]
pub struct OutboxMessage {
    pub event: String,
    pub topic: String,
    pub payload: Value,
}

/// Transactional work surface handed to handlers (backend-owned).
pub trait ExecutionUnit: Send {
    fn tenant_id(&self) -> Option<TenantId>;
    fn record_mutation(&mut self, mutation: MutationRecord);
    fn record_audit(&mut self, audit: AuditRecord);
    fn queue_outbox(&mut self, message: OutboxMessage);
    fn mutation_count(&self) -> usize;
}

/// The business closure the engine passes to the backend.
/// Contract: mutations are recorded synchronously on the unit; the returned
/// future is 'static (it may not borrow the unit), so commit sees the full
/// record set the moment the future completes.
pub type HandlerFn<'a> = Box<
    dyn FnOnce(&mut dyn ExecutionUnit) -> BoxFuture<'static, Result<Value, AppError>> + Send + 'a,
>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommitReceipt {
    pub deltas_written: usize,
    pub audit_written: usize,
    pub outbox_queued: usize,
    pub replayed: bool,
}

impl CommitReceipt {
    pub fn replayed() -> Self {
        Self {
            replayed: true,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub result: Value,
    pub receipt: CommitReceipt,
}

/// KER-03 lineage guards (shared by all backends):
/// - a declared mutation must actually transition state
/// - a read profile may not record transitions
pub fn guard(plan: &ExecutionPlan, mutation_count: usize) -> Result<(), AppError> {
    if plan.delta.required && mutation_count == 0 {
        return Err(AppError::new(
            kelson_foundation::codes::NO_EFFECT,
            kelson_foundation::ErrorCategory::Domain,
            kelson_foundation::Severity::Medium,
            kelson_foundation::Retryability::NotRetryable,
            format!(
                "operation {} declared mutations but emitted none",
                plan.operation
            ),
        ));
    }
    if plan.profile.is_readonly() && mutation_count > 0 {
        return Err(AppError::new(
            kelson_foundation::codes::READ_MUTATED,
            kelson_foundation::ErrorCategory::Domain,
            kelson_foundation::Severity::High,
            kelson_foundation::Retryability::NotRetryable,
            format!(
                "read operation {} attempted a state transition",
                plan.operation
            ),
        ));
    }
    Ok(())
}

/// Backends implement transactional semantics. `run` owns the whole lifecycle:
/// begin → security context → handler → guards → delta/audit/outbox → commit.
/// Dropping the unit without commit (handler error) = rollback.
pub trait ExecutionBackend: Send + Sync {
    fn run<'a>(
        &'a self,
        ctx: &'a ExecutionContext,
        plan: &'a ExecutionPlan,
        handler: HandlerFn<'a>,
    ) -> BoxFuture<'a, Result<RunOutcome, AppError>>;
}
