# Phase 0.6 — The Hosting Control Plane (V0.6 → V1.0)

**Timeline:** ~6 months (24 weeks). Sub-phases 0.6.1 → 0.6.6 below.
**Goal:** Add a system-service management layer (apt-managed
nginx, MySQL, vsftpd, phpMyAdmin, Let's Encrypt) on top of
the V0 deployment runtime, expose it through a multi-tenant
RBAC + OIDC layer, and ship a per-user Telegram chatops
adapter so each end-user (and the operator) can drive the
host from a phone. V1.0 ships the first multi-tenant design
partner.
**Audience:** the lead engineer, 2-4 contributors, 1-2
multi-tenant design partners, 1 C5/ISO auditor.
**Definition of Done:** 50 installs/week, 10 active
production users (mix of operator + multi-tenant),
time-to-first-deploy < 5 min for the 6 golden paths AND
time-to-first-telegram-command < 10 min, 1 public BSI C5
case study in progress, `sovereign doctor --level standard`
returns 0 fails, the 13-gate V0 close-out checklist is
**still green** (V0.6 must not regress V0).

---

## 0. The sub-phase plan

```text
0.6.1 (3 weeks)   S1 — sovereign-systemd crate: 7 service
                   adapters (nginx, mysql, mariadb, redis,
                   vsftpd, letsencrypt, phpmyadmin),
                   apt+systemd wrappers, `sovereign service`
                   CLI. Single-tenant only (operator = the
                   only user).
0.6.2 (2 weeks)   S2 — sovereign-le: Let's Encrypt issue
                   + renew + daily renewal timer; wire to
                   Caddy routes and nginx vhosts.
0.6.3 (6 weeks)   S3 — sovereign-auth crate: user accounts,
                   argon2id passwords, api tokens, RBAC,
                   OIDC RP (Google + GitHub), bootstrap
                   admin, pluggable storage (sqlite default,
                   Postgres opt-in).
0.6.4 (4 weeks)   S4 — sovereign-chatops crate: Telegram
                   long-polling loop, 6-digit `/start` code
                   binding, 5 read-only commands, audit
                   `actor = "user:<id> via telegram:<chat>"`.
0.6.5 (3 weeks)   S5 — Mutating chatops commands, per-user
                   phpMyAdmin nginx vhost, FTP per-app jail
                   with one-time tokens, inline-keyboard
                   confirmation step, rate limit.
0.6.6 (6 weeks)   S6 — V1.0 hardening: pen-test the
                   chatops security model, multi-tenant
                   design partner, backup-of-the-backup,
                   1.0.0 release.
```

Each sub-phase (S1-S6) is described with: **Goal → Why →
Sub-tasks → Code stubs → Tests → Acceptance criteria →
Definition of done**.

---

## 0.5. Locked decisions (V0.6)

These were locked in the planning phase and are not
re-decided here:

| Decision | Value | Source |
|---|---|---|
| Distro support for system services | **Ubuntu 22.04 LTS only** | planning Q1 |
| Credential storage | **Pluggable** (sqlite default, Postgres opt-in via a `StoragePort` trait that both backends implement) | planning Q2 |
| Telegram bot ↔ user binding | **One-time 6-digit code** (`/start 482913`) | planning Q3 |
| Tag sequencing | **v0.1.0 (V0 close-out) → v0.6.0 (S1-S5 done) → v1.0.0 (S6 done)** | planning Q4 |
| Telegram audience | **End-users, multi-tenant** (each user has their own bot) | planning Q1 |
| Service packaging | **All-as-system-services** (apt + systemd; breaks V0's container-first rule for these specific services) | planning Q2 |
| Initial Telegram command surface | **Full cPanel** (read + restart + service install/remove + LE + phpMyAdmin + FTP + user mgmt) | planning Q3 |
| V0 close-out ordering | **Land V0 first**, then start V0.6 | planning Q4 |

### 0.5.1. What's NOT in V0.6 (deferred or rejected)

These are scope guards; if a sub-phase proposal would
require one of these, push back to the planning phase.

- **RHEL family / Rocky / Alma** — Ubuntu 22.04 LTS only.
  Adding a second distro family roughly doubles the S1
  effort (dnf vs apt + SELinux vs AppArmor).
- **System Nginx as the primary app proxy** — Caddy still
  fronts the app fleet. System Nginx is for phpMyAdmin +
  static-file vhosts only.
- **Non-Telegram chat channels** (Slack, Discord, Signal,
  iMessage) — Telegram only. Other chat surfaces can
  reuse the same chatops port; the design leaves room for
  them, but no adapters ship in V0.6.
- **Voice / phone alerts / IVR** — out of scope.
- **Multi-tenant billing / metering** — out of scope.
  Multi-tenant chatops, not multi-tenant SaaS.
- **Shared file systems / NFS / clustered FS** — single
  host per install. Multi-host is V1.5+.
- **HA / failover / rqlite** — single host, no clustering.
  Backups are the only DR story.
- **OIDC provider beyond Google + GitHub** — V0.6 ships
  two; Authentik / Keycloak / Okta / Entra ID land in
  V1 (G21 in `docs/phase-01-v1.md`).

---

## S1. `sovereign-systemd` — system service management

### Goal

Sovereign can `sovereign service install \| remove \| list
\| status \| restart <KIND>` for 7 system services (nginx,
mysql, mariadb, redis, vsftpd, letsencrypt, phpmyadmin).
Each `install` is idempotent, audited, and recoverable.
Sovereign is still single-tenant in S1 (operator = the only
user); S3 (sovereign-auth) is what adds the multi-tenant
model on top.

### Why

The V0 deploys containerized apps behind Caddy. But the
adjacent LAMP-stack services — nginx (for phpMyAdmin +
static), MySQL/MariaDB (for phpMyAdmin + the system-level
"give me a database" story), vsftpd (for legacy file
transfer), Let's Encrypt (for the system nginx vhosts) —
are system services, not containers. The operator wants one
`sovereign` binary to manage all of them, not a parallel
`apt + systemctl + certbot` workflow.

### Sub-tasks

1. **Define the `SystemServicePort` trait** in
   `crates/sovereign-core/src/ports/system_service.rs`:
   ```rust
   #[async_trait]
   pub trait SystemServicePort: Send + Sync {
       fn kind(&self) -> ServiceKind;
       async fn install(&self, spec: ServiceInstallSpec) -> Result<ServiceState, AppError>;
       async fn remove(&self, purge_config: bool) -> Result<(), AppError>;
       async fn status(&self) -> Result<ServiceStatus, AppError>;
       async fn restart(&self) -> Result<(), AppError>;
       async fn config(&self) -> Result<ServiceConfig, AppError>;
   }
   ```
2. **Implement the 7 adapters** in
   `crates/sovereign-systemd/src/adapters/`:
   - `apt.rs` — wraps `apt-get install -y`, `apt-get remove
     -y [--purge]`, `apt-cache policy` (for version pin),
     exit-code capture, idempotency check
   - `systemd.rs` — wraps `systemctl enable --now`, `systemctl
     disable --now`, `systemctl status`, `systemctl restart`,
     unit-file writes
   - `nginx.rs` — config under `/etc/nginx/conf.d/sovereign-*.conf`,
     `nginx -t` syntax check, `systemctl reload nginx`
   - `mysql.rs` — `mysql_secure_installation` non-interactive
     wrapper, root password rotation, `mysql -e "SELECT 1"`
     health check
   - `mariadb.rs` — same shape as `mysql.rs` with
     `mariadb-server` package name
   - `redis.rs` — `redis-cli ping` health check, optional
     `requirepass` write
   - `vsftpd.rs` — vsftpd.conf template writer, per-app
     jail directory creator, PAM-based virtual user
   - `letsencrypt.rs` — `certbot certonly --nginx
     --non-interactive --agree-tos` wrapper; this is
     expanded in S2
   - `phpmyadmin.rs` — nginx vhost writer, php-fpm
     dependency check, config.inc.php writer
3. **Define the audit event** in
   `crates/sovereign-core/src/domain/audit.rs`:
   `AuditKind::SystemService` with `payload = { kind,
   action, exit_code, sha256_config, duration_ms }`.
4. **Wire the CLI** in
   `crates/sovereign/src/commands_service.rs`:
   ```
   sovereign service install <KIND> [--version V] [--no-start]
   sovereign service remove  <KIND> [--purge-config]
   sovereign service list
   sovereign service status  <KIND>
   sovereign service restart <KIND>
   sovereign service config  <KIND>      # prints the live config
   ```
5. **Refuse non-Ubuntu at install time** with a clear
   error and a `sovereign doctor --level basic` check that
   emits a `Fail` on RHEL/Debian/Alpine (the other distros
   planned for the app fleet continue to work; only system
   services are gated).
6. **Document the security model** in
   `docs/security/system-service-isolation.md`: the
   `sovereign` binary needs root (or `sudo` capability) to
   apt-install; the operator's `install.sh` adds the
   `cap_dac_read_search,cap_dac_override,cap_chown` caps
   to the binary so the systemd unit doesn't need to run
   as root. Audit the binary's effective capabilities on
   every install.

### Code stub

```rust
// crates/sovereign-systemd/src/adapters/nginx.rs
use crate::adapters::apt::AptAdapter;
use crate::adapters::systemd::SystemdAdapter;
use sovereign_core::error::AppError;
use sovereign_core::ports::system_service::{
    ServiceInstallSpec, ServiceState, ServiceStatus, SystemServicePort,
};

pub struct NginxAdapter {
    apt: AptAdapter,
    systemd: SystemdAdapter,
}

impl NginxAdapter {
    pub fn new() -> Self {
        Self {
            apt: AptAdapter::new("nginx"),
            systemd: SystemdAdapter::new("nginx"),
        }
    }
}

#[async_trait]
impl SystemServicePort for NginxAdapter {
    fn kind(&self) -> ServiceKind { ServiceKind::Nginx }

    async fn install(&self, spec: ServiceInstallSpec) -> Result<ServiceState, AppError> {
        // Idempotent: skip if already installed at the right version.
        if self.systemd.is_active().await? && spec.version.is_none() {
            return Ok(ServiceState::already_installed());
        }
        let pkg = spec.version
            .as_deref()
            .map(|v| format!("nginx={v}"))
            .unwrap_or_else(|| "nginx".to_string());
        let install = self.apt.install(&pkg).await?;
        self.systemd.enable_now().await?;
        // Verify: `nginx -t` and `curl localhost` should return 200.
        self.systemd.reload().await?;
        let syntax = self.apt.shell("nginx -t").await?;
        if !syntax.success() {
            return Err(AppError::upstream(format!(
                "nginx installed but `nginx -t` failed: {}",
                syntax.stderr_str()
            )));
        }
        Ok(ServiceState::installed_at(install.version()))
    }
    // remove, status, restart, config omitted for brevity
}
```

### Tests

- **Unit (10 per adapter × 7 adapters = 70)**: `apt.rs`
  and `systemd.rs` are wrapped in a trait, and tests use a
  mock that records every call and returns scripted
  outputs. Each adapter has 10 unit tests covering the
  happy path, the "already installed" idempotency case,
  the version pin path, the "apt exit non-zero" failure
  path, the "nginx -t fails" failure path, etc.
- **Integration (6)**: end-to-end on a real Ubuntu 22.04
  CX22: install nginx, install mysql, install
  phpmyadmin (with the php-fpm dependency), remove
  phpmyadmin, remove mysql, list.
- **Security (2)**: verify the binary's effective
  capabilities are the documented set, and that
  `sovereign service install` refuses to run with extra
  caps (defense against the operator adding `cap_sys_admin`
  "to make it work").

### Acceptance criteria

- [ ] `sovereign service install nginx` on a fresh
      Ubuntu 22.04 CX22 finishes in < 60s.
- [ ] `sovereign service list` shows 7 known service
      kinds in alphabetical order.
- [ ] `sovereign service install nginx` run twice in a
      row returns "already installed" with the original
      version; no apt re-install happens.
- [ ] `sovereign service remove mysql --purge-config`
      stops the unit, disables it, removes the package,
      and removes `/etc/mysql/`.
- [ ] Every install/remove/restart writes a
      `SystemService` audit event with exit code + SHA-256
      of the post-state config.
- [ ] Non-Ubuntu hosts get a clear error: "system
      services require Ubuntu 22.04 LTS; this host is
      `<distro>`".

### Definition of done

- 70 unit + 6 integration + 2 security tests pass.
- `cargo fmt --all -- --check`, `cargo clippy
  --workspace --all-targets --no-default-features -- -D
  warnings`, `cargo deny check`, `cargo test --workspace
  --no-default-features` all green.
- The 13 V0 gates are still green.
- `docs/security/system-service-isolation.md` is committed
  and reviewed.
- One operator has run S1 end-to-end on a real CX22
  and committed the evidence.

---

## S2. `sovereign-le` — Let's Encrypt for system services

### Goal

`sovereign service le issue --domain <host> --service
nginx|phpmyadmin|caddy` obtains a real LE cert via
certbot, installs it into the named service's config,
and registers a daily renewal systemd timer.

### Why

Caddy already does auto-TLS for the app fleet (Sovereign's
V0.6 itself fronts the apps with Caddy). But the system
nginx vhosts (phpMyAdmin in S5, any user-added static
vhost) need real LE certs. The operator wants one CLI
command, not a hand-rolled certbot invocation.

### Sub-tasks

1. **Extend `letsencrypt.rs`** (started in S1) with
   `issue(domain, service)` and `renew(domain)`.
2. **Wire to Caddy**: when an LE cert is issued for a
   domain that Caddy is also proxying, Caddy is told to
   use the LE cert instead of `tls internal` (Caddy's
   `on_demand_tls` flow handles this).
3. **Daily renewal timer**: write
   `/etc/systemd/system/sovereign-le-renew.timer` and
   `/etc/systemd/system/sovereign-le-renew.service` with
   `OnCalendar=daily` and `Persistent=true` (so missed
   runs are caught up). The service body calls
   `sovereign service le renew --all`.
4. **HTTP-01 challenge proxy**: certbot's `--nginx`
   plugin needs port 80 open. The system nginx config
   includes a `location ^~ /.well-known/acme-challenge/
   { allow all; }` block that points at certbot's
   webroot.
5. **DNS-01 challenge for wildcard certs**: certbot's
   `--dns-cloudflare` / `--dns-route53` plugins for
   `*.example.com` certs. Configurable per-domain.
6. **Tests**: 8 unit (certbot CLI argument construction,
   config-writer, timer-writer) + 4 integration
   (certbot `--staging` mode against a real domain the
   operator controls; the staging endpoint returns
   trusted test certs without rate-limiting the operator).

### Acceptance criteria

- [ ] `sovereign service le issue --domain
      pma.example.com --service phpmyadmin` on a real
      domain with a real DNS A record finishes in < 30s
      and the cert is in `/etc/letsencrypt/live/`.
- [ ] The cert is loaded by the system nginx vhost
      (verified with `nginx -T 2>&1 | grep ssl_certificate`).
- [ ] `systemctl list-timers sovereign-le-renew.timer`
      shows the daily timer enabled and active.
- [ ] Dry-run renewal (`certbot renew --dry-run`) shows
      0 failures for all certs issued by Sovereign.
- [ ] The Cloudflare and Route 53 DNS-01 plugins are
      configurable via `sovereign service le dns set
      --provider cloudflare --token <t>`.

### Definition of done

- 8 unit + 4 integration tests pass.
- One operator has issued a real cert and committed the
  evidence (the cert SHA-256, the nginx -T output, the
  timer status).
- `docs/operations/le-renewal.md` is committed and reviewed.

---

## S3. `sovereign-auth` — user accounts, RBAC, OIDC

### Goal

Sovereign has a real user model. `sovereign user add /
list / disable`, `sovereign token create / list /
revoke`, a role hierarchy (admin > operator > viewer),
a scope-based RBAC enforcement point, and OIDC RP
support for Google + GitHub. Bootstrap admin: the
first `sovereign user add` after install auto-becomes
admin if no admin exists.

### Why

S4 (Telegram chatops) needs a user identity to bind the
Telegram chat to. S5 (per-user phpMyAdmin + FTP) needs
per-user authorization. The V0 "the operator" model is
fine for one person, but as soon as a second human can
issue `sovereign deploy`, the codebase needs a real
trust model.

### Sub-tasks

1. **New migration 0006**:
   ```sql
   CREATE TABLE user (
       id            BLOB PRIMARY KEY,         -- UserId
       email         TEXT NOT NULL UNIQUE,
       display_name  TEXT NOT NULL,
       password_hash TEXT,                     -- argon2id; null for OIDC-only users
       role          TEXT NOT NULL CHECK (role IN ('admin','operator','viewer')) DEFAULT 'viewer',
       created_at    INTEGER NOT NULL,
       disabled_at   INTEGER,
       telegram_chat_id INTEGER,                -- bound by S4
       oidc_subject  TEXT                      -- bound by S3.7
   );
   CREATE TABLE api_token (
       id            BLOB PRIMARY KEY,
       user_id       BLOB NOT NULL REFERENCES user(id),
       name          TEXT NOT NULL,
       hash          TEXT NOT NULL,            -- sha256(token), never stored plaintext
       scopes        TEXT NOT NULL,            -- CSV: "deploy,secret.read,service.install"
       created_at    INTEGER NOT NULL,
       expires_at    INTEGER
   );
   CREATE TABLE bootstrap_state (
       id            INTEGER PRIMARY KEY CHECK (id = 1),  -- singleton
       admin_user_id BLOB REFERENCES user(id)
   );
   ```
2. **New crate `crates/sovereign-auth/`** with:
   - `password.rs` — argon2id hash + verify. **Re-uses
     the same KDF the master key uses** (64 MiB / t=3 /
     p=1) so we don't ship a second KDF.
   - `token.rs` — opaque 32-byte token generation
     (base62-encoded), sha256 hash, constant-time
     compare.
   - `oidc.rs` — minimal OIDC RP (authorization code +
     PKCE). Supports Google and GitHub for V0.6; the
     adapter trait is open so Authentik/Keycloak/Okta/Entra
     ID slot in V1.
   - `rbac.rs` — `User.can(action, resource) -> bool`,
     where `action` is one of the canonical verbs
     (`deploy`, `secret.read`, `service.install`,
     `phpmyadmin.view`, `ftp.grant`, `user.add`, ...) and
     `resource` is the app or service name (or `*`).
   - `pluggable_storage.rs` — the `UserStore` and
     `TokenStore` traits, with two backends: `SqliteUserStore`
     (default) and `PostgresUserStore` (V0.6+ opt-in).
3. **Pluggable storage wiring**: extend
   `crates/sovereign-core/src/ports/storage.rs` with
   `user_store(): Arc<dyn UserStore>` and `token_store():
   Arc<dyn TokenStore>`. The default
   `sovereign_storage_sqlite::SqliteState` implements
   both. A future `sovereign_storage_postgres` (post-V0.6)
   implements both too. Selection is via the new
   `sovereign.toml` config file (S3.5).
4. **`sovereign.toml` config file** in
   `/etc/sovereign/sovereign.toml` (Linux) /
   `~/Library/Application Support/sovereign/sovereign.toml`
   (macOS) / `%APPDATA%\sovereign\sovereign.toml`
   (Windows). Schema:
   ```toml
   [storage]
   backend = "sqlite"           # or "postgres"
   path    = "/var/lib/sovereign/sovereign.db"
   # when backend = "postgres":
   # url = "postgres://user:pw@host:5432/sovereign"
   [auth]
   bootstrap_admin_email = ""   # operator fill-in on first run
   oidc_google_client_id  = ""
   oidc_google_secret     = ""
   oidc_github_client_id  = ""
   oidc_github_secret     = ""
   [chatops]
   telegram_default_token = ""  # operator fill-in if running a shared bot
   ```
5. **CLI surface**:
   ```
   sovereign user add    <email> [--password | --oidc <google|github>] [--role <ROLE>]
   sovereign user list   [--role <ROLE>]
   sovereign user disable <email>
   sovereign user enable  <email>
   sovereign user whoami             # for the current auth context
   sovereign token create <NAME> --scopes <CSV> [--ttl 30d]
   sovereign token list
   sovereign token revoke <NAME>
   ```
6. **RBAC enforcement point** at every existing
   `sovereign-core` use case: `use_cases::deploy`,
   `use_cases::secret`, `use_cases::service`, etc., all
   take a new first arg `actor: &Actor` where
   `Actor = User | Operator | System`. The use case body
   starts with `self.rbac.check(actor, action,
   resource)?;`. The CLI passes `actor = Operator` for
   the V0 single-tenant commands; the chatops adapter
   passes `actor = User { id, scopes }`.
7. **Bootstrap admin flow**:
   - On first run, if `bootstrap_state` is empty AND
     `sovereign.toml` has `bootstrap_admin_email`, the
     installer creates that user as admin and writes a
     one-time `BOOTSTRAP_TOKEN` to the install log. The
     operator uses that token to set their password.
   - If no `bootstrap_admin_email` is set, the operator
     is prompted on first `sovereign user add` to
     bootstrap the admin.
   - The bootstrap flow is idempotent: re-running it on
     an installed system is a no-op.
8. **Tests**: 30 unit (password, token, RBAC matrix, OIDC
   PKCE flow with a mock OIDC provider) + 8 integration
   (end-to-end user add → token create → token-authed
   `sovereign deploy` against a real but locked-down
   test app).

### Code stub

```rust
// crates/sovereign-auth/src/rbac.rs
use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Action {
    AppDeploy, AppRollback, AppDelete,
    SecretRead, SecretSet, SecretRotate,
    ServiceInstall, ServiceRemove, ServiceRestart,
    PhpMyAdminView, FtpGrant,
    UserAdd, UserDisable, UserList,
    TokenCreate, TokenRevoke,
    ChatopsBind, ChatopsExec,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Role {
    Admin, Operator, Viewer,
}

impl Role {
    pub fn scope_set(&self) -> HashSet<Action> {
        use Action::*;
        match self {
            Role::Admin    => HashSet::from_iter([
                AppDeploy, AppRollback, AppDelete,
                SecretRead, SecretSet, SecretRotate,
                ServiceInstall, ServiceRemove, ServiceRestart,
                PhpMyAdminView, FtpGrant,
                UserAdd, UserDisable, UserList,
                TokenCreate, TokenRevoke,
                ChatopsBind, ChatopsExec,
            ]),
            Role::Operator => HashSet::from_iter([
                AppDeploy, AppRollback,
                SecretRead, SecretSet, SecretRotate,
                ServiceRestart,
                PhpMyAdminView, FtpGrant,
                TokenCreate, TokenRevoke,
                ChatopsBind, ChatopsExec,
            ]),
            Role::Viewer   => HashSet::from_iter([
                SecretRead, PhpMyAdminView,
                UserList, ChatopsBind,
            ]),
        }
    }
}

pub fn check(actor: &Actor, action: Action) -> Result<(), RbacError> {
    let allowed = match actor {
        Actor::Operator => Role::Admin.scope_set(),
        Actor::User { role, .. } => role.scope_set(),
        Actor::System   => Role::Admin.scope_set(),
    };
    if !allowed.contains(&action) {
        return Err(RbacError::forbidden(actor, action));
    }
    Ok(())
}
```

### Acceptance criteria

- [ ] `sovereign user add alice@example.com --role
      admin` creates the user; the password (or OIDC
      subject) is required.
- [ ] `sovereign token create ci-deploy --scopes
      app.deploy,secret.read` returns a one-time token
      that the operator can `export SOVEREIGN_TOKEN=...`
      and use in scripts.
- [ ] Every existing CLI command (`sovereign deploy`,
      `sovereign secret set`, `sovereign service
      install`, ...) works unchanged when run as
      `Actor::Operator` (V0 single-tenant behavior).
- [ ] A user with `Role::Viewer` trying to `sovereign
      service install nginx` gets `RbacError::forbidden`
      with the missing scopes enumerated.
- [ ] `sovereign.toml` reloads on SIGHUP without a
      restart; missing file = `Operator` is the only
      actor, all admin commands still work.

### Definition of done

- 30 unit + 8 integration tests pass.
- The 13 V0 gates are still green.
- The V0 CLI commands are byte-for-byte unchanged when
  run as the V0 operator.
- `docs/security/rbac-model.md` is committed and
  reviewed.
- One operator has run the bootstrap admin flow on a
  fresh install and committed the evidence.

---

## S4. `sovereign-chatops` — Telegram bot (read-only)

### Goal

Each end-user (and the operator) can drive a Telegram bot
linked to their Sovereign user account. The bot is
single-user, single-tenant at the chat level: each user
runs their own bot, scoped to their apps. The
read-only command set (`/status`, `/apps`, `/logs`,
`/doctor`, `/secret list`) ships in S4; the mutating
set ships in S5.

### Why

The operator already has the CLI. The end-user
(especially in the multi-tenant case) often doesn't have
shell access. A Telegram bot bound to their Sovereign
user account gives them "is my app up?" + "restart my
app" without giving them SSH.

### Sub-tasks

1. **New crate `crates/sovereign-chatops/`**:
   - `telegram.rs` — long-polling loop
     (`getUpdates` → handle → `sendMessage`). Uses
     `reqwest` + `tokio::time::interval`. Backoff on
     429.
   - `commands.rs` — the command parser. Each command
     is a `trait ChatopsCommand { fn name(&self) ->
     &'static str; async fn run(&self, ctx: &Ctx,
     args: &Args) -> Result<String, ChatopsError>; }`
     impl.
   - `policy.rs` — RBAC check. Same `rbac.rs` from S3.
   - `formatter.rs` — Telegram MarkdownV2 formatter
     (escape special chars, code blocks, etc.).
   - `audit.rs` — every bot action logs an audit event
     with `actor = "user:<id> via telegram:<chat_id>"`.
2. **New systemd unit**:
   `/etc/systemd/system/sovereign-chatops.service` —
   the operator runs `sovereign chatops init --user
   <email>` which (a) generates a bot token via
   @BotFather, (b) writes the systemd unit with the
   token, (c) starts it.
3. **6-digit binding**:
   - `sovereign chatops init --user me@example.com`
     prints a 6-digit code (e.g. `482913`) and a
     deeplink `https://t.me/<bot>?start=482913`.
   - User clicks the deeplink → Telegram delivers
     `/start 482913` to the bot.
   - Bot looks up the code in
     `chatops_binding_code` (a transient table that
     holds a 5-minute TTL), finds the user, and writes
     `telegram_chat_id` to the user row.
   - Subsequent messages from the same chat_id are
     treated as the bound user.
4. **Read-only commands** (S4 ships these):
   - `/status` — fleet + last deploy + last error
   - `/apps` — list of apps the user has access to
   - `/logs <app>` — last 20 lines (paginated with
     inline keyboard)
   - `/doctor` — current doctor summary
   - `/secret list <app>` — list of secret keys, NEVER
     values
5. **Tests**: 24 unit (command parser, RBAC, formatter,
   6-digit code TTL) + 8 integration (mock Telegram
   transport, end-to-end `/status` against a real
   Sovereign install).
6. **Documentation**:
   - `docs/operations/chatops-onboarding.md` — operator
     runbook for the 6-digit binding flow.
   - `docs/security/chatops-model.md` — the trust model
     (6-digit code is the proof of Telegram-account
     ownership at binding time; the bot trusts the
     chat_id after binding; revoking a binding is
     `sovereign chatops revoke --user <email>`).

### Code stub

```rust
// crates/sovereign-chatops/src/telegram.rs
use reqwest::Client;
use std::time::Duration;
use tokio::time::sleep;

pub struct TelegramPoller {
    token: String,
    api: String,
    client: Client,
    on_update: Box<dyn Fn(Update) + Send + Sync>,
}

impl TelegramPoller {
    pub fn new<F>(token: String, on_update: F) -> Self
    where F: Fn(Update) + Send + Sync + 'static {
        Self {
            api: format!("https://api.telegram.org/bot{token}"),
            token,
            client: Client::new(),
            on_update: Box::new(on_update),
        }
    }

    pub async fn run_forever(self) {
        let mut offset: i64 = 0;
        loop {
            let updates = match self.fetch_updates(offset, 30).await {
                Ok(u) => u,
                Err(e) if e.is_rate_limit() => {
                    tracing::warn!("telegram 429: {:?}", e.retry_after());
                    sleep(Duration::from_secs(e.retry_after().unwrap_or(5))).await;
                    continue;
                }
                Err(e) => {
                    tracing::error!("telegram fetch: {e}");
                    sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };
            for u in updates {
                offset = offset.max(u.update_id + 1);
                (self.on_update)(u);
            }
        }
    }

    async fn fetch_updates(&self, offset: i64, timeout: u64) -> Result<Vec<Update>, TelegramError> {
        let url = format!("{}/getUpdates?offset={offset}&timeout={timeout}", self.api);
        let resp = self.client.get(&url).send().await?.error_for_status()?;
        let body: GetUpdatesResponse = resp.json().await?;
        Ok(body.result)
    }
}
```

### Acceptance criteria

- [ ] `sovereign chatops init --user me@example.com`
      returns a 6-digit code within 2s.
- [ ] Sending `/start 482913` from the user's phone
      binds the chat_id within 5s; subsequent `/status`
      from the same phone returns the user's fleet.
- [ ] `/status` from a different phone (different
      chat_id) returns "not bound".
- [ ] `/secret list <app>` shows the keys, never the
      values, in the reply (regression guard for
      accidental plaintext leak).
- [ ] Every command writes a `Chatops` audit event with
      `actor = "user:<id> via telegram:<chat_id>"`.

### Definition of done

- 24 unit + 8 integration tests pass.
- The 13 V0 gates are still green.
- The V0 CLI commands are unchanged.
- `docs/operations/chatops-onboarding.md` and
  `docs/security/chatops-model.md` are committed and
  reviewed.
- One operator + one design-partner user have run
  the 6-digit binding end-to-end and committed the
  evidence.

---

## S5. Mutating chatops + phpMyAdmin + FTP

### Goal

S5 ships the full cPanel-class command surface on top of
S4. Every mutating command requires an inline-keyboard
confirmation step. Per-user phpMyAdmin vhost. Per-app
FTP jail with one-time tokens. Rate limit (10 mutating
commands per user per hour).

### Sub-tasks

1. **Mutating chatops commands** (S5 ships these):
   - `/app restart <app>` — confirm → call
     `sovereign deploy` with current image
   - `/service status <KIND>` — `systemctl status`
   - `/service restart <KIND>` — confirm → `systemctl
     restart`
   - `/service install <KIND>` — confirm →
     `sovereign service install`
   - `/phpmyadmin link <app>` — confirm → write nginx
     vhost + return URL
   - `/le issue <domain>` — confirm → certbot
2. **Inline-keyboard confirmation**: every mutating
   command replies with a `Confirm / Cancel` keyboard.
   The bot holds the action in memory (or in a 5-minute
   TTL table) keyed by `(user_id, action_id)`. The
   confirm callback calls the action; the cancel
   callback replies "cancelled" and forgets the action.
3. **Per-user phpMyAdmin vhost** at
   `pma.<user-domain>` (default `pma.example.com`):
   - nginx vhost with `auth_basic_user_file
     /etc/nginx/sovereign-pma-<user>.htpasswd`
   - phpMyAdmin config scoped to a single DB
     (the user's database, not `mysql.user` etc.)
   - TLS via S2 (LE)
   - Audit every login to the vhost
4. **Per-app FTP jail** (vsftpd):
   - vsftpd virtual user per `(user, app)`
   - Jail root is `/var/lib/sovereign/ftp/<user>/<app>/`
   - One-time token grant: `/ftp grant <app>` returns a
     username + temporary password that expires in 24h
   - Audit every FTP login
5. **Rate limit**: a `chatops_rate_limit` table holds
   `(user_id, mutating_count, window_start)`. If
   `now - window_start < 1h` AND
   `mutating_count >= 10`, the bot replies "slow down;
   try again in <X>m".
6. **Tests**: 30 unit (confirmation step, rate limit,
   vhost writer, vsftpd config writer, RBAC for each
   mutating action) + 10 integration (end-to-end
   `/app restart` with a real test app, FTP grant +
   login, phpMyAdmin vhost + login).

### Acceptance criteria

- [ ] `/app restart my-app` shows a Confirm / Cancel
      keyboard; the second tap on Confirm restarts the
      app; the second tap on Cancel replies "cancelled".
- [ ] 11 `/app restart my-app` in an hour — the 11th
      gets "slow down; try again in <X>m".
- [ ] `/phpmyadmin link my-app` returns a URL like
      `https://pma.example.com?u=<user>&a=my-app`; the
      URL is single-use and expires in 1h.
- [ ] `/ftp grant my-app` returns `username=<u>` +
      `password=<one-time>`; the next vsftpd login
      succeeds; the same password the next day fails
      (audit-event "token expired").
- [ ] Every mutating action writes a `Chatops` audit
      event with the pre-state + post-state SHA-256.

### Definition of done

- 30 unit + 10 integration tests pass.
- The 13 V0 gates are still green.
- `docs/security/chatops-confirmation.md` is committed
  and reviewed.
- One operator + one design-partner user have run
  each of the 6 mutating commands end-to-end and
  committed the evidence.

---

## S6. V1.0 hardening + first multi-tenant design partner

### Goal

Pen-test the chatops security model, onboard the first
multi-tenant design partner, fix what the pen-test +
design-partner usage finds, ship 1.0.0.

### Why

V0.6 + V0.7 is "we built it". V1.0 is "an external
auditor + a real user with real money have used it and
it didn't fall over".

### Sub-tasks

1. **Penetration test of the chatops security model**:
   - The 6-digit code TTL
   - The rate limit (does it survive 11th-attempt
     from a different chat_id?)
   - The phpMyAdmin vhost (can the user read other
     users' databases? can they read `mysql.user`?)
   - The FTP jail (can the user escape the jail via
     symlink? can they read `/etc/passwd`?)
   - The audit log (can a user delete or modify their
     own audit events? can they insert fake ones?)
2. **Multi-tenant design partner**:
   - 1 external company with 2-5 users
   - Each user has their own bot, their own app
   - We dogfood their bugs for 4 weeks
3. **Backup-of-the-backup**: every chatops action goes
   through the same audit-event path; the audit log is
   backed up via the existing F8a `sovereign backup
   create` (V0.5). The multi-tenant case has more
   failure modes; the V1.0 backup is "backup every 1h
   + on every chatops action" (the chatops action
   trigger is new in V1.0).
4. **V1.0 release**: tag, CHANGELOG, release notes
   that explicitly call out the multi-tenant behavior
   and the security model.

### Acceptance criteria

- [ ] Pen-test report from an external auditor (V1.0
      budget line); zero Critical or High findings
      open.
- [ ] Design partner has run the operator + 2 end-user
      flows for 4 weeks without a Sev-1 incident.
- [ ] 50 installs/week + 10 active production users
      (mix of operator + multi-tenant) on the V1.0
      binary.
- [ ] 1 BSI C5 case study in progress (committed
      under `docs/enterprise/c5-case-study.md`).

### Definition of done

- 13 V0 gates + 6 V0.6 gates all green.
- V1.0 release tagged, CDN mirrored, GitHub release
  notes published.
- The pen-test report and design-partner retrospective
  are committed.

---

## 1. Cumulative test budget

| Sub-phase | New unit | New integration | Cumulative total |
|---|---|---|---|
| V0.5 (current) | — | — | 210 |
| S1 | +70 | +6 | 286 |
| S2 | +8 | +4 | 298 |
| S3 | +30 | +8 | 336 |
| S4 | +24 | +8 | 368 |
| S5 | +30 | +10 | 408 |
| S6 | +0 | +0 | 408 (no new code; pen-test is external) |

The 13 V0 close-out gates are re-run on every sub-phase
PR. The 6 V0.6 sub-phase DoDs above are the per-sub-phase
gates.

## 2. Risk register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `apt-get install` fails non-deterministically on a real CX22 | Med | High | Integration tests run on a real CX22; S1 has 6 integration tests, not 1 |
| Certbot rate-limits the design-partner domain | Low | Med | S2 has `--staging` mode; the operator can switch between staging and prod |
| RBAC matrix has a hole that lets a viewer read secrets | Med | Critical | The S3 unit tests are a 3×18 matrix (role × action) — every cell has a positive and negative test |
| Telegram bot token leaks (commits, screenshots) | High | High | Token is read from `sovereign.toml`, never printed; `sovereign chatops rotate --user <email>` re-rolls |
| 6-digit code is brute-forced in 5 minutes (10^6 = 1M attempts) | Med | High | Code is a 6-digit number (1M possibilities) but the TTL is 5 minutes; rate-limit the code-verify endpoint at 5 attempts/min/user; alert the user on the 6th attempt |
| phpMyAdmin vhost serves a different user's database | Low | Critical | The phpMyAdmin config writer (S5) is path-tested: every test writes a vhost, parses it, asserts the `auth_basic_user_file` is the right per-user file |
| FTP user escapes jail via symlink | Med | High | vsftpd's `chroot_local_user=YES` + `allow_writeable_chroot=YES` + the vsftpd config writer refuses to write a config without these; tested with a symlink-to-/etc/passwd integration test |
| Pen-test finds a Critical in S6 | Med | High | S6 has 6 weeks; the design-partner work + pen-test are scheduled in parallel; fixes are scoped to 2 weeks, the remaining 4 weeks are stabilization |

## 3. How this phase interacts with the V0 line

- **V0 line keeps working.** V0.6 adds new crates; the
  V0 binary at v0.1.0 is byte-for-byte unchanged.
- **V0.6 ships as a parallel release.** v0.1.0 is the
  V0 close-out; v0.6.0 is the first V0.6 release. The
  operator can run v0.1.0 forever; v0.6.0 is opt-in.
- **The V0.5 → V0.6 migration** is automatic for the
  master key (`sovereign login --migrate`); the V0.6
  user model is additive (V0 has no user table; V0.6
  adds one). A V0.6 install bootstraps its admin on
  first run.
- **The 13 V0 close-out gates are re-run on every
  V0.6 PR.** This is the "V0.6 must not regress V0"
  rule. The gates are: `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets
  --no-default-features -- -D warnings`, `cargo deny
  check`, `cargo test --workspace --no-default-features`,
  the F1-F10 functional checks, the auto-rollback
  test, the 18-check basic doctor, the install.sh
  idempotency check, the install.sh end-to-end on a
  fresh Ubuntu 22.04 VM, the `sovereign update` happy
  path, the `sovereign backup create / verify /
  restore` happy path, the `sovereign doctor
  --level basic` exit-0, and the binary size
  ≤ 30 MB (V0.6 is allowed +5 MB over V0's 25 MB;
  30 MB hard limit).

## 4. Operator commitments

The operator (you) commits to:

1. **V0 close-out first** (Step 9 + Step 10 + Show HN).
2. **Tag v0.1.0** from main once the close-out is done.
3. **Open the S1 PR** on `phase-0.6/01-systemd` from
   main; review and merge.
4. **For each sub-phase (S1-S5)**: 1 CX22 worth of
   evidence (~2 hours of operator time per sub-phase);
   commit the evidence under
   `docs/operations/phase-0.6/<sub-phase>-evidence.md`.
5. **For S6**: engage a pen-test auditor; the
   pen-test budget is a separate line item (not in
   this design doc).
6. **For Show HN**: post the v0.1.0 announcement first;
   post the v0.6.0 announcement 3 months later; the
   v1.0.0 announcement goes out only after the
   pen-test closes.

## 5. End of phase 0.6 design

When S6 is done, the docs map at the top of this file
gains a `v1.0.0` row. The V1 phase (G1-G20 from
`docs/phase-01-v1.md`) starts on the V1.0 line.
