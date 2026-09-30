//! keel-worker: durable job runner host (11-SPEC §29/§41, item 23). Boots the
//! same lifecycle host as keel-api, then runs a claim loop: claims due jobs,
//! dispatches them to registered handlers, records completion or failure
//! (retry budget and backoff live in the store), and drains the outbox via a
//! concurrent pump. Degraded boot (D-52): without postgres the worker runs on
//! the in-memory store and only sees jobs enqueued in-process.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use clap::Parser;
use keel_config::{Environment, FrozenConfig, PlatformConfig};
use keel_foundation::{codes, AppError, ErrorCategory, Retryability, Severity};
use keel_event::{ListenerRegistry, OutboxPump, OutboxStore, OutboxStoreService};
use keel_jobs::{
    BatchRuntime, BoxFuture, ChainRuntime, DurableJob, InMemoryJobStore, JobDef, JobHandler,
    JobRegistry, JobStatus, JobStore, JobStoreAdmin, JobStoreAdminService, JobStoreService,
    WorkerRegistry,
};
use keel_lifecycle::{ApplicationHost, BootError, Phase, Provider, ServiceRegistry};
use keel_scheduling::{ScheduleItem, ScheduleRegistry, SchedulerRunner};
use time::OffsetDateTime;

#[derive(Debug, Parser)]
#[command(name = "keel-worker", about = "Keel durable job worker")]
struct Args {
    /// Claim until the queues drain, then exit.
    #[arg(long)]
    once: bool,
    /// Idle poll interval in milliseconds when no job is claimable.
    #[arg(long, default_value_t = 3000)]
    poll_ms: u64,
    /// Worker identity recorded on claimed jobs (claimed_by).
    #[arg(long)]
    worker_name: Option<String>,
    /// Exit after processing this many jobs (0 = unlimited).
    #[arg(long, default_value_t = 0)]
    max_jobs: usize,
}

/// Job store fallback chain shared by the REDIS_URL failure path and the
/// no-redis default (S10 B-13).
fn fallback_job_stores(
    pool: &Option<keel_postgres::PgPool>,
) -> (Arc<dyn JobStore>, Arc<dyn JobStoreAdmin>) {
    match pool {
        Some(pool) => {
            let store = Arc::new(keel_postgres::PgJobStore::new(pool.clone()));
            (store.clone(), store)
        }
        None => {
            let store = Arc::new(InMemoryJobStore::new());
            (store.clone(), store)
        }
    }
}

struct ConfigProvider {
    config: FrozenConfig,
}

impl Provider for ConfigProvider {
    fn name(&self) -> &str {
        "worker.config"
    }
    fn phase(&self) -> Phase {
        Phase::Foundation
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        r.insert("platform.config", self.config.shared())
    }
}

/// Kernel phase: job store ports (+ outbox when a database pool exists),
/// plus the S3 group runtimes (batches/chains/scheduler).
struct KernelProvider {
    jobs: Arc<dyn JobStore>,
    jobs_admin: Arc<dyn JobStoreAdmin>,
    outbox: Option<Arc<dyn OutboxStore>>,
    pump: Option<Arc<OutboxPump>>,
    batches: Arc<BatchRuntime>,
    chains: Arc<ChainRuntime>,
    scheduler: Arc<SchedulerRunner>,
}

impl Provider for KernelProvider {
    fn name(&self) -> &str {
        "worker.kernel"
    }
    fn phase(&self) -> Phase {
        Phase::Kernel
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        r.insert("jobs.store", Arc::new(JobStoreService(self.jobs.clone())))?;
        r.insert(
            "jobs.admin",
            Arc::new(JobStoreAdminService(self.jobs_admin.clone())),
        )?;
        if let Some(store) = &self.outbox {
            r.insert("outbox.store", Arc::new(OutboxStoreService(store.clone())))?;
        }
        if let Some(pump) = &self.pump {
            r.insert("outbox.pump", pump.clone())?;
        }
        r.insert("jobs.batches", self.batches.clone())?;
        r.insert("jobs.chains", self.chains.clone())?;
        r.insert("jobs.scheduler", self.scheduler.clone())?;
        Ok(())
    }
}

