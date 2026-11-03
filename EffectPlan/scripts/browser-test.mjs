/**
 * The browser smoke test.
 *
 * A plain page, no framework and no UI, is served over HTTP and opened in a
 * headless browser. The page plans a batch, times it and posts the outcome
 * back; this script decides pass or fail. It fails the build when no browser is
 * installed, because a silently skipped browser test is not a test.
 */

import { spawn } from "node:child_process";
import { createReadStream, existsSync, mkdtempSync, rmSync, statSync } from "node:fs";
import { createServer } from "node:http";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const PAGE = "/test/browser/index.html";

const TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".json": "application/json; charset=utf-8",
  ".wasm": "application/wasm",
};

/** Finds a browser to drive, or explains why none was found. */
function findBrowser() {
  const candidates = [];
  if (process.env.CHROME_PATH) candidates.push(process.env.CHROME_PATH);
  if (process.platform === "win32") {
    candidates.push(
      "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
      "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
      "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe",
    );
  } else if (process.platform === "darwin") {
    candidates.push(
      "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
      "/Applications/Chromium.app/Contents/MacOS/Chromium",
    );
  } else {
    candidates.push(
      "/usr/bin/google-chrome",
      "/usr/bin/google-chrome-stable",
      "/usr/bin/chromium",
      "/usr/bin/chromium-browser",
      "/snap/bin/chromium",
    );
  }
  for (const candidate of candidates) {
    if (candidate && existsSync(candidate)) return candidate;
  }
  return undefined;
}

const browser = findBrowser();
if (!browser) {
  process.stderr.write(
    "browser-test: no browser found. Set CHROME_PATH to a Chrome or Chromium binary.\n",
  );
  process.exit(1);
}

let resolveResult;
const resultPromise = new Promise((resolve) => { resolveResult = resolve; });

const server = createServer((request, response) => {
  if (request.method === "POST" && request.url === "/result") {
    const chunks = [];
    request.on("data", (chunk) => chunks.push(chunk));
    request.on("end", () => {
      response.writeHead(204).end();
      try {
        resolveResult(JSON.parse(Buffer.concat(chunks).toString("utf8")));
      } catch (cause) {
        resolveResult({ status: "fail", detail: { error: String(cause) } });
      }
    });
    return;
  }
  const requested = new URL(request.url ?? "/", "http://127.0.0.1").pathname;
  const target = path.join(ROOT, path.normalize(requested).replace(/^([/\\])+/, ""));
  if (!target.startsWith(ROOT) || !existsSync(target) || !statSync(target).isFile()) {
    response.writeHead(404).end("not found");
    return;
  }
  response.writeHead(200, { "content-type": TYPES[path.extname(target)] ?? "application/octet-stream" });
  createReadStream(target).pipe(response);
});

await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const port = server.address().port;
const profile = mkdtempSync(path.join(os.tmpdir(), "effectplan-chrome-"));

const child = spawn(
  browser,
  [
    "--headless=new",
    "--disable-gpu",
    "--no-sandbox",
    "--no-first-run",
    "--no-default-browser-check",
    "--disable-dev-shm-usage",
    `--user-data-dir=${profile}`,
    `http://127.0.0.1:${port}${PAGE}`,
  ],
  { stdio: ["ignore", "ignore", "pipe"] },
);
let stderr = "";
child.stderr.on("data", (chunk) => { stderr += chunk.toString(); });

const timeout = setTimeout(() => {
  child.kill();
  resolveResult({ status: "fail", detail: { error: "the browser never reported a result" } });
}, 60_000);

try {
  const outcome = await resultPromise;
  clearTimeout(timeout);
  child.kill();

  const detail = outcome.detail ?? {};
  if (outcome.status === "pass") {
    const checks = detail.checks ?? {};
    for (const [name, ok] of Object.entries(checks)) {
      if (!ok) {
        process.stderr.write(`browser-test: check ${name} failed\n`);
        process.exit(1);
      }
    }
    const execution = detail.execution ?? {};
    const timing = detail.timing ?? {};
    process.stdout.write(
      `browser-test: ok (abi ${detail.abi}, levels r${detail.levelsRevision ?? "?"}, ` +
        `timing r${detail.timingRevision ?? "?"}, ` +
        `edges ${JSON.stringify(detail.edges)}, waves ${JSON.stringify(execution.waves ?? null)}, ` +
        `makespan ${timing.makespan ?? "?"}/${timing.totalWork ?? "?"} ` +
        `peak ${timing.peakConcurrency ?? "?"})\n`,
    );
    process.exit(0);
  }
  process.stderr.write(`browser-test: failed\n${JSON.stringify(detail, null, 2)}\n${stderr}\n`);
  process.exit(1);
} finally {
  server.close();
  rmSync(profile, { recursive: true, force: true });
}
