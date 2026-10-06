//! Telemetry propagation carrier (Sentinel S5, D-74).
//!
//! `TelemetryContext` is the only observability state allowed to cross layer
//! boundaries: it is plain typed data, so web/async/data/execution layers can
//! propagate trace/tenant/principal causality without depending on the
//! observability layer (dependency matrix, STAGE-0-SPEC §3). The observability
//! runtime converts `ExecutionContext` -> `TelemetryContext` at the boundary;
//! framework code never hand-sets tenant or principal on telemetry events.

use std::collections::BTreeMap;

use crate::{
    CorrelationId, ExecutionId, IncidentId, PrincipalId, RequestId, SpanId, TenantId, TraceId,
};

/// Maximum baggage entries carried across the wire (cardinality guard, S5).
pub const MAX_BAGGAGE_ENTRIES: usize = 32;

/// Immutable propagation snapshot for one causal flow. Cheap to clone, safe to
/// serialize into job metadata / HTTP extensions.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TelemetryContext {
    pub trace_id: TraceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<RequestId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_id: Option<ExecutionId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub span_id: Option<SpanId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_span_id: Option<SpanId>,
    /// None only for platform-plane work (provisioning, registry ops).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<TenantId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_id: Option<PrincipalId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<CorrelationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub causation_id: Option<CorrelationId>,
    /// Active incident marker (incident mode, D-76); stamped by the runtime,
    /// never by application code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub incident_id: Option<IncidentId>,
    /// Small bounded key/value map propagated with the context (deploy id,
    /// client hints). Insertions beyond `MAX_BAGGAGE_ENTRIES` are ignored.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub baggage: BTreeMap<String, String>,
}

impl Default for TelemetryContext {
    fn default() -> Self {
        Self::root()
    }
}

impl TelemetryContext {
    /// Fresh root context: new trace, no parent, no tenant.
    pub fn root() -> Self {
        Self {
            trace_id: TraceId::new(),
            request_id: None,
            execution_id: None,
            span_id: None,
            parent_span_id: None,
            tenant_id: None,
            principal_id: None,
            correlation_id: None,
            causation_id: None,
            incident_id: None,
            baggage: BTreeMap::new(),
        }
    }

    /// Context continuing an inbound trace (e.g. parsed `traceparent`).
    pub fn with_trace(trace_id: TraceId) -> Self {
        Self {
            trace_id,
            ..Self::root()
        }
    }

    pub fn with_request(mut self, request_id: RequestId) -> Self {
        self.request_id = Some(request_id);
        self
    }

    pub fn with_execution(mut self, execution_id: ExecutionId) -> Self {
        self.execution_id = Some(execution_id);
        self
    }

    pub fn with_tenant(mut self, tenant_id: TenantId) -> Self {
        self.tenant_id = Some(tenant_id);
        self
    }

    pub fn with_principal(mut self, principal_id: PrincipalId) -> Self {
        self.principal_id = Some(principal_id);
        self
    }

    pub fn with_correlation(mut self, correlation_id: CorrelationId) -> Self {
        self.correlation_id = Some(correlation_id);
        self
    }

    pub fn with_causation(mut self, causation_id: CorrelationId) -> Self {
        self.causation_id = Some(causation_id);
        self
    }

    /// Set baggage with the cardinality cap enforced (overflow ignored).
    pub fn with_baggage(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        if self.baggage.len() < MAX_BAGGAGE_ENTRIES {
            self.baggage.insert(key.into(), value.into());
        }
        self
    }

    /// Child span context: same trace, fresh span id, current span becomes parent.
    pub fn child_span(&self) -> Self {
        let mut child = self.clone();
        child.parent_span_id = self.span_id;
        child.span_id = Some(SpanId::new());
        child
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_context_has_fresh_trace() {
        let a = TelemetryContext::root();
        let b = TelemetryContext::root();
        assert_ne!(a.trace_id, b.trace_id);
        assert!(a.tenant_id.is_none() && a.span_id.is_none());
    }

    #[test]
    fn child_span_links_parent() {
        let root = TelemetryContext::root();
        let parent = TelemetryContext {
            span_id: Some(SpanId::new()),
            ..root.clone()
        };
        let child = parent.child_span();
        assert_eq!(child.trace_id, parent.trace_id);
        assert_eq!(child.parent_span_id, parent.span_id);
        assert_ne!(child.span_id, parent.span_id);
        // grandchild chains further
        let gc = child.child_span();
        assert_eq!(gc.parent_span_id, child.span_id);
    }

    #[test]
    fn baggage_is_capped() {
        let mut ctx = TelemetryContext::root();
        for i in 0..(MAX_BAGGAGE_ENTRIES + 10) {
            ctx = ctx.with_baggage(format!("k{i}"), "v");
        }
        assert_eq!(ctx.baggage.len(), MAX_BAGGAGE_ENTRIES);
    }

    #[test]
    fn serde_roundtrip_preserves_unknown_fields() {
        let ctx = TelemetryContext::root()
            .with_tenant(TenantId::new())
            .with_baggage("deploy", "dep_42");
        let mut json: serde_json::Value = serde_json::to_value(&ctx).unwrap();
        json.as_object_mut()
            .unwrap()
            .insert("future_field".into(), serde_json::json!(7));
        let back: TelemetryContext = serde_json::from_value(json).unwrap();
        assert_eq!(back.tenant_id, ctx.tenant_id);
        assert_eq!(back.baggage.get("deploy"), Some(&"dep_42".to_string()));
    }
}
