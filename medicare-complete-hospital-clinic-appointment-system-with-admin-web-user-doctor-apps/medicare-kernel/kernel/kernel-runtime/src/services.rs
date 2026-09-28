//! Services: shared runtime state injected via axum Extension<Arc<Services>>.
use std::sync::Arc;

use kernel_core::TenantId;
use sqlx::postgres::PgPool;
use time::OffsetDateTime;

pub struct Services {
    pub pool: PgPool,
    pub capability: kernel_capability::Engine,
    /// skeleton-era dev context (M1 replaces with real AuthN): the tenant every
    /// request executes under. Flagged for removal at the M2 gate.
    pub demo_tenant_id: TenantId,
}

impl Services {
    pub fn now(&self) -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }
}

pub type SharedServices = Arc<Services>;