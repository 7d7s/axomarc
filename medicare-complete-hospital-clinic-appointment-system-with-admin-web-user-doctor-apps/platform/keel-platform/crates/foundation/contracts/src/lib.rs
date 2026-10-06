//! # keel-contracts
//! The public contract traits (11-SPEC §10). This is the surface modules implement;
//! everything else stays crate-private until proven.

pub mod api_registry;

use keel_context::ExecutionContext;
use keel_foundation::{DeltaKind, ExecutionProfileKind};

/// A write intention. No HTTP concepts inside — commands are transport-agnostic.
/// `OPERATION` is the stable contract id (`module.operation`).
pub trait Command: Send + Sync + 'static {
    type Output: Send;
    const OPERATION: &'static str;
    const PROFILE: ExecutionProfileKind;
    /// Required capability (module-local name). None for authenticated-only ops.
    const CAPABILITY: Option<&'static str> = None;
}

/// A read intention. Prefer projections over aggregate hydration where cheaper.
pub trait Query: Send + Sync + 'static {
    type Output: Send;
    const OPERATION: &'static str;
}

/// A business fact. Emitted only post-commit via the outbox (D-53).
pub trait DomainEvent: Send + Sync + 'static {
    const NAME: &'static str;
    const STREAM: DeltaKind = DeltaKind::Data;
}

/// A durable execution (not "a queue job"). Retries/deadlines are execution-level.
pub trait Job: Send + Sync + 'static {
    const NAME: &'static str;
    const MAX_ATTEMPTS: u32 = 3;
}

/// Response representation (MVVM View → wire). Decouples domain from HTTP shape.
pub trait Resource: Sized + Send + 'static {
    type Source;
    fn from_source(source: &Self::Source) -> Self;
}

/// Outcome of one authorization decision. Security (L1) owns the decision
/// boundary (STAGE-0-SPEC §3); the execution kernel only consumes it.
#[derive(Debug, Clone)]
pub struct AuthzDecision {
    pub allowed: bool,
    pub reason: Option<String>,
    /// Which rule produced the decision (diagnostic chain).
    pub via: &'static str,
}

impl AuthzDecision {
    pub fn allow(via: &'static str) -> Self {
        Self {
            allowed: true,
            reason: None,
            via,
        }
    }
    pub fn deny(via: &'static str, reason: impl Into<String>) -> Self {
        Self {
            allowed: false,
            reason: Some(reason.into()),
            via,
        }
    }
}

/// Authorization-relevant projection of an `ExecutionPlan`. Lives here (not in
/// the engine) so the security layer can implement the port without depending
/// on the execution layer.
#[derive(Debug, Clone, Default)]
pub struct AuthzRequest<'a> {
    /// Required capability (`module.operation`). None = authenticated-only op.
    pub capability: Option<&'a str>,
}

/// Authorization boundary of the execution engine. Real implementation =
/// keel-policy ALLOW conjunction; dev/test may inject allow-all (never in prod
/// config).
pub trait AuthorizationPort: Send + Sync {
    fn authorize(&self, ctx: &ExecutionContext, request: &AuthzRequest<'_>) -> AuthzDecision;
}
