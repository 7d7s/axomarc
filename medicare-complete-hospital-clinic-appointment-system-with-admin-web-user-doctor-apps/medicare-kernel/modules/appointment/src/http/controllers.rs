//! Thin handlers (10-GUIDELINES §4.1): extract → validate → one service call → wire.
use std::sync::Arc;

use axum::extract::{Extension, Json, Path};
use axum::http::StatusCode;
use uuid::Uuid;

use kernel_core::KernelError;
use kernel_runtime::{Envelope, Services};

use super::requests::{CreateAppointmentInput, CreateAppointmentReq};
use super::resources::AppointmentResp;
use crate::services::{self, ServiceError};

pub async fn create(
    Extension(services): Extension<Arc<Services>>,
    Json(body): Json<CreateAppointmentReq>,
) -> (StatusCode, Json<Envelope<AppointmentResp>>) {
    let input = match CreateAppointmentInput::try_from(body) {
        Ok(i) => i,
        Err(e) => {
            let msg = e.to_string();
            let status = StatusCode::from_u16(e.http_status()).unwrap_or(StatusCode::BAD_REQUEST);
            return (status, Json(Envelope::error_with(&e, &msg)));
        }
    };
    match services::create(&services, input).await {
        Ok(appt) => (
            StatusCode::CREATED,
            Json(Envelope::created(AppointmentResp::from(&appt))),
        ),
        Err(e) => {
            let status = e.http_status();
            let body = Envelope::error_with(e.kernel_error(), &e.to_string());
            (status, Json(body))
        }
    }
}

/// GET /api/v1/appointments/{id} — full impl lands with M5-02.
pub async fn get(Path(_id): Path<Uuid>) -> (StatusCode, Json<Envelope<AppointmentResp>>) {
    let err = KernelError::NotFound;
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(Envelope::error_with(&err, "M5-02 not implemented yet")),
    )
}

/// POST /api/v1/appointments/{id}/transition — workflow kernel wired in M5-01.
pub async fn transition(Path(_id): Path<Uuid>) -> (StatusCode, Json<Envelope<AppointmentResp>>) {
    let err = KernelError::WorkflowInvalidTransition;
    (
        StatusCode::NOT_IMPLEMENTED,
        Json(Envelope::error_with(&err, "M5-01 not implemented yet")),
    )
}

// ServiceError needs a stable kernel mapping for the envelope:
impl ServiceError {
    pub fn kernel_error(&self) -> &KernelError {
        match self {
            ServiceError::Domain(k) | ServiceError::Validation(k) => k,
            ServiceError::Db(_) => &KernelError::Internal,
        }
    }

    pub fn http_status(&self) -> StatusCode {
        let s = self.kernel_error().http_status();
        StatusCode::from_u16(s).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
    }
}