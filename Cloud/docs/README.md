# Sovereign Application Runtime — Documentation

**Product:** Rust-based, self-hosted, single-binary, CLI/TUI/API-first deployment runtime.
**Audience:** founder, lead engineer, first 5 employees, design partners, early investors.
**Status:** Pre-MVP. This documentation is the source of truth for what we are building, in what order, and what we are deliberately refusing to build.

---

## 0. How to read this folder

If you have **15 minutes:** read [`architecture.md`](./architecture.md) §1-3 and [`negative-prompt.md`](./negative-prompt.md) §1.
If you have **1 hour:** read [`architecture.md`](./architecture.md), [`tech-stack.md`](./tech-stack.md), and the phase file for where we are today.
If you are **about to write code:** read [`architecture.md`](./architecture.md) end-to-end, then [`tech-stack.md`](./tech-stack.md) §10 (the static linking contract), then the phase file for the current sprint.
If you are **on-call / paged:** read [`operations-runbook.md`](./operations-runbook.md) §1.1 (the 3am playbook) and [`doctor.md`](./doctor.md) §1.
If you are **about to install / upgrade / uninstall the binary:** read [`product-ux.md`](./product-ux.md) §11 (the end-user contract) and [`tech-stack.md`](./tech-stack.md) §12 (the `install.sh` spec).
If you are **deciding whether to invest:** read [`negative-prompt.md`](./negative-prompt.md) first. If the anti-patterns resonate, the rest of the docs will tell you how we plan to avoid them.
If you are **an enterprise / EU public-sector buyer:** read [`enterprise-readiness.md`](./enterprise-readiness.md) §1-3 (support tiers, SLAs, framework mappings) and §3.3-3.4 (BSI C5:2026, EUCS Substantial).
If you are **a CISO / auditor:** read [`enterprise-readiness.md`](./enterprise-readiness.md) §3 (SOC 2, ISO 27001, BSI C5, EUCS, GDPR mappings), §4 (the pen-test plan), and §5 (the DPA template).

The parent-folder files (`../Research-2.md`, `../Research-3.md`, `../persona-*.md`, `../Executive-Summary.md`) are the long-form research that produced this docs set. Treat this folder as the *implementation contract*; treat the parent folder as the *reasoning behind* the contract.

---

## 1. The four invariants

Every recommendation in this folder is filtered through four invariants. If a proposed change violates any of them, the change is rejected without further discussion.

1. **One operator can run it for 5 years.** Not three SREs. Not a rotation. One human on a Monday morning. This kills Kubernetes, kills service mesh, kills any feature requiring tribal knowledge to operate. **`sovereign doctor` is the on-call's first command** — diagnosis is one CLI invocation, not a 7-tool tour.
2. **A product in 6 months, not a platform in 36.** Scope is the moat. Every phase is opinionated about what to *cut* as much as what to *add*.
3. **"Sovereign" is a structural claim, not a feature.** A Swiss-German non-profit that publishes SBOMs, ships in-tree EU defaults, and can drop US dependencies on 30 days' notice *is* sovereign. A US LLC with encryption-at-rest is not. Sovereignty is decided by the corporate graph, not the feature list. **`sovereign sovereignty-test` is a doctor subcommand** — the 10-point test is verifiable in one command.
4. **The CLI is the product.** TUI is the daily-driver bonus. Web UI is a V2 opt-in for non-terminal users. Documentation is auto-generated from the CLI parser, so it never lies. The doctor is a CLI, the incident toolkit is a CLI, the canary is a CLI, the compliance scan is a CLI.

---

## 2. The docs map

