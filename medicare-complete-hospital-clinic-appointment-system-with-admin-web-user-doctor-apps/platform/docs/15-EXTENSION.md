# 15 — Extension

How the platform is extended without being forked.

## Ports and adapters

Anything pluggable is a trait in a platform crate; implementations live in `adapters/` or
`crates/data` and are wired by runtime binaries at boot. The kernel never imports an adapter.
Current ports include: execution backend, idempotency, concurrency/locks, authorization, storage,
mail/SMS/push, cache, search engine, outbox store, job store, AI client, trace store, clock.

## Providers and lifecycle

Boot is a provider graph: `register -> topological sort (cycle = boot refusal) -> boot in order ->
READY`. Service lifetimes are explicit — PROCESS (pools, caches), APPLICATION (config, registry),
EXECUTION (ExecutionContext), TRANSACTION (tx handle). Constructor injection everywhere; no
reflection, no service location by string.

## Modules

Compile-time composition today (frozen decision D-69): a module is a crate plus a `module.toml`
manifest. The graph compiler (G05/G06) validates capabilities, routes, events, dependencies before
anything runs. Modules never reach into each other — contracts only.

The module lifecycle states: `DISCOVERED -> VALIDATED -> REGISTERED -> INITIALIZED -> READY ->
DEGRADED -> DISABLED`.

## Module ABI

Every manifest declares which version of the module contract it targets:

```toml
[module]
name = "appointments"
api = 1            # optional; defaults to the current MODULE_ABI
```

- `keel_registry::MODULE_ABI` (currently `1`) is the version of the module contract the framework
  speaks. Missing `api` defaults to it, so existing manifests keep compiling.
- `PlatformGraph::compile` rejects any module whose `api != MODULE_ABI` with the
  `GRAPH-MANIFEST-ABI` violation. A framework upgrade therefore never silently reinterprets an
  older manifest — the graph fails closed instead.
- What the ABI covers: the manifest schema (`module.toml` — capabilities, routes, events,
  dependencies, plane/maturity metadata) and the `ModuleProvider` surface registered against
  registry carriers. Additive manifest fields and new carriers are non-breaking; changing or
  removing existing semantics is an ABI bump with a migration window.
- Out-of-workspace runtime loading remains post-v1 (frozen D-69): third-party modules compile
  into the workspace today. The ABI gate is the seam that keeps them stable once a loader exists.

## Package registry (B-24, Wave 8)

Local-path package discovery ships in v1:

- The root `packages.toml` index lists `[[package]] { name, version, path }` entries (relative
  paths resolve against the workspace root; absolute paths work too). v1 is local-path only —
  git fetch stays out under the zero-dependency rule.
- `keel pkg:add <path>` validates the package's `module.toml` (the package v1 contract:
  a manifest + an optional workspace crate member) and appends it; `keel pkg:list` prints the
  index with each manifest's ABI status and fails when any entry is unusable.
- Graph compile (`keel check` / `keel graph` / boot) merges every indexed package's manifest
  into the same fail-closed pass as `applications/` and `modules/`: a missing or malformed
  manifest refuses discovery, `GRAPH-MANIFEST-ABI` rejects an incompatible `api`, and the route
  cache pin covers `packages.toml` + indexed manifests (drift = boot refusal, never a silent
  stale route set).
- Manifest-only packages may declare routes for operations their linked workspace crate
  registers, and may ship a `[config]` table whose defaults merge UNDER the app config file at
  boot (file wins per key; first indexed package wins among packages; schema-checked on their
  own so a package never smuggles an unknown key past `GRAPH-CONFIG-SCHEMA`).

## Package publishing (B-25, Wave 8)

