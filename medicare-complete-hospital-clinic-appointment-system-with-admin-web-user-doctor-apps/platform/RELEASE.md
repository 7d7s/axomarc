# RELEASE.md — Keel build & deployment matrix (STAGE-0-SPEC §12, L66)

`keel build` wraps cargo with the matrix below and writes
`storage/framework/release.json` (keel-deployment `ReleaseManifest`).
Promote named artifacts, never "whatever is in target/".

## Build matrix

| Binary       | Posture                       | Profile        | Notes                                  |
|--------------|-------------------------------|----------------|----------------------------------------|
| keel-api     | HTTP runtime (axum)           | dev / release  | serves /meta/boot, /healthz, routes    |
| keel-worker  | durable job runner            | dev / release  | claim loop + outbox pump + scheduler   |
| keel         | platform CLI                  | dev / release  | doctor/check/migrate/queue/build/...   |

## Commands

- `keel build`            release build of all three binaries + manifest
- `keel build --profile dev` fast dev build (debug=1 per workspace profile)
- `keel release`          print the current release manifest (if present)

## Postures (selected by binary + config env, §12)

- api: config env dev/test/staging/prod -> logging format json|pretty,
  admission pipeline active, degraded boot allowed (D-52).
- worker: same config; degraded boot swaps postgres job store for the
  in-memory store and logs a warning.
- migration: `keel migrate` commands only; never runs from api/worker boot
  except the boot-time migration pass when `database.migrate_on_boot`.

## Promotion states (keel-deployment)

Built -> Staged -> Deployed; any non-terminal state -> RolledBack.
Rollbacks rebuild or redeploy a previous manifest; they never mutate
the current one.

## Release manifest fields

version, profile, artifacts[], git_commit, git_dirty, built_at, rustc.
`keel doctor` verifies toolchain/config assets; `keel check` enforces the
architecture gates (G07) before any build should ship.
