# Getting Started & Architecture — Laravel Feature Atlas Verification (LF-001 to LF-010)

> Verification of the 10 Laravel 13.x "Getting started & architecture" atlas rows against Kelson,
> matching the workflow of `LF-011-TO-LF-027-VERIFICATION.md` and `LF-028-TO-LF-035-VERIFICATION.md`.
> The atlas dispositions (MATCH / FUNCTIONAL EQUIVALENT / EXCLUDE BY DESIGN) are recorded as claimed;
> the Status column here is the verdict from executed evidence, not from documentation.
> Status vocabulary: **VERIFIED** (executed evidence covers the row), **PARTIALLY VERIFIED**
> (implementation present; one or more required-test items missing or not implemented),
> **EXCLUDED** (documented design exclusion + replacement present), **NOT RUN**.
> Evidence pass: 2026-10-19, branch `p0-runtime-safety` (tip `b98e023`), rustc/cargo 1.92.0.

## 1. Verification Matrix

| ID | Feature | Priority | Atlas disposition | Kelson equivalent | Executed evidence (this pass) | Status |
|---|---|---|---|---|---|---|
| LF-001 | Installation and project creation | P0 | FUNCTIONAL EQUIVALENT | kelson-console CLI + embedded templates | `kelson new` scaffold live (exit 0); `doctor`/`about` live; templates 3/3 | PARTIALLY VERIFIED |
| LF-002 | Configuration and environment | P0 | FUNCTIONAL EQUIVALENT | PlatformConfig + .env + TOML + config cache | `kelson-config` 61/61; console config 4/4; live `config:reload` + `env` | PARTIALLY VERIFIED |
| LF-003 | Directory and application structure | P1 | FUNCTIONAL EQUIVALENT | Workspace + shared manifest discovery + codegen | registry 108/108; codegen 16/16; arch gate 1/1 | PARTIALLY VERIFIED |
| LF-004 | Request lifecycle | P0 | FUNCTIONAL EQUIVALENT | Axum serve + middleware pipeline + graceful shutdown | http middleware 20/20; error rendering 9/9; routing edge 1/1 | PARTIALLY VERIFIED |
| LF-005 | Service container / DI | P0 | FUNCTIONAL EQUIVALENT | ServiceRegistry + Provider graph + typed ports | lifecycle 22/22 | PARTIALLY VERIFIED |
| LF-006 | Service providers and boot lifecycle | P1 | FUNCTIONAL EQUIVALENT | Provider phases + module state machine + boot surface | lifecycle 22/22; demo module-manage/healthz/ABI 4/4 | PARTIALLY VERIFIED |
| LF-007 | Contracts / interfaces | P0 | FUNCTIONAL EQUIVALENT | Trait ports + adapters + ABI gates | shared conformance suite 12/12 fake + 13/13 MySQL (found + fixed F6); registry ABI 108/108; MySQL matrix 15/0/0 | PARTIALLY VERIFIED |
| LF-008 | Facades | P3 | EXCLUDE BY DESIGN | Explicit DI + typed refs + shell | exclusion docs + fakes exercised across executed suites | EXCLUDED (documented) |
| LF-009 | Package development and discovery | P1 | FUNCTIONAL EQUIVALENT | packages.toml index + wasm ABI + publish | registry 108/108; console wave8 4/4; ABI refusal 1/1 | PARTIALLY VERIFIED |
| LF-010 | Request-scoped context | P1 | MATCH | ExecutionContext + tenant + queue/outbox context | context 6/6; jobs 3/3; event 2/2; notifications 1/1 | PARTIALLY VERIFIED |

## 2. Method and executed evidence

### 2.1 Test runs (2026-10-19, from `platform/keel-platform`)

