# Application binary interface

The contract between the WebAssembly engine and the host. Phase 1 froze `v1`,
Phase 2 added `v2`, Phase 3 added `v3`. All three exports are present in every
build and all three are tested against the published artifact, not a rebuild.

Numbers in this file are measured from `packages/core/dist/effectplan.wasm` as it
ships (`npm run verify:wasm` prints them and compares against the source
constants).

## 1. Principles

* **Additive, never breaking.** A new capability is a new exported function with
  more parameters. `effectplan_plan` and `effectplan_plan_v2` keep their exact
  signatures and keep producing exactly the bytes they always did.
* **Canonical regions.** Every pointer argument must name the region's base
  *exactly*, and every length argument must name its size *exactly*. A correct
  byte count at a wrong address is a bug in the caller, and it is refused
  (`108`) rather than tolerated.
* **The host discovers, it does not remember.** Bases and sizes come from
  `effectplan_info`. Nothing in the host hardcodes an offset or a ceiling.
* **Commit records.** A region's header is written last. A reader that finds an
  unexpected revision knows the producing pass did not finish, so a failed call
  can never be mistaken for a previous batch's result.
* **Status codes, not messages.** The engine returns an `i32`. Text lives in the
  host layer. No caller identifier and no raw resource id crosses into an error.

## 2. Exports

| Export | Parameters | Added | Produces |
| --- | --- | --- | --- |
| `effectplan_info(kind)` | 1 | Phase 1 | one `u32` describing the build |
| `effectplan_plan` | 12 | Phase 1 | dependency graph in the output region |
| `effectplan_plan_v2` | 14 | Phase 2 | the same, plus execution levels |
| `effectplan_plan_v3` | 18 | Phase 3 | the same, plus the critical path |
| `effectplan_plan_v4` | 20 | Phase 4 | the same, resumable from a checkpoint |
| `effectplan_resource_check` | 12 | Phase 5 | feasibility of declared capacities, no schedule |

`v2` is `plan` plus `levels_ptr`, `levels_len`. `v3` is `v2` plus `durations_ptr`,
`durations_len`, `timing_ptr`, `timing_len`. The `v3` entry point runs the `v2`
body — the same code, not a copy — and then the scheduling pass, so a graph cannot
differ between entry points by construction. `v4` is `v3` plus `checkpoint_ptr`,
`checkpoint_len`, and it commits the record a later call resumes from.

`effectplan_resource_check` is deliberately *not* a twenty-first parameter on `v4`.
It takes the capacity table, the demand offsets, the demand records and the
diagnostic region — twelve parameters, none of them a planning region — and writes
none of the graph, levels, timing or checkpoint bytes. A single call that always
validates, plans, layers, times and schedules could not express the requirement
that a scheduling-only change leave the dependency graph alone.

The `plan`/`v2`/`v3` argument order:

```
abi_version, call_count, access_count, resource_count,
call_modes_ptr, access_offsets_ptr, resource_ids_ptr, access_modes_ptr,
workspace_ptr, workspace_len, output_ptr, output_capacity,
      [v2+] levels_ptr, levels_len,
      [v3+] durations_ptr, durations_len, timing_ptr, timing_len
```

`abi_version` must equal `info(0)`, otherwise `100`.

## 3. Memory layout

Linear memory is 18 pages (1,179,648 bytes) and never grows during a plan. The
engine's arena sits at the top of the data segment; the base is reported by
`info(1)`, and in this build it is 1,050,016.

| Region | Offset from arena base | Bytes | Written by | Read by |
| --- | --- | --- | --- | --- |
| `input` | 0 | 5,764 | host | engine |
| `workspace` | 5,764 | 17,416 | engine | engine |
| `output` | 23,180 | 36,880 | engine | host |
| `levels` | 60,060 | 260 | engine | host |
| `durations` | 60,320 | 512 | host | engine |
| `timing` | 60,832 | 1,056 | engine | host |
| `checkpoint` | 61,888 | 32 | engine | host and engine |
| `capacities` | 61,920 | 256 | host | engine |
| `demand offsets` | 62,176 | 516 | host | engine |
| `demand records` | 62,692 | 8,192 | host | engine |
| *(usage, in-degree, ready, out-degree)* | 70,884 | 1,792 | engine | engine |
| `schedule` | 72,676 | 1,056 | engine | host |
| `resource stats` | 73,732 | 1,056 | engine | host |
| `resource diagnostics` | 74,788 | 1,056 | engine | host |

