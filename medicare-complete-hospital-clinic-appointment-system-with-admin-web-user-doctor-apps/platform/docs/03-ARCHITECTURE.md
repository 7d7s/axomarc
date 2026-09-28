# 03 — Architecture

## Topology

```
crates/
  foundation/    core, context, contracts, validation, lifecycle
  execution/     engine, command, query            (the kernel / TCB)
  application/   configuration, registry, codegen-like support
  web/           http (axum), middleware, view models
  security/      identity, policies, scopes, ownership, elevation, sessions, tokens, integrity
  data/          postgres (implements execution/service ports)
  async/         events, jobs, workflows, scheduler
  services/      cache, storage, notifications, realtime, http-client, search
  observability/ Sentinel (S5) telemetry runtime: logging (keel-observability), telemetry, tracing, metrics, audit, pipeline, buffer, export
  enterprise/    tenancy, feature-flags, deployment
  ai/            client, tools, agents, mcp
adapters/        keel-cache-memory, keel-mail-smtp, keel-sms-msg91, keel-push-fcm,
                 keel-storage-s3, keel-storage-local
runtime/         keel-api, keel-worker
applications/    generated/domain apps (platform NEVER imports these)
```

## Dependency direction (gate G07)

Layers may depend only same-or-lower. The frozen matrix (STAGE-0-SPEC section 3) is enforced by
`keel-analyzer` on every `keel check`:

- foundation depends on nothing; security on foundation; execution on foundation+security.
- application/web/services/async build downward; data implements their ports.
- observability never imports runtime; adapters are imported only by runtime binaries.
- tools may depend on anything except runtime binaries; examples/applications never on platform tooling internals.
- platform crates never import `applications/`; domain modules never import transport.

Violations are hard errors (`DEP-CLASS`), reported with stable codes.

## The kernel boundary

Everything in `crates/execution` plus `crates/foundation` is the trusted computing base. Behavior
that repeats across applications becomes a platform contract; behavior that affects security,
consistency, observability, or recoverability is enforced by the kernel — never by developer
convention.

## Gates (CI)

```
G01 fmt          G02 clippy -D warnings   G03 tests            G04 compile matrix
G05 graph compile (no duplicate routes/caps/events)            G06 manifest validity
G07 dependency matrix + budgets          G08 migrations ordered
G09 golden-path example passes           G10 degraded-boot smoke
G11 architecture tests (domain orthogonal to transport, platform does not absorb applications)
G12 secrets hygiene   G13 docs links      G14 API-ID + semver gate
G15 benchmark baseline                   G16 diagnostic chain persistence
G17 capability completion (13 facets)
G18 telemetry resilience (failure injection + tenant isolation)
```

## Capability completion (G17)

A capability is done only when all 13 facets exist: contract, runtime, API, configuration, CLI,
generator, testkit, observability hooks, security review, documentation, extension points,
performance budget, compatibility policy.
