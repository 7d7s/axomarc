# Keel × Laravel 13 — Forensic Parity Audit

**Stage:** Report-only. No feature work, no Rust edits, no compile.

**Baseline:** Laravel 13 surface as enumerated by the audit specification supplied for this stage.

**Evidence rule:** Every Keel claim is tied to a repository path supplied as a locked evidence anchor in the audit brief. Where the supplied evidence does not prove runtime behavior, the status is intentionally conservative.

**Status vocabulary:** `MATCHED`, `EQUIVALENT`, `SUPERSEDED`, `DELIBERATELY EXCLUDED`, `PARTIAL`, `API-ONLY`, `STUB`, `INCOMPLETE`, `DEAD`, `UNKNOWN`.

**How to read:** A coarse subsystem is never marked `MATCHED` when important child capabilities remain unimplemented. Function-level matrices therefore override any optimistic subsystem impression.

## 1. Executive Verdict

Keel is a **late-alpha internal platform**, not a production Laravel alternative yet. The supplied repository evidence supports a broad architectural surface, but end-to-end application readiness is effectively **near zero** because the graph-driven route declarations do not yet reach the HTTP dispatch path. The exact break is `platform/runtime/keel-api/src/routes.rs:44-51`; graph route declarations are emitted toward CLI/OpenAPI concerns and never become executable HTTP routes.

| Dimension | Verdict | Evidence / rationale |
|---|---|---|
| Maturity | Late-alpha internal platform | Repository reality and architecture gates; no user-facing module reaches the HTTP runtime. |
| Surface parity | ≈55–60% by current feature-surface estimate | Broad core/platform coverage, but several major Laravel application layers remain partial or absent. |
| End-to-end app readiness | Near zero | `platform/runtime/keel-api/src/routes.rs:44-51`; `platform/applications/appointments/module.toml`; no keel-api dependency on the appointments module. |
| Architecture quality | High | `platform/crates/application/registry/src/lib.rs`; graph/lifecycle structure; architecture tests in `platform/tests/architecture/tests/architecture.rs`. |
| Production readiness | NO | No HTTP graph wiring, no durable session path, no PostgreSQL-backed store test matrix, missing upload/cookie layer, metrics endpoint absent. |
| Biggest risk | Edge gap | Graph → HTTP dispatch is the hard blocker to proving a real application. |
| Biggest opportunity | Rust-native engine capabilities | Delta/audit/replay, capability authorization, typed execution/context, MCP/AI-native tooling. |

### Brutal summary

Keel already has more than a typical Rust “web framework starter”: registry, lifecycle host, command engine, policy/capability gates, queues, cache/locks, storage ports, realtime, tenancy, feature flags, observability, testkit and CLI tooling. But those foundations do not add up to a usable Laravel-class application framework until a developer can actually map a module to HTTP, authenticate a user, persist domain state in PostgreSQL, validate input, serialize a response and run that stack repeatedly in local development.

## 2. Repository Reality

The verified repository inventory supplied for this audit is:

- **57 workspace members**
- **41 library crates**
- **7 adapters**
- **2 runtimes**
- **3 tools + xtask**
- testkit
- architecture tests
- 2 examples
- 1 domain app
- approximately **22.7k LOC**
- **112 `.rs` files**
- **336 static `#[test]` attributes**
- **334 tests in the last executed green gate**
- approximately **24 external dependencies**, against the D-62 cap of 25
- binaries: `keel-api`, `keel-worker`, `keel`

### Repository evidence anchors

| Area | Locked evidence | What it proves |
|---|---|---|
| Routing edge | `platform/runtime/keel-api/src/routes.rs:44-51` | Route specification handling exists but is not an executable graph-to-router path. |
| Registry | `platform/crates/application/registry/src/lib.rs` | `RouteSpec` exists; no `name` field means named-route/URL-generation parity is absent. |
| Domain manifest | `platform/applications/appointments/module.toml` | Domain module declares routes at manifest level. |
| HTTP dependency gap | `keel-api` has no `keel-mod-appointments` dependency | The declared domain route is not connected to the API runtime. |
| Admission | `platform/crates/web/http/src/middleware.rs` | Request-id → trace → body-limit → CORS → maintenance → rate-limit. |
| CSRF | `platform/crates/web/http/src/csrf.rs` | Double-submit mechanism and CSRF cookie handling. |
| CSRF config | `platform/runtime/keel-api/src/graph_routes.rs` | Default-on for web-plane routes; skip paths from `[security.csrf]` only (no flag/env — `KEEL_CSRF` removed). |
| Tokens | `platform/crates/security/tokens/src/lib.rs:137-214` | `TokenService` and `InMemoryTokenStore`. |
| Reset flow | `platform/crates/security/tokens/src/reset.rs:45-64` | Reset token flow exists. |
| Sessions | `platform/crates/security/sessions/src/lib.rs:197-279` | Session manager, regeneration and sweep. |
| Identity | `platform/crates/security/identity/src/lib.rs:129-152` | Argon2 and OTP primitives. |
| Persistent token store | `platform/crates/data/postgres/src/tokenstore.rs:64` | PostgreSQL-backed token storage exists. |
| ORM metadata | `platform/crates/data/models/src/lib.rs` | Field/relation/model metadata and graph validation. |
| Query builder | `platform/crates/execution/query/src/select.rs` | Typed `Select`, identifier safety, tenant scope; no write builder. |
| Schema | `platform/database/migrations/*.sql` | 18 framework tables across 2 framework migration files. |
| Persistent stores | `platform/crates/data/postgres/src/jobstore.rs`, `src/outboxstore.rs`, `src/tokenstore.rs` | Pg persistence is selective, not a general domain store. |
| Events/outbox | `platform/crates/async/events/src/lib.rs:135,236` | Outbox store/pump. |
| Worker | `platform/runtime/keel-worker/src/main.rs` | `SKIP LOCKED` claim loop, batches/chains/scheduler. |
| Cache/locks | `platform/crates/services/cache/src/lib.rs:47,62` | Cache delete-if-equals and cache lock. |
| Storage | `platform/crates/services/storage/src/lib.rs:23` | Storage port; local implemented, S3 stub. |
| Notifications | `platform/crates/services/notifications/src/lib.rs:42` | Notification hub. |
| Realtime | `platform/crates/services/realtime/src/lib.rs:69,187` | Broadcast hub and realtime router. |
| Search | `platform/crates/services/search/src/lib.rs` | Search query/result abstraction and in-memory engine. |
| Tenancy | `platform/crates/enterprise/tenancy/src/lib.rs:143,267` | Tenant primitives. |
| Feature flags | `platform/crates/enterprise/feature-flags/src/lib.rs:431,472` | Feature flag runtime. |
| Observability | `platform/crates/observability/logging/src/logsys.rs:88,219,249` | Redaction, init, request spans. |
| Localization | `platform/crates/foundation/core/src/translate.rs:151` | Translation catalog primitive. |
| Factory gap (reconciled) | `platform/tools/keel-codegen/src/generators.rs` | Historical: `todo!` once sat in the generated factory build path; FACTORY_TEMPLATE now builds for real (test `factory_build_is_real_and_deterministic`, no `todo!` — see the make:factory row in §11). |
| Domain store anti-pattern | `platform/applications/appointments/src/lib.rs:45-46` | `Mutex<BTreeMap>` persistence, not a durable Pg store. |
| Pg test gap | repository-wide zero `sqlx::test` | No PostgreSQL-backed integration matrix. |
| Architecture gates | `platform/tests/architecture/tests/architecture.rs` | G07 dependency matrix, topology, graph compile tests. |

## 3. Architecture Map

### Current architecture

```text
manifest
   ↓
PlatformGraph (keel-registry)
   ↓
lifecycle host
   ├─ Config
   ├─ Kernel
   ├─ Platform
   └─ Modules
   ↓
CommandBus / Engine
   ├─ plan
   ├─ idempotency
   ├─ delta
   ├─ audit
   └─ outbox
   ↓
policy / capability gates
   ↓
stores
   ├─ memory
   └─ PostgreSQL (jobs / outbox / tokens)
```

### Exact broken link

The architectural design intent is graph-driven, but the HTTP edge remains hard-coded. `platform/runtime/keel-api/src/routes.rs:44-51` proves route handling at the edge, while `platform/crates/application/registry/src/lib.rs` proves route metadata in the graph. The supplied facts also prove that `platform/applications/appointments/module.toml` declares routes but `keel-api` has no `keel-mod-appointments` dependency. The result is:

```text
Module manifest
  → PlatformGraph
  → CLI/OpenAPI emission
  → STOP

Expected:
Module manifest
  → PlatformGraph
  → HTTP router
  → request pipeline
  → controller/handler
  → Engine
  → response
```

This is the single most important architectural gap in the audit.

### Architectural quality

The underlying engine architecture is strong: lifecycle separation, centralized command execution, capability gates and auditable execution are more disciplined than the typical first-generation Rust web framework. The weakness is not the core engine; it is the missing **application edge contract** that should turn the graph into a runnable product surface.

## 4. Laravel Parity Matrix

The matrix below is deliberately granular. A subsystem cannot be called `MATCHED` when critical child capabilities are absent.

