//! Incident control plane (Sentinel S5 §16, D-76): a file under
//! `storage/framework/telemetry/control/` is the single source of truth for
//! incident mode. `kelson observe incident start|stop` writes/removes the file;
//! the kelson-api admin endpoint writes the same file; every host polls it
//! (2 s) and applies it to `TelemetryControlState`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use kelson_foundation::IncidentId;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Active incident marker (`incident.json`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IncidentState {
    pub id: IncidentId,
    #[serde(with = "time::serde::iso8601")]
    pub started_at: OffsetDateTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_by: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub flags: BTreeMap<String, String>,
}

impl IncidentState {
    pub fn new(started_by: Option<&str>) -> Self {
        Self {
            id: IncidentId::new(),
            started_at: OffsetDateTime::now_utc(),
            started_by: started_by.map(|s| s.to_string()),
            flags: BTreeMap::new(),
        }
    }
}

/// File-backed incident control plane. All operations are best-effort and
/// never panic: a missing/corrupt file means "no incident".
pub struct IncidentControlPlane {
    dir: PathBuf,
}

impl IncidentControlPlane {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn active_path(&self) -> PathBuf {
        self.dir.join("incident.json")
    }

    fn history_dir(&self) -> PathBuf {
        self.dir.join("history")
    }

    /// Read the active incident, if any. Corrupt files count as no incident
    /// (the runtime also records a counter so operators can investigate).
    pub fn read(&self) -> Option<IncidentState> {
        let raw = std::fs::read_to_string(self.active_path()).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// Start an incident: write the marker file (idempotent — an existing
    /// incident is returned unchanged).
    pub fn start(&self, started_by: Option<&str>) -> std::io::Result<IncidentState> {
        self.start_with_flags(started_by, BTreeMap::new())
    }

    /// Start with control flags (e.g. `debug_retention = "true"`). Idempotent.
    pub fn start_with_flags(
        &self,
        started_by: Option<&str>,
        flags: BTreeMap<String, String>,
    ) -> std::io::Result<IncidentState> {
        if let Some(existing) = self.read() {
            return Ok(existing);
        }
        std::fs::create_dir_all(&self.dir)?;
        let mut state = IncidentState::new(started_by);
        state.flags = flags;
        let raw = serde_json::to_string_pretty(&state).expect("incident serializes");
        let tmp = self.dir.join("incident.json.tmp");
        std::fs::write(&tmp, raw)?;
        std::fs::rename(&tmp, self.active_path())?;
        Ok(state)
    }

    /// Stop the active incident: archive it under `history/` and remove the
    /// marker. Returns the stopped state, if there was one.
    pub fn stop(&self) -> std::io::Result<Option<IncidentState>> {
        let Some(state) = self.read() else {
            return Ok(None);
        };
        let history = self.history_dir();
        std::fs::create_dir_all(&history)?;
        let stamp = state
            .started_at
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_else(|_| "unknown".into())
            .replace(':', "-");
        let archive = history.join(format!("incident-{}-{stamp}.json", state.id));
        let raw = serde_json::to_string_pretty(&state).expect("incident serializes");
        std::fs::write(&archive, raw)?;
        let _ = std::fs::remove_file(self.active_path());
        Ok(Some(state))
    }

    /// Archived incidents (newest first by file name).
    pub fn history(&self) -> Vec<IncidentState> {
        let dir = self.history_dir();
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut states: Vec<(String, IncidentState)> = Vec::new();
        for entry in entries.flatten() {
            if entry.path().extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(raw) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            if let Ok(state) = serde_json::from_str(&raw) {
                let name = entry.file_name().to_string_lossy().into_owned();
                states.push((name, state));
            }
        }
        states.sort_by(|a, b| b.0.cmp(&a.0));
        states.into_iter().map(|(_, s)| s).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "kelson-incident-test-{name}-{}",
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn start_stop_roundtrip() {
        let dir = temp_dir("roundtrip");
        let plane = IncidentControlPlane::new(&dir);
        assert!(plane.read().is_none());

        let started = plane.start(Some("operator@example.com")).unwrap();
        let active = plane.read().expect("incident active");
        assert_eq!(active.id, started.id);
        assert_eq!(active.started_by.as_deref(), Some("operator@example.com"));

        // Idempotent start returns the same incident.
        let again = plane.start(Some("someone-else")).unwrap();
        assert_eq!(again.id, started.id);

        let stopped = plane.stop().unwrap().expect("was active");
        assert_eq!(stopped.id, started.id);
        assert!(plane.read().is_none());
        assert_eq!(plane.history().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_marker_counts_as_no_incident() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join("incident.json"), "{ not json").unwrap();
        let plane = IncidentControlPlane::new(&dir);
        assert!(plane.read().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_without_incident_is_noop() {
        let dir = temp_dir("noop");
        let plane = IncidentControlPlane::new(&dir);
        assert!(plane.stop().unwrap().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
