//! Processing DAG (Sentinel S5 §9): an ordered, named stage chain. Every
//! category flows through the full chain; per-category divergence happens at
//! routing time (`router.rs`), which keeps the graph declarative and cheap.

use keel_telemetry::{Event, ProcessOutcome, Processor};

pub struct PipelineGraph {
    stages: Vec<Box<dyn Processor>>,
}

impl Default for PipelineGraph {
    fn default() -> Self {
        Self::new()
    }
}

impl PipelineGraph {
    pub fn new() -> Self {
        Self { stages: Vec::new() }
    }

    pub fn push(mut self, stage: Box<dyn Processor>) -> Self {
        self.stages.push(stage);
        self
    }

    pub fn stage_names(&self) -> Vec<&str> {
        self.stages.iter().map(|s| s.name()).collect()
    }

    /// Run one event through every stage. First drop terminates the chain.
    pub fn process(&self, event: &mut Event) -> Result<(), String> {
        for stage in &self.stages {
            if let ProcessOutcome::Drop(reason) = stage.process(event) {
                return Err(reason);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DropAll;
    impl Processor for DropAll {
        fn name(&self) -> &str {
            "drop-all"
        }
        fn process(&self, _e: &mut Event) -> ProcessOutcome {
            ProcessOutcome::Drop("no".into())
        }
    }

    struct Stamp;
    impl Processor for Stamp {
        fn name(&self) -> &str {
            "stamp"
        }
        fn process(&self, e: &mut Event) -> ProcessOutcome {
            if let Some(f) = e.payload.fields_mut() {
                f.insert("stamped".into(), serde_json::Value::Bool(true));
            }
            ProcessOutcome::Continue
        }
    }

    fn event() -> Event {
        use keel_foundation::TelemetryContext;
        use keel_telemetry::{
            EventEnvelope, Fields, Level, LogPayload, ServiceIdentity, TelemetryPayload,
            SCHEMA_LOG, SCHEMA_LOG_VERSION,
        };
        EventEnvelope::new(
            SCHEMA_LOG,
            SCHEMA_LOG_VERSION,
            ServiceIdentity::new("t", "dev"),
            TelemetryContext::root(),
            TelemetryPayload::Log(LogPayload {
                level: Level::Info,
                message: "m".into(),
                target: None,
                fields: Fields::new(),
                extra: Fields::new(),
            }),
        )
    }

    #[test]
    fn chain_runs_in_order_and_stops_on_drop() {
        let g = PipelineGraph::new()
            .push(Box::new(Stamp))
            .push(Box::new(DropAll));
        let mut e = event();
        let err = g.process(&mut e).unwrap_err();
        assert_eq!(err, "no");
        assert!(e.payload.fields_ref().unwrap().contains_key("stamped"));
        assert_eq!(g.stage_names(), vec!["stamp", "drop-all"]);
    }
}
