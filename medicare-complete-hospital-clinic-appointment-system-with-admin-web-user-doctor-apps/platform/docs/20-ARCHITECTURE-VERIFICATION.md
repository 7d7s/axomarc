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
| 1 | A capability registry maps stable string handler IDs to implementations | EXISTS | `CommandBus { handlers: HashMap<OperationId, Arc<dyn OperationHandler>> }` — `crates/execution/command/src/lib.rs:17-19`; duplicate registration refused `:30-38`; unknown ID → not-found `:50-62`. Stable IDs compile in: `OP_BOOK = "appointments.book"` — `applications/appointments/src/lib.rs:27-30`. Populated at boot from compiled-in handlers — `platform/` `runtime/kelson-demo/src/lib.rs:1376-1399`, `runtime/kelson-demo/src/host.rs:307-354`. Not runtime-populated. |
| 2 | Routes are declared as data and validated against the registry | EXISTS (boot-time) | `[[routes]] method/path/operation/capability` — `applications/appointments/module.toml:23-28`; parsed from disk without recompiling — `runtime/kelson-demo/src/graph_routes.rs:88-108`; unknown operation → `GRAPH-ROUTEFILE-OP` — `platform/` `crates/application/registry/src/lib.rs:945-965`; becomes axum routes at boot — `runtime/kelson-demo/src/graph_routes.rs:344-358`. |
| 3 | A route can change without rebuilding the application | PARTIAL | What hot-swaps is the *resolved chain*, not the route set: `BundleCell = Arc<RwLock<Arc<Router>>>` — `runtime/kelson-demo/src/hot_reload.rs:45-48`; reload refuses a stale artifact and keeps the current bundle — `:136-152`. The `PlatformGraph` itself is a boot-time `Arc` — `platform/` `runtime/kelson-demo/src/lib.rs:873`. Adding or removing a route still needs a restart. |

### Snapshots and generations

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 4 | An immutable `RuntimeSnapshot` with a per-request generation | ABSENT | Zero `.rs` matches for `RuntimeSnapshot`. The root-tree ADR 0015 (`platform/specs/ADR/0015-runtime-snapshot.md`, untracked — owned by the 0.2a port) states the gap at `:22-24`: "there is no named generation carried through the request path". `arc_swap` is not a dependency (`:90-95`). |
| 5 | Per-subsystem revisions exist instead | EXISTS | Config: `ConfigSnapshot { version, config, hash }` with a monotonic revision — `crates/application/configuration/src/runtime.rs:43-52`, swapped through `RwLock<Arc<…>>` `:136`, commit `:221-252`, rollback `:269-286`. Authorization: `AuthorizationRevision` — `crates/foundation/core/src/revision.rs:44-47`, stale-deny `crates/security/policies/src/lib.rs:139-149`. Feature: `FeatureSnapshot` carried on the context — `crates/foundation/context/src/lib.rs:95-99`. Routes: pinned to `manifests_hash` — `crates/application/registry/src/route_plan.rs:4-9`. No single generation unifies them. |

### Feature flags and configuration

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 6 | Feature flags with a revision, swappable at runtime | EXISTS | Deterministic pure `evaluate` — `crates/enterprise/feature-flags/src/lib.rs:180-212`; boot-time catalog, unknown flags cannot be evaluated — `:218-247`; revision bumped per mutation — `:312` (memory) `:416` (file); ambient set hot-swapped by `FlagsReloader` — `runtime/kelson-demo/src/hot_reload.rs:177-235`. Boot-structural flags stay boot-frozen on purpose (`:18-21`). Not one unified snapshot. |
| 7 | A file watcher drives dev reload | PARTIAL | mtime **polling**, not a watcher: "no watcher dependency ships in this crate" — `crates/application/configuration/src/runtime.rs:10-14`; 500 ms poll loop — `platform/` `runtime/kelson-demo/src/lib.rs:977-987`. SIGHUP and etcd/consul are named as plug points only. |

### Security pipeline

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 8 | Mandatory security stage order that extensions cannot reorder | TRUE in the engine, FALSE at the HTTP chain | Engine order is fixed: `plan.validate()` → plane invariant → deadline → authorization boundary → concurrency lease → idempotency → backend `run` — `crates/execution/engine/src/engine.rs:156-226`. The HTTP per-route chain is data-driven and removable: `public = true` strips `auth`+`verified` — `crates/application/registry/src/middleware_catalog.rs:256-258`; `excluded_middleware` can empty a chain entirely — test `:485-493`. The backstop that does hold: dispatch refuses anonymous access on a non-public route regardless of chain — `runtime/kelson-demo/src/graph_routes.rs:1683-1710`. |
| 9 | PostgreSQL row-level security as defense in depth | EXISTS with a fail-open hole | Policies read `tenant_id = kelson_current_tenant() OR kelson_current_tenant() IS NULL` — `database/migrations/20260819000001_foundation.sql:185-200`. With no GUC set every row is visible. The runtime always sets the GUC per transaction — `crates/data/postgres/src/tx.rs:119-126` — and verifies it on session commit — `crates/data/postgres/src/lib.rs:218-264`. MySQL has no equivalent and says so: "No RLS … explicit tenant_id binds are the whole isolation story" — `crates/data/mysql/src/tx.rs:6-13`. |
| 10 | Policy epochs for emergency revocation | EXISTS (deny-on-stale) | `policy.stale-revision` deny — `crates/security/policies/src/lib.rs:139-149`. `codes::STALE_REVISION = "AUTH_STALE_REVISION"` — `crates/foundation/core/src/error.rs:41` — but it is surfaced only through a test helper, `:262-271`. The live enforcement is the deny decision, not the error code. |

