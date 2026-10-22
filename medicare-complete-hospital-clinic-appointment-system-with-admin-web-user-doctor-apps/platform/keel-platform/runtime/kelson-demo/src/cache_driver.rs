//! The cache driver subscriber: the configuration chooses the
//! infrastructure, and a reload changes it **under the running service**.
//!
//! The registry (kelson-caching) owns the selection, the capability
//! manifest and the runtime holder. This subscriber is the wiring that
//! connects a configuration reload to the selection: when `[cache].driver`
//! changes, the named driver is activated, its store is built, and the
//! runtime holder is swapped - so the next cache read in the process goes
//! to the new driver. No rebuild, no restart.
//!
//! A driver that cannot be built (a shared cache that is unreachable) or
//! is not registered is REFUSED, and the previous driver stays active.
//! Fail-closed: a deployment never silently runs on infrastructure it was
//! not configured for.

use std::sync::Arc;

use kelson_config::{ConfigReloader, ConfigSnapshot};
use kelson_foundation::AppError;

/// Swaps the cache driver when the configuration names a different one.
pub struct CacheDriverReloader {
    registry: Arc<kelson_caching::CacheDriverRegistry>,
}

impl CacheDriverReloader {
    /// The subscriber over the registry that owns the selection.
    pub fn new(registry: Arc<kelson_caching::CacheDriverRegistry>) -> Self {
        Self { registry }
    }

    /// The registry's driver name for a configured driver.
    ///
    /// The configuration's enum is the deployment's vocabulary; the
    /// registry's name is the framework's. This is the only place the two
    /// are mapped.
    pub fn driver_name(driver: kelson_config::CacheDriver) -> &'static str {
        match driver {
            kelson_config::CacheDriver::Memory => "memory",
            kelson_config::CacheDriver::Redis => "redis",
        }
    }

    /// Activate the driver the configuration names.
    ///
    /// Returns the driver name that is now active. An error means the
    /// previous driver is still active.
    pub fn refresh(&self, snapshot: &ConfigSnapshot) -> Result<&'static str, AppError> {
        let name = Self::driver_name(snapshot.config.cache.driver);
        self.registry.activate(name)?;
        Ok(name)
    }
}

impl ConfigReloader for CacheDriverReloader {
    fn on_reload(&self, snapshot: &ConfigSnapshot) {
        match self.refresh(snapshot) {
            Ok(name) => tracing::info!(
                version = snapshot.version,
                driver = name,
                swaps = self.registry.swaps(),
                "the cache driver was swapped"
            ),
            Err(error) => tracing::warn!(
                version = snapshot.version,
                error = %error.public_message,
                "the cache driver swap was refused; the previous driver stays active"
            ),
        }
    }
}
