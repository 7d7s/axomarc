//! # keel-caching
//! Six named caches, never one blob (11-SPEC §43): Identity · Authorization ·
//! Configuration · ReferenceData · Query · Edge. Authorization keys must include
//! principal + tenant + scope fingerprint + authorization/feature revisions.

use keel_foundation::{AuthorizationRevision, FeatureRevision, PrincipalId, TenantId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CacheName {
    Identity,
    Authorization,
    Configuration,
    ReferenceData,
    Query,
    Edge,
}

impl CacheName {
    pub fn as_str(&self) -> &'static str {
        match self {
            CacheName::Identity => "identity",
            CacheName::Authorization => "authorization",
            CacheName::Configuration => "configuration",
            CacheName::ReferenceData => "reference-data",
            CacheName::Query => "query",
            CacheName::Edge => "edge",
        }
    }
}

/// Sync byte-level port. Adapters (keel-cache-memory, later redis) implement it.
/// S3 (L37) adds two atomic primitives used by cache locks: `put_if_absent`
/// (set-if-not-exists) and `delete_if_equals` (compare-and-delete). Redis
/// maps these to SET NX and Lua CAS; the in-memory adapter fakes atomicity
/// with a process mutex (fine for single-node, documented).
pub trait CachePort: Send + Sync {
    fn get(&self, key: &str) -> Option<Vec<u8>>;
    fn put(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>);
    fn delete(&self, key: &str);
    fn invalidate_prefix(&self, prefix: &str);
    /// Set-if-not-exists. Returns true when this call acquired the slot.
    /// MUST be atomic (single round-trip) in real adapters.
    fn put_if_absent(&self, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) -> bool;
    /// Delete only when the current value equals `expected`. Returns true on
    /// successful compare-and-delete. Used so a lock holder never frees a
    /// lock that was re-acquired by someone else after TTL expiry.
    fn delete_if_equals(&self, key: &str, expected: &[u8]) -> bool;
}

// --------------------------------------------------------------- locks (L37)

/// Namespaced lock key so locks never collide with real cache entries.
pub fn lock_key(name: &str) -> String {
    format!("lock:{name}")
}

/// Distributed cache lock (L37, S3). Acquire returns an opaque token; the
/// holder releases by presenting the same token (compare-and-delete) so a
/// slow holder whose lock TTL-expired can never release a lock another
/// process has since acquired. Locks ALWAYS carry a TTL to bound worst-case
/// hold time if the holder crashes.
pub struct CacheLock {
    port: std::sync::Arc<dyn CachePort>,
}

/// Held lock: name + the token proving ownership.
#[derive(Debug)]
pub struct LockToken {
    pub name: String,
    pub token: String,
}

impl CacheLock {
    pub fn new(port: std::sync::Arc<dyn CachePort>) -> Self {
        Self { port }
    }

    /// Try to acquire. Returns None when already held (never blocks).
    pub fn try_acquire(&self, name: &str, ttl_secs: u64) -> Option<LockToken> {
        let token = keel_foundation::id::lock_token();
        let ok =
            self.port
                .put_if_absent(&lock_key(name), token.as_bytes().to_vec(), Some(ttl_secs));
        ok.then(|| LockToken {
            name: name.to_string(),
            token,
        })
    }

    /// Release a held lock. Returns true only if THIS token released it.
    pub fn release(&self, token: &LockToken) -> bool {
        self.port
            .delete_if_equals(&lock_key(&token.name), token.token.as_bytes())
    }

    /// Run `f` while holding the lock; returns None if it could not be
    /// acquired. The lock is always released afterwards.
    pub fn with_lock<R>(&self, name: &str, ttl_secs: u64, f: impl FnOnce() -> R) -> Option<R> {
        let token = self.try_acquire(name, ttl_secs)?;
        let out = f();
        self.release(&token);
        Some(out)
    }
}

/// Canonical authorization cache key (cache-isolation safe).
pub fn authorization_cache_key(
    principal: PrincipalId,
    tenant: TenantId,
    scope_fingerprint: &str,
    auth_revision: AuthorizationRevision,
    feature_revision: FeatureRevision,
) -> String {
    format!("authz:{principal}:{tenant}:{scope_fingerprint}:{auth_revision}:{feature_revision}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal in-test port (single node, mutex-atomic) to exercise CacheLock
    /// without an adapter dependency (Services must not depend on Adapters).
    #[derive(Default)]
    struct FakePort {
        map: std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
    }

    impl CachePort for FakePort {
        fn get(&self, key: &str) -> Option<Vec<u8>> {
            self.map.lock().unwrap().get(key).cloned()
        }
        fn put(&self, key: &str, value: Vec<u8>, _ttl: Option<u64>) {
            self.map.lock().unwrap().insert(key.to_string(), value);
        }
        fn delete(&self, key: &str) {
            self.map.lock().unwrap().remove(key);
        }
        fn invalidate_prefix(&self, prefix: &str) {
            let mut map = self.map.lock().unwrap();
            let dead: Vec<String> = map
                .keys()
                .filter(|k| k.starts_with(prefix))
                .cloned()
                .collect();
            for k in dead {
                map.remove(&k);
            }
        }
        fn put_if_absent(&self, key: &str, value: Vec<u8>, _ttl: Option<u64>) -> bool {
            let mut map = self.map.lock().unwrap();
            if map.contains_key(key) {
                false
            } else {
                map.insert(key.to_string(), value);
                true
            }
        }
        fn delete_if_equals(&self, key: &str, expected: &[u8]) -> bool {
            let mut map = self.map.lock().unwrap();
            if map.get(key).map(|v| v.as_slice()) == Some(expected) {
                map.remove(key);
                true
            } else {
                false
            }
        }
    }

    #[test]
    fn cache_lock_acquire_release_and_exclusivity() {
        let lock = CacheLock::new(std::sync::Arc::new(FakePort::default()));
        let first = lock.try_acquire("billing.close", 30).unwrap();
        assert!(
            lock.try_acquire("billing.close", 30).is_none(),
            "lock is exclusive"
        );
        assert!(
            lock.try_acquire("other.lock", 30).is_some(),
            "other names independent"
        );
        assert!(lock.release(&first));
        assert!(
            lock.try_acquire("billing.close", 30).is_some(),
            "released slot is free"
        );
    }

    #[test]
    fn with_lock_runs_and_releases() {
        let lock = CacheLock::new(std::sync::Arc::new(FakePort::default()));
        let ran = lock.with_lock("report.build", 30, || 42);
        assert_eq!(ran, Some(42));
        assert!(
            lock.try_acquire("report.build", 30).is_some(),
            "lock released after with_lock"
        );
        let held = lock.try_acquire("held.el", 30).unwrap();
        assert!(
            lock.with_lock("held.el", 30, || 1).is_none(),
            "held lock blocks with_lock"
        );
        lock.release(&held);
    }

    #[test]
    fn authz_key_includes_revisions() {
        let k1 = authorization_cache_key(
            PrincipalId::new(),
            TenantId::new(),
            "fp",
            AuthorizationRevision(1),
            FeatureRevision(1),
        );
        assert!(k1.starts_with("authz:"));
        assert!(k1.ends_with(":1:1"));
    }
}
