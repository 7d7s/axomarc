# PUBLISHING.md - Keel platform release engineering

This document is the single source of truth for publishing Keel crates to
crates.io. Tooling: `xtask publish` / `xtask bump` (see `platform/xtask`).
Fast path: `just publish-dry` then `just publish`.

## Crate registry status

| Item | Status |
|---|---|
| Registry | [crates.io](https://crates.io) (default Cargo registry) |
| License | MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) |
| Versioning | Lockstep: one workspace version for all crates (`[workspace.package].version`) |
| Versioning history | All internal dependency entries in root `Cargo.toml` carry the same literal version |
| Tags | Namespaced git tags: `keel/platform-vX.Y.Z` |

## Wave status

### Wave 1 (live) - `keel-analyzer`

`keel-analyzer` is the only crate published so far.

### Wave 2 rename - EXECUTED (2026-09-28)

Four package names were squatted on crates.io (verified live, no published
versions, treated as unrecoverable). All four have been renamed in this
workspace; their `publish = false` gate and Wave 2 comment are removed, and
the workspace builds green (`cargo check` / `cargo test --locked` /
`xtask dep-rules`) on the new names.

| Old package | New package | Notes |
|---|---|---|
| `keel-core` | `keel-foundation` | `crates/foundation/core` dir unchanged; imports `keel_core` -> `keel_foundation` |
| `keel-sessions` | `keel-session` | `crates/security/sessions` dir unchanged; imports `keel_sessions` -> `keel_session` |
| `keel-events` | `keel-event` | `crates/async/events` dir unchanged; imports `keel_events` -> `keel_event` |
| `keel-cli` | `keel-console` | dir moved `tools/keel-cli` -> `tools/keel-console`; self-reinvoke in `src/main.rs` is `-p keel-console`; `[[bin]] name = "keel"` unchanged |
| (new) `keel-framework` | `keel-framework` | umbrella crate in `crates/framework` re-exporting all 39 `crates/**` crates - NOT built yet |

Every framework crate transitively depends on `keel-foundation`, so nothing
else could upload until this rename landed. Replacement names were verified
available on crates.io before the sweep.

### Wave 2 (remaining release steps) - the remaining 52 crates

1. ~~Perform the renames (package names, dependency keys, imports, doc
   references in `platform/docs/*` and `platform/API-STABILITY.md`)~~ - DONE.
2. ~~Remove the `publish = false` gate (and the Wave 2 comment) from the four
   renamed crates' manifests~~ - DONE.
3. Add `keel-framework` to `members` and `[workspace.dependencies]`.
4. `just deny && just publish-dry` must both pass.
5. Publish the wave: `just publish` (local) or dispatch `keel-publish.yml`
   with `dry_run=false`. The topologically ordered run publishes all 53
   crates in one pass with registry-propagation retries.

## Prerequisites (human release manager)

1. crates.io account with **two-factor authentication enabled** (mandatory
   for publishing).
2. API token: crates.io -> Settings -> API Tokens. Use a **scoped token**:
   - Wave 1: capability `publish-new` + `publish-update`, crate list restricted
     to `keel-analyzer`.
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
- `--only keel-analyzer` restricts a run to a subset.
- Emits a machine-readable final line for CI:
  `XTASK-PUBLISH: version=... crates=... count=... dry-run=...`
- Fails fast if a publishable crate depends on a gated crate (Wave 2 guard).

## Release flow (CI)

1. Ensure `CARGO_REGISTRY_TOKEN` repo secret is set.
2. Actions -> `keel-publish` -> Run workflow:
   - `dry_run=true` (default): full gate (ubuntu + windows verify matrix,
     MSRV check, cargo-deny, semver gate) + dry-run packaging.
   - `dry_run=false`: same gates, then actual upload, registry smoke test
     (fresh `cargo new` project depending on the published versions), git tag
     `keel/platform-vX.Y.Z`, and git-cliff changelog regeneration.
3. The `crates` input optionally restricts the wave to a subset.

## Version bumps

Lockstep via xtask:

```sh
just bump 0.2.0
```

This updates `[workspace.package].version` and all 49 internal dependency
version literals in the root `Cargo.toml`, and validates the replacement
counts before writing. After bumping: update `CHANGELOG.md` (or regenerate
with `just changelog keel/platform-vX.Y.Z` at tag time), then `just
publish-dry`.

## Semver gate

`cargo run -p xtask -- semver` runs `cargo-semver-checks check-release`.
It skips gracefully while no `keel/platform-v*` tag exists (no baseline);
after the first release it becomes a hard gate in the publish workflow.
Breaking changes before 1.0 follow `API-STABILITY.md` (decision entry +
CHANGELOG note + migration guidance).

## Conventional Commits

Commit messages drive the changelog (git-cliff). Format:

```
type(scope): summary

type  = feat | fix | perf | refactor | docs | style | test | build | ci | chore
scope = free-form, usually a crate name (keel-foundation, ci, release, ...)
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
