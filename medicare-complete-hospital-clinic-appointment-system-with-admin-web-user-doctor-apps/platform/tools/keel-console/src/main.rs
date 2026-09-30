//! keel — the platform CLI (11-SPEC §34, item 29). Implemented: doctor,
//! about, check, graph/modules/route:list/event:list/capabilities,
//! make:module,
//! make:migration, migrate/migrate:status/migrate:fresh, seed, key:generate,
//! config:dump, cache:clear, up/down, job:list, queue:work/failed/retry,
//! schedule:list/run, db:inspect, shell (v1.1 B-29 interactive console).
//! S4 adds the developer platform: inspect
//! (execution/delta/authorization/n-plus-one), replay --dry-run, benchmark
//! (baseline + budget gate), upgrade report, security checks, interactive
//! prompts for scaffolds/destructive ops. Remaining commands print an
//! explicit gate-pending marker until their milestone lands.

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use keel_config::{Environment, PlatformConfig};
use keel_execution::{ExecutionTrace, FileTraceStore, TraceStore};
use keel_postgres::PgPool;

mod observe;

#[derive(Parser)]
#[command(name = "keel", version, about = "Keel platform CLI", long_about = None)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Environment doctor: toolchain, config, database assets, compose file.
    Doctor,
    /// Architecture gate G07: dependency matrix + budget analyzer.
    Check,
    /// Laravel-style environment report (docs/19 L222): framework/app
    /// version, env/debug, database/cache/queue/session drivers, flag
    /// snapshot, graph counts, dependency budget.
    About,
    /// B-27 profiling wrapper: build the target, then record it with samply
    /// (spawned out-of-process when installed; the manual flamegraph
    /// workflow is documented in docs/13-OPERATIONS.md).
    Profile {
        #[command(subcommand)]
        target: ProfileTarget,
    },
    /// Scaffold a new domain module (applications/<name>); prompts when the name is omitted.
    #[command(name = "make:module")]
    MakeModule {
        /// Module name, [a-z][a-z0-9-]* (prompts interactively when omitted).
        name: Option<String>,
        #[arg(long, default_value = "tenant")]
        plane: String,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Create a new timestamped SQL migration file.
    #[command(name = "make:migration")]
    MakeMigration {
        /// Migration name, [a-z0-9_]+.
        name: String,
    },
    /// Generate a scaffold file inside a module (item 30).
    Make {
        /// Kind: migration|seeder|factory|job|event|listener|command|query|policy|workflow|model|test|resource|request|controller.
        kind: String,
        /// Thing being generated (snake_case).
        name: String,
        /// Target module under applications/ (required except for migration).
        #[arg(long)]
        module: Option<String>,
    },
    /// Compile the platform graph from all module.toml manifests.
    Graph,
    /// List registered modules.
    Modules,
    /// List all declared routes (duplicate check is in the graph gate).
    #[command(name = "route:list")]
    RouteList {
        /// Output format: table or json.
        #[arg(long, default_value = "table")]
        format: String,
        /// Keep routes whose name contains this text (case-insensitive).
        #[arg(long)]
        name: Option<String>,
        /// Keep routes whose path contains this text (case-insensitive).
        #[arg(long)]
        path: Option<String>,
        /// Keep routes on this plane (api or web).
        #[arg(long)]
        plane: Option<String>,
    },
    /// Wave 1: resolve every route's middleware chain + bindings + validation
    /// and write storage/framework/routes.json (pinned to a manifest hash).
    /// Boot refuses to serve when the artifact goes stale.
    #[command(name = "route:cache")]
    RouteCache,
    /// Wave 1: delete storage/framework/routes.json (boot falls back to
    /// live route resolution).
    #[command(name = "route:clear")]
    RouteClear,
    /// Wave 2: precompile every template under `[views].paths` into the
    /// compiled cache (`[views].cache_dir`, default storage/framework/views).
    #[command(name = "view:cache")]
    ViewCache,
    /// Wave 2: drop every compiled view cache entry (boot recompiles on
    /// demand; a corrupt or stale entry never survives a clear).
    #[command(name = "view:clear")]
    ViewClear,
    /// Wave 5: warm every boot-pinning artifact in order (route:cache ->
    /// config:cache -> view:cache), aborting on the first failure so a
    /// broken step never leaves a half-warm deploy.
    #[command(name = "optimize")]
    Optimize,
    /// Wave 5: drop all three boot-pinning artifacts (route:cache,
    /// config:cache, view:cache); boot falls back to live compilation.
    #[command(name = "optimize:clear")]
    OptimizeClear,
    /// List declared events.
    #[command(name = "event:list")]
    EventList,
    /// List the capability catalog.
    Capabilities,
    /// Generate the MCP tool manifest from the platform graph (item 34, L49).
    #[command(name = "mcp:tools")]
    McpTools {
        /// Output format: table or json.
        #[arg(long, default_value = "table")]
        format: String,
    },
    /// Emit OpenAPI 3.1 from the platform graph for SDK generation (item L71).
    #[command(name = "sdk:openapi")]
    SdkOpenapi {
        /// Write the document to a file instead of stdout.
        #[arg(long)]
        output: Option<String>,
    },
    /// New application project from templates/project-template (compiles immediately).
    New {
        /// Project name, [a-z][a-z0-9-]* (prompts interactively when omitted).
        name: Option<String>,
        #[arg(long, default_value = "tenant")]
        plane: String,
        #[arg(long, default_value = "")]
        description: String,
        /// Use the SPA-first starter template: auth-wired module + web shell (S9).
        #[arg(long)]
        with_starter: bool,
    },
    Dev,
    /// Run the local app loop: keel-api (+ optional worker) in one command (S7 B-08).
    Serve {
        /// Also start keel-worker alongside keel-api.
        #[arg(long)]
        worker: bool,
    },
    /// Interactive platform console (v1.1 B-29): a Tinker-class REPL over the
    /// booted engine that dispatches under the principal you log in as.
    Shell,
    /// Prune aged/trashed rows for every registered prunable model (Wave 3
    /// Phase 6): boots keel-api in `--prune` maintenance mode and prints
    /// per-model purge counts (no server, no REPL).
    #[command(name = "model:prune")]
    ModelPrune {
        /// Rows purged per store pass (clamped to the 200-row page cap).
        #[arg(long, default_value_t = 200)]
        chunk: u32,
        /// Tenant whose rows to prune (tenant-scoped models match no rows
        /// without it).
        #[arg(long)]
        tenant: Option<String>,
    },
    /// Build runtime binaries and write the release manifest (RELEASE.md).
    Build {
        /// dev or release.
        #[arg(long, default_value = "release")]
        profile: String,
    },
    Test,
    /// Apply pending database migrations.
    Migrate,
    /// Show migration status (applied vs pending).
    #[command(name = "migrate:status")]
    MigrateStatus,
    /// Drop and recreate the schema, then migrate (dev/test only).
    #[command(name = "migrate:fresh")]
    MigrateFresh,
    /// Wave 5: revert the last migration batch (default 1) in reverse
    /// filename order — pair migrations only; forward-only single-file
    /// migrations (database/migrations) never roll back.
    #[command(name = "migrate:rollback")]
    MigrateRollback {
        /// Number of batches to revert (default 1).
        #[arg(long, default_value_t = 1)]
        step: u32,
    },
    /// Wave 5: revert every applied pair migration in reverse order
    /// (dev/test only — the full-history revert is destructive).
    #[command(name = "migrate:reset")]
    MigrateReset,
    /// Wave 5: revert `--step` batches (default 1) and re-apply pending
    /// migrations (dev/test only — the rollback window is destructive).
    #[command(name = "migrate:refresh")]
    MigrateRefresh {
        /// Number of batches to revert before re-applying (default 1).
        #[arg(long, default_value_t = 1)]
        step: u32,
    },
    /// Wave 5: create the pair bookkeeping table (keel_migrations) without
    /// running any migration files.
    #[command(name = "migrate:install")]
    MigrateInstall,
    /// Run seeders (idempotent; comma-separated names with --only).
    Seed {
        #[arg(long)]
        only: Option<String>,
    },
    /// Generate a new application key and store it in the config .env.
    #[command(name = "key:generate")]
    KeyGenerate,
    /// Dump the effective configuration (secrets redacted).
    #[command(name = "config:dump")]
    ConfigDump,
    /// Wave 4 Phase 5: pin the frozen effective config to
    /// storage/framework/config.json + content hash (boot hash-checks and
    /// refuses drift, identical semantics to route:cache).
    #[command(name = "config:cache")]
    ConfigCache,
    /// Wave 4 Phase 5: delete storage/framework/config.json (boot reads the
    /// live config without the drift pin).
    #[command(name = "config:clear")]
    ConfigClear,
    /// Wave 4 Phase 5: print one config section (secrets redacted).
    #[command(name = "config:show")]
    ConfigShow {
        /// Section: app, database, auth, otp, storage, limits, telemetry,
        /// views, http.
        key: String,
        /// Print JSON instead of TOML.
        #[arg(long)]
        json: bool,
    },
    /// Wave 4 Phase 5: print the environment summary (config file layering,
    /// redacted key facts, config-cache pin state).
    #[command(name = "env")]
    Env,
    /// Clear the application cache.
    #[command(name = "cache:clear")]
    CacheClear,
    /// Wave 4 Phase 5: remove exactly one cache key (routes | config |
    /// views:<template>).
    #[command(name = "cache:forget")]
    CacheForget {
        /// Cache key: routes, config, or views:<template>.
        key: String,
    },
    /// Bring the application up (remove the maintenance file).
    Up,
    /// Maintenance mode: serve 503 until "keel up".
    Down {
        /// Retry-After seconds announced to clients.
        #[arg(long)]
        retry: Option<u64>,
        /// Bypass secret (header or query).
        #[arg(long)]
        secret: Option<String>,
    },
    /// List jobs (optionally filtered by status).
    #[command(name = "job:list")]
    JobList {
        /// pending | running | succeeded | failed | dead.
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Process jobs via the keel-worker binary.
    #[command(name = "queue:work")]
    QueueWork {
        #[arg(long)]
        once: bool,
    },
    /// List dead jobs awaiting manual retry.
    #[command(name = "queue:failed")]
    QueueFailed,
    /// Requeue a dead/failed job.
    #[command(name = "queue:retry")]
    QueueRetry {
        /// Job id (uuid).
        id: String,
    },
    /// Wave 5: purge every pending job (dev/test only — queued work is lost).
    #[command(name = "queue:clear")]
    QueueClear,
    /// Wave 5: delete every dead job (their forensic rows are gone after).
    #[command(name = "queue:flush")]
    QueueFlush,
    /// Wave 5: delete one dead job.
    #[command(name = "queue:forget")]
    QueueForget {
        /// Dead job id (uuid).
        id: String,
    },
    /// Wave 5: queue depth + oldest pending age snapshot (alerting).
    #[command(name = "queue:monitor")]
    QueueMonitor,
    /// Wave 5: prune dead jobs that ended before --hours (default 168 = 7 days).
    #[command(name = "queue:prune-failed")]
    QueuePruneFailed {
        /// Age cutoff in hours (jobs ended before now - hours are deleted).
        #[arg(long, default_value_t = 168)]
        hours: u64,
    },
    /// Wave 5: prune terminal batch records older than --hours (default 72).
    /// The batch ledger is not persisted yet — reports an honest empty run.
    #[command(name = "queue:prune-batches")]
    QueuePruneBatches {
        /// Age cutoff in hours (default 72).
        #[arg(long, default_value_t = 72)]
        hours: u64,
    },
    /// Wave 5: write the restart signal file running workers honor
    /// (cooperative stop between jobs; supervisors then respawn them).
    #[command(name = "queue:restart")]
    QueueRestart,
    /// Wave 5: supervise a keel-worker child and auto-restart it on exit
    /// (with crash-loop backoff); Ctrl-C stops the supervisor.
    #[command(name = "queue:listen")]
    QueueListen,
    /// Wave 5: requeue every failed member of a batch. The batch ledger is
    /// not persisted yet — fails closed with the per-job alternative.
    #[command(name = "queue:retry-batch")]
    QueueRetryBatch {
        /// Batch id (uuid).
        id: String,
    },
    /// List registered schedules.
    #[command(name = "schedule:list")]
    ScheduleList,
    /// Run due schedules once.
    #[command(name = "schedule:run")]
    ScheduleRun,
    /// Database inventory: tables + row counts.
    #[command(name = "db:inspect")]
    DbInspect,
    /// Wave 5: server metadata (driver, server version, database) plus the
    /// db:inspect table inventory with row counts.
    #[command(name = "db:show")]
    DbShow,
    /// Wave 5: write a normalized DDL snapshot of the migrated schema to
    /// database/schema.sql (review artifact — never executed by migrate).
    #[command(name = "db:schema:dump")]
    DbSchemaDump,
    /// Inspect recorded executions: what = execution|delta|authorization|n-plus-one.
    Inspect {
        /// execution | delta | authorization | n-plus-one.
        what: String,
        /// Execution id (uuid); omit for the recent list / n-plus-one scan.
        id: Option<String>,
        /// How many recent traces to list/scan.
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Repeat-count threshold for N+1 detection.
        #[arg(long, default_value_t = 3)]
        threshold: usize,
    },
    /// Replay a recorded execution (Original vs Replay comparison).
    Replay {
        /// Execution id (uuid) from `keel inspect execution`.
        id: String,
        /// Show what would be replayed without executing anything.
        #[arg(long)]
        dry_run: bool,
    },
    /// Framework benchmark vs baseline (G15); rejects regressions beyond budget.
    Benchmark {
        /// Persist the current numbers as the new baseline.
        #[arg(long)]
        save_baseline: bool,
        /// Allowed regression per phase, in percent.
        #[arg(long, default_value_t = 10.0)]
        budget: f64,
        /// Timed iterations per phase.
        #[arg(long, default_value_t = 300)]
        iterations: usize,
    },
    /// Upgrade report: versions, public APIs, deprecations, config, migrations, dependencies.
    Upgrade,
    /// Security checks: secrets hygiene, dependency policy, trace locality (L74).
    Security,
    /// Show the current release manifest (written by `keel build`).
    Release,
    /// List registered platform flags and their rollout state.
    #[command(name = "flag:list")]
    FlagList,
    /// Set a flag stage: disabled|internal|1%|10%|50%|100% (optionally per tenant).
    #[command(name = "flag:set")]
    FlagSet {
        name: String,
        stage: String,
        /// Restrict the stage to one tenant id (uuid).
        #[arg(long)]
        tenant: Option<String>,
    },
    /// Sentinel telemetry: status, logs, errors, traces, incidents, audit chain, cost (S5).
    Observe {
        #[command(subcommand)]
        what: observe::ObserveCmd,
    },
}

/// B-27 `keel profile` targets: the two long-running runtimes. The package
/// and the built binary share a name for both.
#[derive(Subcommand)]
enum ProfileTarget {
    /// Record keel-api (the HTTP edge) under samply.
    Api,
    /// Record keel-worker (queue consumer) under samply.
    Worker,
}

impl ProfileTarget {
    /// Workspace package (and built binary name under target/debug).
    fn name(&self) -> &'static str {
        match self {
            ProfileTarget::Api => "keel-api",
            ProfileTarget::Worker => "keel-worker",
        }
    }
}

fn workspace_root() -> anyhow::Result<PathBuf> {
    let mut dir = std::env::current_dir()?;
    loop {
        let cand = dir.join("Cargo.toml");
        if cand.exists() {
            let raw = std::fs::read_to_string(&cand)?;
            if raw.contains("[workspace]") {
                return Ok(dir);
            }
        }
        if !dir.pop() {
            anyhow::bail!(
                "no workspace Cargo.toml found above {}",
                std::env::current_dir()?.display()
            );
        }
    }
}

fn config_path(root: &Path) -> PathBuf {
    match std::env::var("KEEL_CONFIG") {
        Ok(path) => PathBuf::from(path),
        Err(_) => root.join("config/dev.toml"),
    }
}

fn load_config(root: &Path) -> anyhow::Result<PlatformConfig> {
    let path = config_path(root);
    PlatformConfig::load(Some(&path)).map_err(|e| anyhow::anyhow!("{e}"))
}

/// Run a future on a multi-threaded runtime. Multi-threaded is required (not
/// just preferred): the durable store ports bridge sync calls via
/// `block_in_place`, which panics on single-threaded runtimes, and pooled
/// connections must stay on the runtime that created them.
fn rt<F: std::future::Future<Output = anyhow::Result<ExitCode>>>(
    future: F,
) -> anyhow::Result<ExitCode> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime builds")
        .block_on(future)
}

async fn db_pool(root: &Path) -> anyhow::Result<PgPool> {
    let cfg = load_config(root)?;
    let db = cfg.database.as_ref().ok_or_else(|| {
        anyhow::anyhow!("no database configured in {}", config_path(root).display())
    })?;
    keel_postgres::create_pool(&db.url, db.max_connections)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.cmd {
        Cmd::Doctor => doctor(),
        Cmd::Check => check(),
        Cmd::About => rt(about()),
        Cmd::Profile { target } => profile(target),
        Cmd::MakeModule {
            name,
            plane,
            description,
        } => make_module_cmd(name, &plane, &description),
        Cmd::MakeMigration { name } => make_migration(&name),
        Cmd::Graph => graph_report(),
        Cmd::Modules => modules_list(),
        Cmd::RouteList {
            format,
            name,
            path,
            plane,
        } => route_list(
            &format,
            name.as_deref(),
            path.as_deref(),
            plane.as_deref(),
        ),
        Cmd::ViewCache => view_cache(),
        Cmd::ViewClear => view_clear(),
        Cmd::Optimize => optimize(),
        Cmd::OptimizeClear => optimize_clear(),
        Cmd::RouteCache => route_cache(),
        Cmd::RouteClear => route_clear(),
        Cmd::EventList => event_list(),
        Cmd::Capabilities => capabilities_list(),
        Cmd::McpTools { format } => mcp_tools(&format),
        Cmd::SdkOpenapi { output } => sdk_openapi(output),
        Cmd::Migrate => rt(migrate()),
        Cmd::MigrateStatus => rt(migrate_status()),
        Cmd::MigrateFresh => rt(migrate_fresh()),
        Cmd::MigrateRollback { step } => rt(migrate_rollback(step)),
        Cmd::MigrateReset => rt(migrate_reset()),
        Cmd::MigrateRefresh { step } => rt(migrate_refresh(step)),
        Cmd::MigrateInstall => rt(migrate_install()),
        Cmd::Seed { only } => rt(seed(only)),
        Cmd::KeyGenerate => key_generate(),
        Cmd::ConfigDump => config_dump(),
        Cmd::CacheClear => cache_clear(),
        Cmd::ConfigCache => config_cache(),
        Cmd::ConfigClear => config_clear(),
        Cmd::ConfigShow { key, json } => config_show(&key, json),
        Cmd::Env => env_summary(),
        Cmd::CacheForget { key } => cache_forget(&key),
        Cmd::Up => up(),
        Cmd::Down { retry, secret } => down(retry, secret),
        Cmd::JobList { status, limit } => rt(job_list(status, limit)),
        Cmd::QueueWork { once } => queue_work(once),
        Cmd::QueueFailed => rt(queue_failed()),
        Cmd::QueueRetry { id } => rt(queue_retry(&id)),
        Cmd::QueueClear => rt(queue_clear()),
        Cmd::QueueFlush => rt(queue_flush()),
        Cmd::QueueForget { id } => rt(queue_forget(&id)),
        Cmd::QueueMonitor => rt(queue_monitor()),
        Cmd::QueuePruneFailed { hours } => rt(queue_prune_failed(hours)),
        Cmd::QueuePruneBatches { hours } => rt(queue_prune_batches(hours)),
        Cmd::QueueRestart => queue_restart(),
        Cmd::QueueListen => queue_listen(),
        Cmd::QueueRetryBatch { id } => rt(queue_retry_batch(&id)),
        Cmd::ScheduleList => schedule_info("schedule:list"),
        Cmd::ScheduleRun => schedule_info("schedule:run"),
        Cmd::DbInspect => rt(db_inspect()),
        Cmd::DbShow => rt(db_show()),
        Cmd::DbSchemaDump => rt(db_schema_dump()),
        Cmd::New {
            name,
            plane,
            description,
            with_starter,
        } => new_project_cmd(name, &plane, &description, with_starter),
        Cmd::Make { kind, name, module } => make_kind(&kind, &name, module),
        Cmd::Build { profile } => build(&profile),
        Cmd::Serve { worker } => serve(worker),
        Cmd::Shell => shell_console(),
        Cmd::ModelPrune { chunk, tenant } => model_prune(chunk, tenant),
        Cmd::Release => release_manifest(),
        Cmd::FlagList => flag_list(),
        Cmd::FlagSet {
            name,
            stage,
            tenant,
        } => flag_set(&name, &stage, tenant),
        Cmd::Inspect {
            what,
            id,
            limit,
            threshold,
        } => inspect(&what, id.as_deref(), limit, threshold),
        Cmd::Replay { id, dry_run } => replay(&id, dry_run),
        Cmd::Benchmark {
            save_baseline,
            budget,
            iterations,
        } => benchmark(save_baseline, budget, iterations),
        Cmd::Upgrade => upgrade_report(),
        Cmd::Security => security_check(),
        Cmd::Observe { what } => observe::dispatch(what),
        other => gate_pending(&other),
    };
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

// ------------------------------------------------------------- infra checks

fn tool_version(bin: &str, arg: &str) -> String {
    std::process::Command::new(bin)
        .arg(arg)
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "NOT FOUND".into())
}

fn line(ok: bool, label: &str, detail: &str) -> bool {
    println!("  {} {label}: {detail}", if ok { "[ok]" } else { "[!!]" });
    ok
}

