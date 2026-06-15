# Sovereign Application Runtime — Deep Research Report

**Date:** 2026-06-03
**Product:** Rust-based, self-hosted, single-binary, CLI/TUI/API-first deployment platform for solo developers and startups
**Status:** Strategic research synthesis. Source files: `Research-1.txt` (spec), `Research-Report-1.md` (tech), `sovereign-runtime-market-research.md` (market), `competitive-landscape.md` (competition), `user-pain-research.md` (pain).

---

## 0. Executive Summary

The thesis is **build it, but not what the spec is currently scoped to do at full size.** The 2026 market validates a *positioning* (Rust single-binary, sovereign, exportable, low-resource) more than the *full feature surface* the spec implies. Three findings drive every recommendation in this report:

1. **A real, growing, well-funded market exists.** EU sovereignty + cloud repatriation + self-hosting surge = a credible €5–25M ARR path by year 3. Drivers: IPCEI-CIS €1.2bn public funding, EUCS finalization Feb 2026, BSI C5 procurement demand, 95% YoY growth in 2024 self-hosting survey, 70% of VMware customers considering repatriation. Proven analogs: Plausible ($1M+ ARR), Supabase ($5B Series E Oct 2025), Proton, Nextcloud, Bitwarden.

2. **The "Coolify/Dokploy + web panel + Docker" default is the pain, not the solution.** Real complaints cluster around heavy RAM (Coolify 0.8–1.2 GB, Dokploy 0.6 GB, CapRover 400 MB), the panel as attack surface, stuck deployments, dropped in-flight requests on "zero-downtime" deploys, Docker API breaking changes (CapRover 1.13→1.14 fiasco), and the "WordPress-plugin-maintenance feel" of upgrade churn. Dokploy's mixed (Apache + source-available) license is a quantified reason users pick Coolify instead. CasaOS is effectively abandoned. Even Coolify has 1–2 core maintainers and 781 open issues.

3. **The single-binary Rust niche is unclaimed, validated, and very thin.** Three independent 2025–2026 entrants (PIER 20–40 MB AGPL-3.0, sh0 ~50 MB, yoink 12 stars MIT) all attack the same pain with the same pitch. *None of them own the category yet.* The four-way intersection — **sovereignty + small footprint + declarative GitOps + AI-agent-friendly** — is empty.

**The biggest mistake the spec risks is "build the platform first."** The spec already says this. The research confirms it. The right path:

- **MVP = git push to deploy, atomic, with one-command rollback, auto-TLS, encrypted secrets, and a Postgres backup+verify loop.** ~6 weeks of work. Solves 60% of the severity-weighted pain in the research.
- **V1 adds: TUI dashboard, observability primitives, multi-env, agent pattern for multi-server.** ~3 months.
- **V1.5/V2 adds: rqlite-based state for HA, declarative GitOps, BSI C5 / EUCS Substantial package.** ~6 months, gated on real adoption.
- **V3+ adds: marketplace, IDP, EUCS High.** Year 2+, gated on EU enterprise revenue.

**The single highest-risk decision is the single→multi-server architecture.** Get it wrong and you have to rewrite the control plane. Recommendation: build the **agent pattern (server + N agents) from V1.5**, with state moving to **rqlite** (single-binary HA SQLite + Raft) in V2, not Postgres. Document a path to a true 3-node HA in V3 only if user demand justifies it.

**The technology stack is sound.** All the spec's choices (Rust, axum+SQLite, Caddy, Docker/Podman abstraction, ratatui TUI, age/SOPS secrets, Apache 2.0) are validated by 2026 best practice, with two adjustments: (1) revise the "1 vCPU / 512 MB minimum" claim — the *real* floor is 1 GB when Caddy is included; offer Nginx (10 MB) as a low-memory alternative for 512 MB users; (2) defer real canary to V2; rolling + blue-green cover 95% of the use cases.

---

## 1. Strategic Findings

### 1.1 What 2026 changed

| Trend | Implication |
|---|---|
| Heroku "sustaining engineering" (Feb 6 2026) | Active migration wave. A self-hostable target with first-class data import is the explicit antidote. |
| EUCS finalized (Feb 2026) | Sovereign positioning is no longer "nice to have" — it is a procurement requirement for EU public sector and regulated industry. |
| Dokploy's source-available license shift (Jan 2026) | "Truly open" is a defensible wedge. Apache 2.0 with no carve-outs is a *feature*. |
| CasaOS effectively abandoned (Dec 2024) | Solo founders now screen for sustainability. The 18-month "will this be alive?" question is the first objection. |
| v0/Vercel pricing collapse (May 2025) + Riley Walz's $46,485 Jmail bill (Nov 2025) | "Flat, predictable pricing" is a story the buyer has already told themselves. |
| AI coding agents reading `.env` files (Knostic research, 2026) | A first-class encrypted-secrets story is now urgent, not nice-to-have. |
| rqlite, Litestream, SQLite-with-WAL hit production maturity | Single-binary state is a real 2026 architecture, not a workaround. |
| Hetzner +30–50% price increase (Apr 2026) | VPS economics shifting. Sovereign VPS costs may not stay "obviously cheaper than Vercel" forever. Lock in cost predictability with flat pricing. |

### 1.2 The positioning wedge (one sentence)

> "A Rust single-binary deployment platform with first-class secrets, declarative GitOps, and an EU sovereignty package — runs on a Hetzner box, owns your data, exports cleanly, and survives the vendor disappearing."

### 1.3 The wedge in product terms

| Layer | Spec claim | Validation | Adjust |
|---|---|---|---|
| Static binary 8–30 MB | Yes | Achievable (10–25 MB with musl + LTO + abort + strip). Real projects hit 20–40 MB with full feature set. | Keep 8–30 MB as *target*, communicate 20–30 MB as *typical*. |
| Idle RAM 20–50 MB | Yes (control plane alone) | axum baseline 8–24 MB. Realistic for daemon. | Real "minimum" VPS is 1 GB once you include reverse proxy + Docker daemon. |
| 1 vCPU / 512 MB minimum | Yes (daemon) | Tight for full system (Caddy 30 MB + Docker 140–180 MB = 200 MB just for the system). | Offer **Nginx (10 MB) as a low-memory alternative** for 512 MB. Default to 1 GB. |
| Caddy as primary proxy | Yes | Validated. 2026 default. Auto-TLS is the killer feature. | Add Nginx "low-mem" mode. Defer Traefik to V2. |
| Docker + Podman | Yes | Validated. Podman v5+ Docker-API-compatible. | Document gaps honestly (Swarm, some libnetwork drivers). |
| SQLite + Litestream | Yes | Strongly validated. Rails 8 default, Forward Email, Grafana. | Defer rqlite to V2. Don't write a Postgres migration plan. |
| axum + tokio + serde | Yes | Best Rust web stack for 2026. | Keep. |
| TUI (ratatui) | Yes | Validated. k9s / lazydocker / yoink pattern. | Keep. |
| CLI-first | Yes | 2026 standard (Kamal, Rivet, yoink all do it). | Keep. |
| Apache 2.0 licensing | Implied | Matches every comparable PaaS. | Keep Apache 2.0 with **no feature carve-outs**. |
| BuildKit for builds | "Own the build" | Validated, but real ongoing cost. | Support `--image=...` (CI-built) from V1.1 — don't lock users in. |
| Canary deployments | Spec lists it | Heavy feature. Coolify/Dokploy don't really do it. | **Defer to V2.** Rolling + blue-green cover 95% of real needs. |
| Single binary on a single server | Spec assumes | True for V1. | Plan for **agent pattern** in V1.5 + **rqlite HA** in V2. |

---

## 2. Competitive Landscape Synthesis

Detailed in `competitive-landscape.md` (910 lines, ~9,400 words). Key conclusions:

### 2.1 The market map

| Category | Players | What it is |
|---|---|---|
| Self-hosted PaaS, web panel, Docker | **Coolify** (56.3k ★, Apache 2.0, 800–1200 MB RAM), **Dokploy** (34.4k ★, Apache 2.0 + source-available, ~600 MB), **CapRover** (14.9k ★, ~250–400 MB) | "Self-hosted Heroku/Vercel" — full web UI, Git deploy, DBs, SSL, multi-server |
| Single-server PaaS, terminal-first | **Dokku** (~30k ★, MIT, ~150 MB) | Heroku on a VPS, plugin-based, no GUI |
| Container management UI | **Portainer** CE (~36.9k ★, Zlib, 50–100 MB), Dockge, Yacht | Docker/Swarm/K8s GUI, not really a PaaS |
| Personal cloud / homelab app stores | **YunoHost** (AGPL-3.0, since 2012), **CasaOS** (33.4k ★, **abandoned** → closed ZimaOS), **Umbrel** (~11k ★, **source available not OSS**), **Runtipi** (9.3k ★, GPL-3.0, **RCE in backup 2026**), **Cosmos Cloud** (5.8k ★, AGPL-3.0) | Friendly UI for installing self-hosted consumer apps |
| Zero-trust / tunneling | **Pangolin** (20.8k ★, AGPL-3.0 + commercial <$100K free, YC W25) | Reverse proxy + identity layer; not a PaaS but adjacent |
| **Rust single-binary deploy tools (the new wave)** | **PIER** (20–40 MB, AGPL-3.0), **sh0** (~50 MB, "AI CTO PaaS"), **yoink** (12 ★, MIT, "between Kamal and K8s"), **Tako** (no-Docker), **Komodo** (~7k ★, AGPL-3.0, Core+Periphery) | The category the new product is in. **Empty quadrant.** |
| Build/deploy tools (Rust or similar) | **Kamal 2** (14.2k ★, MIT, Ruby, kamal-proxy in Rust), **Shuttle** (6.9k ★, Apache 2.0, Rust, cloud-only), **Rivet** (5.5k ★, Apache 2.0, Rust, actors) | Deploy tool, not PaaS. Proof the Rust-deploy-tooling space is alive. |

### 2.2 The new product's unclaimed position

The **Rust single-binary PaaS for solo devs, no required web panel, GitOps-declarative, AI-agent-friendly, sovereignty-positioned** cell. No one owns it. PIER is single-binary but app-store-style. yoink is single-binary but explicitly *not* a PaaS. Komodo is multi-server but requires Core+Periphery. Tako is single-binary but no-Docker (anti-fit for the spec).

### 2.3 Top user complaints about the incumbents (synthesized from Reddit, HN, dev.to, Medium, LogRocket, GitHub issues)

- **"Coolify is too heavy."** 0.8–1.2 GB idle, six+ containers. "WordPress-plugin-maintenance feel" (mfyz.com).
- **"Dokploy's license is broken."** Apache 2.0 core + source-available for templates/multi-server/previews. Called out as "not actually open source" on HN and Medium.
- **"Zero-downtime deploys drop in-flight requests."** Both Coolify and Dokploy. The pattern: new container comes up, old container gets SIGTERMed, in-flight requests are killed (Autonoma review).
- **"Stuck deployments" with no cancel path.** Dokploy #4461 (May 2026). Same complaint on HN for Coolify.
- **"Docker API broke CapRover."** Issue #2351, Docker 29 broke CapRover 1.13.x. Multi-month user trust damage.
- **"CasaOS is dead, dev moved to closed ZimaOS."** Last commit Dec 2024, GitHub Discussion #2494 confirms.
- **"Runtipi backup had a critical RCE."** GHSA-vrgf-rcj5-6gv9, 2026.
- **"No GitOps / declarative config."** Coolify, Dokploy, CapRover, Dokku all live in the panel's DB. Issue #3872 (Dokploy), #6000+ family (Coolify).
- **"No built-in DNS manager."** Dokploy #4376 (May 2026).
- **"Migrate from Coolify to Dokploy is impossible."** Dokploy #3098 — maintainer said "unfeasible."
- **"Magic env-var variables that don't work"** — both Coolify and Dokploy.

### 2.4 The four positioning gaps worth taking

1. **"Coolify is too heavy. PIER is too thin. We're the middle."** — ~80–150 MB, single binary, git-push PaaS.
2. **"Truly open source, no resale carve-outs."** — Apache 2.0 with zero feature carve-outs.
3. **"No GUI required, TUI is fine, config in your git repo."** — declarative GitOps, k9s-style TUI, AI-agent-friendly.
4. **"Sovereignty + sustainability + 18-month-viability signal."** — commercial-OSS hybrid from day 1, EUCS Substantial package, public roadmap.

---

## 3. User Pain Synthesis

Detailed in `user-pain-research.md` (523 lines, ~6,500 words). Key conclusions:

### 3.1 Top 20 ranked feature opportunities

