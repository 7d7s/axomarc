//! Module contract — the plug-n-play surface (PRD §3 P0, 10-GUIDELINES §2).
//! Every module crate implements `Module`; apps/api registers them.
use crate::context::ApiVersion;

pub struct ModuleManifest {
    pub id: &'static str,
    pub version: &'static str,
    /// semver range of kernel-core this module negotiates with at boot
    pub kernel_range: &'static str,
    pub streams: &'static [&'static str],
    pub capabilities: &'static [&'static str],
    pub depends_on_streams: &'static [&'static str],
}

pub struct RouteSpec {
    pub module: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    /// capability action class guard; None = public (rate-limited) route
    pub capability: Option<&'static str>,
    pub min_api_version: ApiVersion,
    pub notes: &'static str,
}

/// The contract every module fulfils. Router assembly lives in kernel-runtime
/// (`HttpModule`) so this crate stays axum-free.
pub trait Module {
    fn manifest(&self) -> ModuleManifest;
    fn routes(&self) -> Vec<RouteSpec>;
}