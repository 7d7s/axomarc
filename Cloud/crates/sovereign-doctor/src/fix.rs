use async_trait::async_trait;
use thiserror::Error;

/// The outcome of running a `DoctorFix`. `Skipped` is used when the
/// fix is a no-op (the precondition no longer holds, so there is
/// nothing to repair). `Fixed` is success. `Failed` means the fix
/// ran but the underlying problem persists; the operator should
/// read the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FixOutcome {
    Fixed { message: String },
    Skipped { reason: String },
}

#[derive(Debug, Error)]
pub enum FixError {
    #[error("fix failed: {0}")]
    Failed(String),
}

/// A safe, automatable remediation. The fix MUST be idempotent and
/// MUST be non-destructive unless `is_destructive()` returns `true`;
/// the `sovereign doctor --fix` driver uses that flag to decide
/// whether to show a 5s confirmation prompt.
#[async_trait]
pub trait DoctorFix: Send + Sync {
    fn id(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn is_destructive(&self) -> bool;
    async fn apply(&self, ctx: &crate::check::CheckContext) -> Result<FixOutcome, FixError>;
}
