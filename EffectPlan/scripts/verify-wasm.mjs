#!/usr/bin/env node
// Standalone ABI and memory-safety gate for the staged wasm artifact. Runs on
// plain Node with no build dependency, so it still works when TypeScript fails.
// Usage: node scripts/verify-wasm.mjs [--file <path>] [--max-bytes N]
import { readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { EXPECTED_PAGES, MAX_WASM_BYTES } from "./limits.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const arg = (name, fallback) => {
  const i = process.argv.indexOf(`--${name}`);
  return i === -1 ? fallback : process.argv[i + 1];
};
const file = resolve(root, arg("file", join("packages/core/dist/effectplan.wasm")));
const maxBytes = Number(arg("max-bytes", MAX_WASM_BYTES));

const failures = [];
const checks = [];
function check(name, condition, detail = "") {
  checks.push({ name, ok: !!condition, detail });
  if (!condition) failures.push(`${name}${detail ? ` (${detail})` : ""}`);
}

const bytes = readFileSync(file);
check("artifact exists and is non-empty", bytes.length > 0, `${bytes.length} bytes`);
check(`wasm within budget ${maxBytes}B`, bytes.length <= maxBytes, `actual ${bytes.length}B`);

const module = new WebAssembly.Module(bytes);
const imported = WebAssembly.Module.imports(module);
const exported = WebAssembly.Module.exports(module);
check("zero imports (no env, fs, network or host glue)", imported.length === 0, JSON.stringify(imported));

const functions = exported.filter((e) => e.kind === "function").map((e) => e.name).sort();
check(
  "exports exactly the ABI functions",
  functions.length === 7 &&
    functions.includes("effectplan_plan") &&
    functions.includes("effectplan_plan_v2") &&
    functions.includes("effectplan_plan_v3") &&
    functions.includes("effectplan_plan_v4") &&
    functions.includes("effectplan_resource_check") &&
    functions.includes("effectplan_resource_schedule") &&
    functions.includes("effectplan_info"),
  functions.join(","),
);
check(
  "no binding-framework or allocator exports",
  !functions.some((n) => n.includes("wasm_bindgen") || n.includes("__wbindgen") || n.includes("alloc")),
  functions.join(","),
);
check("one linear memory export", exported.filter((e) => e.kind === "memory").length === 1);

// No Rust alloc/panic-formatting machinery may survive into the artifact.
for (const needle of ["__rust_alloc", "wasm-bindgen", "capacity overflow", "already borrowed", "panicked at"]) {
  check(`binary free of "${needle}"`, !bytes.includes(Buffer.from(needle, "utf8")));
}

const instance = new WebAssembly.Instance(module, {});
const info = instance.exports.effectplan_info;
const plan = instance.exports.effectplan_plan;
const planV3 = instance.exports.effectplan_plan_v3;
const planV2 = instance.exports.effectplan_plan_v2;
const planV4 = instance.exports.effectplan_plan_v4;
const resourceCheck = instance.exports.effectplan_resource_check;
const memory = instance.exports.memory;

const K = {
  abi: 0, base: 1, arenaLen: 2, inPtr: 3, inLen: 4, wsPtr: 5, wsLen: 6, outPtr: 7, outLen: 8,
  maxCalls: 9, maxAccess: 10, maxRes: 11, maxEvents: 12, maxEdges: 13, edgeBytes: 14,
  maxNs: 15, maxId: 16, hdrBytes: 17, levelsRev: 21, levelsPtr: 22, levelsLen: 23, levelsMax: 24,
  timingRev: 25, durPtr: 26, durLen: 27, durMax: 28, timingPtr: 29, timingLen: 30, timingMax: 31,
  ckptRev: 32, ckptPtr: 33, ckptLen: 34,
  resRev: 35, capPtr: 36, capLen: 37, offsPtr: 38, offsLen: 39, recPtr: 40, recLen: 41,
  schedPtr: 42, schedLen: 43, statsPtr: 44, statsLen: 45, diagPtr: 46, diagLen: 47,
  maxCapRes: 48, maxDemands: 49, maxDiags: 50, maxCapacity: 51,
};
const v = {};
for (const [name, kind] of Object.entries(K)) v[name] = info(kind);

