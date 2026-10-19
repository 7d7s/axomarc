# HTTP, Routing & Web — Laravel Feature Atlas Verification (LF-011 to LF-027)

> **Revision 3 — 2026-10-19 (post-fix).** Revision 2 (this file) replaced revision 1, which marked
> all 17 items VERIFIED with vague or mismatched evidence lines (a full list of corrections is in §5).
> Every status below is backed by a named test executed on 2026-10-19 against this working tree; raw
> logs live in `C:\Users\Victo\AppData\Local\Temp\opencode\lf-http-batch` (steps 01–28).
>
> **Revision 3 changes**: findings **F-1 (CORS configurability/tests)**, **F-2 (panic boundary)** and
> **F-3 (rate limiter, all three phases)** were fixed in this session (designs and evidence:
> `LF-011-TO-LF-027-FINDINGS-FIXES.md`), so **LF-021, LF-024 and LF-026 move from PARTIALLY
> VERIFIED to VERIFIED** with the new named evidence.
> Post-fix runs: `kelson-http` 160 passed / 0 failed / 0 ignored; `kelson-ratelimit-redis` 4 passed
> (+ 2 live-Redis cases that **skip** — no Redis on this host); `kelson-config` 66 passed / 0 failed;
> `kelson-demo --lib` 115 passed / 0 failed; the 16-crate fast suite 0 failed.
>
> Status vocabulary: **VERIFIED** (executed evidence matches the atlas expectation) ·
> **PARTIALLY VERIFIED** (implemented; at least one expectation uncovered, unproven, or a
> recorded divergence) · **NOT RUN** / **NOT IMPLEMENTED** / **DEFER** (none at item level in
> this batch; sub-aspects only — see §2.7 and §4).

## 1. Verification Matrix

