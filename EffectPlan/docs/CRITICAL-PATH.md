# Critical path intelligence (Phase 3)

What the scheduling layer is, how it was built, what it cost, and what was
decided along the way. The maths is in
[SCHEDULING-MODEL.md](SCHEDULING-MODEL.md), the byte contract in
[ABI.md](ABI.md).

Status: complete and measured. As at Phase 3: Rust 91 tests, TypeScript 156
tests, 93 ABI checks, browser smoke test, both benchmark layers, all gates
passed on the published artifact. Those three counts have grown since — Phase 5
stands at Rust 153, TypeScript 204 and 135 ABI checks (`docs/IMPLEMENTATION.md`),
and the numbers in this document's own tables are the current ones, not the
Phase 3 ones.

## 1. What Phase 3 added

EffectPlan could already say which calls must run before which, and group them
into waves. It could not say how long the plan would take or which calls decide
that. Phase 3 makes it duration-aware:

```ts
const plan = await planner.planBatch(calls, {
  timing: { durations: [2, 5, 2, 3, 4], unit: "ms" },
});

plan.timing!.criticalPathDuration;  // 14
plan.timing!.totalWork;             // 16
plan.timing!.averageParallelism;    // 8 / 7
plan.timing!.peakConcurrency;       // 2
plan.timing!.events[2];             // { earliestStart: 2, slack: 3, critical: false, … }
plan.timing!.criticalEvents;        // [0, 1, 3, 4]
```

Nothing about Phase 1 or Phase 2 changed shape: `planBatch(calls)` still returns
the same frozen plan with `timing: undefined`, `effectplan_plan` and
`effectplan_plan_v2` still exist with the same signatures and the same bytes.

## 2. The prerequisite, proved before the ABI was touched

The specification required the call-index ↔ event-index mapping to be *proved*,
not assumed, before durations were encoded. The proof, re-run for Phase 3:

* Nodes are **call indices**. `emit_edge(pred, succ, …)` is called with `op`
  values — positions in the batch — never with event indices. Event indices exist
  only inside the workspace, as the pending-reader list.
* Every edge points forward: `from < to`. Writers and readers are visited in
  ascending call order, and every edge is emitted from an already-visited call to
  the call currently being visited.
* Records are grouped by successor, ascending. The planner emits all edges into
  call `k` contiguously, in the order it visits calls.

Evidence, not assertion:

| Claim | Where it is checked |
| --- | --- |
| `from < to` for every edge | engine `validate_output`; host `decodePlan`; `verify:wasm` sweeps a batch and asserts it; the Phase 2 exhaustive parity test over 7,381 batches |
| grouped by successor, ascending | host `decodePlan` rejects a record whose `to` is smaller than the previous one |
| the same call index means the same thing in both passes | `plan.timing.events[i]` is checked against `plan.execution.levelByCall[i]` in a test, and against the independent oracle's node `i` |

Consequence for memory: the durations and timing arrays are sized by `maxCalls`
(128), not by `maxEvents` (1,152). Had the graph been over events, the same
feature would have cost `1,152 × 4` plus `1,152 × 8` bytes instead of 512 plus
1,056 — and the levels buffer would have been 2,304 bytes instead of 260.

## 3. The passes

`engine/src/schedule.rs`, one function per job the specification named:

| Specification | Code |
| --- | --- |
| `validate_durations` | `validate_durations` — every call has a positive tick count |
| `compute_earliest_times` | forward pass inside `compute_schedule` |
| `compute_project_duration` | makespan fold at the end of the forward pass |
| `compute_latest_times` | backward pass inside `compute_schedule` |
| `compute_event_slack` | slack fold over the calls |
| `identify_critical_events` | the same fold counts zero-slack calls |
| `compute_peak_concurrency` | `compute_peak_concurrency` — endpoint sort and sweep |
| `validate_schedule_output` | the precedence audit carried by the backward pass |
| `write_schedule_output` | the tail of `compute_schedule`, header last |

