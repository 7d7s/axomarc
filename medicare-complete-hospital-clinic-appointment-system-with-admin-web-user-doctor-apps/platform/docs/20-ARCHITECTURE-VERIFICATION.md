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
| 1 | A capability registry maps stable string handler IDs to implementations | EXISTS | `CommandBus { handlers: HashMap<OperationId, Arc<dyn OperationHandler>> }` — `crates/execution/command/src/lib.rs:17-19`; duplicate registration refused `:30-38`; unknown ID → not-found `:50-62`. Stable IDs compile in: `OP_BOOK = "appointments.book"` — `applications/appointments/src/lib.rs:27-30`. Populated at boot from compiled-in handlers — `platform/` `runtime/kelson-demo/src/lib.rs:1388-1411`, `runtime/kelson-demo/src/host.rs:307-354`. Not runtime-populated. |
| 2 | Routes are declared as data and validated against the registry | EXISTS (boot-time) | `[[routes]] method/path/operation/capability` — `applications/appointments/module.toml:23-28`; parsed from disk without recompiling — `runtime/kelson-demo/src/graph_routes.rs:88-108`; unknown operation → `GRAPH-ROUTEFILE-OP` — `platform/` `crates/application/registry/src/lib.rs:945-965`; becomes axum routes at boot — `runtime/kelson-demo/src/graph_routes.rs:344-358`. |
| 3 | A route can change without rebuilding the application | PARTIAL | What hot-swaps is the *resolved chain*, not the route set: `BundleCell = Arc<RwLock<Arc<Router>>>` — `runtime/kelson-demo/src/hot_reload.rs:45-48`; reload refuses a stale artifact and keeps the current bundle — `:136-152`. The `PlatformGraph` itself is a boot-time `Arc` — `platform/` `runtime/kelson-demo/src/lib.rs:885`. Adding or removing a route still needs a restart. |

### Snapshots and generations

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 4 | An immutable `RuntimeSnapshot` with a per-request generation | ABSENT | Zero `.rs` matches for `RuntimeSnapshot`. The root-tree ADR 0015 (`platform/specs/ADR/0015-runtime-snapshot.md`, untracked — owned by the 0.2a port) states the gap at `:22-24`: "there is no named generation carried through the request path". `arc_swap` is not a dependency (`:90-95`). |
| 5 | Per-subsystem revisions exist instead | EXISTS | Config: `ConfigSnapshot { version, config, hash }` with a monotonic revision — `crates/application/configuration/src/runtime.rs:43-52`, swapped through `RwLock<Arc<…>>` `:136`, commit `:221-252`, rollback `:269-286`. Authorization: `AuthorizationRevision` — `crates/foundation/core/src/revision.rs:44-47`, stale-deny `crates/security/policies/src/lib.rs:139-149`. Feature: `FeatureSnapshot` carried on the context — `crates/foundation/context/src/lib.rs:95-99`. Routes: pinned to `manifests_hash` — `crates/application/registry/src/route_plan.rs:4-9`. No single generation unifies them. |

### Feature flags and configuration

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 6 | Feature flags with a revision, swappable at runtime | EXISTS | Deterministic pure `evaluate` — `crates/enterprise/feature-flags/src/lib.rs:180-212`; boot-time catalog, unknown flags cannot be evaluated — `:218-247`; revision bumped per mutation — `:312` (memory) `:416` (file); ambient set hot-swapped by `FlagsReloader` — `runtime/kelson-demo/src/hot_reload.rs:177-235`. Boot-structural flags stay boot-frozen on purpose (`:18-21`). Not one unified snapshot. |
| 7 | A file watcher drives dev reload | PARTIAL | mtime **polling**, not a watcher: "no watcher dependency ships in this crate" — `crates/application/configuration/src/runtime.rs:10-14`; 500 ms poll loop — `platform/` `runtime/kelson-demo/src/lib.rs:989-999`. SIGHUP and etcd/consul are named as plug points only. |

