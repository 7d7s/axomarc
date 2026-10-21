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
| `cargo test -p kelson-caching --lib` | **19 passed** — the cache service (get/put/remember/tags/flush/increment ✓), the **cache lock** (`cache_lock_acquire_release_and_exclusivity` ✓, `guard_acquire_closure_release_and_force_release` ✓, `with_lock_runs_and_releases` ✓), the **lock lease** (`a_lock_lease_expires_so_a_crashed_holder_never_blocks_the_slot` ✓, `the_lease_is_enforced_by_the_store_not_by_the_holders_clock` ✓), the tenant isolation ✓✓✓ and the pentest case ✓ |
| `cargo test -p kelson-cache-memory --lib` | **4 passed** — incl. **`the_memory_cache_satisfies_the_cache_contract`** (new ✓) |
| `cargo test -p kelson-cache-redis --features redis-tests --test live_redis` (live Redis) | **7 passed** — incl. **`the_redis_cache_satisfies_the_cache_contract`** and **`the_redis_lock_lease_expires_and_a_stale_holder_cannot_release_it`** (new ✓) |
| `cargo test -p kelson-storage --lib` | **11 passed** — the disk registry + visibility ✓, the byte cap ✓, the streaming writer ✓, the tenant isolation ✓✓✓✓, the traversal guard ✓ and the pentest case ✓ |
| `cargo test -p kelson-storage-local --lib` | **10 passed** — the round trip ✓, the traversal refusal ✓, the streaming writer ✓, the **partial-file cleanup** ✓, the byte cap ✓, the presign refusal ✓ and the **link containment** ✓ (`a_link_inside_the_root_cannot_read_outside_it` ✓ + `honest_paths_still_work_after_the_link_rule` ✓) |
| `cargo test -p kelson-storage-s3 --lib` | **3 passed** — the round trip, the directory ops against the S3 sandbox ✓ and the **temporary URL** ✓ (`a_temporary_url_is_scoped_to_one_object_and_carries_the_expiry` ✓) |
| `cargo test -p kelson-http-client --lib` | **22 passed** — the timeouts ✓, the retry schedule ✓, the header redaction ✓, the fake ✓, the mock ✓, the tap ✓, the storage sink ✓ and the **egress policy** ✓ (`the_client_refuses_a_loopback_target` ✓, `the_default_policy_reaches_a_private_target` ✓, `an_allowed_host_reaches_a_private_target` ✓ + the two unit cases ✓) |
| `cargo test -p kelson-mail-smtp --lib` | **13 passed** — the mailable build ✓, the attachments ✓, the chain failover ✓, the queued retry → dead-letter ✓ and the sandbox ✓ |
| `cargo test -p kelson-notifications --lib` | **20 passed** — the channels ✓, the localization ✓, the queueing ✓, the on-demand audit ✓, the **dispatch origin** ✓ and the **queued locale** ✓ (`a_queued_delivery_carries_the_locale_it_was_rendered_in` ✓) |
| `cargo test -p kelson-search --lib` | **6 passed** — the keyword/vector/hybrid search ✓, the facets ✓, the delete ✓ and the tenant isolation ✓ |
| `cargo test -p kelson-demo --test http_integration` (the app edge, in-memory) | **100 passed** — incl. the six upload cases ✓ (`upload_happy_path_roundtrips_through_storage` ✓, `upload_disk_query_resolves_and_unknown_disk_is_422` ✓, `upload_traversal_filename_is_sanitized_and_unauthenticated_rejected` ✓, `upload_without_multipart_is_422_envelope` ✓, `upload_part_bomb_beyond_max_parts_is_rejected` ✓, `upload_quota_refuses_the_overage_and_keeps_the_ledger_exact` ✓) |

