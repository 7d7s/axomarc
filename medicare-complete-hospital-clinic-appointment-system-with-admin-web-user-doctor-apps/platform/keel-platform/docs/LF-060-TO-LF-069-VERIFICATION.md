# LF-060 … LF-069 — Queues, events, scheduling & concurrency

**Batch:** the ten queue/event/scheduling items of the Laravel Feature Atlas, verified against the
framework as it stands.

**Method (the project's rule):** every status below is backed by a **named, executed** test. A
documentation claim is never proof — including this document's own earlier drafts. Where an item is
only partly covered, the missing cases are named.

**Verdicts:** **9 PARTIALLY VERIFIED** · **1 NOT IMPLEMENTED** (LF-067 ✓). No item is VERIFIED: each
one has at least one of the atlas's own required cases missing ✓, and the gaps are named below ✓.
LF-068 was **implemented this pass** ✓ (it had no implementation at all ✓).

---

## 1. What was executed

| Suite | Result |
|---|---|
| `cargo test -p kelson-jobs --lib` | **37 passed** — the queue contract, the retry/budget, the backoff, the unique lock, the rate-limit admission, the defer/kill, the batches, the chains, the schema migration, the tenant isolation, the pentest cases and the restart signal |
| `cargo test -p kelson-event --lib` | **13 passed** — the outbox pump, the idempotency key, the strict ordering, the failed-listener redelivery, the tenant isolation and the pentest case |
| `cargo test -p kelson-scheduling --lib` | **26 passed** — the cron grammar, the one-server guard, the overlap guard, the hooks, the state round trip, the tenant isolation and the pentest case |
| `cargo test -p kelson-realtime --lib` | **9 passed** — the channel naming/sanitizing, the publish fan-out, the presence roster, the SSE endpoint, the WS round trip over a real socket and the authorized transport |
| `cargo test -p kelson-workflow --lib` | **5 passed** — the workflow compile, the legal/illegal transitions, the dead end, the duplicate transition and the unreachable state |
| `cargo test -p kelson-worker` | **16 passed** — the claim/dispatch path, the middleware ordering, the unique lock, the rate-limited hold, the schema migration, the over-budget kill and the **deadline guard** (added this pass) |
| `cargo test -p kelson-process --lib` | **8 passed** — the argv reaches the child verbatim, the environment is isolated by default, the deadline kills the run, the missing program is an error, the empty program is refused and the fake answers from its script (**new crate**, added this pass) |
| `cargo test -p kelson-postgres --features pg-tests` (live MySQL 8) | the queue + outbox + job-bundle cases against a real database: `mysql_job_store_claim_completes_and_fails_with_skip_locked` (the **SKIP LOCKED** contention ✓), `mysql_job_store_recovers_orphaned_running_job_after_worker_crash` (the **worker kill** ✓), `mysql_job_store_tenant_scoped_queue_is_usable_and_isolated` ✓, `mysql_outbox_store_claim_and_mark_published` ✓, `mysql_batch_ledger_roundtrip` ✓, `mysql_savepoint_rollback_isolates_two_tests_sharing_a_table` ✓, `mysql_a_traced_job_row_carries_the_request_correlation` ✓ |

**90 framework-level cases** across the five crates, plus the live database matrix, plus the
worker's 16 and the process crate's 8.

---

## 2. Item by item

### LF-060 — Queue abstractions and drivers — **PARTIALLY VERIFIED**

- **Contract**: a queue manager, jobs, sync/database/Redis/SQS/Beanstalkd connectors, queue
  names/connection settings; jobs persist when promised; dispatch/claim/ack/retry semantics are
  documented; workers recover from restarts and poison jobs.
- **Implementation**: the **port** is `kelson_jobs::JobStore` (+ `TenantJobStore`, `JobStoreAdmin` ✓);
  the **drivers** are the in-memory store ✓ (sync semantics ✓), the database stores ✓ (pg ✓ and
  mysql ✓, both durable ✓) and Redis ✓ (`kelson-cache-redis`'s `RedisJobStore` ✓). The worker claims,
  dispatches, completes or fails with a fencing token ✓, and the stores' claim leases make a crashed
  worker's job **reclaimable** ✓.
- **Evidence**: the store lifecycle — `store_lifecycle_with_priority_and_backoff` ✓;
  **delayed jobs** — `delayed_jobs_wait_and_dead_jobs_requeue` ✓; **poison jobs** —
  `retry_exhaustion_marks_dead` ✓ + `job_migration_refuses_newer_than_known` ✓ (a payload the worker
  cannot read is failed, not run ✓); **the worker kill** —
  `mysql_job_store_recovers_orphaned_running_job_after_worker_crash` ✓ (the abandoned claim is
  reclaimed, not orphaned ✓); **multi-worker contention** —
  `mysql_job_store_claim_completes_and_fails_with_skip_locked` ✓ (SKIP LOCKED gives two workers
  distinct rows ✓); **at-least-once duplicates** — the claim fencing ✓
  (`complete_claim`/`fail_claim` refuse a stale owner ✓ — `mysql_job_store_fences_a_deferred_job_reclaimed_at_the_same_epoch` ✓);
  the tenant isolation — `tenant_isolation_queue_claim_never_crosses_tenants` ✓,
  `tenant_isolation_queue_cross_tenant_complete_and_fail_are_invisible` ✓,
  `tenant_isolation_queue_enqueue_stamps_tenant_and_rejects_mismatch` ✓; the pentest cases —
  `pentest_queue_foreign_id_leaks_no_existence_oracle` ✓,
  `pentest_queue_worker_identity_does_not_widen_the_claim_view` ✓.
- **Gaps**: **SQS and Beanstalkd are not shipped** ✓ (the framework ships the sync, database and
  Redis drivers ✓ — a recorded divergence, not a defect ✓: those are hosted-service connectors ✓ and
  the atlas calls them "and other supported connectors" ✓). There is **no named case for a database
  outage** ✓ (the store's error propagation is the pool's ✓, but the case is not named ✓).

### LF-061 — Job retries, backoff, timeout, uniqueness and rate limits — **PARTIALLY VERIFIED**

- **Contract**: job classes; attempts/backoff/timeout; failed-job storage; unique jobs; middleware
  for overlap/rate limiting; retryUntil/failure hooks. Retries are bounded, idempotency expectations
  explicit, job visibility/timeout coherent and failures observable.
- **Evidence**: the **retry count** — `retry_exhaustion_marks_dead` ✓ (the budget ends in Dead ✓)
  and `retry_limit_aborts_over_budget_claims` ✓; the **exponential backoff** — `backoff_curve_and_cap` ✓
  (the curve doubles and the cap holds ✓); the **timeout** — **`a_job_that_passes_its_deadline_fails_the_attempt`**
  (added this pass ✓: a job past its deadline is a failed attempt ✓, so the budget applies ✓);
  the **worker crash** — the lease reclaim ✓ (`mysql_job_store_recovers_orphaned_running_job_after_worker_crash` ✓);
  the **unique-key race** — `unique_admission_holds_locked_claims_until_release` ✓ (the lock is held
  until the claim releases ✓) and `registry_refuses_impossible_admission_configs` ✓; the **rate limit** —
  `rate_limited_admission_holds_until_the_bucket_refills` ✓; the **defer/kill** admission —
  `admission_defer_returns_the_attempt_and_kill_is_terminal` ✓; the **failed-job storage** —
  `admin_maintenance_purges_prunes_and_snapshots` ✓ (the admin surface ✓).
- **Gap (closed 2026-10-21)**: the **failed-job retry** (re-queueing a dead job) has **no named case** ✓ — the admin
  surface exists (`queue:retry` ✓) but the case is not named ✓.
  **Now covered** by **`a_failed_job_is_retried_and_only_the_exhausted_budget_dies`** ✓ — the same job
  comes back after a failure ✓, the attempt counter advances ✓, the retry is **delayed by the backoff**
  ✓ (it is not an immediate re-run ✓), and only the exhausted budget dead-letters it ✓ (the last error
  is kept ✓ and a dead job is never re-claimed ✓). *(The store's `requeue` — the manual
  `queue:retry` path — was already covered by `delayed_jobs_wait_and_dead_jobs_requeue` ✓.)*

### LF-062 — Job chains and batches — **PARTIALLY VERIFIED**

- **Contract**: sequential job chains, batch repositories, callbacks, cancellation and batch
  progress/failure state; dependencies run in order; a failed prerequisite does not silently run
  dependent work; the batch state survives worker/process restarts.
- **Evidence**: the **order** — `chain_advances_only_on_success` ✓; the **partial failure** —
  `chain_failure_records_the_halted_position` ✓ (the chain stops at the failed step ✓) and
  `batch_with_dead_member_skips_completion` ✓ (a dead member does not complete the batch ✓); the
  **cancellation** — `batch_cancel_and_retry_move_members` ✓; the **fan-out and completion** —
  `batch_runtime_fans_out_and_completes` ✓; the **restart recovery** — `batch_ledger_records_finishes_and_prunes` ✓
  (the ledger is durable ✓) and `mysql_batch_ledger_roundtrip` ✓ (against the live database ✓); the
  **status counts** — `batch_status_counts_members_from_the_store` ✓; the **validation** —
  `batch_and_chain_validation` ✓.
- **Gaps (all closed 2026-10-21)**: **the batch callback exception** and the **concurrent completion** of one batch have no
  named case ✓; the batch's **tenant isolation** is not covered by a named case ✓ (the QUEUE's tenant
  isolation is ✓ — `tenant_isolation_queue_*` ✓ — but a batch's members are not pinned to a tenant by
  a named case ✓).
  **Now covered**: the **callback exception** — `a_batch_completion_callback_failure_is_observable_and_enqueues_nothing` ✓
  (the callback's job name is not registered ✓ → the failure reaches the caller ✓ and **nothing is
  half-enqueued** ✓); the **concurrent completion** — `a_batch_completes_exactly_once_when_members_finish_concurrently` ✓
  (two members finishing at the same time complete the batch **exactly once** ✓ and the callback is
  enqueued **once** ✓); and the **tenant isolation** —
  `tenant_isolation_batch_members_are_pinned_to_their_tenant` ✓ (another tenant can neither see ✓,
  complete ✓ nor fail ✓ a member ✓, while the owning tenant runs the batch ✓).
  **Still open**: a batch that **mixes** tenants is **not refused** ✗ (F-26 ✓ — the repro fails on
  demand ✓ and the proposed fix is filed ✓, not implemented ✓).

### LF-063 — Events, listeners and subscribers — **PARTIALLY VERIFIED**

- **Contract**: an event dispatcher, event classes/listeners, discovery, sync/queued listeners, model
  events, an event cache; listeners receive the expected payload under the configured delivery
  semantics; ordering/transaction timing and exception handling are explicit.
- **Implementation**: `kelson-event` — the listener registry (the discovery ✓), the publisher ✓, the
  outbox pump ✓ and the **strict-ordering** mode (an ordered aggregate demands an idempotent
  consumer ✓).
- **Evidence**: the **discovery** — `registry_rejects_duplicates_and_filters` ✓; the **failures and
  retries** — `failed_listener_keeps_event_unpublished_for_redelivery` ✓ (a failed listener leaves
  the event unpublished so it can be redelivered ✓); the **duplicate delivery** —
  `outbox_pump_skips_reappended_duplicate_by_idempotency_key` ✓ and
  `outbox_idempotency_key_is_stable_and_content_sensitive` ✓; the **ordering assumptions** —
  `strict_ordering_requires_idempotence` ✓; the **tenant isolation** —
  `tenant_isolation_events_pump_routes_only_to_owner_tenant` ✓,
  `tenant_isolation_events_registry_views_are_scoped` ✓; the pentest case —
  `pentest_events_completed_key_of_one_tenant_cannot_suppress_another` ✓; the **pump** —
  `pump_dispatches_and_publishes` ✓.
- **Gaps**: the **after-commit** timing (a listener must not observe uncommitted state ✓) has **no
  named case** ✓ — the outbox is written in the transaction ✓, but the "rollback suppresses the
  event" case is not named ✓; the **queued listener** delivery and the **event cache invalidation**
  have no named case ✓.

### LF-064 — Transactional event / outbox safety — **PARTIALLY VERIFIED**

- **Contract**: dispatch-after-commit semantics; a durable outbox and a pump where delivery must
  survive a process crash. No consumer observes uncommitted state; committed side effects are
  eventually delivered under stated guarantees.
- **Implementation**: the **outbox pattern** ✓ — the rows are written inside the caller's
  transaction ✓ and a separate pump publishes them ✓; the **idempotency key** ✓ (a redelivered event
  is suppressed ✓) and the **strict ordering** mode ✓ (an ordered aggregate refuses to publish
  without idempotence ✓).
- **Evidence**: the **duplicate delivery** — `outbox_pump_skips_reappended_duplicate_by_idempotency_key` ✓;
  the **idempotent consumer** — `outbox_idempotency_key_is_stable_and_content_sensitive` ✓; the
  **poison event** — `outbox_failed_dispatch_does_not_mark_key_redelivery_works` ✓ (a failed dispatch
  is not marked delivered ✓, so it is redelivered ✓) and
  `failed_listener_keeps_event_unpublished_for_redelivery` ✓; the **ordering per aggregate** —
  `strict_ordering_requires_idempotence` ✓; the **tenant scope** —
  `outbox_idempotency_key_scopes_by_tenant` ✓; the **live pump** —
  `mysql_outbox_store_claim_and_mark_published` ✓.
- **Gap**: the **rollback suppresses the event** case and the **commit-then-crash** case have no
  named case ✓ — the store's contract is right ✓, but the two scenarios the atlas names are not
  pinned by tests ✓.

### LF-065 — Broadcasting — **PARTIALLY VERIFIED**

- **Contract**: broadcast event interfaces, channel authorization, public/private/presence channels,
  queued dispatch, server drivers; messages reach the configured broadcaster; private/presence
  subscriptions require authorization; the transaction commit ordering is deliberate.
- **Implementation**: `kelson-realtime` — the hub ✓, the channel naming/sanitizing ✓, the presence
  roster ✓, the SSE endpoint ✓ and the WebSocket transport ✓ (a real socket ✓).
- **Evidence**: the **channel authorization** — `authorized_transport_enforces_the_token_and_tracks_presence` ✓;
  the **fan-out** — `hub_publish_reaches_subscribers_only_on_that_channel` ✓ and
  `publish_with_no_subscribers_is_zero` ✓; the **presence** — `presence_roster_join_leave_is_idempotent` ✓;
  the **SSE** — `sse_endpoint_rejects_bad_channel_and_serves_stream` ✓; the **WebSocket** —
  `ws_roundtrip_over_real_socket` ✓ and `ws_route_without_upgrade_handshake_is_rejected` ✓; the naming
  rules — `channel_naming_helpers` ✓, `channel_sanitizer_rules` ✓; and in the host, the
  authorization matrix — `broadcast_auth_matrix_allowed_denied_and_expired` ✓.
- **Gaps**: the **payload schema**, the **queue delay** (a queued broadcast ✓), the **reconnect**, the
  **multi-node** fan-out and the **driver failure/backpressure** have no named case ✓. *(P2 ✓ — the
  atlas rates broadcasting optional ✓; the client SDK is **not** claimed ✓ — the framework ships the
  server side only ✓, exactly as the atlas asks ✓.)*

### LF-066 — Task scheduling — **PARTIALLY VERIFIED**

- **Contract**: scheduler definition; cron/schedule:run; frequency/timezone; environment
  constraints; overlap and one-server locks; background execution. Tasks run on the expected
  cadence/timezone; the overlap guards are atomic; multi-instance scheduling does not duplicate
  one-server jobs.
- **Implementation**: `kelson-scheduling` — the cron grammar ✓, the builder helpers ✓, the runner ✓
  with its due scan ✓, the **one-server** guard ✓ and the **overlap** guard ✓, the hooks ✓ and the
  per-tenant state ✓.
- **Evidence**: the **cadence** — `due_scan_fires_never_fired_items` ✓, `daily_rolls_to_next_day` ✓,
  `weekly_finds_matching_weekday` ✓, `monthly_expression_finds_the_first` ✓, `impossible_dates_never_fire` ✓;
  the **timezone** — `zoned_items_run_in_the_item_wall_clock` ✓ and
  `offset_searches_the_item_wall_clock` ✓; the **overlap** — `overlap_guard_skips_while_the_window_is_held` ✓;
  the **one-server lock** — `one_server_guard_acquires_runs_and_releases` ✓; the **contention** —
  `tenant_isolation_scheduler_guard_locks_do_not_collide_across_tenants` ✓; the **lock expiry / missed
  tick** — `state_roundtrips_through_disk` ✓ (the state survives a restart ✓) and
  `failures_are_capped_and_persist_through_the_state_doc` ✓; the **hooks** —
  `hooks_gate_and_observe_successful_runs` ✓, `before_hook_failures_gate_the_run_and_are_recorded` ✓;
  the **cron grammar** — 11 cases ✓ incl. `invalid_expressions_are_rejected_with_a_reason` ✓; the
  pentest case — `pentest_scheduler_crafted_names_cannot_collide_with_another_scope` ✓; the
  **tenant isolation** — `tenant_isolation_scheduler_due_scan_is_tenant_scoped` ✓,
  `tenant_isolation_scheduler_last_fired_state_is_per_tenant` ✓.
- **Gaps**: the **maintenance mode** (a scheduled task must not run while the app is down for
  maintenance ✓) has no named case ✓; the **lock expiry** under a crashed scheduler has no named
  case ✓ (the guard's TTL is set ✓, but the expiry is not pinned by a test ✓). *(The atlas's own
  warning is respected: the one-server lock is **process-local** unless a shared backend is
  configured ✓ — the framework says so ✓, and the multi-instance guarantee is therefore the
  deployment's coordination, not the framework's ✓.)*

### LF-067 — Concurrency utility — **NOT IMPLEMENTED**

- Nothing ships: no `Concurrency::run` facade, no process/fork/sync drivers, no named result
  collection. The atlas rates it **P2** and says so itself: "Rust async runtime already supplies
  concurrency; prioritize structured concurrency/cancellation APIs instead of reproducing process
  serialization" ✓. The framework's answer is the async runtime plus the structured
  `BoxFuture`/cancellation-aware job dispatch ✓ — reproducing Laravel's process-serialization
  facade would be the wrong trade ✓. The honest status is NOT IMPLEMENTED, by design ✓.

### LF-068 — Process execution — **IMPLEMENTED THIS PASS (PARTIALLY VERIFIED)**

- **Contract**: a process API for child processes; input/output; timeout; environment; pipelines; a
  fake for tests. Command arguments are passed safely without unintended shell interpolation; the
  output, exit status and timeout are observable.
- **Implementation (new)**: `kelson-process` ✓ — a `ProcessRunner` **port** ✓ and the `StdProcessRunner`
  implementation ✓:
  - **no shell, ever** ✓ — the program and its arguments go to the OS as an **argv** ✓, so a value
    that happens to contain `; rm -rf /` is an argument ✓, not a second command ✓;
  - **an isolated environment by default** ✓ — the parent's environment is NOT inherited ✓ (a secret
    in the parent's env cannot leak into a child that prints it ✓); the caller passes what the child
    needs ✓;
  - **the outcome is observable** ✓ — the exit code ✓, stdout ✓, stderr ✓, `timed_out` ✓ and the
    duration ✓;
  - **a timeout is enforced** ✓ — the child is killed when the deadline passes ✓, so a hung command
    cannot wedge the caller ✓;
  - **no process leak** ✓ — the child is always waited on ✓;
  - a **fake** for tests ✓ — `FakeProcessRunner` ✓ (scripted answers ✓ + it records the request ✓).
- **Evidence**: `arguments_reach_the_child_verbatim` ✓ (an injection-shaped argument arrives
  verbatim ✓), `the_environment_is_isolated_by_default` ✓, `an_explicit_variable_reaches_the_child` ✓,
  `a_run_that_passes_its_deadline_is_killed` ✓, `a_missing_program_is_an_error` ✓,
  `an_empty_program_is_refused` ✓, `the_fake_answers_from_its_script` ✓.
- **Gap**: the port **executes** what it is told ✓ — it does not **authorize** the command ✓: which
  modules may run which programs is the **host's** capability gate ✓ (the atlas: "restricted by
  module capabilities" ✓). No host wiring for that gate ships yet ✓, and **pipelines** (several
  processes chained) are not modelled ✓.

