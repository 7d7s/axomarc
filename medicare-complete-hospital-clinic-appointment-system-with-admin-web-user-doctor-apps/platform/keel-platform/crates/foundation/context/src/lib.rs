//! # kelson-context
//! `ExecutionContext` — the immutable, fully-resolved carrier of authority for one
//! execution (spec §2). No ambient state, no mutation after `build()`.
//!
//! **Immutability audit (S1):** `ExecutionContext` is only produced by
//! `ContextBuilder::build()`, returned as `Arc<ExecutionContext>`, and carries no
//! setters, `&mut` APIs or interior mutability. Security-critical fields
//! (plane, principal, tenant, scope, authorization, trace) are therefore
//! unmodifiable after build — enforcement is structural, not conventional.
//!
//! Identity separation (D lineage): **Principal** (who holds authority) ≠ **Actor**
//! (who physically acts) ≠ **Membership** (the edge that connects them).

use std::collections::BTreeSet;

use kelson_foundation::{
    ActorId, AppError, AuthorizationRevision, CorrelationId, Deadline, ExecutionId,
    ExecutionProfile, ExecutionProfileKind, FeatureRevision, LeaseId, Locale, PrincipalId,
    RequestId, ScopeNodeId, SecurityPlane, TenantId, TimeZone, TraceId,
};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrincipalContext {
    pub id: PrincipalId,
    pub display: Option<String>,
}

