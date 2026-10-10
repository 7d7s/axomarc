//! Patients module — tenant-scoped CRUD example for the `your-app` workspace.
//!
//! Every handler executes inside [`TenantGuardedPool::with_tenant`], so the
//! queries run with `kelson.tenant_id` set and PostgreSQL RLS policies
//! enforcing isolation — the handlers never filter by tenant themselves.
//!
//! The module's graph contract ships beside this crate as `module.toml`
//! (the kernel host discovers `modules/*/module.toml` at boot).

use std::sync::Arc;

use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Json;
use kelson_framework::kelson_http;
use kelson_framework::{AppError, TenantGuardedPool, TenantId};

/// Router for the patients module. The composition root (`apps/api`) merges
/// this into its router behind tenant resolution — `Extension<TenantId>` is
/// inserted upstream by the `x-tenant-id` middleware.
pub fn router(pool: Arc<TenantGuardedPool>) -> axum::Router {
    axum::Router::new()
        .route("/api/v1/patients", get(list_patients).post(create_patient))
        .route("/api/v1/patients/{id}", get(get_patient))
        .with_state(pool)
}

/// `AppError` → wire: status from the transport mapping, body is the
/// standard error envelope (public fields only).
fn wire_err(err: AppError) -> Response {
    kelson_http::error_response(&err).into_response()
}

/// List patients (RLS scopes rows to the request's tenant).
async fn list_patients(
    State(pool): State<Arc<TenantGuardedPool>>,
    Extension(tenant): Extension<TenantId>,
) -> Result<Json<Vec<Patient>>, Response> {
    let patients = pool
        .with_tenant(tenant, |tx| {
            Box::pin(async move {
                sqlx::query_as::<_, Patient>(
                    "SELECT id, tenant_id, name, dob, created_at \
                     FROM patients ORDER BY created_at DESC",
                )
                .fetch_all(&mut **tx)
                .await
                .map_err(|e| AppError::persistence(e.to_string()))
            })
        })
        .await
        .map_err(wire_err)?;
    Ok(Json(patients))
}

/// Create a patient; `tenant_id` comes from the RLS session context.
async fn create_patient(
    State(pool): State<Arc<TenantGuardedPool>>,
    Extension(tenant): Extension<TenantId>,
    Json(input): Json<CreatePatientInput>,
) -> Result<(StatusCode, Json<Patient>), Response> {
    let patient = pool
        .with_tenant(tenant, move |tx| {
            Box::pin(async move {
                sqlx::query_as::<_, Patient>(
                    "INSERT INTO patients (id, tenant_id, name, dob, created_at) \
                     VALUES ($1, $2, $3, $4, now()) \
                     RETURNING id, tenant_id, name, dob, created_at",
                )
                .bind(uuid::Uuid::now_v7())
                .bind(*tenant.as_uuid())
                .bind(&input.name)
                .bind(input.dob)
                .fetch_one(&mut **tx)
                .await
                .map_err(|e| AppError::persistence(e.to_string()))
            })
        })
        .await
        .map_err(wire_err)?;
    Ok((StatusCode::CREATED, Json(patient)))
}

/// Read one patient by id (cross-tenant reads miss under RLS → 404).
async fn get_patient(
    State(pool): State<Arc<TenantGuardedPool>>,
    Extension(tenant): Extension<TenantId>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Json<Patient>, Response> {
    let patient = pool
        .with_tenant(tenant, move |tx| {
            Box::pin(async move {
                sqlx::query_as::<_, Patient>(
                    "SELECT id, tenant_id, name, dob, created_at \
                     FROM patients WHERE id = $1",
                )
                .bind(id)
                .fetch_optional(&mut **tx)
                .await
                .map_err(|e| AppError::persistence(e.to_string()))?
                .ok_or_else(|| AppError::not_found("patient"))
            })
        })
        .await
        .map_err(wire_err)?;
    Ok(Json(patient))
}

// ------------------------------------------------------------ wire contracts

#[derive(serde::Deserialize)]
pub struct CreatePatientInput {
    pub name: String,
    pub dob: time::Date,
}

#[derive(serde::Serialize, sqlx::FromRow)]
pub struct Patient {
    pub id: uuid::Uuid,
    pub tenant_id: uuid::Uuid,
    pub name: String,
    pub dob: time::Date,
    pub created_at: time::OffsetDateTime,
}
