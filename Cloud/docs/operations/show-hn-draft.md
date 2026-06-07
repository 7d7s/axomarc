# Show HN Draft — Sovereign

**Status:** Operator-pasteable draft. The headline, the install
recipe, and the "in scope / out of scope" boxes are final.
The personal-voice tweaks and the GitHub URL are operator
fill-ins. The numbers come from the V0 close-out evidence
(committed in `docs/operations/cx22-evidence-<date>.md`).

---

## Show HN: Sovereign – a 25 MB static binary that turns a €4.85/mo Hetzner box into a one-command deploy host

I built a deployment runtime that fits in a 25 MB musl static
binary, runs on a €4.85/mo Hetzner CX22 with zero other
dependencies, and has a CLI as the product. No SaaS, no
agent, no Kubernetes, no dashboard. The CLI is the UI;
`sovereign doctor` is the on-call's first command.

A fresh install goes from `hcloud server create` to a live
HTTP service in about 5 minutes (90 seconds for a repeat
operator). The full loop:

```bash
# 1. install
curl -sSf https://install.sovereignruntime.dev | sh

# 2. one-shot bootstrap (V0 single-tenant)
sovereign init       # detects your framework, writes app.yaml
sovereign login      # provisions the Argon2id-wrapped age master key
sovereign deploy     # builds, runs, routes, writes sovereign.lock

# 3. verify
curl http://<host>.sovereignruntime.dev/   # → 200
sovereign doctor --level basic            # → 18/18 pass
sovereign status --app my-app             # → 1 deployment, Healthy
```

The V0 ships:

- **One Rust static binary** (≤ 25 MB, musl, statically linked,
  no glibc dependency, runs on Ubuntu 22.04 / Debian 12 /
  Alpine 3.20 with no install step beyond `cp`)
- **SQLite state store** with append-only `audit_event` (triggers
  reject UPDATE/DELETE) and optimistic concurrency via row
  `version` columns
- **Caddy auto-TLS** (V0 ships `tls internal`; Let's Encrypt is
  V0.7)
- **age (X25519) encrypted secret store** with an Argon2id-wrapped
  master key (64 MiB / t=3 / p=1, XChaCha20-Poly1305 AEAD; the
  V0.1.0 bare-Bech32 file is migrated in-place by
  `sovereign login --migrate`)
- **SQLite backup** (VACUUM INTO + size sanity + integrity
  verify on a fresh pool + atomic restore)
- **Auto-rollback prober** (3 consecutive failed health checks
  ⇒ fire a rollback, reuses the same rollback path as
  `sovereign rollback`)
- **`sovereign doctor`** with 18 baseline checks across 6
  categories (system, binary, storage, runtime, proxy, secrets);
  the on-call's first command
- **`sovereign update`** with manifest resolution, SHA-256 +
  cosign keyless-OIDC verification, atomic swap, and a
  per-deployment rollback that uses the recorded SHA-256

What's in the V0 scope, and what isn't:

- **In scope (V0):** single-host V0 deployment, one app, one
  operator, one domain per app, basic Caddy TLS, no OIDC, no
  clustering, no horizontal scaling, no multi-tenant. The
  18-check basic doctor.
- **Out of scope (V0):** multi-tenant, OIDC, Let's Encrypt for
  V0 (use `tls internal` and click through the cert warning),
  clustering, custom domains per environment, blue/green for
  > 1 host.

The whole point of the V0 is the *guarantee*: 5 commands, 5
minutes, zero decisions. The product surface is small on
purpose. The decisions are in the CLI's opinionated defaults
(port 8000 for FastAPI, `/health` health check, blue/green
strategy, 30s prober interval, threshold 3). When the operator
disagrees with a default, they edit `app.yaml`.

The technology choices:

- **Rust 2021, MSRV 1.85.** Single static binary, no runtime
  deps. Trunk-based development with the V0 line on
  `main`; per-feature branches `phase-0/<N>-<slug>` merge via
  `--no-ff` to keep the history.
- **clap 4.6** for the CLI; the help text, completions, and
  man pages come from the same struct.
