// `AuditEvent` — the source of truth for "who did what, when, with what
// policy decision." The `audit_event` table is **append-only**: the
// `audit_no_update` and `audit_no_delete` triggers in
// `migrations/0002_audit.sql` will reject any `UPDATE` or `DELETE`.
// The Ed25519 chain (G23, V1) is layered on top of this row.

use super::timestamp::Timestamp;
use serde::{Deserialize, Serialize};

/// What kind of action an audit event records. The set is open — the
/// use case picks the kind. Common kinds are listed as constants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditKind {
    /// `app.create`, `app.update`, `app.delete`.
    AppLifecycle,
    /// `deploy.start`, `deploy.transition`, `deploy.complete`.
    Deploy,
    /// `rollback`.
    Rollback,
    /// `secret.set`, `secret.rotate`, `secret.delete`.
    SecretChange,
    /// `backup.create`, `backup.verify`, `backup.restore`.
    Backup,
    /// `domain.add`, `domain.remove`, `tls.renew`.
    Domain,
    /// `server.add`, `server.drain`, `server.remove`.
    Server,
    /// `user.add`, `user.disable`, `login.success`, `login.failure`.
    User,
    /// Catch-all for `sovereign doctor` events, CLI config changes, etc.
    System,
    /// `webhook.received`, `webhook.dispatched`, `webhook.rejected`.
    Webhook,
}

impl AuditKind {
    /// The string form used in the `audit_event.kind` column.
    #[inline]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::AppLifecycle => "app",
            Self::Deploy => "deploy",
            Self::Rollback => "rollback",
            Self::SecretChange => "secret",
            Self::Backup => "backup",
            Self::Domain => "domain",
            Self::Server => "server",
            Self::User => "user",
            Self::System => "system",
            Self::Webhook => "webhook",
        }
    }
}

impl std::fmt::Display for AuditKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Common audit kinds as `AuditKind` constants for ergonomic call sites.
pub mod kind {
    use super::AuditKind;
    pub const APP_CREATE: AuditKind = AuditKind::AppLifecycle;
    pub const APP_UPDATE: AuditKind = AuditKind::AppLifecycle;
    pub const APP_DELETE: AuditKind = AuditKind::AppLifecycle;
    pub const DEPLOY_START: AuditKind = AuditKind::Deploy;
    pub const DEPLOY_TRANSITION: AuditKind = AuditKind::Deploy;
    pub const DEPLOY_COMPLETE: AuditKind = AuditKind::Deploy;
    pub const ROLLBACK: AuditKind = AuditKind::Rollback;
    pub const SECRET_SET: AuditKind = AuditKind::SecretChange;
    pub const SECRET_ROTATE: AuditKind = AuditKind::SecretChange;
    pub const SECRET_DELETE: AuditKind = AuditKind::SecretChange;
    pub const BACKUP_CREATE: AuditKind = AuditKind::Backup;
    pub const BACKUP_VERIFY: AuditKind = AuditKind::Backup;
    pub const BACKUP_RESTORE: AuditKind = AuditKind::Backup;
    pub const DOMAIN_ADD: AuditKind = AuditKind::Domain;
    pub const DOMAIN_REMOVE: AuditKind = AuditKind::Domain;
    pub const SERVER_ADD: AuditKind = AuditKind::Server;
    pub const SERVER_DRAIN: AuditKind = AuditKind::Server;
    pub const SERVER_REMOVE: AuditKind = AuditKind::Server;
    pub const USER_LOGIN: AuditKind = AuditKind::User;
    pub const DOCTOR_RUN: AuditKind = AuditKind::System;
    pub const WEBHOOK_RECEIVED: AuditKind = AuditKind::Webhook;
    pub const WEBHOOK_DISPATCHED: AuditKind = AuditKind::Webhook;
    pub const WEBHOOK_REJECTED: AuditKind = AuditKind::Webhook;
}

/// A single row in the `audit_event` table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, sqlx::FromRow)]
pub struct AuditEvent {
    /// `None` when the event is freshly built (the storage layer assigns
    /// the autoincrement `id`).
    pub id: Option<i64>,
    pub ts: Timestamp,
    /// `"user:<id>"` for human actors, `"system"`, `"web:github"`, etc.
    pub actor: String,
    pub kind: AuditKind,
    /// Free-form, e.g. `"app:api"`, `"deployment:7f3..."`, `"backup:..."`.
    pub target: Option<String>,
    /// JSON payload; structure depends on `kind`. **Never** contains
    /// plaintext secrets, PII, or stack traces from inside the trust
    /// boundary.
    pub payload: serde_json::Value,
    /// JSON `{allow, deny, requires_approval, reason}`. `None` in V0
    /// (no Rego policy engine yet).
    pub policy_decision: Option<serde_json::Value>,
}

/// Query for [`crate::ports::StoragePort::query_audit`]. All filters are
/// optional; an empty `AuditQuery` returns the most recent 100 events.
#[derive(Debug, Clone, Default)]
pub struct AuditQuery {
    /// Only events at or after this timestamp. `None` = no lower bound.
    pub since: Option<Timestamp>,
    /// Only events strictly before this timestamp. `None` = no upper bound.
    pub until: Option<Timestamp>,
    /// Only events by this actor (e.g. `"user:alice"`).
    pub actor: Option<String>,
    /// Only events of this kind.
    pub kind: Option<AuditKind>,
    /// Only events whose `target` equals this string.
    pub target: Option<String>,
    /// Maximum number of rows to return. Default 100, capped at 1000.
    pub limit: u32,
}

impl AuditQuery {
    /// The effective limit, clamped to `[1, 1000]`.
    pub fn effective_limit(&self) -> u32 {
        match self.limit {
            0 => 100,
            n if n > 1000 => 1000,
            n => n,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_str_is_stable() {
        // Changing these strings is a wire-format break for any auditor
        // who has already exported a log.
        assert_eq!(kind::APP_CREATE.to_string(), "app");
        assert_eq!(kind::DEPLOY_START.to_string(), "deploy");
        assert_eq!(kind::SECRET_SET.to_string(), "secret");
    }

    #[test]
    fn query_effective_limit_clamps() {
        let q = AuditQuery {
            limit: 0,
            ..Default::default()
        };
        assert_eq!(q.effective_limit(), 100);
        let q = AuditQuery {
            limit: 5_000,
            ..Default::default()
        };
        assert_eq!(q.effective_limit(), 1000);
        let q = AuditQuery {
            limit: 50,
            ..Default::default()
        };
        assert_eq!(q.effective_limit(), 50);
    }
}
