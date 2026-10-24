//! P1.1 / MNC-001 boot matrix: which membership seam may a boot install?
//!
//! An in-memory membership edge vanishes on restart and would silently
//! re-open the tenant gate, so a PROD boot with no database pool must refuse.
//! Dev/test keeps the degraded-boot fallback (D-52) and still publishes the
//! port.
//!
//! This target needs no database: it exercises `IdentityProvider::init`
//! directly, the same call the lifecycle host makes at Platform phase.

use kelson_demo::host::{IdentityProvider, MembershipStoreRef};
use kelson_lifecycle::{BootError, Provider, ServiceRegistry};

#[test]
fn prod_boot_refuses_in_memory_membership_edges() {
    let provider = IdentityProvider {
        pool: None,
        env: kelson_config::Environment::Prod,
    };
    let mut services = ServiceRegistry::new();
    let err = provider
        .init(&mut services)
        .expect_err("prod boot without a pool must refuse");
    let BootError::ProviderFailed { provider, source } = err else {
        panic!("a refused membership seam is a ProviderFailed");
    };
    assert_eq!(provider, "platform.identity");
    assert!(source.contains("S-001"), "{source}");
}

#[test]
fn dev_boot_keeps_the_in_memory_membership_fallback() {
    let provider = IdentityProvider {
        pool: None,
        env: kelson_config::Environment::Dev,
    };
    let mut services = ServiceRegistry::new();
    provider.init(&mut services).expect("degraded boot serves");
    assert!(
        services.get::<MembershipStoreRef>().is_ok(),
        "the membership port is published"
    );
}
