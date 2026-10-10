//! # kelson-ownership
//! Scope answers "where may I operate?"; ownership answers "who owns this resource?"
//! (11-SPEC §3 §15). A finance user may reach BU-4 without controlling the invoice
//! owned by LE-7.

use std::collections::BTreeSet;

use kelson_context::{OwnershipRef, OwnershipSnapshot};
use kelson_foundation::TenantId;
use uuid::Uuid;

/// Runtime registry of ownership facts for resources touched by an execution.
#[derive(Debug, Default)]
pub struct OwnershipGraph {
    records: Vec<OwnershipRef>,
}

impl OwnershipGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn declare(
        &mut self,
        tenant_id: TenantId,
        resource_type: impl Into<String>,
        resource_id: Uuid,
        owner_scope_path: impl Into<String>,
    ) {
        self.records.push(OwnershipRef {
            resource_type: resource_type.into(),
            resource_id,
            owner_scope_path: owner_scope_path.into(),
            tenant_id,
        });
    }

    pub fn owner_of(&self, resource_type: &str, resource_id: &Uuid) -> Option<&OwnershipRef> {
        self.records
            .iter()
            .find(|r| r.resource_type == resource_type && r.resource_id == *resource_id)
    }

    /// Compatibility check: every referenced resource's owner scope must sit inside
    /// the actor's effective scope (prefix rule, same as scope containment).
    pub fn compatible_with(snapshot: &OwnershipSnapshot, scope_paths: &BTreeSet<String>) -> bool {
        snapshot.entries.iter().all(|entry| {
            scope_paths.iter().any(|p| {
                entry.owner_scope_path == *p || entry.owner_scope_path.starts_with(&format!("{p}|"))
            })
        })
    }

    pub fn snapshot(&self) -> OwnershipSnapshot {
        OwnershipSnapshot {
            entries: self.records.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kelson_context::OwnershipRef;

    #[test]
    fn ownership_must_sit_inside_scope() {
        let snap = OwnershipSnapshot {
            entries: vec![OwnershipRef {
                resource_type: "invoice".into(),
                resource_id: Uuid::now_v7(),
                owner_scope_path: "LE7".into(),
                tenant_id: TenantId::new(),
            }],
        };
        let mut scope_paths = BTreeSet::new();
        scope_paths.insert("BU4".to_string());
        assert!(!OwnershipGraph::compatible_with(&snap, &scope_paths));
        scope_paths.insert("LE7".to_string());
        assert!(OwnershipGraph::compatible_with(&snap, &scope_paths));
    }
}
