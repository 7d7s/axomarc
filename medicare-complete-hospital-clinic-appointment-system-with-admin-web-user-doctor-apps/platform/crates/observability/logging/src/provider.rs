//! ObservabilityProvider (Sentinel S5 §27): boots the telemetry runtime in
//! `Phase::Foundation` (depends only on config), installs the tracing bridge
//! and registers the runtime + metrics registry in the service container.
//! Degraded boot (no DB) still gets telemetry (G10): file/in-memory only.

use std::sync::Arc;

use keel_lifecycle::{BootError, Phase, Provider, ServiceRegistry};
use keel_telemetry::{ServiceIdentity, TelemetryPlugin};

use crate::config::TelemetryConfig;
use crate::runtime::ObservabilityRuntime;

pub struct ObservabilityProvider {
    config: TelemetryConfig,
    service: ServiceIdentity,
    plugins: Vec<Box<dyn TelemetryPlugin>>,
    /// Hosts that construct the runtime themselves (e.g. to get logging
    /// before the lifecycle host boots) adopt it here instead of building
    /// a second runtime in `init`.
    prebuilt: Option<Arc<ObservabilityRuntime>>,
}

impl ObservabilityProvider {
    pub fn new(config: TelemetryConfig, service: ServiceIdentity) -> Self {
        Self {
            config,
            service,
            plugins: Vec::new(),
            prebuilt: None,
        }
    }

    /// Build from the platform config section (`PlatformConfig.telemetry`).
    pub fn from_platform(
        app_name: impl Into<String>,
        environment: impl Into<String>,
        section: Option<&serde_json::Value>,
    ) -> Self {
        Self::new(
            TelemetryConfig::from_section(section),
            ServiceIdentity::new(app_name, environment),
        )
    }

    /// Adopt an externally-built runtime. `init` registers this handle in
    /// the service container; `boot` calls `start`, which is idempotent,
    /// so hosts that already started the runtime (early boot logging) are
    /// safe to wire through this provider.
    pub fn from_runtime(runtime: Arc<ObservabilityRuntime>) -> Self {
        Self {
            config: runtime.config().clone(),
            service: runtime.service().clone(),
            plugins: Vec::new(),
            prebuilt: Some(runtime),
        }
    }

    /// Applications/modules may contribute plugins (matrix allows
    /// applications → observability).
    pub fn with_plugin(mut self, plugin: Box<dyn TelemetryPlugin>) -> Self {
        self.plugins.push(plugin);
        self
    }
}

impl Provider for ObservabilityProvider {
    fn name(&self) -> &str {
        "observability"
    }

    fn phase(&self) -> Phase {
        Phase::Foundation
    }

    /// Registration only (no I/O): the inert runtime handle goes into the
    /// container so dependent providers can resolve it; threads start in
    /// `boot`.
    fn init(&self, registry: &mut ServiceRegistry) -> Result<(), BootError> {
        let runtime = match &self.prebuilt {
            Some(runtime) => Arc::clone(runtime),
            None => {
                ObservabilityRuntime::new(self.config.clone(), self.service.clone(), &self.plugins)
                    .map_err(|e| BootError::ProviderFailed {
                        provider: self.name().to_string(),
                        source: e.to_string(),
                    })?
            }
        };
        let metrics = runtime.metrics();
        registry.insert("observability.runtime", runtime)?;
        registry.insert("observability.metrics", metrics)?;
        Ok(())
    }

    fn boot<'a>(
        &'a self,
        registry: &'a ServiceRegistry,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), BootError>> + Send + 'a>>
    {
        Box::pin(async move {
            let runtime: Arc<ObservabilityRuntime> =
                registry.get().map_err(|e| BootError::ProviderFailed {
                    provider: "observability".to_string(),
                    source: e.to_string(),
                })?;
            runtime.start().map_err(|e| BootError::ProviderFailed {
                provider: "observability".to_string(),
                source: e.to_string(),
            })?;
            Ok(())
        })
    }
}
