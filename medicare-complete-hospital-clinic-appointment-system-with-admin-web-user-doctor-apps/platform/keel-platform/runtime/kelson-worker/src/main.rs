//! kelson-worker: durable job runner host (11-SPEC §29/§41, item 23). Boots the
//! same lifecycle host as kelson-demo, then runs a claim loop: claims due jobs,
//! dispatches them to registered handlers, records completion or failure
//! (retry budget and backoff live in the store), and drains the outbox via a
//! concurrent pump. Degraded boot (D-52): without postgres the worker runs on
//! the in-memory store and only sees jobs enqueued in-process.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use clap::Parser;
use kelson_config::{Environment, FrozenConfig, PlatformConfig};
use kelson_event::{ListenerRegistry, OutboxPump, OutboxStore, OutboxStoreService};
use kelson_foundation::{codes, AppError, ErrorCategory, Retryability, Severity};
use kelson_jobs::{
    BatchRuntime, BoxFuture, ChainRuntime, Control, DurableJob, InMemoryJobStore, JobDef,
    JobHandler, JobMiddlewareStacks, JobRegistry, JobStatus, JobStore, JobStoreAdmin,
    JobStoreAdminService, JobStoreService, UniqueLock, WorkerRegistry,
};
use kelson_lifecycle::{ApplicationHost, BootError, Phase, Provider, ServiceRegistry};
use kelson_scheduling::{ScheduleItem, ScheduleLock, ScheduleRegistry, SchedulerRunner};
use time::OffsetDateTime;

#[derive(Debug, Parser)]
#[command(name = "kelson-worker", about = "Kelson durable job worker")]
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
/// no-redis default (S10 B-13). Wave 9 B-31: driver-selected — the pool is
/// the boot-selected `DbPool` variant (postgres or mysql).
fn fallback_job_stores(
    pool: &Option<kelson_mysql::DbPool>,
) -> (Arc<dyn JobStore>, Arc<dyn JobStoreAdmin>) {
    match pool {
        Some(kelson_mysql::DbPool::Postgres(pool)) => {
            let store = Arc::new(kelson_postgres::PgJobStore::new(pool.clone()));
            (store.clone(), store)
        }
        Some(kelson_mysql::DbPool::MySql(pool)) => {
            let store = Arc::new(kelson_mysql::MySqlJobStore::new(pool.clone()));
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

/// Wave 6 P4: the scheduler's guard port over `CacheService::lock` —
/// `without_overlapping` / `on_one_server` keys are backend-held for their
/// TTL (Redis when `REDIS_URL` connects, so `on_one_server` spans nodes;
/// the in-memory port otherwise, D-52 process-local degradation).
struct CacheScheduleLock {
    cache: kelson_caching::CacheService,
}

impl ScheduleLock for CacheScheduleLock {
    fn try_acquire(&self, key: &str, ttl_secs: u64) -> bool {
        // A fresh guard carries no token, so this is a plain backend
        // acquire: free/expired → true, held anywhere → false. The token is
        // dropped on purpose — overlap holds expire by TTL, never by drop.
        self.cache.lock(key, ttl_secs).try_acquire()
    }

    fn release(&self, key: &str) {
        // Token-free release: within a tick we just acquired this key, so
        // `force_release` cannot free someone else's window.
        self.cache.lock(key, 0).force_release();
    }
}

/// Wave 6 P8: the unique-job claim guard over the SAME cache seam as the
/// schedule locks — `queue:unique:*` keys are backend-held for the def's
/// `unique_ttl` (Redis when `REDIS_URL` connects, so skip-if-locked spans
/// nodes; the in-memory port otherwise, D-52 process-local degradation).
struct CacheUniqueLock {
    cache: kelson_caching::CacheService,
}

impl UniqueLock for CacheUniqueLock {
    fn try_acquire(&self, key: &str, ttl: time::Duration) -> Result<bool, AppError> {
        let ttl_secs = ttl.whole_seconds().max(1) as u64;
        Ok(self.cache.lock(key, ttl_secs).try_acquire())
    }

    fn release(&self, key: &str) -> Result<(), AppError> {
        // Token-free release, the schedule-lock posture: this process just
        // acquired the key, so force_release cannot free someone else's.
        self.cache.lock(key, 0).force_release();
        Ok(())
    }
}

/// The guard backend: `REDIS_URL` present + reachable → Redis (the same
/// posture as the job store above); else the in-memory port.
fn schedule_cache_port() -> Arc<dyn kelson_caching::CachePort> {
    let memory: Arc<dyn kelson_caching::CachePort> = kelson_cache_memory::MemoryCache::shared(4096);
    match std::env::var("REDIS_URL") {
        Ok(url) => match kelson_cache_redis::RedisCache::connect(&url) {
            Ok(redis) => {
                tracing::info!("schedule guard: redis (distributed)");
                Arc::new(redis)
            }
            Err(e) => {
                tracing::warn!(
                    error = %e.public_message,
                    "redis unreachable for schedule guard; guards stay process-local"
                );
                memory
            }
        },
        Err(_) => memory,
    }
}

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

/// **F-25**: the worker's panic boundary — a panic inside a job handler
/// becomes a job FAILURE instead of unwinding the claim loop.
///
/// The HTTP edge has the same boundary (F-2, `CatchPanicLayer`): the panic
/// payload is logged server-side and never reaches the caller. Here the
/// payload is logged at ERROR with the job id, and the job's durable
/// `last_error` gets a **redacted** message — a panic payload can carry a
/// secret, and that row is admin-visible.
///
/// With the boundary in place the failed attempt is an ordinary failure:
/// the backoff and the retry budget apply, and the worker keeps claiming.
fn panic_boundary(fut: BoxFuture<'_, Result<(), AppError>>) -> BoxFuture<'_, Result<(), AppError>> {
    Box::pin(PanicBoundary { inner: fut })
}

/// A future that turns a panic into an error. Hand-rolled (no `futures`
/// dependency): the inner future is polled inside `catch_unwind`, so a
/// panic in it is caught at the innermost poll.
struct PanicBoundary<'a> {
    inner: BoxFuture<'a, Result<(), AppError>>,
}

impl std::future::Future for PanicBoundary<'_> {
    type Output = Result<(), AppError>;

    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        // `PanicBoundary` is Unpin (it holds a `Pin<Box<..>>`), so the inner
        // future can be projected without `unsafe`.
        let inner = self.get_mut().inner.as_mut();
        let polled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            std::future::Future::poll(inner, cx)
        }));
        match polled {
            Ok(std::task::Poll::Pending) => std::task::Poll::Pending,
            Ok(std::task::Poll::Ready(result)) => std::task::Poll::Ready(result),
            Err(payload) => {
                // The payload is logged server-side, never stored on the job.
                let msg = payload
                    .downcast_ref::<String>()
                    .map(|s| s.as_str())
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("internal error");
                tracing::error!(panic = %msg, "job handler panicked; failing the attempt");
                std::task::Poll::Ready(Err(AppError::internal("job handler panicked")))
            }
        }
    }
}

