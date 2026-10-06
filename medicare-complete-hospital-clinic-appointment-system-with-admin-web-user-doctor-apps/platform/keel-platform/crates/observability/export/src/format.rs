//! Shared formatting + line-level defense-in-depth (log-injection escaping,
//! secret-key redaction on rendered lines).

use keel_telemetry::{Event, TelemetryPayload};

/// Escape CR/LF so one event can never forge additional console/file lines.
pub fn sanitize_text(raw: &str) -> String {
    raw.replace(['\r', '\n'], " ")
}

/// Redact `key: value` / `"key":"value"` pairs on a rendered line for the
/// kernel secret field list (second line of defense after the redact
/// processor already masked structured fields).
pub fn redact_line(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let lower = line.to_ascii_lowercase();
    let bytes = line.as_bytes();
    let fields: Vec<&str> = keel_telemetry::DEFAULT_SECRET_FIELDS.to_vec();
    let mut i = 0usize;
    while i < bytes.len() {
        let boundary = i == 0 || matches!(bytes[i - 1], b'{' | b',' | b'"' | b' ' | b'(');
        let mut matched: Option<usize> = None;
        if boundary {
            for f in &fields {
                if i + f.len() < bytes.len() && &lower[i..i + f.len()] == *f {
                    let after = bytes[i + f.len()];
                    let sep_ok = after == b':'
                        || after == b'='
                        || (after == b'"'
                            && i + f.len() + 1 < bytes.len()
                            && bytes[i + f.len() + 1] == b':');
                    if sep_ok {
                        matched = Some(f.len());
                        break;
                    }
                }
            }
        }
        match matched {
            Some(len) => {
                out.push_str(&line[i..i + len]);
                i += len;
                while i < bytes.len() && matches!(bytes[i], b':' | b' ' | b'"' | b'=') {
                    out.push(bytes[i] as char);
                    i += 1;
                    if i > 0
                        && (bytes[i - 1] == b'='
                            || (bytes[i - 1] == b':' && !matches!(bytes.get(i), Some(b' '))))
                    {
                        break;
                    }
                }
                if i < bytes.len() && bytes[i] == b'"' {
                    out.push_str("\"[REDACTED]\"");
                    i += 1;
                    while i < bytes.len() && bytes[i] != b'"' {
                        if bytes[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                    if i < bytes.len() {
                        i += 1;
                    }
                } else {
                    out.push_str("[REDACTED]");
                    while i < bytes.len() && !matches!(bytes[i], b',' | b'}' | b' ' | b'\n') {
                        i += 1;
                    }
                }
            }
            None => {
                out.push(line[i..].chars().next().unwrap());
                i += line[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            }
        }
    }
    out
}

/// One JSONL line for an event (compact schema-aware rendering).
pub fn event_to_jsonl(event: &Event) -> String {
    let mut root = serde_json::Map::new();
    root.insert(
        "ts".into(),
        serde_json::to_value(event.at).unwrap_or_default(),
    );
    root.insert("id".into(), serde_json::json!(event.id.to_string()));
    root.insert("schema".into(), serde_json::json!(event.schema));
    root.insert(
        "category".into(),
        serde_json::json!(event.payload.category().as_str()),
    );
    root.insert("env".into(), serde_json::json!(event.service.environment));
    root.insert("service".into(), serde_json::json!(event.service.name));
    root.insert(
        "trace_id".into(),
        serde_json::json!(event.context.trace_id.to_string()),
    );
    if let Some(t) = event.context.tenant_id {
        root.insert("tenant_id".into(), serde_json::json!(t.to_string()));
    }
    if let Some(r) = event.context.request_id {
        root.insert("request_id".into(), serde_json::json!(r.to_string()));
    }
    if let Some(incident) = event.context.incident_id {
        root.insert(
            "incident_id".into(),
            serde_json::json!(incident.to_string()),
        );
    }
    match &event.payload {
        TelemetryPayload::Log(l) => {
            root.insert("level".into(), serde_json::json!(l.level.as_str()));
            root.insert(
                "message".into(),
                serde_json::json!(sanitize_text(&l.message)),
            );
            if let Some(t) = &l.target {
                root.insert("target".into(), serde_json::json!(t));
            }
            if !l.fields.is_empty() {
                root.insert("fields".into(), serde_json::Value::Object(l.fields.clone()));
            }
        }
        TelemetryPayload::Span(s) => {
            root.insert("level".into(), serde_json::json!("info"));
            root.insert(
                "message".into(),
                serde_json::json!(format!(
                    "span {} {:.1}ms [{:?}]",
                    sanitize_text(&s.name),
                    s.duration_ms,
                    s.status
                )),
            );
            root.insert("span_id".into(), serde_json::json!(s.span_id.to_string()));
            root.insert("duration_ms".into(), serde_json::json!(s.duration_ms));
        }
        TelemetryPayload::Metric(m) => {
            root.insert("level".into(), serde_json::json!("info"));
            root.insert("metric".into(), serde_json::json!(m.name));
            root.insert("value".into(), serde_json::json!(m.value));
        }
        TelemetryPayload::Audit(a) => {
            root.insert("level".into(), serde_json::json!("notice"));
            root.insert(
                "message".into(),
                serde_json::json!(format!(
                    "audit {} {} {:?} -> {:?}",
                    sanitize_text(&a.action),
                    sanitize_text(&a.resource),
                    a.outcome,
                    sanitize_text(&a.authority)
                )),
            );
        }
        TelemetryPayload::Exception(x) => {
            root.insert("level".into(), serde_json::json!("error"));
            root.insert(
                "message".into(),
                serde_json::json!(format!(
                    "{}: {}",
                    sanitize_text(&x.type_name),
                    sanitize_text(&x.message)
                )),
            );
            root.insert("fingerprint".into(), serde_json::json!(x.fingerprint));
        }
        TelemetryPayload::Lifecycle(l) => {
            root.insert("level".into(), serde_json::json!("notice"));
            root.insert(
                "message".into(),
                serde_json::json!(format!("lifecycle.{}", l.kind)),
            );
        }
        TelemetryPayload::Security(s) => {
            root.insert("level".into(), serde_json::json!("warn"));
            root.insert(
                "message".into(),
                serde_json::json!(format!("security.{}", s.kind)),
            );
        }
    }
    serde_json::to_string(&serde_json::Value::Object(root)).unwrap_or_default()
}

/// Human-readable pretty line (dev console).
pub fn event_to_pretty(event: &Event) -> String {
    let level = event
        .payload
        .level_hint()
        .map(|l| l.as_str().to_uppercase())
        .unwrap_or_else(|| "INFO".into());
    let ts = event
        .at
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let message = match &event.payload {
        TelemetryPayload::Log(l) => sanitize_text(&l.message),
        TelemetryPayload::Span(s) => {
            format!("span {} {:.1}ms", sanitize_text(&s.name), s.duration_ms)
        }
        TelemetryPayload::Metric(m) => format!("metric {}={}", m.name, m.value),
        TelemetryPayload::Audit(a) => format!(
            "audit {} {}",
            sanitize_text(&a.action),
            sanitize_text(&a.resource)
        ),
        TelemetryPayload::Exception(x) => format!(
            "{}: {}",
            sanitize_text(&x.type_name),
            sanitize_text(&x.message)
        ),
        TelemetryPayload::Lifecycle(l) => format!("lifecycle.{}", l.kind),
        TelemetryPayload::Security(s) => format!("security.{}", s.kind),
    };
    format!("{ts} {level:<8} {message}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_removes_newlines() {
        assert_eq!(sanitize_text("a\r\nb\nc"), "a  b c");
    }

    #[test]
    fn redact_line_masks_json_and_pretty() {
        let l1 = r#"{"password":"hunter2","user":"alice"}"#;
        let r1 = redact_line(l1);
        assert!(r1.contains(r#""password":"[REDACTED]""#), "got {r1}");
        assert!(r1.contains(r#""user":"alice""#));
        let l2 = "start token=abc123 end";
        let r2 = redact_line(l2);
        assert!(r2.contains("token=[REDACTED]"), "got {r2}");
        assert!(!r2.contains("abc123"));
    }
}