Both graph passes walk the *same* record array the planner produced, once
ascending and once descending. Because records are grouped by successor ascending,
a node's successors all sit later in the array, so on the way down a successor's
`latest_finish` is already final when the edge that uses it is read. No reverse
adjacency, no indegree array, no queue, and no pass ever scans all edges for one
vertex.

Peak concurrency sorts `2 × call_count` packed `u64` endpoints — time in the high
bits, "is a start" in the low bit — with an in-place heap sort, then sweeps a
counter. Packing the kind into the low bit makes a finish at tick `t` sort before
a start at tick `t`, which is the half-open rule from the model, and the total
order means the answer cannot depend on input order.

Heap sort rather than a comparison sort with allocation: no Rust heap, no
`Vec`, deterministic.

## 4. Complexity

Per `v3` call: one dependency pass `O(A log A + E)`, one level pass `O(E + V)`,
then `O(V + E)` for the two CPM passes, `O(V + E)` folded audit, and `O(V log V)`
for the endpoint sort. Memory: two fixed 512-byte buffers of `u32` for `earliest`
and `latest_finish`, one for slack, one for durations, plus a 2 KB endpoint
window — all compile-sized, none of it `malloc`.

`V ≤ 128`, so the `V log V` term is bounded and the two graph passes dominate.

## 5. The audit, and why it moved

The first Phase 3 implementation ran a *third* full pass over the records to audit
the schedule, then measured it honestly and failed its own gate: normal workload
+26.6% engine and +37.1% API over Phase 2, against a 20% target. The isolated
native measurement said why: 13.3 µs for the normal shape and 56.5 µs for medium,
about 14 ns per record per pass, because each record read was four separately
bounds-checked `rd_u32` calls.

Two changes, both of which keep the guarantee and drop the cost:

* **One bounds check per record instead of four.** The passes now walk
  `chunks_exact(EDGE_BYTES)`, which yields rows of a statically known length, so
  the per-byte checks disappear. The iterator is double-ended, which is what makes
  the descending pass free of index arithmetic.
* **The audit rides along with the backward pass rather than re-reading the
  graph.** When a record is visited descending, `earliest` is final and the
  successor's `latest_finish` is final, so the precedence check has exactly the
  same information it had in the separate pass — a `pred` that finishes after its
  `succ` starts still fails with `122` — at the cost of one comparison instead of
  a whole extra traversal.

The second half of the old audit, `latest_finish[pred] ≤ latest_finish[succ] −
d(succ)`, was dropped from the engine after analysis: `latest_finish[pred]` is
defined as a minimum over exactly those terms, so the inequality holds by
construction and re-checking it proved nothing. It is still checked **on the host**
in `decodeTiming`, on final values, where it is not a tautology but an independent
look at what the engine wrote — and the host can name the failing edge in a
thrown error instead of returning a bare status.

The result, measured after the change: normal **+14.0%**, medium **+8.4%** at the
engine, both inside the gate; the isolated native scheduling pass fell from 13.3 µs
to 1.9 µs (normal) and 56.5 µs to 6.0 µs (medium). This is recorded as a **cost
that was paid for by an implementation choice, not by the algorithm** — the
specification's gate is what caught it.

Provenance, because the two runs are not in the same file: the pre-change numbers
(+26.6% / +37.1%, 13.3 µs / 56.5 µs) came from development runs whose artifacts
were overwritten, and are quoted here as the measurement that triggered the work.
`artifacts/native-bench.json` and `artifacts/benchmarks.json` hold the committed,
post-change run, and every other number in this document comes from those two
files.

## 6. Memory, decided with a gate rather than a hope

The specification asked for an explicit decision about growing from 17 to 18
pages, and made the exact layout a build-time acceptance gate.

Decision at Phase 3: **stay at 17 pages.** `durations` is 512 bytes (`128 × u32`) and
`timing` is 1,056 bytes (32-byte header + `128 × (u32 ES, u32 slack)`), 1,568
bytes more than Phase 2. The arena was 61,888 bytes as the layout arithmetic of
Phase 3 produced it; Phase 4's checkpoint record added the 32 bytes that make it
61,920, and linear memory 1,114,112 bytes = 17 pages with 2,264 bytes to spare.
Phase 5's capacity regions needed 13,416 of them, so that page was taken: the arena
is 75,336 bytes and linear memory 1,179,648 = 18 pages (`docs/RESOURCE-ABI.md` §1
is the measurement the expansion was approved on). The Phase 3 decision still holds
in the form that mattered — the scheduling feature cost one region pair and no
page — it was the *next* feature that changed the arithmetic.

