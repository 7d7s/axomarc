# 08 — Services

Ports first: every service is a trait in `crates/services/*`; adapters (smtp, S3, MSG91, FCM,
local disk) implement ports and are wired only by runtime binaries.

## Cache (+ locks)

`services/cache` with a memory adapter; `adapters/kelson-cache-redis` (RedisCache) picks up
`REDIS_URL` for the distributed path. `CacheLock` provides atomic delete-if-equals locks
for critical sections.

Wave 4 adds `CacheService` over the unchanged `CachePort`: `remember(key, ttl, f)` /
`remember_forever`, `add` (atomic `put_if_absent`), `forever`, `pull`, `many`/`put_many`,
`has`, `increment`/`decrement`, `flush` (memory: clear; Redis: `FLUSHDB`); cache tags —
`tags([...]).get/put/flush` (memory: key-prefix namespaces; Redis: SET-backed membership);
atomic locks `lock(key, ttl)` → `get`/`release`/`force_release` plus the closure form
`lock.get(|| ...)`; stampede protection collapses concurrent `remember` misses onto one
leader under a per-key lock with jittered wait. Values are serde_json-serialized at the
service layer — `kelson-caching` gains the workspace-budgeted `serde_json` dependency for
it (crate-level addition, no `[workspace.dependencies]` budget change); ports and
adapters are untouched.

## Storage

`services/storage` defines the object port; adapters: local disk (dev) and S3-compatible (minio in
docker-compose). Guards enforce size/type policies.

Wave 6 P2 adds the disk registry. `[storage]` carries `default` (default disk name, `local`) and
`[storage.disks.<name>] { driver = "local"|"s3", root|bucket, visibility = "public"|"private",
url }`. Runtime builders (kelson-api's `StorageProvider`, kelson-worker's `mail_job` — each runtime
owns the conversion because the layer matrix keeps adapter constructors out of config and
service crates) turn the section into a `DiskRegistry` at boot: a driver typo or level-3 key typo
is caught by the typed parse / `CONFIG_SCHEMA_MAPS` walk, while a `local` disk without `root`, an
`s3` disk without `bucket`, or a `storage.default` naming no registered disk refuses boot. With
nothing declared the registry holds one synthetic `local` disk rooted at `<root>/storage/local`
(dev/test parity with the pre-disk wiring). The port gains `list(prefix)` / `make_dir` /
`delete_dir`: prefix matching is boundary-scoped (`uploads` never matches `uploads2`), listings
are sorted, deletes are idempotent, and the shared `validate_dir_prefix` guard refuses traversal
before any path work — the default impls answer `NOT_IMPLEMENTED`, both adapters override (local
walks the tree; the S3 sandbox tracks virtual directory markers, since S3 has no real
directories). URL issuance is visibility-gated: private objects only ever get a signed URL from
the port, public ones resolve through the disk's `url` base as `{base}/{bucket}/{key}` (local
public files go through the `storage:link` symlink path). The upload edge resolves `?disk=`
through the registry (`storage.default` when absent; an unknown name is a 422, never a silent
fallback), and every stored record names the disk it landed on.

Wave 6 P10 adds a chunked write path for large downloads: `StoragePort::writer(bucket, key)` opens
an `ObjectWriter` session (`write(chunk)` / `finish() -> ObjectRef`) that commits exactly once. The
default implementation buffers the chunks and issues one `put` on `finish` — the port stays
implementable with its core four methods — while `kelson-storage-local` overrides it with direct file
writes and removes the partial file when a session is dropped without `finish`. A session that is
never finished commits nothing, so an aborted download can never publish a truncated object.

## Notifications (multi-channel)

`services/notifications` is a hub: mail (sandbox in dev, SMTP live via lettre), SMS (MSG91), push
(FCM). Channels are providers behind one contract; templates and dispatch are audited.

