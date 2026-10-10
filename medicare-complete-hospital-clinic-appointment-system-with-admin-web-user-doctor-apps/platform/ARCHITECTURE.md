# Kelson Architecture (summary)

Full normative text: `docs/rust/11-FOUNDATION-MASTER-SPEC.md`.

## Planes (§1)
- **Edge** — ingress/routing/caching. Cannot mutate tenant state (P1).
- **Platform** — identity, provisioning, registries. No implicit tenant authority (P2).
- **Tenant** — business execution. Cannot mutate platform state (P3).

## Execution (§2/§9)
`ExecutionPlan` declares intent; `ExecutionProfile` resolves semantics;
`ExecutionBackend::run()` wraps begin→handler→guards→commit. Delta, audit and
outbox are written in the same transaction; events fire post-commit (D-53).

Pipeline: plan validate → plane check → deadline → authorization → idempotency →
backend.run → store idempotent result.

## Dependency matrix (STAGE-0-SPEC §3)
foundation→foundation · security→foundation+security ·
execution→foundation+security+execution ·
application/web/services/async→foundation+security+execution+application+own layer ·
data→+services+async · observability→foundation ·
adapters→foundation+services+async · tools→foundation+application ·
tests→foundation+application+tools · runtime→all layers+adapters ·
examples/applications→as published.
Enforced by `tools/kelson-analyzer` (`kelson check`, `cargo xtask dep-rules`) and the
`tests/architecture` gate.

## Multi-tenancy (§24)
Typed ids → `ExecutionContext` → `SET LOCAL kelson.tenant_id` → RLS. Runtime roles
never own tables (`database/roles.sql`).