check("abi version is 1", v.abi === 1, String(v.abi));
// An unknown selector returns the sentinel. WebAssembly hands i32 results to
// JavaScript as signed values, so the sentinel arrives as -1.
check("unknown info kind fails closed", (info(9999) >>> 0) === 0xffffffff, String(info(9999)));
check("prototype limits", v.maxCalls === 128 && v.maxAccess === 1024 && v.maxRes === 1024);
check("edge bound equals 2*(calls+accesses)", v.maxEdges === 2 * v.maxEvents && v.maxEvents === v.maxCalls + v.maxAccess);
check("identifier limits", v.maxNs === 256 && v.maxId === 1024);
check("record framing", v.edgeBytes === 16 && v.hdrBytes === 16);
check("regions are four-byte aligned", [v.base, v.inPtr, v.wsPtr, v.outPtr].every((p) => p % 4 === 0));
check("input region equals declared capacity", v.inLen === v.maxCalls + (v.maxCalls + 1) * 4 + v.maxAccess * 4 + v.maxAccess, String(v.inLen));
check("workspace covers the four state arrays", v.wsLen === (v.maxRes + 1) * 8 + v.maxEvents * 8, String(v.wsLen));
check("output covers header plus every edge", v.outLen === v.hdrBytes + v.maxEdges * v.edgeBytes, String(v.outLen));
check("regions tile the arena without gaps", v.inPtr === v.base && v.wsPtr === v.inPtr + v.inLen && v.outPtr === v.wsPtr + v.wsLen);
check("execution-levels contract is revision 1", v.levelsRev === 1, String(v.levelsRev));
check("levels region is the tail of the output region", v.levelsPtr === v.outPtr + v.outLen, `${v.levelsPtr} vs ${v.outPtr + v.outLen}`);
check("levels region holds one u16 per call slot plus a count", v.levelsLen === 4 + v.maxCalls * 2, String(v.levelsLen));
check("levels capacity matches the call ceiling", v.levelsMax === v.maxCalls, String(v.levelsMax));
check("levels region is four-byte aligned", v.levelsPtr % 4 === 0);
check("levels region stays inside linear memory", memory.buffer.byteLength >= v.levelsPtr + v.levelsLen);
check("arena covers every region", v.arenaLen >= v.levelsPtr - v.base + v.levelsLen, `${v.arenaLen}`);
check("arena fits inside linear memory", memory.buffer.byteLength >= v.base + v.arenaLen, `${memory.buffer.byteLength} vs ${v.base + v.arenaLen}`);
check("scheduling contract is revision 1", v.timingRev === 1, String(v.timingRev));
check("durations region follows the levels region", v.durPtr === v.levelsPtr + v.levelsLen, `${v.durPtr}`);
check("durations region is one u32 per call", v.durLen === v.maxCalls * 4, String(v.durLen));
check("durations capacity matches the call ceiling", v.durMax === v.maxCalls, String(v.durMax));
check("timing region follows the durations region", v.timingPtr === v.durPtr + v.durLen, `${v.timingPtr}`);
check("timing region is a header plus two words per call", v.timingLen === 32 + v.maxCalls * 8, String(v.timingLen));
check("timing capacity matches the call ceiling", v.timingMax === v.maxCalls, String(v.timingMax));
check("the arena holds every region", v.arenaLen >= v.timingPtr - v.base + v.timingLen, `${v.arenaLen}`);
check("checkpoint contract is revision 1", v.ckptRev === 1, String(v.ckptRev));
check("checkpoint region follows the timing region", v.ckptPtr === v.timingPtr + v.timingLen, `${v.ckptPtr}`);
check("checkpoint region is eight words", v.ckptLen === 32, String(v.ckptLen));
check("the arena holds the checkpoint region too", v.arenaLen >= v.ckptPtr - v.base + v.ckptLen, `${v.arenaLen}`);
check("a selector past the last known one fails closed", (info(52) >>> 0) === 0xffffffff, String(info(52)));
check("resource contract is revision 1", v.resRev === 1, String(v.resRev));
check("resource ceilings are the published ones", v.maxCapRes === 64 && v.maxDemands === 1024 && v.maxDiags === 64, `${v.maxCapRes}/${v.maxDemands}/${v.maxDiags}`);
check("capacity ceiling keeps the arithmetic inside a u64", v.maxCapacity === 0x7fffffff, String(v.maxCapacity));
check("capacities follow the checkpoint record", v.capPtr === v.ckptPtr + v.ckptLen, `${v.capPtr}`);
check("capacity region is one word per resource", v.capLen === 4 * v.maxCapRes, String(v.capLen));
check("demand offsets follow the capacities", v.offsPtr === v.capPtr + v.capLen, `${v.offsPtr}`);
check("demand offsets bound every call plus a terminal", v.offsLen === 4 * (v.maxCalls + 1), String(v.offsLen));
check("demand records follow the offsets", v.recPtr === v.offsPtr + v.offsLen, `${v.recPtr}`);
check("a demand record is an index and an amount", v.recLen === 8 * v.maxDemands, String(v.recLen));
check("the schedule sits past the input regions", v.schedPtr > v.recPtr + v.recLen, `${v.schedPtr}`);
check("the schedule is a header plus two words per call", v.schedLen === 32 + 8 * v.maxCalls, String(v.schedLen));
check("stats follow the schedule", v.statsPtr === v.schedPtr + v.schedLen, `${v.statsPtr}`);
check("diagnostics follow the stats", v.diagPtr === v.statsPtr + v.statsLen, `${v.diagPtr}`);
// The arena is the last region's end rounded up to eight bytes, so a few bytes
// of alignment padding may follow it — but nothing else. A gap wider than that
// would mean a region the layout never accounted for.
check(
  "the arena is tiled to its last region, allowing only the eight-byte rounding",
  v.base + v.arenaLen - (v.diagPtr + v.diagLen) < 8,
  `${v.base + v.arenaLen - (v.diagPtr + v.diagLen)} bytes of padding after diagnostics`,
);
check("the resource regions grew the arena past one page", v.arenaLen === 75848, String(v.arenaLen));
check(
  `linear memory stays at ${EXPECTED_PAGES} pages`,
  memory.buffer.byteLength === EXPECTED_PAGES * 65536,
  `${memory.buffer.byteLength} bytes, ${EXPECTED_PAGES} pages, limit from scripts/limits.mjs`,
);
check("memory demand stays modest", memory.buffer.byteLength <= 2 * 1024 * 1024, `${memory.buffer.byteLength}B`);