### Security pipeline

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 8 | Mandatory security stage order that extensions cannot reorder | TRUE in the engine, FALSE at the HTTP chain | Engine order is fixed: `plan.validate()` → plane invariant → deadline → authorization boundary → concurrency lease → idempotency → backend `run` — `crates/execution/engine/src/engine.rs:156-226`. The HTTP per-route chain is data-driven and removable: `public = true` strips `auth`+`verified` — `crates/application/registry/src/middleware_catalog.rs:256-258`; `excluded_middleware` can empty a chain entirely — test `:485-493`. The backstop that does hold: dispatch refuses anonymous access on a non-public route regardless of chain — `runtime/kelson-demo/src/graph_routes.rs:1683-1710`. |
| 9 | PostgreSQL row-level security as defense in depth | EXISTS with a fail-open hole | Policies read `tenant_id = kelson_current_tenant() OR kelson_current_tenant() IS NULL` — `database/migrations/20260819000001_foundation.sql:185-200`. With no GUC set every row is visible. The runtime always sets the GUC per transaction — `crates/data/postgres/src/tx.rs:119-126` — and verifies it on session commit — `crates/data/postgres/src/lib.rs:218-264`. MySQL has no equivalent and says so: "No RLS … explicit tenant_id binds are the whole isolation story" — `crates/data/mysql/src/tx.rs:6-13`. |
| 10 | Policy epochs for emergency revocation | EXISTS (deny-on-stale) | `policy.stale-revision` deny — `crates/security/policies/src/lib.rs:139-149`. `codes::STALE_REVISION = "AUTH_STALE_REVISION"` — `crates/foundation/core/src/error.rs:41` — but it is surfaced only through a test helper, `:262-271`. The live enforcement is the deny decision, not the error code. |

