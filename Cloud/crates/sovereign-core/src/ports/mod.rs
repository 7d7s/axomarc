// The hexagonal ports. V0 ships only `StoragePort`; the other ports
// (Runtime, Proxy, Secrets, Backup, Audit, Observability, Agent,
// Policy) are added in their respective feature phases. Keeping them
// in one module so the use cases can `use sovereign_core::ports::*;`
// and pick up the full set as the workspace grows.

pub mod runtime;
pub mod storage;

pub use runtime::{ContainerSpec, HealthResult, MountSpec, RuntimeEndpoint, RuntimePort};
pub use storage::StoragePort;
