# Keel — Complete Laravel-Competitor Program v2: Web Plane, Eloquent-Class Models, Artisan Surface, Services, Testing, Ecosystem, v2.0 Gates

**Status:** Program specification v2 (2026-08-25). Supersedes the v1 program document ("Keel — Complete Laravel-Competitor Program: HTTP Core, Parity Residuals, Ecosystem, v2.0 Gates", four waves). Where v1 and v2 disagree, this document governs.

**Evidence rule:** Every "landed" claim below is tied to a repository path verified on 2026-08-25. Every new item is decision-complete: exact manifest fields, gate IDs, command syntax, and error codes are specified so an implementer makes no design decisions.

**Budget posture:** Zero new dependencies across Waves 1–8 — the cron parser, route-constraint grammar, pluralization subset, and the `manifest.json` reader are hand-rolled. Exactly one recorded amendment remains in the program: wasmtime (Wave 9, D-62 amendment). MySQL rides an sqlx feature flag (no new direct crate). Items that would break the budget (RS256 JWT, network password checks, CTEs) are decision-gated in Wave 9. `medicare-kernel` untouched. All manifest changes are additive (serde defaults) → `MODULE_ABI` stays 1 through Wave 8.

**Policy carry-over:** §22 deliberate exclusions remain excluded: Laravel `Collection` wrapper, facades/global helpers, Cashier/Socialite/Dusk/Valet core clones, MongoDB. Blade's arbitrary PHP eval and regex-based route constraints are deliberately SUPERSEDED (documented divergences).

## Summary

One sequenced program closing every genuine gap between Keel v1.0 and Laravel-class (and beyond), in nine dependency-ordered waves, each with its own exit gate. Verified against code, not stale doc rows: the graph router, cookies, HTTP auth flows, durable sessions, ORM basics, uploads, named routes/signed URLs/redirects/flash, metrics, `.env` loading, JWT HS256 interop, email verification, and — since the v1 program was written — the entire registry half of v1 Wave 1 (`RouteSpec` middleware/public/deprecated/response/bindings/resource fields, `[[validation]]`, GRAPH gates) have landed and are excluded. In scope:

- **Wave 1 — HTTP core completion:** root `middleware.toml` discovery file + `GRAPH-MW-UNKNOWN` gate, per-route middleware chains at the dispatch edge, manifest FormRequest validation wiring, route model binding (`GRAPH-BINDING-RESOLVER`), pagination injection, `TypedHandler` adapter, `keel route:cache`/`route:clear`, OpenAPI constraint emission.
- **Wave 2 — Routing completion + web plane + views:** api/web route planes, root route files (`routes/web.toml`, `routes/api.toml`, `routes/console.toml`, `routes/channels.toml`), groups/prefix/domains, parameter constraints, singleton/fallback/ANY routes, URL defaults + one-time signed URLs, the template engine with the full directive ledger (`crates/web/views`), HTML routes, error rendering (views + RFC-7807), request/response layer, static assets.
- **Wave 3 — Eloquent & query parity:** finders/mutations, relation queries, ManyToMany/HasManyThrough/polymorphic runtimes, casts registry, visibility control, soft-delete runtime, optimistic locking, strict mode, pruning, dirty tracking; the full query-builder surface; typed API resources.
- **Wave 4 — Validation expansion + config + cache + parity residuals:** ~45 new validation rules, `bail`, dot-paths, custom rules/messages; `FrozenConfig` + config commands; cache semantics + tags + locks + stampede protection; remember-me; `keel about`; API versioning; `keel shell`; profiling; Playwright smoke kit; password-confirm flow.
- **Wave 5 — Command & generator surface:** the full artisan-class CLI matrix (~35 new commands across waves), migrate rollback family with advisory-lock guarding, queue prune/restart/monitor family, `optimize`/`optimize:clear`, `schedule:work`, `storage:link`, `keel test`, `keel call`; codegen kinds 15 → 27; console routes.
- **Wave 6 — Services parity:** notifications v2, Mailable, storage disks, i18n, cron scheduler, broadcast channel auth, advanced queues, HTTP-client parity.
- **Wave 7 — Testing + security parity:** assertion-bearing testkit fakes, the full assertion matrix, time travel, transactional tests, factory runtime; security headers, rehash-on-login, password policy, signed-URL/cookie/CSRF hardening.
- **Wave 8 — Foundation helpers + ecosystem:** `Str`/`Number`/date helpers; package registry + discovery; `keel publish`; OAuth2 module; decision gates (JSON:API, Precognition, ops dashboard).
- **Wave 9 — v2.0 gates:** WASM plugin runtime (wasmtime amendment), MySQL driver, advanced decision-gated backlog.

## Wave mapping (v1 → v2)

| v1 item | v2 destination | Note |
|---|---|---|
| v1 Wave 1 — HTTP core | Wave 1 | Registry half already landed; dispatch/cache residuals carried |
| v1 Wave 3 — B-26 template engine + HTML routes | Wave 2 | Moved earlier: Wave 6 mail templates and the Wave 8 dashboard depend on views |
| v1 Wave 2 — parity residuals + DX | Wave 4 | Carried verbatim + validation/config expansion |
| v1 Wave 3 — B-24 packages, B-25 publish, B-21 OAuth2 | Wave 8 | Carried verbatim + helpers + decision gates |
| v1 Wave 4 — B-30 WASM, B-31 MySQL | Wave 9 | Carried verbatim + advanced backlog |

## Landed and excluded (verified against code, 2026-08-25)

Landed in earlier stages (S7–S10, v0.9 hardening, v1.1) and therefore out of scope here:

- Graph → HTTP router; cookies; HTTP auth flows (register/login/logout/me/password-reset); PgSessionStore — `runtime/keel-api/src/graph_routes.rs`, `runtime/keel-api/src/auth_routes.rs`, `crates/web/http/src/session_mw.rs`, `crates/data/postgres`.
- ORM basics: mass assignment, basic casts, scopes, accessors/mutators, observers, BelongsTo eager load, N+1 detector — `crates/data/models/src/runtime.rs`; Insert/Update/Delete/Upsert builders — `crates/execution/query`.
- Hardened multipart uploads — `crates/web/http/src/upload.rs`; named routes + `GRAPH-DUP-ROUTENAME`, signed URLs, redirects, flash (S9).
- `.env` loader; JWT HS256 opt-in verifier (B-22); email verification flow (B-23); flag-gated `/metrics`; `keel serve`; pg-tests matrix (19 tests, 15 against live PostgreSQL 16).
- **v1 Wave 1 registry half (landed after the v1 program was written):** `RouteSpec` fields `middleware`, `public`, `deprecated`, `response`, `bindings`, `resource`, `only`, `except`, `name`; `ValidationSpec { operation, field, rules }`; `BindingSpec { param, model }`; gates `GRAPH-DUP-ROUTENAME`, `GRAPH-RESOURCE-OP`, `GRAPH-BINDING-PARAM`, `GRAPH-VALIDATION-OP`, `GRAPH-VALIDATION-RULE` — `crates/application/registry/src/lib.rs`. `middleware_catalog` module — `crates/web/http/src/lib.rs`. `MockHttpClient` recording fake — `crates/services/http-client/src/lib.rs`.

## Wave 1 — HTTP core completion

