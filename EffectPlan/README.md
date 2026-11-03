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
- 105,344 bytes unpacked against a 106,565 ceiling. From Phase 5 the package budget
  is a frozen baseline
  (the Phase 4 artifact) plus a per-phase envelope agreed in advance, rather than
  a ceiling raised each time the work outgrew it — see `docs/SIZE-REPORT.md`. The
  wasm artifact is 21,295 bytes (budget: 40,000). Both measured from the packed
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

### Capacity resources

A dependency says two calls must not run at the same time. A *capacity
resource* says how many of them may run at once, and each call declares how much
it holds for the whole of its execution:

```ts
const placed = planner.planResources(calls, {
  capacities: [{ key: "lane", capacity: 2 }],
  demands: [
    [{ resource: "lane", amount: 1 }],
    [{ resource: "lane", amount: 1 }],
  ],
  durations: [4, 6],
});

placed.outcome;         // "feasible"
placed.makespan;        // 10 — the two calls conflict on config.json, so they serialise
placed.statistics[0];   // { key: "lane", capacity: 2, peak: 1, busyTime: 10n }
placed.quality;         // { makespan: 10, dependencyLowerBound: 10,
                        //   resourceLowerBound: 5, lowerBound: 10,
                        //   lowerBoundGap: 0, provablyOptimal: true,
                        //   totalResourceWaiting: 0 }
```

`planResources` places the batch; `checkResources` asks whether it *could* be
placed, and is a separate call on purpose — a scheduling-only change must not
rebuild the dependency graph.

Every placement comes with a **quality certificate**: the makespan, the two
lower bounds the engine can prove (dependency and workload), the stronger of
them, the total waiting, and `provablyOptimal`, which is true only when the
makespan equals the bound. In the example above the dependency bound (10) binds
and the workload bound (5) does not: the two calls cannot overlap because they
touch the same file, not because the lane is too narrow. The bound is a distance
from a number, never an optimality gap — a schedule can sit above it and still
be optimal, because the bound does not capture every interaction.

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
npm run test:resource-oracle # placements against an independent reference
npm run test:resource-api    # the capacity-resource public surface
npm run test:resource-layout # the resource regions and the arena contract
npm run test:browser       # the browser smoke test (needs Chrome or Chromium)
npm run test:levels        # the execution-levels suite
npm run test:timing        # the scheduling suite
npm run test:timing-oracle # exhaustive parity against an independent CPM implementation
npm run test:api-compatibility # Phase 1 and Phase 2 surfaces, unchanged behaviour
npm run verify:wasm        # the ABI, scheduling and memory-layout gate
npm run verify:size        # the release size gate
npm run verify:package     # install the tarball and plan a batch with it
npm run benchmark          # native, wasm, oracle and full-API measurements and gates
npm run benchmark:headroom # how much a Phase 6 optimizer could win, per family
npm run prototype:strategies # candidate priority rules against the exhaustive optimum
npm run ci:check           # the repository-root workflow matches the project's
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

**Phase 6 (schedule optimization) has shipped its first deliverable: the quality
certificate.** The engine can now read back a placement it made and publish the
two lower bounds it can prove — the dependency bound and the workload bound —
together with the total waiting and a single optimality claim that is true only
when the makespan equals the bound. It is a reader, not a second planner: it
writes only its own region, so re-running it cannot disturb the placement it
judges, and it recomputes the makespan from the windows rather than copying the
header, which makes the host's cross-check of the two a real check rather than a
tautology (`docs/RESOURCE-ABI.md` §6).

Phase 6 began with a measurement rather than a feature, and the measurement is
what bounds it. `npm run benchmark:headroom` solves 1,200 generated batches
twice — once with the shipped engine, once with an exhaustive oracle — and finds
the baseline already optimal in 1,187 of them (98.9%), with all 13 improvable
instances in three families and seven families structurally incapable of
improvement. Then `npm run prototype:strategies` ran five candidate priority
rules over that same corpus and found that **every rule regresses more instances
than it wins**: the longest-remaining-path rule wins 4 and loses 9, the
resource-aware rule wins 2 and loses 15. The honest ceiling is 6 of the 13
improvable instances and 17 of the 28 recoverable ticks, and only a bounded
local search gets there — for 11 ticks across 1,200 instances
(`docs/OPTIMIZATION-STRATEGIES.md`). The recommendation on the table is
therefore to ship the certificate and not to build the optimizer; that is the
owner's call (`docs/OPTIMIZATION-RFC.md` §11).

