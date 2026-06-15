# Phase 3 — The Platform (V2)

**Timeline:** Months 10-18 (9 months)
**Goal:** rqlite-based HA control plane, OPA/Rego policy engine, ML-based anomaly scorer, EUCS Substantial + BSI C5:2026 mapping, web UI (opt-in HTMX), Pro tier, foundation transfer plan, the 10-point sovereignty test as a CI gate. Doctor promoted to **paranoid** level (100+ checks) with **compliance scan**, **canary integration**, and **auto-tune**.
**Audience:** The lead engineer + 5-8 contributors. The first EU mid-market and EU public-sector customers.
**Definition of Done:** €100k MRR, 1 EU public-sector reference customer, BSI C5:2026 mapping published, Pro tier live, foundation transfer plan signed, `sovereign doctor --level paranoid` returns 0 fails, the 10-point sovereignty test is a doctor subcommand.

---

## 0. The quarter-by-quarter plan

```text
Months 10-12: I1 (rqlite HA) + I2 (DR / export / import) + I3 (air-gapped install) + I4 (OPA/Rego) + I5 (ML scorer, shadow mode) + I16 (doctor --level paranoid)
Months 13-15: I6 (risk scoring + auto-canary) + I7 (web UI, HTMX, opt-in) + I8 (Pro tier) + I9 (EU pricing) + I10 (EU incorporation complete) + I17 (sovereign compliance scan)
Months 16-18: I11 (BSI C5:2026 mapping) + I12 (EUCS Substantial doc) + I13 (SBOM + cosign + reproducible builds) + I14 (foundation transfer plan) + I15 (V2 release) + I18 (sovereign canary) + I19 (sovereign auto-tune)
```

---

## I1. rqlite-based HA control plane

**Goal:** V1.5's agent pattern becomes V2's rqlite HA. Three rqlite nodes form a Raft consensus group. The control plane survives 1 node failure.