| Rank | Pain | Severity | Frequency | Automation potential |
|---|---|---|---|---|
| 1 | **Silent backup failures / untested restores** | 10 | Hits everyone eventually | 10 |
| 2 | **Vercel / Heroku / Datadog bill shock & lock-in** | 10 | Every renewal | 9 |
| 3 | **Env-var drift across local/CI/host** | 8 | Every deploy | 10 |
| 4 | **"I just want to ship, why 7 accounts" (signup gauntlet)** | 9 | Every new project | 9 |
| 5 | **Scary rollbacks / deploys = weekly Russian roulette** | 9 | Every deploy | 10 |
| 6 | **SSL cert expiry (silent) = 3 AM pages** | 9 | Quarterly if not automated | 10 |
| 7 | **Solo dev = build/infra/release/on-call = one person** | 9 | Constant | 9 |
| 8 | **No monitoring = "17 user emails on Monday"** | 8 | First incident | 10 |
| 9 | **Connection pool exhaustion under serverless burst** | 9 | First scale spike (20–30 concurrent) | 9 |
| 10 | **Reverse proxy hell (Nginx vs Caddy vs Traefik)** | 7 | Every new app/domain | 10 |
| 11 | **Docker layer cache invalidation on CI** | 7 | Every CI run | 9 |
| 12 | **Heroku / Salesforce sustaining-engineering / sunset risk** | 9 | Once per platform | 7 |
| 13 | **DigitalOcean / vendor-specific data-extraction lock-in** | 9 (when it hits) | 1% of users | 10 |
| 14 | **Zero-downtime DB migrations (Rails/Django/Prisma lock)** | 8 | Every schema change on hot table | 6 |
| 15 | **Tooling fragmentation: 6–8 tools for one SaaS** | 8 | Constant | 9 |
| 16 | **"Vercel UX, VPS pricing" gap is explicitly stated** | 9 | Per IH founder | 10 |
| 17 | **.env files read by AI coding agents (Claude/Cursor/Copilot)** | 8 | Every AI session | 10 |
| 18 | **Agency: 10+ client sites, 5-hour patch days** | 7 | Every quarter per agency | 9 |
| 19 | **Data residency / CLOUD Act exposure** | 7 (vertical) | For EU customers | 5 |
| 20 | **Preview environments per PR (non-Vercel stack)** | 6 | Every PR | 9 |

### 3.2 The "ship in 90 days" v1

If forced to ship v1 in 90 days, items 1–8 alone address **~70% of the severity-weighted pain.** The five highest-leverage features:

1. **`git push` to deploy with atomic image-tagged releases + one-command rollback** — most-requested feature in the research.
2. **Built-in TLS via ACME with auto-renewal + hot reload** — removes an entire 3 AM page category.
3. **Bundled reverse proxy (Caddy-style) with first-class domain/route config** — eliminate the Nginx-vs-Caddy-vs-Traefik choice.
4. **Encrypted, zero-disk secrets store with rotation + AI-agent safety** — addresses `.env` in git + `.env` read by Claude/Cursor.
5. **Postgres-as-a-first-class-primitive with `pg_dump` + S3 backup + monthly restore drill + alert** — the single highest-severity pain in the research.

### 3.3 The structural "Vercel UX, VPS pricing" gap

The single most direct product/market statement in the entire research is an Indie Hackers post by the founder of Server Compass, "Vercel UX, VPS pricing. That's what I built." Quote:

> "I was running projects across multiple PaaS platforms. Vercel for frontends, Railway for some backend services, Render for others, Supabase for auth, NeonDB for postgres. Each one felt great individually. Then I actually added up what I was paying: ~$200/month across everything. For side projects and small apps. … I knew the math didn't make sense. A $6 Hetzner VPS could run all of this. But every time I thought about migrating, I remembered what VPS management actually felt like: SSH-ing around, managing PM2 in tmux, grep-ing through logs at 2am."

The new product's positioning is: *make the SSH-ing-around-tmux-grepping-logs-at-2am part optional, not the default.*

---

## 4. Market & Sovereignty Findings

Detailed in `sovereign-runtime-market-research.md` (369 lines, ~5,000 words). Verdict: **build it, window is 2026–2028.**

### 4.1 Market sizing

- **TAM:** EU + EU-adjacent public sector and regulated industries (healthcare, finance, defense-adjacent, critical infrastructure). Tens of thousands of orgs.
- **SAM:** Mid-market EU companies (50–5000 employees) needing sovereignty in customer RFPs. Thousands.
- **SOM (3-year capture):** 1,000–5,000 paying customers; €5–25M ARR.

### 4.2 Real, well-funded tailwinds

- **IPCEI-CIS**: €1.2bn public + €1.4bn private. Backed by DE, FR, IT, ES, NL, HU, BE, PL, SI, LV. Deployment through 2026+.
- **EUCS** (Feb 2026): three levels (Basic, Substantial, High). "High" requires EU ownership, HQ, non-EU-jurisdiction immunity.
- **BSI C5** (DE): 121 controls / 17 domains. De facto German government standard.
- **SecNumCloud** (FR): required for "Cloud de Confiance" / "Souveräner Cloud."
- **EU Cloud Sovereignty Framework v1.2.1** (Oct 2025): six sovereignty objectives.
- **r/selfhosted** 750k+ subscribers, **awesome-selfhosted** ~297k stars, **2024 self-host survey** respondents nearly doubled YoY.
- **VMware 2025 repatriation survey** (reported 2026): ~70% of VMware customers considering repatriation; ~35% actively repatriating.
- **Deta Space shutdown** (2024): cautionary tale, drove self-host migration wave.
- **"Bye Vercel" / OpenNext**: active movement.

### 4.3 Real, named threats

1. **Hyperscaler "sovereign" SKUs** (AWS European Sovereign Cloud, Google Sovereign Cloud via T-Systems/S3NS, Microsoft Cloud for Sovereignty). Cannot offer a *self-hosted* escape hatch — the new product's structural moat.
2. **Edge platforms reframing the question** (Cloudflare Workers, Deno Deploy, Vercel Edge). "The edge is the new region" — Cloudflare marketing 2024–2026.
3. **AI agents reducing the need for a deployment runtime at all.** Counter: regulated buyers still need auditable, deterministic deployment.
4. **Kubernetes dominance.** Counter: K8s is the cluster; the new product replaces the 90% of workloads that don't need it.
5. **"Sovereignty as fashion"** — risk EU regulators soften under US diplomatic pressure.
6. **Coolify / Dokploy feature parity in 12–24 months** (especially if they add sovereignty documentation cheaply).

### 4.4 Pricing benchmarks

- **VPS:** Hetzner CX22 €4.49/mo (+30–50% Apr 2026), CPX31 ~€15/mo. OVH, Netcup, Scaleway dominate EU sovereign VPS.
- **Managed PaaS (the displaced buyers):** Vercel Pro $20/seat/mo, Render $19/seat/mo, Railway $5+usage, Fly $5–30, Heroku Eco $5–25.
- **Sovereign enterprise:** Nextcloud Enterprise ~€50–100/user/yr; Bitwarden Enterprise $3–6/user/mo; Element/Matrix custom 6-figure deals.
- **Recommended:** **per-node pricing €20–100/node/mo**, aligned with EU public procurement and on-prem scoping.

### 4.5 Open-core playbook

- **Plausible Analytics:** $1M+ ARR, 50k+ paying sites. Cited playbook.
- **Supabase:** $5B valuation, Series E ~$100M Oct 2025.
- **Sentry:** $3B+ valuation, ~$200M+ ARR.
- **Cal.com, Bitwarden, Nextcloud, Element, Proton, Mastodon** all prove the model.
- **Recommended structure for the new product:**
  1. **Community edition:** Rust binary, MIT or AGPL, core deployment + observability.
  2. **Pro edition:** managed updates, multi-tenant, EU support.
  3. **Enterprise edition:** on-prem, EUCS-Substantial controls, BSI C5 mapping, DPA, 24/7 SLA.

### 4.6 GTM

- **r/selfhosted** (750k subs) — AMA / launch posts hit 500–2000 upvotes and 200+ comments routinely.
- **awesome-selfhosted** (~297k stars) — inclusion in the right category = months of organic traffic.
- **Show HN** — Self-hosted PaaS launches (Coolify, Dokploy, Easypanel) all hit top 20.
- **Open Source Business Alliance** (Germany) — procurement-grade credibility.
- **EU public-sector events:** IT-SA (Nuremberg), Cloud Expo Europe, Paris Open Source Summit.
- **Recommended year-1 GTM cost:** low five figures in events + content + sponsorships.

---

## 5. Technology Validation Summary

Detailed in `Research-Report-1.md` (1,100 lines, ~9,500 words). Verdict: **stack is sound, 2 spec items need adjustment, 1 architecture decision is high-risk.**

### 5.1 Validated (low risk)

- Static binary 8–30 MB achievable (10–25 MB realistic, 20–40 MB with full feature set).
- axum+SQLite memory 20–40 MB idle realistic.
- Caddy 2.10+ as proxy default (30 MB idle, auto-HTTPS, JSON admin API).
- Docker + Podman abstraction (Podman v5+ Docker-API-compatible).
- SQLite + WAL + Litestream as 2026 production stack (Rails 8 default, Forward Email, Grafana).
- age + SOPS for secrets (Vault is BSL 1.1 since 2023; SOPS is the small-team default).
- ratatui for TUI (k9s/lazydocker/yoink pattern; russh for SSH-hosting if needed).
- axum + tokio + serde is the right Rust web stack.
- Apache 2.0 licensing matches every comparable PaaS.

### 5.2 Adjusted (medium risk)

- **"1 vCPU / 512 MB minimum" is tight.** Control plane fits, but Caddy (30 MB) + Docker daemon (140–180 MB) + system overhead push the real minimum to **~1 GB for a working system**. Either commit to **Nginx (10 MB) for low-RAM users** or revise the spec target.
- **BuildKit self-hosting is a real ongoing cost.** V1 should do both `tool deploy api` (built-in) and `tool deploy api --image=...` (CI-built). Don't lock users in.
- **Canary deployments are heavy.** Defer to V2. Rolling + blue-green cover 95% of real needs.

### 5.3 High risk

- **Single → multi-server architecture is THE architecture decision.** Get it wrong and you rewrite the control plane. Recommendation:
  - **V1 (single server):** single binary, SQLite, systemd service. Done.
  - **V1.5 (multi-server, no HA):** add `agent` mode. Server pushes deploy instructions to agents over mTLS. State in server's SQLite. Agents stateless (pull image, run, report).
  - **V2 (multi-server with HA):** move state to **rqlite** (SQLite + Raft, single binary, no external deps). 3+ server nodes now possible.
  - **V3+:** CRDTs for audit logs (eventually consistent), Raft for active state, "stretch cluster" topology.

### 5.4 Direct competitor to study: **yoink**

`https://github.com/oddur/yoink` — "Small, opinionated container deploy CLI + TUI. Drives Docker on remote hosts via SSH; one YAML file describes services, dependencies, networks, secrets, healthchecks. Sits between Kamal and Kubernetes."

This is essentially the same product positioning, in Rust, single binary, TUI-first. The wedge: provide some PaaS features (git-push auto-deploy, one-click databases, S3 backups) that yoink explicitly leaves to CI, **plus** the sovereign / EUCS positioning yoink doesn't address.

---

## 6. Top 200 Operational Actions

Synthesized from `user-pain-research.md` and `competitive-landscape.md`. Categorized by frequency (Daily, Weekly, Monthly, On-demand, Rare). Each rated on:
- **Frequency** (1=rare, 10=many times daily)
- **Pain Level** (1=trivial, 10=causes incident / data loss / 3am page)
- **Automation Potential** (1=must be human, 10=fully automatable)
- **Business Impact** (1=negligible, 10=revenue/customer-impacting)
- **Priority** (computed: Freq × Pain × Auto ÷ Effort)

### 6.1 Deployment (40 actions)

