// `Deployment` — the result of a single `sovereign deploy` invocation.
// Has the most important state machine in the product: see
// `docs/architecture.md` §3.1 and `docs/phase-00-mvp.md` F3.

use super::id::{AppId, DeploymentId};
use super::timestamp::Timestamp;
use serde::{Deserialize, Serialize};

/// The deployment lifecycle. Every transition must be allowed by
/// [`DeploymentStatus::can_transition_to`]; the storage layer is the
/// single point of enforcement.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash,
    Serialize, Deserialize, sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
pub enum DeploymentStatus {
    /// Row created; nothing has happened yet.
    Pending,
    /// Image is being built (BuildKit, in V1.5+).
    Building,
    /// Built image is being pushed to the local registry.
    Pushing,
    /// Container is being created and started.
    Starting,
    /// Container is running and the health check passed.
    Healthy,
    /// Deploy failed; `error` column is populated.
    Failed,
    /// Was `Healthy`, then explicitly rolled back to the prior version.
    RolledBack,
}

impl DeploymentStatus {
    /// Returns `true` iff a transition from `self` to `other` is allowed
    /// by the documented state machine.
    pub fn can_transition_to(self, other: Self) -> bool {
        use DeploymentStatus::*;
        matches!(
            (self, other),
            (Pending, Building)
                | (Building, Pushing)
                | (Pushing, Starting)
                | (Starting, Healthy)
                | (Starting, Failed)
                | (_, Failed)
                | (Healthy, RolledBack)
                | (Failed, RolledBack)
                | (RolledBack, Building)
        )
    }

    /// `true` if this is a terminal state (no further transitions allowed).
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Healthy)
    }
}

impl std::fmt::Display for DeploymentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Pending => "pending",
            Self::Building => "building",
            Self::Pushing => "pushing",
            Self::Starting => "starting",
            Self::Healthy => "healthy",
            Self::Failed => "failed",
            Self::RolledBack => "rolled_back",
        };
        f.write_str(s)
    }
}

/// Deploy strategy. `BlueGreen` is the default in V0; rolling is
/// available for stateful apps; `Recreate` drops the old container first.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash,
    Serialize, Deserialize, sqlx::Type,
)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
pub enum Strategy {
    /// Stop the old version, start the new.
    Recreate,
    /// Update one replica at a time. V0 surface only; not implemented yet.
    Rolling,
    /// Start the new version alongside the old, shift traffic on health.
    BlueGreen,
}

impl Strategy {
    pub const ALL: &'static [&'static str] = &["recreate", "rolling", "bluegreen"];

    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Recreate => "recreate",
            Self::Rolling => "rolling",
            Self::BlueGreen => "bluegreen",
        }
    }
}

impl std::str::FromStr for Strategy {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "recreate" => Ok(Self::Recreate),
            "rolling" => Ok(Self::Rolling),
            "bluegreen" => Ok(Self::BlueGreen),
            other => Err(format!(
                "invalid strategy `{other}` (expected one of: recreate, rolling, bluegreen)"
            )),
        }
    }
}

impl std::fmt::Display for Strategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A single row in the `deployment` table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
pub struct Deployment {
    pub id: DeploymentId,
    pub app_id: AppId,
    pub image_ref: String,
    pub strategy: Strategy,
    pub status: DeploymentStatus,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    /// `"user:alice"`, `"system"`, `"web:github"`, …
    pub triggered_by: String,
    /// 0-100, from the ML risk scorer (V2+). `None` in V0.
    pub risk_score: Option<i32>,
    /// JSON `{allow, deny, requires_approval, reason}`. `None` in V0.
    pub policy_decision: Option<serde_json::Value>,
    pub error: Option<String>,
    /// Optimistic-concurrency token.
    pub version: i64,
}

/// Input to [`crate::ports::StoragePort::begin_deployment`].
#[derive(Debug, Clone)]
pub struct NewDeployment {
    pub app_id: AppId,
    pub image_ref: String,
    pub strategy: Strategy,
    pub triggered_by: String,
    pub risk_score: Option<i32>,
}

/// Used by the state-machine transitions in `transition_deployment`.
///
/// The `error` field is required when transitioning to `Failed`.
#[derive(Debug, Clone)]
pub struct DeploymentEvent {
    pub to: DeploymentStatus,
    pub error: Option<String>,
    pub actor: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deployment_state_machine() {
        use DeploymentStatus::*;
        // The happy path.
        assert!(Pending.can_transition_to(Building));
        assert!(Building.can_transition_to(Pushing));
        assert!(Pushing.can_transition_to(Starting));
        assert!(Starting.can_transition_to(Healthy));
        // Starting can fail.
        assert!(Starting.can_transition_to(Failed));
        // Anything can fail.
        assert!(Pending.can_transition_to(Failed));
        assert!(Building.can_transition_to(Failed));
        assert!(Pushing.can_transition_to(Failed));
        // Rollback is allowed from Healthy or Failed.
        assert!(Healthy.can_transition_to(RolledBack));
        assert!(Failed.can_transition_to(RolledBack));
        // A re-deploy from a rolled-back state.
        assert!(RolledBack.can_transition_to(Building));
        // Skipping states is forbidden.
        assert!(!Pending.can_transition_to(Healthy));
        assert!(!Pending.can_transition_to(Starting));
        assert!(!Building.can_transition_to(Healthy));
        // Backwards transitions are forbidden.
        assert!(!Building.can_transition_to(Pending));
        assert!(!Healthy.can_transition_to(Pending));
    }

    #[test]
    fn strategy_str_roundtrip() {
        for s in [Strategy::Recreate, Strategy::Rolling, Strategy::BlueGreen] {
            let back: Strategy = s.to_string().parse().unwrap();
            assert_eq!(s, back);
        }
    }
}
