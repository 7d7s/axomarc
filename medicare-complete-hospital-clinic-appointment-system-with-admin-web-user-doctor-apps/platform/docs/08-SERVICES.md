# 08 — Services

Ports first: every service is a trait in `crates/services/*`; adapters (smtp, S3, MSG91, FCM,
local disk) implement ports and are wired only by runtime binaries.

## Cache (+ locks)

`services/cache` with a memory adapter; `adapters/keel-cache-redis` (RedisCache) picks up
`REDIS_URL` for the distributed path. `CacheLock` provides atomic delete-if-equals locks
for critical sections.

Wave 4 adds `CacheService` over the unchanged `CachePort`: `remember(key, ttl, f)` /
`remember_forever`, `add` (atomic `put_if_absent`), `forever`, `pull`, `many`/`put_many`,
`has`, `increment`/`decrement`, `flush` (memory: clear; Redis: `FLUSHDB`); cache tags —
`tags([...]).get/put/flush` (memory: key-prefix namespaces; Redis: SET-backed membership);
atomic locks `lock(key, ttl)` → `get`/`release`/`force_release` plus the closure form
`lock.get(|| ...)`; stampede protection collapses concurrent `remember` misses onto one
leader under a per-key lock with jittered wait. Values are serde_json-serialized at the
service layer — `keel-caching` gains the workspace-budgeted `serde_json` dependency for
it (crate-level addition, no `[workspace.dependencies]` budget change); ports and
adapters are untouched.

## Storage

`services/storage` defines the object port; adapters: local disk (dev) and S3-compatible (minio in
docker-compose). Guards enforce size/type policies.

## Notifications (multi-channel)

`services/notifications` is a hub: mail (sandbox in dev, SMTP live via lettre), SMS (MSG91), push
(FCM). Channels are providers behind one contract; templates and dispatch are audited.

## Realtime

`services/realtime`: BroadcastHub with SSE and WebSocket carriers. Feature-flag gated rollout;
per-tenant channels; used for live UI updates rather than polling.

## HTTP client

`services/http-client`: retry policies, timeouts, redaction of secrets in logs, structured request
logging. Webhooks build on it (S3 surface).

## Search

`services/search` (S6): the contract — `Searchable`, `Index`, `SearchQuery`, `SearchResult`,
`Facet`, `VectorQuery`, `HybridQuery`. Tenancy is part of the contract: every document carries a
tenant tag; queries run inside a tenant and mismatches are invisible.

The in-memory reference engine implements keyword scoring, exact filters, facets, cosine vector
search, and alpha-blended hybrid (`alpha * keyword + (1 - alpha) * similarity`). PostgreSQL/pgvector
and OpenSearch plug in as adapters behind the same port. TestKit ships `FakeSearch`.
