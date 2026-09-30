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
keel queue:retry-batch <id>            # fails closed until the batch ledger persists (B-32)
```

Wave 5 added the admin surface (clear/flush/forget/monitor/prune-*/restart/retry-batch) and
promoted `queue:listen` to a real supervisor that respawns `keel-worker` on every exit with a
crash-loop backoff. `queue:restart` writes the signal file; the worker races it against its own
start time, so a signal older than the worker is ignored (test:
`restart_signal_is_ignored_when_older_than_the_worker_start`). Honest gap: `BatchRuntime` keeps
batches in memory, so `queue:prune-batches` reports an honest empty run and `queue:retry-batch`
fails closed with the per-job alternative (backlog B-32).

Wave 6 P1 registers two job defs at boot: `demo.job` (smoke handler) and `mail.send` (queued
mail — the def owns the retry budget; the handler resolves storage-capped attachments and sends
through the first healthy `[mail]` chain transport, recording the chain detail in `last_error`
on failure). Producers build through the registry with `keel_mail_smtp::enqueue_mailable`,
which validates the `Mailable` and refuses an unregistered job name (tests:
`queued_send_retries_then_dead_letters`, `enqueue_requires_registered_def_and_valid_mail`,
`registries_accept_the_mail_job_once`).

## Batches and chains

- **BatchRuntime** — many items, one durable job: per-item progress, resumable.
- **ChainRuntime** — ordered steps where each step is a job; failure halts the chain.

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
CLI's `schedule_registry()` is the single seam where module-declared schedules will land (Wave 6
module content); `schedule:list`/`schedule:run` remain information stubs over that seam.

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
