// The system-service port. Implemented by `sovereign-systemd`
// (Ubuntu/Debian via apt + systemd). This port lets the CLI and
// use-cases layer install/remove/status/restart managed system
// services without depending on the concrete adapter.
//
// Per `docs/phase-0.6.md` §S1, the 7 managed services are:
// nginx, mysql, mariadb, redis, vsftpd, letsencrypt, phpmyadmin.
// The port exposes the common operations; each adapter translates
// to the correct apt package name and systemctl unit.

use async_trait::async_trait;

use crate::domain::service::{
    ServiceAuditPayload, ServiceConfig, ServiceInstallResult, ServiceInstallSpec, ServiceKind,
    ServiceRemoveSpec, ServiceStatus,
};
use crate::error::AppError;

/// The system-service port. A use-case calls these methods; the
/// concrete adapter performs I/O (apt, systemctl, nginx -t, etc.).
#[async_trait]
pub trait SystemServicePort: Send + Sync {
    /// Install a system service. Returns the installed version and
    /// whether it was already present.
    async fn install(&self, spec: &ServiceInstallSpec) -> Result<ServiceInstallResult, AppError>;

    /// Remove (purge) a system service. If `purge_config` is true,
    /// removes config files too (apt purge vs remove).
    async fn remove(&self, spec: &ServiceRemoveSpec) -> Result<(), AppError>;

    /// Check the current status of a system service.
    async fn status(&self, kind: ServiceKind) -> Result<ServiceStatus, AppError>;

    /// Restart a system service (systemctl restart).
    async fn restart(&self, kind: ServiceKind) -> Result<(), AppError>;

    /// Validate configuration for a system service (e.g. `nginx -t`).
    async fn validate_config(&self, kind: ServiceKind) -> Result<(), AppError>;

    /// List all managed services with their current status.
    async fn list_all(&self) -> Result<Vec<ServiceStatus>, AppError>;

    /// Write a config file atomically and return the SHA-256 of its
    /// content. The adapter is responsible for placing it in the
    /// correct path for the service kind.
    async fn write_config(&self, config: &ServiceConfig) -> Result<String, AppError>;

    /// Return the audit payload for a completed service action.
    /// Callers can pass this directly to `StoragePort::append_audit`.
    fn audit_payload(
        &self,
        kind: ServiceKind,
        action: &str,
        exit_code: Option<i32>,
        sha256_config: Option<String>,
        duration_ms: u64,
    ) -> ServiceAuditPayload {
        ServiceAuditPayload {
            kind,
            action: action.to_string(),
            exit_code,
            sha256_config,
            duration_ms,
        }
    }
}
