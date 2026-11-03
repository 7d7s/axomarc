# @effectplan/core

Deterministic, resource-aware dependency planning for AI tool operations.

Give EffectPlan a batch of tool calls with declared effects and it returns the
dependency graph — which calls must not run together and why — the parallel-safe
execution waves, and, when you supply durations, the critical-path analysis:
makespan, slack and the calls that determine completion time.

**EffectPlan never executes a tool.** It plans; your host executes.

Rust core compiled to `wasm32-unknown-unknown`: no `wasm-bindgen`, no allocator,
no imports. One npm package, zero runtime dependencies, no network, no
filesystem, no telemetry.

```sh
npm install @effectplan/core
```

## Use it

```ts
import { EffectPlan } from "@effectplan/core";

const planner = await EffectPlan.create();

const plan = planner.planBatch(
  [
    { id: "read",  effect: { version: 1, mode: "resources", accesses: [{ resource: { namespace: "fs", id: "c.json" }, mode: "read"  }] } },
    { id: "write", effect: { version: 1, mode: "resources", accesses: [{ resource: { namespace: "fs", id: "c.json" }, mode: "write" }] } },
  ],
  { timing: { durations: [4, 6], unit: "ms" } },
);

plan.edges;            // [{ from: 0, to: 1, resource: 1, reason: 2 }]  read/write
plan.execution.waves;  // [[0], [1]]  what may run in parallel
plan.timing;           // { criticalPathDuration: 10, peakConcurrency: 1, events: [...] }
```

Indices are positions in the array you passed, not call ids.

| API | Purpose |
| --- | --- |
| `EffectPlan.create(options?)` | instantiate the engine; `wasmBytes`/`wasmUrl` override the bundled binary |
| `planBatch(calls, options?)` | validate, plan, layer and (with `options.timing`) analyse one batch |
| `planResources(calls, options)` | place a batch against declared capacity |
| `checkResources(calls, options)` | ask whether the demands can be met |
| `getEngineInfo()` | ABI revisions, limits and the memory layout the engine published |
| `planner.createSession(calls, options?)` | keep one plan alive through appends, replacements and removals |

**Capacity resources.** `planResources` takes `capacities` (each a `key` and an
integer `capacity`), `demands` (one entry per call, in batch order, naming what
that call holds for the whole of its execution) and `durations`, and returns the
placement: `outcome`, `makespan`, `windows`, `deferrals`, `statistics` and
`quality`. `checkResources` asks the same question without a clock, and is a
separate export on purpose: a scheduling-only change must not rebuild the
dependency graph. Resource keys never reach the engine — the host interns them
into indices, and a diagnostic names an index, never your key.

**The quality certificate.** Every placement comes with one: `makespan`, the
dependency lower bound, the resource lower bound, the stronger of the two, the
total waiting, and `provablyOptimal`, which is true only when the makespan equals
the bound. `lowerBoundGap` is a distance from a bound and never an optimality
gap — a schedule can sit above it and still be optimal, because the bound does
not capture every interaction. A placement that cannot be certified is refused
(`EP_INVALID_PLAN`) rather than reported with a number nobody checked.

**Sessions.** `applyChanges({ expectedVersion, changes })` validates the whole
transaction before anything moves; `current()` is an immutable snapshot. Keys are
unique, at most 128 UTF-8 bytes, never echoed in an error. Each update reports its
`strategy` and `reusedPrefixCalls`; a checkpoint that cannot be trusted plans the
batch whole, and the graph is the same either way.

**Sessions and capacity resources.** A session may declare the capacity
resources it runs against (`options.resources.capacities`), and each call may
declare what it holds (`demands`, named by the caller's own key). The table
belongs to the session and holds until `reset()` installs a new one, because a
capacity that changes under a running plan invalidates every placement the
session published. Every batch the session then plans is placed against that
table, and the snapshot carries the placement and its certificate.

**Two invalidation domains.** An update reports which one it touched.
`dependency` means the graph could have changed — an access declaration was
rewritten, or a call was removed. `scheduling` means only the clock and the
demands moved, so the dependency graph was reused whole and nothing about the
conflict rules was recomputed. The session's word is an optimisation, not a
guarantee: the engine checks every restored state against the input prefix that
is supposed to have produced it, and refuses the restore if the two disagree.

**Effect modes** `pure` (no accesses) · `resources` (the accesses you declare) ·
`exclusive` and `unknown` (global barriers). `options.unknownEffectPolicy`
accepts `"serialize"` (default) or `"reject"`.

**Reasons** 1 write/read · 2 read/write · 3 write/write · 4–6 the same over the
barrier. Output is deterministic: the same batch always yields the same bytes,
in the same order, and levels are computed in one forward pass over the graph.

**`plan.timing`** carries `earliestStart`/`earliestFinish`,
`latestStart`/`latestFinish`, `slack` and `critical` per call, plus
`criticalPathDuration`, `totalWork`, `peakConcurrency` and
`averageParallelism`. Durations are positive integers of at most
4,294,967,295 ticks in whatever unit you choose; the unit label is metadata,
never converted. A zero duration is refused, not treated as instant, and a plan
whose ticks would pass the range is refused rather than wrapped.

Concurrency is measured over half-open intervals: a call that finishes at tick 7
and one that starts at tick 7 are not concurrent. Criticality is exact — every
zero-slack call, never an enumerated path — so a plan with two independent
critical chains reports both.

These are estimates under the dependencies you declared and the durations you
supplied, computed at unlimited capacity. CPM does not know about your machine
pool: a plan where sixty calls all read the same file will look faster than it
is, which is what `peakConcurrency` is there to warn you about.

**Errors** are `EffectPlanError` with a stable `code`: `EP_INVALID_INPUT`,
`EP_DUPLICATE_CALL_ID`, `EP_INVALID_EFFECT`, `EP_INVALID_RESOURCE`,
`EP_UNKNOWN_EFFECT_REJECTED`, `EP_LIMIT_EXCEEDED`, `EP_INVALID_PLAN`,
`EP_ENGINE_UNAVAILABLE`, `EP_ENGINE_ERROR`. Messages carry the call's index,
never your resource or call identifiers.

**Capacity resources** declare how many units of a lane may be held:
`capacities` are `{ key, capacity }` pairs, `demands` one list per call. The
result carries the `schedule`, per-resource `statistics` and `diagnostics`; an
impossible demand is `outcome: "infeasible"`.

**Limits per batch** 128 calls · 1,024 accesses · 1,024 resources · 256-byte
namespace · 1,024-byte resource id · 128 durations · 64 capacity resources ·
1,024 demand records.

## Documentation

Architecture: [`docs/ARCHITECTURE.md`](https://github.com/7d7s/axomarc/tree/main/EffectPlan/docs/ARCHITECTURE.md)
· the byte-level ABI: `docs/ABI.md` · the timing model:
`docs/SCHEDULING-MODEL.md` · execution levels: `docs/EXECUTION-LEVELS.md` ·
critical path: `docs/CRITICAL-PATH.md` · measured sizes: `docs/SIZE-REPORT.md`.

## License

MIT