| Command | Result |
|---|---|
| `cargo test -p kelson-config -- --test-threads=1` | ✅ 61 passed / 0 failed (2.3 s) |
| `cargo test -p kelson-lifecycle -- --test-threads=1` | ✅ 22 passed / 0 failed |
| `cargo test -p kelson-context -- --test-threads=1` | ✅ 6 passed / 0 failed |
| `cargo test -p kelson-example-minimal` | ✅ golden test 1 passed (`golden_path_holds_all_invariants`) |
| `cargo test -p kelson-templates` | ✅ 3 passed / 0 failed |
| `cargo test -p kelson-codegen` | ✅ 16 passed / 0 failed |
| `cargo test -p kelson-registry -- --test-threads=1` | ✅ 108 passed / 0 failed |
| `cargo test -p kelson-arch-validation --test architecture platform_graph_compiles_from_manifests` | ✅ 1 passed |
| `cargo test -p kelson-console starter_template_renders_parses_and_compiles` | ✅ 1 passed |
| `cargo test -p kelson-console classic_project_template_still_substitutes` | ✅ 1 passed |
| `cargo test -p kelson-console about_report_carries_the_golden_fields` | ✅ 1 passed |
| `cargo test -p kelson-console wave8_ -- --test-threads=1` | ✅ 4 passed (pkg:add; publish idempotent; publish conflicts/force; config publish) |
| `cargo test -p kelson-console config` | ✅ 4 passed (`config_show_renders_one_redacted_section`; `config_cache_cycle_pins_and_clears_the_artifact`; `config_reload_reports_pin_state`; wave8 config publish) |
| `cargo test -p kelson-console optimize_cycle_warms_and_clears_all_three_artifacts` | ✅ 1 passed |
| `cargo test -p kelson-console env_summary_reports_environment_secrets_and_cache_state` | ✅ 1 passed |
| `cargo test -p kelson-console check_refuses_drift_abi_mismatch_and_corruption` | ⚠ filter matched 0 console tests — the test of that name lives in `kelson-registry::config_cache` and ran green in the registry suite above |
| `cargo test -p kelson-http middleware` | ✅ 20 passed / 0 failed |
| `cargo test -p kelson-jobs tenant_isolation -- --test-threads=1` | ✅ 3 passed |
| `cargo test -p kelson-event tenant_isolation -- --test-threads=1` | ✅ 2 passed |
| `cargo test -p kelson-notifications queued_envelope -- --test-threads=1` | ✅ 1 passed |
| `cargo test -p kelson-demo --lib boot_surface_carries_the_three_proof_fields` | ✅ 1 passed |
| `cargo test -p kelson-demo --lib a_stale_generation_is_refused_and_changes_nothing` | ✅ 1 passed |
| `cargo test -p kelson-demo --lib refused_reload_degrades_config_and_a_landed_reload_recovers_it` | ✅ 1 passed |
| `cargo test -p kelson-demo --lib config_reload_swaps_route_bundle_and_flags_under_50ms` | ✅ 1 passed (75.1 s; boots the full demo) |
| `cargo test -p kelson-demo --test error_rendering_integration` | ✅ 9 passed / 0 failed (74.9 s) |
| `cargo test -p kelson-demo --test http_integration healthz_is_liveness_and_readyz_reflects_degraded_state` | ✅ 1 passed (68.0 s) |
| `cargo test -p kelson-demo --test http_integration module_manage_roundtrip_degrade_recover_disable_and_readyz` | ✅ 1 passed (66.8 s) |
| `cargo test -p kelson-demo --test http_integration disabled_module_refuses_its_routes_while_the_sibling_keeps_serving` | ✅ 1 passed (64.6 s) |
| `cargo test -p kelson-demo --test http_integration package_routes_serve_over_http_and_abi_mismatch_refuses_boot` | ✅ 1 passed (68.2 s) |
| `cargo test -p kelson-demo --test routing_edge_integration domain_gate_matches_before_the_auth_chain` | ✅ 1 passed (66.2 s) |

**Total executed this pass: 275 test functions, 0 failures, 0 skipped.**

### 2.2 Live command proofs (real binaries, this pass)

| Command | Result |
|---|---|
| `kelson new lf-001-scaffold --with-starter --standalone --deps-root <checkout>` | ✅ exit 0; 14 files rendered: own `[workspace]`, `module.toml` (5 CRUD routes + 3 events, `api = 1`), `src/lib.rs`, SPA shell (`web/`), error views, Playwright e2e specs |
| `kelson doctor` | ✅ exit 0 — rustc/cargo probes, `config/dev.toml` parses, migrations present, roles.sql, 82 workspace members; advisory: no pinned toolchain, no docker-compose |
| `kelson about` | ✅ exit 0 — versions, env, drivers, 5 flags, graph 6 modules / 15 routes / 14 operations, dependency budget 25/25 |
| `kelson config:reload` | ✅ exit 0 — schema gate passed in 8 ms; effective hash + pin drift reported; "a running server swaps to this revision without a restart" |
| `kelson env` | ✅ exit 0 — secrets redacted on the wire (`postgres://keel:[REDACTED]@…`); cache pin state shown |

### 2.3 Recorded in earlier sessions (same working tree)

- MySQL driver matrix (`pg_integration`, `mysql://`): **15 passed / 0 failed / 0 skipped**; fresh-schema worker boot applied all migrations `degraded:false` — `P0-MYSQL-VERIFICATION-REPORT.md` §8.
- Live no-rebuild proof: anonymous route flipped **200 → 401 → 200** across 6 route-artifact rewrites, same PID + `process_instance_id`, zero `route reload refused`; artifact restored byte-identical — `P0-MYSQL-VERIFICATION-REPORT.md` §2.
- F3 fix live: `kelson graph` / `route:cache` / `optimize` exit 0 (6 modules, 15 routes); malformed manifest fails closed (exit 1, names file + line/col) — `P0-MYSQL-VERIFICATION-REPORT.md` §6.
- Console F3 repro `discovery_does_not_declare_an_indexed_module_twice`: 1/1 (commit `5d93f63`).

### 2.4 NOT RUN in this pass

- Docker image build (no Docker daemon on this host) — gates LF-001 "clean-machine install".
- PostgreSQL-backed tests (PG backend fork fails on this host, `0xC0000142`) — gates the pg half of the LF-007 matrix and the 13 `pg_conformance_*` cases (they self-skip; §2.5).
- A built-from-scratch scaffolded project (`cargo check` on `kelson new` output).
- Long-lived worker leak soak (no such test exists — see LF-004 / LF-010).

### 2.5 LF-007 conformance batch (same day, later run)

Shared adapter-conformance suite landed: `kelson_models::conformance` (`crates/data/models/src/conformance.rs`) —
13 contract cases generic over `ModelStore` (CRUD roundtrip; tenant isolation; soft-delete restore; find_by /
find_for_update; list sort/pagination/validation; aggregates + existence; atomic increment; filtered batch
update/delete; force_delete purge; upsert on a plain unique key; insert_or_ignore dedupe; hard-delete posture;
one database-only index-guard case). Adapter entry point: `crates/data/postgres/tests/modelstore_conformance.rs`
(13 pg + 13 mysql wrappers; same DATABASE_URL gating as the sibling matrices). The in-memory backend runs the same
12 cases it can host inside `kelson-models`' own tests, so the TestKit fake is held to one contract with the
fabrics.