/// How the actor relates to the authority being exercised.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ActorMode {
    Own,
    Delegation {
        delegator: PrincipalId,
    },
    /// Platform support-mode via an elevation lease (P4/P5).
    Support {
        lease_id: LeaseId,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActorContext {
    pub id: ActorId,
    pub mode: ActorMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantContext {
    pub id: TenantId,
    pub slug: String,
    pub plan: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeNodeRef {
    pub id: ScopeNodeId,
    /// LE / BU / REGION / LOCATION / BRANCH / DEPARTMENT / COST_CENTER / WAREHOUSE / custom.
    pub kind: String,
    /// Materialized path, e.g. `LE1|LOC5`.
    pub path: String,
}

/// Frozen effective scope: computed once (memberships → scope graph), revision-pinned.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopeSnapshot {
    pub nodes: Vec<ScopeNodeRef>,
    pub revision: AuthorizationRevision,
    #[serde(with = "time::serde::iso8601")]
    pub computed_at: OffsetDateTime,
}

/// Ownership fact: which scope owns a resource (separate from "where may I operate").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OwnershipRef {
    pub resource_type: String,
    pub resource_id: Uuid,
    pub owner_scope_path: String,
    pub tenant_id: TenantId,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OwnershipSnapshot {
    pub entries: Vec<OwnershipRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthorizationSnapshot {
    pub revision: AuthorizationRevision,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeatureSnapshot {
    pub revision: FeatureRevision,
    pub flags: BTreeSet<String>,
}

/// Immutable after construction (S1 audit — see crate docs). Handlers receive
/// `Arc<ExecutionContext>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionContext {
    pub execution_id: ExecutionId,
    pub request_id: Option<RequestId>,
    pub correlation_id: CorrelationId,
    pub causation_id: Option<CorrelationId>,
    /// Observability join id for this execution's log/span trail (S1).
    pub trace_id: TraceId,

    pub plane: SecurityPlane,
    pub principal: PrincipalContext,
    pub actor: ActorContext,
    /// None only for platform-plane executions (provisioning, registry ops).
    pub tenant: Option<TenantContext>,
    pub scope: Option<ScopeSnapshot>,
    pub ownership: OwnershipSnapshot,
    pub authorization: Option<AuthorizationSnapshot>,
    pub feature: Option<FeatureSnapshot>,

    pub profile: ExecutionProfile,
    pub deadline: Deadline,
    pub locale: Locale,
    /// IANA timezone for display/business-day semantics; instants stay UTC (S1).
    pub timezone: TimeZone,
    /// The **credential's** narrowing (Sanctum's `abilities`): the
    /// capabilities this credential may exercise, and nothing else.
    ///
    /// `None` = the credential carries no narrowing (a session acts as the
    /// whole principal ✓). `Some(list)` = the decision is confined to that
    /// list ✓ — the authorization boundary intersects the granted
    /// capabilities with it ✓, so a narrowly-scoped token cannot act as the
    /// whole account ✓.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_abilities: Option<Vec<String>>,
}

impl ExecutionContext {
    pub fn builder(kind: ExecutionProfileKind, now: OffsetDateTime) -> ContextBuilder {
        ContextBuilder {
            execution_id: ExecutionId::new(),
            request_id: None,
            correlation_id: CorrelationId::new(),
            causation_id: None,
            trace_id: TraceId::new(),
            plane: SecurityPlane::Tenant,
            principal: None,
            actor: None,
            tenant: None,
            scope: None,
            ownership: OwnershipSnapshot::default(),
            authorization: None,
            feature: None,
            profile: ExecutionProfile::resolve(kind),
            deadline: Deadline::from_timeout(now, ExecutionProfile::resolve(kind).timeout_secs),
            locale: Locale::default(),
            timezone: None,
            credential_abilities: None,
        }
    }

    pub fn tenant_id(&self) -> Option<TenantId> {
        self.tenant.as_ref().map(|t| t.id)
    }
}

/// The only way to obtain a context. Validation happens here, once.
#[derive(Debug, Clone)]
pub struct ContextBuilder {
    execution_id: ExecutionId,
    request_id: Option<RequestId>,
    correlation_id: CorrelationId,
    causation_id: Option<CorrelationId>,
    trace_id: TraceId,
    plane: SecurityPlane,
    principal: Option<PrincipalContext>,
    actor: Option<ActorContext>,
    tenant: Option<TenantContext>,
    scope: Option<ScopeSnapshot>,
    ownership: OwnershipSnapshot,
    authorization: Option<AuthorizationSnapshot>,
    feature: Option<FeatureSnapshot>,
    profile: ExecutionProfile,
    deadline: Deadline,
    locale: Locale,
    timezone: Option<TimeZone>,
    credential_abilities: Option<Vec<String>>,
}

macro_rules! setter {
    ($name:ident, $field:ident, $ty:ty) => {
        pub fn $name(mut self, value: $ty) -> Self {
            self.$field = value;
            self
        }
    };
}

macro_rules! setter_opt {
    ($name:ident, $field:ident, $ty:ty) => {
        pub fn $name(mut self, value: $ty) -> Self {
            self.$field = Some(value);
            self
        }
    };
}

impl ContextBuilder {
    setter_opt!(request_id, request_id, RequestId);
    setter!(correlation_id, correlation_id, CorrelationId);
    setter!(causation_id, causation_id, Option<CorrelationId>);
    setter!(trace_id, trace_id, TraceId);
    setter!(plane, plane, SecurityPlane);
    setter_opt!(principal, principal, PrincipalContext);
    setter_opt!(actor, actor, ActorContext);
    setter_opt!(tenant, tenant, TenantContext);
    setter_opt!(scope, scope, ScopeSnapshot);
    setter!(ownership, ownership, OwnershipSnapshot);
    setter_opt!(authorization, authorization, AuthorizationSnapshot);
    setter_opt!(feature, feature, FeatureSnapshot);
    setter!(deadline, deadline, Deadline);
    setter!(locale, locale, Locale);
    setter_opt!(timezone, timezone, TimeZone);
    /// Confine this execution to the credential's **abilities** (Sanctum's
    /// narrowing ✓). The authorization boundary intersects the granted
    /// capabilities with the list ✓ — a narrowly-scoped token can then only
    /// exercise what it was granted ✓.
    pub fn with_credential_abilities(
        mut self,
        abilities: impl Into<Vec<String>>,
    ) -> Self {
        self.credential_abilities = Some(abilities.into());
        self
    }

    pub fn build(self) -> Result<std::sync::Arc<ExecutionContext>, AppError> {
        let principal = self
            .principal
            .ok_or_else(|| AppError::validation("execution context requires a principal"))?;
        let actor = self.actor.unwrap_or_else(|| ActorContext {
            id: ActorId::from_uuid(*principal.id.as_uuid()),
            mode: ActorMode::Own,
        });
        if self.plane == SecurityPlane::Tenant && self.tenant.is_none() {
            return Err(AppError::validation(
                "tenant-plane execution requires a tenant context",
            ));
        }
        if self.scope.is_none()
            && self.plane == SecurityPlane::Tenant
            && !self.profile.is_readonly()
        {
            return Err(AppError::validation(
                "tenant-plane mutation requires a resolved scope snapshot",
            ));
        }
        // Timezone: explicit override wins; otherwise derived from locale (S1).
        let timezone = self.timezone.unwrap_or_else(|| {
            TimeZone::parse(self.locale.timezone.clone()).unwrap_or_else(|_| TimeZone::utc())
        });
        Ok(std::sync::Arc::new(ExecutionContext {
            execution_id: self.execution_id,
            request_id: self.request_id,
            correlation_id: self.correlation_id,
            causation_id: self.causation_id,
            trace_id: self.trace_id,
            plane: self.plane,
            principal,
            actor,
            tenant: self.tenant,
            scope: self.scope,
            ownership: self.ownership,
            authorization: self.authorization,
            feature: self.feature,
            profile: self.profile,
            deadline: self.deadline,
            locale: self.locale,
            timezone,
            credential_abilities: self.credential_abilities,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn principal() -> PrincipalContext {
        PrincipalContext {
            id: PrincipalId::new(),
            display: None,
        }
    }

    fn tenant() -> TenantContext {
        TenantContext {
            id: TenantId::new(),
            slug: "acme".into(),
            plan: "standard".into(),
        }
    }

    fn scope() -> ScopeSnapshot {
        ScopeSnapshot {
            nodes: vec![],
            revision: AuthorizationRevision(1),
            computed_at: OffsetDateTime::now_utc(),
        }
    }

    #[test]
    fn tenant_plane_requires_tenant() {
        let ctx =
            ExecutionContext::builder(ExecutionProfileKind::Command, OffsetDateTime::now_utc())
                .principal(principal())
                .build();
        assert!(ctx.is_err());
    }

    #[test]
    fn mutation_requires_scope() {
        let ctx =
            ExecutionContext::builder(ExecutionProfileKind::Command, OffsetDateTime::now_utc())
                .principal(principal())
                .tenant(tenant())
                .build();
        assert!(ctx.is_err());
    }

    #[test]
    fn full_context_builds() {
        let ctx =
            ExecutionContext::builder(ExecutionProfileKind::Command, OffsetDateTime::now_utc())
                .principal(principal())
                .tenant(tenant())
                .scope(scope())
                .build()
                .expect("should build");
        assert_eq!(ctx.plane, SecurityPlane::Tenant);
        assert!(ctx.tenant_id().is_some());
    }

    #[test]
    fn platform_plane_allows_no_tenant() {
        let ctx =
            ExecutionContext::builder(ExecutionProfileKind::System, OffsetDateTime::now_utc())
                .plane(SecurityPlane::Platform)
                .principal(principal())
                .build();
        assert!(ctx.is_ok());
    }

    #[test]
    fn trace_id_autogenerated_and_overridable() {
        let a = ExecutionContext::builder(ExecutionProfileKind::Read, OffsetDateTime::now_utc())
            .plane(SecurityPlane::Platform)
            .principal(principal())
            .build()
            .unwrap();
        let explicit = TraceId::new();
        let b = ExecutionContext::builder(ExecutionProfileKind::Read, OffsetDateTime::now_utc())
            .plane(SecurityPlane::Platform)
            .principal(principal())
            .trace_id(explicit)
            .build()
            .unwrap();
        assert_ne!(a.trace_id, b.trace_id);
        assert_eq!(b.trace_id, explicit);
    }

    #[test]
    fn timezone_defaults_from_locale_and_overrides() {
        let ctx = ExecutionContext::builder(ExecutionProfileKind::Read, OffsetDateTime::now_utc())
            .plane(SecurityPlane::Platform)
            .principal(principal())
            .build()
            .unwrap();
        // Locale::default carries Asia/Kolkata; timezone derives from it.
        assert_eq!(ctx.timezone.as_str(), "Asia/Kolkata");

        let ctx = ExecutionContext::builder(ExecutionProfileKind::Read, OffsetDateTime::now_utc())
            .plane(SecurityPlane::Platform)
            .principal(principal())
            .timezone(TimeZone::utc())
            .build()
            .unwrap();
        assert_eq!(ctx.timezone, TimeZone::utc());
    }
}
