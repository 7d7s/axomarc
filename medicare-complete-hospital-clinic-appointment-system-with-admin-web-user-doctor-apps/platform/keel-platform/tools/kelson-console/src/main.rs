//! kelson — the platform CLI (11-SPEC §34, item 29). Implemented: doctor,
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
use kelson_config::{Environment, PlatformConfig};
use kelson_execution::{ExecutionTrace, FileTraceStore, TraceStore};
use kelson_mysql::DbPool;

mod observe;
mod promotion;

#[derive(Parser)]
#[command(name = "kelson", version, about = "Kelson platform CLI", long_about = None)]
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
        /// Kind: kelson-codegen GENERATOR_KINDS — migration, seeder, factory,
        /// job, event, listener, command, query, policy, workflow, model,
        /// test, resource, request, controller + Wave 5's cast, channel,
        /// component, exception, mail, notification, observer, rule, scope,
        /// job-middleware, middleware, view.
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
    /// Wave 8 B-24: show the packages.toml index with per-entry manifest +
    /// ABI compatibility (fail-closed exit when any entry is unusable).
    #[command(name = "pkg:list")]
    PkgList,
    /// Wave 8 B-24: validate a package manifest (module.toml) and append it
    /// to packages.toml so graph compile merges it.
    #[command(name = "pkg:add")]
    PkgAdd {
        /// Package directory containing module.toml (root-relative or absolute).
        path: String,
    },
    /// Wave 8 B-25: materialize an indexed package's migrations, config and
    /// assets into the app; views register under the package namespace at
    /// boot (no copy). Idempotent: identical files are skipped, differing
    /// files are reported as conflicts and skipped unless --force.
    #[command(name = "publish")]
    Publish {
        /// Indexed package name (kelson pkg:list).
        package: String,
        /// Overwrite existing files whose content differs.
        #[arg(long)]
        force: bool,
    },
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
        /// Create a SELF-CONTAINED project outside any cargo workspace: it gets
        /// its own `[workspace]` table and concrete dependency specs, so it
        /// builds on a machine that has never seen this checkout (Phase Q).
        /// Dependencies point at a local checkout via `--deps-root`.
        #[arg(long, requires = "with_starter")]
        standalone: bool,
        /// Checkout that a standalone project's kelson dependencies point at.
        /// Defaults to the workspace root above the current directory.
        #[arg(long, requires = "standalone")]
        deps_root: Option<String>,
    },
    Dev,
    /// Run the local app loop: kelson-demo (+ optional worker) in one command (S7 B-08).
    Serve {
        /// Also start kelson-worker alongside kelson-demo.
        #[arg(long)]
        worker: bool,
    },
    /// Interactive platform console (v1.1 B-29): a Tinker-class REPL over the
    /// booted engine that dispatches under the principal you log in as.
    Shell,
    /// Prune aged/trashed rows for every registered prunable model (Wave 3
    /// Phase 6): boots kelson-demo in `--prune` maintenance mode and prints
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
    /// Wave 5: cargo-test wrapper — streams the harness output and prints a
    /// pass/fail summary.
    Test {
        /// Add --features pg-tests (requires the compose database:
        /// DATABASE_URL in the environment or <root>/.env).
        #[arg(long)]
        pg: bool,
    },
    /// Wave 5: execute a routes/console.toml command through the CommandBus
    /// under the operator principal (capability-checked; runs inside
    /// kelson-demo — the dep-rules matrix keeps tools from linking runtime
    /// crates, same posture as `kelson shell`/`kelson model:prune`).
    #[command(name = "call")]
    Call {
        /// Command name from routes/console.toml.
        name: String,
        /// JSON input payload (default {}).
        input: Option<String>,
        /// Operator principal (uuid); defaults to the first active principal
        /// in dev/test — staging/production refuse without it.
        #[arg(long)]
        principal: Option<String>,
        /// Tenant (uuid); defaults to the dev/test fallback tenant.
        #[arg(long)]
        tenant: Option<String>,
    },
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
    /// Phase 3 §4.1: validate the current config file (schema gate), print
    /// its content hash and the drift state against the pinned
    /// config.json artifact, with timing. A running server's poller picks
    /// up a valid edit within its interval and swaps without a restart
    /// (route/flag swaps measured <50ms); this command is the operator's
    /// pre-flight + report for that path. Exits non-zero when the config
    /// is invalid.
    #[command(name = "config:reload")]
    ConfigReload,
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
    /// Wave 8 B-25: materialize only the package's [config] table to
    /// config/packages/<name>.toml (the editable boot layer; publish scope
    /// restricted to config — no migrations or assets).
    #[command(name = "config:publish")]
    ConfigPublish {
        /// Indexed package name (kelson pkg:list).
        package: String,
        /// Overwrite an existing published copy whose content differs.
        #[arg(long)]
        force: bool,
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
    /// Maintenance mode: serve 503 until "kelson up".
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
    /// Process jobs via the kelson-worker binary.
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
    /// Wave 5: supervise a kelson-worker child and auto-restart it on exit
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
    /// Wave 5: scheduler daemon — tick every 60s and run due items (each due
    /// schedule fires as a durable job; last-fired state persists to
    /// storage/framework/schedule-state.json so restarts do not re-fire).
    #[command(name = "schedule:work")]
    ScheduleWork {
        /// Seconds between ticks (spec default 60).
        #[arg(long, default_value_t = 60)]
        every: u64,
    },
    /// Wave 5: dry-run — print what would fire now without executing.
    #[command(name = "schedule:test")]
    ScheduleTest,
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
    /// Wave 5: create the public/storage symlink to the local disk root
    /// (storage/local). Refuses to overwrite; --force replaces an existing
    /// link or file (a real directory is never deleted automatically).
    #[command(name = "storage:link")]
    StorageLink {
        /// Replace an existing link/file at public/storage.
        #[arg(long)]
        force: bool,
    },
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
        /// Execution id (uuid) from `kelson inspect execution`.
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
    /// Show the current release manifest (written by `kelson build`).
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
    /// §7.2 (Phase 4): promote a release through the four-stage pipeline —
    /// deploy (module pins) -> migrate (pairs) -> activate (config) ->
    /// canary — checkpointing first and restoring every dimension
    /// automatically if any health gate fails (KEEL-004).
    #[command(name = "promote")]
    Promote {
        /// Promotion plan carrying the release id and its five-dimension
        /// target (default storage/framework/promotion.json).
        #[arg(long)]
        plan: Option<String>,
        /// Print the plan and the stages that would run; change nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// §7.1 (Phase 4): restore all five dimensions to the checkpoint saved
    /// by `kelson promote`.
    #[command(name = "rollback")]
    Rollback {
        /// Checkpoint to restore (default storage/framework/promotion.checkpoint.json).
        #[arg(long)]
        checkpoint: Option<String>,
        /// Show the checkpoint without restoring anything.
        #[arg(long)]
        dry_run: bool,
    },
}

/// B-27 `kelson profile` targets: the two long-running runtimes. The package
/// and the built binary share a name for both.
#[derive(Subcommand)]
enum ProfileTarget {
    /// Record kelson-demo (the HTTP edge) under samply.
    Api,
    /// Record kelson-worker (queue consumer) under samply.
    Worker,
}

impl ProfileTarget {
    /// Workspace package (and built binary name under target/debug).
    fn name(&self) -> &'static str {
        match self {
            ProfileTarget::Api => "kelson-demo",
            ProfileTarget::Worker => "kelson-worker",
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

/// Wave 9 B-31: driver-selected pool — `[database].driver` picks the
/// Postgres or MySQL probe. Every database command dispatches on the
/// selected driver (Phase C lifted the pairs family, Phase E lifted
/// `db:schema:dump`, Phase F lifted promotion — `kelson promote`/
/// `kelson rollback` wire their events/migration/probe ports on this same
/// enum); no command in this console is pg-only anymore.
async fn db_pool(root: &Path) -> anyhow::Result<DbPool> {
    let cfg = load_config(root)?;
    let db = cfg.database.as_ref().ok_or_else(|| {
        anyhow::anyhow!("no database configured in {}", config_path(root).display())
    })?;
    match cfg.database_driver() {
        kelson_config::DatabaseDriver::Postgres => kelson_postgres::create_pool(&db.url, db.max_connections)
            .await
            .map(DbPool::Postgres)
            .map_err(|e| anyhow::anyhow!("{e}")),
        kelson_config::DatabaseDriver::MySql => kelson_mysql::create_pool(&db.url, db.max_connections)
            .await
            .map(DbPool::MySql)
            .map_err(|e| anyhow::anyhow!("{e}")),
    }
}

/// Wave 9 B-31: driver-selected queue-admin store — every `queue:*` command
/// builds its handle here, so both drivers expose one admin surface.
fn job_store_admin(pool: &DbPool) -> Arc<dyn kelson_jobs::JobStoreAdmin> {
    match pool {
        DbPool::Postgres(p) => Arc::new(kelson_postgres::PgJobStore::new(p.clone())),
        DbPool::MySql(p) => Arc::new(kelson_mysql::MySqlJobStore::new(p.clone())),
    }
}

/// Wave 9 B-31: driver-selected job store for the enqueue paths
/// (`schedule:work` ticks through `&dyn JobStore`).
fn job_store(pool: &DbPool) -> Arc<dyn kelson_jobs::JobStore> {
    match pool {
        DbPool::Postgres(p) => Arc::new(kelson_postgres::PgJobStore::new(p.clone())),
        DbPool::MySql(p) => Arc::new(kelson_mysql::MySqlJobStore::new(p.clone())),
    }
}

/// Wave 9 B-31: driver-selected batch ledger (`queue:prune-batches`,
/// `queue:retry-batch`).
fn batch_ledger(pool: &DbPool) -> Arc<dyn kelson_jobs::BatchLedger> {
    match pool {
        DbPool::Postgres(p) => Arc::new(kelson_postgres::PgBatchLedger::new(p.clone())),
        DbPool::MySql(p) => Arc::new(kelson_mysql::MySqlBatchLedger::new(p.clone())),
    }
}

/// `SELECT version()` on whichever driver the pool selected (both servers
/// expose it; the statement itself is dialect-neutral).
async fn server_version(pool: &DbPool) -> anyhow::Result<String> {
    match pool {
        DbPool::Postgres(p) => sqlx::query_scalar("SELECT version()")
            .fetch_one(p)
            .await
            .map_err(|e| anyhow::anyhow!("server version: {e}")),
        DbPool::MySql(p) => sqlx::query_scalar("SELECT version()")
            .fetch_one(p)
            .await
            .map_err(|e| anyhow::anyhow!("server version: {e}")),
    }
}

/// Current database name — `current_database()` on Postgres, `DATABASE()`
/// on MySQL (B-31 dialect twin).
async fn current_database(pool: &DbPool) -> anyhow::Result<String> {
    match pool {
        DbPool::Postgres(p) => sqlx::query_scalar("SELECT current_database()")
            .fetch_one(p)
            .await
            .map_err(|e| anyhow::anyhow!("current database: {e}")),
        DbPool::MySql(p) => sqlx::query_scalar("SELECT DATABASE()")
            .fetch_one(p)
            .await
            .map_err(|e| anyhow::anyhow!("current database: {e}")),
    }
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
        Cmd::PkgList => pkg_list(),
        Cmd::PkgAdd { path } => pkg_add(&path),
        Cmd::Publish { package, force } => publish_cmd(&package, force),
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
        Cmd::ConfigReload => config_reload(),
        Cmd::ConfigShow { key, json } => config_show(&key, json),
        Cmd::ConfigPublish { package, force } => config_publish_cmd(&package, force),
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
        Cmd::ScheduleWork { every } => schedule_work(every),
        Cmd::ScheduleTest => schedule_test(),
        Cmd::DbInspect => rt(db_inspect()),
        Cmd::DbShow => rt(db_show()),
        Cmd::DbSchemaDump => rt(db_schema_dump()),
        Cmd::StorageLink { force } => storage_link(force),
        Cmd::Test { pg } => run_tests(pg),
        Cmd::Call { name, input, principal, tenant } => call_command(
            &name,
            input.as_deref(),
            principal.as_deref(),
            tenant.as_deref(),
        ),
        Cmd::New {
            name,
            plane,
            description,
            with_starter,
            standalone,
            deps_root,
        } => new_project_cmd(
            name,
            &plane,
            &description,
            with_starter,
            standalone,
            deps_root.as_deref(),
        ),
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
        Cmd::Promote { plan, dry_run } => rt(promote(plan, dry_run)),
        Cmd::Rollback { checkpoint, dry_run } => rt(rollback(checkpoint, dry_run)),
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

/// Advisory checks report a missing *optional* dev convenience without failing
/// the run. `docker compose` is documented as optional (`01-GETTING-STARTED`
/// Prerequisites), and a workspace may legitimately carry no pinned toolchain -
/// ADR 0014 §5 leaves the MSRV floor to the convergence port. Folding both into
/// `all_ok` made `doctor` unable to exit 0 on a fresh clone, contradicting the
/// documented "doctor and check must be clean" gate. A required check that
/// cannot pass is not a gate, it is noise.
fn advisory(ok: bool, label: &str, detail: &str) -> bool {
    println!("  {} {label}: {detail}", if ok { "[ok]" } else { "[--]" });
    ok
}

fn doctor() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    println!("kelson doctor — workspace {}", root.display());
    let mut all_ok = true;
    let mut advisory_notes: Vec<&str> = Vec::new();

    let rustc = tool_version("rustc", "--version");
    all_ok &= line(rustc != "NOT FOUND", "rustc", &rustc);
    let cargo = tool_version("cargo", "--version");
    all_ok &= line(cargo != "NOT FOUND", "cargo", &cargo);

    let toolchain = root.join("rust-toolchain.toml");
    if !advisory(
        toolchain.exists(),
        "rust-toolchain.toml",
        &toolchain.display().to_string(),
    ) {
        advisory_notes.push("no pinned toolchain");
    }

    let cfg = root.join("config/dev.toml");
    let cfg_ok = cfg.exists()
        && std::fs::read_to_string(&cfg)
            .ok()
            .and_then(|s| s.parse::<toml::Value>().ok())
            .is_some();
    all_ok &= line(cfg_ok, "config/dev.toml", "present + parses");

    let compose = root.join("docker-compose.yml");
    if !advisory(
        compose.exists(),
        "docker-compose.yml",
        "postgres + minio for local dev",
    ) {
        advisory_notes
            .push("no docker-compose.yml (`docker compose up -d` for postgres-backed dev)");
    }

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
        .and_then(|s| kelson_analyzer::parse_members(&s).ok())
        .unwrap_or_default();
    all_ok &= line(
        !members.is_empty(),
        "workspace members",
        &format!("{}", members.len()),
    );

    if !advisory_notes.is_empty() {
        println!();
        println!("  advisory (not failing): {}", advisory_notes.join("; "));
    }
    if !all_ok {
        return Ok(ExitCode::FAILURE);
    }
    println!("doctor complete.");
    Ok(ExitCode::SUCCESS)
}

/// Wave 4 Phase 8 (docs/19 L222): `kelson about` — the Laravel-style report.
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

/// Build the `kelson about` report for an explicit workspace root.
/// `db_version` is the best-effort server-version ping (None = no database
/// configured or unreachable); the golden-fields test drives this directly.
fn about_report(root: &Path, db_version: Option<&str>) -> anyhow::Result<String> {
    let cfg = load_config(root)?;
    let mut out = String::from("kelson about\n\n");
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
    // picks redis for the cache port (kelson-demo run) and the job store
    // (kelson-worker, which falls back to the database queue, then memory).
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

    // Compiled graph counts; a broken graph still reports — `kelson check`
    // carries the violations.
    let compiled = discover_manifests(root).and_then(|manifests| {
        kelson_registry::PlatformGraph::compile(manifests)
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
    server_version(&pool).await.ok()
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
    let registry = Arc::new(kelson_feature_flags::platform_registry());
    let registered = registry.definitions().len();
    let store: Arc<dyn kelson_feature_flags::FlagStore> =
        Arc::new(kelson_feature_flags::FileFlagStore::new(
            kelson_feature_flags::default_flags_path(root),
        ));
    let capabilities = std::collections::BTreeSet::new();
    let input = kelson_feature_flags::FlagInput {
        environment,
        tenant: None,
        region: None,
        capabilities: &capabilities,
        actor: kelson_foundation::PrincipalId::from_uuid(uuid::Uuid::nil()),
        internal_actor: true,
    };
    let service = kelson_feature_flags::FlagService::new(registry, store);
    let snapshot = service
        .snapshot(&input)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok((registered, snapshot.flags))
}

fn check() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    println!("kelson check — architecture gate G07");
    let raw = std::fs::read_to_string(root.join("Cargo.toml"))?;
    let members = kelson_analyzer::parse_members(&raw).map_err(|e| anyhow::anyhow!("{e}"))?;
    let violations = kelson_analyzer::analyze_workspace(&root);
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
    // during boot), so it runs at kelson-demo boot, not here.
    let mut failures = violations.len();
    match discover_manifests(&root) {
        Ok(manifests) => match kelson_registry::PlatformGraph::compile(manifests) {
            Ok(graph) => match kelson_registry::MiddlewareCatalog::load(&root) {
                Ok(catalog) => {
                    let mw = kelson_registry::middleware_violations(&graph, &catalog);
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
    let spec = kelson_codegen::ModuleSpec {
        name: name.to_string(),
        plane: plane.to_string(),
        description: if description.is_empty() {
            format!("Module {name}")
        } else {
            description.to_string()
        },
    };
    let written = kelson_codegen::write_module(&root, &spec).map_err(|e| anyhow::anyhow!(e))?;
    kelson_codegen::add_workspace_member(&root, &spec.member_path())
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
    println!("migration created. Migrations are embedded at compile time; run: kelson migrate.");
    Ok(ExitCode::SUCCESS)
}

// -------------------------------------------------------------------- graph

fn discover_manifests(root: &Path) -> anyhow::Result<Vec<kelson_registry::ModuleManifest>> {
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
                let manifest = kelson_registry::ModuleManifest::from_toml(&raw)
                    .map_err(|v| anyhow::anyhow!("{}: {}", manifest_path.display(), v.message))?;
                out.push(manifest);
            }
        }
    }
    // Wave 8 B-24: indexed packages (root packages.toml) merge into the
    // same fail-closed compile as applications/ and modules/.
    out.extend(
        kelson_registry::PackageIndex::load(root)
            .map_err(anyhow::Error::msg)?
            .manifests(root)
            .map_err(anyhow::Error::msg)?,
    );
    out.extend(discover_route_files(root)?);
    out.sort_by(|a, b| a.name().cmp(b.name()));
    Ok(out)
}

/// Wave 2 root route files (mirroring Laravel's routes/*.php): merge
/// `routes/web.toml` + `routes/api.toml` routes (plane forced by file),
/// `routes/console.toml` commands and `routes/channels.toml` channels
/// into synthetic module manifests. Absent files are fine; the graph gates
/// (GRAPH-ROUTEFILE-OP, GRAPH-CHANNEL-CAP) validate their references.
fn discover_route_files(root: &Path) -> anyhow::Result<Vec<kelson_registry::ModuleManifest>> {
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
            kelson_registry::RouteFileManifest::from_toml(&raw, &format!("routes/{file}"), plane)
                .map_err(|v| anyhow::anyhow!("{}: {}", path.display(), v.message))?;
        out.push(manifest.into_module(module_name));
    }
    Ok(out)
}

fn compile_graph(root: &Path) -> Result<kelson_registry::PlatformGraph, ExitCode> {
    let manifests = discover_manifests(root).map_err(|e| {
        eprintln!("error: {e:#}");
        ExitCode::FAILURE
    })?;
    kelson_registry::PlatformGraph::compile(manifests).map_err(|violations| {
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

// --------------------------------------------------------------- packages

fn pkg_list() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    pkg_list_at(&root)
}

/// `kelson pkg:list` — every indexed package with its manifest + ABI status.
/// Fail-closed: any unusable entry (missing/malformed manifest, ABI
/// mismatch) exits non-zero so CI can gate on it, while still printing the
/// full index for diagnosis.
fn pkg_list_at(root: &Path) -> anyhow::Result<ExitCode> {
    let index =
        kelson_registry::PackageIndex::load(root).map_err(|e| anyhow::anyhow!("{}", e))?;
    if index.package.is_empty() {
        println!("  (no packages - kelson pkg:add <path> to register one)");
        return Ok(ExitCode::SUCCESS);
    }
    let mut broken = false;
    for entry in &index.package {
        match index.entry_manifest(root, entry) {
            Ok(manifest) => {
                let compatible = manifest.module.api == kelson_registry::MODULE_ABI;
                if !compatible {
                    broken = true;
                }
                println!(
                    "  {:<24} v{:<10} path={:<28} abi={} {}",
                    entry.name,
                    entry.version,
                    entry.path,
                    manifest.module.api,
                    if compatible {
                        "compatible"
                    } else {
                        "INCOMPATIBLE (kelson graph refuses it)"
                    }
                );
            }
            Err(err) => {
                broken = true;
                println!(
                    "  {:<24} v{:<10} path={:<28} invalid: {err}",
                    entry.name, entry.version, entry.path
                );
            }
        }
    }
    println!(
        "packages: {} indexed ({})",
        index.package.len(),
        if broken {
            "NOT graph-ready"
        } else {
            "graph-ready"
        }
    );
    Ok(if broken {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn pkg_add(path: &str) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    pkg_add_at(&root, path)
}

/// `kelson pkg:add <path>` — validate the package's `module.toml` (the
/// package v1 contract) and append it to `packages.toml`. Duplicates are
/// refused by name and by resolved directory; an ABI mismatch only warns
/// (the index records what exists — `kelson pkg:list` and graph compile are
/// the fail-closed gates).
fn pkg_add_at(root: &Path, path: &str) -> anyhow::Result<ExitCode> {
    let requested = std::path::Path::new(path);
    let dir = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    let manifest_path = dir.join("module.toml");
    if !manifest_path.exists() {
        anyhow::bail!(
            "no module.toml at {} - a package v1 ships a manifest",
            manifest_path.display()
        );
    }
    let raw = std::fs::read_to_string(&manifest_path)
        .map_err(|e| anyhow::anyhow!("{}: {e}", manifest_path.display()))?;
    let manifest = kelson_registry::ModuleManifest::from_toml(&raw)
        .map_err(|v| anyhow::anyhow!("{}: {}", manifest_path.display(), v.message))?;
    let mut index =
        kelson_registry::PackageIndex::load(root).map_err(|e| anyhow::anyhow!("{}", e))?;
    let name = manifest.name().to_string();
    if index.contains_name(&name) {
        anyhow::bail!("package {name} is already indexed (kelson pkg:list)");
    }
    if index.contains_dir(root, &dir) {
        anyhow::bail!("{} is already indexed under another name", dir.display());
    }
    // Root-relative paths keep the index portable across checkouts;
    // paths outside the root stay absolute.
    let stored = match dir.strip_prefix(root) {
        Ok(rel) => rel.display().to_string(),
        Err(_) => dir.display().to_string(),
    };
    index.append(kelson_registry::PackageEntry {
        name: name.clone(),
        version: manifest.module.version.clone(),
        path: stored.clone(),
    });
    index.save(root).map_err(|e| anyhow::anyhow!("{}", e))?;
    println!(
        "package {name} v{} added -> {} (path={stored})",
        manifest.module.version,
        kelson_registry::PackageIndex::packages_path(root).display()
    );
    if manifest.module.api != kelson_registry::MODULE_ABI {
        println!(
            "warning: manifest api={} does not match host MODULE_ABI={} - kelson graph will refuse it",
            manifest.module.api,
            kelson_registry::MODULE_ABI
        );
    }
    println!("kelson graph merges it fail-closed; kelson pkg:list shows ABI status");
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------- publish (Wave 8)

/// What one publish run did (printed by the command wrappers, asserted by
/// tests). Paths are workspace-root-relative.
#[derive(Debug, Default, PartialEq, Eq)]
struct PublishOutcome {
    /// Targets written this run.
    written: Vec<PathBuf>,
    /// Targets whose bytes already matched (the idempotent no-op).
    unchanged: usize,
    /// Targets whose existing content differs - skipped without --force.
    conflicts: Vec<PathBuf>,
    /// `<name>::` view namespaces whose package ships a views/ directory
    /// (registered at boot by the views engine - views are never copied).
    view_namespaces: Vec<String>,
}

impl PublishOutcome {
    /// A skipped conflict means the publish did not fully land: fail
    /// closed so CI notices (review the drift, re-run with --force).
    fn exit_code(&self) -> ExitCode {
        if self.conflicts.is_empty() {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        }
    }
}

/// Publish scope: the full surface, or config only (`config:publish`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum PublishScope {
    /// Migrations + config + assets (+ the view-namespace report).
    All,
    /// Only config/packages/<name>.toml.
    ConfigOnly,
}

fn publish_cmd(package: &str, force: bool) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let outcome = publish_at(&root, package, force, PublishScope::All)?;
    print_publish_outcome(&outcome);
    Ok(outcome.exit_code())
}

fn config_publish_cmd(package: &str, force: bool) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let outcome = publish_at(&root, package, force, PublishScope::ConfigOnly)?;
    print_publish_outcome(&outcome);
    Ok(outcome.exit_code())
}

fn print_publish_outcome(outcome: &PublishOutcome) {
    for path in &outcome.written {
        println!("  + {}", path.display());
    }
    for path in &outcome.conflicts {
        println!(
            "  ! {} (conflict: content differs, use --force)",
            path.display()
        );
    }
    for ns in &outcome.view_namespaces {
        println!(
            "  = {ns}:: registered at boot from the package views/ directory (no files copied)"
        );
    }
    println!(
        "publish: {} written, {} unchanged, {} conflict(s)",
        outcome.written.len(),
        outcome.unchanged,
        outcome.conflicts.len()
    );
    if outcome
        .written
        .iter()
        .any(|p| p.starts_with("database/migrations"))
    {
        println!("note: migrations are embedded at compile time; rebuild before kelson migrate");
    }
}

/// Wave 8 B-25: materialize an indexed package's resources (docs/15
/// "Publish contract"). Source -> target:
/// `<pkg>/database/migrations/{v}_{desc}.sql` becomes
/// `database/migrations/{v}_{name}_{desc}.sql` (sqlx keys migrations by
/// the leading integer, so the version stays first and the package name
/// namespaces the rest - byte-deterministic, therefore idempotent), the
/// manifest `[config]` table becomes `config/packages/<name>.toml` (the
/// editable overlay boot reads), `<pkg>/public/**` becomes
/// `public/vendor/<name>/**`, and `views/` registers under `<name>::` at
/// boot (never copied). Every target is byte-compared: identical =
/// unchanged, different = conflict (skipped unless --force), missing =
/// written. A non-empty conflict set exits FAILURE.
fn publish_at(
    root: &Path,
    package: &str,
    force: bool,
    scope: PublishScope,
) -> anyhow::Result<PublishOutcome> {
    let index = kelson_registry::PackageIndex::load(root).map_err(|e| anyhow::anyhow!("{}", e))?;
    let entry = index
        .package
        .iter()
        .find(|e| e.name == package)
        .ok_or_else(|| {
            anyhow::anyhow!("package {package} is not indexed (kelson pkg:list shows the index)")
        })?;
    let manifest = index
        .entry_manifest(root, entry)
        .map_err(|e| anyhow::anyhow!("{}", e))?;
    if manifest.name() != package {
        anyhow::bail!(
            "index entry {package} drifted from its manifest (named {}); re-run kelson pkg:add <path>",
            manifest.name()
        );
    }
    let pkg_dir = kelson_registry::PackageIndex::entry_dir(root, entry);
    let mut outcome = PublishOutcome::default();

    // Config (both scopes): the manifest [config] table materializes into
    // the editable boot layer config/packages/<name>.toml. Full publish
    // treats it as optional; config:publish requires it.
    match (scope, manifest.config.as_ref()) {
        (PublishScope::ConfigOnly, None) => {
            anyhow::bail!("package {package} ships no [config] table to publish");
        }
        (PublishScope::ConfigOnly, Some(config)) | (PublishScope::All, Some(config)) => {
            if !config.is_table() {
                anyhow::bail!("package {package}: [config] must be a table");
            }
            let bytes = published_config_bytes(package, config)?;
            materialize(
                root,
                &root
                    .join("config")
                    .join("packages")
                    .join(format!("{package}.toml")),
                &bytes,
                force,
                &mut outcome,
            )?;
        }
        (PublishScope::All, None) => {}
    }
    if scope == PublishScope::ConfigOnly {
        return Ok(outcome);
    }

    // Migrations: validated sources publish name-prefixed after the sqlx
    // integer version; a version already present under a different file
    // name is refused (applied migrations are keyed by version, so a
    // duplicate would break `kelson migrate`).
    let mdir = pkg_dir.join("database").join("migrations");
    if mdir.is_dir() {
        let mut sources = Vec::new();
        walk_files(&mdir, &mut sources)
            .map_err(|e| anyhow::anyhow!("{}: {e}", mdir.display()))?;
        sources.sort();
        let target_dir = root.join("database").join("migrations");
        let mut existing: Vec<(i64, String)> = Vec::new();
        if target_dir.is_dir() {
            let mut files = Vec::new();
            walk_files(&target_dir, &mut files)
                .map_err(|e| anyhow::anyhow!("{}: {e}", target_dir.display()))?;
            files.sort();
            for f in files {
                if let Some(name) = f.file_name().and_then(|n| n.to_str()) {
                    // Unparsable leftovers cannot collide by version.
                    if let Ok((version, _)) = migration_source_parts(name) {
                        existing.push((version, name.to_string()));
                    }
                }
            }
        }
        let mut seen: std::collections::HashMap<i64, String> = std::collections::HashMap::new();
        for src in &sources {
            let file_name = src
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| anyhow::anyhow!("migration file name is not UTF-8: {}", src.display()))?;
            let (version, desc) = migration_source_parts(file_name)?;
            if let Some(prev) = seen.insert(version, file_name.to_string()) {
                anyhow::bail!(
                    "package {package} migrations {prev} and {file_name} share version {version}; give each a unique integer prefix"
                );
            }
            let target_name = format!("{version}_{package}_{desc}.sql");
            for (ex_version, ex_name) in &existing {
                if *ex_version == version && *ex_name != target_name {
                    anyhow::bail!(
                        "migration version {version} collides with database/migrations/{ex_name}; re-stamp {file_name} with a free version"
                    );
                }
            }
            let bytes =
                std::fs::read(src).map_err(|e| anyhow::anyhow!("{}: {e}", src.display()))?;
            materialize(root, &target_dir.join(&target_name), &bytes, force, &mut outcome)?;
        }
    }

    // Assets: <pkg>/public/** -> public/vendor/<name>/**.
    let pdir = pkg_dir.join("public");
    if pdir.is_dir() {
        let mut files = Vec::new();
        walk_files(&pdir, &mut files)
            .map_err(|e| anyhow::anyhow!("{}: {e}", pdir.display()))?;
        files.sort();
        for src in files {
            let rel = src
                .strip_prefix(&pdir)
                .map_err(|_| anyhow::anyhow!("{} escapes the package", src.display()))?;
            let bytes =
                std::fs::read(&src).map_err(|e| anyhow::anyhow!("{}: {e}", src.display()))?;
            materialize(
                root,
                &root.join("public").join("vendor").join(package).join(rel),
                &bytes,
                force,
                &mut outcome,
            )?;
        }
    }

    // Views: registered under <name>:: at boot, never copied.
    if pkg_dir.join("views").is_dir() {
        outcome.view_namespaces.push(package.to_string());
    }

    Ok(outcome)
}

/// Deterministic bytes for the published config copy (header + the
/// manifest [config] table) - byte-identical re-materialization is what
/// makes `config:publish` idempotent.
fn published_config_bytes(package: &str, config: &toml::Value) -> anyhow::Result<Vec<u8>> {
    let body = toml::to_string(config)
        .map_err(|e| anyhow::anyhow!("package {package}: [config] serialize: {e}"))?;
    Ok(format!(
        "# Package {package} config (materialized from its module.toml [config]\n\
         # table by `kelson config:publish`). Edit values here to override the\n\
         # package defaults; the app config file still wins over this file.\n\
         {body}"
    )
    .into_bytes())
}

/// One publish target: identical content is the idempotent no-op,
/// different content is a conflict (overwritten only with --force), and a
/// missing target writes. The outcome records what happened.
fn materialize(
    root: &Path,
    target: &Path,
    bytes: &[u8],
    force: bool,
    outcome: &mut PublishOutcome,
) -> anyhow::Result<()> {
    let rel = target.strip_prefix(root).unwrap_or(target).to_path_buf();
    if target.is_file() {
        let existing =
            std::fs::read(target).map_err(|e| anyhow::anyhow!("{}: {e}", target.display()))?;
        if existing == bytes {
            outcome.unchanged += 1;
        } else if force {
            std::fs::write(target, bytes)
                .map_err(|e| anyhow::anyhow!("{}: {e}", target.display()))?;
            outcome.written.push(rel);
        } else {
            outcome.conflicts.push(rel);
        }
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("{}: {e}", parent.display()))?;
    }
    std::fs::write(target, bytes).map_err(|e| anyhow::anyhow!("{}: {e}", target.display()))?;
    outcome.written.push(rel);
    Ok(())
}

/// Package migration file contract: `<integer version>_<description>.sql`
/// (lowercase description - the same rule `kelson make:migration` enforces).
/// sqlx parses the leading part as i64 and applies migrations by that
/// number, so the version must lead the published filename too.
fn migration_source_parts(name: &str) -> anyhow::Result<(i64, String)> {
    let (version_raw, rest) = name.split_once('_').ok_or_else(|| {
        anyhow::anyhow!(
            "migration {name} must be named <version>_<description>.sql: an integer version prefix leads (sqlx orders migrations by that number)"
        )
    })?;
    if version_raw.is_empty() || !version_raw.bytes().all(|b| b.is_ascii_digit()) {
        anyhow::bail!(
            "migration {name} must start with an integer version prefix (sqlx orders migrations by it, e.g. 20260901000001_{name})"
        );
    }
    let version: i64 = version_raw
        .parse()
        .map_err(|_| anyhow::anyhow!("migration {name}: version prefix {version_raw} does not fit i64"))?;
    let desc = rest
        .strip_suffix(".sql")
        .ok_or_else(|| anyhow::anyhow!("migration {name} must end in .sql"))?;
    if desc.is_empty()
        || !desc
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    {
        anyhow::bail!(
            "migration {name}: description must match [a-z0-9_]+ (the rule kelson make:migration enforces)"
        );
    }
    Ok((version, desc.to_string()))
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
    graph: &kelson_registry::PlatformGraph,
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
    let catalog = kelson_registry::MiddlewareCatalog::load(root)
        .map_err(|e| anyhow::anyhow!("middleware catalog rejected: {e}"))?;
    let plans = kelson_registry::resolve_route_plans(&graph, &catalog).map_err(|violations| {
        anyhow::anyhow!(
            "route resolution failed: {}",
            violations
                .iter()
                .map(|v| format!("{}: {}", v.code, v.message))
                .collect::<Vec<_>>()
                .join("; ")
        )
    })?;
    let artifact = kelson_registry::RouteCacheArtifact {
        module_abi: kelson_registry::MODULE_ABI,
        manifest_hash: kelson_registry::manifests_hash(root)
            .map_err(|e| anyhow::anyhow!("manifest hash failed: {e}"))?,
        plans: plans.clone(),
    };
    let path = kelson_registry::route_cache_path(root);
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
    let path = kelson_registry::route_cache_path(root);
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
        kelson_codegen::openapi::emit_openapi(&graph, "keel-platform", env!("CARGO_PKG_VERSION"));
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
    let tools = kelson_ai_mcp::tools_from_graph(&graph);
    match format {
        "json" => {
            let manifest =
                kelson_ai_mcp::generate_manifest("keel-platform", env!("CARGO_PKG_VERSION"), &tools);
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
fn db_err(e: kelson_foundation::AppError) -> anyhow::Error {
    match e.internal {
        Some(detail) => anyhow::anyhow!("{detail}"),
        None => anyhow::anyhow!("{e}"),
    }
}

fn migrator() -> &'static sqlx::migrate::Migrator {
    // Embedded from database/migrations, same source kelson-postgres boots with.
    static MIGRATOR: std::sync::OnceLock<sqlx::migrate::Migrator> = std::sync::OnceLock::new();
    MIGRATOR.get_or_init(|| sqlx::migrate!("../../database/migrations"))
}

fn mysql_migrator() -> &'static sqlx::migrate::Migrator {
    // Wave 9 B-31: the forward MySQL twin set, same source kelson-mysql boots
    // with (each driver embeds its own directory — sqlx bookkeeping shares
    // the `_sqlx_migrations` table per database).
    static MIGRATOR: std::sync::OnceLock<sqlx::migrate::Migrator> = std::sync::OnceLock::new();
    MIGRATOR.get_or_init(|| sqlx::migrate!("../../database/migrations-mysql"))
}

async fn migrate() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let pair_applied = match &pool {
        DbPool::Postgres(p) => kelson_postgres::run_migrations(p, &root).await.map_err(db_err)?,
        DbPool::MySql(p) => kelson_mysql::run_migrations(p, &root).await.map_err(db_err)?,
    };
    for name in &pair_applied {
        println!("  + {name}");
    }
    println!("migrations applied.");
    Ok(ExitCode::SUCCESS)
}

async fn migrate_status() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let applied: Vec<i64> = match &pool {
        DbPool::Postgres(p) => {
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
                .fetch_all(p)
                .await
        }
        DbPool::MySql(p) => {
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations WHERE success ORDER BY version")
                .fetch_all(p)
                .await
        }
    }
    .unwrap_or_default();
    // B-31: each driver lists its OWN embedded set (database/migrations vs
    // database/migrations-mysql); the applied bookkeeping table is shared.
    let migrator = match &pool {
        DbPool::Postgres(_) => migrator(),
        DbPool::MySql(_) => mysql_migrator(),
    };
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
    // every pair is pending). B-31 Phase C: both drivers share one pairs
    // directory and one bookkeeping table, so the listing is driver-agnostic
    // — only the connection (and which crate scans) changes.
    let pair_applied: Vec<String> = match &pool {
        DbPool::Postgres(p) => {
            sqlx::query_scalar("SELECT migration FROM keel_migrations ORDER BY migration")
                .fetch_all(p)
                .await
                .unwrap_or_default()
        }
        DbPool::MySql(p) => {
            sqlx::query_scalar("SELECT migration FROM keel_migrations ORDER BY migration")
                .fetch_all(p)
                .await
                .unwrap_or_default()
        }
    };
    let scanned: Vec<String> = match &pool {
        DbPool::Postgres(_) => kelson_postgres::pairs::scan_pairs(&root)
            .map_err(db_err)?
            .into_iter()
            .map(|p| p.migration)
            .collect(),
        DbPool::MySql(_) => kelson_mysql::pairs::scan_pairs(&root)
            .map_err(db_err)?
            .into_iter()
            .map(|p| p.migration)
            .collect(),
    };
    for pair in scanned {
        let is_applied = pair_applied.contains(&pair);
        if !is_applied {
            pending += 1;
        }
        println!(
            "  {:<8} {} (pair)",
            if is_applied { "applied" } else { "PENDING" },
            pair
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
    // The schema drop happens outside the migration lock (fresh is dev/test
    // only and confirmed); the migration run that follows locks inside
    // run_migrations — locking here too would deadlock against it on a
    // second connection.
    match &pool {
        DbPool::Postgres(p) => {
            sqlx::query("DROP SCHEMA public CASCADE")
                .execute(p)
                .await
                .map_err(|e| anyhow::anyhow!("drop schema: {e}"))?;
            sqlx::query("CREATE SCHEMA public")
                .execute(p)
                .await
                .map_err(|e| anyhow::anyhow!("create schema: {e}"))?;
        }
        DbPool::MySql(p) => {
            // B-31: MySQL has no cascadeable schema inside a database — drop
            // every table (bookkeeping included) with FK checks off, then the
            // forward run rebuilds from zero. Dev/test only, like pg.
            //
            // Phase H: FOREIGN_KEY_CHECKS is SESSION-scoped, so the switch
            // and every DROP must share ONE pooled connection — as pooled
            // checkouts the SET never reached the drops and MySQL refused the
            // first FK-referenced table (error 3730 on `memberships`). The
            // held-connection shape matches migrate_rollback below.
            let mut conn = p
                .acquire()
                .await
                .map_err(|e| anyhow::anyhow!("drop schema: connect: {e}"))?;
            sqlx::query("SET FOREIGN_KEY_CHECKS = 0")
                .execute(&mut *conn)
                .await
                .map_err(|e| anyhow::anyhow!("drop schema: {e}"))?;
            let tables: Vec<String> = sqlx::query_scalar(
                "SELECT table_name FROM information_schema.tables WHERE table_schema = DATABASE()",
            )
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| anyhow::anyhow!("list tables: {e}"))?;
            for table in &tables {
                sqlx::query(&format!("DROP TABLE IF EXISTS `{table}`"))
                    .execute(&mut *conn)
                    .await
                    .map_err(|e| anyhow::anyhow!("drop table {table}: {e}"))?;
            }
            sqlx::query("SET FOREIGN_KEY_CHECKS = 1")
                .execute(&mut *conn)
                .await
                .map_err(|e| anyhow::anyhow!("restore FK checks: {e}"))?;
        }
    }
    let pair_applied = match &pool {
        DbPool::Postgres(p) => kelson_postgres::run_migrations(p, &root).await.map_err(db_err)?,
        DbPool::MySql(p) => kelson_mysql::run_migrations(p, &root).await.map_err(db_err)?,
    };
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
    // Session-scoped lock on both drivers (`pg_advisory_lock(7259)` /
    // `GET_LOCK('kelson:migrate')`): acquire, roll back, and release on one
    // pooled connection (a concurrent migrate serializes instead of
    // interleaving). The MySQL pair runs without a transaction — DDL
    // auto-commits — so a failure reports which statements already ran.
    let reverted = match pool {
        DbPool::Postgres(pool) => {
            let mut conn = pool
                .acquire()
                .await
                .map_err(|e| anyhow::anyhow!("migrate: connect: {e}"))?;
            kelson_postgres::acquire_migration_lock(&mut conn)
                .await
                .map_err(db_err)?;
            let outcome = kelson_postgres::pairs::rollback(&mut *conn, &root, steps).await;
            let released = kelson_postgres::release_migration_lock(&mut conn).await;
            let reverted = outcome.map_err(db_err)?;
            released.map_err(db_err)?;
            reverted
        }
        DbPool::MySql(pool) => {
            let mut conn = pool
                .acquire()
                .await
                .map_err(|e| anyhow::anyhow!("migrate: connect: {e}"))?;
            kelson_mysql::acquire_migration_lock(&mut conn)
                .await
                .map_err(db_err)?;
            let outcome = kelson_mysql::pairs::rollback(&mut *conn, &root, steps).await;
            let released = kelson_mysql::release_migration_lock(&mut conn).await;
            let reverted = outcome.map_err(db_err)?;
            released.map_err(db_err)?;
            reverted
        }
    };
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

/// §7 (Phase 4): everything `kelson promote`/`kelson rollback` drive, wired once.
///
/// The five dimension ports live in `kelson-deployment` (Enterprise class,
/// which may reach neither `Data` nor `Extension`); this is the Tools-class
/// composition root that supplies the concrete adapters.
struct PromotionWiring {
    executor: Arc<kelson_deployment::rollback::RollbackExecutor>,
    config: Arc<promotion::ConfigAdapter>,
    events: Arc<promotion::EventAdapter>,
    pool: DbPool,
}

/// Build the wiring against `db` — either driver (B-31 Phase F). The pooled
/// ports below (events, migration adapter, health probes) dispatch on
/// `DbPool`, the same enum every other console command selects on; Phase 4
/// wired `PgPool` directly and this function used to refuse MySQL — the
/// last pg-only code path in either console (the refusal helper is deleted
/// with it).
fn wire_promotion(root: &Path, db: &DbPool) -> anyhow::Result<PromotionWiring> {
    let config = Arc::new(
        promotion::ConfigAdapter::boot(root).map_err(|e| anyhow::anyhow!("{e}"))?,
    );
    let events = Arc::new(promotion::EventAdapter::new(db.clone()));
    let modules = promotion::ModulePinAdapter::new(root);
    // No application `JobRegistry`: the CLI does not link application
    // handlers (same posture as `kelson call`). The queue itself is still
    // consulted — see `JobAdapter`'s doc for why both sources matter.
    let jobs = Arc::new(promotion::JobAdapter::new(
        job_store_admin(db),
        Arc::new(kelson_jobs::JobRegistry::new()),
    ));
    let migrations = promotion::MigrationAdapter::new(root.to_path_buf(), db.clone());

    let executor = Arc::new(kelson_deployment::rollback::RollbackExecutor::new(
        config.clone(),
        Arc::new(migrations),
        Arc::new(modules),
        jobs,
        events.clone(),
    ));
    Ok(PromotionWiring {
        executor,
        config,
        events,
        pool: db.clone(),
    })
}

/// `kelson promote` (§7.2): run the four-stage pipeline against the plan,
/// checkpointing first and letting the pipeline auto-rollback on a failed
/// health gate.
async fn promote(plan: Option<String>, dry_run: bool) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let plan_path = promotion::resolve(&root, plan.as_deref(), promotion::PLAN_PATH);
    let artifact = promotion::load_plan(&plan_path).map_err(|e| anyhow::anyhow!("{e}"))?;
    let checkpoint_path = promotion::resolve(&root, None, promotion::CHECKPOINT_PATH);

    let db = db_pool(&root).await?;
    let wiring = wire_promotion(&root, &db)?;

    let probes = promotion::promotion_probes(
        &root,
        wiring.pool.clone(),
        artifact.target.db_version,
        wiring.config.clone(),
        wiring.events.clone(),
    );
    let pipeline =
        kelson_deployment::promote::PromotionPipeline::new(wiring.executor.clone(), probes)
            .map_err(|e| anyhow::anyhow!("{e}"))?;

    if dry_run {
        println!("plan:       {}", plan_path.display());
        println!("release:    {}", artifact.release);
        println!(
            "target:     {}",
            serde_json::to_string_pretty(&artifact.target).map_err(|e| anyhow::anyhow!("{e}"))?
        );
        println!("stages:     post-deploy -> post-migrate -> post-traffic -> canary");
        println!("checkpoint: {}", checkpoint_path.display());
        return Ok(ExitCode::SUCCESS);
    }

    // §7.2 `create_checkpoint`, taken BEFORE anything moves and persisted so
    // a later `kelson rollback` restores exactly this state.
    let checkpoint = wiring
        .executor
        .checkpoint()
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    promotion::save_checkpoint(&checkpoint_path, &checkpoint)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("checkpoint -> {}", checkpoint_path.display());

    let state = pipeline
        .promote(artifact.clone())
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    println!("promoted {} ({:?}); checkpoint retained at {}",
        artifact.release, state, checkpoint_path.display());
    Ok(ExitCode::SUCCESS)
}

/// `kelson rollback` (§7.1): restore every dimension to the saved checkpoint.
async fn rollback(checkpoint: Option<String>, dry_run: bool) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let checkpoint_path = promotion::resolve(
        &root,
        checkpoint.as_deref(),
        promotion::CHECKPOINT_PATH,
    );
    let target = promotion::load_checkpoint(&checkpoint_path)
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    if dry_run {
        println!("checkpoint: {}", checkpoint_path.display());
        println!(
            "{}",
            serde_json::to_string_pretty(&target).map_err(|e| anyhow::anyhow!("{e}"))?
        );
        return Ok(ExitCode::SUCCESS);
    }

    let db = db_pool(&root).await?;
    let wiring = wire_promotion(&root, &db)?;

    // The read-only job/event guards run first inside `rollback`, so an
    // incompatible target refuses with NOTHING half-moved — the error below
    // is that refusal, not a partial rollback.
    let report = wiring
        .executor
        .rollback(&target)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;

    println!("config -> revision {}", report.config_version);
    if report.migrations_rolled_back {
        for name in &report.reverted {
            println!("  - {name}");
        }
    } else {
        println!("migrations: untouched (checkpoint carried no schema watermark)");
    }
    println!(
        "module pins rewritten: {} | guards: jobs ok, events ok",
        report.pins_written
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
    let reverted = match pool {
        DbPool::Postgres(pool) => {
            let mut conn = pool
                .acquire()
                .await
                .map_err(|e| anyhow::anyhow!("migrate: connect: {e}"))?;
            kelson_postgres::acquire_migration_lock(&mut conn)
                .await
                .map_err(db_err)?;
            let outcome = kelson_postgres::pairs::reset(&mut *conn, &root).await;
            let released = kelson_postgres::release_migration_lock(&mut conn).await;
            let reverted = outcome.map_err(db_err)?;
            released.map_err(db_err)?;
            reverted
        }
        DbPool::MySql(pool) => {
            let mut conn = pool
                .acquire()
                .await
                .map_err(|e| anyhow::anyhow!("migrate: connect: {e}"))?;
            kelson_mysql::acquire_migration_lock(&mut conn)
                .await
                .map_err(db_err)?;
            let outcome = kelson_mysql::pairs::reset(&mut *conn, &root).await;
            let released = kelson_mysql::release_migration_lock(&mut conn).await;
            let reverted = outcome.map_err(db_err)?;
            released.map_err(db_err)?;
            reverted
        }
    };
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
    // two phases (session-scoped on both drivers).
    let outcome = match pool {
        DbPool::Postgres(pool) => {
            let mut conn = pool
                .acquire()
                .await
                .map_err(|e| anyhow::anyhow!("migrate: connect: {e}"))?;
            kelson_postgres::acquire_migration_lock(&mut conn)
                .await
                .map_err(db_err)?;
            let outcome = match kelson_postgres::pairs::rollback(&mut *conn, &root, steps).await {
                Err(e) => Err(e),
                Ok(reverted) => match kelson_postgres::run_migrations_held(&mut conn, &root).await {
                    Ok(pair_applied) => Ok((reverted, pair_applied)),
                    Err(e) => Err(e),
                },
            };
            let released = kelson_postgres::release_migration_lock(&mut conn).await;
            let outcome = outcome.map_err(db_err)?;
            released.map_err(db_err)?;
            outcome
        }
        DbPool::MySql(pool) => {
            let mut conn = pool
                .acquire()
                .await
                .map_err(|e| anyhow::anyhow!("migrate: connect: {e}"))?;
            kelson_mysql::acquire_migration_lock(&mut conn)
                .await
                .map_err(db_err)?;
            let outcome = match kelson_mysql::pairs::rollback(&mut *conn, &root, steps).await {
                Err(e) => Err(e),
                Ok(reverted) => match kelson_mysql::run_migrations_held(&mut conn, &root).await {
                    Ok(pair_applied) => Ok((reverted, pair_applied)),
                    Err(e) => Err(e),
                },
            };
            let released = kelson_mysql::release_migration_lock(&mut conn).await;
            let outcome = outcome.map_err(db_err)?;
            released.map_err(db_err)?;
            outcome
        }
    };
    let (reverted, pair_applied) = outcome;
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
    // Same table name and columns on both drivers (B-31 Phase C): the
    // bookkeeping table is driver-owned, the statement is identical.
    match &pool {
        DbPool::Postgres(p) => kelson_postgres::pairs::install(p).await.map_err(db_err)?,
        DbPool::MySql(p) => kelson_mysql::pairs::install(p).await.map_err(db_err)?,
    }
    println!(
        "{} bookkeeping table ready (no migration files run).",
        kelson_postgres::pairs::PAIRS_TABLE
    );
    Ok(ExitCode::SUCCESS)
}

// --------------------------------------------------------------------- seed

async fn seed(only: Option<String>) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let only: Option<Vec<String>> = only.map(|list| {
        list.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    });
    // B-31: driver-selected registry — each fabric runs its own seeder set
    // (the SeedReport types are per-crate, so the pass normalizes the two
    // `(inserted, skipped)` pairs before rendering).
    let results: Vec<(String, (u64, u64))> = match &pool {
        DbPool::Postgres(p) => {
            let mut registry = kelson_postgres::SeederRegistry::new();
            registry
                .register(Arc::new(kelson_postgres::DevTenantSeeder))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            registry
                .run(p, only.as_deref())
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .into_iter()
                .map(|(name, report)| (name, (report.inserted, report.skipped)))
                .collect()
        }
        DbPool::MySql(p) => {
            let mut registry = kelson_mysql::SeederRegistry::new();
            registry
                .register(Arc::new(kelson_mysql::DevTenantSeeder))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            registry
                .run(p, only.as_deref())
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .into_iter()
                .map(|(name, report)| (name, (report.inserted, report.skipped)))
                .collect()
        }
    };
    for (name, (inserted, skipped)) in &results {
        println!("  {name}: inserted={inserted} skipped={skipped}");
    }
    println!("seed complete ({} seeder(s)).", results.len());
    Ok(ExitCode::SUCCESS)
}

// --------------------------------------------------------- config and keys

fn key_generate() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let key = kelson_config::keys::generate_app_key();
    let cfg_path = config_path(&root);
    let dir = kelson_config::dotenv::config_dir_of(&cfg_path);
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
    let artifact = kelson_registry::ConfigCacheArtifact {
        abi: kelson_registry::CONFIG_CACHE_ABI,
        config_hash: kelson_registry::config_hash(&json),
        config: json,
    };
    let path = kelson_registry::config_cache_path(root);
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

/// Phase 3 §4.1: validate + report for the hot-reload path.
fn config_reload() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    config_reload_at(&root)
}

/// Testable core of config:reload against an explicit workspace root:
/// schema-gate the load (invalid config errors out — exit non-zero),
/// hash the effective config, and diff against the pinned artifact.
fn config_reload_at(root: &Path) -> anyhow::Result<ExitCode> {
    let started = std::time::Instant::now();
    let cfg = load_config(root)?.freeze();
    let json = cfg.cache_json();
    let hash = kelson_registry::config_hash(&json);
    let elapsed = started.elapsed();
    println!("config valid (schema gate passed) in {}ms", elapsed.as_millis());
    println!("  effective hash: {hash}");
    let path = kelson_registry::config_cache_path(root);
    match kelson_registry::ConfigCacheArtifact::load(&path)
        .map_err(|e| anyhow::anyhow!("config cache unreadable: {e}"))?
    {
        None => {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            println!("  pin: absent ({}) — boot and reloads read the live config", rel.display());
        }
        Some(artifact) => {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            if artifact.config_hash == hash {
                println!("  pin: {rel} matches — running servers already serve this revision", rel = rel.display());
            } else {
                println!(
                    "  pin: {} is STALE (pinned {}, live {hash}) — re-run config:cache to re-pin, or config:clear to drop it",
                    rel.display(),
                    artifact.config_hash
                );
            }
        }
    }
    println!("reload report done — a running server swaps to this revision without a restart.");
    Ok(ExitCode::SUCCESS)
}

/// Testable core of config:clear against an explicit workspace root.
fn config_clear_at(root: &Path) -> anyhow::Result<ExitCode> {
    let path = kelson_registry::config_cache_path(root);
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

/// Known top-level config sections for `kelson config:show` lookups.
const CONFIG_SECTIONS: &[&str] = &[
    "app", "database", "auth", "otp", "storage", "limits", "telemetry", "views", "http", "mail",
    "i18n", "security", "identity",
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

/// Wave 4 Phase 5 (`kelson env`): environment summary — the config file the
/// layering resolved, redacted key facts and the config-cache pin state.
fn env_summary() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    print!("{}", env_summary_text(&root)?);
    Ok(ExitCode::SUCCESS)
}

/// Testable core of `kelson env` (returns the summary instead of printing).
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
            kelson_config::mask_url_password(&db.url),
            db.max_connections,
            db.run_migrations
        )),
        None => out.push_str("database: not configured\n"),
    }
    out.push_str(&format!("otp: {}\n", cfg.otp.provider));
    let disks = if cfg.storage.disks.is_empty() {
        "(none)".to_string()
    } else {
        cfg.storage.disks.keys().cloned().collect::<Vec<_>>().join(",")
    };
    out.push_str(&format!(
        "storage: endpoint={} bucket={} default={} disks={}\n",
        cfg.storage.endpoint, cfg.storage.bucket, cfg.storage.default, disks
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
        "i18n: default={} paths={}\n",
        cfg.i18n.default,
        cfg.i18n.paths.join(",")
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
    let cache_path = kelson_registry::config_cache_path(root);
    match kelson_registry::ConfigCacheArtifact::load(&cache_path)
        .map_err(|e| anyhow::anyhow!("{e}"))?
    {
        Some(artifact) => out.push_str(&format!(
            "config cache: present (hash {})\n",
            artifact.config_hash
        )),
        None => out.push_str("config cache: not cached (run kelson config:cache)\n"),
    }
    Ok(out)
}

/// Redaction state of one secret for the `kelson env` summary.
fn secret_state(value: &str) -> &'static str {
    if value.is_empty() {
        "empty"
    } else if kelson_config::keys::is_placeholder(value) {
        "placeholder"
    } else {
        "set"
    }
}