| Command | Result |
|---|---|
| `cargo test -p kelson-models --locked` | ✅ 135 passed / 0 failed (includes the 12 in-memory conformance cases) |
| `cargo test -p kelson-postgres --features pg-tests --test modelstore_conformance` (no DATABASE_URL) | ✅ 26 passed — all self-skipped, gating proven |
| same + `DATABASE_URL=mysql://keel:keel@127.0.0.1:3307/keel`, `--nocapture` | ✅ 13/13 MySQL cases executed; 13 PG cases self-skipped (`pg-tests: cannot connect …; skipping`) |
| `cargo test … --test pg_modelstore … mysql` | ✅ 5 passed / 0 failed (existing twins regression) |
| `cargo test … --test pg_integration … mysql` | ✅ 15 passed / 0 failed (matrix regression after the adapter change) |
| `cargo clippy -p kelson-models -p kelson-mysql --all-targets -- -D warnings` | ✅ clean |
| `cargo clippy -p kelson-postgres --features pg-tests --all-targets -- -D warnings` | ✅ clean |
| `cargo run -p xtask -- dep-rules` | ✅ dep-rules: clean; lint: clean |

**Batch total: 181 test functions (168 executed, 13 PG-conformance cases self-skipped; 0 failures) + 2 clippy gates + dep-rules.**

The suite caught **F6** on its first live run — `list(limit)` reported `total = returned rows` on both SQL adapters
while the port's pinned contract (and the fake) report the pre-limit match count. Fixed (user-approved):
`if opts.page.is_some() || opts.limit.is_some()` COUNT branch in `crates/data/{postgres,mysql}/src/modelstore.rs`;
regressions green. Full record: `P0-MYSQL-VERIFICATION-REPORT.md` §9.

## 3. Detailed Verification

### LF-001 — Installation and project creation (P0) — PARTIALLY VERIFIED

- **Contract**: a clean supported environment can create, configure, boot and run a minimal application deterministically (match the outcome, not PHP/Composer mechanics).
- **Implementation**: `tools/kelson-console/src/main.rs` — 60+ commands (`new`, `doctor`, `check`, `about`, `make:*`, `serve`, `build`, `test`, `migrate*`, `queue:*`, `promote`/`rollback`); `new` supports `--with-starter`, `--standalone`, `--deps-root`. Templates embedded in `templates/` (`STARTER_TEMPLATE`, `PROJECT_TEMPLATE`) and guarded by `kelson-templates` tests. `Cargo.lock` is git-tracked; CI runs every job `--locked`. No `rust-toolchain.toml` exists (doctor emits a "no pinned toolchain" advisory).
- **Executed evidence**: live `kelson new` render (exit 0, 14 files, see §2.2); live `doctor`/`about` (exit 0); `starter_template_renders_parses_and_compiles` ✅; `classic_project_template_still_substitutes` ✅; `about_report_carries_the_golden_fields` ✅; `kelson-templates` 3/3 ✅; `kelson-example-minimal` golden 1/1 ✅.
- **Required-test coverage** (from the atlas row):
  - ❌ clean-machine install — not run (Docker build blocked; no scratch-machine job).
  - ✅ version/runtime checks — `doctor` probes rustc/cargo, config parse, migrations, roles; `about` carries the golden report fields (test + live run).
  - ✅ boot smoke — `golden_path_holds_all_invariants` executes the sample app; demo boots exercised repeatedly in §2.3.
  - 🟡 missing-toolchain errors — `doctor` fails closed on missing rustc/cargo in code, but no test exercises the failure path.
  - 🟡 reproducible lockfile install — `Cargo.lock` tracked + CI `--locked`; no clean-machine execution.
- **Gaps**: automated clean-install/boot-smoke job; test for doctor's missing-toolchain refusal; toolchain pin (`rust-toolchain.toml` or documented CI pin); Docker build (blocked on this host).

### LF-002 — Configuration and environment (P0) — PARTIALLY VERIFIED

