# Architecture

EffectPlan answers three questions: given a batch of tool calls with declared
resource effects, which calls must be ordered against each other, which may run
at the same time, and — if the caller supplies durations — how long the plan will
take and which calls decide that. It returns a predecessor → successor graph, the
execution levels that graph implies, and a critical-path analysis. **It never
executes a tool.** The host owns execution; EffectPlan owns the ordering decision
and the timing estimate.

## Layers

```
  host application
        │  ToolCall[]  +  optional durations
        ▼
  @effectplan/core  (TypeScript)
        │  validate · intern · normalise · pack · encode durations
        │  effectplan_plan_v3(18 i32) → i32 status
        ▼
  effectplan-engine  (Rust, wasm32-unknown-unknown, no_std)
        │  plan_arena(): validate → plan → validate output
        │  levels:   one forward pass over the emitted edges
        │  schedule: forward and backward CPM passes over the same edges
        ▼
  linear memory: one statically reserved arena, zero allocation
```

The engine hands back three artefacts: the dependency graph, one `u16` execution
level per call, and — when durations were supplied — two `u32` per call plus the
makespan, work, critical count and peak concurrency. The TypeScript layer groups
levels into waves and derives the four names of the CPM model from those two
numbers; that is presentation and arithmetic, not analysis.

The TypeScript layer never re-implements the conflict algorithm or the scheduling
algorithm. It validates the public contract, turns resource identities into
request-local integers, packs them into the arrays the engine reads, and
re-validates everything that comes back. Every conflict and every timing decision
is made in Rust.

## The raw ABI

Three planning entry points share one contract. `effectplan_plan` (Phase 1) takes
twelve `i32` arguments; `effectplan_plan_v2` (Phase 2) adds the levels region;
`effectplan_plan_v3` (Phase 3) adds the durations and timing regions. All three
return one `i32` status:

```
effectplan_plan(
  abi_version, call_count, access_count, resource_count,
  call_modes_ptr, access_offsets_ptr, resource_ids_ptr, access_modes_ptr,
  workspace_ptr, workspace_len, output_ptr, output_capacity
) -> i32

effectplan_plan_v2(same twelve arguments, levels_ptr, levels_len) -> i32

effectplan_plan_v3(same fourteen arguments,
                   durations_ptr, durations_len, timing_ptr, timing_len) -> i32
```

`v3` runs the `v2` body and then the scheduling pass, so the graph and levels it
produces are the ones `v2` would have produced, by construction rather than by
comparison. The version one export is unchanged: same offsets, same statuses, same
meaning, and it writes neither the levels nor the timing region. The wrapper
requires both the version two and the version three exports — it refuses to load
rather than return plans without levels and without a way to ask for timing.

`0` means success; every other value is a stable error code. The codes are a
wire contract and are never renumbered:

| Code | Meaning                     | Code | Meaning                        |
| ---- | --------------------------- | ---- | ------------------------------ |
| 0    | success                     | 108  | non-canonical region base      |
| 100  | ABI version mismatch        | 110  | invalid effect mode            |
| 101  | too many calls              | 111  | offsets not monotonic          |
| 102  | too many accesses           | 112  | first offset not zero          |
| 103  | too many resources          | 113  | offset terminator mismatch     |
| 104  | pointer out of range        | 114  | resource index out of range    |
| 105  | misaligned region base      | 115  | invalid access mode            |
| 106  | workspace too small         | 116  | levels capacity wrong          |
| 107  | output capacity wrong       | 117  | levels contradict the graph    |
| 118  | a duration is zero          | 119  | timing capacity wrong          |
| 120  | arithmetic overflow         | 121  | tick total would overflow      |
| 122  | schedule contradicts itself | 130  | invalid output graph           |
| 140  | internal inconsistency      |      |                                |

A third export, `effectplan_info(kind)`, publishes the ABI revision, the limits
and the memory layout — including the levels and timing contract revisions and the
canonical regions for all four outputs. It is the single source of truth: the host
never hardcodes an offset or a ceiling, and the engine and the host cannot drift
apart. Unknown selectors return the sentinel, which reaches JavaScript as -1.

## Memory model

The engine owns one contiguous arena. Its base address is the address of a
statically reserved Rust array; what sits below it is the linker's data segment
and shadow stack. The numbers below are the current measured layout — the host
reads them from `effectplan_info`, it does not assume them.