// -------------------------------------------------------- cache:forget (Wave 4)

/// Wave 4 Phase 5 (`kelson cache:forget <key>`): remove exactly one cache key.
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
            forget_artifact(root, kelson_registry::route_cache_path(root), key)
        }
        "config" => {
            forget_artifact(root, kelson_registry::config_cache_path(root), key)
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

/// Wave 5: `kelson optimize` warms every boot-pinning artifact in the
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

/// Wave 5: `kelson optimize:clear` removes all three boot-pinning artifacts.
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
/// of the boot conversion in `kelson-demo`. The dependency matrix forbids
/// `application -> web`, so each consumer maps the plain-data section
/// itself (same rules: relative paths resolve against the workspace root,
/// empty paths fall back to resources/views).
fn views_engine(root: &Path) -> anyhow::Result<kelson_views::ViewsEngine> {
    let cfg = load_config(root)?;
    let env = cfg.app.env.as_str();
    Ok(kelson_views::ViewsEngine::new(views_config(
        &cfg.views,
        root,
        env,
    )))
}

fn views_config(
    section: &kelson_config::ViewsSection,
    root: &Path,
    env: &str,
) -> kelson_views::ViewsConfig {
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
    kelson_views::ViewsConfig {
        paths,
        cache_dir: section.cache_dir.as_deref().map(resolve),
        env: section.env.clone().unwrap_or_else(|| env.to_string()),
    }
}

// -------------------------------------------------------------- maintenance

fn up() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    if kelson_http::read_down_file(&root).is_some() {
        kelson_http::remove_down_file(&root).map_err(|e| anyhow::anyhow!("{e}"))?;
        println!("application is now UP (maintenance file removed).");
    } else {
        println!("application was already up.");
    }
    Ok(ExitCode::SUCCESS)
}

