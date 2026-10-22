//! The swappable cache port.
//!
//! The registry selects a driver; this holder is what makes that selection
//! take effect **at runtime**. A deployment that changes its configuration
//! (moving from a process-local cache to a shared one, say) must not need
//! a rebuild and a restart: the new driver is built and swapped in, and the
//! next read goes to it.
//!
//! The holder is the single point where the active port changes. Every
//! read and every write goes through it, so a swap is atomic from the
//! caller's point of view: a request either sees the old driver or the new
//! one, never a half-swapped pair.
//!
//! One caveat, by design: a cache LOCK taken before a swap is released
//! against the driver that is active afterwards, so it is freed by its
//! LEASE (the TTL) rather than by its holder. The lease is what makes the
//! swap safe, not an accident.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use kelson_foundation::AppError;

use crate::CachePort;

/// The runtime holder for the active cache port.
#[derive(Clone)]
pub struct SwappableCache {
    current: Arc<RwLock<Arc<dyn CachePort>>>,
    /// How many times the active port CHANGED (the operator's evidence
    /// that a configuration change took effect).
    swaps: Arc<AtomicU64>,
}

impl std::fmt::Debug for SwappableCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SwappableCache")
            .field("swaps", &self.swaps.load(Ordering::Relaxed))
            .finish()
    }
}

impl SwappableCache {
    /// A holder over the initial port.
    pub fn new(port: Arc<dyn CachePort>) -> Self {
        Self {
            current: Arc::new(RwLock::new(port)),
            swaps: Arc::new(AtomicU64::new(0)),
        }
    }

    /// The active port.
    pub fn store(&self) -> Arc<dyn CachePort> {
        match self.current.read() {
            Ok(guard) => Arc::clone(&guard),
            // A poisoned lock means a panic while a swap was in flight. The
            // port itself is still valid (it is behind an Arc), so the
            // holder keeps serving rather than failing every read.
            Err(poisoned) => Arc::clone(&poisoned.into_inner()),
        }
    }

    /// Swap in a new port. The swap is atomic: a reader sees the old port
    /// or the new one, never a mixture.
    ///
    /// Swapping the port that is already active is NOT a change and does
    /// not count: the counter answers "how many times did the driver
    /// change", not "how many times was a driver activated".
    pub fn swap(&self, port: Arc<dyn CachePort>) -> Result<(), AppError> {
        let mut current = self.current.write().map_err(|_| {
            AppError::persistence("the swappable cache lock is poisoned")
        })?;
        if Arc::ptr_eq(&*current, &port) {
            return Ok(());
        }
        *current = port;
        self.swaps.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// How many times the active port changed.
    pub fn swaps(&self) -> u64 {
        self.swaps.load(Ordering::Relaxed)
    }
}

/// The holder IS a port: it delegates every call to the active driver, so
/// a consumer that holds the port follows the swap.
impl CachePort for SwappableCache {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.store().get(key)
    }
    fn put(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) {
        self.store().put(key, value, ttl_secs);
    }
    fn delete(&self, key: &str) {
        self.store().delete(key);
    }
    fn invalidate_prefix(&self, prefix: &str) {
        self.store().invalidate_prefix(prefix);
    }
    fn put_if_absent(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) -> bool {
        self.store().put_if_absent(key, value, ttl_secs)
    }
    fn delete_if_equals(&self, key: &str, expected: &[u8]) -> bool {
        self.store().delete_if_equals(key, expected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// A store that records what it was asked to do.
    #[derive(Default)]
    struct TagStore {
        entries: Mutex<BTreeMap<String, Vec<u8>>>,
    }

    impl CachePort for TagStore {
        fn get(&self, key: &str) -> Option<Vec<u8>> {
            self.entries.lock().unwrap().get(key).cloned()
        }
        fn put(&self, key: &str, value: Vec<u8>, _ttl: Option<u64>) {
            self.entries
                .lock()
                .unwrap()
                .insert(key.to_string(), value);
        }
        fn delete(&self, key: &str) {
            self.entries.lock().unwrap().remove(key);
        }
        fn invalidate_prefix(&self, prefix: &str) {
            self.entries
                .lock()
                .unwrap()
                .retain(|k, _| !k.starts_with(prefix));
        }
        fn put_if_absent(&self, key: &str, value: Vec<u8>, _ttl: Option<u64>) -> bool {
            let mut entries = self.entries.lock().unwrap();
            if entries.contains_key(key) {
                false
            } else {
                entries.insert(key.to_string(), value);
                true
            }
        }
        fn delete_if_equals(&self, key: &str, expected: &[u8]) -> bool {
            let mut entries = self.entries.lock().unwrap();
            match entries.get(key) {
                Some(current) if current == expected => {
                    entries.remove(key);
                    true
                }
                _ => false,
            }
        }
    }

    #[test]
    fn a_swap_changes_what_the_holder_reads() {
        let first = Arc::new(TagStore::default());
        let holder = SwappableCache::new(first.clone());
        holder.store().put("k", b"one".to_vec(), None);
        assert_eq!(holder.store().get("k"), Some(b"one".to_vec()));

        // The new driver starts EMPTY: what the holder reads proves which
        // one is active.
        let second = Arc::new(TagStore::default());
        holder.swap(second.clone()).expect("the swap applies");
        assert_eq!(
            holder.store().get("k"),
            None,
            "the holder must read the NEW driver"
        );
        holder.store().put("k", b"two".to_vec(), None);
        assert_eq!(
            second.entries.lock().unwrap().get("k").cloned(),
            Some(b"two".to_vec())
        );
        assert_eq!(
            first.entries.lock().unwrap().get("k").cloned(),
            Some(b"one".to_vec()),
            "the old driver keeps what it had"
        );
    }

    #[test]
    fn the_swap_count_is_the_operators_evidence() {
        let holder = SwappableCache::new(Arc::new(TagStore::default()));
        assert_eq!(holder.swaps(), 0);
        holder
            .swap(Arc::new(TagStore::default()))
            .expect("the swap applies");
        assert_eq!(holder.swaps(), 1);
    }

    #[test]
    fn swapping_the_active_port_again_is_not_a_change() {
        let port = Arc::new(TagStore::default());
        let holder = SwappableCache::new(port.clone());
        holder.swap(port.clone()).expect("the swap applies");
        assert_eq!(
            holder.swaps(),
            0,
            "re-activating the same driver is not a change"
        );
    }

    #[test]
    fn the_holder_is_shared() {
        let holder = SwappableCache::new(Arc::new(TagStore::default()));
        let clone = holder.clone();
        clone
            .swap(Arc::new(TagStore::default()))
            .expect("the swap applies");
        assert_eq!(holder.swaps(), 1, "the clone shares the count");
    }

    #[test]
    fn the_holder_is_a_port_so_a_consumer_follows_the_swap() {
        let first = Arc::new(TagStore::default());
        let holder = SwappableCache::new(first.clone());
        let port: Arc<dyn CachePort> = Arc::new(holder.clone());
        port.put("k", b"one".to_vec(), None);
        assert_eq!(port.get("k"), Some(b"one".to_vec()));

        let second = Arc::new(TagStore::default());
        holder.swap(second.clone()).expect("the swap applies");
        assert_eq!(
            port.get("k"),
            None,
            "a consumer that holds the PORT follows the swap"
        );
        port.put("k", b"two".to_vec(), None);
        assert_eq!(
            second.entries.lock().unwrap().get("k").cloned(),
            Some(b"two".to_vec()),
            "the write landed in the new driver"
        );
    }
}
