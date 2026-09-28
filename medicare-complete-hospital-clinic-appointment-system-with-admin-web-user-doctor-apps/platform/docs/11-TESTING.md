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