fn down(retry: Option<u64>, secret: Option<String>) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let file = kelson_http::MaintenanceFile {
        retry_secs: retry,
        secret,
        started_at: Some(time::OffsetDateTime::now_utc()),
    };
    let has_secret = file.secret.is_some();
    kelson_http::write_down_file(&root, &file).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "application is DOWN (retry_after={:?}, secret={}).",
        retry,
        if has_secret { "set" } else { "none" }
    );
    Ok(ExitCode::SUCCESS)
}

// -------------------------------------------------------------------- jobs

fn parse_job_status(raw: &str) -> anyhow::Result<kelson_jobs::JobStatus> {
    use kelson_jobs::JobStatus;
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

fn print_jobs(jobs: &[kelson_jobs::DurableJob]) {
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
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let status = match status {
        Some(raw) => Some(parse_job_status(&raw)?),
        None => None,
    };
    let store = job_store_admin(&pool);
    let jobs = store
        .list(status, limit)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    print_jobs(&jobs);
    Ok(ExitCode::SUCCESS)
}

async fn queue_failed() -> anyhow::Result<ExitCode> {
    use kelson_jobs::JobStatus;
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let store = job_store_admin(&pool);
    let jobs = store
        .list(Some(JobStatus::Dead), 50)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("dead jobs awaiting manual retry:");
    print_jobs(&jobs);
    Ok(ExitCode::SUCCESS)
}

async fn queue_retry(id: &str) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let job_id = uuid::Uuid::parse_str(id).map_err(|e| anyhow::anyhow!("invalid job id: {e}"))?;
    let pool = db_pool(&root).await?;
    let store = job_store_admin(&pool);
    store.requeue(job_id).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("job {job_id} requeued (pending).");
    Ok(ExitCode::SUCCESS)
}

/// Shared worker-binary resolution for queue:work and queue:listen: the
/// kelson-worker next to this kelson executable.
fn worker_binary() -> anyhow::Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or_else(|| anyhow::anyhow!("cannot resolve executable directory"))?;
    let binary = if cfg!(windows) {
        "kelson-worker.exe"
    } else {
        "kelson-worker"
    };
    let worker = dir.join(binary);
    if !worker.exists() {
        anyhow::bail!("kelson-worker not found next to kelson ({binary}); build it with: cargo build -p kelson-worker");
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
    let store = job_store_admin(&pool);
    let purged = store.purge_pending().map_err(db_err)?;
    println!("queue:clear: {purged} pending job(s) purged.");
    Ok(ExitCode::SUCCESS)
}

/// Wave 5: delete every dead job (forensic rows are gone afterwards —
/// queue:prune-failed is the age-bounded variant).
async fn queue_flush() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let store = job_store_admin(&pool);
    let pruned = store
        .prune_dead(kelson_jobs::flush_cutoff())
        .map_err(db_err)?;
    println!("queue:flush: {pruned} dead job(s) deleted.");
    Ok(ExitCode::SUCCESS)
}

/// Wave 5: delete one dead job (unknown id or not dead = refusal).
async fn queue_forget(id: &str) -> anyhow::Result<ExitCode> {
    let job_id = uuid::Uuid::parse_str(id).map_err(|e| anyhow::anyhow!("invalid job id: {e}"))?;
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let store = job_store_admin(&pool);
    if !store.delete_dead(job_id).map_err(db_err)? {
        anyhow::bail!("job {job_id} is not a dead job (nothing forgotten)");
    }
    println!("job {job_id} forgotten.");
    Ok(ExitCode::SUCCESS)
}

/// Wave 5: queue depth + oldest pending age snapshot.
async fn queue_monitor() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let store = job_store_admin(&pool);
    let snap = store.snapshot().map_err(db_err)?;
    print!(
        "{}",
        render_queue_monitor(&snap, time::OffsetDateTime::now_utc())
    );
    Ok(ExitCode::SUCCESS)
}

/// Render queue:monitor output (pure — unit-tested without a store).
fn render_queue_monitor(snap: &kelson_jobs::QueueSnapshot, now: time::OffsetDateTime) -> String {
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
    let hours = i64::try_from(hours).map_err(|_| anyhow::anyhow!("--hours out of range"))?;
    if hours < 1 {
        anyhow::bail!("--hours must be at least 1");
    }
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let store = job_store_admin(&pool);
    let cutoff = time::OffsetDateTime::now_utc() - time::Duration::hours(hours);
    let pruned = store.prune_dead(cutoff).map_err(db_err)?;
    println!("queue:prune-failed: {pruned} dead job(s) older than {hours}h deleted.");
    Ok(ExitCode::SUCCESS)
}

/// Wave 5/6 P9: prune terminal batch records older than --hours (default
/// 72). The durable ledger (B-32) ages finished records out strictly
/// before the cutoff; unfinished batches are never pruned, and the count
/// printed is what the DELETE actually removed.
async fn queue_prune_batches(hours: u64) -> anyhow::Result<ExitCode> {
    let hours = i64::try_from(hours).map_err(|_| anyhow::anyhow!("--hours out of range"))?;
    if hours < 1 {
        anyhow::bail!("--hours must be at least 1");
    }
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let ledger = batch_ledger(&pool);
    let cutoff = time::OffsetDateTime::now_utc() - time::Duration::hours(hours);
    let pruned = ledger.prune(cutoff).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("queue:prune-batches: pruned {pruned} batch record(s) (cutoff {hours}h).");
    Ok(ExitCode::SUCCESS)
}

/// Wave 5: write the restart signal file running workers honor.
fn queue_restart() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let path = kelson_jobs::write_queue_restart_signal(&root)
        .map_err(|e| anyhow::anyhow!("queue:restart signal write: {e}"))?;
    let rel = path.strip_prefix(&root).unwrap_or(&path);
    println!("  + {}", rel.display());
    println!(
        "restart signal written; running workers stop after their current job (supervisors respawn them — see queue:listen)."
    );
    Ok(ExitCode::SUCCESS)
}

/// Wave 5: supervise kelson-worker — respawn on every exit with a crash-loop
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

/// Wave 5/6 P9: requeue every failed/dead member of a batch — the durable
/// ledger (B-32) carries membership, the job store carries member state,
/// and the operation is the very `BatchOps::retry` the HTTP
/// `POST /api/v1/batches/{id}/retry` runs. The id validates first so a
/// typo fails before any connection is attempted.
async fn queue_retry_batch(id: &str) -> anyhow::Result<ExitCode> {
    use kelson_jobs::BatchOps;
    let batch_id =
        uuid::Uuid::parse_str(id).map_err(|e| anyhow::anyhow!("invalid batch id: {e}"))?;
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let store = job_store_admin(&pool);
    let ledger = batch_ledger(&pool);
    let outcome = BatchOps::new(&*ledger, &*store)
        .retry(batch_id)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "queue:retry-batch {id}: requeued {} failed member(s) ({} pending, {} running, {} succeeded, {} missing).",
        outcome.requeued, outcome.pending, outcome.running, outcome.succeeded, outcome.missing
    );
    Ok(ExitCode::SUCCESS)
}

// ---------------------------------------------------------------- schedules

fn schedule_info(command: &str) -> anyhow::Result<ExitCode> {
    println!("{command}: no module schedules registered yet.");
    println!("Schedules are declared by modules through ScheduleRegistry (kelson-scheduling); the framework surface is in place, module content lands with S3.");
    Ok(ExitCode::SUCCESS)
}

/// The registry `schedule:work`/`schedule:test` consume (docs/19). Schedule
/// items are registered by module boot code through the scheduler service;
/// the CLI source starts EMPTY — no module registers schedules yet — the same
/// honest posture as `schedule:list` and `kelson model:prune` ("none"). This
/// function is the single seam where a future declarative source lands.
fn schedule_registry(
    _root: &Path,
) -> Result<kelson_scheduling::ScheduleRegistry, kelson_foundation::AppError> {
    Ok(kelson_scheduling::ScheduleRegistry::new())
}

/// What the `schedule:test` dry run would do: everything registered plus the
/// subset due at `now` against the persisted last-fired state. Pure over the
/// registry (the crate documents `due` as a pure function of items, state
/// and now) — nothing fires, no state is written.
fn schedule_test_plan(
    registry: &kelson_scheduling::ScheduleRegistry,
    state: &[(String, time::OffsetDateTime)],
    now: time::OffsetDateTime,
) -> Result<SchedulePlan, kelson_foundation::AppError> {
    let due = registry
        .due(state, now)?
        .into_iter()
        .map(|item| (item.name.clone(), item.operation.clone()))
        .collect();
    Ok(SchedulePlan {
        registered: registry.names().into_iter().map(str::to_string).collect(),
        due,
    })
}

/// The plan rendered for the terminal (pure — asserted directly by tests).
/// Disabled items appear in the registry list but never in the due set.
fn schedule_test_lines(plan: &SchedulePlan) -> Vec<String> {
    if plan.registered.is_empty() {
        return vec![
            "schedule:test: no schedules registered yet (nothing would fire).".to_string(),
        ];
    }
    let mut lines = vec![format!(
        "schedule:test: {} registered, {} due now.",
        plan.registered.len(),
        plan.due.len()
    )];
    for (name, operation) in &plan.due {
        lines.push(format!("  would fire: {name} -> {operation}"));
    }
    let not_due = plan
        .registered
        .iter()
        .filter(|name| !plan.due.iter().any(|(d, _)| d == *name))
        .cloned()
        .collect::<Vec<_>>();
    if !not_due.is_empty() {
        lines.push(format!("  not due: {}", not_due.join(", ")));
    }
    lines
}

/// Registered names + due-now (name, operation) pairs: the dry-run answer.
struct SchedulePlan {
    registered: Vec<String>,
    due: Vec<(String, String)>,
}

/// Wave 5: `kelson schedule:test` — print what would fire now without
/// executing (reads the persisted state, executes nothing, writes nothing).
fn schedule_test() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let registry = schedule_registry(&root).map_err(db_err)?;
    let state = kelson_scheduling::load_state(&root).map_err(db_err)?;
    let plan = schedule_test_plan(&registry, &state, time::OffsetDateTime::now_utc())
        .map_err(db_err)?;
    for line in schedule_test_lines(&plan) {
        println!("{line}");
    }
    Ok(ExitCode::SUCCESS)
}

/// One `schedule:work` pass: fire every due schedule as a durable job — the
/// worker's L35 semantics (`item.operation`, `max_attempts = 1`, the item's
/// payload; `queue:work` processes it) — then persist last-fired state so a
/// restart or a `schedule:test` does not re-fire.
fn schedule_tick(
    runner: &kelson_scheduling::SchedulerRunner,
    store: &dyn kelson_jobs::JobStore,
    root: &Path,
    now: time::OffsetDateTime,
) -> Result<Vec<String>, kelson_foundation::AppError> {
    let failures_seen = runner.failures().len();
    let fired = runner.tick(now, |item| {
        let mut def = kelson_jobs::JobDef::new(item.operation.clone());
        def.max_attempts = 1;
        store.enqueue(kelson_jobs::build_from_def(&def, item.payload.clone(), now))?;
        Ok(())
    })?;
    if !fired.is_empty() || runner.failures().len() > failures_seen {
        // Wave 6 P4: persists last-fired AND failures — a failing item is
        // recorded by the tick, never fatal to the pass.
        kelson_scheduling::save_state_doc(root, &runner.state_doc())?;
    }
    Ok(fired)
}

/// Wave 5: `kelson schedule:work` — the scheduler daemon (docs/19: tick every
/// 60s, run due items). With nothing registered it reports the empty posture
/// and exits before touching the database — a daemon over an empty registry
/// would only spin (the `schedule:list`/`model:prune` "none" shape).
/// Otherwise it ticks forever, firing due items as durable jobs and saving
/// state after every firing pass; Ctrl-C stops the loop.
fn schedule_work(every: u64) -> anyhow::Result<ExitCode> {
    rt(async move {
        let root = workspace_root()?;
        let registry = schedule_registry(&root).map_err(db_err)?;
        let count = registry.names().len();
        if count == 0 {
            println!("schedule:work: no schedules registered yet — nothing to fire.");
            return Ok(ExitCode::SUCCESS);
        }
        let state = kelson_scheduling::load_state_doc(&root).map_err(db_err)?;
        let runner = kelson_scheduling::SchedulerRunner::with_state_doc(Arc::new(registry), state);
        let pool = db_pool(&root).await?;
        let store = job_store(&pool);
        let every = every.max(1);
        println!("schedule:work: {count} schedule(s) registered; ticking every {every}s (Ctrl-C stops; state saves after each firing pass)");
        loop {
            let now = time::OffsetDateTime::now_utc();
            match schedule_tick(&runner, &*store, &root, now) {
                Ok(fired) if !fired.is_empty() => {
                    println!("  [{}] fired: {}", rfc3339(now), fired.join(", "));
                }
                Ok(_) => {}
                Err(e) => eprintln!("schedule:work: tick failed: {}", db_err(e)),
            }
            tokio::time::sleep(std::time::Duration::from_secs(every)).await;
        }
    })
}

