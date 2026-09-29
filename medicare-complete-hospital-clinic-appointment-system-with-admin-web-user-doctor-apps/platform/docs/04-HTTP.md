# 04 — HTTP

Transport is `crates/web/http` (axum 0.8): routers, middleware pipeline, envelopes, view models.
Domain modules never import transport — they declare routes in `module.toml` and the runtime wires
them to operations.

## Route declaration

Routes live in the module manifest (graph-validated, duplicate-checked):

```toml
[[routes]]
method = "POST"
path = "/api/v1/appointments"
operation = "book"          # resolves to <module>.book
capability = "book"
version = "1.0"
```

`keel route:list` prints the catalog; `keel sdk:openapi` emits OpenAPI 3.1 from the same source.

## Wire envelope (frozen)

Every response is an envelope:

```json
{ "ok": true, "data": ... }
{ "ok": false, "error": { "code": "KER-*", "message": "...", "retryable": false, "meta": {} } }
```

`Envelope::ok(data)`, `Envelope::error(&err)`, `status_for_error(&err)` map the frozen error
taxonomy onto HTTP status codes. Internal details never leak across the wire.

## View models

- `RequestViewModel` — parse + validate inbound payloads (the FormRequest successor).
- `ResponseViewModel` — shape outbound data (typed resources).
- `Resource` — typed JSON transformer (Wave 3, `JsonResource` parity): `from_source`
  body, `when`/`when_some` conditional fields (false/None omits the key, never null),
  wrapping key (`wrap()`, default `data`), `ResourceCollection` for lists with
  pagination-meta passthrough.

These are traits, checked by the compiler, not by runtime reflection.

**Typed bridge (Wave 1, opt-in).** `TypedHandler<I, O>` (keel-command) adapts a typed
`I -> O` handler function to the `OperationHandler` interface; `Validated<M: RequestViewModel>`
and `TypedResource<R: ResponseViewModel>` (keel-http) supply the JSON → view-model → JSON halves
(`TypedResource` was named `Resource` until Wave 3 Phase 7 gave the name to the transformer trait).
Handlers that don't need the bridge stay raw JSON (the appointments reference handlers are raw).

## Middleware

Two layers: a frozen global admission pipeline, and per-route named chains (Wave 1,
Laravel `$middlewareGroups` parity).

**Global admission pipeline (frozen order).** metrics (outermost) → CSRF → admission
(request-id → trace → body-limit → CORS → maintenance → rate-limit) → session → router.
Maintenance mode answers 503 with Retry-After (bypass secret); the admission rate limiter
stays as the coarse fallback limiter. Order and registration are runtime wiring; modules
contribute through contracts, not by stacking closures ad hoc.

**Named chains (Wave 1).** An optional `middleware.toml` at the discovery root (same root
as `applications/`) declares the catalog; absent file = the builtin catalog.

```toml
[aliases]            # name -> builtin id
ops = "throttle:api"

[groups]             # name -> ordered alias list (first = outermost)
api = ["throttle:api", "auth", "verified"]

[throttle.api]       # token-bucket config per throttle group
capacity = 120
refill_per_sec = 2.0
```

Builtins: `auth` (session extension, else bearer PAT/JWT; installs the
`ResolvedPrincipal` request extension, or a 401 envelope), `verified` (B-23 email gate;
403 `AUTH_DENIED` when `[app].require_verified_email` is on), `throttle:<group>`
(token bucket keyed by group + client key — `x-rate-key` > bearer hash > first `x-forwarded-for` IP > `global`;
429 + `Retry-After`). Tenant-plane graph routes get the implicit `api` group
(`throttle:api` → `auth` → `verified`) unless `public = true`, which strips `auth` +
`verified` only — throttle stays. Explicit route `middleware = [...]` entries resolve
alias-first, then group (flattened in place), and run after the implicit chain.
Modules cannot define custom middleware in v1 — the catalog is framework-owned.

**Boot gates.** Unknown route middleware entries and malformed `middleware.toml` files
refuse boot fail-closed: `GRAPH-MW-UNKNOWN` collects every violation (enforced by both
`keel check` and `keel-api` boot). Resolved chains are serialized into the route-cache
artifact (below).

## Realtime

