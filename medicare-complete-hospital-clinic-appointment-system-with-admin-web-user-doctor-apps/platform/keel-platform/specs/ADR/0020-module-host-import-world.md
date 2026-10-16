# ADR 0020 — Application modules as WASM guests: the host-import world

Status: **Proposed** (2026-10-16) · Gate: Stage C.2 (layer isolation, G3) ·
Implementation: none — this ADR is the contract the port is built against.

## Context

`harness-tester/ARCHITECTURE-CONTRACT.md:83` asks four things of layer
isolation, and clause 2 is the one the tree fails. The layer gate maps
`kelson-demo` to `kernel` and `kelson-mod-appointments` / `kelson-mod-oauth2`
to `application-module`, and declares `kernel → application-module` forbidden
("Kernel must not statically link business implementations"). Both edges are
real: `runtime/kelson-demo/Cargo.toml` carries the two crates as optional
dependencies and the `demo` feature enables them.

The measured verdict is in `docs/STAGE-C-MODULE-DEPENDENCY-INVENTORY.md` and
the short version is that the gate reports exactly two violations with `demo`
on and **zero** with it off. So the finish line is mechanical — delete two
Cargo lines — but the twenty-one references behind them are not.

### Why ports-and-adapters cannot answer this

The obvious cheap fix is a wiring crate: move the two provider registrations
and the bundle pull out of `kelson-demo` into a separate crate that depends on
both, and let the kernel depend only on that. It does not work, and the layer
map says why in two lines rather than one:

- `forbidden_edges` contains `kernel → application-module`, and
- it *also* contains `application-module → kernel`.

Any crate that names both the kernel and a module is therefore either the
kernel (first violation) or depends on the kernel (second violation), and a
crate that names neither has nothing to wire. There is no arrangement of
in-process crates that removes the edge. **The kernel must obtain a module's
behaviour without a dependency edge to any crate that has one**, and the only
mechanism in the tree that does that is the one the Stage C marker already
uses: a `wasm32-wasip1` artifact loaded through `packages.toml`.

### Feasibility, measured

Porting has an obvious risk that the contract crates do not build for the
guest target. They do. Both modules compile for `wasm32-wasip1` today:

```text
cargo build --release --target wasm32-wasip1 -p kelson-mod-oauth2         # ok, 1m15s
cargo build --release --target wasm32-wasip1 -p kelson-mod-appointments   # ok
```

and the closure that pulled in is the whole contract layer — `kelson-models`,
`kelson-query`, `kelson-tokens`, `kelson-execution`, `kelson-lifecycle`,
`kelson-context` — plus `aes-gcm` and `sha2`. Every type a module passes
across the seam (`ModelDef`, `Record`, `FieldFilter`, `ListOpts`,
`Page<Record>`, `NotificationRow`) is already compiled for the guest target
and already serde-serializable.

The builds produce `.rlib` and not `.wasm` because the crates declare the
default `lib` crate-type; the marker declares `cdylib`. That difference is the
first thing the port changes, and it is the reason the two modules link into
the kernel at all.

## Decision

### 1. The guest implements the contract traits; the module's logic does not change

This is the load-bearing decision, and everything else follows from it.

The instinctive port is to rewrite each module's data access as host calls:
replace `self.runtime.insert(&*self.backend, …)` with an import, and so on for
every site. That is a large, error-prone rewrite of 2,381 lines of business
logic, and it is unnecessary — because `kelson-models` compiles for the guest
target.

`OauthClientStore` holds `backend: Arc<dyn ModelStore>` and does all of its
work through `ModelRuntime` (`applications/oauth2/src/lib.rs:151-153`). The
trait it names is a contract-layer trait. So the guest does not need to
*change* what it calls; it needs to change what *implements* it:

> A guest-side `HostModelStore` implements the existing
> `kelson_models::ModelStore` trait, and each of its methods marshals its
> arguments to a host import and unmarshals the answer. The module is handed
> one of those instead of an `InMemoryModelStore`, and **not one line of the
> module's business logic changes.**

The same shape covers `kelson_tokens::TokenService`, which is a concrete
struct over a `TokenStore` trait — the guest constructs the real service with
a guest-side `HostTokenStore` behind it, so token minting keeps its real
implementation and only persistence crosses the boundary.

The port is therefore mostly mechanical: entry points and a backend
implementation, not a rewrite.

### 2. The import surface is JSON over strings

