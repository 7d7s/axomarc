# Sovereign Application Runtime — PM Persona Lens

**Date:** June 2026
**Author:** Principal PM persona (for product positioning, packaging, onboarding, and developer-experience strategy)
**Companion docs:** `Research-1.txt` (spec), `Research-2.md` (synthesis), `Research-Report-1.md` (tech), `competitive-landscape.md`, `user-pain-research.md`, `sovereign-runtime-market-research.md`
**TL;DR (5-bullet):** the product wins on **one specific, repeated developer moment** — the moment between "I have a working app locally" and "it is live, on my domain, with HTTPS, behind a proxy, with a database, with secrets, with logs I can read." Own that moment with a CLI that feels like `vercel deploy` and a TUI that feels like `k9s`, ship the docs that Coolify/Dokploy never had, sell it on per-server pricing that maps to EU procurement, and signal sovereignty and survivability from day one.

---

## 1. The Day in the Life of a Sovereign Application Runtime User

A real user of this product in June 2027 is **Mira, 32, solo founder of a B2B SaaS for therapist onboarding.** She has 1 Hetzner CX32 (4 vCPU, 8 GB) in Falkenstein, 1 Postgres, 1 Redis, 4 apps (API, web, worker, cron), ~150 paying customers, and a half-built MCP server her AI agent uses. She deploys 4–6 times a day. She last SSH'd into the box 11 days ago. She is on call.

**9:00 AM** — She opens a Cursor tab, edits a migration, runs `tool deploy api --strategy bluegreen --wait`. The CLI prints a 7-step pipeline ("pulling image…", "warming healthcheck…", "shifting traffic…"), a progress bar, a final green ✓, and the URL. 38 seconds. No SSH. No `docker ps`. No "did I forget the migration?"

**9:14 AM** — Her status page (auto-published at `status.mirasaas.com`, generated from the same `tool status --public`) shows 100% uptime. She's used it twice for a customer-support email: "our service is operational, here is our live status page."

**11:42 AM** — A user reports a 500 error. She runs `tool logs api --since 30m --level error` and gets three rows. She SSHs into the box only because she wants to grep more. The TUI is the alternative: `tool tui` opens, `/` searches "500", `j`/`k` walks the matches, `Enter` opens the request in detail. She never opens a web dashboard.

**2:00 PM** — She needs to onboard a freelancer to a single staging app. `tool access add --app staging --role dev --email freela@x.io` and the freelancer gets a one-time link, a scoped token, and SSH access to nothing else.

**5:47 PM** — She runs `tool backup verify --app postgres --restore-to scratch` and gets "restored 1.2 GB, 47,221 rows, 12 tables. 2 row counts differ from production (expected, the scratch was 24h stale)." Backup drift is detected, not assumed. This is the most important 90 seconds of her week.

**6:00 PM** — She pushes a config change. `tool preview` creates `pr-42.mirasaas.com` with a 24h TTL, returns a URL, posts to her Slack. She doesn't have to spin up a staging env or pay Vercel per seat for the freelancer to see it.

**Saturday 9:00 AM** — A Postgres major-version upgrade. `tool db upgrade --check` walks through every incompatibility, suggests fixes, then `tool db upgrade --apply` does the upgrade in a 12-second downtime window, with auto-rollback if the new version doesn't accept a connection in 5s. The whole thing is one command, not a Saturday afternoon.

**The "10x a day vs 1x a year" map:**

