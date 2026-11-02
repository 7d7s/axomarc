# Implementation

What exists in this repository, how it is built, and what has been measured.
Every number below was produced by the commands listed next to it.

## Repository layout

```
engine/                    Rust core (no_std, wasm32-unknown-unknown)
  src/lib.rs               the planner, the ABI, the arena layout
  src/levels.rs            the execution-level pass
  src/checkpoint.rs        the resumable-planning record, its digest and its checks
  src/schedule.rs          the critical path: two CPM passes, slack, concurrency
  src/tests.rs             50 host-side tests through the real ABI
  src/levels_tests.rs      21 level tests through the versioned ABI
  src/schedule_tests.rs    20 scheduling tests through the versioned ABI
  benches/native.rs        native benchmark, writes artifacts/native-bench.json
packages/core/             the npm package
  src/errors.ts            stable error surface
  src/loader.ts            instantiation and layout audit
  src/encode.ts            validation, interning, packing
  src/planner.ts           one ABI call per batch, output validation
  src/execution-levels.ts  read the levels region, group waves
  src/timing.ts            write durations, read the timing region, audit a schedule
  src/session.ts           the incremental session: changesets, versions, checkpoints
  test/                    unit, parity, property, ABI, level, timing, session and size tests
benchmarks/
  reference-planner.ts     independent O(N²) oracle (never published)
  reference-levels.ts      independent Kahn level oracle (never published)
  reference-cpm.ts         independent CPM oracle (never published)
  workload.ts              the shared benchmark workload (11 shapes)
  initial-benchmark.ts     the measurement harness and its gates
  incremental-benchmark.ts  an update against a full rebuild
scripts/                   build, verify, pack, filtered-test and browser-test tooling
test/browser/index.html    the browser smoke test
docs/                      ARCHITECTURE, IMPLEMENTATION, ABI, SCHEDULING-MODEL,
                           EXECUTION-LEVELS, CRITICAL-PATH, SIZE-REPORT,
                           INCREMENTAL-PLANNING, CHECKPOINT-ARCHITECTURE,
                           PLAN-VERSIONING, UPDATE-TRANSACTIONS,
                           INVALIDATION-MODEL, INCREMENTAL-BENCHMARKS,
                           INCREMENTAL-SECURITY
```

Artifacts that are committed as evidence: `artifacts/native-bench.json` (this
run's native numbers), `artifacts/benchmarks.json` (this run's wasm/API numbers),
`artifacts/benchmarks-phase2.json` (the Phase 2 report, kept as the recorded
baseline the overhead gate reads), and the size reports.

## Build pipeline

| Command                    | What it does                                                   |
| -------------------------- | -------------------------------------------------------------- |
| `npm run build:wasm`       | `cargo build --target wasm32-unknown-unknown --profile release`, then `wasm-opt --enable-bulk-memory -Oz`, staging `packages/core/dist/effectplan.wasm` |
| `npm run build:js`         | `tsc -p tsconfig.build.json` → `packages/core/dist`             |
| `npm run build`            | both, then `scripts/build-package.mjs`                          |
| `npm run verify:wasm`      | 93 ABI, level, scheduling and memory-safety checks against the built artifact |
| `npm run verify:size`      | extracts the packed tarball and enforces the size limits        |
| `npm run verify:package`   | installs the tarball into a throwaway project and plans a batch |
| `npm test`                 | compiles the test build and runs every TypeScript test          |
| `npm run test:timing`      | the timing suite on its own                                    |
| `npm run test:timing-properties` | the seven CPM properties, filtered from the property suite |
| `npm run test:timing-oracle` | exhaustive parity against `reference-cpm.ts`                 |
| `npm run test:timing-abi`  | the raw `v3` ABI suite                                         |
| `npm run test:timing-overflow` | zero, missing and overflowing durations                    |
| `npm run test:timing-memory` | arena tiling, view re-mapping, growth behaviour              |
| `npm run test:api-compatibility` | Phase 1 and Phase 2 surfaces, unchanged behaviour        |
| `npm run test:browser`     | serves the page and drives it in headless Chrome                |
| `npm run benchmark`        | native benchmark, then the WebAssembly/TypeScript benchmark     |
| `npm run benchmark:incremental` | an update against a full rebuild, with a gate          |
| `npm run test:session`      | the incremental session suite                                 |
| `npm run test:session-properties` | the randomised differential test                        |
| `npm run ci:check`          | the repository-root workflow matches the project's            |
| `npm run measure:profiles` | rebuilds the wasm under each cargo profile and measures it      |
| `npm run test:levels`      | the execution-levels suite on its own                          |