Every value crossing the seam is already serde-serializable, so the ABI v1
imports exchange `JSON`-shaped strings and never a structured value. The
surface is eight additions to the `host` namespace:

```wit
// model access — the ModelStore trait, one import per method
model-list:   func(model: string, tenant: string, filters: string, opts: string) -> string;
model-find:   func(model: string, id: string, tenant: string, include-deleted: bool) -> string;
model-insert: func(model: string, record: string) -> string;
model-update: func(model: string, id: string, tenant: string, changes: string) -> string;
model-delete: func(model: string, id: string, tenant: string) -> bool;

// tokens — issue and verify
token-issue:  func(purpose: string, subject: string) -> string;
token-verify: func(purpose: string, label: string) -> string;
```

An empty string is the refusal for every returning import, matching the
existing four's "answer, never trap" posture
(`crates/extension/wasm/src/imports.rs:87-89`). A guest therefore cannot tell
a denied request from a missing record, which is deliberate: the distinction
is an oracle, and a module that needs to branch should be told *by its own
policy*, not by the shape of a failure.

`tenant` is spelled as a `string` and carries `""` for none, rather than an
`option<string>`, because ABI v1 has no options and the WIT spelling has to
match what the v1 closure can actually pass. ADR 0019 §5 records the same
compromise for `%record`.

### 3. Host-to-guest returns need a second arena

ABI v1 gives the host no way to hand a string back: the four existing imports
return `bool` or nothing, and the guest's response buffer
(`testing/module-marker/src/lib.rs:71-72`) exists for exactly one call and is
read immediately after `kelson_call` returns.

An import that returns a string must allocate *during* the call. `kelson_alloc`
cannot serve it — its contract is one buffer per dispatch
(`testing/module-marker/src/lib.rs:53-56`), and breaking that invariant would
make the arena's no-aliasing argument false. So the guest gains a second
export:

```rust
#[no_mangle] pub extern "C" fn kelson_response_alloc(len: i32) -> i32;
```

The host calls it through the caller, writes the payload, and returns the
packed `(ptr << 32) | len` the existing convention already uses. The arena
reuses from the start the same way `kelson_alloc` does, so an import loop
cannot grow guest memory without bound.

### 4. `CallState` carries the services, and every import stays gated

The four existing primitives are methods on `CallState`
(`crates/extension/wasm/src/imports.rs:90`), which today holds only `label`,
the sidecar-verified `declared` capability set, and the resolved scope chain.
The new imports need real services, so `WasmPlugin` gains an optional
`ModuleHostServices` — `Arc<dyn ModelStore>`, the token store, the
notification hub — populated at boot and handed down at call time.

**Every model import is gated on the declared capability set, and writes go
through the same buffered discipline `capability_write` uses.** This is not a
preference. `capability_write` refuses unless the executing capability runs a
ReadWrite or Admin profile, buffers the delta into `CallState`, and lets the
handler commit after a successful call and discard on failure
(`crates/extension/wasm/src/imports.rs:17-19`). A `model-insert` that wrote
straight through would be a second, unmediated write path into the host's
store — the module would be able to mutate any model it could name, regardless
of profile, with no buffer to discard when its own call later failed. That is
a regression the tree's fail-closed posture has held out against since Wave 9,
and the port does not get to buy G3 with it.

The capability mapping is one rule, stated so it can be checked: **`model-list`
and `model-find` answer to the same gate as `capability_check`; `model-insert`,
`model-update` and `model-delete` answer to the same gate as
`capability_write`, and buffer.**

### 5. `kelson-module-sdk` is the guest's shared half

`harness-tester/config/layer-map.example.json` already names a `kelson-module-sdk`
package and an `sdk` layer, which is the shape this needs. The crate is
`wasm32-wasip1`-only and holds:

- `HostModelStore`, `HostTokenStore` — the trait implementations from §1;
- the `kelson_manifest` / `kelson_alloc` / `kelson_response_alloc` /
  `kelson_call` scaffolding, so a module supplies only a dispatch table over
  the `OperationHandler`s it already has;
- the marshalling in one place, so two modules cannot disagree about how a
  `Uuid` or an absent `tenant` is spelled.

Without it, both modules duplicate the arena and the import bindings, and the
two copies drift — which is precisely the failure ADR 0019 §5 was written to
make impossible on the ABI side.

### 6. What the module crates give up

