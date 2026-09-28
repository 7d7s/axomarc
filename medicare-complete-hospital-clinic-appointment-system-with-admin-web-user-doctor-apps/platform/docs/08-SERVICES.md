# 08 — Services

Ports first: every service is a trait in `crates/services/*`; adapters (smtp, S3, MSG91, FCM,
local disk) implement ports and are wired only by runtime binaries.

## Cache (+ locks)

`services/cache` with a memory adapter. `CacheLock` provides atomic delete-if-equals locks for
critical sections.

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
