//! Tracing bridge (Sentinel S5 §30 abstraction boundary): every `tracing::*`
//! event and span in the process reaches the telemetry bus, so applications
//! never care where events land. Internal runtime targets are skipped to
//! prevent recursion; formatting/redaction happens in the exporters.

use std::sync::Arc;
use std::time::Instant;

use kelson_telemetry::{
    EventEnvelope, Fields, Level, LogPayload, SpanKind, SpanPayload, SpanStatus, TelemetryPayload,
    SCHEMA_LOG, SCHEMA_LOG_VERSION, SCHEMA_SPAN, SCHEMA_SPAN_VERSION,
};
use tracing::field::{Field, Visit};
use tracing::{span, Event, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

use crate::runtime::ObservabilityRuntime;

/// Targets emitted by the telemetry runtime itself (recursion guard).
const INTERNAL_TARGETS: &[&str] = &[
    "kelson_observability",
    "kelson_telemetry",
    "kelson_pipeline",
    "kelson_buffer",
    "kelson_export",
];

fn is_internal(target: &str) -> bool {
    INTERNAL_TARGETS
        .iter()
        .any(|prefix| target == *prefix || target.starts_with(&format!("{prefix}::")))
}

struct SpanData {
    name: String,
    started_at: time::OffsetDateTime,
    start: Instant,
    fields: Fields,
}

struct FieldCollector<'a> {
    fields: &'a mut Fields,
}

impl<'a> Visit for FieldCollector<'a> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let name = field.name();
        let rendered = format!("{value:?}");
        // `message` arrives quoted via Debug; strip the outer quotes.
        let cleaned = rendered
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .map(|s| s.to_string())
            .unwrap_or(rendered);
        self.fields
            .insert(name.to_string(), serde_json::Value::String(cleaned));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.fields.insert(
            field.name().to_string(),
            serde_json::Value::String(value.into()),
        );
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.fields
            .insert(field.name().to_string(), serde_json::Value::Bool(value));
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.fields.insert(
            field.name().to_string(),
            serde_json::Value::Number(value.into()),
        );
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.fields.insert(
            field.name().to_string(),
            serde_json::Value::Number(value.into()),
        );
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        if let Some(number) = serde_json::Number::from_f64(value) {
            self.fields
                .insert(field.name().to_string(), serde_json::Value::Number(number));
        }
    }

    fn record_error(&mut self, field: &Field, value: &(dyn std::error::Error + 'static)) {
        self.fields.insert(
            field.name().to_string(),
            serde_json::Value::String(value.to_string()),
        );
    }
}

fn map_level(level: &tracing::Level) -> Level {
    match *level {
        tracing::Level::TRACE | tracing::Level::DEBUG => Level::Debug,
        tracing::Level::INFO => Level::Info,
        tracing::Level::WARN => Level::Warn,
        tracing::Level::ERROR => Level::Error,
    }
}

fn span_kind(name: &str) -> SpanKind {
    match name {
        "request" => SpanKind::Request,
        "execution" => SpanKind::Execution,
        "authorization" => SpanKind::Authorization,
        "transaction" => SpanKind::Transaction,
        "sql" | "query" => SpanKind::Sql,
        "job" => SpanKind::Job,
        "http" | "http_request" => SpanKind::Http,
        "cache" => SpanKind::Cache,
        _ => SpanKind::Internal,
    }
}

pub struct TelemetryBridge {
    runtime: Arc<ObservabilityRuntime>,
}

impl TelemetryBridge {
    pub fn new(runtime: Arc<ObservabilityRuntime>) -> Self {
        Self { runtime }
    }

    fn emit_log(&self, target: &str, level: Level, fields: Fields) {
        let message = fields
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let mut fields = fields;
        fields.remove("message");
        let event = EventEnvelope::new(
            SCHEMA_LOG,
            SCHEMA_LOG_VERSION,
            self.runtime.service().clone(),
            kelson_foundation::TelemetryContext::root(),
            TelemetryPayload::Log(LogPayload {
                level,
                message,
                target: Some(target.to_string()),
                fields,
                extra: Fields::new(),
            }),
        );
        self.runtime.emit(event);
    }

    fn current_span_name<S>(&self, ctx: &Context<'_, S>) -> Option<String>
    where
        S: Subscriber + for<'a> LookupSpan<'a>,
    {
        ctx.lookup_current().map(|span| span.name().to_string())
    }
}

impl<S> Layer<S> for TelemetryBridge
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &span::Attributes<'_>, id: &span::Id, ctx: Context<'_, S>) {
        let metadata = attrs.metadata();
        if is_internal(metadata.target()) {
            return;
        }
        let mut fields = Fields::new();
        attrs.record(&mut FieldCollector {
            fields: &mut fields,
        });
        let data = SpanData {
            name: metadata.name().to_string(),
            started_at: time::OffsetDateTime::now_utc(),
            start: Instant::now(),
            fields,
        };
        if let Some(span) = ctx.span(id) {
            span.extensions_mut().insert(data);
        }
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let metadata = event.metadata();
        if is_internal(metadata.target()) {
            return;
        }
        let mut fields = Fields::new();
        event.record(&mut FieldCollector {
            fields: &mut fields,
        });
        if let Some(span_name) = self.current_span_name(&ctx) {
            fields
                .entry("span".to_string())
                .or_insert(serde_json::Value::String(span_name));
        }
        self.emit_log(metadata.target(), map_level(metadata.level()), fields);
    }

    fn on_close(&self, id: span::Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(&id) else {
            return;
        };
        let Some(data) = span.extensions_mut().remove::<SpanData>() else {
            return; // internal span
        };
        let duration = data.start.elapsed();
        let status = if data
            .fields
            .get("error")
            .map(|v| v == &serde_json::Value::Bool(true) || v.as_str().is_some())
            .unwrap_or(false)
        {
            SpanStatus::Error
        } else {
            SpanStatus::Ok
        };
        let event = EventEnvelope::new(
            SCHEMA_SPAN,
            SCHEMA_SPAN_VERSION,
            self.runtime.service().clone(),
            kelson_foundation::TelemetryContext::root(),
            TelemetryPayload::Span(SpanPayload {
                span_id: kelson_foundation::SpanId::new(),
                parent_span_id: None,
                name: data.name.clone(),
                kind: span_kind(&data.name),
                started_at: data.started_at,
                duration_ms: duration.as_secs_f64() * 1000.0,
                status,
                fields: data.fields,
                extra: Fields::new(),
            }),
        );
        self.runtime.emit(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_targets_are_skipped() {
        assert!(is_internal("kelson_observability::runtime"));
        assert!(is_internal("kelson_export"));
        assert!(!is_internal("kelson_demo::routes"));
        assert!(!is_internal("my_app"));
    }
}
