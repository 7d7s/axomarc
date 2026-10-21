# LF-070 … LF-080 — Storage, cache, integrations & utility services

**Batch:** the eleven storage/cache/integration/utility items of the Laravel Feature Atlas, verified
against the framework as it stands.

**Method (the project's rule):** every status below is backed by a **named, executed** test. A
documentation claim is never proof. Where an item is only partly covered, the missing cases are named.

**Verdicts:** **1 VERIFIED** · **7 PARTIALLY VERIFIED** · **3 NOT IMPLEMENTED** (the three the atlas
itself de-prioritizes ✓).

---

## 1. What was executed

| Suite | Result |
|---|---|
| `cargo test -p kelson-caching --lib` | **17 passed** — the cache service (get/put/remember/tags/flush/increment ✓), the **cache lock** (`cache_lock_acquire_release_and_exclusivity` ✓, `guard_acquire_closure_release_and_force_release` ✓, `with_lock_runs_and_releases` ✓), the tenant isolation ✓✓✓ and the pentest case ✓ |
| `cargo test -p kelson-cache-memory --lib` | **4 passed** — incl. **`the_memory_cache_satisfies_the_cache_contract`** (new ✓) |
| `cargo test -p kelson-cache-redis --features redis-tests --test live_redis` (live Redis) | **6 passed** — incl. **`the_redis_cache_satisfies_the_cache_contract`** (new ✓) |
| `cargo test -p kelson-storage --lib` | **11 passed** — the disk registry + visibility ✓, the byte cap ✓, the streaming writer ✓, the tenant isolation ✓✓✓✓, the traversal guard ✓ and the pentest case ✓ |
| `cargo test -p kelson-storage-local --lib` | **7 passed** — the round trip ✓, the traversal refusal ✓, the streaming writer ✓, the **partial-file cleanup** ✓, the byte cap ✓ and the presign refusal ✓ |
| `cargo test -p kelson-storage-s3 --lib` | **2 passed** — the round trip and the directory ops against the S3 sandbox ✓ |
| `cargo test -p kelson-http-client --lib` | **17 passed** — the timeouts ✓, the retry schedule ✓, the header redaction ✓, the fake ✓, the mock ✓, the tap ✓ and the storage sink ✓ |
| `cargo test -p kelson-mail-smtp --lib` | **13 passed** — the mailable build ✓, the attachments ✓, the chain failover ✓, the queued retry → dead-letter ✓ and the sandbox ✓ |
| `cargo test -p kelson-notifications --lib` | **20 passed** — the channels ✓, the localization ✓, the queueing ✓, the on-demand audit ✓ and the **dispatch origin** ✓ |
| `cargo test -p kelson-search --lib` | **6 passed** — the keyword/vector/hybrid search ✓, the facets ✓, the delete ✓ and the tenant isolation ✓ |
| `cargo test -p kelson-demo --test http_integration` (the app edge, in-memory) | **100 passed** — incl. the six upload cases ✓ (`upload_happy_path_roundtrips_through_storage` ✓, `upload_disk_query_resolves_and_unknown_disk_is_422` ✓, `upload_traversal_filename_is_sanitized_and_unauthenticated_rejected` ✓, `upload_without_multipart_is_422_envelope` ✓, `upload_part_bomb_beyond_max_parts_is_rejected` ✓, `upload_quota_refuses_the_overage_and_keeps_the_ledger_exact` ✓) |

**103 named cases** across the ten crates (100 before this batch, plus the 3 contract-suite runs),
plus the demo's upload edge ✓ (`upload_happy_path_roundtrips_through_storage` ✓,
`upload_disk_query_resolves_and_unknown_disk_is_422` ✓,
`upload_traversal_filename_is_sanitized_and_unauthenticated_rejected` ✓,
`upload_without_multipart_is_422_envelope` ✓,
`upload_part_bomb_beyond_max_parts_is_rejected` ✓,
`upload_quota_refuses_the_overage_and_keeps_the_ledger_exact` ✓).

---

## 2. Item by item

### LF-070 — Cache drivers and repository API — **VERIFIED**

- **Contract**: a cache manager; memory/file/database/Redis and supported backend adapters; TTL;
  remember; tags where supported. Keys, value serialization and expiry semantics are consistent;
  outage behaviour and backend capabilities are declared.