// -------------------------------------------------------------- db inspect

/// Public tables with their row counts (shared by db:inspect and db:show).
/// Wave 9 B-31: pg_catalog on Postgres, information_schema on MySQL — same
/// shape, same ordering, base tables only.
async fn table_row_counts(pool: &DbPool) -> anyhow::Result<Vec<(String, i64)>> {
    let tables: Vec<String> = match pool {
        DbPool::Postgres(p) => {
            sqlx::query_scalar(
                "SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname = 'public' ORDER BY tablename",
            )
            .fetch_all(p)
            .await
        }
        DbPool::MySql(p) => {
            sqlx::query_scalar(
                "SELECT table_name FROM information_schema.tables \
                 WHERE table_schema = DATABASE() AND table_type = 'BASE TABLE' \
                 ORDER BY table_name",
            )
            .fetch_all(p)
            .await
        }
    }
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut out = Vec::with_capacity(tables.len());
    for table in tables {
        let count: i64 = match pool {
            DbPool::Postgres(p) => {
                sqlx::query_scalar(&format!("SELECT COUNT(*) FROM \"{table}\""))
                    .fetch_one(p)
                    .await
            }
            DbPool::MySql(p) => {
                sqlx::query_scalar(&format!("SELECT COUNT(*) FROM `{table}`"))
                    .fetch_one(p)
                    .await
            }
        }
        .map_err(|e| anyhow::anyhow!("count {table}: {e}"))?;
        out.push((table, count));
    }
    Ok(out)
}

async fn db_inspect() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let counts = table_row_counts(&pool).await?;
    // B-31: MySQL's schema vocabulary is the current database, not a
    // `public` schema; the Postgres line stays byte-identical.
    let scope = if matches!(pool, DbPool::MySql(_)) {
        "current database"
    } else {
        "public schema"
    };
    println!("{scope}: {} table(s)", counts.len());
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
    let server = server_version(&pool).await?;
    let database = current_database(&pool).await?;
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
async fn fetch_schema_tables(pool: &kelson_postgres::PgPool) -> anyhow::Result<Vec<SchemaTable>> {
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

/// One table of the MySQL snapshot: the server's own `SHOW CREATE TABLE`
/// statement (B-31 Phase E). MySQL has no catalog function that
/// reconstructs table DDL, and `information_schema` cannot express
/// generated-column expressions, so the server's rendering is the source —
/// `mysqldump` and Laravel's `schema:dump` read the same bytes (the
/// render fn applies the two byte-stability normalizations).
struct MySqlSchemaTable {
    name: String,
    create_sql: String,
}

/// Base tables of the current database — the same inventory `db:show`
/// reports through information_schema.
const MYSQL_SCHEMA_TABLES_SQL: &str = r#"SELECT table_name
  FROM information_schema.tables
 WHERE table_schema = DATABASE() AND table_type = 'BASE TABLE'
 ORDER BY table_name"#;

/// Fetch the MySQL schema: the connected database's name plus one
/// `SHOW CREATE TABLE` per base table, keyed through a BTreeMap so the
/// render order is byte-deterministic (the same ordering rule as the
/// Postgres renderer). `SHOW CREATE TABLE` is preparable on MySQL 8.0.44
/// (probed — the same COM_STMT_PREPARE path sqlx's `query_as` takes).
async fn fetch_mysql_schema_tables(
    pool: &kelson_mysql::MySqlPool,
) -> anyhow::Result<(String, Vec<MySqlSchemaTable>)> {
    let database: String = sqlx::query_scalar("SELECT DATABASE()")
        .fetch_one(pool)
        .await
        .map_err(|e| anyhow::anyhow!("current database: {e}"))?;
    let names: Vec<String> = sqlx::query_scalar(MYSQL_SCHEMA_TABLES_SQL)
        .fetch_all(pool)
        .await
        .map_err(|e| anyhow::anyhow!("schema tables: {e}"))?;
    let mut statements: std::collections::BTreeMap<String, String> =
        std::collections::BTreeMap::new();
    for name in names {
        let row: (String, String) = sqlx::query_as(&format!("SHOW CREATE TABLE `{name}`"))
            .fetch_one(pool)
            .await
            .map_err(|e| anyhow::anyhow!("show create table {name}: {e}"))?;
        statements.insert(name, row.1);
    }
    let tables = statements
        .into_iter()
        .map(|(name, create_sql)| MySqlSchemaTable { name, create_sql })
        .collect();
    Ok((database, tables))
}

/// Wave 5: `db:schema:dump` — normalized DDL snapshot of the migrated
/// schema, written to database/schema.sql for review. The file is never
/// executed by migrate (migrations stay the source of truth).
/// B-31 Phase E: the snapshot dispatches on driver — Postgres renders the
/// pg_catalog reconstruction, MySQL renders the server's own
/// `SHOW CREATE TABLE` statements (docs/05-DATABASE); same banner, same
/// layout, each dialect's own DDL (a Postgres dump never applies to MySQL
/// — ADR 0010 non-goal).
async fn db_schema_dump() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let (snapshot, table_count) = match pool {
        DbPool::Postgres(pool) => {
            let tables = fetch_schema_tables(&pool).await?;
            (render_schema_snapshot(&tables), tables.len())
        }
        DbPool::MySql(pool) => {
            let (database, tables) = fetch_mysql_schema_tables(&pool).await?;
            (render_mysql_schema_snapshot(&database, &tables), tables.len())
        }
    };
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
        table_count
    );
    println!("database/schema.sql written — review artifact; migrate never executes it.");
    Ok(ExitCode::SUCCESS)
}

/// Render the normalized snapshot (pure — unit-tested without a database):
/// tables sorted by name, one statement per table, PK inline, secondary
/// indexes after their table, deterministic formatting.
fn render_schema_snapshot(tables: &[SchemaTable]) -> String {
    let mut out = String::new();
    out.push_str("-- kelson normalized schema snapshot (generated by `kelson db:schema:dump`)\n");
    out.push_str("-- Review artifact: `kelson migrate` never executes this file.\n");
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

/// Strip the data-dependent `AUTO_INCREMENT=<n>` table option so the
/// snapshot depends on the schema alone (defensive: 0/25 shipped tables
/// carry an AUTO_INCREMENT column — probed on 8.0.44 — but a future
/// migration could). The clause only appears in the table options,
/// space-separated; any other continuation after `=` is kept verbatim
/// (never guess at DDL bytes we do not recognize).
fn strip_auto_increment(create_sql: &str) -> String {
    let mut out = String::with_capacity(create_sql.len());
    let mut rest = create_sql;
    while let Some(pos) = rest.find(" AUTO_INCREMENT=") {
        let (head, tail) = rest.split_at(pos);
        let after = &tail[" AUTO_INCREMENT=".len()..];
        out.push_str(head);
        if !after.starts_with(|c: char| c.is_ascii_digit()) {
            out.push_str(" AUTO_INCREMENT=");
            rest = after;
            continue;
        }
        let digits = after
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(after.len());
        rest = &after[digits..];
    }
    out.push_str(rest);
    out
}

/// Strip column-level `CHARACTER SET`/`COLLATE` clauses that merely repeat
/// the statement's own table options, so the snapshot replays to bytes the
/// server renders back unchanged. MySQL's `SHOW CREATE TABLE` is not a
/// round trip: a column that inherited the table charset renders as
/// ``x COLLATE <table collation>`` (the two bookkeeping tables'
/// `utf8mb4_unicode_ci`), and replaying *that* text marks the column
/// explicit — the next render expands it to `CHARACTER SET utf8mb4
/// COLLATE ...`, a one-shot difference on every re-apply (probed on
/// 8.0.44: inherited -> COLLATE-only -> explicit -> fixed point).
/// Restoring the inherited form is semantics-preserving (charset and
/// collation come from the table) and makes dump -> replay -> dump
/// byte-identical. Only backtick-quoted column lines are touched; the
/// options line and any collation differing from the table's stay verbatim.
fn strip_table_redundant_collation(create_sql: &str) -> String {
    let table_collation = match create_sql.find("COLLATE=") {
        Some(pos) => {
            let rest = &create_sql[pos + "COLLATE=".len()..];
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(rest.len());
            &rest[..end]
        }
        None => return create_sql.to_string(),
    };
    if table_collation.is_empty() {
        return create_sql.to_string();
    }
    let collate_token = format!("COLLATE {table_collation}");
    let mut out = String::with_capacity(create_sql.len());
    for line in create_sql.split_inclusive('\n') {
        // Column definitions only: two-space indent + backtick-quoted name.
        // Options, KEY and CONSTRAINT lines never match.
        if !line.starts_with("  `") {
            out.push_str(line);
            continue;
        }
        let stripped = match line.find(&collate_token) {
            Some(pos) => {
                let after = pos + collate_token.len();
                let boundary = line[after..]
                    .chars()
                    .next()
                    .map(|c| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(true);
                if !boundary {
                    out.push_str(line);
                    continue;
                }
                // Include `CHARACTER SET <charset> ` when it precedes the collation.
                let mut start = pos;
                if let Some(cs) = line[..pos].rfind("CHARACTER SET ") {
                    let between = &line[cs + "CHARACTER SET ".len()..pos];
                    let adjacent = between.ends_with(' ')
                        && between[..between.len() - 1]
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_');
                    if adjacent {
                        start = cs;
                    }
                }
                // Consume exactly one separating space to the left.
                if start > 0 && line.as_bytes()[start - 1] == b' ' {
                    start -= 1;
                }
                format!("{}{}", &line[..start], &line[after..])
            }
            None => line.to_string(),
        };
        out.push_str(&stripped);
    }
    out
}

/// Render the MySQL snapshot (pure — unit-tested without a database): the
/// same banner and layout contract as the Postgres renderer — tables sorted
/// by name, one statement per table, `;`-terminated, a blank line after
/// each table — where each statement is the server's `SHOW CREATE TABLE`
/// output narrowed by exactly two byte-stability normalizations
/// ([`strip_auto_increment`], [`strip_table_redundant_collation`]) so
/// dump -> replay -> dump is byte-identical. The dialect line records
/// provenance: this file is MySQL DDL, and a Postgres snapshot never
/// applies to it (ADR 0010 non-goal).
fn render_mysql_schema_snapshot(database: &str, tables: &[MySqlSchemaTable]) -> String {
    let mut out = String::new();
    out.push_str("-- kelson normalized schema snapshot (generated by `kelson db:schema:dump`)\n");
    out.push_str("-- Review artifact: `kelson migrate` never executes this file.\n");
    out.push_str(
        "-- mysql dialect: SHOW CREATE TABLE per table (AUTO_INCREMENT counter \
         / table-redundant column COLLATE stripped - byte-stable replay).\n",
    );
    if tables.is_empty() {
        out.push_str(&format!("-- (no tables in database `{database}`)\n"));
        return out;
    }
    let mut sorted: Vec<&MySqlSchemaTable> = tables.iter().collect();
    sorted.sort_by(|a, b| a.name.cmp(&b.name));
    for table in sorted {
        let stmt = strip_auto_increment(table.create_sql.trim_end());
        let stmt = strip_table_redundant_collation(&stmt);
        let stmt = stmt.strip_suffix(';').unwrap_or(stmt.as_str());
        out.push_str(stmt);
        out.push_str(";\n\n");
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

/// Where `kelson new` gets its template files from.
///
/// A checkout is preferred whenever its templates are actually present, so a
/// contributor editing one sees the edit on the next run instead of a stale
/// copy baked into the binary. The compiled-in copy is the fallback, and it is
/// the reason this console can be published at all: a binary installed from
/// crates.io has no checkout beside it, and reading templates off disk would
/// leave it generating nothing.
enum TemplateSource {
    /// `<workspace>/templates/...`, read at generation time.
    Disk(PathBuf),
    /// Compiled in by the `kelson-templates` crate.
    Embedded(&'static [kelson_templates::TemplateFile]),
}

impl TemplateSource {
    /// Every file of the template as (project-relative path, body), sorted by
    /// path. Both arms sort, so the two sources produce the same project in
    /// the same order.
    fn files(&self) -> anyhow::Result<Vec<(String, String)>> {
        let mut out: Vec<(String, String)> = match self {
            TemplateSource::Disk(dir) => {
                let mut paths = Vec::new();
                walk_files(dir, &mut paths)?;
                paths.sort();
                let mut files = Vec::with_capacity(paths.len());
                for path in paths {
                    let rel = path.strip_prefix(dir)?.to_string_lossy().replace('\\', "/");
                    files.push((rel, std::fs::read_to_string(&path)?));
                }
                files
            }
            TemplateSource::Embedded(entries) => entries
                .iter()
                .map(|(rel, body)| ((*rel).to_string(), (*body).to_string()))
                .collect(),
        };
        out.sort();
        // A checkout's line endings belong to whoever last edited it, and the
        // compiled-in copy's belong to whoever built the binary - autocrlf
        // decides, and it differs between machines. Normalising here means the
        // same template produces the same bytes wherever it came from, and
        // that a Windows checkout and a Linux one generate identical projects.
        for (_, body) in &mut out {
            if body.contains('\r') {
                *body = body.replace("\r\n", "\n");
            }
        }
        Ok(out)
    }
}

/// Pick the source for one template.
///
/// Disk wins only when the directory is really there; everything else - an
/// installed console, a checkout stripped of its templates - falls back to the
/// copy compiled into the binary.
fn template_source(workspace: Option<&Path>, with_starter: bool) -> TemplateSource {
    let rel = if with_starter {
        "templates/starter-template"
    } else {
        "templates/project-template"
    };
    if let Some(root) = workspace {
        let dir = root.join(rel);
        if dir.is_dir() {
            return TemplateSource::Disk(dir);
        }
    }
    TemplateSource::Embedded(if with_starter {
        kelson_templates::STARTER_TEMPLATE
    } else {
        kelson_templates::PROJECT_TEMPLATE
    })
}

/// Where a generated project lands, and how its manifest expresses its
/// dependencies.
enum ProjectLayout<'a> {
    /// Existing behaviour: `applications/<name>` inside the workspace, joined
    /// to it as a member, manifest inheriting via `{ workspace = true }`.
    Workspace,
    /// Phase Q: `./<name>` beside the cwd, self-contained - its own
    /// `[workspace]` table plus concrete dependency specs pointing at
    /// `checkout`, so the crate builds without this workspace.
    Standalone { checkout: &'a Path },
}

/// kelson new: materialize templates/project-template into applications/<name>,
/// or - with --standalone - a self-contained project beside the current
/// directory that needs no enclosing workspace (Phase Q).
fn new_project(
    name: &str,
    plane: &str,
    description: &str,
    with_starter: bool,
    standalone: bool,
    deps_root: Option<&str>,
) -> anyhow::Result<ExitCode> {
    let description = if description.is_empty() {
        format!("Project {name}")
    } else {
        description.to_string()
    };
    let spec = kelson_codegen::ModuleSpec {
        name: name.to_string(),
        plane: plane.to_string(),
        description: description.clone(),
    };
    spec.validate().map_err(|e| anyhow::anyhow!("{e}"))?;

    // The in-workspace layout joins the workspace it is generated into, so one
    // is required there. `--standalone` is the layout that does not need one -
    // that is most of its point - so an installed console run in a directory
    // with nothing above it still produces a project.
    let workspace = workspace_root().ok();
    if !standalone && workspace.is_none() {
        anyhow::bail!(
            "no workspace Cargo.toml found above {}. `kelson new` without --standalone \
             adds a module to a kelson workspace, so it has to run inside one; pass \
             --standalone to generate a self-contained project here instead.",
            std::env::current_dir()?.display()
        );
    }

    // A standalone project still needs somewhere to take its kelson
    // dependencies from (they are not on crates.io until the publish phase
    // lands); --deps-root selects a checkout, otherwise the enclosing
    // workspace is used.
    let standalone_checkout;
    let target_root;
    let layout;
    if standalone {
        standalone_checkout = match deps_root {
            Some(raw) => {
                let p = PathBuf::from(raw);
                let p = p.canonicalize().map_err(|e| {
                    anyhow::anyhow!(
                        "--deps-root {} is not a readable directory: {e}",
                        p.display()
                    )
                })?;
                if !p.join("Cargo.toml").exists() {
                    anyhow::bail!(
                        "--deps-root {} has no Cargo.toml; point it at a kelson checkout",
                        p.display()
                    );
                }
                p
            }
            None => match &workspace {
                Some(root) => root.clone(),
                // Refusing rather than emitting version-only specs is
                // deliberate: those crates are not published yet, so the
                // manifest would parse and then fail to build on the very
                // machine it was meant to help.
                None => anyhow::bail!(
                    "--standalone needs somewhere to take the kelson dependencies from: \
                     pass --deps-root <path to a kelson checkout>. They are not on \
                     crates.io yet, so a manifest pointing at neither would not build."
                ),
            },
        };
        target_root = std::env::current_dir()?.join(name);
        layout = ProjectLayout::Standalone {
            checkout: &standalone_checkout,
        };
    } else {
        // The check above guarantees this. `standalone_checkout` is left
        // uninitialised on this path: only the standalone layout borrows it.
        let root = workspace
            .as_ref()
            .expect("the in-workspace layout requires a workspace (checked above)");
        target_root = root.join("applications").join(name);
        layout = ProjectLayout::Workspace;
    }
    if target_root.exists() {
        anyhow::bail!("{} already exists", target_root.display());
    }

    // Where each relative path in the "created" listing is anchored.
    let list_base: &Path = if standalone {
        &target_root
    } else {
        workspace
            .as_ref()
            .expect("the in-workspace layout requires a workspace (checked above)")
    };

    let source = template_source(workspace.as_deref(), with_starter);
    let crate_name = format!("kelson-{name}");
    let mut written = Vec::new();
    for (rel, raw) in source.files()? {
        let mut content = raw
            .replace("__PROJECT_NAME__", name)
            .replace("__PROJECT_CRATE__", &crate_name)
            .replace("__PROJECT_DESCRIPTION__", &description);
        // Only the manifest changes shape; every other template file stays
        // byte-for-byte identical between the two layouts.
        if rel == "Cargo.toml" {
            if let ProjectLayout::Standalone { checkout } = &layout {
                content = standalone_manifest(&content, checkout)?;
            }
        }
        let target = target_root.join(&rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&target, content)?;
        written.push(target);
    }
    for path in &written {
        let rel = path.strip_prefix(list_base).unwrap_or(path);
        println!("  + {}", rel.display());
    }
    match &layout {
        ProjectLayout::Workspace => {
            kelson_codegen::add_workspace_member(
                workspace
                    .as_ref()
                    .expect("the in-workspace layout requires a workspace (checked above)"),
                &format!("applications/{name}"),
            )
            .map_err(|e| anyhow::anyhow!("{e}"))?;
            println!("  + workspace member applications/{name}");
        }
        ProjectLayout::Standalone { checkout } => {
            println!(
                "  + standalone project: own [workspace], kelson deps from {}",
                checkout.display()
            );
        }
    }
    println!("project {name} created. Verify: cargo check -p {crate_name}");
    if with_starter {
        println!("starter extras:");
        println!(
            "  - web/index.html + web/app.js — SPA shell talking to /auth/* and your module routes"
        );
        println!("  - module.toml routes are named — generate URLs with the graph and OpenAPI client via: kelson sdk:openapi");
        println!("  - run the loop with: kelson serve");
    }
    Ok(ExitCode::SUCCESS)
}

/// Package fields the templates inherit from `[workspace.package]`. A
/// standalone crate has no workspace to inherit from, so they are spelled out
/// with the platform's own values.
const STANDALONE_PACKAGE: &[(&str, &str)] = &[
    ("version.workspace = true", "version = \"0.1.0\""),
    ("edition.workspace = true", "edition = \"2021\""),
    ("rust-version.workspace = true", "rust-version = \"1.89\""),
    (
        "license.workspace = true",
        "license = \"MIT OR Apache-2.0\"",
    ),
    (
        "repository.workspace = true",
        "repository = \"https://github.com/7d7s/axomarc\"",
    ),
    ("readme.workspace = true", "readme = \"README.md\""),
    (
        "keywords.workspace = true",
        "keywords = [\"kelson\", \"framework\"]",
    ),
];

/// Workspace-relative directories of the kelson crates the templates use, so a
/// standalone manifest can point at them with `path = ...`.
const STANDALONE_KELSON_DEPS: &[(&str, &str)] = &[
    ("kelson-context", "crates/foundation/context"),
    ("kelson-execution", "crates/execution/engine"),
    ("kelson-foundation", "crates/foundation/core"),
    ("kelson-lifecycle", "crates/foundation/lifecycle"),
    ("kelson-models", "crates/data/models"),
    ("kelson-policy", "crates/security/policies"),
    ("kelson-registry", "crates/application/registry"),
];

/// Third-party dependencies, verbatim from `[workspace.dependencies]`, so a
/// standalone crate builds against the versions the platform itself uses.
const STANDALONE_THIRD_PARTY: &[(&str, &str)] = &[
    ("serde", "serde = { version = \"1\", features = [\"derive\"] }"),
    ("serde_json", "serde_json = \"1\""),
    (
        "time",
        "time = { version = \"0.3\", features = [\"serde\", \"formatting\", \"parsing\", \"macros\"] }",
    ),
    (
        "tokio",
        "tokio = { version = \"1\", features = [\"macros\", \"rt-multi-thread\", \"signal\", \"time\"] }",
    ),
    (
        "uuid",
        "uuid = { version = \"1\", features = [\"v4\", \"v5\", \"v7\", \"serde\"] }",
    ),
];

/// TOML-safe path: forward slashes work on every platform and avoid the
/// escaped-backslash mangling a Windows path would otherwise need.
///
/// The Windows verbatim prefix is stripped first. `Path::canonicalize` hands
/// back `\\?\C:\...`, and a manifest carrying that does not parse at all:
/// once the backslashes are swapped it reads `//?/C:/...`, which cargo treats
/// as a URL rather than a path. Left unstripped, every `--deps-root` run on
/// Windows produced a project that would not build. The UNC form
/// `\\?\UNC\server\share` keeps its leading double slash, because that is what
/// a share looks like once the prefix is gone.
fn toml_path(p: &Path) -> String {
    let raw = p.to_string_lossy();
    let unverbatimited = if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = raw.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        raw.to_string()
    };
    unverbatimited.replace('\\', "/")
}

/// The concrete manifest line for one dependency name, or `None` when nothing
/// in the tables above knows how to make it standalone.
fn standalone_dep(name: &str, checkout: &Path) -> Option<String> {
    if let Some((_, dir)) = STANDALONE_KELSON_DEPS.iter().find(|(n, _)| *n == name) {
        return Some(format!(
            "{name} = {{ version = \"0.1.0\", path = \"{}\" }}",
            toml_path(&checkout.join(dir))
        ));
    }
    STANDALONE_THIRD_PARTY
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, spec)| (*spec).to_string())
}

/// Rewrite a workspace-relative manifest into a self-contained one (Phase Q).
///
/// Every `{ workspace = true }` must be resolvable: package fields are spelled
/// out, kelson crates get a `version`/`path` pair aimed at `checkout`,
/// third-party crates get their pinned spec. Anything unresolvable is a hard
/// error rather than a manifest that only fails later on a stranger's machine.
/// A `[workspace]` table is inserted so cargo does not adopt the project into
/// whatever workspace encloses it. The result is checked, not assumed.
fn standalone_manifest(raw: &str, checkout: &Path) -> anyhow::Result<String> {
    let mut out: Vec<String> = Vec::new();
    let mut anchored = false;

    for line in raw.split('\n') {
        let t = line.trim();

        if let Some((_, concrete)) = STANDALONE_PACKAGE.iter().find(|(from, _)| *from == t) {
            out.push((*concrete).to_string());
            continue;
        }

        if t.contains("workspace = true") {
            let name = t.split('=').next().unwrap_or_default().trim();
            let replacement = standalone_dep(name, checkout).ok_or_else(|| {
                anyhow::anyhow!(
                    "cannot make dependency {name:?} standalone: it is declared \
                     `{{ workspace = true }}` but has no known concrete spec. Add it to \
                     STANDALONE_KELSON_DEPS or STANDALONE_THIRD_PARTY in \
                     tools/kelson-console/src/main.rs."
                )
            })?;
            out.push(replacement);
            continue;
        }

        // Anchor the standalone marker on the first top-level section after
        // [package], which is always [dependencies].
        if !anchored && t == "[dependencies]" {
            out.push("[workspace]".to_string());
            out.push(String::new());
            anchored = true;
        }
        out.push(line.to_string());
    }

    if !anchored {
        anyhow::bail!("template manifest has no [dependencies] section to anchor [workspace] on");
    }
    let joined = out.join("\n");
    if joined.contains("workspace = true") {
        anyhow::bail!("standalone manifest still contains `workspace = true`");
    }
    let tables = joined
        .split('\n')
        .filter(|l| l.trim() == "[workspace]")
        .count();
    if tables != 1 {
        anyhow::bail!("standalone manifest has {tables} [workspace] tables, expected exactly 1");
    }
    if !joined.contains("path = ") {
        anyhow::bail!("standalone manifest has no path dependency; deps did not resolve");
    }
    Ok(joined)
}

/// kelson make <kind> <name> [--module <m>]: scaffold one file (item 30).
fn make_kind(kind: &str, name: &str, module: Option<String>) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let module = match kind {
        "migration" => {
            // Timestamped platform migration; same path kelson make:migration uses.
            return make_migration(name);
        }
        _ => module.ok_or_else(|| {
            anyhow::anyhow!("make:{kind} needs --module <name> (create one with make:module)")
        })?,
    };
    if !root.join("applications").join(&module).exists() {
        anyhow::bail!("module {module:?} not found under applications/ (create it with: kelson make:module {module})");
    }
    let (rel, content) = kelson_codegen::generators::generate(kind, &module, name)
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
// -------------------------------------------------- storage + testing (W5)

/// `kelson storage:link` paths: <root>/public/storage -> <root>/storage/local
/// (the local disk root the platform StorageProvider serves — see the
/// `StorageProvider` in kelson-demo's host).
fn storage_link_paths(root: &Path) -> (PathBuf, PathBuf) {
    (
        root.join("public").join("storage"),
        root.join("storage").join("local"),
    )
}

#[cfg(unix)]
fn make_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

#[cfg(windows)]
fn make_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::windows::fs::symlink_dir(target, link)
}

/// Wave 5: create public/storage -> storage/local. Refuses to overwrite
/// (docs/19); `--force` replaces an existing link or file — a real
/// directory at the link path is never deleted automatically.
fn storage_link_at(root: &Path, force: bool) -> anyhow::Result<()> {
    let (link, target) = storage_link_paths(root);
    let link_disp = link.strip_prefix(root).unwrap_or(&link);
    match std::fs::symlink_metadata(&link) {
        // Fresh create.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => anyhow::bail!("cannot inspect {}: {e}", link_disp.display()),
        // A real directory at the link path may hold real files: never
        // delete it, even with --force (honest refusal beats data loss).
        Ok(meta) if meta.is_dir() => anyhow::bail!(
            "{} is a real directory, not a link — remove it manually if you are sure (never deleted automatically)",
            link_disp.display()
        ),
        // Existing link (including a dangling one) or plain file.
        Ok(_) => {
            if !force {
                anyhow::bail!(
                    "the {} link already exists (use --force to replace it)",
                    link_disp.display()
                );
            }
            std::fs::remove_file(&link)
                .map_err(|e| anyhow::anyhow!("cannot replace {}: {e}", link_disp.display()))?;
        }
    }
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("cannot create {}: {e}", parent.display()))?;
    }
    // A dangling link is useless: make sure the disk root exists first
    // (LocalDisk::new would create it at boot anyway).
    std::fs::create_dir_all(&target).map_err(|e| {
        anyhow::anyhow!("cannot create storage root {}: {e}", target.display())
    })?;
    make_symlink(&target, &link).map_err(|e| {
        #[cfg(windows)]
        let msg = format!(
            "cannot create {}: {e} (Windows needs Developer Mode or an elevated shell for symlinks)",
            link_disp.display()
        );
        #[cfg(not(windows))]
        let msg = format!("cannot create {}: {e}", link_disp.display());
        anyhow::anyhow!(msg)
    })?;
    let target_disp = target.strip_prefix(root).unwrap_or(&target);
    println!("  + {} -> {}", link_disp.display(), target_disp.display());
    Ok(())
}

