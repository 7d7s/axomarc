//! Backpressure policy (Sentinel S5 §14): the runtime degrades in a fixed,
//! testable order instead of dropping at random.

use serde::{Deserialize, Serialize};

/// Degradation ladder, evaluated from queue pressure + disk health.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShedLevel {
    /// Normal operation.
    None,
    /// Stop emitting ephemeral (debug) events.
    DropEphemeral,
    /// Additionally halve sampled categories (multiplier applied by sampler).
    AggressiveSample,
    /// Additionally shed non-critical exporters (interactive-only routes).
    ShedNonCritical,
}

/// Decision for one event at the ingestor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DropDecision {
    Enqueue,
    DroppedEphemeral,
    DroppedSampled,
}

/// Map queue fill ratio + disk health to a shed level. Thresholds are fixed
/// and documented so operators can reason about them (no magic knobs).
pub fn shed_level(queue_fill_ratio: f64, disk_healthy: bool) -> ShedLevel {
    if !disk_healthy && queue_fill_ratio > 0.5 {
        return ShedLevel::ShedNonCritical;
    }
    if queue_fill_ratio >= 0.95 {
        ShedLevel::ShedNonCritical
    } else if queue_fill_ratio >= 0.80 {
        ShedLevel::AggressiveSample
    } else if queue_fill_ratio >= 0.60 {
        ShedLevel::DropEphemeral
    } else {
        ShedLevel::None
    }
}

/// Per-event decision given the current shed level and whether the event is
/// subject to sampling (errors/audit/security are never sampled away).
pub fn decide(level: ShedLevel, is_ephemeral: bool, is_sampled_category: bool) -> DropDecision {
    match level {
        ShedLevel::None => DropDecision::Enqueue,
        ShedLevel::DropEphemeral if is_ephemeral => DropDecision::DroppedEphemeral,
        ShedLevel::AggressiveSample | ShedLevel::ShedNonCritical if is_ephemeral => {
            DropDecision::DroppedEphemeral
        }
        _ => {
            let _ = is_sampled_category; // sampling handled by pipeline sampler
            DropDecision::Enqueue
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_orders_pressure() {
        assert_eq!(shed_level(0.1, true), ShedLevel::None);
        assert_eq!(shed_level(0.65, true), ShedLevel::DropEphemeral);
        assert_eq!(shed_level(0.85, true), ShedLevel::AggressiveSample);
        assert_eq!(shed_level(0.97, true), ShedLevel::ShedNonCritical);
        assert_eq!(shed_level(0.6, false), ShedLevel::ShedNonCritical);
    }

    #[test]
    fn decisions_protect_critical_events() {
        assert_eq!(decide(ShedLevel::DropEphemeral, true, false), DropDecision::DroppedEphemeral);
        assert_eq!(decide(ShedLevel::DropEphemeral, false, true), DropDecision::Enqueue);
        assert_eq!(decide(ShedLevel::ShedNonCritical, false, false), DropDecision::Enqueue);
    }
}
