//! Docker runtime adapter — implements [`RuntimePort`] against the local
//! Docker daemon (or a remote one via `DOCKER_HOST`).
//!
//! The runtime owns the bollard [`Docker`] handle. Bollard 0.18 uses
//! `connect_with_*_defaults` constructors and the container config
//! types are in `bollard::container`; the model types (`PortBinding`,
//! `HostConfig`) are re-exported from `bollard::models`.
//!
//! The V0 health-probe convention is to bridge the container port to
//! `127.0.0.1:0` (kernel-assigned), then HTTP-probe `127.0.0.1:<port>`.
//! F6 (`sovereign-proxy-caddy`) replaces this with Caddy-backed health
//! checks once the proxy is wired in.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bollard::container::{
    Config, CreateContainerOptions, RemoveContainerOptions, StopContainerOptions,
};
use bollard::image::CreateImageOptions;
use bollard::models::HostConfig;
use bollard::Docker;
use futures::StreamExt;
use tracing::{debug, info, instrument};

use sovereign_core::error::AppError;
use sovereign_core::ports::{ContainerSpec, HealthResult, RuntimeEndpoint, RuntimePort};

/// The Docker-backed runtime. Cheap to clone (`Arc` inside).
#[derive(Clone)]
pub struct DockerRuntime {
    inner: Arc<Docker>,
}

impl std::fmt::Debug for DockerRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DockerRuntime").finish_non_exhaustive()
    }
}

impl DockerRuntime {
    /// Connect to the daemon at the resolved endpoint.
    #[instrument]
    pub async fn connect(endpoint: RuntimeEndpoint) -> Result<Self, AppError> {
        let docker = match endpoint {
            RuntimeEndpoint::UnixSocket(path) => {
                let clean = path.trim_start_matches("unix://");
                Docker::connect_with_socket(clean, 120, bollard::API_DEFAULT_VERSION)
                    .map_err(|e| AppError::Upstream(format!("docker connect (unix): {e}")))?
            }
            RuntimeEndpoint::Tcp(addr) => {
                let clean = addr.trim_start_matches("tcp://");
                Docker::connect_with_http(clean, 120, bollard::API_DEFAULT_VERSION)
                    .map_err(|e| AppError::Upstream(format!("docker connect (tcp): {e}")))?
            }
            RuntimeEndpoint::NamedPipe(path) => {
                let clean = path.trim_start_matches("npipe://");
                Docker::connect_with_named_pipe(clean, 120, bollard::API_DEFAULT_VERSION)
                    .map_err(|e| AppError::Upstream(format!("docker connect (npipe): {e}")))?
            }
        };
        // Ping the daemon. A successful `version` call proves the
        // socket is alive and the version is compatible.
        docker
            .version()
            .await
            .map_err(|e| AppError::Upstream(format!("docker ping failed: {e}")))?;
        info!("docker runtime connected");
        Ok(Self {
            inner: Arc::new(docker),
        })
    }

    /// Connect using `DOCKER_HOST` (or the platform default).
    pub async fn connect_from_env() -> Result<Self, AppError> {
        Self::connect(RuntimeEndpoint::from_env()).await
    }
}

#[async_trait]
impl RuntimePort for DockerRuntime {
    #[instrument(skip(self))]
    async fn pull_image(&self, image: &str) -> Result<(), AppError> {
        let opts: CreateImageOptions<'_, String> = CreateImageOptions {
            from_image: image.to_string(),
            ..Default::default()
        };
        // `create_image` returns a Stream (synchronous fn, not async).
        // We don't `.await` the call itself; we await the inner
        // `StreamExt::next` futures to drain progress events.
        let mut stream = self.inner.create_image(Some(opts), None, None);
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(_info) => debug!("pull chunk: ok"),
                Err(e) => {
                    return Err(AppError::Upstream(format!(
                        "pull_image({image}) stream: {e}"
                    )));
                }
            }
        }
        Ok(())
    }

    #[instrument(skip(self, spec))]
    async fn create_container(&self, spec: ContainerSpec) -> Result<String, AppError> {
        let opts: CreateContainerOptions<String> = CreateContainerOptions {
            name: spec.name.clone(),
            platform: None,
        };
        // Map container port <spec.port>/tcp to a random host port.
        let port_key = format!("{}/tcp", spec.port);
        let host_binding = bollard::models::PortBinding {
            host_ip: Some("0.0.0.0".into()),
            host_port: None, // ask Docker to assign
        };
        let port_bindings: bollard::models::PortMap =
            [(port_key.clone(), Some(vec![host_binding]))]
                .into_iter()
                .collect();
        let host_config = HostConfig {
            port_bindings: Some(port_bindings),
            ..Default::default()
        };
        let env: Vec<String> = spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        // `ExposedPorts` is `Option<HashMap<T, HashMap<(), ()>>>`.
        let mut exposed: std::collections::HashMap<String, std::collections::HashMap<(), ()>> =
            std::collections::HashMap::new();
        exposed.insert(port_key, std::collections::HashMap::new());
        let config: Config<String> = Config {
            image: Some(spec.image.clone()),
            env: Some(env),
            exposed_ports: Some(exposed),
            host_config: Some(host_config),
            ..Default::default()
        };
        let resp = self
            .inner
            .create_container(Some(opts), config)
            .await
            .map_err(|e| AppError::Upstream(format!("create_container: {e}")))?;
        Ok(resp.id)
    }

    #[instrument(skip(self))]
    async fn start_container(&self, id: &str) -> Result<(), AppError> {
        self.inner
            .start_container::<String>(id, None)
            .await
            .map_err(|e| AppError::Upstream(format!("start_container({id}): {e}")))?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn stop_container(&self, id: &str, timeout: Duration) -> Result<(), AppError> {
        let opts = StopContainerOptions {
            // bollard treats `t` as the SIGTERM grace in seconds.
            t: timeout.as_secs() as i64,
        };
        self.inner
            .stop_container(id, Some(opts))
            .await
            .map_err(|e| AppError::Upstream(format!("stop_container({id}): {e}")))?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn remove_container(&self, id: &str) -> Result<(), AppError> {
        let opts = RemoveContainerOptions {
            force: true,
            ..Default::default()
        };
        self.inner
            .remove_container(id, Some(opts))
            .await
            .map_err(|e| AppError::Upstream(format!("remove_container({id}): {e}")))?;
        Ok(())
    }

    #[instrument(skip(self))]
    async fn healthcheck(
        &self,
        id: &str,
        port: u16,
        path: &str,
        timeout: Duration,
    ) -> Result<HealthResult, AppError> {
        // Resolve the container's IP via inspect. V0 assumes the
        // default bridge network; the F6 proxy adapter will route
        // by container name once Caddy is wired in.
        let inspect = self
            .inner
            .inspect_container(id, None)
            .await
            .map_err(|e| AppError::Upstream(format!("inspect_container({id}): {e}")))?;
        let ip = inspect
            .network_settings
            .as_ref()
            .and_then(|ns| ns.networks.as_ref())
            .and_then(|nets| nets.get("bridge"))
            .and_then(|ep| ep.ip_address.clone())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                AppError::Upstream(format!("no IP for container {id} on bridge network"))
            })?;
        let probe_timeout = timeout / 3; // 3 retries
        let r = sovereign_core::use_cases::health::probe_http(&ip, port, path, probe_timeout).await;
        Ok(r)
    }
}