fn storage_link(force: bool) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    storage_link_at(&root, force)?;
    Ok(ExitCode::SUCCESS)
}

/// Extract `(ok, passed, failed)` from a harness `test result:` line
/// (`test result: ok. 12 passed; 0 failed; ...` or `... FAILED. 12 passed;
/// 1 failed; ...`); unrelated lines report None.
fn parse_test_result(line: &str) -> Option<(bool, u64, u64)> {
    let rest = line.split("test result: ").nth(1)?;
    let mut fields = rest.split(';');
    let outcome = fields.next()?;
    let failed_field = fields.next().unwrap_or_default();
    let ok = outcome.starts_with("ok.");
    let passed = first_count(outcome)?;
    let failed = first_count(failed_field).unwrap_or(0);
    Some((ok, passed, failed))
}

/// First whitespace token that parses as a count (`"ok. 12 passed"` -> 12).
fn first_count(field: &str) -> Option<u64> {
    field.split_whitespace().find_map(|t| t.parse::<u64>().ok())
}

/// Wave 5: cargo-test wrapper — streams the harness output (test lines to
/// stdout, compile output straight through on stderr) and prints a pass/fail
/// summary. `--pg` adds `--features pg-tests` and requires the compose
/// database (DATABASE_URL in the environment or <root>/.env).
fn run_tests(pg: bool) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    if pg && !has_database_url(&root) {
        anyhow::bail!(
            "kelson test --pg requires the compose database: set DATABASE_URL (environment or <root>/.env) — see docs/05-DATABASE.md"
        );
    }
    let mut command = std::process::Command::new("cargo");
    command.arg("test").args(["--workspace", "--locked"]);
    if pg {
        command.args(["--features", "pg-tests"]);
    }
    command
        .current_dir(&root)
        .stdout(std::process::Stdio::piped());
    println!("kelson test{} ...", if pg { " --pg" } else { "" });
    let mut child = command
        .spawn()
        .map_err(|e| anyhow::anyhow!("cannot start cargo: {e}"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("cannot capture cargo output"))?;
    let (mut ok_suites, mut failed_suites, mut passed, mut failed) = (0u64, 0u64, 0u64, 0u64);
    for line in std::io::BufRead::lines(std::io::BufReader::new(stdout)) {
        let line = line?;
        if let Some((ok, p, f)) = parse_test_result(&line) {
            if ok {
                ok_suites += 1;
            } else {
                failed_suites += 1;
            }
            passed += p;
            failed += f;
        }
        println!("{line}");
    }
    let status = child.wait()?;
    let verdict = if status.success() && failed == 0 && failed_suites == 0 {
        "PASS"
    } else {
        "FAIL"
    };
    println!(
        "kelson test: {verdict} — {passed} passed, {failed} failed ({ok_suites} ok / {failed_suites} failed suite result line(s))"
    );
    Ok(if verdict == "PASS" {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// DATABASE_URL from the environment, falling back to <root>/.env (the
/// compose database lands there in dev).
fn has_database_url(root: &Path) -> bool {
    if std::env::var_os("DATABASE_URL").is_some_and(|v| !v.is_empty()) {
        return true;
    }
    let Ok(text) = std::fs::read_to_string(root.join(".env")) else {
        return false;
    };
    text.lines().any(|line| {
        line.trim()
            .strip_prefix("DATABASE_URL=")
            .is_some_and(|value| {
                !value
                    .trim()
                    .trim_matches(|c: char| c == '"' || c == '\'')
                    .is_empty()
            })
    })
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
        kelson_deployment::BuildProfile::parse(profile_raw).map_err(|e| anyhow::anyhow!("{e}"))?;
    println!(
        "kelson build — profile {} (matrix: RELEASE.md)",
        profile.as_str()
    );
    let status = std::process::Command::new("cargo")
        .arg("build")
        .args(profile.cargo_flag())
        .args(["-p", "kelson-demo", "-p", "kelson-worker"])
        .current_dir(&root)
        .status()?;
    if !status.success() {
        anyhow::bail!("cargo build failed ({status})");
    }
    // The running CLI cannot replace its own binary on Windows (file lock),
    // so build kelson-console separately and tolerate that one failure when the
    // running executable lives inside the workspace target dir.
    let cli_status = std::process::Command::new("cargo")
        .arg("build")
        .args(profile.cargo_flag())
        .args(["-p", "kelson-console"])
        .current_dir(&root)
        .status()?;
    if !cli_status.success() {
        let running_from_target = std::env::current_exe()
            .map(|exe| exe.starts_with(root.join("target")))
            .unwrap_or(false);
        if running_from_target {
            println!("warning: kelson-console binary is locked while running; skipped its refresh");
        } else {
            anyhow::bail!("cargo build failed ({cli_status})");
        }
    }
    let (git_commit, git_dirty) = kelson_deployment::git_identity(&root);
    let workspace_version = "0.1.0".to_string();
    let manifest = kelson_deployment::ReleaseManifest {
        version: workspace_version.clone(),
        profile,
        artifacts: kelson_deployment::RELEASE_BINARIES
            .iter()
            .map(|name| kelson_deployment::Artifact {
                name: (*name).to_string(),
                version: workspace_version.clone(),
                profile,
            })
            .collect(),
        git_commit,
        git_dirty,
        built_at: time::OffsetDateTime::now_utc(),
        rustc: rustc_version(),
        // Phase S: record the schema this release ships, so `kelson upgrade`
        // can diff it against `keel_migrations` and tell an operator which
        // framework migrations they still owe. Failing here is deliberate — a
        // release built without its migration set would later report a false
        // all-clear.
        migrations: kelson_deployment::shipped_migrations(&root)
            .map_err(|e| anyhow::anyhow!("{e}"))?,
    };
    let path =
        kelson_deployment::write_manifest(&root, &manifest).map_err(|e| anyhow::anyhow!("{e}"))?;
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
    match kelson_deployment::read_manifest(&root).map_err(|e| anyhow::anyhow!("{e}"))? {
        Some(manifest) => {
            let raw = serde_json::to_string_pretty(&manifest)?;
            println!("{raw}");
            Ok(ExitCode::SUCCESS)
        }
        None => {
            println!("no release manifest found (run `kelson build` first).");
            Ok(ExitCode::SUCCESS)
        }
    }
}

// ------------------------------------------------------------------- flags

fn flag_store(root: &std::path::Path) -> kelson_feature_flags::FileFlagStore {
    kelson_feature_flags::FileFlagStore::new(kelson_feature_flags::default_flags_path(root))
}

fn flag_list() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let registry = kelson_feature_flags::platform_registry();
    let store = flag_store(&root);
    let states = kelson_feature_flags::FlagStore::all(&store).map_err(|e| anyhow::anyhow!("{e}"))?;
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
        kelson_feature_flags::default_flags_path(&root).display()
    );
    Ok(ExitCode::SUCCESS)
}

fn flag_set(name: &str, stage_raw: &str, tenant: Option<String>) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let stage =
        kelson_feature_flags::RolloutStage::parse(stage_raw).map_err(|e| anyhow::anyhow!("{e}"))?;
    let service = kelson_feature_flags::FlagService::new(
        std::sync::Arc::new(kelson_feature_flags::platform_registry()),
        std::sync::Arc::new(flag_store(&root)),
    );
    let state = match tenant {
        Some(raw) => {
            let tenant_id = kelson_foundation::TenantId::from_uuid(
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
    let revision = kelson_feature_flags::FlagStore::revision(&*service.store)
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
    standalone: bool,
    deps_root: Option<&str>,
) -> anyhow::Result<ExitCode> {
    let name = resolve_name_or_prompt("project", name)?;
    new_project(
        &name,
        plane,
        description,
        with_starter,
        standalone,
        deps_root,
    )
}

// ------------------------------------------------------------------ inspect

fn trace_store(root: &Path) -> FileTraceStore {
    FileTraceStore::standard(root)
}

fn parse_execution_id(raw: &str) -> anyhow::Result<kelson_foundation::ExecutionId> {
    let uuid = uuid::Uuid::parse_str(raw)
        .map_err(|e| anyhow::anyhow!("invalid execution id {raw:?}: {e}"))?;
    Ok(kelson_foundation::ExecutionId::from_uuid(uuid))
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

/// `kelson inspect execution|delta|authorization|n-plus-one [id]` (item 45, L60).
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
                        println!("{}", kelson_execution::render_trace(&trace));
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
    println!("inspect one with: kelson inspect execution <id>");
    Ok(ExitCode::SUCCESS)
}

fn inspect_view(store: &dyn TraceStore, id: Option<&str>, view: &str) -> anyhow::Result<ExitCode> {
    let Some(raw) = id else {
        anyhow::bail!("kelson inspect {view} needs an execution id");
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
    let findings = kelson_execution::detect_n_plus_one(&traces, threshold);
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
    println!("inspect one with: kelson inspect execution <id>");
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

/// `kelson replay <execution-id> --dry-run` (item 46, L80): Original vs Replay.
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
    plan: kelson_execution::ExecutionPlan,
}

impl BenchCommand {
    fn new() -> Self {
        let intent = kelson_execution::PlanIntent::new(
            kelson_foundation::OperationId::from_static("bench.write"),
            kelson_foundation::ExecutionProfileKind::Command,
        )
        .with_capability("bench.write");
        Self {
            plan: kelson_execution::PlanCompiler::compile(&intent),
        }
    }
}

impl kelson_execution::OperationHandler for BenchCommand {
    fn plan(&self) -> &kelson_execution::ExecutionPlan {
        &self.plan
    }

    fn handle(
        &self,
        _ctx: Arc<kelson_context::ExecutionContext>,
        input: serde_json::Value,
        unit: &mut dyn kelson_execution::ExecutionUnit,
    ) -> kelson_execution::BoxFuture<'static, Result<serde_json::Value, kelson_foundation::AppError>> {
        unit.record_mutation(kelson_execution::MutationRecord {
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
    plan: kelson_execution::ExecutionPlan,
}

impl BenchRead {
    fn new() -> Self {
        let intent = kelson_execution::PlanIntent::new(
            kelson_foundation::OperationId::from_static("bench.read"),
            kelson_foundation::ExecutionProfileKind::Read,
        );
        Self {
            plan: kelson_execution::PlanCompiler::compile(&intent),
        }
    }
}

impl kelson_execution::OperationHandler for BenchRead {
    fn plan(&self) -> &kelson_execution::ExecutionPlan {
        &self.plan
    }

    fn handle(
        &self,
        _ctx: Arc<kelson_context::ExecutionContext>,
        _input: serde_json::Value,
        _unit: &mut dyn kelson_execution::ExecutionUnit,
    ) -> kelson_execution::BoxFuture<'static, Result<serde_json::Value, kelson_foundation::AppError>> {
        Box::pin(async move { Ok(serde_json::json!({ "items": [] })) })
    }
}

fn bench_policy(
    principal: kelson_foundation::PrincipalId,
    tenant: kelson_foundation::TenantId,
) -> Arc<kelson_policy::PolicyEngine> {
    let policies = Arc::new(kelson_policy::PolicyEngine::new());
    policies.install_snapshot(kelson_policy::AccessSnapshot {
        principal_id: principal,
        tenant_id: tenant,
        membership_active: true,
        capabilities: ["bench.write".to_string()].into_iter().collect(),
        scope_paths: ["/".to_string()].into_iter().collect(),
        authorization_revision: policies.revision(),
        feature_revision: kelson_foundation::FeatureRevision(1),
        module_revision: kelson_foundation::ModuleRevision(1),
        computed_at: time::OffsetDateTime::now_utc(),
    });
    policies
}

fn bench_ctx(
    kind: kelson_foundation::ExecutionProfileKind,
    principal: kelson_foundation::PrincipalId,
    tenant: kelson_foundation::TenantId,
) -> Arc<kelson_context::ExecutionContext> {
    kelson_context::ExecutionContext::builder(kind, time::OffsetDateTime::now_utc())
        .principal(kelson_context::PrincipalContext {
            id: principal,
            display: None,
        })
        .tenant(kelson_context::TenantContext {
            id: tenant,
            slug: "bench".into(),
            plan: "standard".into(),
        })
        .scope(kelson_context::ScopeSnapshot {
            nodes: vec![],
            revision: kelson_foundation::AuthorizationRevision(1),
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

/// `kelson benchmark` (item 47, L75, G15): fixed in-memory workload, median
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
        "kelson benchmark — {iterations} timed iterations per phase (median, in-memory posture)"
    );

    let principal = kelson_foundation::PrincipalId::new();
    let tenant = kelson_foundation::TenantId::new();
    let policies = bench_policy(principal, tenant);
    let mut phases: std::collections::BTreeMap<String, f64> = std::collections::BTreeMap::new();

    // routing: intent -> fully-resolved ExecutionPlan (compiler fast path).
    let intent = kelson_execution::PlanIntent::new(
        kelson_foundation::OperationId::from_static("bench.write"),
        kelson_foundation::ExecutionProfileKind::Command,
    )
    .with_capability("bench.write");
    phases.insert(
        "routing".into(),
        median_micros(iterations, || {
            let start = std::time::Instant::now();
            let plan =
                kelson_execution::PlanCompiler::compile_checked(&intent).expect("plan compiles");
            std::hint::black_box(&plan);
            start.elapsed()
        }),
    );

    // authorization: pure decision path against a hydrated snapshot.
    let decision_input = kelson_policy::DecisionInput {
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
            let cfg = kelson_config::PlatformConfig::load(Some(&config_file)).expect("config loads");
            std::hint::black_box(&cfg);
            start.elapsed()
        }),
    );

    // delta-write + read: the full engine pipeline on the in-memory backend.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let backend = Arc::new(kelson_execution::InMemoryBackend::new());
    let engine = kelson_execution::Engine::new(
        Arc::new(kelson_policy::PolicyAuthz {
            engine: policies.clone(),
        }),
        backend.clone(),
        Arc::new(kelson_execution::InMemoryIdempotency::new()),
    );
    let command = BenchCommand::new();
    let read = BenchRead::new();
    let command_ctx = bench_ctx(kelson_foundation::ExecutionProfileKind::Command, principal, tenant);
    let read_ctx = bench_ctx(kelson_foundation::ExecutionProfileKind::Read, principal, tenant);
    let counter = std::sync::atomic::AtomicUsize::new(0);

    phases.insert(
        "delta-write".into(),
        median_micros(iterations, || {
            let n = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let key = kelson_execution::IdempotencyKey(format!("bench-{n}"));
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
        .join("kelson-bench-telemetry")
        .join(uuid::Uuid::now_v7().to_string());
    std::fs::create_dir_all(&bench_telemetry_dir)?;
    let mut bench_tcfg = kelson_observability::TelemetryConfig::default();
    bench_tcfg.wal.dir = bench_telemetry_dir.join("wal");
    bench_tcfg.paths.incident_dir = bench_telemetry_dir.join("control");
    bench_tcfg.paths.cost_file = bench_telemetry_dir.join("cost.json");
    bench_tcfg.paths.health_dir = bench_telemetry_dir.join("exporters");
    bench_tcfg.exporters.insert(
        "file".into(),
        kelson_observability::ExporterConfig {
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
    let bench_runtime = kelson_observability::ObservabilityRuntime::new(
        bench_tcfg,
        kelson_observability::ServiceIdentity::new("kelson-bench", "benchmark"),
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
                kelson_observability::fields!("i" => 1, "kind" => "bench"),
            );
            start.elapsed()
        }),
    );
    bench_runtime.shutdown(std::time::Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&bench_telemetry_dir);

    // config-reload / config-rollback (Phase 5, §9 box 3): the §10 hot-swap
    // budget and the §5.4 process-local rollback, as measured distributions
    // rather than a single-shot assertion.
    //
    // The file edit is deliberately OUTSIDE the timed span — this measures
    // the reload itself (read → schema gate → hash → swap → subscriber
    // notify), which is what the < 50 ms budget governs.
    //
    // NOTE on scope: these are the CONFIG dimension's numbers. `rollback` in
    // the §9 box means DEPLOYMENT rollback across all five dimensions
    // (RollbackExecutor), whose < 30 s figure needs a live database and is
    // staged separately — see docs/13-OPERATIONS.md.
    {
        struct BenchReload;
        impl kelson_config::ConfigReloader for BenchReload {
            fn on_reload(&self, snapshot: &kelson_config::ConfigSnapshot) {
                // Route/flag subscribers re-derive their artifacts here.
                std::hint::black_box(format!("routes-{}", snapshot.version));
            }
        }

        let dir = std::env::temp_dir().join(format!("kelson-bench-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("bench config dir");
        let path = dir.join("config.toml");
        let write = |ops: u64| {
            std::fs::write(
                &path,
                format!(
                    "[app]\nname = \"kelson-bench\"\nenv = \"dev\"\n\n[limits]\n\
                     default_monthly_operations = {ops}\n"
                ),
            )
            .expect("bench config writes");
        };

        write(1);
        let runtime = kelson_config::ConfigRuntime::from_source(kelson_config::FileConfigSource::new(
            &path,
        ))
        .expect("bench config boots");
        runtime.subscribe(BenchReload);

        let mut ops = 1u64;
        phases.insert(
            "config-reload".into(),
            median_micros(iterations, || {
                ops += 1;
                write(ops); // changed content, so the reload really swaps
                let start = std::time::Instant::now();
                runtime.reload().expect("bench reload succeeds");
                start.elapsed()
            }),
        );

        // Rollback alternates between the two newest held revisions so every
        // call does real work (a self-rollback would time a no-op). The ring
        // holds ≥ 10 revisions by now, so both are held.
        let newest = runtime.get().version;
        let previous = newest.saturating_sub(1).max(1);
        let mut to_newest = false;
        phases.insert(
            "config-rollback".into(),
            median_micros(iterations, || {
                to_newest = !to_newest;
                let target = if to_newest { newest } else { previous };
                let start = std::time::Instant::now();
                runtime
                    .rollback(target)
                    .expect("bench rollback targets a held revision");
                start.elapsed()
            }),
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    // memory: delta-state size after a fixed 500-command burst.
    let mem_backend = Arc::new(kelson_execution::InMemoryBackend::new());
    let mem_engine = kelson_execution::Engine::new(
        Arc::new(kelson_policy::PolicyAuthz {
            engine: policies.clone(),
        }),
        mem_backend.clone(),
        Arc::new(kelson_execution::InMemoryIdempotency::new()),
    );
    for n in 0..500 {
        let key = kelson_execution::IdempotencyKey(format!("mem-{n}"));
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
        None => println!("no baseline yet — run `kelson benchmark --save-baseline` to persist these numbers (G15)."),
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

/// `kelson upgrade` (item 48, L73): version / public APIs / deprecations /
/// config / migrations / dependencies / module contracts. Read-only analysis;
/// automated fixes are limited to the suggested commands.
fn upgrade_report() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    println!("Upgrade Report");
    println!("══════════════");

    // Framework version vs release manifest.
    let current = env!("CARGO_PKG_VERSION");
    let manifest = match kelson_deployment::read_manifest(&root) {
        Ok(manifest) => manifest,
        Err(e) => {
            println!("[!!] release manifest unreadable ({e}) — rebuild with `kelson build`");
            None
        }
    };
    match &manifest {
        Some(m) if m.version == current => {
            println!("[ok] framework version {current} — release manifest matches ({}{})",
                rfc3339(m.built_at),
                if m.git_dirty.unwrap_or(false) { ", dirty build" } else { "" })
        }
        Some(m) => println!("[!!] framework version {current} but release manifest is {} — rebuild with `kelson build`", m.version),
        None => println!("[!!] no release manifest — run `kelson build` (current version {current})"),
    }

    // Phase S: the migration set the release recorded at build time, versus
    // what the tree ships now. The live applied-vs-pending diff needs a
    // database and stays with `kelson migrate:status`; what can be answered
    // offline is whether the release you are running still matches the schema
    // it claimed to ship.
    if let Some(m) = &manifest {
        if !m.migrations.is_empty() {
            println!(
                "[ok] release manifest records {} shipped migration(s)",
                m.migrations.len()
            );
            let current_set =
                kelson_deployment::shipped_migrations(&root).map_err(|e| anyhow::anyhow!("{e}"))?;
            let drift = kelson_deployment::migration_drift(&m.migrations, &current_set);
            if drift.is_empty() {
                println!(
                    "[ok] {} matches the release manifest exactly",
                    kelson_deployment::PAIRS_DIR
                );
            }
            for name in drift.added {
                println!(
                    "[??] migration {name} is in the tree but was not in the release manifest — added after `kelson build`?"
                );
            }
            for name in drift.removed {
                println!(
                    "[!!] migration {name} was in the release manifest but is gone from the tree"
                );
            }
            for name in drift.changed {
                println!(
                    "[!!] migration {name} changed since the release was built. `keel_migrations` \n     records only its name, so `kelson migrate` will NOT reapply it — the \n     database still holds the bytes that were originally applied."
                );
            }
            println!(
                "     applied-vs-pending against the database: run `kelson migrate:status`"
            );
        } else {
            println!(
                "[??] release manifest records no migrations (built before the migration \n     manifest was tracked) — rebuild with `kelson build`"
            );
        }
    }

    // Public API registry (blueprint 49): stability + deprecations.
    let entries = kelson_contracts::api_registry::load_registry();
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
        "[ok] migrations: {} files, latest v{latest} — run `kelson migrate:status` for applied state",
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
        Err(_) => println!("[!!] platform graph gate fails — run `kelson check` for details"),
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
        // Normalize: a leading `/` anchors the rule to the workspace root —
        // exactly what this check means — and a trailing `/` marks a
        // directory (both spellings must count as coverage).
        let t = l.trim().trim_start_matches('/');
        t == pattern || t == format!("{pattern}/")
    })
}

/// `kelson security` (item 51, L74): secrets hygiene (G12), dependency policy
/// (D-62), trace locality (captured inputs never leave the environment).
fn security_check() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let mut failures: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    println!(
        "kelson security — secrets hygiene (G12) · dependency policy (D-62) · trace locality (L74)"
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
        match kelson_config::PlatformConfig::load(Some(&path)) {
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
    println!("gate pending: kelson {} lands with its milestone — see docs/rust/11-FOUNDATION-MASTER-SPEC.md (§13 gates).", cmd_name(cmd));
    Ok(ExitCode::SUCCESS)
}

fn cmd_name(cmd: &Cmd) -> String {
    match cmd {
        Cmd::Dev => "dev",
        Cmd::Serve { .. } => "serve",
        Cmd::Test { .. } => "test",
        _ => "?",
    }
    .to_string()
}

// ---------------------------------------------------------------- profile

/// B-27: `kelson profile <api|worker>` — an out-of-process samply wrapper.
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

/// Testable core of `kelson profile`. When samply is missing we print the
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
    println!("kelson profile: building {package}");
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
        "kelson profile: {} {} (stop the recording to open the flamegraph)",
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
        "kelson profile: samply not found on PATH (target: {})\n  install: cargo install samply\n  manual workflow: docs/13-OPERATIONS.md (flamegraph profiling)\n",
        target.name()
    )
}

// ------------------------------------------------------------------ serve

/// Cargo arguments for spawning the local `kelson-demo` harness.
///
/// Phase P made the demo modules opt-in (`--features demo`, default off) so
/// the harness could be published without dragging unpublished crates into the
/// dependency graph. That is right for a *release* build and wrong for every
/// local command: `serve`, `call`, `shell` and `model:prune` exist to exercise
/// an application, and without the feature the harness not only registered no
/// modules — it refused to boot at all. Every dev invocation therefore enables
/// the feature; callers append `--` before any binary arguments.
fn demo_run_args() -> Vec<&'static str> {
    vec!["run", "--quiet", "-p", "kelson-demo", "--features", "demo"]
}

/// kelson serve (S7 B-08): one-command local loop. Spawns `cargo run -p
/// kelson-demo` (and optionally kelson-worker) with the workspace config and
/// streams their output. Ctrl+C reaches the children through the console;
/// if the API exits first we tear the worker down with it.
fn serve(worker: bool) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let config = config_path(&root);
    println!(
        "kelson serve: kelson-demo{} (config: {})",
        if worker { " + kelson-worker" } else { "" },
        config.display()
    );
    let mut api = std::process::Command::new("cargo")
        .args(demo_run_args())
        .current_dir(&root)
        .env("KEEL_CONFIG", &config)
        .spawn()
        .map_err(|e| anyhow::anyhow!("cannot spawn kelson-demo: {e}"))?;
    let mut worker_child = if worker {
        Some(
            std::process::Command::new("cargo")
                .arg("run")
                .arg("--quiet")
                .arg("-p")
                .arg("kelson-worker")
                .current_dir(&root)
                .env("KEEL_CONFIG", &config)
                .spawn()
                .map_err(|e| anyhow::anyhow!("cannot spawn kelson-worker: {e}"))?,
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
        eprintln!("kelson-demo exited with status {code}");
        Ok(ExitCode::from(code.clamp(0, 255) as u8))
    }
}

// ------------------------------------------------------------------ shell

// --------------------------------------------- console routes (`kelson call`)

/// `routes/console.toml` commands (the CLI-invokable surface, docs/19). An
/// absent file means no commands — `kelson call` then fails with the
/// declaration hint. kelson-demo re-resolves against the compiled graph whose
/// GRAPH-ROUTEFILE-OP gate refuses unknown operations at boot.
fn console_route_commands(
    root: &Path,
) -> anyhow::Result<Vec<kelson_registry::ConsoleCommandSpec>> {
    let path = root.join("routes").join("console.toml");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path)?;
    let manifest = kelson_registry::RouteFileManifest::from_toml(&raw, "routes/console.toml", None)
        .map_err(|v| anyhow::anyhow!("routes/console.toml: {}", v.message))?;
    Ok(manifest.commands)
}

/// Fast pre-flight name lookup — fails before the kelson-demo boot, listing
/// what routes/console.toml actually declares.
fn resolve_console_route<'a>(
    commands: &'a [kelson_registry::ConsoleCommandSpec],
    name: &str,
) -> anyhow::Result<&'a kelson_registry::ConsoleCommandSpec> {
    commands.iter().find(|c| c.name == name).ok_or_else(|| {
        let known = commands
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        if known.is_empty() {
            anyhow::anyhow!("no commands declared — add a [[command]] entry to routes/console.toml")
        } else {
            anyhow::anyhow!("unknown console command {name:?} — declared: {known}")
        }
    })
}

/// Early payload validation (bad JSON fails before the kelson-demo boot).
fn parse_call_input(input: Option<&str>) -> anyhow::Result<serde_json::Value> {
    match input {
        None | Some("") => Ok(serde_json::json!({})),
        Some(raw) => {
            serde_json::from_str(raw).map_err(|e| anyhow::anyhow!("invalid JSON input: {e}"))
        }
    }
}

/// The default operator exists in dev/test only (docs/19); staging and
/// production must pass --principal (kelson-demo re-checks authoritatively).
fn default_operator_allowed(env: &Environment) -> bool {
    matches!(env, Environment::Dev | Environment::Test)
}

/// Wave 5: `kelson call <name> [json]` — pre-flight the route, payload and
/// env gate, then spawn `kelson-demo --call` (same posture as `kelson shell`:
/// Tools-class console, stdio inherited so the JSON result streams straight
/// through) and propagate the exit code so capability denial is scriptable.
fn call_command(
    name: &str,
    input: Option<&str>,
    principal: Option<&str>,
    tenant: Option<&str>,
) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let commands = console_route_commands(&root)?;
    resolve_console_route(&commands, name)?;
    parse_call_input(input)?;
    if principal.is_none() {
        let cfg = load_config(&root)?;
        if !default_operator_allowed(&cfg.app.env) {
            anyhow::bail!(
                "kelson call refuses the default operator outside dev/test (env: {}) — pass --principal <uuid>",
                cfg.app.env.as_str()
            );
        }
    }
    let config = config_path(&root);
    let mut command = std::process::Command::new("cargo");
    command
        .args(demo_run_args())
        .arg("--")
        .arg("--call")
        .arg(name);
    if let Some(raw) = input {
        if !raw.is_empty() {
            command.args(["--input", raw]);
        }
    }
    if let Some(raw) = principal {
        command.args(["--principal", raw]);
    }
    if let Some(raw) = tenant {
        command.args(["--tenant", raw]);
    }
    let status = command
        .current_dir(&root)
        .env("KEEL_CONFIG", &config)
        .status()
        .map_err(|e| anyhow::anyhow!("cannot spawn kelson-demo --call: {e}"))?;
    let code = status.code().unwrap_or(1);
    Ok(if code == 0 {
        ExitCode::SUCCESS
    } else {
        // kelson-demo already printed the failure to stderr — just propagate.
        ExitCode::from(code.clamp(0, 255) as u8)
    })
}

/// B-29: interactive platform console. kelson-console is a Tools-class crate and
/// the dep-rules matrix keeps tools from linking runtime crates, so the REPL
/// itself runs inside kelson-demo behind `--shell`; this spawns it exactly like
/// `serve` (same config, stdio inherited so the prompt is interactive).
fn shell_console() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let config = config_path(&root);
    println!(
        "kelson shell: booting the engine (config: {})",
        config.display()
    );
    let status = std::process::Command::new("cargo")
        .args(demo_run_args())
        .arg("--")
        .arg("--shell")
        .current_dir(&root)
        .env("KEEL_CONFIG", &config)
        .status()
        .map_err(|e| anyhow::anyhow!("cannot spawn kelson-demo --shell: {e}"))?;
    let code = status.code().unwrap_or(1);
    if code == 0 {
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!("kelson shell exited with status {code}");
        Ok(ExitCode::from(code.clamp(0, 255) as u8))
    }
}

// ------------------------------------------------------------------ prune

/// Wave 3 Phase 6: `kelson model:prune`. kelson-console is Tools-class and
/// dep-rules keeps it from linking runtime crates, so the sweep runs inside
/// kelson-demo behind `--prune` (same spawn pattern as `kelson shell`): boots the
/// graph, purges `chunk` rows per store pass across every registered
/// prunable model, prints per-model counts and exits — no server, no REPL.
fn model_prune(chunk: u32, tenant: Option<String>) -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let config = config_path(&root);
    println!(
        "kelson model:prune: booting (chunk: {chunk}, config: {})",
        config.display()
    );
    let mut command = std::process::Command::new("cargo");
    command
        .args(demo_run_args())
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
        .map_err(|e| anyhow::anyhow!("cannot spawn kelson-demo --prune: {e}"))?;
    let code = status.code().unwrap_or(1);
    if code == 0 {
        Ok(ExitCode::SUCCESS)
    } else {
        eprintln!("kelson model:prune exited with status {code}");
        Ok(ExitCode::from(code.clamp(0, 255) as u8))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wave 3 Phase 6: `kelson model:prune` parses with the default chunk
    /// cap and honors `--chunk`/`--tenant`.
    #[test]
    fn model_prune_cli_parses() {
        let cli = Cli::try_parse_from(["kelson", "model:prune"]).expect("parses");
        match cli.cmd {
            Cmd::ModelPrune { chunk, tenant } => {
                assert_eq!(chunk, 200);
                assert!(tenant.is_none());
            }
            _ => panic!("expected model:prune"),
        }
        let cli = Cli::try_parse_from([
            "kelson",
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
        let crate_name = "kelson-starterprobe";
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
            kelson_registry::ModuleManifest::from_toml(&manifest_raw).expect("manifest parses");
        let graph = kelson_registry::PlatformGraph::compile(vec![manifest]).expect("graph compiles");
        // Full CRUD (Phase Q): create + list + read + update + delete.
        assert_eq!(graph.operations.len(), 5, "create + list + read + update + delete");
        assert_eq!(graph.modules[0].routes.len(), 5);

        let list_url = graph
            .resolve_url(name, "notes.list", &std::collections::BTreeMap::new())
            .expect("named list route resolves");
        assert_eq!(list_url, format!("/api/v1/{name}/notes"));
        let create = graph
            .route_named(name, "notes.create")
            .expect("named create route exists");
        assert_eq!(create.method, "POST");
        // The three by-id verbs share the /notes/{id} address.
        for (route, method) in [
            ("notes.read", "GET"),
            ("notes.update", "PUT"),
            ("notes.delete", "DELETE"),
        ] {
            let r = graph
                .route_named(name, route)
                .unwrap_or_else(|| panic!("named {route} route exists"));
            assert_eq!(r.method, method, "{route} verb");
            assert!(
                r.path.ends_with("/notes/{id}"),
                "{route} addresses the by-id path, got {}",
                r.path
            );
        }

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
                .replace("__PROJECT_CRATE__", "kelson-probe")
                .replace("__PROJECT_DESCRIPTION__", "Probe");
            assert!(
                !rendered.contains("__PROJECT_"),
                "unsubstituted marker in {rel}"
            );
        }
    }

    /// The compiled-in templates must be exactly the ones on disk, or a
    /// stranger holding an installed `kelson` would generate a different
    /// project from a contributor holding a checkout of the same version and
    /// commit. This is the invariant the whole embedding change rests on, so
    /// it is checked rather than assumed.
    #[test]
    fn the_embedded_templates_match_the_ones_on_disk() {
        let root = workspace_root().expect("workspace root resolves");
        for with_starter in [false, true] {
            let disk = template_source(Some(&root), with_starter)
                .files()
                .expect("disk template reads");
            let embedded = template_source(None, with_starter)
                .files()
                .expect("embedded template reads");
            assert!(
                !disk.is_empty(),
                "{} template would generate nothing",
                if with_starter { "starter" } else { "project" }
            );
            assert_eq!(
                disk, embedded,
                "{} template differs between disk and the compiled-in copy",
                if with_starter { "starter" } else { "project" }
            );
        }
    }

    /// A checkout must win over the compiled-in copy, so editing a template
    /// takes effect on the next `kelson new` without a rebuild.
    #[test]
    fn a_checkout_is_preferred_over_the_embedded_copy() {
        let root = workspace_root().expect("workspace root resolves");
        for with_starter in [false, true] {
            assert!(
                matches!(
                    template_source(Some(&root), with_starter),
                    TemplateSource::Disk(_)
                ),
                "a checkout's templates must be read from disk"
            );
        }
    }

    /// With no workspace above the cwd there is nothing to read from, which is
    /// the normal case for an installed console. It must fall back rather than
    /// fail - falling back is the entire reason the templates are embedded.
    #[test]
    fn no_workspace_falls_back_to_the_embedded_templates() {
        for with_starter in [false, true] {
            assert!(matches!(
                template_source(None, with_starter),
                TemplateSource::Embedded(_)
            ));
        }
    }

    /// autocrlf decides the line endings of the work-tree files, and on a
    /// Windows build of the binaries too. Normalising is what keeps two builds
    /// of one commit generating byte-identical projects, so no body may carry
    /// a carriage return by the time it reaches the disk.
    #[test]
    fn generated_template_bodies_carry_no_carriage_returns() {
        let root = workspace_root().expect("workspace root resolves");
        for source in [
            template_source(Some(&root), false),
            template_source(Some(&root), true),
            template_source(None, false),
            template_source(None, true),
        ] {
            for (rel, body) in source.files().expect("template reads") {
                assert!(!body.contains('\r'), "{rel} still carries a carriage return");
            }
        }
    }

    /// `Path::canonicalize` on Windows returns a verbatim path, and a manifest
    /// carrying it does not parse at all: with the slashes swapped cargo reads
    /// `//?/C:/...` as a URL and refuses the file. This is the `--deps-root`
    /// route, which is the only way to generate a standalone project until the
    /// kelson crates are on crates.io, so the prefix must never reach the
    /// manifest.
    #[test]
    fn toml_paths_never_carry_the_windows_verbatim_prefix() {
        assert_eq!(toml_path(Path::new(r"C:\src\kelson")), "C:/src/kelson");
        assert_eq!(
            toml_path(Path::new(r"\\?\C:\src\kelson")),
            "C:/src/kelson",
            "the verbatim prefix must be stripped, not slash-swapped"
        );
        assert_eq!(
            toml_path(Path::new(r"\\?\UNC\fileserver\kelson")),
            "//fileserver/kelson",
            "a verbatim UNC path keeps the leading double slash once stripped"
        );
        assert_eq!(toml_path(Path::new(r"\\server\share\x")), "//server/share/x");
        assert_eq!(toml_path(Path::new("/posix/src/kelson")), "/posix/src/kelson");
    }

    /// Wave 1 route-cache cycle: route:cache writes the pinned artifact
    /// (resolved chains + manifest hash), route:clear removes it. The boot
    /// side (hash-mismatch-refuse) is proven by the kelson-demo
    /// route_cache_pin test; this covers the CLI half of the cycle.
    #[test]
    fn route_cache_cycle_writes_and_clears_the_artifact() {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kelson-console-routecache-{}-{}",
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
        let path = kelson_registry::route_cache_path(&dir);
        assert!(path.exists(), "artifact written");
        let artifact = kelson_registry::RouteCacheArtifact::load(&path)
            .expect("artifact loads")
            .expect("artifact present");
        assert_eq!(artifact.module_abi, kelson_registry::MODULE_ABI);
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
            kelson_registry::route_cache_path(&dir).exists(),
            "route artifact warm"
        );
        assert!(
            kelson_registry::config_cache_path(&dir).exists(),
            "config artifact warm"
        );
        let view_dir = dir.join("storage").join("framework").join("views");
        let compiled = std::fs::read_dir(&view_dir).map(|d| d.count()).unwrap_or(0);
        assert!(compiled > 0, "view cache warm ({compiled} entries)");

        // Abort-on-first-failure: drop the config artifact, break the graph
        // (GRAPH-MW-UNKNOWN at the route step), and prove optimize stops
        // there — the config artifact stays absent.
        std::fs::remove_file(kelson_registry::config_cache_path(&dir)).unwrap();
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
            !kelson_registry::config_cache_path(&dir).exists(),
            "abort never reached the config step"
        );

        // optimize:clear -> every remaining artifact gone (clears do not
        // recompile the graph, so they succeed even on a broken manifest).
        assert_eq!(
            optimize_clear_at(&dir).expect("optimize:clear runs"),
            ExitCode::SUCCESS
        );
        assert!(!kelson_registry::route_cache_path(&dir).exists(), "route cold");
        assert!(!kelson_registry::config_cache_path(&dir).exists(), "config cold");
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
        let cli = Cli::try_parse_from(["kelson", "migrate:rollback"]).expect("rollback parses");
        match cli.cmd {
            Cmd::MigrateRollback { step } => assert_eq!(step, 1, "default step is 1"),
            _ => panic!("migrate:rollback maps to MigrateRollback"),
        }
        let cli = Cli::try_parse_from(["kelson", "migrate:rollback", "--step", "3"])
            .expect("explicit step parses");
        match cli.cmd {
            Cmd::MigrateRollback { step } => assert_eq!(step, 3),
            _ => panic!("--step maps to MigrateRollback"),
        }
        let cli = Cli::try_parse_from(["kelson", "migrate:refresh", "--step", "2"])
            .expect("refresh parses");
        match cli.cmd {
            Cmd::MigrateRefresh { step } => assert_eq!(step, 2),
            _ => panic!("migrate:refresh maps to MigrateRefresh"),
        }
        assert!(
            matches!(
                Cli::try_parse_from(["kelson", "migrate:reset"])
                    .expect("reset parses")
                    .cmd,
                Cmd::MigrateReset
            ),
            "migrate:reset maps to MigrateReset"
        );
        assert!(
            matches!(
                Cli::try_parse_from(["kelson", "migrate:install"])
                    .expect("install parses")
                    .cmd,
                Cmd::MigrateInstall
            ),
            "migrate:install maps to MigrateInstall"
        );
    }

    /// §7 (Phase 4): the promotion commands parse under their documented
    /// names and flags — `promote` defaults to the plan artifact, `rollback`
    /// to the saved checkpoint, both with `--dry-run` available.
    #[test]
    fn promote_and_rollback_parse_with_documented_flags() {
        let cli = Cli::try_parse_from(["kelson", "promote"]).expect("promote parses");
        match cli.cmd {
            Cmd::Promote { plan, dry_run } => {
                assert_eq!(plan, None, "plan defaults to storage/framework/promotion.json");
                assert!(!dry_run, "--dry-run defaults to off");
            }
            _ => panic!("promote maps to Promote"),
        }

        let cli = Cli::try_parse_from(["kelson", "promote", "--plan", "release.json", "--dry-run"])
            .expect("explicit plan parses");
        match cli.cmd {
            Cmd::Promote { plan, dry_run } => {
                assert_eq!(plan.as_deref(), Some("release.json"));
                assert!(dry_run, "--dry-run is honored");
            }
            _ => panic!("--plan maps to Promote"),
        }

        let cli = Cli::try_parse_from(["kelson", "rollback"]).expect("rollback parses");
        match cli.cmd {
            Cmd::Rollback { checkpoint, dry_run } => {
                assert_eq!(
                    checkpoint, None,
                    "checkpoint defaults to storage/framework/promotion.checkpoint.json"
                );
                assert!(!dry_run, "--dry-run defaults to off");
            }
            _ => panic!("rollback maps to Rollback"),
        }

        let cli =
            Cli::try_parse_from(["kelson", "rollback", "--checkpoint", "cp.json", "--dry-run"])
                .expect("explicit checkpoint parses");
        match cli.cmd {
            Cmd::Rollback { checkpoint, dry_run } => {
                assert_eq!(checkpoint.as_deref(), Some("cp.json"));
                assert!(dry_run, "--dry-run is honored");
            }
            _ => panic!("--checkpoint maps to Rollback"),
        }
    }

    /// §7.2: the two artifacts on disk are the plan (`ReleaseArtifact`) and
    /// the checkpoint (`PromotionTarget`) — both round-trip byte-for-byte
    /// through the helpers `promote`/`rollback` actually call, so a promote
    /// written by one invocation is restorable by the next.
    #[test]
    fn promotion_plan_and_checkpoint_round_trip_on_disk() {
        let dir = std::env::temp_dir().join(format!("kelson-promotion-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");

        let target = kelson_deployment::rollback::PromotionTarget {
            config_version: 7,
            db_version: Some(5),
            module_versions: std::collections::BTreeMap::from([(
                "echo".to_string(),
                "v2".to_string(),
            )]),
            job_schema_versions: std::collections::BTreeMap::from([(
                "mail.send".to_string(),
                3u32,
            )]),
        };
        let artifact =
            kelson_deployment::promote::ReleaseArtifact::new("1.4.0", target.clone());

        let plan_path = dir.join("promotion.json");
        let raw = serde_json::to_string_pretty(&artifact).expect("plan serializes");
        std::fs::write(&plan_path, raw).expect("plan written");
        let loaded = promotion::load_plan(&plan_path).expect("plan parses");
        assert_eq!(loaded, artifact, "the plan round-trips");

        let cp_path = dir.join("promotion.checkpoint.json");
        promotion::save_checkpoint(&cp_path, &target).expect("checkpoint written");
        let restored = promotion::load_checkpoint(&cp_path).expect("checkpoint parses");
        assert_eq!(restored, target, "the checkpoint round-trips");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// B-31 Phase F: `wire_promotion` builds on EITHER driver — the
    /// `pg_only_on_mysql` refusal at this seam is deleted (the last pg-only
    /// code path in the console). The pools are LAZY (syntactically valid
    /// URLs, never connected): wiring construction is I/O-free by design —
    /// config boot, the five adapters and the executor constructor only
    /// build — so both dispatch arms assert in every workspace gate with no
    /// database attached. The live prove-out (promote + rollback against
    /// MySQL 8.0.44) is the operator transcript recorded in the Phase F
    /// CHANGELOG entry.
    #[test]
    fn promotion_wiring_builds_on_both_drivers() {
        // sqlx's `connect_lazy` asks for a Tokio context (the pool holds a
        // lazy initialization task); a throwaway runtime supplies it. The
        // wiring build itself never awaits — no query ever runs.
        let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
        let _guard = rt.enter();

        let root = workspace_root().expect("workspace root resolves");

        let mysql = DbPool::MySql(
            kelson_mysql::MySqlPoolOptions::new()
                .connect_lazy("mysql://user:pass@127.0.0.1:3306/keel")
                .expect("mysql url parses"),
        );
        let wiring = wire_promotion(&root, &mysql).expect("wiring builds on mysql");
        assert_eq!(wiring.pool.driver_name(), "mysql");

        let pg = DbPool::Postgres(
            sqlx::postgres::PgPoolOptions::new()
                .connect_lazy("postgres://user:pass@127.0.0.1:5432/keel")
                .expect("postgres url parses"),
        );
        let wiring = wire_promotion(&root, &pg).expect("wiring builds on postgres");
        assert_eq!(wiring.pool.driver_name(), "postgres");
    }

    /// Wave 5 Phase 3: the advisory-lock key matches the program spec
    /// (docs/19: `pg_advisory_lock(7259)`) and the fail-closed deadline is
    /// actually bounded.
    #[test]
    fn migration_lock_key_matches_the_program_spec() {
        assert_eq!(kelson_postgres::MIGRATION_LOCK_KEY, 7259);
        assert!(
            !kelson_postgres::MIGRATION_LOCK_TIMEOUT.is_zero(),
            "acquisition deadline is bounded (fail-closed, never a hang)"
        );
    }

    /// Wave 5 Phase 4: the db commands parse under their documented names.
    #[test]
    fn db_commands_parse_with_documented_names() {
        assert!(
            matches!(
                Cli::try_parse_from(["kelson", "db:show"]).expect("db:show parses").cmd,
                Cmd::DbShow
            ),
            "db:show maps to DbShow"
        );
        assert!(
            matches!(
                Cli::try_parse_from(["kelson", "db:schema:dump"])
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
            out.contains("kelson normalized schema snapshot")
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

    /// B-31 Phase E: the MySQL snapshot is the server's SHOW CREATE TABLE
    /// output — statements sorted by name regardless of input order,
    /// verbatim column bytes, `;`-terminated with a blank line after each
    /// table, the data-dependent AUTO_INCREMENT counter stripped, and
    /// banner + dialect provenance recorded.
    #[test]
    fn mysql_schema_dump_renders_normalized_sorted_ddl() {
        let tables = vec![
            MySqlSchemaTable {
                name: "zebras".into(),
                create_sql: "CREATE TABLE `zebras` (\n  `id` bigint NOT NULL,\n  `label` varchar(191) DEFAULT NULL\n) ENGINE=InnoDB AUTO_INCREMENT=42 DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci".into(),
            },
            MySqlSchemaTable {
                name: "apples".into(),
                create_sql: "CREATE TABLE `apples` (\n  `id` binary(16) NOT NULL,\n  `sku` varchar(64) NOT NULL,\n  PRIMARY KEY (`id`,`sku`)\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci".into(),
            },
        ];
        let out = render_mysql_schema_snapshot("keel", &tables);
        // Banner + dialect provenance (this snapshot is MySQL DDL).
        assert!(
            out.contains("kelson normalized schema snapshot")
                && out.contains("never executes this file"),
            "{out}"
        );
        assert!(
            out.contains("mysql dialect: SHOW CREATE TABLE per table"),
            "{out}"
        );
        // Tables sorted by name regardless of input order.
        let apples = out.find("CREATE TABLE `apples`").expect("apples present");
        let zebras = out.find("CREATE TABLE `zebras`").expect("zebras present");
        assert!(apples < zebras, "sorted output:\n{out}");
        // The last table's block is byte-exact: server columns verbatim,
        // AUTO_INCREMENT counter stripped, `;`-terminated + blank line.
        let tail = &out[zebras..];
        assert_eq!(
            tail,
            "CREATE TABLE `zebras` (\n  `id` bigint NOT NULL,\n  `label` varchar(191) DEFAULT NULL\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_0900_ai_ci;\n\n"
        );
        // The counter value is gone; the dialect line still names the rule.
        assert!(!out.contains("AUTO_INCREMENT=42"), "counter stripped:\n{out}");
    }

    /// B-31 Phase E: an empty database still produces a valid, honest
    /// snapshot (banner + marker naming the database) — the MySQL twin of
    /// the empty-schema marker.
    #[test]
    fn mysql_schema_dump_renders_an_empty_marker_for_an_empty_database() {
        let out = render_mysql_schema_snapshot("keel", &[]);
        assert!(out.contains("(no tables in database `keel`)"), "{out}");
        assert!(out.contains("never executes this file"), "{out}");
    }

    /// B-31 Phase E: the AUTO_INCREMENT stripper removes only the digit
    /// counter it knows; any other continuation after `=` passes through
    /// byte-for-byte (never guess at unrecognized DDL).
    #[test]
    fn mysql_schema_dump_strips_only_the_auto_increment_counter() {
        assert_eq!(
            strip_auto_increment(") ENGINE=InnoDB AUTO_INCREMENT=12 DEFAULT CHARSET=utf8mb4"),
            ") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4"
        );
        let unknown = ") ENGINE=InnoDB AUTO_INCREMENT=DEFAULT)";
        assert_eq!(strip_auto_increment(unknown), unknown);
    }

    /// B-31 Phase E: column COLLATE/CHARACTER SET clauses that merely
    /// repeat the statement's own table options are stripped (the inherited
    /// form the server renders back byte-identically); a collation
    /// differing from the table's and the options line stay verbatim.
    #[test]
    fn mysql_schema_dump_strips_table_redundant_column_collation() {
        let tables = vec![MySqlSchemaTable {
            name: "_sqlx_migrations".into(),
            create_sql: "CREATE TABLE `_sqlx_migrations` (\n  `version` bigint NOT NULL,\n  `description` text COLLATE utf8mb4_unicode_ci NOT NULL,\n  `applied` varchar(255) CHARACTER SET utf8mb4 COLLATE utf8mb4_unicode_ci NOT NULL,\n  `other` varchar(64) COLLATE utf8mb4_0900_ai_ci NOT NULL,\n  `kind2` varchar(32) COLLATE utf8mb4_unicode_ci2 NOT NULL,\n  PRIMARY KEY (`version`)\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci"
                .into(),
        }];
        let out = render_mysql_schema_snapshot("keel", &tables);
        // Redundant (== table's) COLLATE removed, with its CHARACTER SET when present.
        assert!(out.contains("`description` text NOT NULL,"), "{out}");
        assert!(out.contains("`applied` varchar(255) NOT NULL,"), "{out}");
        // Collations differing from the table's — including a longer name the
        // table's collation is only a prefix of — stay verbatim.
        assert!(
            out.contains("`other` varchar(64) COLLATE utf8mb4_0900_ai_ci NOT NULL,"),
            "{out}"
        );
        assert!(
            out.contains("`kind2` varchar(32) COLLATE utf8mb4_unicode_ci2 NOT NULL,"),
            "{out}"
        );
        // Non-column lines (options, PK) stay verbatim.
        assert!(
            out.contains(") ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_unicode_ci;\n"),
            "{out}"
        );
        assert!(out.contains("PRIMARY KEY (`version`)"), "{out}");
    }

    /// Wave 5 Phase 5: queue:monitor renders every depth counter plus the
    /// oldest pending age in compact form (pure render, no store needed).
    #[test]
    fn queue_monitor_renders_depth_and_oldest_age() {
        let now = time::OffsetDateTime::now_utc();
        let snap = kelson_jobs::QueueSnapshot {
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
        let empty = kelson_jobs::QueueSnapshot {
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
        let cli = Cli::try_parse_from(["kelson", "queue:prune-failed"]).expect("parses");
        match cli.cmd {
            Cmd::QueuePruneFailed { hours } => assert_eq!(hours, 168, "default 168h"),
            _ => panic!("queue:prune-failed maps to QueuePruneFailed"),
        }
        let cli =
            Cli::try_parse_from(["kelson", "queue:prune-failed", "--hours", "24"]).expect("parses");
        match cli.cmd {
            Cmd::QueuePruneFailed { hours } => assert_eq!(hours, 24),
            _ => panic!("--hours maps to QueuePruneFailed"),
        }
        let cli = Cli::try_parse_from(["kelson", "queue:prune-batches"]).expect("parses");
        match cli.cmd {
            Cmd::QueuePruneBatches { hours } => assert_eq!(hours, 72, "default 72h"),
            _ => panic!("queue:prune-batches maps to QueuePruneBatches"),
        }
        let cli = Cli::try_parse_from(["kelson", "queue:forget", "abc"]).expect("parses");
        match cli.cmd {
            Cmd::QueueForget { id } => assert_eq!(id, "abc"),
            _ => panic!("queue:forget maps to QueueForget"),
        }
        let cli = Cli::try_parse_from(["kelson", "queue:retry-batch", "b-1"]).expect("parses");
        match cli.cmd {
            Cmd::QueueRetryBatch { id } => assert_eq!(id, "b-1"),
            _ => panic!("queue:retry-batch maps to QueueRetryBatch"),
        }
        // The flag-less commands parse under their documented names.
        let cli = Cli::try_parse_from(["kelson", "queue:clear"]).expect("parses");
        assert!(matches!(cli.cmd, Cmd::QueueClear), "queue:clear");
        let cli = Cli::try_parse_from(["kelson", "queue:flush"]).expect("parses");
        assert!(matches!(cli.cmd, Cmd::QueueFlush), "queue:flush");
        let cli = Cli::try_parse_from(["kelson", "queue:monitor"]).expect("parses");
        assert!(matches!(cli.cmd, Cmd::QueueMonitor), "queue:monitor");
        let cli = Cli::try_parse_from(["kelson", "queue:restart"]).expect("parses");
        assert!(matches!(cli.cmd, Cmd::QueueRestart), "queue:restart");
        let cli = Cli::try_parse_from(["kelson", "queue:listen"]).expect("parses");
        assert!(matches!(cli.cmd, Cmd::QueueListen), "queue:listen");
    }

    /// Wave 6 P9: the batch commands validate BEFORE touching a database —
    /// a malformed id and a non-positive cutoff refuse with actionable
    /// errors whether or not DATABASE_URL is set (the id parse runs first).
    #[tokio::test]
    async fn batch_commands_validate_before_connecting() {
        let err = queue_retry_batch("not-a-uuid")
            .await
            .expect_err("malformed batch id refuses");
        assert!(err.to_string().contains("invalid batch id"), "{err}");

        let err = queue_prune_batches(0)
            .await
            .expect_err("zero cutoff refuses");
        assert!(err.to_string().contains("--hours"), "{err}");
    }

    /// Unique temp root per test (Windows-safe: no forward-slash asserts).
    fn fresh_temp(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kelson-console-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    /// Wave 8 B-24: pkg:add validates the package's module.toml (the
    /// package v1 contract) and appends a root-relative entry; duplicates
    /// are refused by name; pkg:list reports ABI compatibility fail-closed
    /// (any unusable entry exits non-zero).
    #[test]
    fn wave8_pkg_add_and_list_validate_the_index() {
        let root = fresh_temp("pkg");

        // No manifest -> refused (a package v1 ships a manifest).
        std::fs::create_dir_all(root.join("vendor").join("nope")).unwrap();
        let err = pkg_add_at(&root, "vendor/nope").expect_err("manifest required");
        assert!(err.to_string().contains("module.toml"), "{err}");

        // Malformed manifest -> refused with the file in the message.
        let broken = root.join("vendor").join("broken");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("module.toml"), "[module\nname = ").unwrap();
        let err = pkg_add_at(&root, "vendor/broken").expect_err("parse fails");
        assert!(err.to_string().contains("module.toml"), "{err}");

        // Valid package -> appended, version taken from the manifest, and
        // the stored path resolves back to the package directory.
        let pkg = root.join("vendor").join("billing");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(
            pkg.join("module.toml"),
            "[module]\nname = \"billing\"\nversion = \"0.3.0\"\n",
        )
        .unwrap();
        pkg_add_at(&root, "vendor/billing").expect("adds");
        let index = kelson_registry::PackageIndex::load(&root).expect("index written");
        assert_eq!(index.package.len(), 1);
        assert_eq!(index.package[0].name, "billing");
        assert_eq!(index.package[0].version, "0.3.0");
        assert_eq!(
            kelson_registry::PackageIndex::entry_dir(&root, &index.package[0]),
            pkg,
            "stored path resolves back to the package directory"
        );

        // Duplicate name (a second directory declaring the same package)
        // -> refused.
        let twin = root.join("vendor").join("twin");
        std::fs::create_dir_all(&twin).unwrap();
        std::fs::write(
            twin.join("module.toml"),
            "[module]\nname = \"billing\"\nversion = \"0.3.1\"\n",
        )
        .unwrap();
        let err = pkg_add_at(&root, "vendor/twin").expect_err("duplicate name");
        assert!(err.to_string().contains("already indexed"), "{err}");

        // Compatible index -> graph-ready (SUCCESS).
        assert_eq!(pkg_list_at(&root).unwrap(), ExitCode::SUCCESS);

        // ABI mismatch (manifest api 2 vs host MODULE_ABI 1) -> NOT
        // graph-ready (FAILURE) while the entry still lists.
        std::fs::write(
            pkg.join("module.toml"),
            "[module]\nname = \"billing\"\nversion = \"0.3.0\"\napi = 2\n",
        )
        .unwrap();
        assert_eq!(pkg_list_at(&root).unwrap(), ExitCode::FAILURE);

        // Missing manifest -> invalid entry, still fail-closed.
        std::fs::remove_file(pkg.join("module.toml")).unwrap();
        assert_eq!(pkg_list_at(&root).unwrap(), ExitCode::FAILURE);

        // Empty index lists cleanly.
        let empty = fresh_temp("pkg-empty");
        assert_eq!(pkg_list_at(&empty).unwrap(), ExitCode::SUCCESS);

        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&empty).ok();
    }

    /// Wave 8 sample package fixture (docs/19 "tested against a sample
    /// package"): billing ships a stamped migration, a [config] table, a
    /// nested asset and a view - the four publish surfaces.
    fn seed_publish_package(root: &Path) -> PathBuf {
        let pkg = root.join("packages").join("billing");
        std::fs::create_dir_all(pkg.join("database").join("migrations")).unwrap();
        std::fs::create_dir_all(pkg.join("public").join("img")).unwrap();
        std::fs::create_dir_all(pkg.join("views")).unwrap();
        std::fs::write(
            pkg.join("module.toml"),
            "[module]\nname = \"billing\"\nversion = \"0.3.0\"\n\n\
             [config.limits]\ndefault_monthly_operations = 11\n",
        )
        .unwrap();
        std::fs::write(
            pkg.join("database")
                .join("migrations")
                .join("20260901000001_create_invoices.sql"),
            "-- create invoices\n",
        )
        .unwrap();
        std::fs::write(pkg.join("public").join("img").join("logo.svg"), "<svg/>\n").unwrap();
        std::fs::write(pkg.join("views").join("welcome.html"), "pkg says hi").unwrap();
        pkg_add_at(root, "packages/billing").expect("indexes the sample package");
        pkg
    }

    /// Wave 8 B-25: publish materializes the sample package's migration
    /// (version prefix kept, package name namespacing the description),
    /// config copy and assets; views register (no copy); a second run is a
    /// byte-identical no-op; both commands parse with --force.
    #[test]
    fn wave8_publish_materializes_a_sample_package_and_is_idempotent() {
        let root = fresh_temp("publish");
        seed_publish_package(&root);

        let outcome = publish_at(&root, "billing", false, PublishScope::All).expect("publishes");
        assert!(
            root.join("database/migrations/20260901000001_billing_create_invoices.sql")
                .is_file(),
            "migration lands name-prefixed after the sqlx integer version"
        );
        let cfg = root.join("config").join("packages").join("billing.toml");
        assert!(cfg.is_file(), "config materializes");
        let cfg_raw = std::fs::read_to_string(&cfg).unwrap();
        assert!(
            cfg_raw.contains("default_monthly_operations = 11"),
            "{cfg_raw}"
        );
        assert!(
            root.join("public/vendor/billing/img/logo.svg").is_file(),
            "assets land under public/vendor/<name>/"
        );
        assert_eq!(
            outcome.view_namespaces,
            vec!["billing".to_string()],
            "views are reported, never copied"
        );
        assert!(!root.join("views").exists(), "no views directory is written");
        assert!(outcome.conflicts.is_empty());
        let written = outcome.written.len();
        assert_eq!(written, 3, "migration + config + asset: {outcome:?}");
        assert_eq!(outcome.exit_code(), ExitCode::SUCCESS);

        // Second run: every target byte-identical (idempotent no-op).
        let again = publish_at(&root, "billing", false, PublishScope::All).expect("republishes");
        assert!(again.written.is_empty());
        assert_eq!(again.unchanged, written);
        assert!(again.conflicts.is_empty());
        assert_eq!(again.exit_code(), ExitCode::SUCCESS);

        // CLI surface: both names parse; --force defaults off.
        let cli = Cli::try_parse_from(["kelson", "publish", "billing", "--force"]).expect("parses");
        match cli.cmd {
            Cmd::Publish { ref package, force } => {
                assert_eq!(package, "billing");
                assert!(force, "--force sets it");
            }
            _ => panic!("publish maps to Publish"),
        }
        let cli = Cli::try_parse_from(["kelson", "publish", "billing"]).expect("parses");
        match cli.cmd {
            Cmd::Publish { force, .. } => assert!(!force, "force defaults off"),
            _ => panic!("publish maps to Publish"),
        }
        let cli = Cli::try_parse_from(["kelson", "config:publish", "billing"]).expect("parses");
        match cli.cmd {
            Cmd::ConfigPublish { ref package, force } => {
                assert_eq!(package, "billing");
                assert!(!force, "force defaults off");
            }
            _ => panic!("config:publish maps to ConfigPublish"),
        }

        std::fs::remove_dir_all(&root).ok();
    }

    /// Wave 8 B-25: an edited target is a reported conflict (skipped,
    /// FAILURE) until --force overwrites it; unindexed names, drifted
    /// entries, malformed migration sources and colliding versions all
    /// refuse fail-closed.
    #[test]
    fn wave8_publish_conflicts_force_and_refusals() {
        let root = fresh_temp("publish-conflict");
        let pkg = seed_publish_package(&root);
        publish_at(&root, "billing", false, PublishScope::All).expect("publishes");

        // Drift one published artifact -> conflict, skipped, FAILURE.
        let mig = root
            .join("database/migrations/20260901000001_billing_create_invoices.sql");
        std::fs::write(&mig, "-- edited by the operator\n").unwrap();
        let outcome = publish_at(&root, "billing", false, PublishScope::All).expect("runs");
        assert_eq!(outcome.conflicts.len(), 1, "{outcome:?}");
        assert_eq!(outcome.unchanged, 2, "config + asset untouched");
        assert_eq!(outcome.exit_code(), ExitCode::FAILURE);
        assert_eq!(
            std::fs::read_to_string(&mig).unwrap(),
            "-- edited by the operator\n",
            "a conflict never overwrites"
        );

        // --force overwrites and clears the conflict.
        let outcome = publish_at(&root, "billing", true, PublishScope::All).expect("runs");
        assert!(outcome.conflicts.is_empty());
        assert_eq!(outcome.exit_code(), ExitCode::SUCCESS);
        assert_ne!(
            std::fs::read_to_string(&mig).unwrap(),
            "-- edited by the operator\n"
        );

        // Unindexed package -> refused with a pointer at the index.
        let err = publish_at(&root, "ghost", false, PublishScope::All).expect_err("refuses");
        assert!(err.to_string().contains("not indexed"), "{err}");

        // Migration source without an integer version prefix -> refused.
        std::fs::write(pkg.join("database/migrations/create_extra.sql"), "-- x\n").unwrap();
        let err = publish_at(&root, "billing", false, PublishScope::All).expect_err("refuses");
        assert!(err.to_string().contains("integer version prefix"), "{err}");
        std::fs::remove_file(pkg.join("database/migrations/create_extra.sql")).unwrap();

        // A file already owns that version -> refused (sqlx keys applied
        // migrations by version, so a duplicate would break kelson migrate).
        let platform = root.join("database/migrations/20260901000001_platform.sql");
        std::fs::write(&platform, "-- platform\n").unwrap();
        let err = publish_at(&root, "billing", false, PublishScope::All).expect_err("refuses");
        assert!(err.to_string().contains("collides"), "{err}");
        std::fs::remove_file(&platform).unwrap();

        // Index entry drifted from its manifest name -> refused.
        std::fs::write(
            root.join("packages.toml"),
            "[[package]]\nname = \"billings\"\nversion = \"0.3.0\"\npath = \"packages/billing\"\n",
        )
        .unwrap();
        let err = publish_at(&root, "billings", false, PublishScope::All).expect_err("refuses");
        assert!(err.to_string().contains("drifted"), "{err}");

        std::fs::remove_dir_all(&root).ok();
    }

    /// Wave 8 B-25: config:publish materializes ONLY the editable config
    /// copy - no migrations, no assets, no view report - through the same
    /// idempotence / conflict / --force cycle; a package without [config]
    /// refuses.
    #[test]
    fn wave8_config_publish_materializes_config_only() {
        let root = fresh_temp("config-publish");
        seed_publish_package(&root);

        let outcome =
            publish_at(&root, "billing", false, PublishScope::ConfigOnly).expect("publishes");
        assert_eq!(outcome.written.len(), 1, "{outcome:?}");
        assert!(root.join("config/packages/billing.toml").is_file());
        assert!(!root.join("database").exists(), "no migrations published");
        assert!(!root.join("public/vendor").exists(), "no assets published");
        assert!(outcome.view_namespaces.is_empty(), "no view report");
        assert_eq!(outcome.exit_code(), ExitCode::SUCCESS);

        // Idempotent no-op.
        let again =
            publish_at(&root, "billing", false, PublishScope::ConfigOnly).expect("reruns");
        assert!(again.written.is_empty());
        assert_eq!(again.unchanged, 1);

        // Conflict + --force on the published copy.
        let cfg = root.join("config/packages/billing.toml");
        std::fs::write(&cfg, "[limits]\ndefault_monthly_operations = 99\n").unwrap();
        let outcome =
            publish_at(&root, "billing", false, PublishScope::ConfigOnly).expect("runs");
        assert_eq!(outcome.conflicts.len(), 1, "{outcome:?}");
        assert_eq!(outcome.exit_code(), ExitCode::FAILURE);
        assert!(
            std::fs::read_to_string(&cfg).unwrap().contains("99"),
            "conflict never overwrites"
        );
        let outcome = publish_at(&root, "billing", true, PublishScope::ConfigOnly).expect("runs");
        assert!(outcome.conflicts.is_empty());
        assert!(
            std::fs::read_to_string(&cfg)
                .unwrap()
                .contains("default_monthly_operations = 11")
        );

        // A package without [config] refuses config-only publish.
        let tax = root.join("packages").join("tax");
        std::fs::create_dir_all(&tax).unwrap();
        std::fs::write(
            tax.join("module.toml"),
            "[module]\nname = \"tax\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        pkg_add_at(&root, "packages/tax").expect("indexes");
        let err = publish_at(&root, "tax", false, PublishScope::ConfigOnly).expect_err("refuses");
        assert!(err.to_string().contains("[config]"), "{err}");

        std::fs::remove_dir_all(&root).ok();
    }

    /// Wave 5 Phase 6: storage:link points public/storage at the local disk
    /// root (storage/local - the host's StorageProvider), whatever the OS
    /// path separator.
    #[test]
    fn storage_link_paths_point_at_the_local_disk_root() {
        let root = fresh_temp("link-paths");
        let (link, target) = storage_link_paths(&root);
        assert_eq!(link, root.join("public").join("storage"));
        assert_eq!(target, root.join("storage").join("local"));
        std::fs::remove_dir_all(&root).ok();
    }

    /// Wave 5 Phase 6: refuses to overwrite without --force; --force replaces
    /// an existing file with the link (skips the symlink assertion where the
    /// environment cannot create symlinks — no Developer Mode on Windows).
    #[test]
    fn storage_link_refuses_to_overwrite_without_force() {
        let root = fresh_temp("link-refuse");
        let (link, target) = storage_link_paths(&root);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::fs::write(&link, b"occupied").unwrap();

        let err = storage_link_at(&root, false).expect_err("refuses without --force");
        assert!(err.to_string().contains("--force"), "{err}");
        assert_eq!(std::fs::read(&link).unwrap(), b"occupied", "untouched");

        match storage_link_at(&root, true) {
            Ok(()) => {
                let meta = std::fs::symlink_metadata(&link).expect("link exists");
                assert!(meta.file_type().is_symlink(), "--force replaces with a link");
                assert_eq!(std::fs::read_link(&link).unwrap(), target);
            }
            Err(e) => eprintln!("skipping symlink assertion: {e}"),
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// Wave 5 Phase 6: a real directory at the link path is refused even
    /// with --force (data-loss guard) and survives the attempt.
    #[test]
    fn storage_link_never_deletes_a_real_directory() {
        let root = fresh_temp("link-dir");
        let (link, _) = storage_link_paths(&root);
        std::fs::create_dir_all(&link).unwrap();
        std::fs::write(link.join("keep.txt"), b"precious").unwrap();

        let err = storage_link_at(&root, true).expect_err("real dir refused even with --force");
        assert!(err.to_string().contains("real directory"), "{err}");
        assert!(link.is_dir(), "directory untouched");
        assert!(link.join("keep.txt").exists(), "contents untouched");
        std::fs::remove_dir_all(&root).ok();
    }

    /// Wave 5 Phase 6: creates public/storage -> storage/local when missing
    /// (creates the target too); skips where symlinks are not permitted.
    #[test]
    fn storage_link_creates_a_missing_link() {
        let root = fresh_temp("link-create");
        let (link, target) = storage_link_paths(&root);
        match storage_link_at(&root, false) {
            Ok(()) => {
                let meta = std::fs::symlink_metadata(&link).expect("link exists");
                assert!(meta.file_type().is_symlink(), "created as a symlink");
                assert_eq!(std::fs::read_link(&link).unwrap(), target);
                assert!(target.is_dir(), "target disk root created");
            }
            Err(e) => eprintln!("skipping: this environment cannot create symlinks: {e}"),
        }
        std::fs::remove_dir_all(&root).ok();
    }

    /// Wave 5 Phase 6: the test wrapper's line parser counts harness results
    /// (plain, doc-test and FAILED shapes) and ignores unrelated lines.
    #[test]
    fn test_result_lines_parse_pass_and_fail() {
        let (ok, passed, failed) = parse_test_result(
            "test result: ok. 1038 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 123.45s",
        )
        .expect("ok line parses");
        assert!((ok, passed, failed) == (true, 1038, 0));

        let (ok, passed, failed) = parse_test_result(
            "test result: FAILED. 12 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s",
        )
        .expect("failed line parses");
        assert!((ok, passed, failed) == (false, 12, 1));

        let (ok, passed, failed) =
            parse_test_result("test result: ok. 3 passed; 0 failed; 0 ignored; ...").expect("doc");
        assert!((ok, passed, failed) == (true, 3, 0));

        assert!(parse_test_result("running 42 tests").is_none(), "unrelated");
        assert!(parse_test_result("").is_none(), "empty");
    }

    /// Wave 5 Phase 6: the DATABASE_URL fallback reads <root>/.env (the env
    /// branch is skipped when the variable is already set).
    #[test]
    fn database_url_falls_back_to_dotenv() {
        if std::env::var_os("DATABASE_URL").is_some() {
            eprintln!("skipping: DATABASE_URL already set in the environment");
            return;
        }
        let root = fresh_temp("dburl");
        assert!(!has_database_url(&root), "no env, no .env -> missing");
        std::fs::write(root.join(".env"), "# comment\nOTHER=x\n").unwrap();
        assert!(!has_database_url(&root), "no DATABASE_URL line -> missing");
        std::fs::write(
            root.join(".env"),
            "OTHER=x\nDATABASE_URL=postgres://user:pw@localhost/keel\n",
        )
        .unwrap();
        assert!(has_database_url(&root), "plain value");
        std::fs::write(root.join(".env"), "DATABASE_URL=\"\"\n").unwrap();
        assert!(!has_database_url(&root), "empty value is not a URL");
        std::fs::write(root.join(".env"), "DATABASE_URL='postgres://x/y'\n").unwrap();
        assert!(has_database_url(&root), "quoted value");
        std::fs::remove_dir_all(&root).ok();
    }

    /// Wave 5 Phase 6: storage:link and test parse under their documented
    /// names with the --force / --pg flags (both default off).
    #[test]
    fn storage_link_and_test_commands_parse() {
        let cli = Cli::try_parse_from(["kelson", "storage:link"]).expect("parses");
        match cli.cmd {
            Cmd::StorageLink { force } => assert!(!force, "force defaults off"),
            _ => panic!("storage:link maps to StorageLink"),
        }
        let cli = Cli::try_parse_from(["kelson", "storage:link", "--force"]).expect("parses");
        match cli.cmd {
            Cmd::StorageLink { force } => assert!(force, "--force sets it"),
            _ => panic!("--force maps to StorageLink"),
        }
        let cli = Cli::try_parse_from(["kelson", "test"]).expect("parses");
        match cli.cmd {
            Cmd::Test { pg } => assert!(!pg, "pg defaults off"),
            _ => panic!("test maps to Test"),
        }
        let cli = Cli::try_parse_from(["kelson", "test", "--pg"]).expect("parses");
        match cli.cmd {
            Cmd::Test { pg } => assert!(pg, "--pg sets it"),
            _ => panic!("--pg maps to Test"),
        }
    }

    /// Wave 5 Phase 7: `kelson call` parses under its documented syntax — name,
    /// optional JSON payload and the --principal/--tenant flags.
    #[test]
    fn call_command_parses_with_documented_flags() {
        let cli = Cli::try_parse_from(["kelson", "call", "book"]).expect("parses");
        match cli.cmd {
            Cmd::Call {
                name,
                input,
                principal,
                tenant,
            } => {
                assert_eq!(name, "book");
                assert!(input.is_none() && principal.is_none() && tenant.is_none());
            }
            _ => panic!("call maps to Call"),
        }
        let cli = Cli::try_parse_from([
            "kelson",
            "call",
            "list",
            "{\"page\":2}",
            "--principal",
            "2f2f2f2f-2f2f-2f2f-2f2f-2f2f2f2f2f2f",
            "--tenant",
            "1f1f1f1f-1f1f-1f1f-1f1f-1f1f1f1f1f1f",
        ])
        .expect("parses");
        match cli.cmd {
            Cmd::Call {
                name,
                input,
                principal,
                tenant,
            } => {
                assert_eq!(name, "list");
                assert_eq!(input.as_deref(), Some("{\"page\":2}"));
                assert!(principal.is_some(), "principal flag");
                assert!(tenant.is_some(), "tenant flag");
            }
            _ => panic!("call with flags maps to Call"),
        }
    }

    /// Wave 5 Phase 7: route/payload pre-flight and the dev/test-only
    /// default-operator gate (docs/19) — all pure, no kelson-demo spawn.
    #[test]
    fn call_pre_flight_resolves_routes_payloads_and_env_gate() {
        let raw = "[[command]]\nname = \"book\"\noperation = \"appointments.book\"\n\n[[command]]\nname = \"list\"\noperation = \"appointments.list\"\n";
        let manifest =
            kelson_registry::RouteFileManifest::from_toml(raw, "routes/console.toml", None)
                .expect("route file parses");
        let commands = manifest.commands;
        let spec = resolve_console_route(&commands, "book").expect("resolves");
        assert_eq!(spec.operation, "appointments.book");
        let err = resolve_console_route(&commands, "boo").expect_err("refuses");
        assert!(err.to_string().contains("book"), "{err}");
        let err = resolve_console_route(&[], "book").expect_err("refuses");
        assert!(err.to_string().contains("routes/console.toml"), "{err}");

        assert_eq!(parse_call_input(None).unwrap(), serde_json::json!({}));
        assert_eq!(parse_call_input(Some("")).unwrap(), serde_json::json!({}));
        assert_eq!(
            parse_call_input(Some("{\"page\":2}")).unwrap(),
            serde_json::json!({ "page": 2 })
        );
        let err = parse_call_input(Some("{bad")).expect_err("refuses");
        assert!(err.to_string().contains("JSON"), "{err}");

        assert!(default_operator_allowed(&Environment::Dev));
        assert!(default_operator_allowed(&Environment::Test));
        assert!(!default_operator_allowed(&Environment::Staging));
        assert!(!default_operator_allowed(&Environment::Prod));
    }

    /// Wave 5 Phase 8: schedule:work/test parse under their documented names —
    /// work defaults to the spec's 60s tick, test takes no flags.
    #[test]
    fn schedule_work_and_test_commands_parse() {
        let cli = Cli::try_parse_from(["kelson", "schedule:work"]).expect("parses");
        match cli.cmd {
            Cmd::ScheduleWork { every } => assert_eq!(every, 60, "spec default tick"),
            _ => panic!("schedule:work maps to ScheduleWork"),
        }
        let cli = Cli::try_parse_from(["kelson", "schedule:work", "--every", "15"]).expect("parses");
        match cli.cmd {
            Cmd::ScheduleWork { every } => assert_eq!(every, 15, "--every overrides"),
            _ => panic!("--every maps to ScheduleWork"),
        }
        let cli = Cli::try_parse_from(["kelson", "schedule:test"]).expect("parses");
        assert!(
            matches!(cli.cmd, Cmd::ScheduleTest),
            "schedule:test maps to ScheduleTest"
        );
    }

    /// Wave 5 Phase 8: the dry-run plan is pure — due items come from
    /// (registry, last-fired, now) with no execution and no state write, and
    /// disabled items never enter the due set.
    #[test]
    fn schedule_test_plan_is_pure_and_skips_disabled() {
        let mut registry = kelson_scheduling::ScheduleRegistry::new();
        for (name, enabled) in [("due-interval", true), ("recently-fired", true), ("disabled", false)] {
            registry
                .register(kelson_scheduling::ScheduleItem {
                    name: name.to_string(),
                    operation: "demo.echo".to_string(),
                    spec: kelson_scheduling::ScheduleSpec::Interval { secs: 60 },
                    payload: serde_json::json!({}),
                    enabled,
                    ..Default::default()
                })
                .expect("registers");
        }
        let now = time::macros::datetime!(2026-03-01 12:00:00 UTC);
        let state = vec![("recently-fired".to_string(), now - time::Duration::seconds(5))];
        let plan = schedule_test_plan(&registry, &state, now).expect("plans");
        assert_eq!(
            plan.registered,
            ["due-interval", "recently-fired", "disabled"],
            "registry lists every item"
        );
        assert_eq!(plan.due.len(), 1, "only the never-fired enabled item");
        assert_eq!(plan.due[0].0, "due-interval");

        let lines = schedule_test_lines(&plan);
        assert!(
            lines[0].contains("3 registered, 1 due now"),
            "{:?}",
            lines
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains("would fire: due-interval -> demo.echo")),
            "{:?}",
            lines
        );
        assert!(
            lines.iter().any(|l| l.contains("not due: recently-fired, disabled")),
            "{:?}",
            lines
        );
    }

    /// Wave 5 Phase 8: an empty registry reports the documented "none"
    /// posture (the schedule:list / model:prune shape).
    #[test]
    fn schedule_test_lines_report_the_empty_posture() {
        let registry = kelson_scheduling::ScheduleRegistry::new();
        let plan =
            schedule_test_plan(&registry, &[], time::OffsetDateTime::now_utc()).expect("plans");
        let lines = schedule_test_lines(&plan);
        assert_eq!(lines.len(), 1);
        assert!(
            lines[0].contains("no schedules registered yet"),
            "{}",
            lines[0]
        );
    }

    /// Wave 5 Phase 8: one daemon pass fires due schedules as durable jobs,
    /// persists last-fired state (so neither a re-tick nor a fresh runner
    /// seeded from disk re-fires), and skips items whose interval has not
    /// elapsed.
    #[test]
    fn schedule_tick_fires_due_items_and_persists_state() {
        use kelson_jobs::JobStore;
        fn registry() -> kelson_scheduling::ScheduleRegistry {
            let mut registry = kelson_scheduling::ScheduleRegistry::new();
            registry
                .register(kelson_scheduling::ScheduleItem {
                    name: "due-interval".to_string(),
                    operation: "demo.echo".to_string(),
                    spec: kelson_scheduling::ScheduleSpec::Interval { secs: 60 },
                    payload: serde_json::json!({"n": 1}),
                    enabled: true,
                    ..Default::default()
                })
                .expect("registers");
            registry
        }
        let root = phase5_root("schedule-tick");
        let runner =
            kelson_scheduling::SchedulerRunner::with_state(Arc::new(registry()), Vec::new());
        let store = kelson_jobs::InMemoryJobStore::new();
        let now = time::macros::datetime!(2026-03-01 12:00:00 UTC);

        let fired = schedule_tick(&runner, &store, &root, now).expect("tick fires");
        assert_eq!(fired, ["due-interval"]);
        // The firing is a real durable job named after the operation.
        let claimed = store
            .claim("schedule-tick-worker")
            .expect("claim works")
            .expect("job enqueued");
        assert_eq!(claimed.name, "demo.echo");

        // Same runner, interval not elapsed: nothing fires.
        let second = schedule_tick(&runner, &store, &root, now + time::Duration::seconds(1))
            .expect("tick");
        assert!(second.is_empty(), "interval not elapsed");

        // State was persisted: a fresh runner seeded from disk does not
        // re-fire either.
        let state = kelson_scheduling::load_state(&root).expect("state loads");
        assert_eq!(state.len(), 1);
        assert_eq!(state[0].0, "due-interval");
        let fresh =
            kelson_scheduling::SchedulerRunner::with_state(Arc::new(registry()), state);
        let third = schedule_tick(&fresh, &store, &root, now + time::Duration::seconds(10))
            .expect("tick");
        assert!(third.is_empty(), "persisted state prevents re-fire");
    }

    /// Wave 5 Phase 8: with the (always-empty today) registry both commands
    /// report the none posture and exit successfully — schedule:work does so
    /// BEFORE touching the database, schedule:test writes nothing.
    #[test]
    fn schedule_commands_exit_cleanly_when_nothing_is_registered() {
        assert!(schedule_work(60).is_ok(), "work: none posture, no database");
        assert!(schedule_test().is_ok(), "test: none posture, read-only");
    }

    /// Wave 4 Phase 5: temp workspace with a schema-clean config (secrets,
    /// database URL password, views roots) for the config/env/cache tests.
    fn phase5_root(tag: &str) -> std::path::PathBuf {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kelson-console-{tag}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::write(
            dir.join("config").join("dev.toml"),
            r#"[app]
name = "kelson"
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
        let path = kelson_registry::config_cache_path(&dir);
        assert!(path.exists(), "artifact written");
        let artifact = kelson_registry::ConfigCacheArtifact::load(&path)
            .expect("artifact loads")
            .expect("artifact present");
        assert_eq!(artifact.abi, kelson_registry::CONFIG_CACHE_ABI);
        assert_eq!(artifact.config["app"]["env"], "dev");
        assert_eq!(
            artifact.config_hash,
            kelson_registry::config_hash(&artifact.config),
            "stored hash matches the stored config"
        );

        // Boot gate: matching pin accepted, drift refused with a pointer.
        kelson_registry::config_cache_check(&dir, &artifact.config_hash)
            .expect("pin accepted");
        let stale = kelson_registry::config_hash(&serde_json::json!({"app": {}}));
        let err = kelson_registry::config_cache_check(&dir, &stale)
            .expect_err("stale pin refuses boot");
        assert!(
            err.contains(kelson_registry::CONFIG_CACHE_CODE) && err.contains("config:cache"),
            "{err}"
        );

        // Config file edit -> re-derived hash differs -> the old pin goes
        // stale (the live drift path, end to end).
        let cfg_path = dir.join("config").join("dev.toml");
        let raw = std::fs::read_to_string(&cfg_path).unwrap();
        std::fs::write(&cfg_path, raw.replace(r#"name = "kelson""#, r#"name = "renamed""#))
            .unwrap();
        let fresh = load_config(&dir)
            .expect("edited config loads")
            .freeze()
            .cache_json();
        let fresh_hash = kelson_registry::config_hash(&fresh);
        assert_ne!(fresh_hash, artifact.config_hash, "edit changes the hash");
        kelson_registry::config_cache_check(&dir, &fresh_hash)
            .expect_err("the old pin refuses the edited config");

        // config:clear -> artifact gone (boot reads the live config again).
        assert_eq!(
            config_clear_at(&dir).expect("config clear runs"),
            ExitCode::SUCCESS
        );
        assert!(!path.exists(), "artifact removed");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Phase 3 §4.1: config:reload validates (schema gate), prints the
    /// effective hash, and diffs it against the pinned artifact — match,
    /// stale, and absent-pin states.
    #[test]
    fn config_reload_reports_pin_state() {
        let dir = phase5_root("configreload");
        // Absent pin: valid config, live path.
        assert_eq!(
            config_reload_at(&dir).expect("reload runs without a pin"),
            ExitCode::SUCCESS
        );

        // Pinned + matching: reload reports the match.
        assert_eq!(
            config_cache_at(&dir).expect("config cache runs"),
            ExitCode::SUCCESS
        );
        assert_eq!(
            config_reload_at(&dir).expect("reload runs with a pin"),
            ExitCode::SUCCESS
        );

        // Edited config: the pin is stale (reload still succeeds — the
        // report is advisory; the refusal lives in the boot gate).
        let cfg_path = dir.join("config").join("dev.toml");
        let raw = std::fs::read_to_string(&cfg_path).unwrap();
        std::fs::write(&cfg_path, raw.replace(r#"name = "kelson""#, r#"name = "renamed""#))
            .unwrap();
        assert_eq!(
            config_reload_at(&dir).expect("reload runs on drift"),
            ExitCode::SUCCESS
        );
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

    /// Wave 4 Phase 5: `kelson env` summarizes the environment with secrets
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
        // Wave 6 P2: the disk registry surface (`storage.default`, disk names).
        assert!(
            text.contains("storage: endpoint=") && text.contains("default=local"),
            "{text}"
        );
        assert!(text.contains("disks=(none)"), "{text}");
        // Wave 6 P3: the i18n surface (default locale + message roots).
        assert!(
            text.contains("i18n: default=en paths=lang"),
            "{text}"
        );

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
        let routes = kelson_registry::route_cache_path(&dir);
        std::fs::create_dir_all(routes.parent().unwrap()).unwrap();
        std::fs::write(&routes, "{}").unwrap();

        assert_eq!(
            cache_forget_at(&dir, "routes").expect("forget runs"),
            ExitCode::SUCCESS
        );
        assert!(!routes.exists(), "routes key gone");
        assert!(
            kelson_registry::config_cache_path(&dir).exists(),
            "the config key survives"
        );

        let err = cache_forget_at(&dir, "routes").expect_err("missing key fails");
        assert!(err.to_string().contains("not found"), "{err}");
        let err = cache_forget_at(&dir, "ghost").expect_err("unknown key fails");
        assert!(err.to_string().contains("views:<template>"), "{err}");

        cache_forget_at(&dir, "config").expect("forget config");
        assert!(!kelson_registry::config_cache_path(&dir).exists());
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
            "kelson-console-routelist-{}-{}",
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

    /// Wave 4 Phase 8 (docs/19 L222): `kelson about` golden fields — every
    /// section the Laravel-style report promises, built from config + the
    /// compiled graph + the workspace dependency table. The database line
    /// reports the driver from the DSN scheme and degrades (the server
    /// version ping is absent here) instead of failing; secrets are never
    /// rendered.
    #[test]
    fn about_report_carries_the_golden_fields() {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "kelson-console-about-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::create_dir_all(dir.join("applications").join("alpha")).unwrap();
        std::fs::write(
            dir.join("Cargo.toml"),
            r#"[workspace]
members = ["tools/kelson-console"]

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
        assert!(text.contains("kelson about"), "{text}");
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
        let registered = kelson_feature_flags::platform_registry().definitions().len();
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

    /// Phase R: every local command that boots the `kelson-demo` harness must
    /// pass `--features demo`. Phase P made the demo modules opt-in so the
    /// harness could be published, but left `module.demo` declaring
    /// dependencies on providers that no longer exist in a non-demo build — so
    /// without the feature the boot died on `UnknownDependency` and took
    /// `serve`/`call`/`shell`/`model:prune` down with it. This pins the flag.
    #[test]
    fn demo_run_args_enable_the_demo_feature() {
        let args = demo_run_args();
        assert_eq!(&args[..4], ["run", "--quiet", "-p", "kelson-demo"]);
        let feat = args
            .iter()
            .position(|a| *a == "--features")
            .expect("--features present");
        assert_eq!(args.get(feat + 1), Some(&"demo"));
        // The feature must be a *cargo* arg, so it has to precede any `--`
        // that hands the rest to the binary.
        assert!(
            args.iter().position(|a| *a == "--").is_none_or(|dd| dd > feat),
            "the demo feature is passed to cargo, not to kelson-demo"
        );
    }

    /// Wave 4 Phase 9 (B-27): `kelson profile` parses per target, the samply
    /// command line records the built binary, and a missing samply fails
    /// closed with the install line + the docs/13 workflow pointer (never
    /// a silent no-op).
    #[test]
    fn profile_cli_parses_and_missing_samply_fails_closed() {
        let cli = Cli::try_parse_from(["kelson", "profile", "api"]).expect("api parses");
        let Cmd::Profile { target: api } = cli.cmd else {
            panic!("profile api parses");
        };
        assert_eq!(api.name(), "kelson-demo");
        let cli = Cli::try_parse_from(["kelson", "profile", "worker"]).expect("worker parses");
        let Cmd::Profile { target: worker } = cli.cmd else {
            panic!("profile worker parses");
        };
        assert_eq!(worker.name(), "kelson-worker");

        // samply command line: `samply record <root>/target/debug/<bin>`.
        let root = std::path::Path::new(".");
        let binary = root.join("target").join("debug").join("kelson-demo");
        let (program, args) = profile_invocation(&binary);
        assert_eq!(program, "samply");
        assert_eq!(args.len(), 2, "{args:?}");
        assert_eq!(args[0], "record");
        assert!(args[1].ends_with("kelson-demo"), "{args:?}");
        assert!(args[1].contains("target"), "{args:?}");

        // Missing samply: guidance carries install + docs pointer, and the
        // run fails closed.
        let guidance = profile_guidance(&ProfileTarget::Api);
        assert!(guidance.contains("samply not found"), "{guidance}");
        assert!(guidance.contains("cargo install samply"), "{guidance}");
        assert!(guidance.contains("docs/13-OPERATIONS.md"), "{guidance}");
        assert!(guidance.contains("kelson-demo"), "{guidance}");
        let dir =
            std::env::temp_dir().join(format!("kelson-console-profile-{}", std::process::id()));
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
            "kelson-console-viewcache-{}-{}",
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
