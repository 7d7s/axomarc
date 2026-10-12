# 20 — Architecture verification

An essay-vs-codebase audit of the Kelson runtime architecture proposal, and the
work this record schedules.

Part 1 is the audit: every architectural claim in the proposal, checked against
the shipped code with a `file:line` citation. Part 2 is the plan those findings
produced. Part 3 is the backlog — what is deliberately not being built.

This file is the working record for that plan. Sections 5 and 6 are updated as
waves land; sections 1 and 2 are written once and then amended by record, never
rewritten.

## 1. Method and honest limits

**What was checked.** The proposal's architecture claims, cross-verified against
both workspace trees (`platform/` and its `platform/keel-platform/` twin) by
reading code, manifests, migrations and specs. Where the two trees differ, the
citation names the tree.

**How evidence was read.** Static reading only. Every verdict below carries a
`file:line` citation; a verdict without one is not in this document.

**Limits, stated up front:**

- **No live database or rollout observation.** PostgreSQL (5432) and Redis (6379)
  are closed on this machine, so row-level security behaviour and multi-node
  promotion were read from SQL and Rust, not observed at runtime. MySQL 8.0.44
  is available and was used where MySQL paths mattered.
- **Two trees.** `platform/` and `platform/keel-platform/` are near-mirrors that
  have diverged. `keel-platform/` is ahead on the WASM write imports, the
  capability profiles, the native-image refusal test and three `20261005*`
  migrations; the root tree is ahead on ADRs 0014–0018. The divergence is real
  and is not summarised away here.
- **The root tree is under an active 0.2a port.** Its dirty files are not mine to
  fix. Findings that live in the root tree are recorded, and the fix lands on
  the `keel-platform/` side until the port settles.

## 2. The verdict

Twenty claims, grouped. `EXISTS` = shipped and cited. `PARTIAL` = shipped in a
weaker form than claimed. `ABSENT` = not in the codebase.

### Registry and dispatch

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 1 | A capability registry maps stable string handler IDs to implementations | EXISTS | `CommandBus { handlers: HashMap<OperationId, Arc<dyn OperationHandler>> }` — `crates/execution/command/src/lib.rs:17-19`; duplicate registration refused `:30-38`; unknown ID → not-found `:50-62`. Stable IDs compile in: `OP_BOOK = "appointments.book"` — `applications/appointments/src/lib.rs:27-30`. Populated at boot from compiled-in handlers — `runtime/kelson-demo/src/lib.rs:1376-1399`, `runtime/kelson-demo/src/host.rs:307-354`. Not runtime-populated. |
| 2 | Routes are declared as data and validated against the registry | EXISTS (boot-time) | `[[routes]] method/path/operation/capability` — `applications/appointments/module.toml:23-28`; parsed from disk without recompiling — `runtime/kelson-demo/src/graph_routes.rs:88-108`; unknown operation → `GRAPH-ROUTEFILE-OP` — `crates/application/registry/src/lib.rs:945-965`; becomes axum routes at boot — `runtime/kelson-demo/src/graph_routes.rs:344-358`. |
| 3 | A route can change without rebuilding the application | PARTIAL | What hot-swaps is the *resolved chain*, not the route set: `BundleCell = Arc<RwLock<Arc<Router>>>` — `runtime/kelson-demo/src/hot_reload.rs:45-48`; reload refuses a stale artifact and keeps the current bundle — `:136-152`. The `PlatformGraph` itself is a boot-time `Arc` — `runtime/kelson-demo/src/lib.rs:873`. Adding or removing a route still needs a restart. |