Mail gains the Wave 6 `Mailable` path (P1): subject, from/reply-to, to/cc/bcc, text + html
bodies, custom RFC822 headers, and attachments that carry only an `ObjectRef` — never bytes.
`resolve_attachments` streams them through `StoragePort::get_capped` under the upload cap
(8 MiB, pinned to `UploadLimits` by a kelson-api test; over-cap is `CAP_LIMIT_EXCEEDED`, never a
truncation, and `LocalDisk` stops reading at `max_bytes + 1`). The `[mail]` section declares
the failover chain: `mail.default` is a comma-separated mailer list (`log` is built in and
needs no declaration), `[mail.mailers.<name>]` sets `transport = "log"|"smtp"` +
host/port/username/password/from/`timeout_ms`. An unknown chain member, a duplicate name, an
empty chain, or an smtp mailer without a host refuses boot; level-3 key typos are caught by the
`CONFIG_SCHEMA_MAPS` walk plus `deny_unknown_fields`, and `redacted_json` masks every non-empty
mailer password.

`MailChain` (kelson-mail-smtp) selects the first healthy transport per send — health is a TCP
connect probe bounded by the mailer's timeout (`log` never probes) — and an all-unhealthy chain
is a retryable external error whose detail lands in `last_error`. Queued sends ride the
`mail.send` job: kelson-worker registers the def + `MailSendHandler` at boot (P2: the handler
carries the registry's default disk port, so attachment `ObjectRef`s resolve against the same
disk the upload edge writes to, and a missing object still fails the attempt explicitly instead
of being dropped), and `enqueue_mailable` validates the `Mailable` and builds the refs-only payload
through the registered def. The durable policy owns the rest: retry with backoff, then
dead-letter.

Notifications v2 (P5) turns the hub into a typed dispatch surface: modules implement
`NotificationDef` (`type_key` + `to_mail() -> Mailable` / `to_sms()` / `to_push()` /
`to_database()` / `template()`, default view `notifications.<type_key>` — dots map to directories)
and `NotifyHub::dispatch_notification` renders the selected channel — mail (recipient filled from
the delivery target; HTML body rendered through the Wave 2 views engine when the
`NotificationViewRenderer` port is wired, a render failure fails closed and an absent port keeps
the text body), SMS (`to_sms()` passed through the catalog path with locale fallback), push (D-17
nudge-not-payload: `signal` + optional `delta_cursor` only, never free-form data) — and writes the
database channel through the new `NotificationStore` port (in-memory reference + a pg
implementation behind the same degradation posture, shared as ONE instance between the hub and the
HTTP routes). The inbox edges (`GET /api/v1/notifications`, `POST /api/v1/notifications/read`)
live in `kelson-api/src/notification_routes.rs`; the mark is gated by the `notifications.mark_read`
framework capability chained into the login-time grant set.