The filtered jobs go through `scripts/run-tests.mjs`, which fails when a pattern
matches fewer tests than the job claims to cover: a filtered job that silently runs
nothing is not a job.

Cargo profiles live in the workspace root `Cargo.toml`: Cargo ignores profile
sections in a member package, so a profile declared in `engine/Cargo.toml` would
silently ship the wrong code generation settings.

## The engine

`engine/src/lib.rs` is `#![no_std]` for the wasm build (the attribute is
compile-enforced, so an accidental use of the standard library fails the
build). Host builds enable `std` only so `cargo test` can link its harness
against the same code.

Required engine functions, all present. Phase 2 added `levels.rs` and the
versioned entry point, Phase 3 added `schedule.rs` and `effectplan_plan_v3`;
nothing in the Phase 1 list moved or changed meaning:

| Function                | Role                                                        |
| ----------------------- | ----------------------------------------------------------- |
| `validate_input`        | ABI revision, count ceilings, capacities, overflow checks   |
| `validate_offsets`      | CSR offsets anchored at zero, monotonic, terminating exactly |
| `validate_accesses`     | effect modes, resource indices, access modes                |
| `initialize_workspace`  | resets the per-batch state                                   |
| `process_read`          | read semantics: wait for the writer, join the reader list   |
| `process_write`         | write semantics: wait for writer and readers, take ownership|
| `emit_edge`             | appends a record, refusing to truncate silently             |
| `plan_batch`            | one pass over the calls in input order                       |
| `validate_output`       | re-reads the graph and refuses anything structurally invalid |
| `compute_execution_levels`| one forward pass over the emitted edges, then self-audit    |
| `validate_durations`    | every call carries a positive tick count                     |
| `compute_earliest_times`| forward pass over the records, ascending                     |
| `compute_project_duration`| makespan fold and the `u64` work fold                      |
| `compute_latest_times`  | backward pass over the same records, descending              |
| `compute_event_slack`   | `LF − d − ES`, refused if negative                           |
| `identify_critical_events`| the same fold counts the zero-slack calls                  |
| `compute_peak_concurrency`| half-open endpoint sweep, heap-sorted, no allocation      |
| `validate_schedule_output`| precedence audit carried by the backward pass            |
| `write_schedule_output` | rows first, header last, as a commit record                  |

The arena layout is owned by Rust and const-asserted: the regions are
contiguous, four-byte aligned, and the assertions fail at compile time if that
ever stops being true. `effectplan_info` publishes the layout so the host
cannot drift from it. Phase 2 appended one region — `levels`, 260 bytes; Phase 3
appended two — `durations`, 512 bytes, and `timing`, 1,056 bytes — and the arena
is still contiguous at 61,888 bytes with linear memory at 17 pages. The timing
region is the tail; `verify:wasm` asserts every base, length, alignment and the
page count, so a layout change fails the build.

`effectplan_plan` keeps its twelve parameters and its behaviour;
`effectplan_plan_v2` adds `levels_ptr` and `levels_len`; `effectplan_plan_v3`
adds `durations_ptr`, `durations_len`, `timing_ptr`, `timing_len`. The new
pointers are validated exactly like every other one: the address must convert to
an arena-relative offset and equal the canonical base, and the length must equal
the canonical capacity. `v3` then runs the `v2` body and the scheduling pass, so
the graph cannot differ between entry points. Five status codes were added across
Phases 2 and 3 (116 and 117 for levels; 118, 119 and 121–122 for timing); no
existing code changed meaning.

## The TypeScript wrapper

Six modules, one responsibility each, and no second implementation of the
conflict or the scheduling algorithm:

- `errors.ts` — the stable codes (`EP_INVALID_INPUT`, `EP_DUPLICATE_CALL_ID`,
  `EP_INVALID_EFFECT`, `EP_INVALID_RESOURCE`, `EP_UNKNOWN_EFFECT_REJECTED`,
  `EP_LIMIT_EXCEEDED`, `EP_INVALID_PLAN`, `EP_ENGINE_UNAVAILABLE`,
  `EP_ENGINE_ERROR`) and the numeric-status → code mapping.
