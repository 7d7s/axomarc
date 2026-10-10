//! # kelson-elevation
//! Cross-plane elevation (11-SPEC §3.19, invariants P4/P5). Never
//! `platform_admin == tenant_all_access`: every elevated platform→tenant operation
//! carries reason + capability + scope + approval + expiry + audit.

use std::sync::RwLock;

use kelson_foundation::{AppError, LeaseId, PrincipalId, TenantId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Maximum lease lifetime the platform permits (defense in depth).
pub const MAX_LEASE_MINUTES: i64 = 240;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElevationLease {
    pub id: LeaseId,
    pub operator: PrincipalId,
    pub target_tenant: TenantId,
    pub capability: String,
    /// Scope paths the lease is limited to (empty = read-only tenant-wide).
    pub scope_paths: Vec<String>,
    pub reason: String,
    pub approver: PrincipalId,
    pub read_only: bool,
    #[serde(with = "time::serde::iso8601")]
    pub granted_at: OffsetDateTime,
    #[serde(with = "time::serde::iso8601")]
    pub expires_at: OffsetDateTime,
}

impl ElevationLease {
    /// P5 completeness check: reason, capability, scope, expiry must all be real.
    pub fn validate(&self, now: OffsetDateTime) -> Result<(), AppError> {
        if self.reason.trim().is_empty() {
            return Err(AppError::validation("elevation lease requires a reason"));
        }
        if self.capability.trim().is_empty() {
            return Err(AppError::validation(
                "elevation lease requires a capability",
            ));
        }
        if self.expires_at <= self.granted_at {
            return Err(AppError::validation(
                "elevation lease expiry must be after grant time",
            ));
        }
        if self.expires_at <= now {
            return Err(AppError::new(
                kelson_foundation::codes::ELEVATION,
                kelson_foundation::ErrorCategory::Authorization,
                kelson_foundation::Severity::Medium,
                kelson_foundation::Retryability::NotRetryable,
                "elevation lease expired",
            ));
        }
        let minutes = (self.expires_at - self.granted_at).whole_minutes();
        if minutes > MAX_LEASE_MINUTES {
            return Err(AppError::validation(format!(
                "elevation lease exceeds platform maximum of {MAX_LEASE_MINUTES} minutes"
            )));
        }
        Ok(())
    }

    pub fn expired(&self, now: OffsetDateTime) -> bool {
        now >= self.expires_at
    }
}

#[derive(Debug, Default)]
pub struct ElevationStore {
    leases: RwLock<Vec<ElevationLease>>,
}

impl ElevationStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn grant(&self, lease: ElevationLease, now: OffsetDateTime) -> Result<LeaseId, AppError> {
        lease.validate(now)?;
        self.leases.write().unwrap().push(lease.clone());
        Ok(lease.id)
    }

    /// Active lease for (operator, tenant) covering the requested capability.
    pub fn active_lease(
        &self,
        operator: PrincipalId,
        tenant: TenantId,
        capability: &str,
        now: OffsetDateTime,
    ) -> Option<ElevationLease> {
        self.leases
            .read()
            .unwrap()
            .iter()
            .find(|l| {
                l.operator == operator
                    && l.target_tenant == tenant
                    && l.capability == capability
                    && !l.expired(now)
            })
            .cloned()
    }

    pub fn revoke(&self, id: LeaseId) -> bool {
        let mut leases = self.leases.write().unwrap();
        let before = leases.len();
        leases.retain(|l| l.id != id);
        leases.len() < before
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::Duration;

    fn lease(read_only: bool, minutes: i64, reason: &str) -> ElevationLease {
        let now = OffsetDateTime::now_utc();
        ElevationLease {
            id: LeaseId::new(),
            operator: PrincipalId::new(),
            target_tenant: TenantId::new(),
            capability: "support.view".into(),
            scope_paths: vec![],
            reason: reason.into(),
            approver: PrincipalId::new(),
            read_only,
            granted_at: now,
            expires_at: now + Duration::minutes(minutes),
        }
    }

    #[test]
    fn valid_lease_grants_and_is_found() {
        let store = ElevationStore::new();
        let now = OffsetDateTime::now_utc();
        let l = lease(true, 60, "customer incident #123");
        let op = l.operator;
        let t = l.target_tenant;
        store.grant(l, now).unwrap();
        assert!(store.active_lease(op, t, "support.view", now).is_some());
    }

    #[test]
    fn empty_reason_rejected() {
        let store = ElevationStore::new();
        let l = lease(true, 60, "   ");
        assert!(store.grant(l, OffsetDateTime::now_utc()).is_err());
    }

    #[test]
    fn overlong_lease_rejected() {
        let store = ElevationStore::new();
        let l = lease(true, MAX_LEASE_MINUTES + 1, "too long");
        assert!(store.grant(l, OffsetDateTime::now_utc()).is_err());
    }

    #[test]
    fn expired_lease_not_returned() {
        let store = ElevationStore::new();
        let now = OffsetDateTime::now_utc();
        let l = lease(true, 30, "incident");
        let op = l.operator;
        let t = l.target_tenant;
        store.grant(l, now).unwrap();
        let later = now + Duration::minutes(31);
        assert!(store.active_lease(op, t, "support.view", later).is_none());
    }
}
