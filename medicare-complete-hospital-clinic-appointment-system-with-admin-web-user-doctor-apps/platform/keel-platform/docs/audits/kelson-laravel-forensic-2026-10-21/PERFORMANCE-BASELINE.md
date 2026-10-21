# PERFORMANCE BASELINE

**Status: NOT RUN — no performance claim is published in either direction.**

The audit's rule: *never publish a performance win without a comparable baseline and method* — and
*compare equivalent features and workloads; report hardware, OS, toolchain, concurrency, dataset,
warming, repetitions, confidence and raw output* ✓. This environment cannot meet that bar ✓, so the
audit publishes **nothing** ✓.

## 1. What was attempted

| Attempt | Command | Result |
|---|---|---|
| The coverage measurement (the whole workspace) | `cargo llvm-cov --workspace --locked --lcov --output-path lcov.info --summary-only` | **FAILED**: `LINK : fatal error LNK1318: Unexpected PDB error; LIMIT (12)` while linking `kelson-example-reference`. The cause was measured, not guessed: the instrumented build needs a **second** target tree (`target/llvm-cov-target`, 2.8 GB on top of the 10.6 GB `target/debug`), and the disk was at **0 bytes free** when the linker ran. This is the **same failure mode the CI disk check (F-16 / KFA-017) exists to catch** — the audit reproduced it independently, on a second target tree the check does not cover |
| The coverage measurement (a scoped subset) | `cargo llvm-cov -p kelson-policy -p kelson-context -p kelson-http-client -p kelson-testkit --summary-only` | **MEASURED**: 6,146 regions / 672 missed = **89.07 % region cover**, 581 functions / 80 missed = **86.23 % function cover**, 3,700 lines / 430 missed = **88.38 % line cover**. Raw: `RAW-EVIDENCE/coverage-summary.txt` |
| A load test | — | **not attempted** — no HTTP load tool (k6/wrk/vegeta) is present ✗ ✗ |
| A microbenchmark | — | **not attempted** — no `criterion` benchmark target exists in the tree ✗ ✗ |
| A comparable Laravel baseline | — | **not possible** — PHP and Composer are present ✓, but no Laravel application, no matching workload and no comparable host are available ✗ ✗ |
| A cold/warm startup measurement | — | **not possible** — the host cannot start ✗ (KFA-005 ✓) ✗ |

The one **quantitative** observation the audit does record is a *negative* one, and it is a
performance finding in its own right:

> The demo's boot took **~69.7 s** on this host before it aborted ✓ (the measured boot time in the
> instrumented run ✓), and the harness's own boot (the path the 118 lib + 102 HTTP cases use ✓) takes
> **~190 s** ✓ per suite ✗ ✗. The dominant cost is the **WASM JIT compilation of the guest module**
> ✓ (the "wasm plugin loaded" log is immediately followed by the abort ✓ and the long pause ✓) — the
> framework compiles guest artifacts **per process** ✓, so every test process and every worker
  restart pays it ✗ ✗.

**That is recorded as a risk, not a benchmark** ✗ — it is a single observation on one host, with no
repetitions and no comparison ✗ ✗.

**And one positive measurement** — the scoped coverage of the four crates the audit names most
heavily (`kelson-policy`, `kelson-context`, `kelson-http-client`, `kelson-testkit`):

| Metric | Covered | Missed | Cover |
|---|---|---|---|
| Regions | 6,146 | 672 | **89.07 %** |
| Functions | 581 | 80 | **86.23 %** |
| Lines | 3,700 | 430 | **88.38 %** |

**This is a partial baseline, not a workspace figure** ✓ — it covers 4 of the 84 members ✓ ✗, and it is
published here so that a future ratchet has a number to start from ✓ ✓ (KFA-018 ✓). It is **not**
evidence that the workspace is covered at 88 % ✗ ✗ — the demo, the console, the data layer and the
WASM crates were not in the measured set ✓ ✓.

**The full-workspace measurement is impossible on this host** ✓ — it needs a second instrumented
target tree ✓, and the disk budget was exhausted ✓ (0 bytes free ✓) when the linker ran ✓ ✓. The
measurement is a CI concern (a CI runner has the disk ✓ and the Linux stack ✓) ✓ ✓.

## 2. The performance-relevant findings (from the code, not from a measurement)

| # | Finding | Why it matters | Where |
|---|---|---|---|
| 1 | The **synchronous `StoragePort`** puts file IO on the async executor thread ✓ | under concurrent uploads the whole server's latency degrades ✗ | KFA-015 ✓ |
| 2 | The **guest artifact is compiled per process** ✓ (the JIT cost observed above ✓) | the cold start of every worker and every test process ✓ | the wasm crate ✓ |
| 3 | The **metric labels are unbounded** ✓ (no cardinality cap ✓) | an unbounded label set is a memory and a collector-cost hazard ✓ | LF-100 ✓ |
| 4 | The **telemetry buffers** are bounded by design ✓ (the ring buffer ✓) but the retention is unnamed ✗ | an operator cannot state the retention ✗ | LF-100 ✓ |
| 5 | The **query-count bound** is asserted ✓ (the eager-load case: 5 parents → 1 query ✓) and the **query budget** is a testkit feature ✓ — but there is no **request-cost** or **query-cost** limit in the runtime ✗ | a single expensive request is bounded only by the timeout ✗ | the audit's F-14 ✓ |

## 3. What a real performance baseline would require

1. A **host that boots** ✓ (KFA-005 ✓) — without it there is no runtime to measure ✗ ✗.
2. A **workload definition** ✓ (the request mix ✓, the data set ✓, the concurrency ✓, the warming ✓)
   and a **repeat count** with the raw output kept ✓ ✓.
3. A **comparable Laravel application** on the same host ✓, the same workload ✓ and the same data ✓,
   so the comparison is a comparison ✗ ✗.
4. The measurement harness itself as a tracked artefact ✓ (today there is none ✗) ✓.

## 4. The honest statement

**Kelson publishes no performance claim, and neither does this audit** ✓ ✓. The competitive
assessment's performance dimension is therefore **UNKNOWN** ✓ — not "equal", not "behind" ✗ ✗. The
one thing that can be said from the evidence is negative ✓: the per-process guest compilation is a
real cold-start cost ✓ (KFA-002's resource class and this section's observation ✓) ✗ ✗.
