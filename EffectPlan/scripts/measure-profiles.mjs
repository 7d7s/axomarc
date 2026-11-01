/**
 * Measures the WebAssembly artifact under each cargo profile, so the size
 * report is backed by real builds rather than by estimates.
 */

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUT_DIR = path.join(ROOT, "artifacts", "profiles");
const PROFILES = ["release", "release-s", "release-3"];

mkdirSync(OUT_DIR, { recursive: true });

const results = [];
for (const profile of PROFILES) {
  const out = path.join(OUT_DIR, `${profile}.wasm`);
  const result = spawnSync(
    process.execPath,
    [path.join(ROOT, "scripts", "build-wasm.mjs"), "--profile", profile, "--out", out],
    { cwd: ROOT, encoding: "utf8" },
  );
  if (result.status !== 0) {
    process.stderr.write(result.stderr ?? "");
    process.stderr.write(`measure-profiles: ${profile} failed\n`);
    process.exit(result.status ?? 1);
  }
  if (!existsSync(out)) {
    process.stderr.write(`measure-profiles: ${profile} produced no artifact\n`);
    process.exit(1);
  }
  const bytes = readFileSync(out).length;
  results.push({ profile, wasmBytes: bytes });
  process.stdout.write(`${profile.padEnd(10)} ${String(bytes).padStart(7)} bytes\n`);
}

writeFileSync(
  path.join(ROOT, "artifacts", "profile-sizes.json"),
  `${JSON.stringify({ measuredAt: new Date().toISOString(), profiles: results }, null, 2)}\n`,
);
process.stdout.write("measure-profiles: ok\n");