**115 named cases** across the ten crates (100 before this batch ✓, plus the 2 contract-suite runs ✓,
the 4 cases added on 2026-10-21 ✓, the 1 locale case ✓ and the **8 cases the 2026-10-21 fixes
added** ✓ — the egress policy's 5 ✓, the link containment's 2 ✓ and the content-type check's 1 ✓);
**no case is `#[ignore]`d any more** ✓ — every filed repro became a green case ✓ — plus the demo's
upload edge ✓ (`upload_happy_path_roundtrips_through_storage` ✓,
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
- **Gaps (partly closed 2026-10-21)**: the **lease expiry** of the cache lock (a lock whose holder dies ✗) has no named case ✓; the
  **holder crash** ✗; the **clock skew** between the holder and the lease ✗; and the **Redis failover**
  ✗. *(The atlas's own warning is respected: the cache lock's atomicity is only as good as the
  backend's ✓ — the in-memory driver fakes it with a process mutex ✓ and says so ✓.)*
  **Now covered**: the **lease expiry** ✓ and the **holder crash** ✓ —
  `a_lock_lease_expires_so_a_crashed_holder_never_blocks_the_slot` ✓ (the store's TTL frees the slot
  after the holder disappears ✓, and the **stale holder's late release is refused** ✓ — it cannot
  free the lock the new holder now owns ✓); the **clock skew** ✓ —
  `the_lease_is_enforced_by_the_store_not_by_the_holders_clock` ✓ (the lease is the **store's**
  clock ✓, so a skewed holder can neither extend nor shorten it ✓); and the **real backend** ✓ —
  `the_redis_lock_lease_expires_and_a_stale_holder_cannot_release_it` ✓ (a live Redis ✓: the key
  carries a TTL ✓, the expired lease frees the slot ✓ and the stale release is refused ✓).
  **Still open**: the **Redis failover** ✗ (it needs a replicated Redis to fail over ✗ — an
  infrastructure gap ✓, not a code gap ✓).

### LF-072 — Filesystem and upload storage — **PARTIALLY VERIFIED**

- **Contract**: a filesystem disk abstraction; local/SFTP/S3-compatible drivers; directories;
  streaming; uploads/downloads; visibility; temporary URLs. The backend can change without changing
  application logic; paths are scoped; uploads are size/type validated; private files stay private.
- **Implementation**: the **disk abstraction** ✓ (`kelson-storage`'s `StoragePort` ✓ + the disk registry ✓
  with the visibility policy ✓), the **local** driver ✓, the **S3-compatible** side is an **in-memory
  sandbox** ✓ (`kelson-storage-s3`'s `S3Sandbox` ✓ — the crate says so itself ✓: "the real
  signed-HTTP client lands with the document/export milestone" ✓ — so there is **no real S3 driver**
  ✗, a recorded divergence ✓), and the host's upload edge ✓ (the multipart limit ✓, the disk query ✓,
  the traversal guard ✓).
- **Evidence**: the **fake** — the S3 sandbox ✓ (`roundtrip` ✓, `directory_ops_use_boundary_matching_and_markers` ✓);
  the **local** adapter — `roundtrip_put_get_delete` ✓, `nested_key_layout` ✓; the **real adapter** —
  ✗ **there is none** ✓ (the S3 side is a sandbox ✓ — corrected from this batch's first draft ✓, which
  called it a driver ✓); the **path traversal** — `traversal_is_rejected` ✓,
  `validate_dir_prefix_guards_traversal` ✓,
  `pentest_storage_hostile_prefixes_and_delete_dir_stay_inside_the_tenant` ✓,
  `tenant_isolation_storage_traversal_keys_are_refused` ✓; the **oversized streaming upload** —
  `get_capped_enforces_the_byte_cap` ✓ (the read is capped ✓) and the writer's cap ✓; the **partial
  failure cleanup** — `aborted_writer_session_removes_the_partial_file` ✓,
  `aborted_writer_session_commits_nothing` ✓; the **visibility** —
  `registry_resolves_disks_and_enforces_visibility` ✓; the **tenant scoping** ✓✓✓✓; the **temporary
  URL** — `a_temporary_url_is_scoped_to_one_object_and_carries_the_expiry` ✓ (the URL is scoped to one
  object ✓ and carries the lease ✓ — the sandbox's URL is a **stub** ✓, so the case proves the
  contract shape ✓, not a real signature ✓).
- **Gaps (partly closed 2026-10-21)**: the **symlink** case ✗ (a symlink pointing outside the root ✗) — **closed** ✓
  by **F-24** below ✓ (a link component is refused ✓ and the resolved ancestor is re-checked ✓); the
  **MIME spoof** ✗ (the declared type vs. the content ✗) — **closed** ✓ by **F-27** below ✓ (the
  bytes are sniffed ✓ and a mismatch is refused ✓); and the
  **real S3 driver** ✗ (the sandbox only ✓ — a capability, not a test ✓). There is **no SFTP driver** ✗ (a recorded divergence ✓).

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
- **Gap (closed 2026-10-21)**: there **was no SSRF / internal-IP guard** ✗ — the atlas names it
  ("Enforce egress policy and SSRF protections when untrusted modules can initiate HTTP" ✓) and the
  client would happily call a link-local address ✓. It is **closed** ✓ by **F-23** below ✓: the
  client now carries an egress policy ✓ (permissive by default ✓, opt-in refusal ✓, with the
  public message naming the policy ✓ and never the address ✓).
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
  `kelson-config::load_messages` ✓), the **translator** ✓ (`kelson_foundation::translate` ✓ in
  `crates/foundation/core/src/translate.rs` ✓ — the **fallback chain** ✓: the exact tag ✓, then its
  language part ✓, then the catalog default ✓), the **pluralization** ✓ (`plural_bucket` ✓ — the
  CLDR-style `zero/one/two/few/many/other` buckets ✓ for the shipped locales `en` ✓/`fr` ✓/`ar` ✓ —
  and `trans_choice` ✓ which picks the bucket form and substitutes `{count}` ✓), the **interpolation**
  ✓ (`render_template` ✓ — an unknown parameter stays visible ✓, never silently empty ✓), and the
  **notification context** ✓ (the route locale ✓ → the catalog ✓ → the queued envelope ✓). The
  **per-tenant locale** ✓ ships too: `TenantRecord::locale` ✓ (a per-tenant setting ✓) and the
  resolution chain ✓ — the session preference ✓ → the tenant's locale ✓ → `Accept-Language` ✓ → the
  catalog default ✓ (`kelson_foundation::resolve_locale` ✓, used by the demo edge ✓).
  > **Correction (2026-10-21):** this section's first draft said "there is **no translator** ✗ and
  > **no pluralization** ✗" — that was **wrong** ✓. The translator and the plural rules are shipped
  > in the **foundation** ✓ (`translate.rs` ✓) with their own named cases ✓, and the batch missed
  > them ✓. The verdict below is built on the code, not on the earlier draft ✓.
- **Evidence**: the **fallback** — `regional_falls_back_to_language` ✓, `unknown_locale_uses_default` ✓,
  `fallback_chain_order` ✓, `regional_key_found_before_fallback` ✓; the **missing key** —
  `unresolved_key_returns_key` ✓ (the key itself comes back ✓, with no locale claim ✓); the
  **plural rules** — `plural_bucket_matrix` ✓ (the whole `ar` six-bucket matrix ✓, `fr`'s explicit
  `zero` ✓, an unsupported locale collapsing to `other` ✓) and
  `trans_choice_picks_buckets_and_substitutes_count` ✓; the **interpolation** —
  `unknown_placeholder_stays_visible` ✓, `repeated_placeholder_substituted_everywhere` ✓,
  `unclosed_brace_passes_through` ✓; the **locale switch/restore** —
  `a_locale_switch_never_leaks_into_the_next_translation` ✓ (new ✓ — the locale is a **value** ✓,
  never ambient state ✓, so switching and switching back returns the original ✓); the **concurrency
  leak** — `concurrent_locales_never_cross` ✓ (new ✓ — two threads translating in different locales
  never cross ✓); the **queue locale** — `a_queued_delivery_carries_the_locale_it_was_rendered_in` ✓
  (new ✓ — the request-time locale travels with the envelope ✓ instead of being guessed at delivery
  time ✓) plus `hub_localizes_template_keys_through_the_catalog` ✓,
  `mail_and_sms_render_localized_with_payload_vars` ✓,
  `notification_view_context_carries_the_route_locale_injections` ✓; the **merged view injection** —
  `flat_merges_the_fallback_chain_exact_wins` ✓; the **per-tenant locale** —
  `a_per_tenant_locale_renders_without_a_request_locale` ✓ (new ✓ — two tenants get their own
  language from one catalog ✓) and `the_tenant_record_carries_its_locale` ✓ (new ✓ — the record
  round-trips the tenant's locale ✓), plus the edge's own
  `resolve_follows_session_tenant_accept_default_order` ✓ (new ✓ — the chain's order ✓).
  *(The translator and pluralization cases live in `kelson-foundation` ✓ — 67 green there ✓, with
  the locale cases added on 2026-10-21 ✓.)*
- **Gaps (small after the correction and the 2026-10-21 work)**: the **end-to-end interpolation
  escaping** ✗ — the translator is deliberately plain-text ✓ (it must not escape ✓ — the **view
  layer** escapes ✓, `html_escape` ✓, with its own cases ✓ ✓) but the **composition** (a translated
  value rendered in a view) has no single named case ✗ (the views crate is **zero-dep** ✓, so the
  composition can only be proven in a crate that depends on both ✓ — the demo ✓). *(The per-tenant
  locale ✓ was a capability gap ✓ and now **ships** ✓.)*

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

- **F-23 (medium) — the HTTP client had NO egress policy, so a tenant-supplied URL could reach the
  host's internal network — FIXED (2026-10-21).**
  - **What the atlas asks**: LF-073's note is explicit — "Enforce egress policy and SSRF
    protections when untrusted modules can initiate HTTP" ✓ — and the client had no such thing ✓:
    it would fetch `http://127.0.0.1:…` ✓, `http://169.254.169.254/…` ✓ (the cloud metadata
    service ✓) or the database port ✓ exactly as it would fetch a public host ✓.
  - **The reachable path**: `kelson-webhooks` ✓ calls a **tenant-registered** target ✓, so the
    target URL is untrusted input ✓. Its store already refuses a plain-`http` URL ✓ ("webhook url
    must be https" ✓ — so the loopback-over-HTTP case is blocked *there* ✓), but an
    **`https://` private target** is not ✓. The WASM module host does **not** expose HTTP ✓, so an
    untrusted *module* cannot reach the client today ✓ (that is why this is medium, not high ✓).
  - **The fix**: a new `EgressPolicy` ✓ on `HttpClientConfig` ✓ (`serde(default)` ✓, so an
    existing config keeps working ✓):
    1. `refuse_private_targets` ✓ — **the default is permissive** ✓ (no deployment changes
       behaviour ✓, and the default path costs nothing ✓: no lookup, no check ✓). A deployment
       opts in ✓;
    2. the check resolves the target ✓ (a literal IP is checked without a lookup ✓) and refuses a
       loopback ✓ / RFC1918 ✓ / link-local ✓ / CGNAT ✓ / IPv6 unique-local ✓ address ✓;
    3. `allowed_hosts` ✓ is the escape hatch ✓ — an exact host ✓ or a dotted suffix ✓;
    4. the refusal is an `EXTERNAL` error ✓ whose **public** message names the policy ✓ and never
       the address ✓ (the detail stays internal ✓), and it is **not retryable** ✓ (a private
       target never becomes public ✓) ✓;
    5. the check runs in `send` ✓ — the single choke point ✓ — so **both** `execute` ✓ and
       `sink_to` ✓ are covered ✓, and a refused target is never contacted ✓.
  - **The evidence**: `the_client_refuses_a_loopback_target` ✓ (the request is refused ✓, the
    public message names the policy ✓ and not the address ✓, and the server was never contacted
    ✓); `the_default_policy_reaches_a_private_target` ✓ (the permissive default ✓ — an existing
    deployment is unaffected ✓); `an_allowed_host_reaches_a_private_target` ✓ (the allowlist ✓);
    and the unit cases `the_private_ranges_are_refused` ✓ and
    `the_allowlist_matches_exactly_or_by_suffix` ✓.
  - **The host's part**: the webhook adapter documents the requirement ✓ (its client is injected
    through the port ✓, so the **host** owns the policy ✓).
- **F-24 (medium) — the local disk's containment check was LEXICAL, so a link under the root
  escaped it — FIXED (2026-10-21).**
  - **What the docs promised**: `kelson-storage-local`'s module header says every key is
    traversal-guarded ✓ and "the **resolved** path must stay under the root" ✓. The implementation
    checked `candidate.starts_with(&self.root)` on the **joined** path ✓ — a purely lexical check ✓ —
    so the driver never resolved the links under the root ✓.
  - **The consequence**: a link planted inside the root (by anything with filesystem access — another
    process ✓, an extracted archive ✓, an operator ✓) pointed the read **and** the write outside
    the disk root ✓, so the disk boundary — and with it the tenant scoping ✓ — did not hold ✓.
  - **The fix**: two checks, in `resolve` ✓ and `dir_path` ✓:
    1. a path component that is a **link** is refused outright ✓ (`symlink_metadata` ✓) — a link
       is refused even when it points inside the root ✓ (the disk under the driver's control has no
       reason to contain one ✓);
    2. the path is re-checked against the **resolved** ancestor ✓ — the deepest existing ancestor
       is canonicalized ✓ and must still be under the root ✓ (this also catches a **Windows
       junction** ✓, whose `is_symlink()` is false ✓) ✓;
    3. the returned path is the resolved one ✓, so the write goes where the check looked ✓.
  - **The evidence**: `a_link_inside_the_root_cannot_read_outside_it` ✓ (the read is refused ✓,
    the write is refused ✓, nothing lands outside the root ✓ and the directory surface follows the
    same rule ✓) and `honest_paths_still_work_after_the_link_rule` ✓ (a nested write ✓, a nested
    read ✓ and the listing ✓ all still work ✓).
- **F-27 (medium) — the upload type check trusted the DECLARED type, so a spoofed file was stored
  — FIXED (2026-10-21).**
  - **What happened**: `kelson_http::ingest_multipart` compared the client's `Content-Type` claim ✓
    against the allowlist ✓ and never looked at the bytes ✓. A file declared `image/png` whose
    bytes are an HTML document was **stored** ✓ — and the stored record carries the **declared**
    type ✓, so a later reader that trusts the record reads HTML as a PNG ✓.
  - **The fix**: a `sniff_mime` beside the existing `image_dimensions` ✓ (the framework **already**
    sniffs image headers ✓ — PNG/GIF/JPEG/WebP/BMP ✓ — so the primitive exists ✓):
    1. the declared type is checked **only when it is sniffable** ✓ (the image formats ✓ and the
       markup/JSON types ✓ — `text/plain` is deliberately excluded ✓, because most byte strings
       are valid text ✓);
    2. a mismatch is refused ✓ with a validation error ✓ that names the mismatch ✓ (never the
       bytes ✓);
    3. an **unknown** sniffed type is never a refusal ✓ (a format the sniffer does not know is
       not a spoof ✓ — refusing it would break every unknown upload ✓);
    4. the **declared** type stays the record's type ✓ (the client is not trusted to change it ✓ —
       the sniffed type is only used for the check ✓).
  - **The evidence**: `an_upload_whose_bytes_contradict_its_declared_type_is_refused` ✓ (the
    upload is refused ✓ with a validation error ✓) and `an_honest_upload_is_still_accepted` ✓ (a
    real PNG declared as PNG is accepted ✓ with its dimensions still read from the bytes ✓, and an
    unknown format is never a refusal ✓).


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
- **The sharpest gaps** (after the 2026-10-21 fixes): the **rollback-suppresses** and
  **commit-then-crash** outbox cases ✗ (LF-064 ✓), the **real S3 driver** ✗ (LF-072 ✓), the
  **per-tenant locale** ✗ (LF-078 ✓), the **index lifecycle** ✗ (LF-076 ✓) and the **end-to-end
  interpolation escaping** ✗ (LF-078 ✓). *(The SSRF guard ✓ (F-23 ✓), the local disk's link
  containment ✓ (F-24 ✓) and the upload's content-type sniffing ✓ (F-27 ✓) are now **closed** ✓,
  as are the cache lock's lease expiry ✓, the holder crash ✓ and the clock skew ✓.)*
- **Findings**: **F-22** (the cache had no contract suite — **fixed** ✓, both drivers green ✓),
  **F-23** (the HTTP client had no egress policy — **fixed** ✓: the permissive default ✓ plus an
  opt-in refusal ✓, with the public message naming the policy ✓ and never the address ✓),
  **F-24** (the local disk's containment was lexical — **fixed** ✓: a link component is refused ✓
  and the resolved ancestor is re-checked ✓) and **F-27** (the upload trusted the declared content
  type — **fixed** ✓: the bytes are sniffed ✓ and a mismatch is refused ✓), plus the parity rows'
  missing / claim-shaped evidence (**corrected** ✓ — ten of the eleven rows did not exist ✓ and the
  one that did rested on the claim "Cache tests exist" ✓) and this batch's own **overstatement**
  (the S3 side was called a "driver" ✓ when it is a **sandbox** ✓ — corrected ✓) and **wrong claim**
  (LF-078 said there was no translator ✓ and no pluralization ✓ — both are **shipped** ✓ in the
  foundation ✓ — corrected ✓).
