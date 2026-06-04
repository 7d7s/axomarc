// `App` — the central domain entity. Per docs/architecture.md §2.1
// the table is called `app` (singular), and the row carries an
// `env`, `git_repo`, `image_ref`, `config_yaml`, `health_path`,
// and a `version` column for optimistic concurrency.

use super::id::AppId;
use super::timestamp::Timestamp;
use serde::{Deserialize, Serialize};

/// The environment an [`App`] is deployed into. Encoded in the
/// `app.env` column with a `CHECK (env IN ('dev','staging','prod'))`
/// constraint, so the SQL and Rust representations must stay in sync.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash,
    Serialize, Deserialize, sqlx::Type,
)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
pub enum AppEnv {
    /// Local development. Auto-deploy on push, no SLA.
    Dev,
    /// Pre-production. Smoke tests required.
    Staging,
    /// Production. SLA, on-call, full audit.
    Prod,
}

impl AppEnv {
    /// The 3 string values accepted by the SQL `CHECK` constraint.
    pub const ALL: &'static [&'static str] = &["dev", "staging", "prod"];

    /// Lower-case string form, suitable for the `env` SQL column.
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Staging => "staging",
            Self::Prod => "prod",
        }
    }
}

impl std::str::FromStr for AppEnv {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "dev" => Ok(Self::Dev),
            "staging" => Ok(Self::Staging),
            "prod" => Ok(Self::Prod),
            other => Err(format!(
                "invalid env `{other}` (expected one of: dev, staging, prod)"
            )),
        }
    }
}

impl std::fmt::Display for AppEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Lifecycle status of an [`App`]. See `docs/architecture.md` §3.2.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash,
    Serialize, Deserialize, sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
pub enum AppStatus {
    /// Accepting deploys and serving traffic.
    Active,
    /// No new deploys accepted; still serving traffic from the last version.
    Draining,
    /// Removed from the active set; row kept for audit.
    Archived,
}

impl AppStatus {
    /// Returns true iff a transition from `self` to `other` is allowed by
    /// the documented state machine.
    pub fn can_transition_to(self, other: Self) -> bool {
        use AppStatus::*;
        matches!(
            (self, other),
            (Active, Draining) | (Active, Archived) | (Draining, Archived)
        )
    }
}

impl std::fmt::Display for AppStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Active => "active",
            Self::Draining => "draining",
            Self::Archived => "archived",
        };
        f.write_str(s)
    }
}

/// A single row in the `app` table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
pub struct App {
    /// Primary key.
    pub id: AppId,
    /// Globally-unique, human-readable. `UNIQUE` in SQL.
    pub name: String,
    /// Owning principal. `"user:<id>"` for human owners, `"team:<name>"`,
    /// or `"system"`. Not enforced by FK in V0 (no `user` FK).
    pub owner: String,
    /// Target environment.
    pub env: AppEnv,
    /// Optional Git source. `None` if the app is image-only.
    pub git_repo: Option<String>,
    /// Last successfully deployed image reference (e.g. `ghcr.io/me/api:v3`).
    pub image_ref: Option<String>,
    /// The full `app.yaml` at deploy time. Kept verbatim for re-deploy.
    pub config_yaml: String,
    /// HTTP path to use for the post-deploy health probe.
    pub health_path: Option<String>,
    /// Row creation time.
    pub created_at: Timestamp,
    /// Last update time.
    pub updated_at: Timestamp,
    /// Optimistic-concurrency token; every `UPDATE` increments it.
    pub version: i64,
    /// Soft-delete flag: `true` once the app is [`AppStatus::Archived`].
    pub status: AppStatus,
}

/// Input to [`crate::ports::StoragePort::create_app`].
#[derive(Debug, Clone)]
pub struct NewApp {
    pub name: String,
    pub owner: String,
    pub env: AppEnv,
    pub git_repo: Option<String>,
    pub image_ref: Option<String>,
    pub config_yaml: String,
    pub health_path: Option<String>,
}

/// Patch to apply via [`crate::ports::StoragePort::update_app`]. Every
/// field is optional; `None` means "leave unchanged." The `expected_version`
/// is the OCC token — see `docs/architecture.md` §2.4.
#[derive(Debug, Clone, Default)]
pub struct AppUpdate {
    pub owner: Option<String>,
    pub env: Option<AppEnv>,
    pub git_repo: Option<Option<String>>,
    pub image_ref: Option<Option<String>>,
    pub config_yaml: Option<String>,
    pub health_path: Option<Option<String>>,
    pub status: Option<AppStatus>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_str_roundtrip() {
        for e in [AppEnv::Dev, AppEnv::Staging, AppEnv::Prod] {
            let s = e.to_string();
            let back: AppEnv = s.parse().unwrap();
            assert_eq!(e, back);
        }
    }

    #[test]
    fn app_status_transitions() {
        use AppStatus::*;
        assert!(Active.can_transition_to(Draining));
        assert!(Active.can_transition_to(Archived));
        assert!(Draining.can_transition_to(Archived));
        // No backwards transitions.
        assert!(!Draining.can_transition_to(Active));
        assert!(!Archived.can_transition_to(Active));
        assert!(!Archived.can_transition_to(Draining));
        // No self-loops.
        assert!(!Active.can_transition_to(Active));
    }
}