fn doctor() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    println!("keel doctor — workspace {}", root.display());
    let mut all_ok = true;

    let rustc = tool_version("rustc", "--version");
    all_ok &= line(rustc != "NOT FOUND", "rustc", &rustc);
    let cargo = tool_version("cargo", "--version");
    all_ok &= line(cargo != "NOT FOUND", "cargo", &cargo);

    let toolchain = root.join("rust-toolchain.toml");
    all_ok &= line(
        toolchain.exists(),
        "rust-toolchain.toml",
        &toolchain.display().to_string(),
    );

    let cfg = root.join("config/dev.toml");
    let cfg_ok = cfg.exists()
        && std::fs::read_to_string(&cfg)
            .ok()
            .and_then(|s| s.parse::<toml::Value>().ok())
            .is_some();
    all_ok &= line(cfg_ok, "config/dev.toml", "present + parses");

    let compose = root.join("docker-compose.yml");
    all_ok &= line(
        compose.exists(),
        "docker-compose.yml",
        "postgres + minio for local dev",
    );

    let migrations = root.join("database/migrations");
    let count = std::fs::read_dir(&migrations)
        .map(|d| d.count())
        .unwrap_or(0);
    all_ok &= line(
        count > 0,
        "migrations",
        &format!("{count} file(s) in database/migrations"),
    );

    let roles = root.join("database/roles.sql");
    all_ok &= line(
        roles.exists(),
        "roles.sql",
        "role separation (owner != runtime)",
    );

    let members = std::fs::read_to_string(root.join("Cargo.toml"))
        .ok()
        .and_then(|s| keel_analyzer::parse_members(&s).ok())
        .unwrap_or_default();
    all_ok &= line(
        !members.is_empty(),
        "workspace members",
        &format!("{}", members.len()),
    );

    if !all_ok {
        return Ok(ExitCode::FAILURE);
    }
    println!("doctor complete.");
    Ok(ExitCode::SUCCESS)
}

/// Wave 4 Phase 8 (docs/19 L222): `keel about` — the Laravel-style report.
/// Config + flag registry/store + compiled graph + the workspace dependency
/// table, all read-only. The database server version is a best-effort
/// `SELECT version()`: an absent or unreachable database degrades to a note,
/// never a failure.
async fn about() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let db_version = db_server_version(&root).await;
    print!("{}", about_report(&root, db_version.as_deref())?);
    Ok(ExitCode::SUCCESS)
}

/// Build the `keel about` report for an explicit workspace root.
/// `db_version` is the best-effort server-version ping (None = no database
/// configured or unreachable); the golden-fields test drives this directly.
fn about_report(root: &Path, db_version: Option<&str>) -> anyhow::Result<String> {
    let cfg = load_config(root)?;
    let mut out = String::from("keel about\n\n");
    out.push_str(&format!(
        "  Framework           {}\n",
        env!("CARGO_PKG_VERSION")
    ));
    out.push_str(&format!("  Application         {}\n", cfg.app.name));
    out.push_str(&format!(
        "  Application version {}\n",
        workspace_version(root)
    ));
    out.push_str(&format!("  Environment         {}\n", cfg.app.env.as_str()));
    out.push_str(&format!("  Debug               {}\n", cfg.app.debug));

    match cfg.database.as_ref().filter(|db| !db.url.is_empty()) {
        None => out.push_str("  Database            none (no database configured)\n"),
        Some(db) => {
            let driver = db_url_scheme(&db.url);
            match db_version {
                Some(version) => {
                    out.push_str(&format!("  Database            {driver} - {version}\n"))
                }
                None => out.push_str(&format!(
                    "  Database            {driver} - server version unreachable\n"
                )),
            }
        }
    }

    // Driver selection mirrors the runtime: REDIS_URL present + reachable
    // picks redis for the cache port (keel-api run) and the job store
    // (keel-worker, which falls back to the database queue, then memory).
    let has_db = cfg
        .database
        .as_ref()
        .is_some_and(|db| !db.url.is_empty());
    let redis = std::env::var("REDIS_URL").is_ok();
    out.push_str(&format!(
        "  Cache driver        {}\n",
        if redis { "redis" } else { "memory" }
    ));
    out.push_str(&format!(
        "  Queue driver        {}\n",
        if redis {
            "redis"
        } else if has_db {
            "database"
        } else {
            "memory"
        }
    ));

    // Flag snapshot through the same registry/store/evaluation path boot
    // uses; sessions are on exactly when the `app.sessions` flag evaluates.
    let (registered, enabled) = flag_snapshot(root, cfg.app.env.as_str())?;
    out.push_str(&format!(
        "  Session driver      {}\n",
        if enabled.contains("app.sessions") {
            "cookie"
        } else {
            "disabled"
        }
    ));
    out.push_str(&format!(
        "  Flags               {registered} registered, {} enabled\n",
        enabled.len()
    ));

    // Compiled graph counts; a broken graph still reports — `keel check`
    // carries the violations.
    let compiled = discover_manifests(root).and_then(|manifests| {
        keel_registry::PlatformGraph::compile(manifests)
            .map_err(|v| anyhow::anyhow!("{} violation(s)", v.len()))
    });
    match compiled {
        Ok(graph) => {
            let routes: usize = graph.modules.iter().map(|m| m.routes.len()).sum();
            out.push_str(&format!(
                "  Graph               {} modules, {routes} routes, {} operations\n",
                graph.modules.len(),
                graph.operations.len()
            ));
        }
        Err(e) => out.push_str(&format!("  Graph               unavailable ({e})\n")),
    }

    match external_deps(root) {
        Some((external, _, _)) => {
            let state = if external > 25 {
                "OVER BUDGET"
            } else {
                "within budget"
            };
            out.push_str(&format!(
                "  Dependencies        {external}/25 direct external ({state})\n"
            ));
        }
        None => out.push_str("  Dependencies        (workspace Cargo.toml unreadable)\n"),
    }
    Ok(out)
}

/// Best-effort database server version for `about`: configured and
/// reachable -> the `SELECT version()` string; anything else -> None (the
/// report degrades, it never fails on an absent or unreachable database).
async fn db_server_version(root: &Path) -> Option<String> {
    let cfg = load_config(root).ok()?;
    let db = cfg.database.as_ref()?;
    if db.url.is_empty() {
        return None;
    }
    let pool = db_pool(root).await.ok()?;
    let version: String = sqlx::query_scalar("SELECT version()")
        .fetch_one(&pool)
        .await
        .ok()?;
    Some(version)
}

/// `[workspace.package] version` from the root Cargo.toml (the application
/// version), falling back to the framework's own version.
fn workspace_version(root: &Path) -> String {
    std::fs::read_to_string(root.join("Cargo.toml"))
        .ok()
        .and_then(|raw| raw.parse::<toml::Value>().ok())
        .and_then(|v| {
            v.get("workspace")?
                .get("package")?
                .get("version")?
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

/// URL scheme of a database DSN (`postgres://...` -> `postgres`); an opaque
/// URL without a scheme reports `unknown`.
fn db_url_scheme(url: &str) -> &str {
    url.split_once("://")
        .map(|(scheme, _)| scheme)
        .unwrap_or("unknown")
}

/// Flag facts for `about`: (registered definitions, enabled set), resolved
/// through the same registry/store/evaluation path boot uses. The actor is
/// the fixed nil principal so percentage stages never reshuffle the report.
fn flag_snapshot(
    root: &Path,
    environment: &str,
) -> anyhow::Result<(usize, std::collections::BTreeSet<String>)> {
    let registry = Arc::new(keel_feature_flags::platform_registry());
    let registered = registry.definitions().len();
    let store: Arc<dyn keel_feature_flags::FlagStore> =
        Arc::new(keel_feature_flags::FileFlagStore::new(
            keel_feature_flags::default_flags_path(root),
        ));
    let capabilities = std::collections::BTreeSet::new();
    let input = keel_feature_flags::FlagInput {
        environment,
        tenant: None,
        region: None,
        capabilities: &capabilities,
        actor: keel_foundation::PrincipalId::from_uuid(uuid::Uuid::nil()),
        internal_actor: true,
    };
    let service = keel_feature_flags::FlagService::new(registry, store);
    let snapshot = service
        .snapshot(&input)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok((registered, snapshot.flags))
}

fn check() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    println!("keel check — architecture gate G07");
    let raw = std::fs::read_to_string(root.join("Cargo.toml"))?;
    let members = keel_analyzer::parse_members(&raw).map_err(|e| anyhow::anyhow!("{e}"))?;
    let violations = keel_analyzer::analyze_workspace(&root);
    for v in &violations {
        println!("  [!!] {} ({}): {}", v.code, v.member, v.message);
    }
    if violations.is_empty() {
        println!(
            "  [ok] dependency matrix + budgets clean ({} members)",
            members.len()
        );
    }

    // Wave 1 graph gates: the platform graph must compile and the middleware
    // catalog must resolve every route's chain (GRAPH-MW-UNKNOWN;
    // collect-all, fail-closed). The companion GRAPH-BINDING-RESOLVER gate
    // needs the live route.bindings registry (modules register resolvers
    // during boot), so it runs at keel-api boot, not here.
    let mut failures = violations.len();
    match discover_manifests(&root) {
        Ok(manifests) => match keel_registry::PlatformGraph::compile(manifests) {
            Ok(graph) => match keel_registry::MiddlewareCatalog::load(&root) {
                Ok(catalog) => {
                    let mw = keel_registry::middleware_violations(&graph, &catalog);
                    for v in &mw {
                        println!("  [!!] {}: {}", v.code, v.message);
                    }
                    if mw.is_empty() {
                        println!("  [ok] graph compiles; middleware catalog resolves every route");
                    } else {
                        failures += mw.len();
                    }
                }
                Err(e) => {
                    println!("  [!!] GRAPH-MW-UNKNOWN: middleware.toml rejected: {e}");
                    failures += 1;
                }
            },
            Err(graph_violations) => {
                for v in &graph_violations {
                    println!("  [!!] {}: {}", v.code, v.message);
                }
                failures += graph_violations.len();
            }
        },
        Err(e) => {
            println!("  [!!] GRAPH-MANIFEST: {e:#}");
            failures += 1;
        }
    }

    if failures == 0 {
        Ok(ExitCode::SUCCESS)
    } else {
        Ok(ExitCode::FAILURE)
    }
}

// ---------------------------------------------------------------- scaffolds

fn make_module(name: &str, plane: &str, description: &str) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let spec = keel_codegen::ModuleSpec {
        name: name.to_string(),
        plane: plane.to_string(),
        description: if description.is_empty() {
            format!("Module {name}")
        } else {
            description.to_string()
        },
    };
    let written = keel_codegen::write_module(&root, &spec).map_err(|e| anyhow::anyhow!(e))?;
    keel_codegen::add_workspace_member(&root, &spec.member_path())
        .map_err(|e| anyhow::anyhow!(e))?;
    for p in written {
        let rel: std::path::PathBuf = p
            .strip_prefix(&root)
            .map(|r| r.to_path_buf())
            .unwrap_or_else(|_| p.clone());
        println!("  + {}", rel.display());
    }
    println!("  + workspace member {}", spec.member_path());
    println!("module {name} scaffolded. Run: cargo check to compile it.");
    Ok(ExitCode::SUCCESS)
}

fn make_migration(name: &str) -> anyhow::Result<ExitCode> {
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        anyhow::bail!("migration name must match [a-z0-9_]+");
    }
    let root = workspace_root()?;
    let stamp = time::OffsetDateTime::now_utc()
        .format(&time::macros::format_description!(
            "[year][month][day][hour][minute][second]"
        ))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let file_name = format!("{stamp}_{name}.sql");
    let path = root.join("database/migrations").join(&file_name);
    if path.exists() {
        anyhow::bail!("{} already exists", path.display());
    }
    std::fs::write(&path, format!("-- {name}\n"))?;
    let rel = path.strip_prefix(&root).unwrap_or(&path);
    println!("  + {}", rel.display());
    println!("migration created. Migrations are embedded at compile time; run: keel migrate.");
    Ok(ExitCode::SUCCESS)
}

// -------------------------------------------------------------------- graph

fn discover_manifests(root: &Path) -> anyhow::Result<Vec<keel_registry::ModuleManifest>> {
    let mut out = Vec::new();
    for base in ["applications", "modules"] {
        let dir = root.join(base);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let manifest_path = entry.path().join("module.toml");
            if manifest_path.exists() {
                let raw = std::fs::read_to_string(&manifest_path)?;
                let manifest = keel_registry::ModuleManifest::from_toml(&raw)
                    .map_err(|v| anyhow::anyhow!("{}: {}", manifest_path.display(), v.message))?;
                out.push(manifest);
            }
        }
    }
    out.extend(discover_route_files(root)?);
    out.sort_by(|a, b| a.name().cmp(b.name()));
    Ok(out)
}

/// Wave 2 root route files (mirroring Laravel's routes/*.php): merge
/// `routes/web.toml` + `routes/api.toml` routes (plane forced by file),
/// `routes/console.toml` commands and `routes/channels.toml` channels
/// into synthetic module manifests. Absent files are fine; the graph gates
/// (GRAPH-ROUTEFILE-OP, GRAPH-CHANNEL-CAP) validate their references.
fn discover_route_files(root: &Path) -> anyhow::Result<Vec<keel_registry::ModuleManifest>> {
    let mut out = Vec::new();
    for (file, plane, module_name) in [
        ("web.toml", Some("web"), "routes_web"),
        ("api.toml", Some("api"), "routes_api"),
        ("console.toml", None, "routes_console"),
        ("channels.toml", None, "routes_channels"),
    ] {
        let path = root.join("routes").join(file);
        if !path.exists() {
            continue;
        }
        let raw = std::fs::read_to_string(&path)?;
        let manifest =
            keel_registry::RouteFileManifest::from_toml(&raw, &format!("routes/{file}"), plane)
                .map_err(|v| anyhow::anyhow!("{}: {}", path.display(), v.message))?;
        out.push(manifest.into_module(module_name));
    }
    Ok(out)
}

fn compile_graph(root: &Path) -> Result<keel_registry::PlatformGraph, ExitCode> {
    let manifests = discover_manifests(root).map_err(|e| {
        eprintln!("error: {e:#}");
        ExitCode::FAILURE
    })?;
    keel_registry::PlatformGraph::compile(manifests).map_err(|violations| {
        for v in &violations {
            eprintln!("  [!!] {}: {}", v.code, v.message);
        }
        ExitCode::FAILURE
    })
}

fn graph_report() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let graph = match compile_graph(&root) {
        Ok(g) => g,
        Err(code) => return Ok(code),
    };
    println!(
        "platform graph: {} module(s), {} operation(s), {} event(s)",
        graph.modules.len(),
        graph.operations.len(),
        graph.events.len()
    );
    for (op, module) in &graph.operations {
        println!("  op    {op} ({module})");
    }
    for (event, module) in &graph.events {
        println!("  event {event} ({module})");
    }
    if graph.modules.is_empty() {
        println!("  (no modules registered yet — foundation stage)");
    }
    Ok(ExitCode::SUCCESS)
}

fn modules_list() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let graph = match compile_graph(&root) {
        Ok(g) => g,
        Err(code) => return Ok(code),
    };
    for m in &graph.modules {
        println!(
            "  {:<20} v{} maturity={:?} plane={:?} capabilities={}",
            m.name(),
            m.module.version,
            m.module.maturity,
            m.module.plane,
            m.capabilities.len()
        );
    }
    if graph.modules.is_empty() {
        println!("  (none)");
    }
    Ok(ExitCode::SUCCESS)
}

/// One row of `route:list` (the table and json formats share it).
#[derive(Debug)]
struct RouteRow {
    method: String,
    path: String,
    module: String,
    operation: String,
    /// `api` (default) or `web` — the declared route plane.
    plane: String,
    /// Owning `[[route_groups]]` name (empty when the route is ungrouped).
    group: String,
    version: String,
    deprecated: bool,
    name: String,
}

/// Build the `route:list` rows for `graph` through the optional filters
/// (19-PARITY "route:list filters"). `--plane` is an EXACT match and is
/// validated up front so a typo never silently matches nothing; `--name`
/// and `--path` are case-insensitive substring filters. Group prefix and
/// group plane are already merged into the spec at parse time
/// (`registry::apply_groups`), so the rows show what the edge serves.
fn route_rows(
    graph: &keel_registry::PlatformGraph,
    name_filter: Option<&str>,
    path_filter: Option<&str>,
    plane_filter: Option<&str>,
) -> anyhow::Result<Vec<RouteRow>> {
    let plane_filter = match plane_filter {
        Some(raw) => {
            let plane = raw.to_ascii_lowercase();
            if plane != "api" && plane != "web" {
                anyhow::bail!("unknown plane {plane:?} (expected api or web)");
            }
            Some(plane)
        }
        None => None,
    };
    let contains = |filter: Option<&str>, value: &str| match filter {
        None => true,
        Some(needle) => value
            .to_lowercase()
            .contains(&needle.to_lowercase()),
    };
    let mut rows = Vec::new();
    for m in &graph.modules {
        for r in &m.routes {
            let row = RouteRow {
                method: r.method.clone(),
                path: r.path.clone(),
                module: m.name().to_string(),
                operation: r.operation.clone(),
                plane: r.plane.clone().unwrap_or_else(|| "api".to_string()),
                group: r.group.clone().unwrap_or_default(),
                version: r.version.clone(),
                deprecated: r.deprecated,
                name: r.name.clone().unwrap_or_default(),
            };
            if let Some(plane) = &plane_filter {
                if &row.plane != plane {
                    continue;
                }
            }
            if !contains(name_filter, &row.name) || !contains(path_filter, &row.path) {
                continue;
            }
            rows.push(row);
        }
    }
    Ok(rows)
}

fn route_list(
    format: &str,
    name: Option<&str>,
    path: Option<&str>,
    plane: Option<&str>,
) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    route_list_at(&root, format, name, path, plane)
}

fn route_list_at(
    root: &Path,
    format: &str,
    name: Option<&str>,
    path: Option<&str>,
    plane: Option<&str>,
) -> anyhow::Result<ExitCode> {
    let graph = match compile_graph(root) {
        Ok(g) => g,
        Err(code) => return Ok(code),
    };
    let rows = route_rows(&graph, name, path, plane)?;
    match format {
        "json" => {
            let json: Vec<serde_json::Value> = rows
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "method": r.method,
                        "path": r.path,
                        "module": r.module,
                        "operation": r.operation,
                        "plane": r.plane,
                        "group": r.group,
                        "version": r.version,
                        "deprecated": r.deprecated,
                        // Absent names stay `null` (the pre-Wave-2 shape).
                        "name": if r.name.is_empty() {
                            serde_json::Value::Null
                        } else {
                            serde_json::Value::String(r.name.clone())
                        },
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
        "table" => {
            if rows.is_empty() {
                println!("  (none)");
            } else {
                println!(
                    "  {:<6} {:<4} {:<34} -> {:<26} {:<7}  {:<12} {:<10} NAME",
                    "METHOD", "PLANE", "PATH", "MODULE.OPERATION", "VERSION",
                    "GROUP", "DEPRECATED"
                );
                for row in &rows {
                    let target = format!("{}.{}", row.module, row.operation);
                    println!(
                        "  {:<6} {:<4} {:<34} -> {:<26} {:<7}  {:<12} {:<10} {}",
                        row.method,
                        row.plane,
                        row.path,
                        target,
                        row.version,
                        row.group,
                        if row.deprecated { "deprecated" } else { "" },
                        row.name,
                    );
                }
            }
        }
        other => anyhow::bail!("unknown format {other:?} (expected table or json)"),
    }
    Ok(ExitCode::SUCCESS)
}

/// Wave 1 route cache: compile the graph, resolve every route's middleware
/// chain/bindings/validation against the catalog, and write the pinned
/// artifact (storage/framework/routes.json). Boot refuses a stale artifact
/// (hash mismatch vs manifests + middleware.toml), so this must be re-run
/// after any manifest change.
fn route_cache() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    route_cache_at(&root)
}

/// Testable core of route:cache against an explicit workspace root.
fn route_cache_at(root: &Path) -> anyhow::Result<ExitCode> {
    let graph = match compile_graph(root) {
        Ok(g) => g,
        Err(code) => return Ok(code),
    };
    let catalog = keel_registry::MiddlewareCatalog::load(root)
        .map_err(|e| anyhow::anyhow!("middleware catalog rejected: {e}"))?;
    let plans = keel_registry::resolve_route_plans(&graph, &catalog).map_err(|violations| {
        anyhow::anyhow!(
            "route resolution failed: {}",
            violations
                .iter()
                .map(|v| format!("{}: {}", v.code, v.message))
                .collect::<Vec<_>>()
                .join("; ")
        )
    })?;
    let artifact = keel_registry::RouteCacheArtifact {
        module_abi: keel_registry::MODULE_ABI,
        manifest_hash: keel_registry::manifests_hash(root)
            .map_err(|e| anyhow::anyhow!("manifest hash failed: {e}"))?,
        plans: plans.clone(),
    };
    let path = keel_registry::route_cache_path(root);
    artifact
        .write(&path)
        .map_err(|e| anyhow::anyhow!("route cache write failed: {e}"))?;
    let rel = path.strip_prefix(root).unwrap_or(&path);
    println!("  + {} ({} routes)", rel.display(), plans.len());
    println!("route cache written. Boot now pins on this artifact; re-run after manifest changes or use route:clear.");
    Ok(ExitCode::SUCCESS)
}

/// Wave 1: delete the route cache artifact (boot falls back to live
/// route resolution from the catalog).
fn route_clear() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    route_clear_at(&root)
}

/// Testable core of route:clear against an explicit workspace root.
fn route_clear_at(root: &Path) -> anyhow::Result<ExitCode> {
    let path = keel_registry::route_cache_path(root);
    if path.exists() {
        std::fs::remove_file(&path)?;
        let rel = path.strip_prefix(root).unwrap_or(&path);
        println!("  - {}", rel.display());
        println!("route cache cleared; boot resolves routes live.");
    } else {
        println!("no route cache present ({}).", path.display());
    }
    Ok(ExitCode::SUCCESS)
}

fn event_list() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let graph = match compile_graph(&root) {
        Ok(g) => g,
        Err(code) => return Ok(code),
    };
    for (event, module) in &graph.events {
        println!("  {event:<40} ({module})");
    }
    if graph.events.is_empty() {
        println!("  (none)");
    }
    Ok(ExitCode::SUCCESS)
}