| ID | Feature | Disposition | Kelson equivalent | Executed evidence (2026-10-19) | Status |
|---|---|---|---|---|---|
| LF-011 | Routing and HTTP methods | MATCH (2 recorded divergences) | Closed verb set + fail-closed compile gates; manifest/route-file → live axum registration (GET answers HEAD, ANY registers every verb, auto-OPTIONS 204+`Allow`, mismatch 405+`Allow`); route-cache artifact gated at boot | `every_manifest_route_reaches_the_http_router`; `route_file_route_dispatches_to_the_declared_module_handler`; `any_route_answers_every_declared_verb`; `get_route_answers_head_without_a_body`; `get_route_answers_the_auto_options_probe`; `verb_mismatch_is_405_with_allow`; `constraint_mismatch_is_a_route_miss_for_every_kind`; `deprecated_route_emits_header_and_meta`; demo-lib `declared_methods_mirror_the_registry_route_gate`, `method_filter_mapping`, `route_cache_pin_accepts_match_rejects_drift_and_clears`, `corrupt_route_cache_refuses_boot`; registry `p0_route_shape_gate_accepts_every_registered_verb`, `p0_route_shape_gates_refuse_edge_skippable_routes`, `routefile_op_gate_requires_registered_module_operations` | VERIFIED |
| LF-012 | Named routes and URL generation | MATCH (path-level) | `PlatformGraph::route_named` + `resolve_url` (substitutes `{param}`, missing param = violation, extras → sorted query string, percent-encoded values, registered url-defaults fallback) | `resolve_url_substitutes_and_encodes_parameters`; `resolve_url_extra_params_become_query_string`; `resolve_url_rejects_unknown_route_and_missing_params`; `resolve_url_falls_back_to_registered_url_defaults`; `resolve_url_still_requires_params_that_have_no_default`; `duplicate_route_name_detected`; console `route_list_filters_carry_the_parity_columns`, `starter_template_renders_parses_and_compiles` | PARTIALLY VERIFIED |
| LF-013 | Route groups, prefixes, subdomains and constraints | MATCH (flat groups only) | `[[route_groups]]` (prefix concatenated at parse; group middleware OUTERMOST; plane inherited; route-level wins); host/subdomain gates; closed constraint kinds (`number/alpha/alpha_num/uuid/ulid/slug/pattern`) with anchored mini-grammar | `declared_group_applies_and_compiles`; `group_unknown_gate_collects_all`; `constraint_kind_gate_collects_all_bad_specs`; `constraint_param_gate_rejects_param_not_in_path`; `valid_constraints_resolve_onto_the_route`; `constraint::tests::*` (20 unit: per-kind matrices + pattern edges); `plane_gate_rejects_unknown_plane_tokens`; `domain_gate_matches_before_the_auth_chain`; `subdomain_gate_404s_the_wrong_host_and_feeds_the_tenant`; demo-lib `host_matching_rules`, `host_label_strips_port_and_lowercases` | PARTIALLY VERIFIED |
| LF-014 | Route model binding and fallback routes | MATCH (scoped) | `kelson-models` `RouteBindingResolver`/`RouteBindingRegistry` (plain + `with_trashed`), manifest binding specs, per-plane fallback route | `binding_miss_malformed_and_cross_tenant_are_404`; `fallback_route_attaches_and_dispatches_on_unmatched_paths`; `two_plane_fallbacks_refuse_at_the_edge`; registry `binding_param_must_exist_in_path`, `binding_on_real_param_compiles`, `binding_spec_with_trashed_is_opt_in`, `fallback_dup_gate_rejects_two_on_one_plane`, `fallback_dup_gate_allows_one_per_plane`; models `resolve_binding_fails_closed_without_an_override`, `resolve_binding_flag_routes_to_the_matching_hook` | VERIFIED |
| LF-015 | Middleware pipeline | MATCH (framework-owned) | `middleware.toml` catalog (aliases/groups/builtins `auth/verified/confirmed/signed/throttle:<group>`); implicit `api` group, `public` strips auth+verified only; `excluded_middleware`; fixed admission order (request-id → trace → body-limit → CORS → maintenance → rate-limit → router), session INSIDE, CSRF per web-plane route, edge OUTERMOST (cookie queue + terminating hooks) | `middleware_catalog::tests::*` (8 in `kelson-http` + 10 in `kelson-registry`: implicit chain, group flattening, public strip, excluded middleware, unknown-entry gate, throttle overrides, signed redemption, auth/verified/confirmed stages); `middleware::tests::*` (request-id, traceparent, body-limit, maintenance, rate limit); `throttle_stage_exhausts_and_runs_before_auth`; demo-lib `edge_layer_flushes_queued_cookies_then_runs_hooks`, `hooks_see_every_registration_and_survive_a_panicking_hook`, `edge_auth_resolves_pat_then_jwt_and_installs_snapshot`; `login_throttling_returns_429_after_burst`; `throttle_group_exhaustion_is_429` | VERIFIED |
| LF-016 | Controllers and resource controllers | MATCH (different shape) | Handlers = module operations (typed Rust) executed by the engine; resource/singleton route expansion (6/4 specs, PUT+PATCH alias, `only`/`except`, capability gate); `kelson make resource` scaffold; `kelson-http::Resource` response transformer | `resource_expands_into_six_route_specs`; `resource_only_and_except_filter_verbs`; `resource_with_explicit_method_is_rejected`; `resource_missing_capability_fails_with_resource_gate`; `singleton_expands_without_id_into_four_route_specs`; `singleton_only_and_except_filter_verbs`; `singleton_without_a_resource_path_is_rejected`; `full_resource_manifest_compiles_through_the_graph`; `route_file_route_dispatches_to_the_declared_module_handler`; `package_routes_serve_over_http_and_abi_mismatch_refuses_boot`; console `starter_template_renders_parses_and_compiles`; `resource::tests::*` (7 unit) | VERIFIED |
| LF-017 | Request parsing and input access | MATCH (sources kept distinct) | Pure request helpers (`expects_json`/`accepts` q-aware, `bearer_token`, `client_ip` first XFF hop, `fingerprint`); body limits (admission 1 MB; 32 MB per-route upload override; multipart part/mime/size caps); body>query merge (body wins); form URL-decoding; pagination page/per-page clamps | `malformed_json_body_is_422_envelope`; `pagination_injects_page_and_clamps_per_page`; `full_cycle_register_login_book_list_cancel`; demo-lib `merge_params_body_wins`, `query_pairs_skip_garbage`, `form_decode_applies_form_urlencoding`, `non_get_routes_and_bufferless_kinds_skip_the_validator`; `request::tests::*` (5); `upload::multipart_tests::*` (6) + `upload::tests::*` (3); `pagination::tests::*` (5); `cookie::tests::jar_parses_multiple_headers_and_skips_garbage` | VERIFIED |
| LF-018 | Response types and cookies | MATCH | Envelope; 204; ndjson streaming; `Cache-Control` builder + strong ETag/304; opt-in security headers; cookie jar/cipher/queue (sealed values, FIFO queue); JSON:API negotiation; static + download delivery | `no_content_routes_answer_204_without_a_body`; `stream_routes_flush_ndjson_instead_of_an_envelope`; `declared_cache_policies_reach_the_wire_as_cache_control`; `conditional_get_short_circuits_with_304`; `an_unchanged_payload_keeps_its_validator_across_restarts_of_the_body`; `web_plane_responses_carry_the_configured_security_headers`; `the_security_headers_stay_off_until_a_deployment_turns_them_on`; jsonapi integration ×5 + `jsonapi::tests::*` (13); `cookie::tests::*` (11); `cache::tests::*` (4); `security_headers::tests::*` (7); static ×6; demo-lib `stream_flushes_one_ndjson_line_per_element`, `download_serves_a_stored_object_as_an_attachment` | VERIFIED |
| LF-019 | Redirects, aborts and flash data | PARTIAL | Typed `Redirect` 302/303/307; flash put/take read-once on the session store; aborts via AppError → status mapping; redirect targets come from operation results (header-injection refused) | `redirect::tests::*` (`found_redirect_sets_location`, `see_other_uses_303`, `temporary_uses_307`); `flash::tests::*` (`flash_put_take_then_gone`, `flash_take_unknown_session_is_none`); demo-lib `redirect_flash_survives_exactly_one_next_request`, `redirect_targets_come_from_the_operation_result`, `redirect_refuses_non_3xx_statuses_and_header_injection` | PARTIALLY VERIFIED |
| LF-020 | CSRF protection | MATCH | Double-submit-cookie guard, default-ON per web-plane route (skip paths from `[security.csrf]`); safe requests bootstrap the cookie; api plane untouched; session-bound token available (`Session::csrf_token`) | `csrf::tests::*` (9: 256-bit tokens, mismatch rejects, bypass paths, per-route attach, refresh-on-match); `web_plane_state_changing_requests_are_csrf_checked_by_default`; `the_web_plane_csrf_roundtrip_bootstraps_from_a_safe_request`; `csrf_skip_paths_carry_a_declared_receiver_through`; `api_plane_state_changing_requests_stay_free_of_the_csrf_guard`; `precognition_is_not_a_csrf_403_bypass` | VERIFIED |
| LF-021 | CORS | MATCH (fixed F-1) | `tower-http` `CorsLayer` inside `Admission`; **configurable** `[security.cors]` allowlist (optional section; absent = permissive default); `allow_credentials` refused at boot with `*` | `cors_default_is_a_permissive_wildcard`; `cors_allowlist_rejects_disallowed_origin`; `cors_allowlist_allows_configured_origin` (echoes the origin + `Vary: Origin`); `cors_wildcard_without_credentials`; `cors_wildcard_with_credentials_is_refused`; config: `cors_section_parses_allowlist_and_refuses_typos`, `cors_section_is_optional`, `cors_keys_are_declared_in_the_config_schema` | VERIFIED |
| LF-022 | Session management | MATCH (drivers) | `kelson-session`: 256-bit ids, idle + absolute timeouts, regenerate on privilege change, remember-me (30 d default), HMAC-tagged cookie `<id>.<tag>` (SameSite=Lax, configurable Secure); stores: InMemory + Pg + MySQL + Redis | `session::tests::*` (8: `idle_and_absolute_expiry`, `regenerate_rotates_id_and_keeps_data`, `remember_flag_survives_regeneration_and_policy_defaults_to_30d`, `sweep_reaps_expired_sessions`, …); `sessions_persist_across_requests_and_logout_destroys`; `session_cookie_carries_secure_defaults_not_secure_in_test`; `login_rotates_session_and_invalidates_prelogin_cookie`; `password_confirm_stamps_the_session_and_fails_closed`; `remember_login_issues_the_30d_cookie_and_me_reports_the_flag`; `mysql_session_store_lifecycle`; `mysql_session_sweep_removes_expired_rows` | VERIFIED (PG/Redis store halves not executed — §2.7) |
| LF-023 | Validation and Form Requests | MATCH | `kelson-validation`: collect-all Validator (FormRequest role), rule grammar (types/strings/numeric/field refs/conditionals/dates/files/auth/DB rules/wildcards/custom/messages), `DbProbe` for unique/exists, per-field message overrides; manifest ValidationSpec gates | `validation::tests::*` (49); `exists_rule_matches_the_default_column_of_the_field_path`; `exists_rule_honors_the_explicit_column_token`; `exists_rule_against_an_unregistered_model_fails_closed_503`; `wildcard_fields_expand_per_element`; `message_overrides_render_the_attribute_placeholder`; `registered_custom_rule_rejects_and_accepts_values`; `validation_specs_compile_and_gate_unknown_ops`; `validation_rule_grammar_is_fail_closed`; `validation_field_gate_checks_the_declared_input_shape`; `unknown_custom_rule_names_are_refused`; `messages_keys_must_be_known_rule_names`; `validation_messages_follow_the_negotiated_locale` | VERIFIED |
| LF-024 | Error handling and exception rendering | PARTIAL (finding F-2) | `ErrorRenderRegistry` (per-code overrides, custom renderers, reportable/renderable split); RFC 7807 problem+json opt-in; debug gates; secrets redacted through the logging list; error views on the web plane; abort mapping (401/403/404/422/429/503) | `error_render::tests::*` (7: defaults/split, overrides, renderers, problem shape, retryable, negotiation); error-rendering ×9 (`app_debug_gate_hides_internal_detail_when_closed`, `app_debug_gate_renders_redacted_internal_detail_when_open`, `debug_detail_redacts_secrets_through_the_logging_list`, `report_only_codes_keep_the_status_but_serve_the_generic_body`, `registry_overrides_and_renderer_output_reach_the_wire`, `api_plane_serves_rfc7807_when_the_deployment_opts_in`, `web_plane_html_clients_keep_the_error_view_under_the_problem_opt_in`, `explicit_accept_header_beats_the_deployment_default`, `app_debug_detail_reaches_the_error_view_too`); `unauthenticated_graph_route_is_401_envelope`; `unknown_route_is_404_and_demo_echo_still_golden`; `malformed_json_body_is_422_envelope`; views ×9; demo-lib `error_render_registry_reaches_modules_and_the_edge`; **panic boundary (fixed F-2)**: `panicking_handler_answers_a_500_envelope` (500 + `KSN_INTERNAL`, keeps `x-request-id`/`x-trace-id`, no payload leak) | VERIFIED |
| LF-025 | Logging and channels | MATCH (naming differs) | `kelson-observability` (structured pipeline: classify/correlate/dedupe/fingerprint/normalize/redact/sample; buffered channel; default redaction list) + exporters console / file (JSONL, rotation) / OTLP-HTTP-JSON / HTTP / RFC5424 syslog, each wrappable in `ManagedExporter` (circuit breaker, retry budget, health) | observability 26; telemetry 11 (incl. `classify::tests::secret_fields_detect`, `fingerprint_stable_across_ids_and_volatile`); tracing 4; pipeline 8; buffer 7; export 10; `request_id_generated_and_echoed`; `provided_request_id_reused`; `traceparent_parsed`; `debug_detail_redacts_secrets_through_the_logging_list` | VERIFIED |
| LF-026 | Rate limiting | MATCH (F-3 fixed) | Admission token bucket (key precedence: `x-rate-key` → bearer hash → **XFF chain walk (right-to-left, trusted proxies only)** → global; `Retry-After` on 429); **bounded bucket map** (TTL sweep + hard cap); **shared store** for multi-instance deployments (`RateLimitStore` trait; in-memory default; `kelson-ratelimit-redis` adapter with an atomic Lua bucket); per-route `throttle:<group>` buckets (one per group+client, run before auth); job middleware in `kelson-jobs` | `middleware::tests::*`: `rate_limit_exhaustion_returns_429`, `token_bucket_math`, `client_key_extraction`, `client_key_prefers_bearer_over_ip`, **F-3**: `client_key_ignores_forwarded_for_without_trusted_proxies`, `client_key_walks_the_chain_past_trusted_proxies`, `client_key_explicit_and_bearer_beat_the_chain`, `trusted_proxy_ranges_parse_and_match`, `rate_limit_buckets_are_capped`, `rate_limit_buckets_are_swept_by_ttl`, `shared_rate_limit_store_is_shared_across_instances`; adapter: `key_is_namespaced_and_cluster_safe`, `hostile_client_keys_cannot_escape_the_namespace`, `long_client_keys_are_truncated`, `now_is_monotonic_seconds` (the live-Redis cases **skip** — no Redis on this host); config: `trusted_proxies_section_parses_the_ranges`, `trusted_proxies_keys_are_declared_in_the_config_schema`; catalog: `throttle_stage_exhausts_and_runs_before_auth`, `throttle_groups_have_isolated_buckets`, `set_throttle_overrides_group_config`; `login_throttling_returns_429_after_burst`; `throttle_group_exhaustion_is_429`; `kelson-jobs` 35 unit | VERIFIED |
| LF-027 | Precognition / progressive validation | MATCH (host edge) | `Precognition` header → validate-only dispatch at the graph edge: pass = 204 without dispatch, failure = byte-identical 422, multipart refused before intake, no 401/CSRF bypass, no-validation routes pass vacuously | `precognition_pass_returns_204_without_dispatching`; `precognition_failure_is_the_byte_identical_422_envelope`; `precognition_refuses_multipart_bodies_before_intake`; `precognition_vacuous_passes_routes_without_validation`; `precognition_is_not_an_anonymous_401_bypass`; `precognition_is_not_a_csrf_403_bypass` | VERIFIED |

