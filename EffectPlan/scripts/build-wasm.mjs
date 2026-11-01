#!/usr/bin/env node
// Builds the WebAssembly engine and stages it inside the publishable package.
// Usage: node scripts/build-wasm.mjs [--profile release|release-s|release-3] [--out <file>] [--no-copy]
import { spawnSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const flag = (name, fallback) => {
  const i = args.indexOf(`--${name}`);
  return i === -1 ? fallback : args[i + 1];
};
const profile = flag("profile", "release");
const outPath = resolve(root, flag("out", join("packages/core/dist/effectplan.wasm")));

function run(command, argv, cwd = root, shell = false) {
  const result = spawnSync(command, argv, { cwd, encoding: "utf8", shell });
  if (result.error) {
    console.error(`:: error :: ${command} failed to start: ${result.error.message}`);
    process.exit(1);
  }
  if (result.status !== 0) {
    console.error(`:: fail :: ${command} ${argv.join(" ")} -> exit ${result.status}`);
    if (result.stderr) process.stderr.write(result.stderr);
    if (result.stdout) process.stdout.write(result.stdout);
    process.exit(result.status ?? 1);
  }
  return result;
}

run("cargo", ["build", "--offline", "-q", "-p", "effectplan-engine", "--target", "wasm32-unknown-unknown", "--profile", profile]);

const built = join(root, "target", "wasm32-unknown-unknown", profile, "effectplan_engine.wasm");
if (!existsSync(built)) {
  console.error(`:: fail :: expected artifact missing: ${built}`);
  process.exit(1);
}
let bytes = new Uint8Array(readFileSync(built));
const rustSize = bytes.length;

// Binaryen is optional: use it only when it is already installed, and keep the
// smaller of the two results rather than assuming -Oz is an improvement.
// npm's own shim is preferred over PATH, because on Windows `wasm-opt` is a
// .cmd file that spawnSync will not resolve without a shell.
function findWasmOpt() {
  const local = join(root, "node_modules", ".bin", "wasm-opt");
  for (const candidate of process.platform === "win32" ? [`${local}.exe`, `${local}.cmd`, local] : [local]) {
    if (existsSync(candidate)) return candidate;
  }
  return spawnSync("wasm-opt", ["--version"], { encoding: "utf8" }).status === 0 ? "wasm-opt" : null;
}

const opt = findWasmOpt();
let usedOpt = null;
if (opt) {
  const probe = spawnSync(opt, ["--version"], { encoding: "utf8", shell: opt.endsWith(".cmd") });
  if (probe.status === 0) {
    const optimized = join(root, "target", "effectplan.wasm-opt.wasm");
    const result = spawnSync(opt, [built, "--enable-bulk-memory", "-Oz", "-o", optimized], {
      cwd: root,
      encoding: "utf8",
      shell: opt.endsWith(".cmd"),
    });
    // A toolchain that cannot read this module is not a reason to fail the
    // build: the cargo artifact is already within budget, and the smaller of
    // the two wins.
    if (result.status === 0 && existsSync(optimized)) {
      const candidate = new Uint8Array(readFileSync(optimized));
      if (candidate.length < bytes.length) {
        bytes = candidate;
        usedOpt = candidate.length;
      }
    } else {
      console.error(
        `note: wasm-opt could not process this module (${(result.stderr ?? String.fromCharCode).split(String.fromCharCode(10))[0]}); shipping the cargo build`,
      );
    }
  }
}

if (!args.includes("--no-copy")) {
  mkdirSync(dirname(outPath), { recursive: true });
  writeFileSync(outPath, bytes);
}

console.log(
  `wasm profile=${profile} rust=${rustSize}B final=${bytes.length}B ` +
    `wasmOpt=${usedOpt === null ? "unavailable" : "applied"} staged=${args.includes("--no-copy") ? "no" : outPath}`,
);
