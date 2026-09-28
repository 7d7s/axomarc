//! Revision domains (spec §3 / D-52). Revisions — not TTLs — govern
//! authorization and cache validity. A snapshot pinned to an older revision is stale.

macro_rules! revision {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl $name {
            pub const ZERO: Self = Self(0);
            pub fn next(self) -> Self {
                Self(self.0 + 1)
            }
            /// True when `self` was taken before `current` (safe-path trigger).
            pub fn is_stale(self, current: Self) -> bool {
                self.0 < current.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::ZERO
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

revision!(
    /// Any platform control-plane mutation bumps this.
    PlatformRevision
);
revision!(
    /// Tenant lifecycle/config mutation.
    TenantRevision
);
revision!(
    /// Any authorization-affecting mutation (membership/grant/capability).
    AuthorizationRevision
);
revision!(
    /// Module enable/disable/version change.
    ModuleRevision
);
revision!(
    /// Feature flag change.
    FeatureRevision
);
revision!(
    /// Schema migration applied.
    SchemaRevision
);
revision!(
    /// Per-stream monotonic delta position.
    DeltaSequence
);