### Snapshots and generations

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 4 | An immutable `RuntimeSnapshot` with a per-request generation | ABSENT | Zero `.rs` matches for `RuntimeSnapshot`. The root-tree ADR 0015 (`platform/specs/ADR/0015-runtime-snapshot.md`, untracked — owned by the 0.2a port) states the gap at `:22-24`: "there is no named generation carried through the request path". `arc_swap` is not a dependency (`:90-95`). |
| 5 | Per-subsystem revisions exist instead | EXISTS | Config: `ConfigSnapshot { version, config, hash }` with a monotonic revision — `crates/application/configuration/src/runtime.rs:43-52`, swapped through `RwLock<Arc<…>>` `:136`, commit `:221-252`, rollback `:269-286`. Authorization: `AuthorizationRevision` — `crates/foundation/core/src/revision.rs:44-47`, stale-deny `crates/security/policies/src/lib.rs:139-149`. Feature: `FeatureSnapshot` carried on the context — `crates/foundation/context/src/lib.rs:95-99`. Routes: pinned to `manifests_hash` — `crates/application/registry/src/route_plan.rs:4-9`. No single generation unifies them. |

### Feature flags and configuration

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 6 | Feature flags with a revision, swappable at runtime | EXISTS | Deterministic pure `evaluate` — `crates/enterprise/feature-flags/src/lib.rs:180-212`; boot-time catalog, unknown flags cannot be evaluated — `:218-247`; revision bumped per mutation — `:312` (memory) `:416` (file); ambient set hot-swapped by `FlagsReloader` — `runtime/kelson-demo/src/hot_reload.rs:177-235`. Boot-structural flags stay boot-frozen on purpose (`:18-21`). Not one unified snapshot. |
| 7 | A file watcher drives dev reload | PARTIAL | mtime **polling**, not a watcher: "no watcher dependency ships in this crate" — `crates/application/configuration/src/runtime.rs:10-14`; 500 ms poll loop — `runtime/kelson-demo/src/lib.rs:977-987`. SIGHUP and etcd/consul are named as plug points only. |

### Security pipeline

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 8 | Mandatory security stage order that extensions cannot reorder | TRUE in the engine, FALSE at the HTTP chain | Engine order is fixed: `plan.validate()` → plane invariant → deadline → authorization boundary → concurrency lease → idempotency → backend `run` — `crates/execution/engine/src/engine.rs:156-226`. The HTTP per-route chain is data-driven and removable: `public = true` strips `auth`+`verified` — `crates/application/registry/src/middleware_catalog.rs:256-258`; `excluded_middleware` can empty a chain entirely — test `:485-493`. The backstop that does hold: dispatch refuses anonymous access on a non-public route regardless of chain — `runtime/kelson-demo/src/graph_routes.rs:1683-1710`. |
| 9 | PostgreSQL row-level security as defense in depth | EXISTS with a fail-open hole | Policies read `tenant_id = kelson_current_tenant() OR kelson_current_tenant() IS NULL` — `database/migrations/20260819000001_foundation.sql:185-200`. With no GUC set every row is visible. The runtime always sets the GUC per transaction — `crates/data/postgres/src/tx.rs:119-126` — and verifies it on session commit — `crates/data/postgres/src/lib.rs:218-264`. MySQL has no equivalent and says so: "No RLS … explicit tenant_id binds are the whole isolation story" — `crates/data/mysql/src/tx.rs:6-13`. |
| 10 | Policy epochs for emergency revocation | EXISTS (deny-on-stale) | `policy.stale-revision` deny — `crates/security/policies/src/lib.rs:139-149`. `codes::STALE_REVISION = "AUTH_STALE_REVISION"` — `crates/foundation/core/src/error.rs:41` — but it is surfaced only through a test helper, `:262-271`. The live enforcement is the deny decision, not the error code. |