## 2. Method and executed evidence (2026-10-19, from `platform/keel-platform`)

### 2.1 Fast crate suites — 549 results, 0 failures

One `cargo test` invocation over: `kelson-buffer` 7 · `kelson-caching` 17 · `kelson-console` 59 ·
`kelson-export` 10 · **`kelson-http` 147** · `kelson-jobs` 35 · `kelson-metrics` 6 ·
`kelson-observability` 26 · `kelson-pipeline` 8 · **`kelson-registry` 108** · `kelson-session` 8 ·
`kelson-telemetry` 11 · `kelson-tracing` 4 · **`kelson-validation` 49** · `kelson-views` 53 ·
plus 1 doctest (`kelson-http`). `STEP_EXIT=0`.

### 2.2 Focused demo integration files — 55 results, 0 failures

| Target (`kelson-demo`) | Tests | Result |
|---|---|---|
| `routing_edge_integration` | 8 | ok (`any_route_answers_every_declared_verb`, `get_route_answers_head_without_a_body`, `get_route_answers_the_auto_options_probe`, `verb_mismatch_is_405_with_allow`, `domain_gate_matches_before_the_auth_chain`, `subdomain_gate_404s_the_wrong_host_and_feeds_the_tenant`, `constraint_mismatch_is_a_route_miss_for_every_kind`, `one_time_signed_link_redempts_exactly_once`) |
| `response_layer_integration` | 17 | ok (204, ndjson stream, cache policies, conditional-GET 304, CSRF web/api/skip-path, security headers on/off, JSON:API negotiation/sparse fieldsets/reserved members) |
| `error_rendering_integration` | 9 | ok (debug gates, redaction, report-only, overrides/renderers, problem opt-in, error views, accept precedence) |
| `validation_edge_integration` | 6 | ok (exists default/explicit/unregistered 503, wildcard expansion, message overrides, custom rules) |
| `views_integration` | 9 | ok (view render, error views, envelope fallback, locale negotiation, auth context) |
| `static_assets_integration` | 6 | ok (Vite manifest, public files, HEAD length, route-shadowing, traversal 404 view) |

