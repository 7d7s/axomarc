//! SmartRouter metadata registry (SRS RT-01..05). Gate evaluation order:
//! version → module → plan tier → flag → canary → capability (02-SYSTEM-DESIGN §6).
//! Skeleton stores the compiled route table; per-request gates land in M3.
use kernel_core::{ApiVersion, RouteSpec};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateVerdict {
    Allowed,
    WrongVersion,
    ModuleDisabled,
    PlanTooLow,
    FlagOff,
    NotInCanary,
    CapabilityDenied,
}

pub struct RouteRegistry {
    specs: Vec<RouteSpec>,
}

impl RouteRegistry {
    pub fn from_modules(modules: &[std::sync::Arc<dyn crate::HttpModule>]) -> Self {
        let specs = modules.iter().flat_map(|m| m.routes()).collect();
        Self { specs }
    }

    pub fn find(&self, method: &str, path: &str) -> Option<&RouteSpec> {
        self.specs.iter().find(|s| s.method.eq_ignore_ascii_case(method) && s.path == path)
    }

    /// Registered route inventory → feeds OpenAPI emission (D-38: xtask, zero deps).
    pub fn inventory(&self) -> Vec<(String, String, String, Option<String>)> {
        self.specs
            .iter()
            .map(|s| {
                (
                    s.method.to_string(),
                    s.path.to_string(),
                    s.module.to_string(),
                    s.capability.map(str::to_string),
                )
            })
            .collect()
    }

    /// Version gate (cheapest first). Full gate chain lands in M3-01.
    pub fn version_allows(spec: &RouteSpec, v: ApiVersion) -> GateVerdict {
        if v >= spec.min_api_version {
            GateVerdict::Allowed
        } else {
            GateVerdict::WrongVersion
        }
    }
}