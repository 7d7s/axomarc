# 11 — Testing

TestKit (`testing/testkit`, keel-testkit) is the framework test surface: harness, fakes, time
control, factories, assertion macros.

## TestApp

Boots a platform host behind the HTTP surface for request-level tests: build the app.

**Assertion matrix (Wave 7 P2)** — every assert consumes and returns the response, and panics
with the visible body/header/context on failure:

- Web flow: `assert_redirect(location)` (exact or prefix), `assert_session_has(key)` /
  `assert_session_missing(key)` (dot-paths; a guest response satisfies missing),
  `assert_cookie(name, value = None)` (presence or exact), `assert_view_has(key)` (dot-path into
  the context the response's LAST rendered view received).
- Content: `assert_json_path("a.b", expected)` (objects; numeric segments index arrays),
  `assert_html_contains(fragment)` (raw substring), `assert_see(text)` (the HTML-escaped form —
  Blade `assertSee` parity; raw fragments belong in `assert_html_contains`).
- Database: `assert_database_has(model, predicates)`, `assert_database_count(model, n)`,
  `assert_database_missing(model, predicates)` — equality predicates ANDed, LIVE rows only
  (soft deletes never count), probed through the ModelStore under the TestApp's tenant
  (`tenant_scoped` models isolate per tenant).
- Auth: `assert_authenticated_as(principal)`, `assert_guest`.
- Existing `assert_status`/`assert_header`/`assert_json_eq`/`assert_json_contains` unchanged.

Probes are opt-in `TestApp` builders (an unwired assert fails with the wiring hint):

- `with_session(mw)` — the SAME `SessionMiddleware` wired as `mw.clone().apply(router)`; a
  `keel_session` Set-Cookie on the response is authoritative (login issues, logout clears),
  else the request cookie carries the session forward.
- `with_views(recorder)` — a `ViewRecorder` also handed to `ViewsEngine::set_tap(...)`; `send()`
  snapshots only this response's renders (a previous response's view never leaks in).
- `with_models(store)` — the `Arc<dyn ModelStore>` the database asserts read; the tenant comes
  from `.tenant(...)` (uuid) and is passed through to the store.

## Fakes (in-memory ports with asserts)

- `QueueFake` (legacy alias `FakeQueue`) — durable queue semantics; `assert_enqueued(name)`,
  `assert_empty`, counts, plus the Wave 7 `assert_pushed(job_type, |j| ...)`,
  `assert_not_pushed(job_type)`, `assert_pushed_count(job_type, n)`.
- `EventFake` — outbox recorder; share `outbox()` with the publishers under test, then
  `assert_dispatched(event_name, |e| ...)`, `assert_not_dispatched(event_name)` (the free
  `assert_event(outbox, name)` remains).
- `NotificationFake` — records what `NotifyHub::with_tap(...)` dispatched;
  `assert_sent_to(principal, notification_type, channel)`.
- `MailFake` (legacy alias `FakeMail`) — implements the notifications `MailPort`, recording
  `Mailable`s for `assert_sent(|m| ...)` (subject/to/attachments count) and mirroring them into
  the `MailMessage` log the message-level asserts read.
- `StorageFake` (legacy alias `FakeStorage`) — object port double; `assert_exists(path)` /
  `assert_missing(path)` on the S3-style `bucket/key` path, `assert_directory_empty(prefix)`.
- `CacheFake` — in-memory `CachePort` with the production TTL posture (lazy expiry, `ttl = 0`
  immediately expired) on an injectable `freeze_time` clock (`with_clock`); `assert_has(key)`,
  `assert_missing(key)`, `ttl_secs(key)`.
- `HttpFake` — re-exported from `keel-http-client`; wraps the mock recording:
  `assert_request_count(n)`, `assert_sent(|req| ...)` (method/url/body), `assert_no_pending()`.
- `FakeSearch` — backed by the in-memory search engine; `assert_indexed`, `assert_hits`
  (exact ordering), `assert_no_hits`, tenant-aware `count`.
- `AuditRecorder` — captures audit records emitted by executions.

Fakes exist per port so tests never need live postgres/smtp/minio.

## Assertions

- `assert_delta!(...)` — delta stones written by an execution (count, resource type, stream).
- `assert_event!(...)` — outbox events queued.
- `assert_queries!` — N+1 guard: assert the exact query tap count for a block; repeated identical
  queries beyond a threshold fail the test (pairs with the N+1 detector in `keel inspect`).

## Time control

`freeze_time`, `advance`, `travel_to` — deterministic schedules, retries, deadlines in tests.

## Factories

`Factory`, `JobFactory`, `EventFactory` plus `sample_job`/`sample_event` primitives.

## Architecture tests

`tests/architecture` runs on every `cargo test --workspace`: the platform graph compiles from real
manifests, every member belongs to a topology class, and the dependency matrix has zero
violations. These are regressions gates, not style checks.

## Golden path

`examples/minimal` is the executable specification of the kernel invariants (one stone per command,
idempotent replay, denial writes nothing). `examples/reference` extends it across the full module
surface. Both run in CI (G09).

## `keel test` (Wave 5)

`keel test [--pg]` runs `cargo test --workspace --locked` from the workspace root, streams the
lines, and tallies every `test result:` line into a final `PASS`/`FAIL` verdict with suite and
test counts — the same tally the parity gates report. `--pg` adds `--features pg-tests` (the
pg-gated matrix: migration pairs apply/rollback/reset round trip, advisory-lock contention,
modelstore CRUD/tenant isolation, sessions/tokens, notification inbox round trip (Wave 6 P5),
batch ledger round trip (Wave 6 P9), pgvector search) and fails closed with a
`DATABASE_URL` pointer when the compose database is not configured — so `keel test` alone stays
hermetic, `keel test --pg` is the full local matrix (backlog B-19 keeps the pg matrix out of the
shared CI).

## Playwright smoke kit (B-28)

`templates/starter-template/e2e/` ships a browser smoke kit with its own `package.json`
(external tooling — it never joins the Rust workspace or the dependency budget):

- `auth.spec.ts` — register → `/auth/me` resolves the principal; login rejects a bad
  password with the envelope error; login round-trips the same principal.
- `notes.spec.ts` — session create + list roundtrip through the graph routes (the notes
  path is read from `../module.toml`, so it follows any project name `keel new` rendered)
  and anonymous create is refused.
- `playwright.config.ts` — `baseURL` defaults to `http://127.0.0.1:8180`
  (`PLAYWRIGHT_BASE_URL` overrides); traces retained on failure.

Run it against a live `keel serve` from the rendered project:

```bash
cd applications/<name>/e2e
npm install && npx playwright install
npm test                      # PLAYWRIGHT_BASE_URL to aim elsewhere
```

The kit is CI-optional by design: it needs npm + browsers, so it runs out-of-band (locally
or an opt-in job), never as a workspace gate. Coverage claim stays honest: auth
register/login + one domain flow (notes create/list) — a smoke suite, not a full E2E net.
