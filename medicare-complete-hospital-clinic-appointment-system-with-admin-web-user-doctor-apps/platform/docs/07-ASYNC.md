# 07 — Async

Four capabilities: events, jobs, workflows, scheduler (`crates/async/*`).

## Events + transactional outbox

Handlers queue events inside the unit of work (`unit.queue_outbox(...)`); the backend flushes them
with the commit. The outbox pump claims unpublished batches and dispatches to registered listeners;
events are marked published only after all listeners succeed. Listeners register via
`ListenerSpec` (event name, handler) — no reflection, no string conventions beyond manifest names.

Declaring events is manifest-level:

```toml
[[events]]
name = "appointment.booked"
```

`keel event:list` prints the catalog; duplicates are graph violations (G05).

## Jobs and queues

Durable jobs (`DurableJob`) with statuses pending/running/succeeded/failed/dead. Claims use
`SELECT ... FOR UPDATE SKIP LOCKED` (postgres) or the same semantics in-memory. `keel-worker` is
the worker binary.

```bash
keel job:list --status failed
keel queue:work --once
keel queue:failed
keel queue:retry <job-id>
keel queue:listen                      # supervise the worker: respawn + crash-loop backoff
keel queue:monitor                     # depth + oldest-age snapshot
keel queue:clear                       # purge pending (dev/test-only, KEEL_FORCE=1 to script)
keel queue:flush                       # delete every dead job
keel queue:forget <id>                 # delete one dead job
keel queue:prune-failed --hours 168    # age-bounded dead-letter pruning (default 168h)
keel queue:prune-batches --hours 72    # prune terminal batch records (default 72h)
keel queue:restart                     # restart-signal file: workers stop after the current job
keel queue:retry-batch <id>            # requeue every failed/dead member of a batch (B-32)
```

Wave 5 added the admin surface (clear/flush/forget/monitor/prune-*/restart/retry-batch) and
promoted `queue:listen` to a real supervisor that respawns `keel-worker` on every exit with a
crash-loop backoff. `queue:restart` writes the signal file; the worker races it against its own
start time, so a signal older than the worker is ignored (test:
`restart_signal_is_ignored_when_older_than_the_worker_start`). Wave 6 P9 closed the batch gap
(B-32): `BatchRuntime` mirrors membership to a pool-gated `BatchLedger` — saved BEFORE enqueue,
so a ledger refusal aborts dispatch with nothing enqueued — `PgBatchLedger` at full boot and
`MemoryBatchLedger` degraded; `queue:prune-batches` ages terminal records out for real and
`queue:retry-batch` requeues members through `BatchOps::retry` (tests
`batch_commands_validate_before_connecting`, `pg_batch_ledger_roundtrip`).

Wave 6 P1 registers two job defs at boot: `demo.job` (smoke handler) and `mail.send` (queued
mail — the def owns the retry budget; the handler resolves storage-capped attachments and sends
through the first healthy `[mail]` chain transport, recording the chain detail in `last_error`
on failure). Producers build through the registry with `keel_mail_smtp::enqueue_mailable`,
which validates the `Mailable` and refuses an unregistered job name (tests:
`queued_send_retries_then_dead_letters`, `enqueue_requires_registered_def_and_valid_mail`,
`registries_accept_the_mail_job_once`).

Wave 6 P8 advanced the queue admission plane (all `serde(default)` additions to `JobDef`):

- **Unique jobs** — `unique_key` + `unique_ttl_secs` (default 300): admission takes a cache
  lock keyed `queue:unique:<job>:<key>` before the handler; a claim whose key is held
  elsewhere is deferred without failing (attempt given back, requeued on a short poll
  cadence) and the lock releases only when the run succeeds. The worker's guard shares the
  scheduler's `CacheService` seam (`CacheUniqueLock`; `MemoryUniqueLock` behind tests).
- **Rate-limited jobs** — `rate_limit = { capacity, refill_per_sec }` shapes a token bucket
  per job type; an exhausted bucket `Hold`s the claim — requeued at the next-token time,
  never failed, never dead-lettered.
- **Job middleware** — `JobMiddleware` (`before` → `Control::{Proceed, Hold, Abort}`, plus
  `after` observing every proceeded result) composes in `MiddlewareStack`s;
  `JobMiddlewareStacks` builds each job's stack lazily as the built-ins (unique →
  rate-limit → retry-limit) followed by module layers, frozen at first dispatch. `Abort`
  kills the job terminal, `Hold` routes through the new `JobStoreAdmin::defer`
  (running → pending, `attempt` given back, `available_at` pushed) — both `defer` and
  `kill` implement on the in-memory, Postgres and Redis stores. Registration refuses
  impossible admission configs (empty unique key, zero TTL, zero capacity,
  non-finite/non-positive refill).
- **Metrics** — `/metrics` gains the scrape-time queue census (`keel_queue_depth{status}`,
  `keel_queue_oldest_age_seconds`, `keel_queue_finished{outcome}`; see 13-OPERATIONS).

Tests (jobs): `unique_admission_holds_locked_claims_until_release`,
`rate_limited_admission_holds_until_the_bucket_refills`,
`admission_defer_returns_the_attempt_and_kill_is_terminal`,
`retry_limit_aborts_over_budget_claims`, `middleware_stack_orders_hooks_and_short_circuits`,
`stacks_compose_builtins_then_module_layers`, `registry_refuses_impossible_admission_configs`;
(worker): `dispatch_holds_a_rate_limited_job_without_failing_it`,
`dispatch_skips_a_locked_unique_job`, `dispatch_runs_module_middleware_around_the_handler`,
`dispatch_kills_an_over_budget_job`; (keel-api):
`metrics_expose_queue_depth_age_and_throughput`. The codegen `job-middleware` kind now emits
`impl JobMiddleware` with the real before/after shape (needle pinned by
`wave5_templates_reference_real_platform_surface`).

