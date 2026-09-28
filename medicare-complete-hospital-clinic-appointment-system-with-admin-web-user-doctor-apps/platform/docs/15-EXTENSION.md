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
- Discovery and a package registry remain post-v1 (frozen D-69): third-party modules compile into
  the workspace today. The ABI gate is the seam that keeps them stable once a loader exists.

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