| # | Action | Freq | Pain | Auto | Impact | Priority |
|---|---|---|---|---|---|---|
| 1 | Deploy application (`git push` → live) | 10 | 8 | 10 | 10 | **80** |
| 2 | Rollback a release | 6 | 9 | 10 | 10 | **54** |
| 3 | View application logs | 10 | 6 | 9 | 7 | **42** |
| 4 | Restart a service | 8 | 5 | 10 | 8 | **40** |
| 5 | Stop / start an app | 6 | 4 | 10 | 6 | **24** |
| 6 | Promote a build from staging to production | 4 | 7 | 9 | 9 | **25** |
| 7 | Trigger a canary deployment (5% / 25% / 100%) | 2 | 7 | 6 | 8 | **11** |
| 8 | Trigger a blue-green deployment | 3 | 6 | 8 | 8 | **14** |
| 9 | Health-check validation post-deploy | 10 | 7 | 10 | 9 | **70** |
| 10 | Block a bad deploy before traffic switch (auto-rollback on health fail) | 4 | 9 | 9 | 10 | **36** |
| 11 | Deploy from a specific git ref (tag, SHA, branch) | 6 | 4 | 10 | 7 | **24** |
| 12 | Deploy from a pre-built image (`--image=...`) | 6 | 4 | 10 | 7 | **24** |
| 13 | Build the application (docker build / buildpack) | 8 | 6 | 9 | 7 | **34** |
| 14 | Cache build layers (registry, S3, local) | 8 | 6 | 9 | 7 | **34** |
| 15 | Cancel a stuck deployment | 2 | 9 | 9 | 9 | **16** |
| 16 | View deployment history (last N versions) | 5 | 4 | 10 | 5 | **20** |
| 17 | Diff two deployments (config, env, image) | 3 | 6 | 9 | 7 | **16** |
| 18 | Replay a deployment (re-run last good config) | 2 | 7 | 9 | 8 | **11** |
| 19 | Pin a deployment (lock a specific image/version) | 3 | 5 | 10 | 7 | **15** |
| 20 | Schedule a deployment (cron, future time) | 2 | 4 | 9 | 5 | **7** |
| 21 | Deploy a multi-container app (Compose) | 5 | 6 | 8 | 7 | **21** |
| 22 | Deploy a static site (no runtime) | 4 | 3 | 10 | 5 | **12** |
| 23 | Deploy a worker / cron (no HTTP) | 4 | 5 | 8 | 6 | **13** |
| 24 | Deploy a one-off job / migration | 5 | 5 | 8 | 6 | **15** |
| 25 | Set deployment strategy (recreate, rolling, blue-green) | 3 | 5 | 9 | 6 | **11** |
| 26 | Configure deployment retries on failure | 2 | 5 | 9 | 6 | **7** |
| 27 | Configure deploy timeout | 2 | 4 | 10 | 5 | **8** |
| 28 | Configure graceful shutdown (preStop, drain) | 3 | 6 | 9 | 7 | **14** |
| 29 | Notify deploy success/failure (Slack, email, webhook) | 6 | 3 | 10 | 5 | **18** |
| 30 | Approve a production deploy (manual gate) | 3 | 5 | 9 | 8 | **12** |
| 31 | Re-deploy a previous version (atomic) | 4 | 7 | 10 | 9 | **28** |
| 32 | View in-flight deploys (live status) | 4 | 5 | 9 | 6 | **14** |
| 33 | Tag a release for audit (`v1.4.2` with notes) | 4 | 3 | 10 | 4 | **12** |
| 34 | Roll forward (re-deploy a different version) | 4 | 6 | 10 | 8 | **24** |
| 35 | Configure zero-downtime healthcheck gate | 5 | 7 | 9 | 9 | **32** |
| 36 | Deploy to a specific server / region | 3 | 5 | 8 | 6 | **10** |
| 37 | Deploy a sidecar (e.g., log shipper) | 2 | 5 | 7 | 5 | **6** |
| 38 | Skip a deploy (mark version as bad, never auto-roll-back to it) | 2 | 6 | 9 | 7 | **9** |
| 39 | View deploy logs (per-stage: build, push, swap) | 5 | 5 | 9 | 6 | **18** |
| 40 | Compare env vars between deployments | 3 | 6 | 9 | 7 | **16** |

### 6.2 Operations / runtime (30 actions)

| # | Action | Freq | Pain | Auto | Impact | Priority |
|---|---|---|---|---|---|---|
| 41 | Restart a stuck container | 5 | 6 | 10 | 8 | **30** |
| 42 | View live resource usage (CPU, RAM, disk) per app | 8 | 4 | 10 | 6 | **32** |
| 43 | Set resource limits (CPU/RAM quotas) per app | 3 | 5 | 9 | 7 | **14** |
| 44 | Tail logs with grep / filter | 10 | 5 | 9 | 7 | **45** |
| 45 | Search logs across all apps / time range | 5 | 7 | 8 | 8 | **28** |
| 46 | Export logs to S3 / external | 2 | 4 | 9 | 5 | **7** |
| 47 | Rotate logs (size, time, retention) | 4 | 4 | 10 | 5 | **16** |
| 48 | View process list / open file descriptors | 3 | 4 | 9 | 5 | **11** |
| 49 | Exec into a running container (debug) | 5 | 5 | 9 | 7 | **23** |
| 50 | Capture coredump / heap dump on crash | 1 | 6 | 8 | 7 | **5** |
| 51 | Configure liveness probe | 3 | 5 | 9 | 8 | **14** |
| 52 | Configure readiness probe | 3 | 5 | 9 | 8 | **14** |
| 53 | Configure startup probe (slow first boot) | 2 | 5 | 9 | 7 | **9** |
| 54 | Set graceful shutdown timeout (drain duration) | 2 | 6 | 9 | 8 | **11** |
| 55 | Trigger an ops alert manually (test) | 1 | 4 | 10 | 5 | **4** |
| 56 | Acknowledge an alert (silence for N minutes) | 5 | 4 | 9 | 6 | **18** |
| 57 | Mute alerts during maintenance window | 2 | 5 | 9 | 6 | **9** |
| 58 | View app uptime (last 30/90 days) | 4 | 4 | 10 | 6 | **16** |
| 59 | View app response time (p50/p95/p99) | 5 | 4 | 10 | 6 | **20** |
| 60 | View app error rate (5xx ratio) | 5 | 5 | 10 | 8 | **25** |
| 61 | Configure healthcheck endpoint | 5 | 5 | 10 | 9 | **25** |
| 62 | Recover from OOM (auto-restart, alert) | 3 | 8 | 9 | 9 | **22** |
| 63 | Recover from disk full (auto-cleanup, alert) | 2 | 8 | 8 | 9 | **14** |
| 64 | Recover from certificate expiry (auto-renew) | 1 | 9 | 10 | 10 | **10** |
| 65 | Force garbage collection / memory release | 1 | 4 | 8 | 4 | **3** |
| 66 | Set timezone / locale for logs | 2 | 3 | 10 | 4 | **6** |
| 67 | Configure log format (JSON, plain, structured) | 3 | 4 | 9 | 5 | **11** |
| 68 | Filter logs by severity | 6 | 4 | 9 | 6 | **22** |
| 69 | Forward logs to external (Datadog, Better Stack) | 3 | 5 | 8 | 6 | **12** |
| 70 | Receive crash notification (on OOM, exit code) | 3 | 7 | 9 | 8 | **19** |

### 6.3 Networking (30 actions)

| # | Action | Freq | Pain | Auto | Impact | Priority |
|---|---|---|---|---|---|---|
| 71 | Add a domain to an app | 5 | 5 | 10 | 7 | **25** |
| 72 | Remove a domain from an app | 2 | 3 | 10 | 5 | **6** |
| 73 | Issue Let's Encrypt cert (HTTP-01) | 5 | 6 | 10 | 9 | **30** |
| 74 | Issue Let's Encrypt cert (DNS-01) | 2 | 7 | 8 | 8 | **11** |
| 75 | Auto-renew a certificate | 4 | 9 | 10 | 10 | **36** |
| 76 | Add a wildcard cert (`*.example.com`) | 2 | 6 | 9 | 7 | **11** |
| 77 | Force HTTPS redirect | 4 | 4 | 10 | 6 | **16** |
| 78 | Configure HSTS | 2 | 4 | 9 | 5 | **7** |
| 79 | Configure custom headers (CORS, CSP, etc.) | 3 | 5 | 9 | 6 | **14** |
| 80 | Configure rate limiting per route | 2 | 5 | 8 | 7 | **10** |
| 81 | Configure request size limits | 2 | 4 | 9 | 6 | **7** |
| 82 | Add a redirect (e.g., `www` → apex) | 3 | 4 | 9 | 5 | **11** |
| 83 | Add a reverse-proxy route (`/api` → backend) | 4 | 5 | 9 | 7 | **18** |
| 84 | Configure websocket upgrade | 3 | 5 | 9 | 6 | **14** |
| 85 | Configure HTTP/2 / HTTP/3 | 2 | 3 | 9 | 4 | **5** |
| 86 | Configure gzip / brotli compression | 2 | 3 | 9 | 4 | **5** |
| 87 | Add a CDN (Cloudflare, BunnyCDN) in front | 3 | 5 | 7 | 7 | **11** |
| 88 | Configure DNS-01 challenge with provider API | 2 | 7 | 7 | 7 | **10** |
| 89 | Set up internal service-to-service networking | 3 | 6 | 7 | 7 | **13** |
| 90 | Add a service-discovery alias (e.g., `postgres.internal`) | 3 | 5 | 8 | 6 | **12** |
| 91 | Configure firewall rules (open/close port) | 2 | 5 | 8 | 7 | **10** |
| 92 | Configure TCP/UDP forwarding (non-HTTP) | 2 | 5 | 7 | 6 | **7** |
| 93 | Configure mTLS between services | 1 | 7 | 6 | 8 | **6** |
| 94 | Whitelist IP range for an admin route | 2 | 5 | 9 | 7 | **10** |
| 95 | Set up a load balancer between replicas | 3 | 6 | 8 | 8 | **14** |
| 96 | Configure sticky sessions / cookie affinity | 1 | 5 | 8 | 6 | **4** |
| 97 | View active connections / traffic patterns | 4 | 4 | 9 | 5 | **14** |
| 98 | Block abusive IP (fail2ban-style) | 2 | 5 | 8 | 6 | **8** |
| 99 | Configure ACME account (different CA) | 1 | 6 | 8 | 5 | **4** |
| 100 | Switch from HTTP-01 to DNS-01 challenge | 1 | 6 | 8 | 5 | **4** |

### 6.4 Secrets (15 actions)

| # | Action | Freq | Pain | Auto | Impact | Priority |
|---|---|---|---|---|---|---|
| 101 | Set a secret (env var, encrypted at rest) | 8 | 6 | 10 | 8 | **48** |
| 102 | Get a secret (decrypt on demand) | 8 | 4 | 10 | 7 | **32** |
| 103 | Rotate a secret (generate new, re-deploy) | 2 | 7 | 8 | 9 | **11** |
| 104 | Audit secret access (who read what when) | 1 | 6 | 9 | 7 | **5** |
| 105 | Bulk import secrets from `.env` file | 3 | 6 | 9 | 6 | **14** |
| 106 | Bulk export secrets to encrypted file | 1 | 5 | 9 | 5 | **4** |
| 107 | Share secrets with team member (age recipient) | 2 | 6 | 8 | 7 | **9** |
| 108 | Inject secrets at process start (zero-disk) | 8 | 7 | 10 | 9 | **56** |
| 109 | Mask secret in logs (auto-redact) | 6 | 6 | 9 | 8 | **36** |
| 110 | Encrypt secret at rest (envelope encryption) | 8 | 6 | 10 | 9 | **48** |
| 111 | Back up secret store (encrypted) | 1 | 7 | 9 | 9 | **6** |
| 112 | Restore secret store from backup | 1 | 7 | 9 | 9 | **6** |
| 113 | Use hardware key (YubiKey, TPM) for master key | 1 | 7 | 7 | 9 | **5** |
| 114 | Detect leaked secret in git history | 1 | 8 | 8 | 9 | **6** |
| 115 | Sync secrets with external manager (Doppler, Infisical) | 2 | 6 | 7 | 6 | **7** |

### 6.5 Storage (10 actions)

| # | Action | Freq | Pain | Auto | Impact | Priority |
|---|---|---|---|---|---|---|
| 116 | Create a persistent volume for an app | 5 | 4 | 9 | 7 | **18** |
| 117 | Resize a volume | 1 | 6 | 8 | 7 | **5** |
| 118 | Snapshot a volume | 2 | 5 | 8 | 7 | **7** |
| 119 | Restore from snapshot | 1 | 7 | 8 | 9 | **6** |
| 120 | Migrate volume to new host | 1 | 7 | 7 | 8 | **5** |
| 121 | Mount an external volume (NFS, S3, CIFS) | 1 | 6 | 7 | 6 | **4** |
| 122 | Inspect volume usage (size, inode count) | 3 | 4 | 9 | 5 | **11** |
| 123 | Clean unused volumes | 1 | 4 | 9 | 4 | **4** |
| 124 | Encrypt a volume at rest | 2 | 5 | 8 | 7 | **7** |
| 125 | Backup a volume to S3 / external | 2 | 5 | 9 | 8 | **9** |

### 6.6 Database (20 actions)