`services/realtime` provides a broadcast hub with SSE and WebSocket carriers (feature-flag gated);
see `08-SERVICES.md`.

## Graph → HTTP dispatch (S7, extended Wave 1)

At boot, `keel-api` compiles the PlatformGraph from every `applications/*/module.toml`
(boot gate: a manifest error refuses boot, parity with `keel check`) and registers one axum route
per `RouteSpec` (`{param}` manifest syntax is axum 0.8 capture syntax), attaching each route's
resolved middleware chain as a per-route layer. The dispatcher then:

1. Reads the principal installed by the `auth` stage (`ResolvedPrincipal` extension). A route
   without one is rejected with 401 unless it is declared `public = true` AND the handler plan
   opts out of authorization (`plan.authorization.required == false`); anonymous dispatch runs
   under a fixed anonymous principal + default tenant. Anonymous reach is never possible on
   non-public routes (fail-closed).
2. Parses the JSON body (empty → `{}`, malformed → 422), merges path params and query pairs into
   the input object.
3. GET routes parse `page`/`per_page` and inject `_page` (pagination, below).
4. Resolves `route.bindings` and injects `_bindings.<param>` AFTER the merge, so request bodies
   cannot forge bindings (model binding, below).
5. Runs the route's manifest `[[validation]]` specs collect-all over the merged input; failures
   answer a 422 envelope with a `fields` meta block before any dispatch.
6. Builds an `ExecutionContext` and dispatches `<module>.<operation>` through the CommandBus;
   capability denial → 403 envelope, unknown token → 401.

`/meta/routes` lists the live route inventory.

## Route model binding (Wave 1)

Routes may bind path parameters to models (Laravel route-model binding parity):

```toml
[[routes]]
method = "DELETE"
path = "/api/v1/appointments/{id}"
operation = "cancel"
bindings = [{ param = "id", model = "Appointment" }]
```

Modules register a resolver per model name into the `route.bindings` registry service at boot
(appointments registers `"Appointment"`); a manifest binding whose model has no registered
resolver fails the `GRAPH-BINDING-RESOLVER` boot gate (fail-closed, at keel-api boot — the
registry only exists once providers have run). At the edge the resolver runs tenant-scoped:
`Ok(Some(record))` injects `_bindings.<param>` (the full record) into the operation input;
malformed UUID, miss, and cross-tenant lookups all answer 404 envelopes (no existence leaking).

## Pagination (Wave 1)

GET graph routes parse `page`/`per_page` query parameters at the edge (`PageQuery::from_pairs`):
`page` starts at 1 (`page=0` → 422 `page starts at 1`), `per_page` defaults to 25 and is clamped
to `MAX_PER_PAGE` (100). The parsed pair is injected into the operation input as
`_page = { page, per_page }`. List handlers return one page plus a `page` meta block —
`{ page, per_page, total, has_more }` — alongside the collection key (see appointments `list`).
Stores may omit `total`; `has_more` then falls back to the full-page heuristic.

## Deprecated routes and API versioning (Wave 1/4, L72)

`deprecated = true` on a route keeps it fully functional and advertises the sunset: success
responses carry a `Deprecation: true` header and `meta.deprecated = true` in the envelope.
`keel sdk:openapi` emits OpenAPI-native `deprecated: true` for the same routes.

Wave 4 completes versioning (L72 ◐ → done): every route's `version` (default `v1`) must
parse as `v<N>` (lowercase `v` + digits) and must agree with any version-looking path
segment (`/api/v2/...`), enforced by the boot-refusing `GRAPH-VERSION-SEMVER` gate.
`keel route:list` prints the VERSION and DEPRECATED columns, and OpenAPI carries the
per-route version as the `x-keel-version` operation extension beside `deprecated: true`.

## Route cache (Wave 1)

`keel route:cache` resolves every route's dispatch plan (chain, bindings, validators, flags) and
writes `storage/framework/routes.json`, pinned to a SHA-256 content hash over every
`applications/*/module.toml` + `modules/*/module.toml` + `middleware.toml` + `MODULE_ABI`.
`keel route:clear` deletes the artifact. Boot semantics are strictly stricter than Laravel: the
graph always recompiles; an absent artifact falls back to live compilation; a present artifact
with a matching hash registers routes from the plan list; a stale or corrupt artifact REFUSES
boot with a pointer to `keel route:cache`. There is no silent live-fallback for drift.

