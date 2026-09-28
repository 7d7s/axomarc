//! Execution profiles (spec §9): reusable semantics packages. Developers declare
//! a profile; engineering policy (tx/locking/retry/idempotency/delta/audit) is
//! encoded once here, not re-implemented per feature.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExecutionProfileKind {
    Read,
    Command,
    Financial,
    Inventory,
    Workflow,
    Batch,
    Report,
    System,
    Replay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConcurrencyStrategy {
    /// Version-column CAS (`WHERE id=$1 AND version=$2`).
    Optimistic,
    /// Row locks (`SELECT ... FOR UPDATE`).
    Pessimistic,
    /// Centralized sequencing for global operations (e.g. financial close).
    Serialized,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    /// Only errors marked retryable may be retried (never blind retry for financial ops).
    pub retryable_only: bool,
}

impl Default for ExecutionProfileKind {
    /// Safest default: read-only, no transaction, no delta.
    fn default() -> Self {
        ExecutionProfileKind::Read
    }
}
/// Resolved execution semantics for one operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionProfile {
    pub kind: ExecutionProfileKind,
    pub transaction_required: bool,
    pub concurrency: ConcurrencyStrategy,
    pub idempotency_required: bool,
    pub delta_required: bool,
    pub audit_required: bool,
    pub timeout_secs: u64,
    pub retry: RetryPolicy,
}

impl ExecutionProfile {
    /// Deterministic resolution — the kernel decides semantics from declared intent.
    pub fn resolve(kind: ExecutionProfileKind) -> Self {
        use ConcurrencyStrategy::*;
        use ExecutionProfileKind::*;
        let retry = |max_attempts: u32, retryable_only: bool| RetryPolicy {
            max_attempts,
            retryable_only,
        };
        match kind {
            Read => Self {
                kind,
                transaction_required: false,
                concurrency: Optimistic,
                idempotency_required: false,
                delta_required: false,
                audit_required: false,
                timeout_secs: 5,
                retry: retry(2, true),
            },
            Report => Self {
                kind,
                transaction_required: false,
                concurrency: Optimistic,
                idempotency_required: false,
                delta_required: false,
                audit_required: false,
                timeout_secs: 60,
                retry: retry(1, true),
            },
            Command => Self {
                kind,
                transaction_required: true,
                concurrency: Optimistic,
                idempotency_required: false,
                delta_required: true,
                audit_required: true,
                timeout_secs: 15,
                retry: retry(1, true),
            },
            Workflow => Self {
                kind,
                transaction_required: true,
                concurrency: Optimistic,
                idempotency_required: true,
                delta_required: true,
                audit_required: true,
                timeout_secs: 30,
                retry: retry(3, true),
            },
            Inventory => Self {
                kind,
                transaction_required: true,
                concurrency: Pessimistic,
                idempotency_required: true,
                delta_required: true,
                audit_required: true,
                timeout_secs: 15,
                retry: retry(2, true),
            },
            Financial => Self {
                kind,
                transaction_required: true,
                concurrency: Serialized,
                idempotency_required: true,
                delta_required: true,
                audit_required: true,
                timeout_secs: 30,
                retry: retry(0, true), // no blind retry; replay only
            },
            Batch => Self {
                kind,
                transaction_required: true,
                concurrency: Pessimistic,
                idempotency_required: true,
                delta_required: true,
                audit_required: true,
                timeout_secs: 300,
                retry: retry(2, true),
            },
            System => Self {
                kind,
                transaction_required: true,
                concurrency: Optimistic,
                idempotency_required: true,
                delta_required: true,
                audit_required: true,
                timeout_secs: 60,
                retry: retry(2, true),
            },
            Replay => Self {
                kind,
                transaction_required: false, // replay targets: DRY_RUN/SHADOW unless LIVE with lease
                concurrency: Optimistic,
                idempotency_required: true,
                delta_required: false,
                audit_required: true,
                timeout_secs: 60,
                retry: retry(0, true),
            },
        }
    }

    pub fn is_readonly(&self) -> bool {
        matches!(
            self.kind,
            ExecutionProfileKind::Read | ExecutionProfileKind::Report
        )
    }
}
