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
