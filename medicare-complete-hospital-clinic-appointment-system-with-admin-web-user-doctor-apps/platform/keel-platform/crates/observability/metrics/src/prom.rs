//! Prometheus text exposition (hand-rolled, no deps; closes gap B-17).

use crate::registry::MetricsSnapshot;

/// Sanitize a metric name for Prometheus (`[a-zA-Z_:][a-zA-Z0-9_:]*`).
pub fn sanitize_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for (i, ch) in raw.chars().enumerate() {
        let ok = ch.is_ascii_alphanumeric() || ch == '_' || ch == ':';
        if i == 0 && ch.is_ascii_digit() {
            out.push('_');
        }
        out.push(if ok { ch } else { '_' });
    }
    out
}

fn render_labels(labels: &std::collections::BTreeMap<String, String>) -> String {
    if labels.is_empty() {
        return String::new();
    }
    let inner: Vec<String> = labels
        .iter()
        .map(|(k, v)| {
            format!(
                "{}=\"{}\"",
                sanitize_name(k),
                v.replace('\\', "\\\\").replace('"', "\\\"")
            )
        })
        .collect();
    format!("{{{}}}", inner.join(","))
}

/// Render a snapshot in the Prometheus 0.0.4 text format.
pub fn render_prometheus(snapshot: &MetricsSnapshot) -> String {
    let mut out = String::new();
    let mut seen_type: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for (key, value) in &snapshot.counters {
        let name = sanitize_name(&key.name);
        if seen_type.insert(format!("{name}_c")) {
            out.push_str(&format!("# TYPE {name} counter\n"));
        }
        out.push_str(&format!("{name}{} {value}\n", render_labels(&key.labels)));
    }
    for (key, value) in &snapshot.gauges {
        let name = sanitize_name(&key.name);
        if seen_type.insert(format!("{name}_g")) {
            out.push_str(&format!("# TYPE {name} gauge\n"));
        }
        out.push_str(&format!("{name}{} {value}\n", render_labels(&key.labels)));
    }
    for (name, hist) in &snapshot.histograms {
        let name = sanitize_name(name);
        out.push_str(&format!("# TYPE {name} histogram\n"));
        for (bound, count) in &hist.buckets {
            out.push_str(&format!("{name}_bucket{{le=\"{bound}\"}} {count}\n"));
        }
        out.push_str(&format!("{name}_bucket{{le=\"+Inf\"}} {}\n", hist.count));
        out.push_str(&format!("{name}_sum {}\n", hist.sum));
        out.push_str(&format!("{name}_count {}\n", hist.count));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::MetricsRegistry;
    use std::collections::BTreeMap;

    #[test]
    fn names_and_escaping() {
        assert_eq!(sanitize_name("http.requests-total"), "http_requests_total");
        assert_eq!(sanitize_name("9bad"), "_9bad");
        let mut l = BTreeMap::new();
        l.insert("route".into(), r#"say "hi""#.into());
        assert!(render_labels(&l).contains("\\\"hi\\\""));
    }

    #[test]
    fn renders_counter_gauge_histogram() {
        let r = MetricsRegistry::new(10);
        r.inc("requests", BTreeMap::new(), 3.0);
        r.set_gauge("queue_depth", BTreeMap::new(), 7.0);
        r.observe_ms("latency", 12.0);
        let text = render_prometheus(&r.snapshot());
        assert!(text.contains("# TYPE requests counter"));
        assert!(text.contains("requests 3"));
        assert!(text.contains("# TYPE queue_depth gauge"));
        assert!(text.contains("# TYPE latency histogram"));
        assert!(text.contains("latency_count 1"));
    }
}