fn sdk_openapi(output: Option<String>) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let graph = match compile_graph(&root) {
        Ok(g) => g,
        Err(code) => return Ok(code),
    };
    let document =
        keel_codegen::openapi::emit_openapi(&graph, "keel-platform", env!("CARGO_PKG_VERSION"));
    let rendered = serde_json::to_string_pretty(&document)?;
    match output {
        Some(path) => {
            std::fs::write(&path, rendered)?;
            println!("  + {path}");
        }
        None => println!("{rendered}"),
    }
    Ok(ExitCode::SUCCESS)
}

fn mcp_tools(format: &str) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let graph = match compile_graph(&root) {
        Ok(g) => g,
        Err(code) => return Ok(code),
    };
    let tools = keel_ai_mcp::tools_from_graph(&graph);
    match format {
        "json" => {
            let manifest =
                keel_ai_mcp::generate_manifest("keel-platform", env!("CARGO_PKG_VERSION"), &tools);
            println!("{}", serde_json::to_string_pretty(&manifest)?);
        }
        "table" => {
            for spec in &tools {
                println!(
                    "  {:<32} {:<32} {}",
                    spec.name,
                    spec.operation.as_str(),
                    spec.capability.as_deref().unwrap_or("-"),
                );
            }
            if tools.is_empty() {
                println!("  (none — no modules registered yet)");
            }
        }
        other => anyhow::bail!("unknown format {other:?} (expected table or json)"),
    }
    Ok(ExitCode::SUCCESS)
}

fn capabilities_list() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let graph = match compile_graph(&root) {
        Ok(g) => g,
        Err(code) => return Ok(code),
    };
    for m in &graph.modules {
        for (cap, spec) in &m.capabilities {
            println!("  {}.{}  profile={:?}", m.name(), cap, spec.profile);
        }
    }
    if graph.modules.iter().all(|m| m.capabilities.is_empty()) {
        println!("  (none)");
    }
    Ok(ExitCode::SUCCESS)
}

// -------------------------------------------------------------- migrations

/// AppError rendered for CLI stderr: the internal detail (pair names, SQL
/// text) when present. HTTP envelopes keep that detail out of responses —
/// a terminal is the operator's own, and `[DB_ERROR] a database error
/// occurred` would hide which migration broke.
fn db_err(e: keel_foundation::AppError) -> anyhow::Error {
    match e.internal {
        Some(detail) => anyhow::anyhow!("{detail}"),
        None => anyhow::anyhow!("{e}"),
    }
}

fn migrator() -> &'static sqlx::migrate::Migrator {
    // Embedded from database/migrations, same source keel-postgres boots with.
    static MIGRATOR: std::sync::OnceLock<sqlx::migrate::Migrator> = std::sync::OnceLock::new();
    MIGRATOR.get_or_init(|| sqlx::migrate!("../../database/migrations"))
}

async fn migrate() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let pair_applied = keel_postgres::run_migrations(&pool, &root)
        .await
        .map_err(db_err)?;
    for name in &pair_applied {
        println!("  + {name}");
    }
    println!("migrations applied.");
    Ok(ExitCode::SUCCESS)
}

async fn migrate_status() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let migrator = migrator();
    let applied: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
            .fetch_all(&pool)
            .await
            .unwrap_or_default();
    let mut pending = 0usize;
    for migration in migrator.iter() {
        let is_applied = applied.contains(&migration.version);
        if !is_applied {
            pending += 1;
        }
        println!(
            "  {:<8} {:<16} {}",
            if is_applied { "applied" } else { "PENDING" },
            migration.version,
            migration.description
        );
    }
    // Wave 5: rollback-capable pairs (keel_migrations may not exist yet —
    // status stays read-only, so a missing bookkeeping table just means
    // every pair is pending).
    let pair_applied: Vec<String> =
        sqlx::query_scalar("SELECT migration FROM keel_migrations ORDER BY migration")
            .fetch_all(&pool)
            .await
            .unwrap_or_default();
    for pair in keel_postgres::pairs::scan_pairs(&root).map_err(db_err)? {
        let is_applied = pair_applied.contains(&pair.migration);
        if !is_applied {
            pending += 1;
        }
        println!(
            "  {:<8} {} (pair)",
            if is_applied { "applied" } else { "PENDING" },
            pair.migration
        );
    }
    println!("{pending} pending migration(s).");
    Ok(ExitCode::SUCCESS)
}

async fn migrate_fresh() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let cfg = load_config(&root)?;
    if cfg.app.env.is_prod() || matches!(cfg.app.env, Environment::Staging) {
        anyhow::bail!("migrate:fresh refuses to run outside dev/test");
    }
    if std::env::var_os("KEEL_FORCE").is_none()
        && !confirm("migrate:fresh DROPS the public schema and re-runs every migration. Continue?")
    {
        anyhow::bail!("aborted (set KEEL_FORCE=1 to skip the prompt in scripts)");
    }
    let pool = db_pool(&root).await?;
    // The schema drop happens outside the advisory lock (fresh is dev/test
    // only and confirmed); the migration run that follows locks inside
    // run_migrations — locking here too would deadlock against it on a
    // second connection.
    sqlx::query("DROP SCHEMA public CASCADE")
        .execute(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("drop schema: {e}"))?;
    sqlx::query("CREATE SCHEMA public")
        .execute(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("create schema: {e}"))?;
    let pair_applied = keel_postgres::run_migrations(&pool, &root)
        .await
        .map_err(db_err)?;
    for name in &pair_applied {
        println!("  + {name}");
    }
    println!("schema dropped, recreated and migrated.");
    Ok(ExitCode::SUCCESS)
}

/// Wave 5: revert the last `steps` migration batches (pair migrations only;
/// legacy single-file migrations stay forward-only). Not env-gated: rollback
/// is the bounded recovery op (`--step`, default 1).
async fn migrate_rollback(steps: u32) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    // The advisory lock is session-scoped: acquire, roll back, and release
    // on one pooled connection (a concurrent migrate serializes instead of
    // interleaving).
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| anyhow::anyhow!("migrate: connect: {e}"))?;
    keel_postgres::acquire_migration_lock(&mut conn)
        .await
        .map_err(db_err)?;
    let outcome = keel_postgres::pairs::rollback(&mut *conn, &root, steps).await;
    let released = keel_postgres::release_migration_lock(&mut conn).await;
    let reverted = outcome.map_err(db_err)?;
    released.map_err(db_err)?;
    if reverted.is_empty() {
        println!("nothing to roll back ({steps} batch(es) requested).");
        return Ok(ExitCode::SUCCESS);
    }
    for name in &reverted {
        println!("  - {name}");
    }
    println!(
        "rolled back {} pair migration(s); forward-only singles untouched.",
        reverted.len()
    );
    Ok(ExitCode::SUCCESS)
}

/// Wave 5: revert every applied pair migration (dev/test only — mirrors
/// migrate:fresh's gate; `migrate` rebuilds what this drops).
async fn migrate_reset() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let cfg = load_config(&root)?;
    if cfg.app.env.is_prod() || matches!(cfg.app.env, Environment::Staging) {
        anyhow::bail!("migrate:reset refuses to run outside dev/test");
    }
    if std::env::var_os("KEEL_FORCE").is_none()
        && !confirm("migrate:reset reverts EVERY applied pair migration (schema objects drop). Continue?")
    {
        anyhow::bail!("aborted (set KEEL_FORCE=1 to skip the prompt in scripts)");
    }
    let pool = db_pool(&root).await?;
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| anyhow::anyhow!("migrate: connect: {e}"))?;
    keel_postgres::acquire_migration_lock(&mut conn)
        .await
        .map_err(db_err)?;
    let outcome = keel_postgres::pairs::reset(&mut *conn, &root).await;
    let released = keel_postgres::release_migration_lock(&mut conn).await;
    let reverted = outcome.map_err(db_err)?;
    released.map_err(db_err)?;
    for name in &reverted {
        println!("  - {name}");
    }
    println!(
        "reset: reverted {} pair migration(s); run migrate to rebuild.",
        reverted.len()
    );
    Ok(ExitCode::SUCCESS)
}

/// Wave 5: rollback `steps` batches then re-apply (dev/test only — the
/// rollback window drops schema objects before recreating them).
async fn migrate_refresh(steps: u32) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let cfg = load_config(&root)?;
    if cfg.app.env.is_prod() || matches!(cfg.app.env, Environment::Staging) {
        anyhow::bail!("migrate:refresh refuses to run outside dev/test");
    }
    if std::env::var_os("KEEL_FORCE").is_none()
        && !confirm(&format!(
            "migrate:refresh rolls back {steps} batch(es) (schema objects drop) and re-applies. Continue?"
        ))
    {
        anyhow::bail!("aborted (set KEEL_FORCE=1 to skip the prompt in scripts)");
    }
    let pool = db_pool(&root).await?;
    // One lock across rollback + re-apply so no run interleaves between the
    // two phases.
    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| anyhow::anyhow!("migrate: connect: {e}"))?;
    keel_postgres::acquire_migration_lock(&mut conn)
        .await
        .map_err(db_err)?;
    let outcome = match keel_postgres::pairs::rollback(&mut *conn, &root, steps).await {
        Err(e) => Err(e),
        Ok(reverted) => match keel_postgres::run_migrations_held(&mut conn, &root).await {
            Ok(pair_applied) => Ok((reverted, pair_applied)),
            Err(e) => Err(e),
        },
    };
    let released = keel_postgres::release_migration_lock(&mut conn).await;
    let (reverted, pair_applied) = outcome.map_err(db_err)?;
    released.map_err(db_err)?;
    for name in &reverted {
        println!("  - {name}");
    }
    for name in &pair_applied {
        println!("  + {name}");
    }
    println!(
        "refresh: reverted {} migration(s), applied {} pair migration(s).",
        reverted.len(),
        pair_applied.len()
    );
    Ok(ExitCode::SUCCESS)
}

/// Wave 5: create the pair bookkeeping table without running files.
async fn migrate_install() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    keel_postgres::pairs::install(&pool)
        .await
        .map_err(db_err)?;
    println!(
        "{} bookkeeping table ready (no migration files run).",
        keel_postgres::pairs::PAIRS_TABLE
    );
    Ok(ExitCode::SUCCESS)
}

// --------------------------------------------------------------------- seed

async fn seed(only: Option<String>) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let mut registry = keel_postgres::SeederRegistry::new();
    registry
        .register(Arc::new(keel_postgres::DevTenantSeeder))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let only: Option<Vec<String>> = only.map(|list| {
        list.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });
    let results = registry
        .run(&pool, only.as_deref())
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    for (name, report) in &results {
        println!(
            "  {name}: inserted={} skipped={}",
            report.inserted, report.skipped
        );
    }
    println!("seed complete ({} seeder(s)).", results.len());
    Ok(ExitCode::SUCCESS)
}

// --------------------------------------------------------- config and keys

fn key_generate() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let key = keel_config::keys::generate_app_key();
    let cfg_path = config_path(&root);
    let dir = keel_config::dotenv::config_dir_of(&cfg_path);
    let env_file = dir.join(".env");
    let mut lines: Vec<String> = std::fs::read_to_string(&env_file)
        .map(|raw| raw.lines().map(String::from).collect())
        .unwrap_or_default();
    let mut replaced = false;
    for entry in lines.iter_mut() {
        if entry.starts_with("KEEL_APP_KEY=") {
            *entry = format!("KEEL_APP_KEY={key}");
            replaced = true;
        }
    }
    if !replaced {
        lines.push(format!("KEEL_APP_KEY={key}"));
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&env_file, lines.join("\n") + "\n")?;
    println!("application key set successfully ({}).", env_file.display());
    Ok(ExitCode::SUCCESS)
}

fn config_dump() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let cfg = load_config(&root)?;
    println!("{}", serde_json::to_string_pretty(&cfg.redacted_json())?);
    Ok(ExitCode::SUCCESS)
}

fn cache_clear() -> anyhow::Result<ExitCode> {
    println!("cache: nothing to clear — platform caches are in-process per node (MemoryCache).");
    println!("Restart api/worker processes to reset them.");
    Ok(ExitCode::SUCCESS)
}

// ------------------------------------------------------- config cache (Wave 4)

/// Wave 4 Phase 5: write the frozen effective config + content hash to
/// storage/framework/config.json. Boot re-derives the effective config and
/// hash-checks the artifact (drift-refuse, identical to route:cache).
fn config_cache() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    config_cache_at(&root)
}

/// Testable core of config:cache against an explicit workspace root.
fn config_cache_at(root: &Path) -> anyhow::Result<ExitCode> {
    let cfg = load_config(root)?.freeze();
    let json = cfg.cache_json();
    let artifact = keel_registry::ConfigCacheArtifact {
        abi: keel_registry::CONFIG_CACHE_ABI,
        config_hash: keel_registry::config_hash(&json),
        config: json,
    };
    let path = keel_registry::config_cache_path(root);
    artifact
        .write(&path)
        .map_err(|e| anyhow::anyhow!("config cache write failed: {e}"))?;
    let rel = path.strip_prefix(root).unwrap_or(&path);
    println!("  + {} (hash {})", rel.display(), artifact.config_hash);
    println!("config cache written. Boot now pins on this artifact; re-run after config changes or use config:clear.");
    Ok(ExitCode::SUCCESS)
}

/// Wave 4 Phase 5: delete the config cache artifact (boot reads the live
/// config without the drift pin).
fn config_clear() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    config_clear_at(&root)
}

/// Testable core of config:clear against an explicit workspace root.
fn config_clear_at(root: &Path) -> anyhow::Result<ExitCode> {
    let path = keel_registry::config_cache_path(root);
    if path.exists() {
        std::fs::remove_file(&path)?;
        let rel = path.strip_prefix(root).unwrap_or(&path);
        println!("  - {}", rel.display());
        println!("config cache cleared; boot reads the live config.");
    } else {
        println!("no config cache present ({}).", path.display());
    }
    Ok(ExitCode::SUCCESS)
}

/// Known top-level config sections for `keel config:show` lookups.
const CONFIG_SECTIONS: &[&str] = &[
    "app", "database", "auth", "otp", "storage", "limits", "telemetry", "views", "http",
];

/// Wave 4 Phase 5: print one config section with every secret redacted
/// (`--json` for JSON, TOML otherwise).
fn config_show(key: &str, json: bool) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    println!("{}", config_show_text(&root, key, json)?);
    Ok(ExitCode::SUCCESS)
}

/// Testable core of config:show: renders one section of the redacted config.
fn config_show_text(root: &Path, key: &str, json: bool) -> anyhow::Result<String> {
    let cfg = load_config(root)?;
    let redacted = cfg.redacted_json();
    let Some(section) = redacted.get(key) else {
        anyhow::bail!(
            "unknown config section {key:?}; known sections: {}",
            CONFIG_SECTIONS.join(", ")
        );
    };
    if section.is_null() {
        anyhow::bail!("config section {key:?} is not configured");
    }
    if json {
        return Ok(serde_json::to_string_pretty(section)?);
    }
    toml::to_string_pretty(section).map_err(|e| {
        anyhow::anyhow!("section {key:?} is not TOML-renderable ({e}); retry with --json")
    })
}

/// Wave 4 Phase 5 (`keel env`): environment summary — the config file the
/// layering resolved, redacted key facts and the config-cache pin state.
fn env_summary() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    print!("{}", env_summary_text(&root)?);
    Ok(ExitCode::SUCCESS)
}

/// Testable core of `keel env` (returns the summary instead of printing).
fn env_summary_text(root: &Path) -> anyhow::Result<String> {
    let cfg_path = config_path(root);
    let cfg = load_config(root)?;
    let mut out = String::new();
    out.push_str(&format!("environment: {}\n", cfg.app.env.as_str()));
    out.push_str(&format!(
        "app: {} (listen {})\n",
        cfg.app.name, cfg.app.listen
    ));
    out.push_str(&format!("config file: {}\n", cfg_path.display()));
    match cfg.database.as_ref() {
        Some(db) => out.push_str(&format!(
            "database: {} (max_connections {}, run_migrations {})\n",
            keel_config::mask_url_password(&db.url),
            db.max_connections,
            db.run_migrations
        )),
        None => out.push_str("database: not configured\n"),
    }
    out.push_str(&format!("otp: {}\n", cfg.otp.provider));
    out.push_str(&format!(
        "storage: endpoint={} bucket={}\n",
        cfg.storage.endpoint, cfg.storage.bucket
    ));
    out.push_str(&format!(
        "limits: default_monthly_operations={}\n",
        cfg.limits.default_monthly_operations
    ));
    out.push_str(&format!(
        "views: {} path(s), cache_dir={}\n",
        cfg.views.paths.len(),
        cfg.views.cache_dir.as_deref().unwrap_or("(disabled)")
    ));
    out.push_str(&format!(
        "http: problem_json={} encrypt_cookies={}\n",
        cfg.http.problem_json, cfg.http.encrypt_cookies
    ));
    out.push_str(&format!(
        "gates: require_verified_email={} debug={}\n",
        cfg.app.require_verified_email, cfg.app.debug
    ));
    out.push_str(&format!(
        "secrets: app.key={} auth.jwt_secret={}\n",
        secret_state(cfg.app.key.as_str()),
        secret_state(cfg.auth.jwt_secret.as_str())
    ));
    let cache_path = keel_registry::config_cache_path(root);
    match keel_registry::ConfigCacheArtifact::load(&cache_path)
        .map_err(|e| anyhow::anyhow!("{e}"))?
    {
        Some(artifact) => out.push_str(&format!(
            "config cache: present (hash {})\n",
            artifact.config_hash
        )),
        None => out.push_str("config cache: not cached (run keel config:cache)\n"),
    }
    Ok(out)
}

/// Redaction state of one secret for the `keel env` summary.
fn secret_state(value: &str) -> &'static str {
    if value.is_empty() {
        "empty"
    } else if keel_config::keys::is_placeholder(value) {
        "placeholder"
    } else {
        "set"
    }
}

// -------------------------------------------------------- cache:forget (Wave 4)

/// Wave 4 Phase 5 (`keel cache:forget <key>`): remove exactly one cache key.
/// The caches that outlive a process are the framework disk artifacts
/// (routes, config) and the compiled view entries; in-process MemoryCache
/// keys belong to the running node (see cache:clear).
fn cache_forget(key: &str) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    cache_forget_at(&root, key)
}

/// Testable core of cache:forget against an explicit workspace root.
fn cache_forget_at(root: &Path, key: &str) -> anyhow::Result<ExitCode> {
    match key {
        "routes" => {
            forget_artifact(root, keel_registry::route_cache_path(root), key)
        }
        "config" => {
            forget_artifact(root, keel_registry::config_cache_path(root), key)
        }
        _ => {
            let Some(name) = key.strip_prefix("views:") else {
                anyhow::bail!(
                    "unknown cache key {key:?}; known keys: routes, config, views:<template>"
                );
            };
            if name.is_empty() {
                anyhow::bail!(
                    "cache key `views:` needs a template name (e.g. views:dashboard.index)"
                );
            }
            let engine = views_engine(root)?;
            let removed = engine
                .forget_cache(name)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            if removed {
                println!("cache: forgot {key}");
                return Ok(ExitCode::SUCCESS);
            }
            anyhow::bail!("cache key {key:?} not found (no compiled entry)")
        }
    }
}

/// Forget one single-file framework artifact (routes / config).
fn forget_artifact(root: &Path, path: std::path::PathBuf, key: &str) -> anyhow::Result<ExitCode> {
    if path.exists() {
        std::fs::remove_file(&path)?;
        let rel = path.strip_prefix(root).unwrap_or(&path);
        println!("cache: forgot {key} ({})", rel.display());
        Ok(ExitCode::SUCCESS)
    } else {
        anyhow::bail!("cache key {key:?} not found ({})", path.display())
    }
}

// -------------------------------------------------------------------- views

/// Wave 2: precompile every discoverable template under `[views].paths`
/// into the compiled cache (`[views].cache_dir`). Boot serves from this
/// cache (mtime + content hash), so a release runs view:cache after the
/// templates ship.
fn view_cache() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    view_cache_at(&root)
}

fn view_cache_at(root: &Path) -> anyhow::Result<ExitCode> {
    let engine = views_engine(root)?;
    let cached = engine
        .precompile_all()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    match engine.config().cache_dir.as_ref() {
        Some(dir) => println!("view:cache: {cached} templates compiled into {}", dir.display()),
        None => println!(
            "view:cache: {cached} templates compiled (no [views] cache_dir; disk cache disabled)"
        ),
    }
    Ok(ExitCode::SUCCESS)
}

/// Wave 2: drop every compiled view cache entry — the in-process memo plus
/// the `*.json` artifacts under the cache dir. Boot recompiles on demand,
/// so a corrupt or stale entry never survives a clear.
fn view_clear() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    view_clear_at(&root)
}

fn view_clear_at(root: &Path) -> anyhow::Result<ExitCode> {
    let engine = views_engine(root)?;
    let cleared = engine
        .clear_cache()
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("view:clear: {cleared} compiled views cleared");
    Ok(ExitCode::SUCCESS)
}

/// Wave 5: `keel optimize` warms every boot-pinning artifact in the
/// documented order (route:cache -> config:cache -> view:cache) and
/// aborts on the first failure — a graph-gate refusal never proceeds to
/// config or view, so a broken deploy fails fast instead of half-warm.
fn optimize() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    optimize_at(&root)
}

/// Testable core of optimize against an explicit workspace root.
fn optimize_at(root: &Path) -> anyhow::Result<ExitCode> {
    let code = route_cache_at(root)
        .map_err(|e| anyhow::anyhow!("optimize aborted at route:cache: {e:#}"))?;
    if code != ExitCode::SUCCESS {
        anyhow::bail!("optimize aborted at route:cache (graph gate refused the artifact)");
    }
    let code = config_cache_at(root)
        .map_err(|e| anyhow::anyhow!("optimize aborted at config:cache: {e:#}"))?;
    if code != ExitCode::SUCCESS {
        anyhow::bail!("optimize aborted at config:cache");
    }
    let code = view_cache_at(root)
        .map_err(|e| anyhow::anyhow!("optimize aborted at view:cache: {e:#}"))?;
    if code != ExitCode::SUCCESS {
        anyhow::bail!("optimize aborted at view:cache");
    }
    println!("optimize: route, config and view caches are warm.");
    Ok(ExitCode::SUCCESS)
}

