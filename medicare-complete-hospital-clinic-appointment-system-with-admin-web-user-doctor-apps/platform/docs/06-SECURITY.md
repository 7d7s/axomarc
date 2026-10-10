# 06 — Security

Security is the kernel, not a feature list. Five crates: identity, policies, scopes, ownership,
elevation (+ sessions/tokens opt-in).

## Model

- **Principal** — authenticated identity (argon2 password hashes, JWT, OTP skeleton).
- **Capability** — named permission, e.g. `appointments.book`. Declared per module in
  `module.toml`, checked on every execution.
- **Scope** — lattice of nodes inside a tenant (regions, sites, departments). Decisions are scoped:
  a principal can hold a capability *and* still be denied outside their scope.
- **Ownership** — resource ownership snapshots feed authorization for object-level rules.
- **Elevation** — temporary, scoped privilege escalation with its own audit chain. Never implicit.

## Enforcement points

1. **Engine authorization boundary** — before any work: `plan.authorization` -> policy decide ->
   `DENIED` with reason recorded in the diagnostic chain.
2. **Plane invariant** — tenant operations cannot run on the platform plane and vice versa.
3. **Revisions** — access snapshots carry authorization/feature/module revisions; stale decisions
   are detectable.
4. **Transport security headers (Wave 7 P4)** — web-plane responses only: `[security.headers]`
   (off by default, additive) stamps `X-Content-Type-Options: nosniff`, HSTS (`max-age`, opt-in
   `includeSubDomains`), the baseline CSP (`default-src 'self'`, per-deployment override) and
   `Referrer-Policy: strict-origin-when-cross-origin` as the outermost per-route layer, so error
   views and route-gate rejections carry them too. The api plane is never touched; handler-set
   headers win over the middleware; an unparseable config value is skipped fail-closed (04-HTTP,
   19-PARITY Wave 7 P4).
5. **CSRF (Wave 7 P7)** — web-plane routes only, always on: the double-submit guard pairs the
   `kelson_csrf` cookie with the `X-CSRF-TOKEN` header (safe requests bootstrap the cookie;
   state-changing requests without the pair answer 403). The only configuration is
   `[security.csrf].skip_paths` — exact paths that skip the check (webhook receivers signed by
   provider HMAC, and the like); there is no flag or env, and the old `KEEL_CSRF` opt-in is gone.
   The api plane stays token-authenticated and platform-plane flows (login, register, reset) are
   unchanged (04-HTTP, 19-PARITY Wave 7 P7).
6. **Cookie-value encryption (Wave 7 P7 roundtrip; toggle Wave 2)** — `[http] encrypt_cookies`
   (default off) seals queued cookie values with AES-256-GCM under a key derived from the
   application key; the wire value is a `k1.` envelope, and only a cipher under the same app key
   can open it (04-HTTP).

## Agents and AI

AI agents execute tools through the exact same engine path under the caller principal (see
`10-AI.md`). There is no elevated agent mode; a denied tool is a tool error reported to the model.

## Secrets hygiene

Gate G12 + `kelson security` scan for secrets, check dependency policy, and verify trace locality.
Secrets are asserted at boot (production refuses silent defaults); config dumps redact them.

## Error taxonomy (frozen)

Categories: Validation, Authentication, Authorization, NotFound, Conflict, Workflow, Domain,
Persistence, External, ResourceExhausted, Internal. Each error carries code, public message,
severity, retryability, meta. Public messages are safe for the wire; internals stay in logs.

## HTTP authentication flows (S7)

- Register/login/logout/me and password reset are served on the platform plane by `kelson-api`.
  Passwords are hashed with argon2 (`kelson-identity`) under the `[identity]` work factors
  (`memory`/`time`/`parallelism`, argon2 baseline defaults); a successful login re-hashes the
  password when the stored verifier's embedded factors drift from that config (rehash-on-login —
  a factor upgrade converges old verifiers on first login without ever blocking the login
  itself). Failed logins run a dummy hash so unknown identifiers do not answer faster than bad
  passwords. Register and password-reset completion enforce the `[auth.password_policy]`
  strength rules (`min_length`, `letters`, `numbers`, `symbols`, `mixed_case` — Laravel-posture
  defaults), the same policy that feeds the Wave 4 `password` validation rule at the dispatch
  edge, so the composable and the flows never drift apart; a refused reset attempt never
  consumes the reset token.
- Browser clients get an HMAC-tagged `kelson_session` cookie (HttpOnly, SameSite=Lax, Secure in prod);
  API clients use hashed PATs via `Authorization: Bearer`. Session fixation is defended at login and
  register: any session the client presented *before* authenticating is destroyed server-side, then
  a fresh session id is issued (rotate-and-invalidate, tested in `http_integration.rs`). Logout
  destroys server-side state and revokes a presented PAT.