| Region    | Offset    | Bytes  | Contents                                   |
| --------- | --------- | ------ | ------------------------------------------ |
| below     | 0         | ~1 MiB | data segment and shadow stack, unused by us |
| arena     | 1,049,960 | 75,336 | everything below                            |
| ├ input   | 1,049,960 | 5,764  | call modes, offsets, resource ids, modes    |
| ├ work    | 1,055,724 | 17,416 | last writers, reader lists, event lists     |
| ├ output  | 1,073,140 | 36,880 | header + 2,304 edge records × 16 bytes      |
| ├ levels  | 1,110,020 |    260 | `u32` count + 128 `u16` execution levels    |
| ├ durations | 1,110,280 |  512  | 128 `u32` tick counts, written by the host  |
| ├ timing  | 1,110,792 |  1,056 | 32-byte header + 128 × (`u32` ES, `u32` slack) |
| ├ checkpoint | 1,111,848 | 32 | the resumable-planning record (Phase 4) |
| ├ capacities | 1,111,880 | 256 | declared capacity per resource (Phase 5) |
| ├ demand offs | 1,112,136 | 516 | bounds of each call's demands |
| ├ demand recs | 1,112,652 | 8,192 | `(resource, amount)` pairs, flattened |
| ├ scratch | 1,120,844 | 1,536 | usage, in-degree, finish — engine-private |
| ├ schedule | 1,122,124 | 1,056 | `(start, finish)` per call (Phase 5) |
| ├ res stats | 1,123,180 | 1,056 | capacity, peak, busy time per resource |
| └ res diags | 1,124,236 | 1,056 | why a demand cannot be met, by index |

Linear memory is 1,179,648 bytes (18 pages) and never grows on its own. The
timing region is the tail of the arena: a version one or version two call never
touches it, and `v3` invalidates its header before it starts, so a refused
schedule cannot be mistaken for the previous batch's result.

**Pointers are never trusted.** The ABI accepts absolute addresses — including
the four scheduling pointers a version three caller supplies — converts each one
to an arena-relative offset with checked subtraction, and then requires the result
to equal the canonical region base exactly. A pointer that merely
points somewhere inside the arena is refused with `ERR_LAYOUT`, because it could
alias a live region. Only after that check does the engine form the disjoint
slices the algorithm works on, using `split_at_mut` over the const-asserted
layout — no raw pointer ever reaches the scheduler.

If the host grows the memory, `memory.buffer` is replaced and the old buffer is
detached. Writes through views on the detached buffer are silently dropped by
the engine, which is why the TypeScript layer re-maps its views whenever the
buffer identity changes, before it writes a batch.

## Input format

The host packs four arrays:

- `call_modes[i]` — 0 pure, 1 resources, 2 exclusive, 3 unknown.
- `access_offsets[i..i+2]` — CSR-style: the accesses of call `i` are
  `access_offsets[i] .. access_offsets[i+1]`.
- `resource_ids[k]` — the request-local resource index of access `k`, `1..=1024`.
  Index `0` is reserved for the implicit global barrier.
- `access_modes[k]` — 0 read, 1 write.

Resources are interned per batch, in first appearance order, keyed by
`(namespace, id)`. The key is length-prefixed, so `("ab", "c")` and
`("a", "bc")` cannot collide.

## The algorithm

The engine is a single streaming pass over the calls in input order.

State per resource: the last writer (a `u32` with `u32::MAX` as "none") and an
intrusive list of pending readers. State per event: the operation that owns it
and the next event in its resource's list.

- A **read** waits for the current writer of that resource, then joins the
  pending-reader list. Two reads never conflict.
- A **write** waits for the previous writer and for every pending reader, then
  takes ownership and clears the reader list.
- Every call also touches the implicit barrier resource `0`: ordinary calls
  read it, `exclusive` and `unknown` calls write it. That is what makes an
  unknown effect a real global barrier instead of a silent parallel.

Edges are grouped by successor. Within a group the records are sorted by
`(predecessor, resource, reason)` and exact duplicates are dropped, so the
output is a deterministic function of the input. The global order is
successor ascending, then predecessor ascending.

### Execution levels

Every emitted edge points from a lower call index to a higher one — enforced by
construction, re-checked by `validate_output` — and the records are emitted in
successor order. That makes the levels a single forward pass rather than a
topological sort:

```
L(v) = 0                      if v has no predecessors
L(v) = 1 + max(L(u))          over the predecessors u of v
       u -> v
```

By the time the pass reaches a record, its predecessor's level is already final,
so no indegree array and no queue are needed. The pass then refuses the result
outright if any edge fails to rise or any level reaches the call count. Levels
are per call, so the maximum is 127 — not the 1,151 a per-event buffer would
imply; the engine's internal event list is not a graph node and never reaches
the output. `docs/EXECUTION-LEVELS.md` carries the proof, the complexity
comparison and the independent Kahn oracle that checks it.