/// Wave 5: `keel optimize:clear` removes all three boot-pinning artifacts.
/// Every step is attempted (a clear never leaves one cache behind because
/// another delete failed); the first error is reported afterwards.
fn optimize_clear() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    optimize_clear_at(&root)
}

/// Testable core of optimize:clear against an explicit workspace root.
fn optimize_clear_at(root: &Path) -> anyhow::Result<ExitCode> {
    let results = [
        route_clear_at(root),
        config_clear_at(root),
        view_clear_at(root),
    ];
    if let Some(err) = results.into_iter().find_map(Result::err) {
        return Err(err);
    }
    println!("optimize:clear: route, config and view caches are cold.");
    Ok(ExitCode::SUCCESS)
}

/// Build the views engine from the loaded `[views]` section — the CLI twin
/// of the boot conversion in `keel-api`. The dependency matrix forbids
/// `application -> web`, so each consumer maps the plain-data section
/// itself (same rules: relative paths resolve against the workspace root,
/// empty paths fall back to resources/views).
fn views_engine(root: &Path) -> anyhow::Result<keel_views::ViewsEngine> {
    let cfg = load_config(root)?;
    let env = cfg.app.env.as_str();
    Ok(keel_views::ViewsEngine::new(views_config(
        &cfg.views,
        root,
        env,
    )))
}

fn views_config(
    section: &keel_config::ViewsSection,
    root: &Path,
    env: &str,
) -> keel_views::ViewsConfig {
    let resolve = |raw: &str| {
        let path = std::path::PathBuf::from(raw);
        if path.is_absolute() {
            path
        } else {
            root.join(path)
        }
    };
    let mut paths: Vec<std::path::PathBuf> = section.paths.iter().map(|p| resolve(p)).collect();
    if paths.is_empty() {
        paths.push(root.join("resources/views"));
    }
    keel_views::ViewsConfig {
        paths,
        cache_dir: section.cache_dir.as_deref().map(resolve),
        env: section.env.clone().unwrap_or_else(|| env.to_string()),
    }
}

// -------------------------------------------------------------- maintenance

fn up() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    if keel_http::read_down_file(&root).is_some() {
        keel_http::remove_down_file(&root).map_err(|e| anyhow::anyhow!("{e}"))?;
        println!("application is now UP (maintenance file removed).");
    } else {
        println!("application was already up.");
    }
    Ok(ExitCode::SUCCESS)
}

fn down(retry: Option<u64>, secret: Option<String>) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let file = keel_http::MaintenanceFile {
        retry_secs: retry,
        secret,
        started_at: Some(time::OffsetDateTime::now_utc()),
    };
    let has_secret = file.secret.is_some();
    keel_http::write_down_file(&root, &file).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "application is DOWN (retry_after={:?}, secret={}).",
        retry,
        if has_secret { "set" } else { "none" }
    );
    Ok(ExitCode::SUCCESS)
}

// -------------------------------------------------------------------- jobs

fn parse_job_status(raw: &str) -> anyhow::Result<keel_jobs::JobStatus> {
    use keel_jobs::JobStatus;
    Ok(match raw {
        "pending" => JobStatus::Pending,
        "running" => JobStatus::Running,
        "succeeded" => JobStatus::Succeeded,
        "failed" => JobStatus::Failed,
        "dead" => JobStatus::Dead,
        other => anyhow::bail!(
            "unknown status {other:?} (expected pending|running|succeeded|failed|dead)"
        ),
    })
}

fn print_jobs(jobs: &[keel_jobs::DurableJob]) {
    for job in jobs {
        println!(
            "  {}  {:<20} q={:<10} {:<9} {}/{}  {}",
            &job.id.to_string()[..8],
            job.name,
            job.queue,
            format!("{:?}", job.status).to_lowercase(),
            job.attempt,
            job.max_attempts,
            job.last_error.as_deref().unwrap_or("")
        );
    }
    if jobs.is_empty() {
        println!("  (none)");
    }
}

async fn job_list(status: Option<String>, limit: usize) -> anyhow::Result<ExitCode> {
    use keel_jobs::JobStoreAdmin;
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let status = match status {
        Some(raw) => Some(parse_job_status(&raw)?),
        None => None,
    };
    let store = keel_postgres::PgJobStore::new(pool);
    let jobs = store
        .list(status, limit)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    print_jobs(&jobs);
    Ok(ExitCode::SUCCESS)
}

async fn queue_failed() -> anyhow::Result<ExitCode> {
    use keel_jobs::{JobStatus, JobStoreAdmin};
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let store = keel_postgres::PgJobStore::new(pool);
    let jobs = store
        .list(Some(JobStatus::Dead), 50)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("dead jobs awaiting manual retry:");
    print_jobs(&jobs);
    Ok(ExitCode::SUCCESS)
}

async fn queue_retry(id: &str) -> anyhow::Result<ExitCode> {
    use keel_jobs::JobStoreAdmin;
    let root = workspace_root()?;
    let job_id = uuid::Uuid::parse_str(id).map_err(|e| anyhow::anyhow!("invalid job id: {e}"))?;
    let pool = db_pool(&root).await?;
    let store = keel_postgres::PgJobStore::new(pool);
    store.requeue(job_id).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("job {job_id} requeued (pending).");
    Ok(ExitCode::SUCCESS)
}

/// Shared worker-binary resolution for queue:work and queue:listen: the
/// keel-worker next to this keel executable.
fn worker_binary() -> anyhow::Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot resolve executable directory"))?;
    let binary = if cfg!(windows) {
        "keel-worker.exe"
    } else {
        "keel-worker"
    };
    let worker = dir.join(binary);
    if !worker.exists() {
        anyhow::bail!("keel-worker not found next to keel ({binary}); build it with: cargo build -p keel-worker");
    }
    Ok(worker)
}

fn queue_work(once: bool) -> anyhow::Result<ExitCode> {
    let worker = worker_binary()?;
    let mut command = std::process::Command::new(&worker);
    if once {
        command.arg("--once");
    }
    println!("starting {} ...", worker.display());
    let status = command.status()?;
    Ok(if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// Wave 5: purge every pending job — dev/test + confirm gate (the
/// migrate:fresh shape); queued work in prod goes through the queue's own
/// lifecycle instead.
async fn queue_clear() -> anyhow::Result<ExitCode> {
    use keel_jobs::JobStoreAdmin;
    let root = workspace_root()?;
    let cfg = load_config(&root)?;
    if cfg.app.env.is_prod() || matches!(cfg.app.env, Environment::Staging) {
        anyhow::bail!("queue:clear refuses to run outside dev/test");
    }
    if std::env::var_os("KEEL_FORCE").is_none()
        && !confirm("queue:clear deletes EVERY pending job (queued work is lost). Continue?")
    {
        anyhow::bail!("aborted (set KEEL_FORCE=1 to skip the prompt in scripts)");
    }
    let pool = db_pool(&root).await?;
    let store = keel_postgres::PgJobStore::new(pool);
    let purged = store.purge_pending().map_err(db_err)?;
    println!("queue:clear: {purged} pending job(s) purged.");
    Ok(ExitCode::SUCCESS)
}

/// Wave 5: delete every dead job (forensic rows are gone afterwards —
/// queue:prune-failed is the age-bounded variant).
async fn queue_flush() -> anyhow::Result<ExitCode> {
    use keel_jobs::JobStoreAdmin;
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let store = keel_postgres::PgJobStore::new(pool);
    let pruned = store
        .prune_dead(keel_jobs::flush_cutoff())
        .map_err(db_err)?;
    println!("queue:flush: {pruned} dead job(s) deleted.");
    Ok(ExitCode::SUCCESS)
}

/// Wave 5: delete one dead job (unknown id or not dead = refusal).
async fn queue_forget(id: &str) -> anyhow::Result<ExitCode> {
    use keel_jobs::JobStoreAdmin;
    let job_id = uuid::Uuid::parse_str(id).map_err(|e| anyhow::anyhow!("invalid job id: {e}"))?;
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let store = keel_postgres::PgJobStore::new(pool);
    if !store.delete_dead(job_id).map_err(db_err)? {
        anyhow::bail!("job {job_id} is not a dead job (nothing forgotten)");
    }
    println!("job {job_id} forgotten.");
    Ok(ExitCode::SUCCESS)
}

/// Wave 5: queue depth + oldest pending age snapshot.
async fn queue_monitor() -> anyhow::Result<ExitCode> {
    use keel_jobs::JobStoreAdmin;
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let store = keel_postgres::PgJobStore::new(pool);
    let snap = store.snapshot().map_err(db_err)?;
    print!(
        "{}",
        render_queue_monitor(&snap, time::OffsetDateTime::now_utc())
    );
    Ok(ExitCode::SUCCESS)
}

/// Render queue:monitor output (pure — unit-tested without a store).
fn render_queue_monitor(snap: &keel_jobs::QueueSnapshot, now: time::OffsetDateTime) -> String {
    let mut out = String::from("queue depth:\n");
    out.push_str(&format!("  pending:   {}", snap.pending));
    if let Some(oldest) = snap.oldest_pending {
        out.push_str(&format!(" (oldest {})", format_age(now - oldest)));
    }
    out.push('\n');
    out.push_str(&format!("  running:   {}\n", snap.running));
    out.push_str(&format!("  dead:      {}\n", snap.dead));
    out.push_str(&format!("  succeeded: {}\n", snap.succeeded));
    out
}

/// Compact human age: 45s / 3m20s / 2h5m.
fn format_age(age: time::Duration) -> String {
    let total = age.whole_seconds().max(0);
    let hours = total / 3600;
    let minutes = (total % 3600) / 60;
    let seconds = total % 60;
    if hours > 0 {
        format!("{hours}h{minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m{seconds}s")
    } else {
        format!("{seconds}s")
    }
}

/// Wave 5: prune dead jobs that ended before now - --hours (default 168).
async fn queue_prune_failed(hours: u64) -> anyhow::Result<ExitCode> {
    use keel_jobs::JobStoreAdmin;
    let hours = i64::try_from(hours).map_err(|_| anyhow::anyhow!("--hours out of range"))?;
    if hours < 1 {
        anyhow::bail!("--hours must be at least 1");
    }
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let store = keel_postgres::PgJobStore::new(pool);
    let cutoff = time::OffsetDateTime::now_utc() - time::Duration::hours(hours);
    let pruned = store.prune_dead(cutoff).map_err(db_err)?;
    println!("queue:prune-failed: {pruned} dead job(s) older than {hours}h deleted.");
    Ok(ExitCode::SUCCESS)
}

/// Wave 5: prune terminal batch records older than --hours (default 72).
/// Honest empty run: the batch ledger (BatchRuntime) is in-memory only, so
/// no terminal batch records are persisted yet. Command, flag and default
/// are real (docs/19 wire contract); output states exactly what happened.
async fn queue_prune_batches(hours: u64) -> anyhow::Result<ExitCode> {
    let hours = i64::try_from(hours).map_err(|_| anyhow::anyhow!("--hours out of range"))?;
    if hours < 1 {
        anyhow::bail!("--hours must be at least 1");
    }
    println!("queue:prune-batches: pruned 0 batch record(s) (cutoff {hours}h).");
    println!(
        "  the batch ledger is in-memory only (BatchRuntime); no terminal batch records are persisted yet."
    );
    Ok(ExitCode::SUCCESS)
}

/// Wave 5: write the restart signal file running workers honor.
fn queue_restart() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let path = keel_jobs::write_queue_restart_signal(&root)
        .map_err(|e| anyhow::anyhow!("queue:restart signal write: {e}"))?;
    let rel = path.strip_prefix(&root).unwrap_or(&path);
    println!("  + {}", rel.display());
    println!(
        "restart signal written; running workers stop after their current job (supervisors respawn them — see queue:listen)."
    );
    Ok(ExitCode::SUCCESS)
}

/// Wave 5: supervise keel-worker — respawn on every exit with a crash-loop
/// backoff so an instantly-dying worker never spins the supervisor hot.
/// Returns only through Ctrl-C (the OS default handler ends the supervisor
/// and its console-attached child together).
fn queue_listen() -> anyhow::Result<ExitCode> {
    let worker = worker_binary()?;
    println!(
        "queue:listen: supervising {} (Ctrl-C stops the supervisor)",
        worker.display()
    );
    let mut generation = 0u64;
    loop {
        generation += 1;
        let started = std::time::Instant::now();
        let status = match std::process::Command::new(&worker).status() {
            Ok(status) => status,
            Err(e) => {
                eprintln!("queue:listen: cannot start the worker: {e}");
                std::thread::sleep(std::time::Duration::from_secs(1));
                continue;
            }
        };
        let how = status
            .code()
            .map(|c| format!("exit {c}"))
            .unwrap_or_else(|| "signal".to_string());
        println!("queue:listen: worker generation {generation} ended ({how}); restarting");
        if started.elapsed() < std::time::Duration::from_secs(2) {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
}

/// Wave 5: requeue every failed member of a batch — fails closed: batch
/// membership cannot be resolved without a persisted ledger. The id is
/// still validated so the wire contract holds when the ledger lands.
async fn queue_retry_batch(id: &str) -> anyhow::Result<ExitCode> {
    let _batch =
        uuid::Uuid::parse_str(id).map_err(|e| anyhow::anyhow!("invalid batch id: {e}"))?;
    anyhow::bail!(
        "queue:retry-batch {id}: the batch ledger is not persisted (BatchRuntime keeps batches in memory; postgres persistence lands with the jobs-ledger follow-up) — requeue members individually with queue:retry <job-id>"
    );
}

// ---------------------------------------------------------------- schedules

fn schedule_info(command: &str) -> anyhow::Result<ExitCode> {
    println!("{command}: no module schedules registered yet.");
    println!("Schedules are declared by modules through ScheduleRegistry (keel-scheduling); the framework surface is in place, module content lands with S3.");
    Ok(ExitCode::SUCCESS)
}

// -------------------------------------------------------------- db inspect

/// Public tables with their row counts (shared by db:inspect and db:show).
async fn table_row_counts(pool: &PgPool) -> anyhow::Result<Vec<(String, i64)>> {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname = 'public' ORDER BY tablename",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut out = Vec::with_capacity(tables.len());
    for table in tables {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM \"{table}\""))
            .fetch_one(pool)
            .await
            .map_err(|e| anyhow::anyhow!("count {table}: {e}"))?;
        out.push((table, count));
    }
    Ok(out)
}

async fn db_inspect() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let counts = table_row_counts(&pool).await?;
    println!("public schema: {} table(s)", counts.len());
    for (table, count) in &counts {
        println!("  {:<24} {count}", table);
    }
    Ok(ExitCode::SUCCESS)
}

/// Server + schema facts behind `db:show`, kept as data so the rendering is
/// unit-testable without a database.
struct DbSummary {
    driver: String,
    server: String,
    database: String,
    tables: Vec<(String, i64)>,
}

/// Wave 5: `db:show` — driver, server version, database name, and the
/// table inventory with row counts (db:inspect extended with server
/// metadata).
async fn db_show() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let cfg = load_config(&root)?;
    let pool = db_pool(&root).await?;
    let driver = cfg
        .database
        .as_ref()
        .and_then(|db| db.url.split("://").next())
        .filter(|scheme| !scheme.is_empty())
        .unwrap_or("postgresql")
        .to_string();
    let server: String = sqlx::query_scalar("SELECT version()")
        .fetch_one(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("server version: {e}"))?;
    let database: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("current database: {e}"))?;
    let summary = DbSummary {
        driver,
        server,
        database,
        tables: table_row_counts(&pool).await?,
    };
    print!("{}", render_db_show(&summary));
    Ok(ExitCode::SUCCESS)
}

/// Render `db:show` output (pure — unit-tested without a database).
fn render_db_show(summary: &DbSummary) -> String {
    let mut out = String::new();
    out.push_str(&format!("driver:   {}\n", summary.driver));
    out.push_str(&format!("server:   {}\n", summary.server));
    out.push_str(&format!("database: {}\n", summary.database));
    out.push_str(&format!("tables:   {}\n", summary.tables.len()));
    for (table, rows) in &summary.tables {
        out.push_str(&format!("  {:<28} {rows:>10}\n", table));
    }
    out
}

// ---------------------------------------------------- schema dump (db:schema:dump)

/// One column of the normalized snapshot (ordinal order upstream).
struct SchemaColumn {
    name: String,
    data_type: String,
    not_null: bool,
    default_expr: Option<String>,
}

/// One table: columns in ordinal order, primary-key columns in key order,
/// and full CREATE INDEX statements (PK indexes excluded — rendered inline).
struct SchemaTable {
    name: String,
    columns: Vec<SchemaColumn>,
    primary_key: Vec<String>,
    indexes: Vec<String>,
}

/// Column catalog (public schema), sorted by table then ordinal.
const SCHEMA_COLUMNS_SQL: &str = r#"SELECT c.relname AS table_name,
       a.attnum AS ordinal,
       a.attname AS column_name,
       format_type(a.atttypid, a.atttypmod) AS data_type,
       a.attnotnull AS not_null,
       pg_get_expr(d.adbin, d.adrelid) AS default_expr
  FROM pg_catalog.pg_class c
  JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
  JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid
  LEFT JOIN pg_catalog.pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
 WHERE n.nspname = 'public' AND c.relkind = 'r'
   AND a.attnum > 0 AND NOT a.attisdropped
 ORDER BY c.relname, a.attnum"#;

/// Primary-key columns (public schema), sorted by table then key position.
const SCHEMA_PRIMARY_KEYS_SQL: &str = r#"SELECT c.relname AS table_name, a.attname AS column_name
  FROM pg_catalog.pg_index ix
  JOIN pg_catalog.pg_class c ON c.oid = ix.indrelid
  JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
  JOIN LATERAL unnest(ix.indkey) WITH ORDINALITY AS k(attnum, ord) ON true
  JOIN pg_catalog.pg_attribute a ON a.attrelid = c.oid AND a.attnum = k.attnum
 WHERE n.nspname = 'public' AND ix.indisprimary AND k.attnum > 0
 ORDER BY c.relname, k.ord"#;

/// Secondary indexes (public schema; primary-key indexes excluded),
/// sorted by table then index name.
const SCHEMA_INDEXES_SQL: &str = r#"SELECT c.relname AS table_name, pg_get_indexdef(ix.indexrelid) AS index_def
  FROM pg_catalog.pg_index ix
  JOIN pg_catalog.pg_class c ON c.oid = ix.indrelid
  JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace
 WHERE n.nspname = 'public' AND NOT ix.indisprimary
 ORDER BY c.relname, ix.indexrelid::regclass::text"#;

/// Fetch the migrated schema from the live catalog, assembled into
/// [`SchemaTable`]s (tables without columns — impossible in practice — are
/// skipped rather than rendered as empty DDL).
async fn fetch_schema_tables(pool: &PgPool) -> anyhow::Result<Vec<SchemaTable>> {
    #[derive(sqlx::FromRow)]
    struct ColRow {
        table_name: String,
        column_name: String,
        data_type: String,
        not_null: bool,
        default_expr: Option<String>,
    }
    #[derive(sqlx::FromRow)]
    struct PkRow {
        table_name: String,
        column_name: String,
    }
    #[derive(sqlx::FromRow)]
    struct IdxRow {
        table_name: String,
        index_def: String,
    }

    let mut tables: std::collections::BTreeMap<String, SchemaTable> =
        std::collections::BTreeMap::new();
    let cols: Vec<ColRow> = sqlx::query_as(SCHEMA_COLUMNS_SQL)
        .fetch_all(pool)
        .await
        .map_err(|e| anyhow::anyhow!("schema columns: {e}"))?;
    for row in cols {
        tables
            .entry(row.table_name.clone())
            .or_insert_with(|| SchemaTable {
                name: row.table_name.clone(),
                columns: Vec::new(),
                primary_key: Vec::new(),
                indexes: Vec::new(),
            })
            .columns
            .push(SchemaColumn {
                name: row.column_name,
                data_type: row.data_type,
                not_null: row.not_null,
                default_expr: row.default_expr,
            });
    }
    let pks: Vec<PkRow> = sqlx::query_as(SCHEMA_PRIMARY_KEYS_SQL)
        .fetch_all(pool)
        .await
        .map_err(|e| anyhow::anyhow!("schema primary keys: {e}"))?;
    for row in pks {
        if let Some(table) = tables.get_mut(&row.table_name) {
            table.primary_key.push(row.column_name);
        }
    }
    let idxs: Vec<IdxRow> = sqlx::query_as(SCHEMA_INDEXES_SQL)
        .fetch_all(pool)
        .await
        .map_err(|e| anyhow::anyhow!("schema indexes: {e}"))?;
    for row in idxs {
        if let Some(table) = tables.get_mut(&row.table_name) {
            table.indexes.push(row.index_def);
        }
    }
    Ok(tables.into_values().collect())
}

/// Wave 5: `db:schema:dump` — normalized DDL snapshot of the migrated
/// schema, written to database/schema.sql for review. The file is never
/// executed by migrate (migrations stay the source of truth).
async fn db_schema_dump() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let tables = fetch_schema_tables(&pool).await?;
    let snapshot = render_schema_snapshot(&tables);
    let path = root.join("database").join("schema.sql");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, &snapshot)
        .map_err(|e| anyhow::anyhow!("schema snapshot write: {e}"))?;
    let rel = path.strip_prefix(&root).unwrap_or(&path);
    println!(
        "  + {} ({} table(s), normalized DDL)",
        rel.display(),
        tables.len()
    );
    println!("database/schema.sql written — review artifact; migrate never executes it.");
    Ok(ExitCode::SUCCESS)
}