| File | Purpose | Read when |
|---|---|---|
| [`architecture.md`](./architecture.md) | Hexagonal layout, data model, state machines, API conventions, failure modes, multi-tenancy | Before writing any code |
| [`tech-stack.md`](./tech-stack.md) | The locked crate stack with rationale, version pins, allocator choice, anti-picks | Before `Cargo.toml` is edited |
| [`phase-00-mvp.md`](./phase-00-mvp.md) | Weeks 1-6. 10 features (incl. basic doctor + self-update). 1 developer can use it. Detailed sub-tasks per feature | The first 6 weeks of work |
| [`phase-0.6.md`](./phase-0.6.md) | V0.6 → V1.0 (~6 months). The hosting control plane: system-service adapters (nginx, mysql, vsftpd, phpmyadmin, LE), user accounts + RBAC + OIDC, per-user Telegram chatops. Ubuntu 22.04 LTS only. Three releases: v0.1.0 (V0 close-out) → v0.6.0 (system services + auth) → v1.0.0 (chatops + pen-test). | After V0 close-out |
| [`phase-01-v1.md`](./phase-01-v1.md) | Weeks 7-21. TUI, scanners, daily-driver, observability, doctor standard + --fix + --explain + bench + cost, **+ OIDC SSO via Authentik/Keycloak/Okta/Entra ID (G21), log shipping to Loki/Datadog/Splunk/Better Stack (G22), append-only signed audit log with CSV/JSONL/Parquet export (G23)**. 23 features | Months 2-5 |
| [`phase-02-v15.md`](./phase-02-v15.md) | Months 5-9. Multi-server agent pattern, RBAC, audit log, declarative mode, doctor full + fleet + incident + retro + game-day, **+ import --from Heroku/Coolify/Dokploy/Render, sovereign dns (7 providers), db pool (PgBouncer/ProxySQL/redis-pool), self-hosted status page**. 23 features | Months 5-9 |
| [`phase-03-v2.md`](./phase-03-v2.md) | Months 10-18. rqlite HA, OPA/Rego, ML scorer, EUCS, web UI, Pro tier, doctor paranoid + compliance + canary + auto-tune. 19 features | Months 9-18 |
| [`doctor.md`](./doctor.md) | The full diagnostic engine spec: 5 levels, 14 categories, 4 subcommands, incident toolkit, retro, game-day, KB, privacy. The on-call's first command. | When paged, when building diagnostic features, when designing observability |
| [`implementation-guide.md`](./implementation-guide.md) | Dev env setup, build, test, fuzz, bench, release, debugging, the Cargo workspace | First day on the codebase |
| [`operations-runbook.md`](./operations-runbook.md) | Morning commands, SLOs, 10 alerts, backup strategy, DR, observability stack, doctor↔alert loop | Running in production |
| [`product-ux.md`](./product-ux.md) | CLI/TUI design, the 5-minute moment, onboarding, pricing, telemetry | Building user-facing surfaces |
| [`sovereignty-and-governance.md`](./sovereignty-and-governance.md) | The 10-point sovereignty test, EU incorporation, OPA/Rego policy, ML scorer, risk scoring | Selling to EU public sector / regulated industry |
| [`enterprise-readiness.md`](./enterprise-readiness.md) | The 3 support tiers + SLAs, SOC 2 / ISO 27001 / BSI C5:2026 / EUCS / GDPR mappings, the penetration test plan, the DPA template, the enterprise onboarding playbook, the "what we explicitly do NOT promise" list | Selling to an MNC or EU public-sector buyer; answering the CISO's questionnaire; the pen-test artefact; the audit evidence |
| [`decision-records.md`](./decision-records.md) | Architecture Decision Records (ADRs) for every locked decision | When someone asks "why did you pick X?" |
| [`negative-prompt.md`](./negative-prompt.md) | Anti-patterns, named case studies, the things we deliberately refuse to build | When someone proposes a "small" feature addition |

---

## 3. The build order (TL;DR)

```text
Phase 0  (Weeks 1-6)   → 10 features: engine + basic doctor + self-update, single binary, single host, SQLite
Phase 0.6 (Months 2-8) → 6 sub-phases: system-service adapters (nginx, mysql, vsftpd, LE, phpMyAdmin), user accounts + RBAC + OIDC, per-user Telegram chatops. 3 releases: v0.1.0 (V0 close-out) → v0.6.0 (system services + auth) → v1.0.0 (chatops + pen-test). See [phase-0.6.md](./phase-0.6.md).
Phase 1   (Weeks 7-21) → 23 features: + TUI, scanners, daily-driver, observability, system packages, doctor standard + --fix + --explain + bench + cost, **OIDC SSO via Authentik/Keycloak/Okta/Entra ID (G21), log shipping to Loki/Datadog/Splunk/Better Stack (G22), append-only signed audit log with CSV/JSONL/Parquet export (G23)**
Phase 2   (Months 5-9) → 23 features: + agent pattern (no rqlite yet), RBAC, audit, declarative apply, doctor full + fleet + incident + retro + game-day, **import --from (Heroku/Coolify/Dokploy/Render), sovereign dns (7 providers), db pool (PgBouncer/ProxySQL/redis-pool), self-hosted status page**
Phase 3   (Months 10-18)→ 19 features: + rqlite HA, OPA/Rego, ML scorer, EUCS, web UI, Pro tier, doctor paranoid + compliance + canary + auto-tune
```

**Current focus (as of the V0.5 cut):** the V0 close-out. Step 9 (Hetzner CX22 end-to-end evidence), Step 10 (tag v0.1.0 from main), and the Show HN post. The V0.6 phase opens once v0.1.0 is tagged.

The `sovereign doctor` is shipped in **every phase** and is the single most important CLI command after `sovereign deploy`. See [`doctor.md`](./doctor.md) for the full spec.

Each phase ends with a **Definition of Done** that is verifiable, not aspirational. Phases cannot be skipped, shortened, or parallelized. The 6-week MVP is the bet we cannot afford to be wrong about — if it doesn't ship, none of the later phases matter.

---

## 4. The decision-making rule

When a decision is forced (a user request, a competitor move, a hiring need), the resolution order is:

