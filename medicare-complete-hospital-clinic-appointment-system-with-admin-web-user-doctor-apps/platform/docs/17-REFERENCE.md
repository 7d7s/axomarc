# 17 — Reference

Quick lookup tables for the platform surface.

## Crate map by class

| Class | Crates |
|---|---|
| foundation | kelson-foundation, kelson-context, kelson-contracts, kelson-validation, kelson-lifecycle |
| execution | kelson-execution (engine), kelson-command, kelson-query |
| application | kelson-config, kelson-registry |
| web | kelson-http |
| security | kelson-identity, kelson-policy, kelson-scope, kelson-ownership, kelson-elevation, kelson-session, kelson-tokens, kelson-integrity |
| data | kelson-postgres |
| async | kelson-event, kelson-jobs, kelson-workflows, kelson-scheduler |
| services | kelson-cache, kelson-storage, kelson-notifications, kelson-realtime, kelson-http-client, kelson-search |
| observability | kelson-observability (Sentinel runtime), kelson-telemetry, kelson-tracing, kelson-metrics, kelson-audit, kelson-pipeline, kelson-buffer, kelson-export |
| enterprise | kelson-tenancy, kelson-feature-flags, kelson-deployment |
| ai | kelson-ai, kelson-ai-tools, kelson-ai-agents, kelson-ai-mcp |
| adapters | kelson-cache-memory, kelson-mail-smtp, kelson-sms-msg91, kelson-push-fcm, kelson-storage-s3 |
| testing | kelson-testkit |
| runtime | kelson-demo, kelson-worker |
| tools | kelson-console, kelson, kelson-analyzer, kelson-codegen, xtask |

Package names keep the `kelson-*` prefix (D-63); directory names may differ from package names
(`crates/execution/engine` = `kelson-execution`).

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
- `KSN-*` — kernel error codes; module-prefixed codes for domain errors.

## API governance registry

Public APIs carry stable API-IDs (`crates/foundation/contracts/api-registry.toml`), e.g.
`EXEC-API-0001`, `SEARCH-API-0001`, `AGENT-API-0001`, `MCP-API-0001`. Stability ladder:
stable | beta | experimental | deprecated (with replacement). `kelson upgrade` reports them;
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
