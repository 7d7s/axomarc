//! Telemetry event model (Sentinel S5 §2): everything becomes an event.
//! One envelope, seven payload families; the runtime bus carries `Event`.

use std::collections::BTreeMap;

use keel_foundation::{AuditId, SpanId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::envelope::{Durability, EventEnvelope};

/// Coarse routing family (the DAG routes on this before the payload body).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TelemetryCategory {
    Log,
    Span,
    Metric,
    Audit,
    Exception,
    Lifecycle,
    Security,
}

impl TelemetryCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            TelemetryCategory::Log => "log",
            TelemetryCategory::Span => "span",
            TelemetryCategory::Metric => "metric",
            TelemetryCategory::Audit => "audit",
            TelemetryCategory::Exception => "exception",
            TelemetryCategory::Lifecycle => "lifecycle",
            TelemetryCategory::Security => "security",
        }
    }
}

impl std::fmt::Display for TelemetryCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Severity ladder (RFC5424-flavoured, ordered).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Debug,
    Info,
    Notice,
    Warn,
    Error,
    Critical,
}

impl Level {
    pub fn parse(raw: &str) -> Option<Level> {
        match raw.to_ascii_lowercase().as_str() {
            "debug" | "trace" => Some(Level::Debug),
            "info" => Some(Level::Info),
            "notice" => Some(Level::Notice),
            "warn" | "warning" => Some(Level::Warn),
            "error" => Some(Level::Error),
            "critical" | "fatal" | "crit" => Some(Level::Critical),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Level::Debug => "debug",
            Level::Info => "info",
            Level::Notice => "notice",
            Level::Warn => "warn",
            Level::Error => "error",
            Level::Critical => "critical",
        }
    }
}

impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Structured, order-preserving field bag (JSON object).
pub type Fields = serde_json::Map<String, serde_json::Value>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogPayload {
    pub level: Level,
    pub message: String,
    /// Emitting module/target (e.g. `keel_api::routes`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    #[serde(default, skip_serializing_if = "Fields::is_empty")]
    pub fields: Fields,
    /// Unknown fields preserved across schema versions.
    #[serde(default, flatten, skip_serializing_if = "Fields::is_empty")]
    pub extra: Fields,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanKind {
    Request,
    Execution,
    Authorization,
    Operation,
    Transaction,
    Sql,
    Delta,
    Outbox,
    Job,
    Http,
    Cache,
    Notify,
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpanStatus {
    Ok,
    Error,
    Timeout,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpanPayload {
    pub span_id: SpanId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<SpanId>,
    pub name: String,
    pub kind: SpanKind,
    #[serde(with = "time::serde::iso8601")]
    pub started_at: OffsetDateTime,
    pub duration_ms: f64,
    pub status: SpanStatus,
    #[serde(default, skip_serializing_if = "Fields::is_empty")]
    pub fields: Fields,
    /// Unknown fields preserved across schema versions.
    #[serde(default, flatten, skip_serializing_if = "Fields::is_empty")]
    pub extra: Fields,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricKind {
    Counter,
    Gauge,
    HistogramBucket,
    HistogramSum,
    HistogramCount,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricPayload {
    pub name: String,
    pub kind: MetricKind,
    pub value: f64,
    /// Bounded labels (cardinality enforced by keel-metrics before emit).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub labels: BTreeMap<String, String>,
    /// Bucket upper bound for `HistogramBucket` metrics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub le: Option<f64>,
    #[serde(default, flatten, skip_serializing_if = "Fields::is_empty")]
    pub extra: Fields,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    Allowed,
    Denied,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditPayload {
    pub audit_id: AuditId,
    pub action: String,
    pub resource: String,
    pub authority: String,
    pub outcome: AuditOutcome,
    #[serde(default, skip_serializing_if = "Fields::is_empty")]
    pub fields: Fields,
    /// Unknown fields preserved across schema versions.
    #[serde(default, flatten, skip_serializing_if = "Fields::is_empty")]
    pub extra: Fields,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExceptionPayload {
    pub type_name: String,
    pub message: String,
    /// Stable grouping key (see `crate::fingerprint`).
    pub fingerprint: String,
    /// Top frames only (bounded).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub frames: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<String>,
    /// Aggregated occurrence count (exception grouping, §24).
    #[serde(default = "default_count")]
    pub count: u64,
    #[serde(default, flatten, skip_serializing_if = "Fields::is_empty")]
    pub extra: Fields,
}

fn default_count() -> u64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecyclePayload {
    /// `boot` | `shutdown` | `degraded` | `incident_start` | `incident_stop` ...
    pub kind: String,
    #[serde(default, skip_serializing_if = "Fields::is_empty")]
    pub fields: Fields,
    /// Unknown fields preserved across schema versions.
    #[serde(default, flatten, skip_serializing_if = "Fields::is_empty")]
    pub extra: Fields,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityPayload {
    /// `authz_denied` | `secret_redacted` | `policy_override` | `audit_tamper` ...
    pub kind: String,
    #[serde(default, skip_serializing_if = "Fields::is_empty")]
    pub fields: Fields,
    /// Unknown fields preserved across schema versions.
    #[serde(default, flatten, skip_serializing_if = "Fields::is_empty")]
    pub extra: Fields,
}

/// Tagged union of all payload families.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TelemetryPayload {
    Log(LogPayload),
    Span(SpanPayload),
    Metric(MetricPayload),
    Audit(AuditPayload),
    Exception(ExceptionPayload),
    Lifecycle(LifecyclePayload),
    Security(SecurityPayload),
}

impl TelemetryPayload {
    pub fn category(&self) -> TelemetryCategory {
        match self {
            TelemetryPayload::Log(_) => TelemetryCategory::Log,
            TelemetryPayload::Span(_) => TelemetryCategory::Span,
            TelemetryPayload::Metric(_) => TelemetryCategory::Metric,
            TelemetryPayload::Audit(_) => TelemetryCategory::Audit,
            TelemetryPayload::Exception(_) => TelemetryCategory::Exception,
            TelemetryPayload::Lifecycle(_) => TelemetryCategory::Lifecycle,
            TelemetryPayload::Security(_) => TelemetryCategory::Security,
        }
    }

    /// Read access to the field bag (for policy guards and exporters).
    pub fn fields_ref(&self) -> Option<&Fields> {
        match self {
            TelemetryPayload::Log(p) => Some(&p.fields),
            TelemetryPayload::Span(p) => Some(&p.fields),
            TelemetryPayload::Audit(p) => Some(&p.fields),
            TelemetryPayload::Lifecycle(p) => Some(&p.fields),
            TelemetryPayload::Security(p) => Some(&p.fields),
            TelemetryPayload::Metric(_) | TelemetryPayload::Exception(_) => None,
        }
    }

    /// Mutable access to the field bag (for redact/enrich processors).
    pub fn fields_mut(&mut self) -> Option<&mut Fields> {
        match self {
            TelemetryPayload::Log(p) => Some(&mut p.fields),
            TelemetryPayload::Span(p) => Some(&mut p.fields),
            TelemetryPayload::Audit(p) => Some(&mut p.fields),
            TelemetryPayload::Lifecycle(p) => Some(&mut p.fields),
            TelemetryPayload::Security(p) => Some(&mut p.fields),
            TelemetryPayload::Metric(_) | TelemetryPayload::Exception(_) => None,
        }
    }

    pub fn level_hint(&self) -> Option<Level> {
        match self {
            TelemetryPayload::Log(p) => Some(p.level),
            TelemetryPayload::Exception(_) => Some(Level::Error),
            TelemetryPayload::Span(p) => match p.status {
                SpanStatus::Ok => None,
                SpanStatus::Error => Some(Level::Error),
                SpanStatus::Timeout => Some(Level::Warn),
                SpanStatus::Cancelled => Some(Level::Debug),
            },
            TelemetryPayload::Security(_) => Some(Level::Warn),
            _ => None,
        }
    }

    /// Default durability per family (overridable by policy).
    pub fn default_durability(&self) -> Durability {
        match self {
            TelemetryPayload::Log(p) if p.level >= Level::Error => Durability::Durable,
            TelemetryPayload::Log(p) if p.level == Level::Debug => Durability::Ephemeral,
            TelemetryPayload::Audit(_)
            | TelemetryPayload::Exception(_)
            | TelemetryPayload::Security(_) => Durability::Durable,
            _ => Durability::Buffered,
        }
    }
}

/// The runtime event: envelope + concrete payload union.
pub type Event = EventEnvelope<TelemetryPayload>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{ServiceIdentity, SCHEMA_LOG, SCHEMA_LOG_VERSION};
    use keel_foundation::TelemetryContext;

    fn log_event(level: Level, msg: &str) -> Event {
        EventEnvelope::new(
            SCHEMA_LOG,
            SCHEMA_LOG_VERSION,
            ServiceIdentity::new("test", "dev"),
            TelemetryContext::root(),
            TelemetryPayload::Log(LogPayload {
                level,
                message: msg.into(),
                target: None,
                fields: Fields::new(),
                extra: Fields::new(),
            }),
        )
    }

    #[test]
    fn level_ordering_and_parse() {
        assert!(Level::Error > Level::Warn && Level::Critical > Level::Error);
        assert_eq!(Level::parse("WARNING"), Some(Level::Warn));
        assert_eq!(Level::parse("fatal"), Some(Level::Critical));
        assert_eq!(Level::parse("nope"), None);
    }

    #[test]
    fn category_and_durability_defaults() {
        let err = log_event(Level::Error, "boom");
        assert_eq!(err.payload.category(), TelemetryCategory::Log);
        assert_eq!(err.payload.default_durability(), Durability::Durable);
        let dbg = log_event(Level::Debug, "chatter");
        assert_eq!(dbg.payload.default_durability(), Durability::Ephemeral);
        let info = log_event(Level::Info, "ok");
        assert_eq!(info.payload.default_durability(), Durability::Buffered);
    }

    #[test]
    fn payload_tagged_serde() {
        let e = log_event(Level::Info, "hi");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["payload"]["kind"], "log");
        let back: Event = serde_json::from_value(v).unwrap();
        assert!(matches!(back.payload, TelemetryPayload::Log(_)));
    }
}