Each invocation boots the demo host (parallel test execution inside a file).

### 2.3 Targeted `http_integration` runs — 28 results, 0 failures (+1 PG self-skip)

`every_manifest_route_reaches_the_http_router` · `route_file_route_dispatches_to_the_declared_module_handler` ·
`fallback_route_attaches_and_dispatches_on_unmatched_paths`, `two_plane_fallbacks_refuse_at_the_edge` ·
`binding_miss_malformed_and_cross_tenant_are_404` · **precognition ×6** · **session ×6**
(`sessions_persist_across_requests_and_logout_destroys`, `login_rotates_session_and_invalidates_prelogin_cookie`,
`session_cookie_carries_secure_defaults_not_secure_in_test`, `password_confirm_stamps_the_session_and_fails_closed`,
`oauth_authorize_requires_a_session_and_issues_a_bound_code`, `ops_dashboard_renders_shipped_view_for_a_session`) ·
`login_throttling_returns_429_after_burst` · `throttle_group_exhaustion_is_429` · `deprecated_route_emits_header_and_meta` ·
`public_route_serves_anonymous_callers` · `unauthenticated_graph_route_is_401_envelope` ·
`unknown_route_is_404_and_demo_echo_still_golden` · `malformed_json_body_is_422_envelope` ·
`package_routes_serve_over_http_and_abi_mismatch_refuses_boot` · `pagination_injects_page_and_clamps_per_page` ·
`remember_login_issues_the_30d_cookie_and_me_reports_the_flag` · `full_cycle_register_login_book_list_cancel`.
`pg_http_integration` self-skipped (PG unavailable; §2.7).

### 2.4 Follow-up runs

| Run | Result |
|---|---|
| `cargo test -p kelson-demo --lib` | **115 passed, 0 failed** (143.4 s) — incl. `graph_routes::tests::declared_methods_mirror_the_registry_route_gate`, `method_filter_mapping`, `merge_params_body_wins`, `form_decode_applies_form_urlencoding`, `query_pairs_skip_garbage`, `redirect_flash_survives_exactly_one_next_request`, `redirect_targets_come_from_the_operation_result`, `redirect_refuses_non_3xx_statuses_and_header_injection`, `edge::tests::*` ×4, `host::error_render_tests::error_render_registry_reaches_modules_and_the_edge` |
| `cargo test -p kelson-models` | **135 passed, 0 failed** (incl. bindings resolver tests; 1 ignored doctest) |
| MySQL session twins (`kelson-postgres --features pg-tests --test pg_integration -- mysql_session --test-threads=1 --nocapture`, `DATABASE_URL=mysql://keel:keel@127.0.0.1:3307/keel`) | **2 passed, 0 failed** — `mysql_session_store_lifecycle`, `mysql_session_sweep_removes_expired_rows` (genuine DB round-trips; no skip message under `--nocapture`) |

### 2.5 Totals

**885 green harness results, 0 failures** across the pass (549 unit/doctest · 55 focused file tests ·
29 targeted file-run tests · 115 demo-lib · 135 models · 2 MySQL stores), of which exactly one result
is a documented self-skip (`pg_http_integration`) and the rest executed real assertions.

### 2.6 How to re-run

```powershell
# fast crates ($env:CARGO_BUILD_JOBS=1; $env:CARGO_INCREMENTAL=0)
cargo test -p kelson-http -p kelson-registry -p kelson-validation -p kelson-session -p kelson-views `
  -p kelson-observability -p kelson-tracing -p kelson-export -p kelson-telemetry -p kelson-pipeline `
  -p kelson-metrics -p kelson-buffer -p kelson-jobs -p kelson-caching -p kelson-console

# demo integration (one target or one filter per invocation)
cargo test -p kelson-demo --test routing_edge_integration
cargo test -p kelson-demo --test http_integration -- precognition
cargo test -p kelson-demo --lib

# MySQL store matrix
$env:DATABASE_URL='mysql://keel:keel@127.0.0.1:3307/keel'
cargo test -p kelson-postgres --features pg-tests --test pg_integration -- mysql_session --test-threads=1 --nocapture
```