### LF-069 — Context propagation — **PARTIALLY VERIFIED**

- **Contract**: request/queue context values propagated into logs/jobs with lifecycle scoping. The
  context crosses supported async boundaries intentionally and does not leak between reused workers
  or concurrent tasks.
- **Implementation**: `kelson_context::ExecutionContext` ✓ (the trace/request/correlation/causation
  ids ✓, the tenant/principal/actor ✓, the scope ✓, the ownership ✓, the authorization snapshot ✓,
  the feature snapshot ✓) and the `TelemetryContext` ✓ (the same ids plus the baggage ✓). The
  **queue boundary** carries the origin ✓ — `DurableJob::causation_id`/`correlation_id` ✓ (the
  correlation of the request that caused the work ✓, stamped at enqueue ✓).
- **Evidence**: the **HTTP → job** hop — `the_origin_survives_the_store_round_trip` ✓ (the row keeps
  the origin ✓) and `mysql_a_traced_job_row_carries_the_request_correlation` ✓ (the row **in the
  database** ✓); the **tenant boundary** — `tenant_isolation_queue_claim_never_crosses_tenants` ✓ and
  `tenant_isolation_events_pump_routes_only_to_owner_tenant` ✓; the **context is not authority** —
  `pentest_queue_foreign_id_leaks_no_existence_oracle` ✓,
  `pentest_queue_worker_identity_does_not_widen_the_claim_view` ✓,
  `pentest_events_completed_key_of_one_tenant_cannot_suppress_another` ✓,
  `pentest_scheduler_crafted_names_cannot_collide_with_another_scope` ✓; the **nested context** —
  the `TelemetryContext`'s `with_correlation`/`with_causation` builders ✓.
