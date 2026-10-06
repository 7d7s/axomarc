//! keel-kernel: Kernel facade crate.
//! 
//! Re-exports the framework crates that kernel binaries (keel-api, keel-worker)
//! and application modules depend on. This crate is published to the registry
//! independently of keel-platform, enabling:
//! - Independent kernel releases (keel-api, keel-worker binaries)
//! - Application modules depending on `keel-kernel` without pulling in
//!   all 75+ keel-platform internal crates
//! - Clear dependency boundaries: apps don't depend on keel-platform internals

// Re-export framework crates
pub use keel_config;
pub use keel_execution;
pub use keel_tenancy;
pub use keel_registry;
pub use keel_http;
pub use keel_jobs;
pub use keel_event;
pub use keel_wasm;
pub use keel_caching;
pub use keel_scheduling;
pub use keel_storage;
pub use keel_search;
pub use keel_realtime;
pub use keel_deployment;
pub use keel_postgres;
pub use keel_mysql;
pub use keel_feature_flags;
pub use keel_session;
pub use keel_identity;
pub use keel_policy;
pub use keel_integrity;
pub use keel_observability;
pub use keel_telemetry;
pub use keel_foundation;
pub use keel_lifecycle;
pub use keel_context;

// Re-export common types for convenience (only what actually exists)
pub use keel_foundation::{
    AppError, PrincipalId, TenantId, ExecutionId, CorrelationId,
    FeatureRevision, AuthorizationRevision,
    ExecutionProfile, ExecutionProfileKind,
};
pub use keel_config::{PlatformConfig, FrozenConfig};
pub use keel_tenancy::TenantScope;
pub use keel_registry::{ModuleManifest, RouteSpec};
pub use keel_http::{CsrfGuard, SecurityHeaders};
pub use keel_postgres::{TenantGuardedPool, PgPool, PgPoolOptions, create_pool, run_migrations};
pub use keel_mysql::DbPool;
pub use keel_jobs::{JobRegistry, JobStore, JobStoreAdmin, DurableJob, JobStatus, InMemoryJobStore, queue_restart_requested};
pub use keel_event::{ListenerRegistry, OutboxPump, OutboxStore};
pub use keel_scheduling::{SchedulerRunner, ScheduleRegistry};
pub use keel_wasm::{WasmPlugin, WasmHandler};
pub use keel_caching::CachePort;
pub use keel_storage::StoragePort;
pub use keel_search::SearchEngine;
pub use keel_realtime::BroadcastHub;
pub use keel_session::{SessionManager, SessionPolicy, SessionStore};
pub use keel_identity::{PasswordParams};
pub use keel_policy::PolicyEngine;
pub use keel_observability::TelemetryConfig;
pub use keel_lifecycle::{ApplicationHost, Phase, Provider, ServiceRegistry, BootError};
pub use keel_context::{ExecutionContext};