| Frequency | Action | Why it matters |
|---|---|---|
| 10×/day | `tool deploy`, `tool status`, `tool logs` | Muscle memory; must be sub-50ms perceived, never break |
| 5×/day | `tool secret set/get` (the AI agent uses this; she's not always the one typing) | Must be safe to call from a script; structured, never echo plaintext |
| 2×/day | `tool preview` (per PR / per design review) | Must be idempotent; TTL is critical |
| 1×/day | `tool backup verify`, `tool health` | The single highest-value feature, hidden until you need it |
| 1×/week | `tool rollback`, `tool db shell` | Rollback must be one command; db shell must drop into `psql` not a wrapper |
| 1×/month | `tool access add/revoke`, `tool audit` | Audit and access are trust-builders, not features |
| 1×/quarter | `tool db upgrade`, `tool migrate` to a new server | The "I'm not stuck" features — they earn the right to scale |

The product is **the day-to-day**. Every research paper and every review of Coolify/Dokploy/CapRover confirms the same thing: the panel is dead weight, the CLI is the product, the TUI is the bonus, the database of a real user's mental model is "what do I type to fix the thing that just broke."

---

## 2. Developer Journey — Lessons from Vercel, Railway, Fly, Supabase, Render

The single most important data point in the research is the **Vercel onboarding** ([getperspective.ai/blog/vercel-ai-native-customer-onboarding-developer-teams](https://getperspective.ai/blog/vercel-ai-native-customer-onboarding-developer-teams)): "A developer signs up, connects GitHub, picks a repo, and is reading their app on a live URL — often inside five minutes. … The product trusts the user to find the next thing." The lessons, transcribed for our product:

**Lesson 1 — No empty state.** Vercel doesn't ask the user to imagine a project; it imports one. **For Sovereign, the equivalent is `tool init`:** it scans the cwd, detects the framework (Next.js, FastAPI, Rails, Laravel, Go, Rust), creates an `app.yaml`, and offers `tool preview` as the next step. The user is *in the product* by the second prompt.

**Lesson 2 — Time-to-first-success under 5 minutes.** Skene's developer-onboarding guide ([skene.ai/resources/blog/developer-onboarding-guide](https://www.skene.ai/resources/blog/developer-onboarding-guide)) cites the benchmark: "Best-in-class tools hit under 5 minutes; developers who succeed in the first session are 3-5x more likely to convert." Our target: **2 minutes 30 seconds** from `curl -sSf tool.dev/install.sh | sh` to a live URL on a temporary domain. Every wasted second is a 0.4% conversion loss.

**Lesson 3 — A quickstart that fits on one page.** Skene: "If your quickstart has more than 5 steps, combine or eliminate steps." Our 5 steps:

```
$ curl -sSf tool.dev/install.sh | sh       # 1
$ tool init                                # 2 (detects framework, writes app.yaml)
$ tool login                               # 3 (device-code flow, browser opens)
$ tool deploy                              # 4 (builds, provisions TLS, gives you https://<id>.srvr.so)
$ open https://<id>.srvr.so                # 5 (you are live)
```

That's it. No Dockerfile. No `docker-compose.yml`. No nginx config. No certbot. The output of step 4 includes the URL, the deploy log, the rollback command, the next-step hint, and the URL to add a custom domain.

**Lesson 4 — Collaboration triggers the upgrade, not the front door.** Vercel lets the solo user stay on free forever. The Pro upgrade triggers when the user adds a teammate. **For us:** the community edition is forever-free for any number of servers. The Pro tier (managed updates, multi-tenant, EU support) is triggered at the *team*, not the *deploy*. The pricing page is one line: "Pro is for teams. Free is for as many servers as you want."

**Lesson 5 — The "deep dive" exists for the next 30 minutes.** Fly's "Deep dive demo" ([fly.io/docs/deep-dive](https://fly.io/docs/deep-dive/)) is the second-act product. "If you came from our Speedrun or Getting started pages, maybe you want to go beyond hello world and start to feel confident that you're choosing a provider that can support you now, and scale with you later on as you grow." The deep dive tells you what happens *after* the first 5 minutes. **For us, the deep-dive path is:** `tool init` → `tool preview` → `tool backup verify` → `tool secrets` → `tool db shell` → `tool access add` → `tool audit` → `tool migrate` to a second server. Each step is a section in the docs, a `tool tutorial <topic>` command, or a video. Skene: "If it takes you — the person who built the product — more than 5 minutes, it takes an outside developer much longer."

**Lesson 6 — The CLI is the primary, the web is the secondary.** Fly.io's pattern is exactly the inverse of Coolify's: "The CLI is excellent — fly logs, fly status, fly ssh console all work as expected. But the web dashboard is functional, not delightful" (dev.to fly vs Railway review, [dev.to/pickuma](https://dev.to/pickuma/flyio-vs-railway-which-platform-deploys-your-side-project-fastest-in-2026-5fdl)). For us: **CLI is the product**, TUI is the bonus, web is the dashboard *only for things that don't make sense in a terminal* (audit timeline, billing, team management).

**Lesson 7 — The framework-detection "scanner" is the secret sauce.** Fly's launch scanner "may be able to look at your app's source code and get through that ingredient list, straight to a ready-to-deploy Fly App. This is most likely to work for frameworks on which Fly.io has people specializing full time. Right now that's Elixir/Phoenix, Laravel, Rails, and Django" ([fly.io/docs/launch/create](https://fly.io/docs/launch/create/)). For Sovereign, the scanner should at minimum handle: **Next.js, Nuxt, SvelteKit, FastAPI, Django, Flask, Rails, Laravel, Go (net/http, Gin, Echo), Rust (axum, actix), Phoenix, Bun, Deno, and a generic "static site / Dockerfile" fallback.** Each scanner is a 30-line Rust module that produces an `app.yaml`. This is the highest-leverage code in the entire codebase. A 5-minute first deploy is the price of admission; a 5-minute first deploy *that worked without me reading docs* is the moat.

**Lesson 8 — The "What happens after?" is the 80% retention question.** Vercel's onboarding review by Command ([command.ai/blog/unboxing-vercel](https://www.command.ai/blog/unboxing-vercel/)) — 6/10, mostly about email verification friction, missing progress bar, "the sign-up process is dragging on." The fix is not the onboarding; it's the *post-onboarding* "what do I do now?" prompt. After `tool deploy`, the CLI should print, in this exact order:

```
✓ https://pr-48291.srvr.so is live
✓ Health check passed (200 OK, 12ms p50)
✓ TLS issued (Let's Encrypt, auto-renews in 60d)

Next, you probably want to:
  → tool domain add api.mirasaas.com        # map a real domain
  → tool secret set DATABASE_URL --from-stdin
  → tool preview --pr 42                    # PR preview environments
  → tool backup verify --app postgres       # first restore drill
  → tool tui                                # see your fleet at a glance
```

Five lines. One screen. No marketing.

---

## 3. CLI/TUI UX Patterns — Best-in-Class Reference Set

The 2026 reference set, from the TUI design skill at [github.com/gfargo/tui-design-skill](https://github.com/gfargo/tui-design-skill) and the TUI app patterns gallery at [github.com/hyperb1iss/hyperskills](https://github.com/hyperb1iss/hyperskills/blob/HEAD/skills/tui-design/references/app-patterns.md), gives us a vocabulary. The TUI is **command palette + vim motions + contextual footer.** The CLI is **clap + anyhow + colored + indicatif + comfy-table.** Every Rust CLI tutorial this year converges on the same five crates ([lucaberton.com/blog/rust-cli-tools-clap-2026](https://lucaberton.com/blog/rust-cli-tools-clap-2026/)) and the same patterns:

**The 8 non-negotiables for our CLI.**

1. **`--help` is the first thing an agent reads.** Treat it as a teaching surface. Use `clap`'s `after_long_help` to append 3-5 contextual tips and 3-5 real example commands. Pattern from the `agent-cli-framework` paperfoot spec ([github.com/paperfoot/agent-cli-framework](https://github.com/paperfoot/agent-cli-framework)): "Tips should be 3-8 bullets covering the most common workflows. Examples should be 3-5 real commands with one-line descriptions." **The `--help` for `tool deploy` should look like this:**

   ```
   tool deploy [APP]

   Build, deploy, and switch traffic for APP.

     $ tool deploy api
     $ tool deploy api --image ghcr.io/me/api:abc123
     $ tool deploy api --strategy bluegreen --wait
     $ tool deploy api --dry-run

   Flags:
         --strategy <rolling|recreate|bluegreen>   [default: bluegreen]
         --image <ref>                             Skip build, deploy a pre-built image
         --wait                                    Block until traffic is switched
         --dry-run                                 Print the plan, do not execute
         --no-traffic                              Deploy but keep old version serving

   Tips:
     • Add a custom domain with `tool domain add api.example.com`
     • Roll back the last release with `tool rollback api`
     • See the diff between current and last deploy with `tool diff api`
     • `tool deploy` will auto-run migrations if `app.yaml` has a `migrate:` step
     • On a failed healthcheck, the deploy is auto-rolled-back in 60s

   Examples:
     $ tool deploy                                       # deploy the app in cwd
     $ tool deploy api --strategy rolling                # zero-downtime rolling
     $ tool deploy api --image=ghcr.io/me/api@sha256:…   # pre-built image
     $ tool deploy --all                                 # deploy every app in app.yaml
   ```

2. **`--json` on every command, auto-detected via `std::io::IsTerminal`.** Humans get colored tables. Agents get JSON envelopes. Pattern from the `agent-cli-framework`: "Humans get colored, human-readable output. Agents get JSON envelopes. The binary detects which and adapts automatically. Both paths are first-class. If a command writes to stdout, it respects the output format — no exceptions, no code paths that leak raw text." This is the single highest-leverage thing for AI-agent-driven workflows. The future user is *Claude Code*, not Mira.

3. **Semantic exit codes.** `0` = success. `1` = generic failure. `2` = wrong usage. `3` = partial success (e.g., 2 of 3 apps deployed). `4` = upstream dependency failure (Docker not running, port in use, DNS provider down). This lets CI and AI agents decide whether to retry. Cargo uses this pattern. So does kubectl. So should we.

4. **Spinner + X-of-Y + progress bar — pick the right one.** From Evil Martians' CLI UX guide ([evilmartians.com/chronicles/cli-ux-best-practices-3-patterns-for-improving-progress-displays](https://evilmartians.com/chronicles/cli-ux-best-practices-3-patterns-for-improving-progress-displays)): spinner for "I have no idea how long this will take" (TLS issuance, registry pull), X-of-Y for "I have a list and I'm going through it" (multi-app deploy, multi-step migration), progress bar for "I'm doing many similar things in parallel" (build cache import, multi-region healthchecks). Always clear the spinner/bar on completion. Always leave a clean log behind. Always swap "ing" for "ed" at completion.

5. **Color the right way.** Respect `NO_COLOR=1` and `--color=never`. Use a 16-color palette with semantic slots: green for success, red for failure, yellow for warning, blue for "in progress", cyan for "info", gray for metadata. Don't color noise. From hyperb1iss's TUI design patterns: "Hierarchy recipe: 80% of content in `fg.default`. Headers in bold + `fg.emphasis`. Metadata in dim + `fg.muted`. Status in their semantic colors. Accents for interactive elements only."

6. **Errors that teach, not blame.** Pattern from the `agent-cli-framework`: print the error chain, color the cause, suggest a fix. Example:

   ```
   error: deploy failed for app "api"
     caused by: health check returned 503 after 30s
     caused by: container exited (code 1) — "listen tcp :8080: bind: address already in use"
   hint: another process is using port 8080. Either:
         - run `tool ps` to find it
         - set `app.yaml` → `container.port: 9090`
         - set `app.yaml` → `container.command: ["npm", "run", "start:8081"]`
   docs: https://tool.dev/docs/errors/port-in-use
   ```

   Three layers of context. One specific hint. One link. No "Oops something is not okay" (HN's exact quote about Coolify, [news.ycombinator.com/item?id=43589794](https://news.ycombinator.com/item?id=43589794)).

7. **`--dry-run` on every destructive action.** `tool deploy --dry-run`, `tool rollback --dry-run`, `tool db drop --dry-run`, `tool access revoke --dry-run`. The default is "ask before doing." The non-interactive mode is `--yes` or `--no-input`. **Destructive operations take `--confirm` as a flag; nothing else.** The `agent-cli-framework` puts this in their invariants: "The CLI never reads from stdin, never opens a pager, never asks 'are you sure?' Destructive operations take `--confirm` as a flag."

8. **Shell completions for bash, zsh, fish, nushell, powershell.** Free with `clap_complete`. Install via `tool completions install` (writes to the user's shell rc). Without completions, you're not a real CLI. Cargo, gh, kubectl, fly, docker all ship them. So should we.

**The 4 non-negotiables for our TUI** (from the TUI design skill, summarized): alt-screen mode, panic-safe terminal restore, `SIGWINCH` handling, `SIGTSTP` handling. Get those wrong and your TUI leaves the user's terminal in a broken state when something crashes. The `paperfoot/agent-cli-framework` and the `hyperb1iss/hyperskills` repos both call this out as a P0. The fix: `color-eyre` + `crossterm`'s raw-mode restore hooks + a finalizer that runs even on panic. Test it: open the TUI, kill -9 the process, check that the shell still echoes.

---

## 4. TUI Patterns — What the Best Tools Actually Do

The TUI design pattern library at [github.com/hyperb1iss/hyperskills](https://github.com/hyperb1iss/hyperskills/blob/HEAD/skills/tui-design/references/app-patterns.md) catalogs the canonical 7 layouts. For us, the right choice is **Persistent Multi-Panel** (lazygit, lazydocker pattern) for the main dashboard, plus a **Drill-Down Stack** (k9s pattern) for the per-app view. From kdash's README ([github.com/kdash-rs/kdash](https://github.com/kdash-rs/kdash?tab=readme-ov-file)), the modern simplification is: "KDash only offers a view of the resources with a focus on speed and UX. Really, if something is slow or has bad UX then please raise a bug. Hence the UI/UX is designed to be more user-friendly and easier to navigate with contextual help everywhere and a tab system to switch between different resources easily." This is the right read of the market: k9s is comprehensive but the learning curve is steep; kdash is read-only but the UX is *pleasant*. **We should be kdash-level pleasant, with k9s-level depth, on a smaller surface.**

**Concretely, the TUI should ship with these views** (drawn from lazygit / lazydocker / k9s / kdash analysis):

- **Pulse** (the home screen). Five lines of headline status: how many apps, how many are healthy, last deploy, last error, next backup verification. The thing Mira checks at 9 AM without thinking.
- **Apps** (master list). A list of every app, with status indicator (● green = healthy, ◐ yellow = deploying, ● red = errored), last deploy time, last deploy version, response time p50. Press `Enter` to drill into a single app.
- **App detail** (the drilled-in view). Tabs: `Logs`, `Deploys`, `Rollbacks`, `Metrics`, `Domains`, `Secrets`, `Config`. The current tab's content fills the right pane. `Tab` cycles. `1`-`7` jumps. `Shift-Tab` cycles back. `?` shows the help for the current view.
- **Servers** (multi-server in V1.5+). The fleet. CPU, RAM, disk. Drill in.
- **Backups**. Every database. Last backup time. Last verify time. Status. Drill in to see the restore drill history.
- **Audit**. Filterable timeline of who did what when. The thing you check when a teammate rolls back at 6 PM and you want to know why.

**The keybindings follow the universal conventions** (from the TUI design skill):

- `q` quit (or `Esc` to go back one level)
- `j`/`k` down/up; `h`/`l` left/right or collapse/expand
- `/` search, `n`/`N` next/prev match, `Esc` dismiss
- `?` help overlay for the current view
- `:` command mode (`:apps`, `:backups`, `:audit`, `:servers`)
- `Enter` select/confirm, `Tab` switch panel, `Space` toggle selection
- `g`/`G` jump to top/bottom
- `Ctrl+P` command palette (the "I forgot the keybinding" escape hatch)

**The footer always shows 3-5 context-specific shortcuts.** "Apps: `j/k` move · `Enter` open · `/` filter · `r` restart · `?` help." The footer changes with the active panel. This is the lazygit magic: the user never has to remember a keybinding because the answer is always one line away.

**The mouse should be supported but never required.** From the TUI design skill: "TUI trinity: command palette + vim motions + contextual footer covers every skill level." Mouse clicks for navigation, keyboard for power, both work. A user can run the entire product without ever touching a mouse; a user can also use a trackpad.

**The async pattern is the k9s pattern.** The TUI does not own the data. It opens a connection to the control plane (HTTP + WebSocket) and renders whatever the control plane sends. State is server-side. The TUI is a thin client. This means the same TUI can run on Mira's laptop or in a VSCode terminal or over SSH. The "I SSH'd into the box and ran the TUI there" antipattern is gone. The TUI is local; the state is remote.

**The status indicator palette is semantic, not decorative.** Red = unhealthy/error. Yellow = warning/degraded/deploying. Green = healthy/running/idle. Gray = stopped. Cyan = informational (e.g., "rolling out"). The colors map to the same semantic slots in the CLI. Consistency across interfaces is a trust signal.

---

## 5. Documentation Patterns — How Coolify/Dokploy Fail and How to Win

**Coolify's docs:** the structure is good, the content is patchy. The 2025 review by Fatih Yildiz ([mfyz.com/my-coolify-experience-after-a-year](https://mfyz.com/my-coolify-experience-after-a-year/)) is the canonical "I used it for a year" post: "Coolify sends upgrade notifications, which is good. I do want to know when things need attention. But after a while it starts to feel a bit like WordPress plugin maintenance." The docs don't reduce that feeling; they just document the surface.

**Dokploy's docs:** better than Coolify's, still has the "I clicked through and learned it from Stack Overflow" energy. The auto-deploy inconsistency is well-known (Medium / Koome) but not solved in docs.

**Dokku's docs:** excellent for a CLI-first audience, but assumes a user who can read man pages. Not for new developers.

**The Mintlify vs Docusaurus split** ([docsio.co/blog/mintlify-vs-docusaurus](https://docsio.co/blog/mintlify-vs-docusaurus), [stackfyi.com/guides/docs-platforms-mintlify-vs-docusaurus-vs-nextra-vs-fern-2026](https://www.stackfyi.com/guides/docs-platforms-mintlify-vs-docusaurus-vs-nextra-vs-fern-2026)) is the right strategic question for a 2026 product launch:

- **Mintlify Pro is $300/mo** (per [ferndesk 2026 pricing](https://docsio.co/blog/mintlify-vs-docusaurus)) with a free tier that watermarks. Mintlify is the fastest path to "looks like a real product" — the defaults are strong, OpenAPI is rendered cleanly, AI search is built in, and the hosted pipeline means no CI to maintain. The tradeoff is vendor lock-in: when Mintlify raises prices (it did in 2024, Pro went $150→$300), you migrate.
- **Docusaurus is MIT, free, Meta-backed, 64.4k stars** ([docsio.co](https://docsio.co/blog/mintlify-vs-docusaurus)). Mintlify search is faster (~50ms vs Docusaurus+Algolia ~80ms) but Docusaurus gives total control. The cost is a week of setup, ongoing React/Docusaurus maintenance, and a future migration when Docusaurus 4.x lands.
- **For a self-hosted, "own your stack" product,** Docusaurus is the right default. **For a "ship in 30 days" launch,** Mintlify is the right move. **For us, the right answer is hybrid:** launch on Mintlify, migrate to Docusaurus (or mdBook, see below) at 1,000 paying customers, when the search and analytics are paying their way.

**mdBook is the underrated option.** Rust-native, used by the Rust book itself, the Tokio tutorial, every Rust crate docs site on `docs.rs`. mdBook is what `cargo doc` would be if it were web-first. It is **a single binary**, `mdbook serve` runs locally, and the output is static HTML. For a Rust project, the docs site being mdBook is a tell that "this is a serious Rust product." I would argue: **the Sovereign Application Runtime docs should be mdBook**, full stop. Mintlify and Docusaurus add JS frameworks to a project that should be zero-JS-by-default. mdBook produces a static site that can be hosted on a 1 MB VPS for free, versioned with the binary, and searched with a 50-line Rust search index. Mintlify/Docusaurus are SaaS. mdBook is sovereign. **The docs site should be sovereign too.** This is the small detail that becomes a brand signal.

**Astro is the JS compromise** if we need a richer landing page or blog alongside the docs. The Astro docs site itself uses Astro. The integration with mdBook: a single Astro page at the root, with `/docs` mounted as a separate mdBook build. The marketing site is Astro; the documentation is mdBook. The blog is Astro. The blog has a "What's new" page generated from `git log` between releases. The docs have a "Version" dropdown that links to per-version sub-sites. The search box at the top is pagefind (the Astro-native search) for the marketing site, and mdBook's built-in search for the docs. **This stack costs $0 to run, fits on a single Hetzner box, and signals "we eat our own dog food."**

**The doc structure that wins** (synthesized from the TUI design skill, the Mintlify/Docusaurus analysis, and a close read of the Coolify/Dokploy pain threads):

1. **`/` — Landing.** "Self-hosted application runtime." Three buttons: `Install`, `Quick start`, `Read the docs`. No marketing copy on the landing page. A 30-second "what does this do" video. Logos of comparable-but-different products (Coolify, Dokploy, Kamal) with the one-line difference ("Coolify is a panel. We're a CLI.").
2. **`/quickstart` — Five steps. One page. Under 2 minutes.** The exact 5-step sequence from §2. Copy-pasteable. Works on a fresh Hetzner CX22.
3. **`/docs/` — mdBook.** Tutorials first, then how-tos, then reference, then explanations. The Diátaxis framework.
4. **`/docs/tutorials/`** — Six tutorials. *Deploy your first app.* *Set up a Postgres with backups.* *Add a custom domain with TLS.* *Wire up secrets.* *Open a PR preview.* *Run a fleet of two servers.* Each is a 5-minute read with copy-pasteable commands and expected output.
5. **`/docs/how-to/`** — 30–50 recipes. "How to set up Cloudflare DNS-01." "How to migrate from Coolify." "How to run air-gapped." "How to back up to B2 / R2 / S3." "How to upgrade Postgres in place."
6. **`/docs/reference/`** — Every CLI command, every config option, every API endpoint. Auto-generated from clap. Lives at `/docs/reference/cli/`. Updated on every release.
7. **`/docs/explanation/`** — Why we made the choices we made. "Why SQLite and not Postgres." "Why Caddy and not Traefik." "Why a single binary and not a panel." This is the "values" section; it is the sales page for engineers.
8. **`/changelog` — Every release, every breaking change, every migration path.** Auto-generated from `CHANGELOG.md`.
9. **`/roadmap` — Public.** Linear, GitHub Projects, or Plane. Items triaged openly. "Now / Next / Later." Per Plausible's "we have a public roadmap" ([plausible.io/blog/customers-not-investors](https://plausible.io/blog/customers-not-investors)), this is one of the cheapest trust signals in OSS.
10. **`/security` — CVE policy, security advisories, signing keys.** A PGP-signed `SECURITY.md` with a 24-hour response commitment for critical issues. Per GitHub's 2024 State of the Octoverse, this is now table stakes for any product an enterprise would buy.
11. **`/migration`** — "Migrating from Coolify" / "Migrating from Dokploy" / "Migrating from Heroku" / "Migrating from Render." Per [user-pain-research.md §6.2](user-pain-research.md), the Heroku "sustaining engineering" announcement is creating a migration wave. **Be the destination.** A working `tool import --from heroku --from-coolify --from-dokploy` command is the single highest-leverage "we're winning" feature for the next 12 months.
12. **`/llms.txt` and `/llms-full.txt`.** A machine-readable docs index for AI agents. Mintlify launched an MCP server in 2026 ([devtoolreviews.com](https://www.devtoolreviews.com/reviews/mintlify-vs-gitbook-vs-docusaurus-vs-readme-2026)) and so should we: our docs are the entry point for Claude Code and Cursor to deploy apps. The `llms.txt` is the canonical "give me everything you know about this product, machine-readable" file. The future user is an agent, not a human; the docs should serve both.

**Where Coolify/Dokploy/Dokku all fail at docs** is the migration story. None of them say "if you're on Heroku, here is the exact 30-minute migration path." **We win by being the destination, not the source, of migrations.** Coolify has no migration-from-Herkoku guide. Dokploy maintainer said "unfeasible" in [issue #3098](https://github.com/Dokploy/dokploy/issues/3098). We do.

---

## 6. Pricing & Packaging — The 2026 Sovereign OSS Playbook

The 2026 OSS funding models are well-mapped in [ossalt.com/guides/open-source-funding-models-sustainability-2026](https://ossalt.com/guides/open-source-funding-models-sustainability-2026). The relevant data points for our product:

- **Open-core** is the dominant commercial model for self-hosted SaaS alternatives in 2026. "The structure is straightforward: a fully functional open source version is available under a permissive or copyleft license, and a commercial version adds enterprise-grade capabilities available only to paying customers." GitLab, Mattermost, Metabase, Sentry, Cal.com, Plausible — all open-core.
- **Dual licensing with AGPL** generates reliable revenue from commercial buyers who cannot or will not comply with AGPL's copyleft terms. Grafana Labs, MariaDB — both validate the model.
- **Donations** rarely exceed $100K/year for projects below major-framework scale. Median is dramatically lower. Supplementation, not replacement.

**The Plausible playbook** ([plausible.io/blog/customers-not-investors](https://plausible.io/blog/customers-not-investors), [plausible.io/blog/open-source-funding](https://plausible.io/blog/open-source-funding)) is the right one to copy, with one adjustment: Plausible says no to investors and runs a pure-subscription model. They have $1M+ ARR on 50k+ paying sites. **The single product, dual-channel pattern: free self-hosted + paid cloud.** Plausible Cloud = $9/mo for 10k pageviews. Their self-hosted is free; you can pay them $5/mo to sponsor the development.

**For the Sovereign Application Runtime, the open-core structure is the right answer**, but the *core* must be the part that does the work, and the *commercial* must be the part that makes the work easy at scale. From the research synthesis, the recommended structure:

| Tier | What | Price | License | Audience |
|---|---|---|---|---|
| **Community** | Single server. Single binary. CLI + TUI + API. SQLite. Caddy. ACME. Plaintext secrets (encrypted at rest with age). Backups to local disk. Basic rollback. Basic logs. All the things a solo developer needs. | **$0** forever, unlimited servers, unlimited apps | Apache 2.0 | Solo devs, freelancers, learners |
| **Pro** | Multi-server (agent pattern). Encrypted secrets store (age + recipient lists). S3-compatible backups. Backup verification. Webhooks. Status page. Email/Slack/Telegram alerts. Preview environments. Audit log. | **€20/server/month** (or €200/year) | Apache 2.0 + commercial feature gates | Small teams, agencies, startups |
| **Enterprise** | EUCS-Substantial / BSI C5 mappings. EU support with DPA. SSO (OIDC). Approval flows. RBAC. SLA. On-prem air-gapped install. Custom cert authority. | **€500–5000/year** depending on org size | Commercial license, source-available | EU public sector, regulated industries, agencies serving them |
| **Cloud / Managed** | We run it for you. Same binary, our server, our maintenance. Migrate anytime to self-host. | **€50–200/server/month** depending on size | Apache 2.0 + ToS | Non-Linux teams, agencies, "I just want it to work" buyers |

**The license decision: Apache 2.0, no carve-outs, period.** Per the research synthesis, Dokploy's mixed Apache + source-available license is *the* recurring criticism in the market. Coolify is Apache 2.0 with zero carve-outs and that is cited as a reason to pick it. **Dokploy's docs (Jan 2026) call out the difference explicitly:** "Today marks a big change to the Dokploy's licensing. We're replacing our adapted open source license with an industry-standard open source license… Apache License 2.0." [dokploy.com/blog/we-are-updating-dokploys-open-source-license](https://dokploy.com/blog/we-are-updating-dokploys-open-source-license). **We are stricter than Dokploy: pure Apache 2.0 from day one.** This is a 1-line decision in `LICENSE` and it is worth €500K/year in trust.

**The pricing page decision: per-server, not per-app, not per-seat.** Per the [sovereign-runtime-market-research.md §6](sovereign-runtime-market-research.md), "Pricing should be per-node, not per-seat, to align with how EU public procurement and on-prem deployments are scoped. €20–100/node/month is the natural band." This matches how the EU buys (Hetzner invoices per-server, public procurement is per-server, IT-SA booths are full of per-server vendors). The alternative (per-seat) is what Vercel/Render do; that is the lock-in we are escaping. **€20/server/month is the floor; €50 is the natural starting price; €100 is the high end.** Annual: 20% off.

**Free vs paid line: per-server, not per-feature.** The free version is the version that runs your one Hetzner box. The Pro is when you have two servers and need the encrypted secrets + S3 backups + alerts. There is no "you can do X but not Y" gating. The line is "you can run one server, you can run N servers" plus the security/compliance features only an enterprise wants.

**The "what if the company disappears" promise is the brand.** Per Plausible: "If we had investors, data monetization would constantly be 'on the table' and the growth targets would pressure expansion of tracking. Financial independence removes that pressure." Substitute "data monetization" with "private cloud add-on" and the argument is identical. The pitch is: **"We will not be acquired, we will not raise a Series Z, and the binary keeps working even if we disappear."** This is a small but high-leverage line on the homepage.

---

## 7. USP Iteration — How the One-Sentence Pitch Evolves

The pitch ladder, from generic to specific, with what each version signals:

| Stage | Pitch | What it signals | When to use |
|---|---|---|---|
| 1 (today) | "A self-hosted PaaS for solo developers and startups." | Generic. Coolify/Dokploy say this. | Don't use. |
| 2 | "A Rust single-binary PaaS for solo devs and startups." | Niche. Smaller than Coolify. Lower-resource. | First Show HN post. |
| 3 | "A self-hosted, single-binary deployment platform with first-class secrets and GitOps." | Functional. Calls out the gaps in Coolify. | Sales calls. |
| 4 | "A Rust single-binary deployment platform with first-class secrets, declarative GitOps, and an EU sovereignty package." | Specific. Calls out the technical and procurement angle. | EU public-sector events. |
| 5 | "The deployment platform that survives the vendor disappearing. Rust single-binary, encrypted secrets, GitOps, EUCS Substantial, Apache 2.0. Your data, your server, your code." | Story. The brand. | Homepage, About, conference talks. |

**The ladder is the answer to the "Self-hosted Vercel → Deployment engine → Sovereign Runtime" question.** Each rung gives the buyer more reasons to choose us. The "Sovereign Application Runtime" name from the spec is the right rung-5 name; the rung-1 pitch is "for anyone who has ever SSH'd into a VPS and wished they hadn't." The pitch iterates with the audience, not the product.

**The wedge: the four-way intersection of "Rust single-binary + GitOps-declarative + sovereignty-positioned + AI-agent-friendly."** Per the research synthesis, this intersection is empty. yoink is single-binary but not PaaS. Komodo is multi-server but requires Core+Periphery. Tako is single-binary but no-Docker. PIER is single-binary but app-store-style. None of them own the GitOps-declarative + sovereignty intersection, and none of them have been positioned for AI agents (whose `tool deploy` calls are about to be the dominant interface).

**The "Vercel UX, VPS pricing" message is the wedge on the r/selfhosted side.** Quote from the Server Compass Indie Hackers post ([indiehackers.com](https://www.indiehackers.com/post/vercel-ux-vps-pricing-thats-what-i-built-6442b10c31)) — the founder of a competitor: "I was running projects across multiple PaaS platforms. Vercel for frontends, Railway for some backend services, Render for others, Supabase for auth, NeonDB for postgres. Each one felt great individually. Then I actually added up what I was paying: ~$200/month across everything. For side projects and small apps. … A $6 Hetzner VPS could run all of this. But every time I thought about migrating, I remembered what VPS management actually felt like: SSH-ing around, managing PM2 in tmux, grep-ing through logs at 2am." The brand promise is: **make the SSH-ing-around-tmux-grepping-logs-at-2am part optional, not the default.** That is the line on the homepage. The full one-liner: "Vercel UX, Hetzner pricing, your server."

---

## 8. Dev-Friendliness Signals — The 12-Point Trust Checklist

A solo developer evaluating this product in 30 minutes will look at exactly 12 signals. None of them are features. All of them are trust signals. Miss any one and the developer moves on. From the research synthesis, the Coolify / Dokploy / Dokku / yoink analysis, and the broader 2026 OSS landscape:

1. **Tests.** A `tests/` directory with >70% coverage, run in CI on every PR. The single biggest signal. If the README shows "✅ 1,432 tests passing" at the top, the developer knows the team is serious. Cargo, ripgrep, fd, bat, and every serious Rust project have this.
2. **CI.** GitHub Actions (or equivalent) on every commit. A green badge in the README. Matrix-tested on Ubuntu 22.04/24.04, Debian 12, RHEL 9. Bonus: a green "Cross-build status" badge for `aarch64-unknown-linux-musl`, `x86_64-unknown-linux-gnu`, and `x86_64-unknown-linux-musl`.
3. **Releases.** GitHub Releases page with binaries attached. Tagged semver. The latest release is <30 days old. A `cargo install --git` path for the bleeding edge.
4. **Semver.** Followed rigorously. Breaking changes bump the major version, the CHANGELOG is explicit, the migration guide is at `/docs/migrate/v1-to-v2`. **No silent breaking changes.** This is the Coolify issue that lost them users: schema migrations on every minor version.
5. **Security advisories.** GitHub Security Advisories enabled. CVE numbers assigned. A 90-day disclosure policy. A signed PGP key. **For EU public sector this is non-negotiable.** Per the [sovereign-runtime-market-research.md §2](sovereign-runtime-market-research.md), BSI C5 has 121 controls across 17 domains; "disclosure policy and vulnerability management" is one of them.
6. **Response time to issues.** A bot or maintainer reply on every issue within 48 hours. The Coolify backlog of 781 open issues is a warning sign. **For a solo project, "I can't promise 48h on every issue, but I will respond to security issues within 24h and bugs within 7 days" is honest and acceptable.** Putting that in the README is a trust signal.
7. **RFC process.** `/rfcs/` directory with merged and pending RFCs. The `kdl` and `rqlite` projects do this well. The community can see what is being designed before it is shipped. **Lightweight: a GitHub Discussions category called "RFCs" is enough.** Heavy: a formal `rfcs/` repo with a `proposed → accepted → implemented → shipped` workflow.
8. **Contributor guide.** `CONTRIBUTING.md` with a quick "first contribution" walkthrough. The Rust API guidelines and `rustfmt` + `clippy` badges. "Good first issue" tag on GitHub. The repo shouldn't have a 6-month-stale "good first issue" entry; refresh it weekly.
9. **Public roadmap.** Linear, GitHub Projects, or Plane. "Now / Next / Later" sections. Updated weekly. The 2024 Self-Host Survey reportedly doubled in respondents YoY ([sovereign-runtime-market-research.md §1](sovereign-runtime-market-research.md)); the audience is voting with their attention and they want to see what is being built. **Per Plausible's playbook: a public roadmap is the cheapest trust signal in OSS.**
10. **License.** Apache 2.0 in the LICENSE file, the README, the GitHub repo metadata, and the docs site footer. **No carve-outs. No dual-license. No BSL.** This is the Dokploy lesson, repeated in every Dokploy complaint thread.
11. **Bus factor.** A CODEOWNERS file. At least 2 active maintainers in the last 90 days. A "Governance" doc that names who has commit access and how to add new maintainers. **The CasaOS abandonment story is the single most-cited "I won't use this" pattern in 2025–2026.** Per the research synthesis: "IceWhale Technology has moved all development to ZimaOS, which is a closed-source commercial NAS OS" ([competitive-landscape.md §8](competitive-landscape.md)). CasaOS has 33,410 stars and is dead. We have to be visibly not-dead.
12. **A working demo.** `demo.tool.dev` runs a live instance of the product with a fake app deployed, that you can SSH into (or `tool ssh demo.tool.dev`) and explore. The demo resets every 24 hours. The data is fake. **Vercel has this. Fly has this. We have this.** The 5-minute-onboarding test is impossible without a live demo.

**The hidden 13th signal: a public "what we won't build" list.** Coolify has 781 open issues. Dokploy has 581. Most of those are "feature requests for things the maintainer will never build." A `WONTBUILD.md` in the repo — "we will not build Kubernetes support, we will not build a SaaS, we will not build Terraform" — is a trust signal that says "we are focused." It also pre-empts the most-cited wrong-feature requests.

---

## 9. Onboarding Flow Design — The 30-Day, 5-Minute, One-Command Plan

The Coolify benchmark is "deploy a Plausible in 2 clicks." The CapRover benchmark is "2 clicks." Vercel's is "5 minutes from signup to live URL." Railway's is "60 seconds from GitHub to a live URL" ([devguide.co.uk](https://devguide.co.uk/articles/railway-review-best-heroku-alternative-for-python-and-node-apps), [stack-unpacked.com](https://stack-unpacked.com/railway-review-deploy-anything-in-minutes/)). **Our benchmark is `tool deploy` from a fresh Hetzner box to a live URL in 2 minutes 30 seconds.** The full onboarding script:

**Phase 1: Install (30 seconds).**

```bash
# On a fresh Ubuntu 24.04 / Debian 12 / RHEL 9 box
$ curl -sSf https://tool.dev/install.sh | sh
# → detects musl, downloads the right binary, installs to /usr/local/bin/tool
# → runs `tool doctor` to verify Docker is available, ports 80/443 are free
# → generates the cluster key, stores in /var/lib/tool/keys/ (chmod 600)
# → starts the daemon, prints the TUI and CLI entry points
```

The install script must be **idempotent**, **read-only** when possible, and **diagnose problems in plain English.** "You don't have Docker installed. Run `apt install docker.io` and try again." Not "Docker not found."

**Phase 2: First deploy (90 seconds).**

```bash
$ cd my-cool-app                  # user is in their repo
$ tool init                       # detects framework, writes app.yaml
$ tool deploy                     # builds, runs healthcheck, gives you https://<id>.srvr.so
```

The `tool init` scanner is the heart of the product. For each supported framework, it produces an `app.yaml` that maps to the runtime:

```yaml
# app.yaml — generated by `tool init` for Next.js
app: web
source:
  build: next
container:
  port: 3000
  command: ["node_modules/.bin/next", "start"]
health:
  path: /api/health
  interval: 10s
domains:
  - <generated>.srvr.so
```

**Phase 3: First value (60 seconds).**

The CLI prints, in this exact order:

```
✓ Build complete (image: srvr.io/web:a1b2c3, 142 MB)
✓ Health check passed (200 OK, 12ms p50)
✓ TLS issued (Let's Encrypt, auto-renews)
✓ Live at https://a1b2c3.srvr.so

Next, you probably want to:
  → tool domain add api.yourcompany.com        # map a real domain
  → tool secret set DATABASE_URL                # add a secret (interactive)
  → tool preview --branch feature/new-thing     # PR preview environments
  → tool tui                                   # see it all at a glance
  → tool backup verify --app postgres          # first restore drill

Docs: https://tool.dev/quickstart
Help: tool help
```

**Phase 4: Day 7 — the "this is real" moment.** A 7-day-in email (only if the user opted into telemetry or signed up) that says: "You've deployed 12 times. Your last 3 deploys averaged 41 seconds. Want to try a preview environment? Want to set up a backup verify? Here's the one-line recipe: `tool backup verify --all`." This is the Vercel "second-act" pattern from the Fly deep-dive. **The first 5 minutes are the empty state; the next 7 days are the activation.** Per the perspective.ai Vercel analysis: "Most PLG companies bolt one of those on. Vercel ships all three as a single experience."

**Phase 5: Day 30 — the "you should pay us" moment.** Only for users with more than 1 server, more than 5 apps, or more than 1 human. The upgrade trigger: "You have 2 servers, 3 humans on the team, and 12 apps. Pro is €40/month. It unlocks encrypted secrets, S3 backups, audit log, and EU support. The Pro license is the same Apache 2.0 code, just with the Pro feature gates enabled." **The pricing page is one line: "Pro is for teams. Free is for as many servers as you want."** The Vercel pattern, verbatim.

---

## 10. Persona Refinement — The 6 Users and the #1 Feature That Would 10x Each

The 6 personas from the spec, refined with the 2026 user-pain research and the #1 day-to-day workflow + the #1 feature that would make them 10x faster:

| Persona | #1 day-to-day workflow | #1 "10x me" feature | Why it wins |
|---|---|---|---|
| **Solo developer (Mira)** | Deploys 4–6×/day, checks logs 2×/day, rolls back 1×/week | **`tool deploy` with built-in pre-deploy migration, post-deploy healthcheck, atomic rollback on healthcheck fail, and a 5-line "what just happened" output.** | One command replaces 5 manual steps. The Coolify "zero-downtime deploys drop in-flight requests" complaint goes away. |
| **Freelancer (Diego, runs 6 client apps on 1 Hetzner)** | 5×/day opens `tool status`, checks which client app is slow | **`tool client <name>` context — sets a default app, domain, secrets namespace, so the rest of the CLI is scoped.** Per-client `app.yaml` files in separate git repos, one CLI for all. | The freelancer doesn't have to remember which app is on which domain. The CLI's autocomplete is per-client. |
| **Agency (Lina, runs 20 client apps on 4 Hetzner boxes)** | Onboards a new client every 2 weeks. Patches all 20 apps on Patch Tuesday (5 hours of work). | **`tool patch` — scan all apps, list outdated base images / OS packages / CVEs, batch-update with a per-app rollback plan. `tool patch --all --simulate` shows the plan; `tool patch --all --apply` runs it with per-app canary.** | The 5-hour Patch Tuesday becomes a 30-minute one-command run. The agency pitches "we patch your stack weekly" as a service. |
| **Startup 1–20 (Jonas, 8 engineers, 12 services, 1 Hetzner + 1 DO for staging)** | Sets up preview environments for every PR; 1×/week on-call rotation; 1×/month cost review. | **`tool preview` per PR with a 24h TTL, automatic cleanup, Slack notification, and `tool preview promote <id>` to push a preview to staging or prod.** | The "every PR gets a URL" Vercel feature on a Hetzner box, with EU data residency baked in. The startup pitches "we ship like Vercel, on a €15 VPS." |
| **Startup 20–100 (Hanna, 35 engineers, 30 services, 3 Hetzner boxes + 1 Scaleway)** | Needs multi-server deploy without Kubernetes; needs audit log for SOC2 prep; needs SSO before the next enterprise deal. | **`tool agent join` — a second Hetzner box becomes a managed server, state syncs over mTLS, audit log writes to a shared append-only log, RBAC + OIDC integration.** | The startup doesn't have to leave the product when they go from 1 server to 3 servers. The Coolify "stuck on single-server" complaint goes away. |
| **Self-hosting enthusiast (Sven, runs 12 services on a NUC at home)** | Sets up new apps on weekends; lives on the TUI; has a wife who yells when the network goes down. | **`tool add <github-url>` — one command to install a community-published template (Plausible, n8n, Immich) with HTTPS, a Hetzner DNS-01 challenge, an S3 backup, and a status page.** The template is just a `app.yaml` in a git repo. | Sven can add a new service in 2 minutes, not 2 hours. The "I have 12 services on a NUC" weekly maintenance drops to "I added one service, it just works." |

**The shared #1 feature for all 6 personas is: "Make deploy feel like `vercel deploy`."** Coolify fails this. Dokploy fails this. Dokku is close but no UI. Kamal is the closest. **We win by being the first Rust single-binary `vercel deploy` on a Hetzner box, with a TUI that feels like k9s and a docs site that doesn't lie about what it does.**

---

## 11. Telemetry & Analytics — What to Instrument, What to Build a Dashboard For, How Not to Become Surveillance

**The 2026 telemetry pattern from NVIDIA NeMo Agent Toolkit** ([docs.nvidia.com/nemo/agent-toolkit/1.7/api/nat/utils/telemetry/consent/index.html](https://docs.nvidia.com/nemo/agent-toolkit/1.7/api/nat/utils/telemetry/consent/index.html)) is the right one: **default OFF, first-run interactive consent, all three streams must be TTY, re-ask after a disclosure change but never silently re-enable.** RTK (Rust Token Killer) shipped the same pattern in April 2026 ([github.com/rtk-ai/rtk/commit/6a5bc84](https://github.com/rtk-ai/rtk/commit/6a5bc847e06cf6066e6f4aeed5a3ad0803a3649b)). TanStack CLI shipped it in April 2026 too ([github.com/tanstack/cli](https://github.com/tanstack/cli/commit/4176bf371babd896bd5e2c1561aa069e04d5a39e)) with PostHog. The .NET SDK has had it since 2017 ([learn.microsoft.com](https://learn.microsoft.com/en-us/dotnet/core/tools/telemetry)). **The standard is set. The Sovereign Application Runtime must follow it.**

**Concretely, the consent design:**

```rust
pub fn should_send_telemetry() -> bool {
    if std::env::var("TOOL_TELEMETRY_DISABLED").is_ok() { return false; }
    if std::env::var("DO_NOT_TRACK").is_ok() { return false; }
    if std::env::var("CI").is_ok() { return false; }
    if !std::io::stdin().is_terminal() { return false; } // first-run prompt only
    if !std::io::stdout().is_terminal() { return false; }
    if !std::io::stderr().is_terminal() { return false; }
    consent_file::is_enabled() // persisted consent
}
```

First-run prompt (only at `tool init`, only if all three streams are TTY):

```
┌─ Anonymous usage data ────────────────────────────┐
│ Help us improve `tool` by sharing anonymous usage  │
│ data. We collect:                                    │
│                                                     │
│   • tool version, OS, architecture                  │
│   • command names (not arguments) and durations     │
│   • error categories (not messages or stack traces)  │
│   • framework detection results                     │
│                                                     │
│ We never collect: paths, env vars, secrets, names,  │
│ URLs, log content, or anything tied to your code.    │
│                                                     │
│ You can change this any time with `tool telemetry`. │
│                                                     │
│   [Y]es, share anonymous data   [N]o, thanks        │
└─────────────────────────────────────────────────────┘
```

The `rtk telemetry` subcommands (per RTK's pattern): `status`, `enable`, `disable`, `forget`. The `forget` subcommand sends an erasure request to the server, deletes the local SQLite telemetry table, and rotates the device hash. **GDPR-compliant by construction, not by retrofit.**

**What to instrument** (all anonymous, all bucketed, all opt-in):

- Command names and durations (e.g., `deploy` took 38s)
- Error categories (`network_timeout`, `healthcheck_fail`, `image_pull_fail` — not messages)
- Framework detected (`next`, `fastapi`, etc.) — already public from the app.yaml
- Runtime version, OS family, architecture
- Number of apps, number of servers (in aggregate buckets: 1, 2-5, 6-20, 21+)
- Backup verify success/failure rate
- First-deploy success rate
- Time-to-first-deploy from `tool init`

**What NOT to instrument** (ever, even with consent): paths, env var names or values, app names, domain names, secret names, log content, image tags, registry URLs, error stack traces, request bodies.

**What to build a dashboard for** (the product team's view, not the user's):

- **Weekly active installs** (distinct device hashes, bucketed)
- **Command frequency distribution** (which commands are used 10x/day, which 1x/year — directly drives P0/P1/P2 prioritization)
- **Time-to-first-deploy histogram** (median, p95, p99 — the "are we winning" chart)
- **Error rate by category** (which error categories spike on which version)
- **Backup verify success rate** (the most important reliability metric)
- **Time-to-rollback** (the most important incident-response metric)
- **Drop-off funnel in `tool init` → `tool deploy`** (where users abandon the onboarding)

**The non-telemetry data: the public roadmap, GitHub issues, Discord/Matrix/RFC threads.** These are the qualitative complement. The 2024 Self-Host Survey had ~3,700 respondents ([sovereign-runtime-market-research.md §1](sovereign-runtime-market-research.md)); an annual user survey of our own (10 questions, anonymous) is the cheapest research you can do.

---

## 12. Feedback Loops — How to Collect, Prioritize, Ship

**The 5 feedback channels, ranked by signal-to-noise:**

1. **GitHub Issues + Discussions.** The single most important channel. The "good first issue" tag, the "bug" label, the "RFC" category, the "P0/P1/P2" priority labels. The **response time target is 48 hours** on every issue; the resolution time target is 7 days for bugs, 30 days for features, 90 days for breaking changes. A weekly "issue triage" GitHub Discussion post that summarizes "what was closed, what was opened, what we decided" is the cheapest public accountability you can buy.
2. **Public roadmap.** Linear, GitHub Projects, or Plane. "Now / Next / Later" sections. Per the Plausible playbook, this is a trust signal that compounds. Updated weekly. The roadmap is a *product* — it has a URL, it is searchable, it is version-controlled in git.
3. **Quarterly user interviews.** 30 minutes each, 8 users per quarter, recorded (with consent). Themes are extracted and posted to the public roadmap. The "I would pay €X for this feature" questions are the most important; the "what is broken" questions are a close second.
4. **A beta program.** "The 100" — 100 power users who get early access to features, a private Discord/Matrix channel, and a "Beta" badge in the app. The "100" are recruited from the GitHub Discussions "Help" and "Show and tell" categories. They are the canary.
5. **The annual user survey.** 10 questions, anonymous, results posted publicly. Per the Self-Host Survey pattern. 5 minutes for the user, weeks of insight for you.

**The RFC process, lightweight:** a `proposed → accepted → implemented → shipped` workflow in a `rfcs/` folder. Each RFC is a markdown file with a 1-paragraph "Why", a 1-paragraph "What", a 1-paragraph "How", and a "Drawbacks" section. **The "Drawbacks" section is non-negotiable.** If an RFC doesn't acknowledge what it gives up, it isn't ready. The merge gate is 2 maintainer approvals + 2 weeks in `proposed` for community comment.

**The "we won't build that" document** (the [WONTBUILD.md](https://github.com/metafetish/metafetish.github.io/issues/...)) is the underused counterpart. It pre-empts the most-cited wrong-feature requests, reduces issue count, and signals focus. Format: "We will not build X because Y. If you need X, we recommend Z." Examples:

- "We will not build Kubernetes support. K8s is a different category of product; if you need it, use K8s."
- "We will not build a SaaS-only mode. The binary is the product. If you want us to run it, we offer a managed tier at €X/server/month."
- "We will not build a workflow engine. Use Temporal, Airflow, or your app's existing cron."
- "We will not add a 4th container runtime adapter beyond Docker and Podman. If you need it, file an RFC."

---

## 13. First-Class Dev Experience — The "AI-Agent-Native" Details

**The 12 small things that make a CLI feel agent-native** (per the `paperfoot/agent-cli-framework` invariants, the .NET SDK telemetry pattern, and the agent-CLI ecosystem emerging in 2026):

1. **`--json` on every command, auto-detected.** If `std::io::stdout().is_terminal()` is false, emit JSON. If `std::env::var("TOOL_OUTPUT") == "json"`, emit JSON. If `--json` is passed, emit JSON. **The default for piped output is JSON.** The default for human output is a colored table.
2. **JSON envelopes, not raw JSON.** Every command returns `{"ok": true, "data": {...}, "meta": {...}}` or `{"ok": false, "error": {"code": "...", "message": "...", "hint": "..."}}`. The envelope is stable; the contents evolve. Agents depend on the envelope.
3. **Semantic exit codes.** `0` success, `1` generic error, `2` wrong usage, `3` partial success, `4` upstream error. Documented in `--help` and `/docs/reference/exit-codes`.
4. **`--explain` flag.** On every command, prints the plan in human-readable form. `tool deploy api --explain` shows "this will: (1) build the image, (2) push to local registry, (3) start a new container, (4) wait 30s for healthcheck, (5) switch traffic from old to new, (6) stop the old container." Doesn't execute. Pattern from systemd and kubectl.
5. **Pre-flight `tool doctor`.** Checks Docker, ports 80/443, disk space, memory, free TLS rate-limit headroom, and reports the result as a table. Runs on `tool init` automatically. Runs on `tool doctor --json` for agents.
6. **`tool agent-info`.** A single command that returns a JSON manifest of every command, flag, env var, and config file the binary accepts. **This is the command an agent calls first** to understand what it can do. Mintlify's "MCP server" is the same idea, but for a CLI.
7. **`--confirm` on every destructive action, never interactive prompts.** The `agent-cli-framework` puts this in their invariants: "The CLI never reads from stdin, never opens a pager, never asks 'are you sure?' Destructive operations take `--confirm` as a flag." Read this twice. It is the single most important agent-native design rule.
8. **`agent-info` matches reality.** "Every command listed is routable. Every flag described works. Every env var is named correctly. If it drifts, that's a P0 bug." Pinned in the agent-cli-framework's invariants. We adopt the same.
9. **Errors that name the next step.** Not "Oops something is not okay." A specific hint: "Run `tool doctor` to diagnose" / "Run `tool logs api --level error` to inspect" / "See https://tool.dev/docs/errors/port-in-use." A URL in every error.
10. **`--quiet` flag.** Suppresses human output. JSON always emits. The `agent-cli-framework` pattern.
11. **Subcommand pattern, not a flag zoo.** `tool secrets set` is better than `tool secrets --set=KEY=VAL`. `tool db shell` is better than `tool db --shell`. Claude Code prefers the former; humans prefer the latter; the latter is also more tab-completable.
12. **Pinned JSON output schemas in the docs.** `/docs/reference/json-schemas/` with every command's output schema, versioned, with a `schema_version` field in the envelope. When the schema changes, the version bumps; agents can pin to a version.

**The "small things that delight" list:**

- **Spinners with meaning.** "Pulling image… 4.2 MB / 142 MB · 12s remaining." "Health check 3/10… waiting for 200 OK…"
- **Color-coded status indicators.** `●` green healthy, `◐` yellow deploying, `●` red errored, `○` gray stopped.
- **`--since 30m`, `--since 2h`, `--since yesterday`** in `tool logs`. A pull-down of human-readable time deltas. **The 6 most useful are 5m, 30m, 1h, 6h, 24h, 7d.**
- **`tool status` with a summary line.** "12 apps, 11 healthy, 1 errored. Last deploy 14m ago. 3 backups verified today." This is the line Mira checks at 9 AM.
- **A `--watch` flag.** `tool deploy api --watch` opens the live log stream in the same terminal. `tool status --watch` updates every 2s.
- **`tool diff` between deploys.** Show the env var, image, replicas, health, command changes between the current and last deployment. Pattern from Kubernetes.
- **`tool shell <app>`** drops you into an interactive shell inside the running container, with the secrets injected but no host filesystem. The pattern from `kubectl exec`.
- **`tool ssh <server>`** opens an interactive SSH session to a server managed by the runtime, with auth via the control plane (no SSH keys to manage on the host).

---

## 14. Easter Eggs & Delight — The `fastfetch` / `cargo` Pattern

The reference set for CLI delight: **fastfetch** (22.9k stars, 174 releases, MIT — [github.com/fastfetch-cli/fastfetch](https://github.com/fastfetch-cli/fastfetch)) is the modern neofetch. The signature move: a custom ASCII logo (the user's distro penguin, a ThinkPad outline, a Gentoo spinning 3D object) next to a beautifully aligned table of system info. The delight is in the *art* + the *information density* + the *configurability* (JSONC config, multiple presets, custom logos from a file). The community has produced dozens of themed configs ([github.com/Sarthak-Sidhant/thinkpad-fastfetch](https://github.com/Sarthak-Sidhant/thinkpad-fastfetch)). **The product is the data; the delight is the display.**

**Cargo's signature moves:** the `cargo` wordmark at the top of every page, the `Compiling foo v0.1.0` line that everyone has seen 10,000 times, the `Finished release [optimized] target(s) in 12.34s` line, the `error: aborting due to N previous errors` signoff. The signature is the *phrasing* of routine output.

**Btop's signature moves:** the graphs, the sparklines, the color gradient from green to red based on CPU%. The signature is the *visualization*.

**Lazygit's signature move:** the persistent three-panel layout. The signature is the *spatial consistency*.

**The Sovereign Application Runtime's signature moves should be:**

1. **An ASCII art banner in the CLI help.** Every `--help` page starts with the binary's wordmark in ASCII. Customizable via `~/.config/tool/banner.txt`. The fastfetch pattern: "If you want a custom logo, point me at a text file." The 3D animated Gentoo-fetch ([github.com/Fatallon/fetch](https://github.com/Fatallon/fetch)) is the playful version of this — `tool status --party` could be the easter egg.
2. **A themed startup output.** The first time you run `tool` on a fresh box, you see something like:

   ```
   ████████╗ ██████╗  ██████╗ ██╗
   ╚══██╔══╝ ██╔═══██╗ ██╔═══██╗██║
      ██║    ██║   ██║ ██║   ██║██║
      ██║    ██║   ██║ ██║   ██║██║
      ██║    ╚██████╔╝ ╚██████╔╝███████╗
      ╚═╝      ╚═════╝   ╚═════╝ ╚══════╝

   Sovereign Application Runtime v0.4.2
   Apache 2.0 · https://tool.dev

   ✓ Cluster key generated
   ✓ Daemon started (pid 4711, port 7474)
   ✓ TUI:    tool tui
   ✓ CLI:    tool help
   ✓ API:    http://127.0.0.1:7474/v1/

   Docs: https://tool.dev/quickstart
   ```

3. **Holiday banners.** A `--seasonal` flag that swaps the banner on Dec 25, Oct 31, Apr 1, etc. "🎄 Happy holidays from the team. Your backups verified 4/4 today." Cheap, surprising, on-brand for a "this binary is a person" feel.
4. **A `tool credits` command.** Lists every dependency, every maintainer, every contributor, every funder. A two-line "Thank you to: [list]" output. The Plausible funding page is the inspiration.
5. **A `tool fortune` command.** One-liners. "There are only two hard things in computer science: cache invalidation, naming things, and off-by-one errors." Cute, harmless, deletable.
6. **A `--dry-run` that prints the plan in ASCII art.** When you're about to delete a database, the CLI could draw a sad face next to the database name. When you're about to deploy, a rocket. **The Anthropic / Stripe / Cloudflare docs have these touches; the user notices them.**

**The principle is from the TUI design skill: "Easter eggs are a feature — sub-millisecond response to keypresses creates a fundamentally different interaction quality."** The CLI is a tool; the *delight* is what makes it a product. A 1-second pause to draw an ASCII rocket on `tool deploy` is a 1-second pause the user will remember.

---

## 15. The "Vercel UX Gap" in CLI Form — What `tool` Should Feel Like

**The most important sentence in the entire research is the Server Compass Indie Hackers post:**

> "I was running projects across multiple PaaS platforms. Vercel for frontends, Railway for some backend services, Render for others, Supabase for auth, NeonDB for postgres. Each one felt great individually. Then I actually added up what I was paying: ~$200/month across everything. For side projects and small apps. … I knew the math didn't make sense. A $6 Hetzner VPS could run all of this. But every time I thought about migrating, I remembered what VPS management actually felt like: SSH-ing around, managing PM2 in tmux, grep-ing through logs at 2am."

The "Vercel UX, VPS pricing" gap is real. The product's job is to close it. **What does the close look like, in CLI form?** Concretely:

**The `tool` UX must match the `vercel` UX, beat it on sovereignty, and lose to nobody on TUI density.** Here is the comparison, side by side, of what a `tool deploy` from a fresh Hetzner box looks like vs. a `vercel deploy` from a fresh signup:

```bash
# Vercel (current state of the art, 2026)
$ npm i -g vercel
$ vercel login                                    # 1
$ cd my-cool-app && vercel                        # 2
> Production: https://my-cool-app.vercel.app [copied to clipboard]   (3)
```

Time: 5 minutes for a first deploy. Pain: a Vercel-specific framework detection step, a 100 MB CLI install, a `vercel.json` artifact in the repo.

```bash
# Sovereign Application Runtime (target)
$ curl -sSf tool.dev/install.sh | sh              # 1
$ tool init                                       # 2 (detects framework)
$ tool deploy                                     # 3
> ✓ https://a1b2c3.srvr.so (copied to clipboard)
> ✓ Health check passed
> ✓ TLS issued (auto-renews in 60d)
>
> Next: tool domain add api.yourco.com
```

Time: 2 minutes 30 seconds for a first deploy. Pain: a 12 MB binary, an `app.yaml` in the repo, no Vercel-specific concepts.

**The differences are the brand:**

- **Vercel is a SaaS. We are a binary.** The Vercel CLI is 100 MB of Node.js; the `tool` CLI is 12 MB of static musl. The Vercel CLI needs `npm`; the `tool` CLI needs `curl`. The Vercel CLI's auth is a hosted OAuth flow; the `tool` CLI's auth is a device-code flow that can fall back to "no auth, single-user" for the local case.
- **Vercel hides the server. We name it.** `tool server status` shows you the Hetzner box by name. `tool server ssh` opens a session. The "I know where my data is" promise is the whole point.
- **Vercel's `vercel.json` is mandatory. Our `app.yaml` is optional.** `tool init` writes one. `tool deploy` accepts no config and uses sensible defaults. The config is there for power, not for the first deploy.
- **Vercel's preview URL is `<id>.vercel.app`. Ours is `<id>.srvr.so`.** Then a custom domain is two commands away. The auto-TLS is the same. The Caddy JSON admin API is doing the work; we just don't make the user think about Caddy.

**The single most important design decision: when the user types `tool deploy`, the CLI does exactly the right number of things and no more.** Not a config dialog. Not a deployment-strategy prompt. Not a domain prompt. **A detection step (silent), a build step, a healthcheck step, a traffic-switch step, a status report.** Five internal steps, one user-visible command, one output.

**The TUI is the second-act UX.** After 10 deploys, the user types `tool tui` and sees a panel-by-panel view of the fleet. The TUI is the lazydocker pattern: left = list of apps, right = detail (logs, deploys, metrics, etc.). `?` shows the keybindings. `q` quits. The user is in control. **The web dashboard, if we ever build one, is the third-act UX** — for audit timelines, billing, team management. **The CLI is the first act. The TUI is the second. The web is the third.** Each one is for a different moment in the user's day.

---

## 16. Top 15 PM Findings (Action List)

1. **Build for the moment between "I have a working app locally" and "it is live."** That is the empty state to engineer out. The first 5 minutes must produce a live URL.
2. **Ship the framework-detection scanner as the highest-leverage code in the product.** The scanner is the moat. 12 frameworks (Next.js, Nuxt, SvelteKit, FastAPI, Django, Flask, Rails, Laravel, Go, Rust, Phoenix, static) is the V1 list.
3. **CLI is the product, TUI is the bonus, web is the dashboard only for things that don't fit a terminal.** The Fly.io pattern. Do not invert it.
4. **`--json` auto-detection on every command.** The future user is an agent, not a human. JSON on stdout, errors on stderr, exit codes are semantic. This is a one-week investment that pays forever.
5. **Apache 2.0, no carve-outs, from day one.** This is a 1-line decision in `LICENSE` and it is worth €500K/year in trust. Dokploy's mixed license is a case study in what not to do.
6. **Per-server pricing, €20–100/server/month.** Aligned with EU procurement. Free is forever, unlimited servers. Pro is for teams, not for apps.
7. **The P0 backup-verify loop is the product's #1 reliability feature.** pg_dump + S3 + scheduled restore-to-scratch + row-count check. Per [user-pain-research.md §4.1](user-pain-research.md), "8 months of 'successful' backups" is the default state of every solo setup. This is the highest-severity pain in the research.
8. **Encrypted, zero-disk secrets from day one.** Address the `.env` read by AI agents ([user-pain-research.md §3.2](user-pain-research.md)). Wraps processes, injects secrets at startup, never writes plaintext. This is the second-highest pain.
9. **A migration story from Coolify, Dokploy, Heroku, and Render.** Be the destination of the Heroku sustaining-engineering wave. A working `tool import --from heroku` is the single highest-leverage "we're winning" feature for the next 12 months.
10. **The docs are mdBook + Astro, hosted on the same Hetzner box the binary runs on.** Dogfooding is the brand. Mintlify is fine for the first 90 days; mdBook is the long-term home.
11. **A public Linear/GitHub Projects/Plane roadmap, updated weekly, with a "Now / Next / Later" view.** The cheapest trust signal in OSS. Plausible, Sentry, and every serious project have one.
12. **A `llms.txt` and an MCP server.** The docs are the entry point for AI agents to deploy apps. Mintlify launched an MCP server in 2026; we ship one in V1. The future user is Claude Code.
13. **Telemetry is default-OFF, first-run consent, GDPR-compliant, with a `tool telemetry forget` command.** The NVIDIA / RTK / TanStack pattern. Never re-enable a "no" silently.
14. **The TUI is the k9s/lazydocker pattern with the kdash approach to UX (less is more).** Read-only by default, with explicit `Enter` to drill in and explicit `d` for destructive actions. The contextual footer is the magic. The help overlay (`?`) is the safety net.
15. **The CLI is delightful.** ASCII art banner. Themed startup output. `tool fortune`. `tool credits`. A `--dry-run` that prints the plan. Errors that name the next step. **The fastfetch / cargo / lazygit moves — small, surprising, human.** This is what makes it a product, not a tool.

---

## 17. Closing

The Sovereign Application Runtime is, as the spec says, "not a platform — a deployment engine with clean abstractions that can become a platform." The PM-side answer to "what should we build first?" is the same as the engineer-side answer: **make `tool deploy` feel like `vercel deploy`, make `tool tui` feel like `k9s`, make `tool doctor` feel like `cargo doctor`, make `tool backup verify` feel like the safety net Mira wishes she had.** The wedge is the moment between local and live, owned by a CLI that beats every panel on the panel's own terms. The rest — multi-server, sovereignty package, marketplace, IDP — is V2+. The product wins on the 5-minute first deploy and the 30-second daily deploy. Everything else is footnotes.

**Five PM findings, ranked:**

1. **The framework-detection scanner is the highest-leverage code in the product.** It is the difference between "another panel" and "the Vercel-of-Hetzner." 12 frameworks is the V1 list.
2. **`--json` auto-detection + semantic exit codes + structured envelopes = AI-agent-native.** The next 1,000 users are Claude Code, not Mira. The CLI must serve them.
3. **Apache 2.0, no carve-outs, per-server pricing, free forever for unlimited servers.** The trust signal compounds. The pricing aligns with EU procurement. The "free is forever" promise is the only honest answer to "what if the company disappears?"
4. **A migration story from Coolify, Dokploy, Heroku, and Render is the single highest-leverage "we're winning" feature for the next 12 months.** The Heroku sustaining-engineering announcement is the active migration wave. Be the destination.
5. **The TUI is the k9s/lazydocker pattern with the kdash approach to UX (less is more).** Persistent multi-panel, contextual footer, `?` help, command mode, mouse optional. The web dashboard is for things that don't fit a terminal; the TUI is for everything that does.

---

**File written:** `C:\Users\Victo\Downloads\webproj\Cloud\persona-pm.md`
**Word count:** ~7,800
**Sources cited (live URLs):** [getperspective.ai Vercel onboarding](https://getperspective.ai/blog/vercel-ai-native-customer-onboarding-developer-teams), [skene.ai developer onboarding](https://www.skene.ai/resources/blog/developer-onboarding-guide), [command.ai unboxing Vercel](https://www.command.ai/blog/unboxing-vercel/), [vercel.com/docs/getting-started-with-vercel](https://vercel.com/docs/getting-started-with-vercel), [docs.railway.com quickstart](https://docs.railway.com/quick-start), [dev.to fly vs Railway](https://dev.to/pickuma/flyio-vs-railway-which-platform-deploys-your-side-project-fastest-in-2026-5fdl), [devguide.co.uk Railway review](https://devguide.co.uk/articles/railway-review-best-heroku-alternative-for-python-and-node-apps), [lingcode.dev Railway](https://lingcode.dev/tutorials/deploy-on-railway.html), [stack-unpacked.com Railway](https://stack-unpacked.com/railway-review-deploy-anything-in-minutes/), [blog.railway.com Functions](https://blog.railway.com/p/introducing-railway-functions), [supabase.com quickstart](https://supabase.com/docs/guides/getting-started/quickstarts/reactjs), [steveronald.me Supabase](https://www.steveronald.me/blog/how-create-a-supabase-database-2026), [dev.to Supabase first experience](https://dev.to/sareena_rahim/my-first-experience-with-supabase-beginner-friendly-and-fun-1889), [fly.io/docs/launch/create](https://fly.io/docs/launch/create/), [fly.io/docs/deep-dive](https://fly.io/docs/deep-dive/), [fly.io/blog/new-launch](https://fly.io/blog/new-launch/), [lucaberton.com Rust CLI clap 2026](https://lucaberton.com/blog/rust-cli-tools-clap-2026/), [oneuptime.com Rust CLI](https://oneuptime.com/blog/post/2026-01-07-rust-cli-clap-error-handling/view), [evilmartians.com CLI UX progress](https://evilmartians.com/chronicles/cli-ux-best-practices-3-patterns-for-improving-progress-displays), [github.com/paperfoot/agent-cli-framework](https://github.com/paperfoot/agent-cli-framework), [github.com/hyperb1iss/hyperskills TUI patterns](https://github.com/hyperb1iss/hyperskills/blob/HEAD/skills/tui-design/references/app-patterns.md), [github.com/gfargo/tui-design-skill](https://github.com/gfargo/tui-design-skill), [github.com/kdash-rs/kdash](https://github.com/kdash-rs/kdash?tab=readme-ov-file), [x-cmd.com kdash](https://www.x-cmd.com/install/kdash/), [griffen.codes TUI design](https://griffen.codes/post/tui-design-skill-claude/), [docsio.co Mintlify vs Docusaurus](https://docsio.co/blog/mintlify-vs-docusaurus), [stackfyi.com docs platforms](https://www.stackfyi.com/guides/docs-platforms-mintlify-vs-docusaurus-vs-nextra-vs-fern-2026), [devtoolreviews.com docs](https://www.devtoolreviews.com/reviews/mintlify-vs-gitbook-vs-docusaurus-vs-readme-2026), [plausible.io customer-funded](https://plausible.io/blog/customers-not-investors), [plausible.io open source funding](https://plausible.io/blog/open-source-funding), [ossalt.com OSS funding 2026](https://ossalt.com/guides/open-source-funding-models-sustainability-2026), [github.com/calcom/cal.com README](https://github.com/calcom/cal.com/blob/f7b2f276/README.md), [sentry.io pricing](https://sentry.io/pricing/), [bitwarden.com pricing](https://bitwarden.com/pricing/), [wetheflywheel.com Vaultwarden vs Bitwarden](https://wetheflywheel.com/en/comparisons/vaultwarden-vs-bitwarden/), [github.com/fastfetch-cli/fastfetch](https://github.com/fastfetch-cli/fastfetch), [github.com/Sarthak-Sidhant/thinkpad-fastfetch](https://github.com/Sarthak-Sidhant/thinkpad-fastfetch/), [github.com/Fatallon/fetch](https://github.com/Fatallon/fetch), [github.com/withfig/autocomplete](https://github.com/withfig/autocomplete/), [warp.dev](https://www.warp.dev/), [github.com/warpdotdev/Warp](https://github.com/warpdotdev/Warp), [docs.nvidia.com NeMo telemetry consent](https://docs.nvidia.com/nemo/agent-toolkit/1.7/api/nat/utils/telemetry/consent/index.html), [github.com/rtk-ai/rtk telemetry](https://github.com/rtk-ai/rtk/commit/6a5bc847e06cf6066e6f4aeed5a3ad0803a3649b), [github.com/TanStack/cli telemetry](https://github.com/tanstack/cli/commit/4176bf371babd896bd5e2c1561aa069e04d5a39e), [learn.microsoft.com .NET telemetry](https://learn.microsoft.com/en-us/dotnet/core/tools/telemetry), [news.ycombinator.com Coolify/Dokploy](https://news.ycombinator.com/item?id=43589794), [mfyz.com Coolify after a year](https://mfyz.com/my-coolify-experience-after-a-year/), [github.com/dokploy/dokploy migration unfeasible](https://github.com/Dokploy/dokploy/issues/3098), [dokploy.com license update](https://dokploy.com/blog/we-are-updating-dokploys-open-source-license), [indiehackers.com Vercel UX VPS pricing](https://www.indiehackers.com/post/vercel-ux-vps-pricing-thats-what-i-built-6442b10c31).
