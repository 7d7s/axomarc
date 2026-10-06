//! Event envelope (Sentinel S5 §3/§4): every telemetry event is wrapped in a
//! versioned envelope carrying identity, deployment, context snapshot,
//! durability and classification metadata. Schemas evolve additively only —
//! unknown fields survive round-trips (forward compatibility).

use crate::classify::Sensitivity;
use keel_foundation::{EventId, TelemetryContext};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Current schema versions (bump on additive change; never reuse a number).
pub const SCHEMA_LOG: &str = "keel.log";
pub const SCHEMA_LOG_VERSION: u16 = 1;
pub const SCHEMA_SPAN: &str = "keel.span";
pub const SCHEMA_SPAN_VERSION: u16 = 1;
pub const SCHEMA_METRIC: &str = "keel.metric";
pub const SCHEMA_METRIC_VERSION: u16 = 1;
pub const SCHEMA_AUDIT: &str = "keel.audit";
pub const SCHEMA_AUDIT_VERSION: u16 = 1;
pub const SCHEMA_EXCEPTION: &str = "keel.exception";
pub const SCHEMA_EXCEPTION_VERSION: u16 = 1;
pub const SCHEMA_LIFECYCLE: &str = "keel.lifecycle";
pub const SCHEMA_LIFECYCLE_VERSION: u16 = 1;
pub const SCHEMA_SECURITY: &str = "keel.security";
pub const SCHEMA_SECURITY_VERSION: u16 = 1;

/// How hard the runtime must try to keep an event (§18 durability classes).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Durability {
    /// May be dropped under any pressure (debug chatter).
    Ephemeral,
    /// In-memory buffering; survives exporter blips, not process death.
    #[default]
    Buffered,
    /// WAL-backed; survives process restart (audit, errors, security).
    Durable,
}

/// Who/where this event came from (deployment identity, §3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceIdentity {
    pub name: String,
    pub environment: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl ServiceIdentity {
    pub fn new(name: impl Into<String>, environment: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            environment: environment.into(),
            deployment: None,
            version: None,
        }
    }
}

/// Standard envelope for every telemetry event. Generic over payload so the
/// pipeline can also carry typed subsets; the runtime uses `Event`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventEnvelope<T> {
    /// Unknown fields preserved across schema versions (S5 section 4).
    #[serde(default, flatten, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
    /// Globally unique, time-ordered (UUIDv7).
    pub id: EventId,
    #[serde(with = "time::serde::iso8601")]
    pub at: OffsetDateTime,
    /// Stable event schema name (e.g. `keel.log`).
    pub schema: String,
    pub schema_version: u16,
    pub service: ServiceIdentity,
    /// Propagation snapshot stamped by the runtime (never hand-edited).
    pub context: TelemetryContext,
    pub durability: Durability,
    /// Set by the classify processor; consumed by policy and exporters.
    #[serde(default)]
    pub sensitivity: Sensitivity,
    pub payload: T,
}

impl<T> EventEnvelope<T> {
    pub fn new(
        schema: impl Into<String>,
        schema_version: u16,
        service: ServiceIdentity,
        context: TelemetryContext,
        payload: T,
    ) -> Self {
        Self {
            extra: Default::default(),
            id: EventId::new(),
            at: OffsetDateTime::now_utc(),
            schema: schema.into(),
            schema_version,
            service,
            context,
            durability: Durability::default(),
            sensitivity: Sensitivity::default(),
            payload,
        }
    }

    pub fn with_durability(mut self, durability: Durability) -> Self {
        self.durability = durability;
        self
    }

    /// Convert the payload while preserving identity/metadata.
    pub fn map_payload<U>(self, f: impl FnOnce(T) -> U) -> EventEnvelope<U> {
        EventEnvelope {
            extra: self.extra,
            id: self.id,
            at: self.at,
            schema: self.schema,
            schema_version: self.schema_version,
            service: self.service,
            context: self.context,
            durability: self.durability,
            sensitivity: self.sensitivity,
            payload: f(self.payload),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_roundtrip_preserves_unknown_fields() {
        let env = EventEnvelope::new(
            SCHEMA_LOG,
            SCHEMA_LOG_VERSION,
            ServiceIdentity::new("keel-api", "dev"),
            TelemetryContext::root(),
            serde_json::json!({ "message": "hello" }),
        );
        let mut value = serde_json::to_value(&env).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("future_field".into(), serde_json::json!({"x": 1}));
        let back: EventEnvelope<serde_json::Value> = serde_json::from_value(value).unwrap();
        assert_eq!(back.id, env.id);
        assert_eq!(back.schema, SCHEMA_LOG);
        let re = serde_json::to_value(&back).unwrap();
        assert_eq!(re.get("future_field"), Some(&serde_json::json!({"x": 1})));
    }

    #[test]
    fn durability_serializes_stable() {
        assert_eq!(
            serde_json::to_string(&Durability::Durable).unwrap(),
            "\"durable\""
        );
    }
}
