// `Backup` — a snapshot of the SQLite state (or a specific database)
// plus its verification status. The "verified" status is the *load-
// bearing* state: a backup you have not restored is a hope. See
// `docs/architecture.md` §3.2 and §6.

use super::id::BackupId;
use super::timestamp::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash,
    Serialize, Deserialize, sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
pub enum BackupStatus {
    /// Snapshot in progress.
    Pending,
    /// Snapshot completed; not yet restored-drilled.
    Success,
    /// Snapshot failed; see `error`.
    Failed,
    /// Snapshot completed AND a restore-drill to scratch passed.
    Verified,
}

impl BackupStatus {
    pub fn can_transition_to(self, other: Self) -> bool {
        use BackupStatus::*;
        matches!(
            (self, other),
            (Pending, Success) | (Pending, Failed) | (Success, Verified)
        )
    }
}

impl std::fmt::Display for BackupStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pending => "pending",
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Verified => "verified",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
pub struct Backup {
    pub id: BackupId,
    /// `"sqlite"` for the state DB; `"postgres:<app_id>"` for app DBs.
    pub target: String,
    pub status: BackupStatus,
    pub size_bytes: Option<i64>,
    pub location: Option<String>,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    pub verified_at: Option<Timestamp>,
    pub verify_result: Option<serde_json::Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewBackup {
    pub target: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_state_machine() {
        use BackupStatus::*;
        assert!(Pending.can_transition_to(Success));
        assert!(Pending.can_transition_to(Failed));
        assert!(Success.can_transition_to(Verified));
        // No going back.
        assert!(!Success.can_transition_to(Failed));
        assert!(!Verified.can_transition_to(Success));
    }
}