Notifications P6 completes the v2 surface — on-demand notifiables, queued delivery, producer
dispatch. `NotifyHub::dispatch_on_demand(n, channel, target, ctx)` delivers to a raw email/phone
with no principal record (the same channel ports; `Channel::Database` is refused up front because
a row is addressed by principal; a def's `queue_via` still applies) and returns
`OnDemandAudit { type_key, channel, target_hash, at }` — the target is rendered and delivered
inside the call, and only its sha256 leaves it; there is no persistence port (D-74: callers
persist the audit through their execution unit's `record_audit`). The queue seam:
`NotificationDef::queue_via -> Option<&str>` names a job; a def that opts in is rendered at
dispatch (request-time locale, filled target, view HTML) and handed to
`NotificationQueuePort::enqueue(job_type, &QueuedNotificationDelivery)` — a v1-versioned envelope
whose `DeliveryPayload` is adjacently tagged Mail/Sms/Push/Database. The drain refuses an unknown
envelope version loudly (retry → dead-letter, never a guess), and `queue_via` with no port fails
closed (`not_implemented`). Both halves are runtime-owned (the layer matrix keeps Async off
Services): kelson-api enqueues pool-gated (`PgJobStore` + the registered `notification.send` def
wired onto the hub through `PlatformProvider.notification_queue`; no pool = no port =
fail-closed), kelson-worker drains (`notification_job` builds the hub over the boot catalog, the
pg/in-memory database store and a `ChainMail` adapter over the P1 `MailChain` — the chain does
not implement `MailPort`; `NotificationDeliveryHandler` → `hub.deliver_queued`, registered beside
`mail.send`). Producer dispatch: the appointments Book/Cancel handlers dispatch
`AppointmentBooked`/`AppointmentCancelled` on the Database channel at the end of the handler —
pre-commit posture, and a refused dispatch fails the operation (fail-closed) — through
`with_notifier()` (the `ModuleProvider` fetches the hub from the registry; Platform phase
precedes Modules, an absent hub degrades silently). Rows carry `{patient, slot, locale}` plus the
request principal/tenant; `lang/{en,fr,ar}` gained `appointment_cancelled`; the defs' 
`template() -> None` keeps dispatched mail text-only (no view file ships). Wave 7 P1 adds the
dispatch observation seam: `NotifyHub::with_tap(Arc<dyn DispatchTap>)` records each successful
`NotificationDef` dispatch as `DispatchRecord { principal (None for on-demand), notification_type,
channel }` at the two dispatch entry points only — `deliver_queued` never taps (it fulfils a
dispatch the entry point already recorded; tapping would double count), the legacy raw `dispatch()`
carries no type key, and a failed render/delivery/enqueue records nothing — which is how the
testkit `NotificationFake` proves `assert_sent_to(principal, notification_type, channel)`.
Honest gaps
carried: notification mail attachments still refuse on this dispatch path (the P1
`not_implemented` seam); no in-repo def opts into `queue_via` yet (the seam is wired and
round-tripped hermetically — producers opt in when a channel must outlive the request);
`dispatch_on_demand` has no in-repo producer (the auth verification mail rides the dev_token
path); and push remains nudge-only (D-17).

## Localization (i18n)

`[i18n] { default = "en", paths = ["lang"] }` (serde-default, no new env vars) selects the
catalog the boot loader builds: `load_messages` reads `<root>/<paths>/<locale>.toml` from each
configured directory. An absent directory yields an empty catalog; a malformed file, a
non-string value, an empty file, an invalid locale filename, or a `default` naming no catalog
refuses boot with the offending file in the message. Keys are flat dotted names — nested TOML
tables and dotted keys flatten identically (`validation.required = "…"`). Shipped locales:
`lang/en.toml`, `fr.toml`, `ar.toml` (ar carries the full zero/one/two/few/many/other choice
matrix).

The edge resolves the locale once per request (session `locale` bag → `Accept-Language`,
quality-ordered first supported → default) and carries it everywhere strings surface: 422
violations use `validation.<rule>` keys with `{attribute}` substituted (manifest messages and
the built-in English wording keep precedence around the catalog), `ExecutionContext.locale`
reports it (timezone stays the bootstrap default), and every HTML view receives `_locale` +
`_lang` — the chain-merged flat map behind `@lang("key")` and `{{ "key" | trans }}` (a miss
renders the key itself, so missing files degrade instead of breaking pages).
`trans_choice(key, n)` picks pipe-separated named forms (`zero=… | one=… | other=…`, `{count}`
substituted) with the plural bucket chosen from the requested locale, even when the message
itself resolves through the fallback chain.

Notification strings ride the same catalog: `NotifyHub::with_catalog` (wired at boot with the
catalog) resolves `NotifyMessage.template` as a key with `vars` as interpolation params when a
producer sets the message's request `locale` (`serde(default)` — pre-catalog queued payloads
load unchanged); literal templates and catalog-less hubs stand exactly as before. End-to-end
notification localization landed at P6: the shipped `platform/lang` catalog through
`load_messages` + the edge's `i18n::resolve` renders `AppointmentBooked` into FR mail
(`Rendez-vous réservé pour Ada`), and an `Accept-Language: fr` booking lands a
`data.locale == "fr"` row in the inbox feed.

## Realtime

`services/realtime`: BroadcastHub with SSE and WebSocket carriers. Feature-flag gated rollout;
per-tenant channels; used for live UI updates rather than polling.

Wave 6 P7 composes the authorization the crate's own header defers to the runtime: the hub
gained a `PresenceHub` roster (idempotent join/leave per member id; `presence.join`/
`presence.leave` events publish through the same hub subscribers listen on) and a
`ChannelAuthorizer` seam the transports run at every connect — `realtime_router_authorized`
takes the host's authorizer and joins a `presence-*` roster before handing over the SSE
stream (a drop-guarded stream leaves on disconnect; the WS session leaves when its loop ends),
while the raw `realtime_router(hub)` stays as the un-authorized transport primitive for the
crate's own tests. The host side lives in `kelson-api`'s `broadcast_routes.rs`: `POST
/api/v1/broadcast/auth` mints the short-TTL channel-bound token after the resolved capability
gate, `GET /api/v1/broadcast/members` reads the roster under the same gate, and
`ChannelTokenAuthorizer` refuses anything but a valid token bound to the requested channel —
one uniform 401. `serve()` mounts only the authorized router (behind `app.realtime`), sharing
the single `PresenceHub` instance between transports and the members endpoint. Honest gaps:
channel names are exact-matched (the Wave 2 wire sanitizer forbids dots and braces, so
parameterized `orders.{id}` names can never be requested — per-record channels ride
`tenant:{id}:...` style names), and the roster is in-process (single-node posture, same as the
hub).

## HTTP client

`services/http-client`: retry policies, timeouts, redaction of secrets in logs, structured request
logging. Webhooks build on it (S3 surface).

Wave 6 P10 completes the parity surface on `ReqwestHttpClient`. The retry schedule is configurable
as the spec's `retry { times, base_ms, max_ms, on }` — `max_retries` / `retry_base_ms` /
`retry_max_ms` (the backoff cap was a hard-coded 10s) / `retry_on`, a validated list of `429`,
`5xx`, `timeout`, `connect` whose default keeps the original all-four posture; all four new knobs
are serde-default additive, and an unknown condition refuses client build instead of silently never
retrying. `retry_on` gates retrying, not classification: 429/5xx remain `EXT_ERROR` (fail fast when
excluded), 4xx remain `Ok` responses for the caller. `HttpRequest::timeout_ms` overrides the client
timeout per request (absent = inherit; skipped on the wire so pre-P10 payloads stay byte-identical).
`execute_pool(client, requests, limit)` fans out over a bounded tokio join-set and returns results
in input order (`limit = 0` clamps to 1). `HttpTap` hooks (`on_request` / `on_response` /
`on_retry` / `on_error`) observe every attempt with headers already passed through
`config.redact` — bodies stay under this crate's never-log rule. `HttpFake` asserts over the
`MockHttpClient` recording: `assert_request_count`, `assert_sent(|req| …)`, `assert_no_pending`
(the Wave 7 testkit names — the testkit fake will delegate here) plus per-index `assert_request` /
`assert_body_json`, and every failure message prints the recorded `method url` set. Finally
`sink_to(req, storage, bucket, key)` streams a 2xx body through the storage port's chunked writer
without buffering it (callers pass the target disk's port from the `DiskRegistry`): 4xx are
`NotRetryable` errors with a capped preview in `internal` — an error page is never sunk over the
target key — and once streaming starts the download is not retried; an aborted session discards the
partial object. The sink is an inherent method, not a port method — `HttpClientPort` keeps its
single `execute` operation. The crate gains a `kelson-storage` workspace dependency for it
(Services→Services, the `notifications → storage` edge).

## Search

`services/search` (S6): the contract — `Searchable`, `Index`, `SearchQuery`, `SearchResult`,
`Facet`, `VectorQuery`, `HybridQuery`. Tenancy is part of the contract: every document carries a
tenant tag; queries run inside a tenant and mismatches are invisible.

The in-memory reference engine implements keyword scoring, exact filters, facets, cosine vector
search, and alpha-blended hybrid (`alpha * keyword + (1 - alpha) * similarity`). PostgreSQL/pgvector
and OpenSearch plug in as adapters behind the same port. TestKit ships `FakeSearch`.
