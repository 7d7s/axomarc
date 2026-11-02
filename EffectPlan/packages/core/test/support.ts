/**
 * Development-only helpers shared by the test suite. Never imported from
 * production code and never packed.
 */

import { readFileSync } from "node:fs";

import type { AccessMode, ExecutionPlan, ResourceAccess, ResourceRef, ToolCall } from "../src/index.js";

/**
 * The staged artifact. Tests exercise the same bytes the published package
 * carries, resolved relative to this module inside the test build output.
 */
export const wasmFileUrl = new URL(
  "../../../../packages/core/dist/effectplan.wasm",
  import.meta.url,
);

/** Bytes over a fresh ArrayBuffer, so callers can hand them to the WebAssembly API. */
export function wasmBytes(): Uint8Array<ArrayBuffer> {
  let file: Buffer;
  try {
    file = readFileSync(wasmFileUrl);
  } catch (cause) {
    throw new Error(
      `missing ${wasmFileUrl.pathname}; run "npm run build:wasm" before the tests (${String(cause)})`,
    );
  }
  const copy = new Uint8Array(file.byteLength);
  copy.set(file);
  return copy;
}

export function ref(namespace: string, id: string): ResourceRef {
  return { namespace, id };
}

export function access(namespace: string, id: string, mode: AccessMode): ResourceAccess {
  return { resource: ref(namespace, id), mode };
}

/** One call with a resources effect over the given accesses. */
export function call(id: string, ...accesses: ResourceAccess[]): ToolCall {
  return { id, effect: { version: 1, mode: "resources", accesses } };
}

/** One call with an explicit effect mode and no declared accesses. */
export function modeCall(id: string, mode: "pure" | "exclusive" | "unknown"): ToolCall {
  return { id, effect: { version: 1, mode, accesses: [] } };
}

export function pairs(plan: ExecutionPlan): Array<readonly [number, number]> {
  return plan.edges.map((e) => [e.from, e.to] as const);
}

export function triples(plan: ExecutionPlan): string {
  return JSON.stringify(plan.edges.map((e) => [e.from, e.to, e.resource, e.reason]));
}

/** Unsigned LEB128, as wasm encodes lengths. */
function uleb(value: number): number[] {
  const bytes: number[] = [];
  let rest = value;
  do {
    let byte = rest & 0x7f;
    rest >>>= 7;
    if (rest !== 0) byte |= 0x80;
    bytes.push(byte);
  } while (rest !== 0);
  return bytes;
}

/** Signed LEB128, as `i32.const` encodes its operand. */
function sleb(value: number): number[] {
  const bytes: number[] = [];
  let rest = value;
  for (;;) {
    let byte = rest & 0x7f;
    rest >>= 7;
    const signedDone = (rest === 0 && (byte & 0x40) === 0) || (rest === -1 && (byte & 0x40) !== 0);
    if (!signedDone) byte |= 0x80;
    bytes.push(byte >>> 0);
    if (signedDone) break;
  }
  return bytes;
}

/**
 * A small but internally consistent arena, so a fake engine can pass every
 * layout check the loader performs, or fail exactly one of them.
 *
 * The numbers are not the real engine's; they are the shape the loader audits:
 * contiguous four-byte-aligned regions, an output region sized from the edge
 * bound, and a levels region sized from the call ceiling.
 */
export const FAKE_INFO: Readonly<Record<number, number>> = Object.freeze({
  0: 1, // abi revision
  1: 1024, // arena base
  2: 612, // arena length: every region below, end to end
  3: 1024, // input pointer: the arena base
  4: 44, // input bytes: 4 calls + 5 offsets + 4 ids + 4 modes
  5: 1068, // workspace pointer
  6: 64, // workspace bytes
  7: 1132, // output pointer
  8: 48, // output bytes: 16-byte header + 2 edges of 16 bytes
  9: 4, // max calls
  10: 4, // max accesses
  11: 4, // max resources
  12: 8, // max events
  13: 2, // max edges
  14: 16, // edge bytes
  15: 256, // namespace bytes
  16: 1024, // resource id bytes
  17: 16, // header bytes
  21: 1, // execution-levels revision
  22: 1180, // levels pointer: output pointer + output bytes
  23: 12, // levels bytes: 4-byte count + 4 u16 levels
  24: 4, // levels entries: one per call slot
  25: 1, // timing revision
  26: 1192, // durations pointer: levels pointer + levels bytes
  27: 16, // durations bytes: one u32 per call slot
  28: 4, // durations entries
  29: 1208, // timing pointer: durations pointer + durations bytes
  30: 64, // timing bytes: 32-byte header + 4 records of 8 bytes
  31: 4, // timing entries: one per call slot
  32: 1, // checkpoint revision
  33: 1272, // checkpoint pointer: timing pointer + timing bytes
  34: 32, // checkpoint bytes: eight words
  35: 1, // resource revision
  36: 1304, // capacities pointer: checkpoint pointer + checkpoint bytes
  37: 16, // capacities bytes: 4 resources of 4 bytes
  38: 1320, // demand offsets pointer
  39: 20, // demand offsets bytes: 5 offsets of 4 bytes
  40: 1340, // demand records pointer
  41: 32, // demand records bytes: 4 records of 8 bytes
  42: 1380, // schedule pointer: after 8 bytes of engine-private scratch
  43: 64, // schedule bytes: 32-byte header + 4 records of 8 bytes
  44: 1444, // resource stats pointer
  45: 96, // resource stats bytes: 32-byte header + 4 rows of 16 bytes
  46: 1540, // resource diagnostics pointer
  47: 96, // resource diagnostics bytes: 32-byte header + 4 records of 16 bytes
  48: 4, // max capacity resources
  49: 4, // max demand records
  50: 4, // max resource diagnostics
  51: 100, // max capacity
});