// ---- Functional smoke over the raw ABI -------------------------------------
const view = new Uint8Array(memory.buffer);
const words = new DataView(memory.buffer);
const IN = v.inPtr;
const OFF = IN + v.maxCalls;
const IDS = OFF + (v.maxCalls + 1) * 4;
const MODES = IDS + v.maxAccess * 4;

function encode(modes, offsets, rids, accessModes) {
  for (let i = 0; i < modes.length; i++) view[IN + i] = modes[i];
  for (let i = 0; i < offsets.length; i++) words.setUint32(OFF + i * 4, offsets[i], true);
  for (let i = 0; i < rids.length; i++) words.setUint32(IDS + i * 4, rids[i], true);
  for (let i = 0; i < accessModes.length; i++) view[MODES + i] = accessModes[i];
  return plan(
    v.abi, modes.length, rids.length, Math.max(...rids, 0) + 1,
    v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen,
  );
}
function readEdges() {
  const n = words.getUint32(v.outPtr + 8, true);
  const out = [];
  for (let i = 0; i < n; i++) {
    const at = v.outPtr + 16 + i * 16;
    out.push([words.getUint32(at, true), words.getUint32(at + 4, true), words.getUint32(at + 8, true), words.getUint32(at + 12, true)]);
  }
  return out;
}

// write R1 then read R1 must order the reader behind the writer.
check("write/read conflict detected", encode([1, 1], [0, 1, 2], [1, 1], [1, 0]) === 0 && JSON.stringify(readEdges()) === "[[0,1,1,1]]", JSON.stringify(readEdges()));
// read R1 then write R1 must order the writer behind the reader.
check("read/write conflict detected", encode([1, 1], [0, 1, 2], [1, 1], [0, 1]) === 0 && JSON.stringify(readEdges()) === "[[0,1,1,2]]", JSON.stringify(readEdges()));
// two reads must stay independent.
check("read/read stays independent", encode([1, 1], [0, 1, 2], [1, 1], [0, 0]) === 0 && readEdges().length === 0);
// distinct resources must stay independent.
check("distinct resources stay independent", encode([1, 1], [0, 1, 2], [1, 2], [1, 0]) === 0 && readEdges().length === 0);
// an exclusive call orders against an earlier ordinary call through the barrier.
check("exclusive acts as a global barrier", encode([1, 2], [0, 1, 1], [1], [0]) === 0 && JSON.stringify(readEdges()) === "[[0,1,0,5]]", JSON.stringify(readEdges()));
// determinism: identical input produces byte-identical output.
encode([1, 3, 1], [0, 1, 1, 2], [1, 1], [1, 0]);
const first = new Uint8Array(memory.buffer.slice(v.outPtr, v.outPtr + 64));
encode([1, 3, 1], [0, 1, 1, 2], [1, 1], [1, 0]);
const second = new Uint8Array(memory.buffer.slice(v.outPtr, v.outPtr + 64));
check("identical input yields identical bytes", Buffer.compare(Buffer.from(first), Buffer.from(second)) === 0);

// ---- Execution levels over the versioned ABI --------------------------------
function encodeV2(modes, offsets, rids, accessModes) {
  const status = encode(modes, offsets, rids, accessModes);
  if (status !== 0) return status;
  // A version-one call writes no levels. Seed the count with a sentinel so a
  // stale region can never masquerade as a fresh result from version two.
  words.setUint32(v.levelsPtr, 0xdeadbeef, true);
  return planV2(
    v.abi, modes.length, rids.length, Math.max(...rids, 0) + 1,
    v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen,
    v.levelsPtr, v.levelsLen,
  );
}
function readLevels() {
  const n = words.getUint32(v.levelsPtr, true);
  const out = [];
  for (let i = 0; i < n; i++) out.push(words.getUint16(v.levelsPtr + 4 + i * 2, true));
  return { count: n, levels: out };
}
function levelsOf(modes, offsets, rids, accessModes) {
  const status = encodeV2(modes, offsets, rids, accessModes);
  if (status !== 0) return { status };
  return { status, ...readLevels() };
}