### WASM extension boundary

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 11 | WebAssembly Component Model with WIT interfaces | PARTIAL — opt-in cargo feature present, off by default | `wasmtime = { version = "36", default-features = false, features = ["cranelift", "runtime", "wat", "std"] }` — `platform/Cargo.toml:219` and its twin `platform/keel-platform/Cargo.toml:202`; the governance comment at `platform/Cargo.toml:201-218` records the security bump and states "no component-model". Zero `.wit` files, no `wasmtime-wasi`, no `wasmtime::component`. Guest ABI v1 is hand-rolled and documented at `crates/extension/wasm/src/lib.rs:9-26`; every export is validated at load (`:552-558`) and a missing or mistyped one refuses the boot. Amended on landing: `wasmtime-component-macro` and `wasmtime-component-util` are **no longer absent** from `keel-platform/Cargo.lock` -- they arrive there through `wasmtime`'s own `component-model` feature, which is how cargo records a feature that no default build activates, so absence stopped being the machine-checkable form of "off". The check that replaced it is the counter: feature off, `cargo test --workspace --no-fail-fast` still reads **1392 passed / 0 failed**, and no `.wit` file, no `wasmtime-wasi` and no `wasmtime::component` appear on the default path. The deferral is no longer open-ended: root-tree ADR 0016 §2's reconsideration trigger is now recorded as **fired by decision**, and the sanctioned path -- v1 and v2 coexisting through a transition window, `component-model` as an opt-in cargo feature that is off by default, zero new direct externals -- is `keel-platform` `specs/ADR/0019-wasm-abi-v2-component-model.md`. The feature has now landed, still off by default: `keel-platform` `crates/extension/wasm/Cargo.toml:27-34` declares `[features]` with `default = []` and `component-model = ["wasmtime/component-model"]`, forwarded at `keel-platform` `runtime/kelson-demo/Cargo.toml:23` and driven in tests by the dev-dependency at `:85`. `DEPENDENCY-POLICY.md:46` was amended in this same commit, as ADR 0019 §8 required, and the external count stays **25/25** because a feature on an existing dependency adds no `[workspace.dependencies]` entry (`keel-platform` `tools/kelson-analyzer/src/lib.rs:432-447`). What remains of the deferral is the WIT world and the in-place upgrade proof, not the enablement. |
| 12 | Native `.so`/`.dll` plugin loading must be avoided | EXISTS and is enforced | No `libloading`, no `dlopen`, no native image handling anywhere in the dependency graph. The refusal is test-pinned: `wasm_only_boundary_refuses_native_images` — `keel-platform` `runtime/kelson-demo/src/wasm_tests.rs:236-280` — feeds ELF, PE and Mach-O images and asserts each is refused at compile, never treated as a library. |
| 13 | A module manifest declaring identity, version, compatibility, capabilities | EXISTS; the resources half closed by Wave M2 | `ModuleManifest` — `platform/` `crates/application/registry/src/lib.rs:604-650`; `MODULE_ABI: u32 = 1` `:37`; mismatch → `GRAPH-MANIFEST-ABI` `:817-828`. Capability profiles — `keel-platform` `crates/extension/wasm/src/lib.rs:130`. Per-plugin execution budgets now exist as `[budget]` — `keel-platform` `crates/application/registry/src/lib.rs:659` (field), `:672-692` (section) — deliberately **not** named `[resources]`: that word already means Laravel-style route resources in this same format, and one word meaning two things in one file is how misconfiguration gets written. |
| 14 | Sandboxed execution with resource budgets | EXISTS — the PARTIAL above was closed by Wave M2 | Host ceilings: fuel `DEFAULT_FUEL = 10M` — `keel-platform` `crates/extension/wasm/src/lib.rs:90`, memory `DEFAULT_MEMORY_BYTES = 16 MiB` `:93`, guest-string copy bound 1 MiB — `keel-platform` `crates/extension/wasm/src/imports.rs:40`, enforced `:60` and `:258`. Per-plugin `[budget]` — `keel-platform` `crates/application/registry/src/lib.rs:672-692`, reachable from `ModuleManifest` `:659` — may only **tighten** those ceilings: `validate_against` `:701-736` collects every violation at once and runs at the top of `WasmHost::prepare` (`keel-platform` `crates/extension/wasm/src/lib.rs:584`, `resolve_budget` `:508-522`), which is the phase `WasmHost::load` `:707` is, *before the binary is compiled or instantiated*. Effective values travel together as `CallBudget` `:67-77`, so fuel `:559`, memory `:541-542` and the epoch deadline `:561` cannot describe different sandboxes (the same three are set again in `call()` `:947-967`). Concurrency: `InFlightSlot` `keel-platform` `crates/extension/wasm/src/lib.rs:308-314` is taken at `:913-940`, before `pre.instantiate` at `:971`, and the over-cap path returns `WasmError::Saturated` `:234-237`. Wall clock: `EpochTicker` `keel-platform` `crates/extension/wasm/src/lib.rs:370-401` advances `Engine::increment_epoch` at `:389` from a **monotonic clock** (`:385-391`) rather than per sleep call (`:392`), and `NO_DEADLINE_TICKS` `:403-412` gives no-deadline plugins a deadline far enough out that wasmtime's default of `0` — which it documents as having "always elapsed" — cannot trap them. Codes `KSN_WASM_FUEL` `keel-platform` `crates/foundation/core/src/error.rs:76`, `KSN_WASM_SATURATED` `:85`, `KSN_WASM_DEADLINE` `:95`, each pinned to HTTP 503 at both boot sites — `keel-platform` `runtime/kelson-demo/src/lib.rs:450-463` (production) and `:1588-1599` (harness). |
| 15 | Capability-limited host API | EXISTS; tenant scoping PARTIAL | Every call checks the sidecar-declared set — `keel-platform` `crates/extension/wasm/src/imports.rs:344` — under a reviewed sidecar the guest cannot exceed (`keel-platform` `crates/extension/wasm/src/lib.rs:661-674`, the drift refusal; note both documents have `budget` cleared before the comparison, since it is host policy a guest cannot declare). Path checks fail closed without a scope — `keel-platform` `crates/extension/wasm/src/imports.rs:350-358`; write imports gated by profile `:395-398`; undefined imports (e.g. `sock_*`) fail at link time — `crates/extension/wasm/src/imports.rs:8`. But `CallState` carries no tenant ID (`keel-platform` `crates/extension/wasm/src/lib.rs:96-119`): tenant isolation on this path rests on the engine's pre-dispatch authorization, not on the host functions themselves. |
| 16 | A staged module lifecycle (prepare/activate/drain) | PARTIAL — EXISTS for WASM plugin generations; ABSENT for modules | The staged mechanism itself now **EXISTS**, for a WASM plugin generation: `keel-platform` `crates/extension/wasm/src/lib.rs:171` declares `PluginState` (`Prepared -> Ready -> Draining -> Drained`); `keel-platform` `crates/extension/wasm/src/lib.rs:574` is `WasmHost::prepare`, which compiles, links, ABI-checks and drift-checks and then stops with no registration; `keel-platform` `crates/extension/wasm/src/lib.rs:804` is the host's `commit` and `keel-platform` `crates/extension/wasm/src/lib.rs:835` its `drain`; `keel-platform` `crates/extension/wasm/src/lib.rs:925-932` gates `WasmPlugin::call` so only a `Ready` generation dispatches, reading the state *after* the in-flight counter is incremented, which is what stops a call slipping past a concurrent drain. It is live boot code, not a test-only API: `keel-platform` `runtime/kelson-demo/src/lib.rs:78` prepares and `keel-platform` `runtime/kelson-demo/src/lib.rs:85-87` commits. Four tests pin the semantics — `keel-platform` `runtime/kelson-demo/src/wasm_tests.rs:891` (a prepared generation refuses dispatch until the host commits), `keel-platform` `runtime/kelson-demo/src/wasm_tests.rs:951` (drain is a one-way retire a recommit cannot undo), `keel-platform` `runtime/kelson-demo/src/wasm_tests.rs:1005` (draining a never-committed generation is refused), `keel-platform` `runtime/kelson-demo/src/wasm_tests.rs:1031` (drain waits on a running guest, reports the wait it could not finish, and stays retired afterwards). Still **ABSENT**: no `ModuleState` enum exists, `docs/15-EXTENSION.md:25-26`'s `DISCOVERED -> VALIDATED -> REGISTERED -> INITIALIZED -> READY -> DEGRADED -> DISABLED` has five of its seven states represented (`docs/15-EXTENSION.md:31-36`) and `DEGRADED`/`DISABLED` remain backlog (§6) — a generation lifecycle retires one generation of one plugin and is not that module state machine, so `PluginState::Drained` is not `DISABLED` (`docs/15-EXTENSION.md:41-47`). ADR 0019 §6 sequences it **independently of the Component Model**: sanctioned as additive to v1 by root ADR 0016 §3, it is not gated behind the `component-model` feature, and the in-place upgrade proof must show it for a v1 module before it is shown working for a v2 component. |
| 23 | `MODULE_ABI`'s default is self-referential | EXISTS as a gap (found during A) | `api` is `#[serde(default = "default_api")]` and `default_api()` returns `MODULE_ABI` itself -- `keel-platform` `crates/application/registry/src/lib.rs:779-780`, `:553-555` -- so a manifest that omits `api` parses as whatever the current value is, while the gate `:928-937` hard-fails any manifest that states one. A bump would therefore refuse boot for the three manifests that pinned their version (`applications/appointments/module.toml:4`, `applications/oauth2/module.toml:4`, `templates/starter-template/module.toml:7`) and **silently re-label** the two that did not (`examples/reference/module.toml`, `templates/project-template/module.toml`) -- the inverse of `docs/15-EXTENSION.md:54-56`, whose "never silently reinterprets an older manifest" guarantee holds only for manifests that pin `api`. Wave A worked around it by versioning the guest ABI by artifact shape instead of by `MODULE_ABI` (ADR 0019 §3); the gate itself is unchanged, and whether the default should become a hard error rather than an echo is backlog (§6). |

