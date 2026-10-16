# Stage C: module dependency inventory (C.1)

**What this document is.** The inventory de-staticization works from. It records
where the kernel names a concrete application module, which of those names are
shipped code and which are test scaffolding, what each module consumes from the
host, what the plugin surface already provides, and the layer gate's verdict as
actually run. Every claim below points at the line or the command that makes it
true; nothing here is a plan.

**Scope.** `platform/keel-platform/**`. The gate that judges this
(`harness-tester/scripts/check-layer-boundaries.py`) and its layer map
(`harness-tester/config/layer-map.local.json`) are harness-owned and outside
this tree's ability to change — §7 records that as a live blocker rather than
working around it.

---

## 1. What G3 asks, and where each clause stands

G3 is `harness-tester/ARCHITECTURE-CONTRACT.md:83-88`, four clauses. Status is
the measured result in §2 and §7, not an intention.

| # | Clause | Status | Detail |
|---|--------|--------|--------|
| 1 | Dependency graph conforms to the layer map after mapping real crate names | **75 of 76 packages mapped** | §7 — one unmapped package, added by Stage C itself |
| 2 | Kernel target has no dependency on concrete application implementation crates | **holds on default features; fails with `demo`** | §2 — exactly two forbidden edges |
| 3 | Installing/upgrading/removing a module does not alter kernel source or artifact hash | **holds for the marker module; fails for the two `demo` modules** | §4, §6 |
| 4 | Two independent modules can be versioned and released separately | **not demonstrated** | §7 — both inherit `version.workspace = true` |

---

## 2. The gate, run

The harness checks Cargo dependency edges from `cargo metadata`, matching
package names to layers and refusing the pairs in `forbidden_edges`. Two runs
are needed to see the whole picture, because `cargo metadata` resolves the
*default* feature set unless told otherwise — and the `demo` feature is what
pulls the module crates in.

```text
cargo metadata --format-version 1                       > md-default.json
cargo metadata --format-version 1 --all-features        > md-all.json

python3 scripts/check-layer-boundaries.py \
  --metadata md-default.json \
  --map config/layer-map.local.json \
  --fail-on-unmapped
```

**Default features — no forbidden edge, but the run does not pass:**

```json
{
  "status": "FAIL",
  "packages_total": 76,
  "mapped_packages": 75,
  "unknown_packages": ["kelson-module-marker"],
  "edges_inspected": 308,
  "resolve_nodes_present": true,
  "violations": [],
  "note": "This checks Cargo dependency edges only. It does not prove runtime dynamic loading or independent deployment."
}
```

Exit code **2** (`fail-on-unmapped`): zero violations, one unmapped package.
The kernel is layer-clean when `demo` is off.

**`--all-features` — the substantive G3 failure:**

```json
{
  "status": "FAIL",
  "edges_inspected": 310,
  "violations": [
    {"from_package": "kelson-demo", "from_layer": "kernel",
     "to_package": "kelson-mod-appointments", "to_layer": "application-module",
     "reason": "Kernel must not statically link business implementations."},
    {"from_package": "kelson-demo", "from_layer": "kernel",
     "to_package": "kelson-mod-oauth2", "to_layer": "application-module",
     "reason": "Kernel must not statically link business implementations."}
  ]
}
```

Exit code **1**. Two violations, both the same shape: `kernel →
application-module`.

**The whole of clause 2's failure is those two Cargo edges.** They exist
because `runtime/kelson-demo/Cargo.toml` declares both crates as optional
dependencies and the `demo` feature enables them:

```toml
[features]
demo = ["kelson-mod-appointments", "kelson-mod-oauth2"]
...
kelson-mod-appointments = { workspace = true, optional = true }
kelson-mod-oauth2      = { workspace = true, optional = true }
```

The default build already proves the shape: with `demo` off the optional edges
are absent and the gate reports `violations: []`. C.2 is therefore not
"remove a violation and hope" — it is "delete those two lines and make every
site behind them compile and behave without them".

---

## 3. Every place the kernel names a module

Recursive scan of `*.rs` and `*.toml` under `platform/keel-platform` for
`kelson_mod_`: **21 references, all in `runtime/kelson-demo`, every one of them
gated by `#[cfg(feature = "demo")]` or a file-level
`#![cfg(feature = "demo")]`.**

### 3.1 Shipped runtime code — 3 references

