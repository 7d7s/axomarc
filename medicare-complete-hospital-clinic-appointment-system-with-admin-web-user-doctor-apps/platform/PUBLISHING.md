# PUBLISHING.md - Kelson platform release engineering

This document is the single source of truth for publishing Kelson crates to
crates.io. Tooling: `xtask publish` / `xtask bump` (see `platform/xtask`).
Fast path: `just publish-dry` then `just publish`.

## Crate registry status

| Item | Status |
|---|---|
| Registry | [crates.io](https://crates.io) (default Cargo registry) |
| License | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) |
| Versioning | Lockstep: one workspace version for all crates (`[workspace.package].version`) |
| Versioning history | All internal dependency entries in root `Cargo.toml` carry the same literal version |
| Tags | Namespaced git tags: `kelson/platform-vX.Y.Z` |

## Wave status

### Wave 1 (live) - `kelson-analyzer`

`kelson-analyzer` is the only crate published so far.

### Wave 2 rename - EXECUTED (2026-09-28)

Four package names were squatted on crates.io (verified live, no published
versions, treated as unrecoverable). All four have been renamed in this
workspace; their `publish = false` gate and Wave 2 comment are removed, and
the workspace builds green (`cargo check` / `cargo test --locked` /
`xtask dep-rules`) on the new names.

| Old package | New package | Notes |
|---|---|---|
| `kelson-core` | `kelson-foundation` | `crates/foundation/core` dir unchanged; imports `kelson_core` -> `kelson_foundation` |
| `kelson-sessions` | `kelson-session` | `crates/security/sessions` dir unchanged; imports `kelson_sessions` -> `kelson_session` |
| `kelson-events` | `kelson-event` | `crates/async/events` dir unchanged; imports `kelson_events` -> `kelson_event` |
| `kelson-cli` | `kelson-console` | dir moved `tools/kelson-cli` -> `tools/kelson-console`; self-reinvoke in `src/main.rs` is `-p kelson-console`; `[[bin]] name = "kelson"` unchanged |
| (new) `kelson-framework` | `kelson-framework` | umbrella crate in `crates/framework` re-exporting the published framework surface - **BUILT (Phase O)**: `crates/framework/{Cargo.toml,README.md,src/lib.rs}` landed with 26 `{ workspace = true }` deps, added to `members` + `[workspace.dependencies]`, and consumed by `your-app` |

Every framework crate transitively depends on `kelson-foundation`, so nothing
else could upload until this rename landed. Replacement names were verified
available on crates.io before the sweep.

### Wave 2 (remaining release steps) - the remaining 60 crates

1. ~~Perform the renames (package names, dependency keys, imports, doc
   references in `platform/docs/*` and `platform/API-STABILITY.md`)~~ - DONE.
2. ~~Remove the `publish = false` gate (and the Wave 2 comment) from the four
   renamed crates' manifests~~ - DONE.
3. ~~Add `kelson-framework` to `members` and `[workspace.dependencies]`~~ - DONE (Phase O).
4. `just deny && just publish-dry` must both pass.
5. Publish the wave: `just publish` (local) or dispatch `keel-publish.yml`
   with `dry_run=false`. The topologically ordered run publishes all 62
   crates in one pass (`kelson-analyzer` is already live; `already exists` is
   treated as success) with registry-propagation retries.

Publish-gate triage (2026-09-29, Wave 2 prep): the Wave 2 guard (`publishable
crate depends on gated crate`) failed on 10 edges. Fixed per crate intent: the
seven Sentinel/integrity crates whose READMEs say *internal-only until a
release wave* had their gate lifted (`kelson-metrics`, `kelson-telemetry`,
`kelson-tracing`, `kelson-pipeline`, `kelson-buffer`, `kelson-export`,
`kelson-integrity`), and `kelson-demo` gained `publish = false` — it is the demo
API host that hard-wires `kelson-mod-appointments` (application code, staying
gated) and is not a library artifact. Result: 69 members, 62 publishable, 7
gated (`kelson-audit`, `kelson-demo`, `kelson-mod-appointments`, `kelson-arch-validation`,
`kelson-example-minimal`, `kelson-example-reference`, `xtask`). Wave 9 B-31 moved
the counts 68/61/7 -> 69/62/7 by adding the publishable `kelson-mysql` fabric
crate (ADR 0010 records the crate decision).

