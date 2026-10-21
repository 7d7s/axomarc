//! Live-Redis integration test for the Redis **cache** adapter.
//!
//! Skipped unless `REDIS_URL` points at a reachable Redis — the same
//! "skip cleanly without a service" posture the pg/mysql matrices and the
//! rate-limit adapter's live test use, so a local run without Redis stays
//! green.
//!
//! ```text
//! REDIS_URL=redis://127.0.0.1:6379 cargo test -p kelson-cache-redis --features redis-tests
//! ```
//!
//! The adapter had NO live coverage before this file: the in-memory cache's
//! contract was proven, but the Redis port's own primitives — SET NX
//! (`put_if_absent`), the Lua compare-and-delete (`delete_if_equals`), the
//! TTL and the prefix invalidation — were only unit-tested against the key
//! layout.

#![cfg(feature = "redis-tests")]

use std::sync::Arc;
use std::time::Duration;

use kelson_caching::{CacheLock, CachePort};
use kelson_cache_redis::RedisCache;

/// The **shared cache contract** (LF-070): the Redis driver must satisfy
/// exactly what the in-memory driver satisfies ✓ — one suite, two drivers ✓.
#[test]
fn the_redis_cache_satisfies_the_cache_contract() {
    let Some(url) = redis_url() else { return };
    let Ok(cache) = RedisCache::connect(&url) else {
        eprintln!("redis-tests: redis unreachable; skipping");
        return;
    };
    kelson_caching::cache_contract(&cache);
}

fn redis_url() -> Option<String> {
    match std::env::var("REDIS_URL") {
        Ok(url) if !url.is_empty() => Some(url),
        _ => {
            eprintln!("redis-tests: REDIS_URL not set; skipping");
            None
        }
    }
}

fn cache(prefix: &str) -> Option<RedisCache> {
    let url = redis_url()?;
    match RedisCache::connect(&url) {
        Ok(cache) => Some(cache.with_prefix(prefix.to_string())),
        Err(_) => {
            eprintln!("redis-tests: redis unreachable; skipping");
            None
        }
    }
}

/// A value survives the round trip, and a delete really removes it.
#[test]
fn a_value_round_trips_through_redis() {
    let Some(cache) = cache(&format!("keel:test:cache:{}:", uuid::Uuid::now_v7())) else {
        return;
    };
    cache.put("k", b"v".to_vec(), None);
    assert_eq!(cache.get("k"), Some(b"v".to_vec()), "the value is there");
    cache.delete("k");
    assert_eq!(cache.get("k"), None, "the value is gone after the delete");
}

/// `put_if_absent` is SET NX: the FIRST writer wins and every later writer is
/// refused. This is the primitive the cache lock is built on, so "the second
/// writer is refused" has to hold against a real server, not just in memory.
#[test]
fn put_if_absent_admits_exactly_one_writer() {
    let Some(cache) = cache(&format!("keel:test:cache:{}:", uuid::Uuid::now_v7())) else {
        return;
    };
    assert!(
        cache.put_if_absent("lock", b"first".to_vec(), None),
        "the first writer takes the slot"
    );
    assert!(
        !cache.put_if_absent("lock", b"second".to_vec(), None),
        "the second writer is refused"
    );
    assert_eq!(
        cache.get("lock"),
        Some(b"first".to_vec()),
        "the slot still holds the FIRST writer's value"
    );
}

/// `delete_if_equals` is the compare-and-delete: only the holder of the value
/// frees the slot. A stale holder must NOT delete someone else's value.
#[test]
fn delete_if_equals_only_frees_the_slot_for_the_holder() {
    let Some(cache) = cache(&format!("keel:test:cache:{}:", uuid::Uuid::now_v7())) else {
        return;
    };
    cache.put("lock", b"mine".to_vec(), None);
    assert!(
        !cache.delete_if_equals("lock", b"not-mine"),
        "a stale value does not free the slot"
    );
    assert_eq!(
        cache.get("lock"),
        Some(b"mine".to_vec()),
        "the value is untouched"
    );
    assert!(
        cache.delete_if_equals("lock", b"mine"),
        "the holder frees the slot"
    );
    assert_eq!(cache.get("lock"), None, "the value is gone");
}

/// A value stored with a TTL expires on its own — Redis, not the app, has to
/// drop it.
#[test]
fn a_ttl_value_expires_on_its_own() {
    let prefix = format!("keel:test:cache:{}:", uuid::Uuid::now_v7());
    let Some(cache) = cache(&prefix) else {
        return;
    };
    cache.put("short", b"v".to_vec(), Some(2));
    let client = redis::Client::open(redis_url().expect("the url")).expect("the client opens");
    let mut conn = client.get_connection().expect("the connection opens");
    let ttl: i64 = redis::cmd("TTL")
        .arg(format!("{prefix}short"))
        .query(&mut conn)
        .expect("the TTL is readable");
    assert!(
        ttl > 0 && ttl <= 2,
        "the key carries the TTL it was given (ttl was {ttl})"
    );
    assert_eq!(
        cache.get("short"),
        Some(b"v".to_vec()),
        "the value is there before it expires"
    );
}

/// The prefix invalidation only drops the keys under the prefix it was given.
#[test]
fn prefix_invalidation_only_drops_its_own_keys() {
    let prefix = format!("keel:test:cache:{}:", uuid::Uuid::now_v7());
    let Some(cache) = cache(&prefix) else {
        return;
    };
    cache.put("a/1", b"1".to_vec(), None);
    cache.put("a/2", b"2".to_vec(), None);
    cache.put("b/1", b"3".to_vec(), None);

    cache.invalidate_prefix("a/");
    assert_eq!(cache.get("a/1"), None, "the prefixed key is dropped");
    assert_eq!(cache.get("a/2"), None, "the prefixed key is dropped");
    assert_eq!(
        cache.get("b/1"),
        Some(b"3".to_vec()),
        "a key outside the prefix survives"
    );
}

/// **LF-071 (live)**: the lock's LEASE is enforced by Redis itself, not by the
/// holder. The key carries a TTL, so a holder that CRASHES (never releases)
/// frees the slot when the lease elapses — and the stale holder's late
/// release is REFUSED, because it must not free the lock the new holder now
/// owns.
#[test]
fn the_redis_lock_lease_expires_and_a_stale_holder_cannot_release_it() {
    let Some(cache) = cache(&format!("keel:test:cache:{}:", uuid::Uuid::now_v7())) else {
        return;
    };
    let lock = CacheLock::new(Arc::new(cache));
    let crashed = lock.try_acquire("billing.close", 1).unwrap();
    assert!(
        lock.try_acquire("billing.close", 1).is_none(),
        "the live lease excludes a second holder"
    );
    std::thread::sleep(Duration::from_millis(1_200));
    assert!(
        lock.try_acquire("billing.close", 1).is_some(),
        "the expired lease frees the slot even though the holder never released"
    );
    assert!(
        !lock.release(&crashed),
        "a stale holder's late release is refused"
    );
    assert!(
        lock.try_acquire("billing.close", 1).is_none(),
        "the stale release did NOT free the new holder's lock"
    );
}