check(
  "a chain is one call per wave",
  JSON.stringify(levelsOf([1, 1, 1], [0, 1, 2, 3], [1, 1, 1], [1, 0, 1])) === '{"status":0,"count":3,"levels":[0,1,2]}',
  JSON.stringify(levelsOf([1, 1, 1], [0, 1, 2, 3], [1, 1, 1], [1, 0, 1])),
);
check(
  "independent calls share a wave",
  JSON.stringify(levelsOf([1, 1], [0, 1, 2], [1, 2], [1, 1])) === '{"status":0,"count":2,"levels":[0,0]}',
  JSON.stringify(levelsOf([1, 1], [0, 1, 2], [1, 2], [1, 1])),
);
check(
  "a diamond layers 0, 1, 1, 2",
  JSON.stringify(levelsOf([1, 1, 1, 1], [0, 1, 2, 3, 4], [1, 1, 1, 1], [1, 0, 0, 1])) === '{"status":0,"count":4,"levels":[0,1,1,2]}',
  JSON.stringify(levelsOf([1, 1, 1, 1], [0, 1, 2, 3, 4], [1, 1, 1, 1], [1, 0, 0, 1])),
);
check(
  "a barrier keeps its wave alone",
  JSON.stringify(levelsOf([1, 2, 1], [0, 1, 1, 1], [1], [0])) === '{"status":0,"count":3,"levels":[0,1,2]}',
  JSON.stringify(levelsOf([1, 2, 1], [0, 1, 1, 1], [1], [0])),
);
{
  // Every emitted edge must rise between waves.
  encodeV2([1, 1, 1, 1], [0, 1, 2, 3, 4], [1, 1, 1, 1], [1, 0, 1, 0]);
  const { count, levels } = readLevels();
  const edges = readEdges();
  check("levels cover every call", count === 4, String(count));
  check(
    "every edge rises",
    edges.every(([from, to]) => levels[from] < levels[to]),
    `${JSON.stringify(edges)} at ${JSON.stringify(levels)}`,
  );
}
check(
  "the two entry points agree on the graph",
  (() => {
    encode([1, 1, 1], [0, 1, 2, 3], [1, 1, 1], [1, 0, 1]);
    const v1 = JSON.stringify(readEdges());
    encodeV2([1, 1, 1], [0, 1, 2, 3], [1, 1, 1], [1, 0, 1]);
    return v1 === JSON.stringify(readEdges());
  })(),
);
check(
  "rejects a levels pointer that is not the canonical region",
  planV2(v.abi, 0, 0, 1, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen, v.outPtr, v.levelsLen) === 108,
);
check(
  "rejects a levels length that is not the canonical region",
  planV2(v.abi, 0, 0, 1, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen, v.levelsPtr, 8) === 116,
);
check(
  "rejects a levels pointer below the arena",
  planV2(v.abi, 0, 0, 1, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen, 0, v.levelsLen) === 104,
);
// The empty batch is legal and must publish an empty level set.
check(
  "empty batch publishes zero levels",
  encodeV2([], [0], [], []) === 0 && JSON.stringify(readLevels()) === '{"count":0,"levels":[]}',
  JSON.stringify(readLevels()),
);

// ---- Critical path intelligence over the scheduling ABI ----------------------
const TP = v.timingPtr;
const writeDurations = (values) => {
  for (let i = 0; i < v.maxCalls; i++) words.setUint32(v.durPtr + i * 4, values[i] ?? 0, true);
};
function encodeV3(modes, offsets, rids, accessModes, durations) {
  if (encode(modes, offsets, rids, accessModes) !== 0) return 999;
  writeDurations(durations);
  return planV3(
    v.abi, modes.length, rids.length, Math.max(...rids, 0) + 1,
    v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen,
    v.levelsPtr, v.levelsLen, v.durPtr, v.durLen, TP, v.timingLen,
  );
}
function readTiming() {
  const count = words.getUint32(TP + 4, true);
  const events = [];
  for (let i = 0; i < count; i++) {
    const at = TP + 32 + i * 8;
    events.push([words.getUint32(at, true), words.getUint32(at + 4, true)]);
  }
  return {
    revision: words.getUint32(TP, true),
    count,
    makespan: words.getUint32(TP + 8, true),
    peak: words.getUint32(TP + 12, true),
    totalWork: words.getUint32(TP + 16, true),
    criticalCount: words.getUint32(TP + 24, true),
    reserved: words.getUint32(TP + 28, true),
    events,
  };
}
const JT = (value) => JSON.stringify(value);

check("a chain sums its durations",
  encodeV3([1, 1, 1], [0, 1, 2, 3], [1, 1, 1], [1, 0, 1], [2, 3, 4]) === 0 &&
    readTiming().makespan === 9 && readTiming().peak === 1 && JT(readTiming().events) === "[[0,0],[2,0],[5,0]]",
  JT(readTiming()));
check("independent calls take the longest branch",
  encodeV3([1, 1, 1], [0, 1, 2, 3], [1, 2, 3], [1, 1, 1], [5, 9, 2]) === 0 &&
    readTiming().makespan === 9 && readTiming().totalWork === 16 &&
    readTiming().peak === 3 && readTiming().criticalCount === 1,
  JT(readTiming()));
