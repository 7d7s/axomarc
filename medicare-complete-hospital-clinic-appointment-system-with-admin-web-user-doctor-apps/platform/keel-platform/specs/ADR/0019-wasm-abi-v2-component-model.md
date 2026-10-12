# ADR 0019 — Wasm ABI v2: Component Model behind an opt-in feature, ABI v1 retained

Status: **Accepted** (2026-10-12) · Wave A of the distribution plan ·
Amends: root-tree `platform/specs/ADR/0016-wasm-module-abi.md` §2 (reconsideration
trigger) and §3 (stale `[resources]`) — that file is untracked and owned by the
0.2a port, so it is **cited, never edited** ·
Implementation: none in this record. The feature flag, the WIT world, the staged
lifecycle and the in-place upgrade proof land under this ADR.

Numbering note: this ADR lives in `keel-platform/specs/ADR/` and is numbered
**0019**, not 0014. The root tree's record already occupies 0014-0018 and ADR
0014 makes root `platform/` canonical at 0.2c, so 0014-0018 are reserved there
and 0019 continues the union sequence without colliding at convergence.

## Context

ADR 0016 (root tree, Status: **Accepted**, 2026-10-11) fixed release 1 on the
hand-rolled core-module ABI and deferred WIT / the Component Model. §2 of that
record does not reject the technology; it sets a reconsideration trigger *and*
a migration shape:

> **Reconsideration trigger:** a first non-Rust guest target, a second host
> embedding, or a genuine need for cross-component type composition. At that
> point migration ships as an explicit ABI v2 with both ABIs supported through
> a transition window — never as a silent swap.

Two things since then need recording rather than re-litigating: the
distribution plan adopted "Design ABI v2 on the WebAssembly Component Model"
as Wave A (`docs/20-ARCHITECTURE-VERIFICATION.md:134`), and Wave M shipped the
per-module enforcement that ADR 0016 §3 listed as Phase 2.2 backlog — under a
different section name (§7 below).

The ABI v1 surface that ABI v2 must mirror is fixed and test-pinned:

**Guest exports** (`platform/keel-platform/crates/extension/wasm/src/lib.rs:11-27`,
every one validated at load `:551-558` — a missing or mistyped export refuses
the boot):

| Export | Contract |
|---|---|
| `memory` | the linear memory the host reads/writes through |
| `kelson_manifest() -> i64` | packed `(ptr << 32) \| len` of JSON shaped like `module.toml`; must serialize *identically* to the sidecar or `WasmError::Drift` refuses boot (`:168`) |
| `kelson_alloc(len: i32) -> i32` | host-driven scratch allocation; `0` on OOM, treated as a call error, never a panic |
| `kelson_call(op_ptr, op_len, in_ptr, in_len) -> i64` | one operation, UTF-8 JSON in and out, packed the same way |

**Host imports** (`platform/keel-platform/crates/extension/wasm/src/imports.rs:297-463`):

| Import | Returns | Refusal rules (all fail closed, never trap) |
|---|---|---|
| `host.log(ptr, len)` | — | structured line, tagged with the plugin label (`:299`) |
| `host.capability_check(a_ptr, a_len, p_ptr, p_len)` | `i32` | action must be in the sidecar-verified declared set (`:344`); a non-empty path additionally requires a resolved scope chain (`:350`), no scope ⇒ deny (`:350-352`) |
| `host.capability_write(a_ptr, a_len, d_ptr, d_len)` | `i32` | profile must `can_write()` (`:395`), action declared (`:398`), delta parses as a JSON *object* (`:401-406`), per-call buffer below `MAX_CALL_EFFECTS` (`:408`) |
| `host.capability_audit(a_ptr, a_len, r_ptr, r_len)` | `i32` | every profile may record, action declared (`:445`), record a JSON object (`:448-453`), same buffer bound (`:455`) |

