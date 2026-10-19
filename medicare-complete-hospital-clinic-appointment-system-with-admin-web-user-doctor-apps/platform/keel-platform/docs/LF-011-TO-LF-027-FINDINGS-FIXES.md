# LF-011→LF-027 Findings — Fixes (F-1, F-2, F-3)

> Evidence-backed from `LF-011-TO-LF-027-VERIFICATION.md` §4 and `P0-EXECUTION-REPORT.md` backlog rows 17–19.
>
> **Status (2026-10-19):** F-1 **implemented**, F-2 **implemented**, F-3 **partially implemented**
> (Phase 1 trusted-proxy walk + Phase 2 bucket eviction ship; Phase 3 — the shared store — is
> proposal-only because it depends on whether the deployment scales horizontally). F-1, F-2 and
> F-3 Phases 1–2 shipped with executed evidence — see each section below.

---

## F-1 — CORS posture is hard-coded permissive and untested (LF-021) — **IMPLEMENTED**

### Was
- `CorsConfig::default().allow_origins = ["*"]` (permissive wildcard).
- The **only** construction outside `kelson-http` was the demo:
  `Admission { maintenance, ..Default::default() }`.
- No `[security.cors]` key existed in the platform config schema.
- Zero tests asserted `Access-Control-Allow-Origin` behaviour.

### Implemented
1. **`[security.cors]` in `SecuritySection`** (`crates/application/configuration/src/lib.rs`):
   ```toml
   [security.cors]
   allow_origins = ["https://app.example.com", "https://admin.example.com"]
   allow_methods = ["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"]
   allow_headers = ["content-type", "authorization", "x-request-id"]
   max_age_secs = 3600
   allow_credentials = false
   ```
   - The section is **optional** (`Option<CorsSection>`): absent → the framework default
     (permissive wildcard, dev posture); present with an empty `allow_origins` → CORS disabled.
   - `deny_unknown_fields` closes the level-3 typo gap (the `[security.headers]` pattern).
   - Declared in the `GRAPH-CONFIG-SCHEMA` walk (`security` → `cors`).

2. **Wired in the demo** (`runtime/kelson-demo/src/lib.rs`): `config.security.cors` is mapped into
   `Admission.cors`; an absent section keeps `CorsConfig::default()`.

3. **`allow_credentials` added to `CorsConfig`** (`crates/web/http/src/middleware.rs`) and passed to
   tower-http's `CorsLayer`. The browser-invalid `*` + credentials combination is now **refused at
   boot** by an explicit assertion in `CorsConfig::layer()` (tower-http would only panic at request
   time — a deployment must learn about it at startup).

### Executed evidence (kelson-http, 153 passed / 0 failed)
- `cors_default_is_a_permissive_wildcard` — the dev default is `["*"]`, credentials off.
- `cors_allowlist_rejects_disallowed_origin` — an origin off the list gets **no**
  `Access-Control-Allow-Origin`.
- `cors_allowlist_allows_configured_origin` — a listed origin is echoed, and `Vary: Origin` is set.
- `cors_wildcard_without_credentials` — the wildcard posture answers `*` and never sets credentials.
- `cors_wildcard_with_credentials_is_refused` — the invalid combination refuses at boot.
- Config parity (`kelson-config`): `cors_section_parses_allowlist_and_refuses_typos`,
  `cors_section_is_optional`, `cors_keys_are_declared_in_the_config_schema`.

### Files touched
- `crates/application/configuration/src/lib.rs` — `CorsSection`, `SecuritySection.cors`, schema walk, 3 tests.
- `crates/web/http/src/middleware.rs` — `CorsConfig.allow_credentials` + boot-time validation + 5 tests.
- `runtime/kelson-demo/src/lib.rs` — plumb `config.security.cors` into `Admission`.

---

## F-2 — No HTTP panic boundary (LF-024) — **IMPLEMENTED**

