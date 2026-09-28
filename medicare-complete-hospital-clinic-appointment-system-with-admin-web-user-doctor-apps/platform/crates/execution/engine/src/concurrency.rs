//! Concurrency guards (S1 extraction): engine module + port.
//!
//! - Optimistic → CAS/version checks live in the data layer (nothing to acquire).
//! - Pessimistic → row locks (`FOR UPDATE`) live in the data layer.
//! - Serialized → centralized sequencing: the engine acquires an exclusive lease
//!   on an expanded key before the unit runs and holds it until completion.
//!
//! The in-memory implementation is the kernel reference; production durability
//! comes from `keel-transaction` locks (postgres advisory locks) wired by the
//! data layer.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};

use keel_foundation::{AppError, OperationId, TenantId};

/// Exclusive, RAII lease over one serialization key. Releasing (drop) wakes a
/// single waiter.
pub struct ConcurrencyLease {
    slot: Option<Arc<Slot>>,
}

struct Slot {
    busy: Mutex<bool>,
    cv: Condvar,
}

impl Drop for ConcurrencyLease {
    fn drop(&mut self) {
        if let Some(slot) = self.slot.take() {
            let mut busy = slot.busy.lock().unwrap();
            *busy = false;
            slot.cv.notify_one();
        }
    }
}

/// Port for centralized sequencing (Serialized strategy).
pub trait ConcurrencyPort: Send + Sync {
    /// Block until the key is exclusively held by this caller.
    fn acquire(&self, key: &str) -> Result<ConcurrencyLease, AppError>;
}

/// Kernel reference implementation: per-key flags + condvars (std only).
/// Briefly blocks the waiting thread under contention; acceptable for
/// dev/test — production Serialized ops should use durable locks.
#[derive(Default)]
pub struct InMemoryConcurrency {
    slots: Mutex<HashMap<String, Arc<Slot>>>,
}

impl InMemoryConcurrency {
    pub fn new() -> Self {
        Self::default()
    }
}

impl ConcurrencyPort for InMemoryConcurrency {
    fn acquire(&self, key: &str) -> Result<ConcurrencyLease, AppError> {
        let slot = self
            .slots
            .lock()
            .unwrap()
            .entry(key.to_string())
            .or_insert_with(|| {
                Arc::new(Slot {
                    busy: Mutex::new(false),
                    cv: Condvar::new(),
                })
            })
            .clone();
        let mut busy = slot.busy.lock().unwrap();
        while *busy {
            busy = slot.cv.wait(busy).unwrap();
        }
        *busy = true;
        drop(busy);
        Ok(ConcurrencyLease { slot: Some(slot) })
    }
}

/// Expand a plan key template: `{tenant}` and `{operation}` placeholders.
pub fn expand_key(template: &str, tenant: Option<TenantId>, operation: &OperationId) -> String {
    template
        .replace(
            "{tenant}",
            &tenant
                .map(|t| t.to_string())
                .unwrap_or_else(|| "platform".into()),
        )
        .replace("{operation}", &operation.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leases_serialize_same_key() {
        let port = InMemoryConcurrency::new();
        let first = port.acquire("close:fin.close").unwrap();
        // Same key is busy while the lease lives: prove via a second thread that
        // it only proceeds after drop.
        let flag = Arc::new(Mutex::new(false));
        let flag_w = flag.clone();
        let port2 = InMemoryConcurrency::new();
        // Independent port instance shares nothing: use the SAME port across threads.
        let port = Arc::new(port);
        drop(port2);
        let handle = {
            let port = port.clone();
            std::thread::spawn(move || {
                let _lease = port.acquire("close:fin.close").unwrap();
                *flag_w.lock().unwrap() = true;
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!*flag.lock().unwrap(), "second acquirer must wait");
        drop(first);
        handle.join().unwrap();
        assert!(
            *flag.lock().unwrap(),
            "second acquirer proceeds after release"
        );
    }

    #[test]
    fn key_template_expansion() {
        let tenant = TenantId::new();
        let op = OperationId::from_static("finance.close");
        assert_eq!(
            expand_key("close:{operation}|{tenant}", Some(tenant), &op),
            format!("close:finance.close|{tenant}")
        );
        assert_eq!(expand_key("{operation}", None, &op), "finance.close");
    }
}