### WASM extension boundary

| # | Claim | Verdict | Evidence |
|---|---|---|---|
| 11 | WebAssembly Component Model with WIT interfaces | ABSENT by explicit decision | `wasmtime = { version = "36", default-features = false, features = ["cranelift", "runtime", "wat", "std"] }` — `platform/Cargo.toml:219` and its twin `platform/keel-platform/Cargo.toml:202`; the governance comment at `platform/Cargo.toml:201-218` records the security bump and states "no component-model". Zero `.wit` files, no `wasmtime-wasi`, no `wasmtime::component`. Guest ABI v1 is hand-rolled and documented at `crates/extension/wasm/src/lib.rs:9-26`; every export is validated at load (`:552-558`) and a missing or mistyped one refuses the boot. Still true as written: `wasmtime-component-macro` and `wasmtime-component-util` are absent from `keel-platform/Cargo.lock`, which is the machine-checkable form of "off". The deferral is no longer open-ended: root-tree ADR 0016 §2's reconsideration trigger is now recorded as **fired by decision**, and the sanctioned path -- v1 and v2 coexisting through a transition window, `component-model` as an opt-in cargo feature that is off by default, zero new direct externals -- is `keel-platform` `specs/ADR/0019-wasm-abi-v2-component-model.md`. Nothing about this row changes until that feature lands. |
| 12 | Native `.so`/`.dll` plugin loading must be avoided | EXISTS and is enforced | No `libloading`, no `dlopen`, no native image handling anywhere in the dependency graph. The refusal is test-pinned: `wasm_only_boundary_refuses_native_images` — `keel-platform` `runtime/kelson-demo/src/wasm_tests.rs:236-280` — feeds ELF, PE and Mach-O images and asserts each is refused at compile, never treated as a library. |
| 13 | A module manifest declaring identity, version, compatibility, capabilities | EXISTS; the resources half closed by Wave M2 | `ModuleManifest` — `platform/` `crates/application/registry/src/lib.rs:604-650`; `MODULE_ABI: u32 = 1` `:37`; mismatch → `GRAPH-MANIFEST-ABI` `:817-828`. Capability profiles — `keel-platform` `crates/extension/wasm/src/lib.rs:130`. Per-plugin execution budgets now exist as `[budget]` — `keel-platform` `crates/application/registry/src/lib.rs:659` (field), `:672-692` (section) — deliberately **not** named `[resources]`: that word already means Laravel-style route resources in this same format, and one word meaning two things in one file is how misconfiguration gets written. |
| 14 | Sandboxed execution with resource budgets | EXISTS — the PARTIAL above was closed by Wave M2 | Host ceilings: fuel `DEFAULT_FUEL = 10M` — `keel-platform` `crates/extension/wasm/src/lib.rs:90`, memory `DEFAULT_MEMORY_BYTES = 16 MiB` `:93`, guest-string copy bound 1 MiB — `keel-platform` `crates/extension/wasm/src/imports.rs:40`, enforced `:60` and `:258`. Per-plugin `[budget]` — `keel-platform` `crates/application/registry/src/lib.rs:672-692`, reachable from `ModuleManifest` `:659` — may only **tighten** those ceilings: `validate_against` `:701-736` collects every violation at once and runs at the top of `WasmHost::load` (`keel-platform` `crates/extension/wasm/src/lib.rs:497`, `resolve_budget` `:436-450`) *before the binary is compiled or instantiated*. Effective values travel together as `CallBudget` `:67-77`, so fuel `:487`, memory `:469-470` and the epoch deadline `:489` cannot describe different sandboxes (the same three are set again in `call()` `:724-744`). Concurrency: `InFlightSlot` `keel-platform` `crates/extension/wasm/src/lib.rs:236-242` is taken at `:706-717`, before `pre.instantiate` at `:748`, and the over-cap path returns `WasmError::Saturated` `:176-179`. Wall clock: `EpochTicker` `keel-platform` `crates/extension/wasm/src/lib.rs:298-329` advances `Engine::increment_epoch` at `:317` from a **monotonic clock** (`:313-319`) rather than per sleep call (`:320`), and `NO_DEADLINE_TICKS` `:331-340` gives no-deadline plugins a deadline far enough out that wasmtime's default of `0` — which it documents as having "always elapsed" — cannot trap them. Codes `KSN_WASM_FUEL` `keel-platform` `crates/foundation/core/src/error.rs:76`, `KSN_WASM_SATURATED` `:85`, `KSN_WASM_DEADLINE` `:95`, each pinned to HTTP 503 at both boot sites — `keel-platform` `runtime/kelson-demo/src/lib.rs:438-451` (production) and `:1576-1587` (harness). |
| 15 | Capability-limited host API | EXISTS; tenant scoping PARTIAL | Every call checks the sidecar-declared set — `keel-platform` `crates/extension/wasm/src/imports.rs:344` — under a reviewed sidecar the guest cannot exceed (`keel-platform` `crates/extension/wasm/src/lib.rs:580-593`, the drift refusal; note both documents have `budget` cleared before the comparison, since it is host policy a guest cannot declare). Path checks fail closed without a scope — `keel-platform` `crates/extension/wasm/src/imports.rs:350-358`; write imports gated by profile `:395-398`; undefined imports (e.g. `sock_*`) fail at link time — `crates/extension/wasm/src/imports.rs:8`. But `CallState` carries no tenant ID (`keel-platform` `crates/extension/wasm/src/lib.rs:96-119`): tenant isolation on this path rests on the engine's pre-dispatch authorization, not on the host functions themselves. |
| 16 | A staged module lifecycle (prepare/activate/drain) | ABSENT | No `ModuleState` enum exists. `docs/15-EXTENSION.md:25-26` describes `DISCOVERED -> VALIDATED -> REGISTERED -> INITIALIZED -> READY -> DEGRADED -> DISABLED`; only the first three have a real representation. ADR 0016 `:68-77` defers `prepare → commit → drain` to a later phase. ADR 0019 §6 now sequences `prepare`/`commit`/`drain` **independently of the Component Model** -- it is sanctioned as additive to v1 by root ADR 0016 §3, so it must not be gated behind the `component-model` feature, and the in-place upgrade proof must show the lifecycle working for a v1 module before it is shown working for a v2 component. Still ABSENT in code: no `ModuleState` enum exists. |
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
3. **The route set is boot-frozen** (`runtime/kelson-demo/src/lib.rs:873`) — "change a
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
| A | Feature-flagged `component-model` enablement | TODO |
| A | WIT package layout mirroring the four host imports | TODO |
| A | `prepare`/`commit`/`drain` staged lifecycle (additive to v1) | TODO |
| A | In-place upgrade proof (v1 -> v1, v2 -> v2, v1 -> v2, one host) | TODO |
| A | `DEPENDENCY-POLICY.md` row 46 amendment (authorized by ADR 0019 §8, applied on landing) | TODO |

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
`keel-platform` `crates/extension/wasm/src/lib.rs:398`, not
only for plugins that declare a deadline, because the engine's configuration is
fixed at construction and a plugin may declare its budget later. The
consequence is the part a future reader must not rediscover the hard way: a
store's epoch deadline starts at `0`, which wasmtime documents as having "always
elapsed", so a store that is never given one traps its guest before it executes
a single instruction. Every plugin that declares no `execution_timeout_ms` is
therefore given `NO_DEADLINE_TICKS` at `:331-340` (`u64::MAX / 2`, not `u64::MAX`, since the
deadline is computed as `current_epoch + ticks` and the larger value would
overflow it) and it is applied to every store at `:489` and `:744`. All 14
pre-existing WASM tests still pass, which is the evidence
that this was handled rather than assumed.

