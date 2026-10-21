# TEST AND CI EVIDENCE

Every command, its exit status and its result, as executed in this audit. The raw artefacts are in
`RAW-EVIDENCE/`.

## 1. The commands that were run

| # | Command | Exit | Result |
|---|---|---|---|
| 1 | `cargo audit --json` | 1 | **10 advisories** (wasmtime ×3, rustls ×4, rustls-webpki, webpki-roots, rsa) — 3 at severity 9.9 |
| 2 | `cargo deny check` | 1 | `advisories FAILED, bans FAILED, licenses ok, sources ok` |
| 3 | `cargo tree -i wasmtime --workspace -e normal` | 0 | `kelson-wasm` → `kelson-demo`, `kelson-framework` — the vulnerable engine is on the runtime's own path |
| 4 | `cargo tree -i webpki-roots@1.0.9` / `-i rustls` / `-i rsa` | 0 | `reqwest` → `kelson-http-client`, `kelson-export`; `sqlx-mysql` → `kelson-mysql` |
| 5 | `cargo xtask dep-rules` | 0 | clean (84 members) |
| 6 | `cargo xtask lint` | 0 | clean |
| 7 | `kelson graph` | 0 | 6 modules, 14 operations, 3 events |
| 8 | `kelson route:list` | 0 | 14 routes with the parity columns |
| 9 | `kelson check` | 0 | "dependency matrix + budgets clean (84 members)"; "graph compiles; middleware catalog resolves every route" |
| 10 | `kelson config:cache` | 0 | wrote the config cache (hash `96cb7da0…`) |
| 11 | `KEEL_CONFIG=config/dev.toml kelson-demo` | **abort** | `thread 'main' (7924) has overflowed its stack` — after the last successful log (`wasm plugin loaded`) |
| 12 | `kelson-demo --call echo.echo --input {..}` | **abort** | the same overflow — so the defect is in the boot path, not the server path |
| 13 | `cargo test -p kelson-postgres --features pg-tests --test pg_integration outbox` | 0 | 4 passed (the MySQL half real; the pg half self-skipped) |

## 2. The suites re-run in this pass (the changed and the adjacent areas)

| Suite | Result |
|---|---|
| `kelson-policy` | 6 passed |
| `kelson-context` | 6 passed |
| `kelson-http` | 178 passed |
| `kelson-http-client` | 23 passed |
| `kelson-testkit` | 63 passed |
| `kelson-deployment` | 36 passed |
| `kelson-contracts` | 3 passed |
| `kelson-execution` | 22 passed |
| `kelson-console` | 63 passed |
| `kelson-demo --lib` | 118 passed |
| `kelson-demo --test http_integration` | 102 passed |

## 3. What was NOT run, and why

| Not run | Reason | The minimum requirement |
|---|---|---|
| The whole-workspace `cargo test --workspace` | too long for the audit window; the per-crate suites above cover the changed areas | a longer audit window |
| Every PostgreSQL-only suite | the service is down (the native fault `0xC0000142`) | a healthy PostgreSQL instance |
| The live-MySQL matrix (the conformance, the job store, the sessions, the outbox) | the portable instance's `mysqld` binary is gone; the system instance denies the `keel` user | the portable MySQL, or the system instance's credentials |
| The container build, a clean-room start, the health/shutdown drill | no Docker daemon | a Docker daemon |
| The browser journey (the Playwright e2e) | no browser, no driver | a browser and the Playwright toolchain |
| The wasmtime fuzzing | no fuzz target exists in the tree | a fuzz target for the guest ABI |
| The coverage measurement | the run was started (`cargo llvm-cov` 0.9.1 is present) but did not complete in the window | a longer window |

**A self-skipping test reads like coverage.** Every PostgreSQL-only case in the matrix currently
passes by returning early, which is why the P0 database claim could not be substantiated; the audit
treats those as **NOT_RUN**, never as passes.

## 4. The CI as it stands (`.github/workflows/keel.yml`)

| Job | Steps | Audit note |
|---|---|---|
| `platform` | check, clippy, test, the G18 telemetry resilience, the architecture gate, **the semver gate**, the publish dry-run | the semver gate is a documented no-op until the first release tag ✓ |
| `keel-platform` | check, **the disk check**, three clippy legs, test, **the coverage step**, the architecture gate | the coverage step can now fail (F-29 ✓) but has **no threshold** ✓ (KFA-018 ✓) |
| `scaffold-build` | scaffold a module, `cargo check`, `cargo test` | the generated crate's compile proof ✓; it edits the workspace manifest ✓ so it needs a disposable checkout ✓ ✓ |
| `platform-mysql` | the driver matrix, **the shared conformance target** | the conformance runs against MySQL 8 in CI ✓ ✓ |
| `your-app` | check, test | the sample app ✓ |

**Gaps in the CI itself:** no `cargo fmt --check` (KFA-019 ✓), no coverage threshold (KFA-018 ✓),
no dependency-audit step (the advisories would fail the build only if the step existed ✓ — KFA-001 ✓),
no doc-link check (KFA-020 ✓), and no boot smoke (the host's crash is invisible to CI because the
suites use the harness, not the real boot ✓ — KFA-005 ✓).

## 5. The test-quality ladder in practice

| Tier | Where Kelson stands |
|---|---|
| T0 static/source | the architecture gate ✓, the env-var lint ✓, the marker scan ✓ (this audit ✓) — but no `unsafe` audit and no fuzzing ✗ |
| T1 unit | strong ✓ — 118 in the demo alone, 178 in the http crate ✓ ✓ |
| T2 contract | **partial** — 2 of the 7 fakes have a real-adapter contract ✓ (KFA-014 ✓) |
| T3 integration | the MySQL and Redis paths ✓, the PostgreSQL path ✗ (KFA-022 ✓) |
| T4 end-to-end | 102 in-process HTTP cases ✓ + a raw-socket drain test ✓ — but the **real server** cannot start ✗ (KFA-005 ✓) |
| T5 adversarial/security | 40 security-named rows ✓ — but no fuzzing, no RLS assertion, no load ✗ |
| T6 concurrency/recovery | the queue race ✓, the promotion lock ✓, the outbox redelivery ✓ (MySQL ✓) — the multi-node and the crash-recovery cases ✗ |
| T7 production-like | **nothing** — no container, no clean-room deploy, no soak, no restore drill ✗ |