### WASM extension boundary

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 11 | WebAssembly Component Model with WIT interfaces | ABSENT by explicit decision | `wasmtime = { version = "36", default-features = false, features = ["cranelift", "runtime", "wat", "std"] }` — `platform/Cargo.toml:219` and its twin `platform/keel-platform/Cargo.toml:202`; the governance comment at `platform/Cargo.toml:201-218` records the security bump and states "no component-model". Zero `.wit` files, no `wasmtime-wasi`, no `wasmtime::component`. Guest ABI v1 is hand-rolled: `memory`, `kelson_manifest`, `kelson_alloc`, `kelson_call` — `crates/extension/wasm/src/lib.rs:298-306`. |
| 12 | Native `.so`/`.dll` plugin loading must be avoided | EXISTS and is enforced | No `libloading`, no `dlopen`, no native image handling anywhere in the dependency graph. The refusal is test-pinned: `wasm_only_boundary_refuses_native_images` — `keel-platform` `runtime/kelson-demo/src/wasm_tests.rs:236-280` — feeds ELF, PE and Mach-O images and asserts each is refused at compile, never treated as a library. |
| 13 | A module manifest declaring identity, version, compatibility, capabilities | EXISTS; resources ABSENT | `ModuleManifest` — `crates/application/registry/src/lib.rs:604-650`; `MODULE_ABI: u32 = 1` `:37`; mismatch → `GRAPH-MANIFEST-ABI` `:817-828`. Capability profiles — `crates/extension/wasm/src/lib.rs:100-127`. No `[resources]` section exists in any manifest; ADR 0016 (root tree, untracked) records it as backlog at `:72-74`. |
| 14 | Sandboxed execution with resource budgets | PARTIAL | Fuel (10M) — `crates/extension/wasm/src/lib.rs:61`, consumed `:230`, per-store `:268`, exhaustion → `KSN_WASM_FUEL` `:686`. Memory cap (16 MiB) — `:64`, `StoreLimitsBuilder` `:250-252`, `:398-400`. Guest-string copy bound 1 MiB — `crates/extension/wasm/src/imports.rs:40`. **Absent:** epoch interruption, wall-clock deadline, concurrency cap, per-module configurable limits. |
| 15 | Capability-limited host API | EXISTS; tenant scoping PARTIAL | Every call checks the sidecar-declared set — `crates/extension/wasm/src/imports.rs:344` — under a reviewed sidecar the guest cannot exceed (`crates/extension/wasm/src/lib.rs:71-72`, drift refusal `:327-332`); path checks fail closed without a scope `:350-358`; write imports gated by profile `:395-397`; undefined imports (e.g. `sock_*`) fail at link time — `crates/extension/wasm/src/imports.rs:7-10`. But `CallState` carries no tenant ID (`crates/extension/wasm/src/lib.rs:67-90`): tenant isolation on this path rests on the engine's pre-dispatch authorization, not on the host functions themselves. |
| 16 | A staged module lifecycle (prepare/activate/drain) | ABSENT | No `ModuleState` enum exists. `docs/15-EXTENSION.md:25-26` describes `DISCOVERED -> VALIDATED -> REGISTERED -> INITIALIZED -> READY -> DEGRADED -> DISABLED`; only the first three have a real representation. ADR 0016 `:68-77` defers `prepare → commit → drain` to a later phase. |

### Releases and artifacts

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 17 | A release manifest binding kernel, modules and schema by digest | PARTIAL | `ReleaseManifest` records version, profile, artifacts, git identity, rustc and the shipped migration set — `crates/enterprise/deployment/src/lib.rs:68-92`. Migrations carry `sha256` `:96-106` so post-build drift is detectable. `Artifact` is `{name, version, profile}` with **no hash** — `:59-64`. Module pins are version strings, not digests. |
| 18 | Signed artifacts, publisher identity, TUF | ABSENT | `kelson-integrity` covers audit rows only: sha256 prev-hash chaining per `(tenant, day)` and HMAC-SHA256 segment heads — `crates/security/integrity/src/lib.rs:1-12`, `crates/security/integrity/src/chain.rs:34-40`, `crates/security/integrity/src/hmac.rs:12-34`. No asymmetric signatures, no publisher identity, no SBOM. |
| 19 | A staged, multi-node promotion pipeline | PARTIAL (local only) | `PROMOTION_STAGES = ["post-deploy", "post-migrate", "post-traffic", "canary"]` — `crates/enterprise/deployment/src/promote.rs:30`; fail-closed probe wiring `:80-98`; `PromotionState { Built, Staged, Deployed, RolledBack }` — `crates/enterprise/deployment/src/lib.rs:112`. The `canary` stage runs a health probe and nothing else (`crates/enterprise/deployment/src/promote.rs:190`) — no traffic shifting. Rollback is process-local: "Revisions are minted per process, so a `kelson` invocation boots its own runtime at revision 1" — `tools/kelson-console/src/promotion.rs:9-14`. No fleet orchestration exists. |
| 19b | Module version pins control what executes | ABSENT | `ModulePinAdapter` writes `storage/framework/module-pins.json` — `tools/kelson-console/src/promotion.rs:347-410`. Nothing in the WASM loader reads it. Root-tree ADR 0017 `:38-41` states it plainly: "module pins have no runtime host reader (written, never consumed)". |

