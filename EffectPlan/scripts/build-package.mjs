/**
 * Assembles the publishable artifact and measures it the way npm measures it.
 *
 * The size numbers in the size report come from `npm pack --dry-run --json`
 * and from the tarball npm itself produced. They are never computed by adding
 * up source file sizes, which would ignore the packing rules npm applies.
 */

import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { parseJsonOutput, runNpm } from "./npm-cli.mjs";
import { MAX_UNPACKED_BYTES, MAX_WASM_BYTES } from "./limits.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const PACKAGE_DIR = path.join(ROOT, "packages", "core");
const OUT_DIR = path.join(ROOT, "artifacts", "package");


function fail(message) {
  process.stderr.write(`build-package: ${message}\n`);
  process.exit(1);
}

const required = [
  path.join(PACKAGE_DIR, "dist", "index.js"),
  path.join(PACKAGE_DIR, "dist", "index.d.ts"),
  path.join(PACKAGE_DIR, "dist", "effectplan.wasm"),
];
for (const file of required) {
  if (!existsSync(file)) {
    fail(`missing ${path.relative(ROOT, file)}; run "npm run build:wasm && npm run build:js" first`);
  }
}

mkdirSync(OUT_DIR, { recursive: true });

// Stage the package before packing it. The staged copy is what ships, and the
// only thing done to it is whitespace: leading indentation and blank lines are
// removed from the emitted JavaScript. The published types, the wasm binary and
// every byte of meaning stay untouched, and the installation smoke test runs
// against this artifact, so a broken transform cannot pass the gate silently.
const NL = String.fromCharCode(10);
const STAGING = path.join(ROOT, "artifacts", "staging");
rmSync(STAGING, { recursive: true, force: true });
mkdirSync(path.join(STAGING, "dist"), { recursive: true });
for (const name of ["package.json", "README.md", "LICENSE"]) {
  copyFileSync(path.join(PACKAGE_DIR, name), path.join(STAGING, name));
}
let packed = 0;
let shaved = 0;
for (const entry of readdirSync(path.join(PACKAGE_DIR, "dist"))) {
  const from = path.join(PACKAGE_DIR, "dist", entry);
  const text = readFileSync(from, "utf8");
  const to = path.join(STAGING, "dist", entry);
  // Declarations are de-indented too, but their comments stay: the types are
  // the published documentation of the API surface.
  if (entry.endsWith(".js") || entry.endsWith(".d.ts")) {
    const lines = text.split(NL);
    const kept = [];
    for (const line of lines) {
      const trimmed = line.trim();
      if (trimmed.length === 0) continue;
      kept.push(trimmed);
    }
    const compact = `${kept.join(NL)}${NL}`;
    writeFileSync(to, compact, "utf8");
    packed += Buffer.byteLength(compact);
    shaved += Buffer.byteLength(text) - Buffer.byteLength(compact);
  } else {
    copyFileSync(from, to);
    packed += statSync(to).size;
  }
}
if (!process.argv.includes("--quiet")) {
  process.stdout.write(`staged: compacted JavaScript, ${shaved} bytes of whitespace removed
`);
}

// npm's own view of the package: the file list and the unpacked size.
const dryRun = parseJsonOutput(runNpm(["pack", "--dry-run", "--json"], STAGING))[0];
if (!dryRun || typeof dryRun.unpackedSize !== "number") {
  fail("npm pack --dry-run reported no unpacked size");
}

// In --dry-run mode nothing is written: the test suite uses this to measure the
// artifact that would ship, without producing one.
const dryRunOnly = process.argv.includes("--dry-run");
if (dryRunOnly) {
  const manifest = {
    name: dryRun.name,
    version: dryRun.version,
    tarball: null,
    tarballBytes: null,
    unpackedBytes: dryRun.unpackedSize,
    entryCount: dryRun.entryCount,
    limits: { maxUnpackedBytes: MAX_UNPACKED_BYTES, maxWasmBytes: MAX_WASM_BYTES },
    files: dryRun.files.map((file) => ({ path: file.path, bytes: file.size })),
    staged: true,
  };
  process.stdout.write(`${JSON.stringify(manifest)}
`);
  const wasm = manifest.files.find((file) => file.path.endsWith(".wasm"));
  if (!wasm || manifest.unpackedBytes > MAX_UNPACKED_BYTES || wasm.bytes > MAX_WASM_BYTES) {
    fail(`dry run: ${manifest.unpackedBytes} unpacked, wasm ${wasm ? wasm.bytes : "missing"}`);
  }
  process.exit(0);
}

// The real tarball, produced by the same packer.
const result = parseJsonOutput(
  runNpm(["pack", "--json", "--pack-destination", OUT_DIR], STAGING),
)[0];
const tarball = path.join(OUT_DIR, result.filename);
if (!existsSync(tarball)) {
  fail(`npm reported ${packed.filename} but no tarball is at ${tarball}`);
}

const manifest = {
  name: result.name,
  version: result.version,
  tarball: path.relative(ROOT, tarball).replaceAll("\\", "/"),
  tarballBytes: result.size,
  unpackedBytes: dryRun.unpackedSize,
  entryCount: dryRun.entryCount,
  limits: { maxUnpackedBytes: MAX_UNPACKED_BYTES, maxWasmBytes: MAX_WASM_BYTES },
  files: dryRun.files.map((file) => ({ path: file.path, bytes: file.size })),
  npm: { size: result.size, shasum: result.shasum, integrity: result.integrity },
  stagedBytes: packed,
  whitespaceRemovedBytes: shaved,
};
writeFileSync(path.join(OUT_DIR, "package-manifest.json"), `${JSON.stringify(manifest, null, 2)}\n`);

const wasm = manifest.files.find((file) => file.path.endsWith(".wasm"));
const rows = [
  ["unpacked bytes", manifest.unpackedBytes, MAX_UNPACKED_BYTES],
  ["tarball bytes", manifest.tarballBytes, "-"],
  ["wasm bytes", wasm ? wasm.bytes : 0, MAX_WASM_BYTES],
];
process.stdout.write(`${manifest.name}@${manifest.version}\n`);
for (const [label, value, limit] of rows) {
  process.stdout.write(`  ${label.padEnd(16)} ${String(value).padStart(8)}${limit === "-" ? "" : ` / ${limit}`}\n`);
}
process.stdout.write(`  files            ${String(manifest.entryCount).padStart(8)}\n`);

if (manifest.unpackedBytes > MAX_UNPACKED_BYTES) {
  fail(`unpacked size ${manifest.unpackedBytes} exceeds the ${MAX_UNPACKED_BYTES} byte release limit`);
}
if (wasm && wasm.bytes > MAX_WASM_BYTES) {
  fail(`wasm artifact is ${wasm.bytes} bytes, over the ${MAX_WASM_BYTES} byte budget`);
}
if (!wasm) {
  fail("the package carries no wasm binary");
}
process.stdout.write("build-package: ok\n");
