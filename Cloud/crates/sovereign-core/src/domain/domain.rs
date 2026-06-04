// `Domain` — a hostname pointed at an `app`, with TLS state tracked
// in-tenant. Per `docs/architecture.md` §3.2 the state machine is
// `Provisioning, Valid, Expired, Failed`.

use super::id::{AppId, DomainId};
use super::timestamp::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash,
    Serialize, Deserialize, sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[sqlx(rename_all = "snake_case")]
pub enum TlsStatus {
    /// ACME challenge in progress; no cert yet.
    Provisioning,
    /// Cert is issued and not within the renewal window.
    Valid,
    /// Cert has lapsed; the proxy stops serving traffic. (Auto-renew should
    /// have prevented this; the state is here for the doctor check.)
    Expired,
    /// Last ACME attempt failed; the operator must intervene.
    Failed,
}

impl TlsStatus {
    pub fn can_transition_to(self, other: Self) -> bool {
        use TlsStatus::*;
        matches!(
            (self, other),
            (Provisioning, Valid)
                | (Provisioning, Failed)
                | (Valid, Expired)
                | (Valid, Provisioning) // re-provision before expiry
        )
    }
}

impl std::fmt::Display for TlsStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Provisioning => "provisioning",
            Self::Valid => "valid",
            Self::Expired => "expired",
            Self::Failed => "failed",
        };
        f.write_str(s)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
pub struct Domain {
    pub id: DomainId,
    pub app_id: AppId,
    pub hostname: String,
    pub tls_status: TlsStatus,
    pub tls_expires: Option<Timestamp>,
    pub created_at: Timestamp,
}

#[derive(Debug, Clone)]
pub struct NewDomain {
    pub app_id: AppId,
    pub hostname: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::id::AppId;

    #[test]
    fn tls_state_machine() {
        use TlsStatus::*;
        assert!(Provisioning.can_transition_to(Valid));
        assert!(Provisioning.can_transition_to(Failed));
        assert!(Valid.can_transition_to(Expired));
        assert!(Valid.can_transition_to(Provisioning));
        // No jumping.
        assert!(!Provisioning.can_transition_to(Expired));
        assert!(!Failed.can_transition_to(Valid));
    }

    #[test]
    fn new_domain_builds() {
        let d = NewDomain {
            app_id: AppId::generate(),
            hostname: "api.example.com".to_string(),
        };
        assert_eq!(d.hostname, "api.example.com");
    }
}