`keel publish <package> [--force]` materializes an indexed package's resources into the host
app (Laravel's `vendor:publish`, scoped to one package). `keel config:publish <package>
[--force]` publishes only the config layer. Source layout → target:

| Package source | Published target | Semantics |
| --- | --- | --- |
| `<pkg>/database/migrations/{v}_{desc}.sql` | `database/migrations/{v}_{name}_{desc}.sql` | sqlx keys applied migrations by the leading integer, so the version stays first and the package name namespaces the rest (lowercase `[a-z0-9_]` description, the rule `keel make:migration` enforces). A version already owned by a differently named file is refused (a duplicate would break `keel migrate`), as are two sources sharing one version. |
| manifest `[config]` table | `config/packages/<name>.toml` | The editable overlay boot reads: package manifest defaults ← published copy ← config file (each layer schema-checked on its own; the published file's errors name the file). Deterministic bytes (header + body) keep re-publish idempotent. |
| `<pkg>/public/**` | `public/vendor/<name>/**` | Plain file copy, recursive. |
| `<pkg>/views/**` | *no copy* | Registered under the `<name>::` view namespace at boot (Wave 2 resolver): `billing::welcome` renders `<pkg>/views/welcome.html`. Without an index the namespaces fail closed. |

Publish contract:

- **Idempotent:** every target is byte-compared. Identical content is skipped (`unchanged`),
  missing targets are written, differing content is a **conflict**: skipped, reported, and the
  command exits non-zero — review the drift, then re-run with `--force` to overwrite.
  `config:publish` cycles through the same semantics for its one target, and refuses packages
  that ship no `[config]` table.
- **Fail-closed inputs:** the package must be indexed (`keel pkg:list`) and its index entry
  must match its manifest name; `config:publish` additionally requires `[config]`.
- Rebuild after publishing migrations: they are embedded at compile time (`keel migrate` runs
  the embedded set).

## WASM plugins (B-30, Wave 9)

The D-62 amendment (wasmtime, recorded in the dependency ledger + `deny.toml`) ships the third
roadmap step as an opt-in runtime — `crates/extension/wasm` (`keel-wasm`, workspace-internal,
`publish = false`):

- **Discovery (sidecar manifests).** The root `packages.toml` gains `[[wasm]] { name, version,
  path }` entries (`serde` default; the key is omitted when empty, so every existing index stays
  byte-identical). Each binary ships a reviewed `<binary>.module.toml` sidecar parsed with the
  same `ModuleManifest` + GRAPH gates as a crate module. `manifests()`, `contains_name()` and
  the route-cache `manifests_hash` all cover WASM entries — the hash mixes in the sidecar AND
  the binary bytes, so editing a plugin refuses boot until `keel route:cache` is cleared.
- **Guest ABI v1.** A module exports `memory`, `keel_manifest() -> i64` (packed
  `ptr << 32 | len` of a NUL-terminated `module.toml`-shaped JSON), `keel_alloc(len) -> i32`
  and `keel_call(op_ptr, op_len, in_ptr, in_len) -> i64`. Load validates every export,
  compiles, instantiates, reads the guest manifest and refuses boot on **drift**: the guest's
  document must serialize IDENTICALLY to the reviewed sidecar — a plugin cannot smuggle a
  capability past review.
- **Sandbox.** Fuel metering (`DEFAULT_FUEL` = 10M instructions) + a 16 MiB store memory cap;
  exhaustion maps to the append-only `KER_WASM_FUEL` code pinned to HTTP 503 at both boot
  sites. The import surface is a hand-rolled WASI-preview1 subset (`fd_write`, `proc_exit`,
  `clock_time_get`, `random_get`, `sched_yield`, `fd_close`; empty args/environ) plus
  `host.log` and `host.capability_check` — there is no `sock_*`, so any module that imports
  networking fails at link time (fail-closed).
- **No self-elevation.** `host.capability_check` answers only from the sidecar-declared set,
  and a path-bearing request additionally needs an attached scope whose node path matches.
  Operation registration goes through `WasmHandler: OperationHandler`, so authorization,
  idempotency, delta and audit run host-side in the existing engine — readonly profiles only,
  enforced both at registry discovery and in `WasmHandler::new`.
- **Lifecycle.** `load_wasm_plugins` runs BEFORE `host.boot()` (errors are prefixed
  `wasm plugin <name>:`); `module.wasm` is registered unconditionally with
  `DemoModuleProvider` depending on it, so init order is deterministic whether or not any
  plugin is installed.

## Plugin roadmap (D-69)

Deliberate order, smallest trusted core first:

1. **Compile-time modules** — today. Zero runtime code loading; the compiler is the sandbox.
2. **Dynamic adapters** — later, at the port seams (e.g. a third-party search backend), loaded by
   the runtime binary, still behind frozen traits.
3. **WASM sandbox** — shipped as B-30 (Wave 9, above): opt-in `[[wasm]]` entries over the
   capability ABI. Nothing above changed when it landed, because the extension seams already
   existed.

## API governance

Every public item is Public-API (versioned, API-ID tracked) or Internal (`#[doc(hidden)]`/
crate-private). The registry lives in `crates/foundation/contracts/api-registry.toml`;
`cargo-semver-checks` gates releases (G14); `keel upgrade` reports deprecations with
replacements. Breaking changes only in a new major, with a one-minor deprecation window.