**Bound.** One barrier event per call plus one event per access gives at most
`MAX_EVENTS = 128 + 1024 = 1152` events, and each event creates at most two
records, so `MAX_EDGES = 2304`. The output region is sized from that bound, and
the engine refuses a batch that would exceed it rather than truncating.

### Critical path

The same two properties — forward edges, records grouped by successor ascending —
are what make the scheduling passes cheap. Earliest times need a forward walk
(Phase 2's shape). Latest times need the reverse: a node's `latest_finish` must be
final before its predecessors use it, which means visiting that node's successors
first. Since a node's successors all carry larger call indices and therefore sit
later in the record array, **the backward pass is the same array walked
backwards**. No reverse adjacency list, no indegree array, no queue, and no vertex
ever scans the whole edge set.

```
forward,  ascending:  ES(succ) = max(ES(succ), ES(pred) + d(pred))
backward, descending: LF(pred) = min(LF(pred), LF(succ) − d(succ))
slack(i) = LF(i) − d(i) − ES(i)
```

Both arithmetic directions are checked: an `ES` that would pass `2^32 − 1` returns
`121`, a `slack` that would go negative returns `122`. Neither can be wrapped
silently into a plausible number, and the backward pass audits the forward one as
it walks — a predecessor finishing after its successor starts fails with `122`
before a byte of the timing region is written.

Peak concurrency sweeps the earliest-start schedule over half-open intervals
`[ES, ES + d)`, so a call finishing at tick 7 and one starting at tick 7 are not
concurrent. Endpoints are packed as `(time << 1) | is_start` into `u64` and sorted
in place with a heap sort: deterministic, total-ordered, `O(V log V)`, no
allocation.

The model, the boundary examples and what is deliberately not modelled — resource
contention above all — are in `docs/SCHEDULING-MODEL.md`. `docs/CRITICAL-PATH.md`
carries the measurements, the memory decision and the independent CPM oracle that
checks the whole thing.

## The output

A 16-byte header — ABI revision, call count, edge count, a reserved word that
must be zero — followed by up to 2,304 records of 16 bytes: predecessor,
successor, resource index, reason. The levels region carries its own `u32` count
header, so a stale region can never be mistaken for the current batch's. The
timing region carries its own revision as a commit record, then makespan, peak
concurrency, total work as a `u64`, the critical count and a reserved word,
followed by two `u32` per call: earliest start and slack. `latest_start`,
`earliest_finish` and `latest_finish` are derived by the host from the durations it
supplied, so nothing is stored twice and nothing can disagree with itself.

Reasons: 1 write/read, 2 read/write, 3 write/write; 4, 5 and 6 are the same
three relations evaluated against the barrier resource.

The TypeScript layer re-reads the whole graph, the whole level array and the whole
timing region before handing anything over, and refuses output that is not
forward-pointing, not in successor order, out of range, carrying a reason outside
1..6, describing a level that reaches outside the batch, leaving an empty wave, or
scheduling a call whose finish passes the makespan or whose edge violates either
precedence direction.

## Safety properties

- The published artifact is `#![no_std]`: no standard library, no allocator, no
  `serde`, no `wasm-bindgen`, no imports of any kind.
- A panic cannot escape: the wasm build installs a panic handler that traps,
  and the host maps the trap to `EP_ENGINE_ERROR`. Failure is always a status
  code, never a silent wrong answer.
- All arithmetic that could overflow is checked; all byte access goes through
  bounds-checked accessors.
- The engine reads and writes only its own arena. No filesystem, no network,
  no environment, no clock, no randomness. Durations are caller data: the engine
  validates their type and range and never interprets them as time.
- Error messages carry the call's index, never the caller's resource ids or
  call ids.

## The TypeScript layer

| Module      | Responsibility                                                  |
| ----------- | --------------------------------------------------------------- |
| `errors.ts` | the stable error surface and the status → code mapping          |
| `loader.ts` | obtain the bytes, instantiate, audit the published layout       |
| `encode.ts` | validate the public contract, intern, normalise, pack           |
| `planner.ts`| one ABI call per batch, re-validate, hand out an immutable plan |
| `execution-levels.ts` | read the levels region, group waves, refuse a bad partition |
| `timing.ts` | write the durations, read the timing region, refuse a schedule that contradicts its own graph |

`EffectPlan.create()`, `planBatch()` and `getEngineInfo()` remain the entire
public surface. `planBatch()` returns `plan.execution` — `levelByCall`, `waves`,
`waveCount` — and, when the caller passed `timing: { durations }`, `plan.timing`
with the critical-path analysis. Every Phase 1 and Phase 2 field is unchanged, and
a batch planned without durations has `timing: undefined` rather than an empty
object standing in for a schedule that was never computed.