| Location | What it does |
|---|---|
| `src/lib.rs:565` | in `pub async fn run()` — `host.add_provider(Box::new(kelson_mod_appointments::ModuleProvider::new()))` |
| `src/lib.rs:569` | in `pub async fn run()` — `host.add_provider(Box::new(kelson_mod_oauth2::ModuleProvider::new()))` |
| `src/host.rs:331` | bus assembly — pulls `kelson_mod_oauth2::OauthHandlerBundle` out of the `ServiceRegistry` (the appointments bundle occupies the plain `Vec<Arc<dyn OperationHandler>>` slot above it) |

These three are the production coupling. They are why the Cargo edge exists,
and they are what a discovery-driven load has to replace.

### 3.2 Dev harness and test scaffolding — 18 references

| Location | Count | Enclosing context |
|---|---|---|
| `src/lib.rs:1432, 1468, 1499, 1503, 1506, 1507, 1570, 1574, 1612, 1622` | 10 | `build_app_state_inner` — documented as *"Harness application state for the integration suite (S7 test plan) and dev harnesses. Mirrors production wiring minus the lifecycle host"* (`src/lib.rs:1436`) |
| `src/lib.rs:2157` | 1 | inside `#[cfg(test)] mod wave6_mail_tests` (`src/lib.rs:1865`) |
| `src/shell.rs:427` | 1 | `#[cfg(feature = "demo")] fn harness()` |
| `tests/http_integration.rs:18` | 1 | file-level `#![cfg(feature = "demo")]` |
| `tests/routing_edge_integration.rs:16, 165` | 2 | file-level gate; `#[cfg(feature = "demo")]` |
| `tests/validation_edge_integration.rs:77, 204, 209` | 3 | `#[cfg(feature = "demo")]` |

### 3.3 The fallback C.2 generalizes

The harness already ships a `not(demo)` shape for two of its sites, which is
the pattern the remaining sixteen have to follow:

```rust
#[cfg(feature = "demo")]
type HarnessAppStore = Arc<kelson_mod_appointments::AppointmentStore>;   // src/lib.rs:1432
#[cfg(not(feature = "demo"))]
type HarnessAppStore = Arc<kelson_models::InMemoryModelStore>;           // src/lib.rs:1434

#[cfg(feature = "demo")]
let bundle: Vec<Arc<dyn OperationHandler>] = vec![ /* four handlers */ ]; // src/lib.rs:1497
#[cfg(not(feature = "demo"))]
let bundle: Vec<Arc<dyn OperationHandler>> = Vec::new();                  // src/lib.rs:1512
```

The comment at `src/lib.rs:1437` states the intent — the harness *"mirrors
production wiring minus the lifecycle host"* — so the harness is the easier
half: it needs the module reachable without naming it, not the module
reachable across a WASM boundary.

Two more comments mark where production already has a non-static path the
harness bypasses:

- `src/lib.rs:1606` — the route binding resolver is registered by hand in the
  harness, *"production boot wires it through the module provider + the
  route.bindings service instead"*.
- `src/lib.rs:1615` — likewise for the model probe, *"production wires it
  through the module provider + the well-known service instead"*.

So production's coupling is the provider registration and the bundle pull, and
both already sit behind services (`ServiceRegistry`, route bindings,
well-known) that a discovery-driven load could populate the same way the
marker's handlers are populated today.

---

## 4. What each module consumes from the host

Type names referenced by each module crate's `src/lib.rs` — the services a
de-staticized module must still be able to reach:

| Host service | `kelson-mod-appointments` | `kelson-mod-oauth2` |
|---|:---:|:---:|
| `ModelStore` | yes | yes |
| `ExecutionContext` | yes | yes |
| `OperationHandler` | yes | yes |
| `ServiceRegistry` | yes | yes |
| `PolicyEngine` / `AccessSnapshot` | yes | — |
| `NotificationStore` | yes | — |
| `TokenService` | — | yes |

Sources: `applications/appointments/src/lib.rs` (1,355 lines, one file) and
`applications/oauth2/src/lib.rs` (1,026 lines, one file).

`ExecutionContext` and `OperationHandler` are the *guest* side of the existing
dispatch contract — a WASM module already receives the former and exports the
latter in effect. The other five are host services a guest cannot reach at all
today.

---

## 5. What the plugin surface provides today

`crates/extension/wasm/wit/kelson-plugin.wit` (ABI v2) and
`crates/extension/wasm/src/imports.rs` (ABI v1, the default build). The
`kelson:plugin@0.2.0` world imports exactly one interface, `host`, with four
functions:

```wit
log:              func(message: string);
capability-check: func(action: string, path: string) -> bool;
capability-write: func(action: string, delta: string) -> bool;
capability-audit: func(action: string, %record: string) -> bool;
```