/// Render the normalized snapshot (pure — unit-tested without a database):
/// tables sorted by name, one statement per table, PK inline, secondary
/// indexes after their table, deterministic formatting.
fn render_schema_snapshot(tables: &[SchemaTable]) -> String {
    let mut out = String::new();
    out.push_str("-- keel normalized schema snapshot (generated by `keel db:schema:dump`)\n");
    out.push_str("-- Review artifact: `keel migrate` never executes this file.\n");
    if tables.is_empty() {
        out.push_str("-- (no tables in schema public)\n");
        return out;
    }
    let mut sorted: Vec<&SchemaTable> = tables.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    for table in sorted {
        out.push_str(&format!("CREATE TABLE \"{}\" (\n", table.name));
        let mut lines: Vec<String> = table
            .columns
            .iter()
            .map(|c| {
                let mut line = format!("  \"{}\" {}", c.name, c.data_type);
                if c.not_null {
                    line.push_str(" NOT NULL");
                }
                if let Some(default) = &c.default_expr {
                    line.push_str(&format!(" DEFAULT {default}"));
                }
                line
            })
            .collect();
        if !table.primary_key.is_empty() {
            let pk: Vec<String> = table
                .primary_key
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect();
            lines.push(format!("  PRIMARY KEY ({})", pk.join(", ")));
        }
        out.push_str(&lines.join(",\n"));
        out.push_str("\n);\n");
        for index in &table.indexes {
            out.push_str(&format!("{index};\n"));
        }
        out.push('\n');
    }
    out
}

// -------------------------------------------------------- project and make

fn walk_files(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            walk_files(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}

/// keel new: materialize templates/project-template into applications/<name>.
fn new_project(
    name: &str,
    plane: &str,
    description: &str,
    with_starter: bool,
) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let description = if description.is_empty() {
        format!("Project {name}")
    } else {
        description.to_string()
    };
    let spec = keel_codegen::ModuleSpec {
        name: name.to_string(),
        plane: plane.to_string(),
        description: description.clone(),
    };
    spec.validate().map_err(|e| anyhow::anyhow!("{e}"))?;

    let template_rel = if with_starter {
        "templates/starter-template"
    } else {
        "templates/project-template"
    };
    let template_root = root.join(template_rel);
    if !template_root.exists() {
        anyhow::bail!("{template_rel} is missing under {}", root.display());
    }
    let target_root = root.join("applications").join(name);
    if target_root.exists() {
        anyhow::bail!("applications/{name} already exists");
    }

    let crate_name = format!("keel-{name}");
    let mut files = Vec::new();
    walk_files(&template_root, &mut files)?;
    files.sort();
    let mut written = Vec::new();
    for file in &files {
        let rel = file.strip_prefix(&template_root)?;
        let raw = std::fs::read_to_string(file)?;
        let content = raw
            .replace("__PROJECT_NAME__", name)
            .replace("__PROJECT_CRATE__", &crate_name)
            .replace("__PROJECT_DESCRIPTION__", &description);
        let target = target_root.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, content)?;
        written.push(target);
    }
    for path in &written {
        let rel = path.strip_prefix(&root).unwrap_or(path);
        println!("  + {}", rel.display());
    }
    keel_codegen::add_workspace_member(&root, &format!("applications/{name}"))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("  + workspace member applications/{name}");
    println!("project {name} created. Verify: cargo check -p {crate_name}");
    if with_starter {
        println!("starter extras:");
        println!(
            "  - web/index.html + web/app.js — SPA shell talking to /auth/* and your module routes"
        );
        println!("  - module.toml routes are named — generate URLs with the graph and OpenAPI client via: keel sdk:openapi");
        println!("  - run the loop with: keel serve");
    }
    Ok(ExitCode::SUCCESS)
}

/// keel make <kind> <name> [--module <m>]: scaffold one file (item 30).
fn make_kind(kind: &str, name: &str, module: Option<String>) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let module = match kind {
        "migration" => {
            // Timestamped platform migration; same path keel make:migration uses.
            return make_migration(name);
        }
        _ => module.ok_or_else(|| {
            anyhow::anyhow!("make:{kind} needs --module <name> (create one with make:module)")
        })?,
    };
    if !root.join("applications").join(&module).exists() {
        anyhow::bail!("module {module:?} not found under applications/ (create it with: keel make:module {module})");
    }
    let (rel, content) = keel_codegen::generators::generate(kind, &module, name)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let path = root.join(&rel);
    if path.exists() {
        anyhow::bail!("refusing to overwrite existing file: {}", path.display());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, content)?;
    let display = path.strip_prefix(&root).unwrap_or(&path);
    println!("  + {}", display.display());
    println!("{kind} {name} scaffolded. Wire it into the module mod tree to compile it.");
    Ok(ExitCode::SUCCESS)
}
// ------------------------------------------------------------------- build

fn rustc_version() -> String {
    std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|v| v.trim().to_string())
        .unwrap_or_else(|| "unknown".into())
}

fn build(profile_raw: &str) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let profile =
        keel_deployment::BuildProfile::parse(profile_raw).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "keel build — profile {} (matrix: RELEASE.md)",
        profile.as_str()
    );
    let status = std::process::Command::new("cargo")
        .arg("build")
        .args(profile.cargo_flag())
        .args(["-p", "keel-api", "-p", "keel-worker"])
        .current_dir(&root)
        .status()?;
    if !status.success() {
        anyhow::bail!("cargo build failed ({status})");
    }
    // The running CLI cannot replace its own binary on Windows (file lock),
    // so build keel-console separately and tolerate that one failure when the
    // running executable lives inside the workspace target dir.
    let cli_status = std::process::Command::new("cargo")
        .arg("build")
        .args(profile.cargo_flag())
        .args(["-p", "keel-console"])
        .current_dir(&root)
        .status()?;
    if !cli_status.success() {
        let running_from_target = std::env::current_exe()
            .map(|exe| exe.starts_with(root.join("target")))
            .unwrap_or(false);
        if running_from_target {
            println!("warning: keel-console binary is locked while running; skipped its refresh");
        } else {
            anyhow::bail!("cargo build failed ({cli_status})");
        }
    }
    let (git_commit, git_dirty) = keel_deployment::git_identity(&root);
    let workspace_version = "0.1.0".to_string();
    let manifest = keel_deployment::ReleaseManifest {
        version: workspace_version.clone(),
        profile,
        artifacts: keel_deployment::RELEASE_BINARIES
            .iter()
            .map(|name| keel_deployment::Artifact {
                name: (*name).to_string(),
                version: workspace_version.clone(),
                profile,
            })
            .collect(),
        git_commit,
        git_dirty,
        built_at: time::OffsetDateTime::now_utc(),
        rustc: rustc_version(),
    };
    let path =
        keel_deployment::write_manifest(&root, &manifest).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("release manifest: {}", path.display());
    println!(
        "artifacts: {}",
        manifest
            .artifacts
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(ExitCode::SUCCESS)
}

fn release_manifest() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    match keel_deployment::read_manifest(&root).map_err(|e| anyhow::anyhow!("{e}"))? {
        Some(manifest) => {
            let raw = serde_json::to_string_pretty(&manifest)?;
            println!("{raw}");
            Ok(ExitCode::SUCCESS)
        }
        None => {
            println!("no release manifest found (run `keel build` first).");
            Ok(ExitCode::SUCCESS)
        }
    }
}

// ------------------------------------------------------------------- flags

fn flag_store(root: &std::path::Path) -> keel_feature_flags::FileFlagStore {
    keel_feature_flags::FileFlagStore::new(keel_feature_flags::default_flags_path(root))
}

fn flag_list() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let registry = keel_feature_flags::platform_registry();
    let store = flag_store(&root);
    let states = keel_feature_flags::FlagStore::all(&store).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("platform flags ({}):", registry.definitions().len());
    for def in registry.definitions() {
        let state = states.iter().find(|st| st.name == def.name);
        let stage = state.map(|st| st.stage).unwrap_or(def.default_stage);
        println!(
            "  {:<20} {:<10} {}",
            def.name,
            stage.as_str(),
            def.description
        );
        if let Some(st) = state {
            for (tenant, tenant_stage) in &st.tenant_stages {
                println!("    tenant {tenant}: {}", tenant_stage.as_str());
            }
        }
    }
    println!(
        "state file: {}",
        keel_feature_flags::default_flags_path(&root).display()
    );
    Ok(ExitCode::SUCCESS)
}

fn flag_set(name: &str, stage_raw: &str, tenant: Option<String>) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let stage =
        keel_feature_flags::RolloutStage::parse(stage_raw).map_err(|e| anyhow::anyhow!("{e}"))?;
    let service = keel_feature_flags::FlagService::new(
        std::sync::Arc::new(keel_feature_flags::platform_registry()),
        std::sync::Arc::new(flag_store(&root)),
    );
    let state = match tenant {
        Some(raw) => {
            let tenant_id = keel_foundation::TenantId::from_uuid(
                uuid::Uuid::parse_str(&raw).map_err(|e| anyhow::anyhow!("bad tenant uuid: {e}"))?,
            );
            service
                .set_tenant_stage(name, tenant_id, stage)
                .map_err(|e| anyhow::anyhow!("{e}"))?
        }
        None => service
            .set_stage(name, stage)
            .map_err(|e| anyhow::anyhow!("{e}"))?,
    };
    let revision = keel_feature_flags::FlagStore::revision(&*service.store)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "flag {} -> {} (feature revision {})",
        state.name,
        state.stage.as_str(),
        revision.0
    );
    Ok(ExitCode::SUCCESS)
}

// ------------------------------------------------------------------ prompts

fn stdin_interactive() -> bool {
    std::io::stdin().is_terminal()
}

/// Ask one question on stdin; `None` when not interactive (item 50, L56).
fn prompt_value(label: &str, default: Option<&str>) -> Option<String> {
    if !stdin_interactive() {
        return None;
    }
    use std::io::Write;
    match default {
        Some(d) => print!("{label} [{d}]: "),
        None => print!("{label}: "),
    }
    let _ = std::io::stdout().flush();
    let mut buf = String::new();
    let read = std::io::stdin().read_line(&mut buf).ok()?;
    if read == 0 {
        return None;
    }
    let trimmed = buf.trim();
    if trimmed.is_empty() {
        default.map(|s| s.to_string())
    } else {
        Some(trimmed.to_string())
    }
}

/// Yes/no confirmation; defaults to no and is always no off-terminal.
fn confirm(question: &str) -> bool {
    matches!(
        prompt_value(question, Some("y/N")).as_deref(),
        Some("y" | "yes" | "Y" | "YES" | "Yes")
    )
}

fn resolve_name_or_prompt(kind: &str, name: Option<String>) -> anyhow::Result<String> {
    if let Some(name) = name {
        return Ok(name);
    }
    match prompt_value(&format!("{kind} name ([a-z], digits, hyphens)"), None) {
        Some(name) if !name.trim().is_empty() => Ok(name),
        _ => anyhow::bail!(
            "{kind} name required (pass it as an argument or run in an interactive terminal)"
        ),
    }
}

fn make_module_cmd(
    name: Option<String>,
    plane: &str,
    description: &str,
) -> anyhow::Result<ExitCode> {
    let name = resolve_name_or_prompt("module", name)?;
    make_module(&name, plane, description)
}

fn new_project_cmd(
    name: Option<String>,
    plane: &str,
    description: &str,
    with_starter: bool,
) -> anyhow::Result<ExitCode> {
    let name = resolve_name_or_prompt("project", name)?;
    new_project(&name, plane, description, with_starter)
}

// ------------------------------------------------------------------ inspect

fn trace_store(root: &Path) -> FileTraceStore {
    FileTraceStore::standard(root)
}

fn parse_execution_id(raw: &str) -> anyhow::Result<keel_foundation::ExecutionId> {
    let uuid = uuid::Uuid::parse_str(raw)
        .map_err(|e| anyhow::anyhow!("invalid execution id {raw:?}: {e}"))?;
    Ok(keel_foundation::ExecutionId::from_uuid(uuid))
}

fn truncate(raw: &str, max: usize) -> String {
    let count = raw.chars().count();
    if count <= max {
        raw.to_string()
    } else {
        let kept: String = raw.chars().take(max.saturating_sub(1)).collect();
        format!("{kept}…")
    }
}

fn rfc3339(when: time::OffsetDateTime) -> String {
    when.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "?".to_string())
}

/// `keel inspect execution|delta|authorization|n-plus-one [id]` (item 45, L60).
fn inspect(
    what: &str,
    id: Option<&str>,
    limit: usize,
    threshold: usize,
) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let store = trace_store(&root);
    match what {
        "execution" => match id {
            Some(raw) => {
                match store
                    .get(parse_execution_id(raw)?)
                    .map_err(|e| anyhow::anyhow!("{e}"))?
                {
                    Some(trace) => {
                        println!("{}", keel_execution::render_trace(&trace));
                        Ok(ExitCode::SUCCESS)
                    }
                    None => {
                        eprintln!("no trace for {raw} under storage/framework/traces");
                        Ok(ExitCode::FAILURE)
                    }
                }
            }
            None => inspect_recent(&store, limit),
        },
        "delta" | "authorization" => inspect_view(&store, id, what),
        "n-plus-one" => inspect_n_plus_one(&store, limit, threshold),
        other => anyhow::bail!(
            "unknown inspect target {other:?} (expected execution|delta|authorization|n-plus-one)"
        ),
    }
}

fn inspect_recent(store: &dyn TraceStore, limit: usize) -> anyhow::Result<ExitCode> {
    let traces = store.recent(limit).map_err(|e| anyhow::anyhow!("{e}"))?;
    if traces.is_empty() {
        println!("no recorded traces under storage/framework/traces");
        return Ok(ExitCode::SUCCESS);
    }
    println!(
        "{:<38} {:<25} {:<32} {:<8} {:>7} {:>9}",
        "execution", "started (utc)", "operation", "authz", "queries", "ms"
    );
    for trace in &traces {
        println!(
            "{:<38} {:<25} {:<32} {:<8} {:>7} {:>9.1}",
            trace.execution_id,
            rfc3339(trace.started_at),
            truncate(&trace.operation, 32),
            trace.authz,
            trace.query_count(),
            trace.duration_ms
        );
    }
    println!("inspect one with: keel inspect execution <id>");
    Ok(ExitCode::SUCCESS)
}

fn inspect_view(store: &dyn TraceStore, id: Option<&str>, view: &str) -> anyhow::Result<ExitCode> {
    let Some(raw) = id else {
        anyhow::bail!("keel inspect {view} needs an execution id");
    };
    let Some(trace) = store
        .get(parse_execution_id(raw)?)
        .map_err(|e| anyhow::anyhow!("{e}"))?
    else {
        eprintln!("no trace for {raw} under storage/framework/traces");
        return Ok(ExitCode::FAILURE);
    };
    println!("{view} — execution {}", trace.execution_id);
    println!("────────────────────────────");
    match view {
        "delta" => {
            println!("deltas written   {}", trace.deltas_written);
            println!("audit written    {}", trace.audit_written);
            println!("outbox queued    {}", trace.outbox_queued);
            println!(
                "replayed         {}",
                if trace.idempotent_replay {
                    "yes (idempotent replay)"
                } else {
                    "no"
                }
            );
            let input = serde_json::to_string(&trace.input).unwrap_or_default();
            println!("captured input   {}", truncate(&input, 120));
            if let Some(error) = &trace.error {
                println!("error            {error}");
            }
            println!("note: stone payloads live in the delta store; traces record counts + captured input only");
        }
        "authorization" => {
            println!("operation        {}", trace.operation);
            println!(
                "capability       {}",
                trace
                    .capability
                    .as_deref()
                    .unwrap_or("— (authenticated-only)")
            );
            println!("decision         {}", trace.authz);
            println!("plane            {}", trace.plane);
            println!("principal        {}", principal_label(&trace));
            println!("tenant           {}", tenant_label(&trace));
            println!(
                "scope            {}",
                if trace.scope_paths.is_empty() {
                    "—".to_string()
                } else {
                    trace.scope_paths.join(", ")
                }
            );
        }
        _ => unreachable!("inspect_view called with {view}"),
    }
    Ok(ExitCode::SUCCESS)
}

fn inspect_n_plus_one(
    store: &dyn TraceStore,
    limit: usize,
    threshold: usize,
) -> anyhow::Result<ExitCode> {
    let traces = store
        .recent(limit.max(20))
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if traces.is_empty() {
        println!("no recorded traces to scan");
        return Ok(ExitCode::SUCCESS);
    }
    let findings = keel_execution::detect_n_plus_one(&traces, threshold);
    if findings.is_empty() {
        println!(
            "clean: no loop-shaped query patterns (threshold {} repeats/shape) across {} recent executions",
            threshold.max(2),
            traces.len()
        );
        return Ok(ExitCode::SUCCESS);
    }
    println!("N+1 candidates ({}):", findings.len());
    for finding in &findings {
        println!(
            "  {} · {} · shape {} × {}",
            finding.execution_id, finding.operation, finding.shape, finding.occurrences
        );
    }
    println!("inspect one with: keel inspect execution <id>");
    Ok(ExitCode::FAILURE)
}

fn principal_label(trace: &ExecutionTrace) -> String {
    match &trace.principal_display {
        Some(display) => format!("{} ({display})", trace.principal),
        None => trace.principal.to_string(),
    }
}

fn tenant_label(trace: &ExecutionTrace) -> String {
    match (&trace.tenant, &trace.tenant_slug) {
        (Some(tenant), Some(slug)) => format!("{tenant} ({slug})"),
        (Some(tenant), None) => tenant.to_string(),
        (None, _) => "—".to_string(),
    }
}

// ------------------------------------------------------------------- replay

/// `keel replay <execution-id> --dry-run` (item 46, L80): Original vs Replay.
fn replay(id: &str, dry_run: bool) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let store = trace_store(&root);
    let Some(trace) = store
        .get(parse_execution_id(id)?)
        .map_err(|e| anyhow::anyhow!("{e}"))?
    else {
        eprintln!("no trace for {id} under storage/framework/traces");
        return Ok(ExitCode::FAILURE);
    };
    if !dry_run {
        eprintln!(
            "live replay re-executes the captured operation through the runtime's registered handlers (spec: REPLAY is a front door into one Execution); the CLI comparison runs with --dry-run"
        );
        return Ok(ExitCode::FAILURE);
    }
    let input = serde_json::to_string(&trace.input).unwrap_or_default();
    println!("Replay {} (dry run)", trace.execution_id);
    println!();
    let row = |label: &str, original: String, replay: &str| {
        format!("{label:<14} {original:<46} {replay}")
    };
    println!("{}", row("field", "Original".to_string(), "Replay"));
    println!("{}", row(&"─".repeat(13), "─".repeat(45), &"─".repeat(40)));
    println!(
        "{}",
        row(
            "context",
            truncate(
                &format!(
                    "{} · {} · {} · plane {}",
                    trace.profile,
                    principal_label(&trace),
                    tenant_label(&trace),
                    trace.plane
                ),
                46
            ),
            "rebuilt from trace (same principal/tenant/scope)"
        )
    );
    println!(
        "{}",
        row(
            "authorization",
            truncate(
                &format!(
                    "{} · capability {}",
                    trace.authz,
                    trace.capability.as_deref().unwrap_or("—")
                ),
                46
            ),
            "re-evaluated at replay time"
        )
    );
    println!(
        "{}",
        row(
            "sql",
            truncate(
                &format!(
                    "{} queries · {:.1} ms",
                    trace.query_count(),
                    trace.total_query_ms()
                ),
                46
            ),
            "re-executed at replay time"
        )
    );
    println!(
        "{}",
        row(
            "result",
            if trace.ok() {
                truncate(&format!("ok · {} bytes", trace.result_bytes), 46)
            } else {
                truncate(
                    &format!("error: {}", trace.error.as_deref().unwrap_or("?")),
                    46,
                )
            },
            "re-executed at replay time"
        )
    );
    println!(
        "{}",
        row(
            "delta",
            truncate(
                &format!(
                    "{} deltas · {} audit · {} outbox{}",
                    trace.deltas_written,
                    trace.audit_written,
                    trace.outbox_queued,
                    if trace.idempotent_replay {
                        " (idempotent replay)"
                    } else {
                        ""
                    }
                ),
                46
            ),
            "new stones; recorded idempotency preserved"
        )
    );
    println!();
    println!(
        "captured input: {} (sensitive — stays under storage/framework/traces)",
        truncate(&input, 140)
    );
    Ok(ExitCode::SUCCESS)
}
// ---------------------------------------------------------------- benchmark

struct BenchCommand {
    plan: keel_execution::ExecutionPlan,
}

impl BenchCommand {
    fn new() -> Self {
        let intent = keel_execution::PlanIntent::new(
            keel_foundation::OperationId::from_static("bench.write"),
            keel_foundation::ExecutionProfileKind::Command,
        )
        .with_capability("bench.write");
        Self {
            plan: keel_execution::PlanCompiler::compile(&intent),
        }
    }
}

impl keel_execution::OperationHandler for BenchCommand {
    fn plan(&self) -> &keel_execution::ExecutionPlan {
        &self.plan
    }

    fn handle(
        &self,
        _ctx: Arc<keel_context::ExecutionContext>,
        input: serde_json::Value,
        unit: &mut dyn keel_execution::ExecutionUnit,
    ) -> keel_execution::BoxFuture<'static, Result<serde_json::Value, keel_foundation::AppError>> {
        unit.record_mutation(keel_execution::MutationRecord {
            resource_type: "bench.note".into(),
            resource_id: Some(uuid::Uuid::now_v7()),
            resource_version: 1,
            before: None,
            after: input,
            stream: None,
        });
        Box::pin(async move { Ok(serde_json::json!({ "ok": true })) })
    }
}

struct BenchRead {
    plan: keel_execution::ExecutionPlan,
}

impl BenchRead {
    fn new() -> Self {
        let intent = keel_execution::PlanIntent::new(
            keel_foundation::OperationId::from_static("bench.read"),
            keel_foundation::ExecutionProfileKind::Read,
        );
        Self {
            plan: keel_execution::PlanCompiler::compile(&intent),
        }
    }
}

impl keel_execution::OperationHandler for BenchRead {
    fn plan(&self) -> &keel_execution::ExecutionPlan {
        &self.plan
    }

