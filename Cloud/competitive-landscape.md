# Competitive Landscape: Sovereign Application Runtime (2026)

**Research date:** June 2026
**Product being researched:** A Rust-based, self-hosted, single-binary, CLI/TUI/API-first deployment platform for solo developers and startups. Explicitly *not* Kubernetes, designed as a credible alternative to Coolify, Dokploy, CapRover, Dokku, Portainer, YunoHost, CasaOS, Umbrel, Runtipi, Pangolin.

This report is intentionally brutal. The goal is to find positioning gaps, not to flatter incumbents.

---

## 0. TL;DR — Strategic Findings

1. **The market is crowded and bifurcating.** The "self-hosted PaaS" space has split into two camps: **batteries-included web-panel platforms** (Coolify, Dokploy, CapRover) and **personal-cloud / homelab app stores** (CasaOS, Umbrel, Runtipi, YunoHost, Cosmos). Almost nothing lives in the **CLI/TUI/API-first, single-binary, low-resource, no-web-PaaS** niche — that is the open ground.
2. **"Web panel + Docker" is the default and that default has well-known failure modes.** Real users consistently complain about: heavy RAM footprint (Coolify 0.8–1.2 GB, Dokploy 0.6 GB, CapRover 400 MB, even for a panel they only use to *deploy apps*); upgrade churn ("WordPress-plugin-maintenance feel"); the panel itself becoming an attack surface; zero-downtime deploys that drop in-flight requests; the management database getting wedged (Dokploy issue #4461); and Docker's breaking-API changes breaking the panel (CapRover 1.13–1.14 fiasco).
3. **The Rust single-binary "shoehorn" is real and validated.** Three independent new entrants — **PIER** (20–40 MB RAM, AGPL-3.0), **sh0** (~50 MB, single binary, claims 230+ endpoints), and **yoink** (12 stars but articulated as "between Kamal and K8s") — have appeared in 2025–2026 explicitly attacking the Coolify/Dokploy resource footprint. The fact that they exist *and* publish foot-print comparison tables tells you the pain is felt.
4. **"Source-available but not open source" is a real, quantified competitive lever.** Dokploy's mixed license (Apache 2.0 core + restricted templates/multi-server/previews) is consistently called out as a reason to pick Coolify (e.g., Shubh on Medium, LogRocket blog). A Rust tool that ships *truly* permissive licensing (MIT or Apache 2.0, no feature carve-outs) can use this as a wedge.
5. **"No GUI required, but a TUI is fine" is an underserved quadrant.** yoink's "CLI + TUI, k9s-style dashboard, no web app, no database, no agent" pitch is essentially the same wedge as the new product. But yoink has 12 stars. This is unclaimed territory with a clear narrative.
6. **The solo developer is being underserved by everyone.** Coolify's UI is "overwhelming at first" (multiple reviewers), Dokploy "assumes you have a PhD in Container Orchestration" (Medium review), CapRover needs Docker Swarm, Dokku has no UI, Portainer isn't a PaaS. A product that is honest about being for 1–50 servers and one operator deserves a market.
7. **Sustainability is a real concern.** CasaOS is effectively abandoned (last meaningful commit Dec 2024, IceWhale has moved dev to closed-source ZimaOS — see IceWhale community thread and discussion #2494). CapRover had a multi-month silence around the Docker 29 breaking change. Even Coolify has an upgrade-migration tax. A solo founder's #1 question is "will this be alive in 18 months."
8. **Bus factor matters.** Coolify is 56k stars but the core is still 1–2 maintainers (Andras Bacsai / @peaklabs-dev). Dokploy is 1–2 maintainers. YunoHost is volunteer, EU-grant funded. CapRover is essentially githubsaturn. The market is willing to pay for a project with credible sustainability.

---

## 1. Market Map

| Category | Players | What it is |
|---|---|---|
| **Self-hosted PaaS, web-panel, Docker-based** | Coolify, Dokploy, CapRover | "Self-hosted Heroku/Vercel" — full web UI, Git deploy, DBs, SSL, multi-server |
| **Single-server PaaS, terminal-first** | Dokku | "Heroku on a single VPS" — no GUI core, plugin-based |
| **Container management UI** | Portainer, Dockge, Yacht | Docker/Swarm/K8s management GUI, not really an app PaaS |
| **Personal cloud / homelab app stores** | CasaOS, Umbrel, Runtipi, YunoHost, Cosmos Cloud, StartOS | Friendly UI for installing self-hosted consumer apps |
| **Zero-trust remote access / tunneling** | Pangolin, Cloudflare Tunnel, Tailscale Funnel | Reverse proxy + identity layer; not a PaaS but adjacent |
| **Rust single-binary deploy tools (new wave)** | yoink, PIER, sh0, Uncloud, Komodo | Thin CLI/TUI/API deploy layers, often explicitly compared to Coolify |

The new product sits in the bottom-right cell that no one owns well: **Rust single-binary PaaS for solo devs, no web panel**.

---

## 2. Coolify (coolify.io)

- **Website:** https://coolify.io
- **GitHub:** https://github.com/coollabsio/coolify
- **Repo (as of June 2026):** 56,330 stars, 4,690 forks, 420 contributors, 781 open issues
- **License:** Apache 2.0 (truly open — "MIT" per some sources, Apache 2.0 per GitHub)
- **Tech stack:** PHP (Laravel + Livewire), Svelte/Svelte 5 frontend, MariaDB/PostgreSQL, Redis, Soketi (WebSockets), Traefik or Caddy, Docker, Nixpacks, Docker Swarm (experimental)
- **Target user:** Indie devs, freelancers, small teams hosting 1–10 services, agencies running per-client infra
- **Pricing:** Free self-hosted (all features), Coolify Cloud $5/mo, "Pro" $5–350/mo paid tiers for GitHub OAuth, team features, white-label. The Cloud and Pro versions are positioned as supporter tiers; the self-hosted free build has no paywalled features.
- **v4.0.0 launched May 18, 2026** (per ByteGrad coverage). v4 added native multi-server orchestration, redesigned dashboard, 280+ one-click services (up from ~200 in v3), Ollama as first-class, and the **Coolify MCP Server** — first self-hosted PaaS with native AI deployment control via Claude Code/Cursor/Cline.

### Core features
- 280+ one-click service templates (Plausible, Umami, n8n, Ghost, MinIO, Supabase, KeyDB, DragonflyDB, etc.)
- Git push auto-deploy from GitHub, GitLab, Bitbucket, Gitea
- Nixpacks + Dockerfile + Docker Compose build paths
- Automatic Let's Encrypt with wildcard support (Traefik or Caddy)
- Database one-click provisioning (Postgres, MySQL, MariaDB, MongoDB, Redis, ClickHouse, Dragonfly, KeyDB)
- S3-compatible scheduled backups + one-click restore
- Real-time logs, terminal in browser
- Multi-server via SSH, Docker Swarm (marked experimental)
- Pull-request preview deployments
- Team roles / permissions
- MCP server for AI agents (v4.0)
- Webhooks, API, 20,000+ Discord members
- Notifications: Discord, Telegram, Slack, email

### Strengths
- **Mature, by far the largest community.** The de-facto answer to "self-hosted Vercel." Almost every search "self-hosted PaaS 2026" returns Coolify first.
- **Truly open license (Apache 2.0) with no feature carve-outs.** Reviewers consistently cite this as a reason to pick it over Dokploy.
- **Feature velocity.** MCP server, redesigned dashboard, 280+ templates, multi-server in v4 — all in one release.
- **No paywall on self-hosted.** $0 gets every feature.
- **Strong reverse-proxy story** (Traefik + Caddy, automatic wildcard certs).
- **Battle-tested.** "The reliability track record is unmatched" — but that's actually more true of Dokku (see §4).

### Weaknesses (from real user complaints)
- **Operational burden / "WordPress-plugin-maintenance feel."** *Fatih Yildiz, after a year on Coolify:* "Coolify sends upgrade notifications, which is good. I do want to know when things need attention. But after a while it starts to feel a bit like WordPress plugin maintenance. There are just too many moving parts, too many dependency upgrades, and too many moments where you wonder whether this week's update is the one that breaks something important." Source: https://mfyz.com/my-coolify-experience-after-a-year/
- **Heavy footprint.** 0.8–1.2 GB idle RAM, six+ containers (Laravel app, PostgreSQL, Redis, Soketi, Horizon, Traefik). On a 2 GB / $5 VPS, you've burned 30–60% of your memory before deploying anything. Multiple reviewers say "needs at least 4 GB."
- **Zero-downtime deploys drop in-flight requests.** *Autonoma review:* "Zero-downtime deploys still kill pending requests during container swaps. The new container comes up healthy and the old one gets stopped, but in-flight requests on the old container can drop." (https://getautonoma.com/blog/coolify-vs-vercel)
- **UI friction and "magic variables" that don't work.** Hacker News thread, https://news.ycombinator.com/item?id=43589794: *"Added 8 variables inside docker-compose, only 7 get recognized. Why my docker-compose works locally and not on Coolify? Oh yeah it has its own network stuff. Error messages like 'Oops something is not okay, are you okay?' 0% helpful information, 100% condescending crap."*
- **Invasive upgrades with schema migrations.** *dev.to:* "Coolify v4 upgrades are the most invasive — schema migrations on every minor version, occasional breaking config changes during the v4 stabilization period. We're past the worst of it as of 2026 but not all the way clear. Their changelog is honest about it, and rollback works, but plan upgrades on a weekday, not Friday afternoon." (https://dev.to/pickuma/coolify-vs-dokku-vs-caprover-self-host-paas-compared-in-2026-b0d)
- **Observability is "okay."** Same review: "I think Coolify's observability is okay. Not bad. Not amazing. Just okay... So yes, Coolify gives you a usable operational view and log drain routing. But if you want something robust and transparent, you'll probably end up running more tools beside it."
- **Security: Docker port publishing bypasses host firewall.** *ceaksan.com:* "In my tested Ubuntu 24.04 + nftables setup, Docker's port publishing bypassed every host-level firewall rule I tried, I received a government warning for an exposed PostgreSQL port, faced SSL certificate issues, and was forced to migrate to managed PostgreSQL." (https://ceaksan.com/en/hetzner-coolify-self-hosting-reality)
- **Critical infrastructure concerns.** *mfyz.com:* "My central Postgres instance is the clearest example. It's not that Coolify can't run it. I just didn't want that layer of abstraction for something so central and shared."
- **SSH-to-itself problem.** *Medium (Shubh):* "I kept running into an issue where Coolify couldn't actually set up the local server for deployments because it couldn't connect to itself via SSH. This was caused by my firewall configuration denying public SSH access." (https://medium.com/@shubhthewriter/coolify-vs-dokploy-why-i-chose-dokploy-for-vps-deployment-in-2026-ea935c2fe9b5)
- **CVE-patch responsibility lands on you.** A 2026 advisory (CVE-2026-31431) required user patching.
- **Build resource contention.** Coolify builds run on the same server as your apps; large builds can OOM the box.

### What it does NOT do
- No native Kubernetes (roadmap).
- No "declarative GitOps" config-as-code. State lives in the panel's database. (An enhancement request for "Coolify as Code" exists.)
- No multi-region failover.
- No first-class secrets manager beyond env vars (you can integrate with Vault/Infisical).
- No high availability on the Coolify control plane itself (you run a single instance).

### Recent updates (2025–2026)
- v4.0.0 — May 18, 2026: multi-server orchestration, MCP server, redesigned dashboard, 280+ services, Ollama.
- Open issues: 781 (per GitHub). Active but a meaningful backlog.

---

## 3. Dokploy (dokploy.com)

- **Website:** https://dokploy.com
- **GitHub:** https://github.com/Dokploy/dokploy
- **Repo (June 2026):** 34,444 stars, 2,566 forks, 310 contributors, 581 open issues
- **License:** Mixed — Apache 2.0 core, but **source-available for some features** (templates, multi-node, preview deployments) with resale restrictions. This is *the* recurring criticism.
- **Tech stack:** TypeScript (99.1%), small amount of Go, Docker, Docker Swarm (native, not experimental), Traefik, BullMQ, PostgreSQL, Redis
- **Target user:** Solo devs and small teams (1–5 developers, 1–10 services) who want a lighter footprint and cleaner UI than Coolify
- **Pricing:** Free self-hosted with all features, Dokploy Cloud $4.50/mo + $3.50 per additional server
- **Launch:** 2024; 26k stars in its first year (fastest-growing self-hosted tool in DevOps)

### Core features
- Git deploy (GitHub, GitLab, Bitbucket, Gitea)
- Nixpacks + Dockerfile + Docker Compose + Heroku/Cloud-Native Buildpacks
- Multi-server via Docker Swarm (native, not experimental)
- Postgres, MySQL, MariaDB, MongoDB, Redis one-click
- Volume backups to S3 (broader than Coolify — backs up SQLite, uploads, etc., not just DBs)
- Scheduled tasks at app, compose-service, AND host level
- Built-in monitoring UI (CPU/mem/disk/net)
- Notifications via Gotify
- Organizations for multi-tenancy
- AI-powered Docker Compose templates
- Custom build server (separate build from deploy)
- Environments: staging/prod/dev within a project (v0.25+)
- Volume backups (Coolify does not have this)
- Traefik advanced UI
- Rollback
- "Cloudflare Tunnels" support

### Strengths
- **Cleaner, faster UI.** Multiple reviewers call it out (Contabo, servercompass, use-apify, LogRocket).
- **Lighter resource footprint.** ~600 MB idle vs Coolify's ~800 MB. Real reviewers on $5 VPSes say this matters.
- **Native Docker Swarm everywhere.** Even single-node deployments use Swarm, so "scale to multi-node" is a real, not experimental, story.
- **Closer to raw Docker.** Multi-service Docker Compose files behave the same in production as locally — fewer magic abstractions.
- **Volume backups.** Backs up anything on a persistent volume, not just databases.
- **Host-level cron + Traefik advanced config + custom build server** are all first-class.
- **Faster install than Coolify** (per LogRocket, 21 minutes faster in his test).
- **Dokploy maintainers are responsive** (one of the most-clicked features in reviews).

### Weaknesses
- **License is not actually open source.** *Shubh on Medium:* "Dokploy is more complicated; while the core is Apache 2, code for key features like templates, multi-node support, and preview deployments is exempt and cannot be sold or offered as a service without consent. Dokploy is effectively 'source available' rather than fully open source. For many, this will be the biggest reason to choose Coolify."
- **Hacker News:** *"Dokploy is not open-source. Broken license."* (https://news.ycombinator.com/item?id=43589794)
- **Smaller community.** 34k stars vs Coolify's 56k, fewer templates (50–100 vs 280+), smaller Discord, fewer tutorials.
- **"Stuck deployment" bug — there is no working "Cancel" path.** *Dokploy issue #4461 (May 2026):* "After a build is killed because it exhausted host resources, the deployment stays 'running' indefinitely. The BullMQ job remains active+locked and is never marked stalled/failed. There is no working 'Cancel' path in the UI, so the queue is wedged until the server is rebooted or Redis/Postgres are edited by hand. New deploys queue behind the stuck one." Manual workaround requires `UPDATE deployment SET status='error'...` and Redis key surgery. (https://github.com/Dokploy/dokploy/issues/4461)
- **No built-in preview environments per PR for Docker Compose** (was harder to set up than Coolify in LogRocket's test).
- **Magic-variables confusion (also Coolify):** *"Dokploy also claims to help with AI during deployment. I added an AI provider, but I could not find where that AI help shows up in the actual deployment flow."* (LogRocket)
- **Auto-deploy inconsistency.** *Medium (Koome):* "Dokploy's auto-deployment feature, which I can only describe as that friend who says 'I'm 5 minutes away' but is actually still in bed. It worked… sometimes. Other times, it just decided to take a vacation." (https://medium.com/@koome_68088/the-great-deployment-showdown-dokploy-vs-coolify-a-developers-comedy-of-errors-c9f4fbe379aa)
- **Log noise, not signal.** Same: "They'll show you some logs. The general Docker logs. But when your frontend throws an error because you forgot to add await somewhere? Good luck finding that in the logs."
- **No built-in DNS manager.** *Issue #4376 (May 2026):* "Managing DNS records is a completely separate workflow I have to jump over to my DNS provider's dashboard, navigate to the right zone, and manually create or update records. It breaks the flow."
- **No GitOps / declarative config-as-code.** *Issue #3872 (March 2026):* "It would be great to manage Dokploy resources (apps, env vars, domains, build settings) through a declarative config file (e.g. dokploy.yaml) living in a Git repo, not just through the UI. The UI-first model works well for getting started, but lacks version history, PR-based review, environment reproducibility."
- **No migration path from Coolify.** *Issue #3098:* a user wanted one-click migration; maintainer replied: *"Honestly, the amount of work required makes it pretty much unfeasible."*
- **Dokploy is opinionated about Docker Swarm.** If you want plain Compose on multiple hosts with no Swarm, this is friction.

### What it does NOT do
- No Kubernetes (roadmap).
- No GitOps YAML in the main UI.
- No first-class observability beyond basic metrics (no log aggregation, no traces).
- No SSO/SAML.

---

## 4. CapRover (caprover.com)

- **Website:** https://caprover.com
- **GitHub:** https://github.com/caprover/caprover
- **Repo (Jan 2026):** 14,950 stars, 970 forks, 70 contributors, 176 open issues
- **License:** Other (NOASSERTION — a mix of Apache 2.0 and other terms; check carefully)
- **Tech stack:** TypeScript (95.6%), Node.js, Docker Swarm, Nginx, custom Captain CLI
- **Target user:** Developers wanting "Heroku on a VPS" with a GUI but lighter than Coolify
- **First release:** 2017 (originally CaptainDuckDuck)

### Core features
- Web dashboard with one-click app store (~100 apps)
- Native Docker Swarm (multi-node, rolling deploys)
- Nginx reverse proxy + automatic Let's Encrypt
- Custom domains + free `*.captain.xyz` subdomains
- HTTPS, HTTP/3
- Custom ports (1.14.0+)
- Custom Certbot commands + DNS-01 challenges
- App log search (regex)
- Disk cleanup automation
- Multiple-app delete
- GoAccess web analytics built-in (1.14+)

### Strengths
- **Smallest GUI-driven PaaS footprint after Dokploy.** ~250–400 MB idle. *dev.to:* "Memory baseline is around 400MB. The one-click app catalog is the strongest selling point — adding a Plausible instance is genuinely two clicks."
- **Mature on Docker Swarm** for multi-node from day one.
- **Single most-clickable "set up a self-hosted service" experience.** Add Plausible = 2 clicks.
- **Long history (2017).** Battle-tested.

### Weaknesses
- **CapRover upgrades have historically gone through the dashboard but Docker / Swarm upgrades can wedge the box.** *GitHub Discussion #2342:* "My captain/captain was restarting all the time until I found through `docker service logs captain-captain --since 60m` that my docker needed to be upgraded... After upgrading docker I have not been able to get `captain-captain` running again — I keep getting 'This node is already part of a swarm.'" Recovery required running the captain service create command by hand from a maintainer comment.
- **Docker API breaking change (Nov 2025 — Docker 29).** *Issue #2351:* "This version includes a breaking change to stop supporting v1.43 of Docker API that CapRover uses. Any CapRover instances before 1.14.1 will break once Docker is updated. It is strongly recommended to upgrade your CapRover instances immediately to 1.14.1+." (https://github.com/caprover/caprover/issues/2351) This is exactly the kind of event that loses user trust.
- **No app-level CPU/memory metrics.** *Discussion #2342 user:* "It makes me really want to have a feature in caprover that could show the memory/cpu usage of each app. That would be really great. We know about NetData integration but that's a bit advanced for our use case. We just want to see very simple metrics per app."
- **Docker Swarm's deprecation cloud.** "Watch the Swarm caveat: if a node goes down mid-deploy, Swarm's recovery is more fragile than Compose's restart loops. Bind-mount volumes also don't move between nodes, which surprises first-time multi-node users." (dev.to)
- **CapRover v1.13.x was released Oct 2024 and v1.14.0 in June 2025** — release cadence has slowed compared to Coolify/Dokploy.
- **No GitOps / no API-first.**
- **No native multi-server UI; the swarm overlay network *is* your multi-server.**
- **Documentation thinner than Coolify**, especially for advanced networking.
- **Linode / OVH have specific issues** with vxlan / overlay networks (maintainer comments in issues).

### What it does NOT do
- No Kubernetes.
- No built-in S3 backup scheduler (plugin-based).
- No Postgres/Redis as one-click *services* (one-click *apps* exist, but they're not managed).
- No DNS manager.
- No SSO/SAML.
- No declarative config-as-code.

---

## 5. Dokku (dokku.com)

- **Website:** https://dokku.com
- **GitHub:** https://github.com/dokku/dokku
- **Stars:** ~30k
- **License:** MIT
- **Tech stack:** Bash, Docker, Buildpacks, Nginx, Heroku-style plugins
- **Age:** 2013 (oldest in the space)
- **Target user:** Engineers comfortable in a terminal, one server, Heroku muscle memory

### Core features
- `git push dokku main` deploys
- Heroku-style buildpacks + Dockerfile + Cloud Native Buildpacks
- Plugin ecosystem: `dokku-postgres`, `dokku-redis`, `dokku-mongo`, `dokku-letsencrypt`, etc.
- Nginx reverse proxy with automatic Let's Encrypt (via plugin)
- Zero-downtime deploys (via plugin)
- Process scaling (`ps:scale`)
- App config via env vars on host

### Strengths
- **Smallest moving-parts footprint of any PaaS.** ~120–150 MB idle. *dev.to:* "Memory baseline is around 150MB."
- **Decade of stability.** *jacar.es:* "Twelve years later, with entire ecosystems swinging between boom and decline and Kubernetes established as the large-scale infrastructure standard, Dokku is still alive, actively maintained and surprisingly relevant for a niche other solutions don't serve well."
- **Heroku compatibility is genuine.** Apps that ran on Heroku run on Dokku unchanged — a big advantage after Heroku killed its free tier in 2022.
- **Plugin system is mature.** Stack Overflow answers exist for nearly every problem.
- **Mit license, truly open.**

### Weaknesses
- **No web UI in core.** *dev.to:* "If you can't ssh, you can't operate Dokku."
- **Single-server only.** No native multi-host.
- **"Configuration and secrets are stored as environment variables on the host. It is straightforward, but rotation, auditing, and isolation are mostly manual, and a compromise of the machine exposes everything running on it."** (UpCloud, 2026)
- **All apps share a kernel** — multi-tenancy is poor.
- **Documentation can be sparse for edge cases** (ProPicked review: "Documentation can be sparse for edge cases").
- **Smaller ecosystem than Coolify** for new frameworks.
- **No preview environments natively** — requires community recipe.
- **Plugin version bumps can be tricky** (Postgres plugin volume layout changes between major versions).

### What it does NOT do
- No multi-server, no HA, no clustering.
- No web GUI.
- No managed database observability.
- No secrets manager (env vars on host).
- No built-in monitoring beyond `docker logs`.
- No preview environments out of the box.
- No SSO.

---

## 6. Portainer (portainer.io)

- **Website:** https://portainer.io
- **GitHub:** https://github.com/portainer/portainer (~36.9k stars)
- **License:** Zlib (Community Edition) / Commercial (Business Edition)
- **Tech stack:** Go, Angular, Docker
- **Target user:** Originally "Docker GUI for everyone," now spans homelab to Volkswagen factories

### Core features (CE)
- Manage Docker Standalone, Swarm, Kubernetes, ACI from one UI
- Stacks (Compose), container lifecycle, image mgmt, volumes, networks
- Real-time logs, exec into container
- App Templates library (community-maintained)
- Git-based stack auto-deploy (basic)
- Edge Agent for remote nodes
- ~50 MB RAM at idle
- One-click Portainer on DO marketplace

### Business Edition adds
- RBAC (Environment Admin, Operator, Helpdesk, etc.)
- Active Directory / LDAP / OIDC
- Audit logs (with SIEM/Syslog export)
- GitOps reconciler
- Edge Compute (Edge Groups, mTLS, pre-pull)
- Kubernetes cluster provisioning (Civo, Linode, DO, GCP, AWS, Azure)
- Resource quotas, registry management, OCI registry
- SLA / 9x5 / 24x7 support
- **"Take3" — 3 free Business nodes, permanently.**

### Strengths
- **Best Docker UI in the world.** *rodak.pro:* "Portainer CE is the default Docker management interface for homelab self-hosters, and for good reason."
- **Mature, enterprise-trusted.** P&G, Volkswagen, healthcare deployments.
- **Genuinely free CE with no commercial strings.** Zlib, no node limits.
- **Lightweight (50–100 MB RAM).**
- **Works on Pi to production.**

### Weaknesses
- **Not a PaaS.** It's a container management UI. No git-push deploy, no DB templates, no PaaS workflow.
- **"The features that matter for teams (RBAC, SSO/LDAP/OIDC, audit logs) are locked behind the Business Edition, pricing for which is not public."** (unsubbed.co)
- **Pricing opacity for BE.** "Out of three free Take3 nodes, you're in a sales conversation without knowing the number." (unsubbed.co)
- **No GitOps reconciler in CE.** "No SSO, no LDAP, no RBAC, no audit logs" in CE.
- **K8s management in CE is shallow** — "not a replacement for kubectl or Helm."
- **Stacks live in the database, not as files on disk.** "Backup story is 'back up the volume.'" (OSSAlt)
- **Heaviest of the homelab alternatives** (150 MB vs Dockge 30 MB vs Komodo 100 MB).
- **UI is dense** — "new users need a guided tour."

### What it does NOT do
- No git-push-deploy from a connected repo to a "production" app in the Heroku sense.
- No one-click database provisioning with backups.
- No PaaS-style environments / previews.
- No built-in DNS / SSL workflow for app domains.
- No SOPS-style secrets manager.
- No single-binary install (Go binary + database).

---

## 7. YunoHost (yunohost.org)

- **Website:** https://yunohost.org
- **License:** AGPL-3.0
- **Tech stack:** Debian distribution, Python, Nginx, Postfix, Dovecot, LDAP, SSOwat
- **Target user:** Families, small communities, associations, NGOs, students, non-technical self-hosters
- **Started:** 2012 (one of the oldest)

### Core features
- 500+ packaged apps in catalog (Nextcloud, Matrix, Mastodon, WordPress, Jitsi, Etherpad…)
- Webadmin + CLI
- Built-in email stack (Postfix + Dovecot + DKIM/SPF/DMARC)
- LDAP-based user directory
- SSO across all installed apps (SSOwat)
- Domain management + Let's Encrypt
- DynDNS (free `.nohost.me` subdomains for home servers)
- Backup system (CLI + web)
- Fail2ban, firewall, 2FA, automatic OS + app updates
- No Docker — apps run as Debian packages under dedicated system users

### Strengths
- **The most accessible entry point to self-hosting.** *unsubbed.co:* "YunoHost is the most accessible entry point into self-hosting for people who aren't sysadmins. It handles the parts that kill non-technical users — SSL certificates, DNS setup, SSO across all apps, email server configuration, backups — without requiring command-line fluency."
- **Email is a first-class citizen** (most competitors treat email as out of scope).
- **No Docker, no resource tax.** Apps run as native services.
- **Mature, volunteer-run, EU-grant funded.** No VC, no exit, no pricing surprises.
- **SSO that actually works** out of the box.

### Weaknesses
- **Not Docker-based — apps must be in the YunoHost catalog.** *doc.yunohost.org:* "YunoHost does not use 'hard' containerization technologies such as Docker, for its apps. This is partly for historical reasons and partly to keep the system lightweight." You can't `docker run` whatever you want.
- **No strict isolation between apps.** "Under the hood, all applications share the same system and environment."
- **Not designed to scale.** *doc.yunohost.org:* "Some technical adjustments may be necessary when reaching 250~500 user accounts, or about 50 simultaneous users on resource-intensive apps."
- **App catalog quality varies.** Community-maintained.
- **Email deliverability is still a problem** for residential IPs.
- **No git-push-deploy, no CI/CD, no PaaS workflow** — YunoHost is *not* in the new product's lane, but it's what a non-developer self-hoster picks.

### What it does NOT do
- No Docker.
- No git deploys.
- No multi-server.
- No Kubernetes.
- No horizontal scaling.

---

## 8. CasaOS (casaos.io) — effectively abandoned

- **Website:** https://casaos.io
- **GitHub:** https://github.com/IceWhaleTech/CasaOS
- **Stars:** 33,410 (per unsubbed.co, May 2026)
- **License:** Apache 2.0
- **Tech stack:** Go, Docker, ZimaOS integration
- **Maintainer:** IceWhale Technology
- **Status:** **Effectively abandoned.** Last commit on the repo: ~9 months ago. IceWhale has moved all development to ZimaOS, which is a closed-source commercial NAS OS.

### Core features
- Friendly personal-cloud UI on top of Docker
- One-click app store (huge)
- File manager
- Mobile-friendly
- Runs on Pi, NUC, repurposed laptop
- Built-in user store, app permissions
- ZimaOS upgrade path (now the company's commercial direction)

### Strengths
- **Slickest UI in the homelab category.**
- **Largest homelab app store.**
- **Single shell-script install.**
- **Free.**

### Weaknesses (per multiple reviewers)
- **No built-in reverse proxy with HTTPS** (in current versions). You bring your own Caddy / Nginx / Traefik.
- **No built-in auth gateway** — apps' own auth is what you get. Tailscale is the typical workaround.
- **No built-in backup.**
- **App store apps are community-maintained; quality varies.**
- **Customization is limited.** If you want a service not in the app store, you fall back to docker-compose by hand.
- **"It starts to feel tighter later, once your setup needs deeper routing or tighter control."** (cloudzy.com)
- **Effectively abandoned.** *IceWhale community forum, Feb 2025:* "There haven't been any updates since Dec'2024 and I don't think any of the GitHub issues are being worked on." Maintainer essentially confirmed: "Icewhale have announced the next hardware project will be a ZimaBoard 2... I would guess that OS is going to get a major upgrade and tweaks when ZimaBlade two is released." (https://community.zimaspace.com/t/has-casaos-been-abandoned/4606)
- *GitHub Discussion #2494 (April 2026):* "They've moved on, to the closed source ZIMA OS. You can use that, or you can be like me." (https://github.com/IceWhaleTech/CasaOS/discussions/2494)
- *Issue #767 (Aug 2025):* "Can we just declare that CasaOS has been abandoned? Hi guys, no news since december, nothing new anywhere. Probably things are going well in ZimaOS and the development have been moved there." (https://github.com/IceWhaleTech/CasaOS-AppStore/issues/767)

### What it does NOT do
- No reverse proxy / HTTPS.
- No built-in auth.
- No built-in backup.
- No git deploy.
- No multi-user proper RBAC.
- No Docker Swarm / multi-node.
- No enterprise features (no SSO, no audit).

---

## 9. Umbrel / umbrelOS (umbrel.com)

- **Website:** https://umbrel.com
- **GitHub:** https://github.com/getumbrel/umbrel (~11k stars)
- **License:** **Source Available, NOT open source** (Umbrel has shifted away from open source — this is a real complaint)
- **Tech stack:** TypeScript, Debian, Docker
- **umbrelOS 1.7** released April 2026
- **Target user:** Originally Bitcoin node runners, now a polished general home-server OS

### Core features
- 300+ app store (Nextcloud, Immich, Jellyfin, Home Assistant, Vaultwarden, Bitcoin/Lightning, Ollama, AdGuard Home)
- Auto-updates
- Widgets on home screen
- "Rewind" — point-in-time file restore
- Network mounts
- External USB storage
- GPU acceleration for apps
- Runs on Pi 4/5, AMD64, VMs
- Commercial hardware: Umbrel Home ($599-ish), Umbrel Pro

### Strengths
- **Polished, Mac-like UI.** The slickest in the personal-cloud category.
- **Bitcoin-grade security defaults** (out of the box).
- **One of the best curated app stores.**
- **Hardware support is excellent** (Pi 4/5, AMD64, NUCs).

### Weaknesses
- **Hard-coded environment variables / Docker Compose edits get reset.** *Tedium (2026 review):* "Why can't I change the Docker Compose files without the app resetting that file every time there's a new version? (What's the point of even having Docker Compose if you're just going to delete my tweaks?) Why is it so hard to give this thing https support?" (https://tedium.co/2026/03/28/self-hosting-platform-tools-guide/)
- **No built-in HTTPS by default.** The Tedium reviewer spent hours figuring it out.
- **No native multi-user with proper isolation.**
- **Not actually open source.** *Blockdyor review:* "UmbrelOS is no longer open source; it shifted to a Source Available model a few years ago... If a controversial Bitcoin soft fork arises and Umbrel's developers oppose it, they could withhold updates to Bitcoin Core or Knots. This centralization gives Umbrel disproportionate control, leaving users dependent on their decisions and potentially limiting innovation or community-driven changes."
- **App behavior is hard to debug.** Solidtime (a Laravel app in the store) "sends a verification email to confirm the change. However, I didn't have an email set up—and there was no easy way to do so in the interface—so I was unable to log back in." Grade: D.
- **Installing OpenHands broke Umbrel's networking entirely for the reviewer.** 45 minutes of troubleshooting, landed on unanswered support threads. Grade: F.
- **Originally Pi-focused; VPS support is unofficial / best-effort.**
- **No git-push-deploy, no PaaS-style CI/CD.**

### What it does NOT do
- No Docker Compose file preservation across updates (in many cases).
- No git deploys.
- No reverse proxy / HTTPS first-class.
- No multi-tenant auth.
- No multi-server orchestration.
- Not open source.

---

## 10. Cosmos Cloud (cosmos-cloud.io)

- **Website:** https://cosmos-cloud.io
- **GitHub:** https://github.com/azukaar/cosmos-server (~5,800 stars, 3 years old)
- **License:** AGPL-3.0 (with paid Cosmos Cloud tier for hosted users)
- **Tech stack:** TypeScript, Go, JavaScript, Bash
- **Target user:** Self-hosters who want a security-first Docker platform with built-in reverse proxy, SSO, and monitoring
- **Active:** Yes, last commit 9 days ago (per openalternative.co)

### Core features
- Built-in reverse proxy with automatic HTTPS
- OIDC / SSO built-in (incl. 2FA / WebAuthn)
- VPN layer (Constellation)
- Smart Shield intrusion detection / WAF
- Docker container management
- Modern web UI
- App marketplace (smaller, growing)
- ~500 MB – 1 GB RAM idle

### Strengths
- **Security-by-default.** 2FA admin, intrusion detection, gated apps.
- **One interface for "run + expose + auth."** *cloudzy.com:* "Cosmos Cloud has the clearest built-in features here. The docs say its reverse proxy can expose apps without the old port-sprawl habit, support HTTPS, and move you from raw port numbers to subdomains."
- **Modern UI** (post-CasaOS, learned from its UX).
- **Active development.**
- **AGPL-3.0 + paid hosted tier** (clear monetization path).

### Weaknesses
- **Smaller community** than CasaOS / YunoHost.
- **Smaller app marketplace.**
- **Younger.** Edge cases not all smoothed over.
- **Lock-in** if you go all-in: "Cosmos manages networking + auth + reverse proxy; backing out means rebuilding those layers separately." (bigiron.cc)
- **Not a PaaS** in the git-push-deploy sense.
- **No git-push-deploy** workflow.

### What it does NOT do
- No git deploys.
- No multi-server orchestration.
- No PaaS database provisioning.
- No Kubernetes.

---

## 11. Runtipi (runtipi.io)

- **Website:** https://runtipi.io
- **GitHub:** https://github.com/runtipi/runtipi — 9,344 stars, 348 forks, 50 contributors, 71 open issues
- **License:** GPL-3.0
- **Tech stack:** TypeScript, Docker, Traefik
- **Latest release:** v4.9.3 (April 2026), v4.8.2 (April 2026)
- **Target user:** People moving past beginner tools; "I learned a bit, now I want more"
- **Started:** 2022

### Core features
- ~300 one-click apps (Nextcloud, Jellyfin, Vaultwarden, Immich, Paperless, etc.)
- Traefik reverse proxy with automatic Let's Encrypt
- Custom apps via JSON + docker-compose
- Built-in backup CLI script (cron-based, since v4.0)
- One-click updates
- Per-app configuration via dashboard
- Compose file editing (changes persist across updates)

### Strengths
- **"Best kept secret in self-hosting."** *appselfhost.com:* "Runtipi is one of the best-kept secrets in the self-hosting world — powerful enough to manage a full suite of services, yet approachable enough for newcomers. With Docker handling the isolation and Runtipi handling the orchestration, you get a home server setup that's reproducible, maintainable, and genuinely fun to use."
- **Docker Compose file editing survives updates** (which is what Umbrel doesn't do).
- **Simpler than Coolify/Dokploy** for app-store-style usage.
- **GPL-3.0, no commercial strings.**
- **Friendly install + UI.**

### Weaknesses
- **Critical RCE in backup (CVE / advisory GHSA-vrgf-rcj5-6gv9, 2026).** *"A critical vulnerability has been identified in the Runtipi backup and restore functionality. The flaw allows an authenticated user to execute arbitrary system commands on the host server by injecting shell metacharacters into backup filenames."* All instances of Runtipi v3.7.0+ affected. Fixed in v4.7.0. (https://github.com/runtipi/runtipi/security/advisories/GHSA-vrgf-rcj5-6gv9) **This is the kind of event that should make any solo founder nervous about Runtipi's security culture.**
- **Data layout is confusing.** *Discussion #768:* "run tipi has data in 3 locations... runtipi has a place for docker files `/runtipi/apps/immich/...`, the images go in another place `~/runtipi/media/data/images/immich/...`, runtipi app data separates the database `/app-data/immich/data/db`... This makes me wonder how i would even backup the app if i run it through runtipi."
- **No built-in backup UI for a long time** (added in v4.0, but it's a cron script, not first-class).
- **No native multi-server.**
- **No built-in reverse proxy with HTTPS *initially* required manual Traefik config** (now better, but historically a stumble).
- **No git-push-deploy, no PaaS workflow.**
- **Smaller community than CasaOS/Umbrel/Coolify.**
- **Single maintainer-driven.** The repo shows ~50 contributors, but the top maintainer (nicotsx) carries a lot.

### What it does NOT do
- No git deploys.
- No multi-server.
- No Kubernetes.
- No first-class reverse-proxy UI (you configure it via settings).
- No SSO / OIDC.
- No built-in DNS / secrets manager.

---

## 12. Pangolin (pangolin.net) — not a PaaS, but adjacent

- **Website:** https://pangolin.net
- **GitHub:** https://github.com/fosrl/pangolin — 20,778 stars, 681 forks, 100 contributors, 96 open issues
- **License:** AGPL-3.0 (Community) / Fossorial Commercial License (Enterprise) — *free for businesses under $100K annual revenue*
- **Tech stack:** TypeScript (98.4%), Go (0.8%)
- **Latest release:** 1.18.4 (May 2026)
- **Backed by:** Y Combinator W25 (Fossorial)
- **Target user:** Self-hosters behind CGNAT, small teams, anyone who wants Cloudflare Tunnels or Tailscale Funnel without trusting a third party
- **Total installs:** 140,000+ in 5 months (per aicoolies.com)

### Core features
- Identity-aware reverse proxy (browser-based access to web apps)
- Client-based private resource access (SSH, DBs, RDP, network ranges)
- WireGuard tunnels (Gerbil + Newt) with NAT traversal
- Site-to-site connections
- OIDC / SSO integration (Authentik, Keycloak, Google, GitHub)
- Time-limited shareable access links
- Traefik for HTTPS termination on the VPS
- Free for <$100K revenue
- Zero-trust RBAC per resource (not per network)

### Strengths
- **Genuinely the best self-hosted Cloudflare-Tunnels replacement.** *XDA Developers:* "Pangolin, and while I've already written about it, one of the main selling points is that it works best when on a VPS. That way, you can use the Newt Docker client to do NAT traversal and avoid all the annoying ISP issues."
- **WireGuard performance** = low latency, automatic key rotation.
- **Dual mode (browser + client) for any TCP/UDP.**
- **Free for hobbyists and businesses < $100K.**
- **Graduated commercial pricing** (hobby → startup → enterprise).
- **140k installs in 5 months.** Strong product-market fit for the access layer.
- **Cloud-native buildpack support, Caddy as nginx alternative, more robust multi-stage Dockerfile support** (Wait, that's Dokku — not Pangolin; copy edit.)

### Weaknesses
- **Not a PaaS.** It's a tunnel + identity layer. You still need to deploy your apps *somewhere* (Pangolin just exposes them).
- **Requires a public VPS hub.** Not pure-mesh like Tailscale.
- **No mobile clients yet.** Limiting for smartphone access.
- **AGPL-3.0 community license** — restricts commercial redistribution.
- **Advanced analytics + HA reserved for commercial tier.**
- **"Documentation could be more extensive for complex multi-site enterprise deployment scenarios."** (aicoolies.com)
- **Setup is "1–2 hours, not 5 minutes"** (bigiron.cc).
- **No DDoS protection beyond your VPS provider's basic measures.**

### What it does NOT do
- No git deploys.
- No app database provisioning.
- No CI/CD.
- No Kubernetes.
- No application-level observability.

---

## 13. Rust-based deployment tools (the new wave)

This is the *most important* section for positioning. The new product is not the first Rust project to attack this niche. Here's the lay of the land.

### 13.1. Kamal 2 (Basecamp / 37signals / DHH)

- **GitHub:** https://github.com/basecamp/kamal — 14,247 stars, 160 contributors
- **License:** MIT
- **Tech stack:** **Ruby** (not Rust, but a major deploy tool)
- **Latest:** v2.11.0 (March 2026)
- **Powers:** HEY.com and Basecamp in production

**What it is:** *Imperative* deploy tool. "Capistrano for containers." You write `config/deploy.yml`, run `kamal deploy`, and your app lands on any number of bare-metal / VPS / cloud servers. kamal-proxy is a Rust-based reverse proxy that ships with it. Multi-app on single server, alias commands, hooks, accessory services.

**Strengths:** simple, transparent, fast, deploys to anywhere with SSH, MIT, from the Basecamp team, kamal-proxy in Rust gives you HTTPS + traffic switching without nginx.

**Weaknesses:** no UI (CLI + YAML), no managed databases, no "web panel," no multi-tenancy, no RBAC. *DHH:* "Kamal is intentionally designed around imperative commands, like Capistrano." — i.e., the opposite of declarative / GitOps.

**Not in direct competition with Coolify** — solves a different problem (deploy tool, not PaaS). But the new product is closer to Kamal-with-a-database than to Coolify-without-a-web-panel.

### 13.2. Shuttle (Rust PaaS, cloud-based)

- **GitHub:** https://github.com/shuttle-hq/shuttle — 6,888 stars, 110 contributors
- **License:** Apache 2.0
- **Tech stack:** Rust
- **Latest:** v0.57.3 (Sept 2025)

**What it is:** Rust-native "Heroku for Rust." You write `#[shuttle_runtime::main]`, run `shuttle deploy`, and your Rust app is live. Cloud-hosted (AWS eu-west-2, London). Optional self-host.

**Strengths:** *incredibly* nice Rust DX. Macros provision Postgres / secrets / storage. Fast redeploys. No Dockerfile needed.

**Weaknesses:** managed cloud, not "self-host on my VPS." You don't get the coolify-style "own my data" story. Latest release is from Sept 2025 — slower cadence. Limited frameworks. Not an "infra-for-any-app" PaaS — it's a Rust opinionated platform.

**Not a direct competitor for the new product's positioning** (different focus), but proof that Rust single-binary / framework-friendly deploy tools have an audience.

### 13.3. Rivet (Rust actors, cloud + self-host)

- **GitHub:** https://github.com/rivet-dev/rivet — 5,515 stars, 30 contributors
- **License:** Apache 2.0
- **Tech stack:** Rust + TypeScript
- **Latest:** v2.2.2-rc.1 (May 2026)

**What it is:** "Actor" primitive for stateful workloads (AI agents, collaborative apps, durable execution). Has self-host mode: "Single Rust binary or Docker container. Works with Postgres, file system, or FoundationDB." Also Rivet Cloud.

**Strengths:** Single Rust binary, self-host-friendly, FoundationDB option, FoundationDB + EPaxos under the hood (multi-region KV).

**Weaknesses:** Not a general PaaS — it's an actor / stateful execution platform. Different abstraction layer.

**Not directly competitive**, but proof the Rust-deploy-tooling space is alive and well-funded.

### 13.4. yoink (Rust, very new, "between Kamal and K8s")

- **GitHub:** https://github.com/oddur/yoink — 12 stars (!!), 1 contributor
- **License:** MIT
- **Tech stack:** Rust
- **Created:** April 2026
- **Latest:** v0.20.9 (May 2026)
- **Website:** https://yoink.is

**What it is:** "A small, opinionated container deploy CLI + TUI for people who run a handful of services on a handful of bare-metal hosts. Sits between Kamal and Kubernetes — opinionated about the same things Kamal is, borrowing the few Kubernetes ideas that actually pay off at this scale."

**Why it matters:** this is *the closest analogue* to the new product's positioning. The pitch is essentially: single Rust binary, k9s-style TUI, no web app, no database, no agent on hosts, sealed secrets in repo, works with CI / AI agents. *"Yoink picks 'batteries included' over 'framework' for the boring-but-important pieces an operator would otherwise have to glue together themselves: hardened container defaults, healthcheck-gated swaps, drift detection, dependency ordering, and sealed secrets."*

**Comparison table yoink publishes vs Coolify** (https://yoink.is/docs/intro/compared):

| | yoink | Coolify |
|---|---|---|
| Architecture | Single binary, runs on demand | Coolify Core (web app + DB + queue) on one host, agent on each managed host |
| Operator interface | CLI + TUI (keyboard-only) | Web UI (primary), REST API (secondary) |
| State of truth | The git repo's `yoink.yaml` | Coolify's database (UI-edited) |
| Build pipelines | Out of scope (use CI / `yoink build`) | Built-in (Nixpacks, Dockerfile, buildpacks) |
| One-click databases | Out of scope | Built-in templates with backups |
| Auto-deploy on git push | Out of scope (CI calls `yoink up`) | Built-in |
| Multi-user, RBAC, audit log | ✗ | ✓ |
| Bundled reverse proxy | ✓ (Caddy) | ✓ (Traefik) |
| Drift detection | ✓ | ✗ |
| Driven by AI agents / CI scripts | ✓ | ◐ (REST API exists, but UI is the primary) |

**Implication for the new product:** the new product needs to differentiate from yoink along at least one axis: 
- **Provide some PaaS features (git-push auto-deploy, one-click databases, S3 backups)** — yoink explicitly leaves these to CI.
- **Provide a web UI as an option, not a requirement.**
- **Provide a hosted / managed tier** for non-Linux users.

### 13.5. sh0 (Rust, "AI CTO PaaS")

- **Website:** https://sh0.dev

**Pitch:** "The Self-Hosted PaaS with a Built-in AI CTO. Deploy, database, auth, storage, email, functions — your entire backend in one binary. Built in Rust. Works on any Linux server with Docker."

**Specs:** single Rust binary, ~50 MB memory footprint, 30+ CLI commands, 180+ API endpoints, code health scoring, 13 backup storage backends, 230+ API endpoints, 12 management tabs per app.

**Why it matters:** validates the "single binary, Rust, Docker, all backend primitives in one tool" pitch. Has the AI angle baked in.

### 13.6. PIER (Rust, minimal single-binary PaaS)

- **Website:** https://devcom.app/en/works/pier
- **Tech stack:** Single Rust binary, embedded SQLite, ~30 KB HTMX frontend, AGPL-3.0

**The numbers game PIER plays** (and which the new product can echo):

| Metric | PIER | Coolify | Dokku | CapRover |
|---|---|---|---|---|
| Idle RAM | **20–40 MB** | 750 MB – 1.2 GB | ~120 MB | ~250 MB |
| Disk footprint | 15–30 MB | ~1 GB | ~150 MB | ~400 MB |
| Idle containers | 1 (+ Traefik) | 6+ | 0 | 5+ |
| Minimum VPS | 512 MB / 1 vCPU | 2 GB / 2 vCPU | 1 GB / 1 vCPU | 1 GB / 1 vCPU |
| Implementation language | Rust | PHP / Laravel | Bash / Go | TypeScript / Node.js |
| Frontend size | ~30 KB | ~300+ KB | no UI | ~500+ KB |
| Database | Embedded SQLite | External PostgreSQL | Filesystem | Embedded LokiJS |
| License | AGPL-3.0 | Apache 2.0 | MIT | Apache 2.0 |

**Why it matters:** PIER proves the single-binary Rust story is *the* differentiator that resonates with solo devs. The "fits on a $5 VPS, runs in 20–40 MB" pitch is the wedge.

### 13.7. Tako (Rust, no-Docker PaaS)

- **Website:** https://tako.sh
- **Tech stack:** Single Rust binary (`tako-server`), CLI, Pingora proxy (Cloudflare's Rust framework)

**Pitch:** "CLI-first. No Docker, no bundled databases, no dashboard." Apps run as native OS processes. TOML config. SDKs for JS/TS/Go. Local dev with built-in HTTPS + DNS.

**Why it matters:** explicitly *anti-Docker* stance. "Coolify runs everything as Docker containers — your apps, its own services, the databases it manages... Docker is a hard requirement. Coolify itself runs as a Docker Compose stack."

### 13.8. Komodo (Rust, multi-server orchestrator)

- ~7k stars, AGPL-3.0, very active in 2026
- **Architecture:** "Komodo Core" (web service + MongoDB) + "Komodo Periphery" (agent on each host)
- Git-driven deploys, secrets, stack-of-stacks, multi-server, GitOps "Resource Sync" file imports
- Web UI primary, CLI/API secondary

**Comparison to yoink:**

| | yoink | Komodo |
|---|---|---|
| Architecture | Single Rust binary, runs on demand | `Komodo Core` (web service + UI) + `Komodo Periphery` (agent on every host) |
| Always-on services | None | Two: Core (with a database) + Periphery on each host |
| Operator interface | CLI + TUI | Web UI + CLI/API |
| State of truth | The git repo's `yoink.yaml` | Komodo's database (with optional GitOps file imports) |
| Multi-user / RBAC | None | First-class users, groups, permissions |

### 13.9. Uncloud (Docker Swarm successor in Rust?)

- Mentioned in yoink's comparison table: "1–10 hosts, multi-service, want a built-in WireGuard mesh + per-host daemon — Uncloud."
- Single Rust binary, mesh networking, Docker-machine-like UX

---

## 14. Cross-cutting themes

### 14.1. Common feature requests (from GitHub issues, Reddit, HN)

| Request | Where it's wanted |
|---|---|
| **"GitOps / declarative config-as-code"** | Coolify (issue #6000+ family), Dokploy (#3872), Komodo has it, yoink has it built in. **Nobody else in the PaaS space does it first-class.** |
| **"Built-in DNS manager"** | Dokploy #4376 (May 2026). Coolify also lacks. Manual Cloudflare dashboard hopping is a consistent complaint. |
| **"Cancel / clear stuck deployments"** | Dokploy #4461 (May 2026). Coolify has the same complaint on HN. |
| **"Preview environments"** | Coolify has them, Dokploy does not (yet), CapRover does not, Dokku does not. |
| **"Auto-deploy on GitHub release"** | Coolify #4972. |
| **"Predefined server env vars"** | Coolify #7738. |
| **"RBAC, audit logs, SSO"** | Coolify partially has it, Dokploy partially, CapRover/Dokku/YunoHost don't. |
| **"Resource quotas"** | Portainer BE has them. Nobody else. |
| **"Real zero-downtime deploys"** | Coolify's drops in-flight requests (acknowledged). Dokploy similar. |
| **"CVE / dependency patching"** | Not provided by any. ceaksan.com got a German BSI email about an exposed PostgreSQL port. |
| **"Migrate from Coolify to Dokploy"** | Dokploy #3098 — feature was rejected as unfeasible. |

### 14.2. Why a solo dev picks Coolify over Dokploy, or vice versa

Based on Medium, dev.to, servercompass, logrocket, use-apify reviews:

**Picks Coolify:**
- Mature, large community, Apache 2.0
- 280+ one-click services
- Preview environments
- Battle-tested in 2026
- MCP server for AI deploys
- Wildcard SSL
- Team management built in
- Lighter learning curve for the actual web UI

**Picks Dokploy:**
- Cleaner, faster UI
- Lighter RAM (~600 MB vs ~800 MB)
- Native Docker Swarm (not experimental)
- Closer to raw Docker / Docker Compose
- Volume backups (not just DB)
- No feature-gated licensing
- Production-oriented (build server, host cron, Traefik advanced)

**The shared pain in both:** neither gives you GitOps / declarative config; neither has a real "no panic" zero-downtime deploy; both have panel-on-server resource cost; both have the panel itself as an attack surface.

### 14.3. Open-source sustainability / commercial-OSS hybrid models

| Project | Model | Status |
|---|---|---|
| **Coolify** | Apache 2.0 self-hosted + $5/mo Cloud + Pro tiers $5–350/mo for OAuth/team/white-label | Sustainable, 1–2 core maintainers + 420 contributors, but small core. Donation-supported. |
| **Dokploy** | Apache 2.0 core + source-available for templates/multi-server/previews + $4.50/mo Cloud | License is the friction point. |
| **CapRover** | Apache 2.0 + paid CapRover Cloud ($5/mo) | Maintainer (githubsaturn) has been at it since 2017. Slowing. |
| **Dokku** | MIT, no paid tier | Volunteer, stable. |
| **Portainer** | Zlib CE + Commercial BE | Profitable company, transparent. |
| **YunoHost** | AGPL-3.0, volunteer, EU grants | Stable, but slow. |
| **CasaOS** | Apache 2.0 → effectively abandoned, dev moved to closed-source ZimaOS | **Warning sign for any Apache-2.0 project with one corporate sponsor.** |
| **Umbrel** | Source Available (not open) | Profitable, hardware sales. |
| **Cosmos Cloud** | AGPL-3.0 + paid hosted | Small team, growing. |
| **Runtipi** | GPL-3.0, volunteer | Recent critical RCE in backup suggests thin security bandwidth. |
| **Pangolin** | AGPL-3.0 + Fossorial Commercial (free < $100K revenue) | YC W25-backed, $4–5/mo VPS, 140k installs. Best growth story of the cohort. |
| **Kamal** | MIT, by Basecamp | DHH-maintained, eternal. |
| **Shuttle** | Apache 2.0, cloud-only primarily | VC-funded. |
| **PIER / sh0 / Tako / yoink** | Various (AGPL-3.0, MIT, etc.) | Solo / small-team. No clear monetization yet. |
| **Komodo** | AGPL-3.0, no paid tier | Solo-ish, very active 2026. |

**Take-away:** the field's most sustainable models are (a) a real commercial product with OSS as funnel (Portainer, Umbrel, Pangolin), (b) a creator-of-record with personal commitment (DHH, githubsaturn, jose-diaz-gonzalez), or (c) a well-funded company with an OSS arm (Dokploy, Cosmos, Shuttle). The pure-volunteer AGPL/MIT projects (YunoHost, Dokku) are stable but slow.

The new product should *signal sustainability early* — either by being a small commercial-OSS hybrid, or by being funded by an entity that won't disappear in 18 months. The CasaOS story is the negative case study.

### 14.4. The "self-hosting tax" (the real cost)

Per *ceaksan.com* (Feb 2026, Hetzner + Coolify for 3 months, 4 projects):

| Category | Self-host (Hetzner + Coolify) | Managed (Vercel + Neon + Inngest) |
|---|---|---|
| Monthly bill | €4.5 ($7) + snapshot/backup | $64–100+/mo |
| Initial setup time | 2–3 days | 30 minutes |
| Security maintenance | Ongoing (CVE tracking, patching, firewall) | Platform's responsibility |
| Incident response | You (BSI email, SSL issue, port conflict) | Support team |
| Forced migrations | Possible (PG → managed) | Rare |
| Stress factor | "Every security update feels urgent" | Low |
| Total time cost (3 months) | ~40–60 hours | ~2–3 hours |

**The "self-hosting tax" is the engineering time cost of running your own infrastructure.** The new product's positioning should be: "we don't make self-hosting free, but we *minimize* the tax by being a single binary, with fewer CVEs, fewer upgrade migrations, fewer moving parts."

---

## 15. Positioning gaps for the Sovereign Application Runtime

Synthesizing all of the above, here are the **unclaimed** or **under-claimed** positions the new product can take.

### Gap 1: "Coolify is too heavy. PIER is too thin. We're the middle."

PIER has 20–40 MB, single binary, AGPL-3.0, but is *app-store*-style. yoink is single binary, k9s-style TUI, but explicitly *not* a PaaS (no git auto-deploy, no one-click DBs). Coolify is 0.8–1.2 GB. There's room for a single-binary, ~80–150 MB, **git-push-PaaS** that sits between them.

### Gap 2: "Truly open source, no resale carve-outs."

Dokploy's source-available license is a real, quantified wedge. A Rust PaaS that ships under **MIT or Apache 2.0 with zero feature carve-outs** and *publicly states that* has direct ammunition.

### Gap 3: "No GUI required, but a TUI is fine, and the entire config is in your git repo."

yoink has this as a solo project. Make it a real product with a small core team, a security-advisory process (cf. Runtipi's RCE), and credible sustainability. The "AI-agent-friendly" angle is real and under-served: a deploy tool that an AI agent can drive without clicking anything.

### Gap 4: "Serverless-style ergonomics for the solo dev."

A solo dev on a $5–$10 VPS doesn't want to manage a Coolify control plane with 6+ containers. The new product's claim — *one binary, runs in 100 MB, fits in 512 MB VPS, ships with everything from SSL to secrets to backups* — is real, defensible, and validated by PIER / sh0 / yoink.

### Gap 5: "Deploy via SSH, expose via Pangolin / Tailscale / Cloudflare Tunnel, no need to invent a reverse-proxy story."

The new product can *not* compete on "we have a great reverse-proxy UI" — that's Pangolin's job, plus Traefik/Caddy already exist. Instead, integrate with them. Be the *deploy* layer; let Pangolin be the *expose* layer. The XDA / BigIron / Portless README confirm this composition pattern is what people are actually building.

### Gap 6: "One operator, 1–50 servers, declarative."

Coolify's 56k-star UI is the wrong tool when you have one operator and twenty small projects. Komodo and yoink understand this. The new product should *unambiguously* target 1–3 operators, 1–50 servers, with config-as-code (in git) and a TUI as the daily-driver interface.

### Gap 7: "Memory-safe, CVE-rare, low attack surface."

A Rust single binary has a fundamentally smaller attack surface than PHP-Laravel + Postgres + Redis + Soketi. Lean into the CVE story.

### Gap 8: "Sustainability signal."

The new product should ship a clear answer to "will this be alive in 18 months." CasaOS is the negative case study. Pangolin's YC backing is the positive case study. A commercial-OSS hybrid (free self-hosted, paid managed control plane, no feature gating) is the most credible model.

---

## 16. Sources

**Coolify:** coolify.io, github.com/coollabsio/coolify, mfyz.com, ceaksan.com, getautonoma.com, pickuma.com, dev.to, europeanstack.com, toolbrain.net, servercompass.app, use-apify.com, contabo.com, ossalt.com, nextgrowth.io, Hacker News.

**Dokploy:** dokploy.com, github.com/Dokploy/dokploy (issues #3872, #4376, #4461, #3098), blog.logrocket.com, ossalt.com, servercompass.app, use-apify.com, contabo.com, nextgrowth.io, koome@medium.

**CapRover:** caprover.com, github.com/caprover/caprover (issues #2351, #2366, #2377, discussion #2342), dev.to.

**Dokku:** dokku.com, propicked.com, jacar.es, training-stack.com, upcloud.com, getautonoma.com, dev.to.

**Portainer:** portainer.io, github.com/portainer/portainer, unsubbed.co, oneuptime.com, saascompared.com, rodak.pro, ossalt.com.

**YunoHost:** yunohost.org, doc.yunohost.org, appmus.com, linuxmind.dev, unsubbed.co, saashub.com, itsfoss.gitlab.io, cheapskatesguide.org, elenarossini.com.

**CasaOS:** github.com/IceWhaleTech/CasaOS (#2494, #767), community.zimaspace.com, data-mammoth.com, bigiron.cc, zimaspace.com, openalternative.co, unsubbed.co, cloudzy.com.

**Umbrel:** github.com/getumbrel/umbrel, blockdyor.com, cnx-software.com, tedium.co, notebookcheck.net, cloudzy.com, xda-developers.com.

**Cosmos Cloud:** makerstack.co, cloudzy.com, bigiron.cc, openalternative.co.

**Runtipi:** runtipi.io, github.com/runtipi/runtipi (advisory GHSA-vrgf-rcj5-6gv9, discussion #768), appselfhost.com.

**Pangolin:** pangolin.net, github.com/fosrl/pangolin, aicoolies.com, rodak.pro, bigiron.cc, xda-developers.com.

**Rust tools:** github.com/basecamp/kamal, github.com/shuttle-hq/shuttle, github.com/rivet-dev/rivet, github.com/oddur/yoink, yoink.is, sh0.dev, devcom.app (PIER), tako.sh, github.com/komodo-server/komodo (via OSSAlt), cloudzy.com.

**Comparison articles:** dev.to/pickuma/coolify-vs-dokku-vs-caprover-self-host-paas-compared-in-2026-b0d, blog.logrocket.com/dokploy-vs-coolify-production/, ossalt.com/guides/dokploy-vs-coolify-self-hosted-paas-2026, use-apify.com/blog/coolify-vs-dokploy-2026, nextgrowth.ai/coolify-vs-dokploy/, getautonoma.com/blog/open-source-alternatives-vercel, servercompass.app/blog/coolify-vs-dokploy-self-hosted-paas-comparison, contabo.com/blog/blog-coolify-vs-dokploy-comparison/, upcloud.com/blog/dokku-vs-coolify-vs-dokploy-production-deployment/.

**Sustainability / commercial OSS:** portainer.io/blog/portainer-community-edition-ce-vs-portainer-business-edition-be-whats-the-difference, yunohost.org (NLnet / NGI / EU grants), Pangolin.net (YC W25), IceWhale PR Newswire (CasaOS → ZimaOS).

---

## Appendix: Quick competitor table

| Tool | Stars | License | Footprint (RAM) | PaaS? | GUI | Year |
|---|---|---|---|---|---|---|
| Coolify | 56.3k | Apache 2.0 | 800–1200 MB | Yes | Web | 2021 |
| Dokploy | 34.4k | Apache 2.0 + source-available | ~600 MB | Yes | Web | 2024 |
| CapRover | 14.9k | Apache 2.0 / NOASSERTION | ~250–400 MB | Yes | Web | 2017 |
| Dokku | ~30k | MIT | ~150 MB | Yes | CLI | 2013 |
| Portainer CE | ~36.9k | Zlib | ~50–100 MB | No (Docker UI) | Web | 2016 |
| YunoHost | n/a | AGPL-3.0 | n/a (Debian distro) | No (app store) | Web | 2012 |
| CasaOS | 33.4k | Apache 2.0 (abandoned) | ~200–400 MB | No (app store) | Web | 2021 |
| Umbrel | ~11k | Source Available | n/a (Debian distro) | No (app store) | Web | 2020 |
| Cosmos Cloud | ~5.8k | AGPL-3.0 | ~500 MB–1 GB | Partial (app store + reverse proxy) | Web | 2022 |
| Runtipi | 9.3k | GPL-3.0 | ~150–300 MB | No (app store) | Web | 2022 |
| Pangolin | 20.8k | AGPL-3.0 (+commercial) | ~150–300 MB | No (tunnel + identity) | Web | 2024 |
| Komodo | ~7k | AGPL-3.0 | ~100 MB (core) | Yes (multi-server) | Web + API | 2024 |
| Kamal 2 | 14.2k | MIT | n/a (deploy tool) | Deploy only | CLI | 2023 |
| Shuttle | 6.9k | Apache 2.0 | n/a (Rust PaaS) | Yes (Rust) | CLI + Cloud | 2022 |
| Rivet | 5.5k | Apache 2.0 | n/a | No (actors) | Web + CLI | 2023 |
| yoink | 12 | MIT | n/a (CLI/TUI) | Deploy only | TUI | 2026 |
| PIER | n/a (small) | AGPL-3.0 | 20–40 MB | Yes (PaaS) | Web (HTMX) | 2025–2026 |
| sh0 | n/a (new) | n/a | ~50 MB | Yes (PaaS) | Web + CLI | 2025–2026 |
| Tako | n/a (new) | n/a | n/a | Yes (no-Docker) | CLI | 2025–2026 |