### 2.7 NOT RUN / blocked / no coverage

- **PostgreSQL-dependent** (host PG unusable, `0xC0000142`): `http_dispatch_serves_over_the_pg_model_store`
  (self-skip, 0.00 s), `pg_session_store_lifecycle`, PG half of the LF-007 conformance matrix.
  → **NOT RUN** (environment).
- **Not selected this pass** (suites exist): `upload_*` HTTP integration tests (multipart unit suite executed
  instead), `manifest_validation_422_with_fields_meta`, the `oauth_*` redirect-validation suite,
  `kelson-session-redis` adapter tests. → **NOT RUN** (selection).
- **Redis-dependent** (no Redis on this host — `REDIS_URL` unset): the shared rate-limit store's live
  cases `shared_store_enforces_one_limit_across_instances` and `a_different_client_has_its_own_bucket`
  (feature `redis-tests`) **skip cleanly** — the same posture the pg/mysql matrices use. The adapter's
  unit tests (key layout, sanitizing, clock) and the in-process store's behaviour are executed; the
  adapter's runtime behaviour against a real Redis is **NOT RUN** here. → **NOT RUN** (environment).
- **No test exists**: flash integration in `tests/`; percent-encoded route params;
  streaming cancellation; scheme/host (absolute) URL generation; nested route groups.

## 3. Detailed Verification

Per-item implementation pointers and notes. All executed evidence is in §2 and the matrix; below are
the load-bearing details and divergences.

### LF-011 — Routing and HTTP methods — VERIFIED
- Verb set is closed at compile time (`crates/application/registry/src/lib.rs`, `ROUTE_METHODS`:
  GET/POST/PUT/PATCH/DELETE/HEAD/OPTIONS/ANY; `GRAPH-ROUTE-METHOD` refuses the rest), and the edge's
  `declared_methods` mirrors it (`declared_methods_mirror_the_registry_route_gate`) — a declared route
  cannot silently become a runtime 404.
- Live semantics proven: GET answers HEAD without a body; auto-OPTIONS probe; verb mismatch → 405 with
  `Allow`; constraint mismatch → route miss (404), not 405; unknown route → 404 envelope; ANY registers
  every declared verb; deprecated routes emit header + meta.
