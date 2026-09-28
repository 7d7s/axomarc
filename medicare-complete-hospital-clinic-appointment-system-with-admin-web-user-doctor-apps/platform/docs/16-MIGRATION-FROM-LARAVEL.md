# 16 — Migration from Laravel

The goal is **conceptual familiarity with superior semantics** — not code compatibility (frozen
position: no PHP API compatibility, ever). A Laravel engineer should think "I know where this
belongs" and then discover "this is safer and more powerful."

## Vocabulary map

| Laravel | Keel successor | Where it lives |
|---|---|---|
| Framework (Laravel core) | Keel platform framework | workspace `platform/` |
| Artisan | `keel` CLI | `tools/keel-console` |
| Eloquent | Data model runtime (ports + delta stones) | `crates/data/postgres`, execution backend |
| FormRequest | Request schema (`RequestViewModel`) | `crates/web/http` |
| Middleware | Middleware pipeline | `crates/web/http` |
| Policy | Policy engine | `crates/security/policies` |
| Gate | Capability | `module.toml` capabilities + policy engine |
| Job | Durable job | `crates/async/jobs` + `keel-worker` |
| Event | Event (transactional outbox) | `crates/async/events` |
| Listener | Listener (registered handler) | `crates/async/events` |
| Notification | Notification hub (multi-channel) | `crates/services/notifications` |
| Mailable | Mail (sandbox + SMTP) | notifications + `adapters/mail-smtp` |
| Filesystem | Storage port + disks | `crates/services/storage` + adapters |
| Cache | Cache port + locks | `crates/services/cache` + adapters |
| Redis cache driver | `RedisCache` (cache port adapter) | `adapters/keel-cache-redis` |
| Redis queue driver | `RedisJobStore` (job store adapter, Lua claim = SKIP-LOCKED) | `adapters/keel-cache-redis` |
| Horizon | Operations dashboard (`/_dashboard`) | `runtime/keel-api` |
| Pulse / metrics | Prometheus text `/metrics` + `MetricsRegistry` | `crates/observability/metrics` |
| Scout | Search port + pgvector engine | `adapters/keel-search-pgvector` |
| Scheduler | Scheduler (runner + worker tick) | `crates/async/scheduler` |
| `route()` / named routes | `PlatformGraph::resolve_url` + named `RouteSpec` (S9) | `crates/application/registry` |
| `URL::signedRoute` | `SignedUrls` (HMAC + expiry) | `crates/web/http` |
| `redirect()` | `Redirect::to/see_other/temporary` | `crates/web/http` |
| `session()->flash()` | `flash_put` / `flash_take` | `crates/web/http` + sessions |
| Blade | Not cloned — SPA-first (OpenAPI + your SPA toolchain); SSR revisited v2.0 | `keel sdk:openapi`, starter template |
| Breeze / Jetstream | Starter template: `keel new --with-starter` | `templates/starter-template` |
| Tinker | `keel shell` (B-29): typed REPL over the booted engine; dispatches through the engine under your principal, capability denial enforced | `runtime/keel-api --shell`, spawned by `tools/keel-console` |

## Per-concept detail

### Artisan -> keel CLI
Same instinct: `keel make:module` for `make:model`-style scaffolding, `keel migrate` for
`artisan migrate`, `keel queue:work` for `queue:work`. Difference: every command is validated
against the workspace graph and config (G05-G08) — scaffolds compile by construction. Migration:
map artisan habits 1:1 via `12-CLI.md`; the platform adds diagnostics artisan never had
(`inspect`, `replay`, `benchmark`, `security`).

### Eloquent -> data model runtime
Difference: the model is not an active record with magic; persistence is a port implemented by the
data layer, and every committed change is a **delta stone** (versioned before/after). You gain
replay (`keel replay`), audit chains for free, and no silent N+1 (assert_queries! + the N+1
detector). Migration: replace model calls with commands/queries through the engine; schema stays
in timestamped SQL migrations.

### FormRequest -> Request Schema
Difference: validation is a trait (`RequestViewModel`) checked by the compiler plus the frozen
error taxonomy — invalid input becomes a structured `Validation` error envelope, not an exception
caught somewhere. Migration: move rules into request view models per operation.

### Middleware
Same concept, frozen pipeline: maintenance mode, rate limiting, and framework hooks are
platform-owned; modules contribute through contracts. Migration: bespoke middleware collapses
into either a platform port or an operation-level guard.

### Policy / Gate -> capability + policy engine
Difference: authorization is data + kernel enforcement, not code paths you remember to call.
Capabilities are declared in `module.toml`; the engine checks them before any work; scopes add
tenant structure; revisions make every decision explainable. Migration: enumerate gate names into
a capability catalog per module; policy classes become policy rules over scopes/ownership.

### Job / Event / Listener
Difference: jobs are durable with SKIP-LOCKED claims and a dead-letter queue; events ride the
**transactional outbox** — queued in the unit of work, flushed with the commit, published only
after listeners succeed. No event loss, no "fired before commit" bugs. Migration: dispatch calls
become `unit.queue_outbox` inside handlers; listeners register by manifest names.

### Notification / Mailable
Difference: one hub, many channels (mail/SMS/push) behind one contract; sandbox in dev, live
adapters in prod; sends are audited. Migration: notification classes map to channel messages;
mailables map to the mail adapter with templates.

### Filesystem / Cache / Scheduler
Same ports, stricter contracts: storage guards (size/type), cache locks with delete-if-equals,
scheduler runs with worker ticks and list/run CLI. Migration: disk config moves to config TOML;
cache calls move to the cache port; scheduled closures become registered schedules.

## Migration strategy, step by step

1. **Inventory**: list artisan commands, jobs, events, gates, disks in the legacy app.
2. **Map**: translate each into the vocabulary table; anything unmapped is a candidate platform
   contract (the framework grows by contracts, not by forks).
3. **Scaffold**: `keel new` + `keel make:module` per bounded context; declare capabilities,
   routes, events in `module.toml`.
4. **Port data**: rewrite schema as migrations; import data; verify with delta-based audits.
5. **Port behavior**: commands/queries through the engine; tests with TestKit fakes and
   assert_delta!/assert_event!.
6. **Cutover**: feature flags per tenant, run both systems read-compare during rollout, then
   decommission the PHP side.
