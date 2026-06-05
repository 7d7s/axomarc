pub mod basic;
pub mod binary;
pub mod proxy;
pub mod runtime;
pub mod secrets;
pub mod storage;
pub mod system;

use crate::check::Check;

/// Returns the 18 V0 basic-level checks: 3 per category for the
/// 6 categories the basic level covers (System, Binary, Storage,
/// Runtime, Proxy, Secrets).
pub fn basic_level_checks() -> Vec<Box<dyn Check>> {
    let mut checks: Vec<Box<dyn Check>> = Vec::with_capacity(18);
    checks.extend(system::basic_checks());
    checks.extend(binary::basic_checks());
    checks.extend(storage::basic_checks());
    checks.extend(runtime::basic_checks());
    checks.extend(proxy::basic_checks());
    checks.extend(secrets::basic_checks());
    checks
}
