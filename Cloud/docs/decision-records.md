# Decision Records (ADRs)

**Status:** Append-only. ADRs are not edited. Changing a decision is a new ADR that supersedes the old one.
**Format:** Michael Nygard's ADR format, adapted.
**Last updated:** 2026-06-04

This document collects the **Architecture Decision Records (ADRs)** for every locked decision in the project. ADRs are the source of truth for "why did we pick X?" When a future engineer asks "why Rust?" or "why SQLite?" or "why Apache 2.0?", the answer is here.

---

## ADR template

```markdown
# ADR-NNNN: <title>

**Date:** YYYY-MM-DD
**Status:** Proposed | Accepted | Superseded by ADR-XXXX
**Deciders:** <who>

## Context
What is the issue we're seeing that motivates this decision?

## Decision
What is the change we're proposing or have agreed to implement?

## Consequences
What becomes easier? What becomes harder?

## Alternatives considered
What other options did we look at? Why did we reject them?
```

---

## ADR-0001: Rust as the implementation language

**Date:** 2026-06-04
**Status:** Accepted

## Context

The product is a self-hosted, single-binary deployment runtime for 1-50 engineer teams. It must:
- Ship as a single static binary ≤ 25 MB that runs on any Linux distro.
- Start in < 100 ms.
- Use < 50 MB RAM at idle.
- Handle 1000+ concurrent connections (deploy API + log streaming + health checks).
- Be auditable and deterministic for sovereignty claims.

## Decision

Use **Rust 2024 edition**, MSRV 1.85.

## Consequences

**Easier:**
- Single static binary via musl + LTO + abort + strip.
- Memory safety by default; no GC pauses.
- Type system prevents entire categories of bugs (use-after-free, data races, null pointer deref).
- The 2026 ecosystem is mature: axum, tokio, sqlx, ratatui, clap.
- "Rust + small binary + low memory" is a defensible moat (Plausible, Supabase, Cloudflare all use it).
- Sovereignty story: "compiled from source, reproducible, auditable."

**Harder:**
- Steeper learning curve for new contributors (mitigated by the locked stack in [`tech-stack.md`](./tech-stack.md) and the implementation guide).
- Compile times are slower than Go or Node.
- Some ecosystem libraries are still immature (notably: cert management, OIDC, ML).

## Alternatives considered

- **Go:** Rejected because the binary is larger (~30-40 MB even with -ldflags="-s -w"), the GC pauses hurt the "no DevOps team" UX, and the type system is weaker (no sum types, no exhaustive matching). Go is fine for a Kubernetes operator; not fine for a single-binary deployment runtime.
- **TypeScript + Deno:** Rejected because the runtime is heavier, the binary is larger, the sovereignty story is weaker (Node dependencies are a supply chain attack surface), and the type system is opt-in (TypeScript checks are not enforced at runtime).
- **Zig:** Rejected because the ecosystem is too immature (no production-ready HTTP server, no sqlx-equivalent, no TUI library).
- **C++:** Rejected because the memory safety story is weak and the sovereignty claim ("compiled from source, auditable") is harder to defend.

---

## ADR-0002: axum as the HTTP server

**Date:** 2026-06-04
**Status:** Accepted

## Context

The control plane exposes a REST + JSON + SSE API. The HTTP server must:
- Be Tokio-native (the rest of the stack is Tokio).
- Have a rich middleware ecosystem (auth, tracing, rate limiting, CORS, etc.).
- Be type-safe end-to-end (extractors, request validation).
- Be production-grade (used by Cloudflare, AWS, Discord).

## Decision

Use **axum 0.8** as the HTTP server.

## Consequences

**Easier:**
- Native Tokio integration.
- Tower middleware ecosystem: `tower-http` provides trace, cors, timeout, request-id, etc. in 5 lines.
- Type-safe extractors (`Query<T>`, `Path<T>`, `Json<T>`) catch bugs at compile time.
- Excellent documentation and a large community.
- The `IntoResponse` trait makes RFC 9457 problem+json trivial to implement.

**Harder:**
- axum 0.8 is recent; some third-party middleware is still on 0.7.
- The Tower ecosystem is large; choosing the right middleware is non-obvious.

## Alternatives considered

