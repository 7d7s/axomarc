//! OTLP exporter (Sentinel S5, D-75): OTLP/HTTP with JSON encoding over
//! reqwest — no `opentelemetry*` dependencies (budget-safe). Logs and
//! exceptions go to `/v1/logs`; spans go to `/v1/traces`.

use std::sync::OnceLock;
use std::time::Duration;

use keel_telemetry::{Event, ExportError, ExportOk, Exporter, Level, SpanStatus, TelemetryPayload};

pub struct OtlpExporterConfig {
    /// Collector base URL, e.g. `http://localhost:4318`.
    pub endpoint: String,
    pub timeout: Duration,
}

pub struct OtlpExporter {
    cfg: OtlpExporterConfig,
    client: OnceLock<reqwest::blocking::Client>,
}

impl OtlpExporter {
    pub fn new(cfg: OtlpExporterConfig) -> Self {
        Self {
            cfg,
            client: OnceLock::new(),
        }
    }

    fn client(&self) -> &reqwest::blocking::Client {
        self.client.get_or_init(|| {
            reqwest::blocking::Client::builder()
                .timeout(self.cfg.timeout)
                .build()
                .expect("reqwest client builds (rustls, no TLS init failure)")
        })
    }

    fn post(&self, path: &str, body: &serde_json::Value) -> Result<usize, ExportError> {
        let raw = serde_json::to_vec(body)
            .map_err(|e| ExportError::Rejected(format!("otlp encode: {e}")))?;
        let len = raw.len();
        let url = format!("{}{path}", self.cfg.endpoint.trim_end_matches('/'));
        let resp = self
            .client()
            .post(&url)
            .header("Content-Type", "application/json")
            .body(raw)
            .send()
            .map_err(|e| ExportError::Transport(format!("otlp send: {e}")))?;
        let status = resp.status();
        if status.is_server_error() {
            return Err(ExportError::Transport(format!("otlp status {status}")));
        }
        if !status.is_success() {
            return Err(ExportError::Rejected(format!("otlp status {status}")));
        }
        Ok(len)
    }
}

fn hex_16(uuid: &uuid::Uuid) -> String {
    uuid.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

fn hex_8(uuid: &uuid::Uuid) -> String {
    uuid.as_bytes()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn kv(key: &str, value: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "key": key, "value": { "stringValue": value.to_string() } })
}

fn severity(level: Level) -> (u8, &'static str) {
    match level {
        Level::Debug => (5, "SEVERITY_NUMBER_DEBUG"),
        Level::Info => (9, "SEVERITY_NUMBER_INFO"),
        Level::Notice => (10, "SEVERITY_NUMBER_INFO2"),
        Level::Warn => (13, "SEVERITY_NUMBER_WARN"),
        Level::Error => (17, "SEVERITY_NUMBER_ERROR"),
        Level::Critical => (21, "SEVERITY_NUMBER_FATAL"),
    }
}

fn time_unix_nano(t: &time::OffsetDateTime) -> i128 {
    t.unix_timestamp_nanos()
}

fn log_record(event: &Event, body: String, level: Level) -> serde_json::Value {
    let (sev_number, sev_text) = severity(level);
    let mut record = serde_json::json!({
        "timeUnixNano": time_unix_nano(&event.at),
        "severityNumber": sev_number,
        "severityText": sev_text,
        "body": { "stringValue": body },
        "traceId": hex_16(event.context.trace_id.as_uuid()),
    });
    if let Some(span) = event.context.span_id {
        record["spanId"] = serde_json::json!(hex_8(span.as_uuid()));
    }
    let mut attrs = vec![
        kv("keel.schema", &serde_json::json!(event.schema)),
        kv("keel.service", &serde_json::json!(event.service.name)),
        kv("keel.env", &serde_json::json!(event.service.environment)),
    ];
    if let Some(tenant) = event.context.tenant_id {
        attrs.push(kv("keel.tenant", &serde_json::json!(tenant.to_string())));
    }
    if let Some(fields) = event.payload.fields_ref() {
        for (k, v) in fields.iter().take(32) {
            attrs.push(kv(k, v));
        }
    }
    record["attributes"] = serde_json::json!(attrs);
    record
}

impl Exporter for OtlpExporter {
    fn name(&self) -> &str {
        "otlp"
    }

    fn export(&self, batch: &[Event]) -> Result<ExportOk, ExportError> {
        let mut log_records: Vec<serde_json::Value> = Vec::new();
        let mut spans: Vec<serde_json::Value> = Vec::new();
        for event in batch {
            match &event.payload {
                TelemetryPayload::Log(l) => {
                    log_records.push(log_record(event, l.message.clone(), l.level));
                }
                TelemetryPayload::Exception(x) => {
                    log_records.push(log_record(
                        event,
                        format!("{}: {}", x.type_name, x.message),
                        Level::Error,
                    ));
                }
                TelemetryPayload::Span(s) => {
                    let start_nanos = time_unix_nano(&s.started_at);
                    let end_nanos = start_nanos + (s.duration_ms * 1_000_000.0) as i128;
                    spans.push(serde_json::json!({
                        "traceId": hex_16(event.context.trace_id.as_uuid()),
                        "spanId": hex_8(s.span_id.as_uuid()),
                        "parentSpanId": s.parent_span_id.map(|p| hex_8(p.as_uuid())),
                        "name": s.name,
                        "startTimeUnixNano": start_nanos,
                        "endTimeUnixNano": end_nanos,
                        "status": {
                            "code": match s.status {
                                SpanStatus::Ok => 1,
                                _ => 2,
                            }
                        },
                    }));
                }
                _ => {}
            }
        }
        let mut total = 0usize;
        if !log_records.is_empty() {
            let body = serde_json::json!({
                "resourceLogs": [{
                    "resource": { "attributes": [
                        { "key": "service.name", "value": { "stringValue": batch[0].service.name.clone() } },
                        { "key": "deployment.environment", "value": { "stringValue": batch[0].service.environment.clone() } }
                    ]},
                    "scopeLogs": [{ "logRecords": log_records }]
                }]
            });
            total += self.post("/v1/logs", &body)?;
        }
        if !spans.is_empty() {
            let body = serde_json::json!({
                "resourceSpans": [{
                    "resource": { "attributes": [
                        { "key": "service.name", "value": { "stringValue": batch[0].service.name.clone() } }
                    ]},
                    "scopeSpans": [{ "spans": spans }]
                }]
            });
            total += self.post("/v1/traces", &body)?;
        }
        Ok(ExportOk {
            bytes: total,
            events: batch.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_widths() {
        let u = uuid::Uuid::nil();
        assert_eq!(hex_16(&u).len(), 32);
        assert_eq!(hex_8(&u).len(), 16);
    }
}