`Provider` / `ServiceRegistry` wiring is host-side
(`applications/oauth2/src/lib.rs:968-1003`) and does not survive the port: a
guest cannot register itself into a host registry, because that is a host
operation. What replaces it is what the marker already does — declare the
capability set, models and routes in the sidecar, and let `prepare` verify
them against the manifest. `kelson-demo`'s three shipped references
(`src/lib.rs:565`, `:569`, `src/host.rs:331`) are all provider registration,
and all three are replaced by discovery rather than reimplemented.

Two costs are accepted and named rather than absorbed:

- **The `demo` feature stops meaning "the two modules, statically linked".**
  After the port it means nothing at all, and the feature goes with the
  dependencies it defined. The harness's eight `demo`-gated sites inside
  `build_app_state_inner` fall back to the `not(demo)` shapes already written
  at `src/lib.rs:1434` and `:1512`.
- **The `ServiceRegistry` lookup at `src/host.rs:331` loses its typed bundle.**
  A guest answers JSON, so the appointments and oauth2 handler bundles arrive
  as the same kind of `Vec<Arc<dyn OperationHandler>>` every other plugin
  answers with, and the `OauthHandlerBundle` newtype has no remaining job.

### 7. Notifications are deferred, and the reason is the calling convention

`NotificationStore::insert` returns a `BoxFuture`
(`crates/services/notifications/src/lib.rs:376-377`). ABI v1 host imports are
synchronous: a guest importing a function cannot await the host's future, and
`kelson_call` returns `i64`, not a pending handle. Making notification emit
work across the seam therefore requires either a host import that blocks the
wasm instance for the duration of a future (acceptable today, because every
in-tree store is in-memory and resolves immediately — but it is a landmine
the moment a store is Postgres), or an ABI change.

Both `kelson-mod-appointments` call sites take the notifier as
`Option<Arc<NotifyHub>>` and no-op when it is `None`
(`applications/appointments/src/lib.rs:414-425`), so a guest that passes
`None` compiles, runs, and loses only notification emission. That is the
chosen posture: **notifications are out of the first port, `None` by
construction, recorded here and in the inventory** rather than shipped as a
blocking import whose danger is invisible until a real store lands.

## Consequences

- **G3 clause 2 resolves.** With the two crates gone from
  `runtime/kelson-demo/Cargo.toml` the gate reports `violations: []` under
  `--all-features`, which is the same result it already reports on the default
  feature set. Clauses 3 and 4 follow for clause 3 automatically: the kernel
  artifact stops moving when a module is rebuilt, because no module code is in
  it.
- **The layer map needs two entries, not one.** `kelson-module-marker` (already
  recorded as an open blocker in the inventory) and `kelson-module-sdk`. Both
  are harness-owned and both are one line; the map's own description says a
  PASS is not a gate until layer choices are reviewed, so this is a review
  that has to happen rather than a bookkeeping step.
- **Two more artifacts to build, review and pin.** Each module gains a
  `cdylib` crate-type, a compiled `.wasm`, a reviewed `module.toml` sidecar and
  a `packages.toml` `[[wasm]]` entry, exactly as the marker has. The build
  command moves from `cargo build -p <module>` to
  `cargo build --release --target wasm32-wasip1 -p <module>` and the artifact
  path is a review surface, not a build output.
- **`kelson-wasm` gains six contract dependencies** and stays `kernel`-layer:
  `kelson-models`, `kelson-query`, `kelson-tokens`, `kelson-execution`,
  `kelson-lifecycle`, `kelson-context`. All six are mapped `contract`, and
  `kernel → contract` carries no forbidden edge. This is the one place the port
  makes the kernel bigger, and it is buying exactly the thing the kernel needs
  to stop being handed modules at compile time.
- **The port's real size is now visible.** Roughly: an ADR (this), one new SDK
  crate, eight imports with a host shim each, `CallState` service handles, two
  `cdylib` entry points, two sidecars, two artifacts, and the removal of
  twenty-one references. The business logic in the two modules is untouched,
  which is the whole reason for §1.

Alternatives considered: a ports-inversion wiring crate (refused — the layer
map forbids both directions of the edge, above); keeping the modules in-process
and reclassifying them out of `application-module` (refused — the map's
description says review layer choices, and relabelling a crate to make a gate
green is the gate-gating posture this tree has refused before); a blocking
`notify` import (refused for now — §7, and a synchronous import over a store
that may be Postgres is a correctness trap, not a shortcut); and porting only
one module first (deferred, not refused — it halves the work but leaves the
gate failing on the second edge, so a partial port buys nothing the inventory
does not already say honestly).
