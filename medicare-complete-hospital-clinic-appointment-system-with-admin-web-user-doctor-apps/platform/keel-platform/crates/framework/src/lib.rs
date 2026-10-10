//! kelson-framework: the framework umbrella crate.
//!
//! Re-exports the published framework surface that host binaries (`kelson-api`,
//! `kelson-worker`) and application modules depend on. This is the one
//! dependency an application takes:
//!
//! - Applications depend on `kelson-framework` without pulling in all 60+
//!   internal workspace crates
//! - Clear dependency boundaries: apps don't depend on framework internals
//! - A single, reviewable public surface governed by `docs/API-STABILITY.md`
//!
//! This crate is re-exports only — it contains no logic and declares no new
//! external dependencies, so the D-62 external dependency budget (25/25) is
//! untouched by depending on it.

// Re-export framework crates
pub use kelson_config;
pub use kelson_execution;
pub use kelson_tenancy;
pub use kelson_registry;
pub use kelson_http;
pub use kelson_jobs;
pub use kelson_event;
pub use kelson_wasm;
pub use kelson_caching;
pub use kelson_scheduling;
pub use kelson_storage;
pub use kelson_search;
pub use kelson_realtime;
pub use kelson_deployment;
pub use kelson_postgres;
pub use kelson_mysql;
pub use kelson_feature_flags;
pub use kelson_session;
pub use kelson_identity;
pub use kelson_policy;
pub use kelson_integrity;
pub use kelson_observability;
pub use kelson_telemetry;
pub use kelson_foundation;
pub use kelson_lifecycle;
pub use kelson_context;

// Re-export common types for convenience (only what actually exists)
pub use kelson_foundation::{
    AppError, PrincipalId, TenantId, ExecutionId, CorrelationId,
    FeatureRevision, AuthorizationRevision,
    ExecutionProfile, ExecutionProfileKind,
};
pub use kelson_config::{PlatformConfig, FrozenConfig};
pub use kelson_tenancy::TenantScope;
pub use kelson_registry::{ModuleManifest, RouteSpec};
pub use kelson_http::{CsrfGuard, SecurityHeaders};
pub use kelson_postgres::{TenantGuardedPool, PgPool, PgPoolOptions, create_pool, run_migrations};
pub use kelson_mysql::DbPool;
pub use kelson_jobs::{JobRegistry, JobStore, JobStoreAdmin, DurableJob, JobStatus, InMemoryJobStore, queue_restart_requested};
pub use kelson_event::{ListenerRegistry, OutboxPump, OutboxStore};
pub use kelson_scheduling::{SchedulerRunner, ScheduleRegistry};
pub use kelson_wasm::{WasmPlugin, WasmHandler};
pub use kelson_caching::CachePort;
pub use kelson_storage::StoragePort;
pub use kelson_search::SearchEngine;
pub use kelson_realtime::BroadcastHub;
pub use kelson_session::{SessionManager, SessionPolicy, SessionStore};
pub use kelson_identity::{PasswordParams};
pub use kelson_policy::PolicyEngine;
pub use kelson_observability::TelemetryConfig;
pub use kelson_lifecycle::{ApplicationHost, Phase, Provider, ServiceRegistry, BootError};
pub use kelson_context::{ExecutionContext};