check("a diamond layers by its deepest branch",
  encodeV3([1, 1, 1, 1, 1], [0, 1, 2, 3, 4, 5], [1, 1, 1, 1, 1], [1, 0, 0, 1, 0], [2, 5, 2, 3, 4]) === 0 &&
    readTiming().makespan === 14 && readTiming().peak === 2 && readTiming().criticalCount === 4 &&
    JT(readTiming().events) === "[[0,0],[2,0],[2,3],[7,0],[10,0]]",
  JT(readTiming()));
check("half-open intervals: a finish at t and a start at t do not overlap",
  encodeV3([1, 1], [0, 1, 2], [1, 1], [1, 1], [5, 4]) === 0 && readTiming().peak === 1,
  JT(readTiming()));
check("overlapping intervals do count as concurrent",
  encodeV3([1, 1], [0, 1, 2], [1, 2], [1, 1], [5, 9]) === 0 && readTiming().peak === 2,
  JT(readTiming()));
check("a zero duration is refused and nothing is committed",
  encodeV3([1, 1], [0, 1, 2], [1, 1], [1, 0], [4, 0]) === 118 && readTiming().revision === 0,
  JT(readTiming()));
check("a makespan beyond u32 is refused, never wrapped",
  encodeV3([1, 1], [0, 1, 2], [1, 1], [1, 0], [0xffffffff, 1]) === 121 && readTiming().revision === 0,
  JT(readTiming()));
check("the largest representable chain is accepted exactly",
  encodeV3([1, 1], [0, 1, 2], [1, 1], [1, 0], [1, 0xfffffffe]) === 0 && readTiming().makespan === 0xffffffff,
  JT(readTiming()));
check("an aliased durations region is refused",
  planV3(v.abi, 0, 0, 1, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen,
    v.levelsPtr, v.levelsLen, v.outPtr, v.durLen, TP, v.timingLen) === 108);
check("a short timing region is refused",
  planV3(v.abi, 0, 0, 1, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen,
    v.levelsPtr, v.levelsLen, v.durPtr, v.durLen, TP, 8) === 119);
// Seed a sentinel: a version one or version two batch must leave the region
// exactly as it found it, so a stale timing result can never look current.
words.setUint32(TP, 0xdeadbeef, true);
encode([1, 1], [0, 1, 2], [1, 1], [1, 0]);
planV2(v.abi, 2, 2, 2, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen,
  v.levelsPtr, v.levelsLen);
check("neither lower entry point writes the timing region",
  (words.getUint32(TP, true) >>> 0) === 0xdeadbeef,
  JT(words.getUint32(TP, true) >>> 0));
check("an empty batch schedules to nothing",
  encodeV3([], [0], [], [], []) === 0 && JT(readTiming().events) === "[]" &&
    readTiming().makespan === 0 && readTiming().peak === 0 && readTiming().totalWork === 0,
  JT(readTiming()));

// ---- Resumable planning over the ABI (Phase 4) ----------------------------
//
// The claim is narrow and absolute: resuming from a checkpoint must produce the
// same bytes as planning the whole batch, and a checkpoint that does not
// describe this prefix must be refused before the suffix is planned.
const CP = v.ckptPtr;
const RECORDS = v.outPtr + 16;
const recordBytes = () => new Uint8Array(memory.buffer, CP, 32).slice();
const wordAt = (at) => words.getUint32(at, true);
const shape = () =>
  JT({
    edges: readEdges(),
    levels: readLevels(),
    timing: readTiming(),
  });
const checkpointWords = () => [wordAt(CP), wordAt(CP + 4), wordAt(CP + 8), wordAt(CP + 12)];

function snapshot() {
  const edges = wordAt(v.outPtr + 8);
  return {
    workspace: new Uint8Array(memory.buffer, v.wsPtr, v.wsLen).slice(),
    records: new Uint8Array(memory.buffer, RECORDS, edges * 16).slice(),
    record: recordBytes(),
  };
}

/// A tampered copy of a *known good* checkpoint. Re-reading the arena after a
/// refused run would not do: a refusal clears the record, so there would be
/// nothing left to corrupt.
function corrupt(snap) {
  return {
    workspace: snap.workspace.slice(),
    records: snap.records.slice(),
    record: snap.record.slice(),
  };
}

function encodeV4(modes, offsets, rids, accessModes, durations, snap) {
  if (encode(modes, offsets, rids, accessModes) !== 0) return 999;
  writeDurations(durations);
  if (snap) {
    new Uint8Array(memory.buffer, v.wsPtr, v.wsLen).set(snap.workspace);
    new Uint8Array(memory.buffer, RECORDS, snap.records.length).set(snap.records);
    new Uint8Array(memory.buffer, CP, 32).set(snap.record);
  } else {
    new Uint8Array(memory.buffer, CP, 32).fill(0);
  }
  return planV4(
    v.abi, modes.length, rids.length, Math.max(...rids, 0) + 1,
    v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen,
    v.levelsPtr, v.levelsLen, v.durPtr, v.durLen, TP, v.timingLen, CP, v.ckptLen,
  );
}