### Releases and artifacts

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 17 | A release manifest binding kernel, modules and schema by digest | PARTIAL | `ReleaseManifest` records version, profile, artifacts, git identity, rustc and the shipped migration set - `keel-platform crates/enterprise/deployment/src/lib.rs:88-114`. Migrations carry `sha256` - `keel-platform crates/enterprise/deployment/src/lib.rs:116-128` - so post-build drift is detectable. **Wave M4** gave `Artifact` a `sha256: Option<String>` - `keel-platform crates/enterprise/deployment/src/lib.rs:67-86` - which `kelson build` fills from the bytes on disk, so the binaries a release names are now content-addressed too. Two things keep this PARTIAL: module pins are still version strings rather than digests, and the root copy is pre-M4 - `platform/crates/enterprise/deployment/src/lib.rs:58-64` still reads `Artifact` as `{name, version, profile}` with no hash. |
| 18 | Signed artifacts, publisher identity, TUF | ABSENT | `kelson-integrity` covers audit rows only: sha256 prev-hash chaining per `(tenant, day)` and HMAC-SHA256 segment heads — `crates/security/integrity/src/lib.rs:1-12`, `crates/security/integrity/src/chain.rs:34-40`, `crates/security/integrity/src/hmac.rs:12-34`. No asymmetric signatures, no publisher identity, no SBOM. |
| 19 | A staged, multi-node promotion pipeline | PARTIAL (local only) | `PROMOTION_STAGES = ["post-deploy", "post-migrate", "post-traffic", "canary"]` — `crates/enterprise/deployment/src/promote.rs:30`; fail-closed probe wiring `:80-98`; `PromotionState { Built, Staged, Deployed, RolledBack }` - `keel-platform crates/enterprise/deployment/src/lib.rs:134`. The `canary` stage runs a health probe and nothing else (`crates/enterprise/deployment/src/promote.rs:190`) — no traffic shifting. Rollback is process-local: "Revisions are minted per process, so a `kelson` invocation boots its own runtime at revision 1" — `tools/kelson-console/src/promotion.rs:9-14`. No fleet orchestration exists. |
| 19b | Module version pins control what executes | ABSENT → **closed by Wave M1** | `ModulePinAdapter` writes `storage/framework/module-pins.json` — `platform/` `tools/kelson-console/src/promotion.rs:347-410`. Before M1 nothing in the WASM loader read it, and root-tree ADR 0017 `:38-41` stated it plainly: "module pins have no runtime host reader (written, never consumed)". Wave M1 made `kelson-registry` the canonical reader — `keel-platform` `crates/application/registry/src/packages.rs:256-265` (`module_pins`) and `:286-311` (`verify_pins_against_index`), composed by `:315-318` (`verify_pinned_boot`) — called from `load_wasm_plugins` before any guest is instantiated, from `kelson check`, from `kelson graph`, and from the `post_deploy` probe (which previously duplicated the rule). A pin that disagrees with the installed index now refuses boot. See §5.1 for the behaviour change this introduced. |

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
   a rollback that restores a version field nothing consults. **Closed by Wave M1** (§5.1).
