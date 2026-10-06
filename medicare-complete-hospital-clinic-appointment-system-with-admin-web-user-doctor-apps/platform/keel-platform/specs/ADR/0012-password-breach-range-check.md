# ADR 0012 — Password breach check (k-anonymity range API)

Status: **Proposed** (2026-10-04) · Gate: Wave 9 advanced backlog (docs/19,
"Advanced backlog") — privacy review + egress-posture decision ·
Implementation: none.

## Context

Password checking today is entirely local: the Wave 4 composable
`password` rule (parameterized) and the Wave 8 `[auth.password_policy]`
posture (8 + all four criteria by default, enforced at register and at
password-reset completion) evaluate what the request carries and never
leave the process. The parity candidate here is Laravel's
`Password::uncompromised` / `password:uncompromised` rule: *is this
password already in a known breach corpus?*

The range-API design (HaveIBeenPwned style) answers it with
k-anonymity: compute `SHA-1(password)`, send only the first 5 hex chars
(20 bits) to a named range endpoint, receive the matching suffixes +
counts, compare locally. The full password and the full hash never leave
the process.

Constraints:

1. It is an outbound network call FROM the validation/auth path —
   request-path latency and availability, offline and air-gapped
   deployments, and a new data flow (a derived password prefix) all
   become part of sign-in and registration.
2. Budget posture is NOT the gate: `keel-http-client` / reqwest are
   already direct externals, so an adapter costs zero new crates. This
   item is decision-gated on privacy + posture, not budget — which is
   why docs/18 §26.2 lists only RS256.
3. The failure posture must be decided explicitly, never implicitly at
   implementation time: fail-open keeps registration available during an
   endpoint outage (weaker guarantee); fail-closed turns a third-party
   outage into a signup/sign-in outage.
4. Workspace gates stay hermetic: no live network in any test — the
   check must run through the existing HTTP client fake/recording port.

## Decision

The check remains decision-gated: no implementation until the privacy
review signs off. The gate conditions if it passes:

- **Data flow**: only the 5-hex-char SHA-1 prefix is ever transmitted,
  to ONE endpoint named by configuration (never derived from request
  data); the residual disclosure (20 bits of a password hash, useless
  without the password itself) is documented for the review.
- **Config surface**: an opt-in section (serde defaults, declared in
  `CONFIG_SCHEMA`, off by default) carrying at least enable, endpoint,
  timeout and cache-TTL — a stock config keeps today's byte-identical
  local-only validation.
- **Failure + budget**: the review records fail-open vs fail-closed (the
  doc recommends fail-open with the outcome surfaced as a warning flag,
  because the check is advisory hardening and a corpus outage must not
  deny service); a bounded timeout so the auth path never waits
  unbounded, and a prefix-keyed TTL cache so a hot password does not
  hammer the endpoint.
- **Evidence**: hermetic tests through the fake HTTP client (prefix-only
  request asserted, match -> validation failure, mismatch -> pass,
  endpoint error -> the recorded failure posture, disabled -> untouched
  path) in the EXISTING suites (suite count stays 150).

Alternatives considered: a local k-anonymity corpus (bundled bloom/range
file — network-free but a large freshness-managed artifact); a
post-authentication async sweep (availability-safe, but it verifies
after the session exists rather than refusing the password); the status
quo (local policy only — remains the documented divergence until this
gate passes).

## Consequences

- Nothing moves now: no code, no config keys, no data flow — the
  validation path stays fully offline.
- This document is the gate artifact: implementation starts only after
  the privacy review records its answers to the conditions above (the
  ADR then flips Proposed -> Accepted with the review reference).
- If adopted later, the egress allowlist posture established here (one
  config-named endpoint, no request-derived destinations) is the
  template for the JWKS fetch question raised in ADR 0011.
