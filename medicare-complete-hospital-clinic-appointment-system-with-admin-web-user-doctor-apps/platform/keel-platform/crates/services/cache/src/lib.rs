//! # kelson-caching
//! Six named caches, never one blob (11-SPEC §43): Identity · Authorization ·
//! Configuration · ReferenceData · Query · Edge. Authorization keys must include
//! principal + tenant + scope fingerprint + authorization/feature revisions.

use kelson_foundation::{AuthorizationRevision, FeatureRevision, PrincipalId, TenantId};

/// Wave 4 Phase 6: value-level `CacheService` (remember/tags/locks) over the
/// unchanged port — see the module docs.
pub mod conformance;
pub mod plugins;
pub mod service;
pub mod swappable;
pub use conformance::cache_contract;
pub use plugins::{CacheCapabilities, CacheDriver, CacheDriverRegistry};
pub use service::{CacheGuard, CacheService, TaggedCache};
pub use swappable::SwappableCache;

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

/// Sync byte-level port. Adapters (kelson-cache-memory, later redis) implement it.
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

// --------------------------------------------- tenant-scoped port (target §3.1)

/// Tenant-scoped cache port: every call carries `TenantId` at the port
/// boundary (target-architecture §3.1) — no ambient context, no thread-local,
/// no caller-side key discipline. Application data access goes through this
/// port; the physical [`CachePort`] stays the platform layer (edge state,
/// infrastructure locks) reachable only as break-glass, mirroring the
/// `kelson_platform` database role.
#[cfg(feature = "tenant-ports")]
pub trait TenantCachePort: Send + Sync {
    fn get(&self, tenant: TenantId, key: &str) -> Option<Vec<u8>>;
    fn put(&self, tenant: TenantId, key: &str, value: Vec<u8>, ttl_secs: Option<u64>);
    fn delete(&self, tenant: TenantId, key: &str);
    fn invalidate_prefix(&self, tenant: TenantId, prefix: &str);
    /// Set-if-not-exists within ONE tenant's keyspace (same atomicity
    /// contract as [`CachePort::put_if_absent`]).
    fn put_if_absent(
        &self,
        tenant: TenantId,
        key: &str,
        value: Vec<u8>,
        ttl_secs: Option<u64>,
    ) -> bool;
    /// Compare-and-delete within ONE tenant's keyspace — a token minted for
    /// tenant A can never release a slot under tenant B.
    fn delete_if_equals(&self, tenant: TenantId, key: &str, expected: &[u8]) -> bool;
}

/// §3.3 transition adapter (`TenantShardedAdapter`): wraps ANY physical
/// [`CachePort`] and namespaces every key under `t:{tenant}:` — the Redis
/// scheme from §3.3 applied at the wrapper, so memory/redis/fake adapters
/// all gain isolation without being rewritten. Key construction lives here,
/// in one place, so a new adapter inherits scoping for free.
#[cfg(feature = "tenant-ports")]
pub struct TenantCacheShard {
    inner: std::sync::Arc<dyn CachePort>,
}

#[cfg(feature = "tenant-ports")]
impl TenantCacheShard {
    pub fn new(inner: std::sync::Arc<dyn CachePort>) -> Self {
        Self { inner }
    }

    /// Namespaced key: `t:{tenant}:{key}`.
    fn k(tenant: TenantId, key: &str) -> String {
        format!("t:{tenant}:{key}")
    }
}

#[cfg(feature = "tenant-ports")]
impl TenantCachePort for TenantCacheShard {
    fn get(&self, tenant: TenantId, key: &str) -> Option<Vec<u8>> {
        self.inner.get(&Self::k(tenant, key))
    }
    fn put(&self, tenant: TenantId, key: &str, value: Vec<u8>, ttl_secs: Option<u64>) {
        self.inner.put(&Self::k(tenant, key), value, ttl_secs)
    }
    fn delete(&self, tenant: TenantId, key: &str) {
        self.inner.delete(&Self::k(tenant, key))
    }
    fn invalidate_prefix(&self, tenant: TenantId, prefix: &str) {
        self.inner.invalidate_prefix(&Self::k(tenant, prefix))
    }
    fn put_if_absent(
        &self,
        tenant: TenantId,
        key: &str,
        value: Vec<u8>,
        ttl_secs: Option<u64>,
    ) -> bool {
        self.inner
            .put_if_absent(&Self::k(tenant, key), value, ttl_secs)
    }
    fn delete_if_equals(&self, tenant: TenantId, key: &str, expected: &[u8]) -> bool {
        self.inner.delete_if_equals(&Self::k(tenant, key), expected)
    }
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
    port: std::sync::Arc<crate::swappable::SwappableCache>,
}

