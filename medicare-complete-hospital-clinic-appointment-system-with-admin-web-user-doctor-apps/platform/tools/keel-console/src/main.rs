//! keel — the platform CLI (11-SPEC §34, item 29). Implemented: doctor,
//! check, graph/modules/route:list/event:list/capabilities, make:module,
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
    /// Clear the application cache.
    #[command(name = "cache:clear")]
    CacheClear,
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
    /// List registered schedules.
    #[command(name = "schedule:list")]
    ScheduleList,
    /// Run due schedules once.
    #[command(name = "schedule:run")]
    ScheduleRun,
    /// Database inventory: tables + row counts.
    #[command(name = "db:inspect")]
    DbInspect,
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
        Cmd::RouteCache => route_cache(),
        Cmd::RouteClear => route_clear(),
        Cmd::EventList => event_list(),
        Cmd::Capabilities => capabilities_list(),
        Cmd::McpTools { format } => mcp_tools(&format),
        Cmd::SdkOpenapi { output } => sdk_openapi(output),
        Cmd::Migrate => rt(migrate()),
        Cmd::MigrateStatus => rt(migrate_status()),
        Cmd::MigrateFresh => rt(migrate_fresh()),
        Cmd::Seed { only } => rt(seed(only)),
        Cmd::KeyGenerate => key_generate(),
        Cmd::ConfigDump => config_dump(),
        Cmd::CacheClear => cache_clear(),
        Cmd::Up => up(),
        Cmd::Down { retry, secret } => down(retry, secret),
        Cmd::JobList { status, limit } => rt(job_list(status, limit)),
        Cmd::QueueWork { once } => queue_work(once),
        Cmd::QueueFailed => rt(queue_failed()),
        Cmd::QueueRetry { id } => rt(queue_retry(&id)),
        Cmd::ScheduleList => schedule_info("schedule:list"),
        Cmd::ScheduleRun => schedule_info("schedule:run"),
        Cmd::DbInspect => rt(db_inspect()),
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

fn migrator() -> &'static sqlx::migrate::Migrator {
    // Embedded from database/migrations, same source keel-postgres boots with.
    static MIGRATOR: std::sync::OnceLock<sqlx::migrate::Migrator> = std::sync::OnceLock::new();
    MIGRATOR.get_or_init(|| sqlx::migrate!("../../database/migrations"))
}

async fn migrate() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    keel_postgres::run_migrations(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
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
    sqlx::query("DROP SCHEMA public CASCADE")
        .execute(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("drop schema: {e}"))?;
    sqlx::query("CREATE SCHEMA public")
        .execute(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("create schema: {e}"))?;
    keel_postgres::run_migrations(&pool)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("schema dropped, recreated and migrated.");
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

fn queue_work(once: bool) -> anyhow::Result<ExitCode> {
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

// ---------------------------------------------------------------- schedules

fn schedule_info(command: &str) -> anyhow::Result<ExitCode> {
    println!("{command}: no module schedules registered yet.");
    println!("Schedules are declared by modules through ScheduleRegistry (keel-scheduling); the framework surface is in place, module content lands with S3.");
    Ok(ExitCode::SUCCESS)
}

// -------------------------------------------------------------- db inspect

async fn db_inspect() -> anyhow::Result<ExitCode> {
    let root = workspace_root()?;
    let pool = db_pool(&root).await?;
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname = 'public' ORDER BY tablename",
    )
    .fetch_all(&pool)
    .await
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    println!("public schema: {} table(s)", tables.len());
    for table in &tables {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM \"{table}\""))
            .fetch_one(&pool)
            .await
            .map_err(|e| anyhow::anyhow!("count {table}: {e}"))?;
        println!("  {:<24} {count}", table);
    }
    Ok(ExitCode::SUCCESS)
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

#[cfg(test)]
mod tests {
    use super::*;

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