### Persistence and durable work

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 20 | Transactional outbox | EXISTS | Domain write, delta, audit and outbox insert share one transaction, then commit — `keel-platform` `crates/data/postgres/src/tx.rs:339-369`; MySQL twin `crates/data/mysql/src/tx.rs:500-530`; the key is computed at append time inside the commit so row and key are atomic. Delivery is a separate at-least-once pump — `crates/async/events/src/lib.rs:354-404`. **Honest limit:** no database test asserts "handler failure ⇒ zero outbox rows"; the evidence is the shared-transaction code and a design note in `database/migrations/20261005000002_outbox_idempotency_key.sql:8-9`. |
| 21 | Versioned job payloads | PARTIAL (twin tree only) | `schema_version` column — `keel-platform` `database/migrations/20261005000003_jobs_queue_schema_version.sql:6` (the file is six lines; the `ALTER` is the last one); migrate-on-claim with fail-closed on a newer schema — `crates/async/jobs/src/lib.rs:337-365`; tests `:3032`, `:3039-3067`. No `handler_version` column exists (0 matches). The root tree lacks all three `20261005*` migrations. |
| 22 | Migrations decoupled from kernel compilation | PARTIAL | Singles are compile-time embedded — `crates/data/postgres/src/lib.rs:160`, `crates/data/mysql/src/lib.rs:203`. Pairs are loaded from disk at runtime with required `.down.sql` and Laravel batch semantics — `crates/data/mysql/src/pairs.rs:1-22`. No `migrate:plan`, no dry-run, no expand-and-contract tooling. |

### Terminology

The proposal's "control plane / data plane" split has no counterpart in this
codebase. In code, "plane" means `SecurityPlane` (tenant/platform) or the route
plane (api/web) — `crates/execution/engine/src/engine.rs:158-164` holds the
invariant "an operation runs only on its declared plane". The literal strings
"control plane" appear only in an incident-file CLI
(`tools/kelson-console/src/observe.rs:3,63`) and in unrelated prose. The nearest
real thing is the promotion pipeline, which is a CLI-time library, not a
request-path split.

### What the essay got right that the code does not

Three findings are the proposal predicting a gap and the code confirming it:

1. **RLS fails open without the GUC** (`database/migrations/20260819000001_foundation.sql:185-200`)
   — exactly the class of hole §32 warns about.
2. **Module pins are written and never read** (`tools/kelson-console/src/promotion.rs:347-410`) —
   a rollback that restores a version field nothing consults.
3. **The route set is boot-frozen** (`runtime/kelson-demo/src/lib.rs:873`) — "change a
   route without rebuilding" is true for middleware chains and false for the
   route inventory.

## 3. What this schedules

Three waves, chosen from the findings above rather than from the essay's own
priority list.

| Wave | Scope | Why |
|---|---|---|
| **V** | Publish this record; correct the two places `docs/15-EXTENSION.md` overstates. | The lifecycle paragraph claims states that do not exist. Documentation that outruns the code is the defect this whole exercise exists to catch. |
| **M** | Module-contract gaps: make module pins real, add manifest `[resources]` with tightening-only budgets, add a concurrency cap and a wall-clock deadline, ship `migrate:plan`, content-address release artifacts. | Every item is testable with no database, no root-tree writes and no new dependency. Together they close findings 13, 14, 17, 19b and 22. |
| **A** | Design ABI v2 on the WebAssembly Component Model: an ADR, a feature-flagged enablement, a WIT package layout mirroring the existing host imports, the `prepare → commit → drain` lifecycle, and an in-place upgrade proof. | Chosen over keeping core modules. Sequenced after M because `[resources]` shapes the WIT world contract. |

