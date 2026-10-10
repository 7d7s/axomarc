# Runbook: Kelson platform release (crates.io)

Owner: release manager. Companion doc: `platform/PUBLISHING.md`.

## Preflight (T-1 day)

- [ ] Working tree clean; `main` green (kelson CI badge green, publish-dry passing on main)
- [ ] `CHANGELOG.md` reflects everything in this release (Conventional Commits since last tag)
- [ ] Version decided: lockstep `X.Y.Z` (`just bump X.Y.Z` if bumping)
- [ ] crates.io account: 2FA on, scoped token valid (`cargo whoami` succeeds)
- [ ] `just deny` clean (no new advisories/license/ban findings)
- [ ] `just check` green (workspace check + tests + dep-rules)
- [ ] `cargo run -p xtask -- semver` passes (auto-skips before the first tag)

## Execute

- [ ] Local path: `just publish-dry` (all crates package cleanly) then `just publish`
  - OR CI path: Actions -> kelson-publish -> Run workflow with `dry_run=true`,
    inspect logs, then re-run with `dry_run=false`
- [ ] Watch the run: verify matrix (ubuntu+windows), MSRV, deny, semver, publish, smoke, tag
- [ ] Confirm `XTASK-PUBLISH:` summary line: version + crate list + count

## Verify (post-publish)

- [ ] Each published crate visible on crates.io at `X.Y.Z`
- [ ] docs.rs builds succeed (may take minutes; check https://docs.rs/kelson-analyzer)
- [ ] Smoke job green (fresh project builds against registry versions)
- [ ] Tag `kelson/platform-vX.Y.Z` exists and changelog commit landed

## Rollback

crates.io versions cannot be deleted.

- Broken version: yank it (`cargo yank --version X.Y.Z kelson-analyzer`), record
  in CHANGELOG, ship a patch release the same day.
- Token compromise: revoke the scoped token immediately in crates.io settings,
  rotate the `CARGO_REGISTRY_TOKEN` secret, audit crate/team activity.

## Wave 2 checklist (future)

Follow `platform/PUBLISHING.md` -> "Wave 2": apply the pre-approved rename
table, un-gate the four crates, add the `kelson-framework` umbrella, run the
full preflight above, then publish the wave in one coordinated run.
