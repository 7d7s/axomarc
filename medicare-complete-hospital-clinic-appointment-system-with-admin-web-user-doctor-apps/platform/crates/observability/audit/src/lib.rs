//! # keel-audit
//! Sentinel (S5) audit observability: audit event metadata, retention policy
//! types and chain-verification report models. Hashing itself lives in the
//! security layer (`keel-integrity`, D-74) so the data layer can write
//! hash-chained rows inside the engine transaction.

use keel_foundation::{AuditId, TenantId};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Retention posture for audit persistence (config surface).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// Keep audit rows for N days (0 = keep forever).
    pub retention_days: u64,
    /// Partition chains per tenant (isolation default) or globally.
    #[serde(default = "default_partition")]
    pub partition: Partition,
    /// HMAC-sign chain segment heads with the app key.
    #[serde(default)]
    pub sign_batches: bool,
}

fn default_partition() -> Partition {
    Partition::Tenant
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Partition {
    Tenant,
    Global,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            retention_days: 0,
            partition: Partition::Tenant,
            sign_batches: false,
        }
    }
}

/// One row as seen by a verifier (storage-agnostic projection).
#[derive(Debug, Clone)]
pub struct AuditRowView {
    pub id: AuditId,
    pub tenant_id: Option<TenantId>,
    pub chain_id: uuid::Uuid,
    pub prev_hash: Option<[u8; 32]>,
    pub chain_hash: [u8; 32],
    /// Canonical payload string (must match the writer's canonicalization).
    pub canonical: String,
    pub at: OffsetDateTime,
}

/// Where a chain broke, if it did.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChainDivergence {
    pub row_id: AuditId,
    pub position: u64,
    pub reason: DivergenceReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DivergenceReason {
    /// Recomputed hash differs from stored `chain_hash` (row tampered).
    HashMismatch,
    /// Stored `prev_hash` does not equal the previous row's `chain_hash`.
    BrokenLink,
    /// Chain head signature failed verification.
    BadSignature,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationReport {
    pub chain_id: uuid::Uuid,
    pub tenant_id: Option<String>,
    pub rows_checked: u64,
    pub ok: bool,
    pub divergence: Option<ChainDivergence>,
    #[serde(with = "time::serde::iso8601")]
    pub verified_at: OffsetDateTime,
}

impl VerificationReport {
    pub fn clean(chain_id: uuid::Uuid, tenant: Option<TenantId>, rows: u64) -> Self {
        Self {
            chain_id,
            tenant_id: tenant.map(|t| t.to_string()),
            rows_checked: rows,
            ok: true,
            divergence: None,
            verified_at: OffsetDateTime::now_utc(),
        }
    }

    pub fn failed(
        chain_id: uuid::Uuid,
        tenant: Option<TenantId>,
        rows: u64,
        divergence: ChainDivergence,
    ) -> Self {
        Self {
            chain_id,
            tenant_id: tenant.map(|t| t.to_string()),
            rows_checked: rows,
            ok: false,
            divergence: Some(divergence),
            verified_at: OffsetDateTime::now_utc(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retention_defaults_safe() {
        let r = RetentionPolicy::default();
        assert_eq!(r.retention_days, 0); // keep forever unless told otherwise
        assert_eq!(r.partition, Partition::Tenant);
        assert!(!r.sign_batches);
    }

    #[test]
    fn report_shapes() {
        let chain = uuid::Uuid::now_v7();
        let ok = VerificationReport::clean(chain, None, 10);
        assert!(ok.ok && ok.divergence.is_none());
        let bad = VerificationReport::failed(
            chain,
            None,
            3,
            ChainDivergence {
                row_id: AuditId::new(),
                position: 2,
                reason: DivergenceReason::HashMismatch,
            },
        );
        assert!(!bad.ok && bad.divergence.is_some());
    }
}