const BATCH = {
  modes: [1, 1, 1, 1, 2, 1],
  offsets: [0, 1, 2, 3, 4, 4, 5],
  rids: [1, 1, 2, 2, 2],
  amodes: [1, 0, 1, 0, 0],
  ticks: [2, 3, 5, 1, 4, 2],
};

check(
  "an empty record plans the whole batch and commits a checkpoint",
  encodeV4(BATCH.modes, BATCH.offsets, BATCH.rids, BATCH.amodes, BATCH.ticks, null) === 0 &&
    checkpointWords()[0] === 1 &&
    checkpointWords()[1] === 6,
  JSON.stringify(checkpointWords()),
);

const wholeShape = shape();
const boundaryFailures = [];
for (let boundary = 0; boundary <= BATCH.modes.length; boundary++) {
  const prefixOffsets = BATCH.offsets.slice(0, boundary + 1);
  const used = prefixOffsets[boundary];
  let status = encodeV4(
    BATCH.modes.slice(0, boundary),
    prefixOffsets,
    BATCH.rids.slice(0, used),
    BATCH.amodes.slice(0, used),
    BATCH.ticks.slice(0, boundary),
    null,
  );
  if (status !== 0) {
    boundaryFailures.push(`planning the first ${boundary} calls failed with ${status}`);
    break;
  }
  const snap = snapshot();
  status = encodeV4(BATCH.modes, BATCH.offsets, BATCH.rids, BATCH.amodes, BATCH.ticks, snap);
  if (status !== 0) {
    boundaryFailures.push(`resuming at ${boundary} failed with ${status}`);
    break;
  }
  if (shape() !== wholeShape) {
    boundaryFailures.push(`resuming at ${boundary} disagrees with the whole run`);
    break;
  }
}
check("resuming at every boundary reproduces the whole plan", boundaryFailures.length === 0, boundaryFailures.join("; ") || "0..6");

encodeV4(BATCH.modes, BATCH.offsets, BATCH.rids, BATCH.amodes, BATCH.ticks, null);
const afterSix = snapshot();
const grown = {
  modes: [...BATCH.modes, 1],
  offsets: [...BATCH.offsets, BATCH.offsets[6] + 1],
  rids: [...BATCH.rids, 3],
  amodes: [...BATCH.amodes, 0],
  ticks: [...BATCH.ticks, 6],
};
encodeV4(grown.modes, grown.offsets, grown.rids, grown.amodes, grown.ticks, afterSix);
const appendedShape = shape();
encodeV4(grown.modes, grown.offsets, grown.rids, grown.amodes, grown.ticks, null);
check(
  "appending after a checkpoint equals a fresh longer batch",
  appendedShape === shape(),
  `${readEdges().length} edges`,
);

encodeV4(BATCH.modes.slice(0, 3), BATCH.offsets.slice(0, 4), [1, 1, 2], [1, 0, 1], BATCH.ticks.slice(0, 3), null);
const early = snapshot();
const editedPrefix = [...BATCH.amodes];
editedPrefix[0] = 0;
const editedStatus = encodeV4(BATCH.modes, BATCH.offsets, BATCH.rids, editedPrefix, BATCH.ticks, early);
check(
  "an edited prefix refuses the checkpoint and leaves no record",
  editedStatus === 131 && wordAt(CP) === 0,
  `status ${editedStatus}, revision ${wordAt(CP)}`,
);

const badFrontier = corrupt(early);
new DataView(badFrontier.workspace.buffer).setUint32(4, 9, true);
const frontierStatus = encodeV4(BATCH.modes, BATCH.offsets, BATCH.rids, BATCH.amodes, BATCH.ticks, badFrontier);
check("a frontier naming a call beyond the checkpoint is refused", frontierStatus === 131, `status ${frontierStatus}`);

const badChain = corrupt(early);
// event_next begins after the two frontier arrays; node 0 may only link to an
// older node, so a forward link is not a chain the planner ever wrote.
new DataView(badChain.workspace.buffer).setUint32((v.maxRes + 1) * 8, 40, true);
const chainStatus = encodeV4(BATCH.modes, BATCH.offsets, BATCH.rids, BATCH.amodes, BATCH.ticks, badChain);
check("a reader chain that runs forward is refused", chainStatus === 131, `status ${chainStatus}`);

const inflated = corrupt(early);
new DataView(inflated.record.buffer).setUint32(12, 9000, true);
const inflatedStatus = encodeV4(BATCH.modes, BATCH.offsets, BATCH.rids, BATCH.amodes, BATCH.ticks, inflated);
check("an edge counter the prefix could not produce is refused", inflatedStatus === 132, `status ${inflatedStatus}`);

const badDigest = corrupt(early);
new DataView(badDigest.record.buffer).setUint32(16, 0xdeadbeef, true);
const digestStatus = encodeV4(BATCH.modes, BATCH.offsets, BATCH.rids, BATCH.amodes, BATCH.ticks, badDigest);
check("a digest that does not match the state is refused", digestStatus === 131, `status ${digestStatus}`);

