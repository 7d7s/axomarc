//! Kernel error codes — stable contract (04-API-CONTRACTS §1, SRS error grammar).
use thiserror::Error;

pub const ERROR_DOC_BASE: &str = "https://docs.medicare.local/errors";

#[derive(Debug, Error)]
pub enum KernelError {
    #[error("invalid or expired token")]
    AuthInvalidToken,
    #[error("tenant suspended")]
    TenantSuspended,
    #[error("module disabled for this tenant")]
    ModuleDisabled,
    #[error("capability denied: {action}")]
    CapabilityDenied { action: String },
    #[error("validation failed: {0}")]
    ValidationFailed(String),
    #[error("invalid workflow transition")]
    WorkflowInvalidTransition,
    #[error("version conflict")]
    ConflictVersion,
    #[error("slot already taken")]
    ConflictSlotTaken,
    #[error("idempotent replay")]
    IdempotencyReplay,
    #[error("plan limit reached: {metric}")]
    PlanLimit { metric: String },
    #[error("not found")]
    NotFound,
    #[error("internal error")]
    Internal,
}

impl KernelError {
    /// Stable machine-readable code (`stage.subject` grammar).
    pub fn code(&self) -> &'static str {
        match self {
            KernelError::AuthInvalidToken => "auth.invalid_token",
            KernelError::TenantSuspended => "tenant.suspended",
            KernelError::ModuleDisabled => "module.disabled",
            KernelError::CapabilityDenied { .. } => "capability.denied",
            KernelError::ValidationFailed(_) => "validation.failed",
            KernelError::WorkflowInvalidTransition => "workflow.invalid_transition",
            KernelError::ConflictVersion => "conflict.version",
            KernelError::ConflictSlotTaken => "conflict.slot_taken",
            KernelError::IdempotencyReplay => "idempotency.replay",
            KernelError::PlanLimit { .. } => "plan.limit",
            KernelError::NotFound => "not_found",
            KernelError::Internal => "internal",
        }
    }

    pub fn http_status(&self) -> u16 {
        match self {
            KernelError::AuthInvalidToken => 401,
            KernelError::TenantSuspended | KernelError::ModuleDisabled => 403,
            KernelError::CapabilityDenied { .. } => 403,
            KernelError::ValidationFailed(_) => 422,
            KernelError::WorkflowInvalidTransition => 409,
            KernelError::ConflictVersion | KernelError::ConflictSlotTaken => 409,
            KernelError::IdempotencyReplay => 200,
            KernelError::PlanLimit { .. } => 429,
            KernelError::NotFound => 404,
            KernelError::Internal => 500,
        }
    }

    pub fn doc_url(&self) -> String {
        format!("{}/{}", ERROR_DOC_BASE, self.code())
    }
}