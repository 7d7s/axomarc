//! Live-Redis integration test for the shared rate-limit store (F-3 Phase 3).
//!
//! Skipped unless `REDIS_URL` points at a reachable Redis — the same
//! "skip cleanly without a database" posture the pg/mysql matrices use, so
//! a local run without Redis stays green.
//!
//! ```text
//! REDIS_URL=redis://127.0.0.1:6379 cargo test -p kelson-ratelimit-redis --features redis-tests
//! ```

#![cfg(feature = "redis-tests")]

use kelson_http::RateLimitStore;
use kelson_ratelimit_redis::RedisRateLimiter;

fn redis_url() -> Option<String> {
    match std::env::var("REDIS_URL") {
        Ok(url) if !url.is_empty() => Some(url),
        _ => {
            eprintln!("redis-tests: REDIS_URL not set; skipping");
            None
        }
    }
}

#[test]
fn shared_store_enforces_one_limit_across_instances() {
    let Some(url) = redis_url() else { return };
    let Ok(limiter) = RedisRateLimiter::connect(&url, 60) else {
        eprintln!("redis-tests: redis unreachable; skipping");
        return;
    };

    // Two "instances" (two connections) share the bucket through Redis.
    let other = RedisRateLimiter::connect(&url, 60).expect("second connection");

    let key = format!("conformance-{}", uuid::Uuid::now_v7());
    let capacity = 3u32;
    let refill = 0.0f64; // no refill: the bucket drains and stays empty

    // The first instance spends the whole burst.
    for i in 0..capacity {
        assert!(
            limiter.take(&key, capacity, refill),
            "request {i} of the burst is allowed"
        );
    }
    // The second instance sees the SAME bucket: the limit is one limit.
    assert!(
        !other.take(&key, capacity, refill),
        "the shared bucket is empty — the limit is enforced across instances"
    );
    assert!(
        !limiter.take(&key, capacity, refill),
        "and the first instance sees it empty too"
    );
}

#[test]
fn a_different_client_has_its_own_bucket() {
    let Some(url) = redis_url() else { return };
    let Ok(limiter) = RedisRateLimiter::connect(&url, 60) else {
        eprintln!("redis-tests: redis unreachable; skipping");
        return;
    };
    let key = format!("conformance-{}", uuid::Uuid::now_v7());
    assert!(limiter.take(&key, 1, 0.0));
    assert!(
        limiter.take(&format!("{key}-other"), 1, 0.0),
        "a different client key has its own bucket"
    );
}