1. **Does it violate an invariant?** (one-operator, 6-month product, sovereign-by-construction, CLI-is-the-product) → **Reject.**
2. **Is it in a phase file as a "must ship" or "defer"?** → Follow the file.
3. **Is it a named anti-pattern in `negative-prompt.md`?** → **Reject.**
4. **Is it already an ADR?** → Follow the ADR.
5. **None of the above?** → Add a new ADR before writing code.

If a "small" feature addition is proposed (e.g., "let's add a web UI in V1", "let's add K8s support", "let's add a custom DSL"), the answer is in `negative-prompt.md`. Read it before arguing.

---

## 5. Companion documents (parent folder)

| File | Purpose | When to read |
|---|---|---|
| `../Research-1.txt` | Original product spec / master requirement gathering prompt | Never — superseded by this folder |
| `../Research-2.md` | First synthesis (competitive, market, tech, pain) | When you need the "why we picked this market" answer |
| `../Research-3.md` | 7-persona synthesis (the master document) | When you need the full reasoning chain |
| `../Executive-Summary.md` | 1-page TL;DR for investors / co-founders | When you have 5 minutes |
| `../persona-pm.md` | PM lens: day-to-day, dev journey, CLI/TUI UX, pricing | Before any product/UX decision |
| `../persona-principal.md` | Principal/Architect lens: hexagonal, data model, state machines, API | Before any architecture decision |
| `../persona-rust.md` | Rust lens: crate selection, type system, async, perf, testing | Before any Rust decision |
| `../persona-devops.md` | DevOps/SRE lens: morning commands, SLOs, alerts, DR, observability | Before any ops/observability decision |
| `../persona-platform.md` | Platform lens: IDP philosophy, golden paths, service catalog, RBAC, drift | Before any platform/IDP decision |
| `../persona-cto.md` | CTO/Founder lens: positioning, funding, moat, named mistakes | Before any business/strategy decision |
| `../persona-crosscutting.md` | Cross-cutting: governance (rules+ML), USP, sovereignty as engineering | Before any governance/USP/sovereignty decision |
| `../competitive-landscape.md` | 12+ competitor analysis | When "is this already done?" comes up |
| `../user-pain-research.md` | 200+ user pain points with quotes | When "do users actually want this?" comes up |
| `../sovereign-runtime-market-research.md` | EU sovereignty + market sizing | When "is the EU market real?" comes up |
| `../Research-Report-1.md` | Tech validation deep-dive | When "is this technically feasible?" comes up |

---

## 6. The negative prompt (read this first if you are new)

[`negative-prompt.md`](./negative-prompt.md) is the most important file in this folder. It documents the **decisions we will not make**, the **features we will not build**, and the **mistakes we will not repeat**. The five core anti-patterns to memorize:

- **The CasaOS mistake:** one corporate sponsor, dev moves on, repo rots. Mitigated by EU incorporation, ≥ 3 core maintainers, foundation transfer by year 3.
- **The Dokploy mistake:** license change to source-available triggers community backlash. Mitigated by Apache 2.0 unmodified, forever, no `proprietary/` directory.
- **The Vercel mistake:** usage-based pricing leads to a $46k surprise bill (Riley Walz's Jmail). Mitigated by flat €19/server/month, no per-bandwidth, no per-deploy, no per-seat.
- **The Kubernetes seduction:** "just add K8s support" triples complexity and kills the one-operator invariant. Mitigated by an explicit "we will never support K8s" rule in this folder.
- **The feature-creep trap:** every "small" feature addition is a 5-year maintenance burden. Mitigated by the "defer or reject" lists in each phase file.

When someone proposes a feature, the first question is not "can we build it?" — it is "is it in the negative prompt?". If yes, the answer is no.

---

## 7. License and ownership

- **Code:** Apache 2.0, unmodified, no carve-outs, no `proprietary/` directory. Ever. This is non-negotiable.
- **Documentation:** CC BY-SA 4.0 (so the same docs can be re-used by forks, with attribution).
- **Trademark:** "Sovereign Application Runtime" and the binary name are reserved. Anyone can fork the code; nobody can rebrand it without permission.
- **Bus factor:** target ≥ 3 core maintainers from month 6. Foundation transfer plan published by month 18. EU-incorporated from day 1.

---

## 8. How this folder evolves

- **Every locked decision** is captured in [`decision-records.md`](./decision-records.md) (ADRs). ADRs are append-only. Changing a decision is a new ADR that supersedes the old one.
- **Every "we won't build this"** is captured in [`negative-prompt.md`](./negative-prompt.md) with a reason and a mitigation. If a "won't build" item moves to "will build", it is *removed* from the negative prompt and added to the relevant phase file.
- **Every phase file** has a Definition of Done. When the DoD is met, the phase is *closed* and the next phase opens.
- **Every month**, the founder + lead engineer walk through this folder and update the "current focus" stamp on the relevant phase file.

---

**End of README. Read [`architecture.md`](./architecture.md) next, or jump to [`negative-prompt.md`](./negative-prompt.md) if you are skeptical.**
