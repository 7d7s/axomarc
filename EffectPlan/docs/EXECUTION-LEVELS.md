# Execution levels

Phase 2 adds one capability to the planner: parallel-execution levels. The
dependency graph says what must happen before what; the levels say what may
happen at the same time.

EffectPlan still never executes anything. The host reads the waves and decides
how — and whether — to run them concurrently.

## The model

Let `G = (V, E)` be the dependency graph the planner emits. `V` is the batch's
**calls**; `E` is the set of dependency edges, each written `from -> to`,
meaning `to` depends on `from`.

The execution level of a call:

```
L(v) = 0                      if v has no predecessors
L(v) = 1 + max(L(u))          over the predecessors u of v
       u -> v
```

The wave at level `k` is `W_k = { v | L(v) = k }`.

The guarantee follows directly:

```
(u, v) in E  =>  L(u) < L(v)
```

Two calls in the same wave therefore have no edge between them. Because every
conflicting pair has a path in the emitted graph (see "The ordering
invariant"), the guarantee extends: no two calls in the same wave conflict on
any declared resource, including the barrier.

These are dependency-optimal levels under the assumption that one call occupies
one scheduling step. They are not a weighted schedule for calls of different
durations — that is Phase 3 territory.

### Example

```
0 writes f1        wave 0: [0]
1 reads  f1        wave 1: [1, 2]     <- may run concurrently
2 reads  f1        wave 2: [3]
3 writes f1
```

## Node identity: calls, not events

One point must be exact, because everything downstream indexes by it.

The engine has two counting concepts:

- **calls** — the batch positions `0..call_count`. These are the graph nodes.
  `edge.from` and `edge.to` are call indices.
- **events** — internal nodes in the pending-reader lists, bounded by
  `MAX_EVENTS = 128 + 1024 = 1152`. These never appear in any output.

Execution levels are per **call**. `levelByCall[i]` is the level of the call at
position `i` of the array the caller passed to `planBatch`, which is the same
index `edge.from` / `edge.to` use, and which maps to the caller's `id` only by
position. Consequently:

| Quantity | Bound | Why |
| --- | --- | --- |
| Levels published | `MAX_CALLS = 128` | one per graph node |
| Maximum level | `127` | a chain over every call |
| Levels buffer | `4 + 128 × 2 = 260 B` | count header plus `u16` array |

The planning spec proposed a 2,304-byte buffer sized from `MAX_EVENTS`. That
sizing would have been wrong for this engine: it would have stored a level per
internal reader-list node, none of which is a graph node. The buffer is 2,044
bytes smaller as a result, and the linear memory stayed at 17 pages (measured,
not assumed).

## The ordering invariant

**Claim.** Every emitted edge points from a lower call index to a higher one:
`(u, v) in E => u < v`.

**Why it holds by construction.**

- `process_read(op, rid)` waits on `last_writer(rid)`. A last writer is set
  only by `process_write` of an *earlier* call, because calls are processed in
  ascending order. The record `writer -> op` therefore has `writer < op`, and a
  self-comparison guard rejects `writer == op`.
- `process_write(op, rid)` waits on `last_writer(rid)` and on every pending
  reader. Readers enter the list in `process_read` of earlier calls and are
  drained only by a later write, so both `writer < op` and `reader < op`, with
  the same self guards.
- The barrier at resource 0 is processed by the same two functions, so barrier
  edges obey the same rule.
- `validate_output` re-checks the property on every record before the call
  returns (`pred >= succ` fails with status 130), so the invariant is enforced
  at runtime, not merely argued.

**Why that makes a topological sort unnecessary.** Edges are emitted grouped by
successor, in ascending successor order. When the pass reaches the records of
call `v`, every record whose successor is `< v` has already been read — and
every predecessor `u` of `v` satisfies `u < v`, so `L(u)` is already final.
A single forward scan therefore computes exact longest-path levels:

```rust
for record in edges {                    // emitted order: successor ascending
    let next = levels[record.from] + 1;
    if next > levels[record.to] { levels[record.to] = next; }
}
```

Roots start at 0 and are never touched, which is exactly `L(v) = 0`. The
forward-propagation form in the planning document is the same computation; this
engine stores dependencies grouped by successor, so it uses the predecessor form.

**Independently verified.** `benchmarks/reference-levels.ts` computes levels a
deliberately different way: a Kahn layered traversal over the full all-pairs
conflict set from the O(N²) oracle. The two methods are compared per node, not
by wave count:

- exhaustively: every batch of up to 4 calls over a 9-shape alphabet (7,381
  batches) — zero disagreements;
- by property: 400 randomised batches × 7 level properties — zero disagreements;
- in the benchmark: all 11 datasets, closure and levels — zero disagreements;
- in Rust: 21 level tests through the real ABI.

The invariant is also asserted directly over a 64-call mixed-mode workload
(`every_edge_points_forward`).

## Complexity and memory

| Operation | Time | Extra working memory |
| --- | --- | --- |
| Dependency planning (Phase 1) | O(C + A + E) | none (static arena) |
| Level computation | O(E) single pass | 256 B on the shadow stack |
| Wave grouping (TypeScript) | O(C) | the public arrays |

The comparison the planning document asked for:

| Approach | Time | Extra state |
| --- | --- | --- |
| Generic Kahn topological sort | O(V + E) | indegree array + queue |
| Repeated relaxation | up to O(V·E) | levels |
| **Chosen: forward-order pass** | **O(E)** | **levels only** |

No heap allocation anywhere in the engine: the level scratch array is a fixed
`[u16; 128]` local, and the result is written into the statically reserved
arena.

## The versioned ABI

`effectplan_plan` keeps its twelve parameters, its offsets and its meaning.
Phase 2 adds an export beside it:

```
effectplan_plan_v2(
  abi_version, call_count, access_count, resource_count,
  call_modes_ptr, access_offsets_ptr, resource_ids_ptr, access_modes_ptr,
  workspace_ptr, workspace_len, output_ptr, output_capacity,
  levels_ptr, levels_len
) -> i32
```

The extra two arguments name the levels region. They are validated the same way
every other pointer is: the address must convert to an arena-relative offset and
must equal the canonical base published by `effectplan_info`, and the length
must equal the canonical capacity. Nothing is trusted because JavaScript handed
it in.

New status codes; no existing code changed meaning:

| Code | Name | Meaning |
| --- | --- | --- |
| 116 | `ERR_LEVELS_CAP` | the levels length is not the canonical capacity |
| 117 | `ERR_LEVELS_INVALID` | the computed levels contradict the graph |

New introspection selectors:

| Kind | Publishes |
| --- | --- |
| 21 | levels contract revision (`1`) |
| 22 | absolute address of the levels region |
| 23 | region length in bytes (`260`) |
| 24 | level entries the region can hold (`128`) |

### Levels region layout

```
offset 0  u32   level_count        == call_count of the batch
offset 4  u16[] levels             little-endian, one entry per call
```

The count header is what makes a stale region impossible: the TypeScript
decoder refuses any region whose count differs from the batch it just planned.

The maximum valid level for a batch of `n` calls is `n - 1`; a level at or above
`n` is refused by the engine with 117 and by the wrapper with `EP_INVALID_PLAN`.

## The TypeScript API

```ts
const plan = planner.planBatch(calls);

plan.execution.levelByCall;  // readonly number[], indexed by call position
plan.execution.waves;        // readonly (readonly number[])[], ascending levels
plan.execution.waveCount;    // number of waves (highest level + 1)
```

Waves are zero-based; calls inside a wave keep their input order; an empty
batch yields zero waves; everything returned is frozen. TypeScript only groups
the numbers the engine produced — it computes no dependency and no level.

## Determinism

The levels are a pure function of the batch: same input, same bytes. The pass
reads records in emission order (successor ascending, predecessor ascending,
deduplicated), so no map iteration or clock enters the result. Tested by
replaying the same batch 25 times with an interleaving batch between replays,
and by the Rust determinism test comparing repeated runs.

## Barrier semantics

The implicit barrier at resource 0 is part of the graph, so it is part of the
level computation. The pass reads emitted edges only — it never looks at
explicit resource ids, and it cannot skip a barrier relationship.

Measured shapes: `[write f1, exclusive, read f2]` gives levels `[0, 1, 2]` and
three waves, and no barrier call ever shares a wave with any other call (property
test "barriers keep their wave to themselves").

## Error handling

| Failure | Behaviour |
| --- | --- |
| Non-canonical levels pointer | status 108, nothing written |
| Wrong levels length | status 116 |
| Backward or out-of-range record feeding the pass | status 130 |
| Level ≥ call count, or levels that fail to explain an edge | status 117 |
| Region count ≠ call count, level out of range, empty wave on decode | `EP_INVALID_PLAN` |
| Engine without `effectplan_plan_v2`, or wrong levels revision | `EP_ENGINE_UNAVAILABLE` |

Every refusal is *before* any write of that region, and the wrapper validates
the decoded output before a caller can see it. A malformed ABI result can never
be read as a valid plan. Error text carries indices and numbers, never caller
resource ids or call ids.

## Memory growth

The levels region lives in the arena, whose addresses survive `memory.grow`;
the *views* the wrapper holds do not. `planBatch` re-maps before every batch,
so levels read after growth come from the live buffer. The integration test
"keeps execution levels correct after the host grows the memory" plans the same
batch before and after a growth and requires identical waves.

## Limitations

1. **Declared effects only.** If a caller under-declares what it touches, the
   waves are unsafe with respect to real behaviour. The planner cannot see side
   effects it was not told about. This is a contract limit, not an algorithm bug.
2. **One call, one step.** Levels assume unit-duration calls. A slow call in an
   early wave does not delay its wave; the host's own limits decide.
3. **Same wave ≠ will run together.** The host may run a wave serially, rate
   limited, or on one worker. Nothing in the model promises concurrency, only
   that nothing in a wave depends on anything else in the same wave.
4. **Per-batch numbering.** Resource ids and levels are scoped to one batch;
   they carry no meaning across calls to `planBatch`.
5. **No execution management.** Retries, cancellation, failure handling and
   worker policy stay in the host. That is deliberate: EffectPlan is a planner.

## Compatibility

- `effectplan_plan` (v1) is unchanged: same parameters, same region bases, same
  statuses. It does not write the levels region; the wrapper refuses to load an
  engine without the v2 export rather than degrade silently.
- `ExecutionPlan` keeps every Phase 1 field. `execution` is additive, so
  existing consumers compile and behave unchanged.
- `getEngineInfo()` gains `levelsRevision` and `layout.levels`.
- Package and artifact limits are unchanged; the measured numbers are in
  [SIZE-REPORT.md](./SIZE-REPORT.md).

## Measured results

These are the Phase 2 measurements, from the session that shipped the level pass;
they are preserved verbatim in `artifacts/benchmarks-phase2.json`, which is also
the recorded baseline the Phase 3 overhead gate reads. The Phase 3 run of the same
workloads, including the levels columns, is in `docs/CRITICAL-PATH.md` and
`artifacts/benchmarks.json`.

Native, `cargo bench` (µs, p50, shipped `release` profile):

| Dataset | plan | plan + levels | levels alone | edges | waves |
| --- | --- | --- | --- | --- | --- |
| tiny 8/32 | 1.9 | 2.5 | 0.5 | 36 | 8 |
| small 32/128 | 7.0 | 9.0 | 2.1 | 144 | 8 |
| normal 64/256 | 13.7 | 17.8 | 4.1 | 288 | 8 |
| medium 128/1024 | 81.4 | 96.6 | 17.2 | 1259 | 16 |
| wide | 1.1 | 1.1 | 0.1 | 0 | 1 |
| deep | 2.8 | 4.1 | 1.4 | 94 | 64 |
| diamond | 2.6 | 3.4 | 0.8 | 56 | 8 |
| barrier | 9.9 | 12.6 | 2.8 | 198 | 43 |
| conflict | 15.6 | 20.2 | 4.6 | 334 | 64 |
| maxbound | 71.9 | 87.6 | 17.1 | 1152 | 8 |
| mixed | 19.3 | 23.8 | 4.6 | 333 | 49 |

`plan + levels` minus `plan` matches the separately measured level pass, which
is what an O(E) walk should look like: 0.1–17.2 µs against a 1.1–81.4 µs plan.

WebAssembly, both entry points measured in the same process (µs, p50):

| Dataset | v1 plan | v2 plan+levels | added | full npm API | TS oracle |
| --- | --- | --- | --- | --- | --- |
| tiny | 12.5 | 13.9 | +1.4 (11.2%) | 24.1 | 15.6 |
| small | 34.9 | 39.8 | +4.9 (14.0%) | 75.9 | 49.5 |
| normal | 64.4 | 74.1 | +9.7 (15.1%) | 145.7 | 201.4 |
| medium | 344.2 | 386.8 | +42.6 (12.4%) | 652.9 | 1,751.1 |
| wide | 14.4 | 14.9 | +0.5 (3.5%) | 60.9 | 118.1 |
| deep | 18.9 | 22.4 | +3.5 (18.5%) | 49.0 | 75.5 |
| diamond | 20.1 | 22.4 | +2.3 (11.4%) | 65.7 | 104.4 |
| barrier | 47.8 | 55.9 | +8.1 (16.9%) | 97.0 | 79.1 |
| conflict | 73.0 | 84.2 | +11.2 (15.3%) | 159.7 | 118.1 |
| maxbound | 310.9 | 348.8 | +37.9 (12.2%) | 627.3 | 1,796.0 |
| mixed | 95.0 | 105.5 | +10.5 (11.1%) | 188.8 | 352.0 |

Cold init — a fresh engine each time, including reading the binary — measured
392–529 µs at the median across the eleven datasets; it is unchanged by Phase 2,
because the extra 2,242 bytes of code are not what cold start pays for.

The 10% target is exceeded on the paired numbers, with the decision recorded in
[IMPLEMENTATION.md](./IMPLEMENTATION.md#level-pass-cost-and-the-10-target).

What the host saves: layering the batch from the outside — all-pairs conflict
detection plus a Kahn traversal, the `benchmarks/reference-levels.ts` path —
costs 75–1,796 µs on these datasets, and needs the conflict pairs first. The
engine produces the same levels for 0.5–42.6 µs, inside a planning call the host
already makes. Cross-session comparison of the *unchanged* v1 path moved by up
to ±30% on this laptop, which is why every regression figure above is a paired
sample from one process rather than a comparison against the Phase 1 session.