| # | Action | Freq | Pain | Auto | Impact | Priority |
|---|---|---|---|---|---|---|
| 126 | Provision a Postgres database (one-click) | 5 | 5 | 10 | 8 | **25** |
| 127 | Provision a MySQL / MariaDB database | 2 | 5 | 10 | 7 | **10** |
| 128 | Provision a Redis instance | 2 | 5 | 10 | 7 | **10** |
| 129 | Provision a SQLite database (per app) | 4 | 3 | 10 | 6 | **12** |
| 130 | Backup a database (`pg_dump`, `mysqldump`) | 8 | 6 | 10 | 10 | **48** |
| 131 | Verify a backup (auto-restore to scratch, row count) | 2 | 9 | 9 | 10 | **16** |
| 132 | Restore a database from backup | 1 | 8 | 9 | 10 | **8** |
| 133 | Schedule a recurring backup (daily/weekly) | 4 | 5 | 10 | 9 | **20** |
| 134 | Run a migration (auto on deploy) | 6 | 5 | 9 | 8 | **27** |
| 135 | Open a one-off psql shell to a database | 4 | 4 | 9 | 6 | **14** |
| 136 | Clone a database (e.g., prod → staging) | 2 | 6 | 8 | 8 | **10** |
| 137 | Configure connection pooling (PgBouncer) | 2 | 7 | 8 | 9 | **11** |
| 138 | View slow queries | 3 | 5 | 8 | 7 | **11** |
| 139 | Set up replication (read replica) | 1 | 8 | 6 | 8 | **5** |
| 140 | Run a point-in-time recovery (PITR) | 1 | 9 | 6 | 10 | **5** |
| 141 | Set up auto-failover | 1 | 9 | 6 | 9 | **5** |
| 142 | Migrate to a new Postgres major version | 1 | 8 | 7 | 9 | **6** |
| 143 | Monitor database size growth | 2 | 4 | 9 | 5 | **7** |
| 144 | Detect and alert on long-running queries | 2 | 6 | 9 | 7 | **11** |
| 145 | Drop a database (with confirmation) | 1 | 7 | 8 | 9 | **6** |

### 6.7 Logging (10 actions)

| # | Action | Freq | Pain | Auto | Impact | Priority |
|---|---|---|---|---|---|---|
| 146 | Stream live logs to terminal | 10 | 4 | 10 | 6 | **40** |
| 147 | Search logs by keyword | 8 | 5 | 9 | 7 | **32** |
| 148 | Filter logs by severity (info, warn, error) | 7 | 4 | 9 | 6 | **25** |
| 149 | Filter logs by time range | 5 | 4 | 9 | 6 | **18** |
| 150 | Save log search as a saved view | 3 | 3 | 9 | 5 | **9** |
| 151 | Aggregate logs across multiple apps | 4 | 6 | 8 | 7 | **17** |
| 152 | Tail logs from past N hours / days | 5 | 5 | 8 | 7 | **20** |
| 153 | Download logs (CSV, JSON) | 2 | 4 | 9 | 5 | **7** |
| 154 | Set log retention (auto-delete after N days) | 3 | 4 | 10 | 5 | **12** |
| 155 | Compress old logs (auto) | 2 | 3 | 10 | 4 | **6** |

### 6.8 Monitoring (10 actions)

| # | Action | Freq | Pain | Auto | Impact | Priority |
|---|---|---|---|---|---|---|
| 156 | View CPU usage (host + per app) | 8 | 4 | 10 | 6 | **32** |
| 157 | View RAM usage (host + per app) | 8 | 4 | 10 | 6 | **32** |
| 158 | View disk usage (per volume) | 6 | 4 | 10 | 6 | **24** |
| 159 | View network I/O (in/out per app) | 4 | 4 | 9 | 5 | **14** |
| 160 | View process count / open FDs | 3 | 4 | 9 | 5 | **11** |
| 161 | View container count / state | 5 | 3 | 10 | 4 | **15** |
| 162 | Set up uptime check (external probe) | 4 | 6 | 9 | 9 | **22** |
| 163 | Configure app response time probe | 3 | 5 | 9 | 7 | **14** |
| 164 | View uptime history (last 30/90 days) | 4 | 4 | 10 | 6 | **16** |
| 165 | Set up external status page (auto-published) | 2 | 5 | 8 | 7 | **8** |

### 6.9 Alerting (10 actions)

| # | Action | Freq | Pain | Auto | Impact | Priority |
|---|---|---|---|---|---|---|
| 166 | Configure CPU > 90% alert | 3 | 5 | 10 | 7 | **15** |
| 167 | Configure RAM > 90% alert | 3 | 5 | 10 | 7 | **15** |
| 168 | Configure disk > 90% alert | 3 | 6 | 10 | 9 | **18** |
| 169 | Configure error rate > 5% alert | 3 | 7 | 9 | 9 | **19** |
| 170 | Configure cert expiry < 14d alert | 2 | 7 | 10 | 9 | **14** |
| 171 | Configure backup failure alert | 2 | 8 | 10 | 10 | **16** |
| 172 | Configure health-check failure alert | 4 | 7 | 10 | 9 | **28** |
| 173 | Send alert to email | 3 | 4 | 10 | 6 | **12** |
| 174 | Send alert to Telegram | 2 | 4 | 10 | 6 | **8** |
| 175 | Send alert to Slack / Discord / webhook | 3 | 4 | 10 | 6 | **12** |

### 6.10 Governance (15 actions)

| # | Action | Freq | Pain | Auto | Impact | Priority |
|---|---|---|---|---|---|---|
| 176 | View audit log (who did what when) | 3 | 5 | 10 | 7 | **15** |
| 177 | Filter audit log by user / action / app | 2 | 5 | 9 | 6 | **9** |
| 178 | Export audit log (CSV, JSON) | 1 | 4 | 9 | 5 | **4** |
| 179 | Add a team member | 2 | 4 | 9 | 6 | **8** |
| 180 | Assign a role to a member (owner/admin/dev/read) | 2 | 4 | 9 | 6 | **8** |
| 181 | Revoke a team member's access | 1 | 5 | 9 | 7 | **5** |
| 182 | Require approval for production deploy | 2 | 5 | 8 | 8 | **8** |
| 183 | Require 2-of-N approvers for prod | 1 | 5 | 6 | 8 | **3** |
| 184 | Lock production environment during incident | 1 | 6 | 9 | 9 | **5** |
| 185 | Tag a deploy as `breaking-change` (extra caution) | 1 | 5 | 9 | 7 | **4** |
| 186 | View who has access to what (access review) | 1 | 5 | 8 | 6 | **3** |
| 187 | Configure session timeout | 1 | 4 | 9 | 5 | **3** |
| 188 | Enable MFA for a user | 1 | 4 | 9 | 7 | **3** |
| 189 | Generate a compliance report (deploys, rollbacks, secret changes) | 1 | 5 | 8 | 6 | **3** |
| 190 | Sign an artifact (cosign / Sigstore) for SBOM | 1 | 6 | 6 | 6 | **3** |

### 6.11 Sovereignty & migration (10 actions)

| # | Action | Freq | Pain | Auto | Impact | Priority |
|---|---|---|---|---|---|---|
| 191 | Export entire app (config, secrets, domains, history) | 1 | 7 | 9 | 9 | **6** |
| 192 | Export platform (all apps, secrets, users, audit) | 1 | 8 | 9 | 10 | **7** |
| 193 | Import from a Coolify / Dokploy / CapRover backup | 1 | 8 | 7 | 9 | **5** |
| 194 | Migrate a running app from one server to another (zero downtime) | 1 | 9 | 6 | 10 | **5** |
| 195 | Run the platform air-gapped (no internet) | 1 | 8 | 6 | 9 | **5** |
| 196 | Verify a backup is restorable (auto-restore to scratch + check) | 2 | 9 | 9 | 10 | **16** |
| 197 | List which data lives where (data inventory) | 1 | 5 | 8 | 6 | **3** |
| 198 | Encrypt data at rest (volume / DB) with a customer-managed key | 1 | 6 | 7 | 7 | **4** |
| 199 | Disable outbound telemetry (run offline-mode strict) | 1 | 4 | 10 | 5 | **4** |
| 200 | Vendor-disappear test: run the binary on a fresh box, see if it works | 1 | 7 | 9 | 10 | **6** |

### 6.12 Summary of Top 10 by Priority Score

1. **`git push` deploy** (80) — daily, painful, fully automatable, high impact
2. **Health-check validation post-deploy** (70) — every deploy, prevents incidents
3. **Inject secrets at process start (zero-disk)** (56) — AI-agent safety, .env in git
4. **Rollback a release** (54) — every bad deploy
5. **Encrypt secret at rest** (48) — baseline security
6. **Backup a database** (48) — every day, "silent failures" is the #1 pain
7. **Set a secret (env var, encrypted)** (48) — every deploy
8. **Tail logs with grep** (45) — every incident
9. **Stream live logs to terminal** (40) — every incident
10. **Restart a service** (40) — every bad deploy

**Items 1, 5, 6, 9 alone represent the bare minimum for a viable 2026 self-hosted PaaS.** The MVP can ship with this and nothing else and still be useful.

---

## 7. Top 100 Features Ranked + Scoring

Each feature scored on:
- **Frequency** (1-10): how often users hit it
- **Time Saved** (1-10): per-use minutes saved
- **Reliability Impact** (1-10): incidents prevented / caused
- **Scaling Value** (1-10): value increases with infra size
- **Complexity Cost** (1-10): build + maintain cost

**Final Score = (Frequency + Time Saved + Reliability + Scaling) − Complexity**

### 7.1 P0 — Mandatory (Score ≥ 25, weekly+ use by most users)

