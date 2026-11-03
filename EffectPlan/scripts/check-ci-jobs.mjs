#!/usr/bin/env node
/**
 * Checks that every CI job which builds the WebAssembly also installs the
 * target it builds for.
 *
 * A job that runs the build without the target fails on the runner with
 * `E0463: can't find crate for core`, which says nothing about the job that
 * was wrong. This check says it before the job does.
 *
 * The build needs `wasm32-unknown-unknown`; the JavaScript build, the type
 * check and the test compilation do not. So the rule is narrow on purpose: a
 * job that runs `npm run build` or `npm run build:wasm` must install the
 * target, and nothing else has to.
 *
 *   node scripts/check-ci-jobs.mjs
 */

import { readFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const SOURCE = path.join(ROOT, ".github", "workflows", "ci.yml");

/** The commands that compile the engine for the browser. */
const BUILDS_WASM = ["npm run build", "npm run build:wasm"];

/**
 * Reads the workflow's jobs.
 *
 * The file is parsed by hand rather than with a YAML library: the project has
 * no runtime dependencies and this is a development script, and the shape it
 * needs - a job's name, its `uses:` steps and its `run:` commands - is
 * regular enough to read without a parser that could disagree with GitHub's.
 */
function readJobs(text) {
  const jobs = [];
  let current = null;
  let inJobs = false;
  for (const line of text.split("\n")) {
    if (!inJobs) {
      if (line === "jobs:") inJobs = true;
      continue;
    }
    const header = /^ {2}([a-z0-9-]+):\s*$/.exec(line);
    if (header) {
      current = { name: header[1], runs: [], uses: [] };
      jobs.push(current);
      continue;
    }
    if (!current) continue;
    const run = /^ {6}-?\s*run:\s*(.+)$/.exec(line);
    if (run) {
      current.runs.push(run[1].trim().replace(/^["']|["']$/g, ""));
      continue;
    }
    const uses = /^ {6}-?\s*uses:\s*(.+)$/.exec(line);
    if (uses) current.uses.push(uses[1].trim());
  }
  return jobs;
}

/** Whether the job installs the wasm target. */
function installsWasmTarget(job) {
  const lines = job.lines ?? [];
  for (let index = 0; index < lines.length; index++) {
    if (!/uses:\s*dtolnay\/rust-toolchain/.test(lines[index])) continue;
    // The `targets:` key is inside the same step, two lines below the `uses:`.
    for (let next = index + 1; next < Math.min(index + 4, lines.length); next++) {
      if (/targets:\s*wasm32-unknown-unknown/.test(lines[next])) return true;
    }
  }
  return false;
}

const text = readFileSync(SOURCE, "utf8").replaceAll("\r\n", "\n");
const jobs = readJobs(text);

// The per-step lines each job owns, so the target can be found in context.
const jobBlocks = text.split("\njobs:\n")[1] ?? "";
const blocks = jobBlocks.split(/^ {2}[a-z0-9-]+:\s*$/m).slice(1);
const names = [...jobBlocks.matchAll(/^ {2}([a-z0-9-]+):\s*$/gm)].map((match) => match[1]);
jobs.forEach((job, index) => {
  job.lines = (blocks[index] ?? "").split("\n");
});

const problems = [];
for (const job of jobs) {
  const builds = job.runs.some((command) =>
    BUILDS_WASM.some((needle) => command === needle || command.startsWith(`${needle} `)),
  );
  if (!builds) continue;
  if (!installsWasmTarget(job)) {
    problems.push(
      `job "${job.name}" runs ${BUILDS_WASM.filter((needle) => job.runs.some((c) => c.startsWith(needle))).join(" or ")} but does not install the wasm32-unknown-unknown target`,
    );
  }
}

if (problems.length > 0) {
  for (const problem of problems) process.stderr.write(`check-ci-jobs: ${problem}\n`);
  process.stderr.write(
    "check-ci-jobs: add a dtolnay/rust-toolchain step with `targets: wasm32-unknown-unknown`\n",
  );
  process.exit(1);
}

process.stdout.write(`check-ci-jobs: ${jobs.length} jobs, every wasm build installs its target\n`);
