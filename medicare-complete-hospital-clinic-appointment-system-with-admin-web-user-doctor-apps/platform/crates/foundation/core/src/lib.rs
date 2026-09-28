//! # keel-foundation
//! The trusted computing base. Dependency rule (11-FOUNDATION-MASTER-SPEC §9):
//! core depends on core + a pinned minimal external set only. Nothing above.
//!
//! Contents: typed ids · AppError protocol · security planes · execution profiles ·
//! revision domains · DeltaStone · money · clock · locale/deadline · secrets.

pub mod clock;
pub mod delta;
pub mod error;
pub mod id;
pub mod locale;
pub mod money;
pub mod plane;
pub mod profile;
pub mod revision;
pub mod secret;
pub mod strings;
pub mod telemetry;
pub mod translate;

pub use clock::{Clock, FixedClock, SystemClock};
pub use delta::{DeltaKind, DeltaStone, StreamRef};
pub use error::{codes, AppError, ErrorCategory, Retryability, Severity};
pub use id::{
    ActorId, AuditId, CorrelationId, CorrelationId as CausationIdAlias, DeltaId, EventId, ExecutionId,
    IncidentId, LeaseId, MembershipId, OperationId, OutboxId, PrincipalId, RequestId, ScopeNodeId,
    SpanId, StreamId, TenantId, TraceId,
};
pub use locale::{Deadline, Locale, TimeZone};
pub use money::MoneyCents;
pub use plane::SecurityPlane;
pub use profile::{ConcurrencyStrategy, ExecutionProfile, ExecutionProfileKind, RetryPolicy};
pub use revision::{
    AuthorizationRevision, DeltaSequence, FeatureRevision, ModuleRevision, PlatformRevision,
    SchemaRevision, TenantRevision,
};
pub use secret::{open_for_key, Envelope, Sealer, Secret};
pub use strings::{to_camel_case, to_kebab_case, to_slug, to_snake_case, to_studly_case};
pub use telemetry::{TelemetryContext, MAX_BAGGAGE_ENTRIES};
pub use translate::{translate, MessageCatalog, Translated};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_id_parse_rules() {
        assert!(OperationId::parse("appointment.book").is_ok());
        assert!(OperationId::parse("inventory.adjust-stock").is_ok());
        assert!(OperationId::parse("no-dot").is_err());
        assert!(OperationId::parse("Upper.case").is_err());
        assert!(OperationId::parse("a..b").is_err());
        assert_eq!(
            OperationId::parse("appointment.book").unwrap().module(),
            "appointment"
        );
    }

    #[test]
    fn profiles_resolve_deterministically() {
        let fin = ExecutionProfile::resolve(ExecutionProfileKind::Financial);
        assert!(fin.idempotency_required && fin.delta_required && fin.audit_required);
        assert_eq!(fin.concurrency, ConcurrencyStrategy::Serialized);
        let read = ExecutionProfile::resolve(ExecutionProfileKind::Read);
        assert!(read.is_readonly() && !read.transaction_required);
    }

    #[test]
    fn revision_staleness() {
        let r142 = AuthorizationRevision(142);
        let r143 = AuthorizationRevision(143);
        assert!(r142.is_stale(r143));
        assert!(!r143.is_stale(r142));
        assert_eq!(r142.next(), r143);
    }

    #[test]
    fn money_validation() {
        assert!(MoneyCents::new(1000, "INR").is_ok());
        assert!(MoneyCents::new(1000, "inr").is_err());
    }

    #[test]
    fn timezone_parse_rules() {
        assert!(TimeZone::parse("Asia/Kolkata").is_ok());
        assert!(TimeZone::parse("America/Argentina/Buenos_Aires").is_ok());
        assert!(TimeZone::parse("UTC").is_ok());
        assert!(TimeZone::parse("not a zone").is_err());
        assert!(TimeZone::parse("Asia/").is_err());
        assert_eq!(TimeZone::utc().as_str(), "UTC");
        assert_eq!(TimeZone::default(), TimeZone::utc());
    }

    #[test]
    fn trace_id_is_typed_and_ordered() {
        let a = TraceId::new();
        let b = TraceId::new();
        assert_ne!(a, b);
        assert_eq!(a.to_string(), a.as_uuid().to_string());
    }
}
