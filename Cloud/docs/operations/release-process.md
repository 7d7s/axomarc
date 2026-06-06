# Release Process

**Status:** Locked for Phase 0 close-out. The full release
pipeline lives in [`.github/workflows/release.yml`](../../.github/workflows/release.yml);
this document is the operator recipe.

## TL;DR

```bash
# 1. Pick a version (semver, no leading 'v' in the cargo bump)
cargo set-version --bump minor 0.1.0

# 2. Update CHANGELOG.md (move the [Unreleased] section into
#    a [vX.Y.Z] - YYYY-MM-DD block).

# 3. Commit + tag
git add -A
git commit -m "release: v0.1.0"
git tag -s v0.1.0 -m "v0.1.0"

# 4. Push the tag (NOT a force-push; the tag is the trigger)
git push origin v0.1.0

# 5. Watch the release workflow
gh run watch

# 6. After ~10 minutes, the GitHub release is live AND mirrored
#    to https://releases.sovereignruntime.dev/v0.1.0/. Verify:
gh release view v0.1.0
```

## What the release workflow does

The pipeline (`.github/workflows/release.yml`) runs six jobs in dependency order:

```
build  →  sign  →  sbom  →  manifest  →  release  →  mirror
```

1. **build** (matrix: x86_64-unknown-linux-musl, aarch64-unknown-linux-musl)
   - `cargo build --release --target <T> --bin sovereign`
   - Strips debug info a second time (belt-and-braces)
   - Asserts binary size ≤ 25 MB
   - Packages the binary + `install.sh` + `Caddyfile` into
     `sovereign-${VERSION}-${TARGET}.tar.xz`
   - Writes a per-target `sha256sums.txt`
   - Uploads the tarball, checksum file, and the stripped binary
     as workflow artifacts

2. **sign** (matrix: same as build)
   - Downloads the tarball
   - `cosign sign-blob --yes` (keyless OIDC; requires
     `id-token: write` permission and `COSIGN_EXPERIMENTAL=1`)
   - Produces `.sig` (signature) and `.bundle` (Sigstore
     transparency-log entry) for each tarball
   - Uploads the signed artifacts

3. **sbom** (single job)
   - `cargo install cargo-cyclonedx`
   - `cargo cyclonedx --format json` for the `sovereign` binary
   - Output: `target/cyclonedx/sovereign.cdx.json` (CycloneDX 1.5)

4. **manifest** (single job)
   - Waits for build + sign + sbom
   - Downloads all artifacts, computes per-artifact SHA-256,
     writes `manifest.json` with the artifact URLs, sizes,
     checksums, and a pointer to the SBOM
   - The manifest is what `sovereign update check` consumes
     (the same shape that `commands_update.rs::run_check` reads)

5. **release** (single job)
   - `softprops/action-gh-release@v2`
   - Creates the GitHub release (with the RELEASE_NOTES.md
     extracted from `CHANGELOG.md` for the matching version)
   - Attaches all the tarballs, signatures, bundles, the
     SBOM, and the manifest

6. **mirror** (single job)
   - Waits for the GitHub release
   - Syncs the same artifacts to the Hetzner Storage Box
     bucket `s3://sovereignruntime-releases/${TAG}/`
   - Also copies `manifest.json` to `stable.json` (the
     `install.sh` reads this to resolve the latest version)
   - Bucket is the install-time source of truth; the GitHub
     release is the immutable audit log

## Why not cargo-dist

`cargo-dist` is the canonical Rust release tool, and we'd
probably adopt it in V1 for the cross-platform (macOS, Windows)
artifact matrix. For the Phase 0 close-out we ship a hand-rolled
workflow because:

1. The release matrix is tiny (2 musl targets, no macOS/Windows
   yet — V0 is Linux-only).
2. We need fine-grained control over the cosign + SBOM steps,
   which is awkward to express in `cargo-dist`'s
   `[dist.custom-build-hook]` plumbing.
3. The Show HN post deadline is "this month"; adopting
   `cargo-dist` is a V1 cleanup.

When we cut v0.5 (post-Show HN), the plan is to migrate to
`cargo-dist` for the macOS/Windows leg. The hand-rolled
workflow will stay for the cosign + SBOM + manifest steps
that `cargo-dist` doesn't natively produce.

## Required secrets

The release workflow reads three secrets from the GitHub repo
settings; the operator must set them before the first release:

| Secret | Purpose | Source |
|---|---|---|
| `SOVEREIGN_CDN_ACCESS_KEY_ID` | S3-compatible access key for the Hetzner Storage Box | Hetzner Cloud Console → Storage Box → Access Keys |
| `SOVEREIGN_CDN_SECRET_ACCESS_KEY` | S3-compatible secret key | (same) |
| `SOVEREIGN_CDN_ENDPOINT` | S3 endpoint URL (`https://<region>.your-storagebox.de`) | (same) |

The `GITHUB_TOKEN` is auto-injected by GitHub Actions; no
secret to set.

## Why two keys (or one) — the cosign key decision

V0 uses **keyless OIDC signing** via Sigstore. The signature is
attested to the GitHub Actions OIDC token; the public key is
the GitHub repo's `https://github.com/<org>/<repo>.git` reference.
This means:

- No key to manage or rotate
- The transparency log (Rekor) is the audit trail
- A consumer can verify the signature with `cosign verify-blob
  --certificate-identity-regexp "https://github.com/<org>/<repo>/.github/workflows/release.yml@refs/tags/v.*"
  --certificate-oidc-issuer "https://token.actions.githubusercontent.com"`
  --bundle <bundle> <tarball>`

V0.5 (post-Show HN) will add a **second, key-based signature**
with a long-lived key for the air-gapped use case (the F10
"manifest not reachable; treating as 'no update available'"
fallback can't verify a keyless signature offline). The
workflow will add a second `cosign sign-blob` step with
`--key <kms-or-hsm>`.

## What "done" means for Step 10

Step 10 is complete when ALL of:

- [x] `.github/workflows/release.yml` exists with the 6-job matrix
- [x] `CHANGELOG.md` exists (so the release notes automation works)
- [x] `install.sh` consumes the same artifact layout the workflow
  produces (it does; no changes needed)
- [x] This document exists (operator recipe)
- [ ] The three CDN secrets are set in GitHub repo settings (operator action)
- [ ] The first tag (v0.1.0) is cut (operator action)
- [ ] The first GitHub release is live with all artifacts (operator action)
- [ ] `curl -sSf https://install.sovereignruntime.dev | sh` works
  on a fresh CX22 (operator action)
