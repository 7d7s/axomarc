# Negative Prompt

**Status:** Locked. This file is the most important file in this folder.
**Audience:** Every engineer, every PM, every founder, every first design partner.
**Last updated:** 2026-06-04

This document is the **list of decisions we will not make, features we will not build, and mistakes we will not repeat.** It is the single most important file in this folder. Read it before proposing a feature, accepting a PR, or pitching a customer.

The structure:
1. The decision-making rule
2. The 5 core anti-patterns (memorize these)
3. The 8 named case studies (learn from these)
4. The 14 explicit refusals (the features we will not build)
5. The feature-creep checklist (use this before any "small" addition)

If a proposed change is in this file, the answer is **no**, regardless of how good the argument sounds.

---

## 1. The decision-making rule

When a decision is forced (a user request, a competitor move, a hiring need), the resolution order is:

1. **Does it violate an invariant?** (one-operator, 6-month product, sovereign-by-construction, CLI-is-the-product) → **Reject.**
2. **Is it in a phase file as a "must ship" or "defer"?** → Follow the file.
3. **Is it a named anti-pattern in §3 below?** → **Reject.**
4. **Is it an explicit refusal in §4 below?** → **Reject.**
5. **Is it already an ADR?** → Follow the ADR.
6. **None of the above?** → Add a new ADR before writing code.

If a "small" feature addition is proposed (e.g., "let's add a web UI in V1", "let's add K8s support", "let's add a custom DSL"), the answer is in this file. Read it before arguing.

---

## 2. The 5 core anti-patterns (memorize these)

### 2.1 The CasaOS mistake — one corporate sponsor, dev moves on, repo rots

