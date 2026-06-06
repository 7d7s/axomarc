# Changelog

All notable changes to Sovereign are documented in this file. The
format is loosely based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

> **How to read this file.** The latest unreleased changes are
> at the top. Each released version has a section header
> `## [vX.Y.Z] - YYYY-MM-DD`. The first release (v0.1.0) will
> mark the Phase 0 close-out.

## [Unreleased]

### Added

- `sovereign init` (F4.6): framework detector + `app.yaml` writer
  for FastAPI / Next.js / Express / Go / Rails / Laravel / Astro
  / Static / Generic. Supports `--force`, `--output`, `--name`.
- `sovereign login` (F4.6): V0 single-tenant master-key
  init/load. Passphrase from `SOVEREIGN_PASSPHRASE` or stdin;
  `--no-input` requires the env var. Prints the Bech32 public
  key on success.
- `sovereign update check|apply|rollback|history` (F10): manifest
  resolution, SHA-256 + cosign verification, atomic swap, and a
  per-deployment rollback path that uses the recorded SHA-256.
- `sovereign doctor --level basic` (F9): 18 checks across 6
  categories. Non-Linux checks default to Skip.
- `sovereign backup create|list|verify|restore` (F8a): VACUUM
  INTO snapshot, size sanity check, integrity verify on a fresh
  pool, atomic restore to a destination path.
- `sovereign-secret set|list|rotate` (F7): age (X25519)
  envelope-encrypted secret store with master-key bootstrap.
- `sovereign domain add|list` (F6): Caddy auto-TLS, route
  stability by host hash, `--app` selector.
- Auto-rollback prober (F8b): threshold-based, one rollback
  per prober lifetime, reuses `rollback::start_rollback`.
- `crates/sovereign-doctor/`: 18-check V0 basic-level doctor.
- `crates/sovereign-update/`: `HttpUpdate` (reqwest) + `MockUpdate`
  (tests) for the F10 self-update port.
- `crates/sovereign-backup/`: `FileBackupSink` adapter.
- Hetzner CX22 verification recipe + paste-and-run script
  ([`docs/operations/cx22-verify.md`](./docs/operations/cx22-verify.md),
  [`scripts/cx22-quickstart.sh`](./scripts/cx22-quickstart.sh)).
- Local distro verification script
  ([`scripts/verify-distros.sh`](./scripts/verify-distros.sh)):
  runs the musl binary in `ubuntu:22.04`, `debian:12`,
  `alpine:3.20`.
- GitHub release pipeline
  ([`.github/workflows/release.yml`](./.github/workflows/release.yml)):
  tag-triggered musl build (x86_64 + aarch64), cosign keyless
  signing, CycloneDX SBOM, manifest.json, GitHub release,
  CDN mirror.
- CI workflow
  ([`.github/workflows/ci.yml`](./.github/workflows/ci.yml)):
  fmt + clippy + deny + audit + test + musl build + distro
  verify.

### Changed

- CLI: `sovereign init` and `sovereign login` are no longer
  F2 stubs; they are the V0 single-tenant bootstrap.
- `Framework` clap enum gained `Express` and `Generic`.
- Install script: the "what next" message now mentions
  `sovereign login`.

### Fixed

- F10 `Version::as_str()` formatting bug (cosmetic; ordering
  is correct).
- F8b test fixture `NewApp.config_yaml` is now set in the
  storage-layer test so the non-empty validation passes.

## [v0.1.0] - 2026-06-06

Initial Phase 0 release. See the README for the feature list
and `docs/phase-00-mvp.md` for the spec this release implements.
The first tagged release will be cut by the operator after the
Show HN post is drafted; the tag-triggered release workflow
([`.github/workflows/release.yml`](./.github/workflows/release.yml))
runs the build, sign, SBOM, and CDN mirror jobs in that order.

[unreleased]: https://github.com/sovereignruntime/sovereign/compare/v0.1.0...HEAD
[v0.1.0]: https://github.com/sovereignruntime/sovereign/releases/tag/v0.1.0