## Batches and chains

- **BatchRuntime** — many items, one durable job: per-item progress, resumable. Wave 6 P9 (B-32)
  gives it a durable `BatchLedger` (membership written once at dispatch, `mark_finished` stamped
  when the run settles) and the `BatchOps` service — `status` (live counts), `cancel` (pending →
  dead via `kill`), `retry` (failed/dead → pending via `requeue`) — shared by the
  capability-gated `GET/POST /api/v1/batches/{id}[/cancel|/retry]` endpoints (docs/04) and
  `queue:retry-batch` (tests `batch_status_counts_members_from_the_store`,
  `batch_cancel_and_retry_move_members`, `batch_ledger_records_finishes_and_prunes`).
- **ChainRuntime** — ordered steps where each step is a job; failure records the halted
  member position (`ChainStatus::Halted { position }`), repeats keep the first failure, and
  a later success (e.g. `queue:retry`) resumes the chain through `Completed`
  (tests `chain_advances_only_on_success`, `chain_failure_records_the_halted_position`).

Both sit on the job store; they add orchestration semantics, not new infrastructure.

## Workflows

Explicit state machines (`async/workflows`): states, transitions, guards. Workflow execution is an
operation profile (`WORKFLOW`) with plan enforcement like any other command.

## Scheduler

Registered schedules tick in the runner; `keel schedule:list` / `keel schedule:run` for inspection
and one-shot due runs.

Wave 5 added the daemon and dry-run surfaces:

```bash
keel schedule:work [--every n]   # tick loop (default 60s): fire due items as durable jobs, save state
keel schedule:test               # pure dry run: registered vs due-now (nothing fires, no writes)
```

`schedule:work` fires each due item as a durable job (`max_attempts = 1`, the item's payload) and
persists last-fired state after every firing pass, so a restart or `schedule:test` never re-fires.
With an empty registry it reports the empty posture and exits before touching the database — the
CLI's `schedule_registry()` is the single seam where module-declared schedules will land — still
empty after Wave 6 (P4 delivered the scheduling vocabulary, while module declarations are module
content outside the nine-wave program); `schedule:list`/`schedule:run` remain information stubs
over that seam.

Wave 6 P4 fills in Laravel's scheduling vocabulary (all additive — `serde(default)` fields on
`ScheduleItem`, so pre-P4 state files and serialized items still load):

- **Cron** — a zero-dep 5-field parser (`minute hour day-of-month month day-of-week`; `*`, lists
  `1,15`, ranges `1-5`, steps `*/5` and `1-30/2`; numbers only). `ScheduleItem.cron` takes
  precedence over `spec`; an invalid expression, an out-of-range value, or an expression that can
  never fire (`0 0 31 2 *`) refuses `register` at boot with the field named. Day-of-month and
  day-of-week both restricted → either matches (classic Vixie OR); `7` on the weekday field is
  Sunday. Builder helpers map to cron strings: `hourly()`, `daily()`, `daily_at("13:00")`,
  `weekly()`, `monthly()`, `every_minutes(1..=59)`.
- **Timezone** — `ScheduleItem.timezone` is an IANA name validated against a bundled static table
  (unknown → registration error). Cron, daily and weekly wall clocks compute `next_run` in the
  item's zone; `Interval` is absolute seconds and ignores it. The table carries **fixed** offsets
  (DST transitions are not modeled — `chrono-tz` would break the zero-new-dependency budget), so
  DST-observing zones run at their standard-time offset; zones that never observe DST — including
  the platform default `Asia/Kolkata` — are exact.
- **Guards** — `without_overlapping` holds a lock keyed `schedule:overlap:<name>` for
  `lock_ttl_secs` (default 3600s = assumed max runtime) across the firing window: a still-held
  lock skips the run silently instead of double-firing; a failed run releases it so the next tick
  retries. `on_one_server` acquires `schedule:one-server:<name>` (TTL 300s safety valve), runs,
  and releases within the tick — serialization across nodes when the lock backend spans them. The
  guard port is `ScheduleLock` (`try_acquire`/`release`); the runner default is the in-process
  `MemoryScheduleLock` (single-server posture, D-52), while the worker builds a
  `CacheService::lock`-backed port — the Redis adapter when `REDIS_URL` connects, the in-memory
  port otherwise. The `schedule:work` console daemon keeps the process-local lock: `Tools` may not
  depend on adapter crates (STAGE-0-SPEC §3), so multi-node guards run through the worker tick.
- **Hooks** — `ScheduleHooks { before, after, on_success, on_failure }` attach per item via
  `ScheduleRegistry::register_hooks` (items stay serializable; double registration or an unknown
  name refuses). `before` gates the run — a rejecting hook skips the enqueue and records a
  failure; `after` observes every attempt; `on_success`/`on_failure` split on the outcome. Hook
  errors are recorded like any other failure (`after`/`on_success`), never swallowed.
- **Failure state** — the scheduler state doc (`storage/framework/schedule-state.json`) gains a
  `failures` array (`serde(default)`: pre-P4 files load unchanged), capped at 100 newest entries.
  A failing item (enqueue or `before`) is recorded and skipped — it never aborts the pass and does
  not advance its last-fired stamp, so the next tick retries it. The recorded text prefers the
  error's internal detail (the file is operator-only local state, never served).
