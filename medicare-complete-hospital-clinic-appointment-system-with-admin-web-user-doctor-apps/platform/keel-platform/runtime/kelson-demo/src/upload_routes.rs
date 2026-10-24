//! Upload edge (S7 C5/B-06): bounded multipart intake over the platform
//! plane. Authenticated via session or bearer PAT; files stream into the
//! StoragePort under uploads/<principal uuid>.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::extract::multipart::MultipartRejection;
use axum::extract::{DefaultBodyLimit, Multipart, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use axum::{Json, Router};
use kelson_foundation::TenantId;
use kelson_http::{Envelope, ResolvedSession, UploadLimits};

use crate::routes::AppState;

/// Upload route body ceiling (admission pre-check skips multipart; the
/// per-route DefaultBodyLimit + per-file caps keep this bounded).
pub const UPLOAD_BODY_LIMIT: usize = 32 * 1024 * 1024;

/// Wave 6 P2: `?disk=<name>` selects the storage disk. Absent =
/// `storage.default` through the registry; an unknown name is a 422 (never
/// a silent fallback to another disk).
#[derive(Debug, Default, serde::Deserialize)]
struct UploadQuery {
    #[serde(default)]
    disk: Option<String>,
}

/// Row 19 (`[files]`): the resolved upload posture for this edge —
/// admission limits, the per-tenant disk quota, and the single-node usage
/// ledger. The ledger is in-memory like the harness queue stats: exact per
/// boot on one node, and tests inject/reset it through the state.
#[derive(Clone, Default)]
pub struct UploadPolicy {
    pub limits: UploadLimits,
    pub quota_bytes: u64,
    pub usage: Arc<Mutex<BTreeMap<TenantId, u64>>>,
}

impl UploadPolicy {
    /// The `[files]` section resolved into the edge posture.
    pub fn from_config(section: &kelson_config::FilesSection) -> Self {
        Self {
            limits: UploadLimits {
                max_parts: section.max_parts,
                max_field_bytes: section.max_field_bytes,
                max_file_bytes: section.max_file_bytes,
                allowed_mime: section.allowed_mime.clone(),
            },
            quota_bytes: section.quota_bytes,
            usage: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    /// The tenant's stored-bytes total this boot.
    pub fn used_bytes(&self, tenant: TenantId) -> u64 {
        self.usage
            .lock()
            .expect("upload ledger")
            .get(&tenant)
            .copied()
            .unwrap_or(0)
    }

    /// Charge a completed ingest to the tenant.
    pub fn credit(&self, tenant: TenantId, bytes: u64) {
        *self
            .usage
            .lock()
            .expect("upload ledger")
            .entry(tenant)
            .or_default() += bytes;
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/uploads", post(upload))
        .layer(DefaultBodyLimit::max(UPLOAD_BODY_LIMIT))
        .with_state(state)
}

async fn upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    session: Option<axum::extract::Extension<ResolvedSession>>,
    Query(params): Query<UploadQuery>,
    multipart: Result<Multipart, MultipartRejection>,
) -> impl IntoResponse {
    let (identity, tenant) =
        match crate::graph_routes::resolve_identity(&state, &headers, session.as_deref()) {
            Ok(resolved) => resolved,
            Err(err) => {
                return state.error_json(&err).into_response();
            }
        };
    // Same tenant resolution the graph dispatch uses (session tenant, else
    // the boot default) — the ledger is keyed exactly like the rows.
    let Some(tenant) = tenant.or(state.default_tenant_id) else {
        let err = kelson_foundation::AppError::unauthenticated();
        return state.error_json(&err).into_response();
    };
    let multipart = match multipart {
        Ok(mp) => mp,
        Err(err) => {
            let err = kelson_foundation::AppError::validation(format!(
                "multipart request required: {err}"
            ));
            return state.error_json(&err).into_response();
        }
    };
    let prefix = format!("{}/", identity.as_uuid());
    // Wave 6 P2: resolve the requested disk before any bytes are read —
    // the registry owns the visibility/port policy per disk.
    let (disk_name, entry) = match state.disks.resolve(params.disk.as_deref()) {
        Ok(resolved) => resolved,
        Err(err) => return state.error_json(&err).into_response(),
    };
    // Row 19: the tenant's stored-bytes ledger. The pre-check refuses an
    // upload that cannot fit at all; the exact check runs after the store
    // (true sizes known) and rolls the batch back when the quota would
    // break — the ledger and the disk always agree.
    let quota = state.uploads.quota_bytes;
    if quota > 0 && state.uploads.used_bytes(tenant) >= quota {
        let err = kelson_foundation::AppError::cap_limit(format!(
            "tenant upload quota reached ({quota} bytes)"
        ));
        return state.error_json(&err).into_response();
    }
    match kelson_http::ingest_multipart(
        multipart,
        &state.uploads.limits,
        &*entry.port,
        disk_name,
        "uploads",
        &prefix,
    )
    .await
    {
        Ok(stored) => {
            let incoming: u64 = stored.iter().map(|f| f.size as u64).sum();
            if quota > 0 && state.uploads.used_bytes(tenant) + incoming > quota {
                // Exact refusal: the just-stored batch is rolled back so a
                // refused upload consumes neither quota nor disk.
                for file in &stored {
                    let _ = entry.port.delete(&file.object);
                }
                let err = kelson_foundation::AppError::cap_limit(format!(
                    "tenant upload quota of {quota} bytes exceeded"
                ));
                return state.error_json(&err).into_response();
            }
            state.uploads.credit(tenant, incoming);
            (
                StatusCode::OK,
                Json(
                    serde_json::to_value(Envelope::ok(serde_json::json!({ "files": stored })))
                        .unwrap_or_default(),
                ),
            )
                .into_response()
        }
        Err(err) => state.error_json(&err).into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Row 19: `[files]` maps 1:1 into the edge posture, and the default
    /// section is the unlimited, crate-default posture.
    #[test]
    fn from_config_maps_every_field() {
        let section = kelson_config::FilesSection {
            max_parts: 3,
            max_field_bytes: 1024,
            max_file_bytes: 2048,
            allowed_mime: vec!["text/csv".into()],
            quota_bytes: 4096,
        };
        let policy = UploadPolicy::from_config(&section);
        assert_eq!(policy.limits.max_parts, 3);
        assert_eq!(policy.limits.max_field_bytes, 1024);
        assert_eq!(policy.limits.max_file_bytes, 2048);
        assert_eq!(policy.limits.allowed_mime, vec!["text/csv".to_string()]);
        assert_eq!(policy.quota_bytes, 4096);
        assert_eq!(policy.used_bytes(TenantId::new()), 0);

        let defaults = UploadPolicy::from_config(&kelson_config::FilesSection::default());
        assert_eq!(defaults.quota_bytes, 0, "no quota by default");
        assert_eq!(defaults.limits.max_file_bytes, 8 * 1024 * 1024);

        let tenant = TenantId::new();
        policy.credit(tenant, 17);
        assert_eq!(policy.used_bytes(tenant), 17, "the ledger credits");
        policy.credit(tenant, 5);
        assert_eq!(policy.used_bytes(tenant), 22, "and accumulates");
        assert_eq!(
            policy.used_bytes(TenantId::new()),
            0,
            "and stays per tenant"
        );
    }
}