**What happened:** CasaOS had 33k stars, Apache 2.0, one corporate sponsor (IceWhale), the lead dev moved to a closed-source project (ZimaOS), the repo rots. ([github.com/IceWhaleTech/CasaOS](https://github.com/IceWhaleTech/CasaOS))

**The lesson:** Apache 2.0 with one corporate sponsor is a ticking time bomb.

**Our mitigation:**
- EU incorporation (Berlin GmbH + Estonian OÜ) — not US, not China.
- Foundation transfer plan by year 3 (LF Europe, Apache, or Eclipse).
- Public 18-month sustainability signal on the website.
- Public funding / revenue transparency.
- ≥ 3 core maintainers, not 1.
- "Maintainer ladder" — anyone can become a core maintainer.

**Trigger phrases that should make you suspicious:**
- "We're just one person right now, but we'll hire more when..."
- "The company is fully self-funded, no investors."
- "The dev is doing this on weekends."
- "We don't need a foundation; the company will outlive us."

### 2.2 The Dokploy mistake — license change to source-available triggers community backlash

**What happened:** Dokploy had 34k stars, changed its license in Jan 2026 to "Apache 2.0 core + source-available for templates/multi-server/previews." Community backlash on issues #3613, #3477. ([github.com/Dokploy/dokploy/issues/3613](https://github.com/Dokploy/dokploy/issues/3613))

**The lesson:** Once you change the license, the community never trusts you again.

**Our mitigation:**
- Apache 2.0 unmodified, forever.
- No `proprietary/` directory, ever.
- No source-available exceptions for any feature.
- Public commitment on the website: "All features, all of the time, Apache 2.0. Forever."
- Trademark policy that protects the name without restricting use.

**Trigger phrases that should make you suspicious:**
- "We need to monetize the enterprise tier, so we'll make templates source-available."
- "The community edition will always be free, but the multi-server feature..."
- "Apache 2.0 with a commercial supplement is the standard model."
- "We'll keep the core open, but the advanced features..."

### 2.3 The Vercel mistake — usage-based pricing leads to a $46k surprise bill

**What happened:** Riley Walz's Jmail hit a $46,485 Vercel bill due to a runaway email-notification loop. The "pay per usage" model punished growth. ([twitter.com/rrrr_walz/status/1788824729600245882](https://twitter.com/rrrr_walz/status/1788824729600245882))

**The lesson:** Usage-based pricing is a tax on growth. It punishes the customers who love the product most.

**Our mitigation:**
- Flat per-server pricing, forever.
- No per-bandwidth, no per-deploy, no per-seat.
- The Pro tier is for *managed updates + support*, not for *features*.
- Public commitment: "€19/server/month. No surprises. No overage. No bandwidth."

**Trigger phrases that should make you suspicious:**
- "We should charge per deploy to align with value."
- "Power users should pay more."
- "Bandwidth is a real cost, we should pass it through."
- "Usage-based is the cloud-native way."

### 2.4 The Kubernetes seduction — "just add K8s support" triples complexity and kills the one-operator invariant

**What happened:** Many self-hosted platforms (Rancher, OpenShift, K3s) add K8s support and inherit K8s's complexity: control plane, etcd, networking, RBAC, ingress, service mesh, operators, Helm, CRDs, etc. The "one operator can run it for 5 years" invariant dies.

**The lesson:** K8s is a platform for platforms. It is not a deployment runtime for 1-50 engineer teams.

**Our mitigation:**
- An explicit "we will never support K8s" rule.
- The architecture is intentionally single-binary, single-host (V0/V1), single-control-plane-multi-agent (V1.5), single-cluster (V2).
- No `kubectl`, no `helm`, no `kubectl apply`, no `kubeconfig`.
- No CRDs, no operators, no Helm charts.

**Trigger phrases that should make you suspicious:**
- "We should add K3s support for the enterprise tier."
- "K8s is the standard; we need to support it."
- "What if a customer wants to deploy our platform *on* K8s?"
- "We can add a K8s adapter behind the `Runtime` trait."

### 2.5 The feature-creep trap — every "small" feature addition is a 5-year maintenance burden

**What happened:** Coolify added 280+ templates, a marketplace, a plugin system, a web UI, multi-server, multi-cloud, white-label, etc. Each was a "small" addition. The result: a product that is hard to operate, hard to test, hard to maintain, and hard to evolve.

**The lesson:** "Small" features are never small. Each is a 5-year maintenance burden, a test surface, a documentation page, a security review, a support ticket.

**Our mitigation:**
- The 14 explicit refusals in §4.
- The feature-creep checklist in §5.
- The "no" is the default; the "yes" requires an ADR.
- The maintainer ladder is for code, not for features.

**Trigger phrases that should make you suspicious:**
- "It's just one more flag."
- "Customers are asking for it."
- "It's a small addition, what could go wrong?"
- "We can always remove it later."

---

## 3. The 8 named case studies (learn from these)

### 3.1 CasaOS — Apache 2.0, one sponsor, repo rots

**The facts:**
- 33k GitHub stars.
- Apache 2.0 license.
- One corporate sponsor: IceWhale.
- The lead dev moved to ZimaOS, a closed-source successor.
- The CasaOS repo is largely unmaintained as of 2026.

**The lesson:** Apache 2.0 + one sponsor = ticking time bomb.

**What we do differently:** EU incorporation + foundation transfer + ≥ 3 core maintainers + public sustainability signal.

**Reference:** [github.com/IceWhaleTech/CasaOS](https://github.com/IceWhaleTech/CasaOS)

### 3.2 Dokploy — license change, community backlash

**The facts:**
- 34k GitHub stars.
- License changed Jan 2026 to "Apache 2.0 core + source-available for templates/multi-server/previews."
- Community backlash on issues #3613, #3477.
- The "open source false marketing" complaint.

**The lesson:** License change is irreversible. The community never trusts you again.

**What we do differently:** Apache 2.0 unmodified, forever. Public commitment. No `proprietary/` dir.

**Reference:** [github.com/Dokploy/dokploy/issues/3613](https://github.com/Dokploy/dokploy/issues/3613)

### 3.3 Vercel — $46k Jmail bill

**The facts:**
- Riley Walz built Jmail (a visualization of all emails sent to a Gmail address).
- Deployed on Vercel.
- Hit a $46,485 bill due to a runaway notification loop.
- The "pay per usage" model punished success.

**The lesson:** Usage-based pricing is a tax on growth. It punishes the customers who love the product most.

**What we do differently:** Flat per-server pricing, forever. No per-bandwidth, no per-deploy, no per-seat.

**Reference:** [twitter.com/rrrr_walz/status/1788824729600245882](https://twitter.com/rrrr_walz/status/1788824729600245882)

### 3.4 Coolify — feature creep, hard to operate

**The facts:**
- 56k GitHub stars.
- 280+ templates.
- Plugin system, marketplace, white-label, multi-cloud.
- "WordPress-plugin-maintenance feel" — depends on apt for updates.
- Hard to operate past ~30 apps.

**The lesson:** "Small" features are never small. Each is a 5-year maintenance burden.

**What we do differently:** 6 framework scanners, no plugin system, no marketplace, no white-label (V3+ only).

**Reference:** [github.com/coollabsio/coolify](https://github.com/coollabsio/coolify)

### 3.5 Heroku — the original "tax on growth"

**The facts:**
- Heroku popularized the "12-factor app" and made deployment trivial.
- Acquired by Salesforce in 2010 for $212M.
- Pricing became usage-based (dyno hours, add-ons, egress).
- Free tier eliminated in 2022.
- "Heroku is dead" became a meme.

**The lesson:** A great product can be killed by the wrong pricing model and the wrong acquirer.

**What we do differently:** Flat per-server pricing, Apache 2.0, EU-incorporated, foundation transfer plan.

### 3.6 GitLab 2017-01-31 — 8 months of empty backups

**The facts:**
- 8 months of "successful" backups, all empty, all useless.
- They recovered 40% of the data.
- The postmortem is public and is the foundational document for the "test your backups" movement.

**The lesson:** Backups you haven't restored are hopes.

**What we do differently:** `sovereign backup verify --restore-to scratch` is the most important V1.2 command. Monthly drill mandatory. 3-2-1-1-0 rule.

**Reference:** [about.gitlab.com/blog/2017/02/10/postmortem-of-database-outage-of-january-31/](https://about.gitlab.com/blog/2017/02/10/postmortem-of-database-outage-of-january-31/)

### 3.7 Cloudflare 2019-07-02 — the regex backtracking outage

**The facts:**
- A single bad regex (`.*.*.*.*`) caused CPU exhaustion across Cloudflare's edge.
- 27 minutes of 502 errors for many Cloudflare customers.
- The postmortem is public and is the foundational document for the "validate untrusted input" principle.

**The lesson:** Fuzz your input parsers. The parser is a security boundary.

**What we do differently:** cargo-fuzz in CI for every parser (app.yaml, rego, JSON, CLI args). The parser is fuzzed with 1-hour nightly runs.

**Reference:** [blog.cloudflare.com/details-of-the-cloudflare-outage-on-july-2-2019/](https://blog.cloudflare.com/details-of-the-cloudflare-outage-on-july-2-2019/)

### 3.8 Supabase — Series E, the open-source growth model

**The facts:**
- Apache 2.0, open source.
- Bootstrapped, then raised ($116M Series E in 2025).
- Per-seat pricing for Pro; enterprise is custom.
- "Open source is the default; commercial is the supplement."

**The lesson:** Apache 2.0 + per-seat Pro + enterprise custom is the 2026 standard for open-source dev tools. The product matches the license, not the pricing.

**Reference:** [supabase.com](https://supabase.com)

---

## 4. The 14 explicit refusals (features we will not build)

These are the features we **deliberately refuse** to build. Each has a reason. If a proposed change is in this list, the answer is **no**.

### 4.1 Kubernetes support — never

**Why:** K8s is a platform for platforms. It is not a deployment runtime for 1-50 engineer teams. Supporting K8s violates the one-operator invariant.

**What we say instead:** "We don't do K8s. If you need K8s, you have bigger problems than we can solve."

### 4.2 Service mesh — never

**Why:** Service mesh (Istio, Linkerd, Consul Connect) adds 3-5 sidecars per service, a control plane, mTLS, and a learning curve. For a 1-50 engineer org, this is over-engineering.

**What we say instead:** "Your service-to-service traffic is fine without a mesh. Use HTTP retries in your app."

### 4.3 Multi-cloud abstraction — never

**Why:** "Cloud-agnostic" is a tax on every feature. Each cloud has a different API, a different IAM model, a different networking model. The abstraction layer is the bug, not the feature.

**What we say instead:** "Pick a cloud (or a VPS provider). Sovereignty is about jurisdiction, not about abstracting the provider."

### 4.4 Terraform / Pulumi replacement — never

**Why:** Terraform and Pulumi are mature, well-supported, and have a large ecosystem. We don't compete with them. We are the deployment runtime; they are the infrastructure-as-code.

**What we say instead:** "Use Terraform for your cloud resources. Use sovereign for your application deploys."

### 4.5 Custom policy DSL — never

**Why:** Every custom DSL is a maintenance burden. "Just write some Rust" is not policy. "Just write some YAML" is not policy. OPA/Rego exists; use it.

**What we say instead:** "Write Rego rules. Install them via `sovereign policy install <pack.rego>`."

### 4.6 gRPC / GraphQL / WebSocket APIs — never

**Why:** gRPC, GraphQL, and WebSockets add complexity. REST + JSON + SSE covers 99% of the use cases. SSE is HTTP, works through every proxy, has built-in reconnect logic.

**What we say instead:** "REST + JSON + SSE. The API is in the OpenAPI spec."

### 4.7 Mobile app — never

**Why:** A mobile app is a 5-year maintenance burden. The CLI is the product. The TUI is the daily-driver. The web UI is V2 opt-in. A mobile app adds nothing.

**What we say instead:** "Use the TUI over SSH. Use the web UI in V2."

### 4.8 Desktop app (Electron, Tauri) — never

**Why:** A desktop app is a 5-year maintenance burden. The user is the operator; the operator is on a server. The CLI runs on the server.

**What we say instead:** "Use the TUI on the server. SSH in."

### 4.9 OIDC provider (be the IdP) — never

**Why:** Being an identity provider is a different product. Auth0, Keycloak, Zitadel, Authentik, Ory are mature. We are a consumer of OIDC, not a provider.

**What we say instead:** "Use Keycloak (or your IdP) for SSO. We are an OIDC client."

### 4.10 Internal developer portal (Backstage-style) — V3+, never V0/V1/V1.5/V2

**Why:** A full IDP (Backstage, OpsLevel, Cortex) is a 5-year investment. For 1-50 engineer orgs, it's an anti-feature. The product is the deployment runtime whose default behavior resembles the IDP's golden path when the user is small.

**What we say instead:** "We are the deployment runtime. The 6 framework scanners are the golden paths. Service catalog is the 16-field view. Backstage is the escape hatch for large orgs."

### 4.11 Plugin marketplace with 3rd-party code — never

**Why:** Plugins are an attack surface (untrusted code), a maintenance burden (version skew), and a testing nightmare. The product is monolithic on purpose.

**What we say instead:** "The 6 framework scanners are the templates. The 3 default rule packs are the policies. For everything else, fork the repo."

### 4.12 Workflow engine (Airflow-like) — never

**Why:** A workflow engine is a different product. Prefect, Airflow, Dagster are mature. We are the deployment runtime.

**What we say instead:** "Run your workflows in a separate container. Deploy it with sovereign."

### 4.13 Custom container runtime — never

**Why:** Docker and Podman exist. We don't write a container runtime. We are a *consumer* of container runtimes, via the `Runtime` trait.

**What we say instead:** "Docker is V1 default. Podman is V1.5. The `Runtime` trait is open for new adapters."

### 4.14 SOC2 reporting — V3+ (only if customer demand)

**Why:** SOC2 is expensive, slow, and not aligned with the EU sovereignty story. BSI C5:2026 + EUCS Substantial is the EU equivalent. We prioritize the EU certifications; SOC2 is V3+ if and only if a US enterprise customer requires it.

**What we say instead:** "We are certified to BSI C5:2026 and EUCS Substantial. SOC2 is on the roadmap if a customer requires it."

---

## 5. The feature-creep checklist (use this before any "small" addition)

Before accepting any "small" feature addition, walk through this checklist. If the answer to any question is "no" or "I don't know," the feature is not added.

### 5.1 The 10 questions

1. **Does it violate an invariant?** (one-operator, 6-month product, sovereign-by-construction, CLI-is-the-product)
   - If yes, **reject.**

2. **Is it in a phase file as "must ship" or "defer"?**
   - If "defer," the answer is **not now.**
   - If "must ship," the answer is **yes, on the schedule.**
   - If neither, **add a new ADR.**

3. **Is it a named anti-pattern in §3?**
   - If yes, **reject.**

4. **Is it an explicit refusal in §4?**
   - If yes, **reject.**

5. **Is it already an ADR?**
   - If yes, follow the ADR.
   - If no, **add a new ADR.**

6. **Is it testable in CI?**
   - If no, **reject.** (The test must be deterministic and reproducible.)

7. **Does it have a runbook?** (if it's a feature the operator uses)
   - If no, **defer until the runbook is written.**

8. **Does it have a public doc?**
   - If no, **defer until the doc is written.**

9. **Is the maintainability test passing?** (See [`sovereignty-and-governance.md` §6](#))
   - If no, **reject.**

10. **Is it mentioned in the marketing?**
    - If yes but it's theatre, **reject.** (See [`sovereignty-and-governance.md` §6.2](#) for the list of theatre features.)

### 5.2 The "small" feature anti-patterns

| "Small" feature | Why it's not small | The 5-year maintenance burden |
|---|---|---|
| "Just add a web UI in V1" | The web UI is a 5-year investment; the CLI is the product | 5+ engineers, 5+ years, 100k+ lines of code |
| "Just add K8s support" | K8s is a 5-year investment; we don't do K8s | 5+ engineers, 5+ years, the one-operator invariant dies |
| "Just add a plugin system" | Plugins are an attack surface | 3+ engineers, 5+ years, security review for every plugin |
| "Just add a custom DSL" | Every custom DSL is a maintenance burden | 2+ engineers, 5+ years, the language is never quite right |
| "Just add a workflow engine" | A workflow engine is a different product | 5+ engineers, 5+ years, you become Airflow |
| "Just add a mobile app" | A mobile app is a 5-year investment | 3+ engineers, 5+ years, app store reviews |
| "Just add a desktop app" | A desktop app is a 5-year investment | 3+ engineers, 5+ years, OS compatibility |
| "Just add multi-cloud" | "Cloud-agnostic" is a tax on every feature | 5+ engineers, 5+ years, every feature is harder |
| "Just add Terraform support" | Terraform is mature; we don't compete with it | 3+ engineers, 5+ years, state file management |
| "Just add a custom DNS server" | Let the user use managed DNS | 1+ engineer, 5+ years, RFC compliance |
| "Just add auto-scaling" | VPS doesn't auto-scale | 3+ engineers, 5+ years, the one-operator invariant dies |

**The rule:** if the "small" feature is in this list, the answer is **no**. Add the user's request to the deferred list in the relevant phase file, with a reason.

---

## 6. The "is this theatre?" checklist

For any feature that is mentioned in the marketing, run this checklist. If any answer is "no," the feature is theatre.

1. **Is the feature testable in CI?**
2. **Is the feature opt-in, not on by default?**
3. **Is the feature documented publicly?**
4. **Is the feature backed by a real implementation, not a badge?**
5. **Is the feature mentioned in the runbook?**

If any answer is "no," the feature is theatre. **Theatre is worse than no feature.** A buyer who discovers theatre loses trust in the entire product.

### 6.1 Theatre features to avoid

- "GDPR compliance" badge without substance
- "Immutable audit log" on an editable DB
- "Air-gapped" without testing
- "SOC2 ready" without SOC2
- "Zero-trust" without ZTA architecture
- "AI-driven" anything that doesn't have a deterministic backup
- "Real-time" anything that doesn't have a backpressure plan
- "Production-ready" without a real user
- "Enterprise-grade" without a real enterprise customer
- "Mission-critical" without a real on-call rotation
- "Future-proof" abstractions that aren't solving today's problem
- "Innovative" deployment strategies (shadow, dark launch, etc.) — over-engineered
- "Encrypted at rest" without the encryption test
- "Auditable" without the audit log query CLI
- "Compliant" without the compliance map doc

---

## 7. The "no" is the default

The most important sentence in this file:

> **The "no" is the default; the "yes" requires an ADR.**

When in doubt, say no. When a feature is in this file, say no. When a feature is not in a phase file, say no. When a feature is in a phase file as "defer," say no.

The "yes" requires:
1. The feature is in a phase file as "must ship" or "will build," OR
2. An ADR is filed and accepted, AND
3. The feature-creep checklist passes, AND
4. The "is this theatre?" checklist passes.

If any of these is missing, the answer is **no**.

---

## 8. The 12 named risks (and the mitigations)

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Coolify adds sovereignty package | High | Medium | Move fast; sovereignty is values + compliance, not just features |
| Dokploy fixes its license / adds GitOps | High | Low | Differentiate on Rust + sovereign values |
| Hyperscaler sovereign SKU wins procurement | Medium | High | Differentiate on self-hosted (hyperscalers can't match) |
| Edge platforms reframe sovereignty | Medium | Medium | Position as for stateful apps, not all apps |
| AI agents reduce need for deployment runtime | Low | High | Counter: regulated buyers need auditable, deterministic deployment |
| Single→multi-server architecture needs rewrite | High (if done wrong) | Critical | Build agent pattern in V1.5, rqlite in V2 |
| Dokploy-style stuck-deployment bug | High | High | Auto-rollback on health fail; cancel path first-class |
| Caddy OOM under load | Medium | Medium | Offer Nginx as low-mem alternative; document Caddy limits |
| SQLite single-writer at multi-server scale | Low | High | Move to rqlite in V2; plan from day 1 |
| BuildKit security (untrusted builds) | Medium | High | Allow `--image=...` to bypass; delegate to CI as default |
| CasaOS-style abandonment | Medium | Critical | EU incorporation + foundation plan + public sustainability |
| License change backlash (Dokploy) | Medium | High | Apache 2.0 forever, public commitment, no `proprietary/` |

---

## 9. The "this is what we are NOT" list

For marketing, for sales, for support, for the website's `/about` page:

**We are NOT:**
- A PaaS (we are self-hosted; PaaS is Heroku, Vercel, Render)
- An IDP (we are a deployment runtime; IDP is Backstage, OpsLevel, Cortex)
- "Kubernetes without Kubernetes" (we don't do K8s)
- A workflow engine (we deploy apps; we don't orchestrate workflows)
- A CI/CD tool (we are the deploy; CI is GitHub Actions, GitLab CI, Buildkite)
- A monitoring tool (we expose `/metrics`; monitoring is VictoriaMetrics, Grafana, Datadog)
- A log aggregation tool (we emit JSON logs; aggregation is Loki, VictoriaLogs, Datadog)
- A secrets manager (we encrypt at rest; full secrets management is Vault, AWS Secrets Manager)
- A database (we provision databases; the database is Postgres, MySQL, Redis, SQLite)
- A multi-cloud abstraction (we are cloud-agnostic at the install level, not at the API level)
- A plugin platform (we are monolithic; the 6 scanners are the templates)

**We ARE:**
- A Rust single-binary, self-hosted, CLI/TUI/API-first deployment runtime
- For solo developers, freelancers, agencies, and 1-50 engineer startups
- That runs on a €4.49 Hetzner box (or any Linux VM)
- And survives the company disappearing
- And is governed by Apache 2.0 forever
- And is EU-incorporated from day 1
- And claims 5 dimensions of sovereignty (data, operational, vendor, legal, technical)
- And is the Plausible of self-hosted deployment

---

## 10. The final word

The discipline of refusal is the moat.

Every "small" feature is a 5-year maintenance burden. Every "easy" addition is a test surface, a documentation page, a security review, a support ticket. Every "customers are asking for it" is a one-way door.

The product is the Plausible of self-hosted deployment. Plausible got to €1M ARR with a 4-person team by saying **no** to 95% of the feature requests. Supabase got to $116M Series E by saying **no** to every feature that wasn't a deploy, query, or auth.

We say **no** by default. The "yes" requires an ADR.

---

**End of `docs/`. You are now ready to write the first line of code. Start with [`phase-00-mvp.md`](./phase-00-mvp.md) §F1, or jump to [`architecture.md`](./architecture.md) if you need the load-bearing wall.**
