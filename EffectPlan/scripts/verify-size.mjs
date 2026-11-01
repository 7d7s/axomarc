/**
 * The release size gate.
 *
 * It measures the tarball npm actually produced: the archive is extracted to a
 * temporary directory and the bytes on disk are summed. The result is
 * cross-checked against the size npm reported, because a gate that only
 * trusts one number is not a gate.
 */

import { existsSync, mkdtempSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

import { extractTarGz } from "./tar.mjs";
import { MAX_UNPACKED_BYTES, MAX_WASM_BYTES } from "./limits.mjs";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const OUT_DIR = path.join(ROOT, "artifacts", "package");
const MANIFEST = path.join(OUT_DIR, "package-manifest.json");


function fail(message) {
  process.stderr.write(`verify-size: ${message}\n`);
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

const workspace = mkdtempSync(path.join(os.tmpdir(), "effectplan-size-"));
try {
  extractTarGz(readFileSync(tarball), workspace);
  const root = path.join(workspace, "package");

  /** Every file in the extracted archive, with the bytes it occupies on disk. */
  const walk = (directory) => {
    const out = [];
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const full = path.join(directory, entry.name);
      if (entry.isDirectory()) {
        out.push(...walk(full));
      } else {
        out.push({ path: path.relative(root, full).replaceAll("\\", "/"), bytes: statSync(full).size });
      }
    }
    return out;
  };

  const extracted = walk(root).sort((a, b) => a.path.localeCompare(b.path));
  const measuredBytes = extracted.reduce((total, file) => total + file.bytes, 0);
  const wasmFiles = extracted.filter((file) => file.path.endsWith(".wasm"));
  const wasmBytes = wasmFiles.reduce((total, file) => total + file.bytes, 0);

  const declared = manifest.files.map((file) => file.path).sort((a, b) => a.localeCompare(b));
  const found = extracted.map((file) => file.path);
  const missing = declared.filter((file) => !found.includes(file));
  const extra = found.filter((file) => !declared.includes(file));

  const report = {
    measuredAt: new Date().toISOString(),
    package: { name: manifest.name, version: manifest.version, tarball: manifest.tarball },
    unpackedBytes: measuredBytes,
    unpackedBytesReportedByNpm: manifest.unpackedBytes,
    tarballBytes: manifest.tarballBytes,
    wasmBytes,
    fileCount: extracted.length,
    limits: { maxUnpackedBytes: MAX_UNPACKED_BYTES, maxWasmBytes: MAX_WASM_BYTES },
    files: extracted,
  };
  writeFileSync(path.join(OUT_DIR, "size-report.json"), `${JSON.stringify(report, null, 2)}\n`);

  process.stdout.write("verify-size: measured from the packed tarball\n");
  process.stdout.write(`  unpacked   ${measuredBytes} bytes (npm reported ${manifest.unpackedBytes})\n`);
  process.stdout.write(`  tarball    ${manifest.tarballBytes} bytes\n`);
  process.stdout.write(`  wasm       ${wasmBytes} bytes\n`);
  process.stdout.write(`  files      ${extracted.length}\n`);

  if (missing.length > 0 || extra.length > 0) {
    fail(`archive contents disagree with the manifest (missing: ${missing.join(", ") || "none"}; extra: ${extra.join(", ") || "none"})`);
  }
  if (measuredBytes !== manifest.unpackedBytes) {
    fail(`extracted ${measuredBytes} bytes but npm reported ${manifest.unpackedBytes}`);
  }
  if (measuredBytes > MAX_UNPACKED_BYTES) {
    fail(`unpacked size ${measuredBytes} exceeds the ${MAX_UNPACKED_BYTES} byte release limit`);
  }
  if (wasmFiles.length !== 1) {
    fail(`the package carries ${wasmFiles.length} wasm binaries, expected exactly one`);
  }
  if (wasmBytes > MAX_WASM_BYTES) {
    fail(`wasm artifact is ${wasmBytes} bytes, over the ${MAX_WASM_BYTES} byte budget`);
  }
  const dependency = readFileSync(path.join(root, "package.json"), "utf8");
  const parsed = JSON.parse(dependency);
  if (parsed.dependencies && Object.keys(parsed.dependencies).length > 0) {
    fail(`the package declares runtime dependencies: ${Object.keys(parsed.dependencies).join(", ")}`);
  }
  process.stdout.write("verify-size: ok\n");
} finally {
  rmSync(workspace, { recursive: true, force: true });
}