What is *not* stored, and why:

| Obvious array | Bytes it would cost | Why it is absent |
| --- | --- | --- |
| `latest_start[]` | 512 | `ES + slack`, derived by the host |
| `earliest_finish[]`, `latest_finish[]` | 1,024 | `ES + d`, `LS + d`, same reason |
| per-event timing | ~9,216 | the graph is over calls, not events |
| a second copy of the durations | 512 | the region the host wrote *is* the input; the engine reads it in place |
| reverse adjacency | ~9,216 | the descending record walk replaces it |

`verify:wasm` asserts each region's base, length, contiguity and published
capacity, that the regions tile the arena exactly, that the u64 total-work field
sits at offset 16, and that linear memory is the page count stated in
`scripts/limits.mjs`. A layout change fails
the build. `const _: () = assert!(…)` lines in `engine/src/lib.rs` say the same
thing at compile time, before anything runs.

## 7. The ABI

`effectplan_plan_v3`, 18 parameters: the 14 of `v2` plus `durations_ptr`,
`durations_len`, `timing_ptr`, `timing_len`. Selectors 25–31 publish the timing
revision and both new regions. Details, including the pointer rules and the
commit-record ordering, are in [ABI.md](ABI.md).

`v3` calls the `v2` body and then the scheduling pass. It does not duplicate the
planner, so a graph produced by `v3` is the graph `v2` would have produced by
construction, and a test says so: `scheduling_a_stale_graph_is_impossible`.

## 8. The TypeScript surface

* `PlanOptions.timing?: { durations: ArrayLike<number>; unit?: string }`.
* `ExecutionPlan.timing: TimingAnalysis | undefined` — `undefined` when no
  durations were supplied, never an empty object pretending to be a schedule.
* `TimingAnalysis`: `unit`, `totalWork`, `criticalPathDuration`,
  `averageParallelism` (`number | null`), `peakConcurrency`, `events`,
  `criticalEvents`. Frozen, like everything else the package returns.
* `EventTiming`: `callIndex`, `duration`, `earliestStart`, `earliestFinish`,
  `latestStart`, `latestFinish`, `slack`, `critical`.

The wrapper does **not** compute anything. `decodeTiming` reads the region, checks
it (revision, count, reserved word, `u64` range, `peak ≤ callCount`, non-negative
slack, `latestStart ≤ makespan`, both edge precedence directions, makespan ≥ every
finish, `totalWork = Σdurations`, zeros for an empty batch) and derives the four
names the model defines from the two the engine stored. Every check is against a
value the engine published or a value the caller supplied; none of them re-derives
a dependency or a longest path.

One Phase 3 implementation note: `decodeTiming` takes the engine's edge buffer as
the `Int32Array` it already is, not as decoded edge objects. Handing it objects
made the host allocate and freeze one record per dependency purely to audit them —
288 allocations on the normal shape — and that was a visible slice of the API
overhead. The audit reads four words per edge and allocates nothing.

Compatibility rules, all verified by the `api-compatibility` job (73 tests):

1. No timing option → the `v2` entry point is called and `timing === undefined`.
2. `version`, `callCount`, `edgeCount`, `statistics`, `execution` unchanged.
3. Durations are validated before anything is written; a bad array cannot leave a
   half-written region behind.
4. A refused schedule leaves no readable header, so no stale timing can be
   mistaken for a new answer.
5. Growing the linear memory between calls re-maps the host's views; a timing
   decode after growth is tested.
6. The package still has zero runtime dependencies and no network, filesystem or
   environment access.
7. An engine without `v3` is refused at load with a clear message rather than
   falling back silently.

## 9. Determinism

