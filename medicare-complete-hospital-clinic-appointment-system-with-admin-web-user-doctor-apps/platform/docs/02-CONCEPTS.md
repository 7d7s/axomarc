# 02 — Concepts

The ten ideas that carry the whole platform.

## 1. Execution context
Every operation runs inside an immutable `ExecutionContext`: execution id, request/correlation ids,
security plane, principal, actor, tenant, scope snapshot, ownership, feature snapshot, execution
profile, deadline, locale, trace id, timezone. Built once, never mutated.

## 2. Security planes
Three planes: **Edge** (public), **Platform** (framework), **Tenant** (business data). An operation
is bound to a plane at plan time and the engine refuses to run it on the wrong plane.

## 3. Capability, scope, policy
Authorization is `principal + tenant + capability + scope`. Capabilities are named permissions
(`appointments.book`); scopes form a lattice over tenant structure; the policy engine decides from
an access snapshot hydrated per execution. Denied means `DENIED`, nothing written, reason recorded.

## 4. Operations and plans
One handler = one operation = one stable contract (`module.operation`). The plan compiler turns a
profile (READ/COMMAND/FINANCIAL/…) into a frozen plan: authorization, transaction, concurrency,
consistency, idempotency, retry, audit, delta, timeout. The engine executes plans; it never
re-derives semantics at runtime.

## 5. Execution state machine
`ACCEPTED -> PLANNED -> AUTHORIZED -> RUNNING -> COMMITTED -> DELIVERED`, with branches to
`DENIED / REJECTED / FAILED (-> RETRY | DEAD)`.

## 6. Idempotency
Commands take an idempotency key; the engine gates on it before execution and stores the result
after commit. Same key + same payload replays the stored result — no second effect.

## 7. Delta and audit
Every committed mutation is a delta stone (resource, version, before/after, stream). Audit is
derived from the same chain. This is what powers `kelson inspect`, `kelson replay`, and downstream
sync/reporting.

## 8. Transactional outbox
Events are queued inside the unit of work and flushed with the commit — never published before the
data is durable, never lost with it.

## 9. Platform graph
Modules declare `module.toml` (capabilities, routes, events, dependencies). The graph compiler
rejects duplicates, unknown deps, routes to undeclared operations, cycles. The same graph feeds CLI
listing, SDK generation (OpenAPI), and MCP tool manifests.

## 10. Revisions
Authorization, feature and module state are versioned. Snapshots carry the revision they were
computed at, so decisions are explainable and replayable.