    fn handle(
        &self,
        _ctx: Arc<keel_context::ExecutionContext>,
        _input: serde_json::Value,
        _unit: &mut dyn keel_execution::ExecutionUnit,
    ) -> keel_execution::BoxFuture<'static, Result<serde_json::Value, keel_foundation::AppError>> {
        Box::pin(async move { Ok(serde_json::json!({ "items": [] })) })
    }
}

fn bench_policy(
    principal: keel_foundation::PrincipalId,
    tenant: keel_foundation::TenantId,
) -> Arc<keel_policy::PolicyEngine> {
    let policies = Arc::new(keel_policy::PolicyEngine::new());
    policies.install_snapshot(keel_policy::AccessSnapshot {
        principal_id: principal,
        tenant_id: tenant,
        membership_active: true,
        capabilities: ["bench.write".to_string()].into_iter().collect(),
        scope_paths: ["/".to_string()].into_iter().collect(),
        authorization_revision: policies.revision(),
        feature_revision: keel_foundation::FeatureRevision(1),
        module_revision: keel_foundation::ModuleRevision(1),
        computed_at: time::OffsetDateTime::now_utc(),
    });
    policies
}

fn bench_ctx(
    kind: keel_foundation::ExecutionProfileKind,
    principal: keel_foundation::PrincipalId,
    tenant: keel_foundation::TenantId,
) -> Arc<keel_context::ExecutionContext> {
    keel_context::ExecutionContext::builder(kind, time::OffsetDateTime::now_utc())
        .principal(keel_context::PrincipalContext {
            id: principal,
            display: None,
        })
        .tenant(keel_context::TenantContext {
            id: tenant,
            slug: "bench".into(),
            plan: "standard".into(),
        })
        .scope(keel_context::ScopeSnapshot {
            nodes: vec![],
            revision: keel_foundation::AuthorizationRevision(1),
            computed_at: time::OffsetDateTime::now_utc(),
        })
        .build()
        .expect("bench context builds")
}

fn median_micros(iterations: usize, mut sample: impl FnMut() -> std::time::Duration) -> f64 {
    for _ in 0..3 {
        sample();
    }
    let mut samples: Vec<f64> = (0..iterations)
        .map(|_| sample().as_secs_f64() * 1_000_000.0)
        .collect();
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    samples[samples.len() / 2]
}

#[derive(serde::Serialize, serde::Deserialize)]
struct BenchmarkBaseline {
    version: u32,
    captured_at: String,
    rustc: String,
    iterations: usize,
    phases: std::collections::BTreeMap<String, f64>,
}

fn baseline_path(root: &Path) -> PathBuf {
    root.join("benchmarks/baseline.json")
}

fn load_baseline(root: &Path) -> Option<BenchmarkBaseline> {
    let raw = std::fs::read_to_string(baseline_path(root)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// `keel benchmark` (item 47, L75, G15): fixed in-memory workload, median
/// timing per phase, compared against `benchmarks/baseline.json`. Regressions
/// beyond the budget fail the command so CI can reject them.
fn benchmark(save_baseline: bool, budget: f64, iterations: usize) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let baseline = load_baseline(&root);
    let iterations = if save_baseline {
        iterations.max(10)
    } else {
        match &baseline {
            Some(b) if b.iterations != iterations.max(10) => {
                println!(
                    "note: comparing at the baseline iteration count ({})",
                    b.iterations.max(10)
                );
                b.iterations.max(10)
            }
            _ => iterations.max(10),
        }
    };
    println!(
        "keel benchmark — {iterations} timed iterations per phase (median, in-memory posture)"
    );

    let principal = keel_foundation::PrincipalId::new();
    let tenant = keel_foundation::TenantId::new();
    let policies = bench_policy(principal, tenant);
    let mut phases: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();

    // routing: intent -> fully-resolved ExecutionPlan (compiler fast path).
    let intent = keel_execution::PlanIntent::new(
        keel_foundation::OperationId::from_static("bench.write"),
        keel_foundation::ExecutionProfileKind::Command,
    )
    .with_capability("bench.write");
    phases.insert(
        "routing".into(),
        median_micros(iterations, || {
            let start = std::time::Instant::now();
            let plan =
                keel_execution::PlanCompiler::compile_checked(&intent).expect("plan compiles");
            std::hint::black_box(&plan);
            start.elapsed()
        }),
    );

    // authorization: pure decision path against a hydrated snapshot.
    let decision_input = keel_policy::DecisionInput {
        principal,
        tenant,
        capability: "bench.write",
        resource_owner_path: None,
        resource_state: None,
        module_enabled: true,
        attributes: None,
        conditions: &[],
    };
    phases.insert(
        "authorization".into(),
        median_micros(iterations, || {
            let start = std::time::Instant::now();
            let decision = policies.decide(&decision_input);
            std::hint::black_box(&decision);
            start.elapsed()
        }),
    );

    // serialization: representative payload round-trip.
    let payload = serde_json::json!({
        "id": "0190f3a5-0000-7000-8000-000000000000",
        "invoice_no": "INV-72",
        "tenant": "acme",
        "state": "posted",
        "lines": (0..8)
            .map(|i| serde_json::json!({ "sku": format!("SKU-{i}"), "qty": i + 1, "unit_price": 1200 + i }))
            .collect::<Vec<_>>(),
        "totals": { "net": 10800, "tax": 1944, "gross": 12744 },
        "meta": { "currency": "INR", "channel": "api", "revision": 7 },
    });
    phases.insert(
        "serialization".into(),
        median_micros(iterations, || {
            let start = std::time::Instant::now();
            let bytes = serde_json::to_vec(&payload).expect("serializes");
            let back: serde_json::Value = serde_json::from_slice(&bytes).expect("round-trips");
            std::hint::black_box(&back);
            start.elapsed()
        }),
    );

    // startup: config load (the file half of boot).
    let config_file = config_path(&root);
    phases.insert(
        "startup".into(),
        median_micros(iterations.min(100), || {
            let start = std::time::Instant::now();
            let cfg = keel_config::PlatformConfig::load(Some(&config_file)).expect("config loads");
            std::hint::black_box(&cfg);
            start.elapsed()
        }),
    );

    // delta-write + read: the full engine pipeline on the in-memory backend.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let backend = Arc::new(keel_execution::InMemoryBackend::new());
    let engine = keel_execution::Engine::new(
        Arc::new(keel_policy::PolicyAuthz {
            engine: policies.clone(),
        }),
        backend.clone(),
        Arc::new(keel_execution::InMemoryIdempotency::new()),
    );
    let command = BenchCommand::new();
    let read = BenchRead::new();
    let command_ctx = bench_ctx(keel_foundation::ExecutionProfileKind::Command, principal, tenant);
    let read_ctx = bench_ctx(keel_foundation::ExecutionProfileKind::Read, principal, tenant);
    let counter = std::sync::atomic::AtomicUsize::new(0);

    phases.insert(
        "delta-write".into(),
        median_micros(iterations, || {
            let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let key = keel_execution::IdempotencyKey(format!("bench-{n}"));
            let start = std::time::Instant::now();
            runtime
                .block_on(engine.execute(
                    command_ctx.clone(),
                    &command,
                    serde_json::json!({ "i": n }),
                    Some(key),
                ))
                .expect("bench command succeeds");
            start.elapsed()
        }),
    );

    phases.insert(
        "read".into(),
        median_micros(iterations, || {
            let start = std::time::Instant::now();
            runtime
                .block_on(engine.execute(read_ctx.clone(), &read, serde_json::json!({}), None))
                .expect("bench read succeeds");
            start.elapsed()
        }),
    );

    // telemetry-emit (Sentinel S5, STAGE-SENTINEL-SPEC §21 performance):
    // ingest overhead of the bounded bus against a live runtime with a real
    // file exporter + WAL. Inert constructs are cheap; only `start` spawns
    // the worker/poller threads, and they drain inside the shutdown deadline.
    let bench_telemetry_dir = root
        .join("target")
        .join("keel-bench-telemetry")
        .join(uuid::Uuid::now_v7().to_string());
    std::fs::create_dir_all(&bench_telemetry_dir)?;
    let mut bench_tcfg = keel_observability::TelemetryConfig::default();
    bench_tcfg.wal.dir = bench_telemetry_dir.join("wal");
    bench_tcfg.paths.incident_dir = bench_telemetry_dir.join("control");
    bench_tcfg.paths.cost_file = bench_telemetry_dir.join("cost.json");
    bench_tcfg.paths.health_dir = bench_telemetry_dir.join("exporters");
    bench_tcfg.exporters.insert(
        "file".into(),
        keel_observability::ExporterConfig {
            kind: "file".into(),
            target: Some(
                bench_telemetry_dir
                    .join("bench.jsonl")
                    .to_string_lossy()
                    .into_owned(),
            ),
            format: None,
            critical: true,
            partition_by_tenant: false,
            retry: None,
            breaker: None,
        },
    );
    let bench_runtime = keel_observability::ObservabilityRuntime::new(
        bench_tcfg,
        keel_observability::ServiceIdentity::new("keel-bench", "benchmark"),
        &[],
    )
    .expect("benchmark telemetry runtime constructs");
    bench_runtime
        .start()
        .expect("benchmark telemetry runtime starts");
    phases.insert(
        "telemetry-emit".into(),
        median_micros(iterations, || {
            let start = std::time::Instant::now();
            bench_runtime.info(
                "bench emit",
                keel_observability::fields!("i" => 1, "kind" => "bench"),
            );
            start.elapsed()
        }),
    );
    bench_runtime.shutdown(std::time::Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&bench_telemetry_dir);

    // memory: delta-state size after a fixed 500-command burst.
    let mem_backend = Arc::new(keel_execution::InMemoryBackend::new());
    let mem_engine = keel_execution::Engine::new(
        Arc::new(keel_policy::PolicyAuthz {
            engine: policies.clone(),
        }),
        mem_backend.clone(),
        Arc::new(keel_execution::InMemoryIdempotency::new()),
    );
    for n in 0..500 {
        let key = keel_execution::IdempotencyKey(format!("mem-{n}"));
        runtime
            .block_on(mem_engine.execute(
                command_ctx.clone(),
                &command,
                serde_json::json!({ "i": n }),
                Some(key),
            ))
            .expect("memory burst succeeds");
    }
    let stone_bytes = serde_json::to_vec(&mem_backend.stones())
        .unwrap_or_default()
        .len() as f64;
    phases.insert("memory".into(), stone_bytes);

    // Report against the baseline.
    println!();
    println!(
        "{:<14} {:>12} {:>12} {:>9}",
        "phase", "baseline", "current", "delta"
    );
    let mut violations: Vec<(String, f64)> = Vec::new();
    for (phase, current) in &phases {
        let unit = if phase == "memory" { "B" } else { "us" };
        match baseline.as_ref().and_then(|b| b.phases.get(phase)) {
            Some(base) if *base > 0.0 => {
                let delta_pct = (current - base) / base * 100.0;
                println!("{phase:<14} {base:>10.1} {unit:<1} {current:>10.1} {unit:<1} {delta_pct:>+7.1}%");
                // Noise floor for timing phases: scheduler jitter on small phases
                // (a few microseconds) must not trip the budget gate.
                let within_noise =
                    phase != "memory" && (current - base).abs() < 50.0_f64.max(base * 0.01);
                if delta_pct > budget && !within_noise {
                    violations.push((phase.clone(), delta_pct));
                }
            }
            _ => println!(
                "{phase:<14} {:>12} {current:>10.1} {unit:<1} {:>8}",
                "-", "-"
            ),
        }
    }
    println!();

    if save_baseline {
        let out = BenchmarkBaseline {
            version: 1,
            captured_at: rfc3339(time::OffsetDateTime::now_utc()),
            rustc: rustc_version(),
            iterations,
            phases,
        };
        std::fs::create_dir_all(root.join("benchmarks"))?;
        let path = baseline_path(&root);
        std::fs::write(&path, serde_json::to_string_pretty(&out)?)?;
        println!("baseline written: {}", path.display());
        return Ok(ExitCode::SUCCESS);
    }
    match &baseline {
        None => println!("no baseline yet — run `keel benchmark --save-baseline` to persist these numbers (G15)."),
        Some(_) if violations.is_empty() => println!("budget ok: all phases within {budget}% of baseline"),
        Some(_) => {
            for (phase, delta_pct) in &violations {
                println!("budget exceeded: {phase} regressed {delta_pct:+.1}% (budget {budget}%)");
            }
            return Ok(ExitCode::FAILURE);
        }
    }
    Ok(ExitCode::SUCCESS)
}
// ----------------------------------------------------------------- upgrade

fn external_deps(root: &Path) -> Option<(usize, usize, Vec<String>)> {
    let raw = std::fs::read_to_string(root.join("Cargo.toml")).ok()?;
    let value: toml::Value = raw.parse().ok()?;
    let table = value.get("workspace")?.get("dependencies")?.as_table()?;
    let total = table.len();
    let mut git_deps: Vec<String> = Vec::new();
    let external = table
        .iter()
        .filter(|(name, v)| {
            let is_path = v.get("path").is_some();
            if v.get("git").is_some() {
                git_deps.push(name.to_string());
            }
            !is_path
        })
        .count();
    Some((external, total, git_deps))
}

fn lock_package_count(root: &Path) -> Option<usize> {
    let raw = std::fs::read_to_string(root.join("Cargo.lock")).ok()?;
    let value: toml::Value = raw.parse().ok()?;
    value.get("package")?.as_array().map(|a| a.len())
}

/// `keel upgrade` (item 48, L73): version / public APIs / deprecations /
/// config / migrations / dependencies / module contracts. Read-only analysis;
/// automated fixes are limited to the suggested commands.
fn upgrade_report() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    println!("Upgrade Report");
    println!("══════════════");

    // Framework version vs release manifest.
    let current = env!("CARGO_PKG_VERSION");
    let manifest = match keel_deployment::read_manifest(&root) {
        Ok(manifest) => manifest,
        Err(e) => {
            println!("[!!] release manifest unreadable ({e}) — rebuild with `keel build`");
            None
        }
    };
    match &manifest {
        Some(m) if m.version == current => {
            println!("[ok] framework version {current} — release manifest matches ({}{})",
                rfc3339(m.built_at),
                if m.git_dirty.unwrap_or(false) { ", dirty build" } else { "" })
        }
        Some(m) => println!("[!!] framework version {current} but release manifest is {} — rebuild with `keel build`", m.version),
        None => println!("[!!] no release manifest — run `keel build` (current version {current})"),
    }

    // Public API registry (blueprint 49): stability + deprecations.
    let entries = keel_contracts::api_registry::load_registry();
    let stable = entries.iter().filter(|e| e.stability == "stable").count();
    let beta = entries
        .iter()
        .filter(|e| e.stability == "beta" || e.stability == "experimental")
        .count();
    let deprecated: Vec<_> = entries
        .iter()
        .filter(|e| e.stability == "deprecated")
        .collect();
    println!(
        "[ok] public API registry: {} entries ({} stable, {} beta/experimental, {} deprecated)",
        entries.len(),
        stable,
        beta,
        deprecated.len()
    );
    for entry in &deprecated {
        println!(
            "     action: {} ({}) deprecated since {} — replacement: {} (removable in {})",
            entry.id,
            entry.api,
            entry.deprecated.as_deref().unwrap_or("?"),
            entry.replacement.as_deref().unwrap_or("none"),
            entry.breaking_version.as_deref().unwrap_or("?")
        );
    }

    // Configuration.
    match load_config(&root) {
        Ok(cfg) => println!(
            "[ok] configuration loads ({}, {:?})",
            config_path(&root).display(),
            cfg.app.env
        ),
        Err(e) => println!("[!!] configuration broken: {e:#}"),
    }

    // Migrations (files; applied state needs the database).
    let migrations: Vec<_> = migrator().iter().collect();
    let latest = migrations.iter().map(|m| m.version).max().unwrap_or(0);
    println!(
        "[ok] migrations: {} files, latest v{latest} — run `keel migrate:status` for applied state",
        migrations.len()
    );

    // Dependency governance (D-62).
    match external_deps(&root) {
        Some((external, total, git_deps)) => {
            if external > 25 {
                println!("[!!] direct external dependencies: {external} (budget 25, D-62)");
            } else {
                println!("[ok] direct external dependencies: {external}/25 (D-62; {total} workspace entries incl. platform crates)");
            }
            for dep in &git_deps {
                println!("[!!] git dependency: {dep} (pin to a released version)");
            }
        }
        None => println!("[!!] could not parse workspace Cargo.toml dependency table"),
    }
    if let Some(packages) = lock_package_count(&root) {
        println!("[ok] transitive packages in Cargo.lock: {packages}");
    }

    // Module contracts + platform graph.
    match discover_manifests(&root) {
        Ok(manifests) => println!("[ok] module contracts: {} manifests", manifests.len()),
        Err(e) => println!("[!!] module manifests: {e:#}"),
    }
    match compile_graph(&root) {
        Ok(_) => println!("[ok] platform graph compiles (no duplicate routes/caps/events)"),
        Err(_) => println!("[!!] platform graph gate fails — run `keel check` for details"),
    }

    println!("no automated fixes applied (read-only analysis); act on [!!] lines with the suggested commands");
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------- security

fn split_kv(line: &str) -> Option<(&str, &str)> {
    let eq = line.find('=')?;
    let key = line[..eq].trim();
    let value = line[eq + 1..].trim();
    if key.is_empty() || value.is_empty() {
        return None;
    }
    Some((key, value))
}

fn git_tracked(path: &Path) -> bool {
    let Some(dir) = path.parent() else {
        return false;
    };
    let Some(name) = path.file_name() else {
        return false;
    };
    std::process::Command::new("git")
        .args(["-C"])
        .arg(dir)
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(name)
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

fn scan_secret_file(path: &Path, failures: &mut Vec<String>, notes: &mut Vec<String>) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    if raw.contains("PRIVATE KEY") {
        failures.push(format!(
            "{}: private key material in the repo (G12)",
            path.display()
        ));
        return;
    }
    let prod_like = path
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n.contains("prod") || n.contains("staging"))
        .unwrap_or(false);
    for (lineno, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = split_kv(trimmed) else {
            continue;
        };
        let lower_key = key.to_lowercase();
        let secret_key = lower_key.contains("secret")
            || lower_key.contains("password")
            || lower_key.contains("app_key")
            || lower_key.contains("api_key")
            || lower_key.contains("token");
        if !secret_key {
            continue;
        }
        let v = value.trim_matches('"').trim();
        if v.is_empty() || v.starts_with("${") || v.starts_with("{{") || v == "null" {
            continue;
        }
        let lower_v = v.to_lowercase();
        if (lower_v.contains("dev-only") || lower_v.contains("dev_only")) && !prod_like {
            notes.push(format!(
                "{}:{}: dev-only literal ({key}) — fine in dev, never in prod/staging",
                path.display(),
                lineno + 1
            ));
            continue;
        }
        if prod_like {
            failures.push(format!(
                "{}:{}: literal secret ({key}) in a prod/staging config file (G12)",
                path.display(),
                lineno + 1
            ));
            continue;
        }
        if git_tracked(path) {
            failures.push(format!(
                "{}:{}: literal secret ({key}) is tracked by git (G12)",
                path.display(),
                lineno + 1
            ));
        } else {
            notes.push(format!(
                "{}:{}: literal secret ({key}) kept local — keep it out of version control",
                path.display(),
                lineno + 1
            ));
        }
    }
}

fn gitignore_covers(root: &Path, pattern: &str) -> bool {
    let Ok(raw) = std::fs::read_to_string(root.join(".gitignore")) else {
        return false;
    };
    raw.lines().any(|l| {
        let t = l.trim();
        t == pattern || t.trim_start_matches('/') == pattern || t == format!("{pattern}/")
    })
}

/// `keel security` (item 51, L74): secrets hygiene (G12), dependency policy
/// (D-62), trace locality (captured inputs never leave the environment).
fn security_check() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let mut failures: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    println!(
        "keel security — secrets hygiene (G12) · dependency policy (D-62) · trace locality (L74)"
    );

    // 1. Secrets hygiene: scan config files + .env for literal key material.
    let mut files: Vec<PathBuf> = Vec::new();
    let config_dir = root.join("config");
    if config_dir.is_dir() {
        for entry in std::fs::read_dir(&config_dir)
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".toml") || name == ".env" || name.ends_with(".env") {
                files.push(entry.path());
            }
        }
    }
    if let Ok(entries) = std::fs::read_dir(&root) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name == ".env" || name.ends_with(".env") {
                files.push(entry.path());
            }
        }
    }
    let scanned = files.len();
    for file in &files {
        scan_secret_file(file, &mut failures, &mut notes);
    }
    println!("[ok] scanned {scanned} config/env files for key material");

    // 2. gitignore coverage for secret-bearing paths.
    if gitignore_covers(&root, ".env") {
        println!("[ok] .gitignore covers .env files");
    } else {
        failures.push(".gitignore does not cover .env (G12)".into());
    }

    // 3. Trace locality: captured inputs live under storage/framework/traces.
    let traces_dir = root.join("storage/framework/traces");
    if traces_dir.is_dir() {
        let count = std::fs::read_dir(&traces_dir)
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
                    .count()
            })
            .unwrap_or(0);
        if gitignore_covers(&root, "storage") {
            println!("[ok] trace store: {count} traces, storage/ git-ignored (captured inputs stay local)");
        } else {
            failures.push(format!("storage/ is not git-ignored — {count} traces contain captured inputs and must never be committed"));
        }
    } else {
        println!("[ok] no execution traces recorded yet");
    }

    // 4. Dependency governance (D-62, blueprint 51).
    match external_deps(&root) {
        Some((external, _total, git_deps)) => {
            if external > 25 {
                failures.push(format!(
                    "direct external dependencies {external} exceed the 25 budget (D-62)"
                ));
            } else {
                println!("[ok] direct external dependencies: {external}/25 (D-62)");
            }
            for dep in &git_deps {
                failures.push(format!(
                    "git dependency {dep}: pin to a released version before shipping"
                ));
            }
        }
        None => notes.push("could not parse workspace dependency table".into()),
    }
    if let Some(packages) = lock_package_count(&root) {
        println!("[ok] transitive packages in Cargo.lock: {packages}");
    }

    // 5. Production posture (if a prod/staging config exists).
    for env_file in ["prod.toml", "staging.toml"] {
        let path = config_dir.join(env_file);
        if !path.exists() {
            continue;
        }
        match keel_config::PlatformConfig::load(Some(&path)) {
            Ok(cfg) => {
                if let Some(db) = &cfg.database {
                    if db.url.contains("sslmode=disable") {
                        failures.push(format!("{env_file}: database url disables TLS"));
                    }
                }
            }
            Err(e) => failures.push(format!("{env_file}: rejected at load: {e}")),
        }
    }

    for note in &notes {
        println!("note: {note}");
    }
    if failures.is_empty() {
        println!("security: clean ({} notes)", notes.len());
        Ok(ExitCode::SUCCESS)
    } else {
        for failure in &failures {
            println!("[!!] {failure}");
        }
        println!("security: {} findings", failures.len());
        Ok(ExitCode::FAILURE)
    }
}
// ------------------------------------------------------------------ pending

