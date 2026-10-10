//! # kelson-policy
//! The authorization decision boundary (TCB boundary, 11-SPEC §3).
//!
//! ALLOW = IdentityValid ∧ MembershipActive ∧ CapabilityGranted ∧ ScopeContainsResource
//!       ∧ OwnershipCompatible ∧ PolicyAllows ∧ ModuleEnabled ∧ ResourceStateAllows
//!       ∧ RevisionCurrent
//!
//! Snapshots pin revisions (D-52): a snapshot pinned to an older revision is stale
//! and forces the safe path (re-hydration), never a silent TTL expiry.

use std::collections::BTreeSet;
use std::sync::RwLock;

use kelson_context::ExecutionContext;
use kelson_contracts::{AuthorizationPort, AuthzDecision, AuthzRequest};
use kelson_foundation::{
    codes, AppError, AuthorizationRevision, FeatureRevision, ModuleRevision, PrincipalId, TenantId,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Condition on a capability grant (ABAC part). Foundation supports eq/in/lte/gte
/// over safe metadata fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    pub field: String,
    pub op: CondOp,
    pub value: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CondOp {
    Eq,
    In,
    Lte,
    Gte,
}

/// Hydrated, revision-pinned authority view for (principal, tenant).
/// Installed by the hydration pipeline (postgres adapter in prod); decisions are
/// pure lookups against it (fast path).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessSnapshot {
    pub principal_id: PrincipalId,
    pub tenant_id: TenantId,
    /// Active membership exists (MembershipActive conjunct).
    pub membership_active: bool,
    /// Granted capability names (`resource.operation`).
    pub capabilities: BTreeSet<String>,
    /// Granted scope paths (descendant inclusion via prefix rule).
    pub scope_paths: BTreeSet<String>,
    pub authorization_revision: AuthorizationRevision,
    pub feature_revision: FeatureRevision,
    pub module_revision: ModuleRevision,
    #[serde(with = "time::serde::iso8601")]
    pub computed_at: OffsetDateTime,
}

impl AccessSnapshot {
    pub fn has_capability(&self, capability: &str) -> bool {
        self.capabilities.contains(capability)
    }

    pub fn scope_contains(&self, resource_owner_path: &str) -> bool {
        self.scope_paths
            .iter()
            .any(|p| resource_owner_path == p || resource_owner_path.starts_with(&format!("{p}|")))
    }
}

/// Everything a single decision needs.
#[derive(Debug, Clone)]
pub struct DecisionInput<'a> {
    pub principal: PrincipalId,
    pub tenant: TenantId,
    pub capability: &'a str,
    /// Owner scope path of the target resource (if the operation targets one).
    pub resource_owner_path: Option<&'a str>,
    /// Resource lifecycle state (e.g. `locked`, `posted`) — state conjunct.
    pub resource_state: Option<&'a str>,
    /// Module enablement at decision time (ModuleEnabled conjunct).
    pub module_enabled: bool,
    /// ABAC fields from the payload/safe metadata (never PHI).
    pub attributes: Option<&'a serde_json::Value>,
    /// Per-capability conditions attached to the grant.
    pub conditions: &'a [Condition],
}

#[derive(Debug, Default)]
pub struct PolicyEngine {
    revision: RwLock<AuthorizationRevision>,
    snapshots: RwLock<std::collections::HashMap<(PrincipalId, TenantId), AccessSnapshot>>,
}