**M2 — a wall-clock deadline is a coarse bound, and may run late.** The epoch
advances once per tick, so ticks must correspond to milliseconds. Counting one
tick per `sleep(1ms)` does not do that: `sleep` is a lower bound, and on
Windows the default timer resolution lands a 1ms sleep near 15.6ms, which would
have made a declared 50ms deadline fire around 780ms — late by a factor that
varies per machine. The ticker reads a monotonic clock at `keel-platform`
`crates/extension/wasm/src/lib.rs:313` and catches the epoch up
at `:316-319` by the milliseconds actually elapsed. What remains, stated
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
`crates/extension/wasm/src/lib.rs:706-717` and released by the
`InFlightSlot` guard at `:236-242` on every exit path
(declared or not), so it is a usable gauge for an uncapped plugin as well; only
the *refusal* is conditional on `max_concurrency`. An early `?` return that
skipped the release would leak a slot permanently and take the plugin out of
service one failed request at a time.

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
| Prove a component fixture can be built without guest-side `wit-bindgen` | 11 | Host side is proven (`wasmtime::component::bindgen` at `wasmtime-36.0.17/src/runtime/component/mod.rs:472`, transitively behind the feature, so no new direct external). Guest side is not: `wat-1.261.0`'s own sources contain no `component` references. If `wast`'s component text format cannot yield a loadable fixture, the upgrade proof needs `wit-bindgen` as a dev-dependency, which `DEPENDENCY-POLICY.md:53` counts against the same 25-cap -- a hard `ANALYZER-BUDGET` failure (`tools/kelson-analyzer/src/lib.rs:442-450`), not a rounding error. Blocks any feature-flag merge. |

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