| # | Feature | Freq | Time | Rel | Scale | Cplx | Score | Class |
|---|---|---|---|---|---|---|---|---|
| 1 | `git push` deploy with atomic release | 10 | 10 | 9 | 8 | 6 | **31** | P0 |
| 2 | One-command rollback (image-tagged history) | 8 | 9 | 10 | 7 | 5 | **29** | P0 |
| 3 | Auto-TLS via ACME (Let's Encrypt) with auto-renewal | 9 | 9 | 10 | 7 | 5 | **30** | P0 |
| 4 | Single static binary (musl + LTO + abort) | 10 | 8 | 8 | 8 | 4 | **30** | P0 |
| 5 | SQLite (WAL) for control plane state | 10 | 7 | 9 | 6 | 3 | **29** | P0 |
| 6 | CLI-first interface (deploy, rollback, logs, status) | 10 | 9 | 7 | 7 | 4 | **29** | P0 |
| 7 | Health-check gated deploy (block traffic switch on fail) | 9 | 7 | 10 | 7 | 5 | **28** | P0 |
| 8 | Encrypted secret store (age + envelope encryption) | 9 | 8 | 9 | 7 | 5 | **28** | P0 |
| 9 | TUI dashboard (k9s-style status + logs + deploy) | 8 | 9 | 6 | 6 | 5 | **24** | P0 |
| 10 | Database backup (pg_dump / mysqldump) to local+S3 | 8 | 8 | 10 | 7 | 5 | **28** | P0 |
| 11 | Database restore from backup | 3 | 7 | 10 | 6 | 4 | **22** | P0 |
| 12 | Live log streaming + filter + grep | 10 | 8 | 6 | 6 | 4 | **26** | P0 |
| 13 | Build image from Dockerfile (in-product BuildKit) | 8 | 7 | 6 | 6 | 7 | **20** | P0 |
| 14 | Support `--image=...` for pre-built images | 7 | 7 | 6 | 6 | 3 | **23** | P0 |
| 15 | API-first (every CLI command has an HTTP equivalent) | 8 | 7 | 7 | 8 | 4 | **26** | P0 |
| 16 | Custom domain mapping with auto-TLS | 8 | 8 | 8 | 6 | 4 | **26** | P0 |
| 17 | Restart / stop / start service | 9 | 7 | 6 | 5 | 3 | **24** | P0 |
| 18 | Encrypted local state (SQLite + age) | 8 | 6 | 9 | 6 | 4 | **25** | P0 |
| 19 | Zero-downtime deploy (blue-green by default) | 8 | 7 | 9 | 7 | 6 | **25** | P0 |
| 20 | Deployment history (last N versions, immutable) | 7 | 6 | 8 | 6 | 4 | **23** | P0 |
| 21 | Graceful shutdown (drain in-flight requests) | 6 | 6 | 9 | 6 | 5 | **22** | P0 |
| 22 | Health check (HTTP path, interval, timeout) | 8 | 7 | 9 | 6 | 4 | **26** | P0 |
| 23 | Multi-app on single server (resource isolation) | 9 | 6 | 7 | 6 | 5 | **23** | P0 |
| 24 | Docker runtime support | 9 | 7 | 7 | 7 | 5 | **25** | P0 |
| 25 | Caddy reverse proxy (auto-HTTPS, JSON admin API) | 9 | 8 | 8 | 6 | 5 | **26** | P0 |
| 26 | Backup verification (auto-restore to scratch + check) | 4 | 8 | 10 | 6 | 6 | **22** | P0 |
| 27 | Systemd integration (run as a unit) | 8 | 6 | 7 | 6 | 3 | **24** | P0 |
| 28 | First-class Linux native (Debian/Ubuntu/RHEL) | 9 | 6 | 7 | 7 | 3 | **26** | P0 |
| 29 | Resource limits (CPU, RAM) per app | 6 | 6 | 8 | 6 | 4 | **22** | P0 |
| 30 | JSON-structured logs (auto-emitted) | 8 | 6 | 7 | 6 | 3 | **24** | P0 |

### 7.2 P1 — High Value (Score 18–24, used regularly by adopters)

| # | Feature | Freq | Time | Rel | Scale | Cplx | Score | Class |
|---|---|---|---|---|---|---|---|---|
| 31 | Multi-environment (dev/staging/prod) in one config | 6 | 8 | 7 | 7 | 6 | **22** | P1 |
| 32 | Preview environments per PR (with TTL) | 5 | 8 | 6 | 6 | 7 | **18** | P1 |
| 33 | Team access (roles: owner, admin, dev, read) | 5 | 7 | 7 | 7 | 5 | **19** | P1 |
| 34 | Audit log (every action → event) | 6 | 6 | 8 | 7 | 5 | **20** | P1 |
| 35 | Postgres one-click provision (in-app) | 6 | 8 | 7 | 5 | 5 | **21** | P1 |
| 36 | Postgres connection pooling (PgBouncer) | 4 | 7 | 9 | 6 | 5 | **21** | P1 |
| 37 | MySQL / MariaDB one-click provision | 4 | 7 | 7 | 5 | 5 | **18** | P1 |
| 38 | Redis one-click provision | 4 | 7 | 7 | 5 | 5 | **18** | P1 |
| 39 | SQLite per-app (lightweight default) | 5 | 6 | 6 | 4 | 3 | **18** | P1 |
| 40 | Volume persistence (named volumes) | 7 | 5 | 7 | 5 | 3 | **21** | P1 |
| 41 | Scheduled backup (cron: daily/weekly/monthly) | 6 | 7 | 8 | 5 | 4 | **22** | P1 |
| 42 | S3-compatible backup target (R2, B2, MinIO) | 6 | 7 | 8 | 6 | 5 | **22** | P1 |
| 43 | Webhook on deploy success/failure | 6 | 6 | 7 | 6 | 3 | **22** | P1 |
| 44 | Email alert on critical events | 5 | 5 | 8 | 6 | 4 | **20** | P1 |
| 45 | Telegram alert | 4 | 5 | 7 | 5 | 3 | **18** | P1 |
| 46 | Slack / Discord alert | 4 | 5 | 7 | 5 | 3 | **18** | P1 |
| 47 | Uptime monitoring (external probe) | 5 | 7 | 9 | 6 | 6 | **21** | P1 |
| 48 | Status page (auto-generated) | 3 | 7 | 7 | 5 | 5 | **17** | P1 |
| 49 | Auto-rollback on health-check fail | 4 | 8 | 10 | 6 | 6 | **22** | P1 |
| 50 | Podman runtime support (Docker-API-compatible) | 5 | 6 | 7 | 6 | 5 | **19** | P1 |
| 51 | Nginx alternative (low-memory mode, 10 MB) | 3 | 5 | 6 | 5 | 5 | **14** | P1 |
| 52 | Wildcard TLS (`*.example.com`) | 4 | 6 | 7 | 5 | 5 | **17** | P1 |
| 53 | DNS-01 ACME challenge (Cloudflare, Route53) | 4 | 6 | 7 | 5 | 6 | **16** | P1 |
| 54 | Custom headers (CORS, CSP) | 5 | 5 | 6 | 5 | 3 | **18** | P1 |
| 55 | WebSocket support (auto-upgrade) | 5 | 5 | 6 | 5 | 3 | **18** | P1 |
| 56 | HTTP→HTTPS redirect (default) | 6 | 5 | 6 | 5 | 2 | **20** | P1 |
| 57 | Force HTTPS / HSTS | 4 | 4 | 7 | 5 | 3 | **17** | P1 |
| 58 | Secret rotation (generate, redeploy, revoke) | 3 | 7 | 8 | 6 | 6 | **18** | P1 |
| 59 | Secret share (age recipient list) | 3 | 6 | 7 | 5 | 5 | **16** | P1 |
| 60 | YubiKey / TPM support for master key | 2 | 6 | 8 | 5 | 7 | **14** | P1 |
| 61 | Mask secrets in logs (auto-redact) | 6 | 6 | 7 | 6 | 4 | **21** | P1 |
| 62 | Bulk import secrets from `.env` | 4 | 7 | 6 | 4 | 4 | **17** | P1 |
| 63 | OpenTelemetry / Prometheus `/metrics` endpoint | 5 | 6 | 7 | 7 | 4 | **21** | P1 |
| 64 | Structured log format (JSON) | 7 | 5 | 7 | 6 | 3 | **22** | P1 |
| 65 | Log retention (auto-delete after N days) | 5 | 5 | 6 | 5 | 3 | **17** | P1 |
| 66 | Log search across all apps | 5 | 7 | 6 | 5 | 6 | **17** | P1 |
| 67 | Resource quota (CPU/RAM cap per app) | 4 | 5 | 7 | 6 | 4 | **18** | P1 |
| 68 | Network metrics (in/out per app) | 4 | 4 | 5 | 5 | 3 | **15** | P1 |
| 69 | Disk usage per volume (with alerts) | 5 | 4 | 6 | 5 | 3 | **17** | P1 |
| 70 | Backup retention policy (auto-purge) | 4 | 5 | 6 | 5 | 3 | **17** | P1 |
| 71 | TLS for control plane API (self-signed → Let's Encrypt) | 6 | 5 | 8 | 6 | 3 | **22** | P1 |
| 72 | Config file (`app.yaml`) for declarative deploy | 6 | 7 | 7 | 7 | 5 | **22** | P1 |
| 73 | GitOps mode (config in repo, reconcile) | 4 | 8 | 8 | 8 | 7 | **21** | P1 |
| 74 | Multi-server (agent + server pattern) | 3 | 7 | 8 | 9 | 8 | **19** | P1 |
| 75 | SSH-driven deploy (kamal-style) | 5 | 6 | 7 | 7 | 4 | **21** | P1 |
| 76 | Volume snapshot | 3 | 5 | 7 | 5 | 5 | **15** | P1 |
| 77 | Volume restore from snapshot | 1 | 6 | 8 | 5 | 5 | **15** | P1 |
| 78 | Migrations runner (auto-detect, with lock-timeout) | 5 | 5 | 7 | 5 | 5 | **17** | P1 |
| 79 | Database shell (`tool db shell <app>`) | 4 | 6 | 5 | 4 | 3 | **16** | P1 |
| 80 | Database clone (prod → staging) | 2 | 6 | 6 | 5 | 6 | **13** | P1 |

### 7.3 P2 — Growth (Score 12–17, useful after adoption)

| # | Feature | Freq | Time | Rel | Scale | Cplx | Score | Class |
|---|---|---|---|---|---|---|---|---|
| 81 | Templates (`tool init fastapi`, `tool init nextjs`) | 5 | 8 | 5 | 5 | 5 | **18** | P2 |
| 82 | Marketplace (1-click: Plausible, n8n, Ghost, etc.) | 3 | 7 | 5 | 5 | 7 | **13** | P2 |
| 83 | rqlite-based HA control plane (3-server) | 1 | 6 | 9 | 8 | 8 | **16** | P2 |
| 84 | True canary deployments (5/25/100% traffic split) | 2 | 6 | 7 | 7 | 8 | **14** | P2 |
| 85 | Multi-region failover | 1 | 6 | 9 | 8 | 9 | **15** | P2 |
| 86 | Notification routing (per-app, per-team) | 4 | 5 | 6 | 5 | 4 | **16** | P2 |
| 87 | Approval flows for production deploys | 2 | 5 | 7 | 6 | 6 | **14** | P2 |
| 88 | SSO / OIDC integration (Authentik, Keycloak) | 2 | 5 | 7 | 6 | 6 | **14** | P2 |
| 89 | MFA (TOTP) | 3 | 4 | 7 | 5 | 4 | **15** | P2 |
| 90 | SBOM / cosign signing of deployed images | 1 | 4 | 6 | 5 | 6 | **10** | P2 |
| 91 | EUCS Substantial-aligned controls checklist | 1 | 5 | 7 | 7 | 6 | **14** | P2 |
| 92 | BSI C5 mapping (controls document) | 1 | 5 | 7 | 7 | 6 | **14** | P2 |
| 93 | Air-gapped install (offline binary + offline docs) | 1 | 6 | 7 | 6 | 7 | **13** | P2 |
| 94 | Cron jobs / workers (no HTTP, scheduled) | 4 | 5 | 5 | 4 | 5 | **13** | P2 |
| 95 | Background workers (long-running) | 4 | 5 | 5 | 4 | 5 | **13** | P2 |
| 96 | Service catalog (apps, DBs, workers, queues) | 3 | 5 | 5 | 5 | 5 | **13** | P2 |
| 97 | Internal Developer Platform (golden paths) | 1 | 5 | 5 | 7 | 7 | **11** | P2 |
| 98 | Migrate-from-Coolify / -Dokploy import | 1 | 6 | 6 | 5 | 7 | **11** | P2 |
| 99 | Export platform (full backup, restorable elsewhere) | 1 | 7 | 8 | 6 | 6 | **16** | P2 |
| 100 | Disaster recovery (`tool recover`) | 1 | 7 | 9 | 6 | 6 | **17** | P2 |

### 7.4 P3 — Enterprise (Defer past V2, score < 12 OR not justifiable for solo/startup)

These features are explicitly **not to build initially** per the spec's own "What I Would NOT Build" section. Listed for completeness:

- Kubernetes runtime support
- Service mesh
- Multi-cloud abstraction
- AI Copilot / natural language deploys (Coolify has MCP, but it's a feature, not core)
- Terraform / Pulumi replacement
- Complex RBAC (5+ roles, custom permissions)
- Custom container runtime
- Workflow engine (Airflow-like)
- Plugin marketplace with 3rd-party code execution
- OPA / Rego policy engine
- SOC2 reporting automation
- Audit log export to SIEM (Splunk, Datadog)
- VPA / HPA / cluster autoscaler
- Network policies (Calico-style)
- Compliance dashboards (HIPAA, PCI templates)
- Approval workflows with 2-of-N approvers
- OIDC provider (being the IdP, not just consumer)
- Multi-tenant governance

### 7.5 Reject (do not build — feature theatre, low value, or trend-driven)

- **Service mesh** (Istio, Linkerd) — wrong target audience.
- **Multi-cloud abstraction** — spec explicitly says cloud-agnostic VPS, not multi-cloud.
- **Terraform / Pulumi replacement** — single-server PaaS, not IaC.
- **Workflow engine** (Airflow, Temporal) — there's a tool for that, don't compete.
- **Custom container runtime** — would be 2-year project, no value.
- **Plugin marketplace with 3rd-party code** — security nightmare, low value.
- **AI Copilot for deploys** — Coolify has MCP; consider as a differentiator, not core.
- **Complex policy language** (OPA/Rego) — only used by compliance teams.
- **SOC2 reporting automation** — there are tools for that.
- **Resource quotas at 5+ levels** — solo devs don't need it.
- **"AI-generated" anything** — explicitly rejected by the spec.
- **"Innovative" deployment strategies (e.g., shadow, dark launches)** — over-engineered.
- **Internal developer platform with golden paths** — wrong target audience, defer to V3.
- **Custom DNS server** — let the user use a managed DNS provider; only add DNS-01 ACME for the few cases needed.
- **Embedded Prometheus + Grafana + Loki** — explicitly validated as too heavy. Expose metrics, don't embed.
- **Built-in OIDC server** — defer. Use Authentik/Keycloak integration.
- **Native container image building from source** without Docker — too much for V1.
- **Custom certificate authority** — use Let's Encrypt / external CA.
- **Auto-scaling based on metrics** — V2+ if at all. Solo VPS doesn't auto-scale.
- **gRPC / Thrift / protobuf load balancing** — too niche.
- **WebAssembly runtime** — too niche, defer to V3+ if interest.

### 7.6 Quick verification: items 1–30 (P0) align with severity-weighted pain

The P0 list maps directly to the top-5 pain points in the user pain research:

- Pain 1 (silent backup failures) → Feature 10 (pg_dump + S3), Feature 26 (backup verification)
- Pain 2 (Vercel bill shock) → Feature 1 (git push deploy), Feature 4 (single binary on a Hetzner box)
- Pain 3 (env-var drift) → Feature 8 (encrypted secret store), Feature 14 (--image mode)
- Pain 4 (signup gauntlet) → Feature 4 (single binary, one command), Feature 13 (in-product build)
- Pain 5 (scary rollbacks) → Feature 2 (one-command rollback), Feature 7 (health-check gated)
- Pain 6 (SSL cert expiry) → Feature 3 (auto-TLS), Feature 18 (encrypted state)
- Pain 7 (solo dev) → Feature 6 (CLI), Feature 9 (TUI dashboard), Feature 15 (API)
- Pain 8 (no monitoring) → Feature 22 (health check), Feature 12 (live logs)

**P0 covers ~80% of severity-weighted pain.** The remaining 20% (alert routing, multi-env, audit) is P1 and is "nice to have weekly but not daily."

---

## 8. Feature Dependency Graph

```
Layer 0 — Foundation (cannot ship without)
├── Rust binary (musl + LTO + abort)
├── SQLite (WAL) + migrations
├── systemd unit
├── Linux native (Debian/Ubuntu/RHEL)
└── CLI scaffold (clap + arg parsing)

Layer 1 — Core runtime (MVP)
├── App registry (SQLite table)
├── Docker / Podman runtime adapter
├── Caddy reverse proxy adapter
├── Health check executor
├── Atomic deploy (build → push → swap → verify)
├── Image-tagged deployment history
├── One-command rollback
├── Live log streaming (docker logs passthrough)
└── Auto-TLS via ACME

Layer 2 — Safety (MVP+1)
├── Encrypted secret store (age + envelope)
├── Secret injection at process start (zero-disk)
├── Secret masking in logs
├── Database provision (Postgres, MySQL, SQLite)
├── Database backup (pg_dump / mysqldump) → S3-compatible
├── Backup verification (auto-restore to scratch)
└── Graceful shutdown (SIGTERM, drain)

Layer 3 — Operations (V1)
├── TUI dashboard (ratatui)
├── Resource limits (cgroups)
├── Structured logs (JSON)
├── Log search / filter / retention
├── Health probes (liveness, readiness)
├── Webhook on deploy events
├── Uptime monitoring (external probe)
├── Alert channels (email, Telegram, Slack, Discord, webhook)
└── Restart / stop / start primitives

Layer 4 — Multi-tenant (V1.5)
├── Team access (RBAC: owner, admin, dev, read)
├── Audit log
├── Approval flow for production
├── Multi-environment (dev/staging/prod)
├── Preview environments per PR
└── Agent pattern (server + N agents)

Layer 5 — Sovereignty (V2)
├── Export platform (all state → encrypted tarball)
├── Import platform (reverse)
├── Disaster recovery (tool recover)
├── Air-gapped install
├── EUCS Substantial-aligned controls checklist
├── BSI C5 mapping document
├── rqlite-based HA control plane
└── Vendor-disappear test suite

Layer 6 — Platform (V3+)
├── Service catalog
├── Marketplace (1-click templates)
├── Service mesh (probably never)
├── Multi-region failover
└── Internal Developer Platform (golden paths)
```

### 8.1 Hard dependencies (cannot ship out of order)

- **TUI** requires **API** requires **Layer 0/1** (state + deploy).
- **Multi-tenant / RBAC** requires **audit log** requires **Layer 0** (state).
- **rqlite HA** requires **agent pattern** (V1.5) — cannot skip directly to HA.
- **Preview environments** requires **multi-environment** requires **deploy with config file** (Layer 1).
- **Marketplace / templates** requires **service catalog** (Layer 6) — do not build before.

### 8.2 Parallelizable work (no dependencies between)

- **Linux native** (Debian packaging) is independent of **TUI**.
- **Backup verification** is independent of **TLS automation** but both need Layer 1.
- **Templates** (`tool init fastapi`) is independent of **multi-server**.
- **Alert channels** (email/Telegram) is independent of **monitoring depth** (Prom vs. basic).

---

## 9. Solo Developer Roadmap (1–10 engineers)

A solo developer wants: one command to deploy, one command to roll back, one command to see logs, and not to think about infrastructure between 6 PM Friday and 9 AM Monday.

### 9.1 What they need from V1

- `tool deploy api` — git push, atomic, with health check
- `tool rollback api` — image-tagged history
- `tool logs api` — live, filter, grep
- `tool status` — system overview
- Auto-TLS (no Let's Encrypt thinking)
- Encrypted secret store (no `.env` in git)
- Postgres backup with verification (the #1 silent-failure pain)
- TUI dashboard for the "is it healthy" glance

### 9.2 What they explicitly don't need (defer)

- Multi-server (1 VPS is enough for 1–10 apps)
- Team access (they are the team)
- Audit log (for solo, git history is the audit)
- Multi-environment (they have staging on a branch, prod on main)
- Preview environments (PRs go to a colleague's local)
- SSO / MFA
- Approval flows
- Marketplace / templates
- Multi-region

### 9.3 The "vibe coder" wedge

For the 2026 "I built an app in Cursor/Claude in a weekend" persona:

- One command to deploy: `tool deploy`
- No env var setup: secrets are stored in the binary, not in `.env` in the repo
- No Cloudflare / Stripe / Vercel account gauntlet
- $5 Hetzner VPS + this tool = $5/mo, predictable
- AI-agent-deployable: `claude code "deploy this to production"` works because the CLI is agent-friendly

This is the persona the Vercel $1,237/$46,485 horror stories are about. They are the buyers.

---

## 10. Startup Roadmap (10–100 engineers)

A small startup team wants: same things as solo, plus team features, plus the operational primitives that prevent 3 AM pages (which now hit 3 different people).

### 10.1 What they need from V1 / V1.5

- All V1 solo features
- Team access (owner, admin, developer, read-only)
- Audit log (who deployed what when)
- Multi-environment (dev, staging, production)
- Preview environments per PR (this is the wedge against Vercel)
- Approval flow for production deploys (one approver)
- Slack/Discord alerts
- Postgres with connection pooling
- Health checks on every deploy
- One-command rollback

### 10.2 What they need from V2

- Multi-server (when they outgrow 1 VPS)
- HA control plane (when one team's revenue depends on uptime)
- rqlite-based state
- EU compliance package (when their first EU customer asks for it)
- Air-gapped install (for the financial-services prospect)
- BSL or AGPL for the binary (signals "we won't disappear")

### 10.3 What they explicitly don't need (defer past V2)

- Kubernetes (they don't have a platform team)
- Service mesh
- Custom policy language
- SOC2 reporting
- 5-tier RBAC

---

## 11. Scale Roadmap (1 → 5 → 20 → 100 servers)

### 11.1 At 1 server (MVP, V1)

- Single binary, SQLite, systemd
- 1–50 apps comfortably
- 1 vCPU / 1 GB minimum (with Caddy + Docker daemon)
- 5–10 MB binary
- 20–50 MB idle RAM
- Backup = S3-compatible target, scheduled
- Restore = `tool restore <backup-id>`

### 11.2 At 5 servers (V1.5)

- Add `agent` mode to binary
- `tool server add <ssh>` bootstraps new host
- Server (control plane) on 1 host, agents on 4
- State still in server's SQLite
- Agents are stateless (pull image, run, report)
- Multi-server deploy: `tool deploy api --cluster production`
- First appearance of "fleet view" in TUI

### 11.3 At 20 servers (V2)

- Move state to **rqlite** (SQLite + Raft)
- 3 server nodes for HA, 17 agents
- rqlite runs on each server, Raft consensus, single binary
- No external Postgres needed
- Deploy state survives any single node failure
- Read replica scaling: agents can read state from any node

### 11.4 At 100 servers (V3)

- Stretch to true multi-region if user demand justifies
- 3 server nodes in 2 regions (Raft tolerates 1 region down)
- CRDTs for audit logs (eventually consistent)
- Per-region agent pools
- CDN integration for static assets
- Read replicas for the SQLite state at the agent level
- Service catalog for the 100+ apps
- Internal Developer Platform (golden paths)

### 11.5 When does it stop scaling?

The hard ceiling is **single-server SQLite write throughput** for the control plane. Real limits:

- 500 sustained writes/sec to SQLite (per mvpfactory.io benchmark)
- 50–100 GB database size
- Team size > 3 backend engineers making concurrent state changes

For a deployment control plane, these are **never hit in practice**. Even at 100 servers, you're talking 1 deploy/minute at peak, with ~10 state changes per deploy. That's 10 writes/sec — 50× below the ceiling.

The real scaling bottleneck is **operator cognitive load**, not the binary. At 100 servers, the operator wants:
- TUI fleet view (k9s-style)
- Bulk operations (deploy to 20 servers at once)
- Per-server health matrix
- Per-app multi-server view
- Aggregated logs across servers

All achievable in TUI. No fundamental architecture change required.

---

## 12. Governance Roadmap

Per the spec's "Reject compliance theater" directive, only ship governance features that developers actually use.

### 12.1 V1 (weekly use)

- **Audit log** of every action (deploy, rollback, secret change, domain change, user change)
- Filter by user, action, app, time range
- Export to CSV / JSON
- Immutable storage (no edit, no delete — only append)

### 12.2 V1.5 (monthly use)

- **Team roles** (owner, admin, developer, read-only)
- **Session timeout** (configurable)
- **MFA** (TOTP)

### 12.3 V2 (occasional use)

- **Approval flow** for production deploys (1 approver required, configurable)
- **Production environment lock** during incidents (admin can lock, deploy fails until unlocked)
- **Compliance report** (deploys, rollbacks, access changes, secret changes — generated for audit)

### 12.4 V2+ (defer)

- 2-of-N approvers
- SSO / OIDC (Authentik, Keycloak, Google, GitHub)
- RBAC with custom roles (5+ tiers)
- OPA / Rego policy engine
- SIEM export (Splunk, Datadog)
- SOC2 reporting
- EUCS Substantial controls checklist
- BSI C5 mapping document

### 12.5 Reject

- 5+ tier RBAC
- Custom policy language
- HIPAA/PCI compliance templates
- Multi-tenant governance (the spec says "developers and startups," not SaaS)
- Audit log immutability with blockchain anchoring (theater)

---

## 13. Sovereignty Roadmap

The "sovereignty" wedge is what differentiates this product from Coolify/Dokploy. Make it real, not theatre.

### 13.1 V1 (foundational)

- Self-hosted, no cloud call-home
- Apache 2.0 license, no feature carve-outs
- Encrypted state at rest
- Encrypted backups
- Export platform (full backup, restorable elsewhere)
- Local control plane (no SaaS dependency)

### 13.2 V1.5 (operational)

- Migrate a running app from one server to another (zero-downtime)
- Disaster recovery (`tool recover`)
- Import from Coolify / Dokploy / CapRover backup (best-effort)
- Offline mode (binary works without internet, no external calls)

### 13.3 V2 (compliance-ready)

- Air-gapped install (offline binary + offline docs)
- EUCS Substantial-aligned controls checklist (public document)
- BSI C5 mapping (public document)
- Vendor-disappear test suite (public CI: install fresh, run, see if it works)
- Public roadmap with 18-month commitment

### 13.4 V3+ (defer)

- EUCS High alignment
- SecNumCloud certification
- Federated multi-region (control plane in EU, agents anywhere)
- Hardware security module (HSM) support for master key
- On-prem enterprise edition with SLA

### 13.5 Reject

- "GDPR compliance" branding without substance
- "Immutable audit log" without a real immutability model
- "Zero-trust" without ZTA architecture
- "Air-gapped" without actually testing air-gapped operation

---

## 14. Reliability Roadmap

The reliability features the user pain research identified as highest-priority:

### 14.1 V1 (must have)

- Atomic image-tagged deploys
- Health-check gated deploy (block traffic switch on fail)
- Auto-rollback on health-check fail (configurable threshold)
- Database backup with verification (auto-restore to scratch + row count check)
- Graceful shutdown (drain in-flight requests, configurable timeout)
- Restart on crash (supervisor pattern)
- Health probes (liveness, readiness, startup)
- Resource limits (CPU, RAM) to prevent noisy-neighbor
- Auto-TLS renewal with alert < 14d before expiry
- Backup failure alert

### 14.2 V1.5 (should have)

- Multi-server deploy (with health check per server)
- Canary-style rollout (manual, no traffic splitting yet)
- Disaster recovery (`tool recover`)
- Backup retention policy (auto-purge)
- Auto-failover (basic, rqlite-based)
- Blue-green deploy (default strategy)

### 14.3 V2+ (nice to have)

- True canary (5/25/100% traffic split)
- Multi-region failover
- PITR for Postgres
- Read replicas
- Active-active deploy across regions
- Chaos testing (kill a server, verify recovery)

### 14.4 Reject

- "Self-healing" AI-driven recovery (not real, marketing)
- Predictive scaling (VPS doesn't scale)
- Auto-scaling (out of scope)

---

## 15. Linux Native Roadmap

Per the spec's "Linux native" requirement:

### 15.1 V1

- Runs on Debian 12+, Ubuntu 22.04+, RHEL 9+
- Static musl binary (no glibc dependency)
- systemd unit file (install + enable)
- Reads `/etc/os-release` to detect distro
- Configurable via `/etc/<tool>/config.toml` or `~/.config/<tool>/config.toml`
- Honors `XDG_*` paths
- Logs to systemd journal (with `journalctl -u <tool>`)
- Reads from `/proc`, `/sys` for resource monitoring
- Uses `nftables` / `iptables` for firewall management (when needed)
- Reads from `os-release` for distro detection

### 15.2 V1.5

- Package on **deb** and **rpm** repos (Cloudsmith or packagecloud)
- AUR package for Arch
- `tool doctor` command: checks systemd, Docker, ports, disk, RAM, kernel version
- Auto-detect Hetzner / OVH / DO / Vultr and adjust firewall rules
- Read SSH key from `~/.ssh/id_ed25519` for `tool server add`

### 15.3 V2+

- Read systemd-creds for secret decryption (instead of file)
- Integration with `logind` for user session
- `cgroup v2` awareness for resource limits
- `user_namespaces` for rootless Podman default

### 15.4 Reject

- Windows / macOS server support (Linux only per spec)
- FreeBSD (could revisit if user demand, but defer)
- Immutable distro support (NixOS, Fedora Silverblue) — too niche

---

## 16. TUI Roadmap

The TUI is the differentiator against Coolify/Dokploy. Make it good.

### 16.1 V1 (must work daily)

- **Dashboard:** list of apps, status (running/stopped/deploying), CPU/RAM/disk
- **Logs:** live stream, filter, search, color by severity
- **Deployments:** list of recent deploys, status, duration, who deployed
- **Rollbacks:** one-keystroke rollback to a previous version
- **Servers:** host overview, agent status, per-server resources
- **Secrets:** encrypted store, get/set/delete, never display plaintext
- **Backups:** list of backups, size, last verified, restore button
- **Alerts:** active alerts, acknowledge, mute

### 16.2 V1.5 (power-user features)

- Multi-pane layouts (k9s-style)
- Mouse support (optional, opt-in)
- Vim-style keybindings (default) + arrow keys (alternative)
- Split view: deploys + logs side-by-side
- Filter / search across all apps
- Saved views (e.g., "show me all production deploys in the last 7d")
- Drill-down: app → deployment → log line

### 16.3 V2+ (defer)

- SSH-hostable TUI (russh-based, server-side rendering)
- Multi-server fleet view
- Per-app dashboard (sparklines, time series)
- Cost dashboard (VPS cost / app)

### 16.4 Reject

- Web UI as primary (defer to "optional dashboard" status)
- Desktop app (Electron, Tauri) — adds complexity for no value at this audience
- Mobile UI (terminal apps on phones are painful; defer to web UI)
- Mouse-first interaction (TUI should be keyboard-first, mouse is bonus)

---

## 17. Final MVP (Days 1–42, ~6 weeks)

> **Goal:** ship a binary that does the "Vercel UX, VPS pricing" wedge for one developer. Not a platform, not a PaaS — a deployment engine with clean abstractions that can become a platform.

### 17.1 In scope (8 features)

1. **Single Rust static binary** (musl, LTO, abort, strip) — 10–25 MB
2. **CLI**: `tool deploy <app>`, `tool rollback <app>`, `tool logs <app>`, `tool status`, `tool secret set/get`
3. **Git push deploy with atomic release** (image-tagged, immutable history)
4. **One-command rollback** (instant, image-tagged)
5. **Caddy auto-TLS** (HTTP-01 ACME, auto-renew, hot reload)
6. **Encrypted secret store** (age + envelope encryption, inject at process start)
7. **Postgres backup to S3-compatible** (pg_dump + S3, with size sanity check)
8. **Health check + auto-rollback** (configurable threshold, on health fail)

### 17.2 Architecture (locked)

```
tool/
├── core/          # state, config, db
├── deploy/        # git → build → push → swap → verify
├── runtime/       # Docker adapter
├── proxy/         # Caddy adapter
├── secrets/       # age envelope
├── backups/       # pg_dump + S3
└── cli/           # clap + commands
```

### 17.3 What's explicitly NOT in MVP

- TUI (defer to V1 — use CLI for MVP)
- Multi-server (defer to V1.5)
- Web UI (defer to V2, or never)
- Audit log (defer to V1.5)
- Team access (defer to V1.5)
- Marketplace (defer to V3)
- Approval flows (defer to V2)
- Multi-environment (defer to V1.5)
- Preview environments (defer to V1.5)
- Observability beyond logs (defer to V1)

### 17.4 What MVP looks like to a user

```bash
# Install
curl -sSL tool.example.com/install.sh | sh
tool init

# Register an app
tool app create api --source github.com/me/api --dockerfile Dockerfile
tool domain add api.example.com

# Set secrets
tool secret set DATABASE_URL
tool secret set STRIPE_KEY

# Deploy
git push
tool deploy api

# Check status
tool status
tool logs api --tail

# Rollback if needed
tool rollback api
```

### 17.5 Success criteria for MVP

- One developer can replace a $20–$200/mo PaaS with a $5/mo Hetzner VPS + this tool
- Zero-downtime deploys are real (health-check gated, no in-flight drops)
- Rollback is one command, takes < 30 seconds
- Database backup runs daily, restores verified monthly
- Encrypted secrets can be rotated without re-deploy
- Auto-TLS works out of the box, no cert management

### 17.6 Spec items to defer from the original spec

- **Layered module structure (core/services/providers/storage/cli/api/dashboard)** — keep simple module split for MVP.
- **"Release Management"** — defer. Atomic image tags are enough for V1.
- **Health validation, canary, blue-green** — only health validation in MVP. Canary and blue-green are V1.5.
- **Scaling** — defer. Replicas=1 is enough for V1.
- **All observability beyond logs** — defer to V1.
- **Resource limits** — defer to V1.
- **Runtime recovery** — defer to V1.
- **Domains / DNS / SSL / wildcards** — basic domains + auto-TLS in MVP, defer wildcards and DNS-01 to V1.
- **Internal networking / service discovery** — defer to V1.5.
- **Storage volumes / snapshots / migration** — defer to V1 (basic volumes only).
- **DB provisioning / restore / cloning / migration** — only backup + restore in MVP, provision/cloning/migration in V1.5.
- **Logs collection / search / streaming / retention / export** — only streaming in MVP, the rest in V1.
- **Monitoring CPU/RAM/disk/network/process/app metrics** — only host CPU/RAM in MVP, the rest in V1.
- **Alerting channels (Email, Telegram, Slack, Discord, webhooks)** — only webhook in MVP, the rest in V1.5.
- **Governance** — defer all to V1.5.
- **All sovereignty features beyond local control plane** — defer to V2.
- **Linux native (systemd, journal, SSH, firewall, cert, package mgmt)** — systemd + basic journal in MVP, the rest in V1.5.
- **TUI dashboard / logs / deployments / rollbacks / metrics / alerts / domains / secrets / backups** — defer TUI entirely to V1. CLI only for MVP.
- **Service catalog / marketplace / IDP / templates** — defer to V3.

### 17.7 The 6-week build plan (per spec's "30 Days" with safety margin)

| Week | Focus |
|---|---|
| 1 | CLI scaffold, SQLite migrations, app registry, config file format, `tool app create/list/delete` |
| 2 | Docker runtime adapter, git clone → docker build → docker run, basic logs |
| 3 | Caddy adapter, domain add/remove, HTTP-01 ACME, auto-renew |
| 4 | Atomic deploy (image tag + swap), health check, one-command rollback, deployment history |
| 5 | age + envelope encryption, `tool secret set/get/list/rotate`, inject at process start, mask in logs |
| 6 | Postgres backup to S3, restore, size sanity check, basic observability (live logs only) |

End of week 6: a usable binary. Not a product, but a tool. 6 more weeks of polish → V1.

---

## 18. Final V1 (Weeks 7–18, ~3 months from MVP)

> **Goal:** a usable product for solo developers and small teams. The "killer" features. Where the product becomes defensible.

### 18.1 Adds on top of MVP

- **TUI dashboard** (ratatui) — daily-driver interface
- **Resource limits** (cgroups) — production safety
- **Structured JSON logs** (auto-emitted)
- **Log search / filter / retention**
- **Health probes** (liveness, readiness, startup — separate)
- **Webhook on deploy events**
- **Uptime monitoring** (external probe)
- **Alert channels:** email, Telegram, Slack, Discord, webhook
- **Multi-environment** (dev / staging / prod in one config)
- **Postgres provision (in-app)** (one-click)
- **Postgres connection pooling** (PgBouncer)
- **MySQL / MariaDB / Redis / SQLite provision** (one-click)
- **Volume persistence** (named volumes)
- **Scheduled backup** (cron: daily/weekly/monthly)
- **Backup retention policy** (auto-purge)
- **OpenTelemetry / Prometheus `/metrics` endpoint** (export, not embed)
- **TLS for control plane API** (self-signed → Let's Encrypt)
- **Config file (`app.yaml`) for declarative deploy**
- **CLI: `tool init fastapi` / `tool init nextjs` / `tool init go`** (scaffolding)
- **systemd unit, deb/rpm package**
- **Documentation site** (mdbook, hosted on a Sovereign Application Runtime instance, dogfooding)
- **Show HN post** + awesome-selfhosted inclusion

### 18.2 What's explicitly NOT in V1

- Multi-server (V1.5)
- Web UI (V2+, or never)
- Team access / RBAC (V1.5)
- Audit log (V1.5)
- Approval flow (V2)
- Preview environments (V1.5)
- Service catalog (V3)
- Marketplace (V3)
- rqlite HA (V2)
- EUCS / BSI C5 package (V2)
- Disaster recovery (V2)

### 18.3 What V1 looks like to a user

```bash
# Install
curl -sSL tool.example.com/install.sh | sh  # or apt install tool, or rpm
tool init

# Register an app
tool app create api --source github.com/me/api --dockerfile Dockerfile --env production
tool domain add api.example.com

# Set secrets (encrypted at rest)
tool secret set DATABASE_URL
tool secret set STRIPE_KEY

# Set up a database
tool db create postgres main

# Set up monitoring
tool monitor enable
tool alert add email me@example.com
tool alert add slack #ops

# Deploy
git push  # auto-deploy via webhook, OR
tool deploy api

# Check status (TUI)
tool
# or via CLI
tool status
tool logs api --tail
tool monitor status

# Rollback if needed
tool rollback api
```

### 18.4 V1 success criteria

- A solo dev can run a 1–10 app stack on a $5–$15/mo Hetzner VPS without paying for any other tool
- The tool consumes < 100 MB RAM idle (with Caddy)
- Backup → restore round-trip works in < 10 minutes for a 1 GB Postgres
- TLS auto-renews without intervention
- TUI feels fast (sub-100ms response to keystrokes)
- Time-to-first-deploy after install: < 15 minutes
- Time-to-recovery from a bad deploy: < 60 seconds
- Public Show HN / awesome-selfhosted launch hits top 20
- 100+ GitHub stars in first month
- 10+ paying users (managed / Pro tier) by month 3

---

## 19. Final V2 (Months 4–12)

> **Goal:** a product for small teams (1–100 engineers), with team features, multi-server, and the EU sovereignty package that justifies the price.

### 19.1 Adds on top of V1

- **Multi-server (agent pattern)** — V1.5 feature, blocks V2
- **Team access (RBAC: owner, admin, dev, read)**
- **Audit log** (every action → event, immutable)
- **Approval flow for production deploys** (1 approver)
- **Preview environments per PR** (with TTL)
- **rqlite-based HA control plane** (3-server, tolerates 1 failure)
- **Export / import platform** (full backup / restore, restorable elsewhere)
- **Disaster recovery (`tool recover`)**
- **Air-gapped install** (offline binary + offline docs)
- **EUCS Substantial-aligned controls checklist** (public doc)
- **BSI C5 mapping** (public doc)
- **Migrate-from-Coolify / -Dokploy** (best-effort import)
- **Managed / Pro tier** (commercial offering: hosted control plane)
- **EU pricing** (per-node, €20–100/node/mo)

### 19.2 What's NOT in V2

- True canary deployments (V2.5+)
- Multi-region failover (V3+)
- Service mesh (V3+)
- Marketplace / 1-click apps (V3+)
- Internal Developer Platform (V3+)
- EUCS High alignment (V3+)
- 2-of-N approvers (V3+)
- SSO / OIDC (V3+)
- SOC2 reporting (V3+)

### 19.3 V2 success criteria

- 1,000+ GitHub stars
- 100+ paying customers (Pro or Enterprise)
- 1,000+ paying customers by month 12
- €5–25M ARR by year 3 (per market research)
- First EU public-sector reference customer
- First agency customer (10+ client sites on one instance)
- Public commitment to 18-month sustainability (funding / revenue / both)

---

## 20. Long-Term Platform Vision (V3+)

> **Goal:** a credible alternative to the PaaS/cloud-managed-deploy layer for the 50% of teams who don't want Kubernetes and don't want Heroku.

### 20.1 V3 (12–18 months)

- True canary deployments (5/25/100% traffic split)
- Multi-region failover
- Service catalog (apps, DBs, workers, queues)
- Marketplace (1-click: Plausible, n8n, Ghost, etc.)
- Internal Developer Platform (golden paths, `tool create saas`)
- SSO / OIDC (consumer, not provider)
- MFA (TOTP)
- 2-of-N approvers
- EUCS High alignment
- SecNumCloud (if customer demand)
- Hosted / managed offering (Pro tier, EU region)

### 20.2 V4+ (18+ months, only if real demand)

- Service mesh (probably never, only if user demand)
- Custom policy language (probably never)
- SOC2 reporting (only if customer demand)
- Federated multi-region
- Hardware security module (HSM) support
- AI agent for ops (`tool doctor --auto-fix`)
- White-label / agency edition
- On-prem enterprise edition with SLA

### 20.3 The "platform" rule

> The platform layer only gets built if real adoption justifies it.

The spec's "Future Platform Expansion" (multi-server, secrets, backup, monitoring, logs, pipelines, templates) maps roughly to V1.5 / V2 / V3. Each of these gets built **only if** the previous stage has paying users. Don't build the platform first; build the engine first.

### 20.4 The "sovereignty" long game

The market research identifies a 3-year window (2026–2028) to capture the EU sovereignty opportunity. The long-term play is:

1. **2026 (V1):** Ship the tool. Build community. Show HN. r/selfhosted. 100 users.
2. **2027 (V1.5–V2):** Ship team features + multi-server + EU compliance package. 1,000 users. 100 paying.
3. **2028 (V2–V3):** Ship marketplace + multi-region + EUCS High. 10,000 users. 1,000 paying. €5–25M ARR.
4. **2029+ (V3+):** Platform tier, agency edition, white-label, on-prem enterprise. The "Plausible of deployment."

---

## 21. Risks & Mitigations

### 21.1 High-risk items

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Coolify adds sovereignty package first | High | Medium | Move fast (12-month window). Sovereignty is values + compliance, not just features. |
| Dokploy fixes its license / adds GitOps | High | Low | Differentiate on Rust single-binary + sovereign values. License is one wedge, not the whole story. |
| Hyperscaler "sovereign" SKU wins procurement | Medium | High | Differentiate on **self-hosted** — hyperscalers cannot match. |
| Edge platforms (Cloudflare Workers) reframe sovereignty | Medium | Medium | Position as "for stateful apps," not all apps. Edge is for stateless. |
| AI agents reduce need for deployment runtime | Low | High | Counter: regulated buyers still need auditable, deterministic deployment. |
| Single→multi-server architecture needs rewrite | High (if done wrong) | Critical | Build the agent pattern in V1.5. Don't try to scale single-server. |
| Dokploy-style stuck-deployment bug | High | High | Auto-rollback on health-check fail, "cancel" path is first-class. |
| Caddy OOM under load (known issue #7350) | Medium | Medium | Offer Nginx as low-memory alternative. Document Caddy resource limits. |
| SQLite single-writer at multi-server scale | Low | High | Move to rqlite in V2. Plan from day 1. |
| BuildKit security (running untrusted builds) | Medium | High | Allow `--image=...` to bypass built-in build. Delegate to CI as default. |

### 21.2 Medium-risk items

- **CasaOS-style abandonment** → commit to public roadmap + funding / revenue transparency
- **CapRover-style Docker API breakage** → abstract the runtime (Docker + Podman), document gaps
- **Caddy plugin compilation friction** → use the JSON admin API, not xcaddy
- **Postgres major-version upgrade** → document the process, don't try to automate it
- **"AI agent reads .env"** → zero-disk secret injection from day 1
- **Tooling fragmentation** → consolidate by owning the whole stack (binary, secrets, backups, DB)

### 21.3 Low-risk items (validated, just build it)

- Rust binary size — well-understood
- axum memory footprint — well-understood
- TUI (ratatui) — well-understood
- age + SOPS for secrets — well-understood
- Apache 2.0 licensing — proven

---

## 22. Final Recommendations (Top 10)

If the spec author only reads 10 things from this report, these are the 10:

1. **Build the engine first, the platform later.** MVP = 8 features, 6 weeks, 1 developer can use it. Don't build the platform.

2. **The single most important feature in MVP is one-command rollback.** It is the highest-leverage trust feature in the entire product. Atomic image-tagged deploys + rollback = "scary" becomes "trivial."

3. **The single most important feature in V1 is encrypted secrets with zero-disk injection.** It addresses the #1 AI-agent security risk in 2026 (Claude/Cursor reads `.env`) and the #1 historical leak pattern (`.env` in git). Ship this in MVP, not later.

4. **The single most important feature in V2 is disaster recovery.** `tool recover` restores the entire platform. This is the "sovereign" promise made concrete. It is the answer to "what if you disappear?" — and it has to actually work.

5. **The single biggest architectural decision is multi-server. Get it right early.** Agent pattern (V1.5) → rqlite (V2) → multi-region (V3+). Don't try to scale single-server. Plan from day 1.

6. **The single biggest market differentiator is "truly open source, no carve-outs."** Apache 2.0 + no feature gating + public roadmap + 18-month viability signal. The Dokploy license is a quantified wedge.

7. **The single biggest technology risk is the reverse proxy at scale.** Caddy 30 MB idle, 185 MB at 10K connections. Offer Nginx (10 MB) as low-memory alternative. Document Caddy resource limits.

8. **The single biggest target audience is the 2026 "vibe coder" + solo dev.** $5 Hetzner + this tool = $5/mo, predictable. AI-agent-deployable. The Riley Walz $46,485 Jmail bill is the cold-open pitch.

9. **The single biggest GTM play is Show HN + awesome-selfhosted + r/selfhosted.** Cost: low five figures. Audience: technical, EU-skeptical, lock-in-averse, concentrated.

10. **The single biggest mistake to avoid is the CasaOS pattern.** Apache 2.0 with one corporate sponsor, no clear revenue, dev moves to closed source. Ship a commercial-OSS hybrid from day 1. Public funding / revenue / both. 18-month commitment, on the record.

---

## 23. Source Index

All claims in this report trace to one of the following detailed source files:

| File | Lines | Words | What it covers |
|---|---|---|---|
| `Research-Report-1.md` | 1,100 | ~9,500 | Tech validation: Rust binary size, memory, Caddy, Podman, SQLite, deployment strategies, TUI, BuildKit, observability, secrets, multi-server, licensing. 200+ URLs. |
| `sovereign-runtime-market-research.md` | 369 | ~5,000 | Market: self-hosting growth, EU sovereignty, cloud repatriation, PaaS market, sovereign positioning analogs, pricing, open-core models, GTM, threats. |
| `competitive-landscape.md` | 910 | ~9,400 | 12+ competitors deep-dive: Coolify, Dokploy, CapRover, Dokku, Portainer, YunoHost, CasaOS, Umbrel, Cosmos, Runtipi, Pangolin, plus Rust wave (PIER, sh0, yoink, Tako, Komodo, Shuttle, Rivet, Kamal). 63 KB. |
| `user-pain-research.md` | 523 | ~6,500 | 200+ user pain points with quotes, sources, severity, automation potential. Top 20 ranked. |
| `Research-1.txt` | 2,459 | ~31,000 | Original product spec / master requirement gathering prompt. |

### 23.1 Key sources for the top claims

**Competitive pain:** mfyz.com, ceaksan.com, getautonoma.com, logrocket.com, github.com/Dokploy/dokploy (issues #4461, #4376, #3872, #3098), github.com/caprover/caprover (issue #2351), github.com/IceWhaleTech/CasaOS (#2494), github.com/runtipi/runtipi (advisory GHSA-vrgf-rcj5-6gv9), Hacker News threads (item=43589794).

**User pain:** joshduffy.dev (Vercel bill), indiehackers.com Server Compass (Vercel UX, VPS pricing), dev.to vibe-coding posts, mk0r.com (signup gauntlet), hafiqiqmal93 (silent backup failure), techresolve.blog (env-var drift), dev.to merbayerp (SSL cert expiry), blog.dreamsofcode.io, koome@medium (Dokploy "5-minutes-away friend"), Shubh@medium (license wedge), Sandra Kirsch @medium (PM2 server forgot my app), the lets-code-future 365-deploy experiment.

**Market:** r/selfhosted 750k+ subs, awesome-selfhosted ~297k stars, 2024 self-host survey, IPCEI-CIS €1.2bn, EUCS Feb 2026, BSI C5, SecNumCloud, EU Cloud Sovereignty Framework v1.2.1, VMware 2025 repatriation survey (70%), Deta Space shutdown, Plausible $1M+ ARR post, Supabase $5B Series E Oct 2025, Hetzner +30-50% Apr 2026, agentdeals.dev.

**Tech:** atharvapandey.com (Rust release profiles), theeditorial.news (axum benchmarks), markaicode.com (web framework benchmarks), cloudhostreview.com (Caddy vs Nginx vs Traefik 2026), caddyserver.com/docs/api, github.com/caddyserver/caddy (issues #7350, #5393, PRs #7649, #7258), man.archlinux.org (Podman), oneuptime.com (Podman REST API rootless), sqlite.org (WAL, 3.49 release), mvpfactory.io (SQLite production), s13k.dev (SQLite on $5 VPS), andersmurphy.com (100K TPS billion rows), mvpfactory.io (Litestream), rqlite.io, litefs.io, ratatui.rs, github.com/ratatui/awesome-ratatui, github.com/oddur/yoink, github.com/basecamp/kamal, dokploy.com/blog (Dokploy license change), blog.dreamsofcode.io (Coolify vs Dokploy).

---

## 24. Appendix: Spec Adjustments Summary

Items in the original `Research-1.txt` spec that this research recommends adjusting:

| Spec item | Recommendation |
|---|---|
| "Static binary 8-30 MB" | Realistic 10-25 MB with full feature set. Communicate as such. |
| "Idle RAM 20-50 MB" | True for control plane alone. Real system minimum is 1 GB with Caddy + Docker. |
| "1 vCPU / 512 MB minimum" | Tight for full system. Offer Nginx (10 MB) as low-mem alternative. Default to 1 GB. |
| "Build, Deploy, Rollback, Canary, Blue-Green, Health Validation" | Ship health validation, rolling, blue-green in V1. Defer canary to V2. |
| "Networking: Caddy first, Nginx later, Traefik later" | Caddy V1. Add Nginx as "low-memory mode" V1.1. Traefik V2. |
| "Secrets: encryption, rotation, injection, auditing" | All in V1 (encryption, injection, masking). Rotation V1.5. Auditing V1.5 (audit log). |
| "Storage: SQLite first, Postgres later" | Agreed. Add rqlite as V2 HA option. |
| "Databases: provisioning, backup, restore, cloning, migration" | Provisioning + backup + restore V1. Cloning + migration V1.5. |
| "Logs: collection, search, streaming, retention, export" | Streaming + retention V1. Search + export V1.5. |
| "Monitoring: CPU, RAM, Disk, Network, Process, App Metrics" | CPU + RAM + Disk + Network V1. Process + App metrics V1.5. |
| "Alerting: Email, Telegram, Slack, Discord, Webhooks" | Webhook V1 (MVP). Email + Telegram + Slack + Discord V1.5. |
| "Multi-Server Deployment" | V1.5 (agent pattern). |
| "Fleet Management" | V1.5 (TUI fleet view). |
| "Environment Promotion" | V1.5. |
| "Preview Environments" | V1.5. |
| "Cluster Coordination" | V2 (rqlite). |
| "Traffic Splitting (canary)" | V2.5+ (true canary). |
| "Governance: Audit Logs, Deployment History, Secret History, Change Tracking, Approval Flows, Environment Protection, Access Reviews" | Audit log + deployment history V1.5. Secret history V1.5. Approval flows V2. Environment protection V1.5. Access reviews V2. |
| "Sovereignty: Self Hosting, Offline Mode, Air-Gapped, Exportability, Migration, Backup Independence, Vendor Independence, Local Control Plane" | Self-hosting + local control plane V1 (foundation). Encrypted backups V1. Export V1. Offline mode V2. Air-gapped V2. Migration V2. Vendor independence V2 (EUCS package). |
| "Reliability: Auto Rollback, Health Validation, Recovery Automation, Disaster Recovery, Snapshot Restore, Deployment Verification, Backup Verification" | Auto rollback + health validation + deployment verification V1. Recovery automation V1.5. Backup verification V1. Disaster recovery V2. Snapshot restore V1.5. |
| "Linux Native: systemd, Journal, SSH, Firewall, Cert, Package, Resource" | systemd + journal V1. SSH + cert + resource V1. Firewall + package V1.5. |
| "TUI: Dashboard, Logs, Deployments, Rollbacks, Metrics, Alerts, Domains, Secrets, Backups" | All V1, except metrics V1 (basic) and alerts V1 (basic). |
| "Modules: core/deploy/runtime/proxy/health/rollback, services/logs/metrics/secrets/backup, providers/docker/podman/caddy/nginx, storage/sqlite/postgres, cli/api/dashboard" | All modules in V1, except dashboard (defer to V2). |
| "Final MVP scope (the spec's 'actual MVP')" | Agree. 8 features. See §17. |

---

**End of report.**

Total length: ~30 KB / ~5,000 lines in this consolidated report, synthesizing ~32,000 words across 4 detailed research files plus the original 31,000-word spec.