fn gate_pending(cmd: &Cmd) -> anyhow::Result<ExitCode> {
    println!("gate pending: keel {} lands with its milestone — see docs/rust/11-FOUNDATION-MASTER-SPEC.md (§13 gates).", cmd_name(cmd));
    Ok(ExitCode::SUCCESS)
}

fn cmd_name(cmd: &Cmd) -> String {
    match cmd {
        Cmd::Dev => "dev",
        Cmd::Serve { .. } => "serve",
        Cmd::Test => "test",
        _ => "?",
    }
    .to_string()
}

// ---------------------------------------------------------------- profile

/// B-27: `keel profile <api|worker>` — an out-of-process samply wrapper.
/// The presence probe asks `samply --version`; everything else lives in
/// `run_profile` so the missing-samply path is deterministically testable.
fn profile(target: ProfileTarget) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let available = std::process::Command::new("samply")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false);
    run_profile(&root, &target, available)
}

/// Testable core of `keel profile`. When samply is missing we print the
/// install line + the documented manual workflow and fail closed — never a
/// silent no-op. When present: build the target package, then hand the
/// fresh binary to `samply record` (out-of-process, zero new dependencies).
fn run_profile(
    root: &Path,
    target: &ProfileTarget,
    samply_available: bool,
) -> anyhow::Result<ExitCode> {
    if !samply_available {
        eprint!("{}", profile_guidance(target));
        return Ok(ExitCode::FAILURE);
    }
    let package = target.name();
    println!("keel profile: building {package}");
    let build = std::process::Command::new("cargo")
        .arg("build")
        .arg("-p")
        .arg(package)
        .current_dir(root)
        .status()
        .map_err(|e| anyhow::anyhow!("cannot run cargo build: {e}"))?;
    if !build.success() {
        anyhow::bail!("cargo build -p {package} failed");
    }
    let binary = root.join("target").join("debug").join(package);
    let (program, args) = profile_invocation(&binary);
    println!(
        "keel profile: {} {} (stop the recording to open the flamegraph)",
        program,
        args.join(" ")
    );
    let status = std::process::Command::new(&program)
        .args(&args)
        .current_dir(root)
        .status()
        .map_err(|e| anyhow::anyhow!("cannot spawn {program}: {e}"))?;
    Ok(ExitCode::from(status.code().unwrap_or(1).clamp(0, 255) as u8))
}

/// Pure samply command line: `samply record <path-to-built-binary>`.
fn profile_invocation(binary: &Path) -> (String, Vec<String>) {
    (
        "samply".to_string(),
        vec!["record".to_string(), binary.display().to_string()],
    )
}

/// The missing-samply guidance (stderr), pointed at the documented manual
/// flamegraph workflow in docs/13-OPERATIONS.md.
fn profile_guidance(target: &ProfileTarget) -> String {
    format!(
        "keel profile: samply not found on PATH (target: {})\n  install: cargo install samply\n  manual workflow: docs/13-OPERATIONS.md (flamegraph profiling)\n",
        target.name()
    )
}

// ------------------------------------------------------------------ serve

/// keel serve (S7 B-08): one-command local loop. Spawns `cargo run -p
/// keel-api` (and optionally keel-worker) with the workspace config and
/// streams their output. Ctrl+C reaches the children through the console;
/// if the API exits first we tear the worker down with it.
fn serve(worker: bool) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let config = config_path(&root);
    println!(
        "keel serve: keel-api{} (config: {})",
        if worker { " + keel-worker" } else { "" },
        config.display()
    );
    let mut api = std::process::Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("-p")
        .arg("keel-api")
        .current_dir(&root)
        .env("KEEL_CONFIG", &config)
        .spawn()
        .map_err(|e| anyhow::anyhow!("cannot spawn keel-api: {e}"))?;
    let mut worker_child = if worker {
        Some(
            std::process::Command::new("cargo")
                .arg("run")
                .arg("--quiet")
                .arg("-p")
                .arg("keel-worker")
                .current_dir(&root)
                .env("KEEL_CONFIG", &config)
                .spawn()
                .map_err(|e| anyhow::anyhow!("cannot spawn keel-worker: {e}"))?,
        )
    } else {
        None
    };
    let status = api.wait()?;
    if let Some(mut child) = worker_child.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let code = status.code().unwrap_or(1);
    if code == 0 {
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!("keel-api exited with status {code}");
        Ok(ExitCode::from(code.clamp(0, 255) as u8))
    }
}

// ------------------------------------------------------------------ shell

/// B-29: interactive platform console. keel-console is a Tools-class crate and
/// the dep-rules matrix keeps tools from linking runtime crates, so the REPL
/// itself runs inside keel-api behind `--shell`; this spawns it exactly like
/// `serve` (same config, stdio inherited so the prompt is interactive).
fn shell_console() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let config = config_path(&root);
    println!(
        "keel shell: booting the engine (config: {})",
        config.display()
    );
    let status = std::process::Command::new("cargo")
        .arg("run")
        .arg("--quiet")
        .arg("-p")
        .arg("keel-api")
        .arg("--")
        .arg("--shell")
        .current_dir(&root)
        .env("KEEL_CONFIG", &config)
        .status()
        .map_err(|e| anyhow::anyhow!("cannot spawn keel-api --shell: {e}"))?;
    let code = status.code().unwrap_or(1);
    if code == 0 {
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!("keel shell exited with status {code}");
        Ok(ExitCode::from(code.clamp(0, 255) as u8))
    }
}

// ------------------------------------------------------------------ prune