Budgets and bounds: `DEFAULT_FUEL = 10_000_000`
(`platform/keel-platform/crates/extension/wasm/src/lib.rs:90`),
`DEFAULT_MEMORY_BYTES = 16 MiB` (`:93`), `MAX_GUEST_STR = 1 MiB`
(`platform/keel-platform/crates/extension/wasm/src/imports.rs:40`, enforced at
`:60` and `:258`), `MAX_CALL_EFFECTS = 64` (`:45`). A hand-rolled minimal
`wasi_snapshot_preview1` shim supplies
`fd_write` / `proc_exit` / `clock_time_get` / `random_get` / `sched_yield` /
`fd_close` with deliberately empty args and environ; anything undefined fails at
link time, i.e. boot refusal (ADR 0016 :20-25).

## Decision

### 1. The reconsideration trigger is fired by decision

ADR 0016's trigger names three conditions; the first — **a first non-Rust guest
target** — is now the express goal of the distribution plan, whose whole point
is a general publishable framework whose plugins are authored outside this
workspace. That is a genuine need for a language-neutral, versioned contract,
which is the substance of the trigger's third condition.

The trigger is therefore recorded as **fired by decision** in this ADR, not
silently assumed. Nothing about ADR 0016 §1 changes: ABI v1 stays the contract
for every module that ships today.

### 2. Migration shape: v1 and v2 coexist, never a silent swap

The migration ships exactly as ADR 0016 §2 prescribes — **both ABIs supported
through a transition window**, no silent replacement:

- **Detection is structural, not a version number.** A core module exporting
  `kelson_manifest` / `kelson_alloc` / `kelson_call` is v1. A Component Model
  component exporting the `kelson:plugin/dispatch` world is v2. The artifact
  shape decides; nothing is inferred from a manifest field that could drift.
- **The `[[wasm]]` sidecar carries an optional explicit `abi` key** so review
  stays fail-closed: absent means v1, so every shipped fixture and
  `your-app/modules/patients` keeps working untouched; present must equal the
  artifact's actual shape or load refuses (the refusal is a `WasmError`-family
  load error — its exact code is chosen at implementation time, deliberately,
  since `ErrorRenderRegistry::set_status` is a global per-code override and a
  new code pins an HTTP status at boot).
- A v1 module is never recompiled, re-instantiated or reinterpreted as v2, and
  a v2 component is never accepted where only v1 was reviewed.
- The transition window has no fixed end date in this record. Closing it is a
  separate decision that requires every in-tree consumer to have moved.

### 3. The guest ABI is *not* `MODULE_ABI` — a bump would both refuse boot and silently re-label

`kelson_registry::MODULE_ABI` is `1`
(`platform/keel-platform/crates/application/registry/src/lib.rs:38`) and gates
the **manifest +
`ModuleProvider`** contract (`docs/15-EXTENSION.md:57-60`), not the wasm guest
surface. `PlatformGraph::compile` pushes a `GRAPH-MANIFEST-ABI` violation
whenever `m.module.api != MODULE_ABI`
(`platform/keel-platform/crates/application/registry/src/lib.rs:928-937`),
which refuses boot.

Two properties make it the wrong lever for an ABI v2:

1. **It hard-fails anything that pins `api`.** Three manifests in this tree pin
   it: `applications/appointments/module.toml:4`,
   `applications/oauth2/module.toml:4`,
   `templates/starter-template/module.toml:7`. Bumping to 2 refuses boot for
   all three.
2. **It silently re-labels anything that omits it.** The field is
   `#[serde(default = "default_api")]`
   (`platform/keel-platform/crates/application/registry/src/lib.rs:779-780`)
   and `default_api()`
   returns `MODULE_ABI` itself (`:553-555`), so an omitted `api` parses as
   whatever the current value is. `examples/reference/module.toml` and
   `templates/project-template/module.toml` omit it.

A bump would therefore fail closed on exactly the manifests that stated their
version, while silently re-interpreting the ones that did not — the inverse of
`docs/15-EXTENSION.md:54-56`, which says the gate exists so a framework upgrade
"never silently reinterprets an older manifest". **That guarantee holds only
for manifests that pin `api`.** Recorded as a backlog finding
(`docs/20-ARCHITECTURE-VERIFICATION.md` §6); the gate itself is unchanged here.