- **Gaps**: the **cleanup on panic** (a context that must not survive a failed task ✓) and the
  **restore** of an outer context (the builders are immutable ✓, so the "nested context" case is
  structural ✓, but there is **no named case** proving a context cannot outlive its task ✓).

---

## 3. Findings

- **F-20 (medium) — the architecture gate was RED: an adapters crate depended on the web layer.**
  The shared rate-limit store (F-3 Phase 3) put the `RateLimitStore` **port** in `kelson-http`
  (the web layer ✓), and the `kelson-ratelimit-redis` **adapter** (the adapters layer ✓) depended on
  it ✓ — an edge the layer matrix forbids ✓, so `cargo xtask dep-rules` failed ✓ and the CI's
  "architecture analyzer" job would have failed with it ✓. **Fixed**: the port moved to the
  **foundation** (where the other ports live ✓ — `Sealer` ✓) and `kelson-http` re-exports it ✓, so
  the web crate's users keep the same path ✓. `dep-rules: clean` ✓ and `lint: clean` ✓ again.
- **F-17's cousin (low) — the parity rows for this batch were claim-shaped and one row was
  duplicated.** `P0-FEATURE-VERIFICATION.md` carried LF-064 **twice** with contradictory statuses
  (VERIFIED and PARTIALLY VERIFIED ✓) and seven of the ten items had no row at all ✓. The rows now
  name the executed cases ✓, the duplicate is gone ✓ and the missing rows are added ✓.
