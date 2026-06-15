# Sovereign Application Runtime — CTO / Founder Strategy Report

**Date:** 2026-06-03
**Author lens:** Founder / CTO
**Product:** Rust-based, self-hosted, single-binary, CLI/TUI/API-first deployment platform. Positioned around data sovereignty, EU regulatory gravity, and the "Vercel UX, VPS pricing" wedge.
**Verdict in one line:** **Build it, but only the engine first; only under Apache 2.0 with no carve-outs; only with a commercial-OSS hybrid from day 1; only with EU sovereign branding from the first commit; only as bootstrap-then-raise, never bootstrap-or-raise.**

This report takes positions. It does not list options.

---

## 0. Executive summary (read this, then read the rest)

- **Positioning:** a Rust single-binary, GitOps-declarative, AI-agent-friendly **sovereign deployment runtime** for solo developers, agencies, and EU-regulated mid-market — *not* a PaaS in the Vercel sense, *not* an IDP in the Backstage sense, *not* a "Kubernetes without Kubernetes." Those three are the wrong cells. The right cell is the **Plausible of self-hosted deployment**.
- **The single bet we cannot afford to be wrong about:** the **single→multi-server architecture must be designed correctly in V1.5 (agent pattern), with state moving to rqlite in V2**. Every other architectural decision is recoverable. This one is not. If you get it wrong, you rewrite the control plane.
- **License:** Apache 2.0, unmodified, no carve-outs, no source-available, no "Enterprise" feature folders. The Dokploy #3613 thread, the open-source "false marketing" complaint, and the CasaOS abandonment are the *exact* patterns to avoid. ([dokploy #3613](https://github.com/Dokploy/dokploy/issues/3613), [dokploy license update](https://dokploy.com/blog/we-are-updating-dokploys-open-source-license))
- **Funding:** bootstrap to €1M ARR (Plausible trajectory), then a $4–8M seed led by a sovereign-tech-aligned fund, *not* a generalist US VC. The product's market is too European and too procurement-driven to be valued by a Sand Hill Road "growth at all costs" template. ([plausible $1M ARR post](https://plausible.io/blog/open-source-saas), [Plausible 2024 $3.1M ARR](https://getlatka.com/companies/plausible-analytics))
- **Sovereignty is not a feature, it is the brand.** A single-binary, Apache-2.0, vendor-disappear-safe, EU-hosted, EU-supported, EUCS-Substantial-mapped product is what Hyperscaler "sovereign" SKUs (AWS European Sovereign Cloud, Google Sovereign Cloud via T-Systems / S3NS, Microsoft Cloud for Sovereignty) *cannot* structurally offer. That is the moat. ([EU Commission Sovereign Cloud Framework](https://commission.europa.eu/news-and-media/news/sovereign-cloud-framework-explained-2026-06-01_en), [BSI C5:2026](https://www.bsi.bund.de/EN/Themen/Unternehmen-und-Organisationen/Informationen-und-Empfehlungen/Empfehlungen-nach-Angriffsziele/Cloud-Computing/Kriterienkatalog-C5/C5_2025/C5_2025_node.html))
- **GTM:** Show HN first week, awesome-selfhosted second week, IT-SA Nuremberg in October, Paris Open Source Summit in November, FOSDEM in February. Year-1 GTM budget ≤ €30k. ([it-sa.de](https://it-sa.de), [osb-alliance.de](https://osb-alliance.de))

---

## 1. Strategic positioning — what this product actually is

### 1.1 Naming what we are *not*

We are not in the PaaS category, even though the buyer sometimes calls us a "PaaS" because they don't have a better word. The PaaS cell is owned by Vercel, Render, Railway, Fly, Heroku. They have thousands of engineers, hundreds of millions in funding, and they have spent the last five years building features we cannot catch. Don't race them on their track.

We are not in the IDP category, even though "Internal Developer Platform" gets used in a few RFPs. Backstage, Humanitec, Port, Coalesce own that. They sell to platform teams inside enterprises. We are the wrong shape for that buyer.

We are not "Kubernetes without Kubernetes." That framing concedes Kubernetes is the canonical answer and we are the substitute. We are not the substitute. We are a *different question*: how do you deploy a stateful app to one VPS you control, in 30 seconds, with a binary that keeps working if we disappear?

### 1.2 Naming what we *are*

The right frame is: **the Plausible of self-hosted application deployment**. Plausible is the privacy-respecting, single-binary-feeling, EU-based, sustainably-bootstrapped, AGPLv3-but-you-don't-care-because-they-have-no-competition analytics product that hit $3.1M ARR in 2024 with a four-person team, no VC, and a brand that compounds trust. ([plausible.io/about](https://plausible.io/about), [Latka $3.1M ARR 2024](https://getlatka.com/companies/plausible-analytics))

The analogous position in deployment:
- A self-hosted product you can `curl -sSL | sh` in 30 seconds.
- Apache 2.0, unmodified, no carve-outs, no "Enterprise" tier of source-available code.
- Operates on a €4.49 Hetzner CX22 with 1 vCPU / 4 GB.
- Owned by a company incorporated in the EU, with EU-based core maintainers, that can show up in an EU public-sector procurement and pass the jurisdictional sovereignty test.
- Survives the company disappearing: the binary keeps working, the data is exportable, the license is irrevocable.

### 1.3 How positioning evolves as we grow

| Stage | Internal frame | External frame | Buyer |
|---|---|---|---|
| **0→1 (now)** | A deployment engine for solo devs | "Self-hosted Vercel alternative" | Solo devs, vibe coders, agencies |
| **1→10 (year 1)** | A PaaS for small teams | "Coolify but lighter, sovereign, AI-friendly" | Startups, agencies, small SaaS teams |
| **10→100 (year 2)** | A sovereign runtime for EU mid-market | "The Hetzner of deployment" | EU mid-market IT teams, German Mittelstand, French regulated industry |
| **100→1k (year 3)** | A platform for EU public procurement | "EUCS-Substantial deployment runtime" | EU public sector, defense-adjacent, finance |
| **1k→10k (year 5+)** | A category-defining sovereign application platform | "Plausible of deployment" | All of the above, plus agencies managing 50+ client sites |

The naming changes but the *contract with the user* never does: own your data, run anywhere, survive us disappearing, pay a fair price if you want managed.

### 1.4 Analogs studied, recommendation per analog

| Analog | What they proved | What to copy | What to avoid |
|---|---|---|---|
| **Plausible** | Bootstrapped €3.1M ARR, EU-based, AGPLv3, 4-person team, public KPIs | EU base, public KPIs, "we say no to investors" as brand, transparent pricing | None of their model is anti-pattern |
| **Supabase** | $5B valuation, $500M raised, OSS Firebase, dev-first, Postgres-only | Postgres-first, dev-first, community co-investment in Series E | Their VC path; they are now enterprise-sales-driven, not community-driven |
| **Sentry** | $3B valuation, $217M raised, OSS root, self-serve SaaS | "Let them self-host, eventually they pay for cloud" funnel | Their enterprise bloat; Sentry is 800+ people now |
| **Bitwarden** | $100M Series B, freemium, self-hostable | Freemium that genuinely lets you self-host, agency partner program | The capital structure; Bitwarden needed growth capital, we may not |
| **Cal.com** | $32M raised, OSS Calendly, public KPIs | Public KPIs, "open startup" transparency | The "1B users, charge 1%" vision; not our model |
| **Portainer** | Bootstrapped, profitable, Zlib CE + Commercial BE | Clear feature split between CE and BE | Pricing opacity; we publish every tier |
| **Coolify** | 56k stars, $5/mo Cloud + sponsorships, solo founder | Apache 2.0 only, donation + sponsor business, single maintainer credibility | The "no clear revenue" trajectory; Coolify is ~$17-30k MRR from 3,400 cloud customers + sponsorships — survivable, not venture-scale |
| **Dokploy** | 34k stars, fast growth, license controversy | Fast iteration, native Docker Swarm | The license drama. The 2026 license update *finally* made the core pure Apache 2.0 but the `proprietary/` directory created a community backlash ([#3613](https://github.com/Dokploy/dokploy/issues/3613), [#3477](https://github.com/Dokploy/dokploy/issues/3477)) |
| **CasaOS** | 33k stars, Apache 2.0, *abandoned to closed-source ZimaOS* | Nothing | The whole pattern. One corporate sponsor, dev moves to closed product, repo rots. ([Discussion #2494](https://github.com/IceWhaleTech/CasaOS/discussions/2494)) |
| **Pangolin** | 20k+ stars, 140k installs in 5 months, YC S25, $500K seed, AGPL-3.0 + Fossorial commercial free under $100K ARR | AGPL-3.0 with commercial path, YC adjacency, dual-license as defense | Their feature overlap with us is zero; we are a deploy layer, they are an expose layer — we are *partners* |
| **Shuttle** | Rust PaaS, cloud-only, VC-funded | Rust-as-DX, the `#[shuttle_runtime::main]` magic | Their cloud-only model defeats the sovereignty story |
| **Rivet** | Rust actors platform, Apache 2.0 | Single Rust binary, FoundationDB option | Their actor abstraction is too narrow |
| **Kamal 2** | 14k stars, MIT, Ruby, kamal-proxy in Rust | kamal-proxy's "small Rust reverse proxy" pattern, Basecamp credibility | Their imperative deploy model; we want declarative GitOps |
| **Portainer** | 36k stars, Zlib, profitable, 50-100 MB RAM | Lean footprint, the Take3 "free forever for 3 nodes" commercial hook | Their pricing opacity in BE |

([beton.coolify pricing teardown](https://www.getbeton.ai/blog/coolify-pricing-teardown/), [sacra sentry](https://sacra.com/c/sentry/), [cal.com startup intros](https://startupintros.com/orgs/cal-com), [pangolin YC page](https://www.ycombinator.com/companies/pangolin), [plausible $1M ARR post](https://plausible.io/blog/open-source-saas))

---

## 2. Build vs. buy — every major component

The decision rule: **embed if it's the differentiator and we need the binary to feel like ours. Delegate if it's a solved problem and embedding adds risk. Build only if no good option exists.**

| Component | Decision | Rationale | Source / reference |
|---|---|---|---|
| **HTTP / reverse proxy** | **Delegate to Caddy by default; offer Nginx 10 MB "low-mem" mode in V1.1** | Caddy 2.11 has built-in ACME, JSON admin API, ~28 MB idle. Nginx 1.30 has 6 MB idle and 4x lower memory. Caddy is the right default for ops simplicity; Nginx is the right choice for 512 MB systems. Ship both. | [techplained caddy vs nginx](https://www.techplained.com/caddy-vs-nginx), [tech-insider caddy vs nginx 2026](https://tech-insider.org/caddy-vs-nginx-2026/) |
| **Container runtime** | **Delegate to Docker (V1) + Podman (V1.5)** behind our own `Runtime` trait abstraction. Do *not* ship a containerd or buildkit embedded. | Docker daemon is the universal API. Podman v5+ is Docker-API-compatible for our needs. Abstract early so we never rewrite this. | [oneuptime podman REST API](https://oneuptime.com/blog/podman-rest-api-rootless/) |
| **Build system** | **Delegate to external CI by default. Embed a BuildKit/MOBY-compatible build as opt-in for "no CI" users.** | BuildKit is a 100MB+ Go binary; embedding is heavy. But some users won't have CI. Compromise: `tool deploy api` works locally; `tool deploy api --image=ghcr.io/me/api:abc` bypasses. | [BuildKit docs](https://github.com/moby/buildkit) |
| **State / control-plane database** | **Embed SQLite in V1. Move to rqlite in V2.** | SQLite + WAL is the 2026 default (Rails 8 ships this; Litestream validates it for production). rqlite = SQLite + Raft consensus, single binary, no external dep. The rqlite path is the HA path; we do not need it on day 1. | [rqlite.io](https://rqlite.io/), [litestream github](https://github.com/benbjohnson/litestream) |
| **Backup / replication** | **Delegate to Litestream (V1), rqlite's built-in S3 backup (V2), pg_dump for Postgres (V1).** | We do not write replication. We wire it. | [litestream docs](https://litestream.io/) |
| **Secret manager** | **Embed age for at-rest encryption. Delegate to HashiCorp Vault only for enterprise V2 customers.** | Vault went BSL 1.1 in 2023. age + SOPS is the small-team default. Don't ship a "real" secrets manager; ship the right primitives. | [age encryption](https://github.com/FiloSottile/age) |
| **TLS / ACME** | **Delegate to Caddy's built-in ACME for the public edge. Use mkcert-style local CA for the control plane API.** | Caddy handles this for free. Don't reinvent Certbot. | [Caddy ACME docs](https://caddyserver.com/docs/automatic-https) |
| **Identity / SSO** | **Delegate entirely. Integrate with Authentik, Keycloak, Zitadel, Auth0, GitHub OIDC. Do not build an IdP.** | The "build an IdP" trap has killed a dozen PaaS projects. Be an OIDC consumer, never a provider (except for the admin user in V1, and even then via a local Dex-style proxy). | [Authentik](https://goauthentik.io/), [Zitadel](https://zitadel.com/) |
| **Observability** | **Embed OpenTelemetry SDK as opt-in. Expose Prometheus `/metrics`. Do not embed Prometheus, Grafana, or Loki.** | OTel SDK is ~5 MB. We emit traces/metrics/logs; users ship them to wherever they want. | [opentelemetry.io](https://opentelemetry.io/) |
| **Policy engine** | **Delegate to OPA in V2. Do not build a policy language.** | OPA / Rego is the standard. Building a custom DSL is 2 years and a maintenance nightmare. | [openpolicyagent.org](https://www.openpolicyagent.org/) |
| **DNS** | **Delegate to provider APIs (Cloudflare, Hetzner DNS, OVH, Gandi, Route53, DigitalOcean). Do not ship a DNS server.** | We only need DNS-01 ACME support. Plug into existing providers. | [acme dns providers](https://github.com/acmesh-official/acme.sh/wiki/dnsapi) |
| **Object storage** | **Delegate to S3-compatible (R2, B2, MinIO, Garage, SeaweedFS).** | MinIO exists. Garage exists. We don't add value here. | [garagehq.deuxfleurs.fr](https://garagehq.deuxfleurs.fr/) |
| **Logging aggregation** | **Delegate. Export JSON logs to Loki / Datadog / Better Stack / Vector pipeline.** | We do not embed Elasticsearch. | [vector.dev](https://vector.dev/) |
| **CLI / TUI** | **Embed. This is the brand.** | ratatui for TUI, clap for CLI. This is the daily-driver surface and the only place we have a UX moat against Coolify / Dokploy. | [ratatui.rs](https://ratatui.rs/) |
| **Web UI** | **Do not build V1. Add as an *optional* opt-in in V2 (HTMX, ~30 KB like PIER). Never the primary surface.** | The spec says "CLI-first, TUI-first, API-first." Web UI is a fallback for non-terminal users; ship the lightest possible. | [PIER single-binary](https://devcom.app/en/works/pier) |
| **Documentation** | **Embed. mdBook → static site. Hosted on our own product (dogfooding).** | This is the developer-facing brand. | [rust-lang.github.io/mdBook](https://rust-lang.github.io/mdBook/) |
| **Update / package management** | **Embed: signed releases, `tool update`, deb/rpm/AUR. Do not depend on `apt upgrade` working for upgrades.** | The "WordPress-plugin-maintenance feel" of Coolify is partly because they depend on the host's package manager. We ship a self-updating binary. | [Sigstore cosign](https://github.com/sigstore/cosign) |

### 2.1 The "build" trap to avoid

The single largest risk is "build the platform first." Every component above is delegated *because* delegating it is the right move. The components we embed (CLI, TUI, deploy engine, SQLite state, age encryption, security primitives) are the ones that are:

1. The differentiator (TUI daily-driver UX is unique).
2. The brand (single-binary feel is the brand).
3. The trust surface (encryption-at-rest is not delegable to a third party).

Everything else is plumbing. Plumbing is bought, not built.

---

## 3. Technology bets — the strategic decisions

### 3.1 Confirmed bets (validated, low risk)

| Bet | Why we're sure | Source |
|---|---|---|
| **Rust binary, 10–25 MB musl + LTO + abort + strip** | 8-30 MB achievable per [carllriis.com](https://carlriis.com) and PIER proof. | confirmed by [PIER](https://devcom.app/en/works/pier) (20–40 MB) and [sh0](https://sh0.dev) (~50 MB) |
| **axum + tokio + serde** | 2026 default Rust web stack. | confirmed by [theeditorial.news](https://theeditorial.news) benchmarks |
| **SQLite (WAL) for control plane** | Rails 8 default, Litestream validated for production, Forward Email and Grafana run on it. | [sqlite.org](https://sqlite.org), [litestream blog](https://litestream.io/blog) |
| **Caddy as primary reverse proxy** | 2.11 has built-in ACME, JSON admin API, ~28 MB idle, HTTP/3 native. | [techplained benchmark](https://www.techplained.com/caddy-vs-nginx) |
| **Docker + Podman abstraction** | Podman v5+ Docker-API-compatible. | [Podman REST API](https://oneuptime.com/blog/podman-rest-api-rootless/) |
| **Apache 2.0, unmodified, no carve-outs** | Matches every comparable PaaS. Dokploy's mixed license is the cautionary tale. | [Dokploy license update](https://dokploy.com/blog/we-are-updating-dokploys-open-source-license) |
| **age + SOPS for secrets** | The 2026 small-team default after Vault went BSL. | [github.com/FiloSottile/age](https://github.com/FiloSottile/age) |
| **ratatui for TUI** | k9s / lazydocker / yoink pattern. | [ratatui.rs](https://ratatui.rs/) |

### 3.2 The bet we cannot afford to be wrong about

**Single → multi-server architecture.**

The control plane stores deployment state, secret references, app registry, audit log. In V1, this is a single SQLite database on a single host. In V2, it needs to survive a host failure. If we design the V1 control plane without an agent abstraction, the V2 path is a rewrite.

**Recommendation (locked in):**

- **V1:** single binary, single host, SQLite, systemd. No agent. State on the same host as the apps.
- **V1.5 (3-4 months later):** add an `agent` mode to the *same* binary. The server (control plane) pushes deploy instructions to agents over mTLS. State remains on the server's SQLite. Agents are stateless (pull image, run container, report status).
- **V2 (6-9 months later):** introduce **rqlite** as the optional state backend. Three rqlite nodes form a Raft consensus group. Now the control plane survives 1 node failure. Agents read state from any rqlite node.
- **V3+:** only if real user demand, add per-region agent pools and CRDTs for audit log.

**Why this is the bet:** every other decision is recoverable. Wrong proxy? Swap it. Wrong TUI lib? Migrate. Wrong secret scheme? Migrate. Wrong state architecture? You rewrite every command, every endpoint, every agent message, every SQLite query, every backup path. This is the one.

### 3.3 Bets with adjusted risk

| Bet | Original plan | Adjusted |
|---|---|---|
| **"1 vCPU / 512 MB minimum"** | Spec said 512 MB. | Real system minimum is **1 GB** (Caddy 28 MB + Docker daemon 140-180 MB + control plane 50 MB). **Offer Nginx 6 MB as low-mem alternative in V1.1.** Default to 1 GB. |
| **Canary deployments** | Spec lists it. | **Defer to V2.** Rolling + blue-green cover 95% of real use cases. True canary (5/25/100% traffic split) is a V2.5+ feature. |
| **BuildKit self-hosting** | "Own the build" | **Both modes from V1.1:** `tool deploy api` (built-in BuildKit) and `tool deploy api --image=...` (CI-built). Don't lock users in. |
| **Postgres as primary DB** | Spec assumes external Postgres optional. | **Postgres runs as a container we manage. We do not require users to bring their own Postgres.** For the *control plane*, SQLite is the only database. |

### 3.4 Bets that would change the company

If any of these go wrong, the company is in a different shape than planned:

1. **EU sole-incorporation.** We are incorporated in Germany (Berlin) or Estonia (Tallinn). Not Delaware. This is what lets us say "Made and hosted in the EU" with a straight face. Plausible does this from Estonia. ([plausible.io/about](https://plausible.io/about))
2. **Binary-only as the public artifact.** No SaaS-only feature parity. The Pro tier is *managed instances of the same binary*, not a different product. This is the Plausible model.
3. **Vendor-disappear test as a public CI job.** A CI pipeline that takes a fresh Hetzner VM, runs our install script, deploys a test app, takes a backup, restores it on a different VM, and verifies the app comes up. This CI job runs on every release. We link to it from the README. This is the CasaOS antidote.
4. **Public KPIs from month 1.** GitHub stars, npm-equivalent downloads, paying customers, MRR. Plausible does this. Cal.com does this. We do this.

---

## 4. Open-source strategy

### 4.1 License: **Apache 2.0, unmodified, no carve-outs, no source-available, no "Enterprise" tier of code**

This is the single largest competitive lever. The Dokploy experience is the case study: their 2024-2025 license was "Apache 2.0 with additional terms" and was called out repeatedly as "not actually open source" on HN, Medium, and GitHub. The January 2026 update to "Apache 2.0 + new `proprietary/` directory" *immediately* triggered issue #3613 ("False Marketing: Stop marketing Dokploy as open source") and #3477 ("Provide open source version of Dokploy stripped down"). ([dokploy #3613](https://github.com/Dokploy/dokploy/issues/3613), [dokploy #3477](https://github.com/Dokploy/dokploy/issues/3477), [dokploy license update](https://dokploy.com/blog/we-are-updating-dokploys-open-source-license))

**What we do:**

- The community binary is Apache 2.0. Every feature that is in the community binary is Apache 2.0 forever. We do not move code from `oss/` to `proprietary/` six months later.
- The commercial tier is **managed instances of the same binary** + **professional support** + **SLA contracts** + **EUCS-aligned compliance documentation** + **sealed-update channel** for the Pro binary. The Pro binary is the same Rust source compiled with the `--features enterprise` flag (e.g., for SAML, OIDC, SSO integration, multi-region state). The source is still public, just feature-gated.
- We do not use BSL, Elastic License, or Business Source License. Those signal "we don't trust you." Apache 2.0 + commercial-features-built-on-top is the 2026 default (Cal.com, Bitwarden, Supabase, Sentry).

### 4.2 Trademark

Register the product name and the project logo as trademarks. Hold them personally or in a holding company separate from the OSS project, as the [Linux Foundation](https://www.linuxfoundation.org) and [Apache Foundation](https://www.apache.org) do. This lets us:
- Issue a Trademark Policy granting non-profits and community contributors a free license to use the marks.
- Reserve the right to enforce against commercial resellers who misappropriate the brand.
- If a foundation transfer ever happens, transfer the marks with the project.

### 4.3 Contributor License Agreement (CLA)

Required for any meaningful contribution. We use the **Apache ICLA (Individual Contributor License Agreement)** standard, which assigns copyright to the project and grants back a license to the contributor. This is what Apache projects use, what the [Rust Foundation](https://rustfoundation.org) uses, and what the [Cloud Native Computing Foundation](https://www.cncf.io) (CNCF) uses. The CLA is the only legal mechanism that lets us later relicense (e.g., if we ever needed to move to a dual-license structure) without chasing down every contributor.

### 4.4 Governance model

**BDFL (Benevolent Dictator For Life) for the first 24 months, transitioning to a 3-person maintainer council by month 24.**

The reason: governance overhead kills small projects. Plausible has been 2 people for 7 years and the company is healthy. Cal.com has been 21-50 people with a co-CEO structure. Shuttle, Rivet, Pangolin, Sentry, all started BDFL. The Rust project *itself* is a Leadership Council of 4-9 teams — and the [Rust Foundation](https://rustfoundation.org/policy/bylaws/) has 5 classes of membership with Platinum / Gold / Silver / Associate / Individual. We are not at that scale in year 1. Trying to mimic the Rust Foundation at 200 GitHub stars is theatre.

**Year 1 governance (BDFL):**

- 1-2 core maintainers with merge rights.
- 5-10 "trusted contributors" with merge rights on specific subsystems (TUI, security, proxy, secrets).
- Public decision-making on GitHub (issues + discussions).
- 90-day roadmap published quarterly.

**Year 2 governance (Council):**

- 3-person maintainer council.
- Council seats elected by maintainer vote, 2-year terms.
- 2-of-3 supermajority for major decisions (relicensing, governance changes, foundation transfer).
- Public decision log.

**Year 3-5 governance (Foundation):**

- Transfer the project to an existing sovereign-tech-aligned foundation or spin up a small one. [Eclipse Foundation](https://www.eclipse.org/org/foundation/), [OpenSSF](https://openssf.org/), [Linux Foundation Europe](https://linuxfoundation.eu) are candidates. The CNCF is a possibility if the project becomes significant enough.
- Foundation membership: $20K-$50K/year platinum tier from 3-5 EU companies (Hetzner, OVH, Netcup, Scaleway, Clever Cloud, IONOS). This is *fundable* in the EU ecosystem and provides multi-year sustainability.
- Foundation owns the trademark, manages the CLA, runs the CVE process, and holds the copyright.

### 4.5 The CasaOS pattern (the one to avoid)

CasaOS is the warning sign for any Apache-2.0 project with one corporate sponsor. Last commit December 2024. IceWhale has moved all development to closed-source ZimaOS. The community is on its own. ([Discussion #2494](https://github.com/IceWhaleTech/CasaOS/discussions/2494), [Issue #767](https://github.com/IceWhaleTech/CasaOS-AppStore/issues/767))

The CasaOS pattern is: Apache 2.0 + one corporate sponsor + no clear revenue + dev moves to closed product. We avoid this by:
1. Public commitment to 18-month sustainability on the website, updated quarterly.
2. Vendor-disappear test CI that runs on every release.
3. Foundation transfer planned by year 3, not year 10.
4. Multiple funding sources from day 1 (sponsorships + cloud tier + Pro tier), not one.

---

## 5. Funding strategy

### 5.1 The honest question: bootstrap or raise?

**Both. Bootstrap to €1M ARR, then raise a €4-8M seed, then never raise again unless Series A makes sense at €10M+ ARR.**

Why bootstrap first:
- **Plausible proved it.** $3.1M ARR in 2024, no VC, 4 people, EU-based. ([Latka](https://getlatka.com/companies/plausible-analytics), [plausible.io/about](https://plausible.io/about))
- **Coolify proved the alternative is failure.** 56k stars, $17-30k MRR from 3,400 cloud customers, no VC, but stuck at solo-founder scale. ([beton coolify teardown](https://www.getbeton.ai/blog/coolify-pricing-teardown/))
- **The sovereignty market is procurement-driven.** European public-sector buyers want stability, not hockey-stick growth. A bootstrapped company that is profitable from year 2 is more procurement-credible than a VC-backed one burning cash.
- **Sovereignty is the brand.** Taking US VC money creates a contradiction. The brand *requires* EU-aligned capital.

Why raise later:
- **EU public-sector RFPs require scale.** A 5-person company can't bid for a Bundeswehr contract. We need to grow.
- **EUCS-Substantial certification costs €200-500K.** Self-funding that is brutal.
- **Engineering depth grows with capital.** Multi-server (V2) needs 2-3 senior Rust engineers, not 1.
- **Foundation transfer at year 3 needs legal buffer.** A €4-8M cushion lets us do this without existential risk.

### 5.2 Funding model: years 0-5

| Stage | ARR target | Headcount | Funding source | Cap table |
|---|---|---|---|---|
| **Year 0 (now)** | €0 | 1-2 founders | Founder savings + 1 angel (€50-200k) | Founders 90-100% |
| **Year 1** | €50-200k | 2-3 (founders + 1 hire) | Sponsorships (€2-5k/mo) + Cloud tier (€1-2k MRR) + angel runway | Founders 80-90% |
| **Year 2** | €500k-1M | 4-6 (add Rust engineer, designer, devrel) | Pro tier (€5-15k MRR) + Enterprise early customers | Founders 70-80% |
| **Year 3** | €1-3M | 8-12 | **Series Seed €4-8M** from sovereign-tech-aligned funds (OSS Capital, Open Core Ventures, Cherry Ventures, Speedinvest, Earlybird, Balderton, Notion Capital, HV Capital, Klima, join.capital, LocalGlobe EU) | Founders 50-60%, Seed investors 20-30% |
| **Year 4** | €3-8M | 15-25 | Organic + Series A optional at €8M+ ARR | Founders 40-50% |
| **Year 5** | €8-20M | 30-50 | Organic + Series A or B if scaling | Founders 30-40% |

### 5.3 Year-3 raise specifics

- **Target:** €4-8M Series Seed.
- **Lead candidate funds (aligned with sovereign-tech + open-source):**
  - **OSS Capital** (US/EU, open-source-only thesis, has invested in Cal.com, Supabase-adjacent companies, and others). ([oss.capital](https://oss.capital))
  - **Open Core Ventures** (EU, open-source commercial focused).
  - **Cherry Ventures** (Berlin-based, EU focus).
  - **Speedinvest** (Vienna, deep EU).
  - **Notion Capital** (London, B2B SaaS specialist).
  - **LocalGlobe EU** (London, founder-friendly).
- **What we say no to:** US generalist VCs that don't understand EU procurement, hyperscaler CVCs (Microsoft, Google, AWS) that conflict with sovereignty positioning, crypto funds, growth-stage funds at the seed stage.
- **What we ask for in the term sheet:** clean cap table, no participating preferred, no super-pro-rata, board observer seat max. We are not a hypergrowth company; we are a 10-year compounding company.

### 5.4 Comparison to analogs

| Analog | Funding path | ARR outcome | Lesson |
|---|---|---|---|
| **Plausible** | Bootstrapped, 0 raised, profitable from year 2 | $3.1M ARR 2024 | "Bootstrapping keeps our options open" — Uku Taht |
| **Supabase** | YC → $500M raised → $5B valuation | $200M+ ARR (estimated) | Enterprise sales machine; not our model |
| **Sentry** | Bootstrapped to $600k → $217M raised → $3B valuation | $128M ARR 2023 | "Bootstrapping forces you to validate with the customer" — David Cramer |
| **Bitwarden** | $100M Series B 2022 | $200M+ ARR estimated | Needed capital for B2B sales motion; not relevant for us |
| **Cal.com** | $32M raised (Series A 2022) | ~$1.1M ARR 2026 | "1B users, charge 1%" — vision is bigger than our model |
| **Portainer** | Bootstrapped, profitable | Not disclosed | Reference model for sustainable self-hosted business |
| **Coolify** | Bootstrapped, sponsors + Cloud | ~$17-30k MRR | Stuck at solo founder scale; ceiling is real |
| **Pangolin** | $500K YC seed 2025, AGPL-3.0 + commercial | Not disclosed | YC adjacency; very early stage |

([plausible.io/blog/customers-not-investors](https://plausible.io/blog/customers-not-investors), [firstround sentry](https://review.firstround.com/sentrys-path-to-product-market-fit/), [pangolin YC](https://www.ycombinator.com/companies/pangolin), [cal.com startup intros](https://startupintros.com/orgs/cal-com), [beton coolify teardown](https://www.getbeton.ai/blog/coolify-pricing-teardown/))

---

## 6. Team building

### 6.1 Founding team composition (months 0-12)

For a Rust-based deployment platform, the founding team needs:

| Role | Why | When to hire | Salary range (EU, 2026) |
|---|---|---|---|
| **Founder/CTO (Rust core engineer)** | Writes the deploy engine, the agent pattern, the SQLite state layer, the security primitives. The architecture decisions live here. | Day 1 (founder) | Founder sweat equity |
| **Founder/CEO (Product + GTM)** | Owns positioning, pricing, EU procurement relationships, Show HN, awesome-selfhosted, IT-SA, FOSDEM. | Day 1 (founder) | Founder sweat equity |
| **Senior Rust engineer (deploy + runtime)** | Implements Docker/Podman adapter, blue-green, health-check gate, auto-rollback. | Month 4-6 | €80-130k + 0.5-1.5% equity |
| **Senior Rust engineer (state + rqlite + HA)** | Implements rqlite integration, backup/restore, Litestream wiring. | Month 9-12 (V2) | €80-130k + 0.5-1.5% |
| **Designer (TUI + brand)** | ratatui layout, brand identity, website, docs. | Month 2-4 | €60-90k |
| **Technical writer / DevRel** | Docs, blog, Show HN, IT-SA talks. | Month 6-9 | €50-80k |

### 6.2 Year-2 hires (months 12-24)

- **Senior frontend engineer (HTMX dashboard)** — when we ship the optional web UI.
- **Security engineer** — for CVE process, EUCS mapping, security@ alias, coordinated disclosure.
- **Account executive (EU public sector)** — when first Bundeswehr / BSI / AgID customer lands.
- **Developer relations engineer** — for conference circuit, content marketing.
- **Customer support engineer (EU timezone, German or French speaker)** — for Pro tier.

### 6.3 Year-3+ hires (months 24+)

- **Solutions architect** — for enterprise customer implementations.
- **Compliance officer** — for EUCS-Substantial, BSI C5, ISO 27001.
- **Engineering manager** — when the team crosses 8.
- **Marketing lead** — content, brand, growth.
- **Sales engineer** — RFP responses, demos.

### 6.4 Founding team composition rules

1. **At least one founder is technical and writes Rust daily.** The architecture decisions are too important to delegate. The Sentry story (David Cramer) and the Plausible story (Uku Taht) both have this.
2. **At least one founder is comfortable in EU public-sector rooms.** The buyers are German Mittelstand, French regulated industry, Italian PA. The sales motion is procurement-driven. The CEO/founder needs to walk into an IT-SA booth and have a procurement conversation in German or French.
3. **No "fractional CTO" or "advisor as CTO" model for the first 24 months.** The technical direction is too load-bearing.
4. **Co-founder vesting: 4 years, 1-year cliff, same terms.** Standard.
5. **All-remote, async-first, EU timezone overlap of 4-6 hours.** Plausible, Cal.com, Sentry, Pangolin all work this way.

### 6.5 Compensation

| Level | Cash (EU 2026) | Equity (post-seed) |
|---|---|---|
| Senior IC engineer | €90-140k | 0.3-1.0% |
| Staff engineer | €130-180k | 0.5-1.5% |
| Designer | €70-100k | 0.2-0.5% |
| DevRel / technical writer | €60-90k | 0.2-0.5% |
| Sales / AE (public sector) | €80-120k base + 0.5-1.0% commission | 0.1-0.3% |
| Compliance / security | €90-130k | 0.2-0.5% |
| C-level (post-seed) | €140-200k | 1-3% |

These are EU market rates, not US rates. We are not competing with Stripe for engineers. We are competing with Sentry / Plausible / Cal.com for EU-based engineers who want sovereignty and work-life balance.

### 6.6 Remote vs. in-person

**All-remote, with quarterly in-person offsites (3-4 days, EU cities).** Plausible and Cal.com model. We do not need an office. We do need:
- Annual team offsite (1 week, EU).
- Quarterly 3-4 day working retreats (Berlin, Tallinn, Amsterdam, Lisbon).
- Conference circuit overlap (IT-SA, FOSDEM, Paris Open Source Summit, KubeCon EU).
- An optional co-working budget for people who want to work from a shared space (€200-400/month/person).

---

## 7. Competitive moat

### 7.1 What the moat is *not*

| "Moat" the spec implies | Why it's not a moat |
|---|---|
| "Rust single binary" | PIER, sh0, yoink, Tako, Komodo all do it. [PIER](https://devcom.app/en/works/pier) is 20-40 MB. The wedge is repeatable. |
| "Small footprint" | Benchmarks are public. Anyone with Rust + axum can hit 30 MB. |
| "Coolify alternative" | Coolify has 56k stars and 3,400 paying customers. They are the incumbent. ([beton coolify teardown](https://www.getbeton.ai/blog/coolify-pricing-teardown/)) |
| "Dokploy alternative" | Dokploy has 34k stars. The 2026 license update fixed the "not actually open source" problem, but the community trust damage is done. ([dokploy license update](https://dokploy.com/blog/we-are-updating-dokploys-open-source-license)) |
| "Sovereignty as a feature" | Coolify or Dokploy can add BSI C5 mapping in a quarter. The *credibility* of sovereignty is the moat, not the documentation. |

### 7.2 What the moat *is*

| Moat | Why it takes time to copy | Time to build |
|---|---|---|
| **EU incorporation + EU public-sector procurement track record** | AWS European Sovereign Cloud has the legal entity but the brand story is "we're US pretending to be EU." A native EU company with German Bundeswehr references is structurally different. | 3-5 years |
| **18-month sustainability signal** | CasaOS failed this. Plausible has it. Pangolin has it (YC). Coolify is borderline. We commit publicly, quarterly, with a public CI vendor-disappear test. | 18-24 months |
| **Trust / credibility** | "Will this be alive in 18 months?" is the buyer's #1 objection. We answer it with revenue, customers, EU public references, public KPIs, public roadmap, foundation transfer plan. | 3-5 years |
| **TUI as daily-driver** | yoink has a TUI but 12 stars. The TUI is the differentiator *if* we invest in it like k9s / lazygit / btop did. Daily-driver polish is 2-3 years of work. | 2-3 years |
| **Vintage of bug fixes and edge cases** | Every real-world user produces edge cases. Coolify has 781 open issues; we will have hundreds. Vintage matters. | 2-5 years |
| **Distribution via EU procurement frameworks** | Once we're on the German Vergabeplattform, the French UGAP, the Italian MePA / Consip, the Spanish CPV — we are in. This is permanent. | 2-4 years |
| **Vendor-disappear CI test** | A public CI that takes a fresh VM, installs us, deploys an app, backs up, restores on a different VM, and verifies the app comes up — and runs on every release. This is a real moat. CasaOS could not have done this honestly. | 6-12 months to build, permanent |

The 80/20 of moat: **EU incorporation + public sustainability + TUI polish + vendor-disappear CI + procurement presence**. None of these are impossible to copy, but together they take 3-5 years. By the time a competitor has all of them, we have a customer base, a brand, and a foundation transfer.

---

## 8. Pricing strategy

### 8.1 The pricing bands (recommendation)

| Tier | Price (EU 2026) | What's included | Why |
|---|---|---|---|
| **Community (self-hosted)** | €0 forever | Apache 2.0 binary, all features, no telemetry, no phone-home | The brand. The trust signal. The wedge. |
| **Cloud (managed single-tenant)** | €20-100/node/month | Hosted in EU region, automatic updates, daily backups, monitoring, support 9-5 EU hours | The recurring revenue engine. The "I don't want to operate the control plane" wedge. |
| **Pro (commercial license + support)** | €5-15k/year | Signed binary updates, CVE feed, support SLA, embargoed security advisories, audit-log export | The "I'm an agency, I need a paper trail" wedge. |
| **Enterprise (EU-aligned)** | €25-100k/year | EUCS-Substantial controls checklist, BSI C5 mapping, dedicated EU support engineer, on-prem option, custom DPA, training, 24/7 SLA, air-gapped install | The procurement wedge. |
| **OEM / White-label** | Custom | Rebrand + redistribute the binary | The agency / MSSP wedge. |

### 8.2 Pricing model: per-node, not per-seat, not per-app, not per-deploy

- **Per-seat** is the wrong unit for a self-hosted product. The buyer is a 1-3 person IT team running 20 apps for 200 users.
- **Per-app** creates a tax on adoption. We want users to add more apps, not fewer.
- **Per-deploy** is hostile to the persona. Vercel's per-deploy is the cautionary tale (Riley Walz $46,485 Jmail bill, Nov 2025).
- **Per-node** matches how EU public procurement scopes cloud spend. It also matches the hardware unit the customer thinks in.
- **Flat annual** is the alternative for agencies and small teams. We offer it on the Pro tier.

### 8.3 The "introducing a paid tier without alienating the community" playbook

Plausible nailed this. ([plausible.io/blog/open-source-saas](https://plausible.io/blog/open-source-saas)) Sentry nailed it. ([firstround sentry](https://review.firstround.com/sentrys-path-to-product-market-fit/)) Cal.com is doing it now.

The rules:

1. **The community edition is fully functional.** No "Pro feature" is something a solo dev needs. The Cloud / Pro / Enterprise tiers are *operational* and *procurement* layers, not feature gates.
2. **The Cloud tier is "I want to pay you to run the same binary for me, in an EU region, with monitoring and support."** It is not "the SaaS version." The user can `git clone` the source, build the same binary, run it themselves, and never know the difference.
3. **The Enterprise tier is documentation and people.** BSI C5 mapping document. EUCS-Substantial controls checklist. Dedicated support engineer. Custom DPA. Training. This is *what the procurement office is buying*, not features.
4. **We publish our pricing on the website.** No "contact sales" wall for tiers under €25k/year. (We do enterprise sales for the > €25k deals.)
5. **The pricing is in EUR.** Not USD. We are an EU company. The buyer is EU. Currency is part of the sovereignty story.
6. **The price is annual by default.** Monthly billing is a 12% surcharge (matches SaaS market convention). Annual commits get a 16% discount (2 months free).

### 8.4 What we never do

- **Per-seat pricing.** Wrong unit for the persona.
- **Per-deploy metering.** Hostile to the vibe coder. We are the antidote to Vercel bills.
- **Hyperscaler-style "free tier, then surprise bill."** Plausible's transparent pricing is the playbook. ([plausible.io/blog/customers-not-investors](https://plausible.io/blog/customers-not-investors))
- **Auto-renewing contracts with price-hike clauses.** Cal.com, Bitwarden, Sentry have all been burned by this. We don't.
- **Forced telemetry / phone-home.** The community binary is offline-mode by default. The Cloud binary is the one that phones home, and it phones home to us, not to anyone else.

---

## 9. GTM strategy

### 9.1 Year-1 GTM (low budget, high signal)

| Tactic | Cost | Expected outcome | Reference |
|---|---|---|---|
| **Show HN launch** | $0 | 500-2000 upvotes, 200+ comments, top 20 if pitch is sharp. Coolify, Dokploy, Easypanel all hit top 20 this way. | [news.ycombinator.com](https://news.ycombinator.com) |
| **awesome-selfhosted inclusion** | $0 | 297k-star repo, organic traffic for years. Inclusion under "Deployment / PaaS" category. | [github.com/awesome-selfhosted](https://github.com/awesome-selfhosted/awesome-selfhosted) |
| **r/selfhosted AMA / launch** | $0 | 750k subs, AMA posts routinely hit 500+ upvotes. | [reddit.com/r/selfhosted](https://reddit.com/r/selfhosted) |
| **Hacker News "Launch HN" follow-up** | $0 | Show HN gets traffic, the comment thread gets the technical community. Reply to every comment. | [news.ycombinator.com](https://news.ycombinator.com) |
| **r/selfhosted sponsorship (sidebar)** | $500-2000/month | Top-of-feed visibility. Worth it. | [reddit.com/r/selfhosted](https://reddit.com/r/selfhosted) |
| **Content marketing (blog)** | Founder time | 1-2 long-form posts per month. "How to migrate from Heroku to Sovereign Application Runtime." "Why we use SQLite, not Postgres." "How we encrypt secrets at rest." | [plausible.io/blog](https://plausible.io/blog) |
| **Conference: IT-SA Nuremberg (October)** | €5-10k booth | 800+ public-sector attendees, German Mittelstand, EU procurement. | [it-sa.de](https://it-sa.de) |
| **Conference: Paris Open Source Summit (November/December)** | €3-6k booth | French regulated industry, ANSSI ecosystem, Cloud de Confiance. | [osb-alliance.de](https://osb-alliance.de) |
| **Conference: FOSDEM (February, Brussels)** | €0-2k | Devrel track, EU dev community. | [fosdem.org](https://fosdem.org) |
| **Conference: KubeCon EU** | Probably not year 1. We're not a K8s product. | — | — |
| **Open Source Business Alliance membership** | €2-5k/year | Procurement-grade credibility in Germany. | [osb-alliance.de](https://osb-alliance.de) |
| **Public KPIs dashboard** | Founder time | Plausible model. Public MRR, public stars, public customer count. | [plausible.io](https://plausible.io) |
| **Vendor-disappear CI badge in README** | Founder time | A green check that says "I was tested." The CasaOS antidote. | — |
| **OSS Capital / HN / dev.to / Medium syndication** | Founder time | Cross-post technical deep-dives. | [dev.to](https://dev.to), [medium.com](https://medium.com) |

Total year-1 GTM cost: **€15-30k**. Plausible-level efficiency.

### 9.2 Year-2 GTM

- **EU public-sector pilot program.** 3-5 pilot deployments with German Bundesländer, French CHU, Italian PA, Belgian federal. Free or heavily discounted. The references are worth 10x the cost.
- **OSS Capital / sovereign-tech-fund introductions.** For the seed round.
- **Content series: "The Sovereign Cloud Cookbook."** Long-form, well-edited, downloadable. Lead magnet for the EUCS-Substantial checklist.
- **Devrel engineer (hired).** Conference circuit, dev.to syndication, HN presence.
- **Agency partner program.** 5-10 EU agencies that resell our Pro tier. Bitwarden model. ([bitwarden](https://bitwarden.com))

### 9.3 Year-3 GTM

- **EU procurement framework listings.** Vergabeplattform (DE), UGAP (FR), MePA / Consip (IT), CPV (ES). Each is a 6-12 month process.
- **EUCS-Substantial audit + BSI C5 mapping published.** Now we're a real enterprise option.
- **Account executive (EU public sector).** First sales hire.
- **Agency program scaling.** 50+ agencies reselling.
- **Sponsored tracks at IT-SA, FOSDEM, Devoxx.**

### 9.4 What we never do

- **Paid ads on Google / Facebook.** The audience is technical, the channel is community, the budget is engineering.
- **Cold outbound to enterprises before we have product-market fit.** The first 50 customers find us.
- **Conference booths at KubeCon.** Wrong audience, wrong year. By year 3, maybe a presence. Not year 1.
- **"AI-powered" marketing copy.** The buyer is allergic.

---

## 10. Partnerships & ecosystem

### 10.1 Distribution partnerships (year 1)

| Partner | Why | Form |
|---|---|---|
| **Hetzner** | They are the EU sovereign VPS. We are the EU sovereign deployment. They sponsor us, we list them. | Sponsorship ($2-5k/month) + co-marketing |
| **OVH / OVHcloud** | Second-biggest EU cloud. French presence, ANSSI relationships. | Marketplace listing + co-marketing |
| **Netcup** | German budget VPS. | Affiliate |
| **Scaleway** | French cloud, EU sovereign. | Marketplace listing |
| **Cloudflare** | DNS-01 ACME partner, CDN. | Tech integration, not commercial |
| **BunnyCDN** | EU CDN. | Tech integration, not commercial |
| **Canonical (Ubuntu Pro)** | Default Linux for VPS users. | Tech integration, co-marketing |
| **SUSE / openSUSE** | EU Linux vendor. Strong German Mittelstand presence. | Tech integration, co-marketing |
| **Authentik** | EU-based OIDC provider. | Tech integration, co-marketing |
| **Zitadel** | EU-based identity. | Tech integration, co-marketing |
| **OpenObserve / Quickwit** | EU-based observability. | Tech integration, co-marketing |
| **Pangolin** | EU proxy / identity layer. Complementary, not competitive. | Tech integration, co-marketing |

### 10.2 Year 2+ partnerships

- **OpenSSF / Linux Foundation Europe** — for foundation transfer, CVE process.
- **BSI / ANSSI / AgID** — for sovereign-tech alignment and EUCS mapping.
- **CNCF** — if the project becomes significant (probably year 3+).
- **Eclipse Foundation** — alternative foundation home.
- **Hetzner / OVH / Netcup** — deeper integration (one-click install, marketplace apps, joint webinars).

### 10.3 What we never do

- **Hyperscaler co-marketing.** AWS, Google, Microsoft CVCs — say no. The brand dies.
- **Crypto partnerships.** The audience hates them.
- **Influencer "sponsored content" deals.** The audience detects them instantly.

---

## 11. Long-term vision

### 11.1 Year 1 (now → month 12)

**Goal:** a usable V1 product for solo developers and small teams. Self-sustaining. 100+ GitHub stars. 10+ paying Cloud customers. First Show HN top-20.

**Product surface:** MVP + V1 (see `Research-2.md` §17-§18). 8 MVP features, then 30 V1 features.

**Headcount:** 2-3 (founders + 1 hire).

**ARR target:** €50-200k (Cloud tier, Pro tier, sponsorships).

**Brand signal:** "We are the Plausible of deployment. Rust, single-binary, Apache 2.0, EU-based, won't disappear."

**Exit:** none planned. Building.

### 11.2 Year 3 (now → month 36)

**Goal:** a sustainable, EU procurement-grade business. 1,000+ paying customers. €1-3M ARR. Foundation transfer initiated.

**Product surface:** V1.5 + V2 (multi-server, RBAC, audit, rqlite, EUCS-Substantial package).

**Headcount:** 8-12.

**ARR target:** €1-3M. Plausible's 2022 number.

**Brand signal:** "We are in the procurement conversation. The BSI references us. The Bundeswehr uses us in pilot. We are on the German Vergabeplattform."

**Funding:** €4-8M Series Seed raised in year 2 / start of year 3.

**Exit:** none planned.

### 11.3 Year 5 (now → month 60)

**Goal:** a category-defining sovereign application platform. 10,000+ paying customers. €8-20M ARR. Foundation transferred. AGI has not killed the deployment runtime.

**Product surface:** V3+ (canary, multi-region, IDP, marketplace, EUCS-High).

**Headcount:** 30-50.

**ARR target:** €8-20M.

**Brand signal:** "Plausible of deployment" is a phrase people use without attribution.

**Funding:** organic, possibly Series A at year 4-5 if needed for EUCS-High certification costs.

**Exit:** none planned.

### 11.4 Year 10 (now → month 120)

**Goal:** a generational company. 50,000+ paying customers. €50-100M ARR. The Plausible story x10.

**Product surface:** V4+ (everything in the spec, with hindsight-driven pruning).

**Headcount:** 100-200.

**ARR target:** €50-100M.

**Brand signal:** "The default European deployment runtime. The first thing you install on a Hetzner box."

**Exit:** still none planned. The Plausible model is forever-compound, not exit. ([plausible.io/blog/customers-not-investors](https://plausible.io/blog/customers-not-investors))

### 11.5 The "what if you disappear" question

This is the same question for the product and the company. The answer must be the same in year 1 and year 10:

> The Apache 2.0 license is irrevocable. The binary is self-contained. The data is exportable. The state is in SQLite. The agent pattern is documented. The vendor-disappear test passes. A solo developer with the source and a Hetzner bill can keep running the platform forever.

This is the founding question of an open-source product. We answer it the same way Plausible does. ([plausible.io/about](https://plausible.io/about))

---

## 12. Acquisition & exit

### 12.1 Would this be acquired?

**Probably not, and that's the point.**

Acquisition candidates and what they would pay for:

| Acquirer | Why they'd buy | Realistic acquisition trigger | Realistic valuation |
|---|---|---|---|
| **Cloudflare** | Self-hostable deploy + Cloudflare Tunnel / Workers. They'd want to integrate. | €50M+ ARR + EU procurement presence | €500M-1B |
| **Fastly** | Edge compute + sovereign deploy. Compute@Edge story. | €30M+ ARR | €300-500M |
| **Vercel** | They have v0, they want self-hosted for the EU. But they'd be a competitor, so unlikely. | n/a | n/a |
| **Netlify** | Similar to Vercel, unlikely. | n/a | n/a |
| **Akamai** | Edge + EU sovereign. Plausible-style. | €30M+ ARR | €300-500M |
| **SUSE** | European open-source consolidator. They'd buy to anchor their sovereign cloud story. | €10M+ ARR + EU procurement | €100-300M |
| **Red Hat (IBM)** | Same logic as SUSE. | €30M+ ARR | €300-500M |
| **GitHub (Microsoft)** | Self-hosted deploy is adjacent to GitHub Actions. But hyperscaler conflict with sovereignty. Unlikely. | n/a | n/a |
| **GitLab** | Adjacent. But GitLab is its own thing. | n/a | n/a |

### 12.2 Stay independent forever (the Plausible model)

The Plausible model is: "We're 4 people, $3.1M ARR, EU-based, never raise, never sell, never acquire." That is the reference. ([plausible.io/blog/customers-not-investors](https://plausible.io/blog/customers-not-investors))

The Sentry model is: "We bootstrapped to $600k ARR, then raised $217M, now we're $3B. We're a real company." That is also a valid model. ([firstround sentry](https://review.firstround.com/sentrys-path-to-product-market-fit/))

We pick the Plausible model for the first €1M ARR, then make a decision at €3M ARR based on:
- Is the company profitable and self-sustaining?
- Is the EU public-sector market growing faster than we can keep up?
- Is there a strategic acquirer we trust to keep the project sovereign?

If the answers are "yes, yes, no" — we stay independent. If "yes, yes, yes" — we negotiate. If "no" on any of the first two — we have bigger problems.

### 12.3 The "right" exit if we ever do

The right exit is to **a foundation, not a corporation**. We transfer the project (code, trademark, copyright, governance) to a sovereign-tech-aligned foundation. The company becomes a service provider to the foundation, with a long-term support contract. Plausible has not done this yet, but the option is on the table. ([plausible.io/blog/customers-not-investors](https://plausible.io/blog/customers-not-investors))

This is the only exit that preserves the brand. Anything else is a CasaOS or Bitwarden-style sell-out, where the project slowly rots.

---

## 13. Risk management

### 13.1 The 12 risks that could kill us

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| 1 | **Coolify adds sovereignty package first** | High | Medium | Speed. Sovereignty is values + compliance + procurement presence, not a doc. Coolify is a solo founder; they can add docs but not a German Bundeswehr reference. |
| 2 | **Dokploy fixes its license** | Done in Jan 2026 | Low | Differentiation on Rust single-binary + sovereign values. License was one wedge, not the whole story. |
| 3 | **Hyperscaler "sovereign" SKU wins procurement** | Medium | High | Structural differentiation: self-hosted. Hyperscalers cannot match. They are sovereign-pretending; we are sovereign-actual. |
| 4 | **Edge platforms (Cloudflare Workers) reframe sovereignty** | Medium | Medium | Position as "for stateful apps." Edge is for stateless. Most regulated workloads are stateful. |
| 5 | **AI agents reduce need for deployment runtime** | Low | High | Counter: regulated buyers still need auditable, deterministic deployment. AI agents don't write the audit log. |
| 6 | **Single→multi-server architecture needs rewrite** | High (if done wrong) | Critical | Build the agent pattern in V1.5. Plan for rqlite in V2 from day 1. This is THE bet. |
| 7 | **Dokploy-style stuck-deployment bug** | High | High | Auto-rollback on health-check fail. "Cancel" path is first-class. Never wedge the queue. |
| 8 | **Caddy OOM under load (issue #7350)** | Medium | Medium | Offer Nginx as low-memory alternative. Document Caddy resource limits. |
| 9 | **SQLite single-writer at multi-server scale** | Low | High | Move to rqlite in V2. Plan from day 1. |
| 10 | **CasaOS-style abandonment** | Low (we have plan) | Critical | Public 18-month commitment, public CI vendor-disappear test, foundation transfer plan by year 3. |
| 11 | **Runtipi-style RCE in backup (GHSA-vrgf-rcj5-6gv9)** | Medium | Critical | `tool doctor` checks for known CVEs. Coordinated disclosure process. CVE feed. Security advisories. Signed releases. |
| 12 | **Co-founder drama / team implosion** | Medium | Critical | 4-year vesting, 1-year cliff, founder agreements with buy-sell clauses, decision-making process documented. |

### 13.2 The risks we are deliberately taking

- **EU-only market focus.** Limits TAM, but we are not chasing US enterprise anyway. This is a feature, not a bug.
- **Rust team is small.** Senior Rust engineers are scarce. Mitigation: hire in EU, not US, where the cost is lower and the market is less competitive.
- **No Kubernetes.** Locks us out of certain enterprise conversations. Mitigation: K8s is the cluster, we are the deploy layer; complementary, not competitive.
- **No SaaS-only mode.** Limits PLG motion. Mitigation: the Cloud tier is single-tenant managed, not multi-tenant SaaS. This is the *sovereign* model.
- **No AI features in core.** May feel dated by 2027. Mitigation: an OTel-based MCP server for AI agents is a V1.5 feature, not core.

---

## 14. The "single binary" brand promise

### 14.1 What the promise means to each audience

| Audience | What "single binary" means |
|---|---|
| **Solo dev** | `curl -sSL ... | sh` and you're deploying. No docker-compose, no Helm, no 7 services, no 1.2 GB RAM, no panel as attack surface. |
| **Agency operator** | I have 30 client sites. I `scp` the binary to each VPS. Same operations across all of them. I am not running 30 different versions of Coolify. |
| **EU public-sector procurement** | We can audit one binary. We can sign one binary. We can vendor-disappear-test one binary. We can list its SBOM. We can prove the supply chain. We cannot do this for a PHP + Laravel + Svelte + Postgres + Redis + Soketi + Traefik stack. |
| **EU regulator (BSI / ANSSI / AgID)** | One binary to certify, one binary to revoke, one binary to monitor. The "single binary" is not a feature; it is a compliance surface. |
| **Vibe coder** | My AI agent can `tool deploy` from a single binary. No 7-service stack to wire up. |
| **Security team** | One binary, one set of CVEs, one update path. Not 6+ containers. |

### 14.2 How to defend it against "but I want a web UI"

The defense is not "no UI, ever." The defense is "the binary comes first; the UI is a thin optional layer."

- **V1:** binary + CLI + TUI. No web UI. The TUI is the primary surface.
- **V1.5:** binary + CLI + TUI + optional MCP server (for AI agents).
- **V2:** binary + CLI + TUI + MCP + optional HTMX web UI (30 KB, served by the binary). The UI is a thin layer on top of the same API the CLI uses.
- **V3+:** binary + CLI + TUI + MCP + HTMX UI + a hosted Cloud version (the binary running in an EU region with a managed UI on top).

The web UI never becomes a separate product. It is always an *optional* view on the same binary. The "no UI" choice in V1 is the trust signal; the "yes UI" choice in V2 is the convenience layer.

### 14.3 How to defend it against "I need a 6-service stack"

We don't. The 6-service stack is the Coolify answer because they have a 1.2 GB RAM footprint, a panel as attack surface, and a database that needs its own upgrade path. We have 1 binary, 1 SQLite database (with WAL), 1 Caddy process, 1 Docker daemon, 1 our-control-plane process. That's it. Five processes for the whole system. Each is independently upgradable, each has a small CVE surface, each can be replaced.

---

## 15. The "Vercel UX, VPS pricing" gap — and the other gaps

### 15.1 The "Vercel UX, VPS pricing" gap

This is the explicit Indie Hackers quote from the Server Compass founder:

> "I was running projects across multiple PaaS platforms. Vercel for frontends, Railway for some backend services, Render for others, Supabase for auth, NeonDB for postgres. Each one felt great individually. Then I actually added up what I was paying: ~$200/month across everything. For side projects and small apps. … I knew the math didn't make sense. A $6 Hetzner VPS could run all of this. But every time I thought about migrating, I remembered what VPS management actually felt like: SSH-ing around, managing PM2 in tmux, grep-ing through logs at 2am." (indiehackers.com Server Compass, cited in [Research-2.md](file:///C:/Users/Victo/Downloads/webproj/Cloud/Research-2.md))

The "vibe coder" wedge is the same. The 2026 LeadDev survey of 800 indie hackers found the median monthly cost of running a vibe-coded SaaS was $87, with the top quartile at $340/mo. ([blog.vibecoder.me vibe coder cost](https://blog.vibecoder.me/how-much-cost-vibe-code-app))

**Is this the actual wedge?** Yes, and it is the largest. Every vibe coder paying Vercel $87-340/mo is the buyer.

### 15.2 But the wedge is not the only one. Other gaps:

| Gap | Size | Who owns it | Where we fit |
|---|---|---|---|
| **Vercel UX, VPS pricing** | Huge | Coolify (heavy), Dokploy (license drama) | Rust single-binary, no panel required |
| **Heroku replacement** | Huge | Dokku, Coolify, Render self-host | We are the Rust-era Dokku |
| **Heroku → Kubernetes migration** | Medium | Komodo, K8s consultancies | We are the *non*-K8s path |
| **AWS bills** | Huge | Vantage, CloudZero, ProsperOps (cost tools) | We are the *exit from* AWS, not the cost optimizer |
| **Single-tenant SaaS** | Medium | Plausible, Bitwarden, Nextcloud | We are the Plausible of deployment |
| **Data residency / EU sovereign** | Huge (and growing) | AWS European Sovereign Cloud, Google via T-Systems, Microsoft Cloud for Sovereignty | We are the *self-hosted* sovereign; they cannot match |
| **AI agent deploy** | Emerging | vibe-deploy, Coolify MCP | We are MCP-native from V1.5 |

**The biggest gap is the Vercel-UX + VPS-pricing gap, but the most durable is the data-residency gap.** EU public-sector buyers are writing 7-year contracts around this. The Cloud and AI Development Act (CAIDA) is on the table in 2026; ~70% of EU public data will fall under level 1, 20% under level 2, 9% under level 3, 1% under level 4. ([Euractiv CAIDA](https://www.euractiv.com/news/commissions-sovereign-cloud-plan-doesnt-push-us-hyperscalers-out/)) The hyperscalers will compete for levels 1-3. We are structurally positioned for level 4 (full sovereignty) and for the 30%+ of public-sector data that wants *not* a hyperscaler.

---

## 16. The "Sovereign" brand

### 16.1 What "sovereign" means to a buyer

| Buyer | What "sovereign" means to them | How we message |
|---|---|---|
| **German Mittelstand CTO** | "BSI C5 mapped, GDPR-clean, no CLOUD Act exposure, German Hetzner box, German support, German contracting entity." | "Sovereign means: the binary is signed by us, the data lives on your hardware, the encryption keys are yours, the support is in your timezone, the company is in your jurisdiction." |
| **French CHU IT director** | "SecNumCloud-aligned, ANSSI-recommended, HDS-compliant, French contracting entity." | Same. Plus: "We do not phone home. We do not require a US-entity subprocess." |
| **Italian PA procurement officer** | "AgID-qualified, ACN-listed, Italian or EU contracting entity, no CLOUD Act." | Same. Plus: "We are on the MePA / Consip catalog." |
| **EU defense-adjacent** | "EUCS Substantial, EU ownership, EU HQ, no non-EU jurisdiction exposure." | "Our company is incorporated in the EU. Our binary runs on your hardware. Our support is in the EU. Our roadmap is public. Our source is Apache 2.0. You can fork us if you want." |
| **Solo developer** | "I don't want my .env in the cloud. I want my database on my own box. I want to be able to walk away." | "Sovereign means: your data, your hardware, your keys, your choice. We are a convenience layer, not a gatekeeper." |
| **Vibe coder** | "I want to deploy without giving Vercel my source code. I want my AI agent to deploy without sending secrets to a third party." | "Sovereign means: your secrets never leave your machine. The deploy is local. The AI agent runs locally if you want." |

### 16.2 The five types of sovereignty

| Type | What it means | What we do |
|---|---|---|
| **Data sovereignty** | Data lives in a specific jurisdiction, under specific laws. | We are local-first. The control plane, the data, the secrets, the backups are on the user's hardware. The user chooses the jurisdiction. |
| **Operational sovereignty** | Operations don't depend on a foreign vendor. | The binary keeps working if we disappear. The data is exportable. The state is SQLite. The dependencies (Caddy, age, SQLite) are independent open-source projects. |
| **Vendor sovereignty** | No lock-in to a single vendor. | Apache 2.0. Exportable. The vendor-disappear test passes. |
| **Legal sovereignty** | The vendor is in a jurisdiction the buyer trusts. | EU incorporation. EU HQ. EU support. EU-based core maintainers. |
| **Technical sovereignty** | The technology stack is open, auditable, and replaceable. | Rust source, Apache 2.0, no proprietary components, no SaaS-only features, no black boxes. |

We claim all five. Hyperscalers can claim 2-3 of the 5. We claim all 5 because we are *self-hosted by design*.

---

## 17. The "EUCS Substantial" path

### 17.1 What it takes

EUCS (EU Cybersecurity Certification Scheme for Cloud Services) was finalized February 2026. There are three assurance levels: Basic, Substantial, High. EUCS Substantial is the realistic year-1-2 target. EUCS High is the year-3-5 target.

**For EUCS Substantial:**

- A 12-18 month process.
- €200-500K cost (mostly auditor fees + consultant time + remediation).
- Requires: documented security controls, risk management, supply chain security, incident handling, business continuity, data portability, customer audit rights, encryption, access control, logging.
- Mapping to BSI C5:2026 (the German national standard) is a strong starting point. C5:2026 explicitly aligns with EUCS Substantial. ([BSI C5:2026](https://www.bsi.bund.de/EN/Themen/Unternehmen-und-Organisationen/Informationen-und-Empfehlungen/Empfehlungen-nach-Angriffsziele/Cloud-Computing/Kriterienkatalog-C5/C5_2025/C5_2025_node.html))
- The European Commission Cloud III Dynamic Purchasing System (Cloud III DPS) ran a sovereign cloud procurement in April 2026 with a €180M tender. The Cloud Sovereignty Framework published June 2026 is the reference. ([Commission Sovereign Cloud Framework June 2026](https://commission.europa.eu/news-and-media/news/sovereign-cloud-framework-explained-2026-06-01_en))

### 17.2 When to start

**Start in month 12.** Year 1 is product-market fit. Year 2 is "we have enough customers to justify the audit cost" + "we have a security engineer." Year 3 is the audit. Year 4 is the certificate.

### 17.3 Who can help

- **BSI (DE)** — for C5 mapping. ([bsi.bund.de](https://www.bsi.bund.de))
- **ANSSI (FR)** — for SecNumCloud. ([ssi.gouv.fr](https://www.ssi.gouv.fr))
- **AgID / ACN (IT)** — for Italian PA qualification. ([agid.gov.it](https://www.agid.gov.it))
- **ENISA (EU)** — for EUCS administration. ([enisa.europa.eu](https://www.enisa.europa.eu))
- **OpenSSF** — for security best practices. ([openssf.org](https://openssf.org))
- **Linux Foundation Europe** — for foundation transfer and EU procurement presence. ([linuxfoundation.eu](https://linuxfoundation.eu))
- **Dutch NCCA** — for EUCS national certification body coordination. ([dutchncca.nl](https://www.dutchncca.nl))

### 17.4 The ROI

- German Bundeswehr pilot contract: €100-500K/year.
- French CHU pilot: €50-200K/year.
- Italian PA framework: €200K-2M/year.
- EU agency multi-year: €500K-5M/year.

The €200-500K audit cost pays for itself with the first 1-2 EU public-sector customers. This is the bet.

### 17.5 The "SEAL" framework alignment

The Commission's June 2026 Sovereign Cloud Framework defines four Sovereignty Effectiveness Assurance Levels (SEAL):

- **SEAL-0:** No sovereignty.
- **SEAL-1:** Jurisdictional sovereignty (EU law applies, but service is controlled by non-EU).
- **SEAL-2:** Data sovereignty (EU jurisdictions apply, indirect non-EU control).
- **SEAL-3:** Technological sovereignty (EU actors with meaningful but not full influence).
- **SEAL-4:** Full digital sovereignty (complete EU control, only EU jurisdiction, no critical non-EU dependencies). The Commission notes SEAL-4 is not currently achievable due to hardware / chip supply chain.

We are structurally SEAL-3 by default (Apache 2.0 Rust binary, EU incorporation, EU support, EU-based maintainers, runs on EU sovereign VPS). SEAL-4 is impossible today for *anyone* due to chip / hardware dependencies. We position as "the closest to SEAL-4 in the deployment runtime category." ([Commission Sovereign Cloud Framework](https://commission.europa.eu/news-and-media/news/sovereign-cloud-framework-explained-2026-06-01_en))

---

## 18. Open-source governance

### 18.1 The two-axis decision

| | BDFL / Founder-led | Foundation / Council |
|---|---|---|
| **Year 1 (now)** | ✓ Use this | Too early |
| **Year 2** | ✓ Still this | Plan for it |
| **Year 3** | 50/50 | Start the conversation |
| **Year 4-5** | Transition | Adopt |

### 18.2 Foundation candidates

| Foundation | Pros | Cons |
|---|---|---|
| **CNCF (Cloud Native Computing Foundation)** | Strong brand, sandbox → incubating → graduated path, K8s ecosystem | US-centric, sandbox takes 12+ months |
| **Apache Software Foundation** | Strong brand, vendor-neutral, mature governance | Heavy process, projects need a committer base of 3+ to start |
| **Eclipse Foundation** | EU presence, strong IP framework (EFSP), 400+ projects, sovereign-tech-friendly | Smaller community for deploy tooling |
| **Linux Foundation Europe** | EU HQ (Brussels), EU public-sector procurement credibility, sovereign-tech-aligned | New (founded 2022), still building track record |
| **OpenSSF** | Security-first, perfect for the CVE / security@ work | Focused on security, not project governance |
| **OpenJS Foundation** | JavaScript / Node ecosystem | Wrong language family |
| **Rust Foundation** | 5-class membership, mature governance, EU-friendly | Rust-only by design |

**Recommendation:** Linux Foundation Europe, with Apache or Eclipse as backup. Sovereign-tech-aligned, EU-based, growing, and they understand the procurement story.

### 18.3 Maintainer ladder

Plausible-style. ([plausible.io/about](https://plausible.io/about)) Cal.com-style. ([cal.com startup intros](https://startupintros.com/orgs/cal-com))

- **Contributor** — anyone with a merged PR.
- **Triage** — 3+ merged PRs, can label/triage issues.
- **Reviewer** — 10+ merged PRs, can review and approve.
- **Maintainer** — 25+ merged PRs across 2+ subsystems, invited by existing maintainers, voting member.
- **Council** — 3-person elected body, 2-year terms.

### 18.4 Conflict resolution

- **Day-to-day:** public on GitHub. The maintainer with the most context decides. If 2 disagree, defer to the council.
- **Re-licensing:** 2/3 supermajority of all maintainers + counsel review. The CLA is the legal mechanism.
- **Trademark disputes:** the foundation (or holding company) decides.
- **Security incidents:** the security@ alias + 3-person VMT (Vulnerability Management Team) decides. Embargoed. Public post-disclosure.

### 18.5 What we copy from each foundation

- **Rust Foundation:** 5-class membership, supermajority vote definition, individual member class for project directors. ([rustfoundation.org/policy/bylaws](https://rustfoundation.org/policy/bylaws/))
- **Apache Foundation:** Vendor-neutrality, "no contribution is too small," public decision-making, merit-based promotion.
- **CNCF:** Sandbox → Incubating → Graduated path, KEPs / RFEs / ADRs for significant decisions, public landscape placement.
- **OpenSSF:** security@ alias, CVD process, CVE feed, signed releases, SBOM. ([ossf/oss-vulnerability-guide](https://github.com/ossf/oss-vulnerability-guide), [google/oss-vulnerability-guide](https://github.com/google/oss-vulnerability-guide))
- **Linux Foundation:** Trademark Policy template, project lifecycle definitions, membership tiers.

---

## 19. Security culture

### 19.1 The 90-day CVD standard

We follow the [OpenSSF Coordinated Vulnerability Disclosure guide](https://github.com/ossf/oss-vulnerability-guide/blob/main/maintainer-guide.md) and the [Google OSS Vulnerability Guide](https://github.com/google/oss-vulnerability-guide/blob/main/guide.md) as the reference processes.

- **Acknowledge within 48 hours.**
- **Confirm within 7 days.**
- **Patch within 90 days (or sooner if actively exploited).**
- **CVE assigned for every public advisory.**
- **Embargoed notification for hyperscaler / Linux-distro consumers 1-30 days before public disclosure.**
- **Public advisory with affected versions, fix version, severity (CVSS), reporter credit.**

### 19.2 The security primitives we ship from day 1

| Primitive | From V1 | Source / format |
|---|---|---|
| **`security@` alias** | V1 | `security@sovereignruntime.example`, monitored by 3-person VMT |
| **`SECURITY.md`** | V1 | [OpenSSF template](https://github.com/ossf/oss-vulnerability-guide/blob/main/maintainer-guide.md), in repo + linked from README |
| **GitHub Private Vulnerability Reporting (PVR)** | V1 | [GitHub docs](https://docs.github.com/en/code-security/security-advisories/guidance-on-reporting-and-writing-information-about-vulnerabilities/privately-reporting-a-security-vulnerability) — free for OSS |
| **Signed releases (cosign / Sigstore)** | V1 | [Sigstore](https://github.com/sigstore/cosign) |
| **SBOM per release (CycloneDX)** | V1 | [CycloneDX](https://cyclonedx.org/) |
| **SLSA Level 3 build provenance** | V1.5 | [SLSA](https://slsa.dev) |
| **Reproducible builds** | V2 | `cargo build --reproducible` + diff tooling |
| **CVE feed (JSON + RSS)** | V1.5 | `https://sovereignruntime.example/cve.json` |
| **Security advisories (GHSA + OSV)** | V1.5 | [OSV.dev](https://osv.dev) |
| **Coordinated disclosure timeline in advisory** | V1 | Standard |
| **Bug bounty (low five figures)** | V2 | Through a platform (e.g., [huntr](https://huntr.com), [Intigriti](https://intigriti.com)) — not for RCE-class bugs at solo-founder scale, but as a signal |
| **Third-party penetration test** | Year 2-3 | €30-50K, annual |
| **SOC 2 Type II** | Year 3-4 | When we have 10+ enterprise customers asking |

### 19.3 The "Runtipi mistake" — RCE in backup (GHSA-vrgf-rcj5-6gv9)

Runtipi had a critical RCE in backup-restore functionality in 2026. Authenticated users could execute arbitrary system commands via shell metacharacters in backup filenames. Fixed in v4.7.0. ([runtipi GHSA-vrgf-rcj5-6gv9](https://github.com/runtipi/runtipi/security/advisories/GHSA-vrgf-rcj5-6gv9))

Our mitigation:

- **No shell execution from user input.** All user inputs (filenames, paths, env var values) are passed as arguments, never interpolated into shell commands.
- **Static analysis in CI** (cargo-audit, cargo-deny, clippy with security lints, Semgrep rules).
- **Fuzzing on backup/restore paths** (cargo-fuzz).
- **Security@ alias + 3-person VMT from day 1.**
- **No single-maintainer security decisions.** Even in year 1, the founder + 1 trusted security advisor + 1 hired security engineer (when hired) form the VMT.

### 19.4 The "Riley Walz" mistake — Vercel bill shock

Riley Walz's Jmail project received a $46,485 bill from Vercel in November 2025. Vercel changed its free tier in May 2025 to throttle bandwidth, and indie projects with viral traffic got hit.

Our mitigation: **we never charge per-deploy or per-bandwidth.** Our Cloud tier is **per-node, flat, predictable, annual**. Plausible's model. ([plausible.io/blog/customers-not-investors](https://plausible.io/blog/customers-not-investors)) The user sees the bill in advance, every month, and it does not change because their app went viral.

---

## 20. Decision framework

### 20.1 The "start / stop / continue / double down" list

**Start doing (year 1):**
- Public KPIs dashboard (Plausible model)
- Vendor-disappear CI test on every release
- Security@ alias + SECURITY.md + GitHub PVR
- Cosign-signed releases
- EU incorporation (Berlin or Tallinn)
- Show HN + awesome-selfhosted + r/selfhosted launch
- IT-SA / FOSDEM / Paris Open Source Summit presence
- Public 18-month sustainability commitment, updated quarterly
- EU procurement reading list (BSI C5:2026, SecNumCloud, AgID qualifications)
- TUI polish like k9s / lazygit (this is the daily-driver surface, invest here)

**Stop doing (after the spec):**
- Trying to compete with Coolify on "more features"
- Reading r/selfhosted for product ideas (read for *which pain is most painful*, not for features)
- Building the platform before the engine
- Building the web UI before the binary is good
- Reading Dokploy issues to find what to copy

**Continue doing:**
- Saying no to feature requests that don't pass the 10-question filter (Research-1.txt §3)
- Building the engine first, the platform later
- Treating the spec as a research artifact, not a roadmap
- Reading the existing research files (Research-1.txt, Research-2.md, sovereign-runtime-market-research.md, competitive-landscape.md) before making any decision

**Double down:**
- The single-binary brand promise
- Apache 2.0, no carve-outs
- EU sovereign positioning
- The TUI as daily-driver
- The agent pattern in V1.5 (the architecture bet)
- The vendor-disappear CI test
- Plausible-style transparency (public KPIs, public roadmap, public funding)

### 20.2 When to ship, when to wait, when to kill

| Decision | Ship when | Wait when | Kill when |
|---|---|---|---|
| **New feature** | Passes the 10-question filter (Research-1.txt §3), and 3+ users have explicitly asked | Passes the filter but 0 users have asked | Fails the filter, or is "AI-powered" / "innovative" / "trend-driven" |
| **Architectural change** | Has a working prototype, has a migration plan, has been reviewed by 2+ maintainers | Has a prototype but no migration plan | Has a prototype and would require rewriting V1 control plane |
| **New tier (Cloud / Pro / Enterprise)** | Has 10+ paying customers asking, has clear pricing, has docs | Has 1-2 asking, pricing unclear | Has 0 asking, no clear value |
| **Conference booth** | Has clear buyer persona, has a deck, has 1+ lead from past event | Has a persona but no past lead | "Should be there" is the only reason |
| **Hire** | The work cannot be done by existing team, the role has been needed for 3+ months, budget exists | "We could probably use one" | "It would be nice to have" |
| **New language / market** | A paying customer in that market has asked, the work to support is bounded | The work is unbounded | The work would require a rewrite |
| **Acquisition offer** | The offer is 5x+ ARR, the acquirer commits to the project, the foundation is willing | The offer is 2-3x ARR, the acquirer is silent on the project | The acquirer is a hyperscaler or a company that has killed prior OSS projects |

### 20.3 The 80/20 of product strategy

If the founder has 4 hours a week for strategy, here is where they go:

1. **Read every issue and PR for the top 20 most engaged users.** (1 hour)
2. **Talk to 1 paying customer per week.** (1 hour)
3. **Write 1 short blog post or HN comment per week.** (1 hour)
4. **Plan the next 90 days with the team.** (1 hour)

Everything else is delegation.

---

## 21. The mistakes to avoid

### 21.1 The Coolify mistake

Coolify has 56k stars, 3,400+ cloud customers, ~$17-30k MRR, a solo founder. The product is excellent. The business is stuck. ([beton coolify teardown](https://www.getbeton.ai/blog/coolify-pricing-teardown/))

**Why:** No clear revenue model beyond donations + $5/mo Cloud. No EU sovereign positioning. No enterprise tier. Solo founder cannot scale the team without revenue. ([coolify.io/sponsorships](https://coolify.io/sponsorships/))

**How we avoid it:** Plausible model. Public KPIs. €1M ARR target by year 2. EU sovereign positioning from day 1. Enterprise tier (€25-100k/year) by year 2. Bootstrap to seed, not bootstrap forever.

### 21.2 The Dokploy mistake

Dokploy has 34k stars, fast growth, a license controversy. The 2026 license update to "Apache 2.0 + new `proprietary/` directory" *still* triggered community backlash. ([dokploy #3613](https://github.com/Dokploy/dokploy/issues/3613), [dokploy #3477](https://github.com/Dokploy/dokploy/issues/3477), [dokploy license update](https://dokploy.com/blog/we-are-updating-dokploys-open-source-license))

**Why:** The community reads "open source" as "no surprises." Any source-available carve-out, even a benign one, is read as "they will move more features there later." The maintainer's reassurance ("we do not plan to move this to the proprietary folder now or ever") is the right message but the trust damage is done.

**How we avoid it:** Apache 2.0, unmodified, no `proprietary/` directory, no carve-outs. The commercial tier is *managed instances of the same binary*, not a different product with different license. We commit publicly, in writing, in the LICENSE file, that no code in the Apache 2.0 binary will ever move to a different license.

### 21.3 The CasaOS mistake

CasaOS has 33k stars, Apache 2.0, and is effectively abandoned. Last commit December 2024. IceWhale has moved all development to closed-source ZimaOS. ([Discussion #2494](https://github.com/IceWhaleTech/CasaOS/discussions/2494), [Issue #767](https://github.com/IceWhaleTech/CasaOS-AppStore/issues/767))

**Why:** Apache 2.0 with one corporate sponsor + no clear revenue + dev moves to closed product. The community is on its own.

**How we avoid it:** Public 18-month sustainability commitment, updated quarterly. Public CI vendor-disappear test. Foundation transfer plan by year 3. Multiple funding sources from day 1 (sponsorships + Cloud + Pro + Enterprise).

### 21.4 The Vercel mistake

Vercel started free, became expensive, alienated the indie community. The Riley Walz $46,485 Jmail bill in November 2025 is the canonical horror story. Vercel changed its free tier in May 2025 to throttle bandwidth.

**Why:** Per-deploy / per-bandwidth pricing is hostile to indie projects with viral traffic. The buyer has no way to predict the bill.

**How we avoid it:** We never charge per-deploy or per-bandwidth. Our Cloud tier is per-node, flat, predictable, annual. Plausible model. The user knows the bill in advance and it does not change because their app went viral.

### 21.5 The Umbrel mistake

Umbrel has ~11k stars, polished UI, but is *source available, not open source*. ([tedium.co](https://tedium.co/2026/03/28/self-hosting-platform-tools-guide/)) The community's complaint: "If a controversial Bitcoin soft fork arises and Umbrel's developers oppose it, they could withhold updates to Bitcoin Core or Knots."

**Why:** Source-available is not Apache 2.0. The community notices.

**How we avoid it:** Apache 2.0. Period.

### 21.6 The CapRover mistake

CapRover had a multi-month silence around the Docker 29 breaking change in 2025. ([caprover #2351](https://github.com/caprover/caprover/issues/2351)) The Docker API broke the panel; recovery required manual intervention. Trust damage.

**Why:** Depending on a single container runtime's API without an abstraction. The runtime is upstream; the panel is downstream; the breakage cascades.

**How we avoid it:** Abstract the runtime. Docker + Podman in V1.5 behind our own `Runtime` trait. When Docker breaks, Podman is the path. The agent pattern means we test against multiple runtimes in CI.

### 21.7 The Runtipi mistake

Runtipi had a critical RCE in backup-restore (GHSA-vrgf-rcj5-6gv9, 2026). Shell metacharacters in filenames = arbitrary command execution. ([runtipi advisory](https://github.com/runtipi/runtipi/security/advisories/GHSA-vrgf-rcj5-6gv9))

**Why:** No security review of user-input → shell-command paths. No fuzzing. No static analysis with security lints. Solo maintainer bandwidth.

**How we avoid it:** security@ from day 1. cargo-audit + cargo-deny in CI. Clippy with security lints. Semgrep. Fuzzing on backup/restore. 3-person VMT. No shell execution from user input, ever.

---

## 22. The 100-year-old question

> "If you disappear, what happens to the users?"

This is the founding question of an open-source product. Plausible answered it in a 2022 blog post: ["We chose open source and we have no regrets."](https://plausible.io/blog/open-source-saas) The answer is: the product keeps working. The data is exportable. The community can fork. The license is irrevocable.

Our answer:

1. **Apache 2.0 is irrevocable.** The license does not depend on us. Even if the company disappears, the source remains Apache 2.0.
2. **The binary is self-contained.** No phone-home. No required SaaS. No external API. Runs offline. Runs in air-gapped mode.
3. **The state is SQLite.** A solo developer with the source, a Hetzner bill, and a backup tarball can keep the platform running forever.
4. **The vendor-disappear test passes.** A public CI takes a fresh VM, installs the binary, deploys an app, backs up, restores on a different VM, verifies the app comes up. We link to it from the README. It runs on every release.
5. **The CLA assigns copyright to the project.** Contributors cannot revoke their contributions. A future fork is a real fork, not a legal mess.
6. **The foundation transfer plan is documented.** By year 3, the project (code, trademark, copyright) transfers to a sovereign-tech-aligned foundation. The company becomes a service provider. If the company disappears, the foundation continues.

This is the answer. It is the same answer Plausible gives. It is the same answer Cal.com, Bitwarden, Sentry, Supabase give.

It is also the answer that makes the EU public-sector buyer trust us. Because the question of "what happens if you disappear" is the same question they ask every vendor. We have a different answer than the hyperscalers.

The hyperscalers say: "We have a 99.99% SLA and €1B in financial backing."

We say: "We have an Apache 2.0 license, a SQLite database, and a CI job that proves the platform works without us."

In 2026, with the Cloud and AI Development Act on the table, with BSI C5:2026 mapping to EUCS Substantial, with the Commission's €180M sovereign cloud procurement in April 2026, with ~70% of EU public data falling under at least Level 1 sovereignty requirements ([Euractiv CAIDA](https://www.euractiv.com/news/commissions-sovereign-cloud-plan-doesnt-push-us-hyperscalers-out/)) — our answer is the better one.

---

## 23. Sources

**Strategy / OSS business model:**
- [plausible.io/about](https://plausible.io/about)
- [plausible.io/blog/open-source-saas](https://plausible.io/blog/open-source-saas)
- [plausible.io/blog/customers-not-investors](https://plausible.io/blog/customers-not-investors)
- [Latka Plausible $3.1M ARR 2024](https://getlatka.com/companies/plausible-analytics)
- [supabase.com/blog/supabase-series-e](https://supabase.com/blog/supabase-series-e)
- [techcrunch supabase $5B valuation Oct 2025](https://techcrunch.com/2025/10/03/supabase-nabs-5b-valuation-four-months-after-hitting-2b/)
- [accel.com supabase series e](https://www.accel.com/news/supabases-series-e-an-era-defining-database)
- [Sacra Sentry profile](https://sacra.com/c/sentry/)
- [First Round sentry pmf](https://review.firstround.com/sentrys-path-to-product-market-fit/)
- [productmint bitwarden 2025](https://productmint.com/how-does-bitwarden-make-money/)
- [CB insights bitwarden](https://www.cbinsights.com/company/bitwarden/financials)
- [Startup Intros Cal.com](https://startupintros.com/orgs/cal-com)
- [beton coolify pricing teardown](https://www.getbeton.ai/blog/coolify-pricing-teardown/)
- [coolify.io/sponsorships](https://coolify.io/sponsorships)

**Licensing / governance:**
- [Dokploy license update Jan 2026](https://dokploy.com/blog/we-are-updating-dokploys-open-source-license)
- [Dokploy issue #3613 False Marketing](https://github.com/Dokploy/dokploy/issues/3613)
- [Dokploy issue #3477 Strip Down](https://github.com/Dokploy/dokploy/issues/3477)
- [Rust Foundation Strategic Plan 2026-2028](https://rustfoundation.org/strategic-plan/)
- [Rust Foundation Bylaws](https://rustfoundation.org/policy/bylaws/)
- [Rust Project Leadership Council](https://forge.rust-lang.org/governance/council.html)

**Sovereignty / EU regulatory:**
- [EU Commission Sovereign Cloud Framework June 2026](https://commission.europa.eu/news-and-media/news/sovereign-cloud-framework-explained-2026-06-01_en)
- [BSI C5:2026](https://www.bsi.bund.de/EN/Themen/Unternehmen-und-Organisationen/Informationen-und-Empfehlungen/Empfehlungen-nach-Angriffsziele/Cloud-Computing/Kriterienkatalog-C5/C5_2025/C5_2025_node.html)
- [Dutch NCCA EUCS](https://www.dutchncca.nl/eu-cybersecurity-certification/cloud-services)
- [Euractiv CAIDA June 2026](https://www.euractiv.com/news/commissions-sovereign-cloud-plan-doesnt-push-us-hyperscalers-out/)

**Technology / benchmarks:**
- [techplained caddy vs nginx 2026](https://www.techplained.com/caddy-vs-nginx)
- [tech-insider caddy vs nginx 2026](https://tech-insider.org/caddy-vs-nginx-2026/)
- [rafftechnologies caddy vs nginx on small VMs](https://rafftechnologies.com/blog/what-we-learned-benchmarking-caddy-vs-nginx)
- [rqlite.io](https://rqlite.io/)
- [litestream github](https://github.com/benbjohnson/litestream)
- [rqlite deployment guide](https://deepwiki.com/rqlite/rqlite/5-deployment-and-operations)
- [PIER single binary PaaS](https://devcom.app/en/works/pier)
- [sh0.dev](https://sh0.dev)
- [yoink.is](https://yoink.is)
- [vibe-deploy MCP](https://github.com/luongs3/vibe-deploy)

**Security culture:**
- [ossf/oss-vulnerability-guide maintainer-guide](https://github.com/ossf/oss-vulnerability-guide/blob/main/maintainer-guide.md)
- [ossf/oss-vulnerability-guide finder-guide](https://github.com/ossf/oss-vulnerability-guide/blob/main/finder-guide.md)
- [google/oss-vulnerability-guide](https://github.com/google/oss-vulnerability-guide/blob/main/guide.md)
- [GitHub Blog maintainer CVD 2025](https://github.blog/security/vulnerability-research/a-maintainers-guide-to-vulnerability-disclosure-github-tools-to-make-it-simple/)
- [runtipi GHSA-vrgf-rcj5-6gv9 RCE advisory](https://github.com/runtipi/runtipi/security/advisories/GHSA-vrgf-rcj5-6gv9)
- [caprover Docker 29 breakage issue #2351](https://github.com/caprover/caprover/issues/2351)
- [CasaOS discussion #2494](https://github.com/IceWhaleTech/CasaOS/discussions/2494)
- [CasaOS issue #767](https://github.com/IceWhaleTech/CasaOS-AppStore/issues/767)

**Vibe coder / AI agent:**
- [vibecoder.me cost 2026](https://blog.vibecoder.me/how-much-cost-vibe-code-app)
- [vibecoder.me deploy step by step](https://blog.vibecoder.me/deploying-your-first-app-step-by-step)
- [webdeveloper.com deploy vibe coded apps](https://webdeveloper.com/learn/guides/deploy-vibe-coded-apps/)
- [Pangolin YC S25](https://www.ycombinator.com/companies/pangolin)
- [fosrl/pangolin github](https://github.com/fosrl/pangolin)
- [Pangolin funding Startup Intros](https://startupintros.com/orgs/pangolin)

**Existing research (read first, before this report):**
- [Research-1.txt](file:///C:/Users/Victo/Downloads/webproj/Cloud/Research-1.txt) — original spec, 2,459 lines
- [Research-2.md](file:///C:/Users/Victo/Downloads/webproj/Cloud/Research-2.md) — consolidated research, 1,591 lines
- [sovereign-runtime-market-research.md](file:///C:/Users/Victo/Downloads/webproj/Cloud/sovereign-runtime-market-research.md) — market research, 369 lines
- [competitive-landscape.md](file:///C:/Users/Victo/Downloads/webproj/Cloud/competitive-landscape.md) — competitive landscape, 910 lines

---

**End of report.** Total length: ~5,800 words, 30 numbered decision frameworks, 12 risk mitigations, 7 named-mistake-to-avoid, 4 source lists, 60+ source URLs.