3. **The route set is boot-frozen** (`runtime/kelson-demo/src/lib.rs:885`) — "change a
   route without rebuilding" is true for middleware chains and false for the
   route inventory.

## 3. What this schedules

Three waves, chosen from the findings above rather than from the essay's own
priority list.

| Wave | Scope | Why |
|---|---|---|
| **V** | Publish this record; correct the two places `docs/15-EXTENSION.md` overstates. | The lifecycle paragraph claims states that do not exist. Documentation that outruns the code is the defect this whole exercise exists to catch. |
| **M** | Module-contract gaps: make module pins real, add manifest `[budget]` with tightening-only limits, add a concurrency cap and a wall-clock deadline, ship `migrate:plan`, content-address release artifacts. | Every item is testable with no database, no root-tree writes and no new dependency. Together they close findings 13, 14, 17, 19b and 22. |
| **A** | Design ABI v2 on the WebAssembly Component Model: an ADR, a feature-flagged enablement, a WIT package layout mirroring the existing host imports, the `prepare → commit → drain` lifecycle, and an in-place upgrade proof. | Chosen over keeping core modules. Sequenced after M because the per-module manifest section that shapes the WIT world contract is `[budget]`, not `[resources]` -- ADR 0016 §3 named the latter, but Wave M shipped the former (ADR 0019 §7). Sub-deliverables: the ADR (this record), a feature-flagged enablement, a WIT package layout mirroring the four host imports, the `prepare`/`commit`/`drain` lifecycle, and an in-place upgrade proof. |

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
| V | `docs/15-EXTENSION.md` lifecycle paragraph corrected | DONE |
| V | CHANGELOG entry | DONE |
| M1 | Module pins gate the loader | DONE |
| M2 | Manifest `[budget]`, concurrency cap, wall-clock deadline | DONE |
| M3 | `kelson migrate:plan` | DONE |
| M4 | Content-addressed release artifacts | DONE |
| M | CHANGELOG + PUBLISHING entries (M1-M4) | DONE |
| A | ABI v2 ADR and design (`keel-platform/specs/ADR/0019`, index row, this record) | DONE |
| A | Feature-flagged `component-model` enablement | DONE |
| A | WIT package layout mirroring the four host imports | TODO |
| A | `prepare`/`commit`/`drain` staged lifecycle (additive to v1) | DONE |
| A | In-place upgrade proof (v1 -> v1, v2 -> v2, v1 -> v2, one host) | TODO |
| A | `DEPENDENCY-POLICY.md` row 46 amendment (authorized by ADR 0019 §8, applied on landing) | DONE |

### 5.1 Behaviour changes shipped by a wave

Recorded here rather than only in the changelog, because a behaviour change is
the thing a future reader most needs to find next to the verdict it came from.

**M1 — a missing pin file is no longer fatal to `kelson promote`.** The old
`post_deploy_probe` (`platform/` `tools/kelson-console/src/promotion.rs:595-623`,
the `pin file unreadable` refusal at `:600`) called an
absent `module-pins.json` an error, while
`ModulePinAdapter::read()` in the same module seeded its pins *from the index*
for that same absent file. Two adapters, one artifact, two meanings. The probe
now calls the registry rule (`keel-platform`
`tools/kelson-console/src/promotion.rs:612-625`), where absent means "no pins declared" and yields
`Ok(0)` — which is also what makes the boot gate safe to add: pinning stays
opt-in, and a workspace that has never run `kelson promote` boots exactly as
before. A pin file that exists but is malformed still refuses, on both paths.

