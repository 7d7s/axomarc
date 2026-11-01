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
| `getEngineInfo()` | ABI revisions, limits and the memory layout the engine published |

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

**Limits per batch** 128 calls · 1,024 accesses · 1,024 resources · 256-byte
namespace · 1,024-byte resource id · 128 durations.

## Documentation

Architecture: [`docs/ARCHITECTURE.md`](https://github.com/7d7s/axomarc/tree/main/EffectPlan/docs/ARCHITECTURE.md)
· the byte-level ABI: `docs/ABI.md` · the timing model:
`docs/SCHEDULING-MODEL.md` · execution levels: `docs/EXECUTION-LEVELS.md` ·
critical path: `docs/CRITICAL-PATH.md` · measured sizes: `docs/SIZE-REPORT.md`.

## License

MIT
