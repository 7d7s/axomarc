# 11 — Testing

TestKit (`testing/testkit`, keel-testkit) is the framework test surface: harness, fakes, time
control, factories, assertion macros.

## TestApp

Boots a platform host behind the HTTP surface for request-level tests: build the app.

## Fakes (in-memory ports with asserts)

- `FakeQueue` — durable queue semantics; `assert_enqueued(name)`, `assert_empty`, counts.
- `FakeStorage` — object port double with inspection helpers.
- `FakeMail` — captured `MailMessage`s for notification tests.
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
pgvector search) and fails closed with a
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