| Subsystem / capability | Status | Evidence | Gap / interpretation | Priority |
|---|---|---|---|---|
| Application bootstrap | EQUIVALENT | platform/crates/application/registry/src/lib.rs | Graph/lifecycle architecture exists; HTTP module boot path incomplete | P0 |
| Service container | EQUIVALENT | platform/crates/application/registry/src/lib.rs | Typed lifecycle/provider registry rather than Laravel facades | P1 |
| Providers / lifecycle | EQUIVALENT | platform/crates/application/registry/src/lib.rs | Config → Kernel → Platform → Modules phases | P1 |
| Facades / global helpers | DELIBERATELY EXCLUDED | typed Rust architecture; no Laravel facade layer | Rust idiom intentionally avoids global facade state | P3 |
| Routing | MATCHED | platform/runtime/keel-api/src/graph_routes.rs; platform/runtime/keel-api/tests/routing_edge_integration.rs | Graph routes dispatch at the edge (Wave 2): verb/ANY dispatch, auto HEAD/OPTIONS with 405 + Allow, api/web planes, groups, domain/subdomain gates, constraints, signed URLs; fallback routes stay gate-only (row `fallback`) | P0 |
| Named routes / URL generation | MATCHED | platform/crates/application/registry/src/lib.rs (RouteSpec.name, GRAPH-DUP-ROUTENAME, resolve_url, UrlSection); platform/tools/keel-console/src/main.rs (route:list --name) | Named routes generate URLs with `[url]` defaults (explicit values win) and list with `--name` filters (Wave 1-2) | P2 |
| Middleware | MATCHED | platform/crates/web/http/src/middleware_catalog.rs; platform/crates/application/registry/src/middleware_catalog.rs | Named middleware chains (Wave 1): middleware.toml aliases/groups/throttle configs, implicit api group (throttle:api → auth → verified), per-route chains at the dispatch edge, GRAPH-MW-UNKNOWN boot gate; catalog is framework-owned in v1 (no module-defined custom middleware; Wave 5 codegen follow-up) | P1 |
| CSRF | EQUIVALENT | platform/crates/web/http/src/csrf.rs; platform/runtime/keel-api/src/graph_routes.rs | Double-submit implementation; default-on for web-plane routes (Wave 7 P7), `[security.csrf].skip_paths` config only, api plane token-authenticated | P1 |
| HTTP request parsing | PARTIAL | platform/crates/web/http/src/middleware.rs | Core request path exists; multipart absent | P0 |
| Multipart uploads | MISSING | repo-wide zero rg hits supplied | No multipart implementation | P0 |
| HTTP responses | MATCHED | platform/runtime/keel-api/src/graph_routes.rs (response dispatch); platform/runtime/keel-api/tests/response_layer_integration.rs | json/html/redirect/download/stream/no_content kinds dispatched at the edge: 204, NDJSON flush, per-route Cache-Control, ETag/304 (Wave 2) | P0 |
| HTML responses | MATCHED | platform/runtime/keel-api/tests/views_integration.rs; platform/runtime/keel-api/tests/error_rendering_integration.rs | `response = "html"` renders the declared view with the operation result as context; web-plane errors render `errors.404` (Wave 2; closes this row per 19-PARITY-PROGRAM) | P2 |
| Redirects | MATCHED | platform/runtime/keel-api/src/graph_routes.rs (redirect_response unit tests) | `Location` from the operation result (string or `{to,status,flash}`), status validated, flash carried for exactly one request (Wave 2) | P2 |
| SSE | MATCHED | platform/crates/services/realtime/src/lib.rs:69,187 | Realtime service exposes streaming path | P2 |
| WebSockets | MATCHED | platform/crates/services/realtime/src/lib.rs:69,187 | Flag-gated realtime router | P2 |
| Controllers | EQUIVALENT | `RequestViewModel` / `ResponseViewModel` supplied in audit facts | Typed controller boundary exists | P1 |
| FormRequest | MATCHED | platform/runtime/keel-api/src/graph_routes.rs (`[[validation]]` enforced before dispatch); `validation_edge_integration` + `http_integration` batteries | Manifest `[[validation]]` answers 422 + `fields` at the edge since Wave 1; the Wave 4 rule battery proves per-rule pass/fail (regex SUPERSEDED by the closed grammar — documented divergence) | P1 |
| Views | EQUIVALENT | platform/crates/web/views/src/engine.rs; platform/crates/web/views/src/parser.rs | Interpreted-AST template engine renders SSR views from `[views].paths` with a compiled-AST cache (`keel view:cache`); SPA-first remains the default web story (Wave 2; S9 preserved) | P2 |
| Blade | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Blade-familiar syntax over the full directive ledger; PHP eval, `@php` and `@while` are deliberately SUPERSEDED (19-PARITY-PROGRAM policy); unimplemented directives flip to MISSING below (Wave 2) | P2 |
| Sessions | EQUIVALENT | platform/crates/security/sessions/src/lib.rs (SessionManager: TTLs, regeneration, remember-me bag); platform/runtime/keel-api/src/auth_routes.rs | Full login/verify/regenerate/logout lifecycle over the SessionStore port; Wave 4 remember-me (cookie `Max-Age` + bag flag) — the server-side remember bound stays open | P0 |
| PgSessionStore | EQUIVALENT | platform/crates/data/postgres/src/sessionstore.rs (table `http_sessions`) | Durable horizontal session store behind the SessionStore port | P0 |
| Cookies | EQUIVALENT | platform/crates/web/http (typed cookie layer, D-62); platform/runtime/keel-api/src/auth_routes.rs (cookie issuance) | Cookie layer ships with the session middleware (`keel_session` = `session_id.hmac_tag`); posture `Path=/ HttpOnly SameSite=Lax`, `Secure` in prod (docs/04); value encryption under `[http] encrypt_cookies` roundtrip-proven through the production edge (Wave 7 P7) | P0 |
| Authentication primitives | EQUIVALENT | platform/crates/security/identity/src/lib.rs:129-152 | Argon2 / OTP primitives exist | P1 |
| HTTP auth flows | MATCHED | platform/runtime/keel-api/src/auth_routes.rs | Full edge auth surface: register/login/logout/me + password-reset + email-verification + `/auth/confirm` (Wave 4), session cookie or hashed PAT, rate-limited attempts (docs/04); Wave 7 P5/P6: argon2 rehash-on-login + `[auth.password_policy]` on register and both reset flows | P0 |
| JWT | EQUIVALENT (future add-on) | JWT absent; hashed PAT + HMAC sessions | Intentional non-default design; add-on later | P2 |
| Authorization | SUPERSEDED | kernel capability authz supplied by audit facts | Capability model exceeds basic role-only framework authorization | P1 |
| Validation | EQUIVALENT | platform/crates/foundation/validation (rule battery); platform/crates/application/registry (`GRAPH-VALIDATION-RULE`/`GRAPH-VALIDATION-FIELD`/`GRAPH-RULE-UNKNOWN`) | Engine wired to HTTP since Wave 1; Wave 4 grows the battery ~45 rules with tenant-scoped DB probes (error → 503, never pass); Wave 7 P6 feeds the composable `password` rule from `[auth.password_policy]` | P1 |
| Error handling | EQUIVALENT | typed envelope/error architecture supplied | Structured Rust errors vs Laravel exception helpers | P1 |
| Logging | EQUIVALENT | platform/crates/observability/logging/src/logsys.rs:88,219,249 | Init/redaction/file mirror | P1 |
| Request tracing | EQUIVALENT | platform/crates/observability/logging/src/logsys.rs:249 | Request spans | P1 |
| Metrics | MISSING | D-68 / no metrics endpoint | No production metrics endpoint | P1 |
| Maintenance mode | EQUIVALENT | platform/crates/web/http/src/middleware.rs | Admission layer includes maintenance | P2 |
| Feature flags | EQUIVALENT | platform/crates/enterprise/feature-flags/src/lib.rs:431,472 | Runtime feature flags | P2 |
| Tenancy | EQUIVALENT | platform/crates/enterprise/tenancy/src/lib.rs:143,267 | Tenant-aware primitives | P1 |
| Localization | EQUIVALENT | platform/crates/foundation/core/src/translate.rs:151; platform/crates/application/configuration/src/lib.rs (`load_messages`); platform/runtime/keel-api/src/i18n.rs | Wave 6 P3: boot-loaded `lang/<locale>.toml` catalogs (fail-closed), session → `Accept-Language` → default resolution, `@lang`/`trans`/`trans_choice` (CLDR-style buckets, hand-rolled rules), localized `validation.<rule>` messages and notification strings through `NotifyHub::with_catalog` | P3 |
| Query builder | MATCHED | platform/crates/execution/query/src/select.rs (clause tree + 33 rendering unit tests: union/union_all, group_by/having, locks, where_exists, date family, aggregates) | Full read-builder surface (Wave 3 Phase 1); writes ride ModelRuntime - keel-query compiles SQL but never executes it (documented divergence) | P0 |
| Eloquent ORM | MATCHED | platform/crates/data/models/src/runtime.rs (finders/mutations, push, pluck/value/chunk/each/cursor); platform/crates/data/models/src/store.rs | Full CRUD runtime (Wave 3 Phases 2-3): every operation runs mass assignment -> mutators -> casts -> scopes -> store -> observers, tenant-scoped, with typed not-found and dirty-tracked updates | P0 |
| Relations | MATCHED | platform/crates/data/models/src/runtime.rs (with/with_constrained, has/where_relation, attach/sync/toggle, morph; tests with_loads_a_nested_path_level_by_level, many_to_many_eager_loads_and_aggregates, morph_to_loads_each_type_through_the_morph_map) | Eager + lazy + pivot + polymorphic + aggregate-eager loading (Wave 3 Phase 4) | P0 |
| Casts | MATCHED | platform/crates/data/models/src/casts.rs; runtime `with_cast` registry (tests cast_kinds_validate_and_normalize_at_set_time, encrypted_cast_seals_through_the_runtime_sealer) | Builtin + custom casts validated at set time; `encrypted` fails closed without a sealer (Wave 3 Phase 5) | P1 |
| Accessors / mutators | MATCHED | platform/crates/data/models/src/runtime.rs (`with_accessor`/`with_mutator`, `append`; tests scopes_accessors_and_listing, mutator_transforms_wire_values) | Attribute transformation wired into the wire/projection paths; Wave 3 hardens `append` against unknown accessor names | P1 |
| Scopes | MATCHED | platform/crates/data/models/src/runtime.rs (`with_scope`; `with_global_scope`/`without_global_scopes`; tests scopes_accessors_and_listing, global_scopes_run_first_and_named_suppression_relaxes_reads) | Named local + global query scopes (Wave 3 Phase 6) | P1 |
| Soft deletes | MATCHED | platform/crates/data/models/src/runtime.rs (find_trashed/with_trashed/only_trashed/restore/force_delete; tests soft_delete_trio_round_trips) plus route-binding `with_trashed`; pg veil roundtrip in platform/crates/data/postgres/tests/pg_modelstore.rs | `deleted_at` veil, trash-scoped reads, restore/force delete, trashed route-model binding (Wave 3 Phase 6) | P2 |
| Model events / observers | MATCHED | platform/crates/data/models/src/runtime.rs (`with_observer`, ModelPhase events; tests model_events_fire_in_order, cas_stale_write_reports_stale_record) | Pre-existing observer runtime; Wave 3 wires it into CAS version stamping + dirty tracking | P2 |
| Pagination | MATCHED | platform/crates/web/http/src/pagination.rs; platform/runtime/keel-api/src/graph_routes.rs | Wired (Wave 1): GET routes parse page/per_page and inject _page; per_page clamped to MAX_PER_PAGE 100; list handlers emit a page meta block | P1 |
| Migrations | MATCHED | platform/database/migrations/*.sql | keel migrate / seed and 18 framework tables | P1 |
| Seeders | MATCHED | keel seed; framework migrations | Seed command exists | P1 |
| Factories | MATCHED | platform/tools/keel-codegen/src/generators.rs (FACTORY_TEMPLATE; tests factory_build_is_real_and_deterministic, generated_code_contains_no_placeholder_macros) | Real `build()` with no `todo!`/`unimplemented!` - the snapshot's "build() contains todo!" claim is stale (S8 reality per sec. 25.3; row reconciled with Wave 3) | P2 |
| Cache | EQUIVALENT | platform/crates/services/cache/src/service.rs (CacheService: remember/tags/locks/stampede; recording-port battery) | CacheService over the unchanged CachePort (Wave 4): remember/remember_forever/add/forever/pull/many/put_many/counters/flush, tag namespaces, atomic locks, stampede single-flight; values serde_json-serialized at the service layer | P2 |
| Redis cache driver | EQUIVALENT | platform/adapters/keel-cache-redis/src/lib.rs (`RedisCache`; SET-backed tags, `FLUSHDB` flush) | Distributed cache behind the same port (memory stays the dev default; `REDIS_URL` selects redis for cache + queue) | P2 |
| Queue | MATCHED | platform/runtime/keel-worker/src/main.rs | Worker, batches, chains, scheduler | P1 |
| Redis queue driver | MISSING | no Redis queue driver supplied | Queue backend choice limited | P2 |
| Events / outbox | MATCHED | platform/crates/async/events/src/lib.rs:135,236 | Outbox store/pump | P1 |
| Scheduler | MATCHED | platform/runtime/keel-worker/src/main.rs | Scheduler path exists | P1 |
| Broadcasting | MATCHED | platform/crates/services/realtime/src/lib.rs (BroadcastHub, SSE/WS transports, PresenceHub); platform/runtime/keel-api/src/broadcast_routes.rs | BroadcastHub + realtime router (Wave 2); Wave 6 P7 adds the auth surface — `POST /api/v1/broadcast/auth` short-TTL channel-bound token over the resolved `routes/channels.toml` capability gate, `GET /api/v1/broadcast/members` presence roster under the same gate, and host-side `ChannelTokenAuthorizer` validation at every `/realtime/sse|ws` connect (uniform 401; token never grants more than its channel) with `presence.join`/`presence.leave` events | P2 |
| Storage abstraction | EQUIVALENT | platform/crates/services/storage/src/lib.rs:23 | StoragePort | P1 |
| Local storage | MATCHED | platform/crates/services/storage/src/lib.rs:23 | Local implementation | P2 |
| S3 storage | STUB | platform/crates/services/storage/src/lib.rs:23 | S3 path is stub | P2 |
| Mail | EQUIVALENT | mail adapter supplied in audit facts | Adapter surface exists; HTML/template depth remains incomplete | P2 |
| SMS | EQUIVALENT | SMS adapter supplied in audit facts | Adapter surface exists | P3 |
| Push notifications | EQUIVALENT | push adapter supplied in audit facts | Adapter surface exists | P3 |
| Notifications | EQUIVALENT | platform/crates/services/notifications/src/lib.rs (NotifyHub, NotificationDef, NotificationStore port); platform/crates/data/postgres/src/notificationstore.rs; platform/runtime/keel-api/src/notification_routes.rs | Typed per-channel renderers (mail/sms/push/database), `notifications` table + in-memory/pg stores behind one port, inbox `GET /api/v1/notifications` + capability-gated `POST /read` (docs/04; tests notifications_endpoints_require_authentication / feed_lists_newest_first_with_unread_meta / read_marks_rows_and_gates_the_capability) (Wave 6 P5); on-demand notifiables (`dispatch_on_demand` + sha256 `OnDemandAudit`, D-74 caller-persisted), the `queue_via` v1-envelope seam (render-at-enqueue, keel-api pool-gated enqueue / keel-worker drain) and appointments producer dispatch with locale e2e landed Wave 6 P6 | P2 |
| Search | PARTIAL | platform/crates/services/search/src/lib.rs | In-memory search only | P2 |
| pgvector | MISSING | platform/crates/services/search/src/lib.rs | No pgvector adapter | P2 |
| HTTP client | MATCHED | platform/crates/services/http-client/src/lib.rs (17 tests incl. live loopback: retry-after-500, retry_on exclusion, per-request 100ms timeout vs 700ms server, redacted taps across retries, pool bound == 2, HttpFake asserts, 100KB sink round trip + 404 refusal) | Wave 6 P10: retry schedule `{times, base_ms, max_ms, on}`, per-request timeout, bounded pool, redacted tap hooks, HttpFake assertions, streaming `sink_to` over `StoragePort::writer`; honest gap: no multipart/streamed request bodies (outside the Wave 6 parity scope) | P2 |
| Process API | UNKNOWN | no locked evidence anchor supplied | Depth not proven | P3 |
| Images | UNKNOWN | no locked evidence anchor supplied | No current production claim | P3 |
| Collections | DELIBERATELY EXCLUDED | Rust iterator model | Rust iterators preferred over Laravel collection wrapper | P3 |
| Strings/helpers | SUPERSEDED | Rust standard-library / typed utilities strategy | Avoid PHP helper cloning where native Rust is stronger | P3 |
| Concurrency | SUPERSEDED | tokio + ExecutionContext supplied in audit facts | Native structured async model is deliberate differentiation | P1 |
| Context | SUPERSEDED | ExecutionContext supplied in audit facts | Typed execution context is deliberate differentiation | P1 |
| Rate limiting | MATCHED | platform/crates/web/http/src/middleware.rs (client_key); platform/crates/web/http/src/middleware_catalog.rs | Per-principal/group token buckets (S7 + Wave 1): throttle:<group> keyed x-rate-key > bearer hash > first x-forwarded-for IP > global; 429 + Retry-After | P1 |
| OpenAPI | MATCHED | route/OpenAPI emission in graph path supplied | Schema emission exists | P2 |
| Swagger UI | MISSING | OpenAPI UI not supplied | Documentation UI absent | P3 |
| GraphQL | MISSING | no GraphQL core supplied | Leave to ecosystem unless strategy changes | P3 |
| Webhooks | EQUIVALENT | adapter exists per audit facts | Signature/idempotency depth requires further hardening | P2 |
| AI/MCP | SUPERSEDED | MCP/AI-native capabilities supplied | First-class capability-aware machine interface | P1 |
| Audit/replay | SUPERSEDED | delta/audit/replay/inspect supplied | Stronger operational forensic model | P1 |
| Testing framework | EQUIVALENT | platform/testing/testkit/src/fakes.rs, app.rs, time.rs, factories.rs (assertion-bearing fakes + the TestResponse/TestApp matrix over session/view/model probes, `freeze_time`/`travel`, `create`/`state`/`Sequence`); platform/crates/data/postgres/src/test_support.rs (savepoint rollback); docs/11-TESTING | Strong framework test primitives (Wave 7: assert/panic fake pairs, full assertion matrix, time travel, transactional savepoints, factory runtime; workspace 1312 tests / 150 suites) | P1 |
| PostgreSQL integration tests | PARTIAL | platform/crates/data/postgres/tests/pg_integration.rs; platform/crates/data/postgres/tests/pg_modelstore.rs (DATABASE_URL-gated suites: CRUD + soft-delete veil roundtrip, tenant isolation, upsert merge, belongs_to eager load, sessions/tokens) | Live-pg suites exist (the snapshot's "zero sqlx::test" claim predates them); the Wave 3 pg-matrix additions (aggregates/exists/restore+force_delete/version column/insert_or_ignore/chunk streaming) were not added - B-19 stays open | P0 |
| Browser / E2E | UNKNOWN | no locked evidence anchor supplied | Dusk equivalent not established | P3 |
| Package ecosystem | PARTIAL | 7 adapters / manifests / compile-time modules | Extension discovery/registry is incomplete | P2 |
| Plugin discovery | MISSING | D-69 | No registry/discovery surface | P2 |
| Multi-DB | DELIBERATELY EXCLUDED | D-62; revisit v2.0 | Dependency cap / scope decision | P3 |
| MongoDB | DELIBERATELY EXCLUDED | D-62 scope | Not a v1 target | P3 |
| Dashboard UI | EQUIVALENT | `/_dashboard` lite (S10: boot/flags/routes/health + `/metrics`) + `/_dashboard/ops` (Wave 8: queue depth, recent jobs, schedule state, execution inspect — `runtime/keel-api/src/dashboard.rs` + platform-shipped `views/dashboard/ops.html`) | Flag-gated (`app.dashboard`), read-only by design; ADR 0009 Accepted (decision gate 2026-10-02); mutating dashboard actions stay on the console commands | P3 |
| Tinker equivalent | MISSING | no interactive shell supplied | Developer exploration gap | P3 |
| Serve dev command | MISSING | no keel serve supplied | Local app loop broken | P1 |
| Vendor publish | MISSING | no vendor:publish equivalent supplied | Package resource publishing absent | P3 |
| Route cache | MATCHED | platform/crates/application/registry/src/route_plan.rs; platform/tools/keel-console/src/main.rs (route:cache) | storage/framework/routes.json artifact pinned to a SHA-256 manifest hash; stale artifact refuses boot (Wave 1; stricter than Laravel) | P2 |
| Config cache | MATCHED | platform/crates/application/registry/src/config_cache.rs; platform/tools/keel-console/src/main.rs (config:cache); test `config_cache_cycle_pins_and_clears_the_artifact` | storage/framework/config.json + content hash; boot `config_cache_check` refuses drift (Wave 4; stricter than Laravel) | P2 |
| View cache | MATCHED | platform/tools/keel-console/src/main.rs (view:cache/view:clear); test `view_cache_cycle_compiles_and_clears` | `[views].cache_dir` artifacts (mtime + content hash); boot recompiles on demand (Wave 2) | P3 |
| Horizon equivalent | EQUIVALENT | `/_dashboard/ops` over the existing job/scheduler/trace stores; `queue:monitor` / `keel inspect` terminal equivalents | Read-only queue dashboard shipped (Wave 8 decision gate, `specs/ADR/0009` Accepted 2026-10-02); job payload and trace input never rendered; worker mutation actions remain console commands | P3 |
| Telescope equivalent | SUPERSEDED | keel inspect/replay | Operational introspection is more general | P2 |
| Breeze/Starter Kit | MISSING | no starter kit supplied | Adoption friction | P2 |
| Jetstream equivalent | MISSING | no full starter stack supplied | Enterprise auth UI not provided | P3 |
| Inertia equivalent | UNKNOWN | S9 web story undecided | SPA integration decision pending | P2 |
| Livewire equivalent | UNKNOWN | S9 web story undecided | Server-driven UI decision pending | P2 |
| Cashier equivalent | MISSING | no billing package supplied | Ecosystem item, not core blocker | P4 |
| Socialite equivalent | MISSING | no social auth package supplied | Ecosystem item | P4 |
| Dusk equivalent | MISSING | no browser automation surface supplied | Ecosystem item | P4 |
| Valet equivalent | DELIBERATELY EXCLUDED | native Rust/container workflow | Local macOS PHP proxy is not a core Rust concern | P4 |

### Migration capability matrix

**Framework schema inventory:** the supplied locked facts establish **18 framework tables across `platform/database/migrations/*.sql`**. The repository migration files are available for this audit and define, in order: `tenants`, `principals`, `credentials`, `sessions`, `revisions`, `memberships`, `scope_nodes`, `scope_grants`, `capability_grants`, `delta_streams`, `delta_stones`, `audit_records`, `outbox`, `idempotency_keys`, `elevation_leases` (foundation migration) and `jobs_queue`, `diag_chain`, `auth_tokens` (services migration). Evidence: `platform/database/migrations/20260819000001_foundation.sql`, `platform/database/migrations/20260820000002_services.sql`. The migration behavior matrix below remains the source-of-truth gap ledger for operation-level behavior.

| Operation | Laravel capability | Keel status | Evidence / gap | Priority |
|---|---|---|---|---|
| create table | Migration SQL / keel migrate | MATCHED | platform/database/migrations/*.sql; keel migrate/seed | P1 |
| drop table | Migration SQL / keel migrate | MATCHED | platform/database/migrations/*.sql; keel migrate/seed | P1 |
| drop if exists | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| rename table | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| add column | Migration SQL / keel migrate | MATCHED | platform/database/migrations/*.sql; keel migrate/seed | P1 |
| alter column | Migration SQL / keel migrate | MATCHED | platform/database/migrations/*.sql; keel migrate/seed | P1 |
| rename column | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| drop column | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| nullable | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| default | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| primary key | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| composite primary key | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| unique | Migration SQL / keel migrate | MATCHED | platform/database/migrations/*.sql; keel migrate/seed | P1 |
| composite unique | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| index | Migration SQL / keel migrate | MATCHED | platform/database/migrations/*.sql; keel migrate/seed | P1 |
| full-text index | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| foreign key | Migration SQL / keel migrate | MATCHED | platform/database/migrations/*.sql; keel migrate/seed | P1 |
| cascade | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| restrict | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| set null | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| enum | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| json | Migration SQL / keel migrate | MATCHED | platform/database/migrations/*.sql; keel migrate/seed | P1 |
| jsonb | Migration SQL / keel migrate | MATCHED | platform/database/migrations/*.sql; keel migrate/seed | P1 |
| uuid | Migration SQL / keel migrate | MATCHED | platform/database/migrations/*.sql; keel migrate/seed | P1 |
| ulid | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| timestamps | Migration SQL / keel migrate | MATCHED | platform/database/migrations/*.sql; keel migrate/seed | P1 |
| soft deletes | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| generated columns | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| check constraints | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| comments | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| raw SQL | Migration SQL / keel migrate | MATCHED | platform/database/migrations/*.sql; keel migrate/seed | P1 |
| transactional migration | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| reversible migration | Migration SQL / keel migrate | PARTIAL | Migration system exists but operation-level behavior is not proven by supplied anchors | P2 |
| migration batch | Migration SQL / keel migrate | MATCHED | platform/database/migrations/*.sql; keel migrate/seed | P1 |
| migration status | Migration SQL / keel migrate | MISSING | No locked evidence for equivalent workflow | P2 |
| rollback | Migration SQL / keel migrate | UNKNOWN | Depth not locked in supplied evidence | P2 |
| reset | Migration SQL / keel migrate | MISSING | No locked evidence for equivalent workflow | P2 |
| refresh | Migration SQL / keel migrate | MISSING | No locked evidence for equivalent workflow | P2 |
| fresh | Migration SQL / keel migrate | MISSING | No locked evidence for equivalent workflow | P2 |
| migration lock | Migration SQL / keel migrate | MISSING | No locked evidence for equivalent workflow | P2 |
| concurrent migration | Migration SQL / keel migrate | MISSING | No locked evidence for equivalent workflow | P2 |
| schema dump | Migration SQL / keel migrate | MISSING | No locked evidence for equivalent workflow | P2 |

### View capability matrix

| View capability | Laravel baseline | Keel status | Evidence / gap | Priority |
|---|---|---|---|---|
| View resolution | Laravel 13 | UNKNOWN | S9 decision | No frozen template engine |
| View paths | Laravel 13 | UNKNOWN | S9 | Pending SSR/SPA decision |
| Layouts | Laravel 13 | EQUIVALENT | platform/crates/web/views/src/parser.rs (Wave 2 directive ledger) | `@extends`/`@section`/`@yield`/`@parent`; layout facet test |
| Partials | Laravel 13 | EQUIVALENT | platform/crates/web/views/src/parser.rs (Wave 2 directive ledger) | `@include`/`@includeIf`/`@includeWhen` |
| Components | Laravel 13 | EQUIVALENT | platform/crates/web/views/src/parser.rs (Wave 2 directive ledger) | `@component` with `@props` |
| Slots | Laravel 13 | EQUIVALENT | platform/crates/web/views/src/parser.rs (Wave 2 directive ledger) | `@slot`/`@endslot` with defaults; component-slot facet test |
| Sections/yield | Laravel 13 | EQUIVALENT | platform/crates/web/views/src/parser.rs (Wave 2 directive ledger) | `@section`/`@yield`/`@show`/`@parent` in the ledger |
| Escaping | Laravel 13 | EQUIVALENT | platform/crates/web/views/src/parser.rs (Wave 2 directive ledger) | `{{ }}` auto-escapes; escaping XSS facet test |
| Raw output | Laravel 13 | EQUIVALENT | platform/crates/web/views/src/parser.rs (Wave 2 directive ledger) | `{!! !!}` raw echo |
| Loops/conditionals | Laravel 13 | EQUIVALENT | platform/crates/web/views/src/parser.rs (Wave 2 directive ledger) | `@if`/`@unless`/`@foreach`/`@for`/`@forelse`; `@while` SUPERSEDED |
| Custom directives | Laravel 13 | EQUIVALENT | platform/crates/web/views/src/engine.rs (register_directive, register_conditional) | Custom directive registry + Blade::if-style conditionals |
| Component attributes | Laravel 13 | EQUIVALENT | platform/crates/web/views/src/parser.rs (Wave 2 directive ledger) | `@props` plus attribute helpers `@class`/`@style`/`@checked`/... |
| View caching | Laravel 13 | EQUIVALENT | platform/crates/web/views/src/engine.rs (precompile_all/clear_cache); platform/tools/keel-console/src/main.rs (view_cache_at) | Compiled-AST cache under `[views].cache_dir`, invalidated by mtime + content hash; `keel view:cache`/`view:clear` (Wave 2) |
| Compiled templates | Laravel 13 | UNKNOWN | S9 | Engine not selected |
| Mail views | Laravel 13 | PARTIAL | platform/runtime/keel-api/src/host.rs (ViewsNotificationRenderer); `Mailable.html` | Notification mail bodies render from view templates (Wave 6 P5); generic mailables stay caller-supplied (P1 posture) |
| Notification views | Laravel 13 | EQUIVALENT | platform/crates/web/views/src/engine.rs; platform/runtime/keel-api/src/host.rs (ViewsNotificationRenderer) | `views/notifications/<type>.html` renders the mail HTML body with the database payload as context; render failure fails closed, no view port = text-only (Wave 6 P5; test notification_html_mail_renders_through_the_views_engine). Plain views — no markdown layout kit (documented) |
| Error views | Laravel 13 | UNKNOWN | S9 | Template engine pending |
| Pagination views | Laravel 13 | UNKNOWN | S9 | Pagination is API-first currently |

### Function-level Laravel capability ledger

This ledger intentionally enumerates individual operations so a broad `Database` or `Validation` label cannot conceal a missing runtime function.

| Subsystem | Laravel function / capability | Keel status | Evidence | Gap |
|---|---|---|---|---|
| Routing / URL | GET | MATCHED | platform/runtime/keel-api/src/graph_routes.rs (declared_methods/route_methods); platform/runtime/keel-api/tests/routing_edge_integration.rs | GET dispatches and also answers auto-HEAD (body stripped) and the auto-OPTIONS probe; Declared verb dispatches at the edge; wrong verb on the path -> 405 with `Allow`; verb-mismatch unit `verb_mismatch_is_405_with_allow` (Wave 2) |
| Routing / URL | POST | MATCHED | platform/runtime/keel-api/src/graph_routes.rs (declared_methods/route_methods); platform/runtime/keel-api/tests/routing_edge_integration.rs | Declared verb dispatches at the edge; wrong verb on the path -> 405 with `Allow`; verb-mismatch unit `verb_mismatch_is_405_with_allow` (Wave 2); store flows verified in the http_integration CRUD roundtrip |
| Routing / URL | PUT | MATCHED | platform/runtime/keel-api/src/graph_routes.rs (declared_methods/route_methods); platform/runtime/keel-api/tests/routing_edge_integration.rs | Declared verb dispatches at the edge; wrong verb on the path -> 405 with `Allow`; verb-mismatch unit `verb_mismatch_is_405_with_allow` (Wave 2); update flows verified in the http_integration CRUD roundtrip |
| Routing / URL | PATCH | MATCHED | platform/runtime/keel-api/src/graph_routes.rs (declared_methods/route_methods); platform/runtime/keel-api/tests/routing_edge_integration.rs | Declared verb dispatches at the edge; wrong verb on the path -> 405 with `Allow`; verb-mismatch unit `verb_mismatch_is_405_with_allow` (Wave 2); resource `update` registers PUT + PATCH under one operation |
| Routing / URL | DELETE | MATCHED | platform/runtime/keel-api/src/graph_routes.rs (declared_methods/route_methods); platform/runtime/keel-api/tests/routing_edge_integration.rs | Declared verb dispatches at the edge; wrong verb on the path -> 405 with `Allow`; verb-mismatch unit `verb_mismatch_is_405_with_allow` (Wave 2); cancel/destroy flows verified in http_integration |
| Routing / URL | OPTIONS | MATCHED | platform/runtime/keel-api/src/graph_routes.rs (declared_methods/route_methods); platform/runtime/keel-api/tests/routing_edge_integration.rs | Auto-OPTIONS probe answers 204 + `Allow` for every method the path serves; an explicit OPTIONS route wins (`get_route_answers_the_auto_options_probe`) |
| Routing / URL | HEAD | MATCHED | platform/runtime/keel-api/src/graph_routes.rs (declared_methods/route_methods); platform/runtime/keel-api/tests/routing_edge_integration.rs | Auto-HEAD mirrors GET with the body stripped (8 MiB cap) via `strip_head_body` (`get_route_answers_head_without_a_body`) |
| Routing / URL | ANY | MATCHED | platform/runtime/keel-api/src/graph_routes.rs (declared_methods/route_methods); platform/runtime/keel-api/tests/routing_edge_integration.rs | `method = "ANY"` expands to the full verb set incl. OPTIONS at registration and dispatches all of them (`any_route_answers_every_declared_verb`) |
| Routing / URL | fallback | API-ONLY | platform/crates/application/registry/src/lib.rs:1005 (GRAPH-FALLBACK-DUP); platform/crates/application/registry/src/route_plan.rs:145 | Wave 2 parses `fallback = true`, gates GRAPH-FALLBACK-DUP (max one per plane) and carries it in the route plan; the HTTP edge does not claim unmatched paths - router fallback serves static files / renders not-found (edge wiring deferred) |
| Routing / URL | group | MATCHED | platform/crates/application/registry/src/lib.rs (apply_groups); platform/tools/keel-console/src/main.rs (route_list_filters_carry_the_parity_columns) | `[[route_groups]]` members inherit prefix/middleware/plane at parse (route fields win; group middleware runs outermost); GRAPH-GROUP-UNKNOWN gate (Wave 2) |
| Routing / URL | prefix | MATCHED | platform/crates/application/registry/src/lib.rs (normalize_prefix/apply_groups) | Group prefixes fold into the route path at parse (slash-normalized); `route:list` prints the merged paths (Wave 2) |
| Routing / URL | domain | MATCHED | platform/runtime/keel-api/src/graph_routes.rs (RouteGate); platform/runtime/keel-api/tests/routing_edge_integration.rs (domain_gate_matches_before_the_auth_chain) | `domain` matches the request Host exactly, `*.example.test` matches any subdomain; the gate runs before auth - mismatch 404s (Wave 2) |
| Routing / URL | subdomain | MATCHED | platform/runtime/keel-api/src/graph_routes.rs (RouteGate); platform/runtime/keel-api/tests/routing_edge_integration.rs (subdomain_gate_404s_the_wrong_host_and_feeds_the_tenant) | `subdomain` resolves a tenant through the host feed into tenant resolution; the wrong host 404s (Wave 2) |
| Routing / URL | name | MATCHED | platform/crates/application/registry/src/lib.rs (RouteSpec.name, GRAPH-DUP-ROUTENAME, resolve_url); docs/19-PARITY-PROGRAM.md:96 | Named routes generate URLs with `[url]` defaults; `keel route:list --name` filters them (Wave 1-2) |
| Routing / URL | where | SUPERSEDED | platform/crates/application/registry/src/route_plan.rs (ConstraintSpec { param, kind, pattern }); docs/19-PARITY-PROGRAM.md:90 | Closed constraint kinds plus a hand-interpreted pattern mini-grammar (chars, classes, `{n,m}`) replace arbitrary-regex `where` - documented divergence, no regex crate (Wave 2) |
| Routing / URL | whereNumber | MATCHED | platform/crates/application/registry/src/route_plan.rs; platform/runtime/keel-api/tests/routing_edge_integration.rs (constraint_mismatch_is_a_route_miss_for_every_kind) | `kind = "number"` resolves as a routing predicate; mismatch is a route miss (404) before auth (Wave 2) |
| Routing / URL | whereAlpha | MATCHED | platform/crates/application/registry/src/route_plan.rs; platform/runtime/keel-api/tests/routing_edge_integration.rs (constraint_mismatch_is_a_route_miss_for_every_kind) | `kind = "alpha"` resolves as a routing predicate; mismatch is a route miss (404) (Wave 2) |
| Routing / URL | whereAlphaNumeric | MATCHED | platform/crates/application/registry/src/route_plan.rs; platform/runtime/keel-api/tests/routing_edge_integration.rs (constraint_mismatch_is_a_route_miss_for_every_kind) | `kind = "alpha-num"` resolves as a routing predicate; mismatch is a route miss (404) (Wave 2) |
| Routing / URL | middleware | MATCHED | platform/crates/application/registry/src/middleware_catalog.rs (resolve_chain); platform/runtime/keel-api/src/graph_routes.rs | Route middleware = [...] resolves alias-first then group, flattens, runs after the implicit api group (Wave 1); unknown entries fail GRAPH-MW-UNKNOWN |
| Routing / URL | withoutMiddleware | MATCHED | platform/crates/application/registry/src/middleware_catalog.rs (excluded_middleware_* tests); platform/runtime/keel-api/src/graph_routes.rs | `excluded_middleware` removes entries from the resolved chain (groups flattened first); naming an alias outside the chain fails GRAPH-MW-UNKNOWN (Wave 2) |
| Routing / URL | controller | EQUIVALENT | platform/runtime/keel-api/src/graph_routes.rs (operation dispatch); docs/18 Core-architecture `Controllers` row | No `Route::controller` class registration; HTTP dispatches to declared module operations behind the typed RequestViewModel/ResponseViewModel controller boundary (Wave 2) |
| Routing / URL | resource | EQUIVALENT | platform/crates/application/registry/src/lib.rs (RESOURCE_VERBS, resource_expands_into_six_route_specs) | `resource` expands at parse into index/show/store/update (PUT+PATCH)/destroy with capability + names; the web create/edit form verbs are not auto-generated - declare them as explicit HTML routes (Wave 1) |
| Routing / URL | apiResource | MATCHED | platform/crates/application/registry/src/lib.rs (RESOURCE_VERBS) | Expansion matches Laravel `apiResource` exactly (index, show, store, update x2, destroy) with `only`/`except` verb filters; the flat list feeds route:list/OpenAPI/the edge (Wave 1) |
| Routing / URL | singleton | EQUIVALENT | platform/crates/application/registry/src/lib.rs (SINGLETON_VERBS, singleton_expands_without_id_into_four_route_specs) | `singleton = true` expands show/update (PUT+PATCH)/destroy without `{id}`; vs Laravel defaults (show/edit/update plus optional creatable/destroyable): the edit form page is not auto-generated and destroy is on by default; `only`/`except` narrows (Wave 2) |
| Routing / URL | apiSingleton | EQUIVALENT | platform/crates/application/registry/src/lib.rs (SINGLETON_VERBS); platform/crates/application/registry/src/lib.rs:1564 (resource/singleton expansion units) | Laravel `apiSingleton` (`only = store/show/update/destroy` over singleton defaults) is covered by `singleton = true` (show/update/destroy without `{id}`); a store on the instance path is declared as an explicit route (Wave 2) |
| Routing / URL | scopeBindings | PARTIAL | platform/crates/application/registry/src/lib.rs (BindingSpec); platform/runtime/keel-api/tests/http_integration.rs (binding hit/miss/cross-tenant -> 404) | Bindings resolve per path param, tenant-scoped (cross-tenant -> 404); no parent -> child binding-scoping model exists in the graph (Wave 1-2) |
| Routing / URL | scopeBindingsByResource | PARTIAL | platform/crates/application/registry/src/lib.rs (BindingSpec); platform/runtime/keel-api/tests/http_integration.rs (binding hit/miss/cross-tenant -> 404) | Same model: per-param tenant-scoped resolution; resource-family scoping is not part of the graph (Wave 1-2) |
| Routing / URL | binding | MATCHED | platform/runtime/keel-api/src/graph_routes.rs; platform/applications/appointments/src/lib.rs (AppointmentBindingResolver) | bindings = [{ param, model }] on RouteSpec; resolver registered in the route.bindings service (Wave 1) |
| Routing / URL | model binding | MATCHED | platform/runtime/keel-api/src/graph_routes.rs (_bindings injection); applications/appointments/module.toml (DELETE cancel) | Edge resolves {id} tenant-scoped and injects _bindings.<param> after the merge; malformed/miss/cross-tenant → 404; missing resolver fails GRAPH-BINDING-RESOLVER at boot (Wave 1) |
| Routing / URL | signed URL | MATCHED | platform/crates/web/http/src/signed.rs (tamper/expiry units); platform/runtime/keel-api/src/graph_routes.rs (`signed` builtin); platform/runtime/keel-api/tests/routing_edge_integration.rs (one_time_signed_link_redempts_exactly_once) | HMAC-SHA256 gate via the `signed` middleware builtin; unsigned -> 403, tampered/expired refused, fail-closed 500 without `app.key` (Wave 2) |
| Routing / URL | temporary signed URL | MATCHED | platform/crates/web/http/src/signed.rs (expiry unit tests); platform/runtime/keel-api/tests/routing_edge_integration.rs | TTL is carried in the link and verified at resolution; Keel adds a one-time redemption variant (spent link -> 404) (Wave 2); TTL bound: `[security.signed_urls].max_ttl_secs` (default 7d, 30d boot ceiling) refuses over-bound boot and binds `sign_once` minting (Wave 7 P6) |
| Routing / URL | url defaults | MATCHED | platform/crates/application/registry/src/lib.rs (UrlSection, resolve_url_falls_back_to_registered_url_defaults) | `[url] defaults` fill `{param}` entries `resolve_url` is not given explicitly; caller values win (Laravel `URL::defaults`) (Wave 2) |
| Routing / URL | route:list | MATCHED | platform/tools/keel-console/src/main.rs (route_rows/route_list_at; test route_list_filters_carry_the_parity_columns) | table/json with plane/group/version/deprecated/name columns; `--name`/`--path` case-insensitive substring filters, `--plane` validated `api`/`web` (Wave 2) |
| Routing / URL | route:cache | MATCHED | platform/tools/keel-console/src/main.rs (route_cache_at); platform/crates/application/registry/src/route_plan.rs | keel route:cache writes storage/framework/routes.json pinned to a SHA-256 manifest hash; boot registers from a matching artifact and refuses a stale one (Wave 1) |
| Routing / URL | route:clear | MATCHED | platform/tools/keel-console/src/main.rs (route_clear_at) | keel route:clear deletes the artifact; boot falls back to live compilation (Wave 1) |
| Validation | required | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | nullable | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | sometimes | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | string | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | integer | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | numeric | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | boolean | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | array | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | json | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | email | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | url | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | uuid | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | ulid | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | date | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | date_format | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | before | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | after | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | before_or_equal | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | after_or_equal | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | between | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | min | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | max | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | size | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | in | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | not_in | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | in_array | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | same | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | different | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | confirmed | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | regex | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | not_regex | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | required_if | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | required_unless | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | required_with | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | required_without | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | prohibited | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | prohibited_if | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | exclude | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | exclude_if | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | exists | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | unique | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | distinct | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | filled | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | present | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | file | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | image | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | mimes | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | mimetypes | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | extensions | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | dimensions | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | password | EQUIVALENT | platform/crates/foundation/validation/src/validator.rs (`PasswordPolicy` struct) + the composable `password` rule; platform/crates/application/configuration/src/lib.rs (`[auth.password_policy]`); platform/runtime/keel-api/src/auth_routes.rs (`validate_password` — register + both reset flows) | Rule battery proven Wave 4; config-driven policy (Laravel defaults) wired into both flows Wave 7 P6 (`register_rejects_a_password_below_the_configured_policy`, `reset_complete_rejects_a_password_below_the_configured_policy`) |
| Validation | current_password | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | enum | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | ascii | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | lowercase | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | uppercase | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | hex_color | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | ip | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | ipv4 | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | ipv6 | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | mac_address | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | timezone | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | decimal | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | multiple_of | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | digits | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | digits_between | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | gt | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | gte | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | lt | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |
| Validation | lte | EQUIVALENT | Validator / Rule enum supplied in audit facts | HTTP wiring and per-rule behavior must be proven before MATCHED |

| Query Builder | select | MATCHED | platform/crates/execution/query/src/select.rs (select projection batches) | Batch projection rendering tested; clause-tree battery proves the shape (Wave 3 Phase 1) |
| Query Builder | addSelect | MATCHED | platform/crates/execution/query/src/select.rs (addSelect batches) | Multiple projection batches render bind-continuously (Wave 3 Phase 1) |
| Query Builder | from | MATCHED | platform/crates/execution/query/src/select.rs | Clause-tree rendering proven by the select.rs test battery (Wave 3) |
| Query Builder | distinct | MATCHED | platform/crates/execution/query/src/select.rs (distinct) | SELECT DISTINCT rendering tested (Wave 3 Phase 1) |
| Query Builder | where | MATCHED | platform/crates/execution/query/src/select.rs (clause tree: Junction/Clause) | Recursive group rendering with bind continuity (Wave 3 Phase 1) |
| Query Builder | orWhere | MATCHED | platform/crates/execution/query/src/select.rs (or_where family + nested groups) | Parenthesised OR groups tested (Wave 3 Phase 1) |
| Query Builder | whereIn | MATCHED | platform/crates/execution/query/src/select.rs (where_in) | Clause-tree rendering proven by the select.rs test battery (Wave 3) |
| Query Builder | whereNotIn | MATCHED | platform/crates/execution/query/src/select.rs (where_not_in) | NOT IN renders through the clause tree (Wave 3 Phase 1) |
| Query Builder | whereNull | MATCHED | platform/crates/execution/query/src/select.rs (where_null) | Clause-tree rendering proven by the select.rs test battery (Wave 3) |
| Query Builder | whereNotNull | MATCHED | platform/crates/execution/query/src/select.rs (where_not_null) | Clause-tree rendering proven by the select.rs test battery (Wave 3) |
| Query Builder | whereBetween | MATCHED | platform/crates/execution/query/src/select.rs (where_between) | Range filter through the clause tree (Wave 3 Phase 1) |
| Query Builder | whereNotBetween | MATCHED | platform/crates/execution/query/src/select.rs (not_between) | Range exclusion renders bind-continuously (Wave 3 Phase 1) |
| Query Builder | whereDate | MATCHED | platform/crates/execution/query/src/select.rs (where_date) | Truncated date comparison; render-tested (Wave 3 Phase 1) |
| Query Builder | whereMonth | MATCHED | platform/crates/execution/query/src/select.rs (where_month) | Month extraction comparison; render-tested (Wave 3 Phase 1) |
| Query Builder | whereYear | MATCHED | platform/crates/execution/query/src/select.rs (where_year) | Year extraction comparison; render-tested (Wave 3 Phase 1) |
| Query Builder | whereColumn | MATCHED | platform/crates/execution/query/src/select.rs (where_column(_op)) | Column-to-column comparison, no binds (Wave 3 Phase 1) |
| Query Builder | whereExists | MATCHED | platform/crates/execution/query/src/select.rs (where_exists/where_not_exists; test or_where_exists_joins_the_preceding_clause) | Correlated subquery against the relation (Wave 3 Phase 4) |
| Query Builder | join | PARTIAL | platform/crates/execution/query/src/select.rs | Typed Select exists; remaining operation not proven |
| Query Builder | leftJoin | PARTIAL | platform/crates/execution/query/src/select.rs | Typed Select exists; remaining operation not proven |
| Query Builder | rightJoin | PARTIAL | platform/crates/execution/query/src/select.rs | Typed Select exists; remaining operation not proven |
| Query Builder | crossJoin | MISSING | No write builder in supplied evidence | Write/advanced query operation not implemented |
| Query Builder | joinSub | MISSING | No write builder in supplied evidence | Write/advanced query operation not implemented |
| Query Builder | union | MATCHED | platform/crates/execution/query/src/select.rs (union/union_all) | Main tail moves to the composite end; render-tested (Wave 3 Phase 1) |
| Query Builder | unionAll | MATCHED | platform/crates/execution/query/src/select.rs (union/union_all) | Non-distinct composite; render-tested (Wave 3 Phase 1) |
| Query Builder | groupBy | MATCHED | platform/crates/execution/query/src/select.rs (group_by/having) | Aggregate grouping tested (Wave 3 Phase 1) |
| Query Builder | having | MATCHED | platform/crates/execution/query/src/select.rs (having + having_aggregate + HAVING-only guard) | Predicate on aggregates, HAVING-only guarded (Wave 3 Phase 1) |
| Query Builder | orderBy | MATCHED | platform/crates/execution/query/src/select.rs | Clause-tree rendering proven by the select.rs test battery (Wave 3) |
| Query Builder | latest | MATCHED | platform/crates/execution/query/src/select.rs (latest, column override) | Default created_at descending ordering (Wave 3 Phase 1) |
| Query Builder | oldest | MATCHED | platform/crates/execution/query/src/select.rs (oldest, column override) | Default created_at ascending ordering (Wave 3 Phase 1) |
| Query Builder | limit | MATCHED | platform/crates/execution/query/src/select.rs | Clause-tree rendering proven by the select.rs test battery (Wave 3) |
| Query Builder | offset | MATCHED | platform/crates/execution/query/src/select.rs | Clause-tree rendering proven by the select.rs test battery (Wave 3) |
| Query Builder | cursor | EQUIVALENT | platform/crates/data/models/src/runtime.rs (cursor; test cursor_iterates_lazily_across_pages) | Lazy RecordCursor pages through the store instead of one SQL stream - documented divergence from Laravel cursor() (Wave 3) |
| Query Builder | lockForUpdate | MATCHED | platform/crates/execution/query/src/select.rs (lock_for_update -> FOR UPDATE) | Pessimistic write-lock clause tested (Wave 3 Phase 1) |
| Query Builder | sharedLock | MATCHED | platform/crates/execution/query/src/select.rs (shared_lock -> FOR SHARE) | Pessimistic read-lock clause tested (Wave 3 Phase 1) |
| Query Builder | insert | EQUIVALENT | platform/crates/data/models/src/runtime.rs (insert; test insert_requires_tenant_and_generates_id); pg roundtrip in platform/crates/data/postgres/tests/pg_modelstore.rs | Writes ride ModelRuntime/ModelStore, not the compiler (documented); tenant-scoped insert with RETURNING (Wave 3) |
| Query Builder | insertGetId | EQUIVALENT | platform/crates/data/models/src/runtime.rs (insert_get_id; test insert_get_id_and_insert_or_ignore) | Insert + RETURNING id through the runtime (Wave 3) |
| Query Builder | insertOrIgnore | EQUIVALENT | platform/crates/data/models/src/runtime.rs (insert_or_ignore; test insert_get_id_and_insert_or_ignore) | ON CONFLICT DO NOTHING through the runtime; pg wire proof pending pg-matrix additions (Wave 3) |
| Query Builder | upsert | EQUIVALENT | platform/crates/data/models/src/runtime.rs (upsert); store upsert_merge; pg test pg_modelstore_upsert_merges_on_conflict | Conflict-merge write rides the runtime/store (Wave 3) |
| Query Builder | update | EQUIVALENT | platform/crates/data/models/src/runtime.rs (update/update_tracked + version_column CAS); pg roundtrip in platform/crates/data/postgres/tests/pg_modelstore.rs | Partial update with RETURNING, dirty-only change set, optimistic lock (Wave 3) |
| Query Builder | updateOrInsert | EQUIVALENT | platform/crates/data/models/src/runtime.rs (update_or_insert; test update_or_insert_upserts_keyed_on_conflict_fields) | Keyed upsert through the runtime (Wave 3) |
| Query Builder | increment | EQUIVALENT | platform/crates/data/models/src/runtime.rs (increment; test increment_decrement_counter_and_guards) | Atomic SET col = col + n; bypasses CAS by design (documented); pg proof pending pg-matrix additions (Wave 3) |
| Query Builder | decrement | EQUIVALENT | platform/crates/data/models/src/runtime.rs (decrement; test increment_decrement_counter_and_guards) | Atomic SET col = col - n; bypasses CAS by design (documented); pg proof pending pg-matrix additions (Wave 3) |
| Query Builder | delete | EQUIVALENT | platform/crates/data/models/src/runtime.rs (delete - stamps deleted_at unless .hard_deletes()); pg veil in pg_modelstore_crud_roundtrip | Soft delete by default with a hard-delete path (Wave 3) |
| Query Builder | truncate | EQUIVALENT | platform/crates/data/models/src/store.rs (truncate, config-gated) | Destructive op rides the store behind a config gate (Wave 3) |
| Query Builder | count | MATCHED | platform/crates/execution/query/src/select.rs (count) | Aggregate compiles to SQL with tested shape (Wave 3 Phase 1) |
| Query Builder | max | MATCHED | platform/crates/execution/query/src/select.rs (max) | Aggregate compiles to SQL with tested shape (Wave 3 Phase 1) |
| Query Builder | min | MATCHED | platform/crates/execution/query/src/select.rs (min) | Aggregate compiles to SQL with tested shape (Wave 3 Phase 1) |
| Query Builder | avg | MATCHED | platform/crates/execution/query/src/select.rs (avg) | Aggregate compiles to SQL with tested shape (Wave 3 Phase 1) |
| Query Builder | sum | MATCHED | platform/crates/execution/query/src/select.rs (sum) | Aggregate compiles to SQL with tested shape (Wave 3 Phase 1) |
| Query Builder | exists | MATCHED | platform/crates/execution/query/src/select.rs (exists) | EXISTS compiles with tested shape (Wave 3 Phase 1) |
| Query Builder | doesntExist | MATCHED | platform/crates/execution/query/src/select.rs (doesnt_exist) | NOT EXISTS compiles with tested shape (Wave 3 Phase 1) |
| Cache | get | EQUIVALENT | platform/crates/services/cache/src/service.rs (`CacheService::get`; test `values_roundtrip_and_pull_deletes`) | CacheService over the unchanged port; recording-port battery (Wave 4), adapters keep their own backend suites |
| Cache | put | EQUIVALENT | platform/crates/services/cache/src/service.rs (`CacheService::put`; test `values_roundtrip_and_pull_deletes`) | CacheService over the unchanged port; recording-port battery (Wave 4), adapters keep their own backend suites |
| Cache | add | EQUIVALENT | platform/crates/services/cache/src/service.rs (`CacheService::add` via `put_if_absent`; test `add_is_atomic_and_forever_skips_the_ttl`) | CacheService over the unchanged port; recording-port battery (Wave 4), adapters keep their own backend suites |
| Cache | remember | EQUIVALENT | platform/crates/services/cache/src/service.rs (`CacheService::remember`; tests `remember_caches_the_miss_and_recomputes_after_removal`, `remember_waiter_collapses_onto_the_leader_result`) | CacheService over the unchanged port; stampede single-flight proven (Wave 4) |
| Cache | rememberForever | EQUIVALENT | platform/crates/services/cache/src/service.rs (`CacheService::remember_forever`; remember-family tests `remember_falls_back_when_the_leader_never_appears`) | CacheService over the unchanged port (Wave 4) |
| Cache | forever | EQUIVALENT | platform/crates/services/cache/src/service.rs (`CacheService::forever` skips the TTL; test `add_is_atomic_and_forever_skips_the_ttl`) | CacheService over the unchanged port; recording-port battery (Wave 4), adapters keep their own backend suites |
| Cache | forget | EQUIVALENT | platform/crates/services/cache/src/service.rs (`CacheService::delete` — Laravel `forget` mapping; `keel cache:forget` CLI) | CacheService over the unchanged port (Wave 4) |
| Cache | flush | EQUIVALENT | platform/crates/services/cache/src/service.rs (test `flush_clears_every_entry_at_once`) | CacheService over the unchanged port; memory clear / Redis `FLUSHDB` (Wave 4) |
| Cache | increment | EQUIVALENT | platform/crates/services/cache/src/service.rs (test `increment_and_decrement_track_integer_counters`) | CacheService over the unchanged port; recording-port battery (Wave 4), adapters keep their own backend suites |
| Cache | decrement | EQUIVALENT | platform/crates/services/cache/src/service.rs (test `increment_and_decrement_track_integer_counters`) | CacheService over the unchanged port; recording-port battery (Wave 4), adapters keep their own backend suites |
| Cache | pull | EQUIVALENT | platform/crates/services/cache/src/service.rs (test `values_roundtrip_and_pull_deletes` — get+delete once) | CacheService over the unchanged port (Wave 4) |
| Cache | many | EQUIVALENT | platform/crates/services/cache/src/service.rs (test `many_and_put_many_cover_hits_and_misses_in_order`) | CacheService over the unchanged port (Wave 4) |
| Cache | putMany | EQUIVALENT | platform/crates/services/cache/src/service.rs (`put_many`; same test) | CacheService over the unchanged port (Wave 4) |
| Cache | getMultiple | EQUIVALENT | platform/crates/services/cache/src/service.rs (`many` covers the multiple-get shape; same test) | CacheService over the unchanged port (Wave 4) |
| Cache | flexible | UNKNOWN | No function-level evidence | moka cache/lock exists; exact Laravel semantics require compatibility tests |
| Cache | lock | EQUIVALENT | platform/crates/services/cache/src/service.rs (`CacheGuard`; tests `guard_acquire_closure_release_and_force_release`, `with_lock` closure form) | Atomic lock over the port primitive, exposed as a service (Wave 4) |
| Cache | forceRelease | EQUIVALENT | platform/crates/services/cache/src/service.rs (`CacheGuard::force_release`; test `guard_acquire_closure_release_and_force_release`) | Atomic lock over the port primitive (Wave 4) |
| Cache | restoreLock | UNKNOWN | No function-level evidence | moka cache/lock exists; exact Laravel semantics require compatibility tests |
| Queues / Scheduler | dispatch | UNKNOWN | platform/runtime/keel-worker/src/main.rs | Worker/batches/chains/scheduler exist; exact API parity not established |
| Queues / Scheduler | dispatchSync | UNKNOWN | platform/runtime/keel-worker/src/main.rs | Worker/batches/chains/scheduler exist; exact API parity not established |
| Queues / Scheduler | dispatchAfterResponse | UNKNOWN | platform/runtime/keel-worker/src/main.rs | Worker/batches/chains/scheduler exist; exact API parity not established |
| Queues / Scheduler | delay | UNKNOWN | platform/runtime/keel-worker/src/main.rs | Worker/batches/chains/scheduler exist; exact API parity not established |
| Queues / Scheduler | onQueue | UNKNOWN | platform/runtime/keel-worker/src/main.rs | Worker/batches/chains/scheduler exist; exact API parity not established |
| Queues / Scheduler | onConnection | UNKNOWN | platform/runtime/keel-worker/src/main.rs | Worker/batches/chains/scheduler exist; exact API parity not established |
| Queues / Scheduler | chain | MATCHED | platform/crates/async/jobs/src/lib.rs (ChainRuntime) | Member N+1 enqueues only on N success (test `chain_advances_only_on_success`); a failure records the halted member position (`ChainStatus::Halted { position }`), repeats keep the first failure and a later success resumes through `Completed` (test `chain_failure_records_the_halted_position`) (Wave 6 P8) |
| Queues / Scheduler | batch | MATCHED | platform/crates/async/jobs/src/lib.rs (`BatchRuntime`/`BatchOps`/`BatchLedger`) + platform/runtime/keel-api/src/batch_routes.rs + platform/crates/data/postgres/src/batchledger.rs | Membership written once at dispatch to the durable `job_batches` ledger (saved before enqueue; `mark_finished` stamped when the run settles); `batch.status` counts live job-store state per member, `batch.cancel` takes pending members to dead (P8 `kill`), `batch.retry` requeues failed/dead members — capability-gated at `GET/POST /api/v1/batches/{id}[/cancel\|/retry]` (`batches.status`/`batches.cancel`/`batches.retry`, 403 without them) and shared with `queue:retry-batch` (tests `batch_status_counts_members_from_the_store`, `batch_cancel_and_retry_move_members`, `batch_ledger_records_finishes_and_prunes`, `batch_status_cancel_retry_roundtrip`, `batch_endpoints_require_authentication_and_capabilities`, `pg_batch_ledger_roundtrip`) (Wave 6 P9) |
| Queues / Scheduler | unique | MATCHED | platform/crates/async/jobs/src/lib.rs (`unique_key` + UniqueMiddleware) + platform/runtime/keel-worker/src/main.rs (CacheUniqueLock) | Per-def `unique_key`/`unique_ttl_secs` (default 300): admission takes `queue:unique:<job>:<key>` over the cache-lock port before the handler — held elsewhere → claim deferred without failing, lock released only on success (tests `unique_admission_holds_locked_claims_until_release`, `dispatch_skips_a_locked_unique_job`) (Wave 6 P8) |
| Queues / Scheduler | middleware | MATCHED | platform/crates/async/jobs/src/lib.rs (JobMiddleware / MiddlewareStack / JobMiddlewareStacks) | `before` → `Control::{Proceed, Hold, Abort}` + `after` observing every proceeded run; built-ins (unique → rate-limit → retry-limit) compose before module layers per job, the stack freezes at first dispatch (tests `middleware_stack_orders_hooks_and_short_circuits`, `stacks_compose_builtins_then_module_layers`, `dispatch_runs_module_middleware_around_the_handler`); codegen kind `job-middleware` emits `impl JobMiddleware` (Wave 6 P8) |
| Queues / Scheduler | retryUntil | UNKNOWN | platform/runtime/keel-worker/src/main.rs | Worker/batches/chains/scheduler exist; exact API parity not established |
| Queues / Scheduler | backoff | UNKNOWN | platform/runtime/keel-worker/src/main.rs | Worker/batches/chains/scheduler exist; exact API parity not established |
| Queues / Scheduler | timeout | UNKNOWN | platform/runtime/keel-worker/src/main.rs | Worker/batches/chains/scheduler exist; exact API parity not established |
| Queues / Scheduler | maxAttempts | MATCHED | platform/crates/async/jobs/src/lib.rs (`retry_limit` + RetryLimitMiddleware) | Per-def `retry_limit` = execution cap (`None` keeps the store's fail/dead path): a claim over the cap is killed terminal before the handler runs, with the reason in `last_error` (tests `retry_limit_aborts_over_budget_claims`, `dispatch_kills_an_over_budget_job`) (Wave 6 P8) |
| Queues / Scheduler | failed | UNKNOWN | platform/runtime/keel-worker/src/main.rs | Worker/batches/chains/scheduler exist; exact API parity not established |
| Queues / Scheduler | queue:work | MATCHED | platform/runtime/keel-worker/src/main.rs | Worker/batches/chains/scheduler exist; exact API parity not established |
| Queues / Scheduler | queue:restart | MATCHED | platform/tools/keel-console/src/main.rs (queue_restart) + runtime/keel-worker/src/main.rs | Restart-signal file written by `keel queue:restart`; the worker ignores a signal older than its start (test `restart_signal_is_ignored_when_older_than_the_worker_start`) (Wave 5) |
| Queues / Scheduler | queue:retry | UNKNOWN | platform/tools/keel-console/src/main.rs (queue_retry) | Implemented since Wave 2; no locked test evidence |
| Queues / Scheduler | queue:failed | UNKNOWN | platform/tools/keel-console/src/main.rs (queue_failed) | Implemented since Wave 2; no locked test evidence |
| Queues / Scheduler | schedule | MATCHED | platform/runtime/keel-worker/src/main.rs | Worker/batches/chains/scheduler exist; exact API parity not established |
| Queues / Scheduler | cron | MATCHED | platform/crates/async/scheduler/src/cron.rs | Zero-dep 5-field parser (`*`, lists, ranges, steps); `ScheduleItem.cron` takes precedence over `spec`, invalid/never-firing expressions refuse registration (tests `cron_items_take_precedence_over_the_spec` + `registration_rejects_invalid_cron_and_unknown_timezones`, parser matrix in `cron.rs`) (Wave 6 P4) |
| Queues / Scheduler | everyMinute | MATCHED | platform/crates/async/scheduler/src/cron.rs (every_minutes) | `every_minutes(1..=59)` → `*/n * * * *`; out-of-range refuses (test `builder_helpers_refuse_bad_input`) (Wave 6 P4) |
| Queues / Scheduler | hourly | MATCHED | platform/crates/async/scheduler/src/cron.rs (hourly) | `hourly()` → `0 * * * *` (test `builder_helpers_map_to_cron_strings`) (Wave 6 P4) |
| Queues / Scheduler | daily | MATCHED | platform/crates/async/scheduler/src/cron.rs (daily/daily_at) | `daily()` → `0 0 * * *`, `daily_at("13:00")` → `0 13 * * *` (bad input refuses) (Wave 6 P4) |
| Queues / Scheduler | weekly | MATCHED | platform/crates/async/scheduler/src/cron.rs (weekly) | `weekly()` → `0 0 * * 0` (Sunday) (Wave 6 P4) |
| Queues / Scheduler | monthly | MATCHED | platform/crates/async/scheduler/src/cron.rs (monthly) | `monthly()` → `0 0 1 * *` (Wave 6 P4) |
| Queues / Scheduler | timezone | MATCHED | platform/crates/foundation/core/src/tz.rs (relocated Wave 8 P2; `keel-scheduler` re-exports `timezone_offset_secs`, the table also feeds the date helpers and `datetime:<tz>` casts) | `ScheduleItem.timezone` (IANA name) validated against the bundled static table — unknown → registration error; zoned daily/weekly/cron `next_run` in the item's wall clock (test `zoned_items_run_in_the_item_wall_clock`); documented gap: fixed offsets, no DST transitions (DST zones at standard time) (Wave 6 P4) |
| Queues / Scheduler | withoutOverlapping | MATCHED | platform/crates/async/scheduler/src/lib.rs (ScheduleLock) | `ScheduleItem.without_overlapping` holds `schedule:overlap:<name>` for `lock_ttl_secs` (default 3600 = max runtime); held → skip silently, failed run releases for retry (tests `overlap_guard_skips_while_the_window_is_held` + `one_server_guard_acquires_runs_and_releases`); runner default in-process lock (D-52), worker wires the Redis-backed port when `REDIS_URL` connects (Wave 6 P4) |
| Queues / Scheduler | onOneServer | MATCHED | platform/crates/async/scheduler/src/lib.rs + platform/runtime/keel-worker/src/main.rs (CacheScheduleLock) | `ScheduleItem.on_one_server` acquires/runs/releases `schedule:one-server:<name>` within the tick (TTL 300s safety valve); cross-node when the worker's `CacheService::lock` port is Redis-backed, process-local otherwise (documented: console daemon cannot reach adapter crates, STAGE-0-SPEC §3) (Wave 6 P4) |

| Filesystem / Storage | put | MATCHED | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | putFile | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | putFileAs | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | get | MATCHED | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | exists | MATCHED | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | missing | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | download | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | url | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | temporaryUrl | STUB | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | size | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | lastModified | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | mimeType | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | path | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | copy | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | move | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | delete | MATCHED | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | deleteDirectory | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | makeDirectory | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | files | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | allFiles | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | directories | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | allDirectories | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | checksum | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Filesystem / Storage | visibility | UNKNOWN | platform/crates/services/storage/src/lib.rs:23 | Local storage is present; S3 is stub; exact operation parity requires adapter tests |
| Auth / Session | login | EQUIVALENT | platform/runtime/keel-api/src/auth_routes.rs (`POST /auth/login`: session cookie or `mode:"token"` PAT, rate-limited per IP+identifier) | Edge login landed (S7); Wave 4 remember-me rides it |
| Auth / Session | logout | EQUIVALENT | platform/runtime/keel-api/src/auth_routes.rs (`POST /auth/logout` invalidates the session) | Edge logout landed (S7) |
| Auth / Session | register | EQUIVALENT | platform/runtime/keel-api/src/auth_routes.rs (`POST /auth/register`, dev/test or `KEEL_ALLOW_REGISTER`) | Edge register landed (S7) |
| Auth / Session | attempt | EQUIVALENT | platform/runtime/keel-api/src/auth_routes.rs (credential verify + access-snapshot install on success) | `Auth::attempt`-equivalent on the edge (S7) |
| Auth / Session | guard | EQUIVALENT | platform/crates/web/http (session middleware guard over `keel_session`) | Guard resolves the session cookie to the caller (S7) |
| Auth / Session | user | EQUIVALENT | platform/runtime/keel-api/src/auth_routes.rs (`/auth/me` resolves the principal + tenant) | Session user resolution on the edge (S7) |
| Auth / Session | id | EQUIVALENT | platform/crates/security/sessions (session id carried as `session_id.hmac_tag`) | (S7) |
| Auth / Session | check | EQUIVALENT | platform/runtime/keel-api/src/auth_routes.rs + session middleware (presence → 200 `/auth/me`, absence → 401/public) | (S7) |
| Auth / Session | guest | EQUIVALENT | platform/crates/web/http session middleware (inverse of check; `guest`-style route gating rides `auth`/public policy) | (S7) |
| Auth / Session | remember | EQUIVALENT | platform/runtime/keel-api/src/auth_routes.rs (`remember: bool` → cookie `Max-Age = remember_ttl` + bag flag; `/auth/me` reports it) | Wave 4: cookie-`Max-Age` only — server-side idle/absolute session bounds unchanged |
| Auth / Session | viaRemember | EQUIVALENT | platform/runtime/keel-api/src/auth_routes.rs (`/auth/me` `remember` flag read from the resolved session bag) | Wave 4: identity still resolves through the single session cookie; the flag reports remembered sessions |
| Auth / Session | validate | EQUIVALENT | platform/crates/security/sessions (HMAC-tag + TTL validation at the edge) | (S7) |
| Auth / Session | authenticate | EQUIVALENT | platform/runtime/keel-api/src/auth_routes.rs (verify credentials + install the policy snapshot) | (S7) |
| Auth / Session | password reset | EQUIVALENT | platform/runtime/keel-api/src/auth_routes.rs (`/auth/password-reset/request|complete`, single-use token) | B-23 |
| Auth / Session | verify email | EQUIVALENT | platform/runtime/keel-api/src/auth_routes.rs (`/auth/email-verification/request|confirm`, `dev_token` in dev/test) | B-23 |
| Auth / Session | session regenerate | EQUIVALENT | platform/crates/security/sessions (regeneration on login); Wave 4 proves `remember` survives it | (S7 + Wave 4) |
| Auth / Session | session invalidate | EQUIVALENT | platform/crates/security/sessions (invalidate on logout) | (S7) |
| Auth / Session | session flash | EQUIVALENT | platform/runtime/keel-api/src/graph_routes.rs (redirect `flash` payload, one-request semantics) | Wave 2 one-request flash through the session bag |
| Auth / Session | session forget | PARTIAL | platform/crates/security/sessions (bag removal exists) | Not pinned as a distinct Laravel-parity API; conservative until proven |
| Auth / Session | token create | EQUIVALENT | platform/crates/security/tokens (hashed 30-day PAT on `mode:"token"` login) | (S7) |
| Auth / Session | token revoke | PARTIAL | platform/crates/security/tokens (purpose-scoped single-use redemption exists) | Logout invalidates the session; a distinct PAT revoke endpoint is not established |
| HTTP Request / Response | json response | MATCHED | platform/runtime/keel-api/tests/http_integration.rs; platform/runtime/keel-api/tests/error_rendering_integration.rs | Envelope end-to-end on the api plane; RFC-7807 `problem+json` opt-in; web-plane clients get error views (Wave 2) |
| HTTP Request / Response | redirect | MATCHED | platform/runtime/keel-api/src/graph_routes.rs (redirect_response unit tests) | `Location` + validated status + one-request flash (Wave 2) |
| HTTP Request / Response | download | MATCHED | platform/runtime/keel-api/src/graph_routes.rs (download_response unit tests) | StoragePort object served as an attachment with sanitized filename + explicit content type (Wave 2) |
| HTTP Request / Response | stream | MATCHED | platform/runtime/keel-api/src/graph_routes.rs (stream_response); platform/runtime/keel-api/tests/response_layer_integration.rs (stream_routes_flush_ndjson_instead_of_an_envelope) | Chunked NDJSON line flush (`Body::from_stream`) instead of one buffered payload (Wave 2) |
| HTTP Request / Response | streamDownload | PARTIAL | platform/runtime/keel-api/src/graph_routes.rs (download_response:1211-1229) | Attachment downloads land, but the object is read fully into memory first - no chunked-download path yet (documented divergence from Laravel streamed download) |
| HTTP Request / Response | file | MATCHED | platform/crates/application/registry/src/lib.rs:89 (`#[serde(alias = "file")]`) | `response = "file"` is an accepted alias for `download` - same StoragePort attachment path (Wave 2) |
| HTTP Request / Response | eventStream | PARTIAL | platform/crates/web/http/src/middleware.rs; platform/runtime/keel-api/src/routes.rs:44-51 | Transport primitives exist but end-to-end edge remains incomplete |
| HTTP Request / Response | cookie set | MISSING | platform/crates/web/http/src/middleware.rs; platform/runtime/keel-api/src/routes.rs:44-51 | Transport primitives exist but end-to-end edge remains incomplete |
| HTTP Request / Response | cookie delete | MISSING | platform/crates/web/http/src/middleware.rs; platform/runtime/keel-api/src/routes.rs:44-51 | Transport primitives exist but end-to-end edge remains incomplete |
| HTTP Request / Response | input | PARTIAL | platform/crates/web/http/src/middleware.rs; platform/runtime/keel-api/src/routes.rs:44-51 | Transport primitives exist but end-to-end edge remains incomplete |
| HTTP Request / Response | query | PARTIAL | platform/crates/web/http/src/middleware.rs; platform/runtime/keel-api/src/routes.rs:44-51 | Transport primitives exist but end-to-end edge remains incomplete |
| HTTP Request / Response | route parameter | PARTIAL | platform/crates/web/http/src/middleware.rs; platform/runtime/keel-api/src/routes.rs:44-51 | Transport primitives exist but end-to-end edge remains incomplete |
| HTTP Request / Response | file input | MISSING | platform/crates/web/http/src/middleware.rs; platform/runtime/keel-api/src/routes.rs:44-51 | Transport primitives exist but end-to-end edge remains incomplete |
| HTTP Request / Response | collect | MISSING | platform/crates/web/http/src/middleware.rs; platform/runtime/keel-api/src/routes.rs:44-51 | Transport primitives exist but end-to-end edge remains incomplete |
| HTTP Request / Response | validate | PARTIAL | platform/crates/web/http/src/middleware.rs; platform/runtime/keel-api/src/routes.rs:44-51 | Transport primitives exist but end-to-end edge remains incomplete |
| HTTP Request / Response | validated | MISSING | platform/crates/web/http/src/middleware.rs; platform/runtime/keel-api/src/routes.rs:44-51 | Transport primitives exist but end-to-end edge remains incomplete |
| Eloquent / ORM | create | MATCHED | platform/crates/data/models/src/runtime.rs (insert/force_create/create_many; tests insert_requires_tenant_and_generates_id, force_create_bypasses_the_mass_assignment_guard) | create() rides the runtime pipeline: mass assignment -> mutators -> casts -> scopes -> store -> observers, tenant-scoped, RETURNING merge (Wave 3 Phase 3) |
| Eloquent / ORM | createMany | MATCHED | platform/crates/data/models/src/runtime.rs (create_many; test create_many_runs_the_pipeline_per_record) | Batch insert runs the full pipeline once per record (Wave 3 Phase 3) |
| Eloquent / ORM | save | MATCHED | platform/crates/data/models/src/runtime.rs (push; test push_saves_self_and_loaded_relations) | Explicit insert-or-update persist of the record + its loaded relations (depth-capped at 8); no implicit save-on-flush (documented divergence) (Wave 3 Phase 3) |
| Eloquent / ORM | update | MATCHED | platform/crates/data/models/src/runtime.rs (update/update_tracked + version_column CAS; tests update_tracked_reports_original_pending_and_stored, cas_stale_write_reports_stale_record) | Dirty-only change set, RETURNING merge, STALE_RECORD -> 409 on version mismatch (Wave 3) |
| Eloquent / ORM | updateOrCreate | MATCHED | platform/crates/data/models/src/runtime.rs (update_or_create; test update_or_create_updates_in_place_then_creates) | Find by predicates, merge attrs in place, else create (Wave 3 Phase 3) |
| Eloquent / ORM | firstOrCreate | MATCHED | platform/crates/data/models/src/runtime.rs (first_or_create; test first_or_create_and_first_or_new) | Find by predicates, else create through the pipeline (Wave 3 Phase 3) |
| Eloquent / ORM | firstOrNew | MATCHED | platform/crates/data/models/src/runtime.rs (first_or_new; test first_or_create_and_first_or_new) | Find else an unsaved record; persistence stays explicit (Wave 3 Phase 3) |
| Eloquent / ORM | forceCreate | MATCHED | platform/crates/data/models/src/runtime.rs (force_create; test force_create_bypasses_the_mass_assignment_guard) | Deliberate mass-assignment guard bypass (Wave 3 Phase 3) |
| Eloquent / ORM | delete | MATCHED | platform/crates/data/models/src/runtime.rs (delete - stamps deleted_at unless .hard_deletes(); tests soft_delete_hides_rows_until_trashed, trashed_reads_refuse_hard_delete_models); pg veil roundtrip in platform/crates/data/postgres/tests/pg_modelstore.rs | Soft delete by default, hard-delete opt-in (Wave 3) |
| Eloquent / ORM | forceDelete | MATCHED | platform/crates/data/models/src/runtime.rs (force_delete; test trashed_reads_refuse_hard_delete_models) | Unconditional removal; store hard-delete returns Ok(None) (Wave 3 Phase 6) |
| Eloquent / ORM | restore | MATCHED | platform/crates/data/models/src/runtime.rs (restore) + store restore roundtrip test | Clears deleted_at through the store port (Wave 3 Phase 6); pg proof pending pg-matrix additions |
| Eloquent / ORM | replicate | MATCHED | platform/crates/data/models/src/runtime.rs (replicate; tests replicate_makes_an_unsaved_clone, replicate_detaches_loaded_relation_payloads) | Clone minus except-fields with a new id, relation payloads detached (Wave 3 Phase 3) |
| Eloquent / ORM | refresh | MATCHED | platform/crates/data/models/src/runtime.rs (refresh; test refresh_and_fresh_rehydrate_from_the_store) | Re-hydrate the same record in place (Wave 3 Phase 3) |
| Eloquent / ORM | fresh | MATCHED | platform/crates/data/models/src/runtime.rs (fresh; test refresh_and_fresh_rehydrate_from_the_store) | Re-fetch as a new record (Wave 3 Phase 3) |
| Eloquent / ORM | with | MATCHED | platform/crates/data/models/src/runtime.rs (with/with_constrained; tests with_loads_a_nested_path_level_by_level, with_constrained_applies_per_level_constraints, with_rejects_an_empty_path) | Nested eager paths with per-level constraints; empty paths refused (Wave 3 Phase 4) |
| Eloquent / ORM | load | MATCHED | platform/crates/data/models/src/runtime.rs (load_relation; test prevent_lazy_loading_flags_the_lazy_path_only) | Lazy eager-load on a hydrated record, fail-closed against unknown relations (Wave 3) |
| Eloquent / ORM | loadMissing | MATCHED | platform/crates/data/models/src/runtime.rs (load_missing; test load_missing_only_loads_unloaded_relations) | Loads only relations not already loaded (Wave 3 Phase 3) |
| Eloquent / ORM | withCount | MATCHED | platform/crates/data/models/src/runtime.rs (with_count/with_sum/with_min/with_max/with_avg; test with_count_and_friends_fold_related_rows) | Aggregate-eager folds related rows into the projection (Wave 3 Phase 4) |
| Eloquent / ORM | whereHas | EQUIVALENT | platform/crates/data/models/src/runtime.rs (has/or_where_has with FieldFilter sub-filters; tests existence_filters_scope_tenant_and_soft_deletes, or_where_has_ors_with_the_preceding_terms) | Constraints ride typed FieldFilter lists instead of query-builder closures (no closure SQL in Rust) - documented divergence (Wave 3) |
| Eloquent / ORM | whereRelation | MATCHED | platform/crates/data/models/src/runtime.rs (where_relation/where_doesnt_have_relation; test where_relation_filters_on_related_columns) | Equality filter on a related column + its negation (Wave 3 Phase 4) |
| Eloquent / ORM | has | MATCHED | platform/crates/data/models/src/runtime.rs (has with operator/count; test has_and_doesnt_have_filter_on_relation_existence) | Count-constrained existence filter, tenant- and veil-aware (Wave 3 Phase 4) |
| Eloquent / ORM | doesntHave | MATCHED | platform/crates/data/models/src/runtime.rs (doesnt_have; test has_and_doesnt_have_filter_on_relation_existence) | Negated existence filter (Wave 3 Phase 4) |
| Eloquent / ORM | withoutGlobalScopes | MATCHED | platform/crates/data/models/src/runtime.rs (without_global_scopes/without_global_scopes_all; test without_global_scopes_all_lifts_the_soft_delete_veil) | Named suppression or full veil-lift per call (Wave 3 Phase 6) |
| Eloquent / ORM | scope | MATCHED | platform/crates/data/models/src/runtime.rs (with_scope + with_global_scope; tests scopes_accessors_and_listing, global_scopes_run_first_and_named_suppression_relaxes_reads) | Named local scopes + globally applied scopes (Wave 3) |
| Eloquent / ORM | makeVisible | MATCHED | platform/crates/data/models/src/runtime.rs (make_visible; tests manifest_hidden_and_visible_shape_the_projection, per_call_overrides_win_over_sensitive_and_manifest_policy) | Projection opt-in wins over manifest hidden (Wave 3 Phase 5) |
| Eloquent / ORM | makeHidden | MATCHED | platform/crates/data/models/src/runtime.rs (make_hidden; test manifest_hidden_and_visible_shape_the_projection) | Projection opt-out; sensitive fields stay redacted in public_record (Wave 3 Phase 5) |
| Eloquent / ORM | append | MATCHED | platform/crates/data/models/src/runtime.rs (append - validates registered accessor names; with_accessor) | Accessor-backed virtual attributes join the wire shape (Wave 3) |
| Eloquent / ORM | withoutTimestamps | MATCHED | platform/crates/data/models/src/runtime.rs (without_timestamps; test without_timestamps_skips_injection_and_touch_becomes_a_noop) | Timestamp injection and touch become no-ops (Wave 3 Phase 6) |
| Eloquent / ORM | observe | MATCHED | platform/crates/data/models/src/runtime.rs (with_observer, ModelPhase events; tests model_events_fire_in_order, cas_stale_write_reports_stale_record) | Observer registry fires create/update/delete phases; CAS version stamping lands there (Wave 3) |

