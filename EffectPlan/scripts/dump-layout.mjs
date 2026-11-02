#!/usr/bin/env node
// Prints the arena layout of the shipped artifact as a markdown table.
//
// The layout is published by the engine through `effectplan_info`, so the table
// in docs/ARCHITECTURE.md can be regenerated instead of hand-computed — hand
// arithmetic on region boundaries has been wrong before. The three
// engine-private scratch regions are not published; they are derived as the gap
// between the last host-visible region and the schedule.
//
// Usage: node scripts/dump-layout.mjs [--markdown]
import fs from "node:fs";
import { fileURLToPath } from "node:url";

const wasmPath = fileURLToPath(new URL("../packages/core/dist/effectplan.wasm", import.meta.url));
const bytes = fs.readFileSync(wasmPath);
const instance = new WebAssembly.Instance(new WebAssembly.Module(bytes), {});
const info = instance.exports.effectplan_info;

const regions = [
  ["input", 3, 4, "call modes, offsets, resource ids, modes"],
  ["work", 5, 6, "last writers, reader lists, event lists"],
  ["output", 7, 8, "header + 2,304 edge records × 16 bytes"],
  ["levels", 22, 23, "`u32` count + 128 `u16` execution levels"],
  ["durations", 26, 27, "128 `u32` tick counts, written by the host"],
  ["timing", 29, 30, "32-byte header + 128 × (`u32` ES, `u32` slack)"],
  ["checkpoint", 33, 34, "the resumable-planning record (Phase 4)"],
  ["capacities", 36, 37, "declared capacity per resource (Phase 5)"],
  ["demand offs", 38, 39, "bounds of each call's demands"],
  ["demand recs", 40, 41, "`(resource, amount)` pairs, flattened"],
];

const base = info(1);
const arenaLen = info(2);
const schedulePtr = info(42);
const scratchStart = info(40) + info(41);
regions.push(["schedule", 42, 43, "`(start, finish)` per call (Phase 5)"]);
regions.push(["res stats", 44, 45, "capacity, peak, busy time per resource"]);
regions.push(["res diags", 46, 47, "why a demand cannot be met, by index"]);

const number = (value) => value.toLocaleString("en-US");
const rows = regions.map(([name, ptrSelector, lenSelector, note]) => {
  const ptr = info(ptrSelector);
  const len = info(lenSelector);
  return { name, address: ptr, bytes: len, note };
});
rows.splice(10, 0, {
  name: "scratch",
  address: scratchStart,
  bytes: schedulePtr - scratchStart,
  note: "usage, in-degree, ready, out-degree — engine-private",
});

if (process.argv.includes("--markdown")) {
  console.log("| Region | Address | Bytes | Contents |");
  console.log("| --- | --- | --- | --- |");
  console.log(`| below | 0 | ~1 MiB | data segment and shadow stack, unused by us |`);
  console.log(`| arena | ${number(base)} | ${number(arenaLen)} | everything below |`);
  for (const row of rows) {
    const label = row.name === rows[0].name ? `├ ${row.name}` : `├ ${row.name}`;
    console.log(`| ${label} | ${number(row.address)} | ${number(row.bytes)} | ${row.note} |`);
  }
} else {
  console.log(`arena base ${base}, arena length ${arenaLen}, arena end ${base + arenaLen}`);
  for (const row of rows) {
    console.log(`${row.name.padEnd(14)} ${String(row.address).padStart(9)} ${String(row.bytes).padStart(7)}  ${row.note}`);
  }
}
