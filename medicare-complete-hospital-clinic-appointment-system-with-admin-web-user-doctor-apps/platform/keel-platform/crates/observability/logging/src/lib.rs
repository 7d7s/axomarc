//! # kelson-observability
//! Sentinel (S5) telemetry runtime: the application-owned observability
//! pipeline. Every execution still gets a diagnostic chain (11-SPEC §30/§48:
//! Request → Execution → Authorization → Command/Query → Transaction → SQL →
//! Delta → Outbox → Result — one id opens the whole chain), and S5 evolves
//! the logging system into the full runtime: typed event bus, processing
//! DAG, TOML policy engine, durability-classed buffering with a WAL,
//! circuit-broken exporters, incident mode, audit-grade durability, metrics
//! and cost controls.
//!
//! ## Abstraction boundary (§30)
//! Applications emit through `tracing::*` macros OR the developer API
//! (`info!`-style helpers / `emit_log`); both reach the same bus and the
//! runtime decides where events land. Telemetry failure never crashes the
//! application: bounded try-send with counted drops, isolated worker, stderr
//! fallback.
//!
//! ## Legacy API
//! `init_logging`, `LogConfig`, `redact_str`, `request_span`, `ChainRecord`,
//! `ChainSink` stay available and behaviour-compatible (S1 contracts).

pub mod bridge;
pub mod config;
pub mod groups;
pub mod incident;
mod logsys;
pub mod provider;
pub mod runtime;

#[cfg(test)]
mod runtime_tests;

pub use logsys::{
    init_logging, redact_str, request_span, LogConfig, LogFormat, DEFAULT_REDACTIONS,
};

pub use config::{ExporterConfig, PolicyRuleConfig, TelemetryConfig};
pub use groups::{ExceptionGroup, ExceptionGroups};
pub use incident::{IncidentControlPlane, IncidentState};
pub use provider::ObservabilityProvider;
pub use runtime::{
    emit_global, emit_log_global, global_runtime, ObservabilityRuntime, RuntimeStatus,
    TelemetryControlState,
};

// Event-model re-exports (kelson-telemetry) so apps need only one dependency.
pub use kelson_foundation::TelemetryContext;
pub use kelson_telemetry::{
    Durability, Event, EventEnvelope, Fields, Level, LogPayload, ServiceIdentity, SpanKind,
    SpanPayload, SpanStatus, TelemetryCategory, TelemetryPayload,
};
// W3C trace-context helpers (S5 context propagation) so edge hosts need
// only kelson-observability.
pub use kelson_tracing::traceparent::{make_traceparent, parse_traceparent, Traceparent};

use kelson_foundation::{ExecutionId, RequestId, TenantId, TraceId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChainStage {
    Request,
    Execution,
    Authorization,
    Operation,
    Transaction,
    Sql,
    Delta,
    Audit,
    Outbox,
    Result,
}

/// One record in the diagnostic chain. Append-only; persisted by the postgres
/// adapter (G16) and inspectable through `kelson inspect execution <id>` /
/// `kelson observe request <id>`. With the S5 runtime active, published records
/// also flow through the telemetry bus (`ChainSink`), preserving the chain
/// structure (not flattened).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainRecord {
    pub id: Uuid,
    pub execution_id: ExecutionId,
    pub request_id: Option<RequestId>,
    pub tenant_id: Option<TenantId>,
    pub stage: ChainStage,
    pub detail: serde_json::Value,
    #[serde(with = "time::serde::iso8601")]
    pub at: OffsetDateTime,
}

impl ChainRecord {
    pub fn new(execution_id: ExecutionId, stage: ChainStage, detail: serde_json::Value) -> Self {
        Self {
            id: Uuid::now_v7(),
            execution_id,
            request_id: None,
            tenant_id: None,
            stage,
            detail,
            at: OffsetDateTime::now_utc(),
        }
    }
}

/// Bridge from the diagnostic chain to the telemetry pipeline: every record
/// becomes a structured event that flows through the same redaction,
/// sampling, routing and exporters. Falls back to plain `tracing::info!`
/// (which the S5 bridge also captures when installed).
pub struct ChainSink;

impl ChainSink {
    pub fn publish(record: &ChainRecord) {
        tracing::info!(
            stage = ?record.stage,
            execution_id = %record.execution_id,
            request_id = ?record.request_id,
            tenant_id = ?record.tenant_id,
            detail = %record.detail,
            "diagnostic.chain",
        );
    }
}

/// Standard tracing span fields — use everywhere for joinable logs.
pub fn span_fields(execution_id: ExecutionId, operation: &str) -> tracing::Span {
    tracing::info_span!(
        "execution",
        execution_id = %execution_id,
        operation = %operation,
    )
}

/// Execution span joined to the trace id (S1: trace_id on the context).
pub fn span_fields_traced(
    execution_id: ExecutionId,
    trace_id: TraceId,
    operation: &str,
) -> tracing::Span {
    tracing::info_span!(
        "execution",
        execution_id = %execution_id,
        trace_id = %trace_id,
        operation = %operation,
    )
}

/// Build a structured field bag: `fields!("order_id" => id, "total" => 42)`.
#[macro_export]
macro_rules! fields {
    () => { $crate::Fields::new() };
    ($($key:literal => $value:expr),+ $(,)?) => {{
        let mut map = $crate::Fields::new();
        $(
            map.insert(
                $key.to_string(),
                serde_json::to_value(&$value).unwrap_or(serde_json::Value::Null),
            );
        )+
        map
    }};
}

/// Emit a debug event through the global telemetry runtime (stderr fallback
/// when no runtime is running — messages are never silently lost).
#[macro_export]
macro_rules! tlog_debug {
    ($msg:expr $(,)?) => {
        $crate::emit_log_global($crate::Level::Debug, $msg, $crate::fields!())
    };
    ($msg:expr, $($key:literal => $value:expr),+ $(,)?) => {
        $crate::emit_log_global($crate::Level::Debug, $msg, $crate::fields!($($key => $value),+))
    };
}

#[macro_export]
macro_rules! tlog_info {
    ($msg:expr $(,)?) => {
        $crate::emit_log_global($crate::Level::Info, $msg, $crate::fields!())
    };
    ($msg:expr, $($key:literal => $value:expr),+ $(,)?) => {
        $crate::emit_log_global($crate::Level::Info, $msg, $crate::fields!($($key => $value),+))
    };
}

#[macro_export]
macro_rules! tlog_warn {
    ($msg:expr $(,)?) => {
        $crate::emit_log_global($crate::Level::Warn, $msg, $crate::fields!())
    };
    ($msg:expr, $($key:literal => $value:expr),+ $(,)?) => {
        $crate::emit_log_global($crate::Level::Warn, $msg, $crate::fields!($($key => $value),+))
    };
}

#[macro_export]
macro_rules! tlog_error {
    ($msg:expr $(,)?) => {
        $crate::emit_log_global($crate::Level::Error, $msg, $crate::fields!())
    };
    ($msg:expr, $($key:literal => $value:expr),+ $(,)?) => {
        $crate::emit_log_global($crate::Level::Error, $msg, $crate::fields!($($key => $value),+))
    };
}
