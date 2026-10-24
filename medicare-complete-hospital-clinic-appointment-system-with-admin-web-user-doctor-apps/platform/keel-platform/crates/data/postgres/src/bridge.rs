//! Async bridge for the sync store ports.
//!
//! The durable store ports (sessions, tokens, principals, jobs, models,
//! search) are sync by contract; sqlx is async. A pooled connection is
//! registered with the tokio I/O driver of the runtime that created it and
//! can only be driven while that runtime is spinning, so the bridge must run
//! every pooled future on the runtime that owns the pool:
//!
//! * inside a multi-threaded runtime (axum host, worker, `#[tokio::main]`):
//!   `block_in_place` + `Handle::block_on` keeps the future on that runtime;
//! * anywhere else (no ambient runtime, or a single-threaded runtime): the
//!   future runs on a single process-wide bridge thread with its own
//!   current-thread runtime, so all fallback use shares one driver.
//!
//! Pools driven through the fallback must therefore be created on that bridge
//! runtime (or never touched by another runtime); pools created under a
//! foreign single-threaded runtime cannot be driven soundly by anyone. Keep
//! hosts multi-threaded (the runtime hosts here are).
//!
//! NOTE: a deliberate copy of this module lives in
//! `adapters/kelson-search-pgvector/src/bridge.rs` (the frozen G07 matrix
//! forbids adapters depending on Data crates). Keep the two in sync.
//!
//! Futures must be owned (`'static + Send`): clone the pool (cheap `Arc`)
//! and any borrowed SQL/params before building the future.

use std::future::Future;
use std::pin::Pin;
use std::sync::mpsc;
use std::sync::Mutex;

/// Run a pooled future to completion from sync code. See module docs for the
/// runtime-flavor rules.
pub fn block_on<T: Send + 'static>(fut: impl Future<Output = T> + Send + 'static) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle)
            if matches!(
                handle.runtime_flavor(),
                tokio::runtime::RuntimeFlavor::MultiThread
            ) =>
        {
            tokio::task::block_in_place(|| handle.block_on(fut))
        }
        _ => bridge_handle().submit(fut),
    }
}

type Job = Pin<Box<dyn Future<Output = ()> + Send>>;

struct Bridge {
    tx: mpsc::SyncSender<Job>,
}

/// MNC-030: the bridge is **rebuildable** rather than a `OnceLock`. The
/// previous `OnceLock<Bridge>` pinned one bridge for the whole process, so a
/// dead bridge thread was dead *forever* — one failure degraded every later
/// call. A `Mutex<Option<..>>` lets a failed bridge be dropped and rebuilt on
/// the next use instead of wedging the process. The lock is only taken on the
/// fallback path (a dedicated current-thread runtime), never on the
/// multi-threaded fast path, so it costs nothing next to a database round-trip.
static BRIDGE: Mutex<Option<Bridge>> = Mutex::new(None);

/// A handle to the live bridge, building one on first use. A cheap `mpsc`
/// sender clone is returned so a caller never holds the lock across work.
fn bridge_handle() -> Bridge {
    let mut guard = BRIDGE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(existing) = guard.as_ref() {
        return Bridge {
            tx: existing.tx.clone(),
        };
    }
    let built = spawn_bridge();
    *guard = Some(Bridge {
        tx: built.tx.clone(),
    });
    built
}

fn spawn_bridge() -> Bridge {
    let (tx, rx) = mpsc::sync_channel::<Job>(64);
    std::thread::Builder::new()
        .name("kelson-pg-bridge".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("kelson-pg-bridge runtime");
            for job in rx {
                // MNC-030: isolate a panicking job. Previously the panic
                // unwound this thread and killed the bridge for the whole
                // process — one bad future became a permanent, fleet-wide
                // outage. `spawn` + `block_on(join)` catches the panic as a
                // `JoinError` instead of unwinding through the driver, so the
                // bridge keeps serving every later job.
                let handle = runtime.spawn(job);
                if runtime.block_on(handle).is_err() {
                    tracing::error!("kelson-pg-bridge job panicked; the bridge stays alive");
                }
            }
        })
        .expect("kelson-pg-bridge thread");
    Bridge { tx }
}

/// Drop the cached bridge so the next call rebuilds it (self-heal after a
/// failed submit). A poisoned lock is not a reason to keep a dead bridge.
fn reset_bridge() {
    if let Ok(mut guard) = BRIDGE.lock() {
        *guard = None;
    }
}

impl Bridge {
    fn submit<T: Send + 'static>(&self, fut: impl Future<Output = T> + Send + 'static) -> T {
        let (done_tx, done_rx) = mpsc::sync_channel::<T>(1);
        let job: Job = Box::pin(async move {
            let out = fut.await;
            let _ = done_tx.send(out);
        });
        if self.tx.send(job).is_err() {
            // The bridge thread is gone (startup failure or an external kill).
            // Clear it so the next call rebuilds a fresh bridge rather than
            // reusing a dead handle forever, then fail this call loudly: there
            // is no `T` to fabricate and the caller must not hang.
            reset_bridge();
            panic!(
                "kelson-pg-bridge: the bridge thread is not accepting work; \
                 this is a fatal runtime-configuration error (hosts must be \
                 multi-threaded — see the module docs)"
            );
        }
        match done_rx.recv() {
            Ok(value) => value,
            Err(_) => {
                // The job ran but never reported: it panicked (isolated above)
                // or the bridge died mid-flight. A hung query is separately
                // bounded by the pool's `statement_timeout` (see `create_pool`),
                // so this is the panic path, not the hang path.
                reset_bridge();
                panic!(
                    "kelson-pg-bridge: a database job did not complete (it likely \
                     panicked — see the bridge log above); there is no result to return"
                );
            }
        }
    }
}
