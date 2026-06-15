# Phase 1 — The Product (V1)

**Timeline:** Weeks 7-18 (12 weeks)
**Goal:** A usable product for solo developers and small teams. TUI as the daily-driver interface. 6 framework scanners. The first "killer" features beyond the engine. Documentation site. System packages. Doctor promoted to the **standard** level with `--fix`, `--explain`, `--report`, and `--watch` continuous, plus a benchmark subcommand and a cost subcommand.
**Audience:** The lead engineer + 2 contributors. First design partners start using it.
**Definition of Done:** 50 installs/week, 10 active production users, time-to-first-deploy < 5 min for the 6 golden paths, 1 public BSI C5 case study in progress, `sovereign doctor --level standard` returns 0 fails.

---

## 0. The week-by-week plan

```text
Week 7-8:   G1 (TUI) + G2 (resource limits / cgroups)
Week 9-10:  G3 (structured logs + search) + G4 (health probes + alert channels)
Week 11-12: G5 (multi-environment) + G6 (Postgres + MySQL + Redis provisioning) + G7 (volumes + backups)
Week 13-14: G8 (OpenTelemetry + Prometheus /metrics) + G9 (TLS for control plane + config file)
Week 15-16: G10 (6 framework scanners) + G11 (morning-report + golden-check) + G12 (backup verify)
Week 17-18: G13 (systemd, deb/rpm) + G14 (mdBook docs, /llms.txt, MCP, /migration) + G15 (Show HN)
Week 19-20: G16 (sovereign doctor standard) + G17 (doctor --fix expansion) + G18 (doctor --explain + KB)
Week 21:    G19 (sovereign bench) + G20 (sovereign cost) + V1 hardening + 1.0.0 release
```

Each feature (G1-G15) is described with: **Goal → Why → Sub-tasks → Code stubs → Tests → Acceptance criteria → Definition of done**.

---

## G1. `sovereign tui` — ratatui dashboard

**Goal:** A keyboard-driven, full-screen terminal dashboard that shows the fleet status, the most recent deploy, the most recent error, the next backup, and per-app drill-down (logs, deploys, rollbacks, metrics, domains, secrets, config).

**Why it matters:** The TUI is the daily-driver. A user who can `ssh box && sovereign tui` and see the state of 12 apps in 2 seconds will never log into a web UI.

### Sub-tasks