- **sqlx 0.8** for SQLite. BLOB IDs (16-byte UUIDs) for foreign
  keys. Append-only `audit_event` table with triggers that
  reject UPDATE/DELETE.
- **bollard 0.18** for the Docker runtime (features: `pipe, http`).
- **age 0.11** for the secret store, **argon2 0.5** for the KDF,
  **chacha20poly1305 0.10** for the AEAD.
- **210 unit + integration tests** across 11 crates; the gates
  are `cargo fmt --all -- --check`, `cargo clippy --workspace
  --all-targets -- -D warnings`, `cargo deny check`, and
  `cargo test --workspace --no-default-features`. All four
  pass on every PR.

The full spec is at
[github.com/<org>/sovereign](https://github.com/<org>/sovereign)
and the Phase 0 close-out checklist (13 gates) is in
`docs/phase-00-mvp.md`. The Show HN evidence — the doctor
JSON, the auto-rollback journal, the binary size, the install
time, the 210-test pass — is committed in the repo. The
release pipeline (tag → build → cosign sign → CycloneDX SBOM
→ GitHub release → CDN mirror) is wired and the v0.1.0 cut is
imminent.

What's next, in public:

- **V0.6 (3 sub-phases over ~3 months):** `sovereign service
  install nginx | mysql | vsftpd | phpmyadmin | letsencrypt`
  (system services, not containers), per-user RBAC and
  OIDC, and a per-user Telegram chatops adapter so the
  operator (and eventually each end-user) can drive the
  host from their phone.
- **V1.0:** hardening, the first multi-tenant design
  partner, 50 installs/week, 10 active production users,
  and the BSI C5 case study.

I'd love feedback on:

- The CLI surface — is the V0.5 bootstrap
  (`init && login && deploy`) the right shape, or should
  `init` auto-prompt for `login` and the KDF?
- The 5-minute quickstart — is 90 seconds on a CX22 (4 GB
  RAM) the right target for V0, or do you want V0 to ship
  30s on a smaller host?
- The doctor taxonomy (6 categories, 18 checks at basic) —
  is this the right shape for a "first thing in the morning"
  tool?
- The V0 → V0.6 → V1.0 scope split — does the multi-tenant
  + system-services + Telegram chatops block belong in V0.6,
  or should I cut it back to the deployment runtime and
  ship that as V1?

## (Operator fill-ins, ~5 minutes of work)

- [ ] GitHub repo URL
- [ ] 1-2 line "what does this save you from?" framing for
      the headline (the current draft leads with size;
      "5 minutes, 5 commands, €4.85/mo" or "your own Vercel
      for the cost of a Pi" might land better)
- [ ] Personal-voice tweaks (the draft is third-person;
      Show HN threads do better in first-person — "I built
      this" not "I built a deploy runtime")
- [ ] Pick the 2-3 questions from the "I'd love feedback"
      list that will spark the best HN discussion

## (Cross-links, post-cut)

- `docs/operations/cx22-verify.md` (the recipe behind the 5
  minutes)
- `docs/operations/cx22-evidence-<date>.md` (the captured
  doctor + rollback + install-time evidence)
- `docs/phase-00-mvp.md` (the spec)
- `CHANGELOG.md` (the v0.1.0 release notes)
- The doctor output JSON (committed as evidence under
  `docs/operations/evidence/doctor.json`)

## What "ready to post" means

This draft is ready when:

- [ ] The operator has filled in the GitHub URL and
      personal-voice tweaks
- [ ] The CX22 evidence files are committed
- [ ] The release workflow has shipped a real v0.1.0 (so
      the link in the post points at a real binary, not a
      404)
- [ ] The "I'd love feedback" questions are pruned to 2-3
- [ ] One re-read for "this sounds like a real person, not
      a launch post"

Show HN threads that get traction usually have:

- a specific, falsifiable claim (5 minutes, 25 MB, €4.85/mo)
- a clear scope boundary (what's V0 vs V0.6 vs V1.0)
- 2-3 questions that aren't just "is this useful?"
- a link to evidence, not a sales pitch