Explicitly out of scope: Cedar, TUF, distributed rollout, a workflow engine, a
file watcher, and `handler_version` on job rows (payload `schema_version` already
fail-closes). See section 6.

## 4. Constraints in force

- Root `platform/` is under an active 0.2a port. Never stage its dirty files;
  clean files outside the port's dirty set are acceptable. Every new migration
  must land in both trees — **suspended** while the port is live.
- Commit scope is `medicare-complete-…/` only, staged with explicit paths. Never
  stage repo-wide.
- Only the `keel` database may be used; the `keel` database is never reset.
  Personal databases on `:3306` are untouched. `mysqlsh` hangs — use
  `C:\xampp\php\php.exe` to probe.
- One `cargo` process at a time, `-j 2`, `CARGO_INCREMENTAL=0`. `cargo fmt` is
  not enforced; `clippy -D warnings` is a hard gate.
- New error codes are append-only (`crates/foundation/core/src/error.rs:36`).
  Frozen records — STAGE-0-SPEC, STAGE-SENTINEL, the ADRs — are amended by
  record, never rewritten.

## 5. Progress

| Wave | Item | State |
|---|---|---|
| V | `20-ARCHITECTURE-VERIFICATION.md` (this file) | DONE |
| V | `docs/15-EXTENSION.md` lifecycle paragraph corrected | TODO |
| V | CHANGELOG entry | TODO |
| M1 | Module pins gate the loader | TODO |
| M2 | Manifest `[resources]`, concurrency cap, wall-clock deadline | TODO |
| M3 | `kelson migrate:plan` | TODO |
| M4 | Content-addressed release artifacts | TODO |
| M | CHANGELOG + PUBLISHING entries | TODO |
| A | ABI v2 ADR and design | TODO |

## 6. Backlog

Recorded, not scheduled. Each carries the finding that produced it.

| Item | Finding | Notes |
|---|---|---|
| `RuntimeSnapshot` with a per-request generation | 4 | Root-tree ADR 0015 exists but is untracked and owned by the 0.2a port. Needs an `ArcSwap`-vs-`RwLock` benchmark before design; do not start while the port is live. |
| RLS `OR … IS NULL` fail-closed | 9 | Requires a **new** migration — editing an applied, checksummed file is illegal — so it is suspended with the two-tree rule. Also untestable while PostgreSQL is closed. |
| Wire `AUTH_STALE_REVISION` into a real surface, or document that the deny decision is the enforcement | 10 | The error code exists and is reachable only from a test helper. |
| Change-impact engine (`change:plan` / `change:validate` / `change:apply`) | — | The proposal's headline differentiator. Derives most of its value from generations (finding 4) landing first. |
| Distributed rollout with readiness and canary | 19 | Root-tree ADR 0017 defers it to Phase 3. |
| Artifact signing, publisher identity, TUF | 18 | Keep the trust model small for first-party deployments; design verifiable artifacts now, evolve distribution later. |
| Cedar as the policy language | 10 | Homegrown `kelson-policy` ships and is tested. Evaluate only with a concrete migration path. |
| File-watcher dev reload | 7 | Polling is documented and adequate; SIGHUP is a named plug point. |
| `handler_version` on job rows | 21 | Payload `schema_version` already fail-closes on a newer schema. |
| Expand-and-contract migration tooling | 22 | Documented practice, no tooling. |
| Route-set hot reload (not just chains) | 3 | Needs the graph to become swappable rather than boot-frozen. |

## 7. Ledger conventions

- **Changelog.** `platform/CHANGELOG.md` (single shared copy, `[Unreleased]`
  section, git-cliff conventional-commit format). One entry per landed commit,
  written in the established shape: the gap, why the obvious fix was wrong, what
  changed, and the evidence.
- **Publishable notes.** `platform/PUBLISHING.md`. Wave M touches four publishable
  crates — `kelson-registry`, `kelson-deployment`, `kelson-foundation`,
  `kelson-console` — so its notes belong there; `kelson-wasm` and `kelson-demo`
  are `publish = false` and are not part of any publish wave.
- **Backlog.** Section 6 above.