/// Wave 3 Phase 6: `keel model:prune`. keel-console is Tools-class and
/// dep-rules keeps it from linking runtime crates, so the sweep runs inside
/// keel-api behind `--prune` (same spawn pattern as `keel shell`): boots the
/// graph, purges `chunk` rows per store pass across every registered
/// prunable model, prints per-model counts and exits — no server, no REPL.
fn model_prune(chunk: u32, tenant: Option<String>) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let config = config_path(&root);
    println!(
        "keel model:prune: booting (chunk: {chunk}, config: {})",
        config.display()
    );
    let mut command = std::process::Command::new("cargo");
    command
        .arg("run")
        .arg("--quiet")
        .arg("-p")
        .arg("keel-api")
        .arg("--")
        .arg("--prune")
        .arg("--chunk")
        .arg(chunk.to_string());
    if let Some(tenant) = tenant.as_deref() {
        command.arg("--tenant").arg(tenant);
    }
    let status = command
        .current_dir(&root)
        .env("KEEL_CONFIG", &config)
        .status()
        .map_err(|e| anyhow::anyhow!("cannot spawn keel-api --prune: {e}"))?;
    let code = status.code().unwrap_or(1);
    if code == 0 {
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!("keel model:prune exited with status {code}");
        Ok(ExitCode::from(code.clamp(0, 255) as u8))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wave 3 Phase 6: `keel model:prune` parses with the default chunk
    /// cap and honors `--chunk`/`--tenant`.
    #[test]
    fn model_prune_cli_parses() {
        let cli = Cli::try_parse_from(["keel", "model:prune"]).expect("parses");
        match cli.cmd {
            Cmd::ModelPrune { chunk, tenant } => {
                assert_eq!(chunk, 200);
                assert!(tenant.is_none());
            }
            _ => panic!("expected model:prune"),
        }
        let cli = Cli::try_parse_from([
            "keel",
            "model:prune",
            "--chunk",
            "50",
            "--tenant",
            "01900000-0000-7000-8000-000000000001",
        ])
        .expect("parses");
        match cli.cmd {
            Cmd::ModelPrune { chunk, tenant } => {
                assert_eq!(chunk, 50);
                assert_eq!(
                    tenant.as_deref(),
                    Some("01900000-0000-7000-8000-000000000001")
                );
            }
            _ => panic!("expected model:prune"),
        }
    }

    /// Starter-template contract (S9 B-15/B-20 smoke): the template renders
    /// fully substituted, its manifest parses, the graph compiles, and named
    /// routes resolve URLs. The generated module mirrors appointments'
    /// ModelStore wiring, which the workspace suite keeps compile-proven.
    #[test]
    fn starter_template_renders_parses_and_compiles() {
        let root = workspace_root().expect("workspace root resolves");
        let template = root.join("templates").join("starter-template");
        let files = [
            "Cargo.toml",
            "module.toml",
            "README.md",
            "src/lib.rs",
            "web/index.html",
            "web/app.js",
        ];
        let name = "starterprobe";
        let crate_name = "keel-starterprobe";
        let description = "Probe starter";
        let mut lib = String::new();
        let mut manifest_raw = String::new();
        for rel in files {
            let raw = std::fs::read_to_string(template.join(rel))
                .unwrap_or_else(|e| panic!("starter file {rel} readable: {e}"));
            let rendered = raw
                .replace("__PROJECT_NAME__", name)
                .replace("__PROJECT_CRATE__", crate_name)
                .replace("__PROJECT_DESCRIPTION__", description);
            assert!(
                !rendered.contains("__PROJECT_NAME__"),
                "unsubstituted name in {rel}"
            );
            assert!(
                !rendered.contains("__PROJECT_CRATE__"),
                "unsubstituted crate in {rel}"
            );
            match rel {
                "src/lib.rs" => lib = rendered,
                "module.toml" => manifest_raw = rendered,
                _ => {}
            }
        }

        let manifest =
            keel_registry::ModuleManifest::from_toml(&manifest_raw).expect("manifest parses");
        let graph = keel_registry::PlatformGraph::compile(vec![manifest]).expect("graph compiles");
        assert_eq!(graph.operations.len(), 2, "create + list");
        assert_eq!(graph.modules[0].routes.len(), 2);

        let list_url = graph
            .resolve_url(name, "notes.list", &std::collections::BTreeMap::new())
            .expect("named list route resolves");
        assert_eq!(list_url, format!("/api/v1/{name}/notes"));
        let create = graph
            .route_named(name, "notes.create")
            .expect("named create route exists");
        assert_eq!(create.method, "POST");

        // Compile-contract markers: the module uses the S8 persistence port.
        assert!(
            lib.contains("ModelStoreRef"),
            "module consumes the store port"
        );
        assert!(lib.contains("pub struct ModuleProvider"));
        assert!(
            lib.contains("InMemoryModelStore"),
            "TestKit/degraded backend"
        );
    }

    #[test]
    fn classic_project_template_still_substitutes() {
        let root = workspace_root().expect("workspace root resolves");
        let template = root.join("templates").join("project-template");
        for rel in ["Cargo.toml", "module.toml", "README.md", "src/lib.rs"] {
            let raw = std::fs::read_to_string(template.join(rel)).expect("template file");
            let rendered = raw
                .replace("__PROJECT_NAME__", "probe")
                .replace("__PROJECT_CRATE__", "keel-probe")
                .replace("__PROJECT_DESCRIPTION__", "Probe");
            assert!(
                !rendered.contains("__PROJECT_"),
                "unsubstituted marker in {rel}"
            );
        }
    }

    /// Wave 1 route-cache cycle: route:cache writes the pinned artifact
    /// (resolved chains + manifest hash), route:clear removes it. The boot
    /// side (hash-mismatch-refuse) is proven by the keel-api
    /// route_cache_pin test; this covers the CLI half of the cycle.
    #[test]
    fn route_cache_cycle_writes_and_clears_the_artifact() {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "keel-console-routecache-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let app_dir = dir.join("applications").join("alpha");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("module.toml"),
            r#"
[module]
name = "alpha"
version = "0.1.0"

[capabilities]
"index" = { profile = "READ" }

[[routes]]
method = "GET"
path = "/x"
operation = "index"
"#,
        )
        .unwrap();

        // route:cache -> artifact present with one resolved plan.
        assert_eq!(route_cache_at(&dir).expect("route cache runs"), ExitCode::SUCCESS);
        let path = keel_registry::route_cache_path(&dir);
        assert!(path.exists(), "artifact written");
        let artifact = keel_registry::RouteCacheArtifact::load(&path)
            .expect("artifact loads")
            .expect("artifact present");
        assert_eq!(artifact.module_abi, keel_registry::MODULE_ABI);
        assert_eq!(artifact.plans.len(), 1);
        assert!(artifact.matches(&dir).expect("hash check runs"));

        // Unknown middleware entries fail the cache (GRAPH-MW-UNKNOWN,
        // fail-closed) instead of writing a broken artifact.
        std::fs::write(
            app_dir.join("module.toml"),
            r#"
[module]
name = "alpha"
version = "0.1.0"

[capabilities]
"index" = { profile = "READ" }

[[routes]]
method = "GET"
path = "/x"
operation = "index"
middleware = ["ghost"]
"#,
        )
        .unwrap();
        let err = route_cache_at(&dir).expect_err("unknown middleware refuses the cache");
        assert!(err.to_string().contains("GRAPH-MW-UNKNOWN"), "{err}");

        // route:clear -> artifact gone (boot falls back to live resolution).
        assert_eq!(route_clear_at(&dir).expect("route clear runs"), ExitCode::SUCCESS);
        assert!(!path.exists(), "artifact removed");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Wave 5 Phase 1: optimize warms route + config + view artifacts in
    /// order and aborts on the first failure (a broken graph never reaches
    /// the config step); optimize:clear drops all three so boot falls back
    /// to live compilation everywhere.
    #[test]
    fn optimize_cycle_warms_and_clears_all_three_artifacts() {
        let dir = phase5_root("optimize");
        let app_dir = dir.join("applications").join("alpha");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("module.toml"),
            r#"
[module]
name = "alpha"
version = "0.1.0"

[capabilities]
"index" = { profile = "READ" }

[[routes]]
method = "GET"
path = "/x"
operation = "index"
"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.join("resources").join("views")).unwrap();
        std::fs::write(
            dir.join("resources").join("views").join("hello.html"),
            "<p>hello</p>",
        )
        .unwrap();

        // optimize -> all three artifacts warm.
        assert_eq!(optimize_at(&dir).expect("optimize runs"), ExitCode::SUCCESS);
        assert!(
            keel_registry::route_cache_path(&dir).exists(),
            "route artifact warm"
        );
        assert!(
            keel_registry::config_cache_path(&dir).exists(),
            "config artifact warm"
        );
        let view_dir = dir.join("storage").join("framework").join("views");
        let compiled = std::fs::read_dir(&view_dir).map(|d| d.count()).unwrap_or(0);
        assert!(compiled > 0, "view cache warm ({compiled} entries)");

        // Abort-on-first-failure: drop the config artifact, break the graph
        // (GRAPH-MW-UNKNOWN at the route step), and prove optimize stops
        // there — the config artifact stays absent.
        std::fs::remove_file(keel_registry::config_cache_path(&dir)).unwrap();
        std::fs::write(
            app_dir.join("module.toml"),
            r#"
[module]
name = "alpha"
version = "0.1.0"

[capabilities]
"index" = { profile = "READ" }

[[routes]]
method = "GET"
path = "/x"
operation = "index"
middleware = ["ghost"]
"#,
        )
        .unwrap();
        let err = optimize_at(&dir).expect_err("broken graph aborts optimize");
        assert!(err.to_string().contains("route:cache"), "{err}");
        assert!(
            !keel_registry::config_cache_path(&dir).exists(),
            "abort never reached the config step"
        );

        // optimize:clear -> every remaining artifact gone (clears do not
        // recompile the graph, so they succeed even on a broken manifest).
        assert_eq!(
            optimize_clear_at(&dir).expect("optimize:clear runs"),
            ExitCode::SUCCESS
        );
        assert!(!keel_registry::route_cache_path(&dir).exists(), "route cold");
        assert!(!keel_registry::config_cache_path(&dir).exists(), "config cold");
        let remaining = std::fs::read_dir(&view_dir)
            .map(|d| d.count())
            .unwrap_or(0);
        assert_eq!(remaining, 0, "view cache cold");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Wave 5 Phase 2: the migration-pair commands parse with their
    /// documented flags — `--step` defaults to 1 (the last batch only).
    #[test]
    fn migrate_pair_commands_parse_with_documented_defaults() {
        let cli = Cli::try_parse_from(["keel", "migrate:rollback"]).expect("rollback parses");
        match cli.cmd {
            Cmd::MigrateRollback { step } => assert_eq!(step, 1, "default step is 1"),
            _ => panic!("migrate:rollback maps to MigrateRollback"),
        }
        let cli = Cli::try_parse_from(["keel", "migrate:rollback", "--step", "3"])
            .expect("explicit step parses");
        match cli.cmd {
            Cmd::MigrateRollback { step } => assert_eq!(step, 3),
            _ => panic!("--step maps to MigrateRollback"),
        }
        let cli = Cli::try_parse_from(["keel", "migrate:refresh", "--step", "2"])
            .expect("refresh parses");
        match cli.cmd {
            Cmd::MigrateRefresh { step } => assert_eq!(step, 2),
            _ => panic!("migrate:refresh maps to MigrateRefresh"),
        }
        assert!(
            matches!(
                Cli::try_parse_from(["keel", "migrate:reset"])
                    .expect("reset parses")
                    .cmd,
                Cmd::MigrateReset
            ),
            "migrate:reset maps to MigrateReset"
        );
        assert!(
            matches!(
                Cli::try_parse_from(["keel", "migrate:install"])
                    .expect("install parses")
                    .cmd,
                Cmd::MigrateInstall
            ),
            "migrate:install maps to MigrateInstall"
        );
    }

    /// Wave 5 Phase 3: the advisory-lock key matches the program spec
    /// (docs/19: `pg_advisory_lock(7259)`) and the fail-closed deadline is
    /// actually bounded.
    #[test]
    fn migration_lock_key_matches_the_program_spec() {
        assert_eq!(keel_postgres::MIGRATION_LOCK_KEY, 7259);
        assert!(
            !keel_postgres::MIGRATION_LOCK_TIMEOUT.is_zero(),
            "acquisition deadline is bounded (fail-closed, never a hang)"
        );
    }

    /// Wave 5 Phase 4: the db commands parse under their documented names.
    #[test]
    fn db_commands_parse_with_documented_names() {
        assert!(
            matches!(
                Cli::try_parse_from(["keel", "db:show"]).expect("db:show parses").cmd,
                Cmd::DbShow
            ),
            "db:show maps to DbShow"
        );
        assert!(
            matches!(
                Cli::try_parse_from(["keel", "db:schema:dump"])
                    .expect("db:schema:dump parses")
                    .cmd,
                Cmd::DbSchemaDump
            ),
            "db:schema:dump maps to DbSchemaDump"
        );
    }

    /// Wave 5 Phase 4: db:show renders driver, server, database and the
    /// table inventory with row counts (pure render, no database needed).
    #[test]
    fn db_show_renders_server_metadata_and_inventory() {
        let summary = DbSummary {
            driver: "postgres".into(),
            server: "PostgreSQL 16.4 on x86_64-pc-windows-msvc".into(),
            database: "keel".into(),
            tables: vec![("appointments".into(), 42), ("patients".into(), 7)],
        };
        let out = render_db_show(&summary);
        assert!(out.contains("driver:   postgres"), "{out}");
        assert!(out.contains("PostgreSQL 16.4"), "{out}");
        assert!(out.contains("database: keel"), "{out}");
        assert!(out.contains("tables:   2"), "{out}");
        assert!(out.contains("appointments"), "{out}");
        assert!(out.contains("42"), "row count rendered: {out}");
        assert!(out.contains("patients"), "{out}");
    }

    /// Wave 5 Phase 4: the schema snapshot is normalized — tables sorted by
    /// name, PK inline (including composite keys), NOT NULL/DEFAULT in a
    /// fixed column order, secondary indexes terminated — and carries the
    /// never-executed banner.
    #[test]
    fn schema_dump_renders_normalized_sorted_ddl() {
        let tables = vec![
            SchemaTable {
                name: "zebras".into(),
                columns: vec![
                    SchemaColumn {
                        name: "id".into(),
                        data_type: "uuid".into(),
                        not_null: true,
                        default_expr: None,
                    },
                    SchemaColumn {
                        name: "label".into(),
                        data_type: "text".into(),
                        not_null: false,
                        default_expr: Some("'anon'::text".into()),
                    },
                ],
                primary_key: vec!["id".into()],
                indexes: vec![
                    "CREATE UNIQUE INDEX zebras_label_idx ON public.zebras USING btree (label)"
                        .into(),
                ],
            },
            SchemaTable {
                name: "apples".into(),
                columns: vec![
                    SchemaColumn {
                        name: "id".into(),
                        data_type: "integer".into(),
                        not_null: true,
                        default_expr: None,
                    },
                    SchemaColumn {
                        name: "sku".into(),
                        data_type: "text".into(),
                        not_null: true,
                        default_expr: None,
                    },
                ],
                primary_key: vec!["id".into(), "sku".into()],
                indexes: vec![],
            },
        ];
        let out = render_schema_snapshot(&tables);
        // Banner: generated artifact, never executed by migrate.
        assert!(
            out.contains("keel normalized schema snapshot")
                && out.contains("never executes this file"),
            "{out}"
        );
        // Tables sorted by name regardless of input order.
        let apples = out.find("CREATE TABLE \"apples\"").expect("apples present");
        let zebras = out.find("CREATE TABLE \"zebras\"").expect("zebras present");
        assert!(apples < zebras, "sorted output:\n{out}");
        // Column clauses: NOT NULL fixed before DEFAULT; nullable stays bare.
        assert!(out.contains("\"id\" uuid NOT NULL"), "{out}");
        assert!(out.contains("\"label\" text DEFAULT 'anon'::text"), "{out}");
        // Composite PK rendered inline at the table level.
        assert!(out.contains("PRIMARY KEY (\"id\", \"sku\")"), "{out}");
        // Secondary index emitted as a terminated statement after its table.
        assert!(
            out.contains(
                "CREATE UNIQUE INDEX zebras_label_idx ON public.zebras USING btree (label);"
            ),
            "{out}"
        );
    }

    /// Wave 5 Phase 4: an empty schema still produces a valid, honest
    /// snapshot (banner + marker) instead of a blank file.
    #[test]
    fn schema_dump_renders_an_empty_marker_for_an_empty_schema() {
        let out = render_schema_snapshot(&[]);
        assert!(out.contains("(no tables in schema public)"), "{out}");
        assert!(out.contains("never executes this file"), "{out}");
    }

    /// Wave 5 Phase 5: queue:monitor renders every depth counter plus the
    /// oldest pending age in compact form (pure render, no store needed).
    #[test]
    fn queue_monitor_renders_depth_and_oldest_age() {
        let now = time::OffsetDateTime::now_utc();
        let snap = keel_jobs::QueueSnapshot {
            pending: 12,
            running: 2,
            dead: 1,
            succeeded: 55,
            oldest_pending: Some(now - time::Duration::seconds(200)),
        };
        let out = render_queue_monitor(&snap, now);
        assert!(out.contains("queue depth:"), "{out}");
        assert!(out.contains("pending:   12"), "{out}");
        assert!(out.contains("(oldest 3m20s)"), "age: {out}");
        assert!(out.contains("running:   2"), "{out}");
        assert!(out.contains("dead:      1"), "{out}");
        assert!(out.contains("succeeded: 55"), "{out}");

        // No pending jobs -> no age clause; ages compose h/m/s compactly.
        let empty = keel_jobs::QueueSnapshot {
            oldest_pending: None,
            ..snap
        };
        let out = render_queue_monitor(&empty, now);
        assert!(!out.contains("oldest"), "{out}");
        assert_eq!(format_age(time::Duration::seconds(45)), "45s");
        assert_eq!(format_age(time::Duration::minutes(3) + time::Duration::seconds(20)), "3m20s");
        assert_eq!(
            format_age(time::Duration::hours(2) + time::Duration::minutes(5)),
            "2h5m"
        );
    }

    /// Wave 5 Phase 5: the queue admin commands parse under their documented
    /// names with the documented cutoff defaults (72h batches / 168h failed).
    #[test]
    fn queue_admin_commands_parse_with_documented_defaults() {
        let cli = Cli::try_parse_from(["keel", "queue:prune-failed"]).expect("parses");
        match cli.cmd {
            Cmd::QueuePruneFailed { hours } => assert_eq!(hours, 168, "default 168h"),
            _ => panic!("queue:prune-failed maps to QueuePruneFailed"),
        }
        let cli =
            Cli::try_parse_from(["keel", "queue:prune-failed", "--hours", "24"]).expect("parses");
        match cli.cmd {
            Cmd::QueuePruneFailed { hours } => assert_eq!(hours, 24),
            _ => panic!("--hours maps to QueuePruneFailed"),
        }
        let cli = Cli::try_parse_from(["keel", "queue:prune-batches"]).expect("parses");
        match cli.cmd {
            Cmd::QueuePruneBatches { hours } => assert_eq!(hours, 72, "default 72h"),
            _ => panic!("queue:prune-batches maps to QueuePruneBatches"),
        }
        let cli = Cli::try_parse_from(["keel", "queue:forget", "abc"]).expect("parses");
        match cli.cmd {
            Cmd::QueueForget { id } => assert_eq!(id, "abc"),
            _ => panic!("queue:forget maps to QueueForget"),
        }
        let cli = Cli::try_parse_from(["keel", "queue:retry-batch", "b-1"]).expect("parses");
        match cli.cmd {
            Cmd::QueueRetryBatch { id } => assert_eq!(id, "b-1"),
            _ => panic!("queue:retry-batch maps to QueueRetryBatch"),
        }
        // The flag-less commands parse under their documented names.
        let cli = Cli::try_parse_from(["keel", "queue:clear"]).expect("parses");
        assert!(matches!(cli.cmd, Cmd::QueueClear), "queue:clear");
        let cli = Cli::try_parse_from(["keel", "queue:flush"]).expect("parses");
        assert!(matches!(cli.cmd, Cmd::QueueFlush), "queue:flush");
        let cli = Cli::try_parse_from(["keel", "queue:monitor"]).expect("parses");
        assert!(matches!(cli.cmd, Cmd::QueueMonitor), "queue:monitor");
        let cli = Cli::try_parse_from(["keel", "queue:restart"]).expect("parses");
        assert!(matches!(cli.cmd, Cmd::QueueRestart), "queue:restart");
        let cli = Cli::try_parse_from(["keel", "queue:listen"]).expect("parses");
        assert!(matches!(cli.cmd, Cmd::QueueListen), "queue:listen");
    }

    /// Wave 4 Phase 5: temp workspace with a schema-clean config (secrets,
    /// database URL password, views roots) for the config/env/cache tests.
    fn phase5_root(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "keel-console-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::write(
            dir.join("config").join("dev.toml"),
            r#"[app]
name = "keel"
env = "dev"

[auth]
jwt_secret = "unit-test-secret-value"

[database]
url = "postgres://user:db-password@localhost:5432/keel"

[views]
paths = ["resources/views"]
cache_dir = "storage/framework/views"
"#,
        )
        .unwrap();
        dir
    }

    /// Wave 4 Phase 5: config:cache pins the frozen effective config; boot
    /// hash-check accepts the pin and refuses drift (identical semantics to
    /// route:cache); config:clear drops the artifact.
    #[test]
    fn config_cache_cycle_pins_and_clears_the_artifact() {
        let dir = phase5_root("configcache");
        assert_eq!(
            config_cache_at(&dir).expect("config cache runs"),
            ExitCode::SUCCESS
        );
        let path = keel_registry::config_cache_path(&dir);
        assert!(path.exists(), "artifact written");
        let artifact = keel_registry::ConfigCacheArtifact::load(&path)
            .expect("artifact loads")
            .expect("artifact present");
        assert_eq!(artifact.abi, keel_registry::CONFIG_CACHE_ABI);
        assert_eq!(artifact.config["app"]["env"], "dev");
        assert_eq!(
            artifact.config_hash,
            keel_registry::config_hash(&artifact.config),
            "stored hash matches the stored config"
        );

        // Boot gate: matching pin accepted, drift refused with a pointer.
        keel_registry::config_cache_check(&dir, &artifact.config_hash)
            .expect("pin accepted");
        let stale = keel_registry::config_hash(&serde_json::json!({"app": {}}));
        let err = keel_registry::config_cache_check(&dir, &stale)
            .expect_err("stale pin refuses boot");
        assert!(
            err.contains(keel_registry::CONFIG_CACHE_CODE) && err.contains("config:cache"),
            "{err}"
        );

        // Config file edit -> re-derived hash differs -> the old pin goes
        // stale (the live drift path, end to end).
        let cfg_path = dir.join("config").join("dev.toml");
        let raw = std::fs::read_to_string(&cfg_path).unwrap();
        std::fs::write(&cfg_path, raw.replace(r#"name = "keel""#, r#"name = "renamed""#))
            .unwrap();
        let fresh = load_config(&dir)
            .expect("edited config loads")
            .freeze()
            .cache_json();
        let fresh_hash = keel_registry::config_hash(&fresh);
        assert_ne!(fresh_hash, artifact.config_hash, "edit changes the hash");
        keel_registry::config_cache_check(&dir, &fresh_hash)
            .expect_err("the old pin refuses the edited config");

        // config:clear -> artifact gone (boot reads the live config again).
        assert_eq!(
            config_clear_at(&dir).expect("config clear runs"),
            ExitCode::SUCCESS
        );
        assert!(!path.exists(), "artifact removed");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Wave 4 Phase 5: config:show prints exactly one section with every
    /// secret redacted (TOML by default, `--json` for JSON) and fails
    /// closed on unknown or unconfigured sections.
    #[test]
    fn config_show_renders_one_redacted_section() {
        let dir = phase5_root("configshow");

        let toml_view = config_show_text(&dir, "auth", false).expect("auth renders");
        assert!(toml_view.contains("[REDACTED]"), "{toml_view}");
        assert!(
            !toml_view.contains("unit-test-secret-value"),
            "secret never rendered"
        );

        let json_view = config_show_text(&dir, "auth", true).expect("json renders");
        let parsed: serde_json::Value =
            serde_json::from_str(&json_view).expect("valid json section");
        assert_eq!(parsed["jwt_secret"], "[REDACTED]");

        let db = config_show_text(&dir, "database", true).expect("database renders");
        assert!(db.contains("[REDACTED]") && !db.contains("db-password"), "{db}");

        let err = config_show_text(&dir, "nope", false).expect_err("unknown section");
        assert!(err.to_string().contains("known sections"), "{err}");

        // An unconfigured section (null in the redacted view) is reported
        // as such, not as an unknown key.
        let dir2 = phase5_root("configshow-empty");
        let raw = std::fs::read_to_string(dir2.join("config").join("dev.toml")).unwrap();
        std::fs::write(
            dir2.join("config").join("dev.toml"),
            raw.split("[database]").next().unwrap(),
        )
        .unwrap();
        let err = config_show_text(&dir2, "database", false).expect_err("not configured");
        assert!(err.to_string().contains("not configured"), "{err}");

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&dir2).ok();
    }

    /// Wave 4 Phase 5: `keel env` summarizes the environment with secrets
    /// and the database URL masked, and reflects the config-cache state.
    #[test]
    fn env_summary_reports_environment_secrets_and_cache_state() {
        let dir = phase5_root("env");
        let text = env_summary_text(&dir).expect("summary renders");
        assert!(text.contains("environment: dev"), "{text}");
        assert!(text.contains("config file:"), "{text}");
        assert!(
            text.contains("secrets: app.key=empty auth.jwt_secret=set"),
            "{text}"
        );
        assert!(text.contains("config cache: not cached"), "{text}");
        assert!(!text.contains("db-password"), "database URL masked: {text}");

        config_cache_at(&dir).expect("config cache runs");
        let text = env_summary_text(&dir).expect("summary renders");
        assert!(text.contains("config cache: present"), "{text}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Wave 4 Phase 5: cache:forget removes exactly one key; a missing key
    /// or an unknown namespace fails closed with the known-key list.
    #[test]
    fn cache_forget_removes_exactly_one_key() {
        let dir = phase5_root("cacheforget");
        config_cache_at(&dir).expect("config cache runs");
        let routes = keel_registry::route_cache_path(&dir);
        std::fs::create_dir_all(routes.parent().unwrap()).unwrap();
        std::fs::write(&routes, "{}").unwrap();

        assert_eq!(
            cache_forget_at(&dir, "routes").expect("forget runs"),
            ExitCode::SUCCESS
        );
        assert!(!routes.exists(), "routes key gone");
        assert!(
            keel_registry::config_cache_path(&dir).exists(),
            "the config key survives"
        );

        let err = cache_forget_at(&dir, "routes").expect_err("missing key fails");
        assert!(err.to_string().contains("not found"), "{err}");
        let err = cache_forget_at(&dir, "ghost").expect_err("unknown key fails");
        assert!(err.to_string().contains("views:<template>"), "{err}");

        cache_forget_at(&dir, "config").expect("forget config");
        assert!(!keel_registry::config_cache_path(&dir).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Wave 4 Phase 5: `cache:forget views:<template>` drops one compiled
    /// view entry — the sibling entry survives.
    #[test]
    fn cache_forget_views_drops_one_compiled_entry() {
        let dir = phase5_root("viewforget");
        let views_dir = dir.join("resources").join("views");
        std::fs::create_dir_all(&views_dir).unwrap();
        std::fs::write(views_dir.join("alpha.html"), "<p>a</p>").unwrap();
        std::fs::write(views_dir.join("beta.html"), "<p>b</p>").unwrap();
        assert_eq!(view_cache_at(&dir).expect("view cache runs"), ExitCode::SUCCESS);
        let cache_dir = dir.join("storage").join("framework").join("views");
        let count = || std::fs::read_dir(&cache_dir).unwrap().count();
        assert_eq!(count(), 2, "both templates compiled");

        assert_eq!(
            cache_forget_at(&dir, "views:alpha").expect("forget runs"),
            ExitCode::SUCCESS
        );
        assert_eq!(count(), 1, "exactly one compiled entry removed");

        let err = cache_forget_at(&dir, "views:alpha").expect_err("gone key fails");
        assert!(err.to_string().contains("not found"), "{err}");
        let err = cache_forget_at(&dir, "views:").expect_err("bare namespace fails");
        assert!(err.to_string().contains("template name"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Wave 2 route:list filters (19-PARITY): `--plane` is a validated
    /// exact match, `--name`/`--path` are case-insensitive substrings, and
    /// every row carries plane/group/version/deprecation — after the group
    /// prefix and plane have been merged into the spec at parse time.
    #[test]
    fn route_list_filters_carry_the_parity_columns() {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "keel-console-routelist-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        let app_dir = dir.join("applications").join("alpha");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("module.toml"),
            r#"
[module]
name = "alpha"
version = "0.1.0"

[capabilities]
"index" = { profile = "READ" }

[[route_groups]]
name = "web-pages"
prefix = "admin"
plane = "web"

[[routes]]
method = "GET"
path = "/notes"
operation = "index"
capability = "index"
name = "notes.index"

[[routes]]
method = "POST"
path = "/notes"
operation = "index"
capability = "index"
name = "notes.create"
group = "web-pages"
deprecated = true

[[routes]]
method = "GET"
path = "/health"
operation = "index"
capability = "index"
"#,
        )
        .unwrap();

        let graph = compile_graph(&dir).expect("fixture graph compiles");

        // No filters: every route, with the parity columns populated.
        let all = route_rows(&graph, None, None, None).expect("rows build");
        assert_eq!(all.len(), 3);
        let web = route_rows(&graph, None, None, Some("web")).expect("web plane filter");
        assert_eq!(web.len(), 1);
        assert_eq!(web[0].path, "/admin/notes", "group prefix merged into the row");
        assert_eq!(web[0].plane, "web", "group plane inherited at parse time");
        assert_eq!(web[0].group, "web-pages");
        assert_eq!(web[0].method, "POST");
        assert!(web[0].deprecated, "deprecation column reflects the manifest");
        let api = route_rows(&graph, None, None, Some("api")).expect("api plane filter");
        assert_eq!(api.len(), 2);
        assert!(api.iter().all(|r| !r.deprecated));
        assert_eq!(api[0].group, "", "ungrouped routes keep an empty group");

        // Name/path filters are case-insensitive substrings.
        let named = route_rows(&graph, Some("NOTES."), None, None).expect("name filter");
        assert_eq!(named.len(), 2);
        let health = route_rows(&graph, None, Some("/HEALTH"), None).expect("path filter");
        assert_eq!(health.len(), 1);
        assert_eq!(health[0].path, "/health");
        let empty = route_rows(&graph, Some("nope"), None, None).expect("no match is empty");
        assert!(empty.is_empty());

        // A typo'd plane refuses instead of silently listing nothing.
        route_rows(&graph, None, None, Some("graphql"))
            .expect_err("unknown plane refuses");

        // Both printers accept the filtered row set end to end.
        assert_eq!(
            route_list_at(&dir, "json", None, None, Some("web")).expect("json listing runs"),
            ExitCode::SUCCESS
        );
        assert_eq!(
            route_list_at(&dir, "table", None, Some("notes"), None).expect("table listing runs"),
            ExitCode::SUCCESS
        );
        route_list_at(&dir, "yaml", None, None, None).expect_err("unknown format refuses");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Wave 4 Phase 8 (docs/19 L222): `keel about` golden fields — every
    /// section the Laravel-style report promises, built from config + the
    /// compiled graph + the workspace dependency table. The database line
    /// reports the driver from the DSN scheme and degrades (the server
    /// version ping is absent here) instead of failing; secrets are never
    /// rendered.
    #[test]
    fn about_report_carries_the_golden_fields() {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "keel-console-about-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::create_dir_all(dir.join("applications").join("alpha")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            r#"[workspace]
members = ["tools/keel-console"]

[workspace.package]
version = "9.9.9"

[workspace.dependencies]
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("config").join("dev.toml"),
            r#"[app]
name = "golden-clinic"
env = "dev"
debug = true

[database]
url = "postgres://user:db-password@localhost:5432/keel"
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("applications").join("alpha").join("module.toml"),
            r#"[module]
name = "alpha"
version = "0.1.0"

[capabilities]
"index" = { profile = "READ" }

[[routes]]
method = "GET"
path = "/api/v1/notes"
operation = "index"
capability = "index"

[[routes]]
method = "POST"
path = "/api/v1/notes"
operation = "index"
capability = "index"
deprecated = true
"#,
        )
        .unwrap();

        let text = about_report(&dir, None).expect("report builds");
        assert!(text.contains("keel about"), "{text}");
        assert!(text.contains("Framework"), "{text}");
        assert!(text.contains(env!("CARGO_PKG_VERSION")), "{text}");
        assert!(text.contains("golden-clinic"), "{text}");
        assert!(
            text.contains("Application version 9.9.9"),
            "workspace.package version: {text}"
        );
        assert!(text.contains("Environment         dev"), "{text}");
        assert!(text.contains("Debug               true"), "{text}");
        // Driver from the DSN scheme; without a ping the server version
        // degrades to a note instead of failing the report.
        assert!(
            text.contains("Database            postgres - server version unreachable"),
            "{text}"
        );
        assert!(text.contains("Cache driver"), "{text}");
        assert!(text.contains("Queue driver"), "{text}");
        assert!(text.contains("Session driver"), "{text}");
        let registered = keel_feature_flags::platform_registry().definitions().len();
        assert!(text.contains(&format!("{registered} registered")), "{text}");
        assert!(
            text.contains("Graph               1 modules, 2 routes, 1 operations"),
            "{text}"
        );
        assert!(
            text.contains("0/25 direct external (within budget)"),
            "{text}"
        );
        assert!(!text.contains("db-password"), "secrets never rendered");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Wave 4 Phase 9 (B-27): `keel profile` parses per target, the samply
    /// command line records the built binary, and a missing samply fails
    /// closed with the install line + the docs/13 workflow pointer (never
    /// a silent no-op).
    #[test]
    fn profile_cli_parses_and_missing_samply_fails_closed() {
        let cli = Cli::try_parse_from(["keel", "profile", "api"]).expect("api parses");
        let Cmd::Profile { target: api } = cli.cmd else {
            panic!("profile api parses");
        };
        assert_eq!(api.name(), "keel-api");
        let cli = Cli::try_parse_from(["keel", "profile", "worker"]).expect("worker parses");
        let Cmd::Profile { target: worker } = cli.cmd else {
            panic!("profile worker parses");
        };
        assert_eq!(worker.name(), "keel-worker");

        // samply command line: `samply record <root>/target/debug/<bin>`.
        let root = std::path::Path::new(".");
        let binary = root.join("target").join("debug").join("keel-api");
        let (program, args) = profile_invocation(&binary);
        assert_eq!(program, "samply");
        assert_eq!(args.len(), 2, "{args:?}");
        assert_eq!(args[0], "record");
        assert!(args[1].ends_with("keel-api"), "{args:?}");
        assert!(args[1].contains("target"), "{args:?}");

        // Missing samply: guidance carries install + docs pointer, and the
        // run fails closed.
        let guidance = profile_guidance(&ProfileTarget::Api);
        assert!(guidance.contains("samply not found"), "{guidance}");
        assert!(guidance.contains("cargo install samply"), "{guidance}");
        assert!(guidance.contains("docs/13-OPERATIONS.md"), "{guidance}");
        assert!(guidance.contains("keel-api"), "{guidance}");
        let dir =
            std::env::temp_dir().join(format!("keel-console-profile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            run_profile(&dir, &ProfileTarget::Worker, false).expect("missing samply runs"),
            ExitCode::FAILURE
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Wave 2 view-cache cycle: view:cache precompiles every template into
    /// the cache dir derived from the same `[views]` section the boot reads
    /// (section defaults apply when the config has no `[views]` table),
    /// re-runs overwrite in place, and view:clear removes the artifacts so
    /// boot recompiles on demand.
    #[test]
    fn view_cache_cycle_compiles_and_clears() {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "keel-console-viewcache-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::write(
            dir.join("config").join("dev.toml"),
            "[app]\nname = \"viewcache\"\nenv = \"test\"\n",
        )
        .unwrap();
        let views = dir.join("resources").join("views");
        std::fs::create_dir_all(views.join("pages")).unwrap();
        std::fs::create_dir_all(views.join("errors")).unwrap();
        std::fs::write(views.join("pages").join("hello.html"), "<p>hello {{ name }}</p>").unwrap();
        std::fs::write(views.join("errors").join("404.html"), "<h1>404</h1>").unwrap();

        assert_eq!(view_cache_at(&dir).expect("view cache runs"), ExitCode::SUCCESS);
        let cache_dir = dir.join("storage").join("framework").join("views");
        let count = |label: &str| {
            std::fs::read_dir(&cache_dir)
                .unwrap_or_else(|e| panic!("{label}: cache dir readable: {e}"))
                .count()
        };
        assert_eq!(count("after cache"), 2, "one compiled artifact per template");

        // Idempotent: drift overwrites the same entry instead of piling up.
        assert_eq!(view_cache_at(&dir).expect("view cache reruns"), ExitCode::SUCCESS);
        assert_eq!(count("after rerun"), 2, "no duplicate artifacts");

        assert_eq!(view_clear_at(&dir).expect("view clear runs"), ExitCode::SUCCESS);
        assert_eq!(count("after clear"), 0, "compiled artifacts removed");
        std::fs::remove_dir_all(&dir).ok();
    }
}