Distribution program, template embedding (2026-10-11): `kelson-templates` is a
new publishable member. It embeds the `kelson new` template files into the
console, because cargo packages a crate's own directory and nothing above it:
an `include_str!("../../templates/...")` living in `tools/kelson-console`
compiles inside a checkout and then fails to compile for anyone who downloads
the crate. `kelson-console` depends on it by version, so `kelson-templates` has
to be published before `kelson-console` (it has no dependencies of its own, so
it sits at the front of the order in any case).

Counts derived from the manifests on 2026-10-11 (`members = [...]` entries, with
`publish = false` counted as gated): keel-platform 75 members / 66 publishable /
9 gated; platform 74 / 65 / 9. These supersede the 69/62/7 figure above, which
records the Wave 2 triage and is left as written. The one-member difference
between the trees is `crates/framework`, the umbrella crate, which exists only
in keel-platform; that divergence is pre-existing and unrelated to this change.
Neither figure was reconciled here - the counts are reported as derived, not
asserted to be correct.

Dry-run bootstrap semantics: `cargo publish --dry-run` resolves every
dependency against the crates.io index, so before the first real publish the
58 crates that depend on unpublished workspace crates cannot be packaged yet
(first-publish bootstrap gap). `xtask publish --dry-run` reports those as
`deferred` (bootstrap gap) and fails on anything else; the real `just publish`
runs in dependency order, resolving each crate before its dependents are
attempted.

### Wave M (in progress) - module-contract gaps

M1, M2 and M3 are landed; M4 has not started. Most of the wave's work is
confined to `kelson-wasm` and `kelson-demo`, both `publish = false`, so none of
the sandbox work reaches crates.io until that gate is deliberately removed.

| Crate | Publishable | Wave M change | Semver |
|---|---|---|---|
| `kelson-registry` | yes | `ModuleManifest` gains `pub budget: ModuleBudget`; new public `ModuleBudget` with `validate_against` / `effective_memory_bytes` / `effective_fuel`; the module-pin rule (`module_pins`, `verify_pins_against_index`, `verify_pinned_boot`) moved here from the console | **MINOR** - additive, and `#[serde(default)]` keeps every existing manifest parsing byte-identically |
| `kelson-foundation` | yes | two append-only codes: `WASM_SATURATED` (`KSN_WASM_SATURATED`), `WASM_DEADLINE` (`KSN_WASM_DEADLINE`) | **MINOR** - additive constants |
| `kelson-console` | yes | `kelson check` / `kelson graph` / `post_deploy_probe` now read module pins through the registry rule instead of their own copy (M1); new read-only `kelson migrate:plan` subcommand that previews the next `migrate` and checks the pending set against the expand-and-contract rule (M3) | **MINOR** - the new subcommand is additive; M1's behaviour change stands: a *missing* pin file went from "unreadable" error to "no pins declared" |
| `kelson-deployment` | yes | (pending M4 - artifact digest) | - |
| `kelson-wasm` | **no** (`publish = false`) | `[budget]` tighten-only enforcement, epoch-based wall-clock deadline, concurrency cap, two new `WasmError` variants | never published in this shape |
| `kelson-demo` | **no** (`publish = false`) | 503 render pins for both new codes, plus the facet suite | never published in this shape |

Two facts worth recording while the wave is open:

* **`ModuleBudget` is public API from the moment it lands**, because it hangs
  off the public `ModuleManifest`. It carries `#[serde(deny_unknown_fields)]`,
  so adding a field to the Rust struct breaks any caller constructing one
  literally - keep new `[budget]` fields additive on the TOML side and re-check
  this gate before widening the struct.
* **The sandbox posture is currently a promise to nobody.** `kelson-wasm` is
  gated, so `docs/15-EXTENSION.md`'s tighten-only rule, the 503 pins and the
  WASM-only plugin boundary are enforced only inside this workspace. Removing
  that gate turns all three into a commitment to every consumer, which is why
  the gate is called out here rather than treated as a packaging detail.

## Prerequisites (human release manager)

1. crates.io account with **two-factor authentication enabled** (mandatory
   for publishing).
2. API token: crates.io -> Settings -> API Tokens. Use a **scoped token**:
   - Wave 1: capability `publish-new` + `publish-update`, crate list restricted
     to `kelson-analyzer`.
   - After Wave 1 lands: regenerate with `publish-update` only.
