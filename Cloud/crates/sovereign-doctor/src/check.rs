use async_trait::async_trait;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;

/// The diagnostic category. The 6 V0 categories are `System`, `Binary`,
/// `Storage`, `Runtime`, `Proxy`, and `Secrets`. The remaining
/// categories are V1+ but the enum is exhaustive so future code does
/// not have to thread an `Option<CheckCategory>` everywhere.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Hash)]
pub enum CheckCategory {
    System,
    Binary,
    Storage,
    Runtime,
    Proxy,
    Secrets,
    Backup,
    Network,
    Agents,
    Observability,
    Security,
    Sovereignty,
    Performance,
    Cost,
}

impl CheckCategory {
    /// Stable, machine-friendly name.
    pub fn as_str(self) -> &'static str {
        match self {
            CheckCategory::System => "system",
            CheckCategory::Binary => "binary",
            CheckCategory::Storage => "storage",
            CheckCategory::Runtime => "runtime",
            CheckCategory::Proxy => "proxy",
            CheckCategory::Secrets => "secrets",
            CheckCategory::Backup => "backup",
            CheckCategory::Network => "network",
            CheckCategory::Agents => "agents",
            CheckCategory::Observability => "observability",
            CheckCategory::Security => "security",
            CheckCategory::Sovereignty => "sovereignty",
            CheckCategory::Performance => "performance",
            CheckCategory::Cost => "cost",
        }
    }
}

impl std::fmt::Display for CheckCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The outcome of a single check. `Skip` is used when a check is
/// not applicable on the current platform (e.g. a Linux-only check
/// on Windows); it is never counted as `Fail` for exit-code purposes.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, Hash)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
    Skip,
}

impl CheckStatus {
    /// CLI exit-code weight: 0 = pass, 1 = warn, 2 = fail, 0 = skip.
    pub fn exit_weight(self) -> u8 {
        match self {
            CheckStatus::Pass | CheckStatus::Skip => 0,
            CheckStatus::Warn => 1,
            CheckStatus::Fail => 2,
        }
    }

    /// One-character glyph for the CLI summary.
    pub fn glyph(self) -> &'static str {
        match self {
            CheckStatus::Pass => "OK",
            CheckStatus::Warn => "WN",
            CheckStatus::Fail => "FL",
            CheckStatus::Skip => "--",
        }
    }
}

/// The shared inputs a check needs.
///
/// `data_dir` is the operator's `/var/lib/sovereign` (or override);
/// `binary_path` is the path to the running `sovereign` binary;
/// the rest are cheap to construct in tests.
#[derive(Clone)]
pub struct CheckContext {
    pub data_dir: PathBuf,
    pub binary_path: PathBuf,
    pub binary_version: &'static str,
    pub db_filename: &'static str,
    pub master_key_filename: &'static str,
    pub docker_socket: PathBuf,
    pub caddy_admin: String,
    pub caddy_binary: PathBuf,
    pub caddy_config: PathBuf,
    pub runtime: Option<Arc<dyn sovereign_core::ports::RuntimePort>>,
}

impl CheckContext {
    /// Build a context with sensible V0 defaults; tests can override
    /// individual fields by mutating the struct.
    pub fn for_tests(data_dir: PathBuf, binary_path: PathBuf) -> Self {
        Self {
            data_dir,
            binary_path,
            binary_version: env!("CARGO_PKG_VERSION"),
            db_filename: "sovereign.db",
            master_key_filename: "master.key",
            docker_socket: PathBuf::from("/var/run/docker.sock"),
            caddy_admin: "http://127.0.0.1:2019".into(),
            caddy_binary: PathBuf::from("caddy"),
            caddy_config: PathBuf::from("/etc/caddy/Caddyfile"),
            runtime: None,
        }
    }
}

/// The result of a single check.
pub struct CheckResult {
    pub status: CheckStatus,
    pub message: String,
    pub suggestion: Option<String>,
    /// Optional auto-fix the operator can opt into with `sovereign doctor --fix`.
    pub fix: Option<Box<dyn crate::fix::DoctorFix>>,
}

impl CheckResult {
    pub fn pass(message: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Pass,
            message: message.into(),
            suggestion: None,
            fix: None,
        }
    }

    pub fn warn(message: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Warn,
            message: message.into(),
            suggestion: None,
            fix: None,
        }
    }

    pub fn fail(message: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Fail,
            message: message.into(),
            suggestion: None,
            fix: None,
        }
    }

    pub fn skip(message: impl Into<String>) -> Self {
        Self {
            status: CheckStatus::Skip,
            message: message.into(),
            suggestion: None,
            fix: None,
        }
    }

    pub fn with_suggestion(mut self, suggestion: impl Into<String>) -> Self {
        self.suggestion = Some(suggestion.into());
        self
    }

    pub fn with_fix(mut self, fix: Box<dyn crate::fix::DoctorFix>) -> Self {
        self.fix = Some(fix);
        self
    }
}

/// A single diagnostic check.
#[async_trait]
pub trait Check: Send + Sync {
    fn name(&self) -> &'static str;
    fn category(&self) -> CheckCategory;
    async fn run(&self, ctx: &CheckContext) -> CheckResult;
}
