//! Security/operational planes (spec §1). Typed; rules enforced by policy + analyzer.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityPlane {
    /// Ingress/routing/caching/read-sync. Cannot mutate tenant business state (P1).
    Edge,
    /// Identity, provisioning, registries, release state. No implicit tenant authority (P2).
    Platform,
    /// Business resources and domain execution. Cannot mutate platform state (P3).
    Tenant,
}

impl SecurityPlane {
    pub fn as_str(&self) -> &'static str {
        match self {
            SecurityPlane::Edge => "edge",
            SecurityPlane::Platform => "platform",
            SecurityPlane::Tenant => "tenant",
        }
    }
}

impl std::fmt::Display for SecurityPlane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
