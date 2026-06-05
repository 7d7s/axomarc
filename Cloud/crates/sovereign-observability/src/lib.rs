//! Observability adapter: structured JSON logs + Prometheus `/metrics`.
//!
//! F1 only calls [`init`] to prove the binary boots cleanly. F2 wires the
//! env-filter, F8 adds the metrics exporter, V1.5 adds the OpenTelemetry SDK.
//! F9 (`sovereign doctor`) reads the metrics to surface the diagnostic.

#![deny(unsafe_code)]
#![allow(missing_docs)]

use std::sync::Once;

static INIT: Once = Once::new();

/// Initialize the global tracing subscriber exactly once.
///
/// F1 is intentionally minimal: the default `RUST_LOG=info` filter is honored,
/// the format is the standard `tracing-subscriber` fmt layer (text to stderr).
/// G3 in V1 will replace this with structured JSON + journald auto-detection.
pub fn init() {
    INIT.call_once(|| {
        let filter = tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
        let _ = tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(false)
            .try_init();
    });
}

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