/// Enforce the job execution deadline; an overrun counts as a failed attempt
/// so the store applies backoff and the retry budget.
///
/// A deadline that has **already passed** fails the attempt immediately: the
/// job must not run at all. (The guard used to hand an expired job a
/// one-second grace, so a job whose deadline was long gone still executed.)
async fn deadline_guard(
    job: &DurableJob,
    fut: BoxFuture<'_, Result<(), AppError>>,
) -> Result<(), AppError> {
    let remaining = job.deadline - OffsetDateTime::now_utc();
    if !remaining.is_positive() {
        return Err(AppError::new(
            codes::DEADLINE,
            ErrorCategory::Workflow,
            Severity::Medium,
            Retryability::Retryable,
            "job exceeded its execution deadline",
        ));
    }
    let wait = std::time::Duration::try_from(remaining)
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
fn mail_chain(
    section: &kelson_config::MailSection,
) -> Result<kelson_mail_smtp::MailChain, AppError> {
    use kelson_mail_smtp::{MailerSpec, TransportKind};
    let specs = section
        .mailers
        .iter()
        .map(|(name, mailer)| MailerSpec {
            name: name.clone(),
            transport: match mailer.transport {
                kelson_config::MailerTransport::Log => TransportKind::Log,
                kelson_config::MailerTransport::Smtp => TransportKind::Smtp,
            },
            host: mailer.host.clone(),
            port: mailer.port,
            username: mailer.username.clone(),
            password: mailer.password.clone(),
            from: mailer.from.clone(),
            timeout_ms: mailer.timeout_ms,
        })
        .collect();
    kelson_mail_smtp::MailChain::new(&section.default, specs)
}

/// Wave 6 P2: build the named-disk registry from `[storage]` — a mirror of
/// kelson-demo's builder (each runtime owns this mapping because the layer
/// matrix keeps adapter constructors out of the config and service crates).
/// No declared disks yields one synthetic `local` disk at `fallback_root`;
/// a `local` disk without `root`, an `s3` disk without `bucket`, or a
/// `storage.default` naming no registered disk refuses boot.
fn disk_registry(
    section: &kelson_config::StorageConfig,
    fallback_root: &std::path::Path,
) -> Result<kelson_storage::DiskRegistry, AppError> {
    let mut registry = kelson_storage::DiskRegistry::new(section.default.clone());
    if section.disks.is_empty() {
        let port: std::sync::Arc<dyn kelson_storage::StoragePort> =
            std::sync::Arc::new(kelson_storage_local::LocalDisk::new(fallback_root)?);
        registry.register(
            "local",
            kelson_storage::DiskEntry {
                port,
                visibility: kelson_storage::Visibility::Private,
                base_url: None,
            },
        )?;
    }
    for (name, disk) in &section.disks {
        let port: std::sync::Arc<dyn kelson_storage::StoragePort> = match disk.driver {
            kelson_config::DiskDriver::Local => {
                let root = disk.root.as_deref().ok_or_else(|| {
                    AppError::validation(format!("storage disk {name:?} (local) needs a `root`"))
                })?;
                std::sync::Arc::new(kelson_storage_local::LocalDisk::new(root)?)
            }
            kelson_config::DiskDriver::S3 => {
                let _bucket = disk.bucket.as_deref().ok_or_else(|| {
                    AppError::validation(format!("storage disk {name:?} (s3) needs a `bucket`"))
                })?;
                std::sync::Arc::new(kelson_storage_s3::S3Sandbox::new())
            }
        };
        let visibility = match disk.visibility {
            kelson_config::DiskVisibility::Public => kelson_storage::Visibility::Public,
            kelson_config::DiskVisibility::Private => kelson_storage::Visibility::Private,
        };
        registry.register(
            name,
            kelson_storage::DiskEntry {
                port,
                visibility,
                base_url: disk.url.clone(),
            },
        )?;
    }
    registry.validate_default()?;
    Ok(registry)
}

/// Wave 6 P1/P2: the queued-mail job def + handler every worker boots with,
/// built from the frozen config. Kept out of `main` so the registration
/// surface is unit-testable. P2: the handler reads attachments through the
/// default disk of the registry built from `[storage]` — `mail.send`
/// object refs resolve against the same disk the upload edge writes to,
/// and a missing object still fails the attempt explicitly (retry, then
/// dead) instead of being dropped.
fn mail_job(
    cfg: &FrozenConfig,
    storage_root: &std::path::Path,
) -> Result<(JobDef, kelson_mail_smtp::MailSendHandler), AppError> {
    let disks = disk_registry(cfg.storage(), storage_root)?;
    let storage = disks.get(disks.default_name())?.port.clone();
    let handler = kelson_mail_smtp::MailSendHandler::new(mail_chain(cfg.mail())?, Some(storage));
    Ok((JobDef::new(kelson_mail_smtp::MAIL_SEND_JOB), handler))
}

/// Wave 6 P6: the worker hub's mail port — `MailChain` speaks its own
/// failover API and does not implement the hub's `MailPort`, so the
/// runtime adapts: the rich path delegates untouched (the mailable
/// validator runs before the transport) and the simple sync path becomes
/// a text-only mailable. The chain itself is the P1-tested component;
/// this wrapper is the layer-matrix seam (runtime owns both sides).
struct ChainMail(kelson_mail_smtp::MailChain);

impl kelson_notifications::MailPort for ChainMail {
    fn send(
        &self,
        msg: &kelson_notifications::NotifyMessage,
        subject: &str,
    ) -> Result<(), AppError> {
        let mail = kelson_notifications::Mailable {
            to: vec![msg.to.clone()],
            cc: vec![],
            bcc: vec![],
            from: None,
            reply_to: None,
            subject: subject.to_string(),
            text: Some(msg.template.clone()),
            html: None,
            headers: vec![],
            attachments: vec![],
        };
        mail.validate()?;
        self.0.send_mailable(&mail, &[])
    }

    fn send_mailable(
        &self,
        mail: &kelson_notifications::Mailable,
        attachments: &[kelson_notifications::ResolvedMailAttachment],
    ) -> Result<(), AppError> {
        self.0.send_mailable(mail, attachments)
    }
}

/// Wave 6 P6: the queued-notification def + drain handler — the
/// counterpart to [`mail_job`], built from the frozen config so the
/// registration surface stays unit-testable. The envelope is pre-rendered
/// at enqueue (request-time locale, target, views); this hub performs I/O
/// only: the `[mail]` failover chain behind the hub's `MailPort`, the
/// database-channel store (postgres when reachable, in-memory otherwise)
/// for `Database` payloads, and the boot catalog for shape parity with
/// the api hub (drain never re-resolves — text was rendered at enqueue;
/// a malformed `[i18n]` source refuses boot here the same way serve()
/// refuses it). The database store arrives from `main` so tests inject
/// their own observable instance.
fn notification_job(
    cfg: &FrozenConfig,
    i18n_root: &std::path::Path,
    store: Arc<dyn kelson_notifications::NotificationStore>,
) -> Result<(JobDef, NotificationDeliveryHandler), AppError> {
    let messages = Arc::new(kelson_config::load_messages(i18n_root, cfg.i18n())?);
    let mut hub = kelson_notifications::NotifyHub::with_catalog(messages).with_store(store);
    hub.mail = Some(Box::new(ChainMail(mail_chain(cfg.mail())?)));
    Ok((
        JobDef::new(kelson_notifications::NOTIFICATION_SEND_JOB),
        NotificationDeliveryHandler { hub },
    ))
}

/// Wave 6 P6: `notification.send` drain — deserialize the v1 envelope (a
/// version this build does not know refuses the attempt: retry →
/// dead-letter, never a guess) and hand it to the hub. Any Err maps to
/// `JobStore::fail` (retry with backoff, then dead-letter), the
/// `mail.send` posture.
struct NotificationDeliveryHandler {
    hub: kelson_notifications::NotifyHub,
}

impl JobHandler for NotificationDeliveryHandler {
    fn name(&self) -> &str {
        kelson_notifications::NOTIFICATION_SEND_JOB
    }

    fn handle<'a>(&'a self, job: &'a DurableJob) -> BoxFuture<'a, Result<(), AppError>> {
        Box::pin(async move {
            let delivery: kelson_notifications::QueuedNotificationDelivery =
                serde_json::from_value(job.payload.clone())
                    .map_err(|e| AppError::validation(format!("notification.send payload: {e}")))?;
            self.hub.deliver_queued(delivery).await
        })
    }
}

/// Wave 6 P8: everything one claim+dispatch pass reads — handlers and the
/// middleware composition up front, the store in both trait views, and the
/// batch/chain books that close on terminal outcomes (L33/L34, S3).
struct DispatchCtx<'a> {
    handlers: &'a WorkerRegistry,
    defs: &'a JobRegistry,
    stacks: &'a JobMiddlewareStacks,
    store: &'a dyn JobStore,
    admin: &'a dyn JobStoreAdmin,
    batches: &'a BatchRuntime,
    chains: &'a ChainRuntime,
}

