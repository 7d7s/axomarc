// The domain types. Per `docs/architecture.md` §1, this module contains
// pure Rust types only — no I/O, no adapters. The only "external" types
// it uses are `serde_json::Value` (for the audit payload column) and
// `uuid::Uuid` (in the ID newtypes).

pub mod app;
pub mod audit;
pub mod backup;
pub mod deployment;
pub mod domain;
pub mod id;
pub mod secret;
pub mod server;
pub mod timestamp;
pub mod user;

pub use app::{App, AppEnv, AppStatus, AppUpdate, NewApp};
pub use audit::{kind, AuditEvent, AuditKind, AuditQuery};
pub use backup::{Backup, BackupStatus, NewBackup};
pub use deployment::{Deployment, DeploymentEvent, DeploymentStatus, NewDeployment, Strategy};
pub use domain::{Domain, NewDomain, TlsStatus};
pub use id::{AppId, BackupId, DeploymentId, DomainId, SecretId, ServerId, UserId};
pub use secret::{NewSecret, Secret, SecretStatus};
pub use server::{NewServer, Server, ServerRole, ServerStatus};
pub use timestamp::Timestamp;
pub use user::{NewUser, User, UserRole};