- **actix-web:** Rejected because it uses its own actor model that fights Tokio's work-stealing. Middleware is harder to compose. The performance difference is negligible.
- **salvo:** Rejected because the ecosystem is smaller and the type-safety story is weaker.
- **poem:** Rejected because the ecosystem is smaller and the project is less mature.
- **rocket:** Rejected because the async story is bolted on (not native) and the macro-heavy API is harder to debug.
- **hyper directly:** Rejected because the boilerplate is significant; axum is hyper with sane defaults.

---

## ADR-0003: SQLite for V1 state, rqlite for V2

**Date:** 2026-06-04
**Status:** Accepted

## Context

The control plane needs a state store for apps, deployments, secrets, backups, audit log, users, etc. The state must:
- Survive a process restart.
- Be restorable on a fresh host from a backup.
- Be auditable (the audit log is append-only).
- Scale to ~50 apps and ~10k deployments per host without a DBA.

## Decision

**V1:** Use **SQLite** (WAL mode) as the only state store. The `RuntimeState` trait abstracts the storage; `SqliteState` is the only V1 implementation.

**V2:** Optionally swap to **rqlite** (Raft consensus over SQLite) via the same `RuntimeState` trait. The use cases do not change.

**Never:** Use Postgres. Use DynamoDB. Use any other store.

## Consequences

**Easier:**
- SQLite is a single file, restorable on any host, no external dependency.
- The audit log is enforceable via SQL triggers (`audit_no_update`, `audit_no_delete`).
- The state is portable: `sovereign export` is `cp sovereign.db sovereign.db.backup`.
- rqlite is "SQLite over Raft"; the same SQL works.
- Forward-only migrations are safe (no schema migrations of a live system).