The same input produces the same bytes: the two CPM passes have no iteration-order
dependence (max and min folds are order-independent, and the forward/backward
walks visit a fixed array), the endpoint sort is a total order on `(tick, kind)`,
and floating point appears nowhere in the engine.

`averageParallelism` is the only `number` produced by division, and it is computed
in the host from two integers — `totalWork / makespan`. A caller that needs
byte-identical output should compare the two integers rather than the quotient.

Tests: `the_same_input_produces_the_same_bytes` (Rust, byte-for-byte over the
timing region), `keeps planning and scheduling identical across repeated batches`
(wasm ABI), `timing is deterministic and independent of the batch before it`
(property), and the browser page's `timingDeterministic` check.

## 10. Barriers, exclusive and unknown effects

A barrier call has no accesses, gets every earlier call as a predecessor, and its
duration is timed like any other. `unknown` behaves as a barrier write. `exclusive`
orders against everything adjacent. None of this needed a change in the scheduling
pass: it reads the graph the dependency pass emitted and knows nothing about why
an edge exists. The `barrier` workload in the benchmark (43 waves, makespan 445
ticks, peak 2) exercises it.

## 11. Errors

`118` a duration is zero or missing. `119` the durations or timing region is not
the exact canonical size. `121` the ticks would overflow `u32`. `122` the schedule
contradicts its own graph. `108` a scheduling pointer is not canonical. The host
additionally refuses durations that are not positive safe integers, arrays whose
length is not the call count, and a `unit` that is not 1–32 printable characters —
before the engine is called, so an impossible batch cannot reach the arena.

No message contains a caller identifier. The engine returns numbers.

## 12. Testing

| Layer | Count | What it covers |
| --- | --- | --- |
| Rust `schedule_tests` | 20 | empty, single, chain, independent, the worked diamond, permuted durations, several critical paths, disconnected components, half-open adjacency vs overlap, maximum batch, large exact durations, overflow, zero and missing duration, refused writes no header, determinism, non-canonical pointers, wrong lengths, `v1`/`v2` never touch the region, stale graph |
| Rust total | 153 | the above plus Phase 1, Phase 2 and the Phase 5 resource suites |
| `timing.test.ts` | 20 | host validation, both readings of the specification's example, levels-vs-timing consistency, oracle agreement, overflow, half-open boundary, frozen and independent results, memory growth, no timing when not asked |
| `property.test.ts` | 22 (7 CPM) | edge precedence in both passes, roots at zero, makespan is the last finish, non-negative slack and the derived fields, the concurrency envelope, determinism, and per-call agreement with the independent oracle |
| `parity.test.ts` | 4 | 7,381 exhaustive batches, comparing every timing number against `benchmarks/reference-cpm.ts` |
| `wasm.test.ts` | 42 (8 scheduling) | raw `v3` through the real ABI: layout publication, lower entry points, the diamond, `118`, `121`, non-canonical regions, 20,000 hostile invocations |
| `verify-wasm.mjs` | 135 | structural, functional and refusal checks against the shipped `.wasm` bytes |
| `benchmarks/reference-cpm.ts` | — | an *independent* implementation: explicit adjacency, Kahn order, reverse-topological backward pass, counted-overlap peak |
| browser | 10 checks | Phase 1 edges, Phase 2 waves, Phase 3 timing in headless Chrome |

The oracle is deliberately a different algorithm, so agreement is evidence about
the mathematics rather than a restatement of the same code.

## 13. Benchmarks

Measured 2026-11-01T09:29:01.806Z, Node v20.15.0, Intel(R) Core(TM) i5-10300H CPU @ 2.50GHz, 20 cold runs, 200 warm-up iterations, per-dataset iteration counts as in `benchmarks/workload.ts`. Native figures come from `cargo bench` on the same `plan_arena*` entry points; they are read from `artifacts/native-bench.json`, never estimated. The tables below are the run that is committed in those two files.

Engine, WebAssembly, p50 microseconds:

