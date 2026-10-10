//! Routing (Sentinel S5 §9): category → exporter name sets. The policy engine
//! can override per event; this table is the compiled default.

use std::collections::BTreeMap;

use kelson_telemetry::{Event, TelemetryCategory};

#[derive(Debug, Clone, Default)]
pub struct RouteTable {
    pub by_category: BTreeMap<TelemetryCategory, Vec<String>>,
    pub default: Vec<String>,
}

impl RouteTable {
    pub fn new(default: Vec<String>) -> Self {
        Self {
            by_category: BTreeMap::new(),
            default,
        }
    }

    pub fn route_category(mut self, category: TelemetryCategory, exporters: Vec<String>) -> Self {
        self.by_category.insert(category, exporters);
        self
    }

    /// Resolve exporter names for an event (policy decisions take precedence
    /// at the runtime level; this is the static default).
    pub fn resolve(&self, event: &Event) -> Vec<String> {
        self.by_category
            .get(&event.payload.category())
            .cloned()
            .unwrap_or_else(|| self.default.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_route_falls_back_to_default() {
        let table = RouteTable::new(vec!["console".into()])
            .route_category(TelemetryCategory::Audit, vec!["audit_file".into()]);
        use kelson_foundation::TelemetryContext;
        use kelson_telemetry::{
            EventEnvelope, Fields, Level, LogPayload, ServiceIdentity, TelemetryPayload,
            SCHEMA_LOG, SCHEMA_LOG_VERSION,
        };
        let log = EventEnvelope::new(
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
        );
        assert_eq!(table.resolve(&log), vec!["console".to_string()]);
    }
}