/// Modules phase: handler and listener registries.
struct ModulesProvider {
    handlers: Arc<WorkerRegistry>,
    listeners: Arc<ListenerRegistry>,
}

impl Provider for ModulesProvider {
    fn name(&self) -> &str {
        "worker.modules"
    }
    fn phase(&self) -> Phase {
        Phase::Modules
    }
    fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
        r.insert("worker.registry", self.handlers.clone())?;
        r.insert("listener.registry", self.listeners.clone())
    }
}

/// Built-in smoke handler: logs the payload and succeeds. Real modules
/// register their own handlers; this one proves the claim/dispatch path.
struct DemoJobHandler;

impl JobHandler for DemoJobHandler {
    fn name(&self) -> &str {
        "demo.job"
    }
    fn handle<'a>(&'a self, job: &'a DurableJob) -> BoxFuture<'a, Result<(), AppError>> {
        Box::pin(async move {
            tracing::info!(
                job_id = %job.id,
                execution_id = %job.execution_id,
                attempt = job.attempt,
                payload = %job.payload,
                "demo.job executed"
            );
            Ok(())
        })
    }
}

/// Enforce the job execution deadline; an overrun counts as a failed attempt
/// so the store applies backoff and the retry budget.
async fn deadline_guard(
    job: &DurableJob,
    fut: BoxFuture<'_, Result<(), AppError>>,
) -> Result<(), AppError> {
    let remaining = job.deadline - OffsetDateTime::now_utc();
    let wait = std::time::Duration::try_from(if remaining.is_positive() {
        remaining
    } else {
        time::Duration::seconds(1)
    })
    .unwrap_or_else(|_| std::time::Duration::from_secs(1));
    match tokio::time::timeout(wait, fut).await {
        Ok(result) => result,
        Err(_) => Err(AppError::new(
            codes::DEADLINE,
            ErrorCategory::Workflow,
            Severity::Medium,
            Retryability::Retryable,
            "job exceeded its execution deadline",
        )),
    }
}

/// Wave 6 P1: convert the `[mail]` section into the adapter's failover chain.
/// The conversion lives in the runtime because the layer matrix forbids
/// adapters -> application; an unknown chain member, a duplicate name, or an
/// smtp mailer without a host refuses boot (the chain builder owns those
/// checks, `MailChain::new`).
fn mail_chain(section: &keel_config::MailSection) -> Result<keel_mail_smtp::MailChain, AppError> {
    use keel_mail_smtp::{MailerSpec, TransportKind};
    let specs = section
        .mailers
        .iter()
        .map(|(name, mailer)| MailerSpec {
            name: name.clone(),
            transport: match mailer.transport {
                keel_config::MailerTransport::Log => TransportKind::Log,
                keel_config::MailerTransport::Smtp => TransportKind::Smtp,
            },
            host: mailer.host.clone(),
            port: mailer.port,
            username: mailer.username.clone(),
            password: mailer.password.clone(),
            from: mailer.from.clone(),
            timeout_ms: mailer.timeout_ms,
        })
        .collect();
    keel_mail_smtp::MailChain::new(&section.default, specs)
}

/// Wave 6 P1: the queued-mail job def + handler every worker boots with,
/// built from the frozen config. Kept out of `main` so the registration
/// surface is unit-testable. Storage is `None` until the disk registry
/// lands (P2): a `mail.send` carrying attachments then fails explicitly at
/// run time instead of silently dropping them.
fn mail_job(
    cfg: &FrozenConfig,
) -> Result<(JobDef, keel_mail_smtp::MailSendHandler), AppError> {
    let handler = keel_mail_smtp::MailSendHandler::new(mail_chain(cfg.mail())?, None);
    Ok((JobDef::new(keel_mail_smtp::MAIL_SEND_JOB), handler))
}