**M2 — enabling epoch interruption makes a deadline mandatory on every store.**
`Config::epoch_interruption(true)` is set on the engine for *every* host at
`keel-platform` `crates/extension/wasm/src/lib.rs:470`, not
only for plugins that declare a deadline, because the engine's configuration is
fixed at construction and a plugin may declare its budget later. The
consequence is the part a future reader must not rediscover the hard way: a
store's epoch deadline starts at `0`, which wasmtime documents as having "always
elapsed", so a store that is never given one traps its guest before it executes
a single instruction. Every plugin that declares no `execution_timeout_ms` is
therefore given `NO_DEADLINE_TICKS` at `:403-412` (`u64::MAX / 2`, not `u64::MAX`, since the
deadline is computed as `current_epoch + ticks` and the larger value would
overflow it) and it is applied to every store at `:561` and `:967`. All 14
pre-existing WASM tests still pass, which is the evidence
that this was handled rather than assumed.

**M2 — a wall-clock deadline is a coarse bound, and may run late.** The epoch
advances once per tick, so ticks must correspond to milliseconds. Counting one
tick per `sleep(1ms)` does not do that: `sleep` is a lower bound, and on
Windows the default timer resolution lands a 1ms sleep near 15.6ms, which would
have made a declared 50ms deadline fire around 780ms — late by a factor that
varies per machine. The ticker reads a monotonic clock at `keel-platform`
`crates/extension/wasm/src/lib.rs:385` and catches the epoch up
at `:388-391` by the milliseconds actually elapsed. What remains, stated
plainly: the
interrupt can be up to one sleep interval plus OS timer slop *later* than
asked for, but never earlier and never by a multiple. It bounds a runaway, it
does not implement a precise timeout.

**M2 — `KSN_DEADLINE` was deliberately not reused for the plugin deadline.**
`ErrorRenderRegistry::set_status` is a global per-code override, and
`KSN_DEADLINE` already renders from two other sites' own categories: the
execution engine's `Internal`/500 (`crates/execution/engine/src/engine.rs:166-174`)
and a worker job's `Workflow`/409 (`runtime/kelson-worker/src/main.rs:240-247`).
Pinning it to 503 for the plugin case would have silently re-skinned both of
those — and both are marked `Retryability::Retryable` while rendering as a 500
and a 409, which is a pre-existing inconsistency, not one this wave should
quietly change. A plugin's wall-clock budget is a sandbox budget in the same
family as `KSN_WASM_FUEL`, so it takes the sibling code `KSN_WASM_DEADLINE`.

**M2 — the in-flight counter counts uncapped calls too.** `WasmPlugin::in_flight`
is incremented on every call at `keel-platform`
`crates/extension/wasm/src/lib.rs:913-940` and released by the
`InFlightSlot` guard at `:308-314` on every exit path
(declared or not), so it is a usable gauge for an uncapped plugin as well; only
the *refusal* is conditional on `max_concurrency`. An early `?` return that
skipped the release would leak a slot permanently and take the plugin out of
service one failed request at a time.

**A — a WASM generation cannot dispatch before the host commits it.** The
staged lifecycle is enforced in code, not just described: `WasmPlugin::call`
reads the generation state at `keel-platform`
`crates/extension/wasm/src/lib.rs:925-932` and refuses anything that is not
`Ready`, returning a new `WasmError::Lifecycle` variant (`keel-platform`
`crates/extension/wasm/src/lib.rs:252`). The read is deliberately taken *after*
the in-flight counter is incremented, because a call that read `Ready` first and
incremented second could land in a generation `drain` had already declared idle
and run guest code against a retired plugin — either that call sees the retire
state and leaves, or `drain` sees its count and waits. Two entry points now
exist: `WasmHost::prepare` (`keel-platform`
`crates/extension/wasm/src/lib.rs:574`) validates and stops, while
`WasmHost::load` (`:707`) is that plus `commit`, so every existing caller keeps
returning a dispatchable plugin. Production boot uses the split form —
`keel-platform` `runtime/kelson-demo/src/lib.rs:78` prepares, `:85-87` commits
— so the host's registration decision is an explicit separate act. Nothing
observable changed for an operator: no new error code, no new HTTP status pin,
and the boot/dispatch path behaves exactly as before. The one thing
deliberately *not* shipped is the HTTP mapping for a call refused while
draining, which deserves 503 but cannot occur until a generation-swap path
exists; adding a code no code could produce would be policy describing
something untrue.

