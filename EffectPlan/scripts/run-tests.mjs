#!/usr/bin/env node
/**
 * Runs a subset of the test files and insists that the subset is not empty.
 *
 * `node --test --test-name-pattern=…` exits 0 when the pattern matches nothing,
 * which would turn a filtered CI job green while asserting nothing at all. This
 * wrapper reads the TAP summary and fails when the run is empty, when a test
 * fails, or when fewer tests ran than the job claims to cover.
 *
 *   node scripts/run-tests.mjs --min 8 --pattern "scheduling ABI" <file> …
 */

import { spawnSync } from "node:child_process";
import process from "node:process";

const args = process.argv.slice(2);
const files = [];
let min = 1;
let pattern;
for (let index = 0; index < args.length; index++) {
  const argument = args[index];
  if (argument === "--min" || argument === "--pattern") {
    const value = args[index + 1];
    if (value === undefined) {
      process.stderr.write(`run-tests: ${argument} needs a value\n`);
      process.exit(2);
    }
    index += 1;
    if (argument === "--min") min = Number(value);
    else pattern = value;
    continue;
  }
  if (argument.startsWith("--")) {
    process.stderr.write(`run-tests: unknown option ${argument}\n`);
    process.exit(2);
  }
  files.push(argument);
}

if (files.length === 0 || !Number.isInteger(min) || min < 1) {
  process.stderr.write(
    'usage: node scripts/run-tests.mjs --min <count> [--pattern <regex>] <test file> …\n',
  );
  process.exit(2);
}

const command = ["--test", "--test-reporter=tap"];
if (pattern !== undefined) command.push(`--test-name-pattern=${pattern}`);
const result = spawnSync(process.execPath, [...command, ...files], {
  encoding: "utf8",
  stdio: ["ignore", "pipe", "pipe"],
});
process.stdout.write(result.stdout);
process.stderr.write(result.stderr);

const summary = (marker) => {
  const line = result.stdout.split("\n").find((entry) => entry.startsWith(`# ${marker} `));
  return line === undefined ? undefined : Number(line.slice(`# ${marker} `.length).trim());
};
if (result.error) {
  process.stderr.write(`run-tests: the test runner failed to start: ${result.error.message}\n`);
  process.exit(1);
}
const passed = summary("pass");
const failed = summary("fail");

if (failed === undefined || passed === undefined) {
  process.stderr.write("run-tests: no TAP summary was produced, so nothing is known about this run\n");
  process.exit(1);
}
if (result.status !== 0 || failed > 0) {
  process.stderr.write(`run-tests: ${failed} test(s) failed (exit ${result.status})\n`);
  process.exit(1);
}
if (passed < min) {
  process.stderr.write(
    `run-tests: the pattern matched ${passed} test(s); this job claims ${min}. ` +
      "A filtered job that runs nothing is not a job.\n",
  );
  process.exit(1);
}
process.stdout.write(`run-tests: ${passed} test(s) passed, at least ${min} required\n`);