/// One claim+dispatch pass. Returns the claimed job, if any. Batch/chain
/// runtimes are notified on terminal outcomes (L33/L34, S3).
async fn dispatch_once(
    registry: &WorkerRegistry,
    store: &dyn JobStore,
    worker_name: &str,
    batches: &BatchRuntime,
    chains: &ChainRuntime,
) -> Result<Option<DurableJob>, AppError> {
    let Some(job) = store.claim(worker_name)? else {
        return Ok(None);
    };
    let id = job.id;
    let name = job.name.clone();
    let started = std::time::Instant::now();
    let outcome = match registry.get(&name) {
        None => Err(AppError::not_found(format!(
            "no handler registered for {name}"
        ))),
        Some(handler) => deadline_guard(&job, handler.handle(&job)).await,
    };
    match outcome {
        Ok(()) => {
            store.complete(id)?;
            if let Some(run) = batches.finish_by_job(id, false)? {
                tracing::info!(batch_id = %run.id, batch = %run.name, "batch finished");
            }
            match chains.finish_by_job(id, true) {
                Ok(Some(next_id)) => tracing::info!(chain_next = %next_id, "chain advanced"),
                Ok(None) => {}
                Err(e) => tracing::warn!(error = %e.public_message, "chain advance failed"),
            }
            tracing::info!(
                job_id = %id,
                job = %name,
                ms = started.elapsed().as_millis() as u64,
                "job succeeded"
            );
        }
        Err(e) => {
            let next = store.fail(id, &e.public_message)?;
            if next == JobStatus::Dead {
                if let Some(run) = batches.finish_by_job(id, true)? {
                    tracing::warn!(batch_id = %run.id, batch = %run.name, ok = run.ok(), "batch finished with dead member");
                }
                if let Err(e) = chains.finish_by_job(id, false) {
                    tracing::warn!(error = %e.public_message, "chain halt failed");
                }
            }
            tracing::warn!(
                job_id = %id,
                job = %name,
                error = %e.public_message,
                next = ?next,
                "job failed"
            );
        }
    }
    Ok(Some(job))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let cfg_path = std::env::var("KEEL_CONFIG").unwrap_or_else(|_| "config/dev.toml".into());
    let cfg =
        PlatformConfig::load(Some(std::path::Path::new(&cfg_path)))?.freeze();

    // Sentinel (S5): the telemetry runtime replaces S1 `init_logging` as the
    // single logging init point. Layering: compiled defaults <- `[telemetry]`
    // section <- env overlay; the S1 env conventions stay honored (RUST_LOG,
    // KEEL_LOG_FILE, environment-driven console format).
    let mut telemetry_config =
        keel_observability::TelemetryConfig::from_section(cfg.telemetry());
    if cfg
        .telemetry()
        .and_then(|s| s.get("format"))
        .is_none()
    {
        telemetry_config.format = if cfg.app().env.is_prod()
            || matches!(cfg.app().env, Environment::Staging)
        {
            "json".into()
        } else {
            "pretty".into()
        };
    }
    if let Ok(level) = std::env::var("RUST_LOG") {
        telemetry_config.level = level;
    }
    if let Ok(path) = std::env::var("KEEL_LOG_FILE") {
        telemetry_config.exporters.insert(
            "file".into(),
            keel_observability::ExporterConfig {
                kind: "file".into(),
                target: Some(path),
                format: None,
                critical: true,
                partition_by_tenant: false,
                retry: None,
                breaker: None,
            },
        );
    }
    telemetry_config.apply_env(); // KEEL_TELEMETRY_* wins over the S1 knobs

    let env = cfg.app().env.clone();
    // Wave 4 Phase 5: frozen here — post-boot mutation is impossible by type.

    // Resolve telemetry storage paths against the workspace root, then
    // construct + start the runtime BEFORE the lifecycle host boots so
    // boot-time events flow through it; the provider boot() is idempotent.
    let root = std::env::current_dir()?;
    telemetry_config.rebase(&root);
    // Wave 5: queue:restart signals written after this point stop the worker
    // between jobs; stale files from before boot are ignored (no restart
    // loop when queue:listen respawns us).
    let worker_started = std::time::SystemTime::now();
    let telemetry_runtime = keel_observability::ObservabilityRuntime::new(
        telemetry_config,
        keel_observability::ServiceIdentity::new("keel-worker", env.as_str()),
        &[],
    )
    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    telemetry_runtime
        .start()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    // Database probe (same semantics as keel-api; degraded boot D-52).
    let pool = match cfg.database() {
        Some(db) => {
            let probe = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                keel_postgres::create_pool(&db.url, db.max_connections),
            )
            .await;
            match probe {
                Ok(Ok(pool)) => {
                    if db.run_migrations {
                        keel_postgres::run_migrations(&pool, &root).await?;
                    }
                    Some(pool)
                }
                other => {
                    if db.required {
                        eprintln!("boot refused: database required but unreachable ({other:?})");
                        std::process::exit(1);
                    }
                    tracing::warn!("database unreachable; running on in-memory job store");
                    None
                }
            }
        }
        None => None,
    };

    let worker_name = args
        .worker_name
        .clone()
        .unwrap_or_else(|| format!("keel-worker-{}", std::process::id()));

    // Stores (S10 B-13): REDIS_URL present + reachable -> Redis job store;
    // else postgres when reachable; else in-memory (D-52 degraded boot).
    let (jobs, jobs_admin): (Arc<dyn JobStore>, Arc<dyn JobStoreAdmin>) =
        if let Ok(url) = std::env::var("REDIS_URL") {
            match keel_cache_redis::RedisJobStore::connect(&url) {
                Ok(store) => {
                    tracing::info!("job store: redis");
                    let store = Arc::new(store);
                    (store.clone(), store)
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e.public_message,
                        "redis unreachable for job store; falling back"
                    );
                    fallback_job_stores(&pool)
                }
            }
        } else {
            fallback_job_stores(&pool)
        };

    // S3 enterprise runtime: batches, chains, scheduler tick (L33/L34/L35).
    let batches = Arc::new(BatchRuntime::new(jobs.clone()));
    let chains = Arc::new(ChainRuntime::new(jobs.clone()));
    let schedules = Arc::new(ScheduleRegistry::new());
    // Wave 6 P1: the queued-mail def + handler, built from `[mail]` (an
    // unknown chain member or a malformed mailer refuses boot right here).
    let (mail_def, mail_handler) = mail_job(&cfg)?;
    // Modules register schedules + job defs here at boot; the framework ships
    // the runner so the first module only adds entries, not machinery. The
    // def registration makes `mail.send` buildable through the registry
    // (def-owned retry budget) for queued-send producers.
    let mut job_defs = JobRegistry::new();
    job_defs.register(mail_def)?;
    let job_defs = Arc::new(job_defs);
    let scheduler = Arc::new(SchedulerRunner::new(schedules.clone()));

    let listeners = Arc::new(ListenerRegistry::new());
    let outbox: Option<Arc<dyn OutboxStore>> = pool.as_ref().map(|pool| {
        Arc::new(keel_postgres::PgOutboxStore::new(pool.clone())) as Arc<dyn OutboxStore>
    });
    let pump: Option<Arc<OutboxPump>> = outbox
        .as_ref()
        .map(|store| Arc::new(OutboxPump::new(store.clone(), listeners.clone())));

    let mut handlers = WorkerRegistry::new();
    handlers.register(Arc::new(DemoJobHandler))?;
    // Wave 6 P1: `mail.send` dispatch; storage stays `None` until the disk
    // registry lands (P2), so attachment mail fails explicitly at run time.
    handlers.register(Arc::new(mail_handler))?;
    let handlers = Arc::new(handlers);

    let mut host = ApplicationHost::new();
    // Sentinel runtime is adopted, not rebuilt, by the provider (boot() is
    // idempotent; the runtime was started above for boot-time logging).
    host.add_provider(Box::new(
        keel_observability::ObservabilityProvider::from_runtime(telemetry_runtime.clone()),
    ));
    host.add_provider(Box::new(ConfigProvider {
        config: cfg.clone(),
    }));
    host.add_provider(Box::new(KernelProvider {
        jobs: jobs.clone(),
        jobs_admin,
        outbox,
        pump: pump.clone(),
        batches: batches.clone(),
        chains: chains.clone(),
        scheduler: scheduler.clone(),
    }));
    host.add_provider(Box::new(ModulesProvider {
        handlers: handlers.clone(),
        listeners,
    }));
    host.boot()
        .await
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    tracing::info!(
        services = ?host.registry().service_names(),
        worker = %worker_name,
        degraded = pool.is_none(),
        "keel-worker booted"
    );

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        tokio::spawn(async move {
            let _ = tokio::signal::ctrl_c().await;
            tracing::info!("shutdown signal received; in-flight job will finish");
            stop.store(true, Ordering::SeqCst);
        });
    }

    let poll = std::time::Duration::from_millis(args.poll_ms.max(50));
    let processed = AtomicUsize::new(0);

    // Scheduler tick (L35): every 30s fire due schedules as durable jobs.
    let scheduler_handle = {
        let scheduler = scheduler.clone();
        let job_defs = job_defs.clone();
        let jobs = jobs.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                let now = OffsetDateTime::now_utc();
                let job_defs = job_defs.clone();
                let jobs = jobs.clone();
                match scheduler.tick(now, move |item: &ScheduleItem| {
                    let def = job_defs.get(&item.operation).cloned().unwrap_or_else(|| {
                        let mut def = JobDef::new(item.operation.clone());
                        def.max_attempts = 1;
                        def
                    });
                    let job = keel_jobs::build_from_def(&def, item.payload.clone(), now);
                    jobs.enqueue(job)?;
                    Ok(())
                }) {
                    Ok(fired) if !fired.is_empty() => {
                        tracing::info!(fired = ?fired, "scheduler tick");
                    }
                    Err(e) => tracing::warn!(error = %e.public_message, "scheduler tick failed"),
                    _ => {}
                }
            }
        })
    };

    // Concurrent outbox drain (only meaningful with a database-backed store).
    let pump_handle = pump.as_ref().map(|pump| {
        let pump = pump.clone();
        let stop = stop.clone();
        tokio::spawn(async move {
            while !stop.load(Ordering::SeqCst) {
                match pump.run_once().await {
                    Ok(report) if report.fetched > 0 => tracing::info!(
                        fetched = report.fetched,
                        dispatched = report.dispatched,
                        failed = report.failed,
                        "outbox pump pass"
                    ),
                    Err(e) => tracing::warn!(error = %e.public_message, "outbox pump pass failed"),
                    Ok(_) => {}
                }
                tokio::time::sleep(poll).await;
            }
        })
    });

    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        // Cooperative queue:restart stop — checked between jobs so an
        // in-flight dispatch always finishes first.
        if keel_jobs::queue_restart_requested(&root, worker_started) {
            tracing::info!("queue:restart signal observed; stopping after the current job");
            stop.store(true, Ordering::SeqCst);
            break;
        }
        match dispatch_once(&handlers, &*jobs, &worker_name, &batches, &chains).await {
            Ok(Some(_)) => {
                let done = processed.fetch_add(1, Ordering::SeqCst) + 1;
                if args.max_jobs > 0 && done >= args.max_jobs {
                    tracing::info!(count = done, "max-jobs reached; exiting");
                    break;
                }
            }
            Ok(None) => {
                if args.once {
                    break;
                }
                tokio::time::sleep(poll).await;
            }
            Err(e) => {
                tracing::error!(error = %e.public_message, "claim loop error; backing off");
                if args.once {
                    break;
                }
                tokio::time::sleep(poll).await;
            }
        }
    }

    // Final drain for bounded runs.
    if let Some(pump) = &pump {
        match pump.run_once().await {
            Ok(report) if report.fetched > 0 => {
                tracing::info!(dispatched = report.dispatched, "final outbox drain")
            }
            Err(e) => tracing::warn!(error = %e.public_message, "final outbox drain failed"),
            Ok(_) => {}
        }
    }
    if let Some(handle) = pump_handle {
        handle.abort();
    }
    scheduler_handle.abort();
    tracing::info!(
        jobs = processed.load(Ordering::SeqCst),
        "keel-worker stopped"
    );
    // Drain telemetry before exit (S5 graceful shutdown: flush buffers + WAL
    // within the deadline; exporters close even on timeout).
    telemetry_runtime.shutdown(std::time::Duration::from_secs(5));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_config::{MailerConfig, MailerTransport, MailSection};

    fn section(default: &str) -> MailSection {
        MailSection {
            default: default.into(),
            ..Default::default()
        }
    }

    fn mailer(transport: MailerTransport, host: &str) -> MailerConfig {
        MailerConfig {
            transport,
            host: host.into(),
            port: 587,
            username: "u".into(),
            password: "p".into(),
            from: "Keel <no-reply@example.com>".into(),
            timeout_ms: 750,
        }
    }

    /// Wave 6 P1: the default config boots the mail job (built-in `log`
    /// chain), the def carries the job-policy retry budget, and declared
    /// mailers convert field-for-field. A log-transport mailer with no host
    /// boots while an smtp one refuses — together proving the transport
    /// mapping (an inverted mapping would fail one of the two halves).
    #[test]
    fn mail_job_boots_from_default_config_and_converts_mailers() {
        let cfg = PlatformConfig::default().freeze();
        let (def, handler) = mail_job(&cfg).expect("default config builds the mail job");
        assert_eq!(def.name, keel_mail_smtp::MAIL_SEND_JOB);
        assert_eq!(def.max_attempts, 3, "job policy owns retry/backoff/dead");
        assert_eq!(handler.name(), keel_mail_smtp::MAIL_SEND_JOB);

        let mut cfg = PlatformConfig {
            mail: section("relay,log"),
            ..Default::default()
        };
        cfg.mail
            .mailers
            .insert("relay".into(), mailer(MailerTransport::Log, ""));
        let frozen = cfg.freeze();
        let chain = mail_chain(frozen.mail()).expect("relay,log chain converts");
        assert_eq!(chain.names().join(","), "relay,log");
    }

    /// Config errors refuse boot: unknown chain member (typo), empty chain,
    /// and an smtp mailer without a host.
    #[test]
    fn mail_chain_refuses_bad_sections() {
        let err = mail_chain(&section("primry")).expect_err("typo refuses boot");
        assert!(
            err.public_message.contains("unknown mailer"),
            "{}",
            err.public_message
        );

        let err = mail_chain(&section(" , ")).expect_err("empty chain refuses boot");
        assert!(err.public_message.contains("empty"), "{}", err.public_message);

        let mut bad = section("relay");
        bad.mailers
            .insert("relay".into(), mailer(MailerTransport::Smtp, ""));
        let err = mail_chain(&bad).expect_err("smtp without host refuses boot");
        assert!(err.public_message.contains("no host"), "{}", err.public_message);
    }

    /// The def + handler register exactly once each — the boot-time guard
    /// both registries enforce (duplicates refuse).
    #[test]
    fn registries_accept_the_mail_job_once() {
        let cfg = PlatformConfig::default().freeze();
        let (def, handler) = mail_job(&cfg).unwrap();

        let mut defs = JobRegistry::new();
        defs.register(def).unwrap();
        let mut handlers = WorkerRegistry::new();
        handlers.register(Arc::new(handler)).unwrap();
        assert_eq!(defs.names(), vec![keel_mail_smtp::MAIL_SEND_JOB]);
        assert_eq!(handlers.names(), vec![keel_mail_smtp::MAIL_SEND_JOB]);

        let (dup_def, dup_handler) = mail_job(&cfg).unwrap();
        assert!(defs.register(dup_def).is_err(), "def refuses duplicates");
        assert!(
            handlers.register(Arc::new(dup_handler)).is_err(),
            "handler refuses duplicates"
        );
    }
}