impl PolicyEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn revision(&self) -> AuthorizationRevision {
        *self.revision.read().unwrap()
    }

    /// Any authorization-affecting mutation must bump this.
    pub fn bump_revision(&self) -> AuthorizationRevision {
        let mut r = self.revision.write().unwrap();
        *r = r.next();
        *r
    }

    pub fn install_snapshot(&self, snapshot: AccessSnapshot) {
        self.snapshots
            .write()
            .unwrap()
            .insert((snapshot.principal_id, snapshot.tenant_id), snapshot);
    }

    pub fn snapshot(&self, principal: PrincipalId, tenant: TenantId) -> Option<AccessSnapshot> {
        self.snapshots
            .read()
            .unwrap()
            .get(&(principal, tenant))
            .cloned()
    }

    /// The ALLOW conjunction. Pure and fast: lookups against the frozen snapshot.
    pub fn decide(&self, input: &DecisionInput<'_>) -> AuthzDecision {
        let snap = match self.snapshot(input.principal, input.tenant) {
            Some(s) => s,
            None => {
                return AuthzDecision::deny(
                    "policy.no-snapshot",
                    "no access snapshot (membership not hydrated)",
                )
            }
        };

        // RevisionCurrent — staleness forces the safe path.
        let current = self.revision();
        if snap.authorization_revision.is_stale(current) {
            return AuthzDecision::deny(
                "policy.stale-revision",
                format!(
                    "snapshot revision {} is stale (current {})",
                    snap.authorization_revision, current
                ),
            );
        }

        if !snap.membership_active {
            return AuthzDecision::deny("policy.membership", "membership inactive");
        }

        if !snap.has_capability(input.capability) {
            return AuthzDecision::deny(
                "policy.capability",
                format!("capability {} not granted", input.capability),
            );
        }

        if let Some(owner_path) = input.resource_owner_path {
            if !snap.scope_contains(owner_path) {
                return AuthzDecision::deny(
                    "policy.scope",
                    format!("resource owner {owner_path} outside effective scope"),
                );
            }
        }

        if !input.module_enabled {
            return AuthzDecision::deny("policy.module", "module disabled for tenant");
        }

        if let Some(state) = input.resource_state {
            if !state_allows(state) {
                return AuthzDecision::deny(
                    "policy.state",
                    format!("resource state {state} refuses the operation"),
                );
            }
        }

        for cond in input.conditions {
            if !condition_holds(cond, input.attributes) {
                return AuthzDecision::deny(
                    "policy.condition",
                    format!("condition on {} ({:?}) not satisfied", cond.field, cond.op),
                );
            }
        }

        AuthzDecision::allow("policy.allow")
    }
}

fn state_allows(state: &str) -> bool {
    !matches!(state, "locked" | "archived" | "cancelled")
}

fn condition_holds(cond: &Condition, attributes: Option<&serde_json::Value>) -> bool {
    let Some(attrs) = attributes else {
        return false;
    };
    let Some(actual) = attrs.get(&cond.field) else {
        return false;
    };
    match cond.op {
        CondOp::Eq => actual == &cond.value,
        CondOp::In => cond
            .value
            .as_array()
            .is_some_and(|arr| arr.contains(actual)),
        CondOp::Lte => as_f64(actual)
            .zip(as_f64(&cond.value))
            .is_some_and(|(a, b)| a <= b),
        CondOp::Gte => as_f64(actual)
            .zip(as_f64(&cond.value))
            .is_some_and(|(a, b)| a >= b),
    }
}

fn as_f64(v: &serde_json::Value) -> Option<f64> {
    v.as_f64()
}

/// Adapter that plugs `PolicyEngine` into the execution engine.
/// Reads the capability from the plan; a plan without a capability requirement
/// passes when a snapshot exists (authenticated-only operations).
pub struct PolicyAuthz {
    pub engine: std::sync::Arc<PolicyEngine>,
}

impl AuthorizationPort for PolicyAuthz {
    fn authorize(&self, ctx: &ExecutionContext, request: &AuthzRequest<'_>) -> AuthzDecision {
        let Some(tenant) = ctx.tenant.as_ref() else {
            return AuthzDecision::deny(
                "policy.plane",
                "tenant-plane decision requires a tenant context",
            );
        };
        let Some(capability) = request.capability else {
            return match self.engine.snapshot(ctx.principal.id, tenant.id) {
                Some(s) if s.membership_active => AuthzDecision::allow("policy.authenticated-only"),
                _ => AuthzDecision::deny("policy.no-snapshot", "no access snapshot"),
            };
        };
        let input = DecisionInput {
            principal: ctx.principal.id,
            tenant: tenant.id,
            capability,
            resource_owner_path: None,
            resource_state: None,
            module_enabled: true,
            attributes: None,
            conditions: &[],
        };
        self.engine.decide(&input)
    }
}