- `loader.ts` — obtains the bytes (explicit bytes, an explicit URL, or the
  packaged file), compiles and instantiates without a binding framework, then
  audits the layout the engine published: the arena must be inside the live
  buffer, the region bases must be aligned and contiguous, and the output
  region must match the edge bound.
- `encode.ts` — validates the public contract, interns resources, normalises
  duplicate accesses (write wins over read), sorts each call's accesses by
  resource id, and writes the four packed arrays.
- `planner.ts` — one ABI call per batch, then a full re-validation of the
  graph before it is handed out. The plan handle owns a private copy, so the
  engine's output region can be reused immediately.
- `execution-levels.ts` — reads the `u32` count and the `u16` array the engine
  wrote, refuses a count that is not the batch size, a level that reaches
  outside the batch, or a hole in the waves, and groups the calls. It computes
  no dependency and no level: grouping is the only arithmetic in this module.
- `timing.ts` — validates the caller's durations and writes them into the
  canonical region, then reads the timing region back and refuses a schedule that
  contradicts itself: an unexpected revision or count, a set reserved word, a
  `u64` work value outside the safe-integer range, `peak > callCount`, a negative
  slack, a latest start past the makespan, an edge that violates either precedence
  direction, a finish later than the makespan, or a total work value that is not
  the sum of the durations the caller supplied. It derives `EF`, `LS` and `LF`
  from `ES`, `slack` and the duration — names, not new measurements — and computes
  no dependency, no longest path and no criticality beyond `slack === 0`.

Two details that were measured rather than assumed:

- **Detached buffers.** `memory.grow(n)` replaces `memory.buffer` and detaches
  the old one. A write through a stale view is *silently dropped* rather than
  throwing, so a wrapper that keeps its views would plan a batch the engine
  never received. `planBatch` compares the buffer identity before every write
  and re-maps when it changed. The integration test asserts exactly this.
- **Error messages.** Messages carry the call's index, never the caller's
  resource ids or call ids. There is a test that feeds in a tenant name, a
  file name and a call id and asserts that none of them appear in the error —
  and the same assertion for a malformed levels region.
- **The ordering invariant.** Before any level code was written, the forward-edge
  property was verified three ways: by construction (`process_read` and
  `process_write` only ever relate a call to an earlier one), at runtime
  (`validate_output` refuses `pred >= succ`), and empirically
  (`every_edge_points_forward`, plus a per-dataset assertion in the benchmark).
  The invariant held, so the engine uses the single forward pass; if it had not,
  the fallback was a general Kahn traversal.

## Testing

### Rust — 110 tests, all through the real ABI

`cargo test -p effectplan-engine` — 110 passed, 0 failed: the 50 Phase 1 tests,
21 execution-level tests in `levels_tests.rs`, 20 scheduling tests in
`schedule_tests.rs` and 19 checkpoint tests in `checkpoint_tests.rs`, all driven
through `effectplan_plan_v2`, `effectplan_plan_v3` and `effectplan_plan_v4`.

The categories the brief asks for: input validation, offset validation, access
validation, workspace initialisation, read processing, write processing, edge
emission, batch planning, output validation, the conflict matrix
(read/read, read/write, write/read, write/write), independent resources,
unknown effects, exclusive effects, determinism, error codes and layout
invariants. The level tests add: empty graph, one call, eight independent
calls, a sequential chain, independent branches, a diamond, multiple
predecessors, disconnected components, barrier waves, unknown/exclusive
preservation, duplicate dependencies, the maximum batch, the deepest chain
(level 127), a refused backward record, refused non-canonical levels pointers
and lengths, and the guarantee that a version one call leaves the levels region
untouched. The scheduling tests add: empty plan, single call, a chain that sums,
independent calls that take the longest not the sum, the worked diamond, a
permuted duration vector, multiple critical paths, disconnected components under
one makespan, adjacent (half-open) intervals versus overlapping ones, the maximum
batch, large durations staying exact, tick overflow reported never wrapped, zero
and missing durations refused, a refused schedule writing no header, byte-for-byte
determinism, non-canonical and wrong-length scheduling regions (108, 119), and the
guarantee that `v1` and `v2` never write the timing region. The tests share one
static arena, so all three test modules serialise on one shared mutex.

