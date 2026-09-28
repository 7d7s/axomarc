# Platform docs

Public documentation for Keel — getting started, concepts, and the reference set (S7).
Normative specs live in `specs/` (platform) and `docs/rust/` (repo root).

| Doc | Content |
|---|---|
| [01-GETTING-STARTED](01-GETTING-STARTED.md) | prerequisites, first commands, layout, examples |
| [02-CONCEPTS](02-CONCEPTS.md) | the ten ideas that carry the platform |
| [03-ARCHITECTURE](03-ARCHITECTURE.md) | topology, dependency direction, gates G01-G17 |
| [04-HTTP](04-HTTP.md) | routes, envelope, view models, middleware |
| [05-DATABASE](05-DATABASE.md) | postgres ports, migrations, roles, degraded boot |
| [06-SECURITY](06-SECURITY.md) | capability/scope/policy model, enforcement, secrets |
| [07-ASYNC](07-ASYNC.md) | events/outbox, jobs, batches/chains, workflows, scheduler |
| [08-SERVICES](08-SERVICES.md) | cache, storage, notifications, realtime, http-client, search |
| [09-ENTERPRISE](09-ENTERPRISE.md) | tenancy, feature flags, deployment, control plane |
| [10-AI](10-AI.md) | AiClient port, tools, agents, MCP |
| [11-TESTING](11-TESTING.md) | TestKit, fakes, assertions, time control, golden path |
| [12-CLI](12-CLI.md) | full `keel` command reference |
| [13-OPERATIONS](13-OPERATIONS.md) | runbooks: diagnostics, queues, telemetry/incident (S5), benchmarks, upgrades |
| [14-DEPLOYMENT](14-DEPLOYMENT.md) | binaries, build, config layering, rollout discipline |
| [15-EXTENSION](15-EXTENSION.md) | ports/adapters, providers, modules, plugin roadmap, API governance |
| [16-MIGRATION-FROM-LARAVEL](16-MIGRATION-FROM-LARAVEL.md) | vocabulary map + per-concept strategy |
| [17-REFERENCE](17-REFERENCE.md) | crate map, profiles, codes, decisions, glossary |
| [18-GAP-ANALYSIS-LARAVEL](18-GAP-ANALYSIS-LARAVEL.md) | Laravel 13 forensic parity audit + v1.0 gate reconciliation |