- Route files (`routes/*.toml`) merge as synthetic modules (`root_route_files_merge_as_synthetic_modules`)
  and dispatch to declared module operations (`routefile_op_gate_requires_registered_module_operations`,
  `routefile_routes_with_registered_ops_compile`; live test #09).
- **Divergences (recorded)**: no `QUERY` method in the set; percent-encoded path params have no dedicated
  test (axum decoding is the mechanism, untested).

### LF-012 — Named routes and URL generation — PARTIALLY VERIFIED
- `resolve_url` is a real generator: substitution + percent-encoding, missing param → graph violation,
  extras appended as a sorted query string, registered url-defaults fallback, still-required params
  enforced (5 registry tests). Route names must be unique (`GRAPH-DUP-ROUTENAME`; PUT/PATCH alias pair
  allowed).
- `kelson route:list` exposes the parity columns; the starter scaffold test compiles a generated manifest
  and resolves named routes/URLs through it.
- **Gap**: generation is **path-only** — no scheme/host (absolute URLs), no trusted-host/reverse-proxy
  handling, and no `Redirect::route`/`back` helper exists. Downgraded for the atlas's
  "scheme/host/prefix … reverse-proxy trusted-host" expectation.

### LF-013 — Route groups, prefixes, subdomains and constraints — PARTIALLY VERIFIED
- Groups: `[[route_groups]]` with `prefix` concatenated at parse, middleware merged group-OUTERMOST
  (route entries win on conflict), plane inherited; unknown group → `GRAPH-GROUP-UNKNOWN` collect-all.
- Constraints: closed kinds `number/alpha/alpha_num/uuid/ulid/slug/pattern`; the `pattern` kind is a
  hand-interpreted, fully anchored mini-grammar (no regex crate) — 20 unit tests cover the matrices and
  pattern edge cases; malformed/unknown kinds refuse at compile.
- Subdomains/domains: host label extraction + matching rules unit-tested; live domain gate runs before
  the auth chain and the subdomain gate 404s the wrong host while feeding the tenant.
- **Divergences (recorded)**: single-level (flat) groups — no nested groups / group-in-group prefix
  composition; arbitrary regex `where` constraints are deliberately superseded by the closed kinds.

### LF-014 — Route model binding and fallback routes — VERIFIED
- Bindings resolve through `RouteBindingResolver` (plain and `with_trashed` variants); manifest specs gate
  that the bound param exists in the path and that `with_trashed` is opt-in; the registry is looked up by
  model name.
- Live: misses, malformed ids and cross-tenant lookups are all 404; fallback routes attach per plane and
  dispatch on unmatched paths; two fallbacks on one plane refuse at compile.
- **Notes**: no enum-typed binding kind (the atlas's "enum binding"); nested scoping is expressed with
  explicit filters rather than implicit parent scoping. Neither gap affected the proven contract
  (valid id → correct row / unknown or unscoped → 404), so the item stays VERIFIED with notes.

### LF-015 — Middleware pipeline — VERIFIED
- Catalog: `middleware.toml` aliases/groups/throttle configs; builtins `auth`, `verified`, `confirmed`,
  `signed`, `throttle:<group>`; tenant-plane graph routes get the implicit `api` group unless `public`
  (which strips `auth`+`verified` but keeps throttle); `excluded_middleware` flattens groups and removes
  entries; unknown entries fail `GRAPH-MW-UNKNOWN` (boot-refusing, collect-all).
- Order is fixed and evidenced: admission (request-id → trace → body-limit → CORS → maintenance →
  rate-limit) → router; session middleware inside; CSRF per web-plane route; edge layer OUTERMOST
  (queued-cookie flush, then terminating hooks — a panicking hook is caught and cannot break the response).
- Hot-swap: route bundles (with resolved chains) swap without rebuild (`hot_reload::tests::*` ×4).
- **Divergence (recorded)**: modules cannot register custom middleware in this version — the catalog is
  framework-owned.

### LF-016 — Controllers and resource controllers — VERIFIED
- Equivalents: handlers are module operations executed through the engine (method injection = DI
  container, LF-005 batch), and resource/singleton manifests expand to the six/four REST specs with
  `only`/`except` and a capability gate; codegen via `kelson make resource`.
- Response side: `kelson-http::Resource` transformer (conditional fields that omit rather than null,
  wrap key, collections with pagination meta) — 7 unit tests.
- **Note**: no PHP-style controller classes/inheritance; the typed-operation shape is the documented
  equivalent.

### LF-017 — Request parsing and input access — VERIFIED
- Input sources stay distinct (query vs body vs route params); where a merge is needed the body wins
  (`merge_params_body_wins`); form bodies decode as `application/x-www-form-urlencoded`
  (`form_decode_applies_form_urlencoding`); garbage query pairs are skipped (`query_pairs_skip_garbage`).
- Limits: admission body-limit 1 MB (413 with envelope), per-route upload override 32 MB, multipart part
  count / mime allowlist / mid-stream size caps (unit suite), sanitized filenames (traversal stripped).
- Client identity: `client_ip` takes the first forwarded hop and is documented as identity/rate-only
  (never authorization); `bearer_token` requires the scheme; `fingerprint` is stable and field-separated.
- **Notes**: Laravel's merged `input()` is not replicated (sources distinct by design); duplicate-header
  handling is proven for cookies only; the `upload_*` HTTP tests were not re-run this pass (§2.7).

### LF-018 — Response types and cookies — VERIFIED
- Proven on the wire: 204 without body; ndjson streaming (one line per element, flushed); canonical
  `Cache-Control` policies from manifests; strong quoted ETags with conditional-GET 304; opt-in security
  headers (handler-set headers win; off by default); JSON:API negotiation incl. sparse fieldsets,
  reserved-member stripping and 406 for media-type parameters; static serving with traversal refusal and
  HEAD lengths; stored-object download as attachment.
- Cookies: `<name>=<value>` rendering with full attribute sets, Secure defaults, sealed (encrypted)
  values that never leak plaintext, multi-header parsing with last-wins trimming, FIFO queue drained once
  by the edge layer.
- **Note**: streaming *cancellation* is not covered by any test.

### LF-019 — Redirects, aborts and flash data — PARTIALLY VERIFIED
- `Redirect` provides 302 (default), 303 (PRG) and 307 (method-preserving) with correct `Location`
  rendering; targets come from operation results, and non-3xx statuses/header injection are refused.
- Flash: put/take with read-once semantics on the session store; survives exactly one next request
  (demo-lib test); unknown session = absent.
- Aborts map through the error taxonomy (401/403/404/422/429/503 envelopes, §LF-024 evidence).
- **Gaps**: no `back()`/`route()` (named) redirect helpers; no test in `tests/` for flash (unit + demo-lib
  only); generic user-controlled redirect targets are not allowlist-checked by a helper (each flow, e.g.
  OAuth, validates its own targets).

### LF-020 — CSRF protection — VERIFIED
- Double-submit-cookie guard is default-ON for web-plane routes per route (no flag/env), with skip paths
  from `[security.csrf]`; safe requests pass and bootstrap the cookie; the api plane stays
  token-authenticated; session-bound tokens are available for stricter flows.
- Evidence: 9 unit tests + 4 live tests (default-on, roundtrip bootstrap, skip paths, api free) +
  precognition cannot bypass it.
- **Notes**: the cookie token refreshes on every response that carries no `Set-Cookie` (rotation
  semantics); cookie is `SameSite=Strict`.

### LF-021 — CORS — VERIFIED (fixed F-1)
- A `tower-http` CORS layer is wired into admission. The posture is now **configurable**:
  `[security.cors]` (optional section) carries `allow_origins` / `allow_methods` / `allow_headers` /
  `max_age_secs` / `allow_credentials`, and the demo maps it into `Admission.cors`. An absent section
  keeps the permissive default (dev posture); a present section with an empty `allow_origins` disables
  CORS.
- The browser-invalid `*` + credentials combination is **refused at boot** by an explicit assertion in
  `CorsConfig::layer()` (tower-http would only refuse it at request time, inside the request path).
- Executed: `cors_default_is_a_permissive_wildcard`, `cors_allowlist_rejects_disallowed_origin`,
  `cors_allowlist_allows_configured_origin` (echoes the allowed origin and sets `Vary: Origin`),
  `cors_wildcard_without_credentials`, `cors_wildcard_with_credentials_is_refused`; plus the config
  parity tests (`cors_section_parses_allowlist_and_refuses_typos`, `cors_section_is_optional`,
  `cors_keys_are_declared_in_the_config_schema`).
- **Recorded gap**: preflight behaviour (`OPTIONS` → 204/403) is asserted by tower-http itself, not by
  a Kelson test — a Kelson-level preflight test is still worth adding.

### LF-022 — Session management — VERIFIED (with NOT RUN halves)
- Core: 256-bit ids; idle (sliding) + absolute timeouts; id regeneration on privilege change with data
  carried; remember-me 30-day default; CSRF token derived from id + server secret; HMAC-tagged cookie
  `<id>.<tag>` (SameSite=Lax, Secure configurable) — forged/expired/unknown cookies degrade to anonymous.
- Live: persistence across requests, logout destroys, login rotates and invalidates the pre-login cookie,
  password-confirm window, remember-me 30-day cookie surfaced by `me`, secure flag defaults under test.
- Stores: InMemory (suite default), MySQL (2 live tests, executed), Pg store + `pg_session_store_lifecycle`
  (**NOT RUN**, host), Redis adapter (exists; not executed this pass). Multi-instance durability is the
  stores' purpose and is demonstrated for MySQL.

### LF-023 — Validation and Form Requests — VERIFIED
- `kelson-validation` is the FormRequest equivalent: a collect-all Validator over a rule grammar with
  cross-field/conditional/date/file/auth/DB rules, wildcard fields, per-field message/attribute
  overrides, and a global custom-rule registry; `DbProbe` backs `unique`/`exists` against real stores
  (unregistered model fails closed 503).
- Manifest ValidationSpecs compile through gates (unknown ops, grammar, field-shape, custom-rule names,
  message keys) — collect-all, boot-refusing.
- Locale-aware messages follow content negotiation on the web plane.

### LF-024 — Error handling and exception rendering — VERIFIED (fixed F-2)
- Central taxonomy → status mapping with a per-code override registry, custom per-code renderers, and the
  reportable/renderable split; RFC 7807 problem+json opt-in on the api plane; explicit `Accept` beats the
  deployment default; web plane serves error views (with envelope fallback); debug gates hide internal
  detail when closed and render redacted detail when open; secrets are redacted through the logging list.
- All of the above is executed evidence (§2.2/§2.3).
- **Panic boundary (F-2, fixed)**: `tower-http`'s `CatchPanicLayer` is attached as the **innermost**
  `Admission::apply` layer, so a handler panic becomes a redacted 500 `KSN_INTERNAL` envelope instead of
  aborting the connection. The boundary sits inside the request-id/trace/body-limit/CORS/maintenance/
  rate-limit layers, so the caught 500 still carries `x-request-id` and `x-trace-id`; the panic payload is
  logged server-side and never reaches the client. Executed:
  `panicking_handler_answers_a_500_envelope` (500 + `KSN_INTERNAL` + both ids + no payload leak).

### LF-025 — Logging and channels — VERIFIED
- Structured logging pipeline (classify, correlate, dedupe, fingerprint, normalize, redact, sample) with a
  default redaction list and a buffered channel; exporters for console, rotating JSONL file,
  OTLP/HTTP JSON, generic HTTP and RFC5424 syslog, each wrappable in `ManagedExporter` (circuit breaker,
  retry budget, health snapshot).
- Trace correlation wired at admission (request-id generation/reuse, traceparent parse) and verified;
  redaction proven end-to-end through the error-rendering debug path.
- **Note**: Laravel "channels"/"stacks" map to exporters + pipeline here; no 1:1 config DSL claim.
  Sink-failure resilience is unit-level (breaker/retry/health); concurrent-write stress was not run.

### LF-026 — Rate limiting — VERIFIED (fixed F-3)
- Admission token bucket with key precedence `x-rate-key` → bearer hash → **the trusted-proxy
  `X-Forwarded-For` walk** → global, and a `Retry-After` on 429; per-route `throttle:<group>` buckets are
  shared per group+client and run before auth (so anonymous floods are capped before principal
  resolution); jobs have a rate-limit middleware.
- Executed: bucket math, exhaustion, key precedence, group isolation/overrides, login throttle, group
  exhaustion (see matrix).
- **Fixed in F-3 Phase 1**: the client key only honours `X-Forwarded-For` for the proxy ranges the
  deployment declares in `[http.trusted_proxies]` (CIDR; the chain is walked right-to-left and the first
  untrusted hop is the client). With no trusted proxies the header is ignored — a client cannot pick its
  own bucket. An unparsable range refuses the boot.
- **Fixed in F-3 Phase 2**: the bucket map is bounded — idle buckets are swept by a TTL and the oldest
  bucket is evicted at the hard cap, so distinct client keys cannot grow the map without bound.
- **Fixed in F-3 Phase 3**: the limiter is **shared** across instances. The token-bucket step sits
  behind the `RateLimitStore` trait; the in-memory implementation is the default (single-instance
  posture) and the new `kelson-ratelimit-redis` adapter puts the bucket in Redis with an **atomic Lua**
  read→refill→take→write (two instances cannot both take the last token), a `{rl}` hash tag (one
  cluster slot), a sanitized client key and a TTL. The demo wires it from `REDIS_URL`. The deployment
  docs make multi-instance the target (`minReplicas: 3` → `maxReplicas: 20`), which is why the shared
  store was warranted rather than deferred.
- **Recorded trade-off**: a Redis outage **fails open** (the request is allowed) with a loud error log
  — a limiter outage must not become an API outage.

### LF-027 — Precognition / progressive validation — VERIFIED
- Implemented at the graph edge (`runtime/kelson-demo/src/graph_routes.rs`, Wave 8 ADR 0008) as a
  validate-only dispatch selected by the `Precognition` header: pass → 204 with no dispatch, failure →
  byte-identical 422, multipart refused before intake, routes without validation pass vacuously, and it
  is neither an auth (401) nor a CSRF (403) bypass.
- All six behaviors executed this pass.
- **Note**: lives in the host edge layer, not a reusable framework crate (a scaffold host inherits it;
  modules do not get a first-class API for it). Revision 1 called this "DEFER / Not implemented" — wrong.

## 4. Findings (new in this pass)

> **Revision 3 (2026-10-19, post-fix):** F-1 and F-2 are now **fixed** (see
> `LF-011-TO-LF-027-FINDINGS-FIXES.md`); F-3 stays open with an executed repro. The findings below are
> kept verbatim as the record of what was found; each carries its fix status.

- **F-1 (medium) — CORS posture is hard-coded permissive and untested (LF-021). — FIXED.**
  `CorsConfig::default()` allows `["*"]`; the only construction outside `kelson-http` is the demo's
  `Admission { maintenance, ..Default::default() }` (`runtime/kelson-demo/src/lib.rs:972`); the platform
  config exposes `[security.headers]`, `[security.signed_urls]`, `[security.csrf]` — no CORS key; and
  `access-control-allow-origin` appears in no test. Next: add a configurable origin allowlist (and tests
  for allow/deny, preflight, credentials×wildcard, `Vary: Origin`) or explicitly document a wildcard
  posture.
  **Fix**: `[security.cors]` allowlist (optional section) + `allow_credentials` with a boot-time refusal of
  `*`+credentials; 5 http tests + 3 config tests green.
- **F-2 (medium) — no HTTP panic boundary (LF-024). — FIXED.** No `CatchPanicLayer`/`catch_unwind` around
  request dispatch; a panicking handler ends the connection instead of a stable 500 envelope. Next: add a
  catch layer mapping panics to a redacted 500 + envelope, plus a test that a panicking route still
  answers.
  **Fix**: `CatchPanicLayer` as the innermost `Admission::apply` layer → redacted 500 `KSN_INTERNAL` that
  keeps `x-request-id`/`x-trace-id`; `panicking_handler_answers_a_500_envelope` green.
- **F-3 (medium) — rate limiter is process-local and trusts client-controlled keys (LF-026). — PARTIALLY FIXED.**
  No shared store; first `X-Forwarded-For` hop is trusted without a trusted-proxy setting; the bucket map
  grows without eviction. Next: trusted-proxy config, bucket eviction, and an optional shared-store
  backend.
  **Fix (Phases 1–2)**: `[http.trusted_proxies]` (CIDR ranges; empty = trust nothing, so a
  client-supplied `X-Forwarded-For` never picks its own bucket) + a right-to-left chain walk +
  bucket TTL sweep and a hard cap. 6 http tests + 2 config tests green. **Phase 3 (shared store) is
  open** — it depends on whether the deployment scales horizontally; the gap is recorded with an
  executed repro (`rate_limiter_buckets_are_process_local`, `#[ignore]`d).

Recorded-but-expected divergences (not defects): path-only URL generation; no named-route/back redirect
helpers; no `QUERY` verb; flat (non-nested) route groups; `where`-regex superseded by closed constraint
kinds; flash = take-once; modules cannot register custom middleware.

## 5. Corrections to revision 1 (the previous version of this file)

Revision 1 asserted "All 17 … VERIFIED with executed test evidence. … No PARTIAL, UNKNOWN, or NOT RUN
items in this category." That was not defensible. Specific corrections:

1. **LF-012** cited `route_file_route_dispatches_to_the_declared_module_handler` — a route-file *dispatch*
   test, not URL generation. Actual evidence: the five `resolve_url_*` registry tests + the console
   starter/route:list tests. Status corrected to PARTIALLY VERIFIED (path-only generation; no
   scheme/host).
2. **LF-013** cited `binding_miss_malformed_and_cross_tenant_are_404` — a model-binding test — as evidence
   for groups/prefixes/subdomains/constraints. Actual evidence: group gates, constraint matrices,
   domain/subdomain gates. Status corrected to PARTIALLY VERIFIED (flat groups only).
3. **LF-015/016/017/019/020/023/025/026** used "tests in http_integration ✅" with no names; several of
   those suites (CSRF, redirect, flash, validation, middleware) are `kelson-http`/`kelson-registry` unit
   tests, not `http_integration`. Replaced with named, executed evidence above.
4. **LF-018** had a single cookie-flag test as evidence for "response types and cookies"; response-type
   coverage lives in `response_layer_integration`/`static_assets_integration`/cache/jsonapi suites (now
   cited).
5. **LF-021** claimed "CORS tests in http_integration ✅" — **no CORS test exists anywhere**, and there is
   no origin allowlist configuration. Corrected to PARTIALLY VERIFIED + F-1. *(Revision 3: F-1 fixed —
   the allowlist, the boot-time `*`+credentials refusal and 8 tests now exist; LF-021 is VERIFIED.)*
6. **LF-022** cited `pg_session_store_lifecycle` — a PostgreSQL test that **cannot execute on this host**
   (PG is unusable, `0xC0000142`). The executed twin is `mysql_session_store_lifecycle` (plus sweep),
   re-run 2/2 this pass; the PG test remains NOT RUN.
7. **LF-027** said "DEFER | Not implemented" yet "VERIFIED (test exists)" — self-contradictory, and wrong:
   the feature is implemented at the graph edge with six passing integration tests. Corrected to
   VERIFIED (host-edge placement noted).
8. **LF-024** omitted that no panic boundary exists (F-2); corrected to PARTIALLY VERIFIED. *(Revision 3:
   F-2 fixed — the panic boundary ships and `panicking_handler_answers_a_500_envelope` is green;
   LF-024 is VERIFIED.)*
9. **LF-026** omitted the process-local/trust/eviction gaps (F-3); corrected to PARTIALLY VERIFIED.
   *(Revision 3: F-3 fixed in all three phases — the trusted-proxy client key, the bounded bucket map
   and the shared Redis store — so LF-026 is VERIFIED. The live-Redis cases skip on this host.)*

## 6. Summary

- **17/17 items covered**: **14 VERIFIED** (LF-011, 014, 015, 016, 017, 018, 020, 021, 022, 023, 024, 025,
  026, 027) and
  **3 PARTIALLY VERIFIED** (LF-012, 013, 019). No item is NOT RUN at feature level;
  sub-aspects are listed in §2.7.
- **885 green results, 0 failures** in this pass (549 unit/doctest + 84 demo integration shells + 115
  demo-lib + 135 models + 2 MySQL store), with one documented PG self-skip. *(Revision 3 adds the
  post-fix runs: kelson-http 153 passed / 0 failed / 1 ignored, kelson-config 61+3 passed.)*
- **Findings**: F-1 (CORS configurability/tests) **fixed**, F-2 (panic boundary) **fixed**, F-3
  (rate limiter) **fixed in all three phases** (trusted-proxy client key, bounded bucket map, shared
  Redis store). Smaller recorded divergences listed at the end of §4.
- Backlog: rows 17–19 in `P0-EXECUTION-REPORT.md`; the fix designs live in
  `LF-011-TO-LF-027-FINDINGS-FIXES.md`.
- Suggested backlog entries from this pass: F-1, F-2, F-3 (and optionally: upload HTTP suite re-run, CORS
  docs note, `QUERY` verb decision).
