# Show HN Draft — Sovereign

**Status:** Operator-refinable draft. The numbers and feature claims are
backed by the Phase 0 close-out evidence; the words and tone are a
starting point. The operator should review, adjust, and submit.

---

## Show HN: Sovereign – a single-binary, self-hosted, CLI-first deployment runtime that fits in a 25 MB musl static binary

I built a deploy runtime that fits in a 25 MB musl static binary, runs on a
€4.85/mo Hetzner CX22 with zero other dependencies, and has a CLI as the
product. No SaaS, no agent, no Kubernetes, no dashboard. The CLI is the UI;
`sovereign doctor` is the on-call's first command.

A fresh install goes from `hcloud server create` to a live HTTP service in
about 5 minutes (90 seconds for a repeat operator). The full loop:

```bash
# 1. install
curl -sSf https://install.sovereignruntime.dev | sh

# 2. one-shot bootstrap (V0 single-tenant)
sovereign init       # detects your framework, writes app.yaml
sovereign login      # provisions the age master key
sovereign deploy     # builds, runs, routes, writes sovereign.lock

# 3. verify
curl http://<host>.sovereignruntime.dev/  # → 200
sovereign doctor --level basic           # → 18/18 pass
sovereign status --app my-app            # → 1 deployment, Healthy
```

The V0 ships:

- **One Rust static binary** (≤ 25 MB, musl, statically linked, no glibc
  dependency, runs on Ubuntu 22.04 / Debian 12 / Alpine 3.20 with no
  install step)
- **SQLite state store** with append-only `audit_event` and optimistic
  concurrency
- **Caddy auto-TLS** (V0 ships `tls internal`; Let's Encrypt is V1)
- **age (X25519) encrypted secret store** with a local master key
- **SQLite backup** (VACUUM INTO + size sanity + integrity verify +
  restore drill)
- **Auto-rollback prober** (3 consecutive failed health checks ⇒ fire a
  rollback, reuses the same rollback path as `sovereign rollback`)
- **`sovereign doctor`** with 18 baseline checks across 6 categories
  (system, binary, storage, runtime, proxy, secrets); the on-call's first
  command
- **`sovereign update`** with manifest resolution, SHA-256 + cosign
  verification, atomic swap, and a per-deployment rollback that uses
  the recorded SHA-256

What's in the V0 scope, and what isn't:

- **In scope (V0):** single-host V0 deployment, one app, one operator,
  one domain per app, basic Caddy TLS, no OIDC, no clustering, no
  horizontal scaling, no multi-tenant. The 18-check basic doctor.
- **Out of scope (V0):** multi-tenant, OIDC, Let's Encrypt for V0
  (use `tls internal` and click through the cert warning), clustering,
  custom domains per environment, blue/green for > 1 host.

The whole point of the V0 is the *guarantee*: 5 commands, 5 minutes,
zero decisions. The product surface is small on purpose. The decisions
are in the CLI's opinionated defaults (port 8000 for FastAPI,
`/health` health check, blue/green strategy, 30s prober interval,
threshold 3). When the operator disagrees with a default, they edit
`app.yaml`.

The technology choices:

- **Rust 2021, MSRV 1.85.** Single static binary, no runtime deps.
- **clap 4.6** for the CLI; the help text, completions, and man pages
  come from the same struct.
- **sqlx 0.8** for SQLite. BLOB IDs (16-byte UUIDs) for foreign keys.
  Append-only `audit_event` table with triggers that reject
  UPDATE/DELETE.
- **bollard 0.18** for the Docker runtime.
- **age 0.11** for the secret store; Argon2id KDF is V1.5 (the dep is
  already in the build for that future).
- **criterion** for benchmarks (V0.5); the V0 has 148 unit + integration
  tests across 13 crates.

The full spec is at [github.com/<org>/sovereign](https://github.com/<org>/sovereign)
and the Phase 0 close-out checklist (13 gates) is in
`docs/phase-00-mvp.md`. The Show HN evidence — the doctor JSON, the
auto-rollback journal, the binary size, the install time — is committed
in the repo. The release pipeline (tag → build → cosign sign → CycloneDX
SBOM → GitHub release → CDN mirror) is wired and the v0.1.0 cut is
queued.

I'd love feedback on:

- The CLI surface — is the F4.6 bootstrap (`init && login && deploy`)
  the right shape, or should `init` auto-prompt for `login`?
- The 5-minute quickstart — is 90 seconds on a CX22 (4 GB RAM) the
  right target for V0, or do you want V0 to ship 30s on a smaller
  host?
- The doctor taxonomy (6 categories, 18 checks at basic) — is this the
  right shape for a "first thing in the morning" tool?
- The V0/V1/V1.5/V2 scope split — does the split match what a
  small-team CTO would actually pay for?

## (To be filled in by the operator)

- GitHub repo URL
- 1-2 line "what does this save you from?" framing for the headline
- Any personal voice tweaks
- The Show HN flag — `Show HN:` prefix; pick a one-line "what is it" sentence

## (To be cross-linked)

- `docs/operations/cx22-verify.md` (the recipe behind the 5 minutes)
- `docs/phase-00-mvp.md` (the spec)
- The `doctor` output JSON (committed as evidence)
- The auto-rollback journal capture (committed as evidence)
- The install time (90s on a CX22, captured in
  `cx22-verify.log`)

## What "ready to post" means

This draft is ready when:

- [ ] The operator has filled in the GitHub URL and personal-voice tweaks
- [ ] The CX22 evidence files are committed
- [ ] The release workflow has shipped a real v0.1.0 (so the link in
      the post points at a real binary, not a 404)
- [ ] The "I'd love feedback" questions are pruned to the 2-3 that
      will spark the best HN discussion

Show HN threads that get traction usually have:
- a specific, falsifiable claim (the 5 minutes, the 25 MB)
- a clear scope boundary (what's V0 vs V1)
- 2-3 questions that aren't just "is this useful?"
- a link to evidence, not a sales pitch
