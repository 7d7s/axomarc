# 09 — Enterprise

The control plane: `crates/enterprise/*` — tenancy, feature flags, deployment.

## Tenancy

`enterprise/tenancy`: tenant store, tenancy service, and `TenantScope`. Every execution context
carries the tenant; storage and search isolate by tenant tag. Plans (pricing tiers) attach to the
tenant context and drive usage caps (per-tenant monthly operation limits enforced by the capability
engine — usage-cap pricing).

The tenancy hierarchy for ERP-grade domains (legal entity, business unit, region, location,
department, cost center) models onto the scope lattice — structure is data, not code.

## Feature flags

`enterprise/feature-flags`: flag registry + file store + CLI.

Dimensions: global, environment, tenant, percentage, capability, actor. Rollout ladder:
`disabled -> internal -> 1% -> 10% -> 50% -> 100%` with instant rollback.

```bash
keel flag:list
keel flag:set new-billing-engine 10% --tenant <uuid>
```

Snapshots resolve once per execution (context carries the `FeatureSnapshot`), so a run never
changes behavior mid-flight.

## Deployment model

`enterprise/deployment` defines the release manifest and build gates. `keel build` wraps cargo
profiles and writes `RELEASE.md`; `keel release` prints the current manifest. Runtime postures:
api binary vs worker binary vs migration runner — selected by config environment + binary.

## Control plane roadmap

Session stores are opt-in (D-66). Metrics/profiling stubs are deliberately deferred (D-68) —
observability hooks exist at the ports; concrete backends land when the ecosystem demands them.