### Was
- No `CatchPanicLayer` / `catch_unwind` around request dispatch.
- Only terminating hooks (`edge.rs:68`) caught panics.
- A panicking handler aborted the connection (hyper default) — no stable 500 envelope, no
  `x-request-id` on the error path, no observability stamp.

### Implemented
`tower-http`'s `catch-panic` feature is enabled and `CatchPanicLayer::custom` is attached as the
**innermost** `Admission::apply` layer:

```rust
router
    // Panic boundary (innermost): catches a handler panic and answers a
    // redacted 500 envelope. The panic payload never reaches the client.
    .layer(CatchPanicLayer::custom(|err: Box<dyn std::any::Any + Send>| {
        let msg = err.downcast_ref::<String>().map(|s| s.as_str())
            .or_else(|| err.downcast_ref::<&str>().copied())
            .unwrap_or("internal server error");
        tracing::error!(panic = %msg, "handler panicked; answering 500");
        let app_err = AppError::internal("internal server error");
        let env = Envelope::<()>::error(&app_err);
        (StatusCode::INTERNAL_SERVER_ERROR, Json(env)).into_response()
    }))
    .layer(rate_limit)      // … maintenance, cors, body-limit, trace, request-id
```

**Why innermost:** the layer order is `request-id -> trace -> body-limit -> CORS -> maintenance ->
rate-limit -> catch-panic -> router`. A panic in the handler (or in the router) is caught at the
boundary; the caught 500 then flows back **out** through the outer layers, so they still stamp their
headers on it (`x-request-id`, `x-trace-id`) and the CORS posture still applies. Placing the boundary
*outside* them would lose those headers, because the unwind skips the post-`await` code of the
interrupted middleware.

The response is a redacted `KSN_INTERNAL` envelope — the panic payload is logged server-side
(`tracing::error!`) and never sent to the client.

### Executed evidence (kelson-http)
- `panicking_handler_answers_a_500_envelope` — a panicking handler answers **500** with the
  `KSN_INTERNAL` envelope, keeps `x-request-id` **and** `x-trace-id`, and the body contains neither
  the panic message nor the embedded secret.

### Files touched
- `Cargo.toml` (workspace) — `tower-http` gains the `catch-panic` feature.
- `crates/web/http/Cargo.toml` — `tracing` (server-side panic log).
- `crates/web/http/src/middleware.rs` — the boundary + the test.

---

## F-2 (superseded) — earlier attempts and why they failed

Two earlier approaches were tried and rejected, both recorded here so the next reader does not
repeat them:

1. **`catch_unwind` on the outer future only** (`FutureExt::catch_unwind` on `next.run(req)`) —
   catches a panic in the *directly polled* future, not in nested futures awaited by intermediate
   middleware. A panic in a handler therefore escaped.
2. **Converting every admission middleware to a `Service` with a panic-catching `Future`** —
   the `tower::Service` bounds (`Future: Future<Output = Result<Response, Error>>`, body type
   parameters, `Layer` impls) made the composition far heavier than the problem; the same
   `catch_unwind` semantics apply at each layer, and the `Result` wrapping changes every signature.

The tower-http layer sidesteps both: it wraps the *service call* (so it covers the whole inner
chain) and the outer layers keep their post-processing because the boundary sits inside them.

---

## F-3 — Rate limiter: process-local, trusts first XFF, unbounded buckets (LF-026) — **PARTIALLY FIXED**

**Status:** **Phase 1 and Phase 2 are implemented** (trusted-proxy walk, bounded bucket map).
**Phase 3 (the shared store) stays open** — it depends on a product decision (does the deployment
scale horizontally?), so it is proposal-only below. The remaining gap is recorded with an executed
repro (`rate_limiter_buckets_are_process_local`, `#[ignore]`d in `kelson-http`).

