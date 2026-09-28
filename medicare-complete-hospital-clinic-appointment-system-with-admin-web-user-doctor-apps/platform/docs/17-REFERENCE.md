# 17 — Reference

Quick lookup tables for the platform surface.

## Crate map by class

| Class | Crates |
|---|---|
| foundation | keel-foundation, keel-context, keel-contracts, keel-validation, keel-lifecycle |
| execution | keel-execution (engine), keel-command, keel-query |
| application | keel-config, keel-registry |
| web | keel-http |
| security | keel-identity, keel-policy, keel-scope, keel-ownership, keel-elevation, keel-session, keel-tokens, keel-integrity |
| data | keel-postgres |
| async | keel-event, keel-jobs, keel-workflows, keel-scheduler |
| services | keel-cache, keel-storage, keel-notifications, keel-realtime, keel-http-client, keel-search |
| observability | keel-observability (Sentinel runtime), keel-telemetry, keel-tracing, keel-metrics, keel-audit, keel-pipeline, keel-buffer, keel-export |
| enterprise | keel-tenancy, keel-feature-flags, keel-deployment |
| ai | keel-ai, keel-ai-tools, keel-ai-agents, keel-ai-mcp |
| adapters | keel-cache-memory, keel-mail-smtp, keel-sms-msg91, keel-push-fcm, keel-storage-s3 |
| testing | keel-testkit |
| runtime | keel-api, keel-worker |
| tools | keel-console, keel, keel-analyzer, keel-codegen, xtask |

Package names keep the `keel-*` prefix (D-63); directory names may differ from package names
(`crates/execution/engine` = `keel-execution`).

## Error categories (frozen)

Validation, Authentication, Authorization, NotFound, Conflict, Workflow, Domain, Persistence,
External, ResourceExhausted, Internal. Wire shape never leaks internals.

## Execution profiles

READ, COMMAND, FINANCIAL, INVENTORY, WORKFLOW, BATCH, REPORT, SYSTEM, REPLAY. A profile compiles
to a full plan: authorization, transaction, concurrency, consistency, idempotency, retry, audit,
delta, timeout.

## Execution state machine

```
ACCEPTED -> PLANNED -> AUTHORIZED -> RUNNING -> COMMITTED -> DELIVERED
             |           |            |
             +-> DENIED  +-> REJECTED +-> FAILED -> (RETRY | DEAD)
```

## Stable diagnostic codes (selection)

- `DEP-CLASS`, `DEP-UNKNOWN-CRATE`, `DEP-UNKNOWN-MEMBER` — dependency matrix violations (G07).
- `GRAPH-DUP-MODULE`, `GRAPH-DUP-OP`, `GRAPH-DUP-ROUTE`, `GRAPH-DUP-EVENT`, `GRAPH-ROUTE-OP`,
  `GRAPH-CYCLE`, `GRAPH-MANIFEST` — platform graph violations (G05/G06).
- `KER-*` — kernel error codes; module-prefixed codes for domain errors.

## API governance registry

Public APIs carry stable API-IDs (`crates/foundation/contracts/api-registry.toml`), e.g.
`EXEC-API-0001`, `SEARCH-API-0001`, `AGENT-API-0001`, `MCP-API-0001`. Stability ladder:
stable | beta | experimental | deprecated (with replacement). `keel upgrade` reports them;
`cargo-semver-checks` enforces the Rust surface at release (G14).

## Frozen decisions (D- index)

The authoritative list lives in `specs/STAGE-0-FOUNDATION-SPEC.md` section 15 and
`docs/rust/08-DECISIONS.md` (repo root). Highlights: D-62 dependency policy (justify table, cap
25), D-65 delta kernel in engine, D-66 sessions opt-in, D-67 AI stubs then ports, D-68 metrics
deferred, D-69 compile-time modules first, D-70 medicare isolation, D-71 API-ID + semver gate,
D-72 G17 capability completion.

## Glossary

- **Delta stone** — versioned before/after record of one committed mutation.
- **Plane** — edge/platform/tenant security boundary for operations.
- **Platform graph** — compiled, validated module manifests; source of truth for routes/events/capabilities.
- **Provider** — boot-time constructor registered in the service graph.
- **Posture** — runtime role: api, worker, migration, benchmark.
- **ToolSpec** — model-visible name bound to exactly one operation (AI/MCP).
- **Unit of work** — the transactional surface handed to handlers (mutations, audit, outbox).
