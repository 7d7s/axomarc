//! # kelson-scope
//! Scope graph (11-SPEC §3): NOT one strict tree — a tenant has many parallel
//! hierarchies (LE / BU / Region / Location / Branch / Department / CostCenter /
//! Warehouse / custom). Effective scope = grant nodes + their descendants, computed
//! once and frozen into the ExecutionContext as a `ScopeSnapshot`.

use std::collections::{BTreeSet, HashMap};

use kelson_context::ScopeSnapshot;
use kelson_foundation::{AppError, AuthorizationRevision, ScopeNodeId, TenantId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeNodeKind {
    LegalEntity,
    BusinessUnit,
    Region,
    Location,
    Branch,
    Department,
    CostCenter,
    Warehouse,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeNode {
    pub id: ScopeNodeId,
    pub tenant_id: TenantId,
    pub kind: ScopeNodeKind,
    pub name: String,
    /// Materialized path (`LE1` / `LE1|LOC5`). Parent path + "|" + name.
    pub path: String,
    pub parent: Option<ScopeNodeId>,
}

/// A grant attaches a membership to a node; descendants are included.
#[derive(Debug, Clone)]
pub struct ScopeGrant {
    pub membership_id: kelson_foundation::MembershipId,
    pub node_id: ScopeNodeId,
}

/// Frozen effective scope: where may this actor operate.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScopeSet {
    /// Materialized paths of granted nodes (descendant inclusion via prefix rule).
    pub paths: BTreeSet<String>,
    pub node_ids: BTreeSet<ScopeNodeId>,
    pub revision: AuthorizationRevision,
}

impl ScopeSet {
    /// Prefix rule: resource path is inside the scope iff it equals a granted path
    /// or extends one (`GRANT|CHILD...`).
    pub fn contains_path(&self, resource_owner_path: &str) -> bool {
        self.paths
            .iter()
            .any(|p| resource_owner_path == p || resource_owner_path.starts_with(&format!("{p}|")))
    }

    pub fn to_snapshot(&self, computed_at: time::OffsetDateTime) -> ScopeSnapshot {
        ScopeSnapshot {
            nodes: Vec::new(), // node details hydrated by the hydrator when needed
            revision: self.revision,
            computed_at,
        }
    }
}

#[derive(Debug, Default)]
pub struct ScopeGraph {
    nodes: HashMap<ScopeNodeId, ScopeNode>,
    children: HashMap<Option<ScopeNodeId>, Vec<ScopeNodeId>>,
}

impl ScopeGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a node; path is computed from parent (+ validation).
    pub fn insert(
        &mut self,
        id: ScopeNodeId,
        tenant_id: TenantId,
        kind: ScopeNodeKind,
        name: impl Into<String>,
        parent: Option<ScopeNodeId>,
    ) -> Result<&ScopeNode, AppError> {
        let name = name.into();
        if name.is_empty() || name.contains('|') {
            return Err(AppError::validation(format!(
                "invalid scope node name {name:?}"
            )));
        }
        let path = match parent {
            None => name.clone(),
            Some(pid) => {
                let p = self.nodes.get(&pid).ok_or_else(|| {
                    AppError::validation(format!("parent scope node {pid} not found"))
                })?;
                format!("{}|{}", p.path, name)
            }
        };
        let node = ScopeNode {
            id,
            tenant_id,
            kind,
            name,
            path,
            parent,
        };
        self.children.entry(parent).or_default().push(id);
        self.nodes.insert(id, node);
        Ok(&self.nodes[&id])
    }

    pub fn get(&self, id: &ScopeNodeId) -> Option<&ScopeNode> {
        self.nodes.get(id)
    }

    pub fn by_path(&self, path: &str) -> Option<&ScopeNode> {
        self.nodes.values().find(|n| n.path == path)
    }

    /// Effective scope for a set of grants: grant nodes + all descendants.
    pub fn effective_scope(
        &self,
        grants: &[ScopeGrant],
        revision: AuthorizationRevision,
    ) -> Result<ScopeSet, AppError> {
        let mut set = ScopeSet {
            revision,
            ..Default::default()
        };
        for g in grants {
            let root = self.nodes.get(&g.node_id).ok_or_else(|| {
                AppError::validation(format!("grant references unknown scope node {}", g.node_id))
            })?;
            self.collect(root.id, &mut set);
        }
        Ok(set)
    }

    fn collect(&self, id: ScopeNodeId, set: &mut ScopeSet) {
        if let Some(node) = self.nodes.get(&id) {
            if set.node_ids.insert(node.id) {
                set.paths.insert(node.path.clone());
                for child in self.children.get(&Some(id)).cloned().unwrap_or_default() {
                    self.collect(child, set);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph() -> (ScopeGraph, ScopeNodeId, ScopeNodeId, TenantId) {
        let tenant = TenantId::new();
        let mut g = ScopeGraph::new();
        let le1 = ScopeNodeId::new();
        let loc5 = ScopeNodeId::new();
        g.insert(le1, tenant, ScopeNodeKind::LegalEntity, "LE1", None)
            .unwrap();
        g.insert(loc5, tenant, ScopeNodeKind::Location, "LOC5", Some(le1))
            .unwrap();
        (g, le1, loc5, tenant)
    }

    fn grant(node: ScopeNodeId) -> ScopeGrant {
        ScopeGrant {
            membership_id: kelson_foundation::MembershipId::new(),
            node_id: node,
        }
    }

    #[test]
    fn paths_materialize_from_parents() {
        let (g, _le1, loc5, _) = graph();
        assert_eq!(g.get(&loc5).unwrap().path, "LE1|LOC5");
    }

    #[test]
    fn grant_includes_descendants() {
        let (g, le1, _loc5, _) = graph();
        let set = g
            .effective_scope(&[grant(le1)], AuthorizationRevision(7))
            .unwrap();
        assert!(set.paths.contains("LE1"));
        assert!(set.paths.contains("LE1|LOC5"));
    }

    #[test]
    fn contains_path_prefix_rule() {
        let (g, _le1, loc5, _) = graph();
        let set = g
            .effective_scope(&[grant(loc5)], AuthorizationRevision(1))
            .unwrap();
        assert!(set.contains_path("LE1|LOC5"));
        assert!(set.contains_path("LE1|LOC5|WARD2"));
        assert!(!set.contains_path("LE1"));
        assert!(!set.contains_path("LE1|LOC55"));
        assert!(!set.contains_path("LE2|LOC5"));
    }

    #[test]
    fn grant_to_unknown_node_fails() {
        let (g, _, _, _) = graph();
        assert!(g
            .effective_scope(&[grant(ScopeNodeId::new())], AuthorizationRevision(1))
            .is_err());
    }
}
