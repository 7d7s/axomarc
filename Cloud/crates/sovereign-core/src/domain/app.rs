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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, Default)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
pub enum AppEnv {
    /// Local development. Auto-deploy on push, no SLA.
    #[default]
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

/// Deploy mode for an [`App`]. Determines how the app is deployed
/// when a webhook or `sovereign deploy` is triggered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, Default)]
#[serde(rename_all = "lowercase")]
#[sqlx(rename_all = "lowercase")]
pub enum DeployMode {
    /// CI builds the image, webhook tells sovereign to pull it.
    #[default]
    Pull,
    /// Sovereign clones the repo and builds the image locally.
    Build,
    /// CI packs a .sov archive, sovereign unpacks and deploys.
    Pack,
    /// Native binary managed by systemd. No container runtime required.
    Native,
}

impl DeployMode {
    /// The string values accepted by the SQL `CHECK` constraint.
    pub const ALL: &'static [&'static str] = &["pull", "build", "pack", "native"];

    /// Lower-case string form.
    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pull => "pull",
            Self::Build => "build",
            Self::Pack => "pack",
            Self::Native => "native",
        }
    }
}

impl std::str::FromStr for DeployMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "pull" => Ok(Self::Pull),
            "build" => Ok(Self::Build),
            "pack" => Ok(Self::Pack),
            "native" => Ok(Self::Native),
            other => Err(format!(
                "invalid deploy_mode `{other}` (expected one of: pull, build, pack, native)"
            )),
        }
    }
}

impl std::fmt::Display for DeployMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Source configuration for an [`App`]. Controls how the app's code
/// is fetched and deployed.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceConfig {
    /// Git repository URL (SSH or HTTPS).
    pub repo: Option<String>,
    /// Branch or tag to deploy. Defaults to "main".
    pub branch: Option<String>,
    /// Deploy mode (pull, build, pack). Defaults to Pull.
    #[serde(default)]
    pub deploy_mode: DeployMode,
    /// Key in the secrets store for the webhook HMAC secret.
    pub webhook_secret_ref: Option<String>,
    /// Whether to auto-deploy on webhook push.
    #[serde(default)]
    pub auto_deploy: bool,
    /// Time window for auto-deploys (e.g. "09:00-17:00 UTC").
    pub auto_deploy_window: Option<String>,
    /// Max auto-deploys per hour. 0 = no limit.
    #[serde(default)]
    pub max_auto_deploys_per_hour: u32,
}

/// Lifecycle status of an [`App`]. See `docs/architecture.md` §3.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type)]
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
    /// Deploy mode (pull, build, pack).
    pub deploy_mode: DeployMode,
    /// Git repository URL for deploy_mode build/pack.
    pub source_repo: Option<String>,
    /// Branch or tag for deploy_mode build/pack.
    pub source_branch: Option<String>,
    /// Whether to auto-deploy on webhook push.
    pub auto_deploy: bool,
    /// Time window for auto-deploys.
    pub auto_deploy_window: Option<String>,
    /// Max auto-deploys per hour.
    pub max_auto_deploys_per_hour: u32,
}

/// Input to [`crate::ports::StoragePort::create_app`].
#[derive(Debug, Clone, Default)]
pub struct NewApp {
    pub name: String,
    pub owner: String,
    pub env: AppEnv,
    pub git_repo: Option<String>,
    pub image_ref: Option<String>,
    pub config_yaml: String,
    pub health_path: Option<String>,
    pub deploy_mode: DeployMode,
    pub source_repo: Option<String>,
    pub source_branch: Option<String>,
    pub auto_deploy: bool,
    pub auto_deploy_window: Option<String>,
    pub max_auto_deploys_per_hour: u32,
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
    pub deploy_mode: Option<DeployMode>,
    pub source_repo: Option<Option<String>>,
    pub source_branch: Option<Option<String>>,
    pub auto_deploy: Option<bool>,
    pub auto_deploy_window: Option<Option<String>>,
    pub max_auto_deploys_per_hour: Option<u32>,
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

    #[test]
    fn deploy_mode_str_roundtrip() {
        for m in [DeployMode::Pull, DeployMode::Build, DeployMode::Pack, DeployMode::Native] {
            let s = m.to_string();
            let back: DeployMode = s.parse().unwrap();
            assert_eq!(m, back);
        }
    }

    #[test]
    fn deploy_mode_default_is_pull() {
        assert_eq!(DeployMode::default(), DeployMode::Pull);
    }

    #[test]
    fn deploy_mode_all_values() {
        assert_eq!(DeployMode::ALL, &["pull", "build", "pack", "native"]);
    }

    #[test]
    fn source_config_default() {
        let s = SourceConfig::default();
        assert_eq!(s.repo, None);
        assert_eq!(s.branch, None);
        assert_eq!(s.deploy_mode, DeployMode::Pull);
        assert!(!s.auto_deploy);
        assert_eq!(s.max_auto_deploys_per_hour, 0);
    }

    #[test]
    fn source_config_serde_roundtrip() {
        let s = SourceConfig {
            repo: Some("git@github.com:user/repo.git".into()),
            branch: Some("main".into()),
            deploy_mode: DeployMode::Pack,
            webhook_secret_ref: Some("MY_SECRET".into()),
            auto_deploy: true,
            auto_deploy_window: Some("09:00-17:00 UTC".into()),
            max_auto_deploys_per_hour: 3,
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: SourceConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}