### TypeScript — 179 tests, all passing

`npm test` — 179 declared tests, 0 failed. The runner's tally reads `# pass 180`
because it also loads `test/support.ts`'s compiled module as a file, which
declares no tests. The package-size suite is green against the ceiling that
`scripts/limits.mjs` states (70,000 bytes unpacked, reviewed in
`docs/SIZE-REPORT.md`).

| File                         | Tests | Covers                                                       |
| ---------------------------- | ----- | ------------------------------------------------------------ |
| `api.test.ts`                | 34    | metadata, limits, layout, the error matrix, loader refusals  |
| `planner.test.ts`   | 22    | conflict matrix, effect modes, encoding, decoding, determinism  |
| `parity.test.ts`    | 4     | exhaustive parity with the independent oracles                    |
| `property.test.ts`  | 22    | fast-check properties: 8 graph, 7 level, 7 critical path          |
| `wasm.test.ts`      | 40    | the raw ABI, hostile arguments, memory growth, the scheduling and checkpoint ABI |
| `execution-levels.test.ts` | 17 | decoding, grouping, oracle parity, immutability             |
| `timing.test.ts`    | 20    | duration validation, the worked examples, oracle agreement, overflow, half-open boundaries, growth, no timing when not asked |
| `session.test.ts`       | 14    | the incremental session: parity, versions, atomicity         |
| `session-property.test.ts` | 2   | randomised edits against a fresh rebuild                     |
| `package-size.test.ts`   | 4     | the packed artifact against the release limits               |

The loader refusals are tested against hand-assembled wasm modules (the bytes
are built in the test, so the layout is auditable): a wrong ABI revision, a
missing export, a missing memory, and a corrupted binary.

### Parity with an independent oracle

`benchmarks/reference-planner.ts` is a deliberately slow all-pairs
implementation of the documented conflict matrix. It shares no algorithmic
structure with the engine: no resource-indexed state, no streaming, just every
pair of calls tested against the matrix. The engine emits a sparse covering
graph; the oracle emits every conflicting pair, which is the transitive
closure of that graph. Parity is therefore checked as closure equality plus a
soundness subset check, not as raw edge-set equality.

- Exhaustive: every batch of up to three calls over a nine-element shape
  alphabet (820 batches), plus every batch of four calls (6,561 batches).
- Property: 400 runs of fifteen properties — determinism, acyclicity,
  forward-only edges, the `2·(N+A)` edge bound, read-only independence,
  disjoint resources, barrier semantics, closure equality with the oracle, plus
  seven level properties: every edge rises, roots sit at zero, a non-root equals
  one over its deepest predecessor, the waves partition the batch, the wave sizes
  sum to the call count, levels stay inside the batch, and the forward pass
  equals the independent Kahn result.

Zero disagreements.

### Level parity with a second algorithm

`benchmarks/reference-levels.ts` computes levels the way the planning document
proposed for the general case: a Kahn layered traversal over the full all-pairs
conflict set — queue, indegree array, no assumption about input order. The
production engine uses the single forward pass over its covering graph. Two
independent methods, compared per node rather than by wave count, on 7,381
exhaustive batches, 400 randomised property runs, all 11 benchmark datasets and
every curated shape. Zero disagreements, which is the empirical answer to the
question "does the sparse graph carry the same longest-path information as the
full closure" — it does, because every conflicting pair has a path between it in
the covering graph.

### Timing parity with a third algorithm

`benchmarks/reference-cpm.ts` computes the schedule the way a textbook would:
explicit predecessor and successor adjacency lists built from the full all-pairs
conflict set, a Kahn order for the forward pass, a reverse-topological backward
pass taking successor minima, and peak concurrency by counting overlaps per start
time. The production engine has none of that machinery — it walks one record array
in each direction and sweeps sorted endpoints.

Parity is checked per call, not by summary: makespan, total work, peak concurrency,
every earliest start, every slack and every critical flag against the oracle's own
arrays, on 7,381 exhaustive batches (in `parity.test.ts`), 7 property-driven runs
per 400 randomised batches (in `property.test.ts`), the curated shapes in
`timing.test.ts` and all 11 benchmark datasets in the harness. Zero disagreements.