3. Local login (stores the token in the Cargo credential store):

   ```sh
   cargo login
   ```

   In CI, set the repo secret `CARGO_REGISTRY_TOKEN` (used by
   `keel-publish.yml` only).

## Release flow (local)

```sh
just deny          # supply-chain gate: advisories, licenses, bans, sources
just check         # workspace check + tests + dep-rules
just publish-dry   # packages every publishable crate, uploads nothing
just publish       # cargo publish in dependency order (retries on propagation)
```

`xtask publish` details:

- Discovers publishable crates from the workspace manifests (`publish = false`
  members are skipped and reported in the log).
- Topologically sorts by internal dependencies and runs
  `cargo publish -p <crate>` in order.
- On failure: up to 5 retries with 30 s waits (registry propagation lag);
  "already exists" is treated as success (idempotent re-runs).
- `--only kelson-analyzer` restricts a run to a subset.
- Emits a machine-readable final line for CI:
  `XTASK-PUBLISH: version=... crates=... count=... dry-run=...`
- Fails fast if a publishable crate depends on a gated crate (Wave 2 guard).
- Dry-run: crates blocked only by the first-publish bootstrap gap
  (workspace-internal `kelson-*` deps absent from the crates.io index) are
  reported as `deferred` in the `XTASK-PUBLISH:` line; any other failure
  still fails the run.

## Release flow (CI)

1. Ensure `CARGO_REGISTRY_TOKEN` repo secret is set.
2. Actions -> `kelson-publish` -> Run workflow:
   - `dry_run=true` (default): full gate (ubuntu + windows verify matrix,
     MSRV check, cargo-deny, semver gate) + dry-run packaging.
   - `dry_run=false`: same gates, then actual upload, registry smoke test
     (fresh `cargo new` project depending on the published versions), git tag
     `kelson/platform-vX.Y.Z`, and git-cliff changelog regeneration.
3. The `crates` input optionally restricts the wave to a subset.

## Version bumps

Lockstep via xtask:

```sh
just bump 0.2.0
```

This updates `[workspace.package].version` and all 65 internal dependency
version literals in the root `Cargo.toml`, and validates the replacement
counts before writing. The count is derived at run time by `bump` itself -
a `[workspace.dependencies]` entry that carries both `version = "..."` and
`, path =` - so 65 is a reported figure, not a value the tool depends on.
It was reconciled against both manifests on 2026-10-12: 65 in keel-platform
and 65 in platform (the previous figure of 49 predated several waves and was
never re-derived). After bumping: update `CHANGELOG.md` (or regenerate
with `just changelog kelson/platform-vX.Y.Z` at tag time), then `just
publish-dry`.

## Semver gate

`cargo run -p xtask -- semver` runs `cargo-semver-checks check-release`.
It skips gracefully while no `kelson/platform-v*` tag exists (no baseline);
after the first release it becomes a hard gate in the publish workflow.
Breaking changes before 1.0 follow `API-STABILITY.md` (decision entry +
CHANGELOG note + migration guidance).

## Conventional Commits

Commit messages drive the changelog (git-cliff). Format:

```
type(scope): summary

type  = feat | fix | perf | refactor | docs | style | test | build | ci | chore
scope = free-form, usually a crate name (kelson-foundation, ci, release, ...)
```

See `CONTRIBUTING.md`. Changelog regeneration: `just changelog <tag>`.

## Yank policy

- Published versions are never deleted.
- Yank only fatally broken versions (security hole, data-loss bug, unusable
  build) and immediately ship a patch release.
- Record each yank as an entry in `CHANGELOG.md` under the affected version.

## Troubleshooting

| Symptom | Fix |
|---|---|
| `the remote server requires a token` during publish | `cargo login` (local) or set `CARGO_REGISTRY_TOKEN` (CI) |
| `crate ... already exists` | Version already published - bump the version; re-runs of the same version are safely skipped |
| `dependency ... not available` right after publishing the previous crate | Propagation lag - the retry loop handles it; re-run `just publish` if it still fails (already-published crates are skipped) |
| `cargo publish --dry-run` fails on missing README/LICENSE references | Ensure `README.md` exists in the crate dir and license files exist at the workspace root |
| semver gate fails | Follow `DEPRECATION.md`; if the baseline tag is wrong, verify with `cargo semver-checks check-release --baseline-rev <tag>` locally |