1. **Set up `ratatui` 0.30 + `crossterm` 0.28** in the binary crate behind `--features tui`.
2. **Implement the alt-screen + raw mode + panic-safe terminal restore** (the 4 non-negotiables from [`product-ux.md` §4](#)):
   - `crossterm::terminal::enable_raw_mode()`
   - `crossterm::execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)`
   - A `Drop` impl on the `App` struct that reverses all of the above (panic-safe)
   - `tokio::signal::ctrl_c()` triggers a graceful shutdown
   - SIGWINCH (terminal resize) is handled via `crossterm::event::resize()`
   - SIGTSTP (Ctrl+Z) is handled by suspending and resuming
3. **Implement the 6 views** (Pulse, Apps, App detail, Servers, Backups, Audit):
   - **Pulse:** 5 lines — app count, healthy count, last deploy, last error, next backup. The "is everything OK?" screen.
   - **Apps:** master list — name, status, last deploy, last deploy version, response time p50. `j`/`k` to navigate, `Enter` to drill down.
   - **App detail:** tabs — Logs, Deploys, Rollbacks, Metrics, Domains, Secrets, Config. `Tab`/`Shift+Tab` to switch.
   - **Servers:** fleet — hostname, status, CPU, RAM, disk. (V0: single server; V1.5+: multi.)
   - **Backups:** every database, last backup, last verify, status.
   - **Audit:** filterable timeline — `sovereign audit` but interactive.
4. **Implement the keybindings** (universal conventions from [`product-ux.md` §6](#)):
   - `q` quit, `Esc` back, `j`/`k` down/up, `h`/`l` left/right
   - `/` search, `n`/`N` next/prev, `Esc` dismiss
   - `?` help for current view
   - `:` command mode (`:rollback`, `:logs`, `:deploy`)
   - `Enter` select, `Tab` switch panel, `Space` toggle
   - `g`/`G` top/bottom, `Ctrl+P` command palette
5. **Implement the data fetching** via the HTTP API (the TUI is a *client* of the control plane, not a separate process).
6. **Implement the live update** via SSE — `sovereign status --watch` streams events, the TUI subscribes.

### Code stub

```rust
// crates/sovereign/src/tui/mod.rs
use ratatui::{Frame, Terminal, backend::CrosstermBackend, layout::{Layout, Constraint, Direction}, widgets::{Block, Borders, List, ListItem, Paragraph}, style::{Color, Style}, text::Line};
use crossterm::{event::{self, Event, KeyCode}, execute, terminal::{EnterAlternateScreen, LeaveAlternateScreen, enable_raw_mode, disable_raw_mode}};
use std::io::{self, Stdout};
use std::panic;

pub struct TuiApp {
    pub state: TuiState,
    pub client: SovereignClient,
}

pub struct TuiState {
    pub current_view: View,
    pub apps: Vec<AppSummary>,
    pub selected: usize,
    pub should_quit: bool,
}

pub enum View { Pulse, Apps, AppDetail(AppId), Servers, Backups, Audit }

impl TuiApp {
    pub async fn run(&mut self) -> anyhow::Result<()> {
        // Set up panic-safe terminal restore
        let original_hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let _ = disable_raw_mode();
            let _ = execute!(io::stdout(), LeaveAlternateScreen);
            original_hook(info);
        }));
        
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        
        let result = self.event_loop(&mut terminal).await;
        
        // Always restore terminal, even on error
        disable_raw_mode()?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
        terminal.show_cursor()?;
        
        result
    }
    
    async fn event_loop<B: ratatui::backend::Backend>(&mut self, terminal: &mut Terminal<B>) -> anyhow::Result<()> {
        loop {
            // Refresh data
            self.state.apps = self.client.list_apps().await?;
            
            // Draw
            terminal.draw(|f| self.ui(f))?;
            
            // Handle events (with 100ms timeout so we can refresh)
            if event::poll(std::time::Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    self.handle_key(key);
                }
            }
            
            if self.state.should_quit { break; }
        }
        Ok(())
    }
    
    fn ui(&self, f: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0), Constraint::Length(3)])
            .split(f.size());
        
        // Header
        let header = Paragraph::new(format!(" sovereign — {} apps, {} healthy", 
            self.state.apps.len(),
            self.state.apps.iter().filter(|a| a.status == "healthy").count()))
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(header, chunks[0]);
        
        // Body — view-specific
        match self.state.current_view {
            View::Pulse => self.ui_pulse(f, chunks[1]),
            View::Apps => self.ui_apps(f, chunks[1]),
            View::AppDetail(id) => self.ui_app_detail(f, chunks[1], id),
            _ => {}
        }
        
        // Footer (keybinding hints)
        let footer = Paragraph::new(" q:quit  /:search  Tab:switch  ?:help ")
            .block(Block::default().borders(Borders::ALL));
        f.render_widget(footer, chunks[2]);
    }
    
    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        match key.code {
            KeyCode::Char('q') => self.state.should_quit = true,
            KeyCode::Char('j') => self.state.selected = (self.state.selected + 1).min(self.state.apps.len().saturating_sub(1)),
            KeyCode::Char('k') => self.state.selected = self.state.selected.saturating_sub(1),
            KeyCode::Enter => {
                if let Some(app) = self.state.apps.get(self.state.selected) {
                    self.state.current_view = View::AppDetail(app.id);
                }
            }
            _ => {}
        }
    }
}
```

### Tests

- Unit test: `handle_key('q')` sets `should_quit = true`.
- Unit test: `handle_key('j')` increments `selected` (clamped).
- Integration test: start the TUI, send `q`, assert the terminal is restored (echo works, no garbage).
- Integration test: panic inside `event_loop` still restores the terminal.
- Snapshot test (insta): the Pulse view renders the expected layout.

### Acceptance criteria

- [ ] `sovereign tui` opens in alt-screen mode
- [ ] On any exit (q, Ctrl+C, panic), the terminal is restored
- [ ] Pulse view shows accurate app count + healthy count
- [ ] Apps view navigates with j/k
- [ ] App detail view shows logs, deploys, rollbacks, metrics, domains, secrets, config tabs
- [ ] `Ctrl+Z` suspends and `fg` resumes cleanly
- [ ] Resize (`SIGWINCH`) reflows the layout

### Definition of done

A test that runs the TUI in a `tmux` session, sends keystrokes, and asserts the terminal is restored on `q` — passes in CI on every PR. A manual test that shows the 6 views in a screen recording.

---

## G2. Resource limits (cgroups)

**Goal:** Every app runs in a cgroup with configurable CPU + memory limits. Exceeding a limit is a `Failed` deploy with a clear error message.

**Why it matters:** Production safety. Without cgroup limits, a memory leak in one app takes down the host. With them, the leak is contained and the deploy is rolled back.

### Sub-tasks

1. **Add `resources` to `app.yaml`**:
   ```yaml
   resources:
     cpu: 0.5       # 0.5 vCPU
     memory: 512M   # 512 MB RAM
   ```
2. **Translate to Docker run flags** in `sovereign-runtime-docker/`:
   - `--cpus=0.5` → Docker's cgroup v1/v2-compatible CPU limit
   - `--memory=512m` → Docker's memory limit (hard kill on OOM)
3. **Detect OOM events** via `docker events` stream; on `oom` event, transition deployment to `Failed` with reason "OOM killed", trigger auto-rollback.
4. **Emit metrics**: `container_cpu_usage`, `container_memory_usage` (gauge).
5. **Document the resource syntax** in the docs site.

### Acceptance criteria

- [ ] `app.yaml` with `memory: 256M` and an app that allocates 512MB is OOM-killed within 5s.
- [ ] The OOM event triggers an auto-rollback.
- [ ] The metrics endpoint shows `container_memory_usage` for each app.

---

## G3. Structured JSON logs + search + retention

**Goal:** Every container's stdout/stderr is captured, structured (JSON in, JSON out), searchable (`sovereign logs --since 30m --level error`), and rotated (30-day retention by default).

**Why it matters:** Logs are the #1 thing an operator looks at during an incident. If they are unstructured, unsearchable, or unretained, the incident becomes a war story.

### Sub-tasks

1. **Implement `RuntimePort::logs(id, opts) -> LogStream`** that returns a `futures::Stream<Item = Result<Bytes>>`.
2. **Persist logs to `/var/lib/sovereign/logs/<app>/<date>.jsonl`** with rotation.
3. **Add log search** via SQLite FTS5 (virtual table over the log lines).
4. **Implement `--follow`** via SSE (`GET /v1/apps/{id}/logs?follow=true`).
5. **Implement `--since`, `--until`, `--level`, `--tail`** in the CLI.
6. **Implement log rotation** via a daily cron (or a tokio task in the binary).

### Code stub

```rust
// crates/sovereign-core/src/ports/runtime.rs
use async_trait::async_trait;
use futures::Stream;
use bytes::Bytes;

pub struct LogOpts {
    pub tail: Option<usize>,
    pub follow: bool,
    pub since: Option<chrono::DateTime<chrono::Utc>>,
    pub until: Option<chrono::DateTime<chrono::Utc>>,
    pub level: Option<String>,
}

pub type LogStream = std::pin::Pin<Box<dyn Stream<Item = Result<Bytes, RuntimeError>> + Send>>;

#[async_trait]
pub trait RuntimePort: Send + Sync {
    // ... existing methods
    async fn logs(&self, container_id: &str, opts: LogOpts) -> Result<LogStream, RuntimeError>;
}
```

### Acceptance criteria

- [ ] `sovereign logs api --tail=100` returns the last 100 lines.
- [ ] `sovereign logs api --since 30m --level error` returns error lines from the last 30 minutes.
- [ ] `sovereign logs api --follow` streams new lines as they arrive.
- [ ] Logs are retained for 30 days; older logs are deleted by the daily rotation task.

---

## G4. Health probes + alert channels

**Goal:** Health probes (liveness, readiness, startup) are first-class. Alert channels (email, Telegram, Slack, Discord, webhook) ship in V1.

### Sub-tasks

1. **Implement 3 probe types** in `app.yaml`:
   ```yaml
   health:
     startup: { path: /startup, timeout: 60s }
     liveness: { path: /health, interval: 30s, threshold: 3 }
     readiness: { path: /ready, interval: 10s, threshold: 2 }
   ```
2. **Spawn background probers** in the binary crate — one tokio task per deployment.
3. **Implement 5 alert channels** in `sovereign-notify/`:
   - `EmailChannel` (SMTP via `lettre`)
   - `TelegramChannel` (Bot API)
   - `SlackChannel` (Incoming Webhook)
   - `DiscordChannel` (Webhook)
   - `WebhookChannel` (generic JSON POST)
4. **Wire `sovereign alerts add <channel>` CLI** for each.
5. **Wire the 10 alerts from [`operations-runbook.md` §3](#)** to the channels.

### Acceptance criteria

- [ ] All 3 probe types work as documented.
- [ ] All 5 alert channels can be configured and fire on a test event.
- [ ] Alert payload is JSON, includes app, env, severity, summary, runbook URL.

---

## G5. Multi-environment (dev / staging / prod in one config)

**Goal:** One app config supports three environments, with promotion between them.

### Sub-tasks

1. **Add `env` to `app.yaml`** (default: `dev`).
2. **Add `promote` CLI** (`sovereign promote api staging prod --actor alice`):
   - Runs `policy::enforce` for prod (V2 stub: always allow in V1)
   - Deploys the current staging image to prod
   - Records the promotion in the audit log
3. **Add per-env resource limits** in `app.yaml`:
   ```yaml
   environments:
     dev: { replicas: 1, cpu: 0.25, memory: 256M }
     staging: { replicas: 1, cpu: 0.5, memory: 512M }
     prod: { replicas: 2, cpu: 1, memory: 1G }
   ```
4. **Add env-scoped secrets** (already supported in V0 via `app` table; just expose in CLI).

### Acceptance criteria

- [ ] `sovereign promote api staging prod` deploys the staging image to prod.
- [ ] The promotion is recorded in the audit log with `kind: "promote"`.

---

## G6. Database provisioning (Postgres, MySQL, Redis)

**Goal:** One-click provisioning for Postgres, MySQL/MariaDB, and Redis as first-class apps.

### Sub-tasks

1. **Define a `DatabaseApp` domain type** (a special `App` with `kind = "database"`).
2. **Implement `sovereign db create postgres --app mydb`**:
   - Starts a `postgres:16-alpine` container with a generated password.
   - Stores the password as a secret under the app.
   - Exposes the app via a private DNS name (e.g., `mydb.internal`).
3. **Implement connection pooling** via PgBouncer (for Postgres).
4. **Implement the same for MySQL/MariaDB** (`mysql:8` or `mariadb:11`) and **Redis** (`redis:7-alpine`).
5. **Implement `sovereign db shell <app>`** that drops into `psql` / `mysql` / `redis-cli` against the container.

### Acceptance criteria

- [ ] `sovereign db create postgres mydb` provisions a running Postgres.
- [ ] `sovereign db shell mydb` drops into `psql` (no wrapper).
- [ ] The connection string is available as a secret: `mydb.url`.

---

## G7. Volumes + scheduled backups

**Goal:** Named volumes for app data. Scheduled backups (daily/weekly/monthly) with retention policies.

### Sub-tasks

1. **Add `volumes` to `app.yaml`**:
   ```yaml
   volumes:
     - name: data
       mount: /var/lib/app/data
   ```
2. **Implement Docker named volumes** in the runtime adapter.
3. **Add backup schedules** to `app.yaml`:
   ```yaml
   backup:
     schedule: "0 3 * * *"
     retention: 30d
     verify: weekly
   ```
4. **Implement the scheduler** as a tokio task in the binary crate.
5. **Implement retention enforcement** (delete backups older than `retention`).

### Acceptance criteria

- [ ] App data persists across restarts.
- [ ] Backups run on schedule and respect retention.

---

## G8. OpenTelemetry + Prometheus `/metrics`

**Goal:** The control plane exposes a Prometheus `/metrics` endpoint with the mandatory metrics from [`architecture.md` §8.2](#). OpenTelemetry SDK is feature-flagged.

### Sub-tasks

1. **Implement `/metrics` route** in `sovereign/src/http/`:
   - Format: Prometheus text exposition.
   - Path: `GET /metrics` on the control plane port.
2. **Wire the 11 mandatory metrics** in `sovereign-observability/`.
3. **Implement the OTel SDK** behind `--features otel`.
4. **Document the OTel exporter config** in the docs site (env vars: `OTEL_EXPORTER_OTLP_ENDPOINT`, etc.).

### Acceptance criteria

- [ ] `curl http://127.0.0.1:7878/metrics` returns Prometheus text.
- [ ] All 11 mandatory metrics are present.
- [ ] `--features otel` enables OTel export; default is off.

---

## G9. TLS for control plane + `app.yaml` declarative deploy

**Goal:** The control plane API itself serves HTTPS (with `mkcert` for self-hosted, or a real cert). The `app.yaml` is the source of truth for declarative deploys.

### Sub-tasks

1. **Implement HTTPS for the control plane**:
   - V1.0: `mkcert`-generated self-signed cert at install time.
   - V1.1: Caddy fronts the control plane with auto-TLS.
2. **Implement `sovereign apply -f app.yaml`** (V1.0 stub; V1.5 full):
   - Parses the YAML, computes the diff against current state, applies.
   - `--dry-run` prints the diff.
3. **Document the `app.yaml` schema** in the docs site (auto-generated from a Rust struct via `schemars`).

### Acceptance criteria

- [ ] `https://<host>:7878/v1/apps` works with a self-signed cert (with `--tls-skip-verify` for the CLI).
- [ ] `sovereign apply -f app.yaml --dry-run` prints the diff and exits 0.
- [ ] `sovereign apply -f app.yaml` applies the diff.

---

## G10. Six framework scanners (`sovereign init <framework>`)

**Goal:** `sovereign init <framework>` detects the framework in the current directory, writes an `app.yaml`, and suggests the next step. Supports FastAPI, Next.js, Laravel, Go, Rails, Astro.

**Why it matters:** The framework scanner is the **highest-leverage code in V1**. A 5-minute first deploy that *worked without me reading docs* is the moat. Coolify has 280+ templates; the scanner doesn't need to be that good for V1, but it must work for the 6 golden paths.

### Sub-tasks

1. **Implement a scanner framework** in `sovereign/src/scanner/`:
   - `fn detect(framework: Framework, dir: &Path) -> Result<AppYaml, ScannerError>`
   - Looks for marker files (`pyproject.toml`, `package.json`, `go.mod`, `Gemfile`, `composer.json`).
   - Reads the manifest, extracts the entry point, the build command, the start command.
2. **Implement 6 scanners** (one per framework):
   - **FastAPI:** detect `pyproject.toml` + `fastapi` + `uvicorn` → `framework: fastapi`, `build: pip install -r requirements.txt`, `start: uvicorn main:app --host 0.0.0.0 --port 8000`.
   - **Next.js:** detect `package.json` + `next` → `framework: nextjs`, `build: npm run build`, `start: npm run start -- -p 3000`.
   - **Laravel:** detect `composer.json` + `laravel/framework` → `framework: laravel`, similar.
   - **Go:** detect `go.mod` → `framework: go`, `build: go build -o app .`, `start: ./app`.
   - **Rails:** detect `Gemfile` + `rails` → `framework: rails`, similar.
   - **Astro:** detect `package.json` + `astro` → `framework: astro`, similar.
3. **Auto-generate a `Dockerfile`** if none exists (V1.0: simple, framework-specific; V1.1: multi-stage).
4. **Wire `sovereign init fastapi|nextjs|laravel|go|rails|astro` CLI** to the scanners.

### Code stub

```rust
// crates/sovereign/src/scanner/fastapi.rs
pub fn detect(dir: &Path) -> Result<AppYaml, ScannerError> {
    let pyproject = dir.join("pyproject.toml");
    if !pyproject.exists() {
        return Err(ScannerError::NotDetected("pyproject.toml not found".into()));
    }
    let content = std::fs::read_to_string(&pyproject)?;
    if !content.contains("fastapi") {
        return Err(ScannerError::NotDetected("fastapi not in pyproject.toml".into()));
    }
    let main_module = detect_main_module(dir)?;  // looks for main.py with `app = FastAPI()`
    let port = 8000;
    Ok(AppYaml {
        app: AppName::from_dir(dir)?,
        framework: Framework::Fastapi,
        source: Source { github: None },  // user fills in
        build: Build {
            dockerfile: Some("Dockerfile".into()),
        },
        deploy: Deploy {
            strategy: Strategy::BlueGreen,
            replicas: 1,
            resources: Resources { cpu: 0.5, memory: "512M".into() },
        },
        health: Health {
            path: "/health".into(),
            interval: "10s".into(),
            timeout: "5s".into(),
            threshold: 3,
        },
        env: vec![],
        secrets: vec!["DATABASE_URL".into()],  // hint
        // ...
    })
}
```

### Acceptance criteria

- [ ] `sovereign init fastapi` in a FastAPI project writes a valid `app.yaml`.
- [ ] The 5 other scanners work analogously.
- [ ] The generated `Dockerfile` builds and runs.
- [ ] A test fixture per framework is checked in (`tests/fixtures/fastapi/`, etc.).

---

## G11. `sovereign morning-report` + `sovereign golden-check`

**Goal:** `morning-report` is a 5-line shell script wrapped as a subcommand that prints the daily health summary. `golden-check` prints the "is this deployment following the golden path?" report.

### Sub-tasks

1. **Implement `morning-report`** per the spec in [`operations-runbook.md` §1](#).
2. **Implement `golden-check`** with the 16-field service catalog from [`architecture.md` §6.3](#) (or a subset for V1).
3. **Bind `morning-report` to a tmux key** in the install script (V1.1).

### Acceptance criteria

- [ ] `sovereign morning-report` prints a 10-line summary in < 5s.
- [ ] `sovereign golden-check` reports ✓/✗ for each of the 16 fields.

---

## G12. `sovereign backup verify` (the single most important V1.2 command)

**Goal:** `sovereign backup verify --app postgres --restore-to scratch` downloads the latest backup, restores it to a scratch database, counts rows in each table, compares to source, drops the scratch DB, returns the diff.

**Why it matters:** The GitLab 2017-01-31 lesson: 8 months of "successful" backups, all empty, all useless. They recovered 40% of the data. **Backups you haven't restored are hopes.**

### Sub-tasks

1. **Implement the verify use case** in `sovereign-core/src/use_cases/backup.rs`:
   - Download the dump from S3 to a scratch dir.
   - `pg_restore --list` to enumerate tables.
   - Create a scratch DB (`sovereign_verify_<timestamp>`).
   - `pg_restore` into the scratch DB.
   - For each table, `SELECT COUNT(*)` and compare to source.
   - Drop the scratch DB.
   - Return the diff (table -> count_source, count_scratch, delta).
2. **Wire the `backup verify` CLI**.
3. **Schedule monthly verify** in the cron (V1.2, not V1.0).
4. **Emit metrics**: `backup_verify_duration_seconds`, `backup_verify_row_count_diff_total`.

### Acceptance criteria

- [ ] `sovereign backup verify --app postgres --restore-to scratch` returns 0 with the diff.
- [ ] The diff matches the source DB row counts.
- [ ] The scratch DB is dropped on success or failure.

---

## G13. systemd unit, deb/rpm packages

**Goal:** The binary installs as a systemd service. The deb/rpm packages are reproducible.

### Sub-tasks

1. **Write a systemd unit** at `packaging/sovereign.service`:
   ```ini
   [Unit]
   Description=Sovereign Application Runtime
   After=network-online.target
   Wants=network-online.target
   
   [Service]
   Type=simple
   User=sovereign
   Group=sovereign
   ExecStart=/usr/bin/sovereign http
   Restart=always
   RestartSec=5
   LimitNOFILE=65536
   StateDirectory=/var/lib/sovereign
   ConfigurationDirectory=/etc/sovereign
   LogsDirectory=/var/log/sovereign
   
   [Install]
   WantedBy=multi-user.target
   ```
2. **Use `cargo-deb` and `cargo-rpm`** to build the packages.
3. **Sign the packages** with a GPG key (V1.5; V1: unsigned, marked as such).
4. **Document the install** in the docs site.

### Acceptance criteria

- [ ] `apt install sovereign` (or `dnf install sovereign`) installs and starts the service.
- [ ] `systemctl status sovereign` shows "active (running)".
- [ ] The service auto-restarts on crash.

---

## G14. mdBook documentation site

**Goal:** A `docs.sovereignruntime.dev` site built with `mdbook`, hosted on the product itself (dogfooding), with auto-generated CLI reference, the AI-agent entry points (`/llms.txt`, `/llms-full.txt`, MCP server), and a public `/migration` section.

### Sub-tasks

1. **Set up `mdbook`** in `docs/`.
2. **Generate the CLI reference** from `clap` at build time:
   ```bash
   sovereign man --output docs/src/reference/cli/
   ```
3. **Generate the API reference** from `axum` + `utoipa` at build time:
   ```bash
   sovereign openapi > docs/src/reference/api/openapi.yaml
   ```
4. **Write the docs**:
   - **Quickstart** (the 5 commands)
   - **Concepts** (deploy, rollback, secret, backup, server, policy)
   - **Tutorials** (per golden path: FastAPI, Next.js, Laravel, Go, Rails, Astro)
   - **How-tos** (per task: set up a domain, configure backup, write a policy rule)
   - **Reference** (CLI, API, config, rego)
   - **Operations** (runbooks, SLOs, alerts, DR)
   - **Migration** (added in this clarification, addresses the Heroku-sunset / Coolify / Dokploy / Render / Dokku wedge from `persona-pm.md` §5):
     - `/migration/from-heroku.md` — 30-minute walkthrough, with "Before you start" checklist, the 8 steps (install, export, dry-run, review, apply, verify, cut DNS, decommission), and the FAQ.
     - `/migration/from-coolify.md` — same structure, with the Compose-to-`app.yaml` transpile explained.
     - `/migration/from-dokploy.md` — same structure, with the `dokploy-export` helper.
     - `/migration/from-render.md` — same structure, with the Render API key instructions.
     - `/migration/from-dokku.md` — V1.5 addition (H20 does Dokku as a thin wrapper).
     - `/migration/from-kamal.md` — V2 addition (smaller user base; not on the V1.5 critical path).
     - Each guide has: a "Why migrate from <platform>?" section (1 paragraph with the pain points cited from the research), a "Why sovereign?" section (3 bullets on the values), and a "What you keep / what you lose" table.
5. **Host on the product itself** in V1.5 (V1: GitHub Pages or Netlify).
6. **Add the AI-agent entry points** (added in this clarification, addresses `persona-pm.md` §5 line 226 — 2026 table stakes for AI agent deploys):
   - **`/llms.txt`** — a single text file at the docs root, in the `llms.txt` spec format. Contents: the product name, a 1-paragraph description, the canonical URL for the full text, the section headers (Quickstart, Concepts, Tutorials, How-tos, Reference, Migration, Doctor KB), and a short example prompt. Used by Claude Code / Cursor / Copilot when they want to "know about this product."
   - **`/llms-full.txt`** — the entire mdBook concatenated into a single text file (with section headers as comments). Used when the agent needs the full reference in context.
   - **MCP server** — a Model Context Protocol server that exposes the docs as resources and the CLI as tools. Runs as a separate binary `sovereign-mcp` (a thin wrapper that imports the same crates). The MCP server exposes:
     - **Resources**: `sovereign://docs/quickstart`, `sovereign://docs/concepts/deploy`, `sovereign://docs/concepts/rollback`, etc. (one per mdBook page).
     - **Tools**: `sovereign_deploy`, `sovereign_rollback`, `sovereign_status`, `sovereign_logs`, `sovereign_doctor`, `sovereign_incident_declare`, etc. (one per CLI subcommand).
     - The MCP server is a thin pass-through to the local `sovereign` binary; it does not store state. The on-call can use Claude Code to "run `sovereign doctor --level standard` and summarize the fails."
   - The MCP server is opt-in: `sovereign mcp serve --port 7878` starts it; the operator adds it to their Claude Code / Cursor config.
   - The MCP server is documented in `/docs/ai-agents.md` — how to install, how to configure, the security model (the MCP server runs locally; it cannot be reached from the network unless the operator explicitly binds it).
7. **Add a "for AI agents" section to the docs** — a `/docs/for-agents.md` page that explains the MCP server, the `llms.txt` convention, and the canonical prompt templates ("Deploy my FastAPI app to a Hetzner box," "Show me the audit log for the last 7 days," "What is the 10-point sovereignty test?").
8. **Add a landing-page note about the AI-agent story**: "The CLI is the product. The MCP server is the entry point for AI agents. `/llms.txt` is the canonical machine-readable index." This is a 1-line footnote on the landing page; it's a brand signal, not a feature.

### Acceptance criteria

- [ ] `cargo doc --no-deps` builds.
- [ ] `mdbook build` builds the docs.
- [ ] The CLI reference page is in sync with the actual `sovereign --help`.
- [ ] The API reference is in sync with the actual OpenAPI spec.
- [ ] `/llms.txt` and `/llms-full.txt` are served at the docs root.
- [ ] `sovereign mcp serve` starts an MCP server on the configured port.
- [ ] Claude Code / Cursor can connect to the MCP server and call `sovereign_deploy`.
- [ ] The 4 `/migration/from-*.md` guides are published.

---

## G15. Show HN

**Goal:** A Show HN post that introduces the product, demonstrates the 5-command quickstart, and lists the first 10 design partners.

### Sub-tasks

1. **Draft the post** (1-2 paragraphs + a 60-second screen recording + a list of features).
2. **Prepare the demo environment** (a public Hetzner box running the binary, accessible for 7 days post-post).
3. **Recruit 10 design partners** (solo founders, agencies) who will publicly vouch for the product.
4. **Schedule the post** for a Tuesday morning (peak Show HN visibility).
5. **Prepare the FAQ** for the inevitable comments.

### Acceptance criteria

- [ ] The post is drafted and approved.
- [ ] 10 design partners have confirmed.
- [ ] The demo environment is live.
- [ ] The post goes live Tuesday morning and stays on the front page for ≥ 4 hours.

---

## G16. `sovereign doctor --level standard` — full coverage + Watch mode

**Goal:** Promote `sovereign doctor` from the V0 **basic** level (10 checks, 6 categories) to the V1 **standard** level (40+ checks, all 14 categories), and add the continuous `--watch` mode that streams check state changes to the operator's terminal.

**Why it matters:** The V0 basic level catches the load-bearing subsystems (system, binary, storage, runtime, proxy, secrets). It misses 8 categories of failure: backup health, network reachability, agents, observability, security, sovereignty, performance, cost. In V1 the operator is running real workloads (5-12 apps per box), and the failure modes shift from "binary works" to "the binary is fine but backups silently stopped 3 days ago" or "the disk is filling up from build cache" or "the Caddy cert auto-renew is failing." Standard level catches all of this.

### Sub-tasks

1. **Add the 8 new V1 check categories** (each a new file in `sovereign-doctor/src/checks/`):
   - **Backup** (6 checks) — `backup_recent` (last successful backup ≤ 26h ago), `backup_size_sane` (no 0-byte dumps), `backup_verify_recent` (last `verify` ≤ 7 days), `backup_offsite` (the dump exists on S3, not just locally), `backup_retention` (≥ 7 daily + 4 weekly + 3 monthly), `backup_encrypted` (the dump is at rest, not plaintext).
   - **Network** (5 checks) — `dns_resolves` (`api.sovereignruntime.dev` A record), `https_egress` (TLS to `releases.sovereignruntime.dev` works, no MITM), `egress_allowlist` (only the documented domains are contacted, all others are denied — enforced by Caddy's egress filter), `port_80_reachable` (from the binary's perspective, port 80 is bound), `port_443_reachable` (same for 443).
   - **Agents** (3 checks) — `agent_count_expected` (number of registered agents == number in the `server` table), `agent_heartbeat_fresh` (last heartbeat ≤ 60s ago per agent), `agent_token_unique` (no two agents share a token; uses `agent_token` table).
   - **Observability** (3 checks) — `metrics_endpoint_up` (the `/metrics` endpoint returns 200), `tracing_exporter_configured` (the OTLP endpoint is set and reachable), `log_redactor_active` (a known secret key does not appear in the logs of the last 1000 audit events).
   - **Security** (6 checks) — `binary_no_setuid` (no setuid bit on `/usr/local/bin/sovereign`), `secrets_dir_0600` (`/var/lib/sovereign/secrets` is 0700), `caddy_admin_localhost_only` (Caddy's admin API binds 127.0.0.1:2019, not 0.0.0.0), `sshd_password_auth_off` (in `/etc/ssh/sshd_config`), `firewall_active` (`nft` or `ufw` is running), `audit_log_append_only` (the `audit` SQLite table is on a `WAL` journal and the `journal_mode=WAL` pragma is set, plus the file's mtime does not regress — detects tampering).
   - **Sovereignty** (3 checks, stub) — `european_owned` (the binary's provenance has a `origin` field set to "EU", and a `--verify-origin` mode is wired in V2; V1 logs `INFO: sovereignty stub, full test in V2`), `sbom_present` (`/usr/local/share/sovereign/sbom.spdx.json` exists, is a valid SPDX 2.3), `no_us_cloud_dependencies` (the egress allowlist check above doubles as this; V1 adds a `--explain sovereignty` mode that points to the docs).
   - **Performance** (4 checks) — `cpu_steal_low` (CPU steal time < 5% over the last 60s, sampled from `/proc/stat`), `memory_pressure_low` (PSI memory pressure from `/proc/pressure/memory` < 10%), `io_pressure_low` (PSI io pressure from `/proc/pressure/io` < 20%), `tls_handshake_fast` (a `curl --resolve` to the local Caddy returns 200 in < 200ms).
   - **Cost** (3 checks, advisory only) — `backup_size_reasonable` (the S3 spend is < 10% of the total estimated cost; logs the actual % for the morning report), `bandwidth_reasonable` (the egress on the box is < 80% of the host's metered cap), `idle_apps_flagged` (any app with < 1 request/min over the last 7 days is listed in the report's "advisory" section; **not** auto-stopped — that's V2).
   - **Plus the 4 cross-cutting checks added in this clarification** (the Tier 3 from the missing-features analysis — see the "what was missing" note in the phase preamble):
     - **`db_pool_configured`** (Network-adjacent, but logically a database check): for every Postgres/MySQL/Redis DB with ≥ 1 app, the `db_pool` is configured. The check is `Fail` for an unpooled DB with ≥ 1 app. Fix: `sovereign db pool create --db <name>`.
     - **`dns_provider_configured`** (Network): if the operator has ≥ 1 domain in the `domain` table, a DNS provider is configured (`sovereign dns provider add`). Fix: instructs the operator to add one.
     - **`deploy_drain_spec`** (Deployment): every `app.yaml` has a `shutdown_grace_period` set. Apps without one are `Warn` (defaults to 10s, but a Rails / Django app with 30s request latency should set 60s+). Fix: writes the default into the file.
     - **`migration_lock_timeout`** (Database): every Postgres DB has `ALTER SYSTEM SET lock_timeout = '5s'` applied (or the equivalent in the `app.yaml` connection string). Catches the "Rails migration locks the table for 8 minutes" failure mode from `user-pain-research.md` §4.3. Fix: applies the `ALTER SYSTEM` (requires confirm, as it's a DB-wide change).
2. **Implement the new checks as a `DoctorExtension` trait** so the binary is statically linked but the check list is dynamic:
   ```rust
   // sovereign-doctor/src/extension.rs
   pub trait DoctorExtension: Send + Sync {
       fn name(&self) -> &'static str;
       fn level(&self) -> DoctorLevel;
       fn checks(&self) -> Vec<Box<dyn Check>>;
       fn fixes(&self) -> Vec<Box<dyn DoctorFix>>;
   }
   pub struct StandardExtension;
   impl DoctorExtension for StandardExtension { /* ... */ }
   ```
3. **Implement `--watch` continuous mode** (the daily-driver UX of standard level):
   - `sovereign doctor --level standard --watch` runs all 40+ checks every 30s.
   - On any state change (a check that was `Pass` is now `Fail`, or vice versa), prints a single line: `[14:22:03] ⚠ backup_recent: PASS → FAIL — last backup was 27h ago`.
   - The state changes are also published to the `audit` table (`kind: "doctor.state_change"`).
   - `Ctrl+C` exits cleanly (the Drop impl unwinds).
   - `--watch --interval 5s` is supported for development.
   - In TUI, `sovereign tui` shows a live doctor widget in the footer.
4. **Implement `sovereign doctor history`** (V1, the operator's retro tool): shows the last 50 doctor runs with their level, pass/warn/fail counts, and the timestamp. `--json` for piping to a script.
5. **Add Prometheus metrics** to the V0 set:
   - `sovereign_doctor_watch_state_changes_total{category,name,from,to}` — counter.
   - `sovereign_doctor_last_full_run_timestamp{level}` — gauge.
6. **Add a CI test**: a GitHub Actions matrix runs `sovereign doctor --level standard` against the basic, secure, and broken fixtures. The "secure" fixture passes 100%; the "broken" fixture fails ≥ 5 checks with the expected messages and fix suggestions.

### Code stub

```rust
// crates/sovereign-doctor/src/extensions/standard.rs
use crate::{DoctorExtension, DoctorLevel, Check, DoctorFix};
pub struct StandardExtension;
impl DoctorExtension for StandardExtension {
    fn name(&self) -> &'static str { "standard" }
    fn level(&self) -> DoctorLevel { DoctorLevel::Standard }
    fn checks(&self) -> Vec<Box<dyn Check>> {
        vec![
            // Backup
            Box::new(checks::backup::BackupRecentCheck),
            Box::new(checks::backup::BackupVerifyRecentCheck),
            Box::new(checks::backup::BackupOffsiteCheck),
            // ... 6 backup checks
            // Network
            Box::new(checks::network::DnsResolvesCheck),
            Box::new(checks::network::HttpsEgressCheck),
            // ... 5 network checks
            // Observability
            Box::new(checks::observability::MetricsEndpointUpCheck),
            Box::new(checks::observability::LogRedactorActiveCheck),
            Box::new(checks::observability::TracingExporterConfiguredCheck),
            // Security
            Box::new(checks::security::SecretsDir0600Check),
            Box::new(checks::security::CaddyAdminLocalhostOnlyCheck),
            Box::new(checks::security::SshdPasswordAuthOffCheck),
            Box::new(checks::security::FirewallActiveCheck),
            Box::new(checks::security::AuditLogAppendOnlyCheck),
            // ... 6 security checks
            // Performance
            Box::new(checks::performance::CpuStealLowCheck),
            Box::new(checks::performance::MemoryPressureLowCheck),
            // ... 4 performance checks
            // Cost (advisory)
            Box::new(checks::cost::IdleAppsFlaggedCheck),
            // ... 3 cost checks
        ]
    }
    fn fixes(&self) -> Vec<Box<dyn DoctorFix>> {
        vec![
            Box::new(fixes::backup::BackupRunNowFix),
            Box::new(fixes::backup::BackupVerifyNowFix),
            Box::new(fixes::security::CaddyAdminLocalhostFix),
            Box::new(fixes::security::EnableSshdKeyOnlyFix),
        ]
    }
}
```

### Tests

- Unit: every check has a unit test that constructs a fake `DoctorContext` (the `inmemory` test fixture from `sovereign-doctor/src/test/fixtures.rs`) and asserts the expected `CheckStatus`.
- Integration: 3 fixture boxes in `tests/fixtures/doctor/{basic,secure,broken}/` are used as Docker images. The test runs `sovereign doctor --level standard` against each and asserts the expected counts.
- Integration: `sovereign doctor --watch` over 5 minutes in a box where a backup is manually aged out produces a `Pass → Fail` state change.
- Property test: the order in which checks run does not affect the result.
- CI gate: a job runs standard level on every PR; **Warn** is allowed, **Fail** fails the build.

### Acceptance criteria

- [ ] `sovereign doctor --level standard` runs 40+ checks in < 30s on a CX22.
- [ ] `--watch` streams state changes with timestamps.
- [ ] `sovereign doctor history` shows the last 50 runs.
- [ ] The TUI's footer shows a live doctor widget.

### Definition of done

A 7-day soak test: 3 production users run `sovereign doctor --level standard --watch` in a `tmux` session for a week. At least 1 unprompted state change per day is caught (e.g., disk filling up, cert renewal about to fail, backup cron silently broken). The `--fix` mode resolves ≥ 50% of the catches without human intervention.

---

## G17. `sovereign doctor --fix` expansion — auto-remediate the 14 standard-level issues

**Goal:** Expand the V0 `--fix` mode (3 fixes) to cover every **non-destructive** standard-level check, with a 5s interactive confirm per destructive fix.

**Why it matters:** The V0 doctor is the on-call's first command at 3am; the V1 `--fix` is the on-call's second command (after `doctor --explain`). If the operator has to manually fix 5 of the 40 checks, the system has lost the 5-year invariant. The fixes must be safe, audited, and reversible (every fix writes a `doctor.fix` audit event with a before/after diff).

### Sub-tasks

1. **Implement the 12 new V1 fixes** (one per non-destructive check that has a remediation; the rest are informational only):
   - `backup_run_now` — calls `backup::snapshot()` and reports the result. Non-destructive.
   - `backup_verify_now` — calls `backup::verify()` on the last 3 backups. Non-destructive.
   - `caddy_admin_localhost` — re-renders the Caddy config with `admin localhost:2019` and reloads. Non-destructive (but service-restart; warn).
   - `enable_sshd_key_only` — sets `PasswordAuthentication no` in `/etc/ssh/sshd_config` and reloads sshd. **Destructive** (operator could lock themselves out); requires 5s confirm.
   - `enable_firewall` — installs `nftables` rules from `/etc/sovereign/nftables.conf` and starts the service. **Destructive**; requires confirm.
   - `disk_cleanup_cache` — `docker system prune -f` + `journalctl --vacuum-size=100M`. **Destructive** (removes stopped containers, old logs); requires confirm.
   - `master_key_repair_perms` (carried from V0) — `chmod 0600`.
   - `caddy_config_reload` (carried from V0).
   - `tls_renew_now` — `caddy reload` to force cert renewal; only run if the cert is < 30 days from expiry. Non-destructive.
   - `migrations_apply` — runs any pending SQLite migrations. Non-destructive.
   - `log_redactor_reload` — re-reads the redactor rules from `sovereign.toml`. Non-destructive.
   - `egress_allowlist_apply` — re-renders the Caddy egress filter and reloads. Non-destructive.
2. **Implement the fix orchestrator** in `sovereign-doctor/src/fixes/orchestrator.rs`:
   - Reads every check's `CheckResult.fix`.
   - Groups fixes by category.
   - Orders: non-destructive first, destructive last, cheapest first.
   - For each destructive fix, prints a warning, waits 5s, runs unless the operator types `n` or `Ctrl+C`.
   - Writes an audit event for every fix attempt: `kind: "doctor.fix"`, `payload: {fix_id, before, after, status}`.
   - Reports the cumulative result: `Fixed 7/9 (2 required confirm and were skipped)`.
3. **Implement `--dry-run`** — prints what `--fix` would do, runs nothing.
4. **Implement `--fix=<id>`** — runs only the named fix (e.g., `sovereign doctor --fix=backup_run_now`).
5. **Implement `--fix --non-interactive`** — for use in CI / cron. Skips the confirms but **skips destructive fixes** with a clear "skipped destructive: enable_firewall" line.
6. **Add a CI test**: a broken fixture is doctor'd with `--fix --non-interactive` and the post-fix doctor returns 0 fails (with the destructive ones reported as "skipped").

### Code stub

```rust
// crates/sovereign-doctor/src/fixes/orchestrator.rs
use crate::{DoctorFix, FixOutcome};
use std::time::Duration;

pub struct FixOrchestrator {
    pub fixes: Vec<Box<dyn DoctorFix>>,
    pub interactive: bool,
    pub dry_run: bool,
}

impl FixOrchestrator {
    pub async fn run(&self, ctx: &DoctorContext) -> FixReport {
        let mut report = FixReport::new();
        let mut ordered = self.fixes.clone();
        ordered.sort_by_key(|f| (f.is_destructive(), f.estimated_cost_ms()));
        for fix in ordered {
            if self.dry_run {
                report.dry_run(fix.id(), fix.description());
                continue;
            }
            if fix.is_destructive() {
                if self.interactive {
                    eprintln!("\n⚠ Fix '{}' is destructive: {}", fix.id(), fix.description());
                    eprint!("Apply? (y/n, 5s timeout): ");
                    if !confirm_with_timeout(Duration::from_secs(5)).await {
                        report.skipped(fix.id());
                        continue;
                    }
                } else {
                    report.skipped_destructive(fix.id());
                    continue;
                }
            }
            let outcome = fix.apply(ctx).await;
            report.record(fix.id(), &outcome);
            ctx.audit.record(format!("doctor.fix.{}", fix.id()), &outcome).await?;
        }
        report
    }
}
```

### Tests

- Unit: each fix has a unit test that asserts the post-state matches the documented "after" state.
- Integration: a broken fixture is `doctor --fix`'d; the post-doctor returns 0 fails (except for the destructive-skipped ones, which are reported as "skipped").
- Integration: `doctor --fix --non-interactive` against the same fixture skips destructive and reports them.
- Integration: `doctor --fix=backup_run_now` runs only that one fix; all others are reported as "skipped (not requested)".
- Audit: every fix writes an audit event with before/after.

### Acceptance criteria

- [ ] `sovereign doctor --level standard --fix` resolves ≥ 70% of standard-level fails without human action.
- [ ] Destructive fixes always require a confirm in interactive mode and are skipped in `--non-interactive`.
- [ ] Every fix writes an audit event with before/after.

### Definition of done

A 4-hour soak test: a fixture box with 9 known fails is doctor'd with `--fix --non-interactive`. After the run, the post-doctor reports 0 fails and 2 destructive-skipped. The audit log shows 7 fix events with before/after.

---

## G18. `sovereign doctor --explain` — KB lookup, "why is this failing?", and a documentation link

**Goal:** Every check's "why is this failing?" message links to a dedicated KB article in the docs site. The KB is a curated set of 40+ pages, one per check, explaining the failure mode, the remediation, the operator's mental model, and the deep links to the architecture and runbook sections.

**Why it matters:** The V0 doctor's output is "✗ backup_recent: last backup was 27h ago." That's enough for an SRE. It's not enough for a solo founder at 3am who has never seen a backup cron fail. `--explain` transforms the output from "this is broken" to "this is broken, here's why, here's the fix, here's the runbook." It's the difference between a tool and a product.

### Sub-tasks

1. **Add the `--explain` flag** to `sovereign doctor` (any level):
   - On a `Fail` or `Warn` check, prints an additional block: `Why: <2-3 sentences>, Fix: <the fix id>, Runbook: <link to the KB page>, Reference: <link to the architecture section>`.
   - On a `Pass` check, prints nothing extra (no spam).
2. **Implement `sovereign doctor kb`** — a subcommand that lists all 40+ KB articles, browsable as a `mdbook` directory:
   ```text
   $ sovereign doctor kb
   Available KB articles:
   - backup_recent          Why your last backup is overdue
   - backup_offsite         Why offsite backup matters
   - cgroups_v2             What cgroups v2 is and why you need it
   - ...
   $ sovereign doctor kb backup_recent
   # Why your last backup is overdue
   ... (full KB article, terminal-friendly markdown) ...
   ```
3. **Author the 40+ KB articles** as markdown in `docs/src/kb/doctor/`:
   - One file per check: `docs/src/kb/doctor/<category>_<check>.md`.
   - Each article: frontmatter (`id`, `category`, `severity`, `introduced_in`, `related_checks`), then sections `# Why this check exists`, `# What it means when it fails`, `# How to fix it`, `# When to ignore it`, `# Related`, `# References`.
   - The articles are auto-collected by `mdbook` and indexed in the docs site.
4. **Implement `sovereign doctor --explain <check_id>`** — a focused lookup: prints the KB article and exits. `--json` returns the article as structured data.
5. **Add a CI test**: `sovereign doctor kb | wc -l` returns ≥ 40 lines. `sovereign doctor kb backup_recent` returns a non-empty markdown body.
6. **Wire `--explain` into the `--report`** — the markdown report includes the KB link per failing check.

### Code stub

```rust
// crates/sovereign-doctor/src/explain.rs
pub struct ExplainEngine {
    kb_dir: std::path::PathBuf,
    cache: moka::sync::Cache<String, Arc<str>>,
}
impl ExplainEngine {
    pub fn new(kb_dir: impl Into<std::path::PathBuf>) -> Self { ... }
    pub async fn explain(&self, check_id: &str) -> Result<Explanation, ExplainError> {
        let path = self.kb_dir.join(format!("{check_id}.md"));
        let raw = tokio::fs::read_to_string(&path).await
            .map_err(|_| ExplainError::NotFound(check_id.to_string()))?;
        let parsed = pulldown_cmark::Parser::new(&raw);
        // ... extract frontmatter + first paragraph + sections ...
        Ok(Explanation { ... })
    }
}
```

### Tests

- Unit: `explain` on a missing check returns `ExplainError::NotFound`.
- Unit: `explain` on a real check returns a structured `Explanation` with the expected sections.
- Integration: `sovereign doctor kb` lists ≥ 40 entries.
- Integration: `sovereign doctor kb backup_recent` returns a non-empty markdown body.
- CI gate: the KB directory is checked in; a job verifies the count is ≥ 40 and each article has the expected frontmatter.

### Acceptance criteria

- [ ] Every standard-level check has a KB article.
- [ ] `sovereign doctor --explain <check>` returns the article (terminal-friendly markdown).
- [ ] `sovereign doctor kb` lists all articles.
- [ ] The docs site has a `/kb/doctor/` section that is auto-built.

### Definition of done

A 5-user review: 5 design partners are given a "broken" box and the KB, asked to diagnose and fix 3 fails. ≥ 4 of 5 resolve all 3 within 30 minutes without opening a support ticket.

---

## G19. `sovereign bench` — reproducible performance benchmark

**Goal:** A `sovereign bench` subcommand that runs a standardized set of micro- and macro-benchmarks against the local install, producing a `bench-<date>.json` artifact and a markdown summary. The benchmarks are the same suite the CI runs on every release, so the operator can compare their box's numbers against the published baseline.

**Why it matters:** A solo operator doesn't have Grafana dashboards with 30-day history. They have `sovereign doctor --explain` and they need a way to know "is my box slower than it was last month?" `sovereign bench` is the answer. It's also the substrate for the V2 ML scorer (which needs a baseline of "normal" for the box).

### Sub-tasks

1. **Implement the bench harness** in `sovereign-doctor/src/bench/`:
   - `sovereign bench` runs the full suite: deploy latency, rollback latency, TLS handshake, health check latency, deploy throughput, log search latency, backup create/verify, doctor basic level, doctor standard level.
   - Each bench prints a single line: `deploy_latency: 87ms (baseline: 92ms ± 5ms, pass)`.
   - Writes `/var/log/sovereign/bench-<timestamp>.json` with the full per-iter data.
   - Writes `/var/log/sovereign/bench-<timestamp>.md` with a markdown summary comparing to the published baseline.
2. **Implement the per-bench modules**:
   - `bench::deploy_latency` — runs `sovereign deploy` against a fixture app 10 times, reports p50/p95/p99.
   - `bench::rollback_latency` — runs `sovereign rollback` 10 times.
   - `bench::tls_handshake` — `curl --resolve` 100 times, reports p50/p95.
   - `bench::health_check_latency` — hits the local Caddy 1000 times.
   - `bench::log_search_latency` — runs `sovereign logs --app <fixture> --since 1h --grep error` 10 times.
   - `bench::backup_create_verify` — runs `backup create` and `backup verify` against a fixture Postgres.
   - `bench::doctor_basic` — runs `doctor --level basic` 10 times.
   - `bench::doctor_standard` — runs `doctor --level standard` 3 times.
3. **Publish the baseline** as part of every release: `releases.sovereignruntime.dev/<version>/bench-baseline.json`. The local bench downloads the matching baseline (or the latest) and reports the comparison.
4. **Implement `sovereign bench --compare <baseline>`** — uses a different baseline (e.g., the last 3 months of this box's history).
5. **Implement `sovereign bench history`** — lists the last 20 bench results from `/var/log/sovereign/bench-*.json` with the trend per metric.
6. **Add a CI job** that runs the full suite on every release; the result is published as a GitHub Actions artifact and as the new baseline.
7. **Wire the bench into the morning report (G11)** — a 1-line "deploy latency: 87ms (was 92ms last week)" line in the report.

### Code stub

```rust
// crates/sovereign-doctor/src/bench/mod.rs
use std::time::Instant;
pub struct Bench {
    name: &'static str,
    iter: usize,
    warmup: usize,
    baseline: Option<BenchBaseline>,
}
impl Bench {
    pub async fn run(&self, ctx: &BenchContext) -> BenchResult {
        for _ in 0..self.warmup { self.single(ctx).await; }
        let mut samples = Vec::with_capacity(self.iter);
        for _ in 0..self.iter {
            let start = Instant::now();
            self.single(ctx).await;
            samples.push(start.elapsed());
        }
        let p50 = percentile(&samples, 0.50);
        let p95 = percentile(&samples, 0.95);
        let p99 = percentile(&samples, 0.99);
        let verdict = self.baseline.as_ref().map(|b| b.compare(p50, p95, p99));
        BenchResult { name: self.name, p50, p95, p99, samples, verdict }
    }
    async fn single(&self, _ctx: &BenchContext) { /* ... */ }
}
```

### Tests

- Unit: `percentile` on a known vector returns the expected value.
- Integration: `sovereign bench` against a fresh fixture runs all 8 benches in < 10 min and produces the JSON+MD artifacts.
- Integration: `sovereign bench history` lists the last 20 results.
- CI gate: a release job runs the full suite and uploads the baseline.

### Acceptance criteria

- [ ] `sovereign bench` runs the full suite in < 10 min on a CX22.
- [ ] The JSON artifact is reproducible (same fixture → same numbers within ± 5%).
- [ ] The MD artifact compares to the published baseline.
- [ ] `sovereign bench history` shows the trend per metric.

### Definition of done

A 4-week soak: a design partner runs `sovereign bench` weekly. The `history` view shows a stable trend. A deliberately degraded box (CPU throttled to 50%) shows a > 20% regression in `deploy_latency` within the same week.

---

## G20. `sovereign cost` — local cost attribution + advisory

**Goal:** A `sovereign cost` subcommand that answers "what is my bill?" using only local data (Docker stats, SQLite sizes, S3 API costs pulled from the bucket's billing if configured, the Hetzner/cloud API if a token is configured). The output is **advisory** — never the source of truth for an actual invoice — but it is accurate enough that the operator can spot a 10× regression in 30 seconds.

**Why it matters:** A solo operator on a $5/mo Hetzner box doesn't need cost attribution. A team operator running 12 apps on a $200/mo box does. A mid-market operator on a $5,000/mo cluster does too. The cost subcommand is the daily-driver check that catches the silent cost regressions (an idle app still running for 2 weeks, a backup that's 100× larger than it should be, an egress spike from a misconfigured Caddy).

### Sub-tasks

1. **Implement the cost model** in `sovereign-doctor/src/cost/`:
   - **Compute** — sum of `docker stats` CPU% × time × the host's $/vCPU-hour (default $0.0064, configurable in `sovereign.toml`).
   - **Memory** — sum of `docker stats` MEM × time × the host's $/GB-hour (default $0.0042).
   - **Storage** — sum of `du -sh /var/lib/sovereign` × $/GB-month for the attached volume (default $0.10).
   - **Backups** — sum of S3 object sizes (if the backup target is S3-compatible and credentials are configured) × $/GB-month.
   - **Egress** — sum of Caddy's access log bytes (last 30 days) × $/GB egress (default $0.01, configurable).
   - **Idle apps** — any app with < 1 request/min over the last 7 days is listed with a "stop?" suggestion (advisory only, never auto-stopped).
2. **Implement `sovereign cost`** CLI:
   - `sovereign cost` — prints the current month's running total with a per-category breakdown.
   - `sovereign cost --month 2025-11` — historical month.
   - `sovereign cost --app <name>` — per-app breakdown.
   - `sovereign cost --json` — structured output.
3. **Implement `sovereign cost history`** — last 12 months per category.
4. **Wire the cost into the morning report (G11)** — a 1-line "estimated MTD cost: $47.20 (was $52.10 last week)" line.
5. **Wire the cost into the doctor cost checks (G16)** — the cost subcommand's data feeds the `backup_size_reasonable`, `bandwidth_reasonable`, `idle_apps_flagged` checks.
6. **Privacy**: the cost subcommand is strictly local. The cloud-provider API calls (Hetzner, S3) are **opt-in** via `sovereign.toml`; without the config, the cost model uses the defaults and warns "cost is estimated; configure [cost] in sovereign.toml for accurate numbers."
7. **Add a CI test**: `sovereign cost --app <fixture>` against a fixture with a known workload reports the expected per-app total within ± 10%.

### Code stub

```rust
// crates/sovereign-doctor/src/cost/mod.rs
use chrono::{Datelike, NaiveDate};
pub struct CostModel {
    pub cpu_per_vcpu_hour: f64,
    pub memory_per_gb_hour: f64,
    pub storage_per_gb_month: f64,
    pub egress_per_gb: f64,
}
impl CostModel {
    pub fn defaults() -> Self { Self {
        cpu_per_vcpu_hour: 0.0064, memory_per_gb_hour: 0.0042,
        storage_per_gb_month: 0.10, egress_per_gb: 0.01,
    } }
    pub fn from_config(c: &sovereign_core::Config) -> Self { ... }
    pub fn monthly(&self, usage: &UsageAggregate) -> CostBreakdown { ... }
}
pub struct CostBreakdown {
    pub compute: f64, pub memory: f64, pub storage: f64,
    pub backups: f64, pub egress: f64, pub total: f64,
}
```

### Tests

- Unit: `CostModel::monthly` on a known `UsageAggregate` returns the expected breakdown.
- Integration: a fixture box with 3 apps and a known workload reports the expected totals.
- Integration: `sovereign cost --app <fixture>` returns a per-app breakdown that sums to the total.
- Privacy: with no `[cost]` config, the cost subcommand uses defaults and warns.
- CI gate: a job runs `sovereign cost` against the standard fixture and asserts the total is within the expected range.

### Acceptance criteria

- [ ] `sovereign cost` prints the MTD total with a per-category breakdown in < 5s.
- [ ] `sovereign cost --app <name>` prints a per-app breakdown.
- [ ] `sovereign cost history` shows 12 months.
- [ ] The morning report includes a 1-line cost summary.
- [ ] The cost subcommand is strictly local; cloud-provider API calls are opt-in.

### Definition of done

A 4-week soak: 3 design partners with production workloads run `sovereign cost` weekly. At least 1 of the 3 finds a silent cost regression (an idle app, a runaway backup, a misconfigured Caddy cache) and resolves it. The cost subcommand's estimate is within ± 20% of the actual cloud bill for ≥ 2 of the 3.

---

## G21. `sovereign auth` — OIDC SSO via Authentik / Keycloak / Zitadel / Auth0 / GitHub

**Goal:** `sovereign auth` is a *consumer* of OIDC providers, not a provider. The four RBAC roles (H3) can be mapped to OIDC groups from the operator's existing IdP, so a 50-engineer team uses Okta / Azure AD / Google Workspace for sign-in and Sovereign inherits the SSO / MFA / SCIM the org already pays for. This is the prerequisite for the Enterprise tier (Procurement buyer asks "is this SAML / OIDC?" and the answer is "yes, against the IdP of your choice").

**Why it matters:** Persona-cto.md §2.5 ("Identity / SSO — Delegate entirely. Integrate with Authentik, Keycloak, Zitadel, Auth0, GitHub OIDC. Do not build an IdP. The 'build an IdP' trap has killed a dozen PaaS projects. Be an OIDC consumer, never a provider") is the load-bearing position. We do **not** build an IdP. We do **not** own user passwords. We do **not** store SSO sessions in our SQLite. The OIDC provider is the source of truth for identity; Sovereign is a consumer.

**What this is NOT:** Not a custom login page. Not a password store. Not an MFA implementation. Not a SAML provider. (SAML is a fallback for procurement offices that mandate it; the `sovereign auth saml` subcommand is a thin wrapper around `samltest.id` / `saml2` crate that delegates the assertion validation to the same `OIDC claims → role mapping` pipeline.)

### Sub-tasks

1. **Implement the `OIDCClient` in `sovereign-auth/`** (a separate crate; not in `sovereign-core`):
   - Uses the `openidconnect` crate (3.x) for the OIDC flow (authorization code with PKCE, no client secret for the public client; confidential client for the service-to-service case).
   - Standard `/.well-known/openid-configuration` discovery — works with Authentik, Keycloak, Zitadel, Auth0, Google, Microsoft Entra ID, Okta, GitHub, GitLab, Bitbucket.
   - **5-minute first-login** UX: `sovereign auth login` opens the system browser, completes the device-code or PKCE flow, stores the refresh token in the V0 secret store (F7), encrypted with the same age master key as every other secret. The user is logged in for the duration of the token's lifetime (default 1h access, 30d refresh, refresh token rotation enabled).
2. **Implement the `role_map` config** (a TOML file at `/etc/sovereign/auth.toml`):
   ```toml
   # auth.toml — RBAC ↔ OIDC group mapping
   [provider]
   issuer = "https://authentik.mirasaas.com/application/o/sovereign/"
   client_id = "sovereign-cli"
   # client_secret is read from the secret store, not the file:
   #   sovereign secret set OIDC_CLIENT_SECRET --from-stdin
   scopes = ["openid", "profile", "email", "groups"]

   [role_map]
   # The IdP group "sovereign-owners" → RBAC role "owner"
   "sovereign-owners"   = "owner"
   "sovereign-admins"    = "admin"
   "sovereign-devs"      = "developer"
   "sovereign-readonly"  = "readonly"

   [session]
   access_token_ttl = "1h"
   refresh_token_ttl = "30d"
   enforce_mfa = true   # require the IdP-asserted `amr` claim includes `mfa`
   ```
   The role map is hot-reloaded (`SIGHUP` to the binary). Empty `role_map` = "no OIDC users get any role, only the local admin (F9 fallback) can do anything."
3. **Implement the local admin fallback** — a `sovereign auth local-admin` command that creates a bcrypt-hashed password in `/etc/sovereign/auth.local` (mode 0600). This is the break-glass account; it survives OIDC provider outage (the user's IdP goes down, the operator can still `ssh sovereign@box && sovereign auth local-admin` to log in). The local admin is **always** role `owner`.
4. **Implement the `sovereign auth status`** subcommand — shows the current user, the IdP claims (`sub`, `email`, `groups`, `amr`), the resolved RBAC role, the token expiry. `--json` for piping.
5. **Implement the `sovereign auth logout`** subcommand — revokes the refresh token at the IdP (RFC 7009) and clears the local encrypted store. Does **not** delete the user from the IdP (we don't own that data).
6. **Wire RBAC enforcement** — every use case in `sovereign-core` consults the `AuthPort::current_role()` before mutation. The H3 RBAC table (owner / admin / developer / readonly) is the source of truth. The role is read from the OIDC claims (via the role_map) or from the local admin file. **No "anonymous" or "guest" role exists.** A request with no valid token returns HTTP 401; with a valid token but no mapped role, HTTP 403.
7. **Implement the `sovereign auth saml` wrapper** for the procurement case (BSI / ANSSI / AgID sometimes mandate SAML, not OIDC):
   - Uses the `saml2` crate (Rust) to parse the SAML assertion.
   - Maps the SAML attribute `urn:oid:1.3.6.1.4.1.5923.1.5.1.1` (the `isMemberOf` attribute) through the same `role_map` table.
   - The `sovereign auth saml login` flow opens the IdP login page, accepts the POSTed SAML response, validates the signature against the IdP's published metadata, extracts the attributes, and resolves the RBAC role.
   - **SAML is a wrapper around OIDC's role-map logic** — there is exactly one role-resolution code path.
8. **Audit log integration** — every `auth.login`, `auth.logout`, `auth.role_resolved` event goes to the audit table (H4). The audit row records: `user.sub`, `user.email`, `user.groups`, `resolved_role`, `client_ip`, `user_agent`, `amr` (the authentication method reference, e.g. `pwd mfa`). The audit log is the record the auditor reads.
9. **Add the 3 new standard-level doctor checks** (G16 §3.15 cross-cutting):
   - `oidc_provider_configured` — `Warn` if no provider is set and the operator has > 1 user.
   - `oidc_token_refresh_fresh` — `Fail` if the refresh token is < 7 days from expiry (warns before the user is locked out at 03:00).
   - `local_admin_present` — `Fail` if the local admin is not set (the break-glass account is mandatory for SOC2 / ISO 27001 / BSI C5 / EUCS).
   - `mfa_enforced_for_remote` — `Warn` if `enforce_mfa = false` and any role is `owner` or `admin`. (The operator can opt out, but the warning is loud.)
10. **Document the IdP setup** in `docs/integrations/{authentik,keycloak,zitadel,auth0,okta,entra,google,github}.md` — one page per provider, with screenshots of the IdP admin UI and the exact `sovereign auth` command to wire it up.

### Code stub

```rust
// crates/sovereign-auth/src/oidc.rs
use openidconnect::{ClientId, IssuerUrl, RedirectUrl, Scope, AuthenticationFlow, CsrfToken, Nonce};
use openidconnect::reqwest::async_http_client;

pub struct OidcClient {
    provider: ProviderConfig,
    role_map: HashMap<String, Role>,
    enforce_mfa: bool,
    local_store: Arc<dyn SecretStore>,
}

impl OidcClient {
    pub async fn login_pkce(&self) -> Result<AuthSession, AuthError> {
        // 1. Build the authorization URL (PKCE, no client secret)
        let (auth_url, csrf_state, _nonce) = self.provider.client
            .authorize_url(
                AuthenticationFlow::<()>::AuthorizationCode,
                CsrfToken::new_random,
                Nonce::new_random,
            )
            .set_redirect_uri(RedirectUrl::new("http://127.0.0.1:8484/callback".into())?)
            .add_scope(Scope::new("openid".into()))
            .add_scope(Scope::new("profile".into()))
            .add_scope(Scope::new("email".into()))
            .add_scope(Scope::new("groups".into()))
            .url();

        // 2. Open the system browser
        open::that(auth_url.as_str())?;

        // 3. Start a localhost listener on 127.0.0.1:8484, wait for the redirect
        let (tx, rx) = tokio::sync::oneshot::channel();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:8484").await?;
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await?;
            // ... parse the redirect, send the response, send (code, state) to tx
        });

        // 4. Exchange the code for tokens
        let token_response = self.provider.client
            .exchange_code(AuthorizationCode::new(code))
            .request_async(async_http_client)
            .await?;

        // 5. Verify the ID token, extract groups, resolve role
        let id_token = token_response.id_token().ok_or(AuthError::NoIdToken)?;
        let claims = id_token.claims(&self.provider.id_token_verifier, &nonce)?;
        let groups: Vec<String> = claims.additional_claims()
            .get("groups").and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        let role = self.resolve_role(&groups);

        // 6. Enforce MFA if configured
        if self.enforce_mfa {
            let amr: Vec<String> = claims.additional_claims()
                .get("amr").and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            if !amr.iter().any(|m| m == "mfa" || m == "totp" || m == "webauthn") {
                return Err(AuthError::MfaRequired);
            }
        }

        // 7. Persist the refresh token (encrypted) in the V0 secret store
        self.local_store.set("oidc_refresh_token", &token_response.refresh_token().unwrap().secret()).await?;
        Ok(AuthSession { role, expires_at: token_response.expires_in().unwrap() + Utc::now() })
    }

    fn resolve_role(&self, groups: &[String]) -> Role {
        // First match wins; no match → "no role" → 403
        for g in groups {
            if let Some(role) = self.role_map.get(g) {
                return role.clone();
            }
        }
        Role::None
    }
}
```

### Tests

- Unit: a fixture IdP mock (uses `wiremock` to serve `/.well-known/openid-configuration`, the JWKS, and a canned token response) is the test backend. The role resolution is tested against 5 group sets: empty, `sovereign-owners`, `sovereign-admins sovereign-readonly` (first match wins), `unknown-group` (→ None → 403), and a `sovereign-devs` + `mfa=true` (passes) vs no `amr` claim (fails).
- Integration: a full `sovereign auth login` against the wiremock IdP returns a valid session, the audit table has 1 row, the role is resolved correctly.
- Integration: `sovereign auth saml login` against a SAML fixture (the `saml2` crate's test backend) returns the same role as the OIDC path for the same group.
- Integration: local admin fallback works when the OIDC provider is unreachable (kill the wiremock, the local admin still logs in).
- Doctor: 3 new checks (oidc_provider_configured, oidc_token_refresh_fresh, local_admin_present) pass on a fresh fixture; the 4th (`mfa_enforced_for_remote`) warns when `enforce_mfa = false`.

### Acceptance criteria

- [ ] `sovereign auth login` opens the system browser, completes the PKCE flow, and persists the refresh token in the encrypted store.
- [ ] `sovereign auth status` shows the current user, IdP claims, resolved role, and token expiry.
- [ ] `sovereign auth logout` revokes the refresh token at the IdP and clears the local store.
- [ ] `sovereign auth saml login` works against a test SAML IdP and resolves the same role as the OIDC path.
- [ ] The `role_map` is hot-reloaded on `SIGHUP`.
- [ ] The local admin fallback works when the OIDC provider is unreachable.
- [ ] Every login / logout / role-resolved event is written to the audit table.
- [ ] 3 new doctor checks are present in `--level standard`.

### Definition of done

A 4-week soak: 3 design partners (one with Authentik, one with Okta, one with Microsoft Entra ID) run `sovereign auth login` against their prod IdP. All 3 successfully map at least 2 OIDC groups to RBAC roles. At least 1 of the 3 reports using `enforce_mfa = true` and the audit log records the `mfa` method on every login. The local admin fallback is tested by killing the IdP network and confirming `sovereign auth local-admin` still works.

---

## G22. `sovereign log ship` — JSON log export to Loki / Datadog / Better Stack / Vector / syslog

**Goal:** Every JSON log line the binary writes (audit events, doctor state changes, deploy events, app stdout/stderr) is also shipped to a centralized log destination the operator chooses. The binary does not embed Elasticsearch, Loki, or a log database. It writes JSON to stdout (or to `journald` on systemd) and ships via a configurable sink.

**Why it matters:** Persona-cto.md §2.5 ("Logging aggregation — Delegate. Export JSON logs to Loki / Datadog / Better Stack / Vector pipeline. We do not embed Elasticsearch") and user-pain-research.md §5 (the LGTM stack requires a platform engineer; the median cost of self-hosting Mimir+Loki+Tempo+Grafana is a part-time platform engineer). For a 1-person team, the answer is "Datadog free tier" or "Better Stack free tier" or "self-hosted Vector pipeline to Loki." For an MNC, the answer is "Splunk / Datadog / Sumo Logic / a SIEM." The binary does not pick. It emits structured JSON and ships it.

### Sub-tasks

1. **Make all log output structured JSON** (this is a refactor of the V0 logger, already partial in G3):
   - Every `tracing` event with a `target` starting with `sovereign::` is serialized as `{"ts": "2026-06-04T14:22:03.123Z", "level": "INFO", "target": "sovereign::doctor", "msg": "backup_recent: PASS → FAIL", "fields": {...}}`.
   - The default sink is `stdout` for the foreground process and `journald` for the systemd service (auto-detected: if `INVOCATION_ID` env var is set, journald; else stdout).
   - **No human-readable text in production logs.** The TUI / `--human` flag is for `sovereign doctor` / `sovereign logs --human` only; the canonical log line is JSON.
2. **Implement the `sovereign log ship` subcommand** (interactive setup):
   ```bash
   $ sovereign log ship
   ? Where do you want to send logs?
     > Loki (self-hosted)
       Datadog (SaaS)
       Better Stack (SaaS)
       Splunk / HEC (on-prem)
       Sumo Logic (SaaS)
       Syslog (RFC 5424) — for SIEM forwarding
       Vector pipeline (BYO config)
       Disable shipping
   ? Endpoint: https://logs.betterstack.com/...
   ? Source token: <paste>
   ✓ Log shipping configured. Last 5 lines shipped OK.
   ```
   The destination config is stored in `/etc/sovereign/log-ship.toml` (mode 0600). The binary tails its own JSON output and ships to the configured sink.
3. **Implement the sinks** (one file per sink in `sovereign-log-ship/src/sinks/`):
   - `loki.rs` — POST JSON to `/loki/api/v1/push` with basic auth (per `loki.rs` crate or hand-rolled `reqwest`).
   - `datadog.rs` — POST to `https://http-intake.logs.datadoghq.com/api/v2/logs` with `DD-API-KEY` header (per the Datadog Logs API spec).
   - `betterstack.rs` — POST to the Better Stack source URL with the source token as a header.
   - `splunk_hec.rs` — POST to the Splunk HEC endpoint with the HEC token.
   - `sumo.rs` — POST to the Sumo Logic HTTP source endpoint.
   - `syslog.rs` — RFC 5424 over TCP/TLS to a syslog server (uses the `syslog` crate or hand-rolled).
   - `vector.rs` — emit to a unix socket / TCP port; the operator's Vector pipeline picks it up.
   - All 7 sinks implement the `LogSink` trait; the binary picks the configured one.
4. **Implement batching and back-pressure**:
   - Buffers 1 MB or 1000 events, whichever first; flushes every 5s; flushes on shutdown (SIGTERM).
   - On a 5xx or network error, retries with exponential backoff (1s, 2s, 4s, ..., 60s max).
   - On 401/403, marks the sink as `disabled` and emits a `log_sink_disabled` audit event (so the operator notices).
   - The `sovereign log ship status` subcommand shows: sink type, last successful ship, last error, queue depth.
5. **Implement log redaction** (privacy-by-construction):
   - A redactor scans every log line for known secret patterns (the same patterns as F7's pre-commit hook: AWS keys, GitHub tokens, JWTs, private keys, age secret keys, the binary's master key fingerprint).
   - Redacted fields are replaced with `***REDACTED***` *before* the line hits the sink. The redactor runs in a `tracing-subscriber` layer so the redaction is applied at the source — no log line ever leaves the box unredacted.
   - The doctor check `log_redactor_active` (G16) verifies the redactor is on and the test pattern is not present in the last 1000 events.
6. **Implement `sovereign log ship --test`** — sends a synthetic test event to the configured sink and reports the HTTP status code. Used by the operator to verify the config without waiting for a real event.
7. **Add the 2 new standard-level doctor checks**:
   - `log_ship_configured` — `Warn` if the operator has the Pro or Enterprise tier and no log ship sink is set. (Community tier: not enforced.) The check looks at `/etc/sovereign/license.json` and `/etc/sovereign/log-ship.toml`.
   - `log_ship_healthy` — `Fail` if the sink is set but the last successful ship was > 1h ago, or if the queue depth is > 10 MB (back-pressure threshold).
8. **Document the 7 sinks** in `docs/integrations/{loki,datadog,betterstack,splunk,sumo,syslog,vector}.md` — one page per sink with the exact config and the SOC 2 / ISO 27001 controls it satisfies.

### Code stub

```rust
// crates/sovereign-log-ship/src/lib.rs
use async_trait::async_trait;

#[async_trait]
pub trait LogSink: Send + Sync {
    fn name(&self) -> &'static str;
    async fn ship(&self, batch: &[LogEvent]) -> Result<ShipOutcome, ShipError>;
    async fn health(&self) -> Result<HealthStatus, ShipError>;
}

pub struct LogShipper {
    sink: Box<dyn LogSink>,
    buffer: tokio::sync::mpsc::Sender<LogEvent>,
    redactor: Redactor,
}

impl LogShipper {
    pub fn spawn(config: LogShipConfig, redactor: Redactor) -> Self {
        let (tx, mut rx) = tokio::sync::mpsc::channel(10_000);
        let sink: Box<dyn LogSink> = match config.sink {
            SinkType::Loki { url, auth } => Box::new(sinks::LokiSink::new(url, auth)),
            SinkType::Datadog { api_key, site } => Box::new(sinks::DatadogSink::new(api_key, site)),
            SinkType::BetterStack { source_url, token } => Box::new(sinks::BetterStackSink::new(source_url, token)),
            SinkType::SplunkHec { url, token } => Box::new(sinks::SplunkHecSink::new(url, token)),
            SinkType::Sumo { url } => Box::new(sinks::SumoSink::new(url)),
            SinkType::Syslog { addr, tls } => Box::new(sinks::SyslogSink::new(addr, tls)),
            SinkType::Vector { endpoint } => Box::new(sinks::VectorSink::new(endpoint)),
            SinkType::Disabled => Box::new(sinks::NullSink),
        };
        // Background task: batch + ship + retry
        tokio::spawn(async move {
            let mut batch = Vec::with_capacity(1000);
            let mut interval = tokio::time::interval(Duration::from_secs(5));
            loop {
                tokio::select! {
                    Some(event) = rx.recv() => {
                        let event = redactor.redact(event);
                        batch.push(event);
                        if batch.len() >= 1000 { Self::flush(&sink, &mut batch).await; }
                    }
                    _ = interval.tick() => { if !batch.is_empty() { Self::flush(&sink, &mut batch).await; } }
                }
            }
        });
        Self { sink, buffer: tx, redactor }
    }

    pub async fn ship(&self, event: LogEvent) {
        let _ = self.buffer.send(event).await;  // back-pressure: block if buffer full
    }

    async fn flush(sink: &Box<dyn LogSink>, batch: &mut Vec<LogEvent>) {
        let outcome = sink.ship(batch).await;
        // exponential backoff on failure; audit log on persistent failure
    }
}
```

### Tests

- Unit: a `MockSink` records shipped events; the redactor strips known patterns; the batcher flushes at 1000 events and at 5s; back-pressure blocks the producer when the buffer is full.
- Integration: each of the 7 sinks is tested against its real endpoint with a `wiremock` backend returning the expected HTTP status codes. 200 → success. 401 → sink disabled + audit event. 503 with retry-after → exponential backoff.
- Integration: `sovereign log ship --test` returns 200 from each sink (mocked).
- Doctor: `log_ship_configured` warns on a Pro license with no sink; `log_ship_healthy` fails on a sink with last-ship > 1h ago.

### Acceptance criteria

- [ ] All log output is structured JSON.
- [ ] `sovereign log ship` interactively configures one of 7 sinks and stores the config in `/etc/sovereign/log-ship.toml`.
- [ ] Batching (1 MB / 1000 events / 5s) and exponential backoff work end-to-end.
- [ ] The redactor strips the 6 documented secret patterns before the line hits the sink.
- [ ] `sovereign log ship --test` returns 200 from each configured sink.
- [ ] 2 new doctor checks (`log_ship_configured`, `log_ship_healthy`) are present in `--level standard`.

### Definition of done

A 4-week soak: 3 design partners (one with Loki self-hosted, one with Datadog, one with Splunk HEC) run `sovereign log ship` against their prod log destination. All 3 report every deploy, doctor state change, and audit event arriving in their SIEM with < 30s lag. At least 1 of the 3 uses the redactor to verify a known secret pattern is stripped. The `log_ship_healthy` doctor check passes on all 3.

---

## G23. `sovereign audit` — append-only, signed, exportable audit log

**Goal:** The audit log is **append-only** (no `UPDATE` or `DELETE` ever runs against the `audit` table), **signed** (each event is hashed with a per-box Ed25519 key and the chain is verifiable), and **exportable** to a CSV / JSONL / Parquet file that an auditor can read without the binary. The export is the canonical record a SOC 2 / ISO 27001 / BSI C5 / EUCS auditor signs off on.

**Why it matters:** SOC 2 CC7.2 (System Operations — monitoring of system components) and ISO 27001 A.12.4.1 (Event logging) both require an *immutable* audit trail. SQLite's `WITHOUT ROWID` + append-only enforcement is the mechanism; the Ed25519 chain is the cryptographic proof; the export is the deliverable.

### Sub-tasks

1. **Enforce append-only at the SQLite layer**:
   - The `audit` table is created with `WITHOUT ROWID` and a `PRIMARY KEY (id INTEGER PRIMARY KEY)` so the only way to insert is `INSERT`.
   - The database connection used by the audit writer has `PRAGMA query_only = ON` *for everything except the `INSERT`* — implemented as a separate `audit-writer` connection with a custom VFS that rejects all non-INSERT statements. The VFS lives in `sovereign-audit/src/vfs_appendonly.rs`.
   - The H4 audit log query command (`sovereign audit --query ...`) uses a *different* connection that has read-only access; the writer connection never executes a `SELECT`.
2. **Implement the Ed25519 chain**:
   - On first boot, the binary generates a per-box Ed25519 keypair (`/var/lib/sovereign/audit-signing.key`, mode 0600, never leaves the box). The public key is published at `/.well-known/sovereign-audit-key.pub` for auditors to fetch.
   - Each audit row has a `prev_hash` column (the SHA-256 of the previous row, in the canonical order) and a `signature` column (the Ed25519 signature of `prev_hash || row_canonical_json`).
   - The chain is verified by `sovereign audit verify` — replays the rows in order, re-computes the hashes, and checks the signatures against the public key. A single byte of tampering breaks the chain and is reported with the row number.
3. **Implement the 3 export formats**:
   - **CSV** — for SIEM ingestion (Splunk, Elastic, QRadar). Columns: `ts, kind, actor, target, prev_hash, signature, payload_json`.
   - **JSONL** — for `jq` and Datadog log shipping. One event per line, the same schema as the live log ship.
   - **Parquet** — for long-term cold storage (S3 + Athena / Snowflake). Uses the `parquet` crate; the schema is fixed and versioned (`audit_v1.parquet`).
   - The export command: `sovereign audit export --since 90d --format parquet --out /var/lib/sovereign/audit-export-2026-06-04.parquet`. The export is incremental (only rows since the last successful export) and idempotent (re-running the same export produces the same byte-for-byte file).
4. **Implement `sovereign audit verify --export <file>`** — verifies the chain of an exported file (the public key is embedded in the export header so the auditor doesn't need access to the box).
5. **Implement the 90-day hot + 1-year cold retention** (from G3):
   - The SQLite audit table holds 90 days; an automatic daily job (`sovereign audit archive --retention 90d`) moves older rows to `/var/lib/sovereign/audit-archive/` as Parquet files (one per day).
   - The Parquet archive directory is rotated: 1 year of daily files. Files older than 1 year are deleted (configurable).
   - The archive path is also signed (the file's name is the SHA-256 of the content; the chain continues across the archive boundary).
6. **Add the 2 new standard-level doctor checks**:
   - `audit_chain_valid` — runs `sovereign audit verify` against the last 1,000 rows. `Fail` if any signature is invalid.
   - `audit_export_recent` — `Warn` if no export to an external destination has happened in the last 30 days. The check looks at the last `audit_export` audit row.
7. **Implement `sovereign audit tail`** — the live tail of the audit log (`tail -f` style, JSONL output). `--json` for piping.
8. **Implement the SOC 2 / ISO 27001 control mapping** in the export header:
   - The Parquet export has a `control_mapping.json` sidecar that maps each `kind` to the relevant control. E.g., `kind=deploy_completed` → `SOC2 CC8.1 Change Management`, `ISO 27001 A.12.1.2 Change Management`. The auditor reads this file, not the binary.
   - The mapping is generated from `docs/enterprise-readiness.md` (the new doc this turn adds) and is versioned with the binary.

### Code stub

```rust
// crates/sovereign-audit/src/lib.rs
use ed25519_dalek::{Keypair, Signer, Signature, Verifier};
use sha2::{Sha256, Digest};

pub struct AuditChain {
    keypair: Keypair,
    last_hash: [u8; 32],
}

impl AuditChain {
    pub fn append(&mut self, event: AuditEvent) -> Result<AuditRow, AuditError> {
        let canonical = serde_json::to_vec(&event.canonical())?;
        let mut hasher = Sha256::new();
        hasher.update(self.last_hash);
        hasher.update(&canonical);
        let hash: [u8; 32] = hasher.finalize().into();
        let signature = self.keypair.sign(&hash);
        let row = AuditRow {
            ts: event.ts,
            kind: event.kind,
            actor: event.actor,
            target: event.target,
            payload: event.payload,
            prev_hash: hex::encode(self.last_hash),
            signature: hex::encode(signature.to_bytes()),
        };
        self.last_hash = hash;
        // INSERT only — the VFS rejects UPDATE/DELETE
        self.writer.insert(&row)?;
        Ok(row)
    }

    pub fn verify(&self, rows: &[AuditRow]) -> Result<VerificationReport, AuditError> {
        let mut prev = [0u8; 32];
        for (i, row) in rows.iter().enumerate() {
            let canonical = serde_json::to_vec(&row.canonical())?;
            let mut hasher = Sha256::new();
            hasher.update(prev);
            hasher.update(&canonical);
            let expected_hash: [u8; 32] = hasher.finalize().into();
            let sig_bytes: [u8; 64] = hex::decode(&row.signature)?.try_into().unwrap();
            let sig = Signature::from_bytes(&sig_bytes);
            self.keypair.public().verify(&expected_hash, &sig)
                .map_err(|_| AuditError::InvalidSignatureAtRow(i))?;
            prev = expected_hash;
        }
        Ok(VerificationReport { rows_verified: rows.len() })
    }
}

// Append-only VFS
pub struct AppendOnlyVfs;
impl AppendOnlyVfs {
    pub fn install() {
        // sqlite3_vfs::register("appendonly", AppendOnlyVfs { ... });
        // The xAccess / xOpen methods reject any non-INSERT statement.
    }
}
```

### Tests

- Unit: 1,000 events appended; chain verify passes; tamper one row's payload → verify fails at the tampered row.
- Unit: the append-only VFS rejects `UPDATE audit SET ...`, `DELETE FROM audit`, `DROP TABLE audit`. The audit writer returns `Error::AppendOnlyViolation`.
- Integration: `sovereign audit export --since 90d --format parquet --out /tmp/audit.parquet` produces a valid Parquet file (read back with `parquet-tools` or `duckdb`). The row count matches the live count. Re-running the export produces a byte-identical file.
- Integration: `sovereign audit verify --export /tmp/audit.parquet` passes against the embedded public key.
- Integration: the 90-day archive cron moves rows correctly; the chain continues across the archive boundary.
- Doctor: `audit_chain_valid` passes on a fresh fixture; `audit_chain_valid` fails when one signature is corrupted (and the doctor output points to the bad row number).

### Acceptance criteria

- [ ] The `audit` table rejects `UPDATE`, `DELETE`, and `DROP` at the VFS layer.
- [ ] Each audit row has a `prev_hash` and a `signature` column; the chain is verifiable.
- [ ] `sovereign audit export` produces CSV / JSONL / Parquet, incrementally, idempotently.
- [ ] `sovereign audit verify` passes against the live chain and against an exported file.
- [ ] 90-day hot + 1-year cold retention is enforced.
- [ ] 2 new doctor checks (`audit_chain_valid`, `audit_export_recent`) are present in `--level standard`.

### Definition of done

A 4-week soak: 3 design partners run `sovereign audit export --since 90d --format parquet` weekly. All 3 produce Parquet files that pass `sovereign audit verify` against the embedded public key. At least 1 of the 3 exports to a SOC 2 auditor's S3 bucket and the auditor accepts the file without further transformation. The `audit_chain_valid` doctor check passes on all 3.

---

## 1. Definition of Done (the phase gate)

Phase 1 closes when **all** of these are true:

- [ ] All 23 features (G1-G23) are merged to `main` with passing CI.
- [ ] `cargo test --workspace` passes.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes.
- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo deny check` passes.
- [ ] `cargo audit` reports no new advisories.
- [ ] `cargo llvm-cov --workspace` reports ≥ 80% line coverage.
- [ ] `cargo doc --no-deps` builds with no warnings.
- [ ] The 5-command quickstart works on a fresh Hetzner CX22 in < 5 min.
- [ ] The 6 framework scanners work end-to-end (FastAPI, Next.js, Laravel, Go, Rails, Astro).
- [ ] The auto-rollback test (from Phase 0) still passes.
- [ ] The backup-verify test passes.
- [ ] The TUI test (panicsafe + 6 views) passes.
- [ ] `sovereign doctor --level standard` returns 0 fails on a clean fixture and ≥ 5 fails on the broken fixture, with the expected messages.
- [ ] `sovereign doctor --fix --non-interactive` repairs ≥ 70% of standard-level fails.
- [ ] `sovereign doctor kb` lists ≥ 40 articles; every check has a KB page.
- [ ] `sovereign bench` produces a JSON+MD artifact, comparable to the published baseline.
- [ ] `sovereign cost` reports the MTD total within ± 20% of the actual cloud bill for ≥ 1 design partner.
- [ ] The Show HN post is published.
- [ ] ≥ 50 installs/week and ≥ 10 active production users within 4 weeks of the Show HN post.
- [ ] The 10-point sovereignty test runs in CI (V0: as a stub; V1: as a real test).
- [ ] The Apache 2.0 LICENSE is unchanged.
- [ ] The version is bumped to `1.0.0`.

**If any of these is false, the phase is not done.**

---

## 2. What we explicitly do not build in Phase 1

Deferred to later phases (see [`negative-prompt.md`](./negative-prompt.md) §3 for rationale):

- Multi-server / agent pattern (V1.5)
- Service catalog export to Backstage (V1.5)
- Drift detection / GitOps auto-reconcile (V1.5)
- OPA/Rego policy (V2)
- ML scorer (V2)
- rqlite (V2)
- Web UI (V2)
- OIDC, MFA, SSO (G21, V1)
- Pro tier billing (V2)
- EU incorporation (V1.5)
- BSI C5 mapping (V2)
- Nginx low-mem (V1.1, not V1.0)
- Podman runtime (V1.5)
- Doctor `--level full|paranoid` (V1.5, V2)
- Doctor fleet/remote mode (V1.5)
- Doctor incident toolkit (`sovereign incident declare/note/resolve`, `sovereign retro`, `sovereign game-day`) (V1.5)
- Doctor compliance scan, canary, auto-tune (V2)

---

**Next: read [`phase-02-v15.md`](./phase-02-v15.md) for Months 5-9, which adds the multi-server agent pattern, RBAC, audit log query, and the doctor fleet/remote mode + incident toolkit.**