The harness compares the engine against the oracle **twice**: once on the engine's
covering graph and once on the oracle's full pair set. Longest-path timing is
identical on both graphs, and that equality is the empirical proof that the covering
graph preserves every path length — the claim both CPM passes rest on. The same
check also compares the native build against the wasm build on every dataset, for
edges, waves, makespan, peak and critical count; that guard is what caught a
workload shape which repeated a resource inside one call.

### WebAssembly integration

`npm run test:wasm` — 33 passed. The artifact loads through the published
entry point with no options; it has no imports and exactly four function
exports; it contains no allocator or binding-framework strings; the raw ABI
executes; the packed typed arrays the wrapper writes are the ones the engine
reads (the raw ABI and the public API agree record for record); a wrong ABI
revision, malformed offsets, out-of-bounds pointers, a short workspace and a
short output are all refused with status codes instead of trapping; a host
that grows the memory is survivable; and 50,000 randomised hostile argument
sets never trap, always return a documented status, and leave no residue — a
canonical call after them still produces the same bytes. The level suite checks
the versioned ABI directly: a chain, an independent pair, a diamond, a barrier,
an empty batch, repeated batches of different sizes, a non-canonical levels
pointer (108), a wrong levels length (116), a below-arena pointer (104), the
untouched levels region after a version one call, and levels read correctly
after the host grows the memory. The scheduling suite drives `v3` the same way:
the timing contract published through `info`, no timing written by `v1` or `v2`
(checked with a sentinel left in the region), the worked diamond through raw
bytes, a zero duration (118), a tick total that will not fit (121), non-canonical
scheduling regions (108) and wrong lengths (119), identical bytes across repeated
batches, and 20,000 hostile scheduling argument sets that never trap and always
return a documented status.

### Browser

`npm run test:browser` — one page, no framework and no UI, served over HTTP
and opened in headless Chrome. The page loads the published package, plans a
batch, checks the graph, the levels revision, `plan.execution` against a
three-wave expectation, `plan.timing` against the chain expectation (makespan 10,
total work 10, peak 1, all three calls critical), that a second identical call
gives identical bytes and that a batch planned without durations reports no
timing at all. It fails the build when no browser is
installed, because a silently skipped browser test is not a test.

## CI

`.github/workflows/ci.yml` runs thirty jobs: the ten Phase 1 and Phase 2 jobs
(Rust formatting, Rust tests, TypeScript type check, TypeScript tests,
WebAssembly build, ABI verification, WebAssembly integration tests, package-size
enforcement, package installation smoke test, browser smoke test), the nine Phase 3
jobs — Rust scheduling tests, scheduling properties, the independent CPM oracle,
the scheduling ABI, duration and tick overflow, the timing memory layout, the
benchmark gates, the browser scheduling smoke test and Phase 1/2 API
compatibility — and eleven Phase 4 jobs: the definition check that the
repository-root workflow mirror matches this file, the Rust checkpoint tests, the
checkpoint ABI against the shipped artifact, incremental parity, version
integrity, transaction safety, change validation, memory validation, the
incremental WebAssembly integration and browser runs, and the incremental
benchmark gate. Jobs that select a subset by name — the Rust scheduling and
checkpoint jobs through their test-filter guards, and every
`--test-name-pattern` job through `scripts/run-tests.mjs` — fail when they match
fewer tests than they claim, so a renamed test breaks a job instead of quietly
emptying it. The wasm-build job still enforces the 40,000-byte artifact budget and
the size job the package ceiling, both read from `scripts/limits.mjs` (70,000
bytes unpacked as reviewed in `docs/SIZE-REPORT.md`).

Every one of those commands has run and passed locally (2026-11-02: Rust format,
110 Rust tests, the checkpoint and scheduling filters, typecheck, 179 TypeScript
tests, the wasm build and its 110 ABI checks, size, package install, browser, the
session and timing subsets — 20 invocations, all exit 0). What has not happened is
a GitHub runner: the branch is pushed, but the mirror workflow triggers on
`pull_request` and on pushes to `main`, so no Actions run exists until a pull
request is opened against it. That is the one piece of Phase 4 evidence this
repository cannot produce on its own.

## Level-pass cost and the 10% target

The planning document set a target of no more than 10% p50 regression on the
normal and medium datasets. The measured paired numbers exceed it slightly. The
decision was to ship the feature and document the cost, not to move the target.

