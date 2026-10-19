# kelson-ratelimit-redis

A **shared** rate-limit store for the Kelson HTTP admission edge.

The framework's default store ([`kelson_http::InMemoryRateLimitStore`]) keeps the token buckets in
the process. That is correct for a single-instance deployment, but the documented topology scales
the API horizontally (`platform/docs/21-DEPLOYMENT.md`: an HPA from `minReplicas: 3` to
`maxReplicas: 20`), and an in-process bucket means **every replica enforces its own limit** — the
effective limit multiplies by the replica count.

This adapter puts the bucket in Redis, so the limit is one limit.

## Bucket semantics

- One Redis hash per client key, holding `tokens` and `ts` (the last update, epoch seconds).
- The read → refill → take → write sequence runs **inside a Lua script**, so it is atomic: two
  instances hitting the same client at the same time cannot both take the last token.
- `capacity` / `refill_per_sec` come from the caller (`kelson_http::RateLimitConfig`), so the limit
  stays configured in one place whatever backend is in play.
- Buckets expire after the configured TTL — Redis cleans up the keyspace, so it cannot grow without
  bound.
- The key uses a Redis **hash tag** (`{rl}`) so every bucket lands in one cluster slot, and the
  client key is sanitized so a hostile key can never escape the namespace.

## Failure posture

A Redis outage must not become an API outage: the store **fails open** (the request is allowed) and
logs the failure loudly. A rate limiter is a protection mechanism, not a correctness barrier — the
same posture the cache adapter takes. A deployment that prefers failing closed can wrap the store
and invert the decision at the call site.

## Usage

```rust
use kelson_http::{Admission, RateLimitStore};
use kelson_ratelimit_redis::RedisRateLimiter;
use std::sync::Arc;

let store = RedisRateLimiter::connect("redis://127.0.0.1:6379", 900)?;
let admission = Admission {
    rate_limit_store: Some(Arc::new(store) as Arc<dyn RateLimitStore>),
    ..Admission::default()
};
```

The demo wires it from `REDIS_URL` (the same gate the Redis cache uses): present + reachable → the
shared store; absent or unreachable → the buckets stay in-process.

## Tests

- Unit tests (no Redis needed): the key layout, the client-key sanitizing, the clock.
- Live tests: `REDIS_URL=redis://127.0.0.1:6379 cargo test -p kelson-ratelimit-redis --features
  redis-tests` — they skip cleanly when `REDIS_URL` is unset or Redis is unreachable.
