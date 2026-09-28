//! RequestContext: built by the pipeline before any handler runs (SRS KER-01).
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ids::{IdentityId, TenantId, UserId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Plane {
    Tenant,
    Platform,
    /// read-only, redacted, watermarked platform session (D-21)
    Support,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum ApiVersion {
    V1,
    V2,
}

impl ApiVersion {
    pub fn as_str(&self) -> &'static str {
        match self {
            ApiVersion::V1 => "v1",
            ApiVersion::V2 => "v2",
        }
    }
}

/// Per-tenant runtime context: plan + enabled module flags (02-SYSTEM-DESIGN §6).
#[derive(Debug, Clone)]
pub struct TenantCtx {
    pub id: TenantId,
    pub plan: String,
    pub enabled_modules: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Subject {
    pub identity_id: IdentityId,
    pub user_id: Option<UserId>,
    pub plane: Plane,
    pub memberships: Vec<Uuid>,
}

#[derive(Debug, Clone)]
pub struct RequestContext {
    pub request_id: String,
    pub subject: Option<Subject>,
    pub tenant: Option<TenantCtx>,
    pub api_version: ApiVersion,
    pub idempotency_key: Option<String>,
}

impl RequestContext {
    pub fn new(request_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            subject: None,
            tenant: None,
            api_version: ApiVersion::V1,
            idempotency_key: None,
        }
    }
}