check(
  "a non-canonical checkpoint region is refused",
  planV4(v.abi, 0, 0, 1, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen,
    v.levelsPtr, v.levelsLen, v.durPtr, v.durLen, TP, v.timingLen, v.outPtr, v.ckptLen) === 108,
);
check(
  "a wrong checkpoint length is refused",
  planV4(v.abi, 0, 0, 1, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen,
    v.levelsPtr, v.levelsLen, v.durPtr, v.durLen, TP, v.timingLen, CP, 8) === 132,
);

words.setUint32(CP, 0xdeadbeef, true);
encodeV3(BATCH.modes, BATCH.offsets, BATCH.rids, BATCH.amodes, BATCH.ticks);
check("the scheduling entry point leaves the checkpoint region alone", wordAt(CP) >>> 0 === 0xdeadbeef, JT(wordAt(CP) >>> 0));
encode(BATCH.modes, BATCH.offsets, BATCH.rids, BATCH.amodes);
planV2(v.abi, 6, 5, 3, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen, v.levelsPtr, v.levelsLen);
check("the levels entry point leaves the checkpoint region alone", wordAt(CP) >>> 0 === 0xdeadbeef, JT(wordAt(CP) >>> 0));

// ---- Malformed and hostile calls ------------------------------------------
const call = (abi, n, a, r, cm, ao, ri, am, ws, wsl, out, outc) =>
  plan(abi, n, a, r, cm, ao, ri, am, ws, wsl, out, outc);

check("rejects abi mismatch", call(v.abi + 1, 0, 0, 1, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen) === 100);
check("rejects call count above the ceiling", call(v.abi, v.maxCalls + 1, 0, 1, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen) === 101);
check("rejects access count above the ceiling", call(v.abi, 1, v.maxAccess + 1, 2, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen) === 102);
check("rejects resource count above the table", call(v.abi, 1, 1, v.maxRes + 2, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen) === 103);
check("rejects zero resource count", call(v.abi, 0, 0, 0, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen) === 103);
check("rejects a pointer below the arena", call(v.abi, 0, 0, 1, 0, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen) === 104);
check("rejects a pointer past the arena", call(v.abi, 0, 0, 1, v.inPtr, OFF, IDS, v.base + v.arenaLen + 4, v.wsPtr, v.wsLen, v.outPtr, v.outLen) === 108, "refused as non-canonical");
check("rejects a non-canonical region base", call(v.abi, 0, 0, 1, v.inPtr, OFF, IDS, MODES, v.wsPtr + 4, v.wsLen, v.outPtr, v.outLen) === 108);
check("rejects an unaligned region base", call(v.abi, 0, 0, 1, v.inPtr + 1, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen) === 108, "layout refused before any read");
check("rejects a short workspace", call(v.abi, 0, 0, 1, v.inPtr, OFF, IDS, MODES, v.wsPtr, 16, v.outPtr, v.outLen) === 106);
check("rejects a short output buffer", call(v.abi, 0, 0, 1, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, 8) === 107);
check("rejects output beyond the canonical region", call(v.abi, 0, 0, 1, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen + 16) === 107);

// Structural input defects: the counts must still describe a valid batch.
view[IN] = 1; view[IN + 1] = 1;
words.setUint32(OFF, 0, true); words.setUint32(OFF + 4, 2, true); words.setUint32(OFF + 8, 1, true);
words.setUint32(IDS, 1, true); words.setUint32(IDS + 4, 1, true);
view[MODES] = 0; view[MODES + 1] = 0;
check("rejects non-monotonic offsets", plan(v.abi, 2, 2, 2, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen) === 111);
words.setUint32(OFF, 1, true);
check("rejects a non-zero first offset", plan(v.abi, 2, 2, 2, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen) === 112);
words.setUint32(OFF, 0, true); words.setUint32(OFF + 4, 0, true); words.setUint32(OFF + 8, 0, true);
check("rejects an offset terminator mismatch", plan(v.abi, 2, 2, 2, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen) === 113);
words.setUint32(OFF + 4, 1, true); words.setUint32(OFF + 8, 2, true);
words.setUint32(IDS, 0, true);
check("rejects resource index zero", plan(v.abi, 2, 2, 2, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen) === 114);
words.setUint32(IDS, 999, true);
check("rejects an out-of-range resource index", plan(v.abi, 2, 2, 2, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen) === 114);
words.setUint32(IDS, 1, true);
view[IN] = 9;
check("rejects an invalid effect mode", plan(v.abi, 2, 2, 2, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen) === 110);
view[IN] = 1; view[MODES + 1] = 4;
check("rejects an invalid access mode", plan(v.abi, 2, 2, 2, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen) === 115);
view[MODES + 1] = 0;