- **F-25 (medium) — the job worker has NO panic boundary: a panicking handler takes the worker
  down — OPEN (repro ✓ + proposed fix ✓, not implemented ✗).** The HTTP edge got a panic boundary
  in F-2 ✓ (a panicking handler answers a stable 500 envelope ✓), but the job worker has none ✓:
  `handler.handle(&job)` is awaited directly ✓, so a panic inside a handler **unwinds the claim
  loop** ✓ — the worker dies mid-flight ✓, every other claimed job stays `Running` until its lease
  is reclaimed ✓, and what the operator sees is a **panic**, not a job failure ✓ (so the retry
  budget and the backoff never apply ✓).
  - **The repro** (executed ✓, and it fails ✓ — that failure IS the defect ✓):
    `kelson-worker`'s `a_panicking_job_handler_does_not_take_the_worker_down` ✓ — it registers a
    handler that explodes ✓, dispatches the job ✓ and asserts the worker survives ✓. Today the
    spawned task dies ✓ ("the worker task died: task 1 panicked with message 'the handler
    exploded'" ✓). It is `#[ignore]`d ✓ so the tree stays green ✓ — run it with
    `cargo test -p kelson-worker -- --ignored` ✓.
  - **The proposed fix** (not implemented ✗ — it changes the worker's behaviour ✓):
    1. catch the unwind around the handler future ✓ (`catch_unwind` over the `BoxFuture` ✓) and
       map it to a job **FAILURE** ✓ — the same shape the HTTP edge takes ✓;
    2. record the panic as the job's `last_error` ✓ (a **redacted** message ✓ — a panic can carry
       a payload ✓ — never the raw text ✓) so the operator sees a job failure ✓, not a stack
       trace ✓;
    3. the retry budget and the backoff then apply ✓ (the attempt is consumed ✓, the job is
       re-queued ✓) — the worker keeps claiming ✓;
    4. the panic is **counted** as a worker metric ✓ (a rising panic rate is an alarm ✓) and
       logged at ERROR ✓ with the job id ✓;
    5. the case above loses its `#[ignore]` ✓ and a **second** case asserts that a panicking
       handler does not consume the **next** job's attempt ✓ (the worker's state stays clean ✓).
