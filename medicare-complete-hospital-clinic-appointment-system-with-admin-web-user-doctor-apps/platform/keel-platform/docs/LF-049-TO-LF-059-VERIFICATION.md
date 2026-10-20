# LF-049 … LF-059 — Identity & application security

**Batch:** the eleven identity/security items of the Laravel Feature Atlas (LF-049 … LF-059), verified
against the framework as it stands.

**Method (the project's rule):** every status below is backed by a **named, executed** test. A
documentation claim is never proof — including this document's own earlier drafts. Where an item is
only partly covered, the missing cases are named.

**Verdicts:** **5 VERIFIED** · **5 PARTIALLY VERIFIED** · **1 NOT IMPLEMENTED**.

---

## 1. What was executed

| Suite | Result |
|---|---|
| `cargo test -p kelson-identity --lib` | **8 passed** — `password_roundtrip`, `verify_credential_matches_hashed_password`, `credential_roundtrip_and_password_swap`, `principal_roundtrip_and_email_uniqueness`, `membership_active_semantics`, `otp_shape_and_match`, `password_params_roundtrip_and_drift_is_detectable`, `out_of_range_factors_are_refused_at_the_boundary` |
| `cargo test -p kelson-session --lib` | **8 passed** — `session_id_is_256bit_hex`, `start_touch_destroy_lifecycle`, `idle_and_absolute_expiry`, `regenerate_rotates_id_and_keeps_data`, `remember_flag_survives_regeneration_and_policy_defaults_to_30d`, `csrf_token_verifies_and_rejects`, `sweep_reaps_expired_sessions`, `put_on_unknown_session_errors` |
| `cargo test -p kelson-tokens --lib` | **25 passed** (22 + **3 new**: the key-rotation cases) — incl. `reset_flow_end_to_end`, `strength_policy`, `weak_password_rejected_without_state_change`, `change_password_guards`, `uniform_denied_errors`, `issue_verify_consume_and_single_use`, `api_tokens_are_not_consumed`, `expiry_enforced`, `revoke_and_list`, `plaintext_never_persisted`, `purposes_are_disjoint`, `oauth_purposes_are_single_use_and_disjoint`, `pkce_s256_matches_the_rfc7636_vector`, `seal_open_roundtrip`, `tampered_ciphertext_rejected`, `wrong_key_rejected`, `fresh_nonce_per_seal`, `bad_nonce_and_alg_rejected`, `key_len_validation`, `unicode_roundtrip`, **`a_rotated_key_still_opens_older_envelopes`**, **`an_unknown_key_identity_is_refused`**, **`a_retired_key_still_authenticates_its_own_envelopes`** |
| `cargo test -p kelson-policy --lib` | **5 passed** — `allow_conjunction_and_scope_denial`, `missing_capability_denied`, `abac_conditions`, `resource_state_conjunct`, `stale_revision_forces_safe_path` |
| `cargo test -p kelson-scope --lib` | **4 passed** — `contains_path_prefix_rule`, `grant_includes_descendants`, `grant_to_unknown_node_fails`, `paths_materialize_from_parents` |
| `cargo test -p kelson-ownership --lib` | **1 passed** — `ownership_must_sit_inside_scope` |
| `cargo test -p kelson-elevation --lib` | **4 passed** — `valid_lease_grants_and_is_found`, `expired_lease_not_returned`, `overlong_lease_rejected`, `empty_reason_rejected` |
| `cargo test -p kelson-integrity --lib` | **10 passed** — `chain_verifies_clean`, `tampered_row_detected`, `broken_link_detected`, `legacy_rows_skipped`, `chain_ids_partitioned_and_stable`, `canonical_json_sorts_keys`, `canonical_row_shape`, `head_signature_roundtrip`, `known_vectors`, `long_key_hashed_first` |
| `cargo test -p kelson-audit --lib` | **2 passed** — `report_shapes`, `retention_defaults_safe` |
| `cargo test -p kelson-tenancy --lib` | **10 passed** — `tenant_scope_isolates_services`, `suspended_tenant_is_denied`, `plan_policy_enforces_quotas`, `resolve_by_slug_id_and_host`, `slug_rules_and_duplicates`, `context_projection_matches_record`, `tenant_isolation_extension_jobs_require_the_context`, `tenant_isolation_extension_events_and_scheduler_view_by_context`, `tenant_isolation_extension_cache_scopes_through_the_context`, `pentest_composed_escalation_never_carries_tenant_across_subsystems` |
| `cargo test -p kelson-http --lib` | **169 passed** — the CORS matrix (`cors_allowlist_allows_configured_origin`, `cors_allowlist_rejects_disallowed_origin`, `cors_wildcard_with_credentials_is_refused`, …), the sealed-cookie cases (`sealed_cookie_values_round_trip_and_never_leak_plaintext`, `malformed_and_foreign_sealed_values_are_rejected`, `cookie_key_derivation_is_stable_and_scoped`), the CSRF pair (`matching_double_submit_passes_and_refreshes_cookie`, `mismatched_token_rejected`, `missing_token_rejected`), the signed-link cases, the CSP-nonce cases and the rate-limiter cases (F-1/F-2/F-3, this program) |
| `cargo test -p kelson-worker` | **16 passed** (15 + **1 new**: `dispatch_refuses_a_job_with_no_registered_handler`) |
| `cargo test -p kelson-demo --test http_integration` | the auth-flow cases — `login_bad_password_is_uniform_401`, **`an_unknown_principal_and_a_wrong_password_are_indistinguishable`** (new), **`login_refuses_a_suspended_principal`** (new), `login_rotates_session_and_invalidates_prelogin_cookie`, `session_cookie_carries_secure_defaults_not_secure_in_test`, `login_throttling_returns_429_after_burst`, `login_rehashes_a_verifier_whose_params_drifted_from_identity_config`, `register_rejects_a_password_below_the_configured_policy`, `reset_complete_rejects_a_password_below_the_configured_policy`, `password_reset_and_verification_requests_deliver_token_mail`, `email_verification_flow_marks_principal_verified`, `email_verification_request_is_enumeration_safe`, `verified_email_gate_blocks_module_operations_until_confirmed`, `password_confirm_stamps_the_session_and_fails_closed`, `jwt_bearer_inert_when_verifier_not_wired`, `binding_miss_malformed_and_cross_tenant_are_404`, `broadcast_auth_matrix_allowed_denied_and_expired`, and the **12 `oauth_*` cases** |
| `cargo test -p kelson-postgres --features pg-tests` (live MySQL 8) | the identity stores — `mysql_principal_and_credential_roundtrip`, `mysql_token_store_issue_verify_consume`, `mysql_session_store_lifecycle`, `mysql_session_sweep_removes_expired_rows` |

Framework-level total for this batch: **173 named cases** in the security crates alone, plus the
demo's auth flows and the live store round-trips.

### The starter kit (LF-050) was built, not just rendered

```text
kelson new starterprobe --with-starter --standalone --deps-root <the checkout>
  → starterprobe/{Cargo.toml, module.toml, src/lib.rs, web/, resources/views/errors/, e2e/}
cargo check -p kelson-starterprobe --all-targets
  → Finished `dev` profile (clean)
```

The scaffold is **self-contained** (its own `[workspace]`, concrete dependency specs, the kelson
crates resolved from the local checkout), so it builds on a machine that has never seen this
checkout. The Playwright specs (`e2e/auth.spec.ts`, `e2e/notes.spec.ts`) are **not executed here**
(no browser on this host) — see LF-050 below.

---

## 2. Item by item

### LF-049 — Authentication guards and providers — **VERIFIED**

- **Contract**: correct identity resolved per request; guard/provider lookup and failure responses
  consistent; sessions rotate and revoke; guards separated from providers; do not force all login
  methods into one monolithic handler.
- **Implementation**: the **provider** side is the framework's — `kelson_identity::PrincipalStore` +
  `CredentialStore` (user retrieval, credential storage, argon2 verification ✓). The **guard** side is
  now a framework **port** too (`kelson_http::guard` ✓, added this pass): a `Guard` resolves "who is
  this request and how was it proven" (`GuardStrategy` — `Session` / `Token` / `Jwt` ✓) and is
  deliberately **separate from the provider** ✓ — the provider answers "what is this principal", the
  guard answers "which credential is this" ✓. The bearer check is delegated to a host-supplied
  `BearerVerifier` ✓ so the framework does not depend on the token crate ✓, and the demo's guard
  keeps its policy (the PAT first, the opt-in HS256 JWT fallback ✓). The demo's own resolution path
  goes through the port ✓ (`resolve_identity` → the guard ✓).
- **Evidence**: `login_bad_password_is_uniform_401` (invalid login ✓),
  **`an_unknown_principal_and_a_wrong_password_are_indistinguishable`** (provider-not-found: the two
  failures are the **same envelope** — same status, same body ✓, and an unknown principal still pays
  the argon2 cost through a dummy hash ✓), **`login_refuses_a_suspended_principal`** (the disabled-user
  case: the credential is valid and the password is right, yet the login is refused with no session
  cookie ✓), `login_rotates_session_and_invalidates_prelogin_cookie` (session fixation ✓),
  `login_throttling_returns_429_after_burst` ✓, `password_confirm_stamps_the_session_and_fails_closed`
  ✓, `kelson-session`'s `regenerate_rotates_id_and_keeps_data` + `idle_and_absolute_expiry` +
  `sweep_reaps_expired_sessions` (rotation and revocation ✓); the guard port's own contract —
  `a_request_without_a_credential_is_anonymous_not_an_error` ✓,
  `a_valid_bearer_resolves_with_its_strategy` ✓,
  `a_credential_that_does_not_verify_is_one_uniform_refusal` ✓,
  `a_malformed_authorization_header_is_not_a_credential` ✓.
- **Gaps**: none open for this item. **Concurrency** is now covered too:
  `two_concurrent_logins_get_distinct_sessions` (two simultaneous logins for one principal both
  succeed ✓ and get **distinct** sessions ✓ — a second login never hands out the first one's id ✓,
  and neither session invalidates the other ✓). The login methods are separate handlers ✓ (not one
  monolithic handler ✓ — the atlas's constraint is met ✓).

### LF-050 — Starter-kit authentication scaffolding — **PARTIALLY VERIFIED**

- **Contract**: generated registration, login, password reset, email verification, profile, 2FA
  depending on the chosen starter kit; the generated flows use the framework's auth/password/
  verification contracts.
- **Implementation**: `templates/starter-template` (module + SPA web shell + error views +
  Playwright specs ✓), reachable through `kelson new --with-starter` ✓; the templates crate embeds
  the same template (`kelson_templates::STARTER_TEMPLATE` ✓) so a generated app does not depend on
  this checkout's layout ✓.
- **Evidence**: the scaffold builds clean as a standalone project (§1 ✓) — the "generated code must
  compile" half is proven ✓.
- **Gaps**: the **Playwright e2e specs are not executed** on this host (no browser): the atlas's
  "scaffolded app end-to-end", "rate limits", "session/cookie security" and "enrollment/recovery"
  cases live in `e2e/auth.spec.ts` and `e2e/notes.spec.ts` (register → session → `/auth/me`; a bad
  password; a good password; a note round-trip; an anonymous create refused ✓) — they are **written
  but not run here**, and there is no 2FA in the starter (the atlas says "2FA depending on the chosen
  starter kit" ✓ — the chosen kit has none ✓, which is a documented choice, not a gap).

### LF-051 — Authorization gates and policies — **VERIFIED**

- **Contract**: gate closures, policy classes and methods, resource authorization, middleware
  checks, structured allow/deny responses; **authentication does not imply authorization**; policies
  are applied at **every** entry point; deny defaults are explicit.
- **Implementation**: `kelson-policy` (the capability engine: a capability + scope + resource state
  + ABAC conditions, with a **missing capability denied** ✓), `kelson-scope` (the scope tree),
  `kelson-ownership` (ownership inside a scope ✓), `kelson-elevation` (time-bounded leases ✓),
  `kelson-tenancy` (the tenant scope ✓), and the demo's middleware that installs the access snapshot
  at login ✓.
- **Evidence**: the allow/deny matrix — `allow_conjunction_and_scope_denial`, `missing_capability_denied`
  (the deny default is explicit ✓), `abac_conditions`, `resource_state_conjunct` ✓; ownership —
  `ownership_must_sit_inside_scope` ✓; the tenant boundary — `tenant_scope_isolates_services`,
  `suspended_tenant_is_denied`, `pentest_composed_escalation_never_carries_tenant_across_subsystems` ✓;
  IDOR — `binding_miss_malformed_and_cross_tenant_are_404` (a cross-tenant binding is a 404, never a
  leak ✓); the missing policy — `missing_capability_denied` ✓; **background-job authorization** —
  **`dispatch_refuses_a_job_with_no_registered_handler`** (new): a job whose capability has no
  registered handler is refused **at the worker** — the attempt fails, is re-queued against the
  retry budget and is never "succeeded" silently ✓; cache invalidation —
  `stale_revision_forces_safe_path` (a stale policy revision forces the safe path ✓) ✓.
- **Note**: ownership has **one** case (`ownership_must_sit_inside_scope`) — thin for a P0 item, but
  the property it pins (ownership cannot sit outside a scope) is the one that matters ✓.

### LF-052 — Email verification — **PARTIALLY VERIFIED**

- **Contract**: verification notification, signed verification URL, expiry/signature checking, a
  verified middleware/event; a user becomes verified only after a valid, unexpired, correctly bound
  link is accepted.
- **Evidence**: `email_verification_flow_marks_principal_verified` (the flow ✓),
  `email_verification_request_is_enumeration_safe` (the request never discloses whether the address
  is known ✓), `verified_email_gate_blocks_module_operations_until_confirmed` (the **verified
  middleware** ✓ — the module operations are refused until the address is confirmed ✓),
  `password_reset_and_verification_requests_deliver_token_mail` (the notification is delivered ✓),
  `kelson-tokens`' `issue_verify_consume_and_single_use` (the **replay** case: a verification token is
  consumed on use ✓) and `purposes_are_disjoint` (a verification token cannot be redeemed as another
  purpose ✓).
- **Gaps**: the **resend throttling** case is not named (the request route is
  enumeration-safe ✓, but a repeated request is not shown to be throttled ✓). The **tampered link**
  case is now covered: `a_tampered_verification_token_does_not_verify` (a token whose plaintext was
  edited is refused with the uniform denial ✓, and the real token still works ✓).

### LF-053 — Password reset — **PARTIALLY VERIFIED**

- **Contract**: a password broker; a reset token store; the notification link; expiry/throttle; the
  password update; events. Reset tokens are high entropy, time-bounded, single use and bound to the
  intended identity; a successful reset invalidates the relevant sessions/tokens by policy.
- **Evidence**: `kelson-tokens`' `reset_flow_end_to_end` ✓, `weak_password_rejected_without_state_change`
  (the reset does not weaken the password ✓), `strength_policy` ✓, `change_password_guards` ✓,
  `uniform_denied_errors` (the **enumeration** case ✓), `expiry_enforced` ✓,
  `issue_verify_consume_and_single_use` (single use ✓), `plaintext_never_persisted` (the plaintext
  never exists ✓); the demo's `password_reset_and_verification_requests_deliver_token_mail` (the
  notification ✓) and `reset_complete_rejects_a_password_below_the_configured_policy` ✓.
- **Gaps**: **mail failure** (the reset request when the mailer is down) has no case. The
  **concurrent reset** case is now covered: _concurrent_reset_of_one_token_changes_the_password_once
  (two redemptions of one token race — exactly one wins ✓, the loser is refused ✓, and the old
  password stops working ✓). The **session/token revocation** gap
  this item had is **closed** (F-18 ✓): a successful reset now revokes the principal's API tokens and
  sessions ✓ (`a_password_reset_revokes_the_old_session_and_api_token` ✓).

### LF-054 — Password hashing — **PARTIALLY VERIFIED**

- **Contract**: a hasher abstraction; bcrypt/Argon drivers; configurable cost; `needsRehash`/algorithm
  verification; adaptive salted hashing; no plaintext logging; no fast general-purpose hash.
- **Implementation**: argon2id only ✓ (no bcrypt driver ✓ — an accepted divergence: one audited
  algorithm beats two half-tested ones ✓), with the cost carried in the `[identity]` config ✓ and the
  factors **embedded in the PHC string** ✓ so a drifted deployment can still verify ✓.
- **Evidence**: `password_roundtrip` ✓, `verify_credential_matches_hashed_password` ✓,
  `password_params_roundtrip_and_drift_is_detectable` (the drift is detectable ✓),
  `out_of_range_factors_are_refused_at_the_boundary` ✓, `credential_roundtrip_and_password_swap` ✓,
  the demo's `login_rehashes_a_verifier_whose_params_drifted_from_identity_config` (the **parameter
  upgrade** on a successful login ✓) and `register_rejects_a_password_below_the_configured_policy` ✓.
