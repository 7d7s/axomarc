//! # kelson-metrics
//! Sentinel (S5) metrics: bounded-cardinality registry (counters, gauges,
//! histograms), hand-rolled Prometheus text exposition (gap B-17) and the
//! telemetry cost ledger (S5 section 21).

pub mod cost;
pub mod prom;
pub mod registry;
pub mod slo;

pub use cost::{CostEntry, CostLedger, CostSnapshot};
pub use prom::{render_prometheus, sanitize_name};
pub use registry::{HistogramState, MetricKey, MetricsRegistry, MetricsSnapshot, DEFAULT_BUCKETS_MS};
pub use slo::{evaluate, render_slo_gauges, render_slo_report, Comparison, Slo, SloStatus, SLOS};