### Exhaustive surface cross-reference ledger

This cross-reference prevents the 24-section report from collapsing Laravel’s large surface into a handful of optimistic labels.

| Area | Laravel capability | Keel status | Evidence / decision | Interpretation |
|---|---|---|---|---|
| Blade / Views | {{ }} | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Auto-escaped echo over dot paths + filters; interpreted expressions replace PHP eval |
| Blade / Views | {!! !!} | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Raw echo - identical semantics |
| Blade / Views | @if | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Conditionals run on the safe expression grammar |
| Blade / Views | @unless | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Inverted conditional, ledger + render facet |
| Blade / Views | @isset | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Presence check over the JSON context |
| Blade / Views | @empty | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Emptiness check, also the @forelse empty branch |
| Blade / Views | @auth | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Auth context from the authenticated principal |
| Blade / Views | @guest | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Inverse of @auth |
| Blade / Views | @env | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Environment gate (e.g. @env("dev")) |
| Blade / Views | @production | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Inverse environment gate |
| Blade / Views | @foreach | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Loop over arrays/objects with loop vars |
| Blade / Views | @for | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Indexed loop, ledger + render |
| Blade / Views | @while | SUPERSEDED | platform/crates/web/views/src/parser.rs:492-496 (targeted refusal) | Refused at parse; @foreach over ranges covers iteration (19-PARITY-PROGRAM policy) |
| Blade / Views | @forelse | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Loop with empty branch; facet test |
| Blade / Views | @include | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Partial render with explicit data |
| Blade / Views | @includeIf | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Conditional partial |
| Blade / Views | @includeWhen | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Context-conditional partial |
| Blade / Views | @extends | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Layout inheritance; layout facet test |
| Blade / Views | @section | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Section content with @endsection/@show |
| Blade / Views | @yield | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Layout placeholder |
| Blade / Views | @show | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Overridable section |
| Blade / Views | @parent | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Parent layout content; layout facet test |
| Blade / Views | @component | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | View-file component with slots |
| Blade / Views | @slot | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Named slots with defaults; component-slot facet test |
| Blade / Views | @props | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Declared props pulled from slot attributes |
| Blade / Views | @aware | MISSING | platform/crates/web/views/src/parser.rs (absent from the closed ledger) | Not in the closed ledger; pass data through @include/@component arguments |
| Blade / Views | @class | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Conditional class attribute helper |
| Blade / Views | @style | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Conditional style attribute helper |
| Blade / Views | @checked | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Form attribute helper |
| Blade / Views | @selected | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Form attribute helper |
| Blade / Views | @disabled | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Form attribute helper |
| Blade / Views | @readonly | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Form attribute helper |
| Blade / Views | @required | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Form attribute helper |
| Blade / Views | @error | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Validation error slot; csrf/method/error/lang facet test |
| Blade / Views | @csrf | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | CSRF field emission; csrf/method/error/lang facet test |
| Blade / Views | @method | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Method-spoof field; csrf/method/error/lang facet test |
| Blade / Views | @verbatim | MISSING | platform/crates/web/views/src/parser.rs (absent from the closed ledger) | Not in the ledger; raw output is {!! !!} |
| Blade / Views | @once | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Render-once scope; stack facet test |
| Blade / Views | @push | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Stack push; stack facet test |
| Blade / Views | @prepend | MISSING | platform/crates/web/views/src/parser.rs (absent from the closed ledger) | Not in the ledger; stacks are @push/@stack only |
| Blade / Views | @stack | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | Stack emission; stack facet test |
| Blade / Views | @session | MISSING | platform/crates/web/views/src/parser.rs (absent from the closed ledger) | Not in the closed ledger |
| Blade / Views | custom directives | EQUIVALENT | platform/crates/web/views/src/engine.rs (register_directive, register_conditional) | Custom directive registry + Blade::if-style conditionals; custom directive/conditional facet test |
| Blade / Views | anonymous components | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | @component("name") resolves a view file with @props/@slot |
| Blade / Views | class components | MISSING | platform/crates/web/views/src/parser.rs (absent from the closed ledger) | No PHP-style component classes; view-file @component with @props covers the data-driven case |
| Blade / Views | dynamic components | MISSING | platform/crates/web/views/src/parser.rs (absent from the closed ledger) | No dynamic component selection in the ledger |
| Blade / Views | component slots | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | @component/@slot/@endslot with defaults; component-slot facet test |
| Blade / Views | component attributes | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | @props receives declared slot attributes; helpers @class/@style/@checked/... |
| Blade / Views | template inheritance | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger) | @extends/@section/@yield/@parent; layout facet test |
| Blade / Views | @lang | EQUIVALENT | platform/crates/web/views/src/parser.rs (is_known ledger); platform/runtime/keel-api/src/i18n.rs | `_lang` flat-map lookup with key fallback, fed by the resolved request locale (directive since Wave 2; boot catalog + resolution landed Wave 6 P3) |
| Blade / Views | trans filter | EQUIVALENT | platform/crates/web/views/src/render.rs (trans_filter) | `{{ "key" \| trans }}` (or an input string as the key) resolves through `_lang`; miss = the key itself. Quoted literals parse via additive `Expr.literal` (serde-default — pre-P3 view-cache ASTs still load). Wave 6 P3 |

