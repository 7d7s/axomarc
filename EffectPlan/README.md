# EffectPlan

Deterministic, resource-aware dependency planning for AI tool operations.

EffectPlan takes a batch of tool calls, each with a declared effect contract,
and returns three things: a predecessor → successor dependency graph — which
calls must not run concurrently because they touch the same resource, and why —
execution levels, the parallel-safe waves that graph implies, and, given
durations, a critical-path analysis: how long the plan takes, which calls decide
that, how much slack everything else has, and how many calls want to run at once.
A host can schedule the batch without implementing its own topological sort or
its own CPM.

**EffectPlan never executes a tool.** It plans; your host executes.

- Rust core, compiled to `wasm32-unknown-unknown`. No `wasm-bindgen`, no
  `serde`, no allocator, no imports.
- One npm package, `@effectplan/core`, zero runtime dependencies, no network,
  no filesystem access, no telemetry.
- 70,042 bytes unpacked. From Phase 5 the package budget is a frozen baseline
  (the Phase 4 artifact) plus a per-phase envelope agreed in advance, rather than
  a ceiling raised each time the work outgrew it — see `docs/SIZE-REPORT.md`. The
  wasm artifact is 15,421 bytes (budget: 40,000). Both measured from the packed
  tarball, not from source.

## Quick start

```ts
import { EffectPlan } from "@effectplan/core";

const planner = await EffectPlan.create();

const calls = [
  {
    id: "read-config",
    effect: {
      version: 1,
      mode: "resources",
      accesses: [{ resource: { namespace: "fs", id: "config.json" }, mode: "read" }],
    },
  },
  {
    id: "rewrite-config",
    effect: {
      version: 1,
      mode: "resources",
      accesses: [{ resource: { namespace: "fs", id: "config.json" }, mode: "write" }],
    },
  },
];

const plan = planner.planBatch(calls);

plan.edges;            // [{ from: 0, to: 1, resource: 1, reason: 2 }]  read/write
plan.execution.waves;  // [[0], [1]]  what may run at the same time
plan.timing;           // undefined — no durations were given
```

With durations, the same call also returns the critical path:

```ts
const timed = planner.planBatch(calls, { timing: { durations: [4, 6], unit: "ms" } });

timed.timing?.criticalPathDuration;  // 10
timed.timing?.totalWork;             // 10
timed.timing?.peakConcurrency;       // 1 — the plan cannot go faster than serial
timed.timing?.averageParallelism;    // 1
timed.timing?.criticalEvents;        // [0, 1] — both calls decide the finish
timed.timing?.events[0];             // earliest/latest start and finish, slack, critical
```

Durations are integer ticks in a unit you name; the engine does not convert them
and does not read them. A zero duration is refused rather than treated as free
work, and a plan whose ticks would overflow is refused rather than wrapped. Omit
`timing` and `plan.timing` is `undefined` — the Phase 1 and Phase 2 calls still
hit the same two entry points and cost what they always cost.

The full API is documented in [`packages/core/README.md`](packages/core/README.md).

## Repository layout

```
engine/         Rust core: the planner, the scheduling passes, the raw ABI, the arena
packages/core/  the npm package: TypeScript wrapper over the wasm engine
benchmarks/     three independent oracles, the workload, the measurement harness
scripts/        build, verification, packing, filtered-test and browser-test tooling
test/browser/   the browser smoke test
docs/           architecture, ABI, implementation, scheduling model, critical path,
                execution levels, size reports
```

## Commands

```sh
npm install                # dev dependencies (TypeScript, fast-check, binaryen, node types)
npm run build              # wasm + JavaScript + the packed npm artifact
npm test                   # the TypeScript suite
npm run test:rust          # the Rust suite
npm run test:wasm          # the WebAssembly integration suite
npm run test:browser       # the browser smoke test (needs Chrome or Chromium)
npm run test:levels        # the execution-levels suite
npm run test:timing        # the scheduling suite
npm run test:timing-oracle # exhaustive parity against an independent CPM implementation
npm run test:api-compatibility # Phase 1 and Phase 2 surfaces, unchanged behaviour
npm run verify:wasm        # the ABI, scheduling and memory-layout gate
npm run verify:size        # the release size gate
npm run verify:package     # install the tarball and plan a batch with it
npm run benchmark          # native, wasm, oracle and full-API measurements and gates
```

The Rust toolchain needs the wasm target:

```sh
rustup target add wasm32-unknown-unknown
```

## Measured results

Intel Core i5-10300H, Node v20.15.0, Windows x64, measured 2026-11-01. Full
numbers in `artifacts/benchmarks.json` and `artifacts/native-bench.json`; p50
microseconds. "Wasm plan" is the raw `v1` entry point, "+ levels" is `v2`, "+
timing" is `v3`; "Full npm API" is `planBatch` without durations and "API +
timing" with them. "CPM oracle" is the independent reference implementation, not
the engine.

