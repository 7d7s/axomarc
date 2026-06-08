// System service domain types. These represent the 7 managed system
// services (nginx, mysql, mariadb, redis, vsftpd, letsencrypt,
// phpmyadmin) that `sovereign service install/remove/status/restart`
// manages. The CLI and use-cases layer reference these types; the
// actual install/remove logic lives in `sovereign-systemd`.

use serde::{Deserialize, Serialize};

use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum ServiceKind {
    Nginx,
    MariaDB,
    MySQL,
    Redis,
    VsFtpd,
    LetEncrypt,
    PhpMyAdmin,
}

impl ServiceKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Nginx => "nginx",
            Self::MariaDB => "mariadb",
            Self::MySQL => "mysql",
            Self::Redis => "redis",
            Self::VsFtpd => "vsftpd",
            Self::LetEncrypt => "letsencrypt",
            Self::PhpMyAdmin => "phpmyadmin",
        }
    }

    pub fn all() -> &'static [ServiceKind] {
        use ServiceKind::*;
        &[Nginx, MariaDB, MySQL, Redis, VsFtpd, LetEncrypt, PhpMyAdmin]
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "nginx" => Some(Self::Nginx),
            "mariadb" => Some(Self::MariaDB),
            "mysql" => Some(Self::MySQL),
            "redis" => Some(Self::Redis),
            "vsftpd" => Some(Self::VsFtpd),
            "letsencrypt" | "le" => Some(Self::LetEncrypt),
            "phpmyadmin" | "pma" => Some(Self::PhpMyAdmin),
            _ => None,
        }
    }
}

impl std::fmt::Display for ServiceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ServiceKind {
    type Err = AppError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s).ok_or_else(|| AppError::validation(format!("unknown service: {s}")))
    }
}

/// Runtime state of a managed service, as reported by
/// `sovereign service status <KIND>`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceStatus {
    pub kind: ServiceKind,
    pub installed: bool,
    pub enabled: bool,
    pub active: bool,
    pub version: Option<String>,
    pub pid: Option<u32>,
    pub uptime_secs: Option<u64>,
    pub config_ok: bool,
    pub error: Option<String>,
}

/// The result of a successful install. Returned to the CLI
/// so it can print "installed nginx 1.24.0".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInstallResult {
    pub kind: ServiceKind,
    pub version: String,
    pub was_already_installed: bool,
}

/// Specification for `sovereign service install <KIND>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceInstallSpec {
    pub kind: ServiceKind,
    /// Pinned version (e.g. "1.24.0-0ubuntu3"). `None` = latest.
    pub version: Option<String>,
    /// Skip `systemctl enable --now` after install.
    pub no_start: bool,
}

/// Specification for `sovereign service remove <KIND>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceRemoveSpec {
    pub kind: ServiceKind,
    pub purge_config: bool,
}

/// Specification for `sovereign service config <KIND>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConfig {
    pub kind: ServiceKind,
    pub path: String,
    pub content: String,
}

/// The audit event payload for a system-service action.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceAuditPayload {
    pub kind: ServiceKind,
    pub action: String,
    pub exit_code: Option<i32>,
    pub sha256_config: Option<String>,
    pub duration_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    #[test]
    fn kind_parse_round_trip() {
        for kind in ServiceKind::all() {
            let s = kind.as_str();
            let parsed = ServiceKind::parse(s).unwrap();
            assert_eq!(*kind, parsed);
        }
    }

    #[test]
    fn kind_from_str_handles_aliases() {
        assert_eq!(
            ServiceKind::from_str("le").unwrap(),
            ServiceKind::LetEncrypt
        );
        assert_eq!(
            ServiceKind::from_str("pma").unwrap(),
            ServiceKind::PhpMyAdmin
        );
    }

    #[test]
    fn kind_from_str_rejects_unknown() {
        assert!(ServiceKind::from_str("unknown").is_err());
    }

    #[test]
    fn kind_all_has_7_entries() {
        assert_eq!(ServiceKind::all().len(), 7);
    }
}