## Sessions and cookies (S7)

`web/http` ships a typed cookie layer (no cookie crate, D-62) and a session middleware: the
`keel_session` cookie carries `session_id.hmac_tag` over the session secret. Posture: `Path=/`,
`HttpOnly`, `SameSite=Lax`, `Max-Age` from config (8h default), `Secure` in prod only. Sessions are
feature-flag gated (`app.sessions`, default disabled — `keel flag:set app.sessions 100`), load via
the `SessionStore` port (in-memory or `PgSessionStore` table `http_sessions`), touch on activity,
and are swept by policy.

Remember-me (Wave 4): login accepts `remember: bool`; true extends the cookie `Max-Age` to
`SessionPolicy.remember_ttl` (30d default vs the 8h session TTL), records `remember: true`
in the session bag, survives session regeneration, and `/auth/me` reports the flag. Honest
gap: the extension is cookie-`Max-Age` only — server-side idle/absolute session bounds are
unchanged, so a remembered session is still swept by policy.

## Uploads (S7)

`POST /api/v1/uploads` accepts `multipart/form-data` behind authentication. Hard limits enforced
mid-stream (8 parts / 64 KiB fields / 8 MiB per file by default), optional MIME allowlist, filenames
sanitized to a safe basename (last path segment, no control chars/path separators), storage through
the `StoragePort` under `uploads/<principal>/`.

## Platform auth endpoints (S7)

`/auth/register` (dev/test or `KEEL_ALLOW_REGISTER`), `/auth/login`, `/auth/logout`, `/auth/me`,
`/auth/password-reset/request|complete`, `/auth/email-verification/request|confirm` (B-23),
`/auth/confirm` (Wave 4).
Login mode: session cookie by default, `mode:"token"` (or
sessions disabled) issues a hashed 30-day PAT; login attempts are rate-limited per IP+identifier.

Password re-confirmation (Wave 4): `POST /auth/confirm` re-verifies the current password and
stamps `confirmed_at` in the session bag; the `confirmed` middleware alias (added to the
catalog) gates routes on freshness (default window 15 min, `auth.confirm_timeout_secs`) and
answers 403 `CONFIRMATION_REQUIRED` on expiry — including the unauthenticated-style envelope
when no confirmation ever happened. Honest gap: the web-plane redirect to a confirm view was
not implemented; both planes answer the envelope (same precedent as the `verified` gate).

Email verification (B-23): `request` issues a single-use 60-minute `verify` token for the
principal owning the address (enumeration-safe: unknown addresses get the same `sent:true`
answer and no token); `confirm` redeems it once and stamps `principals.email_verified_at`,
which `/auth/me` reports as `email_verified_at`. When `[app].require_verified_email = true`,
graph module operations answer 403 `AUTH_DENIED` until the address is proven; auth endpoints
stay open so verification itself can complete. Delivery of the token rides the notification
layer in production; dev/test surfaces it as `dev_token`, mirroring password reset.

## Stateless JWT bearer interop (B-22, v1.1)