### Was
- `RateLimitState.buckets = Arc<Mutex<HashMap<String, BucketState>>>` — process-local.
- `client_key` precedence: `x-rate-key` → bearer hash → **first X-Forwarded-For hop** → `global`.
- No trusted-proxy config → spoofable when not behind a sanitizing proxy.
- `HashMap` had no eviction — distinct keys grew forever (memory leak under attack/bug).

### Phase 1 — Trusted-proxy config + XFF sanitisation — IMPLEMENTED
1. **`[http.trusted_proxies]`** (`HttpSection`, `crates/application/configuration/src/lib.rs`):
   ```toml
   [http]
   trusted_proxies = ["10.0.0.0/8", "172.16.0.0/12"]  # CIDR ranges (v4 + v6)
   ```
   - Empty (the default) = **trust nothing**: a client-supplied `X-Forwarded-For` never decides its
     own rate-limit bucket (it falls through to the `global` bucket).
   - Declared in the `GRAPH-CONFIG-SCHEMA` walk; a typo is reported as `http.trusted_proxy`.

2. **`TrustedProxies` / `TrustedProxyRange`** (`crates/web/http/src/middleware.rs`): a small CIDR
   matcher (no new external dependency — the dependency budget is capped at 25, D-62). IPv4 and IPv6
   ranges; an unparsable range **refuses the boot** (fail closed) instead of widening the trust
   boundary.

3. **`client_key_with_proxies`**: the chain is walked **right-to-left**; the first hop that is not a
   trusted proxy is the client. `x-rate-key` and the bearer credential still outrank the chain.

### Phase 2 — Bucket eviction (TTL + hard cap) — IMPLEMENTED
- `RateLimitConfig` gained `bucket_ttl_secs` (default 900 s) and `max_buckets` (default 100 000).
- On every request the map is swept: idle buckets (untouched for the TTL) are dropped — they would
  have refilled to full capacity anyway, so only the clock resets — and, when the map is at its cap,
  the **oldest** bucket is evicted before a new key is inserted. The map can no longer grow without
  bound.

### Executed evidence (kelson-http)
- `client_key_ignores_forwarded_for_without_trusted_proxies` — no trusted proxies ⇒ the header is
  ignored.
- `client_key_walks_the_chain_past_trusted_proxies` — right-to-left walk, first untrusted hop wins.
- `client_key_explicit_and_bearer_beat_the_chain` — the explicit key and the bearer still win.
- `trusted_proxy_ranges_parse_and_match` — IPv4/IPv6 ranges, host routes, out-of-range and garbage
  refused.
- `rate_limit_buckets_are_capped` — 50 distinct keys against a cap of 4 ⇒ the map stays ≤ 4.
- `rate_limit_buckets_are_swept_by_ttl` — an idle bucket is swept.
- Config parity (kelson-config): `trusted_proxies_section_parses_the_ranges`,
  `trusted_proxies_keys_are_declared_in_the_config_schema`.

### Phase 3 — Shared store (Redis / DB) — PROPOSAL ONLY, NOT IMPLEMENTED
The buckets are still process-local, so a horizontally scaled deployment multiplies the effective
limit by the instance count. The remaining gap is recorded with an executed repro
(`rate_limiter_buckets_are_process_local`, `#[ignore]`d).

**This phase needs a product decision** (does the deployment scale horizontally?):
- **No** → document the process-local semantics (one instance per deployment) and close F-3.
- **Yes** → add a `RateLimitStore` trait to `kelson-http` with an in-memory implementation (today's
  behaviour) and a Redis implementation (the workspace already depends on `redis`), then plumb
  `Admission` to take `Option<Arc<dyn RateLimitStore>>`.

#### Phase 1 — original proposal (implemented above, kept for the record)
1. **Add `[http.trusted_proxies]`** (`HttpSection` in `crates/application/configuration/src/lib.rs`):
   ```toml
   [http]
   trusted_proxies = ["10.0.0.0/8", "172.16.0.0/12"]  # CIDR ranges
   ```
   - Empty = trust nothing (fallback to `x-rate-key` / bearer / socket peer).
   - When non-empty, parse `X-Forwarded-For` right-to-left, skip trusted proxies, take the first untrusted hop.