/// Helper for tests/dev hydration.
pub fn stale_snapshot_error() -> AppError {
    AppError::new(
        codes::STALE_REVISION,
        kelson_foundation::ErrorCategory::Authorization,
        kelson_foundation::Severity::Medium,
        kelson_foundation::Retryability::Retryable,
        "authorization snapshot stale; re-resolve on the safe path",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn engine_with(
        principal: PrincipalId,
        tenant: TenantId,
        rev: u64,
        caps: &[&str],
        paths: &[&str],
    ) -> PolicyEngine {
        let engine = PolicyEngine::new();
        let snapshot = AccessSnapshot {
            principal_id: principal,
            tenant_id: tenant,
            membership_active: true,
            capabilities: caps.iter().map(|c| c.to_string()).collect(),
            scope_paths: paths.iter().map(|p| p.to_string()).collect(),
            authorization_revision: AuthorizationRevision(rev),
            feature_revision: FeatureRevision(1),
            module_revision: ModuleRevision(1),
            computed_at: OffsetDateTime::now_utc(),
        };
        engine.install_snapshot(snapshot);
        engine
    }

    fn ids() -> (PrincipalId, TenantId) {
        (PrincipalId::new(), TenantId::new())
    }

    fn input<'a>(
        principal: PrincipalId,
        tenant: TenantId,
        capability: &'a str,
        owner: Option<&'a str>,
    ) -> DecisionInput<'a> {
        DecisionInput {
            principal,
            tenant,
            capability,
            resource_owner_path: owner,
            resource_state: None,
            module_enabled: true,
            attributes: None,
            conditions: &[],
        }
    }

    #[test]
    fn allow_conjunction_and_scope_denial() {
        let (principal, tenant) = ids();
        let engine = engine_with(
            principal,
            tenant,
            0,
            &["invoice.post"],
            &["LE1", "LE1|LOC5"],
        );
        assert!(
            engine
                .decide(&input(principal, tenant, "invoice.post", Some("LE1|LOC5")))
                .allowed
        );

        // Same principal, narrower scope: owner outside effective scope.
        let engine = engine_with(principal, tenant, 0, &["invoice.post"], &["BU4"]);
        let denied = engine.decide(&input(principal, tenant, "invoice.post", Some("LE1|LOC5")));
        assert!(!denied.allowed);
        assert_eq!(denied.via, "policy.scope");
    }

    #[test]
    fn stale_revision_forces_safe_path() {
        let (principal, tenant) = ids();
        let engine = engine_with(principal, tenant, 0, &["invoice.post"], &["LE1"]);
        engine.bump_revision();
        let d = engine.decide(&input(principal, tenant, "invoice.post", None));
        assert!(!d.allowed);
        assert_eq!(d.via, "policy.stale-revision");
    }

    #[test]
    fn abac_conditions() {
        let (principal, tenant) = ids();
        let engine = engine_with(principal, tenant, 0, &["invoice.post"], &["LE1"]);
        let conds = vec![Condition {
            field: "amount".into(),
            op: CondOp::Lte,
            value: json!(500000),
        }];
        let attrs_ok = json!({"amount": 400000});
        let mut i = input(principal, tenant, "invoice.post", Some("LE1"));
        i.attributes = Some(&attrs_ok);
        i.conditions = &conds;
        assert!(engine.decide(&i).allowed);

        let attrs_big = json!({"amount": 900000});
        let mut i = input(principal, tenant, "invoice.post", Some("LE1"));
        i.attributes = Some(&attrs_big);
        i.conditions = &conds;
        assert!(!engine.decide(&i).allowed);
    }

    #[test]
    fn missing_capability_denied() {
        let (principal, tenant) = ids();
        let engine = engine_with(principal, tenant, 0, &["stock.view"], &["LE1"]);
        let d = engine.decide(&input(principal, tenant, "invoice.post", None));
        assert!(!d.allowed && d.via == "policy.capability");
    }

    #[test]
    fn resource_state_conjunct() {
        let (principal, tenant) = ids();
        let engine = engine_with(principal, tenant, 0, &["invoice.post"], &["LE1"]);
        let mut i = input(principal, tenant, "invoice.post", Some("LE1"));
        i.resource_state = Some("locked");
        let d = engine.decide(&i);
        assert!(!d.allowed && d.via == "policy.state");
    }
}