| Artisan family | cache:clear | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | cache:forget | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | cache:table | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | config:cache | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | config:clear | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | config:publish | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | config:show | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | db | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | db:monitor | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | db:seed | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | db:show | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | db:table | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | event:cache | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | event:clear | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | event:list | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | migrate | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | migrate:fresh | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | migrate:install | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | migrate:refresh | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | migrate:reset | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | migrate:rollback | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | migrate:status | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | queue:clear | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | queue:failed | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | queue:flush | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | queue:forget | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | queue:listen | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | queue:monitor | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | queue:prune-batches | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | queue:prune-failed | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | queue:restart | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | queue:retry | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | queue:retry-batch | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | queue:work | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | route:cache | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | route:clear | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | route:list | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | schedule:clear-cache | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | schedule:list | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | schedule:run | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | schedule:test | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | schedule:work | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | storage:link | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | view:cache | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | view:clear | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Artisan family | vendor:publish | MAPPED IN §11 | see §11 CLI matrix | Individual runtime parity remains governed by command evidence |
| Laravel 13 domain | HTTP | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Routing | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Middleware | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | CSRF | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Controllers | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Requests | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Responses | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Views | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | URLs | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Sessions | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Validation | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Errors | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Logging | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Artisan | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Broadcasting | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Cache | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Collections | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Concurrency | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Context | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Contracts | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Events | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Filesystem | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Helpers | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | HTTP Client | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Images | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Localization | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Mail | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Notifications | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Packages | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Processes | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Queues | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Rate Limiting | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Search | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Strings | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Scheduling | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Authentication | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Authorization | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Email Verification | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Encryption | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Hashing | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Password Reset | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Database | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Migrations | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Seeding | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Redis | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | MongoDB | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Eloquent | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Factories | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | API Resources | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | JSON:API | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | AI SDK | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | MCP | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |
| Laravel 13 domain | Testing | AUDITED | See §4 and function-level ledgers | No coarse MATCHED status unless child capabilities are complete |

### Decision-rule examples

- **Database = PARTIAL**, because migration infrastructure is strong but ORM CRUD and general module PostgreSQL persistence are missing.
- **Authentication = PARTIAL**, because token/session primitives exist but HTTP login/register/logout/token issuance are not wired through the real edge.
- **Views = UNKNOWN**, because S9 has not yet selected SSR/templates versus SPA-first.
- **Collections = DELIBERATELY EXCLUDED**, because Rust iterators provide a native and type-safe alternative.
- **Concurrency/context = SUPERSEDED**, because Tokio plus typed execution context is the intended Rust-native model.
- **AI/MCP and audit/replay = SUPERSEDED**, because these are deliberate architectural advantages rather than parity gaps.

## 5. Missing Features

### P0 — blockers

| ID | Gap | Why it blocks |
|---|---|---|
| C1 | Graph → HTTP router | Without it, declared domain routes cannot execute over HTTP. |
| C2 | HTTP auth flows | No production login/register/logout/token issuance path. |
| C3 | Domain-module persistence port + PostgreSQL store | Domains fall back to in-memory stores. |
| C4 | ORM runtime | Metadata and Select builder are insufficient for application CRUD. |
| C5 | Multipart uploads | File upload workflows cannot be implemented. |
| C6 | Cookie layer | Session/CSRF/auth browser glue is incomplete. |

### P1 / High

- SSR/template strategy or SPA starter decision.
- `PgSessionStore`.
- Named routes and URL generation.
- Per-principal rate limiting.
- `keel serve`.
- Durable Redis cache/queue drivers.
- pgvector search adapter.
- Breeze-equivalent starter kit.

### P2 / Medium

- Horizon/Telescope-equivalent dashboard over `keel inspect` data.
- Tinker-equivalent interactive shell.
- Complete factory data-generation path.
- Mail HTML templates.
- Redirect and flash primitives for conventional web flows.

### P4 / Low / ecosystem

- Cashier equivalents.
- Socialite equivalents.
- Dusk equivalent.
- Valet equivalent is intentionally not a core target.

### Dependency order

The six blockers are not independent. The minimum causal chain is:

```text
C1 graph → HTTP router
   ↓
C2 HTTP auth + C6 cookies
   ↓
C3 domain Pg store
   ↓
C4 ORM runtime
   ↓
C5 multipart/uploads
```

The roadmap therefore starts with edge wiring, not ORM polish.

## 6. Broken / Partial Features

### Factory generation

`platform/tools/keel-codegen/src/generators.rs:126` contains `todo!("build __TYPE__ from factory fields")`. This is a real implementation hole in a generated developer workflow, not merely missing documentation.

**Status:** `STUB`

### Sessions

`platform/crates/security/sessions/src/lib.rs:197-279` proves in-memory session management, regeneration and sweep. No `PgSessionStore` is supplied. Cross-instance durability and restart survival are therefore unproven.

**Status:** `PARTIAL`

### Appointments domain

`platform/applications/appointments/src/lib.rs:45-46` uses `Mutex<BTreeMap>` storage. The supplied audit facts also establish that `keel-api` has no `keel-mod-appointments` dependency and the graph never reaches the HTTP dispatcher.

**Status:** `INCOMPLETE` / unreachable over HTTP.

### Tracker drift identified before this report

- L05 / routing family: must not imply a runnable route path.
- L06 middleware: stale `✗`; current status is `✓`.
- L21 auth: stale JWT claim; current state is Argon2/OTP + hashed PAT + HMAC sessions, no JWT.
- L61 logging: stale `◐`; initialization/redaction/file mirror are present.
- L62 tracing: stale `◐`; request spans are present.

## 7. Architectural Problems

### 7.1 Hard-coded edge versus graph-driven intent

The architecture is designed around a `PlatformGraph`, but the HTTP router remains a separate hard-coded surface. This creates a split-brain framework: the graph knows what the application is, while HTTP must still be manually wired.

**Evidence:** `platform/crates/application/registry/src/lib.rs`; `platform/runtime/keel-api/src/routes.rs:44-51`.

### 7.2 Modules lack a store-port family

The appointments module falling back to `Mutex<BTreeMap>` is the concrete symptom. A framework cannot claim Laravel-class application persistence while module code must invent local store implementations.

**Evidence:** `platform/applications/appointments/src/lib.rs:45-46`.

### 7.3 `keel-models` is not connected to `keel-query`

Model metadata can describe fields and relations, and `Select` can construct safe reads, but there is no proven model runtime that generates CRUD operations from `ModelDef`/`FieldDef`/`RelationDef`.

**Evidence:** `platform/crates/data/models/src/lib.rs`; `platform/crates/execution/query/src/select.rs`.

### 7.4 Session middleware is missing from the admission pipeline

Admission currently proves request-id, tracing, body limits, CORS, maintenance and rate limiting. Session loading/identity hydration is not part of that chain.

**Evidence:** `platform/crates/web/http/src/middleware.rs`; `platform/crates/security/sessions/src/lib.rs:197-279`.

## 8. Security Findings

| Severity | Finding | Evidence | Impact |
|---|---|---|---|
| Medium | In-memory sessions | `platform/crates/security/sessions/src/lib.rs:197-279` | No cross-instance revocation/durability; horizontal scaling is unsafe. |
| Medium | IP-only rate-limit key | `platform/crates/web/http/src/middleware.rs` | Shared-network users can be over-throttled; principal fairness is weak. |
| Low / Info | Per-boot random session secret | session architecture supplied | Restart can invalidate in-memory sessions. |
| Low / Info | CSRF opt-in (resolved in Wave 7 P7) | `platform/runtime/keel-api/src/graph_routes.rs` | Was: safety depended on deployment configuration. Now default-on for web-plane routes with config-only skip paths; the opt-in env is gone. |
| Low / Info | Upload attack surface not yet present | repo-wide zero multipart hits | No upload surface exists yet, but C5 must include hardening before release. |

### Positive security controls

- SHA-256-hashed single-use tokens: `platform/crates/security/tokens/src/lib.rs:137-214`.
- Argon2 / OTP primitives: `platform/crates/security/identity/src/lib.rs:129-152`.
- AES-GCM sealing: supplied verified audit facts.
- Capability-enforced authorization at the kernel boundary: supplied verified audit facts.
- Double-submit CSRF: `platform/crates/web/http/src/csrf.rs`.
- Logging redaction: `platform/crates/observability/logging/src/logsys.rs:88,219,249`.

### Threat-model conclusions

No SQL-injection claim should be made merely because the typed `Select` builder exists. The absence of runtime CRUD means the write surface is not yet present. Conversely, the future upload surface must be designed before C5 lands: MIME sniffing, content limits, path safety, antivirus/content validation and storage isolation must be explicit.

## 9. Performance Findings

### Proven architectural positives

- `SKIP LOCKED` worker claim loop in `platform/runtime/keel-worker/src/main.rs` is appropriate for competing workers.
- `moka` cache in `platform/crates/services/cache/src/lib.rs` provides a practical in-process cache foundation.
- Typed `Select` builder in `platform/crates/execution/query/src/select.rs` gives a safe path to SQL construction without sacrificing composability.

### What cannot yet be honestly benchmarked

A meaningful router benchmark is **NOT POSSIBLE** because the graph-to-HTTP dispatch path is not connected. Benchmarking a hard-coded route table would measure the wrong architecture.

Metrics and profiling are deferred under D-68. There is therefore no production telemetry layer sufficient to establish p95/p99 request behavior, queue saturation, database latency, cache hit ratios or per-handler allocation profiles.

### Performance risks to watch in S7–S8

- model hydration and allocation costs once ORM relation loading exists;
- clone frequency crossing the graph → request → controller → engine boundary;
- session lookups after introducing PgSessionStore;
- serialization overhead for envelope/response models;
- N+1 relation loading once Eloquent-style behavior is added;
- queue throughput after durable domain stores are introduced.

## 10. Developer Experience Audit

### Where Keel is already stronger

Keel has a distinct operational workflow around **inspect, replay, benchmark, upgrade and security-oriented CLI tooling**. This is materially different from the usual Laravel development loop and should remain a differentiator.

### Where Laravel is still materially better

Laravel has a coherent path from project creation to a working web application. Keel currently breaks that loop at the first endpoint because graph-declared routes are not executable HTTP routes.

### Workflow comparison

| Task | Laravel | Keel today | Verdict |
|---|---|---|---|
| Create application | mature project scaffolding | framework exists but end-user scaffold flow not proven | PARTIAL |
| Run local HTTP app | standard `serve` flow | `keel serve` absent | MISSING |
| Create route | route file + runtime dispatch | manifest/graph declaration exists but dispatch breaks | API-ONLY |
| Create controller | first-class generator/runtime | controller/view-model concept exists | EQUIVALENT at type level |
| Validate request | FormRequest + validator | Validator/Rule primitives | PARTIAL |
| Persist model | Eloquent | metadata + typed Select only | INCOMPLETE |
| Authenticate | full HTTP/session flow | primitives only | PARTIAL |
| Inspect runtime | Telescope/Pulse ecosystem | `keel inspect` / replay model | SUPERSEDED opportunity |
| Replay | limited operational model | delta/audit/replay | SUPERSEDED |
| Benchmark | external tooling and Octane ecosystem | framework-native benchmark intent | SUPERSEDED opportunity |

### DX gate

S7 must make this loop work:

```text
create
→ configure
→ migrate
→ make module
→ route
→ serve
→ request
→ authenticate
→ persist
→ test
```

## 11. CLI Audit

The CLI is one of the most important Laravel parity surfaces because it compresses framework complexity into repeatable developer workflows. The table deliberately expands categories and generators instead of treating “34 commands” as sufficient.

