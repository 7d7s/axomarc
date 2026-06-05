// The hexagonal ports. V0 ships only `StoragePort`; the other ports
// (Runtime, Proxy, Secrets, Backup, Audit, Observability, Agent,
// Policy) are added in their respective feature phases. Keeping them
// in one module so the use cases can `use sovereign_core::ports::*;`
// and pick up the full set as the workspace grows.

pub mod proxy;
pub mod runtime;
pub mod secrets;
pub mod storage;

pub use proxy::{default_v0_host, ProxyPort, V0_DEFAULT_HOST_SUFFIX};
pub use runtime::{ContainerSpec, HealthResult, MountSpec, RuntimeEndpoint, RuntimePort};
pub use secrets::SecretsPort;
pub use storage::StoragePort;
