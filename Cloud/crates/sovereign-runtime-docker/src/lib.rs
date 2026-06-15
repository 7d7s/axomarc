//! # sovereign-runtime-docker
//!
//! **Docker runtime adapter for Sovereign.**
//!
//! Implements [`RuntimePort`] against the local Docker daemon (or a remote
//! one via `DOCKER_HOST`). Uses bollard for the Docker REST API.
//!
//! ## Features
//!
//! - **Image pull** — Pull images from any registry
//! - **Container lifecycle** — Create, start, stop, remove
//! - **Port mapping** — Bind to 127.0.0.1:$PORT
//! - **Health checks** — HTTP probe against container IP
//! - **Log streaming** — Stream stdout/stderr with follow mode
//!
//! ## Connection
//!
//! ```rust,no_run
//! use sovereign_runtime_docker::DockerRuntime;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! // Connect from DOCKER_HOST env or platform default
//! let runtime = DockerRuntime::connect_from_env().await?;
//!
//! // Or connect to a specific endpoint
//! use sovereign_core::ports::RuntimeEndpoint;
//! let runtime = DockerRuntime::connect(RuntimeEndpoint::UnixSocket("/var/run/docker.sock".into())).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Container Naming
//!
//! Containers are named `sovereign-{app_id}-{deployment_id}` to avoid
//! collisions during concurrent deploys of the same app.
//!
//! ## Port Binding
//!
//! V0 binds container ports to `127.0.0.1:$PORT` (loopback only).
//! The Caddy reverse proxy handles external traffic.

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

    /// Access the inner bollard `Docker` handle for operations not
    /// covered by `RuntimePort` (e.g. log streaming).
    pub fn inner_handle(&self) -> &Docker {
        &self.inner
    }

    /// Stream container logs. Returns a pinned stream of log lines.
    /// Each line is a `(is_stderr, message_bytes)` tuple.
    pub fn logs(
        &self,
        container_name: &str,
        tail: &str,
        follow: bool,
    ) -> impl futures::Stream<Item = Result<(bool, Vec<u8>), AppError>> {
        let opts = bollard::container::LogsOptions::<String> {
            stdout: true,
            stderr: true,
            tail: tail.to_string(),
            follow,
            ..Default::default()
        };
        let stream = self.inner.logs(container_name, Some(opts));
        stream.map(|chunk| {
            chunk
                .map(|output| match output {
                    bollard::container::LogOutput::StdOut { message } => (false, message.to_vec()),
                    bollard::container::LogOutput::StdErr { message } => (true, message.to_vec()),
                    bollard::container::LogOutput::Console { message } => (false, message.to_vec()),
                    _ => (false, Vec::new()),
                })
                .map_err(|e| AppError::Upstream(format!("log stream: {e}")))
        })
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
        // V0 design: pin the host port to `spec.port` (we expect apps
        // to listen on 8080; the proxy reverse-proxies to that port).
        // Letting Docker pick a random port would force every consumer
        // of the port (the proxy, the healthcheck, the CLI log) to
        // re-inspect the container. V0's UX is "one port, no surprises".
        //
        // V0.5 will lift this restriction via `ContainerSpec::host_port`
        // (None = Docker picks, Some = fixed).
        let port_key = format!("{}/tcp", spec.port);
        let host_binding = bollard::models::PortBinding {
            host_ip: Some("127.0.0.1".into()),
            host_port: Some(spec.port.to_string()),
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
