// `Server` — a box in the fleet. V0 only has the control plane; the
// `agent` role lights up in V1.5. See `docs/architecture.md` §3.2.

use super::id::ServerId;
use super::timestamp::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
pub enum ServerRole {
    /// The single V0 control plane.
    ControlPlane,
    /// A worker box that runs workloads (V1.5+).
    Agent,
}

impl std::fmt::Display for ServerRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::ControlPlane => "control_plane",
            Self::Agent => "agent",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
pub enum ServerStatus {
    Healthy,
    Unreachable,
    Drained,
    /// Terminal. The row is kept for audit but the server is gone.
    Removed,
}

impl ServerStatus {
    pub fn can_transition_to(self, other: Self) -> bool {
        use ServerStatus::*;
        matches!(
            (self, other),
            (Healthy, Unreachable)
                | (Healthy, Drained)
                | (Unreachable, Healthy)
                | (Unreachable, Drained)
                | (Drained, Healthy)
                | (Drained, Removed)
                | (Unreachable, Removed)
        )
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Removed)
    }
}

impl std::fmt::Display for ServerStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Healthy => "healthy",
            Self::Unreachable => "unreachable",
            Self::Drained => "drained",
            Self::Removed => "removed",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
pub struct Server {
    pub id: ServerId,
    pub hostname: String,
    pub role: ServerRole,
    pub api_url: String,
    pub status: ServerStatus,
    pub last_seen: Timestamp,
    pub cpu_cores: Option<i32>,
    pub mem_mb: Option<i32>,
    pub disk_gb: Option<i32>,
    pub joined_at: Timestamp,
}

#[derive(Debug, Clone)]
pub struct NewServer {
    pub hostname: String,
    pub role: ServerRole,
    pub api_url: String,
    pub cpu_cores: Option<i32>,
    pub mem_mb: Option<i32>,
    pub disk_gb: Option<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_state_machine() {
        use ServerStatus::*;
        assert!(Healthy.can_transition_to(Unreachable));
        assert!(Healthy.can_transition_to(Drained));
        assert!(Unreachable.can_transition_to(Healthy));
        assert!(Drained.can_transition_to(Removed));
        assert!(!Removed.can_transition_to(Healthy));
        assert!(Removed.is_terminal());
    }
}
