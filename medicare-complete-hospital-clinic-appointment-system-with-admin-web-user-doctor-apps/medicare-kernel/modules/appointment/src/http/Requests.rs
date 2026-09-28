//! Wire DTOs + boundary validation (= Laravel Form Requests, D-43).
use kernel_core::{KernelError, SlotId};
use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct PatientRefBody {
    pub kind: String,
    pub id: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
pub struct CreateAppointmentReq {
    pub slot_id: Uuid,
    pub patient_ref: PatientRefBody,
    pub service_id: Option<Uuid>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CreateAppointmentInput {
    pub slot_id: SlotId,
    pub patient_kind: String,
    pub patient_id: Option<Uuid>,
    pub service_id: Option<Uuid>,
    pub reason: Option<String>,
}

impl TryFrom<CreateAppointmentReq> for CreateAppointmentInput {
    type Error = KernelError;

    fn try_from(req: CreateAppointmentReq) -> Result<Self, Self::Error> {
        if !matches!(req.patient_ref.kind.as_str(), "self" | "family") {
            return Err(KernelError::ValidationFailed("patient_ref.kind must be self|family".into()));
        }
        if let Some(r) = &req.reason {
            if r.chars().count() > 500 {
                return Err(KernelError::ValidationFailed("reason too long (max 500)".into()));
            }
        }
        Ok(Self {
            slot_id: SlotId::from(req.slot_id),
            patient_kind: req.patient_ref.kind,
            patient_id: req.patient_ref.id,
            service_id: req.service_id,
            reason: req.reason,
        })
    }
}