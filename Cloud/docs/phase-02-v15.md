# Phase 2 — The Team (V1.5)

**Timeline:** Months 5-9 (5 months)
**Goal:** Multi-server via the agent pattern (no rqlite yet), team access (RBAC), audit log, multi-environment promotion, preview environments per PR, Nginx low-mem, Podman runtime, declarative GitOps mode, service catalog. Doctor promoted to **full** level (70+ checks) with **fleet** and **remote** mode, plus the incident toolkit (`sovereign incident declare/note/resolve`, `sovereign retro`, `sovereign game-day`), the four first-class ops features (`sovereign import --from`, `sovereign dns`, `sovereign db pool`, `sovereign status page`).
**Audience:** The lead engineer + 3-4 contributors. The first 50 design partners. The first "team" customer (3+ engineers).
**Definition of Done:** 1 customer with a 3-server fleet, 0 unscheduled downtimes, ≥ 5 team customers, audit log queryable for 90 days, EU incorporation filed, `sovereign doctor --level full` returns 0 fails on the fleet, `sovereign doctor fleet` runs across all agents, `sovereign import --from heroku` migrates a real app in < 30 min.

---

## 0. The month-by-month plan

```text
Month 5:  H1 (agent pattern) + H2 (mTLS) + H3 (RBAC) + H4 (audit log query)
Month 6:  H5 (multi-env promotion) + H6 (preview environments per PR) + H7 (Nginx low-mem) + H16 (doctor --level full)
Month 7:  H8 (Podman runtime) + H9 (declarative apply) + H10 (service catalog) + H17 (doctor fleet + remote mode) + H20 (sovereign import --from)
Month 8:  H11 (Backstage YAML export) + H12 (secret rotation) + H13 (EU incorporation) + H18 (sovereign incident toolkit) + H21 (sovereign dns) + H22 (sovereign db pool)
Month 9:   H14 (V1.5 hardening) + H15 (V1.5 release + blog post) + H19 (sovereign retro + game-day) + H23 (sovereign status page)
```

---

## H1. Agent pattern (server + N agents over mTLS)

**Goal:** The control plane server (running SQLite) can push deploy instructions to N agent processes running on remote hosts. Agents are stateless. All state is on the server.

**Why it matters:** This is the **staging ground for V2 (rqlite HA)**. If the agent pattern is right, V2 is just "add a second server to the cluster." If it's wrong, V2 is a rewrite.

### Sub-tasks

1. **Define the `AgentPort` trait** in `sovereign-core/src/ports/agent.rs`:
   ```rust
   #[async_trait]
   pub trait AgentPort: Send + Sync {
       async fn register(&self, server_url: &str, token: &str) -> Result<AgentId, AgentError>;
       async fn deploy(&self, spec: &DeploySpec) -> Result<DeploymentResult, AgentError>;
       async fn stop(&self, container_id: &str) -> Result<(), AgentError>;
       async fn logs(&self, container_id: &str, opts: LogOpts) -> Result<LogStream, AgentError>;
       async fn healthcheck(&self, container_id: &str, opts: HealthOpts) -> Result<HealthResult, AgentError>;
       async fn heartbeat(&self) -> Result<Heartbeat, AgentError>;
   }
   ```
2. **Implement the server-side `AgentServer`** that listens for agent connections over mTLS.
3. **Implement the `sovereign agent` subcommand** in the binary crate:
   - Reads `/etc/sovereign/agent.toml` (server URL, token, CA cert path).
   - Registers with the server, sends a heartbeat every 10s.
   - Listens for deploy instructions and dispatches to the local runtime adapter.
4. **Implement `sovereign server add <host>`** that SSHs to the host, installs the binary, writes the agent config, starts the agent.
5. **Update the `server` table** to track `last_heartbeat` and surface "unreachable" status.

### Code stub

```rust
// crates/sovereign-core/src/ports/agent.rs
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploySpec {
    pub app_id: AppId,
    pub image_ref: String,
    pub strategy: Strategy,
    pub resources: Resources,
    pub health: HealthOpts,
    pub env: HashMap<String, String>,  // secrets inlined, never on disk
    pub volumes: Vec<VolumeMount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentResult {
    pub container_id: String,
    pub started_at: DateTime<Utc>,
}

#[async_trait]
pub trait AgentPort: Send + Sync {
    async fn register(&self, server_url: &str, token: &str) -> Result<AgentId, AgentError>;
    async fn deploy(&self, spec: &DeploySpec) -> Result<DeploymentResult, AgentError>;
    async fn stop(&self, container_id: &str) -> Result<(), AgentError>;
    async fn logs(&self, container_id: &str, opts: LogOpts) -> Result<LogStream, AgentError>;
    async fn healthcheck(&self, container_id: &str, opts: HealthOpts) -> Result<HealthResult, AgentError>;
    async fn heartbeat(&self) -> Result<Heartbeat, AgentError>;
}
```

```rust
// crates/sovereign/src/agent.rs (runs in the binary, on the agent host)
pub async fn run(config: AgentConfig) -> anyhow::Result<()> {
    let client = SovereignClient::connect(&config.server_url, &config.token).await?;
    let agent_id = client.register(&config.server_url, &config.token).await?;
    
    // Heartbeat loop
    let heartbeat_client = client.clone();
    tokio::spawn(async move {
        loop {
            if let Err(e) = heartbeat_client.heartbeat().await {
                tracing::warn!("heartbeat failed: {}", e);
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
    
    // Listen for deploy instructions
    let mut deploy_stream = client.subscribe_deploys().await?;
    while let Some(spec) = deploy_stream.next().await {
        let result = run_deploy_locally(spec).await;
        client.report_deploy_result(agent_id, result).await?;
    }
    Ok(())
}
```

### Tests

- Unit test: `AgentPort` is mockable with `mockall`.
- Integration test (2 servers + 1 agent): deploy to the agent's host, verify the URL is reachable, verify the audit log records the deploy with `target_server: agent-1`.
- Integration test: agent goes offline, server marks it `unreachable` after 30s.
- Integration test: agent comes back online, server marks it `healthy`.

### Acceptance criteria

- [ ] `sovereign server add <host>` installs the agent on a remote host.
- [ ] Deploys to the remote host succeed via the agent.
- [ ] Heartbeats are sent every 10s and visible in `sovereign server list`.
- [ ] An unreachable agent is marked `unreachable` within 30s.

### Definition of done

A 2-host test fleet (server + 1 agent) deploys an app, serves traffic, and survives the agent host going offline for 60s — passes in CI on every PR.

---

## H2. mTLS for agent-server communication

**Goal:** All agent-server communication is encrypted with mutual TLS. Agents authenticate with a client certificate; the server authenticates with a CA-signed cert.

**Why it matters:** The control plane is the crown jewel. An unencrypted channel means anyone on the network can deploy, rollback, or read secrets.

### Sub-tasks

1. **Generate a private CA** at install time (`sovereign init-ca`):
   - Stores the CA cert and key at `/var/lib/sovereign/ca/`.
2. **Issue per-agent client certs** on `sovereign server add <host>`:
   - The cert's CN is the agent's hostname.
   - The cert is signed by the sovereign CA.
3. **Issue per-server certs** at install time.
4. **Configure the agent to verify the server's cert** against the CA.
5. **Configure the server to verify the agent's cert** against the CA + check the CN.
6. **Document cert rotation** (V2.5: automated; V1.5: manual via `sovereign cert rotate`).

### Acceptance criteria

- [ ] All agent-server traffic is TLS-encrypted.
- [ ] A forged cert (not signed by the sovereign CA) is rejected.
- [ ] Cert rotation does not require server restart.

---

## H3. RBAC (4 roles: owner, admin, developer, readonly)

**Goal:** Users have one of 4 global roles. RBAC is enforced on every API endpoint and CLI subcommand.

**Why it matters:** Team customers (Karim's agency, Lin's startup) need per-user access control. Without it, the platform is a single-tenant tool, not a team platform.

### Sub-tasks

1. **Add the `user` table** (already in V0; expose the CRUD).
2. **Implement `sovereign user add <email> --role <role>`** CLI.
3. **Implement `sovereign user list` / `show` / `update` / `remove`**.
4. **Add the RBAC middleware** in `sovereign/src/http/middleware.rs`:
   - For every request, resolve the actor (user, system, agent).
   - Check the role against the endpoint's required role.
   - Return 403 with RFC 9457 problem+json on deny.