Paired measurements from one process on one machine — the only fair comparison,
because the unchanged version one path moved by up to ±30% between sessions on
this laptop, so session-to-session numbers are not a baseline. The harness also
refuses to publish a report when the native and wasm sides disagree on a
dataset's call, access, edge or wave counts; that guard is what caught a workload
shape which repeated a resource inside one call.

| Dataset | wasm plan p50 | wasm plan + levels p50 | added |
| ------- | ------------- | -------------------- | ----- |
| tiny    | 12.5 µs       | 13.9 µs              | +1.4 µs (11.2%) |
| small   | 34.9 µs       | 39.8 µs              | +4.9 µs (14.0%) |
| normal  | 64.4 µs       | 74.1 µs              | +9.7 µs (15.1%) |
| medium  | 344.2 µs      | 386.8 µs             | +42.6 µs (12.4%) |

Across the seven Phase 2 shapes the same pairing runs from +3.5% (wide, which has
no edges at all) to +18.5% (deep, one edge per level). The native numbers show
the same shape: `plan + levels` minus `plan` matches the separately measured
level pass (0.1–17.2 µs), a single linear walk of the emitted records plus three
validation loops.

Engineering decision: accept the cost.

1. The alternative is not free. A host that layers the batch itself must build
   the conflict closure and run a Kahn traversal: 75–1,796 µs for these datasets
   in the reference implementation, against 0.5–42.6 µs for the wasm level pass,
   for the same answer the engine already has.
2. The cost is O(E) with a small constant, no allocation, and no new sort.
3. Two of the three passes over the records are deliberate redundancy: the
   engine re-reads the graph it just wrote to prove the levels explain it. They
   could be folded into one walk for a few microseconds; the redundancy stayed,
   because a wrong wave is worse than a slow one.

## Scheduling-pass cost and the 20% target

The Phase 3 planning document targets no more than 20% p50 overhead over the
Phase 2 full API, on the normal and medium datasets. Measured, paired in one
process on the published artifact:

| Dataset | wasm `v2` p50 | wasm `v3` p50 | engine added | API without timing | API with timing | API overhead |
| ------- | ------------- | ------------- | ------------ | ------------------ | --------------- | ------------ |
| tiny    | 14.3 µs       | 15.8 µs       | +1.5 µs (10.5%) | 25.7 µs | 28.8 µs | +12.1% |
| small   | 42.2 µs       | 46.6 µs       | +4.4 µs (10.4%) | 79.0 µs | 89.2 µs | +12.9% |
| normal  | 77.0 µs       | 87.8 µs       | +10.8 µs (14.0%) | 148.2 µs | 168.1 µs | **+13.4%** |
| medium  | 385.5 µs      | 417.7 µs      | +32.2 µs (8.4%) | 641.7 µs | 696.7 µs | **+8.6%** |

The two gated numbers pass, and so do their engine counterparts (+14.0% and
+8.4%). The harness gates only what the target names — normal and medium — and
reports the rest, because inventing pass/fail for shapes the document did not name
would be measuring a different claim. API overhead on the untargeted shapes runs
from +7.6% (maxbound) to +33.0% (deep, 64 calls with almost no dependencies, where
the per-call rows dominate the two edge passes).

The first implementation of this section **failed** its own gate: normal +26.6%
engine and +37.1% API. Two changes fixed it, both recorded in
`docs/CRITICAL-PATH.md` §5:

1. Reading each 16-byte record with one bounds check (`chunks_exact`) instead of
   four `rd_u32` calls, each with its own range check and sentinel branch. The
   isolated native timing pass went from 13.3 µs to 1.9 µs on normal and from
   56.5 µs to 6.0 µs on medium.
2. Carrying the precedence audit inside the backward pass instead of walking the
   records a third time, and handing the `latest`-direction check to the host,
   where it is not a tautology over a minimum fold. On the API side, `decodeTiming`
   now reads the engine's edge buffer directly rather than receiving one frozen
   object per dependency — 288 allocations on the normal shape, purely to audit
   them.

Nothing about the algorithm changed: still two record walks and one endpoint sort.
The cost that breached the gate was an implementation choice, and the gate is what
found it.