impl DispatchCtx<'_> {
    /// One claim+dispatch pass. Returns the claimed job, if any.
    ///
    /// Admission runs BEFORE the handler — a `Hold` returns the claim
    /// (`defer`: attempt untouched, requeued at now + delay, no work
    /// reported), an `Abort` kills the job terminal (batch/chain books
    /// close like a dead failure), and the composed post-hooks observe
    /// every Proceeded execution.
    async fn once(&self, worker_name: &str) -> Result<Option<DurableJob>, AppError> {
        let Some(job) = self.store.claim(worker_name)? else {
            return Ok(None);
        };
        let id = job.id;
        let name = job.name.clone();
        // The claim token, captured before `job` is consumed by migration:
        // every terminal write is fenced on it (see JobStore::complete_claim),
        // so a worker whose claim was reclaimed after its lease expired cannot
        // finish (or burn a retry on) a job another worker now owns. The token
        // is (epoch, owner): `defer` hands an attempt back, so the epoch alone
        // could repeat for a deferred job that is later re-claimed.
        let claim_attempt = job.attempt;
        let claim_owner = worker_name.to_string();
        // Phase 3 §5.1 (KEEL-005): the claimed row may predate the
        // handler's schema — migrate BEFORE admission and dispatch. A
        // refusal (newer-than-known, or a migrator failure) takes the
        // normal fail path (retry budget + backoff), never a blind run of
        // a schema the handler cannot read.
        let job = match self.defs.apply_migrations(job) {
            Ok(job) => job,
            Err(e) => {
                let next =
                    self.store
                        .fail_claim(id, claim_attempt, &claim_owner, &e.public_message)?;
                if next == JobStatus::Dead {
                    if let Some(run) = self.batches.finish_by_job(id, true)? {
                        tracing::warn!(batch_id = %run.id, batch = %run.name, ok = run.ok(), "batch finished with dead member");
                    }
                    if let Err(e) = self.chains.finish_by_job(id, false) {
                        tracing::warn!(error = %e.public_message, "chain halt failed");
                    }
                }
                tracing::warn!(
                    job_id = %id,
                    job = %name,
                    error = %e.public_message,
                    next = ?next,
                    "job schema migration refused; failed without running"
                );
                return Ok(None);
            }
        };
        let started = std::time::Instant::now();
        let stack = self.stacks.for_job(&name, self.defs.get(&name));
        let outcome: Result<(), AppError> = match stack.before(&job).await {
            Ok(Control::Proceed) => {
                let ran = match self.handlers.get(&name) {
                    None => Err(AppError::not_found(format!(
                        "no handler registered for {name}"
                    ))),
                    Some(handler) => {
                        deadline_guard(&job, panic_boundary(handler.handle(&job))).await
                    }
                };
                if let Some(err) = stack.after(&job, &ran).await {
                    tracing::warn!(
                        job_id = %id,
                        job = %name,
                        error = %err.public_message,
                        "job middleware post-hook failed"
                    );
                }
                ran
            }
            Ok(Control::Hold(delay)) => {
                let at = OffsetDateTime::now_utc() + delay;
                self.admin
                    .defer_claim(id, claim_attempt, &claim_owner, at)?;
                tracing::info!(
                    job_id = %id,
                    job = %name,
                    retry_at = %at,
                    "job held by admission; requeued without failing"
                );
                return Ok(None);
            }
            Ok(Control::Abort(reason)) => {
                self.admin
                    .kill_claim(id, claim_attempt, &claim_owner, reason)?;
                // Terminal, like a dead failure: close the batch/chain books.
                if let Some(run) = self.batches.finish_by_job(id, true)? {
                    tracing::warn!(batch_id = %run.id, batch = %run.name, "batch member killed by admission");
                }
                if let Err(e) = self.chains.finish_by_job(id, false) {
                    tracing::warn!(error = %e.public_message, "chain halt failed");
                }
                tracing::warn!(
                    job_id = %id,
                    job = %name,
                    reason,
                    "job killed by admission (dead)"
                );
                return Ok(None);
            }
            // Admission backend failure (e.g. the lock cache is down): the
            // normal fail path — backoff and retry budget — rather than a
            // blind run.
            Err(e) => Err(e),
        };
        match outcome {
            Ok(()) => {
                self.store.complete_claim(id, claim_attempt, &claim_owner)?;
                if let Some(run) = self.batches.finish_by_job(id, false)? {
                    tracing::info!(batch_id = %run.id, batch = %run.name, "batch finished");
                }
                match self.chains.finish_by_job(id, true) {
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
                let next =
                    self.store
                        .fail_claim(id, claim_attempt, &claim_owner, &e.public_message)?;
                if next == JobStatus::Dead {
                    if let Some(run) = self.batches.finish_by_job(id, true)? {
                        tracing::warn!(batch_id = %run.id, batch = %run.name, ok = run.ok(), "batch finished with dead member");
                    }
                    if let Err(e) = self.chains.finish_by_job(id, false) {
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
}

/// The production stop trigger: SIGINT (Ctrl+C) **or** SIGTERM (the production
/// orchestrator). Unix gets both; elsewhere only Ctrl+C exists. Mirrors the
/// demo host's shutdown signal so a k8s rollout drains the worker's in-flight
/// job, outbox and telemetry instead of killing it mid-write (MNC-041).
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("SIGINT received");
            }
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM received");
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutdown signal received");
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    let cfg_path = std::env::var("KEEL_CONFIG").unwrap_or_else(|_| "config/dev.toml".into());
    let cfg = PlatformConfig::load(Some(std::path::Path::new(&cfg_path)))?.freeze();

    // Sentinel (S5): the telemetry runtime replaces S1 `init_logging` as the
    // single logging init point. Layering: compiled defaults <- `[telemetry]`
    // section <- env overlay; the S1 env conventions stay honored (RUST_LOG,
    // KEEL_LOG_FILE, environment-driven console format).
    let mut telemetry_config = kelson_observability::TelemetryConfig::from_section(cfg.telemetry());
    if cfg.telemetry().and_then(|s| s.get("format")).is_none() {
        telemetry_config.format =
            if cfg.app().env.is_prod() || matches!(cfg.app().env, Environment::Staging) {
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
            kelson_observability::ExporterConfig {
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
    let telemetry_runtime = kelson_observability::ObservabilityRuntime::new(
        telemetry_config,
        kelson_observability::ServiceIdentity::new("kelson-worker", env.as_str()),
        &[],
    )
    .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
    telemetry_runtime
        .start()
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    // Database probe (same semantics as kelson-demo; degraded boot D-52).
    // Wave 9 B-31: `[database].driver` selects the probe + runner; `postgres`
    // (the default) keeps the existing path byte-identical.
    let pool: Option<kelson_mysql::DbPool> = match cfg.database() {
        Some(db) => {
            let probe = match cfg.database_driver() {
                kelson_config::DatabaseDriver::Postgres => tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    kelson_postgres::create_pool(&db.url, db.max_connections),
                )
                .await
                .map(|r| r.map(kelson_mysql::DbPool::Postgres)),
                kelson_config::DatabaseDriver::MySql => tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    kelson_mysql::create_pool(&db.url, db.max_connections),
                )
                .await
                .map(|r| r.map(kelson_mysql::DbPool::MySql)),
            };
            match probe {
                Ok(Ok(pool)) => {
                    if db.run_migrations {
                        match &pool {
                            kelson_mysql::DbPool::Postgres(p) => {
                                kelson_postgres::run_migrations(p, &root).await?;
                            }
                            kelson_mysql::DbPool::MySql(p) => {
                                kelson_mysql::run_migrations(p, &root).await?;
                            }
                        }
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
        .unwrap_or_else(|| format!("kelson-worker-{}", std::process::id()));

    // Stores (S10 B-13): REDIS_URL present + reachable -> Redis job store;
    // else postgres when reachable; else in-memory (D-52 degraded boot).
    let (jobs, jobs_admin): (Arc<dyn JobStore>, Arc<dyn JobStoreAdmin>) =
        if let Ok(url) = std::env::var("REDIS_URL") {
            match kelson_cache_redis::RedisJobStore::connect(&url) {
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
    // Wave 6 P9 (B-32): batch membership mirrors to the durable ledger —
    // postgres when a pool exists, memory on the degraded boot (D-52) — so
    // kelson-demo's batch operations and the console batch commands resolve
    // batches this worker dispatches, and a restart never loses a fan-out.
    let batch_ledger: Arc<dyn kelson_jobs::BatchLedger> = match &pool {
        Some(kelson_mysql::DbPool::Postgres(pool)) => {
            Arc::new(kelson_postgres::PgBatchLedger::new(pool.clone()))
        }
        Some(kelson_mysql::DbPool::MySql(pool)) => {
            Arc::new(kelson_mysql::MySqlBatchLedger::new(pool.clone()))
        }
        None => Arc::new(kelson_jobs::MemoryBatchLedger::new()),
    };
    let batches = Arc::new(BatchRuntime::with_ledger(jobs.clone(), batch_ledger));
    let chains = Arc::new(ChainRuntime::new(jobs.clone()));
    let schedules = Arc::new(ScheduleRegistry::new());
    // Wave 6 P1/P2: the queued-mail def + handler, built from `[mail]` and
    // the disk registry (an unknown chain member, a malformed mailer, or a
    // bad `[storage]` section refuses boot right here).
    let (mail_def, mail_handler) = mail_job(&cfg, &root.join("storage").join("local"))?;
    // Wave 6 P6: the queued-notification def + drain handler — the
    // database-channel store mirrors kelson-demo's inbox (postgres when
    // reachable, in-memory otherwise, D-52), so a `Database` envelope
    // drained here lands in the same table the HTTP feed reads.
    let notification_store: Arc<dyn kelson_notifications::NotificationStore> = match &pool {
        Some(kelson_mysql::DbPool::Postgres(pool)) => {
            Arc::new(kelson_postgres::PgNotificationStore::new(pool.clone()))
        }
        Some(kelson_mysql::DbPool::MySql(pool)) => {
            Arc::new(kelson_mysql::MySqlNotificationStore::new(pool.clone()))
        }
        None => Arc::new(kelson_notifications::InMemoryNotificationStore::new()),
    };
    let (notification_def, notification_handler) =
        notification_job(&cfg, &root, notification_store)?;
    // Modules register schedules + job defs here at boot; the framework ships
    // the runner so the first module only adds entries, not machinery. The
    // def registration makes `mail.send` and `notification.send` buildable
    // through the registry (def-owned retry budget) for queued producers.
    let mut job_defs = JobRegistry::new();
    job_defs.register(mail_def)?;
    job_defs.register(notification_def)?;
    let job_defs = Arc::new(job_defs);
    let guard_port = schedule_cache_port();
    let scheduler = Arc::new(SchedulerRunner::new(schedules.clone()).with_lock(Arc::new(
        CacheScheduleLock {
            cache: kelson_caching::CacheService::new(guard_port.clone()),
        },
    )));
    // Wave 6 P8: admission + middleware composition — the built-ins (unique
    // / rate-limit / retry-limit) compose lazily per job name from each
    // registered def; module middleware registers before first dispatch.
    // The unique claim guard shares the schedule-lock cache seam.
    let stacks = JobMiddlewareStacks::new(Arc::new(CacheUniqueLock {
        cache: kelson_caching::CacheService::new(guard_port),
    }));

    let listeners = Arc::new(ListenerRegistry::new());
    let outbox: Option<Arc<dyn OutboxStore>> = pool.as_ref().map(|pool| match pool {
        kelson_mysql::DbPool::Postgres(p) => {
            Arc::new(kelson_postgres::PgOutboxStore::new(p.clone())) as Arc<dyn OutboxStore>
        }
        kelson_mysql::DbPool::MySql(p) => {
            Arc::new(kelson_mysql::MySqlOutboxStore::new(p.clone())) as Arc<dyn OutboxStore>
        }
    });
    let pump: Option<Arc<OutboxPump>> = outbox
        .as_ref()
        .map(|store| Arc::new(OutboxPump::new(store.clone(), listeners.clone())));

    let mut handlers = WorkerRegistry::new();
    handlers.register(Arc::new(DemoJobHandler))?;
    // Wave 6 P2: `mail.send` dispatch — the handler carries the default
    // disk port, so attachment refs resolve through the same registry the
    // upload edge uses.
    handlers.register(Arc::new(mail_handler))?;
    // Wave 6 P6: `notification.send` dispatch — pre-rendered envelope in,
    // I/O out (mail chain / database store).
    handlers.register(Arc::new(notification_handler))?;
    let handlers = Arc::new(handlers);
    // Modules register job middleware alongside their handler/def entries
    // (boot-time `register`, before the first dispatch); none exists yet,
    // so the built-in-ready composition arms here as-is.
    let stacks = Arc::new(stacks);

    let mut host = ApplicationHost::new();
    // Sentinel runtime is adopted, not rebuilt, by the provider (boot() is
    // idempotent; the runtime was started above for boot-time logging).
    host.add_provider(Box::new(
        kelson_observability::ObservabilityProvider::from_runtime(telemetry_runtime.clone()),
    ));
    host.add_provider(Box::new(ConfigProvider {
        config: cfg.clone(),
    }));
    host.add_provider(Box::new(KernelProvider {
        jobs: jobs.clone(),
        jobs_admin: jobs_admin.clone(),
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
        "kelson-worker booted"
    );

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        tokio::spawn(async move {
            // MNC-041: stop on SIGINT **or** SIGTERM. Previously only Ctrl+C
            // was handled, so a k8s rollout (SIGTERM) killed the worker
            // mid-job and skipped the outbox drain and telemetry flush — lost
            // jobs and mail on every rollout. A SIGTERM now stops the same
            // cooperative loop, so the in-flight job finishes and the drain
            // paths below run.
            shutdown_signal().await;
            tracing::info!("shutdown signal received; in-flight job will finish");
            stop.store(true, Ordering::SeqCst);
        });
    }

    let poll = std::time::Duration::from_millis(args.poll_ms.max(50));
    let processed = AtomicUsize::new(0);

    // Scheduler tick (L35): every 30s fire due schedules as durable jobs.
    // Wave 7 P3 (docs/19 L340): the tick instant comes from the HOST clock
    // (injectable — default wall time) instead of a bare `now_utc()`.
    let clock = host.clock().clone();
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
                let now = clock.now();
                let job_defs = job_defs.clone();
                let jobs = jobs.clone();
                match scheduler.tick(now, move |item: &ScheduleItem| {
                    let def = job_defs.get(&item.operation).cloned().unwrap_or_else(|| {
                        let mut def = JobDef::new(item.operation.clone());
                        def.max_attempts = 1;
                        def
                    });
                    let job = kelson_jobs::build_from_def(&def, item.payload.clone(), now);
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
        if kelson_jobs::queue_restart_requested(&root, worker_started) {
            tracing::info!("queue:restart signal observed; stopping after the current job");
            stop.store(true, Ordering::SeqCst);
            break;
        }
        let dispatch = DispatchCtx {
            handlers: &handlers,
            defs: &job_defs,
            stacks: &stacks,
            store: &*jobs,
            admin: &*jobs_admin,
            batches: &batches,
            chains: &chains,
        };
        match dispatch.once(&worker_name).await {
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
        "kelson-worker stopped"
    );
    // Drain telemetry before exit (S5 graceful shutdown: flush buffers + WAL
    // within the deadline; exporters close even on timeout).
    telemetry_runtime.shutdown(std::time::Duration::from_secs(5));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kelson_config::{MailSection, MailerConfig, MailerTransport};

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
            from: "Kelson <no-reply@example.com>".into(),
            timeout_ms: 750,
        }
    }

    /// Per-process temp storage root for the mail job's disk registry —
    /// `LocalDisk` creates it, and `create_dir_all` is idempotent under
    /// parallel tests.
    fn mail_storage_root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("kelson-worker-mail-disk-{}", std::process::id()))
    }

    /// Wave 6 P1: the default config boots the mail job (built-in `log`
    /// chain), the def carries the job-policy retry budget, and declared
    /// mailers convert field-for-field. A log-transport mailer with no host
    /// boots while an smtp one refuses — together proving the transport
    /// mapping (an inverted mapping would fail one of the two halves).
    #[test]
    fn mail_job_boots_from_default_config_and_converts_mailers() {
        let cfg = PlatformConfig::default().freeze();
        let (def, handler) =
            mail_job(&cfg, &mail_storage_root()).expect("default config builds the mail job");
        assert_eq!(def.name, kelson_mail_smtp::MAIL_SEND_JOB);
        assert_eq!(def.max_attempts, 3, "job policy owns retry/backoff/dead");
        assert_eq!(handler.name(), kelson_mail_smtp::MAIL_SEND_JOB);

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
        assert!(
            err.public_message.contains("empty"),
            "{}",
            err.public_message
        );

        let mut bad = section("relay");
        bad.mailers
            .insert("relay".into(), mailer(MailerTransport::Smtp, ""));
        let err = mail_chain(&bad).expect_err("smtp without host refuses boot");
        assert!(
            err.public_message.contains("no host"),
            "{}",
            err.public_message
        );
    }

    /// The def + handler register exactly once each — the boot-time guard
    /// both registries enforce (duplicates refuse).
    #[test]
    fn registries_accept_the_mail_job_once() {
        let cfg = PlatformConfig::default().freeze();
        let (def, handler) = mail_job(&cfg, &mail_storage_root()).unwrap();

        let mut defs = JobRegistry::new();
        defs.register(def).unwrap();
        let mut handlers = WorkerRegistry::new();
        handlers.register(Arc::new(handler)).unwrap();
        assert_eq!(defs.names(), vec![kelson_mail_smtp::MAIL_SEND_JOB]);
        assert_eq!(handlers.names(), vec![kelson_mail_smtp::MAIL_SEND_JOB]);

        let (dup_def, dup_handler) = mail_job(&cfg, &mail_storage_root()).unwrap();
        assert!(defs.register(dup_def).is_err(), "def refuses duplicates");
        assert!(
            handlers.register(Arc::new(dup_handler)).is_err(),
            "handler refuses duplicates"
        );
    }

    /// Wave 6 P2: the worker's copy of the disk-registry builder — an empty
    /// `[storage]` yields the synthetic `local` disk (mail-job parity with
    /// the pre-P2 behavior), declared disks map drivers to adapters with
    /// their visibility/url policy, and malformed sections refuse boot.
    #[test]
    fn disk_registry_builds_synthetic_default_and_refuses_bad_sections() {
        let root = std::env::temp_dir().join(format!("kelson-worker-disks-{}", std::process::id()));
        let registry = disk_registry(&kelson_config::StorageConfig::default(), &root)
            .expect("synthetic local disk");
        assert_eq!(registry.default_name(), "local");
        let (name, _) = registry.resolve(None).expect("default resolves");
        assert_eq!(name, "local");

        // Declared disks: driver -> adapter mapping + url/visibility policy.
        let mut section = kelson_config::StorageConfig {
            default: "cdn".into(),
            ..Default::default()
        };
        section.disks.insert(
            "cdn".into(),
            kelson_config::DiskConfig {
                driver: kelson_config::DiskDriver::S3,
                root: None,
                bucket: Some("kelson".into()),
                url: Some("https://cdn.example.com".into()),
                visibility: kelson_config::DiskVisibility::Public,
            },
        );
        let registry = disk_registry(&section, &root).expect("declared s3 disk");
        let obj = kelson_storage::ObjectRef {
            bucket: "b".into(),
            key: "k".into(),
        };
        assert_eq!(
            registry.url_for("cdn", &obj, 30).unwrap(),
            "https://cdn.example.com/b/k"
        );

        // local without `root` refuses boot.
        let mut bad = kelson_config::StorageConfig::default();
        bad.disks.insert(
            "d".into(),
            kelson_config::DiskConfig {
                driver: kelson_config::DiskDriver::Local,
                root: None,
                bucket: None,
                url: None,
                visibility: kelson_config::DiskVisibility::Private,
            },
        );
        let err = disk_registry(&bad, &root).expect_err("root missing");
        assert!(
            err.public_message.contains("root"),
            "{}",
            err.public_message
        );

        // s3 without `bucket` refuses boot.
        let mut bad = kelson_config::StorageConfig::default();
        bad.disks.insert(
            "d".into(),
            kelson_config::DiskConfig {
                driver: kelson_config::DiskDriver::S3,
                root: None,
                bucket: None,
                url: None,
                visibility: kelson_config::DiskVisibility::Private,
            },
        );
        let err = disk_registry(&bad, &root).expect_err("bucket missing");
        assert!(
            err.public_message.contains("bucket"),
            "{}",
            err.public_message
        );

        // A `storage.default` naming no registered disk refuses boot.
        let mut bad = kelson_config::StorageConfig {
            default: "nope".into(),
            ..Default::default()
        };
        bad.disks.insert(
            "d".into(),
            kelson_config::DiskConfig {
                driver: kelson_config::DiskDriver::S3,
                root: None,
                bucket: Some("b".into()),
                url: None,
                visibility: kelson_config::DiskVisibility::Private,
            },
        );
        let err = disk_registry(&bad, &root).expect_err("unknown default");
        assert!(
            err.public_message.contains("nope"),
            "{}",
            err.public_message
        );
    }

    // ------------------------------------------- Wave 6 P6: notification drain

    /// Per-process i18n root for the notification job — a directory that
    /// (deliberately) does not exist yields the empty default catalog,
    /// exactly like a fresh deploy with no `lang/` yet.
    fn i18n_root() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("kelson-worker-i18n-{}", std::process::id()))
    }

    /// Wave 6 P6: the default config boots the notification job (built-in
    /// `log` chain; the database store is injected by the caller) and the
    /// def + handler register exactly once each — the boot-time guard both
    /// registries enforce (duplicates refuse), mirroring `mail.send`.
    #[test]
    fn notification_job_boots_and_registers_once() {
        let cfg = PlatformConfig::default().freeze();
        let store: Arc<dyn kelson_notifications::NotificationStore> =
            Arc::new(kelson_notifications::InMemoryNotificationStore::new());
        let (def, handler) = notification_job(&cfg, &i18n_root(), store)
            .expect("default config builds the notification job");
        assert_eq!(def.name, kelson_notifications::NOTIFICATION_SEND_JOB);
        assert_eq!(def.max_attempts, 3, "job policy owns retry/backoff/dead");
        assert_eq!(handler.name(), kelson_notifications::NOTIFICATION_SEND_JOB);

        let mut defs = JobRegistry::new();
        defs.register(def).unwrap();
        let mut handlers = WorkerRegistry::new();
        handlers.register(Arc::new(handler)).unwrap();
        assert_eq!(
            defs.names(),
            vec![kelson_notifications::NOTIFICATION_SEND_JOB]
        );
        assert_eq!(
            handlers.names(),
            vec![kelson_notifications::NOTIFICATION_SEND_JOB]
        );

        let store: Arc<dyn kelson_notifications::NotificationStore> =
            Arc::new(kelson_notifications::InMemoryNotificationStore::new());
        let (dup_def, dup_handler) = notification_job(&cfg, &i18n_root(), store).unwrap();
        assert!(defs.register(dup_def).is_err(), "def refuses duplicates");
        assert!(
            handlers.register(Arc::new(dup_handler)).is_err(),
            "handler refuses duplicates"
        );
    }

    /// Wave 6 P6: the drain round trip — a serialized v1 envelope (exactly
    /// what kelson-demo's enqueue port builds) deserializes and delivers
    /// through the worker hub: the `Database` payload lands as a row in
    /// the very store the caller injected (the HTTP feed's twin in
    /// production), addressed to the envelope's principal under the
    /// envelope's type.
    #[tokio::test]
    async fn notification_drain_lands_a_database_envelope_in_the_store() {
        use kelson_notifications::{
            DeliveryPayload, NotificationRow, NotificationStore, QueuedNotificationDelivery,
        };

        let cfg = PlatformConfig::default().freeze();
        let store = Arc::new(kelson_notifications::InMemoryNotificationStore::new());
        let (_def, handler) =
            notification_job(&cfg, &i18n_root(), store.clone()).expect("builds the drain");

        let principal = kelson_foundation::PrincipalId::new();
        let envelope = QueuedNotificationDelivery {
            v: QueuedNotificationDelivery::VERSION,
            type_key: "appointment.booked".into(),
            locale: Some("fr".into()),
            principal_id: Some(principal),
            tenant_id: None,
            at: OffsetDateTime::now_utc(),
            payload: DeliveryPayload::Database(NotificationRow {
                id: *principal.as_uuid(),
                principal_id: principal,
                tenant_id: None,
                kind: "appointment.booked".into(),
                data: serde_json::json!({ "patient": "Ada", "locale": "fr" }),
                read_at: None,
                created_at: OffsetDateTime::now_utc(),
            }),
        };
        // The durable job exactly as the enqueue side builds it …
        let mut defs = JobRegistry::new();
        defs.register(JobDef::new(kelson_notifications::NOTIFICATION_SEND_JOB))
            .unwrap();
        let job = defs
            .build(
                kelson_notifications::NOTIFICATION_SEND_JOB,
                serde_json::to_value(&envelope).unwrap(),
                OffsetDateTime::now_utc(),
            )
            .unwrap();
        // … and the drain performs the I/O.
        handler.handle(&job).await.expect("drains the envelope");

        let page = store.list(principal, 0, 10).await.expect("inbox reads");
        assert_eq!(page.total, 1);
        let row = &page.rows[0];
        assert_eq!(row.kind, "appointment.booked");
        assert_eq!(row.principal_id, principal);
        assert_eq!(row.data["patient"], "Ada");
        assert_eq!(row.data["locale"], "fr");
        assert!(row.read_at.is_none());
    }

    /// Wave 6 P6: an envelope version this build does not know refuses
    /// the attempt (retry → dead-letter) instead of being guessed at.
    #[tokio::test]
    async fn notification_drain_refuses_unknown_envelope_versions() {
        use kelson_notifications::{DeliveryPayload, QueuedNotificationDelivery};

        let cfg = PlatformConfig::default().freeze();
        let store: Arc<dyn kelson_notifications::NotificationStore> =
            Arc::new(kelson_notifications::InMemoryNotificationStore::new());
        let (_def, handler) = notification_job(&cfg, &i18n_root(), store).unwrap();

        let principal = kelson_foundation::PrincipalId::new();
        let envelope = QueuedNotificationDelivery {
            v: 99,
            type_key: "appointment.booked".into(),
            locale: None,
            principal_id: Some(principal),
            tenant_id: None,
            at: OffsetDateTime::now_utc(),
            payload: DeliveryPayload::Mail(kelson_notifications::Mailable {
                to: vec!["ada@example.com".into()],
                cc: vec![],
                bcc: vec![],
                from: None,
                reply_to: None,
                subject: "x".into(),
                text: Some("x".into()),
                html: None,
                headers: vec![],
                attachments: vec![],
            }),
        };
        let mut defs = JobRegistry::new();
        defs.register(JobDef::new(kelson_notifications::NOTIFICATION_SEND_JOB))
            .unwrap();
        let job = defs
            .build(
                kelson_notifications::NOTIFICATION_SEND_JOB,
                serde_json::to_value(&envelope).unwrap(),
                OffsetDateTime::now_utc(),
            )
            .unwrap();

        let err = handler.handle(&job).await.expect_err("v99 refuses");
        assert!(err.public_message.contains("v99"), "{}", err.public_message);
    }

    /// Wave 6 P6: the worker's `MailPort` adapter over the P1 failover
    /// chain — the rich path delegates (the mailable validator refuses an
    /// empty recipient list and an address without `@` BEFORE the
    /// transport), and the simple sync path becomes a text-only mailable.
    /// The chain's own behaviour is the P1 suite's contract; this pins
    /// the seam.
    #[test]
    fn chain_mail_adapts_the_failover_chain() {
        use kelson_notifications::{Channel, MailPort, Mailable, NotifyMessage};

        let port = ChainMail(mail_chain(&section("log")).expect("builtin log chain"));

        let mail = Mailable {
            to: vec!["ada@example.com".into()],
            cc: vec![],
            bcc: vec![],
            from: None,
            reply_to: None,
            subject: "Rendez-vous réservé pour Ada".into(),
            text: Some("Rendez-vous réservé pour Ada".into()),
            html: None,
            headers: vec![],
            attachments: vec![],
        };
        port.send_mailable(&mail, &[])
            .expect("delegates to the chain");

        let no_recipients = Mailable {
            to: vec![],
            ..mail.clone()
        };
        assert!(
            port.send_mailable(&no_recipients, &[]).is_err(),
            "validate refuses before the transport"
        );

        let msg = NotifyMessage {
            channel: Channel::Email,
            to: "ada@example.com".into(),
            template: "subject and body".into(),
            vars: Default::default(),
            tenant_id: None,
            locale: None,
        };
        port.send(&msg, "subject and body")
            .expect("simple path delegates");
        let bad_msg = NotifyMessage {
            to: "nope".into(),
            ..msg.clone()
        };
        assert!(
            port.send(&bad_msg, "s").is_err(),
            "recipient validation first"
        );
    }

    // ------------------------------- Wave 6 P8: dispatch admission + middleware

    use kelson_jobs::{
        build_from_def, unique_lock_key, JobMiddleware, JobMigrator, MemoryUniqueLock, RateLimit,
        RETRY_LIMIT_ABORT,
    };
    use std::sync::Mutex;

    struct TraceHandler {
        name: String,
        trace: Arc<Mutex<Vec<String>>>,
    }

    impl JobHandler for TraceHandler {
        fn name(&self) -> &str {
            &self.name
        }

        fn handle<'a>(&'a self, _job: &'a DurableJob) -> BoxFuture<'a, Result<(), AppError>> {
            let trace = self.trace.clone();
            Box::pin(async move {
                trace.lock().unwrap().push("handler".into());
                Ok(())
            })
        }
    }

    /// A handler that explodes: the job worker's counterpart of a panicking
    /// HTTP handler.
    struct PanickingHandler;

    impl JobHandler for PanickingHandler {
        fn name(&self) -> &str {
            "p8.panicking"
        }

        fn handle<'a>(&'a self, _job: &'a DurableJob) -> BoxFuture<'a, Result<(), AppError>> {
            Box::pin(async move {
                panic!("the handler exploded");
            })
        }
    }

    struct TraceMiddleware {
        trace: Arc<Mutex<Vec<String>>>,
    }

    impl JobMiddleware for TraceMiddleware {
        fn name(&self) -> &str {
            "trace"
        }

        fn before<'a>(&'a self, _job: &'a DurableJob) -> BoxFuture<'a, Result<Control, AppError>> {
            let trace = self.trace.clone();
            Box::pin(async move {
                trace.lock().unwrap().push("before".into());
                Ok(Control::Proceed)
            })
        }

        fn after<'a>(
            &'a self,
            _job: &'a DurableJob,
            _result: &'a Result<(), AppError>,
        ) -> BoxFuture<'a, Result<(), AppError>> {
            let trace = self.trace.clone();
            Box::pin(async move {
                trace.lock().unwrap().push("after".into());
                Ok(())
            })
        }
    }

    /// One P8 claim+dispatch pass against the in-memory store (the store is
    /// its own admin twin, so both trait views point at one instance).
    async fn p8_dispatch(
        handlers: &WorkerRegistry,
        defs: &JobRegistry,
        stacks: &JobMiddlewareStacks,
        store: &InMemoryJobStore,
        batches: &BatchRuntime,
        chains: &ChainRuntime,
    ) -> Result<Option<DurableJob>, AppError> {
        DispatchCtx {
            handlers,
            defs,
            stacks,
            store,
            admin: store,
            batches,
            chains,
        }
        .once("p8-test")
        .await
    }

    /// Wave 6 P8: an exhausted rate bucket HOLDS the claim — the job is
    /// requeued without failing (attempt given back, no last_error), the
    /// handler never ran for it, and nothing dead-lettered.
    #[tokio::test]
    async fn dispatch_holds_a_rate_limited_job_without_failing_it() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut def = JobDef::new("p8.rate_job");
        def.rate_limit = Some(RateLimit {
            capacity: 1,
            refill_per_sec: 0.001,
        });
        let mut defs = JobRegistry::new();
        defs.register(def.clone()).unwrap();
        let mut handlers = WorkerRegistry::new();
        handlers
            .register(Arc::new(TraceHandler {
                name: "p8.rate_job".into(),
                trace: trace.clone(),
            }))
            .unwrap();
        let stacks = JobMiddlewareStacks::new(Arc::new(MemoryUniqueLock::new()));
        let store = Arc::new(InMemoryJobStore::new());
        let batches = BatchRuntime::new(store.clone());
        let chains = ChainRuntime::new(store.clone());

        store
            .enqueue(build_from_def(
                &def,
                serde_json::Value::Null,
                OffsetDateTime::now_utc(),
            ))
            .unwrap();
        store
            .enqueue(build_from_def(
                &def,
                serde_json::Value::Null,
                OffsetDateTime::now_utc(),
            ))
            .unwrap();

        let first = p8_dispatch(&handlers, &defs, &stacks, &store, &batches, &chains)
            .await
            .unwrap();
        assert!(first.is_some(), "the burst claim runs");
        let held = p8_dispatch(&handlers, &defs, &stacks, &store, &batches, &chains)
            .await
            .unwrap();
        assert!(held.is_none(), "the exhausted claim reports no work");
        assert_eq!(
            *trace.lock().unwrap(),
            vec!["handler"],
            "the held job never ran"
        );

        let pending = store.list(Some(JobStatus::Pending), 10).unwrap();
        assert_eq!(pending.len(), 1, "the held job returned to pending");
        assert_eq!(pending[0].attempt, 0, "the claim attempt was given back");
        assert!(pending[0].last_error.is_none(), "held is not failed");
        assert!(
            pending[0].available_at > OffsetDateTime::now_utc(),
            "requeued for the refill"
        );
        assert_eq!(
            store.list(Some(JobStatus::Dead), 10).unwrap().len(),
            0,
            "nothing dead-lettered"
        );
    }

    /// Phase 3 §5.1 (the §10 DoD row): a v1 job in flight while v2
    /// deploys — the dispatch migrates the payload BEFORE the handler
    /// runs. Zero deserialization errors; the handler observes the v2
    /// shape and the returned job carries the current stamp.
    #[tokio::test]
    async fn dispatch_migrates_v1_job_to_v2_before_handler_runs() {
        struct SchemaProbeHandler {
            seen: Arc<Mutex<Vec<serde_json::Value>>>,
        }

        impl JobHandler for SchemaProbeHandler {
            fn name(&self) -> &str {
                "p8.schema_job"
            }

            fn handle<'a>(&'a self, job: &'a DurableJob) -> BoxFuture<'a, Result<(), AppError>> {
                let seen = self.seen.clone();
                let payload = job.payload.clone();
                Box::pin(async move {
                    seen.lock().unwrap().push(payload);
                    Ok(())
                })
            }
        }

        struct V1ToV2;

        impl JobMigrator for V1ToV2 {
            fn job_name(&self) -> &str {
                "p8.schema_job"
            }

            fn current_version(&self) -> u32 {
                2
            }

            fn migrate(
                &self,
                from: u32,
                mut payload: serde_json::Value,
            ) -> Result<serde_json::Value, AppError> {
                assert_eq!(from, 1, "only v1 rows exist in this probe");
                payload["locale"] = serde_json::json!("en");
                Ok(payload)
            }
        }

        let def = JobDef::new("p8.schema_job");
        // Enqueued by the OLD code: v1 stamp, v1 shape.
        let v1 = build_from_def(
            &def,
            serde_json::json!({"to": "a@test.dev"}),
            OffsetDateTime::now_utc(),
        );
        assert_eq!(v1.schema_version, 1);

        // Claimed after the v2 deploy (migrator registered).
        let mut defs = JobRegistry::new();
        defs.register(def).unwrap();
        defs.register_migrator(Arc::new(V1ToV2)).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut handlers = WorkerRegistry::new();
        handlers
            .register(Arc::new(SchemaProbeHandler { seen: seen.clone() }))
            .unwrap();
        let stacks = JobMiddlewareStacks::new(Arc::new(MemoryUniqueLock::new()));
        let store = Arc::new(InMemoryJobStore::new());
        let batches = BatchRuntime::new(store.clone());
        let chains = ChainRuntime::new(store.clone());
        store.enqueue(v1).unwrap();

        let ran = p8_dispatch(&handlers, &defs, &stacks, &store, &batches, &chains)
            .await
            .unwrap()
            .expect("the migrated job ran");
        assert_eq!(ran.schema_version, 2, "returned job carries the stamp");
        assert_eq!(
            *seen.lock().unwrap(),
            vec![serde_json::json!({"to": "a@test.dev", "locale": "en"})],
            "the handler observed the v2 shape"
        );
        assert_eq!(
            store.list(Some(JobStatus::Succeeded), 10).unwrap().len(),
            1,
            "migrated job completed"
        );
    }

    /// Wave 6 P8: skip-if-locked — a peer-held unique key defers the claim
    /// (attempt returned, handler never runs); once the key releases a
    /// fresh claim of the same type runs.
    #[tokio::test]
    async fn dispatch_skips_a_locked_unique_job() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut def = JobDef::new("p8.unique_job");
        def.unique_key = Some("row-7".into());
        let mut defs = JobRegistry::new();
        defs.register(def.clone()).unwrap();
        let mut handlers = WorkerRegistry::new();
        handlers
            .register(Arc::new(TraceHandler {
                name: "p8.unique_job".into(),
                trace: trace.clone(),
            }))
            .unwrap();
        let lock = Arc::new(MemoryUniqueLock::new());
        let stacks = JobMiddlewareStacks::new(lock.clone());
        let store = Arc::new(InMemoryJobStore::new());
        let batches = BatchRuntime::new(store.clone());
        let chains = ChainRuntime::new(store.clone());
        let key = unique_lock_key("p8.unique_job", "row-7");

        // A peer worker holds the key.
        assert!(
            lock.try_acquire(&key, time::Duration::seconds(60)).unwrap(),
            "the peer takes the lock"
        );
        store
            .enqueue(build_from_def(
                &def,
                serde_json::Value::Null,
                OffsetDateTime::now_utc(),
            ))
            .unwrap();
        assert!(
            p8_dispatch(&handlers, &defs, &stacks, &store, &batches, &chains)
                .await
                .unwrap()
                .is_none(),
            "a locked claim skips"
        );
        let pending = store.list(Some(JobStatus::Pending), 10).unwrap();
        assert_eq!(pending.len(), 1, "the skipped job stays pending");
        assert_eq!(pending[0].attempt, 0, "the claim attempt was given back");
        assert!(pending[0].last_error.is_none(), "skipped is not failed");
        assert!(
            pending[0].available_at > OffsetDateTime::now_utc(),
            "requeued on the unique-poll cadence"
        );
        assert!(trace.lock().unwrap().is_empty(), "the locked job never ran");

        // Released: a fresh claim of the same type runs.
        lock.release(&key).unwrap();
        store
            .enqueue(build_from_def(
                &def,
                serde_json::Value::Null,
                OffsetDateTime::now_utc(),
            ))
            .unwrap();
        assert!(
            p8_dispatch(&handlers, &defs, &stacks, &store, &batches, &chains)
                .await
                .unwrap()
                .is_some(),
            "an unlocked claim runs"
        );
        assert_eq!(*trace.lock().unwrap(), vec!["handler"]);
    }

    /// Wave 6 P8: a registered module middleware wraps the execution —
    /// before → handler → after, in stack order around the real handler.
    #[tokio::test]
    async fn dispatch_runs_module_middleware_around_the_handler() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let def = JobDef::new("p8.mw_job");
        let mut defs = JobRegistry::new();
        defs.register(def.clone()).unwrap();
        let mut handlers = WorkerRegistry::new();
        handlers
            .register(Arc::new(TraceHandler {
                name: "p8.mw_job".into(),
                trace: trace.clone(),
            }))
            .unwrap();
        let mut stacks = JobMiddlewareStacks::new(Arc::new(MemoryUniqueLock::new()));
        stacks
            .register(
                "p8.mw_job",
                Arc::new(TraceMiddleware {
                    trace: trace.clone(),
                }),
            )
            .unwrap();
        let store = Arc::new(InMemoryJobStore::new());
        let batches = BatchRuntime::new(store.clone());
        let chains = ChainRuntime::new(store.clone());
        store
            .enqueue(build_from_def(
                &def,
                serde_json::Value::Null,
                OffsetDateTime::now_utc(),
            ))
            .unwrap();

        assert!(
            p8_dispatch(&handlers, &defs, &stacks, &store, &batches, &chains)
                .await
                .unwrap()
                .is_some(),
            "the job ran"
        );
        assert_eq!(
            *trace.lock().unwrap(),
            vec!["before", "handler", "after"],
            "pre-hook, handler, post-hook in order"
        );
    }

    /// Wave 6 P8: an over-budget claim is killed terminal — the handler
    /// never runs, the job lands dead with the retry-limit reason.
    #[tokio::test]
    async fn dispatch_kills_an_over_budget_job() {
        let trace = Arc::new(Mutex::new(Vec::new()));
        let mut def = JobDef::new("p8.retry_job");
        def.retry_limit = Some(1); // two executions allowed
        let mut defs = JobRegistry::new();
        defs.register(def.clone()).unwrap();
        let mut handlers = WorkerRegistry::new();
        handlers
            .register(Arc::new(TraceHandler {
                name: "p8.retry_job".into(),
                trace: trace.clone(),
            }))
            .unwrap();
        let stacks = JobMiddlewareStacks::new(Arc::new(MemoryUniqueLock::new()));
        let store = Arc::new(InMemoryJobStore::new());
        let batches = BatchRuntime::new(store.clone());
        let chains = ChainRuntime::new(store.clone());

        let mut job = build_from_def(&def, serde_json::Value::Null, OffsetDateTime::now_utc());
        job.attempt = 2; // claim() bumps to 3 — over the two-execution cap
        store.enqueue(job).unwrap();

        assert!(
            p8_dispatch(&handlers, &defs, &stacks, &store, &batches, &chains)
                .await
                .unwrap()
                .is_none(),
            "a killed claim reports no work"
        );
        assert!(
            trace.lock().unwrap().is_empty(),
            "the over-budget handler never ran"
        );
        let dead = store.list(Some(JobStatus::Dead), 10).unwrap();
        assert_eq!(dead.len(), 1, "kill is terminal");
        assert_eq!(dead[0].last_error.as_deref(), Some(RETRY_LIMIT_ABORT));
        assert!(dead[0].finished_at.is_some(), "kill stamps finished_at");
    }

    /// LF-051 (background-job authorization): a job whose capability has **no
    /// registered handler** is REFUSED at the worker boundary. The attempt
    /// fails — so the retry budget applies and the job can dead-letter — and it
    /// never "succeeds" silently. Authorization is enforced where the work
    /// runs, not only at the route middleware that enqueued it.
    #[tokio::test]
    async fn dispatch_refuses_a_job_with_no_registered_handler() {
        use kelson_jobs::{JobStore, JobStoreAdmin};

        let store = Arc::new(InMemoryJobStore::new());
        let mut defs = JobRegistry::new();
        defs.register(JobDef::new("known.job"))
            .expect("def registers");
        let mut handlers = WorkerRegistry::new();
        handlers
            .register(Arc::new(DemoJobHandler))
            .expect("handler registers");

        let memory: Arc<dyn kelson_caching::CachePort> =
            kelson_cache_memory::MemoryCache::shared(64);
        let stacks = JobMiddlewareStacks::new(Arc::new(CacheUniqueLock {
            cache: kelson_caching::CacheService::new(memory),
        }));
        let batches = Arc::new(BatchRuntime::with_ledger(
            store.clone(),
            Arc::new(kelson_jobs::MemoryBatchLedger::new()),
        ));
        let chains = Arc::new(ChainRuntime::new(store.clone()));

        // A job for a capability nobody registered a handler for.
        let now = OffsetDateTime::now_utc();
        let mut job = kelson_jobs::build_from_def(
            &JobDef::new("unknown.capability"),
            serde_json::json!({}),
            now,
        );
        job.available_at = now - time::Duration::seconds(60);
        let id = store.enqueue(job).expect("the job enqueues");

        let ctx = DispatchCtx {
            handlers: &handlers,
            defs: &defs,
            stacks: &stacks,
            store: &*store,
            admin: &*store,
            batches: &batches,
            chains: &chains,
        };
        ctx.once("soak-worker")
            .await
            .expect("the dispatch pass runs");

        let row = store.get(id).expect("the store reads").expect("the row");
        assert_ne!(
            row.status,
            JobStatus::Succeeded,
            "an unauthorized job never succeeds silently"
        );
        assert_eq!(
            row.status,
            JobStatus::Pending,
            "the refusal re-queues the attempt (the retry budget governs it now)"
        );
        assert_eq!(
            row.attempt, 1,
            "the attempt was consumed by the refusal — the budget shrinks"
        );
        let error = row.last_error.expect("the failure is recorded");
        assert!(
            error.contains("no handler registered"),
            "the failure names the refusal: {error}"
        );

        // It is visible to the admin as a failed attempt, not as work done.
        let listed = store
            .list(Some(JobStatus::Pending), 10)
            .expect("the admin list runs");
        assert_eq!(listed.len(), 1, "the re-queued job is visible to the admin");
    }

    /// LF-061 (the **timeout** case): the job deadline is enforced. A job that
    /// runs past its deadline is a FAILED attempt (so the retry budget and the
    /// backoff apply ✓) — never a job that hangs the worker ✓.
    #[tokio::test]
    async fn a_job_that_passes_its_deadline_fails_the_attempt() {
        let now = OffsetDateTime::now_utc();
        let mut job =
            kelson_jobs::build_from_def(&JobDef::new("deadline.probe"), serde_json::json!({}), now);
        // A deadline already in the past: the guard must refuse the run.
        job.deadline = now - time::Duration::seconds(1);
        let slow: BoxFuture<'static, Result<(), kelson_foundation::AppError>> = Box::pin(async {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            Ok(())
        });
        let err = deadline_guard(&job, slow)
            .await
            .expect_err("a job past its deadline fails");
        assert_eq!(
            err.public_message, "job exceeded its execution deadline",
            "the failure names the deadline"
        );
    }

    /// **F-25 — a PANICKING job handler does not take the worker down.**
    ///
    /// The HTTP edge has a panic boundary (F-2: a panicking handler answers a
    /// stable 500 envelope), and the job worker now has the same posture: a
    /// panic inside a handler becomes a job FAILURE, so the retry budget and
    /// the backoff apply and the worker keeps claiming.
    #[tokio::test]
    async fn a_panicking_job_handler_does_not_take_the_worker_down() {
        let dispatched = tokio::spawn(async move {
            let def = JobDef::new("p8.panicking");
            let mut defs = JobRegistry::new();
            defs.register(def.clone()).expect("the def registers");
            let mut handlers = WorkerRegistry::new();
            handlers
                .register(Arc::new(PanickingHandler))
                .expect("the handler registers");
            let stacks = JobMiddlewareStacks::new(Arc::new(MemoryUniqueLock::new()));
            let store = Arc::new(InMemoryJobStore::new());
            let batches = BatchRuntime::new(store.clone());
            let chains = ChainRuntime::new(store.clone());
            let now = OffsetDateTime::now_utc();
            let mut job = build_from_def(&def, serde_json::json!({}), now);
            job.available_at = now - time::Duration::seconds(60);
            let id = store.enqueue(job).expect("the job enqueues");
            let outcome = p8_dispatch(&handlers, &defs, &stacks, &store, &batches, &chains).await;
            (id, store, outcome)
        })
        .await;

        let (id, store, outcome) = match dispatched {
            Ok(out) => out,
            Err(join) => panic!("the worker task died: {join}"),
        };
        assert!(
            outcome.is_ok(),
            "a panicking handler must not unwind the worker's dispatch pass"
        );
        let row = store.get(id).expect("the store reads").expect("the row");
        assert_ne!(
            row.status,
            JobStatus::Running,
            "the panicking job must not stay claimed — it must be a FAILED attempt"
        );
        assert!(
            row.last_error.is_some(),
            "the panic is recorded as the job's failure, not lost"
        );
        assert!(
            !row.last_error
                .as_deref()
                .unwrap_or_default()
                .contains("exploded"),
            "the durable error never carries the panic payload: {:?}",
            row.last_error
        );
    }

    /// **F-25** — a panicking handler fails ITS OWN job and nothing else: the
    /// next job in the queue still runs with its own attempt budget.
    #[tokio::test]
    async fn a_panicking_handler_does_not_consume_the_next_jobs_attempt() {
        let store = Arc::new(InMemoryJobStore::new());
        let mut defs = JobRegistry::new();
        defs.register(JobDef::new("p8.panicking"))
            .expect("the def registers");
        defs.register(JobDef::new("p8.after"))
            .expect("the def registers");
        let mut handlers = WorkerRegistry::new();
        handlers
            .register(Arc::new(PanickingHandler))
            .expect("the handler registers");
        handlers
            .register(Arc::new(TraceHandler {
                name: "p8.after".into(),
                trace: Arc::new(Mutex::new(Vec::new())),
            }))
            .expect("the handler registers");
        let stacks = JobMiddlewareStacks::new(Arc::new(MemoryUniqueLock::new()));
        let batches = BatchRuntime::new(store.clone());
        let chains = ChainRuntime::new(store.clone());

        let now = OffsetDateTime::now_utc();
        for name in ["p8.panicking", "p8.after"] {
            let mut job = build_from_def(&JobDef::new(name), serde_json::json!({}), now);
            job.available_at = now - time::Duration::seconds(60);
            store.enqueue(job).expect("the job enqueues");
        }

        // The panicking job is claimed first (equal priority, FIFO).
        let first = p8_dispatch(&handlers, &defs, &stacks, &store, &batches, &chains)
            .await
            .expect("the dispatch pass runs")
            .expect("a job was claimed");
        assert_eq!(first.name, "p8.panicking");
        let panicking = store
            .get(first.id)
            .expect("the store reads")
            .expect("the row");
        assert!(
            panicking.last_error.is_some(),
            "the panicking job is a failed attempt"
        );

        // The next job still runs, with its own attempt.
        let second = p8_dispatch(&handlers, &defs, &stacks, &store, &batches, &chains)
            .await
            .expect("the dispatch pass runs")
            .expect("a job was claimed");
        assert_eq!(second.name, "p8.after", "the next job is not blocked");
        assert_eq!(
            second.attempt, 1,
            "the panicking job consumed only its OWN attempt"
        );
    }

    // ------------------------------------------- long-lived worker: no leak

    /// The handler records what it was given, so the soak can prove that each
    /// dispatch saw ONLY its own job.
    struct RecordingHandler {
        seen: Arc<std::sync::Mutex<Vec<(String, String)>>>,
    }

    impl JobHandler for RecordingHandler {
        fn name(&self) -> &str {
            "soak.job"
        }
        fn handle<'a>(&'a self, job: &'a DurableJob) -> BoxFuture<'a, Result<(), AppError>> {
            let seen = self.seen.clone();
            Box::pin(async move {
                let marker = job
                    .payload
                    .get("marker")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                seen.lock().unwrap().push((job.id.to_string(), marker));
                Ok(())
            })
        }
    }

    /// Row 6: a **long-lived worker** that processes many jobs must not leak
    /// state from one job into the next.
    ///
    /// The soak enqueues many jobs, each in its OWN tenant with its OWN
    /// payload marker and its OWN correlation, and drains them through the
    /// worker's real claim+dispatch pass (`DispatchCtx::once`). It then
    /// asserts:
    ///
    /// 1. every dispatch saw its OWN job (the payload marker matches the job
    ///    id) — a cross-job leak would show a marker from another job;
    /// 2. every job ended **Succeeded** — the fencing is per claim, so a
    ///    leaked claim token from the previous job would make the completion
    ///    a CONFLICT and the job would not be Succeeded;
    /// 3. the queue is empty afterwards (nothing was double-dispatched);
    /// 4. every row kept its own correlation (the origin does not bleed).
    #[tokio::test]
    async fn a_long_lived_worker_leaks_nothing_between_jobs() {
        use kelson_jobs::{JobStore, JobStoreAdmin, TenantJobStore};

        const JOBS: usize = 40;

        let store = Arc::new(InMemoryJobStore::new());
        let mut defs = JobRegistry::new();
        defs.register(JobDef::new("soak.job"))
            .expect("def registers");

        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut handlers = WorkerRegistry::new();
        handlers
            .register(Arc::new(RecordingHandler { seen: seen.clone() }))
            .expect("handler registers");

        let memory: Arc<dyn kelson_caching::CachePort> =
            kelson_cache_memory::MemoryCache::shared(64);
        let stacks = JobMiddlewareStacks::new(Arc::new(CacheUniqueLock {
            cache: kelson_caching::CacheService::new(memory),
        }));

        let batches = Arc::new(BatchRuntime::with_ledger(
            store.clone(),
            Arc::new(kelson_jobs::MemoryBatchLedger::new()),
        ));
        let chains = Arc::new(ChainRuntime::new(store.clone()));

        // Each job: its own tenant, its own payload marker, its own
        // correlation.
        let now = OffsetDateTime::now_utc();
        let mut expected = std::collections::HashMap::new();
        for i in 0..JOBS {
            let tenant = kelson_foundation::TenantId::new();
            let correlation = kelson_foundation::CorrelationId::new();
            let mut job = kelson_jobs::build_from_def(
                &JobDef::new("soak.job"),
                serde_json::json!({ "marker": format!("job-{i}") }),
                now,
            );
            job.available_at = now - time::Duration::seconds(60);
            let job = job.with_origin(Some(correlation), Some(correlation));
            let id = store.enqueue_for(tenant, job).expect("the job enqueues");
            expected.insert(id.to_string(), (format!("job-{i}"), correlation, tenant));
        }

        let ctx = DispatchCtx {
            handlers: &handlers,
            defs: &defs,
            stacks: &stacks,
            store: &*store,
            admin: &*store,
            batches: &batches,
            chains: &chains,
        };

        for _ in 0..JOBS {
            let claimed = ctx
                .once("soak-worker")
                .await
                .expect("the dispatch pass runs")
                .expect("a job was claimable");
            assert!(
                expected.contains_key(&claimed.id.to_string()),
                "the worker claimed a job it never enqueued"
            );
        }

        // 1. Every dispatch saw its OWN job.
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), JOBS, "every job was dispatched exactly once");
        for (id, marker) in seen.iter() {
            let (want_marker, _, _) = expected.get(id).expect("the dispatched job is one of ours");
            assert_eq!(
                marker, want_marker,
                "job {id} saw another job's payload — state leaked between jobs"
            );
        }
        drop(seen);

        // 2. Every job ended Succeeded — the per-claim fencing held (a leaked
        // token would have made the completion a CONFLICT).
        let rows = store.list(None, JOBS * 2).expect("the admin list runs");
        assert_eq!(rows.len(), JOBS, "no job was dispatched twice");
        for row in &rows {
            assert_eq!(
                row.status,
                JobStatus::Succeeded,
                "job {} did not succeed (a leaked claim token would show here)",
                row.id
            );
        }

        // 3. The queue is empty afterwards.
        assert!(
            store
                .claim("soak-worker")
                .expect("the claim runs")
                .is_none(),
            "the queue is drained — nothing was left behind"
        );

        // 4. Every row kept its OWN correlation.
        for row in &rows {
            let (_, correlation, _) = expected
                .get(&row.id.to_string())
                .expect("the row is one of ours");
            assert_eq!(
                row.correlation_id,
                Some(*correlation),
                "job {} carries another job's correlation",
                row.id
            );
        }
    }

    /// The same soak, tenant-scoped: a worker that claims for tenant A must
    /// never see tenant B's job, no matter how many jobs it has already run.
    #[tokio::test]
    async fn a_long_lived_worker_never_crosses_tenants() {
        use kelson_jobs::TenantJobStore;

        const JOBS: usize = 20;

        let store = Arc::new(InMemoryJobStore::new());
        let tenant = kelson_foundation::TenantId::new();
        let other = kelson_foundation::TenantId::new();
        let now = OffsetDateTime::now_utc();

        // The other tenant's job is enqueued FIRST, so a worker that ignored
        // the tenant scope would take it before the tenant's own.
        let mut foreign = kelson_jobs::build_from_def(
            &JobDef::new("soak.job"),
            serde_json::json!({ "marker": "foreign" }),
            now,
        );
        foreign.available_at = now - time::Duration::seconds(60);
        store
            .enqueue_for(other, foreign)
            .expect("the foreign job enqueues");

        for i in 0..JOBS {
            let mut job = kelson_jobs::build_from_def(
                &JobDef::new("soak.job"),
                serde_json::json!({ "marker": format!("job-{i}") }),
                now,
            );
            job.available_at = now - time::Duration::seconds(60);
            store.enqueue_for(tenant, job).expect("the job enqueues");
        }

        for i in 0..JOBS {
            let claimed = store
                .claim_for(tenant, "soak-worker")
                .expect("the claim runs")
                .expect("the tenant's job is claimable");
            assert_eq!(
                claimed.tenant_id,
                Some(tenant),
                "the worker claimed another tenant's job"
            );
            let marker = claimed
                .payload
                .get("marker")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                marker,
                format!("job-{i}"),
                "the tenant's job came back with another job's payload"
            );
        }

        // The foreign job is untouched: the tenant-scoped claim never saw it.
        assert!(
            store
                .claim_for(other, "other-worker")
                .expect("the foreign claim runs")
                .is_some(),
            "the other tenant's job is still there, unclaimed"
        );
    }
}