- **Contract**: typed consumers resolve configured values; secrets not leaked; cached config behaves as documented; environment mismatch fails safely; distinguish immutable boot config from hot-reloadable runtime config.
- **Implementation**: `crates/application/configuration/src/lib.rs` (`PlatformConfig` layering defaults < package defaults < published package config < file < `.env` < env; `redacted_json`; freeze; schema gates refuse unknown keys), `src/runtime.rs` (`ConfigRuntime`/`ConfigSnapshot`/`FileConfigSource`: poll, reload, rollback, history ring, <50 ms swap), `crates/application/registry/src/config_cache.rs` (`storage/framework/config.json` artifact + ABI pin + drift check), console `config:cache|clear|reload|show|publish|dump`, `env`, `key:generate`.
- **Executed evidence**: `kelson-config` **61/61** ✅ including — precedence: `env_override_wins_over_file`, `package_config_defaults_merge_under_the_file`, `published_package_config_overrides_manifest_defaults`, `dotenv::load_applies_only_unset_vars`; invalid: `unknown_keys_are_reported_and_refused_at_load`, `config_reload_invalid_config_keeps_current_revision`, `prod_rejects_dev_secret_and_placeholder_key`; redaction: `redacted_json_masks_secrets_on_frozen_config`, `mask::masks_password_only`; reload: 8 `runtime::tests` (poll-once, no-op, invalid-keeps-revision, <50 ms swap, under-load never tears, rollback, snapshot, history cap). Console: `config_cache_cycle_pins_and_clears_the_artifact` ✅, `config_show_renders_one_redacted_section` ✅, `config_reload_reports_pin_state` ✅, `optimize_cycle_warms_and_clears_all_three_artifacts` ✅, `env_summary_reports_environment_secrets_and_cache_state` ✅; registry `config_cache::check_refuses_drift_abi_mismatch_and_corruption` ✅. Demo: `config_reload_swaps_route_bundle_and_flags_under_50ms` ✅, `a_stale_generation_is_refused_and_changes_nothing` ✅, `refused_reload_degrades_config_and_a_landed_reload_recovers_it` ✅. Live: `config:reload` (schema gate + drift report + no-restart semantics) ✅, `env` (live redaction) ✅; P0 no-rebuild hot-swap proof (§2.3).
- **Required-test coverage**:
  - ✅ precedence — executed (env > file > package/published defaults; `.env` only fills unset vars).
  - 🟡 missing/invalid config — invalid executed (unknown keys refused, typos name the file); **absent config file has no test**.
  - ✅ config-cache parity — artifact pin/clear/drift/ABI tests executed (console + registry); `optimize` warms all three artifacts.
  - ✅ secret redaction — unit + console + live `env` output.
  - ✅ reload/restart semantics — 8 runtime tests + 3 demo tests + live report; route/config swaps proven live without restart (§2.3).
  - ❌ config isolation — no code symbol or test (grep "isolation" in the config crate: 0 matches); semantics undocumented.
- **Gaps**: test for an absent config file; config-isolation concept (define or drop); note the doc-level discrepancy below (atlas undercounted the reload tests).

### LF-003 — Directory and application structure (P1) — PARTIALLY VERIFIED

- **Contract**: conventional structure makes routes, services, models, jobs, migrations, tests and assets discoverable; layout remains customizable.
- **Implementation**: `crates/application/registry/src/discovery.rs` — one shared `discover_manifests` (walks `applications/` + `modules/`, merges the `packages.toml` index, synthesizes modules from `routes/*.toml`); F3 fix (commit `b98e023`) removed the console/demo/arch-test copies. `packages.rs` (`PackageIndex`), `lib.rs` (`PlatformGraph::compile` + `GRAPH-*` gates). `tools/kelson-codegen` (module scaffolding, 12 generator kinds, OpenAPI emission). Workspace layout: `crates/` (grouped subsystems), `runtime/`, `tools/`, `templates/`, `examples/`, `tests/architecture/`.
- **Executed evidence**: registry **108/108** ✅ (discovery 4/4 — indexed-module dedupe, drift-stays-a-duplicate refusal, missing-manifest context refusal, route-file synthetic modules; packages 13; graph gates); `platform_graph_compiles_from_manifests` ✅; codegen **16/16** ✅ (`emits_three_files_with_substitutions`, `writes_module_and_refuses_overwrite`, `adds_workspace_member_idempotently`, `generated_code_contains_no_placeholder_macros`, OpenAPI determinism); console `starter_template_renders_parses_and_compiles` ✅; live `kelson new` scaffold structure (§2.2).
- **Required-test coverage**:
  - 🟡 scaffold golden — renders + parses + graph-compiles verified; the generated project is **not built** (no `cargo check`).
  - ✅ generator path tests — codegen suite executed.
  - ✅ package/autoload tests — `packages.toml` index round trips; typo/absent index fail closed; wave8 console tests executed.
  - 🟡 clean sample app build — `examples/minimal` golden executed (workspace sample compiles + invariants hold); a generated standalone project build is not executed.
- **Gaps**: build a scaffolded project in an automated test/job (scaffold → graph → compile); keep structure docs (`docs/00-ARCHITECTURE.md`) in sync with the manifest-discovery rule.

### LF-004 — Request lifecycle (P0) — PARTIALLY VERIFIED

- **Contract**: a request is bootstrapped once, transformed in the correct order, handled, rendered, logged and cleaned up, with exception behavior preserved.
- **Implementation**: `runtime/kelson-demo/src/lib.rs` — middleware order (telemetry context → session → admission → metrics → edge) and `axum::serve(..).with_graceful_shutdown(..)` with SIGINT/SIGTERM, poller abort, bounded telemetry shutdown; `crates/web/http/src/middleware.rs` (admission pipeline: request-id → trace → body limit → CORS → maintenance → rate limit → router) + `middleware_catalog.rs` (chain resolution); error envelope/problem rendering in `runtime/kelson-demo/src/routes.rs` (`error_envelope`, `problem_response`, debug redaction); terminating hooks in `edge.rs`.
- **Executed evidence**: `kelson-http middleware` **20/20** ✅ (`request_id_generated_and_echoed`, `provided_request_id_reused`, `traceparent_parsed`, `body_limit_rejects_oversized_content_length`, `maintenance_blocks_with_retry_after_and_secret_bypass`, `rate_limit_exhaustion_returns_429`, `throttle_stage_exhausts_and_runs_before_auth`, `throttle_groups_have_isolated_buckets`, auth/signed/confirmed/verified stages, security headers); `error_rendering_integration` **9/9** ✅ (RFC 7807 opt-in, web/html view, report-only codes, debug-detail redaction through the logging list); `domain_gate_matches_before_the_auth_chain` ✅ (routing edge integration).
- **Required-test coverage**:
  - ✅ lifecycle ordering — throttle-before-auth and domain-gate-before-auth executed; package routes serve only after a clean boot (ABI refusal test).
  - ✅ middleware order tests — catalog + pipeline tests executed.
  - ✅ exception propagation — the 9-test error-rendering suite executed.
  - 🟡 termination behavior — implementation exists (signal-driven graceful shutdown + bounded drain); `g18_shutdown_drain_deadline_is_bounded` exists in `kelson-observability` but **no axum-serve-level shutdown test was executed**.
  - 🟡 request-scoped cleanup — terminating-hook tests exist (`edge` lib tests) and passed previously; no executed assertion this pass that per-request state is cleared.
  - ❌ long-lived worker leak test — none exists (grep "leak" finds only data-leak assertions).
