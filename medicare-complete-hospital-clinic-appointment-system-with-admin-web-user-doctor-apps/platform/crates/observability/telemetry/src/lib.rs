//! # kelson-telemetry
//! Sentinel (S5) telemetry event model: versioned envelopes, the seven event
//! families (log/span/metric/audit/exception/lifecycle/security), typed UUIDv7
//! ids (via kelson-foundation), classification vocabulary, exception fingerprinting and
//! the plugin registration surface. This crate is the contract; the runtime in
//! `kelson-observability` wires it to pipelines, buffers and exporters.

pub mod classify;
pub mod envelope;
pub mod event;
pub mod fingerprint;
pub mod plugin;

pub use classify::{
    classify_fields, is_pii_field, is_secret_field, Sensitivity, DEFAULT_PII_FIELDS,
    DEFAULT_SECRET_FIELDS,
};
pub use envelope::{
    Durability, EventEnvelope, ServiceIdentity, SCHEMA_AUDIT, SCHEMA_AUDIT_VERSION,
    SCHEMA_EXCEPTION, SCHEMA_EXCEPTION_VERSION, SCHEMA_LIFECYCLE, SCHEMA_LIFECYCLE_VERSION,
    SCHEMA_LOG, SCHEMA_LOG_VERSION, SCHEMA_METRIC, SCHEMA_METRIC_VERSION, SCHEMA_SECURITY,
    SCHEMA_SECURITY_VERSION, SCHEMA_SPAN, SCHEMA_SPAN_VERSION,
};
pub use event::{
    AuditOutcome, AuditPayload, Event, ExceptionPayload, Fields, Level, LifecyclePayload,
    LogPayload, MetricKind, MetricPayload, SecurityPayload, SpanKind, SpanPayload, SpanStatus,
    TelemetryCategory, TelemetryPayload,
};
pub use fingerprint::{exception_fingerprint, normalize_component, MAX_FINGERPRINT_FRAMES};
pub use plugin::{
    ExportError, ExportOk, Exporter, ProcessOutcome, Processor, RegisteredExporter,
    TelemetryPlugin, TelemetryRegistry,
};