**Harder:**
- Single-writer at multi-server scale (mitigated by V2's rqlite).
- No `SELECT FOR UPDATE` (mitigated by optimistic concurrency via `version` columns).
- The `sqlx` query macros require `DATABASE_URL` at compile time (mitigated by offline mode with `cargo sqlx prepare`).

## Alternatives considered

- **Postgres:** Rejected because it requires a separate process, a separate host (or container), and a DBA. The "single binary, single host" invariant is broken. Postgres is the right answer for a 50-engineer platform; wrong answer for a 1-50 engineer self-hosted runtime.
- **MySQL / MariaDB:** Rejected for the same reason as Postgres.
- **Redis:** Rejected because the data is the state of record, not a cache. Redis is the right answer for ephemeral state; wrong answer for audit log.
- **FoundationDB:** Rejected because the binary is 100+ MB, the deployment is complex, and the ecosystem is small.
- **etcd / Consul:** Rejected because the data model is wrong (key-value, not relational). Apps, deployments, and audit log are relational.
- **JSON files on disk:** Rejected because concurrency is hell, queries are O(n), and audit is unenforceable.

---

## ADR-0004: Caddy as the default reverse proxy

**Date:** 2026-06-04
**Status:** Accepted

## Context

Every deployed app needs a reverse proxy that:
- Terminates TLS.
- Routes by hostname to the upstream container.
- Auto-renews certs.
- Reloads without dropping traffic.
- Uses < 100 MB RAM at idle.

## Decision

**V1:** Use **Caddy** as the default reverse proxy, with **Nginx** as a low-memory alternative (V1.1).

## Consequences

**Easier:**
- Caddy has built-in ACME (HTTP-01 and DNS-01). No certbot.
- The Caddy admin API (`/config/`, `/load`) is JSON, so we can manage routes programmatically.
- Auto-renewal is automatic; we just have to call `caddy add-route`.
- Hot reload via `caddy reload` or admin API; no downtime.

**Harder:**
- Caddy uses ~80 MB RAM at idle (Nginx uses ~10 MB). On a CX22 with 2 GB RAM, this is 4% of memory.
- Caddyfile syntax is unique; some users know Nginx better.
- The Caddy admin API is JSON but the schema is not formally documented.

## Alternatives considered

- **Nginx:** Rejected as the default (too many config files, certbot is a separate tool) but accepted as a V1.1 low-mem alternative.
- **Traefik:** Rejected because it requires Docker labels, which conflicts with our hexagonal architecture. Also uses ~100 MB RAM.
- **Envoy:** Rejected because the config is YAML, the resource usage is high, and the operational complexity is not justified.
- **HAProxy:** Rejected because TLS + ACME is bolted on (not built-in).
- **Apache httpd:** Rejected because the config is verbose, ACME is third-party, and the ecosystem is older.

---

## ADR-0005: Apache 2.0, unmodified, no carve-outs

**Date:** 2026-06-04
**Status:** Accepted

## Context

The product's "sovereign" claim depends on the license. The license must:
- Be irrevocable.
- Allow commercial use, modification, redistribution.
- Not require derivative works to be open source (we want enterprise customers to be able to embed it).
- Be a standard license that procurement officers recognize.

## Decision

Use **Apache 2.0, unmodified, no carve-outs, no source-available exceptions, no `proprietary/` directory**.

## Consequences

**Easier:**
- The binary is forever open source; the company can disappear and the product keeps working.
- The license is irrevocable; no "we changed the license" backlash.
- Plausible (AGPLv3) and Supabase (Apache 2.0) are the model; the product matches Supabase's license.
- The trademark ("Sovereign Application Runtime") is reserved but the code is open.
- BSI C5:2026 + EUCS procurement is easier with a standard, well-understood license.

**Harder:**
- We cannot "open-core" the product (e.g., "templates are source-available"). The product is fully open.
- Enterprise differentiation is via support, certification, and managed updates — not via source-available features.

## Alternatives considered

- **AGPLv3 (Plausible):** Rejected because it scares enterprise customers ("viral" license). The procurement officer sees "AGPL" and says no.
- **BSL / FSL (Sentry):** Rejected because the source-available model is exactly what Dokploy did and got backlash for.
- **MIT:** Rejected because it lacks the patent grant that Apache 2.0 has.
- **GPLv3:** Rejected for the same reason as AGPLv3, plus it's stronger (derivative works must be open).
- **Custom license:** Rejected because no procurement officer recognizes a custom license.
- **Dual license (Apache 2.0 + commercial):** Rejected because it adds complexity and the "Dokploy mistake" pattern.

---

## ADR-0006: Hexagonal architecture with `Arc<dyn Port>` DI

**Date:** 2026-06-04
**Status:** Accepted

## Context

The control plane has many external dependencies: SQLite, Docker, Caddy, age, S3, OPA (V2), rqlite (V2). The domain logic (use cases) must not depend on any of these. The architecture must:
- Allow swapping SQLite for rqlite (V2) without changing use cases.
- Allow swapping Docker for Podman (V1.5) without changing use cases.
- Allow swapping Caddy for Nginx (V1.1) without changing use cases.
- Be testable in isolation (use cases with mocks).
- Be auditable (the dependency graph is explicit).

## Decision

Use **hexagonal architecture** (a.k.a. ports and adapters) with `Arc<dyn Port>` dependency injection. Domain types are pure Rust; use cases call ports; adapters implement ports. The composition root (`main.rs`) is the only place that names concrete types.

## Consequences

**Easier:**
- Use cases are pure functions over `AppState`; tests are 5 lines (mock the ports).
- Swapping a backend is a one-line change in `main.rs`; use cases do not change.
- The dependency graph is explicit; `cargo-deny` and a custom lint enforce it.
- The audit log is enforceable: every mutation goes through a port that wraps the transaction.

**Harder:**
- The boilerplate is higher than a monolithic binary (a port trait, an adapter, a use case).
- The compilation graph is wider (more crates to compile).
- The `Arc<dyn Port>` pattern requires `#[async_trait]` everywhere.

## Alternatives considered

- **Layered architecture (controllers → services → repositories):** Rejected because the dependency direction is implicit; a "service" can import any "repository" without lint enforcement.
- **Service locator (global `REGISTRY`):** Rejected because hidden global state is the enemy of testability.
- **Microservices:** Rejected because the problem domain is one binary, one process; microservices are over-engineering.
- **Plugin system:** Rejected because plugins are an attack surface (untrusted code), a maintenance burden, and a version-skew nightmare.

---

## ADR-0007: EU incorporation (Berlin GmbH + Estonian OÜ)

**Date:** 2026-06-04
**Status:** Accepted

## Context

The "sovereign" claim depends on the corporate structure. The company must be:
- Incorporated in a jurisdiction the buyer trusts (EU for BSI C5, EUCS, IPCEI-CIS).
- Funded such that it survives a downturn (revenue or long runway).
- Have a credible bus factor (≥ 3 core maintainers, or a foundation).

## Decision

Incorporate as **two entities**:
- **Berlin GmbH** for BSI C5, EU public sector, IPCEI-CIS procurement.
- **Estonian OÜ** for e-residency, digital-first team, 0% on reinvested profits.

## Consequences

**Easier:**
- EU public-sector buyers can procure from the GmbH (BSI C5, EUCS).
- The OÜ is 100% digital, e-residency-friendly, fast to set up.
- The dual structure matches the dual mission (sovereign for enterprise; lean for the team).
- The 0% reinvested-profit tax in Estonia encourages growth.

**Harder:**
- The setup is more complex than a single entity (two registrations, two tax advisors, two bank accounts).
- The GmbH requires €25,000 minimum capital.
- Tax compliance is doubled.

## Alternatives considered

- **US LLC (Delaware):** Rejected because US incorporation contradicts the "sovereign" claim. Hyperscalers can claim "sovereign" with US incorporation; we cannot.
- **UK Ltd:** Rejected because post-Brexit, UK is no longer EU; BSI C5 / EUCS does not apply.
- **Single German GmbH:** Rejected because the OÜ's e-residency + 0% reinvested tax is too valuable.
- **French SAS:** Rejected because the SecNumCloud requirement is overkill for V1; we can add a French entity in V3 if customer demand validates.
- **Swiss GmbH:** Rejected because Switzerland is not EU; the "EU sovereign" claim is weaker.

---

## ADR-0008: Flat per-server pricing, no overage, no bandwidth

**Date:** 2026-06-04
**Status:** Accepted

## Context

The product is a self-hosted deployment runtime. Pricing must:
- Align with EU procurement (per-node, not per-seat).
- Be predictable for the buyer (no surprise bills).
- Avoid the Vercel / Heroku / AWS pattern of usage-based pricing that punishes growth.
- Be defensible to the CFO ("why are we paying for this?").

## Decision

**Three tiers:**
- **Community (free, Apache 2.0):** unlimited servers, all features, no support.
- **Pro (€19/server/month):** managed updates, email support, EU-region updates, official deb/rpm repos, 1-day SLA on security advisories.
- **Enterprise (custom, starts €5k/year):** EUCS Substantial package, BSI C5 mapping, on-prem, 24/7 SLA, dedicated CSM, training.

**No per-bandwidth. No per-deploy. No per-seat. No overage. Public commitment.**

## Consequences

**Easier:**
- Predictable for the buyer; a 10-server customer pays €190/mo.
- Aligns with EU procurement (per-node, not per-seat).
- The free tier is the funnel (Plausible model).
- The Pro tier is for *managed updates + support*, not for *features*.
- The Enterprise tier is for *certification*, not for *features*.

**Harder:**
- A single big customer does not generate huge revenue.
- A small customer who uses a lot of bandwidth does not pay more.
- The "no overage" rule means we cannot monetize power users.
- Sales is harder: no upsell, no expansion revenue, no per-seat growth.

## Alternatives considered

- **Per-seat (Vercel):** Rejected because the buyer's CFO sees "seats" as a tax on hiring.
- **Per-bandwidth (Vercel, AWS):** Rejected because the buyer's engineering team sees "bandwidth" as unpredictable.
- **Per-deploy (Vercel):** Rejected because the buyer's engineering team sees "per deploy" as a tax on shipping.
- **Usage-based (Vercel, AWS):** Rejected because of Riley Walz's $46k Jmail bill.
- **Open core (GitLab):** Rejected because the "Dokploy mistake" pattern. All features are Apache 2.0.
- **Per-tier (Vercel Pro, Enterprise):** Rejected for the same reason as per-seat.

---

## ADR-0009: Agent pattern in V1.5, rqlite in V2

**Date:** 2026-06-04
**Status:** Accepted

## Context

The product is single-host in V0/V1. V1.5 adds multi-host. The architecture must:
- Support N agents on remote hosts, controlled by 1 control plane.
- Survive 1 host failure in V2.
- Be extensible to multi-region in V3.
- Not require a rewrite of the use cases, the API, or the state schema.

## Decision

**V1.5:** Add the **agent pattern**. The control plane (1 host) pushes deploy instructions to N agent processes (on N hosts) over mTLS. State is on the control plane's SQLite. Agents are stateless.

**V2:** Add **rqlite** as an optional state backend. 3 rqlite nodes form a Raft consensus group. The control plane survives 1 node failure. The same `RuntimeState` trait is implemented by `RqliteState`.

**V3+:** Per-region agent pools, CRDTs for audit log.

## Consequences

**Easier:**
- The agent pattern in V1.5 is the staging ground for V2's rqlite HA. If the agent pattern is right, V2 is just "add a second server to the cluster."
- The `RuntimeState` trait is the same; V1's `SqliteState` and V2's `RqliteState` are interchangeable.
- The use cases do not change between V1, V1.5, and V2.
- mTLS is added in V1.5; the security model is correct from the start.

**Harder:**
- The agent pattern adds operational complexity (SSH, agent registration, heartbeat).
- mTLS requires a private CA, which adds setup.
- rqlite is HTTP, not native SQLite; the wire format is different (writes go through Raft).

## Alternatives considered

- **Skip V1.5; jump straight to V2 rqlite:** Rejected because the agent pattern is the staging ground; if rqlite ships first, we have no fallback when rqlite fails.
- **Use Kubernetes (k3s) for multi-host:** Rejected because K8s is forbidden (see [`negative-prompt.md`](./negative-prompt.md) §3.5).
- **Use Docker Swarm:** Rejected because the ecosystem is shrinking, the Sovereign story is weaker, and the operational complexity is unjustified.
- **Use Nomad:** Rejected because the ecosystem is smaller than K8s, the sovereignty story is weaker, and the operational complexity is similar to K8s.
- **Use a managed K8s (EKS, GKE, AKS):** Rejected because the "sovereign" claim is broken by the hyperscaler dependency.

---

## ADR-0010: OPA/Rego for policy, never a custom DSL

**Date:** 2026-06-04
**Status:** Accepted

## Context

V2 adds a policy engine. Policy must:
- Be explainable ("why was this deploy blocked?").
- Be auditable (the rule + the input is in the audit log).
- Be standard (procurement officers recognize OPA).
- Be composable (rule packs from different sources).

## Decision

Use **OPA / Rego** as the policy language. Implement in-process via `regorus` (Rust Rego). Optionally use OPA WASM bundles for advanced rules.

**Never:** Build a custom policy DSL. "Just write some Rust" is not policy. "Just write some YAML" is not policy. Rego exists; use it.

## Consequences

**Easier:**
- OPA is the industry standard for policy-as-code.
- Rego is a real language with a real spec, a real test framework, and real documentation.
- Procurement officers (especially EU public sector) recognize OPA.
- Rule packs can be installed via `sovereign policy install <pack.rego>`.

**Harder:**
- Rego has a learning curve; the team's first 2 weeks will be Rego-onboarding.
- `regorus` is less mature than OPA itself; some advanced OPA features may not be supported.
- OPA WASM bundles are a build dependency.

## Alternatives considered

- **Custom DSL:** Rejected. Every custom DSL is a maintenance burden. "Just write some Rust" is not policy. "Just write some YAML" is not policy.
- **Cedar (AWS):** Rejected because it's AWS-specific and the ecosystem is smaller than OPA.
- **Kyverno (K8s):** Rejected because it's K8s-specific and we don't do K8s.
- **JSON-based rules:** Rejected because explainability is poor and the language is too limited.

---

## ADR-0011: Layered governance (rules + ML), 6-month staged rollout

**Date:** 2026-06-04
**Status:** Accepted

## Context

V2 adds an ML-based anomaly scorer. ML must:
- Be explainable ("why was this deploy scored 67?").
- Be auditable (the model inputs + outputs are in the audit log).
- Not be the only signal (rules are still the law).
- Be staged (no enforcement for the first 2 months).

## Decision

**Layered governance:**
- **Layer 1: Telemetry** — SQLite + WAL, no external DB.
- **Layer 2: ML Anomaly Scorer** — Prophet (V2) / TimesFM (V2.5).
- **Layer 3: Rego Rules** — explainable, auditable.
- **Layer 4: Human Override** — audit log + manual ack.

**6-month staged rollout:**
- **Month 1-2:** Rules only, no ML.
- **Month 3-4:** ML in shadow mode (scores logged, not enforced).
- **Month 5:** ML in advisory mode (`sovereign policy advise`).
- **Month 6:** ML enforcement for low-risk decisions only.

## Consequences

**Easier:**
- Rules are the law; ML is the early-warning radar. No "the ML did it" hand-waving.
- Every ML decision is explainable; the audit log has the inputs and the contributing features.
- The 6-month staged rollout is the same pattern as Google SRE 2026, Datadog Watchdog, OPA/Gatekeeper, Kyverno.
- The model card is public (training data, features, performance, limitations).

**Harder:**
- The first 2 months have no ML; the first 4 months have ML but no enforcement.
- The model needs to be trained per (app, metric); cold-start is a problem.
- The model can be wrong; the human override is always available.

## Alternatives considered

- **Pure rules:** Rejected because rules are brittle, false-positive-heavy, and can't see slow drift.
- **Pure ML:** Rejected because ML is opaque, un-auditable, and illegal under GDPR Art. 22 for decisions affecting a person.
- **No governance:** Rejected because the EU public-sector buyer requires auditable policy.
- **Third-party (Datadog, etc.):** Rejected because the data is the user's, the rules are the user's, and the audit is the user's. We don't ship a SaaS dependency for governance.

---

## ADR-0012: One USP, defended for 24 months

**Date:** 2026-06-04
**Status:** Accepted

## Context

The product has many possible USPs:
- "Self-hosted Vercel"
- "The deployment engine"
- "Sovereign Application Runtime"
- "Vercel UX, VPS pricing"
- "Coolify alternative"
- "Kubernetes without Kubernetes"
- "The Heroku of self-hosting"
- "AI-native deploy"
- "Run your own cloud. Single binary. No DevOps team."

The team cannot defend 9 USPs. The buyer cannot remember 9 USPs. The marketing site cannot communicate 9 USPs.

## Decision

**One primary USP, defended for 24 months:**

> **"Run your own cloud. Single binary. No DevOps team."**

**Iteration rule:** if a customer uses a phrase to describe the product, that's the new USP candidate. If 3 customers use the same phrase in 30 days, that's the new USP. Test with the "explain to your mom" test, the "explain to your CTO" test, the "explain in one tweet" test.

## Consequences

**Easier:**
- The team knows what to ship (the 5-minute moment is the proof of the USP).
- The marketing site has one headline.
- The sales pitch is one sentence.
- The competitive positioning is clear (no Coolify, no Vercel, no K8s).
- The 24-month commitment prevents "USP drift."

**Harder:**
- The USP is opinionated; some buyers will reject it ("I want K8s" → "not us").
- The 24-month commitment means we cannot pivot to a new USP without a clear signal.
- The "no DevOps team" claim is hard to defend in V0 (the product requires SSH knowledge to install).

## Alternatives considered

(See [`sovereignty-and-governance.md` §4.4](#) for the full list of rejected USPs.)
- "Self-hosted Vercel" — concedes Vercel is the canonical answer
- "The deployment engine" — too generic
- "Sovereign Application Runtime" — too corporate, not a buyer-facing phrase
- "Vercel UX, VPS pricing" — used too much, loses signal
- "Coolify alternative" — concedes Coolify is the default
- "Kubernetes without Kubernetes" — concedes Kubernetes is the answer
- "The Heroku of self-hosting" — too narrow
- "AI-native deploy" — hype, not substance

---

## ADR-0013: Foundation transfer plan by year 3

**Date:** 2026-06-04
**Status:** Accepted

## Context

The CasaOS mistake: 33k stars, Apache 2.0, one corporate sponsor (IceWhale), dev moves to closed-source ZimaOS, repo rots. The bus factor is the biggest long-term risk for an open-source project.

## Decision

By year 3, the project is owned by a **foundation** (Linux Foundation Europe, Apache, or Eclipse). The transfer plan is published and signed by year 1.5.

**Conditions for the transfer:**
- The project has ≥ 3 core maintainers.
- The project has ≥ 1 EU public-sector reference customer.
- The project has a public sustainability signal (revenue or long runway).
- The foundation accepts the transfer.

## Consequences

**Easier:**
- The bus factor is no longer a single point of failure.
- EU public-sector procurement is easier (foundations are neutral, not commercial).
- The "vendor sovereignty" claim is structural, not aspirational.
- The license is irrevocable; the project is not tied to a single company.

**Harder:**
- The transfer is a multi-month legal process.
- The company loses "ownership" of the project (though the trademark remains).
- The community must accept the foundation's governance model.

## Alternatives considered

- **Never transfer; keep the project under the GmbH forever:** Rejected because the bus factor is a single point of failure.
- **Transfer to a single maintainer's personal foundation:** Rejected because that's just renaming the bus factor.
- **Transfer to a US foundation (Apache, CNCF):** Rejected because the "EU sovereign" claim is weakened.
- **Transfer to a different EU foundation (e.g., a national one):** Considered; LF Europe is the default but not the only option.

---

## ADR-0014: The 10-point sovereignty test is a CI gate

**Date:** 2026-06-04
**Status:** Accepted

## Context

The "sovereign" claim is the brand. If the claim is theatre, the product is just another Coolify. The claim must be **structural** (enforced by CI), not **aspirational** (a wiki page).

## Decision

Every release runs the **10-point sovereignty test** in CI. If any point fails, the release does not ship. The test is public (the workflow file is in the repo); the result is public (the test output is in the release notes).

## Consequences

**Easier:**
- The sovereignty claim is verifiable; the buyer can run the test themselves.
- The team is forced to maintain the sovereignty properties; the test fails if any drift.
- The 10 points are a checklist for new features: does this feature pass the maintainability test? (See [`sovereignty-and-governance.md` §6](#).)

**Harder:**
- The CI test is non-trivial; it requires a network-isolated container, a separate VM for the import test, and a re-build for the reproducibility test.
- The CI test takes longer than a normal build (~30 min vs. ~5 min).
- The "EU incorporation" point requires a public `/sovereignty` page; the team must keep it updated.

## Alternatives considered

- **Wiki page that says "we're sovereign":** Rejected because the buyer cannot verify the claim.
- **Annual external audit:** Rejected because it's expensive, slow, and not continuous.
- **Self-attestation with a checkbox:** Rejected because it's the same as the wiki page.
- **Third-party certification (BSI C5, EUCS):** Accepted as V2, but the CI gate is the floor.

---

## ADR-0015: "Sovereign" means 5 dimensions, not 1

**Date:** 2026-06-04
**Status:** Accepted

## Context

Hyperscalers (AWS, GCP, Azure) have started marketing "sovereign cloud" SKUs. The marketing is about data residency (data is in the EU), but the corporate structure is still US. The buyer who reads the marketing is misled.

## Decision

The product claims **5 dimensions of sovereignty**, all by construction:

1. **Data sovereignty:** my data is in a jurisdiction I trust
2. **Operational sovereignty:** I can run it without calling anyone for permission
3. **Vendor sovereignty:** the vendor can disappear and the product keeps working
4. **Legal sovereignty:** the company is incorporated where I trust
5. **Technical sovereignty:** the technology is open and inspectable

Hyperscalers can claim 1-2. The product claims all 5.

## Consequences

**Easier:**
- The 5-dimension definition is unambiguous; the buyer knows what they're getting.
- Each dimension is backed by a feature, a test, and a public doc.
- The marketing is honest; the buyer is not misled.

**Harder:**
- The 5-dimension definition is harder to communicate than "data is in the EU."
- The team must maintain all 5 dimensions; a shortcut in one breaks the claim.
- The "operational sovereignty" claim is hard to defend in V0 (the product requires SSH knowledge).

## Alternatives considered

- **One dimension (data residency):** Rejected because it's the same as the hyperscalers' "sovereign cloud" SKU.
- **Two dimensions (data + corporate):** Rejected because the operational, vendor, and technical dimensions are also important.
- **Three dimensions (data + corporate + open source):** Considered; the 5-dimension definition is the more rigorous version.

---

## Future ADRs (planned, not yet drafted)

- ADR-0016: Nginx low-mem mode (V1.1)
- ADR-0017: Podman runtime support (V1.5)
- ADR-0018: ML scorer model architecture (V2)
- ADR-0019: BSI C5:2026 certification timeline (V2)
- ADR-0020: Foundation selection (V1.5)
- ADR-0021: True canary rollout strategy (V2.5)
- ADR-0022: Multi-region failover (V3)
- ADR-0023: OIDC SSO via `openidconnect` crate (G21, V1) — Delegate to Authentik/Keycloak/Okta/Entra ID; do not build an IdP. Local admin fallback is mandatory.
- ADR-0024: Log shipping delegated to 7 external sinks (G22, V1) — Loki/Datadog/Better Stack/Splunk HEC/Sumo/syslog/Vector; we do not embed Elasticsearch or Loki in the binary.
- ADR-0025: Audit log is append-only with Ed25519 chain (G23, V1) — SQLite VFS rejects UPDATE/DELETE; per-box Ed25519 keypair; chain verifiable via `sovereign audit verify`.

---

**Next: read [`negative-prompt.md`](./negative-prompt.md) for the anti-patterns and named case studies, and [`enterprise-readiness.md`](./enterprise-readiness.md) for the support tiers, SLAs, framework mappings, and the enterprise onboarding playbook.**
