/**
 * The release size gate, expressed as a test: the artifact that would be
 * published is measured the way npm measures it, never by adding up source
 * file sizes.
 */

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { describe, it } from "node:test";
import { fileURLToPath } from "node:url";

import { wasmBytes, wasmFileUrl } from "./support.js";

/** The repository root, four levels above the emitted test file. */
const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../../..");
const PACKAGE_DIR = path.join(ROOT, "packages", "core");

/**
 * The release policy, stated here rather than read from the tool it constrains:
 * if a script's number moves, this test fails. Through Phase 4 the unpacked limit
 * was a single ceiling raised three times by owner review (50,000 -> 60,000 ->
 * 70,000 -> 72,000). From Phase 5 it is a frozen baseline (the Phase 4 artifact,
 * 70,042) plus a per-phase envelope agreed in advance (12,000), so 82,042 —
 * deliberately not a round number: the ceiling is derived from a measurement and
 * a decision, not fitted to whatever the build happens to need. The artifact
 * budget, the dependency rule and the memory-page demand are unchanged from the
 * Phase 1 brief; the 18th page is approved for Phase 5 on measurement.
 */
const MAX_UNPACKED_BYTES = 82_042;
const MAX_WASM_BYTES = 40_000;

/**
 * The packer itself, in dry-run mode: it stages the shipped layout (the same
 * compaction the release artifact goes through) and asks npm what it would
 * pack. Measuring `packages/core` directly would measure the development
 * tree, not the package.
 */
function packageReport(): {
  unpackedBytes: number;
  entryCount: number;
  limits: { maxUnpackedBytes: number; maxWasmBytes: number };
  files: Array<{ path: string; bytes: number }>;
} {
  const execPath = process.env.npm_execpath;
  const viaNode = typeof execPath === "string" && execPath.endsWith(".js");
  const command = viaNode ? process.execPath : process.platform === "win32" ? "node.exe" : "node";
  const stdout = execFileSync(
    command,
    [viaNode ? path.join(ROOT, "scripts", "build-package.mjs") : "scripts/build-package.mjs", "--dry-run", "--quiet"],
    { cwd: ROOT, encoding: "utf8", maxBuffer: 32 * 1024 * 1024 },
  );
  const line = stdout.split(String.fromCharCode(10)).find((entry) => entry.trim().startsWith("{"));
  assert.ok(line, `the packer printed no manifest:
${stdout}`);
  return JSON.parse(line);
}

describe("package size", () => {
  it("keeps the wasm artifact inside its budget", () => {
    const bytes = wasmBytes();
    assert.ok(bytes.length <= MAX_WASM_BYTES, `wasm artifact is ${bytes.length} bytes`);
    assert.equal(readFileSync(wasmFileUrl).length, bytes.length);
  });

  it("keeps the packed package inside the release limit", () => {
    const report = packageReport();
    // The tool must enforce the limit this test states, not one it invented.
    assert.equal(report.limits.maxUnpackedBytes, MAX_UNPACKED_BYTES, "packager unpacked limit drifted");
    assert.equal(report.limits.maxWasmBytes, MAX_WASM_BYTES, "packager wasm limit drifted");
    assert.ok(
      report.unpackedBytes <= MAX_UNPACKED_BYTES,
      `unpacked package is ${report.unpackedBytes} bytes, over the ${MAX_UNPACKED_BYTES} byte limit`,
    );
  });

  it("ships exactly one wasm binary and no native addon", () => {
    const report = packageReport();
    const wasm = report.files.filter((file) => file.path.endsWith(".wasm"));
    assert.equal(wasm.length, 1, "one binary, no duplicates");
    const native = report.files.filter((file) => /\.(node|dylib|so|dll|exe)$/i.test(file.path));
    assert.deepEqual(native, [], "no native addon");
    const unwanted = report.files.filter(
      (file) =>
        file.path.startsWith("test/") ||
        file.path.startsWith("src/") ||
        (file.path.endsWith(".ts") && !file.path.endsWith(".d.ts")),
    );
    assert.deepEqual(unwanted, [], "no sources or tests in the published package");
  });

  it("publishes the files the entry point actually imports", () => {
    const report = packageReport();
    const shipped = new Set(report.files.map((file) => file.path));
    const entry = path.join(PACKAGE_DIR, "dist", "index.js");
    const sources = [entry];
    while (sources.length > 0) {
      const current = sources.pop()!;
      const text = readFileSync(current, "utf8");
      for (const match of text.matchAll(/from\s+"(\.[^"]+)"/g)) {
        const resolved = path.resolve(path.dirname(current), match[1]!);
        sources.push(resolved);
      }
    }
    for (const source of sources) {
      const relative = path.relative(PACKAGE_DIR, source).replaceAll("\\", "/");
      assert.ok(shipped.has(relative), `${relative} is imported but not published`);
    }
    assert.ok(shipped.has("dist/effectplan.wasm"), "the wasm binary must be published");
    assert.ok(readdirSync(path.join(PACKAGE_DIR, "dist")).includes("effectplan.wasm"));
  });
});