| Laravel command | Keel command | Status | Evidence / gap | Priority |
|---|---|---|---|---|
| about | about | MATCHED | `keel about` golden-fields report: framework/app version, env, debug, DB driver + server version, cache/queue/session drivers, flag snapshot, graph module/route/operation counts, dep-budget state (test `about_report_carries_the_golden_fields`) (Wave 4) | P3 |
| clear-compiled | optimize:clear | MATCHED | `keel optimize:clear` drops all three boot-pinning artifacts (route/config/view), attempting every step and reporting the first error (test `optimize_cycle_warms_and_clears_all_three_artifacts`) (Wave 5) | P3 |
| down | down | EQUIVALENT | Maintenance mode capability | P2 |
| env | env | MATCHED | `keel env` environment summary: resolved config file, redacted key facts, config-cache pin state (test `env_summary...`) (Wave 4) | P3 |
| inspire | — | DELIBERATELY EXCLUDED | Not a framework capability | P4 |
| list | list | MATCHED | CLI command surface exists | P2 |
| optimize | optimize | MATCHED | `keel optimize` warms route:cache → config:cache → view:cache in order, aborting at the first refusal (test `optimize_cycle_warms_and_clears_all_three_artifacts`) (Wave 5) | P2 |
| optimize:clear | optimize:clear | MATCHED | same test: all three artifacts cleared, no partial clear left behind (Wave 5) | P2 |
| up | up | EQUIVALENT | Maintenance disable path | P2 |
| version | version | MATCHED | CLI binary exists | P2 |
| cache:clear | cache:clear | PARTIAL | Cache command surface not fully mapped | P2 |
| cache:forget | cache:forget | MATCHED | `keel cache:forget <key>` removes one key through the CacheService (Wave 4) | P2 |
| cache:table | cache:table | MISSING | DB cache migration not established (deliberate: memory + Redis only, docs/19 §5) | P3 |
| config:cache | config:cache | MATCHED | `keel config:cache` pins the frozen effective config + content hash; boot refuses drift via `config_cache_check` (tests `config_cache_cycle_pins_and_clears_the_artifact` + the `keel-registry::config_cache` units) (Wave 4) | P2 |
| config:clear | config:clear | MATCHED | `keel config:clear` drops the artifact (same test) (Wave 4) | P2 |
| config:publish | config:publish | MATCHED | `keel config:publish <package> [--force]` materializes only the manifest `[config]` to `config/packages/<name>.toml` — the same byte-compared idempotence / conflict-report / `--force` cycle as `keel publish`, refusing packages without `[config]` (Wave 8) | P3 |
| config:show | config:show | MATCHED | `keel config:show <key> [--json]` renders one redacted section (test `config_show_renders_one_redacted_section`) (Wave 4) | P3 |
| db | db | MATCHED | CLI/runtime exists at database layer | P2 |
| db:monitor | db:monitor | UNKNOWN | No locked evidence | P3 |
| db:seed | seed | MATCHED | keel seed | P1 |
| db:show | db:show | MATCHED | `keel db:show` renders driver, server version, database + the `db:inspect` table/row inventory (test `db_show_renders_server_metadata_and_inventory`) (Wave 5) | P3 |
| db:table | db:table | UNKNOWN | No locked evidence | P3 |
| event:cache | event:cache | MISSING | No separate event discovery cache (events are manifest-declared; no runtime discovery to cache) | P3 |
| event:clear | event:clear | MISSING | No event cache command | P3 |
| event:list | event:list | UNKNOWN | Event runtime exists but list command not proven | P3 |
| key:generate | key:generate | UNKNOWN | No locked evidence | P3 |
| make:cast | make cast | MATCHED | platform/tools/keel-codegen/src/generators.rs CAST_TEMPLATE emits `impl Cast for __TYPE__Cast` against the real `keel_models::Cast` trait (tests wave5_adds_twelve_generator_kinds + wave5_templates_reference_real_platform_surface) (Wave 5) | P2 |
| make:channel | make channel | PARTIAL | platform/tools/keel-codegen/src/generators.rs CHANNEL_TEMPLATE points at the real `routes/channels.toml` capability registry (same test); broadcast auth enforcement landed Wave 6 P7 (`broadcast/auth` + presence members + host-side connect validation in `keel-api/src/broadcast_routes.rs`) | P2 |
| make:class | make class | MISSING | platform/tools/keel-codegen/src/generators.rs; no individually verified Laravel-parity generator evidence | P2 |
| make:command | make command | MISSING | platform/tools/keel-codegen/src/generators.rs; no individually verified Laravel-parity generator evidence | P2 |
| make:component | make component | MATCHED | COMPONENT_TEMPLATE renders under the module's real view root with `@props` (test asserts the components/ path + `@props`) (Wave 5) | P2 |
| make:controller | make controller | PARTIAL | platform/tools/keel-codegen/src/generators.rs; no individually verified Laravel-parity generator evidence | P2 |
| make:event | make event | MISSING | platform/tools/keel-codegen/src/generators.rs; no individually verified Laravel-parity generator evidence | P2 |
| make:exception | make exception | MATCHED | EXCEPTION_TEMPLATE emits `impl From<__TYPE__Error> for AppError` (test `wave5_templates_reference_real_platform_surface`) (Wave 5) | P2 |
| make:factory | make factory | PARTIAL | platform/tools/keel-codegen/src/generators.rs (FACTORY_TEMPLATE; test factory_build_is_real_and_deterministic) - real build() with no todo! (stale STUB claim reconciled) | P2 |
| make:interface | make interface | MISSING | platform/tools/keel-codegen/src/generators.rs; no individually verified Laravel-parity generator evidence | P2 |
| make:job | make job | MISSING | platform/tools/keel-codegen/src/generators.rs; no individually verified Laravel-parity generator evidence | P2 |
| make:job-middleware | make job-middleware | MATCHED | JOB_MIDDLEWARE_TEMPLATE emits `impl JobMiddleware for __TYPE__JobMiddleware` with the real `before`/`after` shape against `keel_jobs::{BoxFuture, Control, DurableJob, JobMiddleware}` (needle pinned by `wave5_templates_reference_real_platform_surface`); worker-side composition (`JobMiddlewareStacks`) landed Wave 6 P8 | P2 |
| make:listener | make listener | MISSING | platform/tools/keel-codegen/src/generators.rs; no individually verified Laravel-parity generator evidence | P2 |
| make:mail | make mail | MATCHED | MAIL_TEMPLATE emits a standalone Mailable definition (test asserts it documents its landing); the send/transport surface is real since Wave 6 P1 — `Mailable` + `MailPort::send_mailable` + the `mail.send` queued job with StoragePort-capped attachments | P2 |
| make:middleware | make middleware | PARTIAL | MIDDLEWARE_TEMPLATE emits a real axum `from_fn` layer (`pub async fn __NAME___middleware(request, next)` — test `wave5_templates_reference_real_platform_surface`); the platform admission chain stays framework-owned (middleware.toml aliases/groups, GRAPH-MW-UNKNOWN) and module-local layering is the only consumption path (Wave 5) | P2 |
| make:model | make model | PARTIAL | platform/tools/keel-codegen/src/generators.rs; no individually verified Laravel-parity generator evidence | P2 |
| make:notification | make notification | MATCHED | NOTIFICATION_TEMPLATE emits `impl NotificationDef` — type_key + mail/sms/push/database renderers + `template()` seam, compiled against the real keel-notifications trait (pinned by `wave5_templates_reference_real_platform_surface`); dispatch surface landed Wave 6 P5 (hub `dispatch_notification` + inbox endpoints); producer dispatch landed Wave 6 P6 (appointments Book/Cancel → `AppointmentBooked`/`AppointmentCancelled` Database rows through the ModuleProvider-fetched hub, tests producer_dispatches_the_inbox_row_and_carries_the_locale / cancel_dispatches_the_cancelled_inbox_row / producer_dispatch_failure_fails_the_operation) | P2 |
| make:notifications-table | make notifications-table | PARTIAL | platform/database/migrations/20261001000001_notifications.sql (Wave 6 P5) — the table ships as a framework migration (id, principal_id, tenant_id, type, data, read_at, created_at; pg round trip `pg_notification_store_roundtrip`); no generator kind — the table is framework-owned, apps never regenerate it | P2 |
| make:observer | make observer | MATCHED | OBSERVER_TEMPLATE emits `impl ModelObserver for __TYPE__Observer` against the real `keel_models::ModelObserver` trait (tests wave5_adds_twelve_generator_kinds + wave5_templates_reference_real_platform_surface) (Wave 5) | P2 |
| make:policy | make policy | MISSING | platform/tools/keel-codegen/src/generators.rs; no individually verified Laravel-parity generator evidence | P2 |
| make:provider | make provider | MISSING | platform/tools/keel-codegen/src/generators.rs; no individually verified Laravel-parity generator evidence | P2 |
| make:request | make request | MISSING | platform/tools/keel-codegen/src/generators.rs; no individually verified Laravel-parity generator evidence | P2 |
| make:resource | make resource | PARTIAL | platform/tools/keel-codegen/src/generators.rs (RESOURCE_TEMPLATE emits a compiling `impl keel_http::Resource`; tests resource_template_emits_the_trait_impl + the keel-http doctest build proof, Wave 3); no end-to-end CLI verification yet | P2 |
| make:rule | make rule | MATCHED | RULE_TEMPLATE registers through the real `register_custom_rule("sample_thing", ...)` validation API (test `wave5_templates_reference_real_platform_surface`) (Wave 5) | P2 |
| make:scope | make scope | MATCHED | SCOPE_TEMPLATE emits `pub fn __NAME___scope(filters: Vec<FieldFilter>)` over the real keel-models filter surface (same test) (Wave 5) | P2 |
| make:seeder | make seeder | PARTIAL | platform/tools/keel-codegen/src/generators.rs; no individually verified Laravel-parity generator evidence | P2 |
| make:test | make test | PARTIAL | platform/tools/keel-codegen/src/generators.rs; no individually verified Laravel-parity generator evidence | P2 |
| make:view | make view | MATCHED | VIEW_TEMPLATE lands under the module's real view root (`applications/<m>/resources/views/<name>.html`) and documents `@include` resolution; renders in the Wave 2 views engine (test asserts the path + `@include`) (Wave 5) | P2 |
| migrate | migrate | MATCHED | 6 framework migration files / 20 tables (advisory-lock guarded; pair bookkeeping in `keel_migrations`) | P1 |
| migrate:fresh | migrate:fresh | PARTIAL | `keel migrate:fresh` drop + recreate implemented with the dev/test-only refusal (main.rs `migrate_fresh`, docs/05); no dedicated locked test — the Wave 5 parse lock covers the pairs family, not fresh | P2 |
| migrate:install | migrate:install | MATCHED | `keel migrate:install` creates the `keel_migrations` bookkeeping table without running files (test `migrate_pair_commands_parse_with_documented_defaults`; pg round trip `pair_migrations_apply_rollback_and_reset_with_batches`) (Wave 5) | P3 |
| migrate:refresh | migrate:refresh | MATCHED | `keel migrate:refresh [--step n]` = rollback then re-apply; dev/test-only destructive window (same test) (Wave 5) | P2 |
| migrate:reset | migrate:reset | MATCHED | `keel migrate:reset` reverts every batch; dev/test-only (same test + the pg round trip) (Wave 5) | P2 |
| migrate:rollback | migrate:rollback | MATCHED | `keel migrate:rollback [--step n]` (default 1): batches newest-first, reverse filename order, pair `.down.sql` only — forward-only singles untouched (tests `rollback_order_walks_batches_newest_first_in_reverse_filename_order` + `migrate_pair_commands_parse_with_documented_defaults`; pg-gated `pair_migrations_apply_rollback_and_reset_with_batches`) (Wave 5) | P2 |
| migrate:status | migrate:status | PARTIAL | `keel migrate:status` reports applied vs pending incl. `keel_migrations` pair bookkeeping (main.rs `migrate_status`, docs/05); no dedicated locked test (Wave 5) | P3 |
| model:prune | model:prune | MATCHED | PruneRegistry sweep through ModelRuntime; `--tenant`/`--chunk` flags, chunked passes (test model_prune_cli_parses) (Wave 3) | P3 |
| queue:clear | queue:clear | MATCHED | `keel queue:clear` purges pending jobs; refuses prod/staging + `KEEL_FORCE` confirm (test `queue_admin_commands_parse_with_documented_defaults` + the env gate) (Wave 5) | P3 |
| queue:failed | queue:failed | UNKNOWN | Implemented since Wave 2 (main.rs `queue_failed`); no locked test evidence | P3 |
| queue:flush | queue:flush | MATCHED | `keel queue:flush` deletes every dead job via `prune_dead(flush_cutoff())` (same parse test + jobs `admin_maintenance_purges_prunes_and_snapshots`) (Wave 5) | P3 |
| queue:forget | queue:forget | MATCHED | `keel queue:forget <id>` deletes one dead job; unknown/non-dead id refuses (same parse test) (Wave 5) | P3 |
| queue:listen | queue:listen | MATCHED | supervisor spawning `keel-worker`, respawn on exit with crash-loop backoff (test `queue_admin_commands_parse_with_documented_defaults`) (Wave 5) | P2 |
| queue:monitor | queue:monitor | MATCHED | `keel queue:monitor` renders pending (+ oldest age), running, dead, succeeded (tests `queue_monitor_renders_depth_and_oldest_age` + the parse test) (Wave 5) | P3 |
| queue:prune-batches | queue:prune-batches | MATCHED | `keel queue:prune-batches [--hours n]` (default 72) deletes finished `job_batches` rows strictly before the cutoff — never unfinished ones — and prints the real count (parse test + `batch_commands_validate_before_connecting`; durable ledger B-32, `pg_batch_ledger_roundtrip`) (Wave 5; Wave 6 P9) | P3 |
| queue:prune-failed | queue:prune-failed | MATCHED | `keel queue:prune-failed [--hours n]` (default 168) prunes dead jobs by age (parse test + jobs `admin_maintenance_purges_prunes_and_snapshots`) (Wave 5) | P3 |
| queue:restart | queue:restart | MATCHED | restart-signal file; worker ignores a signal older than its start (jobs test `restart_signal_is_ignored_when_older_than_the_worker_start`) (Wave 5) | P3 |
| queue:retry | queue:retry | UNKNOWN | Implemented since Wave 2 (main.rs `queue_retry`); no locked test evidence | P3 |
| queue:retry-batch | queue:retry-batch | MATCHED | `keel queue:retry-batch <id>` validates the uuid before connecting, then requeues every failed/dead member through `BatchOps::retry` over the durable batch ledger — the same service as `POST /api/v1/batches/{id}/retry` (parse test + `batch_commands_validate_before_connecting`; B-32 done) (Wave 5; Wave 6 P9) | P3 |
| queue:work | queue:work | MATCHED | keel-worker | P1 |
| route:cache | route:cache | MATCHED | keel route:cache writes storage/framework/routes.json (resolved chains/bindings/validators + SHA-256 manifest hash); boot registers from a matching artifact and refuses a stale one (Wave 1) | P2 |
| route:clear | route:clear | MATCHED | keel route:clear deletes the artifact; boot falls back to live compilation (Wave 1) | P2 |
| route:list | route:list | MATCHED | table/json with plane/group/version/deprecation columns; `--name`/`--path`/`--plane` filters (Wave 2) | P2 |
| schedule:clear-cache | schedule:clear-cache | UNKNOWN | Guard locks exist since Wave 6 P4 (`ScheduleLock` — in-process/`CacheService`, TTL-expiring; no schedule plan cache is kept), but no clear command ships — stale lock keys free themselves by TTL | P4 |
| schedule:list | schedule:list | PARTIAL | Information stub over the `schedule_registry()` seam: reports the registry's empty posture — still empty after Wave 6 (P4 delivered cron/guards, not module declarations); implementation main.rs `schedule_info` (Wave 5 documented this posture) | P3 |
| schedule:run | schedule:run | PARTIAL | Same `schedule_info` stub today — the runtime tick exists (schedule:work) but one-shot due runs await module schedules (still empty after Wave 6) | P1 |
| schedule:test | schedule:test | MATCHED | `keel schedule:test` pure dry run (registered vs due-now, nothing fires or writes; tests `schedule_test_plan_is_pure_and_skips_disabled` + `schedule_test_lines_report_the_empty_posture`) (Wave 5) | P3 |
| schedule:work | schedule:work | MATCHED | `keel schedule:work [--every n]` daemon: fires due items as durable jobs, persists state after each pass, exits before the DB on an empty registry (tests `schedule_tick_fires_due_items_and_persists_state` + `schedule_commands_exit_cleanly_when_nothing_is_registered`) (Wave 5) | P1 |
| storage:link | storage:link | MATCHED | `keel storage:link [--force]` creates the public/storage symlink, refuses to overwrite without `--force`, never deletes a real directory (tests `storage_link_creates_a_missing_link`, `storage_link_refuses_to_overwrite_without_force`, `storage_link_never_deletes_a_real_directory`, `storage_link_paths_point_at_the_local_disk_root`) (Wave 5) | P3 |
| schema:dump | db:schema:dump | MATCHED | `keel db:schema:dump` writes a normalized, sorted DDL snapshot to `database/schema.sql` (empty schema → honest marker; tests `schema_dump_renders_normalized_sorted_ddl` + `schema_dump_renders_an_empty_marker_for_an_empty_schema`) (Wave 5; added to the inventory here — Laravel gained `schema:dump` in 10.x) | P3 |
| test | test | MATCHED | `keel test [--pg]` runs `cargo test --workspace --locked`, tallies `test result:` lines into PASS/FAIL; `--pg` adds `pg-tests` and fails closed without `DATABASE_URL` (tests `test_result_lines_parse_pass_and_fail` + `storage_link_and_test_commands_parse`) (Wave 5; inventory row added) | P2 |
| view:cache | view:cache | MATCHED | precompiles every template under `[views].paths` into `[views].cache_dir` (mtime + content hash) (Wave 2) | P3 |
| view:clear | view:clear | MATCHED | drops the in-process memo plus every `*.json` artifact; boot recompiles on demand (Wave 2) | P3 |
| vendor:publish | vendor:publish | MISSING | Package resource publish absent | P3 |
| horizon | inspect/queue + /_dashboard/ops | EQUIVALENT (read-only) | Web ops dashboard `GET /_dashboard/ops` (queue depth, recent jobs, schedule state, execution inspect; ADR 0009 Accepted 2026-10-02) plus `queue:monitor`/`keel inspect`; worker start/pause mutations stay out of scope (console commands) | P3 |
| telescope | inspect/replay | SUPERSEDED | Keel inspect/replay model is broader | P2 |
| tinker | shell | MATCHED | `keel shell` REPL (v1.1: login/whoami/ops/routes/run under the caller principal, capability denial enforced; Wave 4 adds `caps` + `models`) | P3 |
| serve | serve | MATCHED | `keel serve [--worker]` one-command local loop (S7 B-08; `tools/keel-console`) | P1 |

### Current verified command maturity

The repository brief states **34/34 planned Keel commands are done (L55 ✓)**. This proves planning completion, not full Laravel command parity. The gaps above are the difference between a closed internal roadmap and a Laravel-equivalent CLI surface. Wave 5 added 20+ commands beyond that count (the migrate family, db:show/schema:dump, the queue admin set, schedule:work/test, storage:link, optimize pair, `keel test`, `keel call`); the rows above carry their locked evidence (or honest UNKNOWN/PARTIAL) individually.

## 12. ORM Audit

The ORM is the largest structural parity gap after the HTTP edge.

### What exists

`platform/crates/data/models/src/lib.rs` provides:

- `FieldType`
- `FieldDef`
- `RelationDef`
- `ModelRegistry`
- `validate_graph`

`platform/crates/execution/query/src/select.rs` provides:

- typed `Select` builder
- joins
- operators
- `tenant_scope`
- identifier safety

That is **metadata + typed read composition**, not an Eloquent-class ORM runtime.

### Function-level matrix

| Capability | Status | Evidence | Gap | Priority |
|---|---|---|---|---|
| Model definition | EQUIVALENT | platform/crates/data/models/src/lib.rs | Compile-time metadata exists | P1 |
| Create/save | MATCHED | platform/crates/data/models/src/runtime.rs (insert + push = insert-or-update with relations; test push_saves_self_and_loaded_relations) | Explicit persist through the runtime pipeline (Wave 3) | P0 |
| Insert many | MATCHED | platform/crates/data/models/src/runtime.rs (create_many; test create_many_runs_the_pipeline_per_record) | Batch insert runs mutators/casts/observers once per record (Wave 3) | P1 |
| Update | MATCHED | platform/crates/data/models/src/runtime.rs (update/update_tracked + version CAS; tests update_tracked_reports_original_pending_and_stored, cas_stale_write_reports_stale_record) | Dirty-only change set with RETURNING merge (Wave 3) | P0 |
| Delete | MATCHED | platform/crates/data/models/src/runtime.rs (delete/force_delete; pg veil roundtrip in platform/crates/data/postgres/tests/pg_modelstore.rs) | Soft delete by default, hard-delete opt-in; pg-proven (Wave 3) | P0 |
| Upsert | MATCHED | platform/crates/data/models/src/runtime.rs (upsert/update_or_insert); pg test pg_modelstore_upsert_merges_on_conflict | Conflict-merge upsert through the store port (Wave 3) | P1 |
| Find one | MATCHED | platform/crates/data/models/src/runtime.rs (find/get/first/first_or_not_found; test find_keeps_ok_none_while_error_finders_raise_not_found) | Model runtime with typed not-found (Wave 3) | P1 |
| Eager loading | MATCHED | platform/crates/data/models/src/runtime.rs (with/with_constrained; tests with_loads_a_nested_path_level_by_level, has_many_eager_load, belongs_to_eager_load) | Nested eager paths with per-level constraints (Wave 3) | P0 |
| Lazy loading | MATCHED | platform/crates/data/models/src/runtime.rs (load_relation/load_missing; test load_missing_only_loads_unloaded_relations) | On-demand relation fetch on hydrated records (Wave 3) | P1 |
| Lazy-loading prevention | MATCHED | platform/crates/data/models/src/runtime.rs (prevent_lazy_loading; test prevent_lazy_loading_flags_the_lazy_path_only) | Strict-mode guard flags the lazy path (Wave 3) | P2 |
| Relations | MATCHED | platform/crates/data/models/src/runtime.rs (belongs_to/has_many/many-through + aggregate eager; test many_through_eager_load_and_aggregates) | Relation execution complete: eager + lazy + aggregates (Wave 3) | P0 |
| Many-to-many / pivot | MATCHED | platform/crates/data/models/src/runtime.rs (attach/detach/sync/toggle/update_existing_pivot; test pivot_sync_detach_toggle_and_update) | Runtime pivot abstraction with sync idempotence (Wave 3) | P2 |
| Polymorphic relations | MATCHED | platform/crates/data/models/src/runtime.rs (morph_to/morph_many/morph_to_many; tests morph_to_loads_each_type_through_the_morph_map, morph_many_pins_owner_type_on_load_and_exists) | Morph-map resolution at load and existence (Wave 3) | P3 |
| Casts | MATCHED | platform/crates/data/models/src/casts.rs + runtime `with_cast` registry (tests cast_kinds_validate_and_normalize_at_set_time, encrypted_cast_seals_through_the_runtime_sealer, hashed_cast_hashes_with_argon2_and_never_round_trips) | Builtin + custom casts validated at set time (Wave 3) | P1 |
| Accessors | MATCHED | platform/crates/data/models/src/runtime.rs (with_accessor/append; test scopes_accessors_and_listing) | Computed attributes join the wire shape (Wave 3) | P1 |
| Mutators | MATCHED | platform/crates/data/models/src/runtime.rs (with_mutator; test mutator_transforms_wire_values) | Setter transformation runs before persist (Wave 3) | P1 |
| Fillable / mass assignment | MATCHED | platform/crates/data/models/src/runtime.rs (fill guard; test force_create_bypasses_the_mass_assignment_guard) | Guarded fill with a deliberate force_create bypass (Wave 3) | P1 |
| Hidden / visible serialization | MATCHED | platform/crates/data/models/src/runtime.rs (wire/make_visible/make_hidden/public_record; tests manifest_hidden_and_visible_shape_the_projection, sensitive_fields_redacted_in_public_record) | Serializer policy with sensitive-field redaction (Wave 3) | P2 |
| Scopes | MATCHED | platform/crates/data/models/src/runtime.rs (with_scope/with_global_scope/without_global_scopes; tests scopes_accessors_and_listing, global_scopes_run_first_and_named_suppression_relaxes_reads) | Local + global scope runtime (Wave 3) | P1 |
| Soft deletes | MATCHED | platform/crates/data/models/src/runtime.rs (find_trashed/with_trashed/only_trashed/restore/force_delete; test soft_delete_trio_round_trips) plus route-binding `with_trashed` | Veil/trash/restore/force-delete runtime (Wave 3) | P2 |
| Model events | MATCHED | platform/crates/data/models/src/runtime.rs (ModelPhase events; test model_events_fire_in_order) | Lifecycle events fire in declared order (Wave 3) | P2 |
| Observers | MATCHED | platform/crates/data/models/src/runtime.rs (with_observer; tests model_events_fire_in_order, cas_stale_write_reports_stale_record) | Observer registry wired into CAS version stamping + dirty tracking (Wave 3) | P2 |
| Factories | MATCHED | platform/tools/keel-codegen/src/generators.rs (FACTORY_TEMPLATE; tests factory_build_is_real_and_deterministic, generated_code_contains_no_placeholder_macros) | Real build() with no todo! - stale claim reconciled (Wave 3) | P2 |
| Seeders | MATCHED | keel seed | Seed command exists | P1 |
| Pagination integration | MATCHED | platform/crates/web/http/src/pagination.rs; platform/runtime/keel-api/src/graph_routes.rs | Wired to HTTP (Wave 1): GET routes parse page/per_page, inject _page, clamp per_page to MAX_PER_PAGE 100; list handlers emit a page meta block (page/per_page/total/has_more) | P1 |
| N+1 detection | UNKNOWN | No locked evidence | Eager loading removes the common N+1, but instrumentation-level detection still does not exist | P3 |
| Transactions | EQUIVALENT | platform/crates/application/registry/src/lib.rs; data runtime facts | Command engine supports idempotent execution, but domain DB transaction semantics need proof | P1 |
| Optimistic locking | MATCHED | platform/crates/data/models/src/runtime.rs (version_column CAS -> STALE_RECORD -> 409; tests cas_stale_write_reports_stale_record, cas_refuses_a_row_without_an_integer_version, version_column_seeds_one_and_stamps_every_update) | Integer version column with stale-write rejection (Wave 3) | P3 |
| Model serialization | MATCHED | platform/crates/data/models/src/runtime.rs (wire/append + visibility overrides; tests per_call_overrides_win_over_sensitive_and_manifest_policy, mutator_transforms_wire_values) | Eloquent-equivalent projection control (Wave 3) | P2 |

### Model → query generation gap

The highest-leverage ORM design decision is to connect `ModelDef`/`FieldDef`/`RelationDef` to a typed CRUD/query generation layer instead of building a second untyped ORM on top of `sqlx`. The intended bridge should be additive and compile-time aware.

## 13. Package Ecosystem Audit

Keel already has a useful port/adapter story: **7 adapters** ship and manifests/compile-time modules are already part of the architecture. That is the foundation for a package ecosystem, but not yet the ecosystem itself.