**Decision:** ABI v2 is versioned by artifact shape (§2), and `MODULE_ABI`
stays `1`. Its bump remains reserved for a break in the manifest /
`ModuleProvider` contract, where it behaves as documented for manifests that
pin it.

### 4. `component-model` is an opt-in cargo feature, off by default

`kelson-wasm` gains:

```toml
[features]
default = []
component-model = ["wasmtime/component-model"]
```

with the workspace dependency untouched (`platform/keel-platform/Cargo.toml:202`
keeps `default-features = false, features = ["cranelift", "runtime", "wat",
"std"]`). Nothing changes for a default build.

This costs **zero** on the dependency cap. `kelson-analyzer` counts only
`[workspace.dependencies]` entries that have no `path`
(`platform/keel-platform/tools/kelson-analyzer/src/lib.rs:432-441`) and fails
above 25 (`:442-450`); a feature on an existing dependency adds no entry, so
the count stays 25/25. Host-side WIT binding comes from `wasmtime::component::bindgen`,
already exported by wasmtime 36 (`wasmtime-36.0.17/src/runtime/component/mod.rs:472`)
and reachable only with the feature, since the module is declared under
`#[cfg(feature = "component-model")]` (`wasmtime-36.0.17/src/runtime.rs:61-62`)
> **Amended by record when the feature landed.** The text this record was
> written with read "— so **no `wit-bindgen` enters the workspace**. `wasmtime-component-macro`
> and `wasmtime-component-util` are transitive consequences of the feature and
> are absent from `keel-platform/Cargo.lock` today, which is the
> machine-checkable proof that the feature is off at the time of writing."
> That was true of the feature-off tree it described, and neither half survives
> enabling the flag. What enabling it brings in: `wasmtime-internal-component-macro`
> and `wasmtime-internal-component-util` (direct, wasmtime's own), and through
> the former **`wasmtime-internal-wit-bindgen`** and **`wit-parser`**
> (transitive — also wasmtime's own `bindgen!` machinery, *not* the `wit-bindgen`
> crate anyone would add as a dev-dependency). So no *direct* external is added,
> the `[workspace.dependencies]` count stays 25/25
> (`platform/keel-platform/tools/kelson-analyzer/src/lib.rs:432-447`), and
> `platform/DEPENDENCY-POLICY.md:53` still does not engage — the conclusion
> this paragraph was reaching for holds. What does not hold is its
> *machine-checkable form*: `Cargo.lock` cannot be the "off" signal any more,
> because cargo records the packages behind a feature that no default build
> activates. The check that replaced it is the counter — feature off, the suite
> reads 1392 passed / 0 failed and compiles no `wasmtime::component`; feature
> on, 1393.

The feature is deliberately *not* enabled by `--all-features` gates in this
record: it turns the sandbox posture into a public commitment, so enabling it is
its own reviewable step.

### 5. The WIT world mirrors the four host imports

```wit
package kelson:plugin@0.2.0;

interface host {
  /// Structured line, tagged with the plugin label by the host.
  log: func(message: string);

  /// `action` must be in the sidecar-verified capability set; a non-empty
  /// `path` must sit inside the execution's resolved scope chain. Returns
  /// false when no scope resolved — fail-closed, never a trap.
  capability-check: func(action: string, path: string) -> bool;

  /// Records a mutation delta for the handler to commit through the
  /// execution unit. Returns false — never a trap — when the profile is
  /// read-only, the action is undeclared, the delta is not a JSON object,
  /// or the per-call buffer is full.
  capability-write: func(action: string, delta: string) -> bool;

  /// Append-only audit record. Every profile may record; same refusals.
  capability-audit: func(action: string, record: string) -> bool;
}

enum call-error {
  /// The guest could not complete the operation.
  failed,
  /// The instruction budget ran out (KSN_WASM_FUEL, HTTP 503).
  out-of-fuel,
}

interface dispatch {
  /// The guest manifest, JSON-shaped like `module.toml`.
  manifest: func() -> string;

  /// Dispatch one operation; UTF-8 JSON in, UTF-8 JSON out.
  call: func(op: string, input: string) -> result<string, call-error>;
}

world plugin {
  import host;
  export dispatch;
}
```

Design points, each mirroring v1 rather than reinventing it:

- Four imports in, four functions out — the names, the refusal-as-return-value
  discipline and the fail-closed scope rule are carried across unchanged. A
  refusal stays a `false` return, never a trap, so a hostile guest cannot turn
  a denial into a panic path.
- `string` and `result<_, call-error>` replace `kelson_alloc` and the
  `(ptr << 32) | len` packing outright; `manifest()` and `call()` return
  values instead of packed pointers.
- The canonical ABI still stages strings through guest memory, so a size bound
  remains necessary — `MAX_GUEST_STR` does **not** become dead with v2. The
  exact bound is re-derived at implementation time rather than assumed equal.
- `capability_write` and `capability_audit` keep the `MAX_CALL_EFFECTS = 64`
  per-call buffer bound; that is a host-side budget, not an ABI shape, so it
  carries over verbatim.
- The world is exported, so the drift gate survives: `manifest()` output is
  still normalized-compared against the reviewed sidecar, and a mismatch is
  still a boot refusal.

### 6. `prepare → commit → drain` is additive to v1, not a v2 feature

ADR 0016 §3 already sanctions the staged lifecycle as an *extension of v1*, and
`docs/15-EXTENSION.md:38-40` ties `DEGRADED` / `DISABLED` to it. It is
therefore sequenced **independently of the Component Model** and must not be
gated behind the `component-model` feature:

- **prepare** — instantiate and validate (drift check, export or world check)
  with **no** dispatch registration; a failure here leaves the previous
  generation serving.
- **commit** — the **host** registers the dispatch. The guest never registers
  itself, which is the same asymmetry as the sidecar-verified capability set.
- **drain** — retire in-flight work against the old generation before it stops
  accepting calls.

Because it lands independently, an in-place upgrade proof must show the
lifecycle working for **v1 modules first**, then for a v2 component, then for a
v1 → v2 swap under one `WasmHost` with no window in which neither generation
serves.

### 7. `[budget]` supersedes ADR 0016 §3's `[resources]`

ADR 0016 §3 lists as Phase 2.2 backlog "a manifest `[resources]` section
(per-module memory/timeout enforced against `StoreLimits`/fuel instead of only
host constants)". Wave M shipped exactly that enforcement as the **`[budget]`**
section (commit `b036894`): `ModuleBudget` at
`platform/keel-platform/crates/application/registry/src/lib.rs:670-692` with
`max_memory_mb` (`:676`), `max_fuel` (`:680`), `max_concurrency` (`:685`) and
`execution_timeout_ms` (`:691`), field declared at `:658-659`, tighten-only and
validated at load by `ModuleBudget::validate_against` (`:701`).

**`[budget]` is the section name of record; `[resources]` was never landed.**
ADR 0016 is untracked and owned by the 0.2a port, so its §3 claim is superseded
here by citation rather than edited — it is now stale.

The retired name is also the better outcome: `resources` already means routes
in this codebase (`fn expand_resources`,
`platform/keel-platform/crates/application/registry/src/lib.rs:513-519`,
expanding `resource`/`singleton` route entries into `RouteSpec`s), so
`[resources]` as a
memory/timeout section would have collided with an existing term.

### 8. DEPENDENCY-POLICY amendment — authorized here, applied on landing

`platform/DEPENDENCY-POLICY.md:46` records wasmtime as "default features OFF
(cranelift/runtime/wat/std only: **no component-model** …)" and is frozen by
D-62 (STAGE-0-SPEC §15), while `:52` states that changes adding features
require an ADR. **This ADR is that authorizing record.**

The row is *not* edited in this commit: the feature does not exist yet, and
editing it now would make the policy claim something untrue. On landing, row 46
changes from "no component-model" to "`component-model` opt-in via the
`kelson-wasm` `component-model` cargo feature, **off by default** (ADR 0019)",
with the cap unchanged at 25/25. Nothing else in the policy moves.

## Consequences

- The distribution plan's Wave A has a fixed target that does not contradict
  ADR 0016: v2 is built *on that record's own terms*, and v1 keeps serving.
- Two version numbers now exist and are deliberately separate: `MODULE_ABI` for
  the manifest/`ModuleProvider` contract, artifact shape for the guest ABI.
  Conflating them would produce the §3 double-failure.
- The default build, the dependency cap, and the `publish = false` posture on
  `kelson-wasm` are all unchanged by this record.
- `docs/20-ARCHITECTURE-VERIFICATION.md:78` stays **true as written** —
  Component Model remains absent until the feature lands; this ADR changes the
  recorded deferral into a recorded plan, not the code.

## Deliberate deferrals and unproven claims

- **Resolved — hand-authoring a component fixture without guest-side
  `wit-bindgen`.** (Recorded as *Unproven* here when this ADR was written; the
  resolution below is an amendment by record, added when the feature flag
  landed.) The host side was already proven (`bindgen!` at
  `wasmtime-36.0.17/src/runtime/component/mod.rs:472`). The fixture side turned
  out to be the easy half, and the sentence that said otherwise was wrong: "the
  guest/fixture side is not [proven]: `wat-1.261.0`'s own sources contain no
  `component` references" was a literal-string grep, not a reading of the crate.
  `wat` 1.261 declares `component-model = ["wast/component-model"]` and turns it
  on by default, `wast` 261 defaults it on too (`default = ["wasm-module",
  "component-model"]`), and `wast-261.0.0/src/wat.rs:44-45` routes a top-level
  `(component ...)` to the 998-line encoder at
  `wast-261.0.0/src/component/binary.rs:13`. The authoring half therefore needed
  no dependency change whatsoever — the support was already compiled into every
  build we ship. Proven by running it, not by re-reading it:
  `platform/keel-platform/runtime/kelson-demo/src/wasm_tests.rs:1123`
  hand-writes the component text, loads it through `Component::new` (which
  accepts WAT per `wasmtime-36.0.17/src/runtime/component/component.rs:156-164`),
  instantiates it and calls the lifted export. No `.wit` file, no guest code
  generator, and `platform/DEPENDENCY-POLICY.md:53` never engages, because there
  is no new *direct* external — the count stays 25/25. **One correction the same
  experiment forced:** enabling the feature *does* put
  `wasmtime-internal-wit-bindgen` and `wit-parser` into
  `keel-platform/Cargo.lock`, transitively through
  `wasmtime-internal-component-macro`, which is wasmtime's own `bindgen!` macro.
  That is a transitive consequence, not a workspace dependency, so the cap is
  unaffected — but §4's "no `wit-bindgen` enters the workspace" no longer
  describes it, and was amended below rather than left standing.
- **Resolved — that enabling the feature compiles clean.** (Also an amendment
  by record; the claim was written as "unproven".) It does. Feature **on**:
  `clippy --workspace --all-targets -- -D warnings` exits 0 with zero warnings,
  and `cargo test --workspace --no-fail-fast` reads 1393 passed, the single
  failure being the known pre-existing `hot_reload` wall-clock flake
  (`platform/keel-platform/runtime/kelson-demo/src/hot_reload.rs:493`, 57ms
  against a 50ms budget, passing on re-run at 0.16ms) — unrelated to this
  feature and not counted against it. Feature **off**: the mandated gate is
  unchanged at clippy exit 0 and **1392 passed / 0 failed**, which is the
  "no regressions, not a frozen number" reading this bullet asked for:
  1392 → 1393, the one difference being the new fixture case.
- Guest write/delta profiles beyond what `host.capability_write` already
  records; per-module publisher identity and signed artifacts (TUF-class
  supply chain); `module build/test/publish` as first-class CLI verbs;
  separate-process isolation for hostile workloads. All per ADR 0016's
  deliberate-deferrals list and unchanged by this record.
- The Component Model's async / resource types (`component-model-async`) are
  out of scope: this ADR uses only the synchronous `component-model` feature.
