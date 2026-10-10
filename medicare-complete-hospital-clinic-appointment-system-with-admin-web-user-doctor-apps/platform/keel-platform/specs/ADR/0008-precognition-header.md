# ADR 0008 — Precognition (validate-only dispatch)

Status: **Accepted** (2026-10-02) · Gate: Wave 8 decision gate (docs/19, "Decision gates") ·
Implementation: landed 2026-10-02, the day the gate passed review — the one header read
plus the early return in `graph_dispatch` exactly as the Locus section specifies.
Implementation-reconciliation notes are folded into the sections below.

## Context

Laravel's Precognition lets a client fire a request and get back ONLY the validation
outcome — 200/204 when the payload would pass, 422 with the error bag when it would not —
without touching the application (no dispatch, no jobs, no writes). The platform's graph
dispatch today always runs the full pipeline: auth → CSRF → intake → merge → pagination →
bindings → locale → manifest `[[validation]]` (collect-all, 422 envelope with `fields`
meta, `graph_routes.rs` "FormRequest wiring") → idempotency read → `ExecutionContext` →
`CommandBus::dispatch`. The parity gap is recorded as a Wave 8 decision gate: "Precognition
header on graph routes, runs `[[validation]]` only" (docs/19 Decision gates; docs/18 row).

Constraints:

1. **Zero side effects by construction**, not by policy — nothing may be written, queued,
   minted, or consumed (idempotency keys, audit, outbox, jobs, tokens, uploads).
2. Budget posture: no new dependency, no new env var, no manifest change (`MODULE_ABI`
   stays 1), and the default path (no header) stays byte-identical.
3. Security posture unchanged: precognition must never become an authz bypass.

## Decision

### Dispatch spec

- **Header**: `Precognition` (name matched ASCII case-insensitively per HTTP). The mode
  activates only when the value, trimmed and compared ASCII case-insensitively, is
  `true`. Any other value is treated as absent — an unrecognized value never fails a
  request (documented; the header is a client hint, not a contract with teeth).
- **Scope**: graph routes only, both planes (web + module). Platform routes (`/auth/*`,
  `/api/v1/broadcast`, uploads, meta/health) ignore the header entirely.
- **Pipeline position**: every stage up to and including manifest validation runs exactly
  as today — edge admission (rate limit, maintenance, CSRF), identity + capability
  (`401`/`403` unchanged), body intake (JSON/form/multipart rules below), input merge,
  pagination injection, route bindings, locale resolution, then the collect-all
  `[[validation]]` run over the merged input.
- **Outcome**:
  - validation **passes** → respond `204 No Content`, empty body. The code below the
    return does not exist: no idempotency-key read, no `ExecutionContext`, no
    `CommandBus::dispatch`, no mutation record, no audit/outbox/job/token path, no
    execution trace. Side-effect freedom is structural (the return is placed immediately
    after the validator's success branch, before everything else named above).
  - validation **fails** → the existing 422 envelope, byte-identical to a normal
    validation failure (same wording, same `fields` meta — one response, two causes).
  - the route declares **no** `[[validation]]` specs → vacuous pass → `204` (nothing to
    check; the request still proves auth + CSRF + existence).
- **Security posture**: `401`/`403` happen before validation exactly as today — a
  precognition probe learns only what the caller is already allowed to attempt. Web-plane
  POSTs still require the double-submit CSRF token (documented: precognition forms send
  the same token as the real submit).
- **Reads vs writes during validation**: `unique`/`exists` row probes and the
  `current_password` verifier run — they are reads and cannot mutate state (documented:
  "side-effect-free" is about writes, not about avoiding SELECTs).
- **Refusal**: `multipart/form-data` bodies are refused with a `422` envelope
  (`KSN_VALIDATION`, message "precognition does not accept multipart bodies"). Multipart
  intake stores upload parts during the intake stage — i.e. BEFORE validation — so
  accepting the body would break the zero-side-effect promise before the validator ever
  runs (fail-closed instead). JSON and form-urlencoded bodies are accepted normally.
- **Responses carry no special headers**: `204` has no body and no precognition-branded
  header; failures are the ordinary envelope (clients key off status alone).
- **Always on**: no flag, no config, no feature gate — the mode cannot write, so there is
  nothing to protect against enabling by default.
- **Locus**: one header read plus one early return in `graph_dispatch`
  (`runtime/kelson-api/src/graph_routes.rs`), immediately after the manifest-validation
  success branch and before the idempotency-header read. No engine, config, registry, or
  manifest surface moves.

*Implementation reconciliation:* the header read sits with the other request metadata
before intake (`HeaderMap::get` supplies name case-insensitivity; the value is trimmed
and compared with `eq_ignore_ascii_case`), and the multipart refusal sits immediately
after the `multipart/form-data` content-type probe — before `ingest_multipart` can store
a part — as `AppError::validation` (the exact message above) through `dispatch_error`,
so it renders the standard `KSN_VALIDATION` 422 envelope. The early return issues a bare
`StatusCode::NO_CONTENT` from `graph_dispatch`: the shared success-render tail
(Cache-Control/ETag/Deprecation) belongs to dispatched responses and does not run for a
probe, per the Outcome rule that everything below the return does not exist.

## Consequences

- Zero new dependencies, zero new env vars, zero manifest fields; `MODULE_ABI` stays 1.
- The default path is untouched (no header → identical control flow), so no existing
  test changed; evidence landed in the EXISTING `http_integration` suite as six tests
  (pass → 204 with the store proving zero dispatch side effects + an unrecognized value
  behaving as absent, fail → the byte-identical 422 envelope with `fields` meta, no-
  validation route → vacuous 204 with the no-header control still 200, multipart → the
  documented 422 with the same body booking normally without the header, anonymous →
  401, missing CSRF → 403) — suite count stays 150.
- Clients get a cheap "would this payload validate?" probe that is safe to point at
  production: structurally incapable of mutating state.
- Known non-goal: precognition does not preview dispatch RESULTS (serialization,
  effects) — only the validation outcome (parity with Laravel Precognition's scope).
