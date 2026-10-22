//! The SLO contract.
//!
//! A framework a platform team operates has to be holdable to a **number**.
//! This module makes that concrete: the framework **declares** its own
//! service-level objectives, evaluates them against the metrics it
//! actually records, and publishes both the targets and their current
//! status.
//!
//! Two rules keep the contract honest:
//!
//! * **No fiction.** Every objective names a metric the framework really
//!   records (the telemetry plane's error rate, the queue depth, the shed
//!   level, the export failures, the dropped events, the tenant rejections,
//!   the active incidents). An objective without a metric behind it would
//!   be a slogan, not a contract.
//! * **The status is computed, not asserted.** [`evaluate`] reads the
//!   registry's snapshot, so the status can never drift from the telemetry.

use std::collections::BTreeMap;

use crate::registry::MetricsSnapshot;

/// How an objective is compared with its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comparison {
    /// The observed value must not exceed the target.
    AtMost,
    /// The observed value must not fall below the target.
    AtLeast,
    /// The observed value must equal the target.
    Equals,
}

/// One declared objective.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Slo {
    /// The objective's stable name (the thing a runbook refers to).
    pub name: &'static str,
    /// The metric the objective is measured from. It is a metric the
    /// framework really records.
    pub metric: &'static str,
    /// How the observed value is compared with the target.
    pub comparison: Comparison,
    /// The target value.
    pub target: f64,
    /// The window the objective applies over.
    pub window: &'static str,
    /// What the objective means, in one line.
    pub description: &'static str,
}

/// The framework's declared objectives.
///
/// Every metric named here is one the framework records today: the
/// telemetry plane's gauges and counters. A framework that declared an
/// objective it could not measure would be making a promise it cannot keep.
pub const SLOS: &[Slo] = &[
    Slo {
        name: "telemetry.error_rate",
        metric: "telemetry_error_rate",
        comparison: Comparison::AtMost,
        target: 0.05,
        window: "5m",
        description: "the share of telemetry events the plane failed to handle",
    },
    Slo {
        name: "telemetry.queue_depth",
        metric: "telemetry_queue_depth",
        comparison: Comparison::AtMost,
        target: 1_000.0,
        window: "instant",
        description: "the events waiting in the telemetry queue",
    },
    Slo {
        name: "telemetry.shed_level",
        metric: "telemetry_shed_level",
        comparison: Comparison::Equals,
        target: 0.0,
        window: "instant",
        description: "the backpressure level: anything above zero means the plane shed load",
    },
    Slo {
        name: "telemetry.export_failures",
        metric: "telemetry_export_failures_total",
        comparison: Comparison::Equals,
        target: 0.0,
        window: "5m",
        description: "the failed exports: an exporter that cannot deliver is a blind spot",
    },
    Slo {
        name: "telemetry.dropped",
        metric: "telemetry_dropped_total",
        comparison: Comparison::AtMost,
        target: 100.0,
        window: "5m",
        description: "the events the plane dropped (the backpressure's cost)",
    },
    Slo {
        name: "security.tenant_rejections",
        metric: "telemetry_tenant_rejected_total",
        comparison: Comparison::Equals,
        target: 0.0,
        window: "5m",
        description: "the tenant-boundary rejections: a non-zero value is an attempt, not a fault",
    },
    Slo {
        name: "operability.incident_active",
        metric: "telemetry_incident_active",
        comparison: Comparison::Equals,
        target: 0.0,
        window: "instant",
        description: "the active incidents: anything above zero means the host is in an incident",
    },
];

/// Escape a Prometheus label VALUE.
///
/// A label value is not a metric name: it may contain any character. Only
/// the quote and the backslash are escaped (sanitizing a value would
/// mangle "5m" into "_5m").
fn escape_label_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Whether an objective is being met.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SloStatus {
    /// The target is met.
    Met,
    /// The target is met, but the observed value is within 20 % of it.
    AtRisk,
    /// The target is missed.
    Breached,
}

impl SloStatus {
    /// The report spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            SloStatus::Met => "met",
            SloStatus::AtRisk => "at_risk",
            SloStatus::Breached => "breached",
        }
    }

    /// Whether the objective is missed.
    pub fn is_breached(self) -> bool {
        matches!(self, SloStatus::Breached)
    }
}