Total 75,848 bytes, contiguous, four-byte aligned at every region boundary, with
53,784 bytes of address space left after the eight-byte rounding. The four
engine-private scratch regions are not published through `info`: a host has no
business naming them, and an argument that cannot name them cannot alias them. `verify:wasm` asserts the tiling (`region N+1 base
== region N base + region N length`), the alignment, each region's published
capacity and the exact page count. This is a build-time acceptance gate: a layout
drift fails the build rather than surfacing as a wrong plan.

`node scripts/dump-layout.mjs` prints this table from the shipped artifact, so
the numbers in this section are measured rather than transcribed.

Why the two new regions are so small: the graph's nodes are **calls**, not the
engine's internal events. `durations` is `128 × u32` and `timing` is a 32-byte
header plus `128 × (u32, u32)`. A per-event version of the same would have been
`1,152 × 4` and `1,152 × 8`.

## 4. Input region

Written by the host before a call, in the engine's canonical order.

| Array | Element | Count | Notes |
| --- | --- | --- | --- |
| `call_modes` | `u8` | `maxCalls` | 1 = resources, 2 = exclusive, 3 = unknown |
| `access_offsets` | `u32` | `maxCalls + 1` | monotonic, first is 0, last is `access_count` |
| `resource_ids` | `u32` | `maxAccesses` | interned, ascending within a call, `0` is the barrier |
| `access_modes` | `u8` | `maxAccesses` | 1 = read, 2 = write |

The host interns resource identifiers on first appearance, so the engine sees
dense small integers and never sees a caller string.

## 5. Output region

Header (16 bytes) then records:

| Offset | Field | Validation by the host |
| --- | --- | --- |
| 0 | `abi_version` | must equal `info(0)` |
| 4 | `call_count` | must equal the batch the engine was given |
| 8 | `edge_count` | must be `≤ maxEdges` |
| 12 | `reserved` | must be 0 |

Each record is 16 bytes: `from`, `to`, `resource`, `reason`, all `u32`
little-endian. `from < to` for every record (the forward-edge invariant), `to <
call_count`, `resource < resource_count`, and records are grouped by successor in
ascending successor order — the host checks that they are not out of order.

`reason` is `1` write→read, `2` read drained by a later write, `3` write→write,
and `4`/`5`/`6` are the same three for a barrier (`resource == 0`) edge.

## 6. Levels region (Phase 2)

`u32` count, then one `u16` level per call. `levels_len` is 260 bytes
(4 + 128 × 2). The count must equal `call_count`; a `v2` or `v3` call whose
region is not exactly this size returns `116`.

## 7. Durations region (Phase 3)

`maxCalls` `u32` slots, 512 bytes, written by the host. Slot `i` is call `i`'s
duration in ticks. The engine reads only the first `call_count` slots and requires
each of them to be non-zero: `0` returns `118`. The host encoder additionally
refuses non-integers, negatives, values above `2^32 − 1`, and a short array before
anything is written.

## 8. Timing region (Phase 3)

Header (32 bytes) then `maxCalls` rows of 8 bytes — 1,056 bytes in total.

| Offset | Field | Type |
| --- | --- | --- |
| 0 | `revision` | `u32` — `1`, written last |
| 4 | `event_count` | `u32` — equals `call_count` |
| 8 | `makespan` | `u32` |
| 12 | `peak_concurrency` | `u32` |
| 16 | `total_work` | `u64` |
| 24 | `critical_count` | `u32` |
| 28 | `reserved` | `u32` — must be 0 |
| 32 + 8i | call `i`: `earliest_start`, `slack` | two `u32` |

`latest_start` and both finish times are derived by the host from the durations it
supplied: `EF = ES + d`, `LS = ES + slack`, `LF = LS + d`. Nothing is stored twice,
so nothing can disagree with itself.