| workload | calls | edges | waves | makespan | peak | `plan` | `+levels` | `+timing` | timing pass alone (native) |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| tiny | 8 | 36 | 8 | 66 | 1 | 13.3 | 14.3 | 15.8 | 0.3 |
| small | 32 | 144 | 8 | 108 | 4 | 37.1 | 42.2 | 46.6 | 0.9 |
| normal | 64 | 288 | 8 | 78 | 8 | 67.2 | 77.0 | 87.8 | 1.9 |
| medium | 128 | 1259 | 16 | 149 | 8 | 343.7 | 385.5 | 417.7 | 6.0 |
| wide | 64 | 0 | 1 | 17 | 64 | 14.3 | 15.1 | 20.2 | 0.9 |
| deep | 64 | 94 | 64 | 568 | 1 | 19.3 | 23.0 | 50.5 | 1.5 |
| diamond | 64 | 56 | 8 | 76 | 36 | 19.8 | 22.2 | 29.6 | 1.2 |
| barrier | 64 | 198 | 43 | 445 | 2 | 49.2 | 56.2 | 65.6 | 1.7 |
| conflict | 64 | 334 | 64 | 568 | 1 | 77.5 | 89.3 | 99.0 | 2.1 |
| maxbound | 128 | 1152 | 8 | 82 | 16 | 311.8 | 344.2 | 379.9 | 5.7 |
| mixed | 96 | 333 | 49 | 550 | 4 | 94.8 | 106.3 | 121.5 | 2.8 |

Published API, p50 microseconds, and the overhead against the Phase 2 call in the same process:

| workload | no timing (Phase 2 path) | with timing (Phase 3) | overhead | timed vs recorded | untouched vs recorded |
| --- | --- | --- | --- | --- | --- |
| tiny | 25.7 | 28.8 | +12.1% | 19.5% | 6.6% |
| small | 79.0 | 89.2 | +12.9% | 17.5% | 4.1% |
| **normal** | 148.2 | 168.1 | **+13.4%** | 15.4% | 1.7% |
| **medium** | 641.7 | 696.7 | **+8.6%** | 6.7% | -1.7% |
| wide | 62.7 | 72.9 | +16.3% | 19.7% | 3.0% |
| deep | 48.5 | 64.5 | +33.0% | 31.6% | -1.0% |
| diamond | 65.7 | 78.4 | +19.3% | 19.3% | 0.0% |
| barrier | 96.5 | 111.9 | +16.0% | 15.4% | -0.5% |
| conflict | 166.7 | 182.1 | +9.2% | 14.0% | 4.4% |
| maxbound | 627.2 | 674.9 | +7.6% | 7.6% | -0.0% |
| mixed | 188.3 | 215.5 | +14.4% | 14.1% | -0.3% |

The recorded Phase 2 column is the `npmApiE2eMicros.p50` of the same workload in `artifacts/benchmarks-phase2.json`, which is Phase 2's committed report; the last column is the same comparison for the path this phase did not touch, so the two columns together show how much of the movement is the library and how much is the machine.

The 20% target is stated for the normal and medium workloads, and those are the two the harness gates: **+13.4%** and **+8.6%** at the API, **+14.0%** and **+8.4%** at the engine, all four passing. Other shapes report their ratio without a pass/fail, because applying a target to a workload the specification did not name would be measuring a different claim. `deep`, `wide` and `diamond` pay most relatively — 64 calls with few or no dependencies, so the per-call rows dominate the two edge passes.

Paired in the same process, the untouched Phase 2 path moved between −1.7% and +6.6% against its own recorded numbers, which is this machine's noise floor for a cross-process comparison. The recorded comparisons are therefore reported, never gated: an early Phase 3 run put the tiny shape at +87.6% along a code path that had not been touched at all, which was CPU contention from a parallel `cargo` build, not the library. Same-process ratios are the evidence; cross-process ratios are context.

Two numbers to keep honest about run-to-run variation, from three runs of identical code on this laptop:

* `deep` measured `v2` 23.0 µs and `v3` 50.5 µs here (+119.6% at the engine, +33.0% at the API) and 22.9 µs / 30.8 µs in the previous run (+34.5%). The native isolated scheduling pass for that shape is 1.5 µs in both. The difference is V8 tiering between adjacent measurement blocks, not work the engine did.
* `mixed` measured `v2` 165.5 µs and `v3` 129.1 µs in one earlier run — `v3` appearing 22% *faster* than `v2`, which is impossible and therefore noise.

