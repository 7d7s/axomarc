//! Adaptive sampling (Sentinel S5 §13): deterministic escalation ladder from
//! runtime signals. No ML — thresholds are fixed, documented, and testable.

use serde::{Deserialize, Serialize};

/// Sampling tier the runtime applies to span/log categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RateTier {
    /// Base rate (config default, e.g. 0.01–0.1).
    Normal,
    /// Error/latency pressure: bump to 25%.
    Elevated,
    /// Severe incident: 100% of relevant telemetry.
    Full,
}

impl RateTier {
    /// Effective multiplier relative to the configured base rate.
    pub fn multiplier(&self, base_rate: f64) -> f64 {
        match self {
            RateTier::Normal => 1.0,
            RateTier::Elevated => (0.25 / base_rate.max(0.0001)).min(1.0 / base_rate.max(0.0001)),
            RateTier::Full => 1.0 / base_rate.max(0.0001),
        }
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct EscalationSignals {
    /// Rolling error rate in [0,1].
    pub error_rate: f64,
    /// Current p95 latency (ms).
    pub p95_latency_ms: f64,
    /// Baseline p95 latency (ms).
    pub baseline_latency_ms: f64,
    /// Queue backlog fill ratio in [0,1].
    pub queue_fill: f64,
    /// Incident mode active (overrides everything).
    pub incident: bool,
}

/// Fixed thresholds (documented in STAGE-SENTINEL-SPEC).
pub const ERROR_RATE_ELEVATED: f64 = 0.05;
pub const ERROR_RATE_FULL: f64 = 0.20;
pub const LATENCY_RATIO_ELEVATED: f64 = 3.0;
pub const LATENCY_RATIO_FULL: f64 = 10.0;
pub const QUEUE_FILL_FULL: f64 = 0.90;

pub fn escalation(signals: &EscalationSignals) -> RateTier {
    if signals.incident {
        return RateTier::Full;
    }
    let latency_ratio = if signals.baseline_latency_ms > 0.0 {
        signals.p95_latency_ms / signals.baseline_latency_ms
    } else {
        1.0
    };
    if signals.error_rate >= ERROR_RATE_FULL
        || latency_ratio >= LATENCY_RATIO_FULL
        || signals.queue_fill >= QUEUE_FILL_FULL
    {
        return RateTier::Full;
    }
    if signals.error_rate >= ERROR_RATE_ELEVATED || latency_ratio >= LATENCY_RATIO_ELEVATED {
        return RateTier::Elevated;
    }
    RateTier::Normal
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals() -> EscalationSignals {
        EscalationSignals {
            error_rate: 0.0,
            p95_latency_ms: 80.0,
            baseline_latency_ms: 80.0,
            queue_fill: 0.1,
            incident: false,
        }
    }

    #[test]
    fn ladder_follows_signals() {
        assert_eq!(escalation(&signals()), RateTier::Normal);
        let mut s = signals();
        s.error_rate = 0.07;
        assert_eq!(escalation(&s), RateTier::Elevated);
        s.error_rate = 0.30;
        assert_eq!(escalation(&s), RateTier::Full);
        let mut s = signals();
        s.p95_latency_ms = 900.0;
        assert_eq!(escalation(&s), RateTier::Full);
        let mut s = signals();
        s.incident = true;
        assert_eq!(escalation(&s), RateTier::Full);
    }

    #[test]
    fn multiplier_caps_at_one_over_base() {
        let m = RateTier::Full.multiplier(0.01);
        assert!((m - 100.0).abs() < 1e-9);
        assert!(RateTier::Elevated.multiplier(0.5) <= 2.0);
    }
}
