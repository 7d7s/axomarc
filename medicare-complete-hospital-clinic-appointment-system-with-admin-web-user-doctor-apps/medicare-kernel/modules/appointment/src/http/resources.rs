//! Response DTOs (= Laravel API Resources, D-43). version + allowed_events
//! let clients render correctly without client-side state machines.
use serde::Serialize;

use crate::models::Appointment;
use crate::workflows;

#[derive(Debug, Serialize)]
pub struct AppointmentResp {
    pub id: uuid::Uuid,
    pub status: &'static str,
    pub starts_at: String,
    pub doctor_user_id: uuid::Uuid,
    pub allowed_events: Vec<&'static str>,
    pub version: i64,
}

impl From<&Appointment> for AppointmentResp {
    fn from(a: &Appointment) -> Self {
        Self {
            id: *a.id.as_uuid(),
            status: a.status,
            starts_at: a
                .starts_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
            doctor_user_id: a.doctor_user_id,
            allowed_events: workflows::machine().allowed_events(a.status),
            version: a.version,
        }
    }
}