**Why it matters:** The single most important architectural milestone. The "bet we cannot afford to be wrong about" (per [`architecture.md` §1](#) and the CTO verdict in `../persona-cto.md`).

### Sub-tasks

1. **Implement `RqliteState`** in `sovereign-storage-rqlite/`:
   - Same `StoragePort` trait as `SqliteState`.
   - Talks to rqlite over HTTP (rqlite's wire protocol is HTTP + JSON).
   - All writes go through rqlite's `/db/execute?level=strong` for linearizability.
   - Reads can use `/db/query?level=weak` for performance.
2. **Add rqlite as a separate process** managed by the binary (or as a sidecar container managed by sovereign itself).
3. **Add a `state.backend` config option** to `sovereign.toml`:
   ```toml
   [state]
   backend = "rqlite"  # or "sqlite"
   nodes = ["https://rqlite-1:4001", "https://rqlite-2:4001", "https://rqlite-3:4001"]
   ```
4. **Implement the failover logic** — when the leader is down, the surviving nodes elect a new leader via Raft. The sovereign binary retries on the new leader.
5. **Implement rqlite backup to S3** via the rqlite backup API.
6. **Update the `sovereign-storage-sqlite` and `sovereign-storage-rqlite` crates** to be feature-flagged; the binary selects one.

### Code stub

```rust
// crates/sovereign-storage-rqlite/src/lib.rs
use async_trait::async_trait;
use reqwest::Client;
use sovereign_core::ports::StoragePort;

pub struct RqliteState {
    client: Client,
    nodes: Vec<String>,  // rqlite URLs, in priority order
    current_leader: Arc<tokio::sync::RwLock<Option<String>>>,
}

impl RqliteState {
    pub async fn connect(nodes: Vec<String>) -> anyhow::Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?;
        // Discover the current leader
        let leader = Self::discover_leader(&client, &nodes).await?;
        Ok(Self {
            client,
            nodes,
            current_leader: Arc::new(tokio::sync::RwLock::new(Some(leader))),
        })
    }
    
    async fn discover_leader(client: &Client, nodes: &[String]) -> anyhow::Result<String> {
        for node in nodes {
            let resp = client.get(format!("{}/status", node)).send().await?;
            if resp.status().is_success() {
                let body: serde_json::Value = resp.json().await?;
                if let Some(leader) = body["store"]["leader"].as_str() {
                    return Ok(leader.to_string());
                }
            }
        }
        anyhow::bail!("no rqlite leader found");
    }
}

#[async_trait]
impl StoragePort for RqliteState {
    async fn create_app(&self, new: NewApp, actor: &str) -> Result<App, AppError> {
        let leader = self.current_leader.read().await.clone()
            .ok_or(AppError::Internal("no rqlite leader".into()))?;
        let sql = format!(
            "INSERT INTO app (id, name, owner, env, config_yaml, created_at, updated_at, version) VALUES ('{}', '{}', '{}', '{}', '{}', {}, {}, 1)",
            new.id.0, new.name, new.owner, new.env, new.config_yaml, now(), now()
        );
        let resp = self.client.post(format!("{}/db/execute?level=strong", leader))
            .json(&serde_json::json!([sql]))
            .send().await?;
        if !resp.status().is_success() {
            // Leader may have changed; rediscovery
            let new_leader = Self::discover_leader(&self.client, &self.nodes).await?;
            *self.current_leader.write().await = Some(new_leader.clone());
            // Retry
            return Box::pin(self.create_app(new, actor)).await;
        }
        // ... read back the row
    }
}
```

### Tests

- Unit test: `RqliteState` is mockable with `wiremock`.
- Integration test (3-node rqlite cluster via `testcontainers`): kill the leader, verify the new leader is elected within 5s, verify a write to the new leader succeeds.
- Integration test: 2 of 3 nodes down, writes fail (consensus requires majority).
- Integration test: 1 of 3 nodes down, writes succeed (consensus has 2/3 majority).

### Acceptance criteria

- [ ] A 3-node rqlite cluster survives the loss of any 1 node.
- [ ] Writes are linearizable (`level=strong`).
- [ ] Reads can be `level=weak` for performance.
- [ ] The sovereign binary auto-rediscovers the leader on failover.

### Definition of done

A test that deploys a 3-node rqlite cluster in CI, deploys an app, kills the rqlite leader, verifies the new leader is elected, verifies the app still serves traffic — passes in CI on every PR.

---

## I2. Disaster recovery — `sovereign export / import / recover`

**Goal:** `sovereign export platform > backup.tar.gz` produces a complete backup (SQLite + Litestream WAL + S3 backups + config). `sovereign import platform < backup.tar.gz` restores on a fresh host. `sovereign recover` does the same, interactively.

**Why it matters:** The 100-year-old question. "If you disappear, what happens to the users?" The answer is in the binary, the export, the import.

### Sub-tasks

1. **Implement `sovereign export platform`**:
   - Stop the control plane.
   - Snapshot the SQLite DB (via `VACUUM INTO`).
   - Bundle the SQLite snapshot, the Litestream WAL position, the config, and the manifest of S3 backups.
   - Tar + gzip + cosign sign.
   - Output: `sovereign-backup-<timestamp>.tar.gz` + `.sig` + `.pem`.
2. **Implement `sovereign import platform < backup.tar.gz`**:
   - Verify the cosign signature.
   - Restore the SQLite snapshot.
   - Restore the config.
   - Optionally restore the S3 backups.
   - Restart the control plane.
3. **Implement `sovereign recover`** — interactive mode that walks the user through import.
4. **Document the vendor-disappear test** as a CI gate (see I13).

### Acceptance criteria

- [ ] `sovereign export platform` produces a tarball with the full platform state.
- [ ] On a fresh VM, `sovereign import platform < backup.tar.gz` restores the platform.
- [ ] The cosign signature is verified on import.
- [ ] The import is idempotent (running twice = same result).

---

## I3. Air-gapped install (offline binary + offline docs)

**Goal:** The product can be installed and operated without any internet connection. The binary is self-contained; the docs are bundled.

**Why it matters:** EU public sector and some defense-adjacent customers operate air-gapped. A product that requires internet for installation cannot serve them.

### Sub-tasks

1. **Bundle a local Caddy binary** in the deb/rpm package (V1.5: system Caddy; V2: bundled).
2. **Bundle a local Docker / Podman binary** (V2: optional; default is system).
3. **Bundle the docs** in the deb/rpm package.
4. **Document the air-gapped install** in the docs site.
5. **Add a CI test** that runs in a network-isolated container and verifies all operations work.

### Acceptance criteria

- [ ] On a VM with no internet, `apt install sovereign` and `sovereign deploy` both work.
- [ ] No outbound network calls are made by the binary in air-gapped mode.

---

## I4. OPA / Rego policy engine

**Goal:** `sovereign policy check --app <APP> --env prod` runs the installed Rego rule packs against the current state and returns `allow | deny | warn`. `sovereign policy install <pack.rego>` uploads a rule pack.

**Why it matters:** Strict governance at scale. Rules are the law; ML is the early-warning radar (I5).

### Sub-tasks

1. **Implement `RegoPolicy`** in `sovereign-policy-rego/`:
   - In-process Rego evaluation via `regorus` (Rust Rego implementation).
   - Optionally, OPA WASM bundles for advanced rules.
2. **Add the `policy_pack` table** (or extend the storage).
3. **Implement the 3 default rule packs**:
   - `baseline.rego` — non-negotiable guardrails (no unsigned images, no public-read buckets, no unowned services)
   - `cost.rego` — bill-shock prevention (egress budget caps, instance type restrictions)
   - `compliance.rego` — BSI C5:2026 / EUCS mapping (audit log required, encryption at rest required, etc.)
4. **Wire `sovereign policy check`** — dry-run the rules against current state.
5. **Wire `sovereign policy install <pack>`** — upload a rule pack.
6. **Wire `sovereign policy list`** — show installed packs.
7. **Integrate with the deploy use case** — every deploy runs `policy::enforce` first; `deny` blocks the deploy.
8. **Document the rule schema** in the docs site.
9. **Ship 4 DB-migration safety templates** (the expand/contract patterns that the `user-pain-research.md` §4.3 calls the #1 source of "the migration that took the site down" — every Rails / Django / Prisma team hits this eventually). These are Rego policies that run at `deploy` time against the `app.yaml` + the active DB connection, and `deny` the deploy if the migration looks risky:
   - **`migrations/expand_contract_required.rego`** — A migration file in `migrations/` that contains `DROP COLUMN`, `RENAME COLUMN`, `ALTER COLUMN ... TYPE`, or `DROP TABLE` must also contain the corresponding `add_*.sql` and `backfill_*.sql` files in `migrations/expand/` and `migrations/contract/` for the next 2 release versions. Rationale: forces the expand → migrate → contract pattern (Prisma's "expand & contract", Rails' "strong migrations", Django's "separate_schema_and_data_migrations"). The check parses the migration's `up` SQL; if it sees a destructive DDL keyword, it walks the next 2 release branches in git and requires the matching expand + backfill files. If the files are missing, `deny` with: `migration 0042_drop_users_email is destructive; create migrations/expand/0042_add_users_email_legacy.sql and migrations/backfill/0042_backfill_users_email_legacy.sql in the next 2 release branches`.
   - **`migrations/lock_timeout_required.rego`** — Every Postgres app must set `lock_timeout` in the DB connection pool (already covered by the `migration_lock_timeout` doctor check in G16). The Rego check duplicates the rule at the *policy* layer so a violating config can be blocked at deploy time, not just detected at morning doctor. `deny` with: `app 'api-prod' has no lock_timeout configured; set db.pool.lock_timeout: 5s in app.yaml`.
   - **`migrations/large_table_alter_warn.rego`** — A migration that runs `ALTER TABLE` on a table with ≥ 1M rows (estimated via `pg_stat_user_tables.n_live_tup` from the inspector) is `warn` (not `deny`) with a hint to use `pg_repack` / `pg_rewrite` for online schema change. The operator can `--policy-warn-allow` to ship anyway, but the audit log records the override (the I5 ML scorer is fed the override as a labeled "this was a risky deploy" event).
   - **`migrations/concurrent_index_required.rego`** — A migration that creates an index on a table with ≥ 100K rows must use `CREATE INDEX CONCURRENTLY` (Postgres) or `ALGORITHM=INPLACE, LOCK=NONE` (MySQL). Plain `CREATE INDEX` is `deny` for big tables — the table lock blocks reads for the duration of the index build. `deny` with: `migration 0103_idx_orders_user_id would lock 'orders' (1.2M rows); use CREATE INDEX CONCURRENTLY`.
   - All 4 templates are loaded by default in V2; they can be disabled with `sovereign policy disable migrations/lock_timeout_required` (audit log records the disable).
   - A new `sovereign policy templates` command lists the 4 with one-line descriptions and links to `/docs/policy/migration-safety/`.

### Code stub

```rust
// crates/sovereign-policy-rego/src/lib.rs
use regorus::Engine;

pub struct RegoPolicy {
    engine: Engine,
}

impl RegoPolicy {
    pub fn new() -> Self {
        let mut engine = Engine::new();
        // Load the default rule packs
        engine.add_policy("baseline".into(), include_str!("packs/baseline.rego").to_string())?;
        engine.add_policy("cost".into(), include_str!("packs/cost.rego").to_string())?;
        engine.add_policy("migrations".into(), include_str!("packs/migrations.rego").to_string())?;
        Self { engine }
    }

    pub fn check(&self, input: &PolicyInput) -> Result<PolicyDecision, PolicyError> {
        self.engine.set_input(input);
        let result = self.engine.eval_query("data.sovereign.baseline.deny", true)?;
        // ... evaluate cost, compliance, migrations, return combined decision
    }
}
```

```rego
# baseline.rego
package sovereign.baseline

deny[msg] {
    input.kind == "deploy"
    not input.image.attestation.signature
    msg := sprintf("image %v has no cosign signature", [input.image.ref])
}

deny[msg] {
    input.kind == "config_change"
    input.resource.type == "bucket"
    input.resource.acl == "public-read"
    msg := "public-read buckets are not allowed"
}

deny[msg] {
    input.kind == "deploy"
    not input.service.owner
    msg := sprintf("service %v has no owner declared", [input.service.name])
}
```

```rego
# migrations/expand_contract_required.rego
package sovereign.migrations

import future.keywords.in

# A migration is "destructive" if its up SQL contains any of these keywords
destructive_keywords := ["DROP COLUMN", "RENAME COLUMN", "ALTER COLUMN", "DROP TABLE", "TRUNCATE"]

is_destructive(migration) {
    some kw in destructive_keywords
    contains(migration.up_sql, kw)
}

# The next 2 release branches must contain the matching expand + backfill files
deny[msg] {
    input.kind == "deploy"
    some m in input.app.migrations
    is_destructive(m)
    not has_expand_file(m, input.release.next_branches)
    msg := sprintf(
        "migration %v is destructive; create migrations/expand/%v_add_legacy.sql in the next 2 release branches",
        [m.name, m.name]
    )
}

deny[msg] {
    input.kind == "deploy"
    some m in input.app.migrations
    is_destructive(m)
    not has_backfill_file(m, input.release.next_branches)
    msg := sprintf(
        "migration %v is destructive; create migrations/backfill/%v_backfill_legacy.sql in the next 2 release branches",
        [m.name, m.name]
    )
}

# migrations/lock_timeout_required.rego
deny[msg] {
    input.kind == "deploy"
    input.app.db.engine == "postgres"
    not input.app.db.pool.lock_timeout
    msg := sprintf(
        "app '%v' has no lock_timeout configured; set db.pool.lock_timeout: 5s in app.yaml",
        [input.app.name]
    )
}

# migrations/concurrent_index_required.rego
deny[msg] {
    input.kind == "deploy"
    some m in input.app.migrations
    creates_index(m)
    not uses_concurrent(m)
    m.target_table.n_live_tup >= 100000
    msg := sprintf(
        "migration %v would lock '%v' (%v rows); use CREATE INDEX CONCURRENTLY",
        [m.name, m.target_table.name, m.target_table.n_live_tup]
    )
}

warn[msg] {
    input.kind == "deploy"
    some m in input.app.migrations
    alters_large_table(m)
    msg := sprintf(
        "migration %v alters '%v' (%v rows); consider pg_repack for online schema change",
        [m.name, m.target_table.name, m.target_table.n_live_tup]
    )
}
```

### Tests

- Unit: each of the 4 templates is tested against a synthetic `PolicyInput` (4 cases: clean expand/contract, missing expand file, missing backfill, missing concurrent index on 1M-row table).
- Integration: a fixture Rails app with a `0042_drop_users_email.sql` migration that has no expand file is deployed against the `sovereign policy` test harness; the deploy is `deny`'d with the expected message; adding the expand + backfill files to the next 2 release branches makes the deploy `allow`.
- Integration: a fixture Django app with a plain `CREATE INDEX` on a 1M-row table is denied; switching to `CREATE INDEX CONCURRENTLY` makes it `allow`.
- Integration: a fixture app that runs `ALTER TABLE` on a 5M-row table is `warn`'d (not denied); `sovereign deploy --policy-warn-allow` ships it; the override appears in the audit log.

### Acceptance criteria

- [ ] `sovereign policy check --app api --env prod` returns the policy decision.
- [ ] A deploy that violates a rule is blocked with a clear error.
- [ ] Custom rule packs can be installed via `sovereign policy install <pack.rego>`.
- [ ] `sovereign policy templates` lists the 4 migration-safety templates.
- [ ] A deploy with a missing expand file is denied with the expected message.
- [ ] A deploy with a missing `CONCURRENTLY` keyword on a big table is denied with the expected message.

---

## I5. ML-based anomaly scorer (shadow mode)

**Goal:** A Prophet-based anomaly scorer runs in shadow mode, scores every deploy, logs the score to the audit log, and (in month 5 of this phase) starts to influence low-risk decisions.

**Why it matters:** ML catches the slow drifts that rules miss. Pure rules are brittle; pure ML is opaque. Layered is the answer.

### Sub-tasks

1. **Implement `ProphetScorer`** in `sovereign-ml-scorer/`:
   - Fits a Prophet model per (app, metric) on the last 30 days of telemetry.
   - On each deploy, scores the predicted vs. actual CPU/mem/error_rate/p99 for the next 1h.
   - Returns a 0-100 risk score with the top-3 contributing features.
2. **Run in shadow mode for 2 months** — the scorer runs, scores every deploy, but doesn't influence decisions. The scores are logged to the audit log for human review.
3. **Document the model card** (training data, feature set, performance metrics, failure modes).
4. **Wire `sovereign policy advise`** — the CLI says "ML scored this deploy 67, recommend canary at 5%" but doesn't enforce.

### Acceptance criteria

- [ ] The scorer runs on every deploy and emits a risk score.
- [ ] The score is in the audit log.
- [ ] The model card is published on the docs site.
- [ ] `sovereign policy advise` returns the recommendation without enforcing it.

---

## I6. Risk scoring + auto-canary

**Goal:** Every deploy is scored 0-100. The score determines the rollout strategy: 0-19 = auto-approve, 20-49 = log, 50-79 = canary at 5%, 80-100 = halt + page on-call.

**Why it matters:** Risk-based deploys. Low-risk deploys ship fast; high-risk deploys get human attention.

### Sub-tasks

1. **Implement the risk score formula** from `../persona-crosscutting.md` §2:
   ```text
   risk_score = (
     0.30 * change_size_factor
     + 0.20 * env_factor
     + 0.15 * author_history_factor
     + 0.10 * time_factor
     + 0.10 * dependency_factor
     + 0.10 * secret_factor
     + 0.05 * anomaly_factor
   ) * 100
   ```
2. **Implement the 4 rollout bands**: auto-approve, log, canary, halt.
3. **Wire auto-canary at 5%** for the 50-79 band.
4. **Wire the halt + page on-call** for the 80-100 band, with `sovereign deploy --confirm-risk` to bypass.
5. **Document the risk model** in the docs site.

### Acceptance criteria

- [ ] Every deploy has a risk score in the audit log.
- [ ] A 50-79 score triggers a canary at 5%.
- [ ] An 80-100 score halts the deploy and pages on-call.
- [ ] `sovereign deploy --confirm-risk` bypasses the halt.

---

## I7. Web UI (HTMX, opt-in, never primary)

**Goal:** A web UI at `https://<host>:7878/ui` (V2: opt-in via `--features web`) that shows the same information as the TUI, but in a browser. Uses HTMX (~30 KB), not React/Vue.

**Why it matters:** Some users don't live in the terminal. The web UI is the escape hatch — never the primary surface, but a real, working alternative.

### Sub-tasks

1. **Add `axum` routes for `/ui/*`** behind `--features web`.
2. **Implement HTMX views** (server-rendered HTML, ~30 KB of JS):
   - Dashboard (mirrors the TUI Pulse)
   - Apps list + detail
   - Servers list
   - Backups list
   - Audit log
3. **No JS framework.** No React, no Vue, no Svelte. HTMX + a thin CSS file.
4. **The web UI is a consumer of the HTTP API.** It does not have its own backend logic.

### Acceptance criteria

- [ ] `sovereign http --features web` serves the UI at `/ui`.
- [ ] The dashboard shows the same info as the TUI Pulse.
- [ ] No JS framework is loaded; the page works with JS disabled (graceful degradation).

---

## I8. Pro tier (managed updates + support)

**Goal:** A commercial Pro tier at €19/server/month that includes managed updates, email support, EU-region updates, official deb/rpm repos, and a 1-day SLA on security advisories.

**Why it matters:** The unit economics. Open source is the funnel; Pro is the revenue.

### Sub-tasks

1. **Implement license key validation** (V2.5: full SaaS; V2: simple signed license file).
2. **Implement the `sovereign update` command** that checks for updates, downloads, verifies the cosign signature, and applies.
3. **Implement a Pro-only deb/rpm repo** (apt.sovereignruntime.dev, yum.sovereignruntime.dev).
4. **Implement Pro-only features**:
   - Auto-update channel (stable, beta, nightly)
   - Email support ticketing
   - EU-region update mirrors
5. **Integrate with Stripe** for billing.
6. **Build the marketing site** at sovereignruntime.dev with the pricing page.
7. **Document the upgrade path** in the docs site.

### Acceptance criteria

- [ ] A Pro license unlocks the Pro features.
- [ ] `sovereign update` works for Pro users.
- [ ] The deb/rpm repo is reachable and signed.
- [ ] Stripe billing works end-to-end.

---

## I9. EU pricing (per-node)

**Goal:** Pricing is per-node, in EUR, with EU VAT handling. Annual contracts available with a 15% discount.

### Sub-tasks

1. **Update the pricing page** to show the per-node model.
2. **Implement annual contracts** via Stripe.
3. **Implement EU VAT handling** (Stripe Tax or a 3rd-party).
4. **Document the pricing** in the docs site and the marketing site.

### Acceptance criteria

- [ ] Pricing is per-node in EUR.
- [ ] Annual contracts are available.
- [ ] EU VAT is handled correctly.

---

## I10. EU incorporation complete

**Goal:** The GmbH is fully operational. The OÜ is fully operational. The company has a real bank account, a real tax ID, a real registered address.

### Sub-tasks

1. **Open a business bank account** in Berlin (Deutsche Bank, solarisbank, or similar).
2. **Register for VAT** in Germany.
3. **Hire a tax advisor** for the GmbH + OÜ.
4. **Apply for EU public-sector procurement IDs** (e.g., the German " Vergabestelle" registration).
5. **Publish the corporate details** on the website.

### Acceptance criteria

- [ ] The GmbH has a bank account, a tax ID, a VAT ID.
- [ ] The OÜ is operational.
- [ ] The corporate details are published.

---

## I11. BSI C5:2026 mapping (public document)

**Goal:** A public `docs/sovereignty/bsi-c5-2026-mapping.md` that maps each BSI C5:2026 control to the product feature that satisfies it.

**Why it matters:** EU public-sector procurement requires BSI C5:2026 (or equivalent). A public mapping is the entry ticket.

### Sub-tasks

1. **Engage a BSI C5:2026 auditor** (KPMG, TÜV, etc.) for a gap analysis.
2. **Write the mapping document** (121 controls → product features).
3. **Publish the mapping** on the docs site.
4. **Run a gap analysis** internally and fix any gaps in the V2.5 cycle.

### Acceptance criteria

- [ ] The mapping covers all 121 BSI C5:2026 controls.
- [ ] The mapping is published.
- [ ] A gap analysis report is filed.

---

## I12. EUCS Substantial controls checklist (public document)

**Goal:** A public `docs/sovereignty/eucs-substantial-checklist.md` that maps each EUCS Substantial control to the product feature that satisfies it.

### Sub-tasks

1. **Engage an EUCS auditor** for a gap analysis.
2. **Write the mapping document** (similar to BSI C5).
3. **Publish the mapping**.

### Acceptance criteria

- [ ] The mapping covers all EUCS Substantial controls.
- [ ] The mapping is published.

---

## I13. SBOM + cosign signing + reproducible builds (CI gate)

**Goal:** Every release has a CycloneDX SBOM, is signed with cosign, and is reproducible bit-for-bit from the source.

**Why it matters:** Sovereignty is a structural claim. SBOM, cosign, and reproducible builds are the technical proof.

### Sub-tasks

1. **Generate CycloneDX SBOM** at build time via `cargo-cyclonedx`.
2. **Sign the release artifacts** with cosign (keyless, via Sigstore Fulcio).
3. **Implement reproducible builds** via `cargo build --locked` + a pinned toolchain + `SOURCE_DATE_EPOCH`.
4. **Verify the build** in CI by re-building and comparing SHA-256 hashes.
5. **Publish the SBOM, the signature, and the verification instructions** with each release.

### Acceptance criteria

- [ ] Every release has a CycloneDX SBOM.
- [ ] Every release is cosign-signed.
- [ ] The release is reproducible bit-for-bit.
- [ ] The verification instructions are public.

---

## I14. Foundation transfer plan (signed)

**Goal:** A signed letter of intent to transfer the project to a foundation (Linux Foundation Europe, Apache, Eclipse) by year 3. The plan is published.

**Why it matters:** Bus factor. The CasaOS mistake.

### Sub-tasks

1. **Engage a foundation** (LF Europe is the most likely).
2. **Write the transfer plan** (trademark, code, domain, infrastructure).
3. **Get the plan signed** by the founders, the core maintainers, and the foundation.
4. **Publish the plan** on the website.

### Acceptance criteria

- [ ] The plan is signed.
- [ ] The plan is published.

---

## I15. V2 release + blog post

**Goal:** A V2 release announcement. The "sovereign PaaS" is real. The Pro tier is live. The 10-point sovereignty test is a CI gate.

### Sub-tasks

1. **Tag the release** as `v2.0.0`.
2. **Generate the release notes**.
3. **Write the blog post** announcing V2: rqlite HA, OPA/Rego, ML scorer, EUCS, web UI, Pro tier.
4. **Record a 10-minute demo video**.
5. **Re-engage all V1.5 design partners** with the V2 upgrade.
6. **Pitch EU mid-market and EU public-sector customers** with the BSI C5 + EUCS mapping.

### Acceptance criteria

- [ ] `v2.0.0` is tagged and released.
- [ ] Blog post is published.
- [ ] Demo video is published.
- [ ] First EU mid-market customer signed.
- [ ] First EU public-sector customer signed.

---

## I16. `sovereign doctor --level paranoid` — 100+ checks, the 10-point sovereignty test, and the deepest mode

**Goal:** Promote `sovereign doctor` from V1.5 **full** (70+ checks) to V2 **paranoid** (100+ checks, every category saturated, the 10-point sovereignty test as a first-class check group, and the "this is what BSI C5 / EUCS / SOC2 would check" mode). Paranoid is the V2 release's claim to "we are a sovereign PaaS" — the level that an EU public-sector auditor runs.

**Why it matters:** Full level catches 70+ failure modes. The V2 release's promise is different: "if `sovereign doctor --level paranoid` returns 0 fails, this box passes the 10-point sovereignty test from [`sovereignty-and-governance.md`](./sovereignty-and-governance.md), the BSI C5:2026 mapping from I11, and the EUCS Substantial checklist from I12." That's the claim that sells to EU public sector. The claim is verifiable in one command, on the operator's box, with no black-box SaaS. This is the highest-leverage feature in V2.

### Sub-tasks

1. **Add the 30+ new V2 checks** (the paranoid set):
   - **Sovereignty** (+10) — the 10-point sovereignty test as a single check group. Each of the 10 points (corporate jurisdiction, code provenance, build environment, distribution channel, SBOM, signature, license compliance, dependency map, egress allowlist, decommission plan) is a sub-check. The check group returns `Pass` if all 10 sub-checks pass.
   - **Security** (+6) — `bsi_c5_cryptography` (only FIPS-approved ciphers in TLS, SSH, at-rest), `bsi_c5_logging` (the audit log meets C5 §8 — tamper-evident, time-stamped, retained 1 year), `bsi_c5_authentication` (MFA enabled for all admin users, password policy enforced), `eucs_substantial_geo_restriction` (no data ever leaves the EU jurisdiction; a strict egress allowlist), `eucs_substantial_personnel` (the foundation transfer plan is in place; the binary's maintainer list is published), `no_telemetry` (a `tcpdump` of the binary's egress for 1 hour shows zero packets to non-allowlisted hosts; the implementation reuses the V1.5 `no_us_cloud_dependencies_egress` check but with a stricter 1-hour observation window).
   - **Backup** (+3) — `backup_restore_drill_automated` (the V1.5 game-day scenario `db-failover` runs on a cron and the result is in the audit log), `backup_immutability` (the S3 bucket has Object Lock enabled; the dump cannot be deleted before the retention period expires), `backup_geo_redundancy` (≥ 1 copy is in a different EU jurisdiction than the primary).
   - **Network** (+3) — `tls_1_3_only` (no TLS 1.0/1.1/1.2 connections accepted; verified by a `nmap --script ssl-enum-ciphers`), `hsts_preload` (the served domain has HSTS preload; the check verifies the header is set and the domain is on the preload list), `dane_tlsa` (DNSSEC + TLSA records for the served domain; the check is a no-op until the operator enables it).
   - **Performance** (+3) — `p99_deploy_latency` (the 99th percentile of `sovereign bench` `deploy_latency` over the last 7 days < 5× p50), `audit_log_replay` (the audit log can be replayed end-to-end in < 60s; a regression test for the V1.5 `audit_log_hash_chain`), `backup_window_respected` (the daily backup window fits in the 1-hour `0 3 * * *` cron slot; the check is the bench `backup_create_verify` result).
   - **Cost** (+3) — `cost_per_box` (estimated $/box/month, computed from `sovereign cost` divided by the box count), `cost_anomaly` (a > 3× deviation from the 30-day median is a Warn), `pro_tier_breakeven` (if the operator is paying for the Pro tier, the cost savings from the included features > the Pro subscription; advisory).
   - **Observability** (+2) — `slo_definition_published` (the operator's 3 SLOs from [`operations-runbook.md`](./operations-runbook.md) are present in a `slo.yaml` and the doctor validates the format), `alerting_routes_valid` (every alert in the alertmanager config has a Slack or PagerDuty route).
2. **Implement the `ParanoidExtension`** in `sovereign-doctor/src/extensions/paranoid.rs`:
   ```rust
   pub struct ParanoidExtension;
   impl DoctorExtension for ParanoidExtension {
       fn name(&self) -> &'static str { "paranoid" }
       fn level(&self) -> DoctorLevel { DoctorLevel::Paranoid }
       fn checks(&self) -> Vec<Box<dyn Check>> {
           let mut v = FullExtension.checks();
           v.extend_from_slice(sovereignty::ten_point_test::checks());
           v.extend_from_slice(security::bsi_c5::checks());
           v.extend_from_slice(security::eucs::checks());
           // ... 30+ new checks ...
           v
       }
       fn fixes(&self) -> Vec<Box<dyn DoctorFix>> { /* ... 15+ new fixes ... */ }
   }
   ```
3. **Implement the 10-point sovereignty test as a single check group** — the test is defined in [`sovereignty-and-governance.md`](./sovereignty-and-governance.md) §2. Each point is a check; the group returns `Pass` iff all 10 pass. The output is a per-point breakdown: `1. ✓ corporate_jurisdiction: GmbH registered in DE; 2. ✓ code_provenance: signed by EU-resident CI key; ...`.
4. **Implement `sovereign sovereignty-test`** as an alias of `sovereign doctor --level paranoid --category sovereignty` — the auditor's command. It exits 0 iff the 10-point test passes.
5. **Implement the V2 fixes** (15+ new fixes, all non-destructive or with explicit confirm):
   - `bsi_c5_logging_retain_1y` — re-renders the logrotate config with `rotate 365` and reloads.
   - `eucs_geo_restriction_apply` — re-renders Caddy's egress filter with the strict EU-only allowlist.
   - `hsts_preload_apply` — re-renders the Caddy config with `header Strict-Transport-Security "max-age=63072000; includeSubDomains; preload"`.
   - `tls_1_3_only_apply` — re-renders the Caddy config to require TLS 1.3.
   - ... (11 more)
6. **Author the 30+ new KB articles** — the V2 KB is 100+ articles.
7. **Wire paranoid into the V2 release's CI gate** — every release PR runs `sovereign doctor --level paranoid` against the secure fixture; a Fail fails the build.
8. **Add Prometheus metrics**:
   - `sovereign_doctor_paranoid_runs_total{result}` — counter.
   - `sovereign_sovereignty_test_passes_total` — counter, incremented when the 10-point test passes.
   - `sovereign_sovereignty_test_fails_total{point}` — counter, per failed point.

### Code stub

```rust
// crates/sovereign-doctor/src/extensions/paranoid.rs
use crate::{DoctorExtension, DoctorLevel};
pub struct ParanoidExtension;
impl DoctorExtension for ParanoidExtension {
    fn name(&self) -> &'static str { "paranoid" }
    fn level(&self) -> DoctorLevel { DoctorLevel::Paranoid }
    fn checks(&self) -> Vec<Box<dyn Check>> {
        let mut v = FullExtension.checks();
        // 10-point sovereignty test
        v.extend(sovereignty::ten_point_test::checks());
        // BSI C5:2026
        v.extend(security::bsi_c5::BSI_C5_CHECKS.iter().map(|c| c.boxed()));
        // EUCS Substantial
        v.extend(security::eucs::EUCS_CHECKS.iter().map(|c| c.boxed()));
        // The remaining 30+ checks
        v.extend_from_slice(&[
            Box::new(backup::BackupRestoreDrillAutomatedCheck),
            Box::new(backup::BackupImmutabilityCheck),
            Box::new(backup::BackupGeoRedundancyCheck),
            Box::new(network::Tls13OnlyCheck),
            Box::new(network::HstsPreloadCheck),
            Box::new(network::DaneTlsaCheck),
            Box::new(performance::P99DeployLatencyCheck),
            Box::new(performance::AuditLogReplayCheck),
            Box::new(performance::BackupWindowRespectedCheck),
            Box::new(cost::CostPerBoxCheck),
            Box::new(cost::CostAnomalyCheck),
            Box::new(cost::ProTierBreakevenCheck),
            Box::new(observability::SloDefinitionPublishedCheck),
            Box::new(observability::AlertingRoutesValidCheck),
        ]);
        v
    }
    fn fixes(&self) -> Vec<Box<dyn DoctorFix>> { /* ... */ }
}
```

```rust
// crates/sovereign-doctor/src/checks/sovereignty/ten_point_test.rs
pub fn checks() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(CorporateJurisdictionCheck),
        Box::new(CodeProvenanceCheck),
        Box::new(BuildEnvironmentCheck),
        Box::new(DistributionChannelCheck),
        Box::new(SbomPresentCheck),
        Box::new(SignatureValidCheck),
        Box::new(LicenseComplianceCheck),
        Box::new(DependencyMapCheck),
        Box::new(EgressAllowlistCheck),
        Box::new(DecommissionPlanCheck),
    ]
}
pub struct TenPointTestGroup;
impl TenPointTestGroup {
    pub fn evaluate(results: &[CheckResult]) -> GroupResult {
        let pass = results.iter().all(|r| r.status == CheckStatus::Pass);
        GroupResult {
            name: "10_point_sovereignty_test",
            pass,
            per_point: results.iter().map(|r| PointResult {
                name: r.name.clone(),
                status: r.status,
                message: r.message.clone(),
            }).collect(),
        }
    }
}
```

### Tests

- Unit: every new check has a unit test.
- Unit: `TenPointTestGroup::evaluate` returns `Pass` iff all 10 sub-checks pass; a single `Fail` flips the group to `Fail`.
- Integration: the secure fixture passes all 100+ paranoid checks.
- Integration: the broken fixture fails ≥ 15 checks across ≥ 5 categories.
- Integration: `sovereign sovereignty-test` is the alias and exits 0 on the secure fixture.
- BSI C5: a fixture with an `nftables` rule that allows `0.0.0.0/0` egress to `amazonaws.com` fails `eucs_substantial_geo_restriction`.
- CI gate: every release PR runs paranoid; a Fail fails the build.

### Acceptance criteria

- [ ] `sovereign doctor --level paranoid` runs 100+ checks in < 120s on a single box, < 180s on a 3-server fleet.
- [ ] `sovereign sovereignty-test` is the auditor's command and exits 0 on the secure fixture.
- [ ] The 10-point sovereignty test is a single check group with a clear per-point breakdown.
- [ ] BSI C5:2026 and EUCS Substantial checks are present and pass on the secure fixture.

### Definition of done

The V2 release announcement includes a 60-second screen recording: an EU public-sector auditor SSHes into a Hetzner box, runs `sovereign sovereignty-test`, and the output is "All 10 points pass." The recording is the centerpiece of the EUCS Substantial pitch deck. ≥ 1 EU public-sector reference customer is signed within 90 days of the V2 release.

---

## I17. `sovereign compliance scan` — auditor-friendly, multi-framework, evidence bundle

**Goal:** A new subcommand `sovereign compliance scan` that runs the paranoid doctor and produces an **evidence bundle** — a tarball containing the doctor output, the SBOM, the signatures, the audit log hash chain, the license compliance report, the corporate documents (GmbH registration, transfer plan), the build provenance, and a "compliance report" markdown per framework (BSI C5:2026, EUCS Substantial, ISO 27001, SOC2 type 1, NIS2). The bundle is what an auditor takes home.

**Why it matters:** Sovereignty is a *structural* claim, not a feature list (invariant #3). The way you prove a structural claim is with an evidence bundle that an auditor can independently verify. Without the bundle, the operator has to manually collect 12 different artifacts and write a 30-page compliance report. With the bundle, `sovereign compliance scan --framework bsi-c5 --out /tmp/bundle.tar.gz` is the answer. This is the single most important feature for EU public-sector sales.

### Sub-tasks

1. **Implement the `sovereign compliance scan` CLI**:
   - Flags: `--framework bsi-c5|eucs-substantial|iso-27001|soc2|nis2|all`, `--out <path>` (default `/var/log/sovereign/compliance-<date>.tar.gz`), `--include-audit-log`, `--include-doctor-history`, `--redact-secrets` (default `true`).
   - Runs `sovereign doctor --level paranoid` and captures the full report (all 100+ checks, with the KB links and fix suggestions).
   - Collects the SBOM, the cosign signature, the build provenance, the license compliance report, the corporate documents (a directory configurable in `sovereign.toml`).
   - Runs the `sovereign sovereignty-test` and includes the result.
   - Runs `sovereign bench` and includes the JSON+MD artifacts.
   - Includes a `REPORT.md` that maps the doctor output to the framework's controls (e.g., for BSI C5:2026, the 114 controls are mapped to the relevant doctor checks; each control has a Pass/Fail/N-A and a link to the doctor check).
   - Tarball everything, with a `MANIFEST.yaml` listing every file and its SHA-256.
2. **Implement the framework mappings**:
   - **BSI C5:2026** — 114 controls. Each control is mapped to ≥ 1 doctor check or to a "manual verification required" entry. The mapping is maintained in `docs/compliance/bsi-c5-2026-mapping.md` and machine-readable at `docs/compliance/bsi-c5-2026-mapping.yaml`.
   - **EUCS Substantial** — the EUCS Substantial checklist (from I12). Same mapping structure.
   - **ISO 27001:2022** — 93 controls (Annex A). Same.
   - **SOC2 type 1** — the 5 Trust Services Criteria. Same.
   - **NIS2** — the 10 NIS2 security measures. Same.
3. **Implement the redaction** — the bundle never contains plaintext secrets. The `sovereign.toml` `[compliance] redact_secrets = true` flag (default true) replaces any secret value in the audit log, the doctor output, and the deploy history with `<redacted>`.
4. **Implement `sovereign compliance verify <bundle.tar.gz>`** — re-opens the bundle, re-verifies every signature, re-hashes every file, and compares to the `MANIFEST.yaml`. Exits 0 iff the bundle is intact.
5. **Wire compliance scan into the V2 release** — the release artifact is a `sovereign-compliance-bundle.tar.gz` that the customer can verify with `sovereign compliance verify`.
6. **Add a CI test**: the secure fixture is scanned; the bundle is generated; the bundle is verified; the REPORT.md contains the expected mappings.

### Code stub

```rust
// crates/sovereign/src/cmd/compliance.rs
use clap::{Args, ValueEnum};
#[derive(Copy, Clone, ValueEnum, PartialEq, Eq)]
pub enum Framework { BsiC5, EucsSubstantial, Iso27001, Soc2, Nis2, All }
#[derive(Args, Debug)]
pub struct ComplianceCmd {
    #[command(subcommand)]
    action: ComplianceAction,
}
#[derive(Subcommand, Debug)]
pub enum ComplianceAction {
    Scan { #[arg(long, value_enum, default_value_t = Framework::All)] framework: Framework,
           #[arg(long, default_value_os = "/var/log/sovereign/compliance-<date>.tar.gz")] out: std::path::PathBuf,
           #[arg(long, default_value_t = true)] redact_secrets: bool },
    Verify { bundle: std::path::PathBuf },
}
pub async fn run(cmd: ComplianceCmd, ctx: AppContext) -> anyhow::Result<()> {
    match cmd.action {
        ComplianceAction::Scan { framework, out, redact_secrets } => {
            let doctor = ctx.doctor.run(DoctorLevel::Paranoid, &ctx).await?;
            let sbom = std::fs::read("/usr/local/share/sovereign/sbom.spdx.json")?;
            let signature = std::fs::read("/usr/local/share/sovereign/sbom.sig")?;
            let provenance = std::fs::read("/usr/local/share/sovereign/provenance.json")?;
            let audit = ctx.state.dump_audit_log().await?;
            let bench = ctx.bench.run_all().await?;
            let report = render_report(framework, &doctor)?;
            let bundle = build_bundle(BundleInputs { doctor, sbom, signature, provenance, audit, bench, report, redact_secrets })?;
            std::fs::write(&out, bundle)?;
            println!("Compliance bundle written to {}", out.display());
            Ok(())
        }
        ComplianceAction::Verify { bundle } => {
            let v = verify_bundle(&bundle)?;
            if v.ok { println!("✓ Bundle {} is intact ({} files, {} bytes)", bundle.display(), v.file_count, v.total_bytes); }
            else { eprintln!("✗ Bundle {} is corrupted: {}", bundle.display(), v.reason); std::process::exit(2); }
            Ok(())
        }
    }
}
```

### Tests

- Unit: `build_bundle` includes the expected files; `verify_bundle` re-hashes and matches.
- Unit: `redact_secrets` replaces every occurrence of a known secret in the audit log with `<redacted>`.
- Unit: the framework mapping is valid (every doctor check referenced exists; every control is mapped).
- Integration: the secure fixture is scanned; the bundle contains all expected files; the bundle verifies cleanly.
- Integration: a bundle with a tampered file fails `compliance verify` with the expected reason.
- CI gate: every release PR runs `compliance scan --framework bsi-c5` and the bundle is uploaded as a release artifact.

### Acceptance criteria

- [ ] `sovereign compliance scan --framework <f> --out <path>` produces a tarball with the expected contents.
- [ ] `sovereign compliance verify <bundle>` re-verifies and exits 0 on an intact bundle, 2 on a tampered one.
- [ ] The REPORT.md maps every framework control to a doctor check or a manual verification entry.
- [ ] Secrets are redacted by default.

### Definition of done

An EU public-sector auditor receives a `sovereign-compliance-bundle.tar.gz` from a design partner, runs `sovereign compliance verify`, sees "intact," opens the REPORT.md, and finds every BSI C5:2026 control mapped to a Pass or a documented manual verification. The auditor's first reaction is "this is the first PaaS that has handed me an evidence bundle I can verify on the box itself."

---

## I18. `sovereign canary` — auto-canary with ML scorer and the doctor's full level

**Goal:** The V2 ML scorer (I5/I6) and the doctor paranoid level (I16) are integrated into a `sovereign canary` workflow: a new deploy is rolled out 5% → 25% → 100% (or 5% → 0% rolled back) based on real-time signals from the doctor and the scorer. The canary is fully CLI-driven, fully auditable, and ships with a "what would have happened" mode for the postmortem.

**Why it matters:** The auto-rollback (F8b) is reactive: 3 healthcheck fails, rollback. The canary is proactive: 5% of traffic for 5 minutes, if the doctor's full level stays Pass and the scorer's risk score stays < 0.3, promote to 25%; if not, roll back. This is the V2 answer to "we ship to production 10× a day and we want the system to decide, not us." It's also the first feature that uses the doctor as a *gating signal* for production traffic, not just a diagnostic.

### Sub-tasks

1. **Implement the canary state machine** in `sovereign-core/src/canary.rs`:
   - States: `Candidate → Canary5% → Canary25% → Promoted | RolledBack`.
   - Transitions: each transition is gated by a canary policy (Rego, from I4's policy engine).
   - The default policy: `if doctor_paranoid == Pass and risk_score < 0.3: promote; else: rollback`.
2. **Implement `sovereign canary deploy <app> --image <ref>`** CLI:
   - Starts the canary at 5%; routes 5% of traffic to the new version via Caddy's `split` directive.
   - Runs `sovereign doctor --level paranoid` every 30s for 5 min.
   - If doctor stays `Pass` and the scorer's risk score stays < 0.3 for 5 min, promote to 25%.
   - Repeats the 5-min observation at 25%; then promotes to 100% (the old version is removed).
   - If any check fails or the risk score exceeds 0.3 at any point, rolls back to 100% on the old version.
   - Writes an audit event at every state transition: `kind: "canary.transition"`, `payload: {app, from, to, doctor_summary, risk_score}`.
3. **Implement the "what would have happened" mode** — `sovereign canary what-if <app> --image <ref>` runs the canary in shadow mode (no traffic split, just doctor + scorer observation) and reports what the canary *would* have done. This is the postmortem tool.
4. **Implement `sovereign canary history`** — lists the last 50 canary runs with the per-step doctor snapshots, the risk scores, and the final state.
5. **Wire the canary into the deploy command** — `sovereign deploy` becomes `sovereign canary deploy` behind a feature flag (`[canary] enabled = true` in `sovereign.toml`). When the flag is off, the V0 blue/green deploy is used.
6. **Wire the canary into the TUI** — the Apps view's detail tab has a `Canary` tab that shows the live state machine.
7. **Wire the canary into the doctor's canary-specific checks** — 3 new paranoid-level checks: `canary_policy_present`, `canary_history_no_failures`, `canary_scorer_calibrated` (the scorer's predictions vs. actual rollback rate over the last 30 canaries; calibration error < 10%).
8. **Add Prometheus metrics**:
   - `sovereign_canary_runs_total{app,final_state}` — counter.
   - `sovereign_canary_step_duration_seconds{app,step}` — histogram.
   - `sovereign_canary_risk_score{app,step}` — gauge.
   - `sovereign_canary_doctor_passes_total{app,step}` — counter.
9. **Add a CI test**: a fixture with a "good" canary and a "bad" canary; the good one promotes to 100%, the bad one rolls back at 5%.

### Code stub

```rust
// crates/sovereign-core/src/canary.rs
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanaryState { Candidate, Canary5, Canary25, Promoted, RolledBack }
pub struct Canary {
    pub app: AppId,
    pub old_image: String,
    pub new_image: String,
    pub state: CanaryState,
    pub policy: CanaryPolicy,
    pub doctor_snapshots: Vec<DoctorSnapshot>,
    pub risk_scores: Vec<f64>,
}
impl Canary {
    pub async fn step(&mut self, ctx: &AppContext) -> Result<CanaryTransition, CanaryError> {
        let doctor = ctx.doctor.run(DoctorLevel::Paranoid, ctx).await?;
        let risk = ctx.scorer.score(self.app, &self.new_image, &doctor).await?;
        self.doctor_snapshots.push(doctor.snapshot());
        self.risk_scores.push(risk);
        let decision = self.policy.evaluate(CanarySignals { doctor: &doctor, risk });
        let next = decision.next_state(self.state);
        let from = self.state;
        self.state = next;
        ctx.audit.record("canary.transition", &json!({
            "app": self.app, "from": from, "to": next,
            "doctor_summary": doctor.summary(), "risk_score": risk,
        })).await?;
        Ok(CanaryTransition { from, to: next, doctor, risk })
    }
}
```

### Tests

- Unit: the state machine transitions correctly under all doctor/risk inputs.
- Unit: the default policy promotes at Pass + risk < 0.3, rolls back otherwise.
- Integration: a fixture with a "good" canary runs through 5% → 25% → 100% in ~10 min.
- Integration: a "bad" canary (a broken image) rolls back at 5% within 5 min.
- Integration: `sovereign canary what-if` reports the would-be decision without changing traffic.
- TUI: the Canary tab shows the live state machine.
- CI gate: every release PR runs the canary e2e test.

### Acceptance criteria

- [ ] `sovereign canary deploy <app> --image <ref>` runs the 5% → 25% → 100% state machine.
- [ ] The doctor paranoid level is the gating signal.
- [ ] The risk scorer is the second gating signal.
- [ ] `sovereign canary what-if` runs the canary in shadow mode.
- [ ] Every transition is audited.

### Definition of done

A 4-week soak: 3 design partners enable canary in production. ≥ 90% of canaries that pass the doctor + scorer promote to 100%. ≥ 1 canary per partner per month is rolled back by the canary, not by a human. The `canary_history` shows a clear, auditable decision trail for every deploy.

---

## I19. `sovereign auto-tune` — ML-driven, doctor-gated, always-reversible config tuning

**Goal:** The V2 ML scorer (I5/I6) analyzes the last 30 days of doctor output, bench output, cost output, and audit log, and proposes config tweaks (resource limits, backup retention, cgroup settings, log retention, log redaction rules, Caddy config, kernel parameters). The operator reviews the proposals and accepts/rejects each. Every accepted change is auditable, reversible, and gated on the doctor staying Pass after the change.

**Why it matters:** The "one operator can run it for 5 years" invariant (invariant #1) is a *workload* on that operator. Auto-tune is the only V2 feature that *reduces* that workload, by handling the "you should bump this resource limit" and "your backup retention is too aggressive for your workload" tweaks that would otherwise require an SRE to notice. The gate on the doctor means every accepted change is safe — the system doesn't apply a tweak that breaks any check.

### Sub-tasks

1. **Implement the `AutoTuneAdvisor`** in `sovereign-doctor/src/autotune/`:
   - Consumes the last 30 days of: doctor output, bench output, cost output, audit log, deploy history.
   - Runs a small set of heuristic rules (the V2 set; the V3 set adds ML):
     - **Rule 1**: `if doctor.cpu_steal_low == Warn 3+ times/week: propose cpu_limit increase`.
     - **Rule 2**: `if cost.egress_spike == Warn 2+ times/week: propose caddy_cache_ttl increase`.
     - **Rule 3**: `if doctor.backup_retention_3_2_1_1_0 == Fail: propose backup retention increase`.
     - **Rule 4**: `if bench.p99_deploy_latency > 5*p50 for 3 days: propose control plane resource increase`.
     - **Rule 5**: `if audit.bytes/day > 1GB: propose audit sampling (redact-only the 0.1% of events that are noisy)`.
     - ... (10+ more rules)
   - Each rule produces a `Proposal { id, title, description, current_value, proposed_value, expected_impact, risk_level, reversible }`.
2. **Implement `sovereign auto-tune propose`** CLI:
   - Runs the advisor; lists all proposals.
   - Default: dry-run (just prints).
   - `--apply` flag: applies the non-destructive proposals (e.g., the caddy cache TTL change), runs the doctor paranoid level, and if it stays Pass, commits the change. If doctor fails, rolls back the change.
   - `--accept <id>` / `--reject <id>` for per-proposal control.
3. **Implement the rollback path** — every accepted change is recorded in the `auto_tune_history` table with the diff. `sovereign auto-tune rollback <id>` reverts the change and runs the doctor.
4. **Wire auto-tune into the morning report (G11)** — a 1-line "Auto-tune: 3 proposals pending review" line.
5. **Wire auto-tune into the doctor's canary-specific checks** — 2 new paranoid-level checks: `auto_tune_proposals_reviewed` (no proposal is > 7 days old without a decision), `auto_tune_history_healthy` (the last 10 applied changes did not cause a doctor fail).
6. **Add Prometheus metrics**:
   - `sovereign_auto_tune_proposals_total{rule,status}` — counter.
   - `sovereign_auto_tune_last_applied_timestamp` — gauge.
   - `sovereign_auto_tune_rollback_total{rule}` — counter.
7. **Add a CI test**: a fixture with the doctor `cpu_steal_low` Warn pattern; `auto-tune propose` returns the expected proposal; `auto-tune --apply` applies it; the doctor stays Pass; the rollback path reverts cleanly.

### Code stub

```rust
// crates/sovereign-doctor/src/autotune/advisor.rs
pub trait TuneRule: Send + Sync {
    fn id(&self) -> &'static str;
    fn evaluate(&self, ctx: &TuneContext) -> Option<Proposal>;
    fn apply(&self, proposal: &Proposal, ctx: &TuneContext) -> Result<(), TuneError>;
    fn rollback(&self, proposal: &Proposal, ctx: &TuneContext) -> Result<(), TuneError>;
}
pub struct CpuStealRule;
impl TuneRule for CpuStealRule {
    fn id(&self) -> &'static str { "cpu_steal_low" }
    fn evaluate(&self, ctx: &TuneContext) -> Option<Proposal> {
        let warn_count = ctx.doctor_history.last_30_days()
            .filter(|r| r.check_name == "cpu_steal_low" && r.status == CheckStatus::Warn)
            .count();
        if warn_count >= 3 {
            Some(Proposal {
                id: format!("cpu_steal_low-{}", Utc::now().timestamp()),
                title: "Increase CPU limit by 25%",
                description: format!("`cpu_steal_low` has been Warn {} times in the last 7 days. A 25% CPU limit increase is likely to resolve the warning without oversubscribing the host.", warn_count),
                current_value: ctx.config.resource.cpu_limit.clone(),
                proposed_value: (ctx.config.resource.cpu_limit.parse::<u32>()? * 5 / 4).to_string(),
                expected_impact: "Reduces cpu_steal_low warns by an estimated 80%",
                risk_level: RiskLevel::Low,
                reversible: true,
            })
        } else { None }
    }
    fn apply(&self, p: &Proposal, ctx: &TuneContext) -> Result<(), TuneError> { /* update cgroup */ }
    fn rollback(&self, p: &Proposal, ctx: &TuneContext) -> Result<(), TuneError> { /* restore cgroup */ }
}
pub struct AutoTuneAdvisor { rules: Vec<Box<dyn TuneRule>> }
impl AutoTuneAdvisor {
    pub fn propose(&self, ctx: &TuneContext) -> Vec<Proposal> {
        self.rules.iter().filter_map(|r| r.evaluate(ctx)).collect()
    }
}
```

### Tests

- Unit: each rule's `evaluate` returns the expected proposal on a synthetic context.
- Unit: each rule's `apply` and `rollback` are correct and reversible.
- Integration: the fixture with `cpu_steal_low` Warn pattern produces the expected proposal.
- Integration: `auto-tune --apply` applies a non-destructive proposal; doctor stays Pass; rollback reverts.
- Integration: a destructive proposal (a change that would fail doctor) is rolled back automatically.
- CI gate: a job runs `auto-tune propose` on the secure fixture and asserts the proposal list is stable (no flapping rules).

### Acceptance criteria

- [ ] `sovereign auto-tune propose` returns the list of pending proposals.
- [ ] `sovereign auto-tune --apply` applies non-destructive proposals with the doctor gate.
- [ ] `sovereign auto-tune rollback <id>` reverts a change.
- [ ] Every applied change is auditable and reversible.

### Definition of done

A 4-week soak: 3 design partners run `sovereign auto-tune propose` weekly. The advisor surfaces ≥ 1 actionable proposal per partner per month. ≥ 80% of accepted proposals do not cause a doctor regression. The auto-tune history shows a clear, auditable decision trail for every change.

---

## 1. Definition of Done (the phase gate)

Phase 3 closes when **all** of these are true:

- [ ] All 19 features (I1-I19) are merged to `main` with passing CI.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo llvm-cov --workspace` reports ≥ 80% line coverage.
- [ ] rqlite HA is in production for ≥ 1 customer with 0 unscheduled downtimes.
- [ ] The OPA/Rego policy engine is in production for ≥ 1 customer.
- [ ] The ML scorer is in shadow mode (no enforcement) for ≥ 1 customer.
- [ ] `sovereign doctor --level paranoid` returns 0 fails on the secure fixture; the 10-point sovereignty test passes.
- [ ] `sovereign sovereignty-test` is a documented subcommand and exits 0 on the secure fixture.
- [ ] `sovereign compliance scan --framework bsi-c5` produces a verifiable bundle.
- [ ] `sovereign canary` is enabled in production for ≥ 1 customer; ≥ 90% of canaries that pass doctor + scorer promote to 100%.
- [ ] `sovereign auto-tune propose` returns ≥ 1 actionable proposal per design partner per month; ≥ 80% of accepted proposals do not cause a doctor regression.
- [ ] The 10-point sovereignty test is a CI gate (release fails if any point fails).
- [ ] BSI C5:2026 mapping is published.
- [ ] EUCS Substantial mapping is published.
- [ ] SBOM, cosign signing, reproducible builds are CI-gated.
- [ ] Foundation transfer plan is signed and published.
- [ ] Pro tier is live with ≥ 10 paying customers.
- [ ] First EU public-sector reference customer is signed.
- [ ] The Apache 2.0 LICENSE is unchanged.
- [ ] The version is bumped to `2.0.0`.

**If any of these is false, the phase is not done.**

---

## 2. What we explicitly do not build in Phase 3

- True canary (5/25/100% traffic split) — V2.5+
- Multi-region failover — V3+
- Service mesh — never
- K8s — never
- Multi-cloud abstraction — never
- gRPC / WebSockets — never
- Mobile app — never
- Desktop app — never
- 5+ tier RBAC — V3+
- SOC2 reporting — V3+ (only if customer demand)
- SecNumCloud — only if customer demand
- Doctor auto-rebalance (auto-move apps between agents based on doctor signals) — V3+
- Doctor multi-cluster (cross-region doctor) — V3+
- Doctor ML auto-tune rules (replace heuristics with ML) — V3+

---

**Next: read [`doctor.md`](./doctor.md) for the full doctor spec (5 levels, 14 categories, 4 subcommands, incident toolkit), then [`implementation-guide.md`](./implementation-guide.md) for the dev setup, build, test, and release process.**