plus WASI `preview1` (`args`, `clock`, `environ`, `fd`, `random`, `sched_yield`,
`proc_exit`) — the hand-rolled shim at `crates/extension/wasm/src/imports.rs`.

That surface is what the Stage C marker module runs on, and it is deliberately
narrow: log, check, write, audit. It carries **no model access, no notification
emission, no token issuance**.

---

## 6. The C.2 gap, stated precisely

Reading §4 against §5, the host imports a de-staticized `kelson-mod-appointments`
or `kelson-mod-oauth2` would need beyond what exists:

1. **`ModelStore`** — record read/write. Required by *both* modules; without it
   neither can persist or load a single row.
2. **`NotificationStore`** — emit a notification (`appointments`, the book/cancel
   handlers attach a notifier).
3. **`TokenService`** — issue and verify tokens (`oauth2`).
4. **`PolicyEngine` / `AccessSnapshot`** — scope and capability resolution
   (`appointments`). `capability-check` covers *an action against a path*;
   whether it covers what the module actually reads off `AccessSnapshot` is a
   contract question, not a code one.
5. **Registration into kernel wiring** — how a guest reaches
   `RouteBindingRegistry` and `ProbeRegistry`. Production wires both through
   the module provider + services (`src/lib.rs:1606`, `:1615`), so this is a
   question of what a module *declares* rather than what it *calls*; the
   marker already declares its operations through `kelson_manifest()`.

Items 1-3 are new WIT additions with a host implementation and a guest binding
each. That is the ADR-sized part. Item 4 is a contract decision. Item 5 is
likely already shaped by the manifest, but is unverified here — it is listed as
open rather than as done.

---

## 7. What already satisfies G3, and what blocks it

**The marker module path already meets clauses 2 and 3.**
`testing/module-marker` is a separate crate compiled to
`wasm32-wasip1`; its bytes are reached through `packages.toml`'s `[[wasm]]`
entry (`packages.toml:26`) and loaded by `load_wasm_plugins`
(`src/lib.rs:38-95`). No source file in `runtime/kelson-demo` names it — the
only references are `wasm_tests.rs:1860-1872` locating the *artifact* by path,
which is data, not a dependency edge. Changing its build changes its `.wasm`
and leaves the kernel artifact untouched; that is clause 3 for one module,
demonstrated. C.2 is bringing the other two to the same posture.

**Blocker 1 — one unmapped package, and the map is not in this tree.**

`kelson-module-marker` (added at `Cargo.toml:80`) is absent from
`harness-tester/config/layer-map.local.json`, so `--fail-on-unmapped` exits 2
even with zero violations. The neighbouring crate `testing/testkit` is mapped
as `tooling`, so the file's convention already covers `testing/*`; it simply
predates this crate. The fix is one line — `"kelson-module-marker": "tooling"`
(or `application-module`, since nothing depends on it and so it introduces no
edge either way) — in a file this tree is explicitly forbidden to touch.

Two routes out, neither taken unilaterally:

- add the mapping in `harness-tester/config/layer-map.local.json`, which
  requires touching a harness-owned file; or
- drop `"testing/module-marker"` from `workspace.members`, which removes the
  package from the gate's view. This is *not* taken here: it would make the
  gate pass by making the package invisible, which is the same shape of move as
  relabelling a layer to dodge a check. It also diverges from the tree's own
  convention, since `testing/testkit` is a member.

Until one route is chosen, clause 1 reads **incomplete, not passing**, and that
is a deliberate record rather than an oversight.

**Blocker 2 — clause 4 is untested by any gate.**

Both module crates set `version.workspace = true`, so they carry the workspace
version (`0.1.0`) and move together. They are separate packages with no edge
between either of them and the other (confirmed by the gate: no
`application-module → application-module` violation), so they *can* be released
separately — but "versioned separately" is not something the tree currently
does, and no gate checks it. Recorded as not demonstrated rather than as
satisfied.

---

## 8. Sequencing note

The gate's `violations` array is empty on default features. That means C.2 has
a mechanically checkable finish line which does not depend on any judgement
call: delete the two optional dependencies from `runtime/kelson-demo/Cargo.toml`
and the `demo` feature's definition of them, then make all 21 sites compile
with no `kelson_mod_` name anywhere in `runtime/kelson-demo`. The harness sites
(§3.2) have a demonstrated fallback already (§3.3). The three shipped sites
(§3.1) are the part that needs the surface in §5 to grow into §6 — which is why
that half is an ADR and this half is an inventory.