// Capacity: two edges required, one record offered.
view[IN] = 1; view[IN + 1] = 1; view[IN + 2] = 1;
words.setUint32(OFF, 0, true); words.setUint32(OFF + 4, 1, true); words.setUint32(OFF + 8, 2, true); words.setUint32(OFF + 12, 3, true);
for (let i = 0; i < 3; i++) { words.setUint32(IDS + i * 4, 1, true); view[MODES + i] = 1; }
check("reports output capacity overflow instead of truncating", plan(v.abi, 3, 3, 2, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, 16 + 16) === 107);
check("accepts the same batch with enough capacity", plan(v.abi, 3, 3, 2, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen) === 0);

// A trap must never be reachable: drive the maximum batch through the ABI.
const modes = [];
for (let i = 0; i < v.maxCalls; i++) { view[IN + i] = 1; modes.push(1); }
for (let i = 0; i <= v.maxCalls; i++) words.setUint32(OFF + i * 4, (i * v.maxAccess) / v.maxCalls, true);
for (let k = 0; k < v.maxAccess; k++) {
  words.setUint32(IDS + k * 4, (k % 256) + 1, true);
  view[MODES + k] = k % 2;
}
let trap = null;
try {
  const status = plan(v.abi, v.maxCalls, v.maxAccess, v.maxRes + 1, v.inPtr, OFF, IDS, MODES, v.wsPtr, v.wsLen, v.outPtr, v.outLen);
  check("maximum batch plans without error", status === 0, `status ${status}`);
  const edges = readEdges();
  check("maximum batch respects the edge bound", edges.length <= v.maxEdges, `${edges.length} edges`);
  check("maximum batch edges point forward", edges.every(([f, t]) => f < t));
} catch (cause) {
  trap = cause;
  check("maximum batch must not trap", false, String(cause));
}

// --- Phase 5: the resource feasibility entry point on the shipped artifact ---
// Written and read through the same canonical regions the host will use, so an
// ABI that works in Rust but not in wasm cannot pass the gate.
const writeDemands = (capacities, perCall) => {
  for (let i = 0; i < v.maxCapRes; i++) words.setUint32(v.capPtr + i * 4, capacities[i] ?? 0, true);
  let cursor = 0;
  words.setUint32(v.offsPtr, 0, true);
  for (let call = 0; call < perCall.length; call++) {
    for (const [resource, amount] of perCall[call]) {
      words.setUint32(v.recPtr + cursor * 8, resource, true);
      words.setUint32(v.recPtr + cursor * 8 + 4, amount, true);
      cursor += 1;
    }
    words.setUint32(v.offsPtr + (call + 1) * 4, cursor, true);
  }
  return cursor;
};
const runResourceCheck = (calls, resources, demands, capPtr = v.capPtr) =>
  resourceCheck(
    v.abi, calls, resources, demands,
    capPtr, v.capLen,
    v.offsPtr, v.offsLen,
    v.recPtr, v.recLen,
    v.diagPtr, v.diagLen,
  );
const diagCount = () => words.getUint32(v.diagPtr + 4, true);
const diagStatus = () => words.getUint32(v.diagPtr + 8, true) | 0;
const diagKind = (index) => words.getUint32(v.diagPtr + 32 + index * 16, true);

check("demands within capacity are feasible", runResourceCheck(2, 2, writeDemands([10, 4], [[ [0, 6] ], [ [1, 4] ]])) === 0);
check("a demand over capacity is refused before scheduling", runResourceCheck(1, 1, writeDemands([2], [[ [0, 3] ]])) === 133);
check("the refusal names the problem", diagKind(0) === 1 && diagCount() === 1, `kind ${diagKind(0)} count ${diagCount()}`);
check("the published status echoes the returned one", diagStatus() === 133, String(diagStatus()));
check("a feasible run clears the previous refusal", runResourceCheck(1, 1, writeDemands([4], [[ [0, 4] ]])) === 0 && diagCount() === 0);
check("a zero capacity is malformed, not merely infeasible", runResourceCheck(1, 1, writeDemands([0], [[]])) === 136);
check("an undeclared resource index is refused", runResourceCheck(1, 1, writeDemands([4], [[ [7, 1] ]])) === 135);
check("a pointer into the middle of a region is refused", runResourceCheck(1, 1, writeDemands([4], [[ [0, 1] ]]), v.capPtr + 4) === 108);
check("a count over the ceiling is refused without reading", runResourceCheck(1, v.maxCapRes + 1, writeDemands([4], [[ [0, 1] ]])) === 136);
check("the entry point leaves the diagnostic revision published", words.getUint32(v.diagPtr, true) === 1, String(words.getUint32(v.diagPtr, true)));

for (const c of checks) console.log(`${c.ok ? "PASS" : "FAIL"}  ${c.name}${c.detail ? `  [${c.detail}]` : ""}`);
console.log(`\n${checks.length - failures.length}/${checks.length} ABI checks passed`);
if (failures.length) {
  for (const f of failures) console.error(`:: fail :: ${f}`);
  process.exit(1);
}
console.log(`wasm ${bytes.length}B, linear memory ${memory.buffer.byteLength}B, no trap observed (${trap ?? "none"})`);
