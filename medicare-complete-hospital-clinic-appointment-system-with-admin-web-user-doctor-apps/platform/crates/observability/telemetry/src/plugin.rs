//! Plugin architecture (Sentinel S5 §26): third-party packages register
//! categories, processors and exporters through `TelemetryPlugin`. Trait
//! definitions live here (the lowest observability crate) so pipeline and
//! export crates can implement them without circular dependencies.

use std::sync::Arc;

use crate::event::Event;

/// Outcome of one processor step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessOutcome {
    /// Continue through the DAG.
    Continue,
    /// Stop processing and drop the event (reason recorded in drop counters).
    Drop(String),
}

/// One named transformation in the processing DAG (§9).
pub trait Processor: Send + Sync {
    fn name(&self) -> &str;
    fn process(&self, event: &mut Event) -> ProcessOutcome;
}

/// Error surface for exporter attempts (breaker/retry consume this).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportError {
    /// Transport-level failure; retryable under the retry budget.
    Transport(String),
    /// Endpoint said it will not accept this (bad schema/auth); not retryable.
    Rejected(String),
    /// Exporter is unavailable (circuit open / resource shed); try fallbacks.
    Unavailable(String),
}

impl std::fmt::Display for ExportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExportError::Transport(m) => write!(f, "transport: {m}"),
            ExportError::Rejected(m) => write!(f, "rejected: {m}"),
            ExportError::Unavailable(m) => write!(f, "unavailable: {m}"),
        }
    }
}

/// Byte count of one successful export (feeds cost accounting, §21).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExportOk {
    pub bytes: usize,
    pub events: usize,
}

/// A destination for batches of events. Implementations must be safe to call
/// from a single dedicated worker thread; blocking is allowed (bounded by
/// their own timeouts).
pub trait Exporter: Send + Sync {
    fn name(&self) -> &str;
    fn export(&self, batch: &[Event]) -> Result<ExportOk, ExportError>;
    /// Cheap health probe used by breakers after cooldown (default: ok).
    fn probe(&self) -> bool {
        true
    }
}

/// An exporter registered under a configured name.
pub struct RegisteredExporter {
    pub name: String,
    pub exporter: Arc<dyn Exporter>,
}

/// Registration surface handed to plugins before the runtime starts.
#[derive(Default)]
pub struct TelemetryRegistry {
    pub categories: Vec<String>,
    pub processors: Vec<Box<dyn Processor>>,
    pub exporters: Vec<RegisteredExporter>,
}

impl TelemetryRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_category(&mut self, name: impl Into<String>) {
        let name = name.into();
        if !self.categories.contains(&name) {
            self.categories.push(name);
        }
    }

    pub fn register_processor(&mut self, processor: Box<dyn Processor>) {
        self.processors.push(processor);
    }

    pub fn register_exporter(&mut self, name: impl Into<String>, exporter: Arc<dyn Exporter>) {
        self.exports();
        let name = name.into();
        if !self.exporters.iter().any(|e| e.name == name) {
            self.exporters.push(RegisteredExporter { name, exporter });
        }
    }

    fn exports(&self) {}
}

/// A plugin contributes telemetry registrations at boot (§26).
pub trait TelemetryPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn register(&self, registry: &mut TelemetryRegistry);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{ServiceIdentity, SCHEMA_LOG, SCHEMA_LOG_VERSION};
    use crate::event::{Fields, Level, LogPayload, TelemetryPayload};
    use kelson_foundation::TelemetryContext;

    struct DropDebug;
    impl Processor for DropDebug {
        fn name(&self) -> &str {
            "drop-debug"
        }
        fn process(&self, event: &mut Event) -> ProcessOutcome {
            if let TelemetryPayload::Log(l) = &event.payload {
                if l.level == Level::Debug {
                    return ProcessOutcome::Drop("debug disabled".into());
                }
            }
            ProcessOutcome::Continue
        }
    }

    fn log(level: Level) -> Event {
        crate::envelope::EventEnvelope::new(
            SCHEMA_LOG,
            SCHEMA_LOG_VERSION,
            ServiceIdentity::new("t", "dev"),
            TelemetryContext::root(),
            TelemetryPayload::Log(LogPayload {
                level,
                message: "m".into(),
                target: None,
                fields: Fields::new(),
                extra: Fields::new(),
            }),
        )
    }

    #[test]
    fn processor_outcomes() {
        let p = DropDebug;
        let mut d = log(Level::Debug);
        assert!(matches!(p.process(&mut d), ProcessOutcome::Drop(_)));
        let mut i = log(Level::Info);
        assert_eq!(p.process(&mut i), ProcessOutcome::Continue);
    }

    #[test]
    fn registry_dedupes() {
        let mut r = TelemetryRegistry::new();
        r.register_category("payment");
        r.register_category("payment");
        assert_eq!(r.categories.len(), 1);
    }
}