5. **Update the CLI** to pass the actor's token and surface 403 errors gracefully.
6. **Document the RBAC matrix** in [`architecture.md` §7.3](#) and the docs site.

### Code stub

```rust
// crates/sovereign/src/http/middleware.rs
use axum::{middleware::Next, response::Response, http::Request};
use sovereign_core::domain::UserRole;

pub async fn rbac_middleware<B>(
    axum::extract::State(state): axum::extract::State<AppState>,
    req: Request<B>,
    next: Next<B>,
) -> Result<Response, AppError> {
    let actor = state.resolve_actor(&req).await?;
    let required = required_role_for(&req);
    if !actor.role.at_least(required) {
        return Err(AppError::Auth(format!("role {} required, actor is {}", required, actor.role)));
    }
    // Attach actor to request extensions
    let mut req = req;
    req.extensions_mut().insert(actor);
    Ok(next.run(req).await)
}

fn required_role_for<B>(req: &Request<B>) -> UserRole {
    let path = req.uri().path();
    let method = req.method();
    match (method.as_str(), path) {
        ("GET", _) => UserRole::Readonly,        // reads
        ("POST", p) if p.starts_with("/v1/apps") => UserRole::Developer,
        ("DELETE", _) => UserRole::Admin,
        ("PUT", p) if p.contains("/policy/") => UserRole::Admin,
        _ => UserRole::Developer,                 // default
    }
}
```

### Acceptance criteria

- [ ] A `readonly` user can list apps but cannot deploy.
- [ ] A `developer` user can deploy but cannot delete apps.
- [ ] An `admin` user can manage users.
- [ ] An `owner` user can change the company name and the license key.

---

## H4. Audit log query + export

**Goal:** The audit log is queryable via `sovereign audit --since 7d --actor user:alice --kind deploy`. Exportable as CSV or PDF for compliance.

**Why it matters:** Trust. A user who can see "who did what, when, and why" can hand the platform to their security officer. A user who can't is the security officer's problem.

### Sub-tasks

1. **Implement `sovereign audit` CLI** with filters: `--since`, `--until`, `--actor`, `--kind`, `--target`, `--limit`.
2. **Implement `sovereign audit export --format csv|pdf`** for compliance reports.
3. **Add the audit log view to the TUI** (G1 in V1 ships the placeholder; V1.5 adds filtering and search).
4. **Document the audit log schema** in the docs site.

### Acceptance criteria

- [ ] `sovereign audit --since 7d` returns all events from the last 7 days.
- [ ] Filters compose: `sovereign audit --since 7d --actor user:alice --kind deploy`.
- [ ] CSV export opens in Excel/LibreOffice without errors.
- [ ] PDF export is a single-page summary with the same data.

---

## H5. Multi-environment promotion

**Goal:** `sovereign promote <app> <from-env> <to-env>` deploys the current `<from-env>` image to `<to-env>`. Promotion is recorded in the audit log.

### Sub-tasks

1. **Implement the `promote` use case** in `sovereign-core/src/use_cases/promote.rs`:
   - Find the current `Healthy` deployment in `<from-env>`.
   - Trigger a deploy in `<to-env>` with the same image.
   - Record the promotion in the audit log with `kind: "promote"`, `payload: {from_env, to_env, source_deployment_id}`.
2. **Wire the `promote` CLI** subcommand.
3. **Document the promotion flow** in the docs site.

### Acceptance criteria

- [ ] `sovereign promote api staging prod` deploys the staging image to prod.
- [ ] The promotion is in the audit log.
- [ ] Promotion respects env-scoped secrets and env-scoped resources.

---

## H6. Preview environments per PR (with TTL)

**Goal:** A webhook from GitHub/Gitlab on PR open creates a preview environment at `pr-<N>.<app>.<domain>`. The environment is destroyed (with a 24h TTL by default) when the PR is closed or merged.

**Why it matters:** Preview environments are the #1 collaboration feature for teams. Vercel, Netlify, Render all have them. Without them, the platform is solo-dev only.

### Sub-tasks

1. **Implement the webhook handler** at `POST /v1/webhooks/<provider>`:
   - Verifies the webhook signature (HMAC-SHA256 with a shared secret).
   - Parses the event (PR opened, closed, merged, synchronized).
   - For "opened" or "synchronized": create a preview environment.
   - For "closed" or "merged": destroy the preview environment.
2. **Implement the preview environment** as a special `App` with `kind = "preview"`, `parent_app_id = <original app>`, `ttl = 24h`.
3. **Implement a tokio task** that destroys preview environments past their TTL.
4. **Wire `sovereign preview --pr <N>`** CLI for manual preview creation.
5. **Add a preview URL** to the PR comment (via the GitHub API).

### Acceptance criteria

- [ ] Opening a PR creates a preview environment at `pr-<N>.<app>.<domain>`.
- [ ] The preview is destroyed 24h after the PR is closed.
- [ ] The preview is destroyed immediately on PR merge.
- [ ] The webhook signature is verified.

---

## H7. Nginx low-mem mode

**Goal:** An Nginx adapter is available as an alternative to Caddy for memory-constrained boxes (e.g., Hetzner CX22 with 2 GB RAM, where Caddy's 80 MB baseline is a problem).

**Why it matters:** Memory-constrained boxes are a real use case (vibe coders, edge devices). Caddy is great but not optimal for them.

### Sub-tasks

1. **Define the `ProxyPort` trait** in `sovereign-core/src/ports/proxy.rs` (already in V0; refine).
2. **Implement `NginxProxy`** in `sovereign-proxy-nginx/`:
   - `connect()` starts Nginx with a generated config.
   - `add_route` / `remove_route` regenerate the config and `nginx -s reload`.
   - `url_for` returns the same URL as the Caddy adapter.
3. **Use `nginx` package** from Debian/Alpine repos (V1.5: 1 MB static binary from a custom build).
4. **Make the proxy adapter configurable** in `sovereign.toml`:
   ```toml
   [proxy]
   adapter = "caddy"  # or "nginx"
   ```
5. **Benchmark memory** of both adapters and document the trade-off.

### Acceptance criteria

- [ ] Switching from Caddy to Nginx in `sovereign.toml` works without code changes.
- [ ] Nginx uses < 10 MB RAM in steady state.
- [ ] All routes work identically to the Caddy adapter.

---

## H8. Podman runtime support

**Goal:** Podman is a drop-in alternative to Docker, with rootless mode. It's the default on RHEL/Fedora and the de-facto standard for sovereign deployments.

**Why it matters:** Some users (especially EU public sector) require rootless containers. Podman is the answer; Docker is not.

### Sub-tasks

1. **Implement `PodmanRuntime`** in `sovereign-runtime-podman/`:
   - Talks to the Podman socket (Unix or TCP).
   - Same trait as `DockerRuntime`.
2. **Test the same `sovereign deploy` use case** with the Podman adapter.
3. **Document the trade-off** (Docker is more compatible, Podman is more sovereign).

### Acceptance criteria

- [ ] `sovereign.toml` with `runtime.adapter = "podman"` works.
- [ ] All V1 features work with Podman.

---

## H9. Declarative `apply` (read-only drift detection)

**Goal:** `sovereign apply -f app.yaml` computes the diff between the YAML and the current state, prints the plan, and applies on `--confirm`. No auto-reconcile (V1.5); the human approves their own diffs.

**Why it matters:** GitOps without auto-reconcile. The user can see what would change, then approve. This is the pattern that prevents the "platform quietly overwrote my fix at 3am" horror story.

### Sub-tasks

1. **Implement the `apply` use case** in `sovereign-core/src/use_cases/apply.rs`:
   - Parse the YAML.
   - Compute the diff against current state.
   - Print the plan (added, modified, removed fields).
   - On `--confirm`, apply the changes in a transaction with audit.
2. **Implement `sovereign diff -f app.yaml`** to just print the diff.
3. **Add a `tool apply --watch`** (V2.5) that polls and re-applies.

### Acceptance criteria

- [ ] `sovereign diff -f app.yaml` prints the diff.
- [ ] `sovereign apply -f app.yaml --dry-run` prints the plan and exits 0.
- [ ] `sovereign apply -f app.yaml --confirm` applies the diff.

---

## H10. Service catalog (16 fields, auto-derived)

**Goal:** Every deployed app appears in a queryable service catalog with 16 fields (name, owner, env, git_repo, image_ref, domains, health_path, resources, last_deploy, deploy_count_30d, last_health_fail, backup_id, on_call, runbook, slo_target, created_by/updated_at).

**Why it matters:** The service catalog is the foundation for everything platform-engineering: golden paths, drift detection, RBAC scoping, audit scoping, cost attribution.

### Sub-tasks

1. **Add the 16 fields to the `app` table** (or to a new `service_catalog` view that joins across tables).
2. **Implement `sovereign catalog list`** with the 16 fields.
3. **Implement `sovereign catalog show <app>`** with full detail.
4. **Add a catalog view to the TUI**.
5. **Emit metrics** for `catalog.apps_total{env, owner}`.

### Acceptance criteria

- [ ] `sovereign catalog list` returns the 16 fields for all apps.
- [ ] The catalog is auto-derived (no manual entry).
- [ ] The TUI catalog view is searchable by name, owner, env.

---

## H11. Service catalog export to Backstage YAML

**Goal:** `sovereign catalog export` produces a Backstage-compatible `catalog-info.yaml` for every app. Users who already run Backstage can import the catalog.

**Why it matters:** Backstage is the dominant internal developer portal. We don't compete with it; we feed it. The user can use Backstage for "service ownership docs, on-call schedules" and we handle "deploy, rollback, secret rotation."

### Sub-tasks

1. **Implement the Backstage YAML schema mapping** (the 16 fields → Backstage's `Component` schema).
2. **Implement `sovereign catalog export --format backstage > catalog-info.yaml`**.
3. **Document the import** in the docs site.

### Acceptance criteria

- [ ] `sovereign catalog export --format backstage` produces a valid Backstage YAML.
- [ ] Importing the YAML into Backstage shows the apps.

---

## H12. Secret rotation

**Goal:** `sovereign secret rotate <APP> <KEY>` reads a new value, encrypts, updates the row, redeploys the app, and verifies the new secret is in use.

### Sub-tasks

1. **Implement the `rotate` use case** in `sovereign-core/src/use_cases/secret.rs`:
   - Read the new value from stdin.
   - Encrypt with the master key.
   - Update the `secret` row (with `version + 1`, `rotated_at = now()`).
   - Trigger a `redeploy` use case.
   - Verify the new secret is in use (e.g., by running a probe command in the container).
2. **Wire `sovereign secret rotate <APP> <KEY>`** CLI.
3. **Schedule periodic rotation** (V2: configurable; V1.5: manual only).
4. **Add a `--all-envs` flag** to rotate across dev/staging/prod.

### Acceptance criteria

- [ ] `sovereign secret rotate api DATABASE_URL` rotates the secret.
- [ ] The app is redeployed with the new secret.
- [ ] The audit log shows the rotation with `kind: "secret.rotate"`.

---

## H13. EU incorporation (Berlin GmbH + Estonian OÜ)

**Goal:** The company is incorporated in the EU. Two entities: a German GmbH (for BSI C5, EU public sector, IPCEI-CIS) and an Estonian OÜ (for e-residency, digital-first team).

**Why it matters:** "Sovereign" is a structural claim. The corporate graph decides sovereignty, not the feature list. US incorporation = not sovereign.

### Sub-tasks

1. **Engage a Berlin law firm** (e.g., Linklaters, Taylor Wessing) to file the GmbH.
2. **Engage an Estonian e-residency agency** to file the OÜ.
3. **Open a Berlin bank account** for the GmbH.
4. **Set up the OÜ as a 100% subsidiary** of the GmbH (or vice-versa, depending on tax advice).
5. **Apply for BSI C5:2026 attestation** (V2; V1.5: gap analysis only).
6. **Publish the incorporation details** on the website (jurisdiction, registration number, registered address).
7. **Update the LICENSE** file with the copyright holder being the GmbH.

### Acceptance criteria

- [ ] GmbH is registered at the Berlin Handelsregister.
- [ ] OÜ is registered at the Estonian e-Business Registry.
- [ ] The website has a "Sovereignty" page with the corporate details.
- [ ] The LICENSE copyright is updated.

---

## H14. V1.5 hardening

**Goal:** Address all known issues from V1, fix any security advisories, add missing features requested by the first 50 design partners.

### Sub-tasks

1. **Triage the issue tracker** — every "P1" bug from V1 is fixed.
2. **Run a security audit** — engage an external firm (NCC Group, Cure53) for a week-long audit.
3. **Performance pass** — every CLI subcommand is profiled; > 100ms is optimized.
4. **Documentation pass** — every "how-to" has a working example.
5. **Migration from Coolify / Dokploy** — `sovereign import --from coolify` reads a Coolify export and creates the equivalent apps.

### Acceptance criteria

- [ ] 0 P1 bugs open.
- [ ] Security audit report is published (issues are fixed or have a public timeline).
- [ ] Migration from Coolify works for the 12 most common app types.

---

## H15. V1.5 release + blog post

**Goal:** A V1.5 release announcement with a blog post, a "what's new" video, and a re-engagement email to all V1 users.

### Sub-tasks

1. **Tag the release** as `v1.5.0`.
2. **Generate the release notes** from the issue tracker (closed issues since v1.0.0).
3. **Write the blog post** (1-2 pages, with screenshots of the TUI, the catalog, the audit log).
4. **Record a 5-minute demo video** showing multi-server, RBAC, audit log, preview environments.
5. **Send the re-engagement email** to all V1 users.
6. **Submit to awesome-selfhosted**.

### Acceptance criteria

- [ ] `v1.5.0` is tagged and released.
- [ ] Blog post is published.
- [ ] Demo video is published.
- [ ] awesome-selfhosted PR is merged.

---

## H16. `sovereign doctor --level full` — 70+ checks, every category, every severity

**Goal:** Promote `sovereign doctor` from V1 **standard** (40+ checks) to V1.5 **full** (70+ checks, every category with 4-6 checks each, every check has a fix or a `doctor kb` page). The full level is the on-call's *daily* level; standard is the morning report's level; basic is the install gate.

**Why it matters:** Standard level catches 40+ failure modes. In a multi-server fleet with RBAC, declarative apply, and audit log, the failure modes shift again: certificate rotations, RBAC drift, declarative vs. actual state divergence, secret rotation overdue, multi-env promotion failures, preview environment leaks, Podman vs. Docker inconsistencies, Nginx low-mem alerts. Full level catches all of these, and the `--fleet` mode (H17) runs them across the entire fleet in parallel.

### Sub-tasks

1. **Add the 30+ new V1.5 checks** (each is a new file in `sovereign-doctor/src/checks/<category>.rs`):
   - **Backup** (+4) — `backup_corruption_check` (a 3-pass `pg_restore --list` integrity hash on a scratch DB), `backup_retention_3_2_1_1_0` (3 copies, 2 media, 1 offsite, 1 immutable, 0 errors — the 3-2-1-1-0 rule from [`operations-runbook.md`](./operations-runbook.md)), `backup_restore_drill_recent` (last successful restore drill ≤ 30 days), `backup_drift` (the on-disk DB size vs. the last backup's reported size, ratio > 2× is a Warn).
   - **Network** (+3) — `mtls_handshake_fast` (a `curl --cert --key` to the agent endpoint in < 200ms), `agent_reachable` (each registered agent responds on its mTLS port), `caddy_ocsp_stapled` (Caddy staples OCSP for the served cert; a `openssl s_client -status` returns `OCSP Response Status: successful`).
   - **Agents** (+4) — `agent_version_match` (every agent runs the same binary version as the server; the `agent_version` table), `agent_capacity` (the agent's reported `available_memory` and `cpu_idle` are sufficient for the next deploy), `agent_quarantine` (no agent is in `quarantined` state; the `agent_state` table), `agent_token_rotation_due` (any agent token > 90 days old is listed).
   - **Observability** (+3) — `slo_burn_rate` (the error budget burn rate from the Prometheus SLO data is < 1× the 28-day budget), `audit_query_latency` (the last 100 `sovereign audit` queries returned in < 200ms p95), `trace_correlation` (every recent log line has a `trace_id`; > 5% missing is a Warn).
   - **Security** (+8) — `rbac_role_drift` (every user has exactly the roles in `sovereign.toml`; the `user_role` table is the source of truth), `audit_log_hash_chain` (each audit event's `prev_hash` matches the prior event's hash; a break is a Fail — tamper detection), `secret_rotation_due` (any secret > 180 days old is listed), `binary_no_world_readable` (no `o+r` on any file in `/usr/local/share/sovereign/`), `caddy_config_no_secrets` (the Caddy config has no plaintext secrets — a `grep` of the config for known secret keys returns 0), `coredumps_disabled` (`/etc/security/limits.conf` has `* hard core 0`), `kernel_modules_locked` (the running kernel's loaded modules match the allowlist in `sovereign.toml`), `sshd_root_login_off` (`PermitRootLogin no`).
   - **Sovereignty** (+4) — `european_owned_verified` (the binary's provenance attestation, signed by the EU-resident CI key, is present and the signature is valid; this is the V1.5 stub of the V2 10-point test), `sbom_signed` (the SBOM is signed with `cosign` and the signature verifies), `no_us_cloud_dependencies_egress` (re-runs the egress allowlist check with a stricter allowlist — the V1.5 list excludes any `*.amazonaws.com`, `*.googleapis.com`, `*.azure.com`, `*.cloudfront.net`, `*.fastly.com`, `*.herokuapp.com`), `license_compliance` (every crate in `Cargo.lock` is Apache 2.0, MIT, BSD-2, BSD-3, MPL-2.0, or Zlib; the list is maintained in `docs/SOVEREIGN_LICENSES.md`).
   - **Performance** (+3) — `agent_dial_latency` (the dial latency from the server to each agent, p95 < 50ms), `tls_session_resumption` (a second `curl` reuses the session, latency < 50ms), `audit_write_amp` (the audit log's `WAL` size < 10× the schema size).
   - **Cost** (+3) — `cost_per_request` (the estimated $/request from `sovereign cost` per app; > 10× the median is a Warn), `egress_spike` (egress in the last 24h vs. the 7-day average, > 3× is a Warn), `backup_cost_drift` (backup S3 spend this month vs. last month, > 2× is a Warn).
2. **Implement the `FullExtension`** in `sovereign-doctor/src/extensions/full.rs`, following the `DoctorExtension` pattern from V1's G16. The `Doctor::for_level(DoctorLevel::Full)` constructor composes `Basic + Standard + Full`.
3. **Implement the check priority for `--watch`** — the full level's `--watch` mode only emits state changes for `Fail` checks (and Warn for the security and sovereignty categories). This is to avoid alert fatigue in a 70-check continuous run.
4. **Implement the per-check "suggested action"** — every check has a `suggested_action: &'static str` (e.g., `for backup_recent: "sovereign backup run --app postgres && sovereign doctor --explain backup_recent"`). The morning report uses this to produce a "Top 5 actions" list.
5. **Author 30+ new KB articles** in `docs/src/kb/doctor/` (the 30+ new V1.5 checks). The total KB is now 70+ articles.
6. **Add the 20+ new V1.5 fixes** (one per non-destructive check). Total V1.5 fixes: ~30 (3 from V0 + 12 from V1 + 20 from V1.5).
7. **Add a CI test**: a `sovereign doctor --level full` matrix runs against 4 fixtures (`basic-secure`, `standard-secure`, `full-secure`, `full-broken`). The `full-broken` fixture fails ≥ 10 checks; the secure ones pass 100%.

### Code stub

```rust
// crates/sovereign-doctor/src/extensions/full.rs
use crate::{DoctorExtension, DoctorLevel};
pub struct FullExtension;
impl DoctorExtension for FullExtension {
    fn name(&self) -> &'static str { "full" }
    fn level(&self) -> DoctorLevel { DoctorLevel::Full }
    fn checks(&self) -> Vec<Box<dyn Check>> {
        let mut v = StandardExtension.checks();
        v.extend_from_slice(&[
            // Backup (+4)
            Box::new(checks::backup::BackupCorruptionCheck),
            Box::new(checks::backup::Backup321100Check),
            Box::new(checks::backup::BackupRestoreDrillRecentCheck),
            Box::new(checks::backup::BackupDriftCheck),
            // Network (+3)
            Box::new(checks::network::MtlsHandshakeFastCheck),
            Box::new(checks::network::AgentReachableCheck),
            Box::new(checks::network::CaddyOcspStapledCheck),
            // Agents (+4)
            Box::new(checks::agents::AgentVersionMatchCheck),
            Box::new(checks::agents::AgentCapacityCheck),
            Box::new(checks::agents::AgentQuarantineCheck),
            Box::new(checks::agents::AgentTokenRotationDueCheck),
            // Observability (+3)
            Box::new(checks::observability::SloBurnRateCheck),
            Box::new(checks::observability::AuditQueryLatencyCheck),
            Box::new(checks::observability::TraceCorrelationCheck),
            // Security (+8)
            Box::new(checks::security::RbacRoleDriftCheck),
            Box::new(checks::security::AuditLogHashChainCheck),
            Box::new(checks::security::SecretRotationDueCheck),
            Box::new(checks::security::BinaryNoWorldReadableCheck),
            Box::new(checks::security::CaddyConfigNoSecretsCheck),
            Box::new(checks::security::CoredumpsDisabledCheck),
            Box::new(checks::security::KernelModulesLockedCheck),
            Box::new(checks::security::SshdRootLoginOffCheck),
            // Sovereignty (+4)
            Box::new(checks::sovereignty::EuropeanOwnedVerifiedCheck),
            Box::new(checks::sovereignty::SbomSignedCheck),
            Box::new(checks::sovereignty::NoUsCloudDependenciesEgressCheck),
            Box::new(checks::sovereignty::LicenseComplianceCheck),
            // Performance (+3)
            Box::new(checks::performance::AgentDialLatencyCheck),
            Box::new(checks::performance::TlsSessionResumptionCheck),
            Box::new(checks::performance::AuditWriteAmpCheck),
            // Cost (+3)
            Box::new(checks::cost::CostPerRequestCheck),
            Box::new(checks::cost::EgressSpikeCheck),
            Box::new(checks::cost::BackupCostDriftCheck),
        ]);
        v
    }
    fn fixes(&self) -> Vec<Box<dyn DoctorFix>> { /* ... 20+ new fixes ... */ }
}
```

### Tests

- Unit: every new check has a unit test against the `inmemory` fixture.
- Integration: 4-fixture matrix in CI.
- Integration: `sovereign doctor --level full` on a 3-server fleet fixture (Docker Compose) runs in < 60s parallel, returns 0 fails on the secure fleet.
- Integration: `--watch` over 1 hour in a fixture where one agent's heartbeat is killed produces a `Pass → Fail` state change for `agent_heartbeat_fresh`.
- KB: `sovereign doctor kb | wc -l` returns ≥ 70.
- Audit hash chain: a tampered audit event (a row's `payload` is modified by hand) makes `audit_log_hash_chain` return `Fail` with the offending row id.

### Acceptance criteria

- [ ] `sovereign doctor --level full` runs 70+ checks in < 60s on a single box, < 90s on a 3-server fleet.
- [ ] Every category has 4-6 checks.
- [ ] Every check has a fix or a KB page.
- [ ] The audit hash chain catches tampering.
- [ ] KB has 70+ articles.

### Definition of done

A 4-week soak: 5 design partners with multi-server fleets run `sovereign doctor --level full` daily. At least 1 unprompted issue per partner per week is caught (a token rotation due, a backup drift, a coredump enabled, a sovereignty regression). The auto-fix rate is ≥ 50%.

---

## H17. `sovereign doctor fleet` and `sovereign doctor remote <host>` — multi-server diagnostic

**Goal:** Two new subcommands that run the doctor across the whole fleet in parallel and let the operator target a single agent from the server. The fleet view is the on-call's command when a customer reports "the app is slow" and the operator doesn't know if it's the server, an agent, the network, or the database.

**Why it matters:** The single-host `sovereign doctor` is the on-call's first command at 3am. The **fleet** doctor is the on-call's first command at 3am when the customer has 3 servers and the issue could be on any of them. Without fleet mode, the on-call SSHes into each box, runs doctor, manually correlates. With fleet mode, the answer is one command, in one table, with a clear "this box is the problem."

### Sub-tasks

1. **Implement `sovereign doctor fleet`** in `crates/sovereign/src/cmd/doctor.rs`:
   - Discovers all agents via the `server` table (the H1 agent pattern).
   - For each agent, in parallel (max 8 concurrent), runs the doctor over the existing `AgentPort::run_doctor` method (added in this feature).
   - Aggregates the results into a single table: `host | category | check | status | message | fix?`.
   - The `--summary` flag collapses the table to one row per host: `host | pass | warn | fail | top_fail`.
   - The `--json` and `--report` modes work the same as the single-host versions but with a fleet-wide scope.
   - The `--level` flag applies to all agents; the operator can pass `--level basic` for a quick fleet scan.
   - The default is `standard` (V1.5); `--level full` is supported but slower.
2. **Implement the `AgentPort::run_doctor` method** in H1's `AgentPort` trait:
   ```rust
   #[async_trait]
   pub trait AgentPort: Send + Sync {
       // ... existing methods ...
       async fn run_doctor(&self, level: DoctorLevel) -> Result<DoctorReport, AgentError>;
   }
   ```
   The agent runs the doctor locally and streams the result back over mTLS.
3. **Implement `sovereign doctor remote <host>`** — runs the doctor on a single agent (specified by hostname or agent id), with the same output formatting as the single-host version.
4. **Implement `sovereign doctor fleet diff`** — runs the doctor fleet-wide twice (e.g., 1 hour apart) and shows the state changes. Catches "this agent was passing an hour ago and is now failing."
5. **Implement the fleet-wide audit event** — when fleet doctor finds a fail on an agent, the server records a `kind: "doctor.fleet_fail"` audit event with the agent id, the check, the status.
6. **Wire the fleet doctor into the morning report (G11)** — a per-host summary is included.
7. **Wire the fleet doctor into the TUI** — the Servers view shows a doctor column; pressing `d` on a row runs the remote doctor.
8. **Add Prometheus metrics**:
   - `sovereign_doctor_fleet_runs_total{level,result}` — counter.
   - `sovereign_doctor_fleet_check_duration_seconds{host,category,name}` — histogram.
   - `sovereign_doctor_fleet_failing_hosts{level}` — gauge, number of hosts with ≥ 1 fail.
9. **Add a CI test**: a 3-server fleet fixture (Docker Compose) is doctor'd with `sovereign doctor fleet --level standard`; the result is a 3-row table, all 0 fails on the secure fleet.

### Code stub

```rust
// crates/sovereign/src/cmd/doctor.rs (additions)
#[derive(Args, Debug)]
pub struct DoctorFleetCmd {
    #[arg(long, value_enum, default_value_t = DoctorLevel::Standard)]
    pub level: DoctorLevel,
    #[arg(long)] pub summary: bool,
    #[arg(long, default_value_t = 8)] pub concurrency: usize,
    #[arg(long)] pub json: bool,
    #[arg(long)] pub report: Option<std::path::PathBuf>,
    #[arg(long)] pub diff_since: Option<chrono::DateTime<chrono::Utc>>,
}
pub async fn fleet(cmd: DoctorFleetCmd, ctx: AppContext) -> anyhow::Result<()> {
    let agents = ctx.state.list_agents().await?;
    let semaphore = Arc::new(tokio::sync::Semaphore::new(cmd.concurrency));
    let mut tasks = vec![];
    for agent in agents {
        let sem = semaphore.clone();
        let ctx2 = ctx.clone();
        let level = cmd.level;
        tasks.push(tokio::spawn(async move {
            let _permit = sem.acquire().await?;
            let port = ctx2.agent_for(&agent).await?;
            port.run_doctor(level).await
        }));
    }
    let results: Vec<_> = futures::future::join_all(tasks).await
        .into_iter().filter_map(|r| r.ok()).collect();
    if cmd.diff_since.is_some() { /* ... */ }
    render_fleet_table(&results, cmd.summary);
    if let Some(path) = cmd.report { write_report(&results, &path)?; }
    Ok(())
}
```

### Tests

- Unit: the fleet aggregator correctly merges 3 agent reports into a 3-row table.
- Unit: `doctor fleet diff` on two reports with state changes produces the expected diff.
- Integration: 3-server fleet fixture; `sovereign doctor fleet --level standard` returns 0 fails on the secure fleet; the broken fleet has ≥ 3 fails across 2 agents.
- Integration: `sovereign doctor remote agent-1` returns a single-agent report.
- TUI: the Servers view's `d` keybinding runs the remote doctor and shows the result inline.
- CI gate: a job runs `sovereign doctor fleet` against the 3-server fixture on every PR.

### Acceptance criteria

- [ ] `sovereign doctor fleet` runs doctor across all agents in parallel.
- [ ] `sovereign doctor remote <host>` runs doctor on a single agent.
- [ ] The TUI's Servers view has a doctor column and a `d` shortcut.
- [ ] `--summary` collapses to one row per host.

### Definition of done

A 4-week soak: 3 design partners with multi-server fleets run `sovereign doctor fleet --summary` daily. The summary view is the first thing they check; the per-host drill-down is the second. At least 1 unprompted fleet-wide issue per partner per month is caught (an agent version drift, a token rotation due on one agent, a backup failing on one agent only).

---

## H18. `sovereign incident` toolkit — declare, note, resolve, escalate, postmortem

**Goal:** A new subcommand family that turns the on-call's "incident response" into a CLI workflow: declare an incident, append timestamped notes, resolve it, and (if the team tier is active) auto-generate a postmortem skeleton. The toolkit is **always available** (even on the free tier) because losing a customer to a 4-hour MTTR is worse than the cost of the feature.

**Why it matters:** The 3am test has a second half: after the operator runs `sovereign doctor --fix` and the system is back, they need to write down what happened, when, and why. Without a CLI toolkit, the postmortem is a 2-day-later Notion document written from memory. With the toolkit, the postmortem is a 30-second `sovereign incident resolve --postmortem` invocation that produces a markdown skeleton pre-populated with the doctor snapshots, the audit trail, the deploy history, and the rollback events.

### Sub-tasks

1. **Implement the `incident` table** in SQLite:
   ```sql
   CREATE TABLE incident (
       id TEXT PRIMARY KEY,             -- e.g., "inc-2025-12-01-001"
       title TEXT NOT NULL,
       severity TEXT NOT NULL,          -- sev1, sev2, sev3
       status TEXT NOT NULL,            -- declared, mitigating, resolved, postmortem_written
       declared_at TEXT NOT NULL,
       declared_by TEXT NOT NULL,       -- user id
       resolved_at TEXT,
       resolved_by TEXT,
       summary TEXT,                    -- 1-2 sentence root cause
       tags TEXT,                       -- JSON array
       related_apps TEXT,               -- JSON array
       related_doctor_runs TEXT         -- JSON array of doctor run ids
   );
   CREATE TABLE incident_note (
       id INTEGER PRIMARY KEY AUTOINCREMENT,
       incident_id TEXT NOT NULL,
       at TEXT NOT NULL,
       author TEXT NOT NULL,
       body TEXT NOT NULL,
       FOREIGN KEY (incident_id) REFERENCES incident(id)
   );
   ```
2. **Implement `sovereign incident declare`** CLI:
   - Flags: `--title`, `--severity sev1|sev2|sev3`, `--apps <csv>`, `--tags <csv>`.
   - Generates a unique id (date + sequence).
   - Records a `doctor --level basic` snapshot and attaches its id to the incident.
   - Writes an audit event: `kind: "incident.declared"`.
   - Prints the incident id and the suggested first actions: `1. sovereign doctor --level standard --fix; 2. sovereign incident note "..."; 3. sovereign incident resolve`.
3. **Implement `sovereign incident note <incident_id> "<body>"`**:
   - Appends a timestamped note (id, at, author, body).
   - Writes an audit event.
4. **Implement `sovereign incident resolve <incident_id>`**:
   - Flags: `--summary`, `--root-cause`, `--postmortem <path>`.
   - Marks the incident as `resolved`.
   - Records the final doctor snapshot.
   - If `--postmortem` is given, generates a markdown file pre-populated with the incident timeline (declared_at, all notes, resolved_at), the doctor snapshots, the related audit events, the deploys/rollbacks during the incident window, and a blank "What went well / What went wrong / Where we got lucky / Action items" section for the operator to fill in.
   - Writes an audit event: `kind: "incident.resolved"`.
5. **Implement `sovereign incident list`** — lists all open and recent (last 30 days) incidents with status, severity, age.
6. **Implement `sovereign incident show <incident_id>`** — prints the full incident timeline with all notes and the auto-collected doctor snapshots.
7. **Implement `sovereign incident escalate`** — for team tier, sends a notification to the team's PagerDuty / Slack / email channel. V1.5 supports Slack and email; PagerDuty is V2.
8. **Wire the incident toolkit into the auto-rollback path (F8b)**: when a deployment auto-rolls back, the binary checks "is there an open incident for this app?" If yes, appends a note to the incident. If no, and the rollback is the second in 1 hour, auto-declares a sev3 incident.
9. **Wire the incident toolkit into doctor**: when `sovereign doctor --level standard --fix` finds a fail that it cannot auto-fix, the operator is prompted: "Auto-declare a sev3 incident? (y/n, 5s)".
10. **Add Prometheus metrics**:
    - `sovereign_incidents_declared_total{severity}` — counter.
    - `sovereign_incidents_open{severity}` — gauge.
    - `sovereign_incident_mttr_seconds{severity}` — histogram (resolved_at - declared_at).
11. **Add a CI test**: declare → 3 notes → resolve with a postmortem path; assert the postmortem file is generated and contains the timeline.

### Code stub

```rust
// crates/sovereign/src/cmd/incident.rs
use clap::{Args, Subcommand};
#[derive(Args, Debug)]
pub struct IncidentCmd { #[command(subcommand)] action: IncidentAction }
#[derive(Subcommand, Debug)]
pub enum IncidentAction {
    Declare { #[arg(long)] title: String, #[arg(long, value_parser = parse_severity)] severity: Severity,
              #[arg(long, value_delimiter = ',')] apps: Vec<String>,
              #[arg(long, value_delimiter = ',')] tags: Vec<String> },
    Note { id: String, body: String },
    Resolve { id: String, #[arg(long)] summary: String, #[arg(long)] root_cause: Option<String>,
              #[arg(long)] postmortem: Option<std::path::PathBuf> },
    List { #[arg(long, default_value_t = 30)] days: u32 },
    Show { id: String },
    Escalate { id: String, #[arg(long)] channel: String },
}
pub async fn run(cmd: IncidentCmd, ctx: AppContext) -> anyhow::Result<()> {
    match cmd.action {
        IncidentAction::Declare { title, severity, apps, tags } => {
            let id = generate_incident_id();
            let snapshot = ctx.doctor.run(DoctorLevel::Basic, &ctx).await?;
            ctx.state.insert_incident(&Incident { id: id.clone(), title, severity, status: IncidentStatus::Declared, declared_at: now(), declared_by: ctx.user.id(), summary: None, tags, related_apps: apps, related_doctor_runs: vec![snapshot.id] }).await?;
            ctx.audit.record("incident.declared", &json!({"id": id})).await?;
            println!("Declared {id}. First actions:");
            println!("  1. sovereign doctor --level standard --fix");
            println!("  2. sovereign incident note {id} \"...\"");
            println!("  3. sovereign incident resolve {id}");
            Ok(())
        }
        // ... other actions
    }
}
```

### Tests

- Unit: the postmortem generator produces a markdown file with the expected sections.
- Unit: `incident.escalate` calls the Slack/email adapter with the right payload.
- Integration: declare → 3 notes → resolve with postmortem; the postmortem file is valid markdown, contains the timeline.
- Integration: a 2nd auto-rollback within 1 hour auto-declares a sev3 incident.
- CI gate: a job runs the declare→resolve cycle and asserts the audit log has 4 events.

### Acceptance criteria

- [ ] `sovereign incident declare/note/resolve/list/show/escalate` work.
- [ ] The postmortem skeleton is pre-populated with timeline + doctor snapshots + audit + deploys.
- [ ] The 2nd auto-rollback in 1 hour auto-declares a sev3.

### Definition of done

A 4-week soak: 5 design partners use `sovereign incident declare` at least once per real outage. ≥ 4 of 5 use `--postmortem` to generate a postmortem file. The median time from `declare` to `resolve` is < 30 min. The audit log shows every incident has a complete timeline.

---

## H19. `sovereign retro` and `sovereign game-day` — postmortem aggregation + chaos drills

**Goal:** Two new subcommands that turn the incident toolkit into a learning loop. `sovereign retro` aggregates a window of incidents and produces a quarterly review. `sovereign game-day` is a CLI-driven chaos engineering suite that runs 8 pre-canned failure scenarios (server kill, network partition, disk full, CPU spike, backup corrupt, cert expire, DB failover, rate limit) against a staging box and asserts the system recovers.

**Why it matters:** The on-call's life is: (1) detect, (2) mitigate, (3) resolve, (4) write postmortem, (5) learn. The first 4 are V1.5's incident toolkit (H18). The 5th is `sovereign retro`. And the meta-skill — being sure the system actually recovers when the failure mode hits — is `sovereign game-day`. Together, these close the loop and answer the only question that matters: *"will the system survive a real outage?"*

### Sub-tasks

1. **Implement `sovereign retro`** CLI:
   - Flags: `--since 2025-09-01`, `--until 2025-12-01`, `--format md|json`, `--out <path>`.
   - Aggregates all `incident` rows in the window.
   - Computes: MTTR per severity, top 5 root causes (tagged), top 5 most-affected apps, doctor fail trends (which checks fail most often), action item rollup (from postmortem "Action items" sections).
   - Writes a markdown retro with sections `# Executive summary`, `# MTTR trends`, `# Top root causes`, `# Top affected apps`, `# Doctor trends`, `# Action items (rollup)`, `# Recommended focus for next quarter`.
   - Prints to stdout if `--out` is not given.
2. **Implement `sovereign retro --publish`** — for the team tier, publishes the retro to the team's `#engineering` Slack channel.
3. **Implement `sovereign game-day`** CLI:
   - Subcommand for each of the 8 scenarios: `sovereign game-day scenario <name> [--target staging]`.
   - The 8 scenarios:
     1. **`server-kill`** — `docker kill` the running server, assert the agent re-registers within 30s.
     2. **`network-partition`** — `iptables -A INPUT -s <other-server> -j DROP`, assert the server marks the agent as unreachable, then `iptables -D` to heal, assert the agent re-registers.
     3. **`disk-full`** — `dd if=/dev/zero of=/var/lib/sovereign/fill bs=1M count=10000`, assert `doctor` reports `disk` fail, then `rm` the file, assert recovery.
     4. **`cpu-spike`** — `stress-ng --cpu 4 --timeout 60s`, assert `doctor` reports `cpu_steal_low` warn (or the per-host equivalent), then `pkill stress-ng`.
     5. **`backup-corrupt`** — replace the latest backup file with random bytes, assert `doctor` reports `backup_recent` or `backup_offsite` fail, then restore from the previous good backup.
     6. **`cert-expire`** — replace the served cert with an expired one, assert `doctor` reports `caddy_ocsp_stapled` or `tls_handshake_fast` fail, then restore the good cert.
     7. **`db-failover`** — kill the SQLite writer, assert the auto-rollback triggers (F8b), then restart the writer.
     8. **`rate-limit`** — fire 10,000 requests in 10s, assert Caddy returns 429s and the app doesn't OOM, then stop the load.
   - Each scenario has: pre-conditions, the action, the expected doctor/audit/rollback outputs, the recovery steps.
   - `sovereign game-day run-all` runs all 8 sequentially with a 60s pause between.
   - The result of each scenario is recorded in a `game_day_run` table.
4. **Implement `sovereign game-day report`** — aggregates the last 20 game-day runs and produces a markdown report: `# Game-day report`, per-scenario pass/fail, the time to recovery per scenario, the doctor fails caught.
5. **Wire game-day into CI** — a nightly job runs `sovereign game-day run-all` against a staging box; the result is posted to the team's `#ci` channel.
6. **Wire retro into the monthly blog post pipeline** — `sovereign retro --since 2025-09-01 --format md` is the input to the "What we learned this quarter" section of the engineering blog.
7. **Add a CI test**: each game-day scenario is run in a Docker fixture; the test asserts the scenario's expected doctor output and the recovery is clean.

### Code stub

```rust
// crates/sovereign/src/cmd/game_day.rs
use clap::{Args, Subcommand};
#[derive(Args, Debug)]
pub struct GameDayCmd { #[command(subcommand)] action: GameDayAction }
#[derive(Subcommand, Debug)]
pub enum GameDayAction {
    Scenario { name: String, #[arg(long, default_value = "staging")] target: String },
    RunAll { #[arg(long, default_value = "staging")] target: String },
    Report { #[arg(long, default_value_t = 30)] days: u32 },
    List,  // print the 8 scenarios with descriptions
}
pub async fn run(cmd: GameDayCmd, ctx: AppContext) -> anyhow::Result<()> {
    match cmd.action {
        GameDayAction::Scenario { name, target } => {
            let scenario = scenarios::by_name(&name).ok_or_else(|| anyhow!("unknown scenario: {name}"))?;
            println!("▶ Running game-day scenario: {} on {}", scenario.name, target);
            scenario.pre_check(&ctx, &target).await?;
            scenario.run(&ctx, &target).await?;
            let doctor = ctx.doctor.run(DoctorLevel::Standard, &ctx).await?;
            scenario.assert(&doctor, &ctx, &target).await?;
            scenario.recover(&ctx, &target).await?;
            let doctor2 = ctx.doctor.run(DoctorLevel::Standard, &ctx).await?;
            scenario.assert_recovery(&doctor2, &ctx, &target).await?;
            println!("✓ Scenario {} passed in {}", scenario.name, scenario.elapsed());
            Ok(())
        }
        // ...
    }
}
```

### Tests

- Unit: `retro` on a fixture of 10 incidents produces the expected MTTR and top root cause list.
- Unit: each game-day scenario's `assert` is correct.
- Integration: each of the 8 scenarios runs in a Docker fixture and passes the assertion.
- Integration: `sovereign game-day run-all` against the fixture runs all 8 in < 30 min and the report is generated.
- CI gate: a nightly job runs game-day against staging; the result is posted to Slack.

### Acceptance criteria

- [ ] `sovereign retro` aggregates a window of incidents into a markdown retro.
- [ ] `sovereign game-day list` shows the 8 scenarios.
- [ ] `sovereign game-day scenario <name>` runs the named scenario.
- [ ] `sovereign game-day run-all` runs all 8 sequentially.
- [ ] `sovereign game-day report` aggregates the last 20 runs.

### Definition of done

A quarterly review: 3 design partners run `sovereign game-day run-all` monthly against their staging. ≥ 1 unprompted catch per partner per quarter (a scenario that "passed" in CI but "failed" in the partner's real environment — usually a config drift). The retro is the input to the engineering blog's "What we learned this quarter."

---

## H20. `sovereign import --from <platform>` — migration from Heroku, Coolify, Dokploy, Render

**Goal:** A first-class `sovereign import` subcommand that reads a platform's export and creates the equivalent sovereign apps, databases, and env files. Four platforms are supported in V1.5: **heroku** (the biggest single opportunity — the Feb 2026 sustaining-engineering announcement is a once-in-a-decade migration window), **coolify** (the largest OSS self-hosted PaaS by installs), **dokploy** (the fastest-growing 2024-2025), and **render** (the most similar managed PaaS). Dokku and Kamal are V2+ (smaller user bases, command-line parity with us already).

**Why it matters:** Per `persona-pm.md` §5: "Be the destination, not the source, of migrations. Coolify has no migration-from-Herkoku guide. Dokploy maintainer said 'unfeasible' in issue #3098. We do." Per `user-pain-research.md` §6.2, Heroku's "sustaining engineering" announcement is the highest-leverage 12-month feature. The wedge is: every team leaving Heroku is now shopping, and the team that gives them a 30-minute migration wins by default.

### Sub-tasks

1. **Define the `ImportPort` trait** in `sovereign-core/src/ports/import.rs`:
   ```rust
   #[async_trait]
   pub trait ImportPort: Send + Sync {
       fn platform(&self) -> &'static str;
       async fn detect(&self, source: &ImportSource) -> Result<ImportPlan, ImportError>;
       async fn execute(&self, plan: &ImportPlan, ctx: &ImportContext) -> Result<ImportReport, ImportError>;
   }
   pub struct ImportPlan { pub apps: Vec<AppSpec>, pub databases: Vec<DbSpec>, pub env: HashMap<String, String>, pub warnings: Vec<String> }
   pub struct ImportReport { pub apps_created: usize, pub dbs_created: usize, pub env_imported: usize, pub manual_steps: Vec<String> }
   ```
2. **Implement the `heroku` adapter** (the highest-leverage):
   - `sovereign import --from heroku --app <name> --api-key <key>`:
     - Calls the Heroku Platform API v3 to list apps, list addons (the Postgres / Redis / MySQL ones), list config vars, list buildpacks.
     - For each app: writes a `sovereign.toml` that maps the buildpack → our framework scanner (e.g., `heroku/ruby` → `framework: rails`); calls `sovereign deploy` with `--image=herokuish` for the rare case of an unsupported buildpack.
     - For each Postgres addon: `heroku pg:backups:capture --app <name>` → download the dump URL → `sovereign db import postgres` to create the local DB. Resets the connection string; prints the new DATABASE_URL the user must paste into the imported env.
     - For each config var: `sovereign secret set` from the value (the value is read from Heroku and piped; never appears in the CLI args).
     - For each domain: `sovereign domain add <domain>`.
   - The full import runs in a `--dry-run` mode that prints the plan, then a `--apply` mode that runs it. `--diff` shows the delta against the current sovereign state.
3. **Implement the `coolify` adapter** (the most "feature parity" import):
   - Reads a `coolify export <name> --format json` payload (the user's running Coolify box produces the export via our import command in *their* Coolify? No — we read the exported JSON directly from the user's `~/coolify-export.json`).
   - Maps Coolify's "applications" → our `app.yaml`; maps Coolify's "service" rows (Postgres, MySQL, Redis) → our DBs; maps Coolify's "environment" → our secrets.
   - The hard part: Coolify's app format is docker-compose, not our `app.yaml`. The adapter transpiles a Compose project to a sovereign app; if the Compose uses features we don't support (named volumes with `driver_opts`, network aliases), the import reports a warning and skips the unsupported bits.
4. **Implement the `dokploy` adapter**:
   - Reads a `dokploy export --format yaml` payload.
   - Maps Dokploy's project → our app; maps Dokploy's compose services → our apps; maps Dokploy's mariadb/postgres/redis/mongo services → our DBs.
   - Dokploy's UI has no export, so the import also ships a small `dokploy-export` CLI tool the user runs once on the Dokploy box (a 200-line Go binary or a Python script that reads Dokploy's Postgres state). The script writes a `dokploy-export.json` that our import consumes.
5. **Implement the `render` adapter**:
   - Calls the Render API v1 with a user's API key.
   - For each "service": maps to our app; reads the `Dockerfile` and `dockerCommand`; produces a sovereign app.
   - For each "postgres" / "redis" addon: pg_dump or redis dump, then `sovereign db import`.
   - For each "env var" group: `sovereign secret set` from the value.
6. **Implement `sovereign import list`** — shows the 4 supported platforms, what they support (apps / DBs / env / domains), and the export command the user runs on the source platform.
7. **Implement `sovereign import report <id>`** — shows the report from a previous import: which apps were created, which DBs, which env vars, which warnings, which manual steps remain.
8. **Implement `sovereign import rollback <id>`** — deletes the apps/DBs/secrets created by a specific import (with a 5s confirm; the operation is auditable). This is the safety net for the "I imported the wrong thing" case.
9. **Write the 4 public migration guides** (in `docs/src/migration/`):
   - `migration/from-heroku.md` — 30-minute walkthrough, screenshot, expected output at every step.
   - `migration/from-coolify.md` — same.
   - `migration/from-dokploy.md` — same.
   - `migration/from-render.md` — same.
   - Each guide has: "Before you start" (a checklist), "Step 1: install sovereign", "Step 2: export from <platform>", "Step 3: run import --dry-run", "Step 4: review the plan", "Step 5: import --apply", "Step 6: verify", "Step 7: cut DNS over", "Step 8: keep <platform> for 7 days, then delete".
10. **Wire the import into the morning report (G11)** — a 1-line "Last import: 12 apps from heroku, 3d ago" line for the team tier.
11. **Privacy**: import never sends the imported data anywhere. The Heroku API key is read from stdin, used once, discarded. The exported payloads are stored in `/var/lib/sovereign/imports/<id>/` and never uploaded.

### Code stub

```rust
// crates/sovereign/src/cmd/import.rs
use clap::{Args, ValueEnum};
#[derive(Copy, Clone, ValueEnum, PartialEq, Eq)]
pub enum ImportSource { Heroku, Coolify, Dokploy, Render }
#[derive(Args, Debug)]
pub struct ImportCmd {
    #[arg(long, value_enum)] from: ImportSource,
    /// App name (Heroku, Render) or path to exported JSON (Coolify, Dokploy)
    #[arg(long)] app: Option<String>,
    #[arg(long)] api_key: Option<String>,    // read from stdin if not given
    #[arg(long, default_value_t = false)] dry_run: bool,
    #[arg(long, default_value_t = false)] apply: bool,
    #[arg(long, default_value_t = false)] diff: bool,
}
pub async fn run(cmd: ImportCmd, ctx: AppContext) -> anyhow::Result<()> {
    let port = pick_port(cmd.from);
    let source = if matches!(cmd.from, ImportSource::Coolify | ImportSource::Dokploy) {
        ImportSource::File(cmd.app.context("expected path to export JSON")?)
    } else {
        ImportSource::Api { app: cmd.app.context("--app required")?, key: read_key(cmd.api_key)? }
    };
    let plan = port.detect(&source).await?;
    if cmd.diff { print_diff(&plan, &ctx)?; return Ok(()); }
    if cmd.dry_run { print_plan(&plan); return Ok(()); }
    let report = port.execute(&plan, &ImportContext::from(&ctx)).await?;
    ctx.audit.record("import.applied", &json!({
        "from": cmd.from, "apps": report.apps_created,
        "dbs": report.dbs_created, "env": report.env_imported,
    })).await?;
    print_report(&report);
    Ok(())
}
```

### Tests

- Unit: each adapter's `detect` returns the expected plan for a fixture export.
- Unit: each adapter's `execute` creates the expected apps/DBs/secrets.
- Unit: `sovereign import rollback` removes the created resources.
- Integration: the Heroku fixture (a real test Heroku app) imports into a local sovereign; the resulting app serves the same HTTP responses.
- Integration: the Coolify fixture (an exported JSON) imports into a local sovereign; the resulting compose apps start.
- CI gate: a job runs all 4 adapters' `--dry-run` against fixture exports; the plans are stable across runs.

### Acceptance criteria

- [ ] `sovereign import --from heroku --app <name>` imports a real Heroku app in < 30 min.
- [ ] `sovereign import --from coolify` reads a Coolify export and creates the equivalent apps.
- [ ] `sovereign import --from dokploy` reads a Dokploy export and creates the equivalent apps.
- [ ] `sovereign import --from render` imports a real Render service in < 15 min.
- [ ] `sovereign import rollback <id>` removes the imported resources.
- [ ] The 4 migration guides are published.

### Definition of done

A 12-week soak: 5 Heroku customers (recruited from the r/heroku, r/rails, and Show HN audiences) use `sovereign import --from heroku` to migrate a real production app. ≥ 4 of 5 complete the migration in < 30 minutes. The `migration/from-heroku.md` guide is the most-viewed page on the docs site within 4 weeks of the V1.5 release.

---

## H21. `sovereign dns` — first-class DNS manager

**Goal:** A `sovereign dns` subcommand family that owns the DNS lifecycle for the user's domains. Integrates with the major registrars (Cloudflare, Hetzner DNS, OVH, Gandi, Namecheap, Porkbun, RFC 2136 generic) via the same `DnsProviderPort` trait pattern as the other ports. Cert issuance (F6) becomes "point your domain at our nameservers or set the ACME challenge; we handle the rest."

**Why it matters:** Per `competitive-landscape.md` §14.1: "Built-in DNS manager" is a top complaint against both Coolify and Dokploy (Dokploy issue #4376, May 2026: "Why is there no DNS manager? I have to hop to Cloudflare every time I add a domain"). Every Show HN for a self-hosted PaaS gets a top comment asking "does it handle DNS." The current F6 cert issuance flow requires the user to manually point the domain; a `sovereign dns` subcommand that owns the zone records is the natural next step, and it removes a daily ops friction.

### Sub-tasks

1. **Define the `DnsProviderPort` trait** in `sovereign-core/src/ports/dns.rs`:
   ```rust
   #[async_trait]
   pub trait DnsProviderPort: Send + Sync {
       fn name(&self) -> &'static str;            // "cloudflare", "hetzner", etc.
       async fn list_zones(&self) -> Result<Vec<DnsZone>, DnsError>;
       async fn list_records(&self, zone: &str) -> Result<Vec<DnsRecord>, DnsError>;
       async fn upsert_record(&self, zone: &str, rec: &DnsRecord) -> Result<(), DnsError>;
       async fn delete_record(&self, zone: &str, id: &str) -> Result<(), DnsError>;
   }
   ```
2. **Implement the 7 providers** as in-tree adapters in `sovereign-dns/`:
   - `cloudflare` (the most common; uses the Cloudflare API v4 with a scoped API token)
   - `hetzner` (the recommended EU-sovereign choice; Hetzner DNS API)
   - `ovh` (the French/Italian EU public-sector choice; OVH API)
   - `gandi` (the historical French choice; Gandi LiveDNS API)
   - `namecheap` (the budget choice; Namecheap API)
   - `porkbun` (the indie favorite; Porkbun API)
   - `rfc2136` (the generic; for self-hosted BIND, PowerDNS, etc., used in air-gapped installs)
3. **Implement the CLI** in `crates/sovereign/src/cmd/dns.rs`:
   - `sovereign dns provider add <name> --token <from-stdin>` — saves a provider credential (encrypted with F7's master key).
   - `sovereign dns provider list` — shows the configured providers.
   - `sovereign dns zone list --provider <name>` — lists the user's zones.
   - `sovereign dns zone import <zone> --provider <name>` — imports a zone (all records) into our state; we mirror it.
   - `sovereign dns record list <zone> --provider <name>` — lists records.
   - `sovereign dns record add <zone> <name> <type> <value> --provider <name>` — adds a record.
   - `sovereign dns record delete <zone> <id> --provider <name>` — deletes a record.
4. **Integrate with `sovereign domain add`** (F6):
   - When the operator runs `sovereign domain add api.example.com`, the binary:
     - Detects the registrar (via SOA query or by asking).
     - If a provider is configured, automatically creates the `A`/`AAAA` record (or `CNAME` for the `*.srvr.so` case).
     - If no provider is configured, prints the exact records to add (A, AAAA, CAA) and a one-click "open Cloudflare" link.
     - In both cases, polls DNS until the record propagates (timeout 5 min), then triggers Caddy ACME.
5. **Integrate with the certificate renewal** (F6):
   - When a wildcard cert (`*.example.com`) is needed, the binary auto-adds the ACME DNS-01 challenge record (`_acme-challenge.example.com`) via the configured provider, polls for propagation, triggers renewal, then deletes the challenge record.
   - This removes the "DNS-01 needs a script" friction.
6. **Implement the audit log** for every DNS mutation: `kind: "dns.upsert"`, `kind: "dns.delete"`, with the zone, record, and provider in the payload.
7. **Add a doctor check** `dns_provider_configured` (added to G16 standard level): the configured provider's API token validates; a test zone can be read; the test record can be written and deleted.
8. **Add a doctor check** `dns_zone_delegated` (added to G16 standard level): the user's NS records at the registrar point at the configured provider's nameservers.
9. **Wire the DNS mutations into the morning report (G11)** — a 1-line "DNS: 4 providers, 23 zones, 0 stale records" line.
10. **Privacy**: provider tokens are encrypted with F7's master key; never logged; never sent anywhere except the provider's API.
11. **Add a CI test**: each of the 7 providers' adapter is tested against a mock HTTP server that implements the provider's API contract; the test asserts the expected mutations.

### Code stub

```rust
// crates/sovereign-dns/src/providers/cloudflare.rs
use async_trait::async_trait;
use reqwest::Client;
use sovereign_core::ports::dns::{DnsProviderPort, DnsZone, DnsRecord, DnsError};
pub struct CloudflareProvider { token: String, client: Client }
impl CloudflareProvider {
    pub fn new(token: String) -> Self {
        let client = Client::builder()
            .default_headers({ let mut h = reqwest::header::HeaderMap::new();
                h.insert("Authorization", format!("Bearer {token}").parse().unwrap());
                h.insert("Content-Type", "application/json".parse().unwrap()); h })
            .build().unwrap();
        Self { token, client }
    }
}
#[async_trait]
impl DnsProviderPort for CloudflareProvider {
    fn name(&self) -> &'static str { "cloudflare" }
    async fn list_zones(&self) -> Result<Vec<DnsZone>, DnsError> { /* GET /zones */ }
    async fn upsert_record(&self, zone: &str, rec: &DnsRecord) -> Result<(), DnsError> { /* POST /zones/:id/dns_records */ }
    // ...
}
```

### Tests

- Unit: each provider's adapter is correct against a mock server (per-provider fixtures).
- Integration: a real Cloudflare test zone has records added/listed/deleted.
- Integration: `sovereign domain add` with a configured provider auto-creates the A record.
- Integration: a wildcard cert (`*.example.com`) renews via DNS-01 with no operator action.
- Doctor: `dns_provider_configured` and `dns_zone_delegated` return Pass on a configured test box.
- CI gate: a job exercises all 7 providers' adapters.

### Acceptance criteria

- [ ] `sovereign dns provider add/list` works for all 7 providers.
- [ ] `sovereign dns zone import <zone> --provider <name>` mirrors a zone.
- [ ] `sovereign domain add` auto-creates the A/AAAA record via the configured provider.
- [ ] Wildcard cert renewal uses DNS-01 with no operator action.
- [ ] The 2 new doctor checks (`dns_provider_configured`, `dns_zone_delegated`) work.

### Definition of done

A 4-week soak: 5 design partners (each on a different provider) use `sovereign domain add` for a new domain. ≥ 4 of 5 complete the add in < 2 minutes (vs. the 5-10 minute manual flow). ≥ 1 unprompted catch: a stale `A` record from a previous deploy that the binary detects and removes (a "DNS drift" check, added to the morning report).

---

## H22. `sovereign db pool` — first-class connection pool (PgBouncer, MySQL proxy, Redis pool)

**Goal:** A `sovereign db pool` subcommand that manages a connection pool in front of every provisioned database (G6). The pool is a thin layer over **PgBouncer** (Postgres), **ProxySQL** (MySQL), or **redis-pool** (Redis), managed as a separate process by the sovereign binary. The user gets the same `DATABASE_URL` (now pointing at `127.0.0.1:6432`), but the actual `max_connections` is decoupled from the app's connection count — the canonical "AI-built app at 30 concurrent requests crashes" failure mode becomes a non-event.

**Why it matters:** Per `user-pain-research.md` §4.2: "Connection pool exhaustion under serverless load" is the **#9 pain point at 9/10 severity** with **9/10 automation potential**. The architectural advantage of a long-lived server process is real, but the *product surface* doesn't name it. A solo developer with a single Hetzner box doesn't think about connection pools — until their AI-built app crashes at 20 concurrent users. With `sovereign db pool`, the binary ships the pool; the operator's `DATABASE_URL` is automatically routed through it; the doctor checks that the pool is healthy; the cost is one extra 30 MB process.

### Sub-tasks

1. **Define the `ConnectionPoolPort` trait** in `sovereign-core/src/ports/pool.rs`:
   ```rust
   #[async_trait]
   pub trait ConnectionPoolPort: Send + Sync {
       fn name(&self) -> &'static str;            // "pgbouncer", "proxysql", "redis"
       fn backend(&self) -> DbBackend;            // Postgres, MySQL, Redis
       async fn start(&self, db: &DbSpec) -> Result<PoolHandle, PoolError>;
       async fn stop(&self, handle: PoolHandle) -> Result<(), PoolError>;
       async fn status(&self, handle: PoolHandle) -> Result<PoolStatus, PoolError>;
   }
   pub struct PoolStatus { pub active: u32, pub waiting: u32, pub max: u32, pub uptime: Duration }
   ```
2. **Implement the 3 backends** as in-tree adapters in `sovereign-db-pool/`:
   - `pgbouncer` (Postgres) — downloads the static `pgbouncer` binary (or builds from source via the `nix` static build, similar to the caddy approach), renders a `pgbouncer.ini` from the `DbSpec`, manages the process.
   - `proxysql` (MySQL) — same pattern, downloads the static `proxysql` binary, renders the config, manages the process.
   - `redis-pool` (Redis) — uses our own minimal in-Rust pool (no external binary needed; `deadpool-redis` is the dependency). This is the lightest, since Redis is single-threaded by design.
3. **Auto-pool on `sovereign db create`**:
   - When the operator runs `sovereign db create postgres --name my-app`, the binary:
     - Provisions the Postgres (G6 path).
     - Asks "enable a connection pool? [Y/n]". Default: Y, with the explanation "your app will route through a pool, surviving 100+ concurrent connections."
     - If Y, starts the pool process, writes the new `DATABASE_URL` to the secret store (under a separate key, e.g., `DATABASE_URL_POOL`), and prints the env var to update in the app.
4. **Auto-update `app.yaml`** to use the pooled URL:
   - When the pool is enabled for a DB, the binary scans the apps that reference that DB and updates their `env.DATABASE_URL` to point at the pool (with a 5s confirm per app).
5. **Implement the CLI** in `crates/sovereign/src/cmd/db/pool.rs`:
   - `sovereign db pool create --db <name> [--max-conn 100] [--min-conn 5]` — starts a pool in front of a DB.
   - `sovereign db pool list` — shows the pools, their status (active / waiting / max).
   - `sovereign db pool status <pool-id>` — detailed status with per-client connection counts.
   - `sovereign db pool stop <pool-id>` — stops the pool (the DB is unaffected; apps using `DATABASE_URL_POOL` will fail over to the direct URL after a 30s timeout).
   - `sovereign db pool restart <pool-id>` — restarts (e.g., after a config change).
6. **Wire the pool status into the TUI** — the Apps detail view's Resources tab shows the pool stats for the app's DB.
7. **Wire the pool status into the doctor** (G16 standard level):
   - `db_pool_configured` — for every Postgres/MySQL/Redis DB with > 1 app, the pool is configured.
   - `db_pool_active_low` — the active connection count is < 80% of max.
   - `db_pool_waiting_zero` — no clients have been waiting > 1s for a connection (sustained).
8. **Wire the pool into the morning report (G11)** — a 1-line "Pools: 3/3 healthy, max active 47/100" line.
9. **Add Prometheus metrics**:
   - `sovereign_db_pool_active{db,pool_id}` — gauge.
   - `sovereign_db_pool_waiting{db,pool_id}` — gauge.
   - `sovereign_db_pool_wait_seconds{db,pool_id}` — histogram.
   - `sovereign_db_pool_clients_total{db,pool_id}` — gauge.
10. **Add a CI test**: a fixture app makes 200 concurrent `SELECT 1` queries against a pooled DB; the pool keeps `active < max`; the doctor returns 0 fails.

### Code stub

```rust
// crates/sovereign-db-pool/src/pgbouncer.rs
use async_trait::async_trait;
use sovereign_core::ports::pool::{ConnectionPoolPort, PoolHandle, PoolStatus, DbBackend};
pub struct PgBouncerPool { data_dir: std::path::PathBuf }
#[async_trait]
impl ConnectionPoolPort for PgBouncerPool {
    fn name(&self) -> &'static str { "pgbouncer" }
    fn backend(&self) -> DbBackend { DbBackend::Postgres }
    async fn start(&self, db: &DbSpec) -> Result<PoolHandle, PoolError> {
        let cfg = render_pgbouncer_ini(db, &self.data_dir)?;
        let handle = tokio::process::Command::new("pgbouncer")
            .arg(cfg.path()).spawn()?;
        Ok(PoolHandle { id: db.id.clone(), pid: handle.id().unwrap(), listen: "127.0.0.1:6432".into() })
    }
    async fn status(&self, handle: PoolHandle) -> Result<PoolStatus, PoolError> {
        // `pgbouncer -R` returns SHOW POOLS; parse it
        // ...
        Ok(PoolStatus { active: 0, waiting: 0, max: 100, uptime: Duration::from_secs(0) })
    }
    // ...
}
```

### Tests

- Unit: `render_pgbouncer_ini` produces a valid config from a `DbSpec`.
- Unit: `PoolStatus` parsing from `pgbouncer -R` output.
- Integration: a Postgres DB with a pool serves 200 concurrent connections from a fixture app; the pool keeps `active < max`; the doctor's `db_pool_active_low` passes.
- Integration: `sovereign db pool stop` stops the process; subsequent `DATABASE_URL_POOL` connections fail.
- Integration: the auto-pool-on-create flow writes `DATABASE_URL_POOL` to the secret store.
- Doctor: 3 new checks (`db_pool_configured`, `db_pool_active_low`, `db_pool_waiting_zero`) pass on a healthy fixture.
- CI gate: a job runs the 200-concurrent-connection test on every PR.

### Acceptance criteria

- [ ] `sovereign db pool create --db <name>` starts a pool.
- [ ] The pool survives 200 concurrent connections without the underlying DB hitting `max_connections`.
- [ ] The 3 new doctor checks pass on a healthy fixture.
- [ ] `sovereign db pool status` returns real-time active/waiting/max.
- [ ] Auto-pool on `sovereign db create` writes `DATABASE_URL_POOL` to secrets.

### Definition of done

A 4-week soak: 5 design partners running AI-built apps (Cursor, Claude Code, v0) enable the pool. The "30 concurrent users crash" failure mode disappears. The doctor's `db_pool_configured` check is what catches the partner who *didn't* enable the pool: the doctor warns "you have 3 apps using the same Postgres, but no pool — the first 20-30 concurrent users will crash your app."

---

## H23. `sovereign status page` — built-in, opt-in, no third-party service

**Goal:** A `sovereign status page` subcommand that serves a public, read-only status page at `status.<your-domain>` (or at a sovereign-hosted path on the existing domain). The page reports the live status of every app, database, server, and the last 90 days of incidents. Uptime is calculated from the doctor output (per app, per check, per day). Incidents from H18 are public with the operator's opt-in. **No third-party service** (no StatusPage.io at $29/mo, no Uptime Kuma as a separate app).

**Why it matters:** Per `user-pain-research.md` §5.4: "Status page as a separate product" is severity 5/10 but **automation potential 9/10**. Every serious product (Plausible, Bitwarden, Stripe, GitHub) has a public status page; a solo developer with a $6 Hetzner box usually does not. Building it as a sovereign subcommand (a static page rendered from our state) is 2 days of work, and it removes one more "I would set this up if I had a weekend" excuse. The page is also a credibility signal: a public uptime of 99.95% over 90 days is worth more than a tweet.

### Sub-tasks

1. **Implement the `StatusPagePort` trait** in `sovereign-core/src/ports/status_page.rs`:
   ```rust
   #[async_trait]
   pub trait StatusPagePort: Send + Sync {
       async fn render(&self, period: StatusPeriod) -> Result<StatusPage, StatusPageError>;
       async fn publish(&self, page: &StatusPage, target: &PublishTarget) -> Result<(), StatusPageError>;
   }
   pub struct StatusPage { pub overall: StatusLevel, pub components: Vec<ComponentStatus>, pub incidents: Vec<IncidentSummary>, pub uptime_90d: f64, pub period: (DateTime, DateTime) }
   ```
2. **Implement the data aggregation** in `sovereign-status-page/`:
   - Per-component uptime: for each (app, db, server), compute uptime per day over the last 90 days from the `doctor --watch` event log (or, if no `doctor` history, from the deploy + health-check event log).
   - Per-day overall status: the worst component's status is the day's status.
   - 90-day rolling uptime: `days_up / 90`.
   - Active incidents: from the `incident` table (H18) where `status != 'resolved'`.
   - Resolved incidents in the period: from the same table, with their `summary`.
3. **Render the static page** as a single self-contained HTML file (no JS framework, no CDN, no external fonts — sovereignty: the page works on an air-gapped box). Use the same `maud` templating as the rest of the binary.
4. **Implement the CLI** in `crates/sovereign/src/cmd/status_page.rs`:
   - `sovereign status page render --period 90d --out /var/lib/sovereign/status-page.html` — renders the static HTML.
   - `sovereign status page publish --target serve` — starts a tiny Caddy-managed HTTPS endpoint at `status.<your-domain>` that serves the static page + a JSON API for live status.
   - `sovereign status page publish --target s3 --bucket <name>` — uploads to S3-compatible storage (for users who want the page hosted by a CDN).
   - `sovereign status page subscribe --email <addr>` — sends a "status changed" email on each incident declare/resolve (uses the user's SMTP config; or integrates with Resend/Postmark if configured).
5. **Implement the public JSON API** at `status.<your-domain>/api/v1/status`:
   - Returns the current `StatusPage` as JSON.
   - Used by the operator's own dashboard, by uptime monitors, and by the embedded `iframable` status badge.
6. **Implement the status badge** (the small "100% uptime" / "99.9% uptime" image you see on GitHub READMEs):
   - `![status](https://status.<your-domain>/badge.svg)` — a static SVG that shows the rolling 90-day uptime as a colored pill (green > 99.5%, yellow > 99%, red below).
   - Generated on every render; cached at the CDN.
7. **Integrate with the doctor and incident toolkit**:
   - Every `doctor` state change (`Pass → Fail` on a check) updates the page's "live status" JSON.
   - Every `incident declare/note/resolve` updates the page's incidents list.
   - The morning report (G11) includes a 1-line "Status page: live at status.<your-domain>, 99.97% uptime 90d."
8. **Privacy**: the status page exposes component names, but **never** error messages, logs, or user data. The public `summary` field on `incident` is the only incident text shown (the internal `notes` are not).
9. **Wire the status page into the doctor** (G16 standard level):
   - `status_page_published` — a status page is configured and live (or the operator has explicitly opted out via `[status_page] enabled = false` in `sovereign.toml`).
   - `status_page_uptime_90d` — the rolling 90-day uptime is ≥ 99.5% (warn if < 99%, fail if < 95%).
10. **Add a CI test**: a fixture with 30 days of doctor + incident history renders the page; the HTML is valid; the JSON API returns the expected structure; the badge SVG renders.

### Code stub

```rust
// crates/sovereign-status-page/src/lib.rs
use askama::Template;
#[derive(Template)]
#[template(path = "status_page.html")]
pub struct StatusPageTemplate {
    pub title: String,
    pub overall: String,
    pub overall_color: String,
    pub uptime_90d: f64,
    pub components: Vec<ComponentRow>,
    pub incidents: Vec<IncidentRow>,
    pub period_label: String,
    pub last_updated: chrono::DateTime<chrono::Utc>,
}
pub struct ComponentRow { pub name: String, pub status: String, pub uptime_90d: f64, pub description: String }
pub struct IncidentRow { pub title: String, pub started_at: chrono::DateTime<chrono::Utc>, pub resolved_at: Option<chrono::DateTime<chrono::Utc>>, pub summary: Option<String> }
```

```html
<!-- templates/status_page.html -->
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>{{ title }} — Status</title>
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <style> /* inline, no CDN, no external font */ </style>
</head>
<body>
  <header>
    <h1>{{ title }}</h1>
    <p class="overall {{ overall_color }}">● {{ overall }}</p>
    <p>90-day uptime: <strong>{{ "%.2f"|format(uptime_90d) }}%</strong></p>
    <p>Last updated: {{ last_updated }}</p>
  </header>
  <main>
    <h2>Components</h2>
    <table>{% for c in components %}
      <tr><td>{{ c.name }}</td><td class="{{ c.status }}">{{ c.status }}</td><td>{{ "%.2f"|format(c.uptime_90d) }}%</td></tr>{% endfor %}
    </table>
    <h2>Recent incidents</h2>
    <ul>{% for i in incidents %}
      <li><strong>{{ i.title }}</strong> — {{ i.started_at }} {% if let Some(r) = i.resolved_at %}– resolved {{ r }}{% endif %}{% if let Some(s) = i.summary %}: {{ s }}{% endif %}</li>{% endfor %}
    </ul>
  </main>
</body>
</html>
```

### Tests

- Unit: `render` with a fixture of 30 days of doctor + incident history produces the expected HTML.
- Unit: the JSON API returns the expected structure.
- Unit: the badge SVG renders the correct color and percentage.
- Integration: a fixture with 99% uptime over 90 days renders a yellow pill; 99.5% renders green; 95% renders red.
- Integration: `sovereign status page publish --target serve` starts the HTTPS endpoint; a `curl` returns the expected HTML.
- Privacy: the public HTML never contains error messages, logs, or user data.
- Doctor: the 2 new checks (`status_page_published`, `status_page_uptime_90d`) pass on a healthy fixture.
- CI gate: a job renders the page on every release PR; the HTML diff is reviewed for unexpected changes.

### Acceptance criteria

- [ ] `sovereign status page render` produces a self-contained HTML file.
- [ ] `sovereign status page publish --target serve` serves the page at `status.<your-domain>`.
- [ ] The JSON API at `/api/v1/status` returns the current state.
- [ ] The badge SVG at `/badge.svg` shows the rolling 90-day uptime.
- [ ] Incidents (declared + resolved) appear on the page.
- [ ] The 2 new doctor checks pass on a healthy fixture.

### Definition of done

A 4-week soak: 5 design partners enable the status page. ≥ 4 of 5 link the badge on their GitHub README within a week. The `status_page_uptime_90d` doctor check is the on-call's "did we actually meet SLO" tripwire — the check turns red *before* a customer reports downtime.

---

## 1. Definition of Done (the phase gate)

Phase 2 closes when **all** of these are true:

- [ ] All 23 features (H1-H23) are merged to `main` with passing CI.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo llvm-cov --workspace` reports ≥ 80% line coverage.
- [ ] 1 customer has a 3-server fleet running in production for ≥ 30 days.
- [ ] 0 unscheduled downtimes across all V1.5 design partners.
- [ ] ≥ 5 team customers (3+ engineers) are active.
- [ ] Audit log is queryable for 90 days.
- [ ] `sovereign doctor --level full` returns 0 fails on the fleet.
- [ ] `sovereign doctor fleet --summary` runs in < 90s on a 3-server fleet.
- [ ] `sovereign incident declare/note/resolve` is used in ≥ 80% of real outages.
- [ ] `sovereign game-day run-all` passes all 8 scenarios in CI.
- [ ] `sovereign import --from heroku` migrates a real Heroku app in < 30 min for ≥ 4 of 5 design partners.
- [ ] `sovereign dns` works for all 7 providers; `sovereign domain add` auto-creates the A/AAAA record.
- [ ] `sovereign db pool` survives 200 concurrent connections; the 3 new doctor checks pass.
- [ ] `sovereign status page` is published for ≥ 4 of 5 design partners; the badge appears on their GitHub README.
- [ ] GmbH is registered; OÜ is registered.
- [ ] The 10-point sovereignty test runs in CI.
- [ ] The Apache 2.0 LICENSE is unchanged.
- [ ] The version is bumped to `1.5.0`.

**If any of these is false, the phase is not done.**

---

## 2. What we explicitly do not build in Phase 2

- rqlite HA (V3, not V1.5) — agent pattern is the staging ground
- OPA/Rego (V2)
- ML scorer (V2)
- Web UI (V2)
- OIDC, MFA (V2)
- Pro tier billing (V2)
- BSI C5 mapping (V2)
- True canary (V2.5+)
- Multi-region (V3+)
- Service mesh (never)
- K8s (never)
- gRPC / WebSockets (never)
- Doctor `--level paranoid` (V2)
- Doctor compliance scan, canary integration, auto-tune (V2)
- Doctor canary (`sovereign canary`) — V2.5+
- Doctor fleet auto-rebalance — V3+
- Import from Dokku, Kamal, Fly.io (V2; V1.5 covers Heroku, Coolify, Dokploy, Render)

---

**Next: read [`phase-03-v2.md`](./phase-03-v2.md) for Months 10-18, which adds rqlite HA, OPA/Rego, ML scorer, the doctor paranoid level + compliance scan + canary + auto-tune.**