The gated pair is measured the same way in every run and the harness fails closed on a breach, so a shape whose blocks happen to tier badly cannot hide behind the average.

Sizes, measured from the artifact the packager writes *at Phase 3*: wasm 10,222 bytes against 40,000; unpacked package 48,766 bytes against 50,000; 18 files; 17 pages of linear memory. All four are superseded — see `docs/SIZE-REPORT.md` for the current artifact. Cold `EffectPlan.create()` in Node, p50 401.6–618.4 µs across the shapes. The specification's report list asks for memory consumption: it is 1,114,112 bytes of linear memory (constant, never grown by planning) plus the host's per-plan rows — `128 × 8` bytes of timing region and one frozen object per call in `plan.timing.events`. The RSS and heap deltas in `artifacts/benchmarks.json` swing around zero, which is what a fixed arena should look like; the per-batch host allocation is the visible cost of the API, not of the engine.

## 14. Limitations

* Durations are the caller's claims. The engine verifies their type, range and
  that they are non-zero; it cannot know whether 5 ms is realistic.
* **CPM ignores resource contention.** The makespan assumes unlimited capacity, so
  a plan where 60 calls all read the same 128-byte file will be optimistic. Two
  calls that conflict are already ordered by the dependency pass, but the schedule
  does not model a capacity-limited machine pool. That is Phase 5's job, and the
  number that belongs to it is `peakConcurrency` — the observed 64 on `wide` is
  precisely the pressure a capacity model would have to resolve.
* `maxCalls = 128` and `u32` ticks are the current ceilings. A 4,294,967,295-tick
  chain is refused (`121`), not truncated.
* Timing is a property of the whole batch; there is no per-wave or per-subgraph
  makespan. `plan.execution.waves` and `plan.timing` describe the same graph and
  a test checks they are consistent, but a per-wave duration is arithmetic the
  caller does with both.
* No calendars, no lags, no preemption, no stochastic durations
  (SCHEDULING-MODEL.md §8).
* Nothing here is a scheduler: EffectPlan still does not start, stop, order at
  runtime, or own an execution queue.

## 15. Deviations from the specification, with reasons

| Spec said | Built | Why |
| --- | --- | --- |
| `event_levels: [u16; MAX_EVENTS]` (Phase 2) and per-event timing arrays | per-**call** arrays, 260 and 1,056 bytes | the graph's nodes are calls; proved in §2. Per-event buffers would be 9× the size to describe 1,152 things that are not nodes |
| a separate `validate_schedule_output` pass | the precedence audit is carried by the backward pass | a third traversal of the records cost 33% of the scheduling pass for a check with the same information; measured in §5, gate-driven |
| `latest_finish[pred] ≤ latest_finish[succ] − d` checked in the engine | checked on the host instead | in the engine it is a tautology over a min-fold; on the host it is an independent check that can name the edge |
| "never scan every edge for every vertex" | honoured | neither pass does; each record is read exactly twice, once per direction |
| durations may be stored as `u32` per call | `u32` region written by the host and read in place | no copy, no second buffer |
| 17 → 18 pages acceptable | 17 pages at Phase 3, 18 from Phase 5 | §6; the layout is a build-time gate either way, and the page was taken on a measurement rather than a guess |
| the specification's example has makespan 10 | both readings implemented as tests | the prose fixes the durations and the critical path but not `E`'s edges; SCHEDULING-MODEL.md §7 |

## 16. What is next

Phase 3 is the last capability the specification allows itself to build now: the
dependency graph, the levels and the critical path are all verified against
independent implementations. Phase 4 (runtime scheduling) and Phase 5 (capacity
and contention) are explicitly not authorised while any part of this remains
unverified — and one part does: CI on a real GitHub runner has never executed,
because the branch has not been pushed. Everything else in the acceptance list has
evidence in this repository.
