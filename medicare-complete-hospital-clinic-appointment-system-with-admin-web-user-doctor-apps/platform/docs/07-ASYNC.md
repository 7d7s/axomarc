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
```

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
