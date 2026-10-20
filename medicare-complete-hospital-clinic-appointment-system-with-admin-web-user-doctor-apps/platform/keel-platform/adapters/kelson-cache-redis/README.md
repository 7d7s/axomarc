# kelson-cache-redis

Redis adapter behind two frozen ports:

- `CachePort` (kelson-caching): `GET`, `SET ... [EX ttl]`, `DEL`, prefix
  invalidation via `SCAN MATCH`, `SET NX` for `put_if_absent` and a Lua
  compare-and-delete script for `delete_if_equals` (the lock-holder safety
  primitive). Cache failures degrade to a miss (never panic the caller).
- `JobStore` + `JobStoreAdmin` (kelson-jobs): one JSON item per job
  (`{prefix}jobs:item:{id}`) plus four zsets (pending / running / done /
  dead). Claim is a single Lua round-trip (ZRANGEBYSCORE + ZREM), giving
  SKIP-LOCKED-equivalent exclusivity across workers sharing one Redis.
  Member encodes priority (`{MAX-priority padded}:{id}`) so higher priority
  claims first within the same available instant. Retries use
  `kelson_jobs::backoff_delay`.

Requires Redis >= 5 (EVAL, ZADD, SET NX/EX). Blocking client only: both ports
are sync traits, so the adapter uses redis-rs without the async feature set.
Multi-node Redis Cluster is out of scope (cross-slot key concatenation).

Integration tests run behind `--features redis-tests` with `REDIS_URL` set;
without it they skip.

```text
REDIS_URL=redis://127.0.0.1:6379 cargo test -p kelson-cache-redis --features redis-tests
```

`tests/live_redis.rs` covers the cache port against a real server (round
trip, SET NX admitting exactly one writer, the Lua compare-and-delete not
freeing another holder's slot, TTL expiry, prefix invalidation scoped to its
own keys). `tests/live_jobs.rs` covers the durable job store (enqueue → claim
→ complete, two workers never claiming the same job, tenant scoping, a
failure re-queueing and an exhausted job being dead-lettered).