Opt-in and non-default (report R5): with the `app.jwt` feature flag on
(`keel flag:set app.jwt 100`) and a non-empty `[auth].jwt_secret`, the edge accepts
externally-issued HS256 JWTs as `Authorization: Bearer` credentials. Bearer resolution
order is session cookie → hashed PAT → JWT; PAT stays the preferred API credential.
The verifier (`keel-http` `jwt.rs`, zero new dependencies — it reuses the tested sha2
`signed::hmac_sha256` + `constant_time_eq`) accepts only `alg = HS256` (`none` and every
other algorithm are rejected before any crypto runs), verifies the signature over the
exact received signing input (never a re-encoding), and requires `exp` (enforced).
Malformed, expired, forged, and tampered tokens all collapse to the same 401 as an
invalid PAT. Claims: `sub` (principal UUID, required), `tenant` (optional tenant UUID;
absent falls back to the caller's default tenant), `exp`, plus informational `iat`/`jti`.
A stateless JWT never logged in through this process, so its access snapshot is hydrated
at resolution time (idempotent install); session/PAT paths keep login-time snapshot
semantics. An empty `jwt_secret` with the flag on logs a warning and leaves interop
disabled rather than booting a zero-key verifier. RS256 would require promoting the
transitive `ring` dependency to a direct one and a recorded budget decision.

## Named routes and URL generation (S9)

Routes may declare an optional `name` in `module.toml` (unique per module —
the graph gate `GRAPH-DUP-ROUTENAME` enforces it). `PlatformGraph::resolve_url`
generates concrete paths: every `{param}` is substituted (percent-encoded) and
extra parameters become a sorted query string. `keel route:list` prints names.

## Signed URLs (S9)

`SignedUrls` (keel-http) signs a path with HMAC-SHA256 over the existing sha2
digest and an optional expiry: `sign(path, Some(Duration::minutes(30)))`
appends `expires=` + `signature=`; `verify` rejects tampered paths,
parameters, expiry values, and expired links (constant-time compare). This is
the direct equivalent of Laravel `URL::signedRoute` / temporary signed URLs
with no new dependency.

## Redirects and flash (S9)

- `Redirect::to / see_other / temporary` — 302/303/307 with `Location`.
- `flash_put(manager, id, key, value)` / `flash_take(manager, id, key)` —
  put-once / read-once values in the session bag (`_flash.*`); `flash_take`
  removes atomically, so a flash survives exactly one request after a
  redirect.

## SPA-first web story (S9 decision)

Keel v1 is API-first/SPA-first: the framework serves JSON envelopes and emits
OpenAPI (`keel sdk:openapi`); browser UIs are SPAs consuming that contract.
`keel new --with-starter` scaffolds `templates/starter-template`: a
ModelStore-backed notes module (named routes, ModuleProvider over
`ModelStoreRef`) plus a `web/` shell implementing the documented auth pattern
(cookie session same-origin, hashed PAT for API clients). SSR/template engines
are deliberately deferred to v2.0.

## Metrics endpoint (S10, B-17)

`keel-api` records process metrics unconditionally via `keel-metrics`
(registry with a hard 4096-series cardinality cap per kind) and exposes them
at `GET /metrics` in Prometheus text format 0.0.4 — but only when the
`app.metrics` feature flag is enabled; otherwise the route answers a 404
envelope. Recording paths:

- `record_metrics` middleware — mounted **outermost** (after admission and
  the CSRF guard), so rate-limit/maintenance/CSRF rejections are counted
  alongside happy paths. Emits `keel_http_requests_total{method,status_class}`
  and `keel_http_request_duration_seconds{method}` (fixed bucket histogram).
- `/metrics` handler — refreshes `keel_uptime_seconds`, `keel_degraded`
  (0/1) and `keel_routes_declared` gauges on every scrape.
- Dropped series (cardinality guard) surface as
  `keel_metrics_dropped_series`.

Enable via `storage/framework/flags.json` (flag ladder, S3) — no restart
state beyond the boot snapshot is consulted.

## Horizon-lite dashboard (S10, B-20)

`GET /_dashboard` serves a single static page (no JS framework, no build
step) that polls `/healthz`, `/readyz`, `/meta/boot`, `/meta/flags`,
`/meta/routes` and `/metrics`, rendering boot info, flag pills, the graph
route table, registered services and the raw Prometheus text. Gating: always
served in Dev/Test; production requires the `app.dashboard` flag. Read-only
by design — queue/cache internals land in v0.9+ behind the same surface.

## Redis cache backend (S10, B-12)

When `REDIS_URL` is set and reachable, the platform cache port
(`cache.port` / `CachePortRef` carrier) binds `keel-cache-redis::RedisCache`
instead of the moka in-memory adapter; unreachable Redis degrades to memory
with a warning (D-52 parity). Redis primitives: `SET ... EX`, `SET NX` for
`put_if_absent`, `SCAN MATCH` prefix invalidation and a Lua compare-and-
delete for `delete_if_equals` (the `CacheLock` holder-safety primitive).
Cache failures degrade to misses — the cache is never a correctness barrier.

## Blocking notes (perf budget)

`keel-metrics` uses one coarse lock around integer increments — fine at v0.5
ops rates, revisit before v1.0 load targets. Redis cache serializes commands
per instance connection (control-plane rates); a pooled variant is a later
revision.
