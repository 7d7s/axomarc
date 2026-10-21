//! # kelson-cache-memory
//! In-process cache adapter (moka). Sufficient for single-node dev/test; redis
//! adapter later shares the same CachePort. Per-entry TTL is carried in the
//! entry value itself so the adapter works on any moka 0.12 point release.

use std::sync::Arc;
use std::time::{Duration, Instant};

use kelson_caching::CachePort;

#[derive(Clone)]
struct CacheEntry {
    value: Vec<u8>,
    expires_at: Option<Instant>,
}

impl CacheEntry {
    fn is_expired(&self) -> bool {
        self.expires_at
            .map(|t| Instant::now() >= t)
            .unwrap_or(false)
    }
}

pub struct MemoryCache {
    inner: moka::sync::Cache<String, CacheEntry>,
    /// Serializes the atomic compound ops (put_if_absent / delete_if_equals).
    atomic: std::sync::Mutex<()>,
}

impl MemoryCache {
    pub fn new(max_entries: u64) -> Self {
        Self {
            inner: moka::sync::Cache::builder()
                .max_capacity(max_entries)
                .build(),
            atomic: std::sync::Mutex::new(()),
        }
    }

    pub fn shared(max_entries: u64) -> Arc<Self> {
        Arc::new(Self::new(max_entries))
    }
}

impl CachePort for MemoryCache {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        let entry = self.inner.get(key)?;
        if entry.is_expired() {
            self.inner.invalidate(key);
            return None;
        }
        Some(entry.value)
    }

    fn put(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) {
        let entry = CacheEntry {
            value,
            expires_at: ttl_secs.map(|s| Instant::now() + Duration::from_secs(s)),
        };
        self.inner.insert(key.to_string(), entry);
    }

    fn delete(&self, key: &str) {
        self.inner.invalidate(key);
    }

    fn put_if_absent(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) -> bool {
        let _guard = self.atomic.lock().unwrap();
        if self.get(key).is_some() {
            return false;
        }
        let entry = CacheEntry {
            value,
            expires_at: ttl_secs.map(|s| Instant::now() + Duration::from_secs(s)),
        };
        self.inner.insert(key.to_string(), entry);
        true
    }

    fn delete_if_equals(&self, key: &str, expected: &[u8]) -> bool {
        let _guard = self.atomic.lock().unwrap();
        match self.get(key) {
            Some(current) if current == expected => {
                self.inner.invalidate(key);
                true
            }
            _ => false,
        }
    }

    fn invalidate_prefix(&self, prefix: &str) {
        // moka has no prefix scan; iterate and remove (bounded by max_capacity).
        let keys: Vec<String> = self
            .inner
            .iter()
            .map(|(k, _)| (*k).clone())
            .filter(|k| k.starts_with(prefix))
            .collect();
        for k in keys {
            self.inner.invalidate(&k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The **shared cache contract** (LF-070): the in-memory driver must
    /// satisfy exactly what the Redis driver satisfies ✓ — one suite, two
    /// drivers ✓.
    #[test]
    fn the_memory_cache_satisfies_the_cache_contract() {
        let cache = MemoryCache::new(100);
        kelson_caching::cache_contract(&cache);
    }

    #[test]
    fn put_get_invalidate() {
        let cache = MemoryCache::new(100);
        cache.put("authz:u1:t1", vec![1, 2], None);
        cache.put("authz:u2:t1", vec![3], None);
        cache.put("query:products", vec![9], None);
        assert_eq!(cache.get("authz:u1:t1"), Some(vec![1, 2]));
        cache.invalidate_prefix("authz:");
        assert_eq!(cache.get("authz:u1:t1"), None);
        assert_eq!(cache.get("authz:u2:t1"), None);
        assert_eq!(cache.get("query:products"), Some(vec![9]));
    }

    #[test]
    fn put_if_absent_is_atomic_and_delete_if_equals_checks_value() {
        let cache = std::sync::Arc::new(MemoryCache::new(10));
        assert!(cache.put_if_absent("lock:job", b"a".to_vec(), None));
        assert!(
            !cache.put_if_absent("lock:job", b"b".to_vec(), None),
            "second writer loses"
        );
        assert!(
            !cache.delete_if_equals("lock:job", b"b"),
            "wrong token cannot release"
        );
        assert!(cache.delete_if_equals("lock:job", b"a"));
        assert!(
            cache.put_if_absent("lock:job", b"c".to_vec(), None),
            "slot free again"
        );

        // Contention: N threads race for the same lock; exactly one wins.
        let mut handles = Vec::new();
        for _ in 0..8 {
            let c = cache.clone();
            handles.push(std::thread::spawn(move || {
                c.put_if_absent("lock:race", b"w".to_vec(), None)
            }));
        }
        let wins: usize = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|won| *won)
            .count();
        assert_eq!(wins, 1, "exactly one racer acquires");
    }

    #[test]
    fn ttl_expiry_honored() {
        let cache = MemoryCache::new(10);
        cache.put("short", vec![1], Some(0));
        std::thread::sleep(Duration::from_millis(5));
        assert_eq!(cache.get("short"), None);
    }
}
