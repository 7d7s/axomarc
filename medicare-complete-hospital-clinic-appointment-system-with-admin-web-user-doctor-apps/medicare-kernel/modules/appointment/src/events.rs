//! Delta builders (DLS-01): same-tx, immutable, scoped by owner_path.
use kernel_core::{Delta, DeltaAction};
use uuid::Uuid;

use crate::models::Appointment;

pub const STREAM: &str = "appointments";
pub const AGGREGATE: &str = "appointment";

pub fn created(a: &Appointment, causation_id: Option<Uuid>) -> Delta {
    Delta {
        tenant_id: a.tenant_id.0,
        stream: STREAM,
        aggregate: AGGREGATE,
        aggregate_id: *a.id.as_uuid(),
        version: a.version,
        action: DeltaAction::Create,
        state: serde_json::json!({
            "status": a.status,
            "starts_at": a.starts_at.format(&time::format_description::well_known::Rfc3339).unwrap_or_default(),
            "doctor_user_id": a.doctor_user_id,
            "patient_kind": a.patient_kind,
        }),
        changed: serde_json::json!({}),
        owner_path: a.owner_path.clone(),
        actor_id: None, // wired with AuthN (M1)
        capability_grant_id: None,
        causation_id,
    }
}