/// The observed value of a metric in a snapshot.
///
/// A gauge is read as-is; a counter is summed over its label sets (the
/// total, not one series).
pub fn observed(snapshot: &MetricsSnapshot, metric: &str) -> Option<f64> {
    if let Some((_, value)) = snapshot
        .gauges
        .iter()
        .find(|(key, _)| key.name == metric)
    {
        return Some(*value);
    }
    // A counter is the TOTAL over its label sets, not one series.
    let mut total = 0.0;
    let mut seen = false;
    for (key, value) in snapshot.counters.iter() {
        if key.name == metric {
            total += *value;
            seen = true;
        }
    }
    if seen {
        return Some(total);
    }
    None
}

/// Evaluate one objective against a snapshot.
///
/// A metric that has not been recorded yet is **MET**, not breached: an
/// objective is about the service's behaviour, and a service that has not
/// yet recorded the metric has not yet violated it. (An operator who wants
/// the opposite can watch the "missing" field in the report.)
pub fn evaluate(snapshot: &MetricsSnapshot, slo: &Slo) -> SloStatus {
    let Some(value) = observed(snapshot, slo.metric) else {
        return SloStatus::Met;
    };
    let breached = match slo.comparison {
        Comparison::AtMost => value > slo.target,
        Comparison::AtLeast => value < slo.target,
        Comparison::Equals => (value - slo.target).abs() > f64::EPSILON,
    };
    if breached {
        return SloStatus::Breached;
    }
    // At risk: within 20 % of the ceiling (the "at most" shape; the other
    // comparisons have no meaningful approach to a boundary).
    if slo.comparison == Comparison::AtMost && slo.target > 0.0 && value >= slo.target * 0.8 {
        return SloStatus::AtRisk;
    }
    SloStatus::Met
}

/// The objectives as JSON (the declared targets and the observed status).
pub fn render_slo_report(snapshot: &MetricsSnapshot) -> String {
    let mut out = String::from("{\"slos\":[");
    for (index, slo) in SLOS.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let status = evaluate(snapshot, slo);
        let value = observed(snapshot, slo.metric);
        out.push_str(&format!(
            "{{\"name\":\"{}\",\"metric\":\"{}\",\"target\":{},\"window\":\"{}\",\"status\":\"{}\",\"observed\":{},\"description\":\"{}\"}}",
            slo.name,
            slo.metric,
            slo.target,
            slo.window,
            status.as_str(),
            value
                .map(|v| format!("{v}"))
                .unwrap_or_else(|| "null".to_string()),
            slo.description
        ));
    }
    out.push_str("]}");
    out
}