Phase 5 (resource intelligence) is **complete**. The
engine understands capacity resources as a model separate from dependency
resources: typed declarations with an integer capacity, demands that a call
holds for the whole of its execution, atomic reservation, and a deterministic
list scheduler that places a batch against the capacities it was given.
`planner.planResources()` and `planner.checkResources()` are the public surface;
the check is its own export so that a scheduling-only change cannot rebuild the
dependency graph. The schedules are verified against an independent reference — a
tick-by-tick capacity checker and an exhaustive search over precedence-feasible
sequences whose makespan is a true lower bound — and the heuristic is second-best
on two of 32 seeded instances, by at most three ticks, which is stated rather
than implied (`docs/RESOURCE-MODEL.md` §6). A session can carry those
declarations too: it owns the capacity table, places every batch it plans, and
reports which of the two invalidation domains a transaction touched —
`dependency` when the graph could have changed, `scheduling` when only the clock
and the demands moved, in which case the dependency graph is reused whole. The
engine has the last word either way: it checks every restored state against the
input prefix that is supposed to have produced it, and refuses the restore if the
two disagree. D3 — whether the engine computes longest remaining path — is
answered by measurement now, and the answer is no.

Phase 5's memory cost is measured, not estimated: the capacity regions needed
13,416 bytes and 2,264 were spare, so the 18th page was approved on measurement
(`docs/RESOURCE-ABI.md` §1) and linear memory is 18 pages. The certificate added
64 bytes of arena, so the arena is 75,912 and 53,720 bytes of address space are
still spare. The package sits at
105,344 bytes unpacked against a 106,565-byte ceiling — the frozen Phase 5
baseline plus a 12,000-byte Phase 6 envelope, owner review 2026-11-03. The
baseline rolls forward at each phase boundary to whatever a tool printed, which
is what makes the envelope mean something; `docs/SIZE-REPORT.md` records the
review history and the policy that replaced the ad-hoc raises.

Phase 4 (incremental planning) is **complete**: the engine can stop at a call
boundary, commit a checkpoint of the state it stopped in, and continue from that
checkpoint or refuse it; `planner.createSession()` keeps a plan alive through
appends, replacements and removals, and an incremental plan is proved equal to a
fresh rebuild by a randomised differential test. Phase 4 kept linear memory at
17 pages because the session state lives in the host.

Phase 1 (the dependency planner), Phase 2 (execution levels) and Phase 3 (the
critical path) are complete and measured: the engine, the package, the tests,
the gates and the benchmarks all exist and run.

Phase 3 added one versioned ABI export (`effectplan_plan_v3`, 18 parameters), two
output regions (512 bytes of durations in, 1,056 bytes of schedule out), the
additive `timing` option, and 20 Rust scheduling tests alongside a TypeScript
suite that grew from 122 to 156 — 20 timing tests, 7 critical-path properties and
8 raw-ABI scheduling checks — without touching the twelve- and fourteen-parameter
contracts, the 40,000-byte artifact budget or the 50,000-byte package limit that
was in force until the Phase 4 review.
Linear memory is 18 pages, and that is a build-time gate rather than a hope.

**The CI has run on a real GitHub runner.** Pull request #1 was the first
attempt, and it failed with 34 of 38 jobs green — on four defects in the CI
definition that no local run could have found, because each was only true on a
clean checkout: a type check that could not run without a package build, a test
job with no Rust toolchain, and two wasm jobs that staged the artifact without
building the package. All four are fixed; the second run passed 38 of 38
(`docs/IMPLEMENTATION.md`).

- the model and the arithmetic contract: [`docs/SCHEDULING-MODEL.md`](docs/SCHEDULING-MODEL.md)
- the byte-level ABI for all three phases: [`docs/ABI.md`](docs/ABI.md)
- the Phase 3 report, the memory decision and the cost analysis: [`docs/CRITICAL-PATH.md`](docs/CRITICAL-PATH.md)
- the Phase 2 specification, the ordering-invariant proof and its measured cost: [`docs/EXECUTION-LEVELS.md`](docs/EXECUTION-LEVELS.md)
- the Phase 5 model, the memory budget and the scheduler:
  [`docs/RESOURCE-MODEL.md`](docs/RESOURCE-MODEL.md),
  [`docs/RESOURCE-ABI.md`](docs/RESOURCE-ABI.md),
  [`docs/RESOURCE-SCHEDULING.md`](docs/RESOURCE-SCHEDULING.md)
- Phase 6, before any of it is built — what could be won, the objective that would
  judge it, and the memory it would cost:
  [`docs/OPTIMIZATION-BENCHMARKS.md`](docs/OPTIMIZATION-BENCHMARKS.md),
  [`docs/OPTIMIZATION-RFC.md`](docs/OPTIMIZATION-RFC.md),
  [`docs/OPTIMIZATION-STRATEGIES.md`](docs/OPTIMIZATION-STRATEGIES.md),
  [`docs/OPTIMIZATION-BUDGETS.md`](docs/OPTIMIZATION-BUDGETS.md)
- the rest: [`docs/IMPLEMENTATION.md`](docs/IMPLEMENTATION.md),
  [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md),
  [`docs/SIZE-REPORT.md`](docs/SIZE-REPORT.md)

Evidenced: CI on a real GitHub runner, 38 of 38 jobs green on
[run 41595818305](https://github.com/7d7s/axomarc/actions/runs/41595818305).

## License

MIT