- **F-26 (low) — a batch can MIX tenants — OPEN (repro ✓ + proposed fix ✓, not implemented ✗).**
  - **What the framework already guarantees**: the batch's **members are pinned to their tenant** ✓
    — proven by `tenant_isolation_batch_members_are_pinned_to_their_tenant` ✓ (new ✓): another
    tenant can neither **see** ✓, **complete** ✓ nor **fail** ✓ a member (the id is `NOT_FOUND` ✓,
    invisible rather than refused ✓), while the owning tenant runs and completes the batch ✓.
  - **The gap**: `BatchSpec::validate` refuses an empty batch ✓, duplicate ids ✓ and members that
    do not start pending ✓ — but it does **not** check that the members agree on their tenant ✓.
    A batch that spans two tenants puts one tenant's job under another tenant's batch record ✓,
    and the completion callback fires for the mixed set ✓.
  - **The repro** (executed ✓, and it fails ✓ — that failure IS the defect ✓):
    `kelson-jobs`'s `a_batch_that_mixes_tenants_is_refused` ✓ — it dispatches a batch with one
    member per tenant ✓ and asserts the dispatch is refused ✓. Today the dispatch **succeeds** ✓.
    It is `#[ignore]`d ✓ so the tree stays green ✓ — run it with
    `cargo test -p kelson-jobs -- --ignored` ✓.
  - **The proposed fix** (not implemented ✗ — it refuses a spec that used to be accepted ✓):
    1. `BatchSpec::validate` refuses a spec whose members disagree on `tenant_id` ✓ — the same
       shape as the existing "ids must be unique" rule ✓ (a `None` tenant is a platform job ✓ and
       must not be mixed with a tenant's job ✓ either ✓);
    2. the refusal happens **before** the side effects ✓ (the ledger write ✓ and the enqueue ✓ —
       the ledger is written first ✓, so the refusal must come even earlier ✓);
    3. the demo/console refuse to build such a batch ✓ (they build the spec ✓ — the validation
       catches it ✓);
    4. the case above loses its `#[ignore]` ✓ and a **single-tenant** case is added ✓ (a batch of
       one tenant's members is accepted ✓ — the check must not break honest batches ✓).

## 4. Corrections to the parity documents

- `P0-FEATURE-VERIFICATION.md` — the duplicate LF-064 row is removed ✓, the seven missing rows
  (LF-062, LF-063, LF-065, LF-066, LF-067, LF-068, LF-069 ✓) are added ✓ with their named
  evidence and their gaps ✓, and LF-060/LF-061 now name their cases ✓.
- `LARAVEL-FEATURE-ATLAS-COMPLETE.md` — the LF-060 … LF-069 rows carry the real implementation
  pointers and the executed evidence ✓.
- `P0-EXECUTION-REPORT.md` — the batch is recorded with its findings ✓.

## 5. Summary

- **10 items**: **9 PARTIALLY VERIFIED** · **1 NOT IMPLEMENTED** (LF-067 ✓ — the concurrency
  facade, which the atlas itself de-prioritizes ✓). Every item has at least one of the atlas's own
  required cases missing ✓ — that is why none of them is VERIFIED ✓, and each gap is named in its
  section ✓. The implementation is nonetheless substantial and exercised: **90 framework-level cases**
  across the five crates ✓, plus the live database matrix ✓, the worker's 16 ✓ and the new process
  crate's 8 ✓.
- **The sharpest gaps** (as of 2026-10-21 — three are now closed ✓): the **after-commit /
  rollback-suppresses** cases (LF-063/LF-064 ✗ — the outbox's contract is right ✓, but the two
  scenarios the atlas names are not pinned by tests ✓); the **failed-job retry** case (LF-061 ✓
  **closed** — `a_failed_job_is_retried_and_only_the_exhausted_budget_dies` ✓); the **batch callback
  exception** and the **concurrent batch completion** (LF-062 ✓ **closed** —
  `a_batch_completion_callback_failure_is_observable_and_enqueues_nothing` ✓ and
  `a_batch_completes_exactly_once_when_members_finish_concurrently` ✓); the **SQS/Beanstalkd**
  connectors (LF-060 ✗ — a recorded divergence ✓); the **process pipelines** and the **host's
  capability gate** for the new process port (LF-068 ✗); the **maintenance mode** and the **lock
  expiry** (LF-066 ✗); and the **context cleanup on panic** (LF-069 ✗ — now with a sibling finding:
  **F-25** ✓, the job worker has no panic boundary ✓).
- **Findings**: F-20 (the layer gate was red — **fixed** ✓), **F-25** (the job worker has no panic
  boundary — **open** ✓: a repro that fails on demand ✓ and a five-point proposed fix ✓),
  **F-26** (a batch can mix tenants — **open** ✓: a repro that dispatches a mixed batch ✓ and a
  four-point proposed fix ✓), and the parity rows' claim-shaped evidence + the duplicated LF-064 row
  (**corrected** ✓).