/// Held lock: name + the token proving ownership.
#[derive(Debug)]
pub struct LockToken {
    pub name: String,
    pub token: String,
}

impl CacheLock {
    pub fn new(port: std::sync::Arc<crate::swappable::SwappableCache>) -> Self {
        Self { port }
    }

    /// Try to acquire. Returns None when already held (never blocks).
    pub fn try_acquire(&self, name: &str, ttl_secs: u64) -> Option<LockToken> {
        let token = kelson_foundation::id::lock_token();
        let ok = self.port.store().put_if_absent(
            &lock_key(name),
            token.as_bytes().to_vec(),
            Some(ttl_secs),
        );
        ok.then(|| LockToken {
            name: name.to_string(),
            token,
        })
    }

    /// Release a held lock. Returns true only if THIS token released it.
    pub fn release(&self, token: &LockToken) -> bool {
        self.port
            .store()
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
        let lock = CacheLock::new(std::sync::Arc::new(crate::swappable::SwappableCache::new(
            std::sync::Arc::new(FakePort::default()),
        )));
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

    /// The fake store's entries: the bytes and the TTL the store was given.
    type FakeEntries = std::collections::BTreeMap<String, (Vec<u8>, Option<u64>)>;

    /// Port with a **fake clock** (the atlas's "TTL/fake clock"): the TTL is
    /// honoured against a test-controlled clock, so the lease cases are
    /// deterministic instead of sleeping. A real driver's expiry is proven
    /// separately against a live Redis.
    #[derive(Default)]
    struct FakeClockPort {
        map: std::sync::Mutex<FakeEntries>,
        now: std::sync::Mutex<u64>,
    }

    impl FakeClockPort {
        /// Move the store's clock forward.
        fn advance(&self, secs: u64) {
            let mut now = self.now.lock().unwrap();
            *now += secs;
        }

        /// The expiry the store recorded for a key (`None` = no expiry).
        fn expiry_of(&self, key: &str) -> Option<Option<u64>> {
            let map = self.map.lock().unwrap();
            map.get(key).map(|(_, expiry)| *expiry)
        }

        fn live(&self, key: &str) -> bool {
            let now = *self.now.lock().unwrap();
            let mut map = self.map.lock().unwrap();
            match map.get(key) {
                Some((_, Some(expiry))) if *expiry <= now => {
                    map.remove(key);
                    false
                }
                Some(_) => true,
                None => false,
            }
        }
    }

    impl CachePort for FakeClockPort {
        fn get(&self, key: &str) -> Option<Vec<u8>> {
            if !self.live(key) {
                return None;
            }
            let map = self.map.lock().unwrap();
            map.get(key).map(|(value, _)| value.clone())
        }
        fn put(&self, key: &str, value: Vec<u8>, ttl: Option<u64>) {
            let now = *self.now.lock().unwrap();
            let expiry = ttl.map(|secs| now + secs);
            let mut map = self.map.lock().unwrap();
            map.insert(key.to_string(), (value, expiry));
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
        fn put_if_absent(&self, key: &str, value: Vec<u8>, ttl: Option<u64>) -> bool {
            let now = *self.now.lock().unwrap();
            let expiry = ttl.map(|secs| now + secs);
            let mut map = self.map.lock().unwrap();
            let live = match map.get(key) {
                Some((_, Some(expiry))) => *expiry > now,
                Some((_, None)) => true,
                None => false,
            };
            if live {
                return false;
            }
            map.insert(key.to_string(), (value, expiry));
            true
        }
        fn delete_if_equals(&self, key: &str, expected: &[u8]) -> bool {
            if !self.live(key) {
                return false;
            }
            let mut map = self.map.lock().unwrap();
            if map.get(key).map(|(v, _)| v.as_slice()) == Some(expected) {
                map.remove(key);
                true
            } else {
                false
            }
        }
    }

    /// **LF-071**: every lock carries a lease, so a holder that CRASHES
    /// cannot hold the slot forever — the store's TTL frees it.
    #[test]
    fn a_lock_lease_expires_so_a_crashed_holder_never_blocks_the_slot() {
        let port = std::sync::Arc::new(FakeClockPort::default());
        let lock = CacheLock::new(std::sync::Arc::new(crate::swappable::SwappableCache::new(
            port.clone(),
        )));
        let crashed = lock.try_acquire("billing.close", 30).unwrap();
        assert!(
            lock.try_acquire("billing.close", 30).is_none(),
            "the live lease excludes a second holder"
        );
        port.advance(31);
        assert!(
            lock.try_acquire("billing.close", 30).is_some(),
            "the expired lease frees the slot even though the holder never released"
        );
        assert!(
            !lock.release(&crashed),
            "a stale holder's late release is refused"
        );
        assert!(
            lock.try_acquire("billing.close", 30).is_none(),
            "the stale release did NOT free the new holder's lock"
        );
    }

    /// **LF-071**: the lease is enforced by the STORE, not by the holder's
    /// clock — a holder with a skewed clock can neither extend nor shorten
    /// it (the store's TTL is the only authority).
    #[test]
    fn the_lease_is_enforced_by_the_store_not_by_the_holders_clock() {
        let port = std::sync::Arc::new(FakeClockPort::default());
        let lock = CacheLock::new(std::sync::Arc::new(crate::swappable::SwappableCache::new(
            port.clone(),
        )));
        lock.try_acquire("report.build", 30).unwrap();
        assert_eq!(
            port.expiry_of(&lock_key("report.build")),
            Some(Some(30)),
            "the lock asks the store for a bounded lease — never 'no expiry'"
        );
        port.advance(29);
        assert!(
            lock.try_acquire("report.build", 30).is_none(),
            "the lease has not elapsed yet"
        );
        port.advance(2);
        assert!(
            lock.try_acquire("report.build", 30).is_some(),
            "the store's clock decides — the holder cannot hold the slot past its lease"
        );
    }

    #[test]
    fn with_lock_runs_and_releases() {
        let lock = CacheLock::new(std::sync::Arc::new(crate::swappable::SwappableCache::new(
            std::sync::Arc::new(FakePort::default()),
        )));
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

    // ------------------------------------------- tenant isolation probes (DoD)

    /// Cross-tenant read/write probe: same logical key, two tenants — each
    /// sees only its own value, and one tenant's delete never erases the
    /// other's entry.
    #[cfg(feature = "tenant-ports")]
    #[test]
    fn tenant_isolation_cache_entries_do_not_cross_tenants() {
        let shard = TenantCacheShard::new(std::sync::Arc::new(FakePort::default()));
        let a = TenantId::new();
        let b = TenantId::new();

        shard.put(a, "patient:1", b"alpha".to_vec(), None);
        assert_eq!(shard.get(a, "patient:1"), Some(b"alpha".to_vec()));
        assert_eq!(
            shard.get(b, "patient:1"),
            None,
            "cross-tenant read must miss"
        );

        // B writes the same logical key — A still reads its own value.
        shard.put(b, "patient:1", b"beta".to_vec(), None);
        assert_eq!(shard.get(a, "patient:1"), Some(b"alpha".to_vec()));
        assert_eq!(shard.get(b, "patient:1"), Some(b"beta".to_vec()));

        // B's delete is confined to B's namespace.
        shard.delete(b, "patient:1");
        assert_eq!(shard.get(b, "patient:1"), None);
        assert_eq!(
            shard.get(a, "patient:1"),
            Some(b"alpha".to_vec()),
            "cross-tenant delete must not erase"
        );
    }

    /// Prefix invalidation (tag-style) stays inside the calling tenant.
    #[cfg(feature = "tenant-ports")]
    #[test]
    fn tenant_isolation_cache_prefix_invalidation_stays_inside_one_tenant() {
        let shard = TenantCacheShard::new(std::sync::Arc::new(FakePort::default()));
        let a = TenantId::new();
        let b = TenantId::new();

        shard.put(a, "query:list-1", vec![1], None);
        shard.put(a, "query:list-2", vec![2], None);
        shard.put(b, "query:list-1", vec![9], None);

        shard.invalidate_prefix(a, "query:");
        assert_eq!(shard.get(a, "query:list-1"), None);
        assert_eq!(shard.get(a, "query:list-2"), None);
        assert_eq!(
            shard.get(b, "query:list-1"),
            Some(vec![9]),
            "other tenant's entries survive"
        );
    }

    /// The atomic primitives (set-if-not-exists, compare-and-delete) are
    /// per-tenant slots: same lock NAME under two tenants is two slots, and
    /// neither side can free the other's token.
    #[cfg(feature = "tenant-ports")]
    #[test]
    fn tenant_isolation_cache_atomic_slots_are_per_tenant() {
        let shard = TenantCacheShard::new(std::sync::Arc::new(FakePort::default()));
        let a = TenantId::new();
        let b = TenantId::new();

        assert!(shard.put_if_absent(a, "lock:report", b"t1".to_vec(), Some(30)));
        assert!(!shard.put_if_absent(a, "lock:report", b"t2".to_vec(), Some(30)));
        // Identical lock name under another tenant is an independent slot.
        assert!(shard.put_if_absent(b, "lock:report", b"t3".to_vec(), Some(30)));

        assert!(
            !shard.delete_if_equals(b, "lock:report", b"t1"),
            "tenant B must not release tenant A's token"
        );
        assert!(shard.delete_if_equals(a, "lock:report", b"t1"));
    }

    // -------------------------------------- pentest (Phase 5, §9 box 2)

    /// Penetration test: an attacker holding a VALID tenant context for A
    /// tries to escape it with hostile keys, hostile prefixes and a stolen
    /// lock token.
    ///
    /// The `tenant_isolation_*` probes above exercise the scoping layer as
    /// it was designed to be used; this one starts from the attacker's side
    /// and asserts only that operations are refused or invisible — never
    /// silently successful against another tenant.
    ///
    /// The empty-prefix case (2) is the load-bearing one: `invalidate_prefix`
    /// with `""` invalidates A's ENTIRE namespace, the strongest single
    /// operation a tenant can issue. It reaching B would mean
    /// `t:{A}:` is a prefix of `t:{B}:`, i.e. the delimiter or the id
    /// encoding is injectable. It is not: `TenantId` is a UUID newtype with
    /// no `FromStr` (`foundation/core/src/id.rs`), UUID text carries no `:`,
    /// and every UUID is exactly 36 chars — so no namespace can contain
    /// another. This pins that structural invariant behaviorally.
    #[cfg(feature = "tenant-ports")]
    #[test]
    fn pentest_cache_hostile_keys_and_prefixes_cannot_escape_the_caller() {
        let shard = TenantCacheShard::new(std::sync::Arc::new(FakePort::default()));
        let a = TenantId::new();
        let b = TenantId::new();

        // A seeds the namespace it will attack; B holds a decoy on every
        // shape A is about to try to reach.
        shard.put(a, "patient:1", b"a-secret".to_vec(), None);
        shard.put(b, "patient:1", b"b-secret".to_vec(), None);
        shard.put(b, "report", b"b-report".to_vec(), None);

        // (1) A writes a key that LITERALLY spells out another tenant's
        //     namespaced path. The wrapper always prepends A's own
        //     namespace, so the forged raw key lands under A and can never
        //     be equal to B's.
        let forged = format!("/{b}/patient:1");
        shard.put(a, &forged, b"planted".to_vec(), None);
        assert_eq!(
            shard.get(b, &forged),
            None,
            "a key forged by A must not resolve inside B"
        );
        assert_eq!(
            shard.get(b, "patient:1"),
            Some(b"b-secret".to_vec()),
            "B's real entry survives the forgery"
        );
        assert_eq!(shard.get(a, "patient:1"), Some(b"a-secret".to_vec()));

        // (2) Hostile prefixes, ending with the full-namespace wipe. Every
        //     one is issued under A and must leave B byte-identical.
        let hostile = vec![
            String::new(),
            ":".to_string(),
            "..".to_string(),
            "../".to_string(),
            "*".to_string(),
            "patient".to_string(),
            format!("{b}:"),
            format!("t:{b}:"),
        ];
        for prefix in &hostile {
            shard.invalidate_prefix(a, prefix);
        }
        assert_eq!(shard.get(a, "patient:1"), None, "A wiped itself");
        assert_eq!(
            shard.get(b, "patient:1"),
            Some(b"b-secret".to_vec()),
            "A's full-namespace wipe must not cross the tenant delimiter"
        );
        assert_eq!(shard.get(b, "report"), Some(b"b-report".to_vec()));

        // (3) Stolen token: A and B hold same-named lock slots, and A tries
        //     to free B's by presenting A's own token. Compare-and-delete
        //     must refuse — the slots are physically distinct keys.
        assert!(shard.put_if_absent(a, "lock:billing", b"a-token".to_vec(), Some(30)));
        assert!(shard.put_if_absent(b, "lock:billing", b"b-token".to_vec(), Some(30)));
        assert!(
            !shard.delete_if_equals(b, "lock:billing", b"a-token"),
            "A's token must not release B's slot"
        );
        assert_eq!(
            shard.get(b, "lock:billing"),
            Some(b"b-token".to_vec()),
            "B still holds its lock"
        );
        assert_eq!(shard.get(a, "lock:billing"), Some(b"a-token".to_vec()));
    }
}
