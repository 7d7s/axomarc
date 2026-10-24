//! Metrics registry (Sentinel S5 §21/§22): counters/gauges/histograms with
//! bounded label cardinality (overflow collapses to a hashed bucket, never
//! unbounded unique series).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use moka::sync::Cache;

/// Fixed latency histogram buckets in milliseconds.
pub const DEFAULT_BUCKETS_MS: &[f64] = &[
    1.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0, 1000.0, 2500.0, 5000.0, 10000.0,
];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MetricKey {
    pub name: String,
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct HistogramState {
    pub buckets: Vec<(f64, u64)>, // cumulative counts per upper bound
    pub sum: f64,
    pub count: u64,
}

#[derive(Debug, Clone)]
pub struct MetricsSnapshot {
    pub counters: Vec<(MetricKey, f64)>,
    pub gauges: Vec<(MetricKey, f64)>,
    pub histograms: Vec<(String, HistogramState)>,
}

pub struct MetricsRegistry {
    counters: Mutex<BTreeMap<MetricKey, f64>>,
    gauges: Mutex<BTreeMap<MetricKey, f64>>,
    histograms: Mutex<BTreeMap<String, HistogramState>>,
    bound_ms: Vec<f64>,
    /// Known label-set signatures; overflow collapses to a hashed bucket.
    label_sets: Cache<String, ()>,
    known_sets: AtomicU64,
    max_label_sets: u64,
}

impl MetricsRegistry {
    pub fn new(max_label_sets: u64) -> Self {
        Self {
            counters: Mutex::new(BTreeMap::new()),
            gauges: Mutex::new(BTreeMap::new()),
            histograms: Mutex::new(BTreeMap::new()),
            bound_ms: DEFAULT_BUCKETS_MS.to_vec(),
            label_sets: Cache::builder().max_capacity(max_label_sets).build(),
            known_sets: AtomicU64::new(0),
            max_label_sets,
        }
    }

    fn bound_labels(&self, labels: BTreeMap<String, String>) -> BTreeMap<String, String> {
        let signature: String = labels
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(";");
        if self.label_sets.get(&signature).is_some() {
            return labels;
        }
        let len = self.known_sets.load(Ordering::Relaxed);
        if len >= self.max_label_sets {
            // Cardinality overflow (§22): collapse all unseen label sets into
            // ONE overflow bucket so total series stay bounded.
            let mut collapsed = BTreeMap::new();
            collapsed.insert("__overflow__".into(), "collapsed".into());
            return collapsed;
        }
        self.label_sets.insert(signature, ());
        self.known_sets.fetch_add(1, Ordering::Relaxed);
        labels
    }

    pub fn inc(&self, name: &str, labels: BTreeMap<String, String>, by: f64) {
        let key = MetricKey {
            name: name.into(),
            labels: self.bound_labels(labels),
        };
        self.counters
            .lock()
            .expect("counters poisoned")
            .entry(key)
            .and_modify(|v| *v += by)
            .or_insert(by);
    }

    pub fn set_gauge(&self, name: &str, labels: BTreeMap<String, String>, value: f64) {
        let key = MetricKey {
            name: name.into(),
            labels: self.bound_labels(labels),
        };
        self.gauges
            .lock()
            .expect("gauges poisoned")
            .insert(key, value);
    }

    pub fn observe_ms(&self, name: &str, value_ms: f64) {
        let mut guard = self.histograms.lock().expect("histograms poisoned");
        let state = guard
            .entry(name.to_string())
            .or_insert_with(|| HistogramState {
                buckets: self.bound_ms.iter().map(|b| (*b, 0)).collect(),
                sum: 0.0,
                count: 0,
            });
        state.sum += value_ms;
        state.count += 1;
        for (bound, count) in state.buckets.iter_mut() {
            if value_ms <= *bound {
                *count += 1;
            }
        }
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            counters: self
                .counters
                .lock()
                .expect("counters poisoned")
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            gauges: self
                .gauges
                .lock()
                .expect("gauges poisoned")
                .iter()
                .map(|(k, v)| (k.clone(), *v))
                .collect(),
            histograms: self
                .histograms
                .lock()
                .expect("histograms poisoned")
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_accumulates_per_label_set() {
        let r = MetricsRegistry::new(100);
        let mut l = BTreeMap::new();
        l.insert("route".into(), "/a".into());
        r.inc("requests", l.clone(), 1.0);
        r.inc("requests", l.clone(), 2.0);
        let s = r.snapshot();
        assert_eq!(s.counters.len(), 1);
        assert!((s.counters[0].1 - 3.0).abs() < 1e-9);
    }

    #[test]
    fn histogram_counts_buckets() {
        let r = MetricsRegistry::new(100);
        r.observe_ms("latency", 3.0);
        r.observe_ms("latency", 300.0);
        let s = r.snapshot();
        let (_, h) = &s.histograms[0];
        assert_eq!(h.count, 2);
        let under_5 = h.buckets.iter().find(|(b, _)| *b == 5.0).unwrap().1;
        let under_500 = h.buckets.iter().find(|(b, _)| *b == 500.0).unwrap().1;
        assert_eq!(under_5, 1);
        assert_eq!(under_500, 2);
    }

    #[test]
    fn cardinality_overflow_collapses_to_hash() {
        let r = MetricsRegistry::new(2);
        for i in 0..10 {
            let mut l = BTreeMap::new();
            l.insert("user".into(), format!("u{i}"));
            r.inc("hits", l, 1.0);
        }
        let s = r.snapshot();
        // At most 2 real series + overflow bucket(s).
        let overflowed = s
            .counters
            .iter()
            .filter(|(k, _)| k.labels.contains_key("__overflow__"))
            .count();
        assert!(
            overflowed >= 1,
            "expected overflow collapse, got {:?}",
            s.counters.len()
        );
        assert!(s.counters.len() <= 4);
    }
}
