//! Capability engine: pure prefix-matching decisions over cached snapshots
//! (02-SYSTEM-DESIGN §5). Snapshot loading from grants table lands in M2-03;
//! the decision core is already complete and unit-tested.
use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Grant {
    pub id: Uuid,
    pub action_class: String,
    pub node_path_prefix: String,
    pub module: Option<String>,
    pub valid_until: Option<time::OffsetDateTime>,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub subject_id: Uuid,
    pub version: i64,
    pub grants: Vec<Grant>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub allowed: bool,
    pub matched_grant: Option<Uuid>,
}

impl Snapshot {
    /// POL-02: decide by (action, resource owner path, module) — prefix containment.
    pub fn decide(&self, action: &str, owner_path: &str, module: &str, now: time::OffsetDateTime) -> Decision {
        for g in &self.grants {
            if g.action_class != action {
                continue;
            }
            if let Some(m) = &g.module {
                if m != module {
                    continue;
                }
            }
            if let Some(exp) = g.valid_until {
                if exp < now {
                    continue;
                }
            }
            if owner_path.starts_with(&g.node_path_prefix) {
                return Decision { allowed: true, matched_grant: Some(g.id) };
            }
        }
        Decision { allowed: false, matched_grant: None }
    }
}

/// Snapshot cache (ORG-06: invalidation < 2s after grant mutations).
pub struct Engine {
    cache: moka::sync::Cache<Uuid, Arc<Snapshot>>,
}

impl Engine {
    pub fn new() -> Self {
        Self {
            cache: moka::sync::Cache::builder()
                .time_to_live(Duration::from_secs(60))
                .max_capacity(10_000)
                .build(),
        }
    }

    pub fn insert(&self, snap: Snapshot) {
        self.cache.insert(snap.subject_id, Arc::new(snap));
    }

    pub fn get(&self, subject: Uuid) -> Option<Arc<Snapshot>> {
        self.cache.get(&subject)
    }

    pub fn invalidate(&self, subject: Uuid) {
        self.cache.invalidate(&subject);
    }

    /// POL-01: deny-by-default. No snapshot = deny.
    pub fn decide(&self, subject: Uuid, action: &str, owner_path: &str, module: &str, now: time::OffsetDateTime) -> Decision {
        match self.get(subject) {
            Some(snap) => snap.decide(action, owner_path, module, now),
            None => Decision { allowed: false, matched_grant: None },
        }
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(grants: Vec<Grant>) -> Snapshot {
        Snapshot { subject_id: Uuid::now_v7(), version: 1, grants }
    }

    fn grant(action: &str, prefix: &str) -> Grant {
        Grant { id: Uuid::now_v7(), action_class: action.into(), node_path_prefix: prefix.into(), module: None, valid_until: None }
    }

    fn epoch() -> time::OffsetDateTime {
        time::OffsetDateTime::UNIX_EPOCH
    }

    #[test]
    fn prefix_containment_allows_subtree() {
        let s = snap(vec![grant("appointment.read", "LE1|LOC5")]);
        assert!(s.decide("appointment.read", "LE1|LOC5|DEP3", "appointment", epoch()).allowed);
    }

    #[test]
    fn sibling_branch_denied() {
        let s = snap(vec![grant("appointment.read", "LE1|LOC5")]);
        assert!(!s.decide("appointment.read", "LE1|LOC6", "appointment", epoch()).allowed);
    }

    #[test]
    fn wrong_action_denied() {
        let s = snap(vec![grant("appointment.read", "LE1")]);
        assert!(!s.decide("appointment.write", "LE1|LOC5", "appointment", epoch()).allowed);
    }

    #[test]
    fn expired_grant_denied() {
        let mut g = grant("appointment.read", "LE1");
        g.valid_until = Some(time::OffsetDateTime::UNIX_EPOCH);
        let s = snap(vec![g]);
        let later = time::OffsetDateTime::UNIX_EPOCH + time::Duration::seconds(10);
        assert!(!s.decide("appointment.read", "LE1|LOC5", "appointment", later).allowed);
    }

    #[test]
    fn engine_denies_without_snapshot() {
        let e = Engine::new();
        let d = e.decide(Uuid::now_v7(), "x", "y", "z", epoch());
        assert!(!d.allowed);
    }
}