**A — the `component-model` feature landed, and the record it landed under was
wrong in both directions.** The flag is where ADR 0019 §4 said it would be:
`keel-platform` `crates/extension/wasm/Cargo.toml:27-34`, `default = []` with
`component-model = ["wasmtime/component-model"]`, forwarded at `keel-platform`
`runtime/kelson-demo/Cargo.toml:23`. The workspace dependency at
`platform/keel-platform/Cargo.toml:202` is untouched, so a default build compiles no
`wasmtime::component` at all — which is the property the gate actually checks
now: feature off, `cargo test --workspace --no-fail-fast` still reads **1392
passed / 0 failed**, and with the feature on it reads 1393, the one difference
being the new case at `runtime/kelson-demo/src/wasm_tests.rs:1123`.

Two corrections the experiment forced, both recorded rather than quietly fixed.
The first was the blocker's own premise: "`wat-1.261.0`'s own sources contain
no `component` references" was a literal-string grep, not a reading of the
crate. `wat` 1.261 defaults `component-model = ["wast/component-model"]` on,
`wast` 261 defaults it on too, and `wast-261.0.0/src/wat.rs:44-45` routes a
top-level `(component ...)` to the encoder at
`wast-261.0.0/src/component/binary.rs:13`. The authoring half therefore needed
no dependency change whatsoever — the support was already compiled into every
build we ship. The second is the one that changes what a future reader should
expect: ADR 0019 §4 stated that "no `wit-bindgen` enters the workspace", and
enabling the feature *does* put `wasmtime-internal-wit-bindgen` and
`wit-parser` into `keel-platform/Cargo.lock`, transitively through
`wasmtime-internal-component-macro`, which is wasmtime's own `bindgen!`
macro. What stayed true is the part the cap depends on: those are
*transitive*, no `[workspace.dependencies]` entry is added, and
`keel-platform` `tools/kelson-analyzer/src/lib.rs:432-447` still counts
**25/25**. So the ADR's conclusion held and its sentence did not; the sentence
was amended by record.

Two smaller things a future reader should not rediscover. Component-level
types are not core types: the lifted export is declared `(result s32)`, not
`(result i32)`, and the validator's own error lists the legal set
(`bool`, `s8`, `u8`, …, `s32`, `u32`, …, `char`, `string`) — the core guest
module keeps returning `i32` and the canonical ABI does the mapping. And a
first attempt at this branch failed at *resolution*, not compilation, with
`failed to select a version for the requirement winch-codegen = "=36.0.17"`:
`wasmtime`'s `component-model` feature references `wasmtime-winch?/component-model`,
which puts the optional `wasmtime-internal-winch` into the resolve graph, and
the local sparse index was stale. It resolved cleanly once the index had
refreshed, and `winch-codegen 36.0.17` is present on crates.io — so the
symptom looks exactly like a broken wasmtime patch release and is not one.

## 6. Backlog

Recorded, not scheduled. Each carries the finding that produced it.