- **Gaps**: an executable graceful-shutdown test (send SIGTERM → expect drain within deadline); explicit request-scope teardown assertions; a worker/loop leak soak.

### LF-005 — Service container / DI (P0) — PARTIALLY VERIFIED

- **Contract**: dependencies resolve predictably; scopes match request/job lifetimes; circular/missing bindings fail usefully; implementations are replaceable. The atlas row endorses compile-time typed ports + an explicit registry where dynamic modules require it.
- **Implementation**: `crates/foundation/lifecycle/src/lib.rs` — `ServiceRegistry` (TypeId-keyed, name, `ServiceLifetime`, `Arc<dyn Any>`, freeze) and `Provider` graph (`name`/`phase`/`dependencies`/`init`/`boot`, deterministic Kahn topo order); `ServiceLifetime` has 6 variants (`Process`, `Application`, `Request`, `Execution`, `Tenant`, `Transaction`) recorded **for diagnostics and future scoped containers**. Runtime wiring: `runtime/kelson-demo/src/lib.rs` (`host.registry().get::<T>()`, `build_app_state` / `build_app_state_with_backend`), `routes.rs` `AppState`; tenant scope in `crates/enterprise/tenancy`; fakes in `testing/testkit`.
- **Executed evidence**: `kelson-lifecycle` **22/22** ✅ — `providers_boot_in_phase_order_and_freeze`, `frozen_registry_rejects_inserts`, `duplicate_service_is_boot_error`, `cycle_is_boot_refusal`, `unknown_dependency_is_boot_refusal`, `dependencies_override_phase_order`, `host_clock_is_injectable`, `host_boot_tracks_module_states`, `host_shared_module_handle_sees_boot_states_and_runtime_transitions`, plus the full module-state suite.
- **Required-test coverage**:
  - ✅ binding resolution — executed (`get::<T>()`, MissingService, freeze).
  - 🟡 lifetimes — declared + recorded per registration (read at `lib.rs:282-296`); no scoped containers exist (grep `ServiceLifetime::Request|Execution|Tenant|Transaction` usage: 0 matches).
  - ❌ scoped reset between requests/jobs — no request/execution scope; tenant-lifetime `TenantScope` evict exists in tenancy (test exists, not executed this pass); jobs carry tenant ids in payloads instead.
  - ✅ circular dependency diagnostics — provider-name cycle + unknown dependency refusals executed; service-level cycles are impossible by constructor injection.
  - ✅ fake override — `build_app_state_with_backend` + injectable clock executed; testkit fakes used across all executed demo suites.
  - ❌ concurrent resolution — no concurrency test (resolution is synchronous Arc retrieval).
- **Gaps**: execute the tenancy `tenant_scope_isolates_services` test; decide whether request/execution-scoped services are in scope (build the scoped container or document the compile-time-only posture); concurrency smoke test; note `ApplicationHost::register_module` has zero call sites (see LF-006).

### LF-006 — Service providers and boot lifecycle (P1) — PARTIALLY VERIFIED

- **Contract**: registration is deterministic; registration is side-effect limited; boot runs after dependencies register; deferred services load lazily.
- **Implementation**: provider phases + topo boot (`crates/foundation/lifecycle/src/lib.rs`); `ModuleState` (7 variants) + `ModuleRegistry` transitions — degrade/recover/disable (disable terminal) at `lib.rs:49-276`; `crates/application/registry/src/lib.rs` `GRAPH-MANIFEST-ABI` / `GRAPH-DUP-*` gates; demo boot: provider add + `host.boot()`, eager WASM plugin load (gates + degraded exit(1)), config-poll degrade/recover, post-boot hot reload; surfaces `/healthz`, `/readyz`, `/meta/boot` (`runtime_status.rs`).
- **Executed evidence**: lifecycle **22/22** ✅ (`providers_boot_in_phase_order_and_freeze`, `dependencies_override_phase_order`, `module_registry_boot_path_reaches_ready`, `module_registry_degrade_and_recover`, `module_registry_disable_is_terminal`, `module_registry_rejects_invalid_transition`, `module_registry_snapshot_copies_all_states`, `module_state_*`); registry **108/108** ✅ (`duplicate_routes_detected`, `duplicate_route_name_detected`, `a_name_the_index_resolves_to_different_content_stays_a_duplicate`, `mismatched_module_abi_is_rejected_by_the_gate`); demo: `module_manage_roundtrip_degrade_recover_disable_and_readyz` ✅ (Ready → Degraded → Ready → Disabled; `/readyz` 200/503; `/meta/boot` shows `Disabled`; Disabled terminal 409), `disabled_module_refuses_its_routes_while_the_sibling_keeps_serving` ✅, `healthz_is_liveness_and_readyz_reflects_degraded_state` ✅, `boot_surface_carries_the_three_proof_fields` ✅, `package_routes_serve_over_http_and_abi_mismatch_refuses_boot` ✅.
- **Required-test coverage**:
  - ✅ provider ordering — executed (phase order + topo; dependencies override phase).
  - 🟡 duplicate registration — service-level and manifest/route-level gates executed; the `DuplicateProvider` error variant exists but has no test.
  - ❌ deferred loading — not implemented: providers/graph/routes/WASM all load eagerly; `ApplicationHost::register_module` (lib.rs:618) has zero call sites.
  - ❌ boot failure rollback — no rollback/compensation; the shipped equivalent is fail-closed refusal (cycle refusal, ABI-mismatch boot refusal, refused reload degrades) — all executed.
  - ✅ app boot snapshot — `/meta/boot` + runtime-status tests executed.
  - ✅ provider test harness — `build_app_state` / `build_app_state_with_backend` drive every executed demo suite.
