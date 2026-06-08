// The adapter modules. Each module wraps one managed system service.
// The `CompositeService` struct in this module delegates to the
// appropriate adapter based on `ServiceKind`.

use async_trait::async_trait;

use sovereign_core::domain::service::{
    ServiceConfig, ServiceInstallResult, ServiceInstallSpec, ServiceKind, ServiceRemoveSpec,
    ServiceStatus,
};
use sovereign_core::error::AppError;
use sovereign_core::ports::system_service::SystemServicePort;

pub mod letsencrypt;
pub mod mariadb;
pub mod mysql;
pub mod nginx;
pub mod phpmyadmin;
pub mod redis;
pub mod vsftpd;

/// Delegates to the correct adapter based on `ServiceKind`.
/// This is the top-level entry point for `sovereign service` commands.
pub struct CompositeService;

impl CompositeService {
    pub fn new() -> Self {
        Self
    }

    fn adapter(&self, kind: ServiceKind) -> Box<dyn SystemServicePort> {
        match kind {
            ServiceKind::Nginx => Box::new(nginx::NginxAdapter::default()),
            ServiceKind::MySQL => Box::new(mysql::MysqlAdapter),
            ServiceKind::MariaDB => Box::new(mariadb::MariaDbAdapter),
            ServiceKind::Redis => Box::new(redis::RedisAdapter),
            ServiceKind::VsFtpd => Box::new(vsftpd::VsFtpdAdapter),
            ServiceKind::LetEncrypt => Box::new(letsencrypt::LetsEncryptAdapter),
            ServiceKind::PhpMyAdmin => Box::new(phpmyadmin::PhpMyAdminAdapter),
        }
    }
}

impl Default for CompositeService {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SystemServicePort for CompositeService {
    async fn install(&self, spec: &ServiceInstallSpec) -> Result<ServiceInstallResult, AppError> {
        self.adapter(spec.kind).install(spec).await
    }

    async fn remove(&self, spec: &ServiceRemoveSpec) -> Result<(), AppError> {
        self.adapter(spec.kind).remove(spec).await
    }

    async fn status(&self, kind: ServiceKind) -> Result<ServiceStatus, AppError> {
        self.adapter(kind).status(kind).await
    }

    async fn restart(&self, kind: ServiceKind) -> Result<(), AppError> {
        self.adapter(kind).restart(kind).await
    }

    async fn validate_config(&self, kind: ServiceKind) -> Result<(), AppError> {
        self.adapter(kind).validate_config(kind).await
    }

    async fn list_all(&self) -> Result<Vec<ServiceStatus>, AppError> {
        let mut statuses = Vec::with_capacity(ServiceKind::all().len());
        for kind in ServiceKind::all() {
            statuses.push(self.adapter(*kind).status(*kind).await?);
        }
        Ok(statuses)
    }

    async fn write_config(&self, config: &ServiceConfig) -> Result<String, AppError> {
        self.adapter(config.kind).write_config(config).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composite_new_is_default() {
        let c = CompositeService::new();
        let d = CompositeService;
        // Both compile; smoke test
        let _ = (c, d);
    }
}
