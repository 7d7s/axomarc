/**
 * The installation smoke test: the packed tarball is installed into a throwaway
 * project with npm, imported from node_modules, and asked to plan a batch.
 *
 * This is the only check that proves the published artifact works on its own,
 * outside this repository, with no build step and no source tree in sight.
 */

import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { runNpm } from "./npm-cli.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUT_DIR = path.join(ROOT, "artifacts", "package");
const MANIFEST = path.join(OUT_DIR, "package-manifest.json");

function fail(message) {
  process.stderr.write(`verify-package: ${message}\n`);
  process.exit(1);
}

if (!existsSync(MANIFEST)) {
  fail("no package manifest; run \"npm run build\" first");
}
const manifest = JSON.parse(readFileSync(MANIFEST, "utf8"));
const tarball = path.join(ROOT, manifest.tarball);
if (!existsSync(tarball)) {
  fail(`no tarball at ${manifest.tarball}; run \"npm run build\" first`);
}

const workspace = mkdtempSync(path.join(os.tmpdir(), "effectplan-install-"));
try {
  writeFileSync(
    path.join(workspace, "package.json"),
    `${JSON.stringify({ name: "effectplan-smoke", private: true, version: "0.0.0", type: "module" }, null, 2)}\n`,
  );

  const install = runNpm(
    ["install", tarball, "--no-audit", "--no-fund", "--ignore-scripts", "--loglevel", "error"],
    workspace,
  );
  if (install.trim().length > 0) {
    process.stdout.write(`  npm install: ${install.trim()}\n`);
  }

  const installed = path.join(workspace, "node_modules", "@effectplan", "core");
  if (!existsSync(path.join(installed, "dist", "effectplan.wasm"))) {
    fail("the installed package carries no wasm binary");
  }

  const tree = JSON.parse(runNpm(["ls", "--json", "--all"], workspace));
  const dependencies = Object.keys(tree.dependencies ?? {});
  if (dependencies.length !== 1 || dependencies[0] !== "@effectplan/core") {
    fail(`the installed tree carries unexpected packages: ${dependencies.join(", ") || "none"}`);
  }
  const installedManifest = JSON.parse(readFileSync(path.join(installed, "package.json"), "utf8"));
  if (installedManifest.dependencies && Object.keys(installedManifest.dependencies).length > 0) {
    fail("the installed package declares runtime dependencies");
  }

  const smoke = `
import assert from "node:assert/strict";
import { EffectPlan } from "@effectplan/core";

const planner = await EffectPlan.create();
const info = planner.getEngineInfo();
assert.equal(info.abiVersion, 1);
assert.equal(info.limits.maxCalls, 128);
assert.equal(info.limits.maxAccesses, 1024);
assert.equal(info.limits.maxResources, 1024);

const plan = planner.planBatch([
  { id: "a", effect: { version: 1, mode: "resources", accesses: [{ resource: { namespace: "fs", id: "f1" }, mode: "write" }] } },
  { id: "b", effect: { version: 1, mode: "resources", accesses: [{ resource: { namespace: "fs", id: "f1" }, mode: "read" }] } },
]);
assert.deepEqual(plan.edges.map((edge) => [edge.from, edge.to, edge.reason]), [[0, 1, 1]]);
assert.equal(plan.statistics.callCount, 2);
process.stdout.write("installed package planned a batch\\n");
`;
  writeFileSync(path.join(workspace, "smoke.mjs"), smoke);
  const output = execFileSync(process.execPath, ["smoke.mjs"], { cwd: workspace, encoding: "utf8" });
  process.stdout.write(`  ${output.trim()}\n`);
  process.stdout.write("verify-package: ok\n");
} catch (cause) {
  fail(cause instanceof Error ? cause.message : String(cause));
} finally {
  rmSync(workspace, { recursive: true, force: true });
}
