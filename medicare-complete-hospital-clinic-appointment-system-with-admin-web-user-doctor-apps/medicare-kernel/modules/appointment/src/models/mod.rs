//! Domain (Tier A): appointments aggregate + booking invariants.
//! TODO(M4-05): Slot moves to modules/doctor; skeleton keeps it here.
use kernel_core::{AppointmentId, KernelError, NodeId, SlotId, TenantId};
use time::OffsetDateTime;
use uuid::Uuid;

pub const STATUS_SCHEDULED: &str = "SCHEDULED";

#[derive(Debug, Clone)]
pub struct Slot {
    pub id: SlotId,
    pub node_id: NodeId,
    pub node_path: String,
    pub doctor_user_id: Uuid,
    pub starts_at: OffsetDateTime,
    pub state: String,
}

#[derive(Debug, Clone)]
pub struct Appointment {
    pub id: AppointmentId,
    pub tenant_id: TenantId,
    pub owner_node: NodeId,
    pub owner_path: String,
    pub slot_id: SlotId,
    pub patient_kind: String,
    pub patient_id: Option<Uuid>,
    pub doctor_user_id: Uuid,
    pub status: &'static str,
    pub starts_at: OffsetDateTime,
    pub version: i64,
}

impl Appointment {
    /// Pure booking decision: invariants enforced before any write (10-GUIDELINES §4.3).
    pub fn book(
        tenant_id: TenantId,
        now: OffsetDateTime,
        slot: &Slot,
        patient_kind: &str,
        patient_id: Option<Uuid>,
    ) -> Result<Self, KernelError> {
        if slot.state != "open" {
            return Err(KernelError::ConflictSlotTaken);
        }
        if slot.starts_at <= now {
            return Err(KernelError::ValidationFailed("slot is in the past".into()));
        }
        Ok(Self {
            id: AppointmentId::new(),
            tenant_id,
            owner_node: slot.node_id,
            owner_path: slot.node_path.clone(),
            slot_id: slot.id,
            patient_kind: patient_kind.to_string(),
            patient_id,
            doctor_user_id: slot.doctor_user_id,
            status: STATUS_SCHEDULED,
            starts_at: slot.starts_at,
            version: 1,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    fn slot(state: &'static str, starts_at: OffsetDateTime) -> Slot {
        Slot {
            id: SlotId::new(),
            node_id: NodeId::new(),
            node_path: "LE1|LOC5".into(),
            doctor_user_id: Uuid::now_v7(),
            starts_at,
            state: state.into(),
        }
    }

    #[test]
    fn books_open_future_slot() {
        let now = OffsetDateTime::now_utc();
        let s = slot("open", now + Duration::hours(2));
        let a = Appointment::book(TenantId::new(), now, &s, "self", None).unwrap();
        assert_eq!(a.status, STATUS_SCHEDULED);
        assert_eq!(a.version, 1);
        assert_eq!(a.owner_path, "LE1|LOC5");
    }

    #[test]
    fn rejects_held_slot() {
        let now = OffsetDateTime::now_utc();
        let s = slot("held", now + Duration::hours(2));
        assert!(matches!(
            Appointment::book(TenantId::new(), now, &s, "self", None),
            Err(KernelError::ConflictSlotTaken)
        ));
    }

    #[test]
    fn rejects_past_slot() {
        let now = OffsetDateTime::now_utc();
        let s = slot("open", now - Duration::minutes(5));
        assert!(matches!(
            Appointment::book(TenantId::new(), now, &s, "self", None),
            Err(KernelError::ValidationFailed(_))
        ));
    }
}