**Registry & manifest (`crates/application/registry/src/lib.rs`) — LANDED**
- `RouteSpec` serde-defaulted fields: `middleware: Vec<String>`, `public: bool` (false), `deprecated: bool` (false), `response: "json"|"html"` (json), `bindings: Vec<BindingSpec { param, model }>`, resource inputs (`resource = "<name>"` with optional `only`/`except` verb lists instead of method/path/operation), and `name` — all landed.
- Resource expansion at parse time → 5 RouteSpecs named `<name>.{index,show,store,update,destroy}`: `GET /<name>` → index, `GET /<name>/{id}` → show, `POST /<name>` → store, `PUT+PATCH /<name>/{id}` → update (two specs, one operation), `DELETE /<name>/{id}` → destroy; `GRAPH-RESOURCE-OP` collect-all — landed.
- `[[validation]]` entries `{ operation, field, rules }` with the rules string grammar mapping 1:1 to the existing `Rule` enum; `GRAPH-VALIDATION-OP`/`GRAPH-VALIDATION-RULE` — landed. The grammar itself grows in Wave 4.
- REMAINING gates from the v1 spec: `GRAPH-MW-UNKNOWN` (middleware alias/group not present in the catalog) and `GRAPH-BINDING-RESOLVER` (no registered resolver for a binding's model) — fail-closed, collect-all, boot-refusing, same reporting as existing GRAPH-*.

**Middleware model (`crates/web/http`, new root `middleware.toml`)**
- The `middleware_catalog` module is landed; remaining: the discovery file `middleware.toml` at the same discovery root as `applications/`. Schema: `[aliases]` (name → builtin id), `[groups]` (name → ordered alias list), `[throttle.<group>]` `{ capacity, refill_per_sec }`.
- Builtins extracted from today's hardcoded dispatch behavior: `auth` (session cookie → bearer PAT → optional JWT; installs `ResolvedPrincipal { principal, tenant, via }` into request extensions; 401 envelope otherwise), `verified` (email-verified gate; 403 `AUTH_DENIED`), `throttle:<group>` (per-group token buckets reusing the existing client-key logic: bearer > `x-rate-key` > IP).
- Implicit chain for tenant-plane graph routes: group `api` = `["throttle:api", "auth", "verified"]` (first = outermost), applied unless `public = true` (which strips `auth` + `verified` only). Explicit `middleware = [...]` entries resolve alias-first then group (flattened in place) and run after the implicit chain. Global order frozen: metrics → CSRF → admission (request-id → trace → body-limit → CORS → maintenance → rate-limit) → session → router; named middleware is strictly innermost. Modules cannot define custom middleware in this program (documented follow-up; Wave 5 ships a `middleware` codegen kind for builtin registration).

**Dispatch edge (`runtime/keel-api/src/graph_routes.rs`, `lib.rs`)**
- `graph_router` resolves each route's full chain at registration and attaches it to the `MethodRouter` via layers; `RouteDispatch` additionally carries the compiled `Validator` and binding specs.
- `graph_dispatch` slimmed: principal/tenant/verified logic deleted (now middleware); dispatch reads `ResolvedPrincipal` from extensions — anonymous reach is only possible on `public` routes, and handlers demanding a principal then 401. Path/query merge, `Idempotency-Key`, envelope semantics unchanged.
- Model binding: before merge, each `{param}` with a declared binding resolves via a new well-known registry service `route.bindings` (model name → tenant-scoped `ModelStore::find` resolver) that modules register in `Provider::init`; appointments registers one for `Appointment` as reference. Miss or cross-tenant → 404 envelope. Resolved record injected as `_bindings.<param>`; raw param still merges. Missing resolver → `GRAPH-BINDING-RESOLVER` at boot.
- Pagination: GET routes parse `page`/`per_page` with existing `PageQuery` (clamped to `MAX_PER_PAGE`) and inject `_page`; `appointments.list` adopts `Page` meta as reference. Deprecated routes emit `Deprecation: true` header + meta.
- `TypedHandler` adapter (in `keel-command`): bridges `RequestViewModel`/`ResponseViewModel` impls onto `OperationHandler` so controllers get compile-time typed input/output instead of raw `Value`. Appointments `BookHandler` migrated as reference; existing JSON handlers remain valid (opt-in).

**Route cache (`tools/keel-console`, boot path)**
- `keel route:cache` writes `storage/framework/routes.json`: resolved chains, validators, bindings, manifest content hash, `MODULE_ABI`. `keel route:clear` deletes it.
- Boot: graph always recompiles (fail-closed, stricter than Laravel); hash mismatch vs artifact → refuse boot with "run route:cache" error; match → register from artifact; absent → live compile (today).

**OpenAPI**
- `keel sdk:openapi` emits `required`/`min`/`max`/`one_of` constraints from `[[validation]]` entries.

## Wave 2 — Routing completion, web plane, views, request/response, errors, assets

**RouteSpec additions (`crates/application/registry/src/lib.rs`)**
- New serde-defaulted fields: `plane: "api"|"web"` (default `"api"`), `group: Option<String>`, `constraints: Vec<ConstraintSpec { param, kind, pattern }>`, `singleton: bool`, `fallback: bool`, `excluded_middleware: Vec<String>` (withoutMiddleware equivalent — may only name aliases present in the route's resolved chain, otherwise `GRAPH-MW-UNKNOWN`), `view: Option<String>`; `method = "ANY"` matches every method; GET routes auto-register HEAD + OPTIONS.
- `singleton = true` expands to show/update/destroy without `{id}` (Laravel `Route::singleton` family — Keel's verb set matches the `destroyable` variant, minus the `edit` form page, which is declared as an explicit HTML route), named `<name>.{show,update,destroy}`.
- New fail-closed gates (collect-all, boot-refusing): `GRAPH-PLANE` (plane value outside {api, web}), `GRAPH-GROUP-UNKNOWN` (route references an undeclared group), `GRAPH-CONSTRAINT-PARAM` (constrained param not in the path), `GRAPH-CONSTRAINT-KIND` (kind outside the closed set), `GRAPH-FALLBACK-DUP` (more than one `fallback = true` route per plane).

**Root route files (new discovery-root inputs, mirroring Laravel's `routes/*.php`)**
- `routes/web.toml` and `routes/api.toml`: `[[route]]` entries (same shape as module `[[routes]]`) merged into the graph alongside module manifests; plane forced by file. Entries must reference operations registered by modules — unknown operation → `GRAPH-ROUTEFILE-OP` (collect-all, boot-refusing).
- `routes/console.toml`: `[[command]] { name, operation }` entries register console operations executed by `keel call` (Wave 5).
- `routes/channels.toml`: `[[channel]] { name, capability }` entries feed Wave 6 broadcast auth; unknown capability → `GRAPH-CHANNEL-CAP`.

**Groups & domains**
- `[[route_groups]] { name, prefix, middleware, plane }` in module manifests or root route files; member routes set `group = "<name>"` and inherit prefix + middleware + plane (route-level fields win on conflict).
- `domain`/`subdomain` fields on route entries: host matching runs before `auth`; `subdomain` drives a tenant lookup feeding tenant resolution (documented multi-tenant web-hosting pattern).

**Parameter constraints (Laravel `where*` equivalents)**
- Closed zero-dep kinds: `number`, `alpha`, `alpha_num`, `uuid`, `ulid`, `slug`.
- Optional `pattern` mini-grammar, hand-interpreted (**no regex crate** — absent from `Cargo.lock`): literal chars, char classes `[a-z]`/`[A-Z0-9]` (incl. ranges), repetition `{n,m}` on the previous element. Arbitrary regex constraints stay deliberately SUPERSEDED — documented as the `where`-regex divergence.
- Constraint mismatch → the route does not match → 404; tested per kind.

**URL generation**
- URL default parameters for `resolve_url` (e.g., `locale`): registered defaults apply when a param is omitted; explicit values win.
- Temporary signed URLs: TTL verified at resolution; new one-time redemption variant (single-use signed URL; redemption recorded via the token store).
- `keel route:list` gains `--name <filter>`, `--path <filter>`, `--plane api|web`; output adds plane, group, version, deprecation columns.

**Views engine (`crates/web/views`, new crate, zero-dep — carried from v1 Wave 3 B-26)**
- Interpreted-AST engine (no codegen), Blade-familiar syntax over a JSON context; deliberately supersedes Blade's arbitrary PHP eval — recorded in the parity matrix vocabulary.
- Directive ledger (complete): `@extends`/`@section`/`@endsection`/`@yield`/`@show`/`@parent`; `@include("partial", {data})`/`@includeIf`/`@includeWhen`; `@component("name")` with `@slot("s")`…`@endslot` and `@props`; `@if`/`@elseif`/`@else`/`@endif`/`@unless`/`@isset`/`@empty`; `@auth`/`@guest`/`@env("dev")`/`@production`; `@foreach`/`@endforeach`/`@for`/`@endfor`/`@forelse`/`@empty`/`@endforelse`; `@once`; `@push("stack")`/`@stack("stack")`; `@csrf`/`@method("PUT")`; `@error("field")`/`@enderror`; `@lang("key")`; `@js(expr)` (JSON-escaped inline); attribute helpers `@class`/`@style`/`@checked`/`@selected`/`@disabled`/`@readonly`/`@required`.
- Expressions: `{{ expr }}` auto-escaped (dot paths + filters, e.g. `{{ user.name | upper }}`); `{!! expr !!}` raw.
- Deliberately SUPERSEDED (documented, not implemented): `@php` and every arbitrary-eval directive; `@while` (covered by `@foreach` over ranges).
- Extension points: custom directives + `Blade::if`-style custom conditionals via a directive registry; package view namespaces (`pkg::view` resolves under the package's view root, Wave 8); view hooks (composers equivalent) as registered per-view callbacks invoked before render.
- Compiled cache: parsed AST serialized to `storage/framework/views/`, invalidated by template mtime + content hash; `keel view:cache`/`view:clear`; `[views]` config section (paths, cache dir); escaping/security test facet per G17.

**HTML routes**
- Routes with `response = "html"` render the `view` with the operation result as context, `Content-Type: text/html`; closes the "HTML responses MISSING" parity rows. Starter template gains one SSR page alongside the SPA shell; S9 SPA-first remains the default, SSR is opt-in.
- Web-plane envelope: errors on web routes render the error views below instead of JSON envelopes; `expects_json` requests still receive the envelope.

**Error rendering**
- Typed error → HTTP status mapping registry (`AppError` kind → status, extensible); `reportable` vs `renderable` split (reportable flows to observability; renderable carries a render path).
- Custom renderer registration per error type (modules register renderers in `Provider::init`).
- Error views: `errors/404`, `errors/500`, `errors/maintenance` (web plane); RFC-7807 `application/problem+json` output option for the api plane alongside the existing envelope (per-deployment opt-in).
- Dev-only debug detail gate: stack/context detail renders only when `app.debug = true`; secrets never rendered (reuses the logging redaction list).

**Request/response layer**
- Request helpers: `expects_json()`, `bearer_token()`, `client_ip()`, `fingerprint()`, `has_valid_signature()`, `accepts(...)`.
- Response kinds: json (default envelope), html (view), redirect (with flash), `download`/`file` (from StoragePort objects), `stream` (chunked body), `no_content` (204).
- HTTP caching: per-route cache-control builder + ETag generation with conditional-GET (304) handling for GET routes.
- Cookies: queued cookies on responses + optional cookie encryption (AES-GCM, reuses the sealing primitive) toggle.
- Terminating hooks: post-response callbacks run after the response is sent (metrics/close-span safe).

**Static assets**
- Path-traversal-safe static file middleware for `public/` on the web plane (canonicalize + prefix check; traversal → 404).
- Immutable cache headers for fingerprinted asset paths; zero-dep `manifest.json` (Vite-build) reader + asset URL helper usable from views; SPA shell remains the default web story.

## Wave 3 — Eloquent & query parity + API resources

**Finders & mutations (ModelRuntime, `crates/data/models/src/runtime.rs`)**
- New operations: `first_or_create` (find by predicates → else create attrs), `first_or_new` (find → else unsaved record), `update_or_create` (find by predicates → merge attrs), `force_create` (bypasses mass-assignment guard), `create_many` (batch insert through the runtime: mutators, casts, observers once per record), `insert_get_id` (Insert + `RETURNING id`), `insert_or_ignore` (`ON CONFLICT DO NOTHING`), `update_or_insert` (Upsert builder via the runtime), `increment`/`decrement` (atomic `SET col = col ± n` with optional extra-column updates), `replicate` (clone minus except-fields, new id, optional relation detach), `refresh` (re-hydrate in place), `fresh` (re-fetch as new record), `load_missing` (load only unloaded relations), `touch` (update timestamps), `push` (save record then its loaded relations).
- `find` miss → typed `AppError::not_found` (drives the Wave 2 error registry → 404 render). Phase 3 delivery note: `find` itself keeps `Ok(None)` because the Wave 1 route-binding contract maps a miss to a 404 at the edge (`find_for_binding`, `Ok(None) = miss`); the typed error path is `get` (id) and `first_or_not_found` (predicates), both `KER_NOT_FOUND`.
- Every operation runs the existing pipeline: mass assignment → mutators → casts → scopes → store → observers; tenant scoping enforced in every path.

**Relation query surface**
- Aggregate eager: `with_count`, `with_sum`, `with_min`, `with_max`, `with_avg` (subquery aggregates attached as `<relation>_<agg>` columns).
- Existence filters: `has`, `doesnt_have`, `or_where_has`, `where_relation`, `where_doesnt_have_relation` — implemented by `Select` gaining `where_exists`/`where_not_exists` subquery composition (correlated via the relation's key columns).
- Nested eager paths (`post.comments.author`) with per-level constraint closures (filter/sort/limit the eager subquery).

**Relation kinds (RelationDef runtime)**
- ManyToMany: pivot table named `<a>_<b>` alphabetical convention (overridable in RelationDef); `attach`/`detach`/`sync` (idempotent diff: insert missing, delete absent, keep rest)/`toggle`/`update_existing_pivot`; pivot attributes supported; optional pivot model.
- HasManyThrough (intermediate join table, no pivot writes).
- Polymorphic: `MorphTo`/`MorphMany`/`MorphToMany` over `<name>_type` + `<name>_id` columns; morph map registry (alias → model) so type strings stay stable across renames.

**Serialization control**
- Per-call overrides on top of the existing `public_record`: `make_visible([...])`, `make_hidden([...])`, `append([...])` (computed accessor values included in output).
- Manifest-declared `hidden`/`visible` field lists on ModelDef enforced at serialization time.

**Casts registry (runtime)**
- Built-in cast kinds: `date`, `datetime`, `decimal:<n>`, `json`, `array`, `enum:<v1,v2,...>` (validation at set time), `encrypted` (AES-GCM via the sealing primitive, stored as base64), `hashed` (argon2 on write, never round-trips).
- Custom casts: `Cast` trait (`to_db`/`from_db`); modules register implementations per model field; codegen `cast` kind (Wave 5) emits the impl skeleton.

**Soft-delete runtime**
- `with_trashed`, `only_trashed`, `restore`, `force_delete`; route model binding gains a `with_trashed` option; global-scope removal API `without_global_scopes` (named or all).
- `deleted_at` semantics reuse the store-level soft delete already proven in the pg matrix.

**Optimistic locking**
- Additive `version_column: Option<String>` in ModelDef; when set, `update` adds `WHERE version = $n` and `SET version = version + 1`; zero rows affected → `AppError::conflict("STALE_RECORD")` → 409 via the error registry.

**Strict mode flags**
- Runtime toggles on ModelRuntime: prevent lazy loading (error `LAZY_LOAD_PREVENTED`), prevent silently-discarded attributes (error on unknown fields in mass assignment instead of drop), prevent accessing missing attributes — each with a unit test asserting the error.

**Pruning & dirty tracking**
- `Prunable` marker: per-model predicate (e.g., `created_at < now() - 90d`); `keel model:prune [--chunk n]` deletes in chunks through the store (soft-deleted models prune hard).
- Dirty tracking on runtime records: `is_dirty`, `was_changed`, `original`; `without_timestamps` override for system updates.

**Query builder additions (`crates/execution/query`)**
- Set ops: `union`/`union_all` (compiled shape tested).
- Grouping: `group_by`/`having` (having supports the same operator set as where).
- Where family: `or_where` + nested boolean groups, `where_like` (ILIKE), `where_between`/`where_not_between`, `where_column`, `where_date`/`where_year`/`where_month`/`where_day`/`where_time`, `distinct`.
- Selection & ordering: `select` subset projection, `latest`/`oldest` (default `created_at`, column-overridable).
- Locking: `lock_for_update` (`FOR UPDATE`), `shared_lock` (`FOR SHARE`).
- Aggregates & existence: `count`, `sum`, `avg`, `min`, `max`, `exists`, `doesnt_exist`.
- Retrieval: conditional `when`/`tap`. Phase 3 note: `pluck`, `value`, `chunk`/`each`, and `cursor` are execution methods — they landed on `ModelRuntime` (store/runtime level) instead of the builder, since `keel-query` compiles but never executes. `chunk`/`each` walk offset pages (200-row cap), `cursor` fetches pages lazily through a `RecordCursor` iterator (documented divergence from Laravel's single-statement stream), `pluck` fetches full rows (no column projection yet).
- `truncate` dev/test-gated (refuses outside dev/test env).
- Raw expressions only via the existing `Ident`-guarded safe API — no free-form SQL string injection path.

**API resources**
- Typed `Resource` transformer trait (JsonResource equivalent): conditional fields (`when`), wrapping key, pagination meta passthrough; `ResourceCollection` for lists.
- Existing `make resource` codegen kind rewired to emit the trait impl; full JSON:API spec server stays a Wave 8 decision gate.

**Port impact**
- `ModelStore` trait gains: aggregate ops (`aggregate`: count/sum/avg/min/max), `exists`, find-by (first match, `find_by`), `find_for_update` (`FOR UPDATE` variant), `insert_or_ignore`, `truncate` (dev/test-gated via a fail-closed `build_gated`/`check_env` token), filter-keyed `update_filtered`/`delete_filtered` (pivot writes; empty filter list refused), and `force_delete` (trash purge / prune — hard-deletes even soft-delete models).
- Phase 3 growth: `increment` (atomic `SET field = field + by` plus prepared extra columns in one statement; the runtime `decrement` signs the step; `validate_counter` — declared, numeric, mutable — shared by both backends) — in-memory, `PgModelStore`, and `Arc<T>` delegation together.
- In-memory `upsert` now preserves the surviving row's `id` on merge (mirrors the pg `exclude_from_conflict_update(["id"])` posture; the `update_or_insert` test caught the backend divergence).
- Tenant-scoped query builders guard every new operation; `PgModelStore` + in-memory implementations updated together.
## Wave 4 — Validation expansion + config + cache + parity residuals

**Validation rule growth (~45 new `Rule` variants, manifest grammar 1:1)**
- Types: `nullable`, `boolean`, `integer`, `numeric`, `string`, `array`, `json`, `ascii`.
- Strings: `alpha`, `alpha_num`, `alpha_dash`, `lowercase`, `uppercase`, `starts_with:<s>`, `ends_with:<s>`, `contains:<s>`, `doesnt_start_with:<s>`, `doesnt_end_with:<s>`, `doesnt_contain:<s>`, `hex_color`, `mac_address`, `timezone`, `ulid`.
- Numeric: `decimal:<n>`, `digits:<n>`, `digits_between:<a,b>`, `multiple_of:<n>`, `gt:<n|field>`, `gte:<n|field>`, `lt:<n|field>`, `lte:<n|field>` (field refs resolve other validated values).
- Conditionals: `required_if:<f,v>`, `required_unless:<f,v>`, `required_with:<f>`, `required_without:<f>`, `accepted`, `accepted_if:<f,v>`, `declined`, `declined_if:<f,v>`, `prohibited`, `prohibited_if:<f,v>`, `prohibited_with:<f>`, `exclude`, `exclude_if:<f,v>`, `missing`, `missing_if:<f,v>`, `missing_with:<f>`, `sometimes`, `filled`, `present`.
- Cross-field: `same:<f>`, `different:<f>`, `confirmed` (matches `<field>_confirmation`), `in_array:<f>`, `distinct` (array-unique).
- Dates: `date_format:<fmt>` (closed format tokens: `Y`,`m`,`d`,`H`,`i`,`s`), `before:<d>`, `after:<d>`, `before_or_equal:<d>`, `after_or_equal:<d>` (`<d>` is a literal date or the keyword `now`, or another field name).
- Files (validated against the upload pipeline's part metadata): `file`, `image`, `mimes:<a,b>`, `mimetypes:<a,b>`, `extensions:<a,b>`, `dimensions:<w,h,min_w,max_w,min_h,max_h>` (subset parse of the declared keys).
- Auth: `password` (composable sub-rules: `min:<n>`, `letters`, `numbers`, `symbols`, `mixed_case`), `current_password` (verifies against the authenticated principal's verifier).
- Database: `unique:<model>[,ignore_id]`, `exists:<model>[,where_col=val]` — resolved through the ModelStore probe under the request tenant; DB errors fail the request closed (503 envelope), never pass validation.
- `bail` prefix rule: stop evaluating the field on first failure.
- New fail-closed gates: `GRAPH-RULE-UNKNOWN` (token outside the grammar), `GRAPH-VALIDATION-FIELD` (field path not in the operation's declared input shape when the shape is declared).
- Custom rules: modules register `Rule` trait objects by name; manifest references them via `custom = "<name>"` in `[[validation]]` entries; unknown custom name → `GRAPH-RULE-UNKNOWN`.
- Nested dot-path validation (`items.0.name`, `items.*.name` with wildcard expansion); custom per-field messages + attribute display names in `[[validation]]` entries; localized messages via Wave 6 i18n keys (`validation.<rule>`).
- **Regex rules deliberately SUPERSEDED** by the closed pattern grammar (documented divergence, same as route constraints).

**Config surface**
- `PlatformConfig::freeze() -> FrozenConfig` (Arc, getters only) consumed by boot; post-boot mutation impossible by type; xtask lint fails CI if `std::env::var` appears outside sanctioned boot paths (closes L02 ◐).
- Config schema gate `GRAPH-CONFIG-SCHEMA`: typed accessors declare defaults; unknown keys in config files rejected at boot (fail-closed report).
- `keel config:cache` writes the frozen effective config to `storage/framework/config.json` + content hash; boot hash-check with drift-refuse semantics identical to route cache. `keel config:clear` deletes it. `keel config:show <key> [--json]` prints one section (secrets redacted). `keel config:publish <package>` lands with Wave 8. `keel env` prints environment summary. `keel cache:forget <key>` removes one key.

**Cache semantics + advanced**
- `CacheService` over the unchanged `CachePort`: `remember(key, ttl, f)`, `remember_forever`, `add` (atomic via `put_if_absent`, returns bool), `forever`, `pull` (get+delete), `many`/`put_many`, `has`, `increment`/`decrement`, `flush` (memory: clear; Redis: `FLUSHDB`). Values serde_json-serialized at the service layer; ports/adapters untouched.
- Cache tags: memory backend = key-prefix namespaces (`tag:{t}:{key}` + registry); Redis backend = SET-backed membership; `tags([...]).get/put/flush`.
- Atomic locks API: `lock(key, ttl)` → `get`/`release`/`force_release` + closure form `lock.get(|| ...)`; built on the existing lock primitive, exposed as a service.
- Stampede protection: `remember` under a per-key lock with jittered wait (single-flight collapse).

**Parity residuals (v1 Wave 2, carried)**
- **Session remember-me (`crates/security/sessions`, `auth_routes.rs`):** login body gains `remember: bool`; true → cookie `Max-Age` = `SessionPolicy.remember_ttl` (default 30d vs 8h) and session bag records `remember: true`; regeneration preserves it; `/auth/me` reports it.
- **Password-confirm flow (new):** `POST /auth/confirm` re-verifies the current password and stamps `confirmed_at` in the session bag; `confirmed` middleware alias (added to the catalog) gates routes requiring fresh confirmation (default window 15 min, config `auth.confirm_timeout_secs`); expiry → 403 `CONFIRMATION_REQUIRED` envelope (web plane: redirect to confirm view).
- **`keel about` (CLI):** Laravel-style report — framework/app version, env, debug, DB driver + server version, cache/queue/session drivers, flag snapshot, module/route/operation counts, dep-budget state. Reads config + compiled graph; zero-dep.
- **API versioning completion (L72 ◐):** `GRAPH-VERSION-SEMVER` gate validates route `version` parses as `v<N>` and matches the path's version segment when present; `deprecated` flag (Wave 1) drives `Deprecation` header/meta; `keel route:list` prints version + deprecation; OpenAPI carries both.
- **B-29 `keel shell`:** zero-dep stdin REPL over the production-equivalent host (`build_app_state`): `:principal <uuid>`, `:tenant <uuid>`, `op <operation> {json}`, `routes`, `caps`, `models`, `exit`. Operations execute through the CommandBus under the chosen principal; capability denial renders the envelope error (tested).
- **B-27 profiling:** `keel profile <subcommand>` wrapper that spawns samply when present (out-of-process, zero-dep) + documented flamegraph workflow for keel-api and keel-worker in `docs/13-OPERATIONS.md`.
- **B-28 Playwright smoke kit:** `templates/starter-template/e2e/` (own package.json, external tooling): auth register/login + notes create/list against `keel serve`; documented in `docs/11-TESTING.md`; CI-optional job.
## Wave 5 — Command & generator surface

**Full artisan-class CLI matrix (new commands, exact syntax)**
- Discovery/diagnostics: `keel about` (Wave 4), `keel env`.
- Optimization: `keel optimize` (= `route:cache` + `config:cache` + `view:cache`, run in that order, abort-on-first-failure), `keel optimize:clear` (= `route:clear` + `config:clear` + `view:clear`).
- Config: `keel config:cache`, `keel config:clear`, `keel config:show <key> [--json]` (Wave 4); `keel config:publish <package>` (lands with Wave 8 publish).
- Cache: `keel cache:forget <key>`.
- Migrations (SQL-first, additive over existing `keel migrate`/`migrate:status`/`migrate:fresh`): `keel migrate:rollback [--step n]` (default step 1: revert last batch in reverse filename order), `keel migrate:reset` (revert everything in reverse order), `keel migrate:refresh [--step n]` (rollback n + migrate), `keel migrate:install` (ensure the migrations bookkeeping table exists without running files), `keel db:schema:dump` (emit a normalized DDL snapshot of the migrated schema to `database/schema.sql` for review), `keel db:show` (driver, server version, database, table + row counts — extends `db:inspect` with server metadata).
- Rollback mechanism: each migration pair ships as `<ts>_<name>.up.sql` + `<ts>_<name>.down.sql` (existing single-file migrations remain forward-only; rollback applies only to pairs — documented divergence). `migrate` gains advisory-lock guarding: `pg_advisory_lock(7259)` acquired before any migration run, fail-closed on timeout (concurrent runs serialize instead of corrupting).
- Documented SUPERSEDED/EXCLUDED CLI rows: `event:cache`/`event:clear` (boot-time graph compile supersedes event discovery caches), `cache:table` (cache driver policy: memory + Redis only), `vendor:publish` mapped to `keel publish` (Wave 8).
- Queues (additive over existing `queue:work`/`queue:failed`/`queue:retry`): `keel queue:clear` (purge pending jobs, dev/test-gated), `keel queue:flush` (purge dead jobs), `keel queue:forget <id>` (delete one dead job), `keel queue:listen` (supervise + auto-restart a worker via spawned child process), `keel queue:monitor` (queue depth + oldest-age snapshot for alerting), `keel queue:prune-batches [--hours n]` (prune terminal batch records, default 72h), `keel queue:prune-failed [--hours n]` (prune dead jobs, default 168h), `keel queue:restart` (write the restart signal file honored by running workers), `keel queue:retry-batch <id>` (requeue every failed member of a batch).
- Scheduling: `keel schedule:work` (daemon loop: tick every 60s, run due items), `keel schedule:test` (dry-run: print what would fire now without executing).
- Storage: `keel storage:link` (create `public/storage` symlink to the local disk root; refuses to overwrite, `--force` replaces).
- Views: `keel view:cache`, `keel view:clear` (Wave 2 engine).
- Testing: `keel test [--pg]` (cargo-test wrapper; `--pg` adds `--features pg-tests` and requires the compose DB; streams pass/fail summary).
- Models: `keel model:prune [--chunk n]` (Wave 3).
- Console routes: `keel call <operation> [json]` — executes a `routes/console.toml` operation through the CommandBus under an operator principal (flag `--principal <uuid>`, defaults to the first admin principal in dev/test only; production refuses without an explicit principal); capability checks enforced.

**Codegen kinds 15 → 27 (`tools/keel-codegen/src/generators.rs`)**
- Existing kinds (unchanged): migration, seeder, factory, job, event, listener, command, query, policy, workflow, model, test, resource, request, controller.
- New kinds: `cast` (Cast trait impl skeleton), `channel` (broadcast channel capability registration stub), `component` (view component template + props struct), `exception` (typed AppError extension + renderer registration), `mail` (Mailable definition), `notification` (NotificationDef with channel renderers), `observer` (ModelObserver impl), `rule` (custom validation Rule impl), `scope` (model scope function), `job-middleware` (job middleware skeleton), `middleware` (builtin middleware registration skeleton), `view` (template file under the module's view root).
- Same placeholder-free template assertions as existing kinds (`generated_code_contains_no_placeholder_macros` test pattern extended to every new kind).

**Console routes**
- `routes/console.toml` entries (`[[command]] { name, operation }`) register as CLI-invokable operations; `keel call` executes them through the CommandBus under the operator principal with capability checks; unknown operation → `GRAPH-ROUTEFILE-OP` (shared gate, Wave 2).
## Wave 6 — Services parity

**Notifications v2 (`crates/services/notifications`)**
- `NotificationDef`: typed notification with per-channel renderers — `to_mail() -> Mailable`, `to_sms() -> text`, `to_push() -> payload`, `to_database() -> row`; modules declare notification types; the existing `NotifyHub` dispatches through the channel ports.
- `notifications` table + migration (`id, principal_id, tenant_id, type, data jsonb, read_at, created_at`); database-channel writes rows; endpoints `GET /api/v1/notifications` (paginated list + unread count meta) and `POST /api/v1/notifications/read` (mark ids read, capability-gated).
- On-demand notifiables: deliver to an email/phone target without a principal record (routing through the same channel ports; audit entry records the target hash, not the raw address).
- Queued delivery: `queue_via = "<job_type>"` registers a delivery job through the job registry; immediate dispatch remains the default.
- HTML mail bodies rendered via Wave 2 views (notification templates under `views/notifications/`).

**Mail (`crates/services/notifications` + `adapters/keel-mail-smtp`)**
- `Mailable` abstraction: subject, from/reply-to, cc/bcc, text + html bodies (view-rendered), attachments from StoragePort objects (streamed, size-capped by the upload limits), headers.
- Queued sends via the job registry (`mail.send` job type); failure → retry/backoff per the job policy, then dead-letter.
- `[mail.mailers.<name>]` config: `{ transport = "smtp"|"log", host, port, ... }` with failover-chain selection (`mail.default` names the chain; first healthy transport wins, health = connection probe with timeout).

**Storage disks (`crates/services/storage`)**
- `[storage.disks.<name>] { driver = "local"|"s3", root|bucket, visibility = "public"|"private", url }` config; disk registry resolves a named disk to a StoragePort adapter instance at boot; unknown driver → boot-refusing config error.
- Upload pipeline (Wave-landed multipart) gains a `disk` parameter (default disk config `storage.default`); stored object records the disk name.
- Directory ops on the port: `list(prefix)`, `make_dir`, `delete_dir` (local + S3 implementations).
- Visibility honored by presigned URL issuance: private objects only get signed URLs; public objects resolve through the `url` base (or the `storage:link` path for local disks).

**i18n (`crates/foundation/core/src/translate.rs` + boot loader)**
- `lang/<locale>.toml` files loaded into the existing `MessageCatalog` at boot; flat keys with dotted nesting (`validation.required = "..."`).
- Locale resolution order: session preference → `Accept-Language` (first supported locale, quality-ordered) → configured default; resolved into a request-scoped context value.
- View integration: `@lang("key")` directive + `trans` filter (Wave 2 engine hooks the catalog).
- Pluralization subset: `trans_choice("key", n)` with CLDR-style buckets (`zero|one|two|few|many|other`), hand-rolled rule table for the supported locales (en + any locale file shipped); unsupported locale falls back to `other`.
- Localized validation messages (`validation.<rule>` keys) and notification strings resolve through the same catalog.

**Scheduler (`crates/async/scheduler`)**
- Zero-dep 5-field cron expression parser (minute hour day-of-month month day-of-week): `*`, lists (`1,15`), ranges (`1-5`), steps (`*/5`, `1-30/2`); invalid expression → `ScheduleItem` registration error at boot. `ScheduleItem.cron: Option<String>` (additive; existing interval items unchanged).
- Builder helpers mapping to cron strings: `hourly()`, `daily()`, `daily_at("13:00")`, `weekly()`, `monthly()`, `every_minutes(n)`.
- Per-item timezone (IANA name, validated against a bundled static table; unknown → registration error); `next_run` computed in the item's zone.
- Guards: `without_overlapping` (cache lock keyed by item name, TTL = max runtime), `on_one_server` (distributed lock via the Redis adapter when configured; silently degrades to local-only lock when single-server).
- Hooks: `before`/`after`/`on_success`/`on_failure` callbacks per item; failures recorded in the scheduler state doc.
- `keel schedule:work` daemon + `keel schedule:test` dry-run (Wave 5 commands consume this).

**Broadcasting (`crates/services/realtime` edge)**
- `routes/channels.toml` (Wave 2) + `[[channels]] { name, capability }` manifest entries; `GRAPH-CHANNEL-CAP` gate.
- `POST /api/v1/broadcast/auth` (session or PAT authenticated): body `{ channel }`; checks the channel's capability for the principal, issues a short-TTL signed channel token (TokenService pattern); the realtime subscription path validates the token at connect.
- Presence channels: `presence-<name>` prefix; member list endpoint `GET /api/v1/broadcast/members?channel=...` (capability-gated); join/leave events broadcast to members.
- Subscription authorization runs host-side per connection; a channel token never grants more than its channel.

**Queues advanced (`crates/async/jobs`)**
- Unique jobs: `unique_key` on job registration; claim guarded by a cache lock (skip-if-locked semantics; configurable `unique_ttl`).
- Rate-limited jobs: token bucket per job type (`{ capacity, refill_per_sec }` config); exhausted → requeue with backoff delay instead of failing.
- Job middleware: composable pre/post hooks on a job execution (retry-limit, rate-limit, unique are built-in middlewares); codegen kind `job-middleware` (Wave 5).
- Batch API operations over the existing `BatchSpec`: `batch.status` (progress counts), `batch.cancel` (pending members → dead), `batch.retry` (failed members → pending); exposed as capability-gated operations + used by `queue:retry-batch`.
- Chain execution over the existing `ChainSpec`: sequential dispatch where member N+1 enqueues only on member N success; failure → chain dead with position recorded.
- `queue:restart` signal file honored by workers (cooperative stop between jobs); prune commands per Wave 5; queue depth/age/throughput counters on `/metrics`.

**HTTP client parity (`crates/services/http-client`)**
- Retries with exponential backoff (`retry { times, base_ms, max_ms, on: [5xx, timeout] }`) on the `ReqwestHttpClient`.
- Per-request timeout override; concurrency pool helper (bounded tokio join-set map over requests).
- Request/response tap hooks (observability + redaction-aware logging through the existing redact config).
- `HttpFake` for tests built on the existing `MockHttpClient` recording (assert request count/method/url/body; Wave 7 wires the assertions).
- Response sink to StoragePort (`sink_to(disk, path)`) for large downloads without buffering the full body in memory.
## Wave 7 — Testing parity + security parity

**Testkit fakes (assertion-bearing, `testing/testkit`)**
- `QueueFake`: records pushed jobs; `assert_pushed(job_type, |j| ...)`, `assert_not_pushed(job_type)`, `assert_pushed_count(job_type, n)`.
- `EventFake`: records outbox event emissions; `assert_dispatched(event_name, |e| ...)`, `assert_not_dispatched(event_name)`.
- `NotificationFake`: records NotificationDef dispatches per channel; `assert_sent_to(principal, notification_type, channel)`.
- `MailFake`: records Mailables; `assert_sent(|m| ...)` matching subject/to/attachments count.
- `StorageFake`: in-memory disk implementing StoragePort; `assert_exists(path)`, `assert_missing(path)`, `assert_directory_empty(prefix)`.
- `HttpFake`: wraps `MockHttpClient` recording; `assert_request_count(n)`, `assert_sent(|req| ...)` (method/url/body), `assert_no_pending()`.
- `CacheFake`: in-memory CachePort with `assert_has(key)`, `assert_missing(key)`, TTL inspection.
- All fakes register through the existing registry/service wiring so handlers under test hit them without code changes.

**Assertion matrix (TestResponse/TestApp, `testing/testkit/src/app.rs`)**
- Web flow: `assert_redirect(location)` (exact or prefix), `assert_session_has(key)`/`assert_session_missing(key)`, `assert_cookie(name, value=None)` (presence or exact), `assert_view_has(key)` (context passed to the rendered view).
- Content: `assert_json_path("a.b", expected)`, `assert_html_contains(fragment)`, `assert_see(text)` (escaped-text search in body).
- Database: `assert_database_has(model, predicates)`, `assert_database_count(model, n)`, `assert_database_missing(model, predicates)` — run through the ModelStore probe under the test tenant.
- Auth: `assert_authenticated_as(principal)`, `assert_guest`.
- Existing `assert_status`/`assert_header`/`assert_json_eq`/`assert_json_contains` unchanged.

**Test infrastructure**
- Time travel: injectable clock on the host (`ClockPort` already threaded where timestamps are taken); `travel_to(datetime)`, `freeze_time()`, `travel(seconds)` helpers for tests; scheduler `next_run`, session TTLs, and token expiry all read from the clock.
- Transactional tests: per-test savepoint rollback helper against live Pg (`BEGIN; SAVEPOINT keel_test; ... ROLLBACK TO SAVEPOINT`) so pg-matrix tests isolate without schema drops; documented as the RefreshDatabase-equivalent suite setup.
- Fake scheduler + fake queue wiring: `TestApp::with_fake_queue()` / `with_fake_scheduler()` swap the stores; `run_due_schedules()` helper executes due items synchronously.
- Factory runtime: `create()` persistence through the ModelStore (returns the stored record), `state(|f| ...)` overrides, deterministic sequences (counter-seeded per test); codegen factory `build()` already real — `create()` is the addition.

**Security parity**
- Security-headers middleware (web plane, config `[security.headers]`): HSTS (`max-age`, `includeSubDomains` opt), `X-Content-Type-Options: nosniff`, baseline CSP (`default-src 'self'` + per-deployment overrides), `Referrer-Policy: strict-origin-when-cross-origin`; api plane unchanged.
- Argon2 rehash-on-login: when the stored verifier's params differ from current `[identity]` config (memory/time/parallelism drift), re-hash the password on successful login and update the verifier (tested with a drifted-params fixture).
- `[auth.password_policy]` config feeding the `password` rule composable (Wave 4): `{ min_length, letters, numbers, symbols, mixed_case }`; register/password-reset flows enforce it.
- Signed-URL hardening: one-time redemption variant (Wave 2) + TTL bounds (`max_ttl_secs` config; over-bound → boot-refusing config error).
- Optional cookie encryption toggle (`cookies.encrypted = true`, AES-GCM reuse); CSRF on-by-default for the web plane (api plane stays token-authenticated, unchanged).
## Wave 8 — Foundation helpers + ecosystem

**Helpers (zero-dep, `crates/foundation/core`)**
- `Str` equivalents: `slug`, `snake`, `camel`, `kebab`, `studly`, `title`, `words` (limit/truncate), `random(len)` (CSPRNG via the existing crypto dependency), `uuid`/`ulid` generation, `excerpt` (context snippet), `plural`/`singular` — hand-rolled English inflection subset (regular rules + irregular table; documented subset, not full inflection).
- `Number`: currency formatting (locale-aware symbol + decimals), percentage, `clamp`, precision formatting — locale-aware through the Wave 6 catalog.
- Date helpers over the existing `time` dependency: parse/format with the closed format-token set (same tokens as the `date_format` validation rule), timezone conversions feeding model casts (Wave 3), `now_in(tz)`.
- A `collect()`-class wrapper stays DELIBERATELY EXCLUDED (§22; Rust iterators are the native model).

**Ecosystem (v1 Wave 3, carried)**
- **B-24 package registry + discovery:** new root `packages.toml` index: `[[package]] { name, version, path }` (local-path packages in v1; git fetch stays out — zero-dep rule). `keel pkg:list` shows index + ABI compatibility; `keel pkg:add <path>` validates the manifest and appends. Graph compile (`graph_routes::discover_manifests` + `keel graph`/`keel check`) merges manifests from `applications/`, `modules/`, and every indexed package path; existing GRAPH gates apply fail-closed. Package v1 contract = manifest + optional workspace crate member; manifest-only packages may declare routes for operations registered by linked crates; packages may ship `[config]` defaults merged under app config at boot.
- **B-25 `keel publish <package>`:** materializes package migrations → `database/migrations/` (name-prefixed), config → `config/packages/<name>.toml`, assets → `public/vendor/<name>/`, views → registered under the `pkg::` view namespace (Wave 2); idempotent with conflict report; `--force` overwrites; `keel config:publish <package>` publishes config only; tested against a sample package; 15-EXTENSION updated.
- **B-21 OAuth2 module (`applications/oauth2`):** ships through the module ABI gate, never core. Capabilities: `authorize`, `token`, `revoke`, `clients.manage`. Endpoints: `GET|POST /oauth/authorize` (session-authenticated, authorization code + PKCE S256), `POST /oauth/token` (`authorization_code` + `code_verifier`, `refresh_token`), `POST /oauth/revoke`. Client registry table `oauth_clients` (secret argon2-hashed, redirect allowlist, scopes); codes/refresh tokens reuse the `TokenService` hashed single-use TTL pattern; access tokens are existing PATs. Migration `20260826000001_oauth2.sql`. Zero new deps (sha2/argon2/SignedUrls patterns).

**Decision gates (P3 — decision doc first; implementation only after the gate passes)**
- **JSON:API spec server:** full `application/vnd.api+json` compound-document server on top of Wave 3 resources (sparse fieldsets, includes, links). Budget-neutral (serialization only). Gate output: decision doc + resource mapping spec.
- **Precognition:** `Precognition: true` request header on graph routes → run `[[validation]]` only and return 204 with no side effects on success, or the standard 422 envelope on failure (live-validation form pattern). Gate output: decision doc + dispatch spec.
- **Ops dashboard (Horizon-class):** read-only dashboard over the inspect/job/scheduler stores using Wave 2 views + HTML routes, flag-gated (`app.dashboard`): queue depth, recent jobs, schedule state, execution inspect. Gate output: decision doc + data-source spec (all data from existing stores; no new collection paths).

## Wave 9 — v2.0 gates + advanced decision-gated backlog

**B-30 WASM plugin runtime (`crates/extension/wasm`, wasmtime — recorded D-62 amendment + deny.toml entry)**
- `WasmHost` loads plugin modules exporting `keel_manifest() -> json` (module.toml-equivalent) and `keel_call(op, input_json) -> output_json` over WASI-preview1 with host imports `host.log` and `host.capability_check(action, path)`.
- Sandbox: no network/DB/sockets (no wasi-http); fuel metering caps instructions; memory limiter caps heap; fuel exhaustion = timeout → 503 envelope.
- Graph integration: `[[wasm]]` entries in `packages.toml`; manifests pass the same GRAPH gates including ABI; plugin operations register as `WasmHandler: OperationHandler` so authorization/idempotency/delta/audit run host-side in the existing engine — a plugin can never elevate its own capability (host checks before dispatch).
- Sample echo/fib plugin passes the facet suite: manifest validation, dispatch roundtrip, capability denial, fuel exhaustion, malformed-manifest boot refusal.

**B-31 MySQL driver (sqlx `mysql` feature — no new direct crate)**
- Second first-class driver behind existing ports: `MySqlModelStore`, `MySqlSessionStore`, `MySqlJobStore` (`FOR UPDATE SKIP LOCKED`, MySQL 8), `MySqlTokenStore`, `MySqlPrincipalStore`, `MySqlOutboxStore`, `MySqlIdempotency`.
- SQL-first migrations in `database/migrations-mysql/` mirroring the Postgres set with dialect handling: `ON DUPLICATE KEY UPDATE` upserts, JSON columns, no RLS → tenant isolation enforced solely by the tenant-guarded query builders (documented divergence from the Postgres RLS blade).
- `[database].driver = "postgres"|"mysql"` selects the probe + stores at boot.

**Advanced backlog (requires fresh budget decisions — not scheduled in this program)**
- RS256 JWT: promote transitive `ring` to a direct dependency; needs a D-62 amendment or swap.
- Uncompromised-password k-anonymity check (range-API style): outbound network call from the validation path + privacy review; decision-gated.
- Recursive/CTE relations (hierarchical data): query-builder CTE support; complexity + risk review gate.

## Test Plan

- **Wave 1:** registry units (alias/group resolution; `GRAPH-MW-UNKNOWN` + `GRAPH-BINDING-RESOLVER` collect-all); keel-http units (per-route ordering, throttle-group bucket isolation, `auth` extension install, `middleware.toml` parse/reject); `http_integration` (422 with `fields` meta; 401 anonymous vs 200 PAT; `public` anonymous 200; verified 403→200; binding hit/miss/cross-tenant→404; throttle 429; resource CRUD roundtrip on a test module; `per_page` clamping); route-cache cycle (cache → identical `route:list`, manifest drift → boot refusal, clear → live); OpenAPI emits required/min/max/one_of constraints.
- **Wave 2:** registry units (every new gate: `GRAPH-PLANE`, `GRAPH-GROUP-UNKNOWN`, `GRAPH-CONSTRAINT-PARAM`, `GRAPH-CONSTRAINT-KIND`, `GRAPH-FALLBACK-DUP`, `GRAPH-ROUTEFILE-OP`, `GRAPH-CHANNEL-CAP` — collect-all; singleton expansion; constraint-kind matrix incl. pattern mini-grammar; `excluded_middleware` removal from the resolved chain); `routing_edge_integration` (constraint mismatch → 404 per kind; ANY method dispatch; verb mismatch → 405 with `Allow`; HEAD/OPTIONS auto-registration; domain/subdomain gates before auth; one-time signed URL double-redemption → 404); deferred items noted honestly: fallback route rendering (the `fallback = true` route stays gate-only — docs/18 `fallback` row remains API-ONLY) and the web-plane CSRF roundtrip (moves to the Wave 7 CSRF default-on item); engine facets (escaping XSS vectors, layout inheritance via `@extends`/`@parent`, component slots, `@once`/`@push` stacks, `@forelse` empty branch, compiled-cache hit/invalidation, custom directive + custom conditional, package namespaces); error rendering (web-plane error views, problem+json option, `app.debug` gate hides internals); static middleware traversal tests; ETag/304 roundtrip.
- **Wave 3:** Select/ModelRuntime units (aggregate SQL shape, union/union_all shape, group_by/having, lock clauses, where_exists correlation, pluck/value, chunk keyset pagination); finder matrix (`first_or_create` race semantics, `update_or_create` merge, `insert_or_ignore` conflict, increment/decrement atomicity); pivot `sync` idempotence + `toggle` + pivot attributes; polymorphic morph-map resolution; optimistic-lock conflict → `STALE_RECORD`; strict-mode errors (lazy-load, discarded attr, missing attr); cast roundtrips incl. `encrypted`/`hashed`; soft-delete trio + binding `with_trashed`; prune chunking; pg matrix additions (aggregates/exists/soft-delete trio/version column/`insert_or_ignore`/chunk streaming vs live PostgreSQL 16); resource transformer conditional-field tests.
- **Wave 4:** validation rule battery (every new rule × pass/fail, incl. field-ref forms, `bail` short-circuit, dot-path wildcards, `unique`/`exists` tenant-scoped probes, DB-failure → 503 not pass); cache service semantics incl. `add` atomicity and `pull` once-ness on both backends; tag isolation; lock get/release/force + closure form; stampede single-flight test; remember-me TTL + regeneration persistence; password-confirm window expiry; `keel about` golden fields; freeze type + CI lint green; `GRAPH-VERSION-SEMVER` violations; shell principal-scoped denial test; Playwright suite green locally; profiling wrapper smoke.
- **Wave 5:** CLI smoke per new command (about, env, optimize/optimize:clear, config:cache/clear/show, cache:forget, migrate:rollback/reset/refresh/install + advisory-lock contention test, db:schema:dump, db:show, queue:clear/flush/forget/listen/monitor/prune-batches/prune-failed/restart/retry-batch, schedule:work/test, storage:link, view:cache/clear, test, model:prune, call); codegen: every new kind produces substituted, placeholder-free content (extended `all_kinds_produce_substituted_content` + `generated_code_contains_no_placeholder_macros`); console-route capability denial test.
- **Wave 6:** notification per-channel render + database channel list/read endpoints + unread count; on-demand notifiable audit-hash test; Mailable attachment streaming + failover transport selection; disk registry boot errors + visibility-gated presign; i18n catalog load, locale resolution order, `trans_choice` bucket matrix; cron parser matrix (lists/ranges/steps/invalid) + timezone next_run + `without_overlapping`/`on_one_server` lock behavior + hooks; broadcast auth matrix (allowed/denied/expired token, presence member list); unique-job skip-if-locked, rate-limited requeue, batch status/cancel/retry, chain failure-position, restart signal; HTTP client retry/backoff + sink_to + HttpFake assertions.
- **Wave 7:** every fake's assertion methods (positive + negative); full assertion matrix against a fixture app (redirect/session/cookie/view/json-path/html/database/auth); time-travel determinism (session TTL + scheduler next_run under frozen clock); transactional savepoint isolation between two tests touching the same table; factory `create()`/`state()`/sequences; security-header emission per config; rehash-on-login with drifted params fixture; password policy enforcement on register + reset; signed-URL TTL bound boot refusal; cookie encryption roundtrip; web-plane CSRF default-on.
- **Wave 8:** `Str`/`Number`/date helper units (incl. plural/singular irregular table, currency locale variants); third-party-style package install → routes served over HTTP + boot refusal on ABI mismatch; publish idempotence + conflict report + `--force` + config-only publish; OAuth2 HTTP matrix (code+PKCE roundtrip, refresh, revocation, bad client, code replay rejected, redirect allowlist); decision-gate docs reviewed (no implementation until gates pass).
- **Wave 9:** WASM facet suite (manifest validation, dispatch roundtrip, capability denial, fuel exhaustion, malformed-manifest boot refusal); MySQL full store matrix (CRUD/tenant isolation/transactions/skip-locked) vs live MySQL 8 in docker-compose.
- **Program gates:** all existing 535+ workspace tests, pg matrix vs live PostgreSQL 16, clippy on touched crates, architecture + dep-rules tests stay green; parity matrix rows in `docs/18` flipped with new evidence anchors per wave; `docs/18` backlog rows marked DONE as they land; CHANGELOG + dependency ledger updated per wave; docs updated in-wave: `04-HTTP`, `05-DATABASE`, `07-ASYNC`, `08-SERVICES`, `11-TESTING`, `12-CLI`, `13-OPERATIONS`, `15-EXTENSION`, `16-MIGRATION-FROM-LARAVEL`, `18-GAP-ANALYSIS-LARAVEL` (rows + ledger).

## Assumptions

- Additive manifest fields → `MODULE_ABI` stays 1 across Waves 1–8; WASM plugins (Wave 9) negotiate the same ABI via `keel_manifest()`.
- Exactly one budget amendment (wasmtime); MySQL via sqlx feature flag; everything else zero-new-dep — the cron parser, route-constraint pattern grammar, validation date-format tokens, English pluralization subset, and the Vite `manifest.json` reader are all hand-rolled. Items that would break this (RS256 JWT, network password checks, CTEs) remain decision-gated in Wave 9.
- All deployment caches (routes, config, views) use hash-mismatch-refuse boot semantics — strictly stricter than Laravel (never serve stale).
- Route constraints replace Laravel's regex `where` with a closed pattern grammar; validation replaces regex rules with the same grammar; both are documented SUPERSEDED divergences, not gaps.
- SSR is opt-in with SPA-first still the default web story; the ops dashboard, JSON:API, and Precognition are decision gates, not committed work.
- Parity vocabulary: Blade rows flip to EQUIVALENT/SUPERSEDED (interpreted expressions deliberately replace PHP eval); Laravel `Collection`, facades, Cashier/Socialite/Dusk/Valet clones, MongoDB stay DELIBERATELY EXCLUDED per §22 policy.
- `medicare-kernel` and the domain applications under the repo root are untouched by this program except where explicitly named as reference migrations (appointments binding/pagination adoption, oauth2 module).