| Capability | Status | Evidence | Gap |
|---|---|---|---|
| Package metadata | EQUIVALENT | module manifests | Need stable published package contract |
| Service registration | EQUIVALENT | lifecycle/provider architecture | Need public third-party registration contract |
| Configuration publishing | MISSING | no vendor-publish equivalent | Needed for package UX |
| Routes | PARTIAL | route declarations in manifests | Runtime router integration is still blocked |
| Migrations | EQUIVALENT | migration system | Package-owned migrations need lifecycle contract |
| Views/assets | UNKNOWN | S9 web decision pending | Package view contract cannot be frozen yet |
| CLI commands | PARTIAL | 34 planned commands | Package command registration needs public extension API |
| Auto-discovery | MISSING | no registry/discovery evidence | Install-time discovery absent |
| Extension API | PARTIAL | D-69 | Needs stable third-party extension contract |
| Version compatibility | UNKNOWN | no locked evidence | Requires framework version/feature negotiation |
| Marketplace/registry | DELIBERATELY EXCLUDED for v1 | dependency cap and scope | External registry can follow later |

### Strategic decision

Do not build a package marketplace before the runtime extension contract is stable. The correct order is: **module contract → extension API → discovery → package tooling → optional registry**.

## 14. Testing Audit

### Current evidence

- **336 static `#[test]` attributes**.
- **334 tests in the last executed green gate**.
- Architecture gates exist in `platform/tests/architecture/tests/architecture.rs`: G07 dependency matrix, topology, graph compile.
- TestKit and `assert_queries!` exist as framework test primitives.

### Critical gap

There are **zero PostgreSQL-backed tests** in the supplied repository facts; specifically, zero `sqlx::test` references. That is a **High risk** for every store that is intended to be production-backed.

### Test pyramid assessment

| Layer | Status | Finding |
|---|---|---|
| Unit | MATCHED | 336 static test attributes. |
| Architecture tests | MATCHED | G07/topology/graph gates. |
| HTTP integration | INCOMPLETE | Edge routing is not executable end-to-end. |
| PostgreSQL integration | MISSING | No real Pg-backed matrix. |
| Queue integration | PARTIAL | Worker exists; store breadth needs more real-backend tests. |
| Auth integration | PARTIAL | Primitives exist; HTTP wiring is missing. |
| Upload/security tests | MISSING | Multipart surface does not exist yet. |
| Browser/E2E | UNKNOWN | No locked evidence. |
| Load/perf | UNKNOWN | Router benchmark cannot yet be meaningful. |
| Fuzz/property | UNKNOWN | No locked evidence. |

### Release implication

A framework can have hundreds of green unit tests and still be unsafe to release when the only durable store lacks integration coverage. PostgreSQL-backed tests must be a v0.9 gate.

## 15. Observability Audit

| Capability | Status | Evidence |
|---|---|---|
| Logging init | MATCHED | `platform/crates/observability/logging/src/logsys.rs:88,219,249` |
| Redaction | MATCHED | same |
| File mirror | MATCHED | same |
| Request spans | MATCHED | same |
| Structured trace store | SUPERSEDED | supplied delta/audit/replay design |
| `keel inspect` | SUPERSEDED | supplied CLI facts |
| `keel replay` | SUPERSEDED | supplied CLI facts |
| Metrics endpoint | MISSING | D-68 |
| Profiling | DEFERRED / not a core status | D-68; no production profile evidence |
| Queue metrics | UNKNOWN | no locked evidence |
| DB metrics | UNKNOWN | no locked evidence |
| Health | EQUIVALENT | `keel-api` meta/health surface implied by runtime facts |

### Why inspect/replay is a real differentiator

Traditional application frameworks typically stop at request logs and tracing. Keel's delta/audit/replay model is closer to a forensic execution record. This should be protected as a **SUPERSEDED** capability, not flattened into a “Telescope clone.”

## 16. Competitive Analysis

| Framework | Strength relative to Keel today | Keel advantage | Brutal gap |
|---|---|---|---|
| Laravel 13 | Mature end-to-end application platform, enormous ecosystem | Typed Rust core, audit/replay, capability authz, MCP-native design | HTTP edge, ORM, sessions, templates, ecosystem maturity |
| Loco | Closest full Rust application rival; strong auth/scaffolding/SSR | Keel's execution engine, capability model, observability/replay | Loco has the more coherent app-development loop today |
| Axum | Excellent minimal async HTTP foundation | Batteries-included platform layer | Keel currently inherits Axum-class edge constraints until graph wiring is complete |
| Actix Web | High-performance mature Rust HTTP | More opinionated application platform | Less end-to-end ecosystem breadth than Actix/Laravel ecosystem |
| Poem | Flexible Rust framework | Deeper application engine | Ecosystem maturity |
| Rocket | Strong ergonomic Rust web story | PlatformGraph + engine + capability model | Full-stack app tooling maturity |
| Rails | Extremely coherent developer experience | Rust safety/performance | Productive web conventions and ecosystem |
| Django | Batteries-included Python platform | Typed/compiled runtime and systems performance | ORM/admin/ecosystem maturity |
| Phoenix | Realtime excellence | Capability model and enterprise execution engine | Realtime/productivity ecosystem maturity |
| NestJS | Excellent modular enterprise TS framework | Rust runtime/type safety | Package ecosystem and ready-made web integrations |
| Spring Boot | Enterprise maturity and huge ecosystem | Lower-level efficiency and typed capabilities | Enterprise integrations, tooling, ecosystem |
| ASP.NET Core | Mature typed full-stack platform | Smaller runtime footprint + explicit capability engine | Ecosystem and application surface breadth |

### Laravel ecosystem product matrix

| Laravel product | Keel strategic status | Evidence | Gap / interpretation | Priority |
|---|---|---|---|---|
| Cashier (Stripe) | DELIBERATELY EXCLUDED | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Cashier (Paddle) | DELIBERATELY EXCLUDED | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Dusk | DELIBERATELY EXCLUDED | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Envoy | UNKNOWN / ECOSYSTEM | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Fortify | EQUIVALENT / FUTURE | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Folio | UNKNOWN / ECOSYSTEM | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Herd | UNKNOWN / ECOSYSTEM | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Head | UNKNOWN / ECOSYSTEM | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Horizon | EQUIVALENT (decision gate passed review 2026-10-02) | `specs/ADR/0009` Accepted — read-only `GET /_dashboard/ops` over the existing job/scheduler/trace stores: queue depth, recent jobs, schedule state, execution inspect; flag-gated (`app.dashboard`), views-rendered (platform-shipped template, app override) with JSON degradation, payload/input never rendered; 7 tests (3 `dashboard.rs` units + 4 `http_integration`) | Implemented as the ADR 0009 ops dashboard (docs/19 Wave 8 decision gates); historical metrics charts stay on the `/metrics` scrape surface and mutating actions stay on the console commands | P2/P4 |
| Octane | SUPERSEDED | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Passport | UNKNOWN / ECOSYSTEM | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Pennant | UNKNOWN / ECOSYSTEM | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Pint | UNKNOWN / ECOSYSTEM | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Precognition | EQUIVALENT (decision gate passed review 2026-10-02) | `specs/ADR/0008` Accepted — validate-only `Precognition` header on graph routes: `[[validation]]` only (204 pass / byte-identical 422 failure), multipart refused pre-intake, `401`/`403` unchanged; 6 `http_integration` tests | Implemented as the ADR 0008 dispatch spec (docs/19 Wave 8 decision gates); previewing dispatch results stays out of scope (Laravel parity: validation outcome only) | P2/P4 |
| Prompts | UNKNOWN / ECOSYSTEM | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Pulse | UNKNOWN / ECOSYSTEM | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Reverb | EQUIVALENT / FUTURE | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Sail | DELIBERATELY EXCLUDED | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Sanctum | EQUIVALENT / FUTURE | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Scout | EQUIVALENT / FUTURE | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Socialite | DELIBERATELY EXCLUDED | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Telescope | SUPERSEDED | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Valet | DELIBERATELY EXCLUDED | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Breeze | EQUIVALENT / FUTURE | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Jetstream | UNKNOWN / ECOSYSTEM | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Inertia | UNKNOWN / ECOSYSTEM | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |
| Livewire | UNKNOWN / ECOSYSTEM | No standalone Keel product mapping is locked beyond the stated audit facts | Treat as ecosystem decisions; do not inflate core parity | P2/P4 |

### Security API ledger

| Capability | Status | Evidence | Gap |
|---|---|---|---|
| encrypt | PARTIAL | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| decrypt | PARTIAL | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| hash | EQUIVALENT | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| check | EQUIVALENT | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| needsRehash | PARTIAL | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| randomBytes | PARTIAL | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| randomInt | PARTIAL | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| verifySignature | PARTIAL | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| hmac | PARTIAL | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| password reset token | PARTIAL | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| email verification token | PARTIAL | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| signed URL | PARTIAL | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| temporary signed URL | PARTIAL | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| csrf token | EQUIVALENT | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| session id regenerate | PARTIAL | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| session invalidate | PARTIAL | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| authorization gate | SUPERSEDED | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| policy check | SUPERSEDED | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| capability check | SUPERSEDED | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| tenant isolation | EQUIVALENT | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| secret redaction | EQUIVALENT | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| request ID | EQUIVALENT | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |
| trace ID | EQUIVALENT | platform/crates/security/identity/src/lib.rs:129-152; platform/crates/security/tokens/src/lib.rs:137-214; platform/crates/web/http/src/csrf.rs; platform/crates/enterprise/tenancy/src/lib.rs:143,267; platform/crates/observability/logging/src/logsys.rs:88,219,249 | Exact runtime behavior must be covered by security tests |

### Verdict

The realistic Rust competitor today is **Loco**, not Laravel. Laravel remains the application-platform benchmark. Keel should not claim superiority until S7 proves a real application path and S8 proves real domain persistence.

## 17. Rust-Native Opportunities

The framework should not clone PHP history where Rust gives a better primitive.

### Deliberate advantages

1. **Compile-time architecture gates** — topology and dependency invariants can fail before runtime. Evidence: `platform/tests/architecture/tests/architecture.rs`.
2. **Typed envelopes and errors** — structured result propagation can replace stringly exception conventions.
3. **Delta/audit/replay** — execution can become a first-class forensic artifact.
4. **Capability authorization** — capabilities can be enforced at the engine boundary rather than distributed across controllers.
5. **MCP/AI-native interfaces** — framework capabilities can be exposed through machine-readable contracts.
6. **Zero-cost typed middleware** — use Rust's type system instead of dynamic middleware reflection.
7. **Typed concurrency/context** — Tokio + `ExecutionContext` instead of callback-style concurrency.
8. **Compile-time query safety** — typed `Select`/identifier safety can eliminate classes of malformed query construction.

### What not to copy

- facades/global helpers;
- PHP-style magic property access;
- runtime reflection when procedural macros can provide compile-time contracts;
- a generic Collection wrapper where iterators are stronger;
- dynamic service resolution where static wiring gives better diagnostics.

## 18. Architecture Redesign Recommendations

All recommendations are **additive**. No rewrite is justified by the current evidence.

### R1 — Graph-driven edge router

Add an adapter in `keel-api` that consumes `PlatformGraph` route specs and produces executable HTTP routes. Keep `routes.rs` as the transport-specific edge implementation, but remove the duplicate source of truth.

### R2 — Module store-port family

Introduce a consistent module-facing store contract so a domain module can declare persistence capabilities once and bind a memory/test or PostgreSQL implementation behind the same port. This removes `Mutex<BTreeMap>` as a permanent architectural pattern.

### R3 — Model metadata → query generation

Connect `platform/crates/data/models/src/lib.rs` metadata to `platform/crates/execution/query/src/select.rs` and future typed write builders. Use compile-time generation for CRUD and relation-loading shapes.

### R4 — Session middleware + PostgreSQL store

Insert session loading into the admission/request context chain and add `PgSessionStore`. Do not ship distributed authentication without durable session state.

### R5 — Optional JWT add-on

JWT should remain non-default. The current hashed PAT + HMAC session model is intentionally simpler and more revocable. JWT can be added later as an `EQUIVALENT` option where stateless integration is required.

## 19. Prioritized Backlog

| ID | Domain | Feature | Status | Priority | Evidence | Dependencies | Complexity | Risk | Reason | Acceptance Criteria |
|---|---|---|---|---|---|---|---|---|---|---|
| B-01 | Edge | C1 graph → HTTP router | API-ONLY | P0 | routes.rs:44-51; registry RouteSpec | None | High | Critical | Remove graph/HTTP split | Declared module route becomes executable HTTP endpoint. |
| B-02 | Auth | C2 HTTP auth flows | PARTIAL | P0 | security/tokens; identity | B-01 | High | Critical | Expose login/register/logout/token issuance | Authenticated request can traverse HTTP pipeline. |
| B-03 | Sessions | C6 cookie layer + PgSessionStore | PARTIAL | P0 | security/sessions | B-02 | High | High | Make browser session durable | Restart + multi-instance preserve valid sessions. |
| B-04 | Persistence | C3 module store ports + Pg stores | INCOMPLETE | P0 | appointments BTreeMap; postgres stores | B-01 | High | Critical | Standardize domain persistence ports | Appointments persists to Pg without bespoke store code. |
| B-05 | ORM | C4 CRUD runtime | DONE (Wave 3: ModelRuntime finders/mutations + ModelStore port growth - create/update/delete/upsert proven in the runtime/store suites and the pg CRUD roundtrip; force-create/push/typed not-found) | P0 | models + select | B-04 | Very High | Critical | Add typed write path | Create/update/delete/upsert integration tests pass. |
| B-06 | Uploads | C5 multipart | MISSING | P0 | zero multipart hits | B-01,B-03 | High | High | Add bounded multipart/file pipeline | Secure upload test matrix passes. |
| B-07 | Routing | Named routes + URL generation | MISSING | P1 | registry RouteSpec lacks name | B-01 | Medium | Medium | Add named route metadata | URL helper resolves named route parameters. |
| B-08 | DX | keel serve | MISSING | P1 | CLI gap | B-01 | Medium | Medium | Provide local app loop | Developer runs real domain over HTTP with one command. |
| B-09 | Rate limiting | Per-principal rate limits | DONE (S7 bearer-hash keying; Wave 1 named throttle:<group> token buckets keyed x-rate-key > bearer hash > client IP, per-group capacity/refill in middleware.toml, 429 + Retry-After) | P1 | http middleware client_key + middleware_catalog throttle stage | B-02 | Medium | Medium | Use principal/tenant keying | Authenticated users receive isolated quotas. |
| B-10 | ORM | Relations/eager loading | DONE (Wave 3: eager + lazy + nested `with` with constraints, aggregate eager, pivots, polymorphic - tests with_loads_a_nested_path_level_by_level, pivot_sync_detach_toggle_and_update, morph_to_loads_each_type_through_the_morph_map) | P1 | models RelationDef | B-05 | High | High | Compile relation loaders | N+1-safe eager-load examples pass. |
| B-11 | ORM | Casts/accessors/mutators/scopes | DONE (Wave 3: cast registry incl. encrypted/hashed fail-closed, accessors/mutators/`append`, local + global scopes, visibility + strict-mode flags) | P1 | models metadata | B-05 | High | High | Add model runtime semantics | Typed model behavior matches documented contract. |
| B-12 | Cache | Redis driver | MISSING | P2 | cache service | None | Medium | Medium | Add distributed cache adapter | Cache tests pass against Redis. |
| B-13 | Queues | Redis queue driver | MISSING | P2 | worker runtime | None | Medium | Medium | Add durable Redis queue | Worker integration passes. |
| B-14 | Search | pgvector adapter | MISSING | P2 | services/search | B-04 | Medium | Medium | Add Pg vector search port | Vector query integration passes. |
| B-15 | Web | SSR/SPA strategy + starter | UNKNOWN | P2 | S9 decision | B-01 | High | High | Freeze web UX architecture | One supported full-stack application path documented. |
| B-16 | CLI | Tinker-equivalent shell | MISSING | P3 | CLI surface | B-05 | Medium | Low | Provide typed interactive console | Developer can inspect/query real app state safely. |
| B-17 | Observability | Metrics endpoint | MISSING | P1 | D-68 | B-01 | Medium | High | Expose metrics and health | Prometheus-compatible endpoint and tests. |
| B-18 | Packages | Extension registry/discovery | PARTIAL | P2 | D-69 | B-01 | High | Medium | Stabilize third-party module contract | Third-party package can register routes/commands/config. |
| B-19 | Testing | PostgreSQL integration matrix | MISSING | P0 | zero sqlx::test | B-04,B-05 | High | Critical | Prove real persistence | Pg-backed CI matrix is green. |
| B-20 | Ecosystem | Breeze-equivalent starter + dashboard | MISSING | P2 | starter/dashboard gap | B-15,B-17 | High | Medium | Reduce adoption friction | New app can bootstrap auth/admin/observability path. |

## 20. Dependency-Aware Roadmap

### S7 — Edge Wiring

Scope:

- graph-driven HTTP router;
- HTTP authentication flow;
- cookie layer;
- durable Pg sessions;
- multipart uploads;
- real domain module reachable over HTTP.

**Exit criteria:** a real domain module is served over HTTP, authenticated, session-backed, and able to process a bounded request lifecycle without hand-wired graph bypasses.

### S8 — Data Runtime

Scope:

- ORM CRUD;
- relation loading;
- casts;
- accessors/mutators;
- scopes;
- module PostgreSQL stores;
- transaction/runtime tests.

**Exit criteria:** a real domain module can create/read/update/delete durable PostgreSQL data through the framework's model/persistence contract.

### S9 — Web Story

Scope:

- decide SSR/template engine vs SPA-first vs dual-mode;
- named routes and URL generation;
- redirects;
- flash state;
- starter web application contract.

**Exit criteria:** a real user-facing web workflow is buildable without framework-specific escape hatches.

### S10 — Ecosystem

Scope:

- Redis cache/queue;
- pgvector;
- package discovery/registry;
- starter kit;
- dashboard over inspect/replay;
- extension API hardening.

**Exit criteria:** third-party developers can install/extend Keel without modifying framework internals.

## 21. Definition of Done

| Release | Definition of Done |
|---|---|
| v0.1 | S7 exit: one real module served over HTTP with working authentication and durable sessions. |
| v0.2 | S8 exit: durable PostgreSQL domain persistence with CRUD, relations and core model semantics. |
| v0.5 | S9 + core S10: coherent web story, starter path, core ecosystem contracts. |
| v0.9 | PostgreSQL-backed integration matrix, metrics endpoint, security hardening, deployment gates. |
| v1.0 | §22 parity gate green, documentation complete, benchmark suite documented, upgrade/migration story established. |
| v2.0 | WASM plugins and multi-database support, subject to a new scope/dependency decision. |

### Release acceptance dimensions

| Dimension | Required evidence before v1.0 |
|---|---|
| HTTP | real route from graph to handler and response |
| Auth | login/logout/session/token flow over HTTP |
| Persistence | real Pg CRUD and domain store integration |
| ORM | CRUD + relations + casts/scopes + serialization |
| Uploads | multipart limits, validation and storage tests |
| CLI | command matrix reconciled with documented framework surface |
| Packages | third-party module can register supported resources |
| Observability | logs + traces + metrics with redaction |
| Security | attack-oriented tests for auth/session/CSRF/upload/tenant isolation |
| Documentation | examples execute against actual APIs |

### v1.0 non-negotiables

No feature can be declared complete if it is only a trait, macro, metadata object, or happy-path example. Runtime behavior plus integration evidence is required.

## 22. Laravel Parity Gate

This table assigns the final strategic decision, not merely today's runtime status. `EQUIVALENT` does not mean “already implemented”; it may mean the selected destination architecture is an intentional equivalent. Runtime readiness remains governed by §§4–6.

| Capability | Parity decision |
|---|---|
| Application bootstrap | EQUIVALENT |
| Container | EQUIVALENT |
| Providers | EQUIVALENT |
| Facades/global helpers | DELIBERATELY EXCLUDED |
| Routing core | EQUIVALENT |
| Named routes | EQUIVALENT (landed: resolve_url + [url] defaults + route:list --name) |
| Middleware | EQUIVALENT |
| CSRF | EQUIVALENT (web-plane double-submit default-on + `[security.csrf]` skip paths landed Wave 7 P7) |
| Controllers | EQUIVALENT |
| Requests/FormRequest | EQUIVALENT (landed: manifest `[[validation]]` at the edge since Wave 1; Wave 4 rule battery) |
| Responses | MATCHED (Wave 2 response kinds at the edge) |
| Views | EQUIVALENT (Wave 2 template engine) |
| Blade | EQUIVALENT / SUPERSEDED (Wave 2 directive ledger) |
| URL generation | EQUIVALENT (landed: resolve_url + [url] defaults) |
| Sessions | EQUIVALENT (landed: SessionStore port + PgSessionStore; Wave 4 remember-me) |
| Cookies | EQUIVALENT (landed: typed cookie layer + `keel_session`; AES-GCM value encryption roundtrip-proven Wave 7 P7; docs/04) |
| Validation | EQUIVALENT (landed: wired since Wave 1; Wave 4 battery, regex SUPERSEDED; `[auth.password_policy]` enforcement Wave 7 P6) |
| Errors | EQUIVALENT |
| Logging | EQUIVALENT |
| Artisan/CLI | EQUIVALENT (Wave 4 added about/env/profile + config/cache rows; the full CLI matrix was reconciled through Wave 5 + Wave 6 — per-command rows carry per-wave evidence) |
| Broadcasting | EQUIVALENT (auth matrix + presence roster landed Wave 6 P7) |
| Cache | EQUIVALENT (landed: CacheService — Wave 4) |
| Collections | DELIBERATELY EXCLUDED |
| Concurrency | SUPERSEDED |
| Context | SUPERSEDED |
| Events | EQUIVALENT |
| Filesystem | EQUIVALENT (disk registry + `?disk=` uploads + directory ops landed Wave 6 P2; chunked writer sessions Wave 6 P10) |
| HTTP client | MATCHED (landed: port + reqwest adapter; Wave 6 P10 retry `{times, base_ms, max_ms, on}` schedule, per-request timeout override, bounded `execute_pool`, redacted `HttpTap` hooks, `HttpFake` assertions, streaming `sink_to` over `StoragePort::writer`) |
| Images | UNKNOWN |
| Localization | EQUIVALENT (boot `lang/<locale>.toml` catalogs + resolution order + `@lang`/`trans`/`trans_choice` landed Wave 6 P3) |
| Mail | EQUIVALENT (`Mailable` + `[mail.mailers]` failover chain + `mail.send` queued job landed Wave 6 P1; notification HTML through `views/notifications/` Wave 6 P5) |
| Notifications | EQUIVALENT (typed renderers + inbox endpoints + queued delivery + producers landed Wave 6 P5/P6) |
| Processes | UNKNOWN |
| Queues | EQUIVALENT (unique/rate-limit/middleware admission landed Wave 6 P8; durable batch ledger + batch endpoints Wave 6 P9) |
| Rate limiting | EQUIVALENT future per-principal |
| Search | EQUIVALENT future pgvector |
| Strings/helpers | SUPERSEDED |
| Scheduler | EQUIVALENT (cron/tz/overlap/one-server vocabulary + hooks landed Wave 6 P4; module-declared schedules remain undeclared) |
| Authentication | EQUIVALENT future (HTTP flows) |
| Authorization | SUPERSEDED |
| Email verification | EQUIVALENT future |
| Encryption | EQUIVALENT |
| Hashing | EQUIVALENT (argon2 rehash-on-login over `[identity]` factors landed Wave 7 P5) |
| Password reset | EQUIVALENT |
| Database core | EQUIVALENT |
| Query builder | EQUIVALENT future CRUD |
| Pagination | EQUIVALENT future HTTP |
| Migrations | MATCHED |
| Seeding | MATCHED |
| Redis | EQUIVALENT future |
| MongoDB | DELIBERATELY EXCLUDED |
| Eloquent | EQUIVALENT future ORM runtime |
| Factories | EQUIVALENT future |
| API resources | EQUIVALENT future |
| JSON:API | EQUIVALENT (decision gate passed review 2026-10-02: `specs/ADR/0007` Accepted — compound-document server implemented over resource routes: content negotiation, sparse fieldsets, includes, links, 406 media-type-parameter refusal, byte-identical default; `filter[]`/`sort[]`/cursor conventions recorded as future gates) |
| AI/MCP | SUPERSEDED |
| Testing | EQUIVALENT (assertion-bearing fakes + TestResponse/TestApp assertion matrix + time travel + savepoint isolation + factory runtime landed Wave 7) |
| Browser automation / Dusk | DELIBERATELY EXCLUDED from core v1; ecosystem |
| Cashier | DELIBERATELY EXCLUDED from core |
| Socialite | DELIBERATELY EXCLUDED from core |
| Valet | DELIBERATELY EXCLUDED |
| Telescope | SUPERSEDED by inspect/replay |
| Horizon | DELIBERATELY EXCLUDED from core v1 (no Laravel package clone); operational dashboard SHIPPED — ops-dashboard decision gate passed review 2026-10-02, `specs/ADR/0009` Accepted: read-only `/_dashboard/ops` over the existing job/scheduler/trace stores (queue depth, recent jobs, schedule state, execution inspect), flag-gated (`app.dashboard`), payload/input never rendered |
| Octane | SUPERSEDED by native Rust runtime model |
| Sail | DELIBERATELY EXCLUDED |
| Fortify | EQUIVALENT via auth services, not Laravel package clone |
| Sanctum | EQUIVALENT via hashed PAT + session model |
| Passport | EQUIVALENT via the optional `applications/oauth2` module (Wave 8 B-21: code + PKCE, refresh rotation, client registry, revocation) |
| Scout | EQUIVALENT future search adapters |
| Reverb | EQUIVALENT via realtime service |
| Folio | UNKNOWN until S9 |
| Livewire | UNKNOWN until S9 |
| Inertia | UNKNOWN until S9 |
| Breeze | EQUIVALENT future starter kit |
| Jetstream | DELIBERATELY EXCLUDED as exact clone; starter kit can cover outcome |
| Multi-database | DELIBERATELY EXCLUDED per D-62; revisit v2.0 |

### Important policy decisions

- **Blade/views:** resolved by the Wave 2 template engine (`crates/web/views`): rows flip to `EQUIVALENT` (directive ledger) / `SUPERSEDED` (`@php`, `@while`); directives outside the closed ledger flip to `MISSING`.
- **Facades/global helpers:** `DELIBERATELY EXCLUDED` because Rust dependency injection is the intended idiom.
- **Collections:** `DELIBERATELY EXCLUDED`; Rust iterators are the native equivalent.
- **JWT:** future `EQUIVALENT` add-on, not a defect in the current non-JWT default.
- **Multi-database:** `DELIBERATELY EXCLUDED` for v1 under D-62; revisit v2.0.
- **Telescope:** `SUPERSEDED` where inspect/replay provides stronger forensic semantics.
- **Octane:** `SUPERSEDED` by the native Rust runtime model rather than copied as a PHP worker accelerator.

## 23. Production Readiness Gate

The following checklist is a hard gate. A release is not production-ready until every item is green:

