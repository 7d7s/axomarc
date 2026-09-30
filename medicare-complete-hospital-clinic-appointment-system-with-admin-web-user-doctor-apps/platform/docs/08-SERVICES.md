# 08 — Services

Ports first: every service is a trait in `crates/services/*`; adapters (smtp, S3, MSG91, FCM,
local disk) implement ports and are wired only by runtime binaries.

## Cache (+ locks)

`services/cache` with a memory adapter; `adapters/keel-cache-redis` (RedisCache) picks up
`REDIS_URL` for the distributed path. `CacheLock` provides atomic delete-if-equals locks
for critical sections.

Wave 4 adds `CacheService` over the unchanged `CachePort`: `remember(key, ttl, f)` /
`remember_forever`, `add` (atomic `put_if_absent`), `forever`, `pull`, `many`/`put_many`,
`has`, `increment`/`decrement`, `flush` (memory: clear; Redis: `FLUSHDB`); cache tags —
`tags([...]).get/put/flush` (memory: key-prefix namespaces; Redis: SET-backed membership);
atomic locks `lock(key, ttl)` → `get`/`release`/`force_release` plus the closure form
`lock.get(|| ...)`; stampede protection collapses concurrent `remember` misses onto one
leader under a per-key lock with jittered wait. Values are serde_json-serialized at the
service layer — `keel-caching` gains the workspace-budgeted `serde_json` dependency for
it (crate-level addition, no `[workspace.dependencies]` budget change); ports and
adapters are untouched.

## Storage

`services/storage` defines the object port; adapters: local disk (dev) and S3-compatible (minio in
docker-compose). Guards enforce size/type policies.

Wave 6 P2 adds the disk registry. `[storage]` carries `default` (default disk name, `local`) and
`[storage.disks.<name>] { driver = "local"|"s3", root|bucket, visibility = "public"|"private",
url }`. Runtime builders (keel-api's `StorageProvider`, keel-worker's `mail_job` — each runtime
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

## Notifications (multi-channel)

`services/notifications` is a hub: mail (sandbox in dev, SMTP live via lettre), SMS (MSG91), push
(FCM). Channels are providers behind one contract; templates and dispatch are audited.

Mail gains the Wave 6 `Mailable` path (P1): subject, from/reply-to, to/cc/bcc, text + html
bodies, custom RFC822 headers, and attachments that carry only an `ObjectRef` — never bytes.
`resolve_attachments` streams them through `StoragePort::get_capped` under the upload cap
(8 MiB, pinned to `UploadLimits` by a keel-api test; over-cap is `CAP_LIMIT_EXCEEDED`, never a
truncation, and `LocalDisk` stops reading at `max_bytes + 1`). The `[mail]` section declares
the failover chain: `mail.default` is a comma-separated mailer list (`log` is built in and
needs no declaration), `[mail.mailers.<name>]` sets `transport = "log"|"smtp"` +
host/port/username/password/from/`timeout_ms`. An unknown chain member, a duplicate name, an
empty chain, or an smtp mailer without a host refuses boot; level-3 key typos are caught by the
`CONFIG_SCHEMA_MAPS` walk plus `deny_unknown_fields`, and `redacted_json` masks every non-empty
mailer password.

`MailChain` (keel-mail-smtp) selects the first healthy transport per send — health is a TCP
connect probe bounded by the mailer's timeout (`log` never probes) — and an all-unhealthy chain
is a retryable external error whose detail lands in `last_error`. Queued sends ride the
`mail.send` job: keel-worker registers the def + `MailSendHandler` at boot (P2: the handler
carries the registry's default disk port, so attachment `ObjectRef`s resolve against the same
disk the upload edge writes to, and a missing object still fails the attempt explicitly instead
of being dropped), and `enqueue_mailable` validates the `Mailable` and builds the refs-only payload
through the registered def. The durable policy owns the rest: retry with backoff, then
dead-letter.

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
notification localization is exercised at P5/P6.

## Realtime

`services/realtime`: BroadcastHub with SSE and WebSocket carriers. Feature-flag gated rollout;
per-tenant channels; used for live UI updates rather than polling.

## HTTP client

`services/http-client`: retry policies, timeouts, redaction of secrets in logs, structured request
logging. Webhooks build on it (S3 surface).

## Search

`services/search` (S6): the contract — `Searchable`, `Index`, `SearchQuery`, `SearchResult`,
`Facet`, `VectorQuery`, `HybridQuery`. Tenancy is part of the contract: every document carries a
tenant tag; queries run inside a tenant and mismatches are invisible.

The in-memory reference engine implements keyword scoring, exact filters, facets, cosine vector
search, and alpha-blended hybrid (`alpha * keyword + (1 - alpha) * similarity`). PostgreSQL/pgvector
and OpenSearch plug in as adapters behind the same port. TestKit ships `FakeSearch`.