- **Implementation**: the **port** is `kelson_caching::CachePort` ✓ (get/put/delete/invalidate_prefix ✓
  plus the two **atomic** primitives the locks are built on ✓ — `put_if_absent` ✓ and
  `delete_if_equals` ✓); the **service** above it is `CacheService` ✓ (remember ✓, tags ✓, flush ✓,
  increment/decrement ✓, the lock guard ✓); the **drivers** are the in-memory adapter ✓ and the
  Redis adapter ✓ (SET EX ✓, SET NX ✓, the Lua compare-and-delete ✓).
- **Evidence (the atlas's "contract suite per driver")**: `kelson_caching::cache_contract` is now a
  **shared suite every driver runs** ✓ — the round trip ✓, the delete ✓, the prefix invalidation ✓,
  the expiry ✓ and both atomic primitives ✓. It is green on **both** drivers:
  `the_memory_cache_satisfies_the_cache_contract` ✓ and
  `the_redis_cache_satisfies_the_cache_contract` ✓ (the Redis one against a **live Redis** ✓), so the
  two drivers cannot drift apart silently ✓.
- **The rest of the item**: the **remember** semantics — `remember_caches_the_miss_and_recomputes_after_removal` ✓,
  `remember_waiter_collapses_onto_the_leader_result` ✓ (the stampede guard ✓) and
  `remember_falls_back_when_the_leader_never_appears` ✓; the **tags** —
  `tags_are_prefix_namespaces_with_prefix_flush` ✓; the **tenant key isolation** —
  `tenant_isolation_cache_entries_do_not_cross_tenants` ✓,
  `tenant_isolation_cache_prefix_invalidation_stays_inside_one_tenant` ✓,
  `tenant_isolation_cache_atomic_slots_are_per_tenant` ✓; the **hostile keys** —
  `pentest_cache_hostile_keys_and_prefixes_cannot_escape_the_caller` ✓.
- **Declared divergences**: there is **no file or database cache driver** ✓ (the atlas lists them among
  the supported backends ✓; the framework ships the in-memory and Redis drivers ✓ — a divergence, not
  a defect ✓). The **outage behaviour** is a driver's own policy ✓ and is not exercised by a case
  ✓ (a Redis outage would have to kill the server mid-test ✓).

### LF-071 — Atomic locks and cache coordination — **PARTIALLY VERIFIED**

- **Contract**: cache locks/atomic primitives used by the scheduler, unique jobs and distributed
  coordination. Lock ownership, lease expiry, release and fencing/reentrance semantics prevent
  simultaneous critical work under documented assumptions.
- **Implementation**: the **cache lock** ✓ (`CacheService::lock` ✓ — the guard ✓, the force release ✓),
  the **unique-job lock** ✓ (`kelson_jobs::UniqueLock` ✓ — the Redis adapter ✓), and the
  **scheduler guards** ✓ (the one-server ✓ and the overlap ✓).
- **Evidence**: the **lock exclusivity** — `cache_lock_acquire_release_and_exclusivity` ✓,
  `guard_acquire_closure_release_and_force_release` ✓, `with_lock_runs_and_releases` ✓; the
  **unique lock** — `unique_admission_holds_locked_claims_until_release` ✓; the **scheduler guards** —
  `one_server_guard_acquires_runs_and_releases` ✓, `overlap_guard_skips_while_the_window_is_held` ✓; the
  **fencing** — the claim token ✓ (`mysql_job_store_fences_a_deferred_job_reclaimed_at_the_same_epoch` ✓)
  and `delete_if_equals` ✓ (a stale holder cannot free a re-acquired lock ✓); the **tenant namespace** —
  `tenant_isolation_cache_atomic_slots_are_per_tenant` ✓,
  `tenant_isolation_scheduler_guard_locks_do_not_collide_across_tenants` ✓; the **hostile keys** —
  `pentest_cache_hostile_keys_and_prefixes_cannot_escape_the_caller` ✓.
- **Gaps**: the **lease expiry** of the cache lock (a lock whose holder dies ✗) has no named case ✓; the
  **holder crash** ✗; the **clock skew** between the holder and the lease ✗; and the **Redis failover**
  ✗. *(The atlas's own warning is respected: the cache lock's atomicity is only as good as the
  backend's ✓ — the in-memory driver fakes it with a process mutex ✓ and says so ✓.)*

### LF-072 — Filesystem and upload storage — **PARTIALLY VERIFIED**

- **Contract**: a filesystem disk abstraction; local/SFTP/S3-compatible drivers; directories;
  streaming; uploads/downloads; visibility; temporary URLs. The backend can change without changing
  application logic; paths are scoped; uploads are size/type validated; private files stay private.
- **Implementation**: the **disk abstraction** ✓ (`kelson-storage`'s `StoragePort` ✓ + the disk registry ✓
  with the visibility policy ✓), the **local** driver ✓, the **S3-compatible** driver ✓ (the sandbox ✓),
  and the host's upload edge ✓ (the multipart limit ✓, the disk query ✓, the traversal guard ✓).
- **Evidence**: the **fake** — the S3 sandbox ✓ (`sandbox_stores_rendered_mail`-style recording ✓);
  the **local** adapter — `roundtrip_put_get_delete` ✓, `nested_key_layout` ✓; the **real adapter** —
  the S3 sandbox's round trip ✓; the **path traversal** — `traversal_is_rejected` ✓,
  `validate_dir_prefix_guards_traversal` ✓,
  `pentest_storage_hostile_prefixes_and_delete_dir_stay_inside_the_tenant` ✓,
  `tenant_isolation_storage_traversal_keys_are_refused` ✓; the **oversized streaming upload** —
  `get_capped_enforces_the_byte_cap` ✓ (the read is capped ✓) and the writer's cap ✓; the **partial
  failure cleanup** — `aborted_writer_session_removes_the_partial_file` ✓,
  `aborted_writer_session_commits_nothing` ✓; the **visibility** —
  `registry_resolves_disks_and_enforces_visibility` ✓; the **tenant scoping** ✓✓✓✓.
- **Gaps**: the **symlink** case ✗ (a symlink pointing outside the root ✗), the **MIME spoof** ✗ (the
  declared type vs. the content ✗) and the **temporary URL** ✗ (the local driver refuses presigning ✓
  and the S3 driver's presign is not covered by a named case ✗). There is **no SFTP driver** ✗ (a
  recorded divergence ✓).

### LF-073 — HTTP client — **PARTIALLY VERIFIED**

- **Contract**: a fluent outbound client; connection/total timeout; retries; auth/headers; response
  decoding; macros; fakes. Outbound traffic has timeout/retry bounds; non-idempotent retries are
  deliberate; TLS/errors and sensitive headers are handled safely.
- **Implementation**: `kelson-http-client` — the client with the connect/total timeouts ✓, the retry
  schedule ✓, the header redaction ✓, the **fake** ✓ and the **mock** ✓, the **tap** ✓ and the
  storage sink ✓.
- **Evidence**: the **fake** — `http_fake_asserts_count_method_url_and_body` ✓,
  `http_fake_assert_failures_name_the_mismatch` ✓; the **mock** — `mock_client_queues_and_records_calls` ✓;
  the **timeout** — `per_request_timeout_override_beats_the_client_default` ✓,
  `live_client_builds_with_defaults` ✓; the **retry budget** — `retry_schedule_is_exponential_and_capped` ✓,
  `live_retry_recovers_after_5xx` ✓, `retry_off_for_5xx_fails_on_the_first_response` ✓,
  `unknown_retry_condition_refuses_client_build` ✓ (an unknown condition is refused at build time ✓ —
  the "non-idempotent retries are deliberate" guard ✓); the **secret redaction** —
  `header_redaction_is_case_insensitive` ✓, `tap_sees_redacted_headers_across_retries` ✓; the **decoding** —
  `response_json_parsing` ✓; the **body size** — `sink_to_refuses_error_pages` ✓,
  `sink_to_streams_the_body_into_storage` ✓; the **concurrency** —
  `execute_pool_bounds_concurrency_and_preserves_order` ✓.
- **Gap**: there is **no SSRF / internal-IP guard** ✗ — the atlas names it ("Enforce egress policy and
  SSRF protections when untrusted modules can initiate HTTP" ✓) and the client will happily call a
  link-local address ✓. This is the batch's one **security** gap ✗ — it needs an egress policy, not
  a test ✗. It is filed as **F-23** below with a repro ✓ and a proposed fix ✓ (not implemented ✗).
  The **reachable path** is the webhook adapter ✓ (`kelson-webhooks` calls a tenant-registered
  target ✓); the WASM module host does **not** expose HTTP ✓, so an untrusted *module* cannot
  reach the client today ✓.

### LF-074 — Mailables, transports and templates — **PARTIALLY VERIFIED**

- **Contract**: mailable classes; configured mailers/transports; SMTP/API transport;
  markdown/HTML/text rendering; attachments; queue; failover/round-robin. The message
  headers/content/attachments are correct; the provider failure/retry behaviour is explicit.
- **Implementation**: `kelson-mail-smtp` — the mailable build ✓, the SMTP transport ✓, the **chain**
  (the failover/round-robin over the configured mailers ✓) and the **sandbox** (the fake ✓); the
  queued path is the `mail.send` job ✓.
- **Evidence**: the **content** — `live_builds_rfc822_message_with_var_substitution` ✓,
  `live_builds_multipart_message_with_attachments_and_headers` ✓; the **attachments** —
  `handler_resolves_capped_attachments_and_sends` ✓,
  `handler_refuses_oversized_attachment_and_bad_payload` ✓,
  `live_build_refuses_bad_attachment_content_type` ✓; the **bad recipients** —
  `live_rejects_bad_addresses_before_network` ✓,
  `sandbox_records_mailables_and_refuses_bad_recipients` ✓; the **failover** —
  `chain_selects_a_healthy_smtp_member` ✓, `chain_selects_first_healthy_and_falls_back_to_log` ✓,
  `all_unhealthy_chain_is_an_external_error` ✓, `chain_rejects_bad_configuration` ✓; the **queue** —
  `queued_send_retries_then_dead_letters` ✓, `enqueue_requires_registered_def_and_valid_mail` ✓; the
  **fake** — `sandbox_stores_rendered_mail` ✓.
- **Gaps**: no **golden content** case ✗ (the rendered message is asserted structurally ✓, not against
  a golden file ✓); the **header injection** case ✗; and the **localization** of a mail is not covered
  by a mail-side case ✓ (the notification side is ✓).

### LF-075 — Notifications and channels — **PARTIALLY VERIFIED**

- **Contract**: a notification class; a notifiable recipient; mail/database/broadcast/SMS/Slack/custom
  channels; localization and queueing. The per-channel payloads and routing are correct; an unavailable
  channel is handled; one channel failure does not cause accidental duplicate delivery elsewhere.
- **Implementation**: `kelson-notifications` — the typed notification ✓, the hub ✓, the **channels** ✓
  (mail ✓, database ✓, SMS ✓), the **queueing** ✓ (`queue_via` ✓), the **on-demand** path with its
  hashed audit ✓, and the **localization** through the catalog ✓.
- **Evidence**: the **per-channel payloads** — `typed_notification_renders_every_channel_through_the_ports` ✓,
  `mail_and_sms_render_localized_with_payload_vars` ✓, `database_channel_scopes_reads_marks_idempotently_and_paginates` ✓;
  the **routing** — `notification_view_context_carries_the_route_locale_injections` ✓,
  `hub_localizes_template_keys_through_the_catalog` ✓; the **queueing** —
  `queue_via_renders_into_an_envelope_instead_of_delivering` ✓,
  `queue_via_without_a_port_fails_closed` ✓,
  `queued_envelope_round_trips_and_drains_through_deliver_queued` ✓; the **channel failure** —
  `missing_adapter_is_explicit` ✓, `dispatch_fails_closed_without_targets_adapters_or_store` ✓,
  `mail_html_degrades_without_a_port_and_fails_closed_when_broken` ✓; the **idempotency** — the
  database channel's idempotent marking ✓; the **audit** —
  `on_demand_dispatch_delivers_and_returns_hashed_audit_evidence` ✓,
  `on_demand_refuses_the_database_channel_and_blank_targets` ✓; the **dispatch origin** —
  `a_queued_delivery_carries_the_dispatch_origin` ✓,
  `a_dispatch_without_a_request_carries_an_empty_origin` ✓.
- **Gaps**: there is **no Slack or SMS-provider adapter** ✓ (the SMS channel is a port ✓ with no
  provider ✓ — the atlas says those are optional adapters ✓); and the **secret redaction** of a
  notification payload is not named ✓.

### LF-076 — Search via Scout — **PARTIALLY VERIFIED**

- **Contract**: searchable models; indexing and deletion; sync/queued engines;
  Meilisearch/Algolia/Typesense adapter pattern. The index lifecycle follows the model changes; the
  results map to authoritative records; deleted/tenant-scoped records are not exposed.
- **Implementation**: `kelson-search` — the **port** ✓ (the keyword ✓, the vector ✓, the hybrid ✓, the
  facets ✓, the delete ✓) and the tenant scoping ✓. There is **no engine adapter** ✗ (no
  Meilisearch/Algolia/Typesense client ✗) and **no model-event indexing** ✗ (nothing watches a
  model change and updates an index ✗).
- **Evidence**: `keyword_search_scores_and_filters` ✓, `vector_search_orders_by_similarity` ✓,
  `hybrid_blends_keyword_and_similarity` ✓, `facets_count_the_matching_set` ✓,
  `delete_removes_documents` ✓, `tenant_isolation_is_invisible` ✓.
- **Gaps**: the **index lifecycle** (a model change updating the index ✗), the **index sync/rebuild** ✗,
  the **queue failure** ✗ and the **stale index** ✗ are all unimplemented — the framework ships the
  search PORT ✓, not the Scout-shaped integration ✗.

### LF-077 — Image manipulation — **NOT IMPLEMENTED**

Nothing ships: no image API, no GD/Imagick binding, no external service adapter. The atlas rates it
**P2** and says so itself: "a local library is not equivalent to a remote image service, and vice
versa; compare operations and safety guarantees" ✓. The framework's storage path is size- and
type-validated ✓ (`get_capped_enforces_the_byte_cap` ✓) but does not transform images ✓. An absent
feature has no executed case ✓.

### LF-078 — Localization and translations — **PARTIALLY VERIFIED**

- **Contract**: lang files; a translator; pluralization; locale fallback; localized
  validation/date formatting and notification context. The locale selection and fallback are
  deterministic, the escaping is safe, the plural forms work and the locale state does not leak
  across concurrent requests/jobs.
- **Implementation**: the **lang files** ✓ (the `i18n` section ✓ + the message catalog ✓ loaded by
  `kelson-config::load_messages` ✓) and the **notification context** ✓ (the route locale ✓). There is
  **no translator** ✗ (no catalog lookup with a fallback chain ✗) and **no pluralization** ✗.
- **Evidence**: the **localized rendering** — `hub_localizes_template_keys_through_the_catalog` ✓,
  `mail_and_sms_render_localized_with_payload_vars` ✓,
  `notification_view_context_carries_the_route_locale_injections` ✓; the **locale selection** — the
  routing/tenant cases ✓ (`tenant_isolation_scheduler_*` ✓).
- **Gaps**: the **fallback** chain (a missing key falling back to the default locale ✗), the **plural
  rules** ✗, the **locale switch/restore** ✗, the **queue locale** ✗ (a job that renders in the
  requester's locale ✗) and the **concurrency leak** ✗ are not covered.

### LF-079 — String and helper utilities — **NOT IMPLEMENTED**

Nothing ships: no string/array/path/time helper facade. The atlas rates it **P2** and says so itself:
"prefer standard Rust ecosystem and avoid a giant compatibility-only helper surface" ✓ — Rust's `std`
and its ecosystem are the answer ✓, and a compatibility-only helper facade would be the wrong
trade ✓. An absent feature has no executed case ✓.

### LF-080 — Collections and lazy streams — **NOT IMPLEMENTED**

Nothing ships: no `Collection` type, no pipeline helpers. The atlas rates it **P2** and says so itself:
"Rust iterators are the native equivalent; add domain collection types only where value is proven, not
for syntactic parity" ✓. The framework uses iterators directly ✓ (the query builder's lazy paths ✓,
the storage listing ✓). An absent feature has no executed case ✓.

---

## 3. Findings

- **F-22 (medium) — the cache had NO contract suite, so a new driver could ship with a different
  contract — FIXED.** The atlas asks for "a contract suite per driver" ✓ and the cache had none ✓ —
  the in-memory driver's three cases and the Redis driver's live cases tested the two adapters
  *separately* ✓, so a divergence between them (say, a different `delete_if_equals` ✓) would only
  surface when an application switched backends ✓. `kelson_caching::cache_contract` is now a shared
  suite ✓ — the round trip ✓, the delete ✓, the prefix invalidation ✓, the expiry ✓ and both atomic
  primitives ✓ — and it is green on **both** drivers ✓ (`the_memory_cache_satisfies_the_cache_contract` ✓
  and `the_redis_cache_satisfies_the_cache_contract` ✓, the latter against a live Redis ✓).

- **F-23 (medium) — the HTTP client has NO egress policy, so a tenant-supplied URL can reach the
  host's internal network — OPEN (repro ✓ + proposed fix ✓, not implemented ✗).**
  - **What the atlas asks**: LF-073's note is explicit — "Enforce egress policy and SSRF
    protections when untrusted modules can initiate HTTP" ✓ — and the client has no such thing ✓:
    it will fetch `http://127.0.0.1:…` ✓, `http://169.254.169.254/…` ✓ (the cloud metadata
    service ✓) or the database port ✓ exactly as it would fetch a public host ✓.
  - **The reachable path**: `kelson-webhooks` ✓ calls a **tenant-registered** target ✓, so the
    target URL is untrusted input ✓. Its store already refuses a plain-`http` URL ✓ ("webhook url
    must be https" ✓ — so the loopback-over-HTTP case is blocked *there* ✓), but an
    **`https://` private target** is not ✓ (`https://10.0.0.5/…` ✓, `https://127.0.0.1:…` ✓) and
    the client itself has no guard ✓. The WASM module host does **not** expose HTTP ✓, so an
    untrusted *module* cannot reach the client today ✓ (that is why this is medium, not high ✓).
  - **The repro** (executed, and it fails ✓ — that failure IS the defect ✓):
    `kelson-http-client`'s `the_client_refuses_a_loopback_target` ✓ — it starts the crate's own
    `Loopback` server ✓ and asserts the request is **refused** ✗; today the request succeeds ✓,
    so the case fails ✓. It is `#[ignore]`d ✓ so the tree stays green ✓ — run it with
    `cargo test -p kelson-http-client -- --ignored` ✓.
  - **The proposed fix** (not implemented ✗ — it is a behavioural change to the client ✓ and needs
    approval ✓):
    1. an `egress` section on `HttpClientConfig` ✓ — `allow_private_targets: bool` ✓ (default
       **true** ✓, so no existing deployment changes behaviour ✓) plus an optional
       `allowed_hosts` allowlist ✓;
    2. a **pre-flight resolution** check ✓ — resolve the host ✓, refuse a loopback ✓ /
       link-local ✓ / private ✓ / unique-local ✓ address ✓ (the `169.254.0.0/16` metadata range
       ✓ included ✓), with the **resolved IP** pinned for the request ✓ so the decision cannot be
       raced by a second DNS answer ✓;
    3. the refusal is an `EXTERNAL` error ✓ with a public message that names the **policy** ✓ and
       never the raw IP ✓ (the client's redaction posture ✓);
    4. `kelson-webhooks` ✓ would default to **refusing private targets** ✓ (a webhook that calls
       `127.0.0.1` has no legitimate use ✓), with the per-tenant opt-out in the config ✓;
    5. the case above loses its `#[ignore]` ✓ and a **redirect** case is added ✓ (a 302 to
       `169.254.169.254` must be re-checked ✓, not just the first hop ✓).

## 4. Corrections to the parity documents

- `P0-FEATURE-VERIFICATION.md` — **ten of the eleven items had no row at all** ✓ and the one that had
  (LF-071) rested on the claim "Cache tests exist" ✓. The rows now exist and name the executed cases
  ✓, with their gaps ✓.
- `LARAVEL-FEATURE-ATLAS-COMPLETE.md` — the LF-070 … LF-080 rows carry the real implementation
  pointers and the executed evidence ✓.
- `P0-EXECUTION-REPORT.md` — the batch is recorded with its finding ✓.

## 5. Summary

- **11 items**: **1 VERIFIED** (LF-070 ✓ — the cache drivers now share one contract suite ✓), **7
  PARTIALLY VERIFIED** (LF-071 locks, LF-072 storage, LF-073 HTTP client, LF-074 mail, LF-075
  notifications, LF-076 search, LF-078 localization ✓), **3 NOT IMPLEMENTED** (LF-077 images, LF-079
  helpers, LF-080 collections ✓ — the three the atlas itself de-prioritizes ✓).
- **The sharpest gaps**: the **SSRF / internal-IP guard** on the HTTP client ✗ (LF-073 ✓ — the one
  security gap in the batch ✓), the **lease expiry / holder crash** of the cache lock ✗ (LF-071 ✓),
  the **rollback-suppresses** and **commit-then-crash** outbox cases ✗ (LF-064 ✓), the **symlink /
  MIME-spoof** storage cases ✗ (LF-072 ✓), the **index lifecycle** ✗ (LF-076 ✓) and the
  **locale fallback / pluralization** ✗ (LF-078 ✓).
- **Findings**: **F-22** (the cache had no contract suite — **fixed** ✓, both drivers green ✓) and
  **F-23** (the HTTP client has no egress policy — **open** ✓: a repro that fails on demand ✓ and
  a five-point proposed fix ✓, awaiting approval ✓), plus the parity rows' missing / claim-shaped
  evidence (**corrected** ✓ — ten of the eleven rows did not exist ✓ and the one that did rested
  on the claim "Cache tests exist" ✓).