2. **Rewrite `client_key`** to use the trusted-proxy list:
   ```rust
   fn client_key(headers: &HeaderMap, trusted: &[IpNet]) -> String {
       // 1. x-rate-key (explicit, wins)
       // 2. Bearer token hash
       // 3. XFF right-to-left, first IP NOT in trusted list
       // 4. Socket peer addr (if available) → "peer:<ip>"
       // 5. "global"
   }
   ```

3. **Tests**:
   - `client_key_trusted_proxy_skips_internal_hops`
   - `client_key_no_trusted_proxy_falls_back_to_first_xff`
   - `client_key_x_rate_key_beats_xff`
   - `client_key_bearer_hash_beats_xff`

#### Phase 2 — Bucket eviction (TTL + max entries)
- `RateLimitConfig` gains:
  ```rust
  pub struct RateLimitConfig {
      pub capacity: u32,
      pub refill_per_sec: f64,
      pub bucket_ttl_secs: u64,      // entries idle this long are swept
      pub max_buckets: usize,        // hard cap; LRU eviction on insert when full
  }
  ```
- Background sweep task (or lazy on insert) removes idle buckets.
- LRU list or `linked-hash-map` for max-cap eviction.

#### Phase 3 — Optional shared store (Redis / DB)
- Trait `RateLimitStore: Send + Sync` with `check(&key, capacity, refill) -> bool`.
- InMemory implementation (current), Redis implementation (`kelson-http-redis` optional crate).
- `Admission` takes `Option<Arc<dyn RateLimitStore>>`; `None` = in-memory (back-compat).
- Only if multi-instance horizontal scaling is a P0 requirement.

### Files touched (Phase 1)
- `crates/application/configuration/src/lib.rs` — `HttpSection.trusted_proxies`.
- `crates/web/http/src/middleware.rs` — `client_key` rewrite, `RateLimitConfig` + `Admission` pass trusted list.
- `runtime/kelson-demo/src/lib.rs` — plumb `config.http.trusted_proxies` into `Admission`.
- `crates/web/http/src/middleware.rs` (tests) — 4 new unit tests.

### Files touched (Phase 2)
- `crates/web/http/src/middleware.rs` — `BucketState` gets `last_access`, sweep task, LRU on insert.
- Tests: `bucket_eviction_ttl`, `bucket_eviction_max_cap`.

### Files touched (Phase 3) — optional
- New crate `crates/web/http-redis` (or feature `rate-limit-redis`).
- `kelson-http` defines `RateLimitStore` trait.

---

## Recommended order

| Phase | Work | Risk | Notes |
|---|---|---|---|
| F-2 Phase 1 | Add `catch-panic` feature + layer + promote tests | low | Uses existing `ErrorRenderRegistry`; no new deps beyond feature flag |
| F-1 | Add `[security.cors]` + 7 tests | low | Pure config + wiring; mirrors `SecurityHeadersSection` pattern |
| F-3 Phase 1 | Trusted-proxy config + `client_key` rewrite | med | Touches request identity path; add fuzz tests |
| F-3 Phase 2 | Bucket TTL + max-cap eviction | low | Internal to `RateLimitState`; no API change |
| F-3 Phase 3 | Shared-store trait + Redis impl | high | New crate / feature; only if multi-instance is P0 |

---

## How to run the repros (today)

```powershell
# F-1
cargo test -p kelson-http --lib cors_default_is_wildcard_and_unconfigurable -- --ignored --nocapture

# F-2
cargo test -p kelson-http --lib panicking_handler_answers_a_500_envelope -- --ignored --nocapture

# F-3
cargo test -p kelson-http --lib rate_limiter_gaps_first_xff_trusted_unbounded_no_shared -- --ignored --nocapture
```

All three pass today (they assert the current buggy behaviour). Each fix should un-ignore and green the corresponding repro.