| Item | Finding | Notes |
|---|---|---|
| `RuntimeSnapshot` with a per-request generation | 4 | Root-tree ADR 0015 exists but is untracked and owned by the 0.2a port. Needs an `ArcSwap`-vs-`RwLock` benchmark before design; do not start while the port is live. |
| RLS `OR … IS NULL` fail-closed | 9 | Requires a **new** migration — editing an applied, checksummed file is illegal — so it is suspended with the two-tree rule. Also untestable while PostgreSQL is closed. |
| Wire `AUTH_STALE_REVISION` into a real surface, or document that the deny decision is the enforcement | 10 | The error code exists and is reachable only from a test helper. |
| Change-impact engine (`change:plan` / `change:validate` / `change:apply`) | — | The proposal's headline differentiator. Derives most of its value from generations (finding 4) landing first. |
| Distributed rollout with readiness and canary | 19 | Root-tree ADR 0017 defers it to Phase 3. |
| Artifact signing, publisher identity, TUF | 18 | **The verifiable-artifact half is closed by Wave M4** — `kelson build` records each binary's SHA-256 and `kelson release --verify` re-hashes, refusing a mismatch, a missing binary, or an artifact whose digest was never taken. That is *integrity*, not *authenticity*: a manifest is plain JSON, so anyone who can rewrite it can forge matching bytes. Signing, publisher identity and TUF remain deliberately open. |
| Cedar as the policy language | 10 | Homegrown `kelson-policy` ships and is tested. Evaluate only with a concrete migration path. |
| File-watcher dev reload | 7 | Polling is documented and adequate; SIGHUP is a named plug point. |
| `handler_version` on job rows | 21 | Payload `schema_version` already fail-closes on a newer schema. |
| Expand-and-contract migration tooling | 22 | **Closed by Wave M3** — `kelson migrate:plan` checks the rule that was prose, over the pending set only (applied migrations are history, and a verdict you cannot act on is noise rather than a gate). Two deliberate limits stand beside the closure: `EAC-003` is advisory and PostgreSQL-only, because every shipped migration builds its indexes without `CONCURRENTLY` (measured 9 of 9 per driver) so a fatal verdict would condemn applied history, and MySQL has no `CONCURRENTLY` to be missing; and the rule's third clause — no table-rewrite locks during business hours — remains **unchecked**, because a static scan of SQL cannot see a deployment window. |
| Route-set hot reload (not just chains) | 3 | Needs the graph to become swappable rather than boot-frozen. |
| Decide whether `MODULE_ABI`'s default should hard-fail instead of echo | 23 | `default_api()` returns `MODULE_ABI`, so an omitted `api` survives any bump. Wave A sidestepped it by versioning the guest ABI by artifact shape (ADR 0019 §3), but the same trap springs the moment the *manifest* contract is bumped. Making omission an error is expand-and-contract-safe only if every shipped manifest pins `api` first -- three do, two do not (finding 23 lists them). |
| Root-tree ADR 0016 §3 still lists `[resources]` as Phase 2.2 backlog | 13 | **Superseded by ADR 0019 §7**: Wave M shipped the enforcement as `[budget]`, and `resources` already means Laravel-style route resources in the same file (`fn expand_resources`, `keel-platform` `crates/application/registry/src/lib.rs:513-519`). The file is untracked and owned by the 0.2a port, so it is cited, never edited -- reconcile at convergence (ADR 0014), not before. |
| Prove a component fixture can be built without guest-side `wit-bindgen` | 11 | **Closed by the Wave A feature-flag landing (no `wit-bindgen` dev-dependency).** The old premise -- "`wat-1.261.0`'s own sources contain no `component` references" -- was a literal-string grep that missed where the support actually lives: `wat` 1.261 declares `component-model = ["wast/component-model"]` and turns it on by default, `wast` 261 defaults it on too (`default = ["wasm-module", "component-model"]`), and `wast-261.0.0/src/wat.rs:44-45` routes a top-level `(component ...)` to the 998-line encoder at `wast-261.0.0/src/component/binary.rs:13`. Proven by executing it, not by reading it: `keel-platform` `runtime/kelson-demo/src/wasm_tests.rs:1123` hand-writes the component text, loads it through `Component::new` (`wasmtime-36.0.17/src/runtime/component/component.rs:156-164`, which routes through `wat::parse_bytes`), instantiates it and calls the lifted export. So `DEPENDENCY-POLICY.md:53` never engages -- there is no new *direct* external, and `keel-platform` `tools/kelson-analyzer/src/lib.rs:432-447` still counts 25/25. **One correction the experiment forced:** enabling the feature *does* put `wasmtime-internal-wit-bindgen` and `wit-parser` into `keel-platform/Cargo.lock`, transitively through `wasmtime-internal-component-macro` (wasmtime's own `bindgen!` macro), which is why ADR 0019 §4's "no `wit-bindgen` enters the workspace" was amended by record instead of left standing. |
| Module-level `DEGRADED` / `DISABLED` (no `ModuleState` enum) | 16 | `docs/15-EXTENSION.md:25-36` lists seven states and only `DISCOVERED` / `VALIDATED` / `REGISTERED` have a representation. **Landing the generation lifecycle did not close this:** `keel-platform` `crates/extension/wasm/src/lib.rs:171`'s `PluginState` retires one generation of one plugin and is not that state machine, so `PluginState::Drained` must not be read as `DISABLED` (`docs/15-EXTENSION.md:41-47`). Nothing yet marks a loaded module degraded or disables one in place. Recorded here because both `docs/15-EXTENSION.md:38-39` and claim 16 point at this row — a backlog reference with no row behind it is the same defect as a claim with no evidence. |
| Generation swap path: the production caller for `drain`, and the HTTP mapping for a call refused mid-drain | 16 | `keel-platform` `crates/extension/wasm/src/lib.rs:835` ships and is proven by `keel-platform` `runtime/kelson-demo/src/wasm_tests.rs:1031`, but has **no production caller** — there is no swap or unload path, so nothing retires a generation in a running process. That same absence is why no `KSN_*` code and no HTTP pin were added: the draining case is unreachable in production, so `WasmError::Lifecycle` falls through `to_app_error`'s wildcard as an internal error, which is the right verdict for the one case that *is* reachable (dispatching a generation the host never committed). Landing a swap must do both halves — give `drain` a caller **and** decide whether a mid-drain refusal renders 503 — because adding the code now would make the append-only list describe behaviour nothing emits. |

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
