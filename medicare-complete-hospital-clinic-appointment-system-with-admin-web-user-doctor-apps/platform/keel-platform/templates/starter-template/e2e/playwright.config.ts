import { defineConfig } from '@playwright/test';

// B-28 smoke kit: runs against an already-running `kelson serve` (default
// 127.0.0.1:8180). Point PLAYWRIGHT_BASE_URL elsewhere to test another
// edge. External tooling: this folder owns its package.json and never joins
// the Rust workspace or the dependency budget.
export default defineConfig({
  testDir: '.',
  timeout: 30_000,
  retries: 0,
  use: {
    baseURL: process.env.PLAYWRIGHT_BASE_URL ?? 'http://127.0.0.1:8180',
    trace: 'retain-on-failure',
  },
  reporter: [['list']],
});