- [ ] Graph → HTTP router works end-to-end.
- [ ] Login works over HTTP.
- [ ] Registration works over HTTP where the application requires it.
- [ ] Logout invalidates server-side state.
- [ ] Cookie set/read/delete semantics are tested.
- [ ] Sessions are durable across restart and instances.
- [ ] Multipart uploads are implemented and hardened.
- [ ] Real PostgreSQL stores exist for domain modules.
- [ ] PostgreSQL integration tests are present and green.
- [ ] ORM CRUD is implemented and tested.
- [ ] Relation loading is implemented and tested.
- [ ] Per-principal/tenant rate limiting exists.
- [ ] Metrics endpoint exists.
- [ ] No `todo!`, `unimplemented!`, or equivalent placeholder remains in generated production paths.
- [ ] `keel serve` or a documented equivalent provides a one-command local loop.
- [ ] Named routes/URL generation are either implemented or explicitly superseded.
- [ ] Deployment cache semantics are documented.
- [ ] Health/readiness behavior is documented and tested.
- [ ] Graceful shutdown is verified for API and worker runtimes.
- [ ] Security review covers authentication, cookies, uploads, CSRF, authorization and tenancy.
- [ ] Framework blueprint tracker matches actual repository reality.

### Current gate result

**FAIL**. The failures are concentrated in the edge/data/authentication boundary rather than the core execution engine.

## 24. Final Brutal Verdict

### If released today

If Keel were released today as a purported Laravel alternative, adoption would **stall at the first real `curl`** for any domain application that relies on the graph-driven module model. The framework has substantial internal machinery, but the defining promise of an application framework is that a developer can turn domain code into a running, secure application. Today that path stops before the HTTP runtime.

### Ten adoption killers

1. Graph routes do not become executable HTTP routes.
2. No durable session store.
3. No cookie layer.
4. No HTTP login/register/logout flow.
5. Domain persistence still falls back to `BTreeMap` in the example module.
6. ORM is metadata + typed Select, not a CRUD runtime.
7. Multipart upload support is absent.
8. `keel serve` is absent, weakening the local loop.
9. No PostgreSQL-backed integration test matrix.
10. Web/view strategy is undecided, so full-stack application shape is not yet frozen.

### Ten genuine advantages over Laravel

1. Compile-time architecture gates.
2. Typed execution/context model.
3. Capability-enforced authorization at the engine boundary.
4. Delta/audit/replay as a first-class runtime artifact.
5. `keel inspect` / replay workflow.
6. Rust-native async/concurrency model.
7. Typed query construction and identifier safety.
8. Manifest/graph-driven module architecture.
9. MCP/AI-native capability exposure.
10. Native binary/runtime posture with lower-level control over memory and concurrency.

### Must-fix before Keel is a serious framework

The non-negotiable first wave is exactly **C1–C6**:

```text
C1 Graph → HTTP router
C2 HTTP auth flows
C3 Domain persistence ports + PostgreSQL store
C4 ORM runtime
C5 Multipart uploads
C6 Cookie layer / durable session glue
```

### Brutal verdict

**Current verdict: PROMISING, but not yet strong enough for external framework adoption.**

The architectural foundation is stronger than the current product surface. That is good news: the recommended path is additive, not a rewrite. But the team should resist the temptation to add advanced ecosystem features before the application edge is real. Do not spend the next cycle polishing AI/MCP, dashboards or package marketplaces while a simple domain route cannot reach an HTTP handler.

The next implementation decision should therefore come from **S7 Edge Wiring**, not from another feature audit.

## 25. v1.0 Gate Reconciliation (2026-08-24)

Sections 1-24 are the frozen forensic snapshot of 2026-08-22 (pre-S7). The S7-S10 implementation
push (v0.1-v0.5) and the v0.9 hardening pass have since landed. This section reconciles the
Definition of Done (section 21), the parity gate (section 22) and the production-readiness gate
(section 23) with verified repository reality. Where this section disagrees with an earlier
section, this section governs.

### 25.1 The ten adoption killers - resolved

| # | Snapshot blocker (section 24) | Resolution | Evidence |
|---|---|---|---|
| 1 | Graph routes do not become executable HTTP routes | S7 C1: graph router compiles RouteSpecs into axum routes; appointments served over HTTP | `runtime/keel-api/src/graph_routes.rs`; `http_integration::full_cycle_register_login_book_list_cancel` |
| 2 | No durable session store | S7 C6: PgSessionStore behind SessionPort; in-memory for degraded boot | `crates/data/postgres` (sessions), pg-tests slice |
| 3 | No cookie layer | S7 C6: typed Cookie/CookieJar + HMAC-tagged `keel_session` cookie | `crates/web/http/src/session_mw.rs`, cookie-shape integration tests |
| 4 | No HTTP login/register/logout flow | S7 C2: /auth/register,login,logout,me,password-reset with session + PAT modes | `runtime/keel-api/src/auth_routes.rs`; throttle/bad-password/bearer tests |
| 5 | Domain persistence falls back to BTreeMap | S8 C3: ModelStore port; appointments refactored onto it | `applications/appointments`; PgModelStore |
| 6 | ORM is metadata, not a CRUD runtime | S8 C4: ModelRuntime (mass assignment, casts, scopes, accessors/mutators, events, eager loading) + Insert/Update/Delete/Upsert builders | `crates/data/models`, `crates/execution/query`, pg matrix |
| 7 | Multipart uploads absent | S7 C5: bounded multipart pipeline (8 parts / 64 KB fields / 8 MB files), MIME hook, filename sanitization into StoragePort | `crates/web/http/src/upload.rs`; upload + part-bomb attack tests |
| 8 | keel serve absent | S7 B-08: one-command local loop (api + optional worker) | `tools/keel-console` serve; 12-CLI.md |
| 9 | No PostgreSQL integration matrix | S8 B-19 + v0.9: pg-tests feature; 30 tests green under the feature — a live pass against PostgreSQL 16 covers bridge, sessions, tokens, principals, model store CRUD/tenant/upsert/soft-delete/eager-load, migration pairs, the Wave 6 P5 notification inbox round trip, the Wave 6 P9 batch ledger round trip and pgvector keyword/facet/vector/hybrid (codec/SQL-shape unit slices alongside; every DB slice skips gracefully without `DATABASE_URL`) | `cargo test --features pg-tests`; v0.9 milestone read |
| 10 | Web/view strategy undecided | S9 B-15: SPA-first frozen (named routes + signed URLs + redirects + flash + OpenAPI client + starter kit); SSR deliberately deferred to v2.0 | `templates/starter-template`; parity policy below |

### 25.2 Production-readiness gate (section 23): PASS

| Gate item | Result | Evidence |
|---|---|---|
| Graph → HTTP router works end-to-end | GREEN | http_integration suite (19 tests) |
| Login works over HTTP | GREEN | full_cycle + bearer tests |
| Registration works over HTTP where required | GREEN | register tests; env-gated (dev/test or KEEL_ALLOW_REGISTER) |
| Logout invalidates server-side state | GREEN | sessions_persist_across_requests_and_logout_destroys |
| Cookie set/read/delete semantics tested | GREEN | cookie-shape + forged-tag + logout clear tests |
| Sessions durable across restart and instances | GREEN | PgSessionStore + restart durability proof (v0.9 pg pass); KEEL_SESSION_SECRET stability documented |
| Multipart uploads implemented and hardened | GREEN | limits/traversal/part-bomb/unauthenticated/non-multipart tests |
| Real PostgreSQL stores for domain modules | GREEN | PgModelStore; appointments persists real rows |
| PostgreSQL integration tests present and green | GREEN | pg-tests matrix: 30 tests (keel-postgres 21 + pgvector 9), live-green incl. tenant isolation and the Wave 6 P5 notification + Wave 6 P9 batch ledger round trips |
| ORM CRUD implemented and tested | GREEN | pg_modelstore CRUD/RETURNING/upsert tests |
| Relation loading implemented and tested | GREEN | BelongsTo eager-load test; N+1 detector as guard |
| Per-principal/tenant rate limiting | GREEN | bearer-principal keying (S7 B-09); login throttle test |
| Metrics endpoint exists | GREEN | flag-gated /metrics (app.metrics), Prometheus text; middleware counts tested |
| No todo!/unimplemented! in generated production paths | GREEN | v0.9 placeholder audit; codegen templates asserted placeholder-free by test |
| keel serve provides one-command local loop | GREEN | keel-console serve (+ --worker) |
| Named routes/URL generation implemented or superseded | GREEN | implemented: RouteSpec name + resolve_url + GRAPH-DUP-ROUTENAME gate (S9 B-07) |
| Deployment cache semantics documented | GREEN | 13-OPERATIONS "Deployment cache semantics" (boot-time graph compile stays the source of truth; Wave 1 adds the optional hash-pinned route:cache artifact with drift-refusing boot) |
| Health/readiness documented and tested | GREEN | 13-OPERATIONS probe semantics + healthz/readyz degraded-state test |
| Graceful shutdown verified for API and worker | GREEN | axum with_graceful_shutdown drain; worker cooperative stop + final outbox drain; documented |
| Security review covers auth/cookies/uploads/CSRF/authz/tenancy | GREEN | v0.9 hardening pass: constant-time comparisons, session fixation, login timing, enumeration resistance, part bomb, forged cookies; 06-SECURITY records the matrix |
| Framework blueprint tracker matches repository reality | GREEN | docs/rust/13-FRAMEWORK-BLUEPRINT.md milestone reads S7-S10 + v0.9 hardening; CLI matrix reconciled at 42 commands |

### 25.3 Parity-gate rows resolved since the snapshot

| Row (section 22) | Snapshot position | v1.0 resolution |
|---|---|---|
| Routing core / Responses | EQUIVALENT future edge completion | Implemented (S7); envelope + redirect + flash edge complete |
| Named routes / URL generation | EQUIVALENT future S7/S9 | Implemented (S9), incl. signed + temporary signed URLs |
| Requests/FormRequest | EQUIVALENT future HTTP wiring | Wired: validation runs in-engine before operations; 422 envelopes at the edge |
| Sessions / Cookies | EQUIVALENT future durable implementation | Implemented (S7) + hardened (v0.9): rotation, constant-time tags, durable Pg store |
| Authentication | EQUIVALENT future HTTP | Implemented (S7): register/login/logout/me/password-reset, PAT + session modes |
| Query builder | EQUIVALENT future CRUD | Implemented (S8): Insert/Update/Delete/Upsert with tenant guards + RETURNING |
| Eloquent | EQUIVALENT future ORM runtime | Implemented (S8): CRUD, relations, casts, scopes, accessors/mutators, events |
| Factories | EQUIVALENT future | Implemented (S8): codegen build() real, placeholder-free |
| Pagination | EQUIVALENT future HTTP | Wired through list operations (PageReq envelope); Wave 1: GET routes parse page/per_page at the edge, inject _page (MAX_PER_PAGE 100 clamp), list handlers emit a page meta block |
| Redis | EQUIVALENT future | Implemented (S10): RedisCache + RedisJobStore adapters, feature-gated |
| Search | EQUIVALENT future pgvector | Implemented (S10): pgvector adapter, keyword/facet/vector/hybrid, tenant-isolated |
| Rate limiting | EQUIVALENT future per-principal | Implemented (S7); Wave 1 named throttle:<group> chains (per-group token buckets in middleware.toml, 429 + Retry-After) |
| Broadcasting / Reverb | EQUIVALENT via realtime service | Flag-gated realtime router + SSE/WebSocket paths; Wave 6 P7 adds channel-token auth at connect + presence roster/members (broadcast/auth) |
| Artisan/CLI | EQUIVALENT | 42 commands incl. serve + observe family; matrix reconciled in 12-CLI |
| Breeze | EQUIVALENT future starter kit | Implemented (S9): starter template behind keel new --with-starter |
| Sanctum | EQUIVALENT via hashed PAT + session model | Implemented (S7 auth flows) |
| Fortify | EQUIVALENT via auth services | Implemented (S7 register/login/reset) |
| Scout | EQUIVALENT future search adapters | Implemented (S10 pgvector) |
| Telescope | SUPERSEDED by inspect/replay | Extended: keel observe family (status/logs/errors/traces/request/audit/incidents/exporters/policies/buffers/health/cost/metrics) |
| Horizon | dashboard roadmap | Implemented (S10): Horizon-lite /_dashboard over meta/queue/flag JSON |
| Octane | SUPERSEDED by native runtime | Unchanged |
| Views / Blade / Folio / Livewire / Inertia | EQUIVALENT (Wave 2 views engine) | SPA-first stays the default web story; the Wave 2 template engine covers Blade-equivalent views (full directive ledger, compiled cache). Folio/Livewire/Inertia stay out of scope under the S9 SPA-first policy |
| Passport (OAuth2) | EQUIVALENT future | Implemented (Wave 8 B-21): optional `applications/oauth2` module — authorization code + PKCE S256, refresh rotation, `oauth_clients` registry, RFC 7009 revocation; PAT + session stay the default |
| Metrics (D-68) | MISSING | Scoped delivery (S10): runtime metrics live (/metrics); the profiling remainder landed Wave 4 (B-27): `keel profile` samply wrapper + flamegraph workflow in 13-OPERATIONS (out-of-process, zero workspace deps) |

### 25.4 Deliberate v1.0 deferrals and exclusions (unchanged policy)

- **SSR/template engine** - v2.0 revisit with a fresh dependency-budget decision; SPA-first is the supported v1.0 web story.

All deferrals below are tracked as concrete backlog items in section 26.
- **Package marketplace/registry discovery** - post-v1.0; the extension contract is stabilized first (module ABI gate, S10).
- **JWT default, OAuth2 server** - JWT stays a future add-on; the OAuth2 server ships as the optional Wave 8 module (`applications/oauth2`, B-21) while hashed PAT + HMAC sessions remain the default.
- **Facades/global helpers, collections wrapper, multi-DB/MongoDB, Cashier/Socialite/Dusk/Valet clones** - DELIBERATELY EXCLUDED per section 22 policy; Rust-native equivalents supersede them.

### 25.5 v1.0 verdict (supersedes section 24 snapshot)

Definition of Done check: parity gate green per 25.2-25.3; documentation complete (01-18
reconciled with repository reality, CLI matrix at 42 commands, examples execute against actual
APIs); benchmark suite documented (keel benchmark, benchmarks/baseline.json, G15 budget gate);
upgrade/migration story established (keel upgrade + 16-MIGRATION-FROM-LARAVEL vocabulary map and
strategy).

**v1.0 gate result: PASS.** The snapshot's brutal verdict ("promising, but not yet strong enough
for external framework adoption") was correct for 2026-08-22. As of this reconciliation the
application edge is real, durable, authenticated, attack-tested and PostgreSQL-proven. Keel meets
its v1.0 parity gate as an API-first, SPA-friendly Laravel-class application platform with a
Rust-native execution engine - with the documented deferrals above as the agreed v1.x/v2.0 agenda.

## 26. v1.x Backlog (drafted from section 25.4 deferrals, 2026-08-24)

Numbering continues the section 19 backlog (B-21 onward). Sources: the 25.4 deferral list plus
residual parity rows that were committed as "EQUIVALENT future" but were not v1.0-blocking.
Budget rule: the D-62 external-dependency budget is exhausted as of v1.0 (the final slot was
spent on the S10 redis client). Every item below is either zero-new-dependency or explicitly
requires a budget amendment or a decision gate.

Program: these backlog items are sequenced, expanded and superseded by the nine-wave program in `19-PARITY-PROGRAM.md` (v2, 2026-08-25); where that document assigns an item to a wave, the wave assignment governs over this table's priorities.

Wave 1 completion entry (2026-08-28): Wave 1 — HTTP core completion is DONE per `19-PARITY-PROGRAM.md`: named middleware chains (`middleware.toml` aliases/groups/`throttle.<group>` + implicit `api` group + `GRAPH-MW-UNKNOWN` boot gate), route model binding (`route.bindings` registry service, `_bindings.<param>` injection, `GRAPH-BINDING-RESOLVER` at keel-api boot), pagination injection (`_page`, `MAX_PER_PAGE` 100 clamp, page meta block), manifest `[[validation]]` enforced at the edge (422 + `fields` meta), deprecated-route `Deprecation: true` header/meta, `keel route:cache`/`route:clear` with hash-pinned boot refusal (`storage/framework/routes.json`), and OpenAPI constraint emission. Evidence: CHANGELOG `[Unreleased]`; `http_integration` Wave 1 battery; 609 workspace tests + architecture gate green; clippy clean on all touched crates. The §4 rows (Middleware, Rate limiting, Route cache, Pagination), the function-level ledger rows (middleware, binding, model binding, route:cache, route:clear), the §11 CLI rows and §19 B-09 above are flipped accordingly.

Wave 2 completion entry (2026-09-29): Wave 2 - routing completion, web plane, views, request/response, errors and static assets is DONE per `19-PARITY-PROGRAM.md`: verb/ANY dispatch with auto HEAD/OPTIONS (405 + Allow), api/web planes, `[[route_groups]]` prefix/middleware/plane merge, domain/subdomain RouteGate before auth, closed constraint kinds + pattern mini-grammar (mismatch -> 404), `resolve_url` `[url]` defaults + temporary/one-time signed URLs, the `crates/web/views` template engine (directive ledger, compiled-AST cache, `keel view:cache`/`view:clear`), HTML routes + web-plane error views, response kinds (json/html/redirect/download/file/stream/no_content), ETag/304, RFC-7807 option, static assets for `public/` (traversal-safe, fingerprinted immutable caching, Vite `manifest.json` reader + `@asset`), and `keel route:list --name/--path/--plane` with parity columns. Evidence: CHANGELOG `[Unreleased]`; keel-api integration suites (http/views/error_rendering/response_layer/routing_edge/static_assets); 800+ workspace tests green; `clippy -D warnings` and `dep-rules` clean. The section 4 rows (Routing, Named routes, HTTP responses, HTML responses, Redirects, Views, Blade), the routing + response rows, the Laravel 13 template rows, the Blade/views cross-reference ledger, the section 11 CLI rows (route:list, view:cache, view:clear) and backlog B-26 are flipped accordingly.

Wave 3 completion entry (2026-09-29): Wave 3 — Eloquent & query parity + API resources is DONE per `19-PARITY-PROGRAM.md`: full keel-query read-builder surface (clause-tree rendering, unions, group/having, aggregates + existence compile, locks, date family, correlated `where_exists` — 33 rendering tests; execution rides ModelRuntime as documented), ModelStore port growth (aggregate/exists/find_for_update/filtered updates + deletes/force_delete/restore/truncate), ModelRuntime finders & mutations (`first_or_create`, `update_or_create`, `force_create`, `create_many`, `insert_get_id`, `insert_or_ignore`, `increment`/`decrement`, `replicate`, `refresh`/`fresh`, `load_missing`, `touch`, `push`, `pluck`/`value`/`chunk`/`each`/`cursor`), relation parity (eager aggregates, `has`/`where_relation`, nested `with` + per-level constraints, `attach`/`sync`/`toggle`, polymorphic morph-map), casts + serialization visibility + strict mode, soft-delete trio + route-binding `with_trashed` + global scopes, optimistic locking (`version_column` -> `STALE_RECORD` -> 409), `RecordChanges` dirty tracking, `without_timestamps`, `PruneRegistry` + `keel model:prune`, and the typed `Resource` transformer (`when`-conditional fields, wrapping key, `ResourceCollection` + pagination-meta passthrough) with `make resource` rewired to a compiling trait impl. Evidence: CHANGELOG `[Unreleased]`; keel-models runtime/store/prune/binding suites; keel-query rendering suites; keel-http resource suites; keel-codegen template gates; soft-delete veil roundtrip in the pg modelstore suite (DATABASE_URL-gated); 932 workspace tests with dep-rules + clippy clean. Deferred honestly: the Wave 3 live-pg matrix additions (aggregates/exists/restore + force_delete/version column/`insert_or_ignore`/chunk streaming vs PostgreSQL 16) were not added — B-19 stays open and the affected rows carry conservative wire-proof notes; the join rows (join/leftJoin/rightJoin/crossJoin/joinSub) are untouched Wave-3-out-of-scope. The section 4 rows (Query builder, Eloquent ORM, Relations, Casts, Accessors/mutators, Scopes, Soft deletes, Model events/observers, Factories, PostgreSQL integration tests), the query-builder and Eloquent function-ledger rows, the section 12 ORM matrix, the section 11 CLI rows (model:prune, make:factory, make:resource) and backlog B-05/B-10/B-11 are flipped accordingly.