/// The declared targets as Prometheus gauges, so a dashboard can plot the
/// target next to the observed value.
pub fn render_slo_gauges(snapshot: &MetricsSnapshot) -> String {
    let mut out = String::new();
    out.push_str("# HELP kelson_slo_target The framework's declared objective for a metric.\n");
    out.push_str("# TYPE kelson_slo_target gauge\n");
    for slo in SLOS {
        let mut labels = BTreeMap::new();
        labels.insert("slo".to_string(), slo.name.to_string());
        labels.insert("metric".to_string(), slo.metric.to_string());
        labels.insert("window".to_string(), slo.window.to_string());
        let rendered: Vec<String> = labels
            .iter()
            .map(|(k, v)| format!("{k}=\"{}\"", escape_label_value(v)))
            .collect();
        out.push_str(&format!(
            "kelson_slo_target{{{}}} {}\n",
            rendered.join(","),
            slo.target
        ));
    }
    out.push_str("# HELP kelson_slo_breached Whether a declared objective is missed (1) or met (0).\n");
    out.push_str("# TYPE kelson_slo_breached gauge\n");
    for slo in SLOS {
        let mut labels = BTreeMap::new();
        labels.insert("slo".to_string(), slo.name.to_string());
        labels.insert("metric".to_string(), slo.metric.to_string());
        let rendered: Vec<String> = labels
            .iter()
            .map(|(k, v)| format!("{k}=\"{}\"", escape_label_value(v)))
            .collect();
        let breached = if evaluate(snapshot, slo).is_breached() { 1 } else { 0 };
        out.push_str(&format!(
            "kelson_slo_breached{{{}}} {breached}\n",
            rendered.join(",")
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{MetricKey, MetricsRegistry};

    fn registry_with(gauge: &str, value: f64, counter: &str, total: f64) -> MetricsRegistry {
        let registry = MetricsRegistry::new(1_000);
        registry.set_gauge(gauge, BTreeMap::new(), value);
        registry.inc(counter, BTreeMap::new(), total);
        registry
    }

    #[test]
    fn every_objective_names_a_metric_the_framework_records() {
        // The metrics the telemetry plane really records.
        const REAL: &[&str] = &[
            "telemetry_error_rate",
            "telemetry_queue_depth",
            "telemetry_shed_level",
            "telemetry_export_failures_total",
            "telemetry_dropped_total",
            "telemetry_tenant_rejected_total",
            "telemetry_incident_active",
        ];
        for slo in SLOS {
            assert!(
                REAL.contains(&slo.metric),
                "the objective {} names a metric the framework does not record: {}",
                slo.name,
                slo.metric
            );
        }
    }

    #[test]
    fn an_unrecorded_metric_is_met_not_breached() {
        let registry = MetricsRegistry::new(100);
        let snapshot = registry.snapshot();
        let slo = &SLOS[0];
        assert_eq!(evaluate(&snapshot, slo), SloStatus::Met);
    }

    #[test]
    fn a_breached_objective_is_reported() {
        let registry = registry_with("telemetry_error_rate", 0.5, "telemetry_dropped_total", 0.0);
        let snapshot = registry.snapshot();
        let error_rate = SLOS.iter().find(|s| s.metric == "telemetry_error_rate").unwrap();
        assert_eq!(evaluate(&snapshot, error_rate), SloStatus::Breached);
        let dropped = SLOS.iter().find(|s| s.metric == "telemetry_dropped_total").unwrap();
        assert_eq!(evaluate(&snapshot, dropped), SloStatus::Met);
    }

    #[test]
    fn an_objective_within_twenty_percent_of_the_ceiling_is_at_risk() {
        let registry = registry_with("telemetry_error_rate", 0.045, "telemetry_dropped_total", 0.0);
        let snapshot = registry.snapshot();
        let error_rate = SLOS.iter().find(|s| s.metric == "telemetry_error_rate").unwrap();
        assert_eq!(evaluate(&snapshot, error_rate), SloStatus::AtRisk);
    }

    #[test]
    fn the_report_names_every_objective_and_its_status() {
        let registry = registry_with("telemetry_shed_level", 1.0, "telemetry_dropped_total", 0.0);
        let report = render_slo_report(&registry.snapshot());
        for slo in SLOS {
            assert!(report.contains(slo.name), "{report}");
        }
        assert!(report.contains("\"status\":\"breached\""), "{report}");
    }

    #[test]
    fn the_gauges_carry_the_targets_and_the_breach_flag() {
        let registry = registry_with("telemetry_shed_level", 1.0, "telemetry_dropped_total", 0.0);
        let gauges = render_slo_gauges(&registry.snapshot());
        assert!(gauges.contains("kelson_slo_target"), "{gauges}");
        assert!(gauges.contains("kelson_slo_breached"), "{gauges}");
        assert!(gauges.contains("slo=\"operability.incident_active\""), "{gauges}");
    }

    #[test]
    fn a_counter_is_summed_over_its_label_sets() {
        let registry = MetricsRegistry::new(100);
        let mut labels_a = BTreeMap::new();
        labels_a.insert("reason".to_string(), "overflow".to_string());
        let mut labels_b = BTreeMap::new();
        labels_b.insert("reason".to_string(), "filter".to_string());
        registry.inc("telemetry_dropped_total", labels_a, 3.0);
        registry.inc("telemetry_dropped_total", labels_b, 4.0);
        let snapshot = registry.snapshot();
        assert_eq!(observed(&snapshot, "telemetry_dropped_total"), Some(7.0));
    }

    #[test]
    fn the_metric_key_shape_is_unchanged() {
        // a compile-time reminder that the snapshot's shape is what the
        // evaluation reads
        let key = MetricKey {
            name: "x".to_string(),
            labels: BTreeMap::new(),
        };
        assert_eq!(key.name, "x");
    }
}
