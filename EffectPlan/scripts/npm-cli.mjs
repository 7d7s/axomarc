/**
 * The one place that knows how to invoke npm from a script, so the packing
 * tools behave the same on Windows, macOS and Linux CI runners.
 */

import { spawnSync } from "node:child_process";
import process from "node:process";

/**
 * Runs npm with the given arguments in the given directory and returns stdout.
 * Throws with npm's own diagnostics when the command fails.
 */
export function runNpm(args, cwd) {
  const execPath = process.env.npm_execpath;
  const viaNode = typeof execPath === "string" && execPath.endsWith(".js");
  const command = viaNode ? process.execPath : process.platform === "win32" ? "npm.cmd" : "npm";
  const argv = viaNode ? [execPath, ...args] : args;
  const result = spawnSync(command, argv, {
    cwd,
    encoding: "utf8",
    shell: !viaNode && process.platform === "win32",
    maxBuffer: 64 * 1024 * 1024,
    env: { ...process.env, npm_config_update_notifier: "false" },
  });
  if (result.error) {
    throw result.error;
  }
  if (result.status !== 0) {
    throw new Error(
      `npm ${args.join(" ")} failed with exit code ${result.status}\n${result.stderr ?? ""}${result.stdout ?? ""}`,
    );
  }
  return result.stdout ?? "";
}

/** Parses the JSON document npm prints, ignoring any leading log noise. */
export function parseJsonOutput(text) {
  const start = text.indexOf("[");
  const end = text.lastIndexOf("]");
  if (start < 0 || end < start) {
    throw new Error(`npm printed no JSON document:\n${text}`);
  }
  return JSON.parse(text.slice(start, end + 1));
}
