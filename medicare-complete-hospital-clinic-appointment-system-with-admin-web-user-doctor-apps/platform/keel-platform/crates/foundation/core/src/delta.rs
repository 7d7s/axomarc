//! DeltaStone: the state-transition primitive (spec §4/§36). Immutable, append-only,
//! replayable. "Update the audit record" is unexpressable. Delta ≠ Audit ≠ Event ≠ Outbox.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::id::{CorrelationId, DeltaId, ExecutionId, PrincipalId, TenantId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeltaKind {
    /// Module/deployment/feature/tenant-lifecycle changes.
    Platform,
    /// Membership/role/scope/delegation/authorization changes.
    Scope,
    /// Business mutations.
    Data,
}

/// A stream is a monotonic sequence; partitions are addressed as `kind:partition`
/// (e.g. `tenant:{id}`, `module:inventory`, `resource:stock:{sku}`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StreamRef {
    pub kind: DeltaKind,
    pub partition: String,
}

impl StreamRef {
    pub fn data(partition: impl Into<String>) -> Self {
        Self {
            kind: DeltaKind::Data,
            partition: partition.into(),
        }
    }
    pub fn scope(partition: impl Into<String>) -> Self {
        Self {
            kind: DeltaKind::Scope,
            partition: partition.into(),
        }
    }
    pub fn platform(partition: impl Into<String>) -> Self {
        Self {
            kind: DeltaKind::Platform,
            partition: partition.into(),
        }
    }
}

impl std::fmt::Display for StreamRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.kind {
            DeltaKind::Platform => "platform",
            DeltaKind::Scope => "scope",
            DeltaKind::Data => "data",
        };
        write!(f, "{kind}:{}", self.partition)
    }
}

/// Immutable state-transition record. Sequence is assigned by the delta store at
/// write time (monotonic per stream) — never by application code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaStone {
    pub id: DeltaId,
    pub stream: StreamRef,
    pub sequence: Option<u64>,
    pub resource_type: String,
    pub resource_id: Option<Uuid>,
    pub resource_version: i64,
    pub execution_id: ExecutionId,
    pub principal_id: Option<PrincipalId>,
    pub tenant_id: Option<TenantId>,
    /// Frozen scope path strings at execution time (e.g. ["LE1", "LE1|LOC5"]).
    pub scope_snapshot: Vec<String>,
    /// Frozen ownership facts at execution time.
    pub ownership_snapshot: Vec<String>,
    pub before: Option<serde_json::Value>,
    pub command: String,
    pub after: serde_json::Value,
    pub causation_id: Option<CorrelationId>,
    pub correlation_id: Option<CorrelationId>,
    #[serde(with = "time::serde::iso8601")]
    pub at: OffsetDateTime,
}
