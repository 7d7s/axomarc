# ADR 0009 — Operations dashboard (read-only over inspect/job/scheduler stores)

Status: **Proposed** (2026-10-02) · Gate: Wave 8 decision gate (docs/19, "Decision gates") ·
Implementation: only after the gate passes review

## Context

`/_dashboard` today is Horizon-lite (`runtime/keel-api/src/dashboard.rs`): a const HTML
page that fetches the JSON meta surface (`/healthz`, `/readyz`, `/meta/boot`,
`/meta/flags`, `/meta/routes`, `/metrics`), served by a plain HTML route behind the
`dashboard_enabled` gate (dev/test default; production needs the `app.dashboard` flag;
disabled → 404 envelope). docs/16 maps Laravel Horizon onto it; docs/18 records the
remaining gap. The Wave 8 decision gate asks for the operational half: a **read-only**
dashboard over the inspect/job/scheduler stores — queue depth, recent jobs, schedule
state, execution inspect — using Wave 2 views + HTML routes, flag-gated, with **all data
from existing stores and no new collection paths** (docs/19 Decision gates).

The stores exist and are already read by other surfaces:

- `JobStoreAdmin` (held on `AppState` as `queue: Option<Arc<dyn JobStoreAdmin>>`) offers
  `snapshot() -> QueueSnapshot { pending, running, dead, succeeded, oldest_pending }`
  (the `queue:monitor` source) and `list(status, limit) -> Vec<DurableJob>`.
- The scheduler persists `SchedulerStateDoc { last_fired, failures }` at
  `storage/framework/schedule-state.json` (`state_path`/`load_state_doc`), written by
  `schedule:work`; `ScheduleRegistry` (names + `next_run`) is the in-process item set.
- The engine saves an `ExecutionTrace` per execution (`engine.rs`: `tracer.save(&trace)`)
  into `FileTraceStore::standard(root)` — the same file plane `keel inspect` reads.
  `TraceStore` offers `recent(limit)` and `get(id)`. `keel-api` holds no trace store yet.
- `ExecutionTrace` carries operation/profile/principal/tenant/plane/capability/authz,
  query taps, delta/audit/outbox counts, error, started_at — and `input`, which is
  documented as environment-internal ("must not leave the environment").

## Decision

### Page + route

- New route **`GET /_dashboard/ops`**, an HTML route rendered by the Wave 2 views engine
  (`dashboard/ops.html` + partials), gated by the SAME `dashboard_enabled` switch
  (dev/test default, `app.dashboard` in production, disabled → the existing 404
  envelope). The existing lite page at `/_dashboard` is left byte-for-byte untouched
  (zero regression; it links onward to `/ops` is an implementation nicety, not a
  requirement).
- **Identity**: the ops route resolves a principal through the shared identity resolver
  (session cookie or bearer PAT, same posture as the platform-plane endpoints);
  anonymous → 401 envelope. No capability is demanded (no admin capability exists —
  B-33 backlog; documented).
- **No views engine** (degraded harness): the route degrades to a JSON envelope carrying
  the same four sections, mirroring the documented HTML-route degradation — never a 500.
- **Refresh**: the view carries `<meta http-equiv="refresh" content="10">` — server-side
  polling only; no JS, no client fetch fan-out (the lite page's 5s fetch loop stays as
  is).
- Each section renders independently and degrades to inline "unavailable" text on a
  store error or `None` handle — the page NEVER 500s (the lite page's "n/a" posture).

### Data-source spec (read-only; zero new collection paths)

| Section | Source | Fields rendered |
|---|---|---|
| Queue depth | `AppState.queue` → `JobStoreAdmin::snapshot()` | `pending`, `running`, `dead`, `succeeded`, oldest-pending age (`now - oldest_pending`); `None` handle → "queue store unavailable (degraded boot)" |
| Recent jobs | `AppState.queue` → `JobStoreAdmin::list(None, 50)` | `id`, `name`, `queue`, `status`, `attempt`/`max_attempts`, `priority`, `enqueued_at`, `finished_at`, `last_error` — **`payload` is never rendered** (business data, not an audit surface) |
| Schedule state | `load_state_doc(root)` (the file `schedule:work` persists) plus, when attached, `AppState.schedule: Option<Arc<ScheduleRegistry>>` | `last_fired` per item (name + instant), `failures[]` (name, at, error — operator-only state file content), registered items with `next_run(now)` from the registry; missing file → "no schedule state yet"; `None` registry (today) → "no schedules registered (registration is programmatic; see docs/13)" |
| Execution inspect | `AppState.traces: Option<Arc<dyn TraceStore>>` — NEW optional AppState field wired to `FileTraceStore::standard(root)`, the byte-identical plane the engine saves to and `keel inspect` reads → `recent(50)`; detail row → `get(id)` | `execution_id`, `operation`, `profile`, `principal`, `tenant`, `capability`, `authz`, `ok`/error, query count + total ms, delta/audit/outbox counts, `started_at` — **`input` is never rendered** (documented environment-internal); `None` handle → "trace store unavailable" |

- All four sections read at render time from handles the process already has (or the
  state file / file-backed trace plane it already writes) — **no new writes, no new
  pollers, no new files, no new env vars, no new config keys**. The only AppState
  additions are two `Option` handles (`schedule`, `traces`) following the D-52
  degraded-boot posture already used by `queue`/`batches`.
- Secrets/payloads stay out of HTML by construction: the renderer whitelists the field
  sets above; no store object is serialized wholesale.

## Consequences

- Zero new dependencies, zero new env vars; no manifest changes (`MODULE_ABI` stays 1);
  the flag gate and disabled-404 posture of the existing dashboard are reused unchanged.
- Suite count stays 150: evidence lands in EXISTING suites (view unit: four sections
  render + per-section degradation + sensitive-field absence; keel-api route tests:
  404 flag-off, 401 anonymous, 200 principal + view, JSON degradation without views).
- Operators get Horizon-class visibility (queue depth, recent jobs, schedule state,
  execution inspect) from stores that already exist — `queue:monitor`/`keel inspect`
  remain the terminal equivalents of the same data.
- Recorded non-goals for this gate: job payloads and trace inputs (never exposed),
  mutating actions (retry/kill/purge — those stay on the console commands), historical
  metrics charts (the `/metrics` scrape surface already covers time series), and a
  separate admin capability (blocked on B-33).