export interface FakeEngineOptions {
  /** The value `effectplan_info(0)` answers. */
  readonly abi?: number;
  /** The value `effectplan_info(21)` answers. */
  readonly levelsRevision?: number;
  /** The value `effectplan_info(25)` answers. */
  readonly timingRevision?: number;
  /** The value `effectplan_info(32)` answers. */
  readonly checkpointRevision?: number;
  /** Export names to leave out of the module. */
  readonly omit?: readonly string[];
  /** Hostile values for individual info selectors, applied last. */
  readonly overrides?: Readonly<Record<number, number>>;
}

/**
 * Hand-assembled minimal wasm module used to prove the loader refuses engines
 * it cannot trust. Assembled here rather than trusted from a fixture file, so
 * the byte layout is auditable.
 *
 * `effectplan_info` is a real switch over the selector kinds; the planning
 * exports exist but always answer 0, because these modules only ever test
 * loading, never planning.
 */
export function fakeEngineBytes(options: FakeEngineOptions = {}): Uint8Array<ArrayBuffer> {
  const info: Record<number, number> = { ...FAKE_INFO };
  if (options.abi !== undefined) info[0] = options.abi;
  if (options.levelsRevision !== undefined) info[21] = options.levelsRevision;
  if (options.timingRevision !== undefined) info[25] = options.timingRevision;
  if (options.checkpointRevision !== undefined) info[32] = options.checkpointRevision;
  if (options.overrides) Object.assign(info, options.overrides);

  const section = (id: number, payload: readonly number[]): number[] => [id, ...uleb(payload.length), ...payload];
  const vec = (items: readonly (readonly number[])[]): number[] => [
    ...uleb(items.length),
    ...items.flat(),
  ];
  const name = (text: string): number[] => [...uleb(text.length), ...Array.from(text, (c) => c.charCodeAt(0))];
  const codeEntry = (body: readonly number[]): number[] => [...uleb(body.length), ...body];

  const kinds = Object.keys(info)
    .map(Number)
    .sort((a, b) => a - b);
  // if kind == k { v } else if ... { default }
  const infoBody: number[] = [0x00];
  for (const kind of kinds) {
    infoBody.push(
      0x20, 0x00, // local.get 0
      0x41, ...sleb(kind), 0x46, // i32.const kind; i32.eq
      0x04, 0x7f, // if (result i32)
      0x41, ...sleb(info[kind] ?? 0),
      0x05, // else
    );
  }
  infoBody.push(0x41, ...sleb(-1)); // unknown selector: the sentinel
  // One "end" closes each open "if", and one closes the function body.
  for (let depth = 0; depth <= kinds.length; depth++) infoBody.push(0x0b);

  const types = vec([[0x60, 0x01, 0x7f, 0x01, 0x7f], [0x60, 0x00, 0x01, 0x7f]]);
  const functions = vec([[0x00], [0x01], [0x01], [0x01], [0x01], [0x01], [0x01]]);
  const memory = vec([[0x00, 0x01]]);
  const omit = options.omit ?? [];
  const wanted: Array<readonly [string, number, number]> = [
    ["effectplan_info", 0x00, 0],
    ["effectplan_plan", 0x00, 1],
    ["effectplan_plan_v2", 0x00, 2],
    ["effectplan_plan_v3", 0x00, 3],
    ["effectplan_plan_v4", 0x00, 4],
    ["effectplan_resource_check", 0x00, 5],
    ["effectplan_resource_schedule", 0x00, 6],
    ["memory", 0x02, 0],
  ];
  const exports = vec(
    wanted
      .filter(([text]) => !omit.includes(text))
      .map(([text, kind, index]) => [...name(text), kind, index]),
  );
  // `unreachable` first: the body never runs, and the rest of it needs no
  // locals, so one shape fits every export here.
  const zeroBody = [0x00, 0x41, 0x00, 0x0b];
  const code = vec([
    codeEntry(infoBody),
    codeEntry(zeroBody),
    codeEntry(zeroBody),
    codeEntry(zeroBody),
    codeEntry(zeroBody),
    codeEntry(zeroBody),
    codeEntry(zeroBody),
  ]);

  return new Uint8Array([
    0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00,
    ...section(0x01, types),
    ...section(0x03, functions),
    ...section(0x05, memory),
    ...section(0x07, exports),
    ...section(0x0a, code),
  ]);
}