Wave 4 completion entry (2026-09-30): Wave 4 — Validation expansion + config + cache + parity residuals is DONE per `19-PARITY-PROGRAM.md`: ~45 new validation rules (types/strings/numeric/conditionals/cross-field/dates/files/composable `password` + tenant-scoped `unique`/`exists` ModelStore probes with DB error -> 503) with `bail`, dot-path wildcards and custom messages behind the collect-all boot gates `GRAPH-VALIDATION-RULE`/`GRAPH-VALIDATION-FIELD`/`GRAPH-RULE-UNKNOWN` (docs/19's earlier line named the token gate `GRAPH-RULE-UNKNOWN` — corrected: the code reserves that name for unregistered custom rules); `PlatformConfig::freeze()` + `GRAPH-CONFIG-SCHEMA` + the xtask env lint + `keel env`/`config:cache`/`config:clear`/`config:show`/`cache:forget` with hash-pinned drift refusal; `CacheService` (remember + stampede single-flight, atomic `add`, tags, locks, counters; serde_json values — `keel-caching` gains the workspace-budgeted `serde_json`, no budget change); session remember-me (cookie `Max-Age` + bag flag, regeneration-persistent, `/auth/me`-reported) and `POST /auth/confirm` + `confirmed` middleware; `keel about`; `GRAPH-VERSION-SEMVER` + route version columns + OpenAPI `x-keel-version`; shell `caps`/`models`; the `keel profile` samply wrapper; and the Playwright smoke kit at `templates/starter-template/e2e/`. Evidence: CHANGELOG `[Unreleased]`; the validation rule battery + `validation_edge_integration`; the keel-caching recording-port suite; keel-console config/about/profile/env tests; shell `caps_and_models_introspect_grants_and_registries`; registry `graph_version_semver_gate_rejects_bad_tokens_and_segment_mismatches`; the `openapi_emits_validation_constraints_and_deprecation` `x-keel-version` asserts; 1023 workspace tests with dep-rules + clippy clean; publish dry-run count=61 deferred=58. Deferred honestly: the web-plane password-confirm redirect was not implemented (both planes answer the 403 envelope — same precedent as `verified`); remember-me is cookie-`Max-Age` only (server-side idle/absolute session bounds unchanged); the Playwright kit executes out-of-band on external tooling (npm + browsers — not a workspace gate) and the profiling record runs only where samply is installed (the absent-samply path is unit-proven); no dedicated live-pg `unique`/`exists` wire tests were added (B-19 stays open). The §4 rows (FormRequest, Sessions, PgSessionStore, Cookies, HTTP auth flows, Validation, Cache, Redis cache driver, Config cache, View cache), the cache function-ledger rows (get/put/add/remember/rememberForever/forever/forget/flush/increment/decrement/pull/many/putMany/getMultiple/lock/forceRelease), the Auth/Session function-ledger rows (every row except `session forget` + `token revoke`, which stay conservative), the §11 CLI rows (about, env, cache:forget, config:cache/clear/show, tinker, serve), the §22 parity-gate annotations (Requests/FormRequest, Sessions, Cookies, Validation, Artisan/CLI, Cache) and backlog B-27/B-28/B-29 are flipped accordingly.

Wave 5 completion entry (2026-09-30): Wave 5 — Command & generator surface is DONE per `19-PARITY-PROGRAM.md`: the optimize pair (`keel optimize` warms route/config/view artifacts in order, aborting at the first refusal; `keel optimize:clear` drops all three attempting every step); the migration family (`migrate:rollback [--step n]` over revertible `NNN.up.sql`/`NNN.down.sql` pairs in batches newest-first + reverse filename order with `keel_migrations` bookkeeping, `migrate:reset`/`migrate:refresh [--step n]` behind the dev/test-only destructive window, `migrate:install`, forward-only singles explicitly skipped); the advisory lock (`pg_advisory_lock(7259)` before any run, fail-closed on timeout — concurrent migrators serialize); `keel db:show` + `keel db:schema:dump` (normalized DDL snapshot to `database/schema.sql`, empty-schema marker); the queue admin surface (`queue:clear` dev/test-gated + `KEEL_FORCE`, `queue:flush` unbounded dead-job purge, `queue:forget`, `queue:monitor` depth/age render, `queue:prune-failed --hours 168`, `queue:prune-batches --hours 72`, `queue:restart` signal file + worker start-time race, `queue:listen` supervisor with crash-loop backoff, `queue:retry-batch` id-validated fail-closed, `queue:failed`/`queue:retry` left honest); `keel storage:link [--force]` (refuses overwrite, never deletes a real directory); `keel test [--pg]` cargo-test wrapper with PASS/FAIL tally + `DATABASE_URL` fail-closed; `keel call <name> [json] [--principal] [--tenant]` console routes through the CommandBus under the same policy engine as HTTP; the schedule daemon (`schedule:work [--every n]` ticking durable jobs + persisted state, `schedule:test` pure dry run) over the deliberately empty `schedule_registry()` seam; and codegen 15 → 27 kinds (cast, channel, component, exception, mail, notification, observer, rule, scope, job-middleware, middleware, view) with per-kind substitution/placeholder-free tests + real-platform-surface needle asserts. Evidence: CHANGELOG `[Unreleased]`; `migrate_pair_commands_parse_with_documented_defaults`, `rollback_order_walks_batches_newest_first_in_reverse_filename_order`, `migration_lock_key_matches_the_program_spec`, `db_show_renders_server_metadata_and_inventory`, `schema_dump_renders_normalized_sorted_ddl`, `queue_admin_commands_parse_with_documented_defaults`, `queue_monitor_renders_depth_and_oldest_age`, `admin_maintenance_purges_prunes_and_snapshots` (jobs), `restart_signal_is_ignored_when_older_than_the_worker_start` (jobs), `storage_link_*` (4 tests), `test_result_lines_parse_pass_and_fail`, `call_command_parses_with_documented_flags` + `call_pre_flight_resolves_routes_payloads_and_env_gate` (console) + `console_route_dispatch_enforces_capability_checks` + `operator_resolution_picks_first_active_and_rejects_unknown_explicit` + `tenant_resolution_falls_back_to_the_dev_tenant` (keel-api), `schedule_tick_fires_due_items_and_persists_state` + `schedule_commands_exit_cleanly_when_nothing_is_registered` + `schedule_test_plan_is_pure_and_skips_disabled`, `wave5_adds_twelve_generator_kinds` + `wave5_templates_reference_real_platform_surface`, and the pg-gated `pair_migrations_apply_rollback_and_reset_with_batches` + `advisory_lock_serializes_runs_and_fails_closed_on_timeout` (DATABASE_URL-gated, skipped in the default matrix — B-19 stays open); 1058 workspace tests with dep-rules + clippy clean; publish dry-run count=61 deferred=58. Deferred honestly: the batch ledger is in-memory only, so `queue:prune-batches` reports an honest empty run and `queue:retry-batch` fails closed (new backlog B-32); there is no admin-role model, so `keel call`'s default operator is the first active principal — dev/test-only, explicit `--principal` is the production path (new backlog B-33); production tenant resolution has no fallback at all (the dev/test default tenant is dev/test-only — production requires `--tenant`); `keel call` runs pre-flight only and skips the HTTP edge-stage validation layer (routes are pre-validated at boot); `schedule:list`/`schedule:run` remain `schedule_info` information stubs over the empty registry (module schedules land Wave 6); mail/notification templates are standalone definitions that document their Wave 6 landing; `keel dev` remains the one gate-pending CLI placeholder; `queue:failed`/`queue:retry` keep UNKNOWN (Wave 2 implementation, no locked test); pg-gated tests skip without `DATABASE_URL` (B-19). The §11 CLI rows (optimize, optimize:clear, clear-compiled, db:show, schema:dump, test, the make:* rows for the 12 new kinds, the migrate family, the queue admin rows, schedule:list/run/test/work, storage:link), the §4 queue:restart row, the §2 stale factory row and this backlog (B-32, B-33) are flipped accordingly.

Wave 6 completion entry (2026-10-01): Wave 6 — Services parity is DONE per `19-PARITY-PROGRAM.md`: notifications v2 (typed `NotificationDef` per-channel renderers, the `notifications` table behind a `NotificationStore` port with pg + in-memory stores, inbox `GET /api/v1/notifications` + capability-gated `POST /api/v1/notifications/read` with unread-count meta, on-demand notifiables with sha256 target-hash audit (D-74 caller-persisted), `queue_via` v1-envelope queued delivery drained by the `notification.send` worker job, appointments Book/Cancel producer dispatch in the request locale); Mail (`Mailable` with StoragePort-capped attachments, `[mail]` config with password redaction, `MailChain` first-healthy failover, the `mail.send` queued job, notification HTML through `views/notifications/`); storage disks (`[storage] default` + `[storage.disks.<name>]` registry with boot refusal, visibility-gated `url_for`, port `list`/`make_dir`/`delete_dir`, `?disk=` upload selection (unknown disk → 422), chunked `StoragePort::writer` commit/abort sessions); i18n (fail-closed `lang/<locale>.toml` boot loader with shipped en/fr/ar, session → quality-ordered `Accept-Language` → default resolution, `@lang`/`trans`/`trans_choice` CLDR-style buckets, localized `validation.<rule>` and notification messages); the scheduling vocabulary (zero-dep 5-field cron parser + builder helpers, bundled static IANA timezone table, `without_overlapping`/`on_one_server` `ScheduleLock` guards, per-item hooks — `schedule:work`/`schedule:test` keep consuming the deliberately empty registry seam); broadcast channel auth (`POST /api/v1/broadcast/auth` issuing a 300s `TokenPurpose::Channel` token over the boot-resolved `routes/channels.toml` capability gate, `GET /api/v1/broadcast/members` presence roster under the same gate, host-side `ChannelTokenAuthorizer` at every `/realtime/sse|ws` connect with a uniform 401); the queue admission plane (unique cache-lock admission that defers rather than fails, rate-limit token-bucket holds, `JobMiddleware`/`MiddlewareStacks` with built-ins composed before module layers, `retry_limit`, `JobStoreAdmin` defer/kill, chain failure positions, `/metrics` queue depth/age/throughput census); the durable batch ledger (B-32: `BatchLedger` memory/Pg over `20261001000002_job_batches.sql` with membership written once before enqueue, `BatchOps::{status,cancel,retry}`, capability-gated batch endpoints `GET /api/v1/batches/{id}` + `POST .../cancel|/retry`, `queue:retry-batch`/`queue:prune-batches` on the real ledger); and HTTP-client parity (retry schedule `{times, base_ms, max_ms, on}` with `retry_on` gating retries only, per-request `timeout_ms`, bounded `execute_pool`, redacted `HttpTap` hooks, `HttpFake` assertions, streaming `sink_to` over `StoragePort::writer`). Evidence: CHANGELOG `[Unreleased]`; the notifications/mail/storage/i18n/scheduler/realtime/jobs suites; keel-api integration (notification + batch + broadcast endpoints, `?disk=` uploads, locale e2e, `/metrics` census); keel-worker job registration + drains; keel-console batch commands; http-client's 17-test loopback battery (scripted std-thread HTTP/1.1 server — zero new deps); keel-storage/keel-storage-local writer sessions; pg round trips `pg_notification_store_roundtrip` + `pg_batch_ledger_roundtrip` live vs PostgreSQL 16 (matrix 30 = keel-postgres 21 + pgvector 9); 1205 workspace tests with dep-rules + clippy clean; publish dry-run count=61 deferred=58. Deferred honestly: no multipart/streamed request bodies on the HTTP client (outside the wave scope); the `s3` disk driver stays the in-memory sandbox (program decision); cron is numbers-only (no `MON`/`?`/`L`/`#`, no seconds field) and the timezone table carries fixed offsets (no DST transitions); parameterized validation messages keep their built-in English wording; the session locale is read-only (no endpoint sets it yet); module-declared schedules did not land — `schedule_registry()` stays empty (module content outside the nine-wave program, superseding the Wave 5 plan note); and HttpFake's assertions ship in the crate while the 11-TESTING fakes list gains HttpFake with the Wave 7 testkit. Reconciled here: the §11 `make:mail` row (send surface landed P1 → MATCHED) and the §22 status rows (Artisan/CLI, Filesystem, Localization, Mail, Notifications, Queues, Scheduler); §4 evidence rows landed with their phases; backlog B-32 is marked DONE.

Wave 7 completion entry (2026-10-02): Wave 7 — Testing parity + security parity is DONE per `19-PARITY-PROGRAM.md`: assertion-bearing testkit fakes (`QueueFake`/`EventFake`/`NotificationFake`/`MailFake`/`StorageFake`/`CacheFake` + the `HttpFake` re-export; spec names canonical with `FakeQueue`/`FakeMail`/`FakeStorage` kept as documented `pub type` aliases; `NotifyHub` `DispatchTap` seam feeding `assert_sent_to`); the `TestResponse`/`TestApp` assertion matrix (redirect/session/cookie/view/json-path/html/database/auth asserts, each consuming and returning the response, over `with_session`/`with_views`/`with_models` probes with the `ViewRecorder` seam); test infrastructure (injectable `ApplicationHost` clock with `freeze_time`/`travel` threaded through sessions/tokens/scheduler/fakes, `keel_postgres::test_support::Savepoint` rollback isolation, fake queue/scheduler builders, factory `create`/`state`/`Sequence`); and security parity — web-plane `[security.headers]` posture (nosniff/HSTS/CSP/Referrer-Policy, handler-set headers win, fail-closed on unparseable values, default-off), argon2 rehash-on-login over `[identity]` factors with PHC-drift detection, `[auth.password_policy]` (Laravel defaults) feeding the `password` rule and enforced at register + both resets (`reset_complete` validates before consuming the token), `[security.signed_urls].max_ttl_secs` (default 7d, 30d boot ceiling, `sign_once` mint bound), the cookie-value encryption roundtrip under `[http] encrypt_cookies`, and CSRF default-on for the web plane (`[security.csrf]` `skip_paths`-only config, double-submit `keel_csrf` vs `x-csrf-token` in constant time, safe-method bootstrap cookie, `KEEL_CSRF` removed). Evidence: CHANGELOG `[Unreleased]`; the testkit fakes/app/time/factories suites + `tests/harness` surface smokes; keel-api `http_integration`/config-schema/edge-roundtrip suites; 1312 workspace tests (150 suites) with `dep-rules` + clippy clean; publish dry-run count=61 deferred=58. Deferred honestly: platform-plane routes (auth, uploads, telemetry) keep the token-authenticated posture (the spec's split); the spec's `cookies.encrypted` name ships as `[http] encrypt_cookies` (config keys are additive-only — no rename); `[security.headers]` is default-off while CSRF is default-on; the `schedule:work` daemon and identity stamps keep wall time (the injected clock covers workers, scheduler, sessions and tokens); the pg-gated savepoint test skips without `DATABASE_URL` (B-19 stays open). Reconciled here: the §4 rows (Cookies, HTTP auth flows, Validation, Testing framework), the function-level rows (temporary signed URL, password), the §22 status rows (CSRF, Cookies, Validation, Hashing, Testing) and the §26.2 dependency ledger (Wave 7 = zero new deps); no backlog row changes (B-19 stays open).

Wave 8 completion entry (2026-10-02): Wave 8 — Foundation helpers + ecosystem is DONE per `19-PARITY-PROGRAM.md`: the `Str` family (`to_title_case`, `words(limit, end)`, `excerpt(search, before, after, limit)` match-preserving char window, CSPRNG `random(len)` over the existing workspace `rand`, `uuid()` v4 + a hand-rolled `ulid()` on the `ulid`-rule charset, and the `inflect` module — `plural`/`singular` over regular rules, a documented ~50-pair irregular table and ~28 uncountables, round-trip tested); `Number` formatting (`format_currency`/`format_cents`/`format_percent`/`format_precision` locale-aware through the Wave 6 catalog — en/fr/ar rules, ~14-code symbol table with minor units, `en-IN`/`fr-CA` base matches, unknown locale → en — plus a `NaN`-safe `clamp`); the shared closed date-format grammar (`keel-validation`'s `date_format:<fmt>` now DELEGATES spec check and value match to `keel-foundation` — one grammar, not a convention) with `DateParts`/`now_in`/`in_tz`/`local_wall` over the IANA table RELOCATED to `crates/foundation/core/src/tz.rs` (`keel-scheduler` re-exports `timezone_offset_secs`), feeding the new `datetime:<tz>` model cast; the package registry + discovery (root `packages.toml` index with `deny_unknown_fields`, `keel pkg:list`/`keel pkg:add`, graph-compile discovery merged through BOTH existing copies into the fail-closed GRAPH gates, the route-cache `manifests_hash` pin extended over the index + every indexed manifest, and boot config merging each package's `[config]` under the config file — serde-default `ModuleManifest::config`, `MODULE_ABI` stays 1); `keel publish`/`keel config:publish` (name-prefixed migrations after the sqlx integer version, deterministic config bytes at `config/packages/<name>.toml`, assets to `public/vendor/<name>/`, `pkg::` view namespaces wired at boot, byte-compared idempotence + conflict report + `--force`, exit non-zero whenever a conflict was skipped); and the OAuth2 authorization server as the optional `applications/oauth2` module (B-21, never core): authorization code + PKCE S256, single-use refresh rotation, `oauth_clients` registry (argon2 secret shown once, redirect allowlist, `version` CAS), RFC 7009 revocation answering 200 for unknown tokens, access tokens = existing PATs, web-plane authorize behind the CSRF double-submit, the new `application/x-www-form-urlencoded` intake branch in `graph_dispatch` (string values, query strings still never decoded), an `oauth-clients` console row, migration `20260826000001_oauth2.sql` — zero new deps throughout. The three decision gates delivered their docs only: `specs/ADR/0007` (JSON:API decision doc + resource mapping spec), `specs/ADR/0008` (Precognition decision doc + dispatch spec), `specs/ADR/0009` (ops dashboard decision doc + data-source spec) — status Proposed, implementation blocked until the gates pass review. Evidence: CHANGELOG `[Unreleased]`; foundation units, registry/console publish suites, the keel-api `http_integration` oauth matrix + `form_decode` unit; 1371 workspace tests (150 suites) with `dep-rules` + clippy clean; publish dry-run count=61 deferred=58. Deferred honestly: the decision-gate IMPLEMENTATIONS (JSON:API compound documents, Precognition, ops dashboard) are not built — docs first, per the gate rule; `collect()` stays excluded (§22); the master decision log (`docs/rust/08-DECISIONS.md`) lives at the repo root outside this program's platform-only scope, so no D-numbers are minted here. The §11 `config:publish` row, the §22 JSON:API/Horizon/Passport rows, the §25.3 Passport row, the §25.4 OAuth2 deferral bullet, and the §26 B-21/B-24/B-25 + v1.2 roadmap rows are flipped accordingly.

Wave 9 completion entry (2026-10-03): Wave 9 - v2.0 gates is IN PROGRESS per `19-PARITY-PROGRAM.md`: B-30 (WASM plugin runtime) is DONE as `crates/extension/wasm` (`keel-wasm`, workspace-internal, `publish = false`) over wasmtime 34.0.2 - the one recorded D-62 amendment (`default-features = false`, cranelift/runtime/wat/std; `Apache-2.0 WITH LLVM-exception` added to the deny allowlist; analyzer matrix gains `Class::Extension`, externals 25/25). Sidecar-manifest discovery: `packages.toml` `[[wasm]] { name, version, path }` (serde-default, key omitted when empty), each binary paired with a reviewed `<binary>.module.toml` parsed through the same GRAPH gates, and `manifests()`/`contains_name()`/`manifests_hash` (sidecar + binary bytes) all covering plugins. Guest ABI v1 (`memory`, `keel_manifest`, `keel_alloc`, `keel_call`) with byte-identical guest-vs-sidecar drift refusal at load; sandbox = 10M fuel + 16 MiB store cap (exhaustion -> append-only `KER_WASM_FUEL` pinned 503 at both boot sites) and a hand-rolled WASI-preview1 subset with NO `sock_*` imports (networking guests fail at link time) + `host.log`/`host.capability_check` answering only from the sidecar-declared set under an attached scope. `WasmHandler: OperationHandler` runs readonly-only (enforced at discovery AND dispatch) through the existing engine's authorization/idempotency/audit; `load_wasm_plugins` runs before `host.boot()` and `module.wasm` is registered unconditionally with `DemoModuleProvider` depending on it (`ApplicationHost::boot` now computes topo order before init). Evidence: CHANGELOG `[Unreleased]`; keel-api `wasm_tests` (5-test facet suite over five `.wat` fixtures: echo, fib, spin, capcheck, garbage); registry `wasm_sidecars_discover_with_full_context_refusals`, `save_omits_the_wasm_key_when_empty_and_round_trips_plugins`, `wasm_plugins_participate_in_the_manifest_hash`; docs/15 WASM section. B-31 (MySQL) and the remaining decision-gated backlog (RS256, k-anonymity, CTE) follow. The §19 B-30 row and the §26 decision-gate/budget bullets are flipped accordingly.

| ID | Domain | Feature | Status | Priority | Evidence | Dependencies | Complexity | Risk | Reason | Acceptance criteria |
|---|---|---|---|---|---|---|---|---|---|---|
| B-21 | Auth | OAuth2 authorization-server module (Passport-class): authorization code + PKCE, refresh tokens, client registry, revocation; ships as a module, never core; PAT + session stays the default | DONE Wave 8 (applications/oauth2 module: authorization code + PKCE S256, single-use refresh rotation, oauth_clients registry with argon2 secret shown once + redirect allowlist + version CAS, RFC 7009 revoke answering 200 for unknown tokens, PAT access tokens, web-plane authorize behind the CSRF double-submit, form-urlencoded intake branch, oauth-clients console row, 12 HTTP tests; zero new deps) | P2 | 19-PARITY-PROGRAM P5 delivery note; applications/oauth2; 06-SECURITY OAuth2 section; 16-MIGRATION Passport section; 05-DATABASE oauth_clients migration; runtime/keel-api tests/http_integration.rs oauth matrix | v1.0 stable; module ABI gate | High | High (security surface) | third-party API integration; parity commitment | Module registers via ABI gate; code+PKCE/refresh/revocation flows HTTP-tested; zero-new-dep build on existing crypto (sha2/argon2/SignedUrls pattern) or recorded D-62 amendment; 06-SECURITY + 16-MIGRATION updated |
| B-22 | Auth | JWT stateless interop add-on (non-default, report R5): opt-in verification for bearer JWTs with expiry + revocation story (jti denylist or short TTL) | DONE v1.1 (opt-in HS256 verifier behind app.jwt + non-empty auth.jwt_secret; PAT-first bearer resolution with JWT fallback; alg pinned HS256, exp enforced, uniform 401 on every failure; zero new deps over sha2; PAT/session default unchanged; revocation rides short TTL + jti hygiene; RS256 budget-gated) | P3 | R5 recommendation; TokenService; sha2 already a direct dep | none | Medium | Medium | interop with stateless ecosystems without changing the default model | app.jwt flag opt-in; HS256 zero-new-dep over sha2; RS256 only with a recorded budget decision (transitive ring promotion); PAT/session default unchanged; integration tests; docs |
| B-23 | Auth | Email verification HTTP flow: /auth/email-verification request+confirm over existing single-use TokenPurpose::Verify + notification delivery; configurable gating for unverified principals | DONE v1.1 (endpoints + gate + durable store column + tests; mail delivery rides dev_token until notification wiring) | P2 | crates/security/tokens/src/lib.rs (TokenPurpose::Verify); reset.rs flow pattern; mail adapter | notifications/mail adapter | Medium | Low | parity commitment (email verification EQUIVALENT future); account trust | request/confirm endpoints enumeration-resistant; expiry enforced; single-use redemption; unverified gating configurable; tests; 06-SECURITY updated |
| B-24 | Ecosystem | Package registry + discovery: registry index, keel pkg:list/add, manifest + API-ID compatibility validation at graph compile, fail-closed | DONE Wave 8 (root packages.toml index with deny_unknown_fields; keel pkg:list/pkg:add; graph-compile discovery merged through BOTH existing copies into the fail-closed GRAPH gates; route-cache manifests_hash pin covers the index + every indexed manifest; boot [config] merge under the config file; MODULE_ABI stays 1) | P2 | 19-PARITY-PROGRAM P3 delivery note; 12-CLI pkg rows; 15-EXTENSION; 16-MIGRATION Composer-discovery row; S10 module ABI gate (module.toml [module].api, MODULE_ABI=1) | continued ABI contract stability | High | Medium | third-party extension at scale without framework edits | third-party manifest-only package installs and registers routes/commands/config; graph-compile validation fails closed; publishing story documented |
| B-25 | Ecosystem/DX | Package resource publishing (vendor:publish-class): keel publish materializes a package's migrations/config/assets into the host app | DONE Wave 8 (keel publish + keel config:publish over one root-parameterized publish_at engine: migrations name-prefixed after the sqlx integer version, deterministic config bytes to config/packages/<name>.toml, assets to public/vendor/<name>/, pkg:: view namespaces wired at boot; byte-compared idempotence + conflict report, --force overwrites, exit non-zero whenever a conflict was skipped) | P3 | 19-PARITY-PROGRAM P4 delivery note; 12-CLI publish rows; 15-EXTENSION; parity matrix row (vendor publish) | B-24 | Medium | Low | packages need a first-class way to ship resources | publish is idempotent with conflict reporting; tested against a sample package; 15-EXTENSION updated |
| B-26 | Web | SSR/template-engine decision gate (v2.0): adopt a template engine (layouts/components/escaping/compiled cache) dual-mode with the SPA story, or formally defer again | DONE Wave 2 (template engine `crates/web/views` + HTML routes + `keel view:cache`/`view:clear`; directive ledger + G17 facets green; parity rows flipped) | P2 (decision) | sec.25.3 views rows; 19-PARITY-PROGRAM Wave 2 (B-26 moved earlier) | zero-new-dep (no budget amendment needed) | decision Low; impl High | Medium | server-rendered Laravel-class web story | decision doc with budget impact; if adopted: engine crate passes G17 facets, dual-mode starter, parity rows updated; if deferred: rationale recorded |
| B-27 | Observability | Profiling workflow (D-68 remainder): documented CPU/alloc flamegraph path for keel-api + keel-worker | DONE Wave 4 (`keel profile api\|worker`: builds the package, then `samply record target/debug/<bin>`; missing samply fails closed with `cargo install samply` + the docs/13 workflow — test `profile_cli_parses_and_missing_samply_fails_closed`; out-of-process, zero workspace deps) | P3 | D-68 decision; 13-OPERATIONS Profiling section; tools/keel-console profile | none (samply runs out-of-process: zero workspace deps) | Medium | Low | performance investigation workflow | flamegraph workflow documented and runnable for both runtimes; no runtime impact when not attached; optional keel profile wrapper |
| B-28 | Testing | Browser/E2E smoke kit: Playwright-based smoke suite against the starter template (external tooling, zero workspace deps) | DONE Wave 4 (templates/starter-template/e2e/: package.json + playwright.config.ts + auth.spec.ts (register/login/me + bad-password envelope) + notes.spec.ts (create/list roundtrip + anonymous refusal); rendered with the template; CI-optional, executed out-of-band on external tooling) | P3 | parity matrix browser/E2E row; templates/starter-template/e2e; 11-TESTING Playwright section | starter kit | Medium | Low | user-facing regression confidence | starter smoke suite covers auth + one domain flow; documented in 11-TESTING; CI-optional job |
| B-29 | DX | keel shell (Tinker-class): typed interactive console over the app graph/stores under the caller principal | DONE v1.1 (`keel shell` spawns `keel-api --shell` per dep-rules topology; REPL over the production boot: login (argon2, same as POST /auth/login), whoami, ops, routes, run <module.operation> [json] through the CommandBus under the caller principal; capability denial enforced by the policy engine and tested (denied run writes nothing); prod refused unless KEEL_SHELL_ALLOW=1; Wave 4 adds `caps` (declared catalog + session grants) and `models` (binding/probe registries) — test `caps_and_models_introspect_grants_and_registries`) | P3 | sec.19 B-16 | model runtime (S8) | Medium | Low | exploration/ops developer experience | REPL executes operations through the engine under a principal; capability denial enforced and tested |
| B-30 | Ecosystem | WASM plugin runtime decision gate (v2.0): sandboxed third-party modules over the capability ABI | DONE (Wave 9: `keel-wasm` over wasmtime - the one recorded D-62 amendment + `Apache-2.0 WITH LLVM-exception` in deny.toml; sidecar `[[wasm]]` manifests through the same GRAPH gates, guest-manifest drift refusal, fuel+memory caps with `KER_WASM_FUEL` -> 503, no `sock_*` imports (link-time refusal), `WasmHandler` readonly-only so capability checks never self-elevate; facet suite in keel-api wasm_tests + registry, docs/15 extension section) | P3 (decision) | DoD v2.0; D-69 roadmap | B-24; scope + budget decision | Very High | High | sandboxed third-party code without native compilation | decision doc first; if adopted: minimal wasm host passes G07 and the capability ABI; budget amendment recorded |
| B-31 | Data | Multi-database driver decision gate (v2.0): second first-class driver behind the existing store ports | DELIBERATELY EXCLUDED v1 (D-62); DoD v2.0 roadmap | P3 (decision) | D-62; DoD v2.0 | D-62 amendment | Very High | High | ecosystem expansion | decision doc first; if adopted: second driver passes the full store matrix (CRUD/tenant/transactions); migrations remain SQL-first |
| B-32 | Async/Queues | Durable batch ledger: persist batch records (members, per-item state, terminal outcome) in the job store so batch status/cancel/retry and `queue:retry-batch` can operate; today BatchRuntime is in-memory, so `queue:prune-batches` reports an honest empty run and `queue:retry-batch` fails closed | DONE Wave 6 P9 (`job_batches` durable ledger: `BatchLedger` + `MemoryBatchLedger`/`PgBatchLedger` over `20261001000002_job_batches.sql`; `BatchOps::{status,cancel,retry}` over `JobStoreAdmin::get`; capability-gated `GET/POST /api/v1/batches/{id}[/cancel\|/retry]`; `queue:retry-batch`/`queue:prune-batches` run the real ledger — tests jobs +3 / keel-api +3 / console +1 / pg +1, `pg_batch_ledger_roundtrip` green live vs PostgreSQL 16) | P2 | docs/19 Wave 6 P9 delivery note (bullet 304); async/jobs `BatchOps`/`BatchLedger`; keel-api `batch_routes.rs`; postgres `batchledger.rs` + `20261001000002_job_batches.sql`; main.rs `queue_retry_batch`/`queue_prune_batches` | none (zero-new-dep: existing job store + `keel_migrations`-style bookkeeping pattern) | Medium | Medium | batch parity without a persistence story | batch rows durable across restarts; `queue:retry-batch` requeues failed members; prune ages records; pg-gated round trip |
| B-33 | Auth | Admin-role model: an explicit operator/admin role for the default principal; today `keel call` falls back to the first active principal (dev/test only) and production requires an explicit `--principal` | MISSING (Wave 5 shipped the dev/test first-active default with the honest posture) | P2 | Wave 5 delivery note; runtime/keel-api/src/call.rs `operator_resolution_picks_first_active_and_rejects_unknown_explicit` | B-21-class role model groundwork; module principal registry | Medium | Medium | production CLIs/scripts need a named operator, not a scan order | role-resolved operator; dev/test default documented; prod refuses to guess; tests + 06/12 docs updated |

### 26.1 Sequencing proposal

- **v1.1 - zero-dep trust builders:** B-23 (email verification), B-22 (JWT HS256 add-on),
  B-29 (keel shell). All three build on landed primitives (TokenPurpose::Verify, sha2, model
  runtime) with no dependency-budget impact.
- **v1.2 - ecosystem + auth expansion (all five delivered inside the v1.0 program):** B-21
  (OAuth2, Wave 8 P5), B-24 (registry/discovery, Wave 8 P3), B-25 (publishing, Wave 8 P4),
  B-27 (profiling, Wave 4), B-28 (browser smoke, Wave 4). B-21 was the headline item and the
  most security-sensitive; it landed only after the ABI contract had a release of stability
  behind it (module ABI gate first, then Wave 8).
- **v2.0 - decision gates:** B-26 (SSR), B-31 (multi-DB); B-30 (WASM) passed its gate and
  shipped in Wave 9 (wasmtime recorded as the D-62 amendment). Each remaining gate opens with a
  written decision + fresh budget case; implementation starts only after the gate passes. This
  matches the Definition of Done v2.0 row (WASM plugins and multi-database subject to a new
  scope/dependency decision).

### 26.2 Dependency-budget ledger for v1.x

- D-62 as of v1.0: exhausted (cap 25; slot spent on the S10 redis client).
  Wave 9 amendment: the last free slot (25/25) was recorded by docs/19 and
  spent on wasmtime (B-30); the table is full again.
- Zero-new-dep items: B-22 HS256 (sha2), B-23/B-21 (existing crypto + token patterns),
  B-27 (out-of-process samply), B-28 (out-of-process Playwright), B-29 (pure Rust),
  B-32 (job-store bookkeeping pattern), B-33 (role model over existing principals).
- Decision-gated candidates requiring amendment or removal if adopted: RS256 JWT (promote
  transitive ring to direct), any template engine (B-26), second database driver (B-31).
  (B-30 wasm was one of these; Wave 9 adopted it with the recorded wasmtime amendment —
  `Apache-2.0 WITH LLVM-exception` added to the deny allowlist.)
- Wave 7 (testing + security parity): zero new dependencies — the testkit's workspace
  additions (`keel-http`, `keel-session`, `keel-views`, `keel-models`) are workspace-internal;
  D-62 untouched (publish count stays 61).

### 26.3 Permanent exclusions (not backlog)

Per section 22 policy, these are design decisions, not deferred work, and must not re-enter the
backlog without an explicit strategy change: facades/global helpers, a Collection wrapper,
MongoDB, JWT as the default auth model, Cashier/Socialite/Dusk/Valet core clones, and a package
marketplace before the extension contract stabilizes.
