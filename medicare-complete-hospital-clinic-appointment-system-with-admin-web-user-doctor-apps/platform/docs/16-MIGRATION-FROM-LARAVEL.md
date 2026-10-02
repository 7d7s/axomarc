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
| `Str::slug/snake/camel/studly/title/words/excerpt/random` | String helpers: `to_slug`, `to_snake_case`, `to_camel_case`, `to_studly_case`, `to_title_case`, `words`, `excerpt`, `random` (CSPRNG alphanumeric) | `crates/foundation/core` |
| `Str::uuid` / `Str::ulid` | `uuid()` (random v4) / `ulid()` (26-char Crockford base32) generators | `crates/foundation/core` |
| `Str::plural` / `Str::singular` | `plural` / `singular` — documented English inflection subset | `crates/foundation/core` |
| `Number::currency/percentage/precision/clamp` | Number helpers: `format_currency`, `format_percent`, `format_precision`, `clamp` (locale rules from the Wave 6 identities: en/fr/ar primary subtag, en fallback; curated ISO symbol + minor-unit tables) | `crates/foundation/core` |
| `Carbon` (parse/format/timezone/now) | Date helpers: `parse_date_with` / `format_date_with` (closed `Y m d H i s` set — the `date_format` rule delegates here), `now_in` / `in_tz` / `local_wall` over the bundled IANA table | `crates/foundation/core` |
| Composer package discovery | Package registry: root `packages.toml` index (`[[package]] { name, version, path }`), `keel pkg:list` / `keel pkg:add`, graph compile merges every indexed package's `module.toml` fail-closed (ABI-gated); `keel publish` / `keel config:publish` materialize package resources (vendor:publish-class) | `crates/application/registry`, `tools/keel-console` |

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

### Str / helper functions -> keel-foundation helpers
Same instinct: familiar names for casing, slugs, snippets, tokens and inflection — pure free
functions in `keel-foundation`, no PHP runtime and no new dependencies. Difference: inflection
is a **documented subset**, not a full English inflector — regular rules (`s/x/z/ch/sh -> es`,
`consonant+y -> ies`, default `+s`, with their reversals stopping at `ss`/`us`/`is` stems) plus
curated tables: irregulars (`person/people`, `child/children`, `knife/knives`, `potato/potatoes`,
`matrix/matrices`, `quiz/quizzes`, `bus/buses`...) and uncountables (`sheep`, `series`,
`information` never inflect). The tables are the extension point for anything outside the
subset. `excerpt` windows by **character counts** (not locale delimiters) and matches
case-sensitively; `title` normalizes `_`/`-`/camel boundaries to single spaces because Keel
treats identifiers as words (Laravel's `ucwords` keeps the underscore — documented divergence).
Migration: drop the `Str::` prefix for free functions — `Str::plural('person')` ->
`plural("person")`, `Str::slug($s)` -> `to_slug($s)`, `Str::uuid()` -> `uuid()`.

### Number / Carbon -> keel-foundation number + date helpers
Same instinct: `Number::currency($v, 'USD', 'fr')` -> `format_currency(v, "USD", "fr")`,
`Carbon::createFromFormat('Y-m-d', $s)` -> `parse_date_with(s, "Y-m-d")`,
`Carbon::now('Asia/Kolkata')` -> `now_in("Asia/Kolkata")`. Difference: formatting is a
**documented subset** — the Wave 6 locale identities select rules by primary subtag
(`en`/`fr`/`ar` ship, `en-IN` matches `en`, unknown locales fall back to `en` rules —
display-only), with ASCII digits and 3-digit grouping everywhere (no Arabic-Indic digits,
no Indian lakh grouping), a curated ISO symbol table (unknown codes render as themselves)
and per-code minor units (`JPY`/`KRW` = 0, default 2). Dates share **one grammar**:
`parse_date_with`/`format_date_with` implement the closed `Y m d H i s` token set and the
`date_format` validation rule delegates to it, so helper and rule cannot drift. Timezone
conversions (`now_in`, `in_tz`, `local_wall`) read the bundled IANA offset table —
relocated from `keel-scheduler` into `keel-foundation` this wave — with the same
fixed-offset caveat (no DST transitions) and `None` for unknown zones instead of ever
converting in the wrong zone; the `datetime:<tz>` model cast feeds on them (UTC storage,
zone-local reads) while bare `datetime` is unchanged. Migration: drop the class prefix;
formats are strings in both worlds, wall-clock parses land as UTC instants (convert with
`in_tz` when a zone applies).

### Composer package discovery -> packages.toml registry
Same instinct: a third-party package registers its routes/config without framework edits.
Difference: discovery is **compile-time**, not runtime — the root `packages.toml` index lists
local-path packages, `keel pkg:add <path>` validates the package's `module.toml` and appends it,
`keel pkg:list` shows manifest + ABI compatibility, and graph compile merges the manifest through
the same fail-closed gates as your own modules (an unusable entry refuses `keel check`/boot).
Packages may ship `[config]` defaults that merge UNDER your config file at boot; `keel
config:publish <package>` materializes an editable copy at `config/packages/<name>.toml` that
layers above the manifest defaults and still under your config file. Package resources map
`vendor:publish` to `keel publish <package>`: migrations (name-prefixed after the sqlx integer
version), config and assets materialize byte-compared-idempotent — differing files are reported
as conflicts and skipped unless `--force` — while views register under the `<name>::` namespace
at boot (no copy). Migration: replace `composer require` + provider auto-discovery with copying
the package in, `keel pkg:add`, `keel publish`, and a workspace member for its crate; v1 is
local-path only (git fetch is out — zero-dep rule).

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
