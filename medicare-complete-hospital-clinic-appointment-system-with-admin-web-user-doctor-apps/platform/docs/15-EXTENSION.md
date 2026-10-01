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

## Plugin roadmap (D-69)

Deliberate order, smallest trusted core first:

1. **Compile-time modules** — today. Zero runtime code loading; the compiler is the sandbox.
2. **Dynamic adapters** — later, at the port seams (e.g. a third-party search backend), loaded by
   the runtime binary, still behind frozen traits.
3. **WASM sandbox** — only if the ecosystem proves demand. Nothing above changes when it lands,
   because the extension seams already exist.

## API governance

Every public item is Public-API (versioned, API-ID tracked) or Internal (`#[doc(hidden)]`/
crate-private). The registry lives in `crates/foundation/contracts/api-registry.toml`;
`cargo-semver-checks` gates releases (G14); `keel upgrade` reports deprecations with
replacements. Breaking changes only in a new major, with a one-minor deprecation window.
