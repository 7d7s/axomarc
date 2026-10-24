//! KFA-006: the degraded-boot refusal is derived from the environment, not
//! from one optional config key. Staging/prod must never fall back to volatile
//! in-memory stores when the database is unreachable — a silently
//! non-durable deployment accepts writes it will lose while looking healthy
//! to a naive monitor. Dev/test keeps the degraded fallback (D-52).

use kelson_config::Environment;
use kelson_demo::degraded_boot_refused;

#[test]
fn a_prod_boot_refuses_the_degraded_fallback_without_the_flag() {
    // The exact KFA-006 scenario: prod config, `[database].required` absent
    // (serde default false) — the environment alone must refuse.
    assert!(degraded_boot_refused(false, &Environment::Prod));
}

#[test]
fn a_staging_boot_refuses_the_degraded_fallback_without_the_flag() {
    assert!(degraded_boot_refused(false, &Environment::Staging));
}

#[test]
fn dev_and_test_keep_the_degraded_boot() {
    assert!(!degraded_boot_refused(false, &Environment::Dev));
    assert!(!degraded_boot_refused(false, &Environment::Test));
}

#[test]
fn the_required_flag_refuses_in_any_environment() {
    for env in [
        Environment::Dev,
        Environment::Test,
        Environment::Staging,
        Environment::Prod,
    ] {
        assert!(degraded_boot_refused(true, &env), "flag on, env {env:?}");
    }
}
