# Sovereign Application Runtime — Executive Summary (1-Page TL;DR)

**Date:** 2026-06-04
**For:** Founder, co-founders, lead engineer, first angel investor
**Reads in:** ~5-7 minutes
**Companion docs (in same folder):** `Research-1.txt` (spec), `Research-2.md` (first synthesis), `Research-3.md` (7-persona synthesis, ~100 KB), plus 7 persona files and 4 supporting research files

---

## 0. The pitch (read this first)

> **"Run your own cloud. Single binary. No DevOps team."**

That is the one USP we defend for 24 months. It is specific, ownable, testable in 5 minutes, and combines three moats no single competitor owns today: single-binary Rust, sovereign-by-construction, AI-agent-friendly. Everything below exists to make that sentence true on a €4.49 Hetzner box for a solo founder, and still true at €5-25M ARR for the EU mid-market.

The product is **the Plausible of self-hosted application deployment** — not a PaaS, not an IDP, not "Kubernetes without Kubernetes." Same trajectory: bootstrapped or near-bootstrapped, EU-incorporated, Apache 2.0 unmodified, 4-5 person team at €1M ARR, foundation transfer by year 3. [Plausible's "we chose open source and we have no regrets"](https://plausible.io/blog/open-source) is the model, not Supabase's.

---

## 1. The problem (sourced, not vibes)

- **Cloud repatriation is real, not a fad.** The EU's IPCEI-CIS commitment alone is €1.2 bn through 2026, and EUCS Substantial is now the procurement floor for German Mittelstand, French regulated industry, and any public-sector deal touching citizen data. ([EU IPCEI-CIS](https://digital-strategy.ec.europa.eu/en/policies/ipcei-cis), [BSI C5:2026](https://www.bsi.bund.de/EN/Themen/Cloud-Computing/Compliance-Kriterien/Compliance-Kriterien_node.html))
- **Solo devs and 1-50 engineer orgs are the underserved cell.** Coolify (56k stars) and Dokploy (34k) prove demand; their license/sovereignty/abandonment patterns prove the gap. ([Dokploy #3613](https://github.com/Dokploy/dokploy/issues/3613), [CasaOS announcement](https://github.com/IceWhaleTech/CasaOS))
- **"Vercel UX, VPS pricing" is the unlocked positioning.** PIER (single-binary Rust deploy), yoink, sh0 prove the tech; nobody has combined it with the "no DevOps team" claim.
- **AI agents are a first-class user now.** Claude Code, Cursor, and Aider drive `git push && tool deploy` workflows; the CLI must be agent-friendly from day one or it loses the next 24 months of adoption.

The cost of doing nothing is Dokploy, CasaOS, or Heroku — the buyer will pick whichever of those still exists in 3 years, on whatever terms the new owner dictates.

---

## 2. The customer (5 personas, 1 line each)

| Persona | Server | Pain | Why us |
|---|---|---|---|
| **Mira**, 32, solo founder of a B2B SaaS | 1 Hetzner CX32 (€8.59/mo) | 4-6 deploys/day, last SSH 11 days ago, on call | 5 commands to live URL; rollback is one command |
| **Karim**, 41, agency owner | 1 dedicated, 12 client sites | 5 freelancers, white-label audit, fleet ops | `tool fleet` TUI + per-client RBAC + bulk `backup verify` |
| **Lin**, 28, infra at a 40-person startup | 3 prod, 1 staging, 1 dev | 5-week deploy cadence, owns on-call | `tool policy check` + `tool audit export` + rqlite HA in V2 |
| **Sara**, 35, EU public-sector IT lead | BSI C5:2026 mandate | 121 controls, auditable everything | `tool sovereignty check` (10/10) + BSI C5 mapping doc |
| **Alex**, 24, vibe coder with Claude Code | 1 Hetzner CX22 (€4.49/mo) | Wants `git push` and to wake up to a working app | `--json` everywhere; the agent drives the CLI |

Five users, five workflows, one binary. The daily-driver is the CLI; the TUI is the bonus; the web UI is V2 opt-in. ([Fly.io deep-dive](https://fly.io/docs/), [Perspective AI on dev onboarding](https://www.perspective.co))

---

## 3. The product (3 pillars)

1. **One Rust binary (10-25 MB, musl + LTO + abort + strip).** `curl -sSf sovereign.dev/install.sh | sh` → `sovereign init` → `sovereign login` → `sovereign deploy` → live URL on a temporary domain. **Target: 2 min 30 sec to first deploy.** ([Rust release optimization](https://leapcell.io/blog/optimizing-rust-binary-size-techniques-for-building-lightweight-applications))
2. **Hexagonal architecture, `Arc<dyn Port>` DI.** Domain never imports adapters. SQLite V1 → rqlite V2 → never Postgres. State machines are the first code written. ([hexagonal in Rust](https://howtocodeit.com/articles/master-hexagonal-architecture-in-rust))
3. **Layered governance.** Telemetry → ML anomaly scorer (Prophet V2, TimesFM V2.5) → Rego rules (OPA) → human override. Rules are the law; ML is the early-warning radar. 6-month staged rollout, every ML decision replayable from audit log. ([OPA](https://www.openpolicyagent.org/), [Facebook Prophet](https://facebook.github.io/prophet/))

---

## 4. The tech bets (5-6 critical decisions, defended)

| Decision | Pick | Rejected alternative | Why |
|---|---|---|---|
| **Async runtime** | `tokio 1.45+` | async-std (discontinued Mar 2025) | All batteries live here |
| **HTTP server** | `axum 0.8` | actix-web, salvo, rocket | Tower middleware is the killer feature for a control plane |
| **Database** | `sqlx 0.8` + SQLite (WAL) V1 → rqlite V2 | diesel (sync), sea-orm (ORM tax), Postgres (overkill) | Single-file state, forward-only migrations, same `RuntimeState` trait across both |
| **TUI** | `ratatui 0.30` + `crossterm` | cursive, termion (unmaintained) | k9s/lazydocker/yoink pattern is the reference |
| **Allocator** | `mimalloc` (musl) + `jemalloc` (glibc) | std | Recovers 15-60% on alloc-heavy workloads ([benchmarks](https://theeditorial.news/posts/rust-allocators-performance-comparison/)) |
| **Proxy** | Caddy (default) + Nginx 10 MB (low-mem, V1.1) | Traefik, Envoy | Caddy has built-in ACME; Nginx is 6 MB for memory-constrained boxes |
| **State backend** | `RuntimeState` trait, SQLite V1, rqlite V2, Postgres **never** | Hard-coded Postgres | **This is the bet we cannot afford to get wrong** — see §5 |

Stack refuses: never K8s, never service mesh, never custom policy DSL (use OPA/Rego), never multi-cloud abstraction, never a custom container runtime. ([sqlx 0.8](https://docs.rs/sqlx), [axum 0.8](https://docs.rs/axum))

---

## 5. The bet we cannot afford to be wrong about

**Single → multi-server architecture.**

- **V1:** single binary, single host, SQLite, systemd. State on the same host as the apps. No agent.
- **V1.5:** add `agent` mode. Server pushes deploy instructions to agents over mTLS. State on the server's SQLite. Agents are stateless.
- **V2:** rqlite as the optional state backend. 3-node Raft consensus. Control plane survives 1 node failure.
- **V3+:** per-region agent pools, CRDTs for audit log.

**Why this is the bet:** every other decision is recoverable (wrong proxy? swap. wrong TUI lib? migrate. wrong secret scheme? migrate.). Wrong state architecture? You rewrite every command, every endpoint, every agent message, every SQLite query, every backup path. ([rqlite 9.0](https://github.com/rqlite/rqlite))

---

## 6. The 5-minute moment (the metric that defines success)

```text
$ curl -sSf sovereign.dev/install.sh | sh       # 1 — install (10s)
$ sovereign init                                # 2 — detect framework, write app.yaml (5s)
$ sovereign login                               # 3 — device-code flow, browser opens (15s)
$ sovereign deploy                              # 4 — build, TLS, URL (90s)
$ open https://<id>.srvr.so                     # 5 — you are live
```

**Target: 2 min 30 sec.** Above 5 min, Mira is gone. Above 10 min, Alex's Claude Code times out. The framework scanner is the highest-leverage code in V1.

---

## 7. Top 5 risks (with named mitigation)

| Risk | Mitigation |
|---|---|
| **CasaOS-style abandonment** (33k stars → 1 sponsor → rots) | EU incorporation (Berlin GmbH + Estonian OÜ), ≥ 3 core maintainers, foundation transfer plan by year 3, public 18-month sustainability signal ([CasaOS](https://github.com/IceWhaleTech/CasaOS)) |
| **Dokploy-style license backlash** (Jan 2026 source-available for templates/previews) | Apache 2.0 unmodified, **forever**, no `proprietary/` dir, public commitment ([Dokploy #3613](https://github.com/Dokploy/dokploy/issues/3613)) |
| **Vercel-style bill shock** ($46k Jmail bill) | Flat €19/server/month, no per-bandwidth, no per-deploy, no per-seat, public commitment |
| **Single→multi-server rewrite** | Build agent pattern in V1.5, rqlite in V2, `RuntimeState` trait from V1 day 1 |
| **Backups you've never restored** (GitLab 2017-01-31, 8 months of empty backups) | `sovereign backup verify --restore-to scratch` is the single most important V1.2 command; monthly drill mandatory; 3-2-1-1-0 rule ([GitLab postmortem](https://about.gitlab.com/blog/2017/02/10/postmortem-of-database-outage-of-january-31/)) |

Other risks (Caddy OOM, SQLite single-writer, BuildKit untrusted builds, Coolify adding sovereignty, hyperscaler sovereign SKU, ML governance opacity) are documented in `persona-cto.md` §11 with named mitigations.

---

## 8. The 90-day plan

**Goal of the 90 days:** ship V1 to 50 design partners with at least 10 running 1+ production app each. The metric is "did 1 person deploy a real app and keep it for 30 days?"

| Weeks | Milestone | Acceptance |
|---|---|---|
| **1-6** | **The engine (MVP):** single Rust binary, `deploy`/`rollback`/`logs`/`status`/`secret`, Caddy auto-TLS, age-encrypted secrets, `pg_dump` to S3, health-check auto-rollback | 1 developer can use it for 1 real app. Binary < 25 MB. Cold install < 60s. |
| **7-12** | **TUI + scanners:** `sovereign tui` (ratatui), `sovereign init fastapi\|nextjs\|laravel\|go\|rails\|astro`, `sovereign morning-report`, `sovereign golden-check` | Time-to-first-deploy < 5 min for the 6 golden paths. TUI panic-safe. |
| **13-18** | **The product (V1):** audit log, RBAC, `sovereign backup verify` (the single most important V1.2 command), systemd unit, deb/rpm, `mdbook` docs site, Show HN | 50 installs/week, 10 active production users, 1 public BSI C5 reference case. |
| **19-24** | **Trust + first money:** Apache 2.0 public commitment, 10-point sovereignty test in CI, EU incorporation (Berlin GmbH + Tallinn OÜ), Pro tier (€19/server/mo) billing wired, foundation transfer plan published | Apache 2.0 forever, sovereignty test runs in CI, first €1k MRR, BSI C5 mapping doc on the website. |
| **25-36** | **The team (V1.5):** agent pattern, multi-server (no rqlite yet), `declarative apply` (read-only drift, no auto-reconcile), Nginx low-mem, secret rotation | 1 customer with 3-server fleet. 0 unscheduled downtimes. |

The 18-month roadmap (V2 = rqlite HA, OPA/Rego, ML scorer, EUCS mapping, web UI) lives in `Research-3.md` §11.

---

## 9. The non-negotiables (ship, defer, reject)

**Ship in V1 (must):** single binary 10-25 MB; CLI as primary surface; axum + tokio + sqlx + ratatui + clap 4.6; SQLite WAL; Caddy default + Nginx low-mem; Docker V1, Podman V1.5; encrypted zero-disk secrets; auto-TLS; atomic image-tagged deploys; one-command rollback; health-check auto-rollback; Postgres backup + verification; Apache 2.0 unmodified; EU incorporation year 1; 10-point sovereignty test in CI; `--json` everywhere; `--dry-run` on every destructive action; 6 framework scanners; 10 alerts each with a runbook; SLOs 99.5% / 300ms p99 / 60s freshness; 3-2-1-1-0 backups with monthly drill; hexagonal architecture; layered governance; append-only audit log.

**Defer (V2+ or never):** true canary (V2.5+); multi-region failover (V3+); K8s (never); service mesh (never); custom policy DSL (never — OPA); complex 5+ tier RBAC (V3+); plugin marketplace with 3rd-party code (never); gRPC / GraphQL / WebSocket APIs (never); OIDC provider (never — consumer only); mobile / desktop apps (never); auto-scaling (VPS doesn't).

**Reject (do not build, even if requested):** "GDPR compliance" without substance; "immutable audit log" on an editable DB; "air-gapped" without testing; "SOC2 ready" without SOC2; "zero-trust" without ZTA; "AI-driven" anything without a deterministic backup; "real-time" anything without a backpressure plan; anything that requires a SaaS, an account, a credit card, or telemetry to be useful.

---

## 10. The ask (what to do tomorrow)

1. **Stake out the GitHub org + `LICENSE` (Apache 2.0) + `CODEOWNERS` (≥ 3 core maintainers).** Public commit history from day 1.
2. **Start the EU incorporation in parallel.** Berlin GmbH for BSI C5 / EU public-sector deals; Estonian OÜ for the digital-first e-residency team. Two entities, one mission.
3. **Build the 8-feature MVP in 6 weeks.** `sovereign deploy`, `rollback`, `logs`, `status`, `secret set/get`, Caddy auto-TLS, encrypted secrets, Postgres backup. No new features until this works for 1 real app.
4. **Find 10 design partners by week 8.** Solo founders, agencies, EU mid-market. Paid, even if small. The first reference customer matters more than the first €10k.
5. **Write the 10-point sovereignty test in CI before V1 ships.** If it fails, the release doesn't ship. Make it a gate, not a wiki page.
6. **Read `persona-rust.md` (the locked stack) and `persona-principal.md` (the data model).** That is the codebase skeleton. Start there.
7. **Set up the EU sovereign-tech VC pipeline by month 4.** Targets: OSS Capital, Cherry, Speedinvest, Notion, LocalGlobe EU. Pitch: "the Plausible of self-hosted deployment, EU-incorporated, Apache 2.0, sovereignty as a CI gate."
8. **Do not add a web UI in V1.** Do not add K8s support. Do not introduce a `proprietary/` directory. Do not change the license. The discipline of refusal is the moat.

---

## 11. The founding question

> **"If you disappear, what happens to the users?"**

- Apache 2.0: the binary keeps working, the data is portable.
- SQLite state: the database is a single file, restorable on any host.
- Litestream backups: encrypted, to any S3, restorable anywhere.
- `sovereign export`: produces a complete backup of the platform.
- Reproducible install: `curl ... | sh` works offline from a USB stick.
- Public CI proof: a vendor-disappear test runs on every release.
- Foundation transfer plan: by year 3, the project is owned by a foundation.

The Plausible blog post "We chose open source and we have no regrets" is the answer. The Dokploy #3613 thread, the CasaOS abandonment, and Riley Walz's $46k Jmail bill are the anti-patterns. ([Plausible ARR story](https://plausible.io/blog/open-source-funding), [Dokploy #3613](https://github.com/Dokploy/dokploy/issues/3613))

---

## 12. Curated sources (15, the spine of the research)

**Market & buyer:**
1. [Plausible "We chose open source and we have no regrets"](https://plausible.io/blog/open-source) — the business model reference
2. [EU IPCEI-CIS €1.2 bn](https://digital-strategy.ec.europa.eu/en/policies/ipcei-cis) — the demand floor
3. [BSI C5:2026](https://www.bsi.bund.de/EN/Themen/Cloud-Computing/Compliance-Kriterien/Compliance-Kriterien_node.html) — the procurement gate
4. [EUCS Cloud Scheme](https://www.enisa.europa.eu/topics/csa) — the public-sector standard

**Competitive & anti-patterns:**
5. [Dokploy #3613 license backlash](https://github.com/Dokploy/dokploy/issues/3613) — never introduce source-available
6. [CasaOS abandonment](https://github.com/IceWhaleTech/CasaOS) — never rely on one sponsor
7. [Riley Walz $46k Jmail bill (Vercel)](https://twitter.com/rrrr_walz/status/1788824729600245882) — never surprise on price
8. [Coolify pricing teardown](https://github.com/coollabsio/coolify) — the per-server reference
9. [PIER single-binary deploy](https://github.com/0xPITECH/PIER) — the tech reference

**Tech & engineering:**
10. [axum 0.8 docs](https://docs.rs/axum) — the HTTP server
11. [sqlx 0.8](https://docs.rs/sqlx) — the DB layer
12. [ratatui 0.30](https://ratatui.rs) — the TUI
13. [rqlite 9.0](https://github.com/rqlite/rqlite) — the V2 state backend
14. [OPA / Rego](https://www.openpolicyagent.org/) — the policy engine
15. [GitLab 2017-01-31 postmortem](https://about.gitlab.com/blog/2017/02/10/postmortem-of-database-outage-of-january-31/) — the backup-drill lesson

For the long-form research, the file order to read is: `persona-cto.md` (positioning, funding, moat) → `persona-pm.md` (5-min moment, CLI/TUI UX, pricing) → `persona-principal.md` (architecture, data model, state machines) → `persona-rust.md` (locked stack, type system) → `persona-devops.md` (morning commands, SLOs, alerts, backups) → `persona-platform.md` (IDP philosophy, golden paths) → `persona-crosscutting.md` (governance, USP, sovereignty as engineering).

---

**End of executive summary. Total research corpus: ~154,000 words / ~14,500 lines across 13 files.**