- Invalid/revoked bearer credentials map to 401 at the transport edge regardless of the domain
  error category; capability denials stay 403.
- Login attempts are throttled per IP+identifier (burst 5) before credential checks run.
- JWT stays non-default (report R5): B-22 (v1.1) ships an opt-in HS256 *interop verifier*
  behind the `app.jwt` flag (`kelson flag:set app.jwt 100` plus a non-empty `[auth].jwt_secret`).
  Only `alg = HS256` is accepted (`none` and every other algorithm rejected before any crypto),
  the signature is verified over the exact received signing input with the tested sha2 HMAC in
  constant time, `exp` is mandatory, and every failure collapses to the same uniform 401 as a
  bad PAT — covered by `jwt_expired_forged_tampered_garbage_are_uniform_401`,
  `jwt_bearer_inert_when_verifier_not_wired`, and `pat_bearer_still_authenticates_with_jwt_enabled`.
  Bearer resolution is PAT-first; the default model (hashed PAT + HMAC session) is unchanged,
  and RS256 remains budget-gated (transitive `ring` promotion needs a recorded decision).
- Email verification (B-23, v1.1): `/auth/email-verification/request` issues a single-use 60-minute
  `verify` token (`TokenPurpose::Verify`); `/auth/email-verification/confirm` redeems it once and
  stamps `principals.email_verified_at`. Enumeration-safe (unknown addresses get the same
  `sent:true` answer, no token), replay-rejected (single-use), and covered by
  `email_verification_flow_marks_principal_verified`, `email_verification_request_is_enumeration_safe`
  and the durable-path `pg_email_verification_timestamp_roundtrip`. Optional config
  `[app].require_verified_email = true` gates graph module operations with 403 `AUTH_DENIED` until
  the address is proven; auth endpoints stay open so verification itself can complete.

## v0.9 hardening pass

The release-gate security review (auth, cookies, uploads, CSRF, authorization, tenancy) landed the
following changes and tests:

- **Constant-time comparisons.** The HMAC verification in `web/http/src/signed.rs` already used a
  constant-time compare; `constant_time_eq` is now the single comparison primitive for all
  secret-token checks at the edge: the CSRF double-submit header/cookie compare and the session
  cookie tag check both route through it (previously plain equality). Its semantics (equal,
  per-position difference, length mismatch) are pinned by unit tests so a future optimisation
  cannot reintroduce a timing side channel.
- **Session fixation.** `finish_login` (register + login) now receives the client-presented
  pre-authentication session, destroys it, and issues a fresh authenticated session. Covered by
  `login_rotates_session_and_invalidates_prelogin_cookie`: K1 live, login presenting K1, K2
  issued, K1 dead server-side, K2 live.
- **Login timing equalization.** Unknown identifiers and missing credentials verify against a dummy
  argon2 hash so error paths do not answer faster than a real bad-password check (pre-existing,
  re-audited; Wave 7 P5 builds the dummy at the configured `[identity]` factors on first use, so
  the equalization holds after a factor change too).
- **Enumeration resistance.** Password-reset request answers identically for known and unknown
  identifiers (pre-existing, re-audited).
- **Upload abuse surface.** Bounded multipart pipeline: 8 parts, 64 KB fields, 8 MB files, enforced
  mid-stream (oversized parts abort before buffering). Attack tests: a part bomb beyond the cap
  trips `429 TOO_MANY_REQUESTS` while a request at exactly the cap still succeeds; traversal
  filenames (`../../evil.sh` becomes `evil.sh`) are stripped to basenames; unauthenticated uploads
  are 401; non-multipart bodies are 422 envelopes. Filenames never contribute path components to
  storage keys.
- **Forged cookies.** A session cookie with an invalid HMAC tag is treated as anonymous (no error
  oracle), asserted by `session_cookie_carries_secure_defaults_not_secure_in_test` and the
  `forged_tag_is_anonymous` middleware test.
- **Tenancy.** Every query path applies `tenant_scope`; cross-tenant reads are covered by the
  PostgreSQL integration matrix (`pg-tests`).

Out of scope for v1 (parity-gate decisions, unchanged): JWT default, WAF-style edge filtering.
The OAuth2 authorization server ships as an optional module (`applications/oauth2`, Wave 8
B-21), never a default: authorization code + PKCE S256 with a consent-less first-party flow,
single-use codes, refresh rotation, RFC 7009 revocation (200 `{"revoked": bool}` even for
unknown tokens), an argon2-hashed `oauth_clients` registry (redirect allowlist, scopes, secret
shown once) and capability-gated client management — hashed PAT + HMAC sessions remain the
platform's auth, and the module's web-plane POSTs ride the same CSRF double-submit guard as
everything else (asserted by the `oauth_*` matrix in `runtime/kelson-api/tests/http_integration.rs`).
