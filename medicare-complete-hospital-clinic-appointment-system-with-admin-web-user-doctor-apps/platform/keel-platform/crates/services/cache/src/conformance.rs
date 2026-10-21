//! The **cache contract suite** (LF-070): one set of cases every cache
//! driver must satisfy, so a new backend cannot ship with a different
//! contract (the same pattern as the model store's conformance suite ✓).
//!
//! The suite covers what the atlas names for a cache: "keys, value
//! serialization and expiry semantics are consistent" ✓ — the round trip ✓,
//! the delete ✓, the prefix invalidation ✓, the expiry ✓ and the two ATOMIC
//! primitives the cache locks are built on ✓ (`put_if_absent` ✓ and
//! `delete_if_equals` ✓).
//!
//! What it deliberately does NOT cover (each is a driver's own business ✓):
//! the tenant namespace ✓ (that is the tenant-scoped port ✓), the stampede
//! guard ✓ (that is the `CacheService`'s `remember` ✓) and the outage
//! behaviour ✓ (a driver's own policy ✓).
//!
//! The expiry case waits for a real second: the port carries no clock ✓, so
//! a fake clock cannot be used here ✓ — the one-second wait is the price of
//! testing expiry through the port itself ✓.

use std::time::Duration;

use crate::CachePort;

/// The shared cache contract. A driver runs this once and is conformant.
pub fn cache_contract<P: CachePort>(port: &P) {
    values_round_trip(port);
    delete_removes_the_entry(port);
    prefix_invalidation_only_drops_its_own_keys(port);
    put_if_absent_admits_exactly_one_writer(port);
    delete_if_equals_only_frees_the_holders_value(port);
    a_ttl_expires_the_entry(port);
}

fn values_round_trip<P: CachePort>(port: &P) {
    port.put("contract/value", b"payload".to_vec(), None);
    assert_eq!(
        port.get("contract/value"),
        Some(b"payload".to_vec()),
        "a value round-trips byte for byte"
    );
    port.delete("contract/value");
}

fn delete_removes_the_entry<P: CachePort>(port: &P) {
    port.put("contract/deleted", b"x".to_vec(), None);
    port.delete("contract/deleted");
    assert_eq!(
        port.get("contract/deleted"),
        None,
        "a deleted entry is gone"
    );
}

fn prefix_invalidation_only_drops_its_own_keys<P: CachePort>(port: &P) {
    port.put("contract/prefix/a", b"1".to_vec(), None);
    port.put("contract/prefix/b", b"2".to_vec(), None);
    port.put("contract/other", b"3".to_vec(), None);
    port.invalidate_prefix("contract/prefix/");
    assert_eq!(
        port.get("contract/prefix/a"),
        None,
        "the prefixed entry is dropped"
    );
    assert_eq!(
        port.get("contract/other"),
        Some(b"3".to_vec()),
        "an entry outside the prefix survives"
    );
    port.delete("contract/other");
}

fn put_if_absent_admits_exactly_one_writer<P: CachePort>(port: &P) {
    port.delete("contract/slot");
    assert!(
        port.put_if_absent("contract/slot", b"first".to_vec(), None),
        "the first writer takes the slot"
    );
    assert!(
        !port.put_if_absent("contract/slot", b"second".to_vec(), None),
        "the second writer is refused"
    );
    assert_eq!(
        port.get("contract/slot"),
        Some(b"first".to_vec()),
        "the slot still holds the FIRST writer's value"
    );
    port.delete("contract/slot");
}

fn delete_if_equals_only_frees_the_holders_value<P: CachePort>(port: &P) {
    port.delete("contract/lock");
    port.put("contract/lock", b"mine".to_vec(), None);
    assert!(
        !port.delete_if_equals("contract/lock", b"not-mine"),
        "a stale value does not free the slot"
    );
    assert_eq!(
        port.get("contract/lock"),
        Some(b"mine".to_vec()),
        "the value is untouched"
    );
    assert!(
        port.delete_if_equals("contract/lock", b"mine"),
        "the holder frees the slot"
    );
    assert_eq!(port.get("contract/lock"), None, "the value is gone");
}

fn a_ttl_expires_the_entry<P: CachePort>(port: &P) {
    port.put("contract/ttl", b"v".to_vec(), Some(1));
    assert_eq!(
        port.get("contract/ttl"),
        Some(b"v".to_vec()),
        "the entry is there before its TTL"
    );
    std::thread::sleep(Duration::from_millis(1_200));
    assert_eq!(
        port.get("contract/ttl"),
        None,
        "the entry expires on its own — the driver honours the TTL"
    );
}