| Dataset  | Calls / accesses | Edges | Waves | Makespan | Peak | Wasm plan | + levels | + timing | Full npm API | API + timing | CPM oracle |
| -------- | ---------------- | ----- | ----- | -------- | ---- | --------- | -------- | -------- | ------------ | ------------ | ---------- |
| tiny     |           8 / 32 |    36 |     8 |       66 |    1 |      13.3 |     14.3 |     15.8 |         25.7 |         28.8 |       36.5 |
| small    |         32 / 128 |   144 |     8 |      108 |    4 |      37.1 |     42.2 |     46.6 |         79.0 |         89.2 |      111.9 |
| normal   |         64 / 256 |   288 |     8 |       78 |    8 |      67.2 |     77.0 |     87.8 |        148.2 |        168.1 |      418.8 |
| medium   |       128 / 1024 |  1,259 |    16 |      149 |    8 |     343.7 |    385.5 |    417.7 |        641.7 |        696.7 |     1799.0 |
| wide     |         64 / 128 |     0 |     1 |       17 |   64 |      14.3 |     15.1 |     20.2 |         62.7 |         72.9 |      131.0 |
| deep     |          64 / 64 |    94 |    64 |      568 |    1 |      19.3 |     23.0 |     50.5 |         48.5 |         64.5 |      323.9 |
| diamond  |         64 / 128 |    56 |     8 |       76 |   36 |      19.8 |     22.2 |     29.6 |         65.7 |         78.4 |      127.2 |
| barrier  |          64 / 86 |   198 |    43 |      445 |    2 |      49.2 |     56.2 |     65.6 |         96.5 |        111.9 |      150.2 |
| conflict |         64 / 256 |   334 |    64 |      568 |    1 |      77.5 |     89.3 |     99.0 |        166.7 |        182.1 |      226.9 |
| maxbound |       128 / 1024 |  1,152 |     8 |       82 |   16 |     311.8 |    344.2 |    379.9 |        627.2 |        674.9 |     1853.1 |
| mixed    |         96 / 276 |   333 |    49 |      550 |    4 |      94.8 |    106.3 |    121.5 |        188.3 |        215.5 |      511.2 |

The overhead gates the specification names — normal and medium, 20% over the
Phase 2 API — pass: **+13.4%** and **+8.6%** at the API, **+14.0%** and **+8.4%**
at the engine, measured as paired blocks in one process. The harness fails closed
if a gated workload breaches them.

The engine's single forward level pass agrees with an independent Kahn traversal,
and its two CPM passes agree with an independent critical-path implementation, on
every one of the eleven shapes and on 7,381 exhaustively generated batches; the
wasm build and the native build agree on edges, waves, makespan, peak concurrency
and critical count for every dataset. Cold start, a fresh engine every time
including reading the binary: 402–618 µs at the median.

## Status

Phase 4 (incremental planning) is **in progress**: the engine can stop at a call
boundary, commit a checkpoint of the state it stopped in, and continue from that
checkpoint or refuse it; `planner.createSession()` keeps a plan alive through
appends, replacements and removals, and an incremental plan is proved equal to a
fresh rebuild by a randomised differential test. Phase 4 kept linear memory at
17 pages because the session state lives in the host; Phase 5's capacity regions
took the 18th page, approved on measurement (`docs/RESOURCE-ABI.md` §1). The
package sits at 71,964 bytes unpacked against an 82,042-byte ceiling —
`docs/SIZE-REPORT.md` records the three Phase 4 reviews and the envelope policy
that replaced them. What is outstanding is the CI evidence: every job has passed
locally and the branch is pushed, but no GitHub Actions run exists until a pull
request opens.

Phase 1 (the dependency planner), Phase 2 (execution levels) and Phase 3 (the
critical path) are complete and measured: the engine, the package, the tests, the
gates and the benchmarks all exist and run.

Phase 3 added one versioned ABI export (`effectplan_plan_v3`, 18 parameters), two
output regions (512 bytes of durations in, 1,056 bytes of schedule out), the
additive `timing` option, and 20 Rust scheduling tests alongside a TypeScript
suite that grew from 122 to 156 — 20 timing tests, 7 critical-path properties and
8 raw-ABI scheduling checks — without touching the twelve- and fourteen-parameter
contracts, the 40,000-byte artifact budget or the 50,000-byte package limit that
was in force until the Phase 4 review.
Linear memory is 18 pages, and that is a build-time gate rather than a hope.

- the model and the arithmetic contract: [`docs/SCHEDULING-MODEL.md`](docs/SCHEDULING-MODEL.md)
- the byte-level ABI for all three phases: [`docs/ABI.md`](docs/ABI.md)
- the Phase 3 report, the memory decision and the cost analysis: [`docs/CRITICAL-PATH.md`](docs/CRITICAL-PATH.md)
- the Phase 2 specification, the ordering-invariant proof and its measured cost: [`docs/EXECUTION-LEVELS.md`](docs/EXECUTION-LEVELS.md)
- the rest: [`docs/IMPLEMENTATION.md`](docs/IMPLEMENTATION.md),
  [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md),
  [`docs/SIZE-REPORT.md`](docs/SIZE-REPORT.md)

Not yet evidenced: CI on a real GitHub runner. The workflow exists and every job
runs locally; the branch has not been pushed, which is a decision for whoever owns
the remote.

## License

MIT