Against the recorded Phase 2 report (`artifacts/benchmarks-phase2.json`, Node
v20.15.0, same machine, different session) the timed API runs +6.7% to +31.6%.
Those comparisons are printed as notes, never gated: in the same session the
*untouched* Phase 2 path measured −1.7% to +6.6% against its own recorded numbers,
and one early Phase 3 run put it at +87.6% on the tiny shape while a parallel
`cargo` build was running. Cross-process numbers on this laptop are context; the
paired in-process ratios are the evidence.

Two anomalies worth naming rather than burying, from three runs of identical
code: `deep` measured 23.0 µs for `v2` and 50.5 µs for `v3` in the committed run
(+119.6% at the engine) where the previous run measured 22.9 µs and 30.8 µs
(+34.5%), and `mixed` once measured 165.5 µs for `v2` and 129.1 µs for `v3` —
`v3` apparently 22% *faster* than `v2`, which is impossible and therefore noise.
Adjacent timing blocks in one V8 process are not guaranteed to be tiered
identically, and the native isolated pass for both shapes is 1.5–2.8 µs. The
gated shapes were measured four times in this phase and passed every time; the
harness fails closed on a breach, so a badly tiered block cannot hide behind an
average.

## Deviations from the brief

- **npm, not pnpm.** The brief mentions a `pnpm-lock.yaml`; the environment
  has npm 10 and a `package-lock.json`, and adding a second package manager
  would add a dependency for no gain. Nothing in the design depends on the
  choice.
- **`wasm-opt` is optional in the script, and present in the build.**
  `scripts/build-wasm.mjs` applies `wasm-opt --enable-bulk-memory -Oz` when a
  binary is reachable and falls back with a printed note when it is not.
  `binaryen` is a devDependency, so the shipped artifact is the optimised one
  (13,742 bytes from cargo, 10,222 after). The flag is not decoration: rustc emits
  bulk-memory `memory.fill`, and `wasm-opt` refuses to optimise a module that uses
  it unless the feature is enabled.
- **`decodePlan` is exported from `planner.ts`** so the benchmark can measure
  the decode stage on its own. `index.ts` does not re-export it, so it is not
  part of the public surface.
- **Levels and timing are per call, not per event.** The planning documents sized
  the levels buffer from `MAX_EVENTS` — 2,304 bytes, maximum level 1,151 — and the
  timing discussion assumed per-event arrays. The engine's graph nodes are calls;
  its internal event list never reaches the output. The regions are 260, 512 and
  1,056 bytes and the maximum level is 127. This was proved before the ABI was
  touched (`docs/CRITICAL-PATH.md` §2), and the API names the mapping directly:
  `levelByCall` and `events[i].callIndex`, indexed exactly like `edge.from` and
  `edge.to`.
- **`levelByCall`, not `levelByEvent`.** Same reason — the name has to describe
  the real index space, which is the batch position.
- **A separate `effectplan_plan_v2` export rather than a widened v1, and a
  separate `v3` rather than a widened v2.** The twelve- and fourteen-parameter
  ABIs are published contracts. The extended entry points sit beside them, and the
  wrapper refuses to load an engine without them rather than silently returning
  plans without levels or without timing.
- **The schedule audit moved.** `validate_schedule_output` was specified as its own
  pass; the precedence check now rides in the backward pass, and the
  `latest`-direction check was dropped from the engine because `latest_finish` is
  *defined* as the minimum over exactly that term, which makes the in-engine test
  unbreakable and therefore meaningless. It is still checked on the host, on final
  values, where it can name the failing edge. §5 of `docs/CRITICAL-PATH.md` has the
  measurement that forced the change.
- **`decodeTiming` takes the edge buffer, not edge objects.** Handing it decoded
  edges cost one allocation and one freeze per dependency purely so the host could
  audit them; it now reads four words per record out of the `Int32Array` the plan
  already owns.
- **Both readings of the specification's worked example are implemented.** The
  prose gives the example a makespan of 10 with the path `A -> B -> D`, which
  requires `E` not to be downstream of `D`; the shape the tests had been written
  against gives 14. Two tests, one document section, no silent choice
  (`docs/SCHEDULING-MODEL.md` §7).
- **`unit` is metadata, never a conversion.** A duration is a tick count and the
  engine does not look at the caller's unit string beyond its shape. Converting
  milliseconds to ticks is the caller's decision, and no rounding error should
  come from this library.
