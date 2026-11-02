# Size report

The limits are release constraints, and a limit that moves needs a record of why
it moved. One has moved, twice, both times by owner review on 2026-11-01:

* **50,000 → 60,000**, before any of the incremental-planning TypeScript existed.
  That number came from a projection: 51,791 bytes with the resumable engine and
  no session layer, plus an estimated session module.
* **60,000 → 70,000**, once the session layer existed and had been measured. It
  came to 12,813 bytes rather than the ~5,000 the projection had allowed for, and
  the measured total was 68,645. The second review was against the measurement.

Everything else below is still the Phase 1 brief: the 40,000-byte artifact, zero
runtime dependencies, zero Rust heap allocation, and 17 pages of linear memory —
Phase 4 was designed around that page count rather than against it, which is why
session state lives in the host.

| Constraint                        | Limit    | Measured | Headroom |
| --------------------------------- | -------- | -------- | -------- |
| npm package, unpacked             | 70,000 B | 68,645 B | 1,355 B  |
| WebAssembly artifact              | 40,000 B | 13,499 B | 26,501 B |
| WebAssembly linear memory         | 17 pages | 17 pages | 2,328 B of spare address space |

Where the 16,854 bytes since Phase 3 went: the session module and its
declarations are 12,813 of them, the resumable engine is 3,286, and the loader,
encoder and planner grew by 1,900 between them. The headroom is deliberately
thin — the next feature that needs room will need another review, against a
measurement.

Phase 1 shipped at 36,891 / 7,948, Phase 2 at 43,543 / 10,190 and Phase 3 at
51,791 / 13,247; the 16,715-byte increase since is the incremental-planning
layer, measured from the same packer.

## How the package was measured

`npm pack --dry-run --json` reports the file list and the unpacked size that
npm itself would produce. `scripts/verify-size.mjs` then extracts the tarball
npm actually wrote and sums the bytes on disk. The two numbers have to agree
before the gate passes, so neither measurement can drift on its own:

```
verify-size: measured from the packed tarball
  unpacked   48766 bytes (npm reported 48766)
  tarball    17185 bytes
  wasm       10222 bytes
  files      18
```

`packages/core/test/package-size.test.ts` measures the same artifact through
`scripts/build-package.mjs --dry-run`, which enforces both limits before it will
print a manifest at all, so the test cannot pass on a hand-written number. No
number in this document is derived from source file sizes.

## What is in the package

| File                         | Bytes  |                             |
| ---------------------------- | ------ | --------------------------- |
| `dist/effectplan.wasm`       | 13,499 | the engine                  |
| `dist/loader.js`             |  7,068 | layout audit, four plans    |
| `dist/timing.js`             |  6,098 | new in Phase 3              |
| `dist/session.js`             | 10,764 | new in Phase 4              |
| `dist/encode.js`             |  5,265 |                             |
| `dist/planner.js`            |  4,806 |                             |
| `README.md`                  |  4,137 | package documentation       |
| `dist/loader.d.ts`           |  2,229 |                             |
| `dist/encode.d.ts`           |  1,729 |                             |
| `dist/planner.d.ts`          |  1,295 |                             |
| `dist/execution-levels.js`   |  1,125 |                             |
| `dist/timing.d.ts`           |  1,095 | new in Phase 3              |
| `dist/session.d.ts`           |  2,049 | new in Phase 4              |
| `LICENSE`                    |  1,068 |                             |
| `dist/errors.d.ts`           |    895 |                             |
| `dist/errors.js`             |    797 |                             |
| `package.json`               |    722 |                             |
| `dist/index.d.ts`            |    632 |                             |
| `dist/execution-levels.d.ts` |    318 |                             |
| `dist/index.js`              |     90 | re-export only              |
| **total**                    | **68,645** | 20 files                |

The tarball is 22,463 bytes. The package declares no runtime dependencies, and
the installation smoke test confirms that installing it produces a tree with
exactly one package in it.

## How the Phase 3 size was paid for

`scripts/build-package.mjs` stages the package into `artifacts/staging` and packs
from there, so the repository's own sources are never rewritten. In staging every
`.js` and `.d.ts` line is trimmed of indentation and blank lines are dropped:

| Measure | Bytes |
| --- | --- |
| `dist/` as `tsc` and cargo emitted it | 51,136 |
| whitespace transform | −5,272 |
| staged `dist/` (the manifest's `stagedBytes`) | 45,864 |
| plus `README.md` 4,137, `LICENSE` 1,068, `package.json` 722 | **51,791** |

The transform is whitespace only — no minifier, no mangling, no bundling, no
sourcemap — so the published JavaScript is still readable inside the tarball and
the `.d.ts` files still carry the API's types. `tsc` runs with `removeComments:
true`, which is why prose in the source files never costs package bytes. The
packager prints the removed byte count on every run and `--dry-run` refuses to
produce a manifest over the limit, so the reduction is visible and auditable
rather than a silent shrink.

The 1,234 bytes of headroom are *after* that. If a future change adds 2,000 bytes
of real code, the gate fails and something has to be deleted — which is the
intended behaviour.

## Cargo profile comparison

The shipped binary is built with the size-tuned `release` profile and then
`wasm-opt`. The alternatives were built and measured, not estimated
(`npm run measure:profiles` → `artifacts/profile-sizes.json`, 2026-11-01T09:18Z):

| Profile     | `opt-level` | Bytes (Phase 4) | Bytes (Phase 3) | Bytes (Phase 2) | Note                 |
| ----------- | ----------- | --------------- | --------------- | --------------- | -------------------- |
| `release`   | `z`         | 13,247          | 10,222          | 10,190          | shipped              |
| `release-s` | `s`         | 13,072          | 10,743          | 11,933          | **175 smaller than shipped** |
| `release-3` | `3`         | 16,680          | 10,625          | 12,572          | +3,433 over shipped  |

The Phase 2 column was measured *without* `wasm-opt` (Phase 1 was 7,948 on the
same basis), so the three columns are not one series. Two things changed with the
resumable engine: the checkpoint digest, its validation and the fifth export cost
3,025 bytes, and the size ordering of the profiles *inverted* — `opt-level = "s"`
is now 175 bytes smaller than the shipped `"z"`, where it had been 521 bytes
larger. `z` still ships because every benchmark number in this repository was
produced on it and a profile switch would silently invalidate that evidence; with
26,753 bytes of headroom on the artifact limit there is no pressure to trade
correctness of comparison for 175 bytes. If Phase 5 needs the space, the switch is
measured, not assumed.

`wasm-opt` needs `--enable-bulk-memory` here: rustc emits `memory.fill` for the
arena's zero-initialisation, and without the flag the validator rejects the module
("memory.fill operations require bulk memory") and the pipeline falls back to the
cargo output with a printed note. `binaryen` is a devDependency, so CI gets the
optimised artifact from `npm ci`. On a machine without it the build still ships, at
13,742 bytes — inside the 40,000-byte limit, so the size gate holds either way.

## What the binary contains

- No imports at all: the module declares no host functions.
- Four exports: `effectplan_info`, `effectplan_plan`, `effectplan_plan_v2` and
  `effectplan_plan_v3`.
- No allocator, no `wasm-bindgen`, no `serde`, no panic formatting — the
  strings `wasm_bindgen`, `__wbindgen`, `__rust_alloc`, `panicked at` and
  `capacity overflow` do not appear in the binary.
- The arena — including the new 512-byte durations region and the 1,056-byte
  timing region — is a statically reserved zero-initialised array, so it is
  emitted as BSS and costs nothing in the file. Linear memory at run time is
  1,114,112 bytes: 17 pages. That is measured from the artifact *and* from a live
  instantiation (`npm run verify:wasm` asserts both), not inferred from the arena
  arithmetic, which is the step the planning document warned about.
- The 3,025 bytes of file growth between Phase 3 and Phase 4 are the resumable
  machinery: the checkpoint record, the digest that binds it to an input prefix,
  the restored-state validator, the `v3`/`v2`/`v1` resume twins, the fifth export
  and its twenty argument conversions. The new region is 32 bytes of BSS, which
  costs nothing in the file — the pages come from the arena arithmetic, not the
  bytes on disk.

## Phase 3 growth, itemised

Against the committed Phase 2 manifest (`git show
e35380a:./artifacts/package/package-manifest.json`), reconciled exactly:

| Change | Bytes |
| --- | --- |
| wasm artifact (scheduling pass, `v3` export, seven selectors) | +32 |
| `dist/timing.js` + `dist/timing.d.ts` | +7,193 |
| `dist/loader.js` + `.d.ts` (audit durations and timing, require `v3`) | +1,214 |
| `dist/encode.*`, `dist/planner.*`, `dist/errors.*`, `dist/index.d.ts`, `dist/execution-levels.*` | +1,676 |
| whitespace transform, applied across all of the above | −5,272 |
| `README.md` rewritten shorter | −1,728 |
| **package total** | **+5,223** |

Read as "what the feature cost before any size work": the engine grew by 32 bytes
and the TypeScript grew by 12,223 bytes measured the way Phase 2 was measured
(uncompacted `dist/`: 35,888 → 48,111). The transform paid back 5,272 of that and
the documentation trim paid back 1,728, so what shipped is +5,223. Both figures are
in this file because the second one flatters the phase more than the first.

## The gates

- `npm run verify:size` — fails if the unpacked package exceeds its declared
  limit, if the wasm exceeds 40,000 bytes, if the package carries anything other than
  exactly one wasm binary, or if the installed manifest declares runtime
  dependencies.
- `packages/core/test/package-size.test.ts` — the same limits as a test, and a
  check that every module the entry point imports is actually published.
- `npm run verify:wasm` — 110 checks, including the arena tiling, each region's
  published capacity and that linear memory is still 17 pages. This is the
  build-time memory-layout acceptance gate the Phase 3 planning document asked
  for: a layout change fails the build instead of surfacing later as a wrong plan.
- `scripts/build-package.mjs --dry-run` — enforces both limits before writing a
  manifest, so the packer itself cannot produce an oversized artifact.
- `npm run test:timing-memory` — the arena, view re-mapping and memory-growth
  behaviour as tests.
- CI jobs "Package size enforcement" and "Timing memory layout" run the gates on
  every pull request.

## One shared source for the numbers

`scripts/limits.mjs` holds `MAX_UNPACKED_BYTES`, `MAX_WASM_BYTES`, `MAX_PAGES` and
`PREVIOUS_MAX_UNPACKED_BYTES`, and is imported by the packager and the verifier, so
the two tools cannot enforce different rules. `package-size.test.ts` states the
policy independently and asserts that the manifest the packager emits declares
exactly those numbers — a silent edit to a script fails a test rather than quietly
widening the budget. The wasm-opt flags were measured, not assumed: `-Oz`,
`-Oz --closed-world` and `-Oz --closed-world --shrink-level=2` all produce 13,247
bytes, so there is no flag-driven saving left to make.

## Version

The workspace and the package are at 0.3.0: Phase 1 was 0.1.0, Phase 2 0.2.0. The
bump is a feature addition with nothing broken — `abiVersion` is still 1, the
levels revision is still 1, the timing revision is 1, and both older entry points
are byte-for-byte untouched.
