//! Use-case orchestration (10-GUIDELINES §4.2): infra → domain decision → infra → delta.
use kernel_core::KernelError;
use kernel_db::Tx;
use kernel_runtime::Services;
use thiserror::Error;

use crate::http::requests::CreateAppointmentInput;
use crate::models::Appointment;
use crate::{events, repositories};

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error("db: {0}")]
    Db(#[from] kernel_db::DbError),
    #[error(transparent)]
    Domain(#[from] KernelError),
    #[error(transparent)]
    Validation(KernelError),
}

pub async fn create(services: &Services, input: CreateAppointmentInput) -> Result<Appointment, ServiceError> {
    // TODO(M1): tenant from RequestContext subject; demo tenant is the dev bridge.
    let mut tx = Tx::begin(&services.pool, services.demo_tenant_id).await?;
    let slot = repositories::lock_open_slot(&mut tx, input.slot_id).await?;
    let appt = Appointment::book(tx.tenant_id, services.now(), &slot, &input.patient_kind, input.patient_id)?;
    repositories::insert_appointment(&mut tx, &appt).await?;
    repositories::mark_slot_booked(&mut tx, appt.slot_id, appt.id).await?;
    tx.emit(events::created(&appt, None));
    tx.commit(true).await?; // KER-03: commit refused if no delta emitted
    Ok(appt)
}