`v1` and `v2` never write this region, and `v3` clears the revision and count
before it starts, so a batch without timing keeps whatever the *previous* timed
batch left in the rows while the header says "no result". `verify:wasm` and two
Rust tests check that with a sentinel; a TS test checks the API reports no timing
at all unless it was asked for.

## 9. `effectplan_info` selectors

| Kind | Value |
| --- | --- |
| 0 | ABI version |
| 1 | arena base |
| 2 | arena bytes |
| 3, 4 | input base, input length |
| 5, 6 | workspace base, workspace length |
| 7, 8 | output base, output length |
| 9–13 | `maxCalls`, `maxAccesses`, `maxResources`, `maxEvents`, `maxEdges` |
| 14, 15, 16 | edge record bytes, max namespace bytes, max resource-id bytes |
| 17, 18, 19 | output header bytes, resource slot count, output record bytes |
| 20 | reason-code revision |
| 21–24 | levels revision, levels base, levels length, levels capacity |
| 25–28 | timing revision, durations base, durations length, durations capacity |
| 29–31 | timing base, timing length, timing capacity |
| 32–34 | checkpoint revision, checkpoint base, checkpoint length |
| 35 | resource revision |
| 36–47 | capacities base/length, demand offsets base/length, demand records base/length, schedule base/length, resource stats base/length, resource diagnostics base/length |
| 48–51 | max capacity resources, max demand records, max diagnostics, max capacity |

An unknown kind returns `u32::MAX`, which reaches the host as `-1` and fails the
loader's audit. That is how an older host notices a newer engine, and how a newer
host notices an older engine: `loadEngine` refuses to continue rather than
substituting a default.

## 10. Status codes

The engine returns a number; the host turns it into one stable error code. The
mapping in `packages/core/src/errors.ts` is exactly this:

| Code | Meaning | Public error |
| --- | --- | --- |
| 0 | success | — |
| 100 | ABI version mismatch | `EP_ENGINE_ERROR` |
| 101–103 | call, access or resource limit | `EP_LIMIT_EXCEEDED` |
| 104, 105 | pointer out of range, misaligned | `EP_ENGINE_ERROR` |
| 106, 107 | workspace or output capacity | `EP_ENGINE_ERROR` |
| 108 | a region pointer is not canonical | `EP_ENGINE_ERROR` |
| 110–115 | input format: mode, offsets, resource index, access mode | `EP_INVALID_INPUT` |
| 116, 117 | levels region size, levels invariant | `EP_INVALID_INPUT` |
| 118 | a duration is zero or missing | `EP_INVALID_INPUT` |
| 119 | durations or timing region is not the exact size | `EP_INVALID_INPUT` |
| 120 | internal event count overflow | `EP_ENGINE_ERROR` |
| 121 | ticks would overflow `u32` | `EP_LIMIT_EXCEEDED` |
| 122 | the schedule contradicts itself | `EP_ENGINE_ERROR` |
| 130 | output framing invalid | `EP_ENGINE_ERROR` |
| 140 | internal invariant violated | `EP_ENGINE_ERROR` |

Two other families never come from a status code. The host refuses bad *input*
before the engine is called — a durations array of the wrong length, a non-integer
or out-of-range duration, a batch over the limit — with `EP_INVALID_INPUT` or
`EP_LIMIT_EXCEEDED`. And the host refuses a bad *result* after the engine
returned 0 — an unexpected revision, a set reserved word, an edge that points
backwards, a schedule that contradicts its own graph — with `EP_INVALID_PLAN`.
A loader that cannot audit the engine's layout refuses to construct at all, with
`EP_ENGINE_UNAVAILABLE`.

`118`, `119` and `121` are the Phase 3 additions. They are stable identifiers: a
caller may branch on them, so they are never reused for a different meaning, and
they carry no data. The distinction that matters is between *the batch was
impossible* (`118`, `121` — the caller's durations) and *the engine was driven
wrong* (`108`, `119` — the host's pointers).

## 11. Adding a `v4`

The pattern the three versions establish: a new export that takes the previous
arguments plus new canonical region pairs; a new revision selector; a new region
appended after `timing` with the arena re-tiled and the page count re-checked; and
the old exports left alone. Anything that changes what an existing export writes
requires a new export name, not a changed behaviour.