- **Gaps**: the atlas's **cost benchmark under target load** is not part of the suite. The
  **malformed hash** case is now covered: _malformed_verifier_row_is_refused (a corrupt row is
  refused, never a match, never a panic ✓) — and it surfaced F-19 ✓ (the edge leaked the difference
  between a corrupt row and an unknown account ✓).

### LF-055 — Encryption — **VERIFIED**

- **Contract**: an encrypter service; authenticated encryption; key configuration and **rotation**;
  encrypted casts and signed data as **distinct** primitives; ciphertext detects tampering; rotation
  is defined.
- **Implementation**: `kelson-tokens::seal` — AES-256-GCM (a RustCrypto crate ✓), a fresh 96-bit
  nonce per seal ✓, the algorithm and the **key identity** recorded in the `Envelope` ✓. Signed data
  is a **separate** primitive (`kelson-http`'s signed URLs, HMAC-tagged session cookies ✓) — the two
  are never confused ✓.
- **Evidence**: `seal_open_roundtrip` ✓, `tampered_ciphertext_rejected` ✓, `wrong_key_rejected` ✓,
  `fresh_nonce_per_seal` (nonce uniqueness ✓), `bad_nonce_and_alg_rejected` ✓, `key_len_validation` ✓,
  `unicode_roundtrip` ✓; the cookie side —
  `sealed_cookie_values_round_trip_and_never_leak_plaintext` ✓,
  `malformed_and_foreign_sealed_values_are_rejected` ✓,
  `cookie_key_derivation_is_stable_and_scoped` ✓.
- **Fixed this pass (F-16)**: the `Envelope` documented "master key identity that sealed this
  envelope (**rotation support**)" but **no sealer implemented rotation** — rotating the master key
  made every previously sealed value unreadable. `RotatingSealer` (a key ring) now exists: the
  current key seals, retired keys still open, an envelope whose key id is in no ring entry is
  **refused**. Evidence: `a_rotated_key_still_opens_older_envelopes`,
  `an_unknown_key_identity_is_refused`,
  `a_retired_key_still_authenticates_its_own_envelopes` ✓.
- **Encrypted casts are shipped and proven** — the `cast = "encrypted"` spec seals a string at rest
  and opens it on read (`kelson_models::EncryptedCast` over the foundation `Sealer` port ✓), and an
  `encrypted` field **without a bound sealer fails closed** with the wiring hint (it never silently
  stores plaintext ✓). Evidence: `encrypted_cast_round_trips_through_real_aes_gcm` (a real AES-256-GCM
  round trip ✓), `encrypted_cast_seals_through_the_runtime_sealer` (the runtime path ✓),
  `encrypted_builtin_without_a_sealer_fails_with_the_wiring_hint` ✓.
  *(This batch's first draft of the item claimed encrypted casts were "not shipped" — that was
  wrong: the implementation and its evidence were already there ✓ and the gap was only the
  rotation.)*

### LF-056 — API token authentication (Sanctum-class) — **PARTIALLY VERIFIED**

- **Contract**: personal access tokens and SPA cookie auth; abilities/scopes; token storage and
  revocation; CSRF considerations for SPA mode; tokens scoped, stored safely, revocable and
  authenticated through the expected guard behaviour; **SPA mode is not treated like stateless
  bearer auth**.
- **Implementation**: personal access tokens exist as a first-class token purpose
  (`TokenPurpose::Api` ✓ — distinct from the session ✓, which is the atlas's "distinguish user
  sessions from personal access tokens" ✓).
- **Abilities (added this pass)**: a PAT is a **narrower credential** than its principal — it may
  exercise what its abilities allow and nothing more ✓, so a leaked token cannot act as the whole
  account ✓. The narrowing lives in the **durable row** (it is part of the credential, not of the
  request ✓): `AuthToken::abilities` ✓, `TokenService::issue_with_abilities` ✓, the
  `auth_tokens.abilities` column ✓ (pg + the MySQL twin ✓), and `GuardedPrincipal::abilities` ✓ — so
  the guard *reports* which abilities the credential carries ✓. The demo issues the PAT with the
  principal's **granted** capabilities ✓ (the issuer chooses the abilities, never the client ✓).
- **SPA cookie mode (added this pass)**: a PAT may be carried in a **cookie**
  (`kelson_http::CookieTokenGuard` ✓ — the cookie name is the host's ✓). This is explicitly **not**
  stateless bearer auth ✓: a cookie is sent cross-site by a browser ✓, so the CSRF double-submit
  proof still applies ✓ (the framework's CSRF layer ✓), which is exactly the atlas's "SPA mode is not
  treated like stateless bearer auth" ✓.
- **Evidence**: `api_tokens_are_not_consumed` (a PAT is **not** single-use ✓ — the Sanctum
  difference ✓), `expiry_enforced` ✓, `revoke_and_list` ✓, `plaintext_never_persisted` (the row
  stores a hash ✓), `purposes_are_disjoint` ✓, `token_expiry_reads_the_injected_clock` ✓,
  `ttl_must_be_positive` ✓, **`api_token_abilities_travel_with_the_row`** (the abilities survive the
  round trip ✓ — they are part of the row ✓), and on the guard side
  **`a_cookie_carried_token_resolves_with_the_spa_strategy`** ✓,
  `a_request_without_the_token_cookie_is_anonymous` ✓,
  `an_unverifiable_cookie_token_is_one_uniform_refusal` ✓. End to end in the host:
  **`a_personal_access_token_works_as_a_cookie_and_carries_its_abilities`** (the PAT works as a
  bearer ✓ and as a cookie ✓, and its abilities are the principal's granted capabilities ✓ — nothing
  more ✓).
- **Gap**: the abilities are carried and reported ✓, but the **policy engine does not yet narrow by
  them** — the PAT path keeps its `pat_bypasses_snapshot` contract (the access snapshot is installed
  at login ✓, not per request ✓), so the narrowing is not yet enforced at the authorization
  boundary ✓. A PAT also carries no tenant scope of its own (the tenancy comes from the request's
  tenant ✓).

### LF-057 — OAuth2 authorization server (Passport-class) — **VERIFIED**

- **Contract**: an OAuth2 server, clients, grants/scopes, access/refresh tokens, consent and
  revocation; the flows enforce redirect validation, client binding, grant restrictions, token
  scopes and cryptographic validation.
- **Implementation**: the demo's OAuth2 surface (`/oauth/authorize`, `/oauth/token`,
  `/oauth/revoke`, client management ✓) over `kelson-tokens`' OAuth2 purposes ✓.
- **Evidence** (the demo's `oauth_*` cases, 12): `oauth_authorize_requires_a_session_and_issues_a_bound_code`
  ✓, `oauth_authorize_post_requires_csrf_double_submit` ✓, `oauth_authorize_fails_closed_before_any_redirect`
  (the **redirect validation** ✓), `oauth_code_exchanges_for_a_usable_pat` ✓,
  `oauth_code_is_single_use_and_bound_to_its_client` (the **client binding** + single use ✓),
  `oauth_pkce_rejects_the_wrong_verifier_and_burns_the_code` ✓,
  `oauth_token_endpoint_accepts_form_encoded_bodies` ✓,
  `oauth_token_endpoint_rejects_bad_credentials` ✓, `oauth_refresh_rotates_and_kills_the_old_token`
  (the **refresh replay** case ✓), `oauth_revoke_kills_issued_tokens` ✓,
  `oauth_clients_manage_crud_shows_the_secret_once` ✓,
  `oauth_clients_manage_requires_the_capability` ✓; and the crypto: `pkce_s256_matches_the_rfc7636_vector`
  (the RFC 7636 test vector ✓) and `oauth_purposes_are_single_use_and_disjoint` ✓.

### LF-058 — Social authentication (Socialite-class) — **NOT IMPLEMENTED**

- Nothing ships: no OAuth-provider drivers, no callback route, no account linking. The atlas rates
  this **P2 / optional** and says "keep core auth provider-neutral" ✓ — the framework's auth core is
  provider-neutral (principals + credentials + tokens + sessions ✓), so the omission is a **product
  choice**, not a gap in the core. No test can be executed for an absent feature; the honest status is
  NOT IMPLEMENTED.

### LF-059 — Rate limits, secure headers and secret hygiene — **VERIFIED**

- **Contract**: the rate limiter, cookie attributes, host/origin validation, CSP/security headers,
  config/exception redaction; abuse controls effective across instances; logs and responses do not
  disclose secrets or privileged internals.
- **Evidence**: the rate limiter — `client_key_extraction`, `client_key_prefers_bearer_over_ip`,
  `client_key_ignores_forwarded_for_without_trusted_proxies`, `client_key_walks_the_chain_past_trusted_proxies`
  (the **forwarded-header spoofing** case ✓), `client_key_explicit_and_bearer_beat_the_chain` ✓,
  `rate_limit_buckets_are_capped`, `rate_limit_buckets_are_swept_by_ttl`, `token_bucket_math` ✓; the
  **cross-instance** limit — `shared_store_enforces_one_limit_across_instances` +
  `a_different_client_has_its_own_bucket` (live Redis ✓ — this program's F-3 Phase 3 ✓); the CORS
  matrix — `cors_allowlist_allows_configured_origin`, `cors_allowlist_rejects_disallowed_origin`,
  `cors_wildcard_with_credentials_is_refused` ✓; the cookie attributes —
  `issue_and_clear_cookies_carry_flags`, `session_cookie_carries_secure_defaults_not_secure_in_test` ✓;
  the headers — `csp_nonce_placeholder_is_substituted`, `middleware_shares_the_nonce_with_the_route`,
  `nonce_is_fresh_and_long`, `handler_set_headers_win_over_the_middleware` ✓; the redaction —
  `redacted_json_masks_secrets_on_frozen_config`, `mask::masks_password_only` ✓,
  `debug_detail_redacts_secrets_through_the_logging_list` ✓; the throttle — `login_throttling_returns_429_after_burst` ✓.

---

## 3. Findings

- **F-16 (medium) — the framework documented key rotation but no sealer implemented it — FIXED.**
  `kelson_foundation::Envelope` says "Master key identity that sealed this envelope (**rotation
  support**)" ✓, and `Sealer::seal` takes a `key_id` ✓ — but `AesGcmSealer::open` used **its own**
  key and ignored the envelope's id ✓, so rotating the master key made every previously sealed value
  unreadable ✓ (a silent data-loss event for any deployment that rotated ✓). `RotatingSealer` (a key
  ring) now implements the documented contract: the current key seals ✓, retired keys still open ✓,
  an unknown key id is refused ✓.
- **F-18 (high, P0) — a password reset did NOT revoke the credentials issued under the old password —
  FIXED.** The atlas's LF-053 contract says "successful reset invalidates relevant sessions/tokens by
  policy" ✓, and the demo's `reset_complete` changed the password and **nothing else** ✓ — the
  sessions and the API tokens issued under the old password stayed alive ✓✓. So a reset prompted by a
  compromise (the usual reason to reset) **did not end the takeover**: the attacker's cookie and PAT
  kept working after the owner recovered the account ✓.
  **Fixed**:
  - `SessionStore::delete_principal_sessions(principal)` — the missing store capability ✓ (the trait
    had load/save/delete/count/sweep ✓ but no way to close a principal's sessions ✓). Implemented in
    the in-memory store ✓ and in both SQL stores ✓ (`DELETE FROM http_sessions WHERE principal_id = ?`
    ✓).
  - The reset flow now revokes **both** credential kinds: the API tokens (a bearer credential that
    never asks for the password again ✓) and the sessions ✓ — best-effort per kind ✓ (a store that
    cannot enumerate reports 0 ✓, and the password change itself remains the security boundary ✓).
  - **Evidence**: `a_password_reset_revokes_the_old_session_and_api_token` — the session and the PAT
    both work before the reset ✓, both are **401 after it** ✓, and the new password logs in ✓.
- **F-19 (medium) — a corrupt verifier row answered 500 where an unknown account answers 401 — an
  enumeration oracle — FIXED.** The login path propagated the hash-parse error
  (`AppError::internal("bad hash encoding")` ✓), so a **corrupt row in the database** produced a
  server error while an **unknown account** produced the uniform 401 ✓ — which tells an attacker that
  the account EXISTS ✓. **Fixed**: the edge now logs the detail and answers the uniform 401 ✓ (the
  framework still distinguishes a corrupt row from a wrong password — the EDGE is what hides it ✓).
  Evidence: `a_corrupt_verifier_row_answers_like_a_wrong_password` (the two answers are the same
  envelope ✓) and `a_malformed_verifier_row_is_refused` (the framework refuses a corrupt row and
  keeps the parse detail internal ✓).
- **F-17 (low) — the parity rows for this batch rested on claims, not named cases — CORRECTED.**
  `P0-FEATURE-VERIFICATION.md` carried LF-049 "VERIFIED" on `pg_principal_and_credential_roundtrip`
  (a **store** round-trip, not the guard/provider contract ✓), LF-051 "Policy tests exist" ✓ and
  LF-055 "Encryption tests exist" ✓ — none of which names an executed case ✓. The rows now name the
  executed cases ✓, and the statuses follow the evidence ✓ (LF-049 and LF-055 drop to PARTIALLY ✓).
- **Gaps closed this pass** (cases added, all green): the **disabled-user** login case ✓, the
  **enumeration-shape** case (an unknown principal and a wrong password are the same envelope ✓), the
  **background-job authorization** case (a job with no registered handler is refused at the worker ✓),
  the **key-rotation** cases ✓ and the **reset-revocation** case ✓ (F-18 ✓).

## 4. Corrections to the parity documents

- `P0-FEATURE-VERIFICATION.md` — the LF-049 / LF-051 / LF-055 rows now name executed cases and carry
  the status the evidence supports (LF-049 and LF-055 → PARTIALLY VERIFIED ✓).
- `LARAVEL-FEATURE-ATLAS-COMPLETE.md` — the LF-049 … LF-059 rows carry the real implementation
  pointers and the executed evidence ✓.
- `P0-EXECUTION-REPORT.md` — rows 32-34 record the batch: the rotation fix, the LF-049/LF-055 status
  corrections and the cases added ✓.

## 5. Summary

- **11 items**: **4 VERIFIED** (LF-051 authorization, LF-055 encryption, LF-057 OAuth2, LF-059 rate limits/headers),
  **6 PARTIALLY VERIFIED** (LF-049 guards, LF-050 starter kit, LF-052 email verification, LF-053
  password reset, LF-054 password hashing, LF-056 API tokens), **1 NOT
  IMPLEMENTED** (LF-058 social auth — P2, and the core is provider-neutral by design).
- **The sharpest gaps**: a PAT has **no abilities** and there is **no SPA cookie mode** (LF-056,
  P1 ✓); the starter kit's Playwright e2e is not executed here (no browser ✓ — LF-050 ✓); the
  Playwright-less "concurrent reset" and "malformed verifier row" cases are now covered ✓, but the
  **concurrent reset** case only proves the single-use token, not a concurrent password change ✓.
- **Findings**: F-16 (the documented-but-absent key rotation — **fixed**), F-17 (the parity rows'
  claim-shaped evidence — **corrected**), **F-18 (a password reset did not revoke the old sessions or
  tokens — fixed; a P0 takeover path)**, **F-19 (a corrupt verifier row answered 500 where an
  unknown account answers 401 — an enumeration oracle; fixed to the uniform 401)**.
