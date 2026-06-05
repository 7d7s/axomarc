// F2: Exit codes for the sovereign binary.
// Per docs/phase-00-mvp.md §F2 sub-task 8.

use std::process::ExitCode;

/// The contract for sovereign's exit codes. Stable across all V0+ versions;
/// scripts and CI pipelines can rely on these values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
// `Partial` (batch deploys) and `Doctor` (F9) are reserved for V0.5+; the
// variants are part of the public contract that scripts and CI rely on, so
// we keep them even when no caller in V0.1 produces them.
#[allow(dead_code)]
pub enum AppExit {
    /// Command succeeded.
    Success = 0,
    /// Generic / unspecified error.
    Generic = 1,
    /// Usage error (bad flags, missing args, etc.).
    Usage = 2,
    /// Partial success (e.g. one of N apps failed to deploy).
    Partial = 3,
    /// An upstream service (Docker daemon, Caddy admin API, OIDC provider,
    /// log ship sink) returned an error.
    Upstream = 4,
    /// The doctor found fail-severity issues. Reserved for `sovereign doctor`.
    Doctor = 5,
}

impl AppExit {
    /// Convert to `std::process::ExitCode`.
    pub fn into_process(self) -> ExitCode {
        std::process::ExitCode::from(self as u8)
    }
}

impl std::fmt::Display for AppExit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", *self as u8)
    }
}
