//! Delta: the single unit of change (SRS DLS-01..07). Written in the mutation tx.
//! `state` = post-state, `changed` = minimal diff; secrets never appear here.
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeltaAction {
    Create,
    Update,
    Transition,
    Compensate,
}

impl DeltaAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            DeltaAction::Create => "create",
            DeltaAction::Update => "update",
            DeltaAction::Transition => "transition",
            DeltaAction::Compensate => "compensate",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Delta {
    pub tenant_id: Uuid,
    pub stream: &'static str,
    pub aggregate: &'static str,
    pub aggregate_id: Uuid,
    pub version: i64,
    pub action: DeltaAction,
    pub state: serde_json::Value,
    pub changed: serde_json::Value,
    /// hierarchy owner path — the key of delta-level isolation (05-SECURITY-ISOLATION §4)
    pub owner_path: String,
    pub actor_id: Option<Uuid>,
    pub capability_grant_id: Option<Uuid>,
    pub causation_id: Option<Uuid>,
}