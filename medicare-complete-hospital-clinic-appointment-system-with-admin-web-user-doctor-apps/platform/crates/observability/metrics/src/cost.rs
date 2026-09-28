//! Cost accounting (Sentinel S5 §21): per-source, per-exporter byte/event
//! counters rolled into a snapshot the CLI renders as a cost table.

use std::collections::BTreeMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct CostEntry {
    pub events: u64,
    pub bytes: u64,
}

#[derive(Debug, Default)]
pub struct CostLedger {
    inner: Mutex<CostLedgerInner>,
}

#[derive(Debug, Default)]
struct CostLedgerInner {
    by_source: BTreeMap<String, CostEntry>,
    by_exporter: BTreeMap<String, CostEntry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CostSnapshot {
    pub by_source: BTreeMap<String, CostEntry>,
    pub by_exporter: BTreeMap<String, CostEntry>,
    pub total_events: u64,
    pub total_bytes: u64,
}

impl CostLedger {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_source(&self, source: &str, events: u64, bytes: u64) {
        let mut inner = self.inner.lock().expect("cost poisoned");
        let entry = inner.by_source.entry(source.into()).or_default();
        entry.events += events;
        entry.bytes += bytes;
    }

    pub fn record_export(&self, exporter: &str, events: u64, bytes: u64) {
        let mut inner = self.inner.lock().expect("cost poisoned");
        let entry = inner.by_exporter.entry(exporter.into()).or_default();
        entry.events += events;
        entry.bytes += bytes;
    }

    pub fn snapshot(&self) -> CostSnapshot {
        let inner = self.inner.lock().expect("cost poisoned");
        CostSnapshot {
            total_events: inner.by_source.values().map(|e| e.events).sum(),
            total_bytes: inner.by_source.values().map(|e| e.bytes).sum(),
            by_source: inner.by_source.clone(),
            by_exporter: inner.by_exporter.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_rolls_up() {
        let l = CostLedger::new();
        l.record_source("http", 10, 1000);
        l.record_source("http", 5, 500);
        l.record_export("file", 15, 1500);
        let s = l.snapshot();
        assert_eq!(s.total_events, 15);
        assert_eq!(s.total_bytes, 1500);
        assert_eq!(s.by_source["http"].events, 15);
        assert_eq!(s.by_exporter["file"].bytes, 1500);
    }
}
