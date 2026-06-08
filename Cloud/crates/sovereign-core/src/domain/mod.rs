// The domain types. Per `docs/architecture.md` §1, this module contains
// pure Rust types only — no I/O, no adapters. The only "external" types
// it uses are `serde_json::Value` (for the audit payload column) and
// `uuid::Uuid` (in the ID newtypes).
//
// The submodule was originally named `domain` (clashing with this
// parent module); it was renamed to `hostname` in the phase-0 close-out.
// The re-export keeps the public type name as `Domain`.

pub mod app;
pub mod audit;
pub mod backup;
pub mod deployment;
pub mod hostname;
pub mod id;
pub mod secret;
pub mod server;
pub mod service;
pub mod timestamp;
pub mod user;

pub use app::{App, AppEnv, AppStatus, AppUpdate, NewApp};
pub use audit::{kind, AuditEvent, AuditKind, AuditQuery};
pub use backup::{Backup, BackupStatus, NewBackup};
pub use deployment::{Deployment, DeploymentEvent, DeploymentStatus, NewDeployment, Strategy};
pub use hostname::{Domain, NewDomain, TlsStatus};
pub use id::{AppId, BackupId, DeploymentId, DomainId, SecretId, ServerId, UserId};
pub use secret::{NewSecret, Secret, SecretStatus};
pub use server::{NewServer, Server, ServerRole, ServerStatus};
pub use service::{
    ServiceAuditPayload, ServiceConfig, ServiceInstallResult, ServiceInstallSpec, ServiceKind,
    ServiceRemoveSpec, ServiceStatus,
};
pub use timestamp::Timestamp;
pub use user::{NewUser, User, UserRole};
