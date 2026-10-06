//! Async bridge for the sync store ports.
//!
//! The durable store ports (sessions, tokens, principals, jobs, models) are
//! sync by contract; sqlx is async. A pooled connection is registered with
//! the tokio I/O driver of the runtime that created it and can only be
//! driven while that runtime is spinning, so the bridge must run every
//! pooled future on the runtime that owns the pool:
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
//! `crates/data/postgres/src/bridge.rs` (and, for the frozen G07 matrix,
//! `adapters/keel-search-pgvector/src/bridge.rs`). The publishable data
//! fabrics never depend on each other; keep the copies in sync.
//!
//! Futures must be owned (`'static + Send`): clone the pool (cheap `Arc`)
//! and any borrowed SQL/params before building the future.

use std::future::Future;
use std::pin::Pin;
use std::sync::mpsc;
use std::sync::OnceLock;

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
        _ => bridge().submit(fut),
    }
}

type Job = Pin<Box<dyn Future<Output = ()> + Send>>;

struct Bridge {
    tx: mpsc::SyncSender<Job>,
}

static BRIDGE: OnceLock<Bridge> = OnceLock::new();

fn bridge() -> &'static Bridge {
    BRIDGE.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel::<Job>(64);
        std::thread::Builder::new()
            .name("keel-mysql-bridge".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("keel-mysql-bridge runtime");
                for job in rx {
                    runtime.block_on(job);
                }
            })
            .expect("keel-mysql-bridge thread");
        Bridge { tx }
    })
}

impl Bridge {
    fn submit<T: Send + 'static>(&self, fut: impl Future<Output = T> + Send + 'static) -> T {
        let (done_tx, done_rx) = mpsc::sync_channel::<T>(1);
        let job: Job = Box::pin(async move {
            let out = fut.await;
            let _ = done_tx.send(out);
        });
        self.tx.send(job).expect("keel-mysql-bridge alive");
        done_rx.recv().expect("keel-mysql-bridge result")
    }
}
