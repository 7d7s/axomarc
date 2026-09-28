//! Typed identifiers (TCB). Every id is a namespaced newtype; raw Uuids do not
//! cross module boundaries. UUID v7 keeps ids time-ordered for btree indexes.

macro_rules! typed_id {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub uuid::Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(uuid::Uuid::now_v7())
            }
            pub fn from_uuid(u: uuid::Uuid) -> Self {
                Self(u)
            }
            pub fn as_uuid(&self) -> &uuid::Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

typed_id!(
    /// Join key of the whole diagnostic chain (request -> execution -> delta -> outbox).
    ExecutionId
);
typed_id!(
    /// Transport-level identifier (HTTP request, CLI invocation...).
    RequestId
);
typed_id!(
    /// Business correlation (spans executions of one business flow).
    CorrelationId
);
typed_id!(
    /// Tenant identifier — the isolation anchor.
    TenantId
);
typed_id!(
    /// Global identity holding authority.
    PrincipalId
);
typed_id!(
    /// Physical actor profile (may differ from principal under delegation/support).
    ActorId
);
typed_id!(
    /// Principal <tenant/organization membership edge.
    MembershipId
);
typed_id!(
    /// Node in the scope graph (LE/BU/Region/Location/Dept/CostCenter/Warehouse/custom).
    ScopeNodeId
);
typed_id!(
    /// Delta stream identifier.
    StreamId
);
typed_id!(
    /// DeltaStone identifier.
    DeltaId
);
typed_id!(
    /// Audit record identifier.
    AuditId
);
typed_id!(
    /// Outbox message identifier.
    OutboxId
);
typed_id!(
    /// Elevation lease identifier (P4/P5).
    LeaseId
);
typed_id!(
    /// Observability trace: joins logs/spans of one execution across services (S1).
    TraceId
);

typed_id!(
    /// Telemetry event identifier (Sentinel S5).
    EventId
);
typed_id!(
    /// Span identifier within a trace (Sentinel S5).
    SpanId
);
typed_id!(
    /// Incident identifier for incident mode (Sentinel S5, D-76).
    IncidentId
);

/// Namespaced operation identifier: `module.operation` (e.g. `appointment.book`).
/// Stable contract — changes require versioning (CONTRACT-023).
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct OperationId(String);

impl OperationId {
    pub fn parse(raw: impl Into<String>) -> Result<Self, crate::AppError> {
        let raw = raw.into();
        let ok = raw.split('.').count() >= 2
            && raw.split('.').all(|seg| {
                !seg.is_empty()
                    && seg.chars().all(|c| {
                        c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-'
                    })
            });
        if ok {
            Ok(Self(raw))
        } else {
            Err(crate::AppError::validation(format!(
                "invalid operation id {raw:?}: expected module.operation (lowercase, dot-separated)"
            )))
        }
    }

    /// Unchecked constructor for compile-time-verified literals (`"module.operation"`).
    /// Registry validation (G11/G12) catches malformed ids in manifests and handlers.
    pub fn from_static(raw: &'static str) -> Self {
        Self(raw.to_string())
    }

    pub fn module(&self) -> &str {
        self.0.split('.').next().unwrap_or_default()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for OperationId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::str::FromStr for OperationId {
    type Err = crate::AppError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

/// Opaque 128-bit random token (hex) for cache locks / one-shot secrets.
/// Not a typed id: deliberately unguessable, never parsed or ordered.
pub fn lock_token() -> String {
    uuid::Uuid::now_v7().as_hyphenated().to_string()
}