- **Gaps**: deferred/lazy loading and rollback remain unimplemented (disclose or build); `DuplicateProvider` test; note the doc discrepancy below.

### LF-007 — Contracts / interfaces (P0) — PARTIALLY VERIFIED

- **Contract**: applications and packages depend on stable abstractions; drivers swap without coupling; conformance is validated per adapter.
- **Implementation**: ~40 `pub trait` ports — e.g. `ModelStore` (`crates/data/models/src/store.rs:210`), `JobStore`/`TenantJobStore`/`JobStoreAdmin` (`crates/async/jobs/src/lib.rs`), `SessionStore`, `TokenStore`, `PrincipalStore`/`CredentialStore`, `SignedUrlPort`, `Mailer`; adapters `crates/data/postgres`, `crates/data/mysql`, `adapters/*`; fakes `testing/testkit/src/fakes.rs` + `InMemoryModelStore` (`store.rs:969`). Compatibility: `MODULE_ABI = 1` (`crates/application/registry/src/lib.rs:40`), graph ABI gate, `crates/foundation/contracts` api-registry. `cargo xtask semver` exists but is wired to no CI job.
- **Executed evidence**: registry **108/108** ✅ (`mismatched_module_abi_is_rejected_by_the_gate`, duplicate/route/manifest gates); MySQL driver matrix **15/0/0** ✅ earlier this session (§2.3) with `pg_*` self-skipping on a `mysql://` URL; **shared adapter-conformance suite** ✅ (§2.5): 13 cases executed green on MySQL + 12 on the in-memory fake; pg half wired but self-skipped on this host. The suite caught **F6** on its first live run (`list(limit)` `total` diverged between the fake and both SQL adapters) — fixed, regressions green (`P0-MYSQL-VERIFICATION-REPORT.md` §9).
- **Required-test coverage**:
  - ✅ contract conformance suite per adapter — `kelson_models::conformance` (this batch): one case list, every backend; adapters supply only pool + migrated schema + seeded tenants. MySQL 13/13 and the fake 12/12 executed; PG wired (`pg_conformance_*` 13/13 self-skip here — no PG on this host).
  - ✅ compatibility checks — ABI + graph version gates executed; contracts-crate registry tests exist (`registry_is_valid`, deprecation carry, invalid-entry refusal) but were not executed this pass.
  - 🟡 fake implementation contract — the same case code now runs against `InMemoryModelStore` (12/12), so fake ≡ adapters for everything the fake can host; the database-only case (schema index guard) is outside its scope by design. **One divergence recorded, not reconciled**: raw `store.update` on a *trashed* row — the fake veils it (`None`), MySQL writes (`Some`); pg `modelstore.rs:540` carries the same no-veil structure but is **NOT RUN**. Probe executed this batch; adjudication (align fake vs align adapters) pending.
  - ❌ semver/API stability — no semver step in `.github/workflows/keel.yml` (jobs: check, clippy ×3, test, llvm-cov, dep-rules, MySQL matrix); `xtask semver` unwired. The new conformance target is likewise not wired into CI (`keel.yml` runs `--test pg_integration` only; the file is under the program's no-touch constraint) — one-line addition proposed.
- **Gaps**: semver wiring; PG execution of the conformance target (host fork failure) + its CI wiring; reconcile the trashed-update divergence.

### LF-008 — Facades (P3) — EXCLUDED (documented)

- **Exclusion**: `platform/docs/18-GAP-ANALYSIS-LARAVEL.md` §26.3 ("Permanent exclusions (not backlog)": facades/global helpers are design decisions, not deferred work), re-asserted in `docs/rust/13-FRAMEWORK-BLUEPRINT.md`; `LARAVEL-PARITY-MATRIX.md` permanent-exclusions table lists Facades → "Explicit dependency injection over global helpers".
- **Replacement alternative (present)**:
  - Typed constructor/handle injection with no reflection and no string service location; explicit lifetimes — `platform/docs/15-EXTENSION.md` §"Providers and lifecycle" and `crates/foundation/lifecycle` (`ServiceLifetime`).
  - Short-lived typed handles: `runtime/kelson-demo/src/host.rs` `*Ref` newtypes and `AppState` / `Extension` extractors.
  - CLI/interactive ergonomics: `kelson shell` (Tinker-class REPL; capability-checked) and `kelson call`.
  - Testability: `testing/testkit` fakes + InMemory stores — exercised by every executed suite in §2.1.
- **Row-notes coverage** (the row asks to exclude the syntax, not the capabilities, and to document the alternative): ✅ injection/fakes/CLI exist and are exercised; ✅ no global mutable facade state exists by construction; 🟡 the "typed injection is the facade replacement" mapping is stated in scattered docs but never collected into one migration note.
- **Status**: EXCLUDED (documented) — no test gap blocks it; only ergonomics documentation could be improved.

### LF-009 — Package development and discovery (P1) — PARTIALLY VERIFIED

- **Contract**: installable packages register services and resources deterministically, declare version/compatibility constraints, and do not break host boot.
- **Implementation**: `crates/application/registry/src/packages.rs` (`PackageIndex`, wasm entries, pins `verify_pins_against_index`), `discovery.rs` (index ↔ layout merge, fail-closed), console `pkg:list`, `pkg:add`, `publish` (materialize migrations/config/assets; idempotent; `--force` for conflicts), `config:publish`; wasm ABI detection/declared-ABI checks in `crates/extension/wasm`; `kelson upgrade` is a **report only** (applies nothing).
- **Executed evidence**: registry **108/108** ✅ (packages 13: absent index empty + typo fails closed, pin verify/refuse, sidecar discovery, manifest merge fail-closed with entry context); console wave8 **4/4** ✅ (`pkg:add` validation, publish idempotence byte-compared, conflicts/`--force`/refusals, config publish); `package_routes_serve_over_http_and_abi_mismatch_refuses_boot` ✅; F3 discovery repro ✅ (§2.3).
- **Required-test coverage**:
  - ❌ fresh install — no registry fetch/install flow; `pkg:add` validates and indexes a local path only.
  - ✅ package discovery — index↔layout merge + dedupe + typo/absence fail-closed executed.
  - ✅ publish idempotence — executed (identical files skipped; differing files conflict unless `--force`).
  - 🟡 version conflict rejection — equality-only: pins refuse a different installed version; duplicate name/dir refusal; publish conflicts; semver-range resolution is absent.
  - ❌ uninstall/upgrade — no uninstall path; `kelson upgrade` reports only.
  - ✅ resource collision tests — publish conflict/force + config-publish idempotence executed.
- **Gaps**: install/uninstall/upgrade flows (or an explicit "local-only" disclosure); semver ranges; registry-fetch publish.

### LF-010 — Request-scoped context (P1) — PARTIALLY VERIFIED

- **Contract**: correlation and explicitly propagated values follow a request/job across async boundaries and are cleared before reuse; tenant/auth context is unforgeable.
- **Implementation**: `crates/foundation/context` `ExecutionContext` (trace/request/correlation/causation ids; builder invariants); admission extracts request/trace ids and builds the context per request (demo `graph_routes.rs`); `telemetry_context` middleware; `DurableJob` carries tenant/correlation/causation fields (jobs `lib.rs`), `TenantJobStore`; outbox events carry tenant + idempotency from payload correlation; notification queue envelopes carry locale/principal/tenant.
- **Executed evidence**: `kelson-context` **6/6** ✅ (`full_context_builds`, `tenant_plane_requires_tenant`, `platform_plane_allows_no_tenant`, `mutation_requires_scope`, `trace_id_autogenerated_and_overridable`, timezone defaults); `kelson-jobs` tenant isolation **3/3** ✅ (claim never crosses tenants; enqueue stamps tenant and rejects mismatch; cross-tenant complete/fail invisible); `kelson-event` tenant isolation **2/2** ✅ (events pump only to the owner tenant); notifications queue round-trip ✅; redaction: `debug_detail_redacts_secrets_through_the_logging_list` ✅ (in the 9/9 error-rendering suite).
- **Required-test coverage**:
  - ✅ propagation HTTP → queue → event — context construction/gating executed; queue + notification round trips executed; **trace/correlation ARE now stamped into the job rows** (see the gap note below). Tenant propagation is wired and tested.
  - ✅ missing context — tenant-plane refusal + mutation-without-scope refusal executed; `queue_via_without_a_port_fails_closed` exists (not executed this pass).
  - ✅ privacy redaction — executed (log list + error surfaces).
  - ✅ long-lived worker cross-request leak — **now covered** (row 6): `kelson-worker`'s
    `a_long_lived_worker_leaks_nothing_between_jobs` drains 40 jobs (each in its own tenant, with
    its own payload marker and its own correlation) through the worker's **real** claim+dispatch
    pass and asserts that every dispatch saw only its own job, every job ended `Succeeded` (a
    leaked claim token would make the completion a CONFLICT), the queue drained, and every row kept
    its own correlation. `a_long_lived_worker_never_crosses_tenants` is the tenant-scoped twin: a
    worker claiming for tenant A never sees tenant B's job, even after many jobs.
  - 🟡 async concurrency isolation — jobs/events tenant tests executed; the concurrent multi-tenant hammer (`g18_isolation_hammer_no_cross_tenant_leakage`) exists in `kelson-observability` and runs in CI, but was not executed this pass.
- **The correlation gap is CLOSED (2026-10-20).** The finding above said "trace/correlation are not
  stamped into job rows by default (job builder writes `None`; the transport enqueues without the
  execution context)". That was true and is now fixed end to end:
  - `kelson-jobs`: `DurableJob::with_origin(correlation_id, causation_id)` stamps the origin on a
    job. A **platform** job (a scheduler tick, an admin sweep) has no inbound request, so it keeps
    the empty origin — that is the correct value there, not a missing stamp.
  - The **module seam** (`kelson-module-contract`'s `NotificationEnvelope`) now carries the host's
    correlation (the host fills it from the executing context, never from the guest — the same
    rule as the principal/tenant/locale), and the notification service's dispatch context carries it
    as a `DeliveryOrigin`.
  - The **enqueue seam** (the host's `JobQueuePort`) stamps the origin onto the durable row.
  - **Evidence**: `kelson-jobs` — `a_job_carries_its_origin` and
    `the_origin_survives_the_store_round_trip`; `kelson-notifications` —
    `a_queued_delivery_carries_the_dispatch_origin` and
    `a_dispatch_without_a_request_carries_an_empty_origin`; `kelson-demo` — the queued-delivery
    round trip now asserts the **claimed job row** carries the correlation; and the live matrix —
    `mysql_a_traced_job_row_carries_the_request_correlation` proves the row **in the database**
    carries it (write → claim read-back).
- **Gaps**: worker cross-job leak test; run the g18 hammer locally.

## 4. Corrections to Prior Documents

Findings from this pass that contradict or refine earlier documents (none of these files were modified):

1. **`LARAVEL-FEATURE-ATLAS-COMPLETE.md`** — LF-001/LF-003 cite `cargo build --workspace ✅`; that is a compile claim, not an install/structure test. LF-005 "Unit tests ✅" and LF-007 "Contract tests ✅" were unsupported as written: there **was no shared contract/conformance suite** (0 symbol matches) — **closed in the conformance batch** (`kelson_models::conformance`: 13 cases, executed 12/12 on the fake + 13/13 on MySQL; `P0-MYSQL-VERIFICATION-REPORT.md` §9), so the LF-007 claim is now unsupported only for semver and CI wiring — and LF-009's "wasm_tests ✅" covers only the WASM facet, not install/uninstall/upgrade. LF-002's "(4 reload tests)" undercounts — the config crate executed 8 runtime reload/rollback tests (61 total) plus 3 demo tests.
2. **`P0-FEATURE-VERIFICATION.md`** — rows LF-001 ("cargo build --workspace succeeds"), LF-004 ("HTTP integration tests pass"), LF-005 ("Unit tests pass"), LF-007 ("Contract tests exist") are evidence-thin; this document refines them to PARTIALLY VERIFIED with explicit gaps. (LF-002's row is accurate but undercounts reload coverage.)
3. **`platform/docs/15-EXTENSION.md`** — the module-lifecycle annotation ("no `ModuleState` type exists"; "DEGRADED / DISABLED **not represented in v1**") is **stale**: `ModuleState` exists with all 7 variants (`crates/foundation/lifecycle/src/lib.rs:49`), and the live degrade→recover→disable→readyz roundtrip is executed green (LF-006). The document's "five of seven states have a representation" table should be updated.
4. **`docs/22-INSTALLER.md`** — describes the legacy PHP-era installer (PHP version/extension checks, `.env` writes, `routes/install.php`), not the Rust `kelson new` flow. Keep for the migrated app's history, but do not cite it as the framework installer.
5. **`REGRESSION-TEST-PLAN.md`** §8 "No-Rebuild (NOT YET TESTED)" — still true as an **automated** regression test, but the experiment itself has now been run manually with full evidence (`P0-MYSQL-VERIFICATION-REPORT.md` §2); an automated digest-comparison test remains a gap. The CI snippet in §"CI Integration" targets `.github/workflows/keel.yml`, which exists (untracked) at the app root and does not yet contain those regression steps.

## 5. Summary

| Status | Count | Features |
|---|---|---|
| VERIFIED | 0 | — |
| PARTIALLY VERIFIED | 9 | LF-001…LF-007, LF-009, LF-010 |
| EXCLUDED (documented) | 1 | LF-008 |
| NOT RUN | 0 rows | (row-level; see per-feature NOT RUN items) |

- **275 test functions executed in this pass, 0 failures, 0 skipped**, plus 5 live CLI proofs and the prior-session MySQL/no-rebuild/F3 evidence. The LF-007 conformance batch adds **181 test functions (168 executed, 13 PG-conformance cases self-skipped; 0 failures)**, 2 clippy gates, and dep-rules (§2.5).
- The first pass found no functional defect in LF-001…LF-010; the LF-007 conformance batch that followed caught **F6** (adapter `total` under `limit` diverged from the pinned fake contract — fixed, §2.5) and **one recorded, unreconciled divergence** (raw `update` on a trashed row: the fake veils, MySQL writes, pg source-identical but NOT RUN). The partial verdicts come from required-test items that do not exist yet (clean-machine install, config isolation, long-lived leak soak, semver gate, worker trace propagation; shared adapter conformance now exists) and from three capabilities that are simply not implemented (LF-006 deferred loading + rollback, LF-009 install/uninstall/upgrade; LF-005 request-scoped containers are declared-but-not-built). All three are disclosed; none breaks the documented behavior contracts.
- **Blocked here**: Docker build, PostgreSQL-backed execution, clean-machine install — infrastructure limits of this host, not code findings.
- Suggested follow-up order: (1) adapter conformance harness (LF-007) — **done this batch** (suite landed; executed on the fake + MySQL; remaining sub-items: PG execution, CI wiring via a one-line `keel.yml` addition — file frozen, trashed-update adjudication); (2) graceful-shutdown test + worker leak soak (LF-004/010), (3) automated scaffold-build job (LF-001/003), (4) decide deferred-loading/rollback and scoped-container posture (LF-005/006), (5) config-isolation definition + absent-config test (LF-002), (6) package install/uninstall/upgrade decision (LF-009).
