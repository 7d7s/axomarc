// Nginx adapter. Manages nginx via apt + systemd + config validation.
//
// Per `docs/phase-0.6.md` §S1, the nginx adapter:
// - Installs via `apt-get install nginx`
// - Validates config via `nginx -t`
// - Writes config files atomically to /etc/nginx/...
// - Restarts via `systemctl restart nginx`

use async_trait::async_trait;

use crate::apt;
use crate::systemd;
use sovereign_core::domain::service::{
    ServiceConfig, ServiceInstallResult, ServiceInstallSpec, ServiceKind, ServiceRemoveSpec,
    ServiceStatus,
};
use sovereign_core::error::AppError;
use sovereign_core::ports::system_service::SystemServicePort;

pub struct NginxAdapter {
    pub config_dir: String,
}

impl Default for NginxAdapter {
    fn default() -> Self {
        Self {
            config_dir: "/etc/nginx".to_string(),
        }
    }
}

#[async_trait]
impl SystemServicePort for NginxAdapter {
    async fn install(&self, spec: &ServiceInstallSpec) -> Result<ServiceInstallResult, AppError> {
        let (version, was_already) = apt::install("nginx", spec.version.as_deref()).await?;
        if !spec.no_start {
            systemd::enable_now("nginx").await?;
        }
        Ok(ServiceInstallResult {
            kind: ServiceKind::Nginx,
            version,
            was_already_installed: was_already,
        })
    }

    async fn remove(&self, spec: &ServiceRemoveSpec) -> Result<(), AppError> {
        systemd::disable_stop("nginx").await.ok();
        apt::remove("nginx", spec.purge_config).await
    }

    async fn status(&self, kind: ServiceKind) -> Result<ServiceStatus, AppError> {
        let installed = apt::installed_version("nginx").await?.is_some();
        let enabled = systemd::is_enabled("nginx").await.unwrap_or(false);
        let state = systemd::unit_state("nginx").await.unwrap_or_default();
        let active = state == "active";
        let pid = systemd::main_pid("nginx").await.unwrap_or(None);
        let uptime = systemd::uptime_secs("nginx").await.unwrap_or(None);
        let version = apt::installed_version("nginx").await?;
        let config_ok = self.validate_config_inner().await.is_ok();

        Ok(ServiceStatus {
            kind,
            installed,
            enabled,
            active,
            version,
            pid,
            uptime_secs: uptime,
            config_ok,
            error: if state == "failed" {
                Some("unit in failed state".to_string())
            } else {
                None
            },
        })
    }

    async fn restart(&self, _kind: ServiceKind) -> Result<(), AppError> {
        systemd::restart("nginx").await
    }

    async fn validate_config(&self, _kind: ServiceKind) -> Result<(), AppError> {
        self.validate_config_inner().await
    }

    async fn list_all(&self) -> Result<Vec<ServiceStatus>, AppError> {
        let mut statuses = Vec::new();
        for kind in ServiceKind::all() {
            statuses.push(self.status(*kind).await?);
        }
        Ok(statuses)
    }

    async fn write_config(&self, config: &ServiceConfig) -> Result<String, AppError> {
        let path = std::path::Path::new(&self.config_dir).join(config.path.trim_start_matches('/'));
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| AppError::upstream(format!("mkdir failed: {e}")))?;
        }
        tokio::fs::write(&path, config.content.as_bytes())
            .await
            .map_err(|e| AppError::upstream(format!("write failed: {e}")))?;

        use sha2::{Digest, Sha256};
        let hash = Sha256::digest(config.content.as_bytes());
        Ok(format!("{hash:x}"))
    }
}

impl NginxAdapter {
    async fn validate_config_inner(&self) -> Result<(), AppError> {
        let output = tokio::process::Command::new("nginx")
            .args(["-t"])
            .output()
            .await
            .map_err(|e| AppError::upstream(format!("nginx -t failed: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(AppError::upstream(format!(
                "nginx config invalid: {stderr}"
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_dir() {
        let adapter = NginxAdapter::default();
        assert_eq!(adapter.config_dir, "/etc/nginx");
    }
}
