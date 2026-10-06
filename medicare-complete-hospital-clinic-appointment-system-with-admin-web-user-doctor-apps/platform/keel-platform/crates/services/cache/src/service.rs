//! Wave 4 Phase 6: `CacheService` — the value-level runtime service over the
//! unchanged `CachePort` (docs/19 L213–217).
//!
//! The port speaks bytes and atomic primitives; this service adds the
//! application-facing vocabulary:
//!
//! - **Values** are [`serde_json::Value`]s, serialized at the service layer —
//!   ports and adapters stay untouched (they only ever see bytes).
//! - **`remember`** is cache-aside with stampede protection: a miss elects one
//!   leader under `lock:remember:{key}`; jittered waiters poll for the
//!   leader's write and never run the closure (single-flight collapse). A
//!   dead leader is outwaited — waiters compute themselves after a bounded
//!   window, so `remember` never errors and never blocks indefinitely.
//! - **Tags** are key-prefix namespaces (`tag:{t}:{key}`): membership *is* the
//!   namespace, so `flush` maps straight onto the port's `invalidate_prefix`
//!   and no separate registry (or adapter-specific code) is needed. The same
//!   service therefore runs unchanged on memory and Redis adapters through the
//!   port; Redis SET-backed membership is an adapter-side optimization that
//!   stays out of scope while ports/adapters are frozen. Flush is
//!   prefix-based: avoid `:` inside tag names, and co-tagged copies under tags
//!   outside the flushed set stay until those tags are flushed.
//! - **Locks** re-expose the existing [`CacheLock`] primitive as a service:
//!   `lock(name, ttl)` returns a guard with `try_acquire` (the spec's plain
//!   `get`), the closure form `get(f)` (spec's `lock.get(|| ...)`),
//!   `release`, and `force_release`.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::{lock_key, CacheLock, CachePort, LockToken};

/// Single-flight lock TTL for `remember`: bounds how long a crashed leader
/// can hold the slot open. Waiters stop polling far earlier than this.
const REMEMBER_LOCK_TTL_SECS: u64 = 30;
/// Waiter poll attempts before a straggler computes anyway — availability
/// over collapse (each attempt sleeps 6..=21 ms, so the wait window is
/// bounded to roughly 60..=210 ms).
const REMEMBER_WAIT_ATTEMPTS: u32 = 10;

/// Value-level cache service (docs/19 L214) over the byte-level [`CachePort`].
///
/// Cheap to build; share one instance per port (`Arc<CacheService>`).
pub struct CacheService {
    port: Arc<dyn CachePort>,
    locks: CacheLock,
}

impl CacheService {
    pub fn new(port: Arc<dyn CachePort>) -> Self {
        Self {
            locks: CacheLock::new(port.clone()),
            port,
        }
    }

    // ------------------------------------------------------------ values ---

    /// Read a value. Corrupt or foreign bytes read as a miss (fail-open
    /// reads; `has` still reports the key as present).
    pub fn get(&self, key: &str) -> Option<Value> {
        let bytes = self.port.get(key)?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Write a value with an optional TTL (`None` = no expiry).
    pub fn put(&self, key: &str, value: &Value, ttl_secs: Option<u64>) {
        self.port.put(key, to_bytes(value), ttl_secs);
    }

    /// Key presence at the byte level — true even when the payload would not
    /// deserialize.
    pub fn has(&self, key: &str) -> bool {
        self.port.get(key).is_some()
    }

    pub fn delete(&self, key: &str) {
        self.port.delete(key);
    }

    /// Read + delete (Laravel `pull` parity). Removes the key even when its
    /// bytes were corrupt.
    pub fn pull(&self, key: &str) -> Option<Value> {
        let value = self.get(key);
        self.port.delete(key);
        value
    }

    /// Read the given keys in input order; misses stay `None`.
    pub fn many(&self, keys: &[&str]) -> Vec<Option<Value>> {
        keys.iter().map(|k| self.get(k)).collect()
    }

    /// Write `(key, value)` pairs sharing one TTL.
    pub fn put_many(&self, entries: &[(&str, Value)], ttl_secs: Option<u64>) {
        for (key, value) in entries {
            self.port.put(key, to_bytes(value), ttl_secs);
        }
    }

    /// Atomic set-if-absent (`put_if_absent`): true only when this call
    /// created the entry — the existing atomic primitive, exposed.
    pub fn add(&self, key: &str, value: &Value, ttl_secs: Option<u64>) -> bool {
        self.port.put_if_absent(key, to_bytes(value), ttl_secs)
    }

    /// Write without an expiry.
    pub fn forever(&self, key: &str, value: &Value) {
        self.port.put(key, to_bytes(value), None);
    }

    /// Read-modify-write counter: missing or non-integer values count from 0,
    /// saturating on overflow. The port exposes no atomic counter op and no
    /// remaining-TTL read, so an increment rewrites the entry without a TTL
    /// (documented single-node semantic).
    pub fn increment(&self, key: &str, by: i64) -> i64 {
        let current = self.get(key).and_then(|v| v.as_i64()).unwrap_or(0);
        let next = current.saturating_add(by);
        self.put(key, &Value::from(next), None);
        next
    }

    pub fn decrement(&self, key: &str, by: i64) -> i64 {
        self.increment(key, by.saturating_neg())
    }

    /// Flush the whole cache (memory: clear; the Redis adapter maps `""` to
    /// `FLUSHDB` when it lands — adapters untouched). Clearing includes lock
    /// keys, which are cache entries too (Laravel flush parity).
    pub fn flush(&self) {
        self.port.invalidate_prefix("");
    }

    // ---------------------------------------------------------- remember ---

    /// Cache-aside read with stampede protection (docs/19 L217): a hit
    /// returns immediately; a miss elects one leader under
    /// `lock:remember:{key}` which double-checks and computes. Waiters
    /// jitter-poll for the leader's write and return it without computing
    /// (single-flight collapse). If the leader never appears (crash, expired
    /// TTL), waiters compute themselves after the bounded window.
    pub fn remember(
        &self,
        key: &str,
        ttl_secs: Option<u64>,
        f: impl FnOnce() -> Value,
    ) -> Value {
        if let Some(hit) = self.get(key) {
            return hit;
        }
        let name = remember_lock_name(key);
        // The closure lives here; each path consumes it at most once.
        let mut pending = Some(f);
        if let Some(token) = self.locks.try_acquire(&name, REMEMBER_LOCK_TTL_SECS) {
            let out = self.compute_once(key, ttl_secs, &mut pending);
            self.locks.release(&token);
            return out;
        }
        for _ in 0..REMEMBER_WAIT_ATTEMPTS {
            jittered_sleep(key);
            if let Some(hit) = self.get(key) {
                return hit; // leader finished — collapse, `f` never runs
            }
            if let Some(token) = self.locks.try_acquire(&name, REMEMBER_LOCK_TTL_SECS) {
                let out = self.compute_once(key, ttl_secs, &mut pending);
                self.locks.release(&token);
                return out;
            }
        }
        // The slot never freed and no value appeared: compute anyway
        // (availability over collapse; never error, never block longer).
        let value = pending.take().expect("f is available on the fallback path")();
        self.put(key, &value, ttl_secs);
        value
    }

    /// `remember` without an expiry.
    pub fn remember_forever(&self, key: &str, f: impl FnOnce() -> Value) -> Value {
        self.remember(key, None, f)
    }

    /// Leader path: double-check under the lock (a previous leader may have
    /// finished between our miss and this acquire), then compute once.
    fn compute_once(
        &self,
        key: &str,
        ttl_secs: Option<u64>,
        pending: &mut Option<impl FnOnce() -> Value>,
    ) -> Value {
        if let Some(hit) = self.get(key) {
            return hit;
        }
        let value = pending.take().expect("one compute per remember call")();
        self.put(key, &value, ttl_secs);
        value
    }

    // -------------------------------------------------------------- tags ---

    /// Namespaced view (docs/19 L215): every value is mirrored into each
    /// tag's `tag:{t}:{key}` namespace; `flush` invalidates those prefixes.
    pub fn tags<'a>(&'a self, tags: &[&str]) -> TaggedCache<'a> {
        TaggedCache {
            service: self,
            tags: tags.iter().map(|t| t.to_string()).collect(),
        }
    }

    // ------------------------------------------------------------- locks ---

    /// Lock handle (docs/19 L216) over the existing [`CacheLock`] primitive:
    /// exclusive, TTL-bounded, token-checked. One guard is one logical
    /// acquisition attempt; guards are cheap and not shared across threads.
    pub fn lock<'a>(&'a self, name: &str, ttl_secs: u64) -> CacheGuard<'a> {
        CacheGuard {
            locks: &self.locks,
            name: name.to_string(),
            ttl_secs,
            token: None,
        }
    }
}

/// `remember:{key}` — passed to [`CacheLock`], which namespaces it as
/// `lock:remember:{key}` so single-flight slots never collide with values.
fn remember_lock_name(key: &str) -> String {
    format!("remember:{key}")
}

/// Bounded jittered backoff for waiters: 6..=21 ms from the wall clock plus
/// a stable per-key offset, so racing waiters desynchronize without a `rand`
/// dependency.
fn jittered_sleep(key: &str) {
    let offset = key
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(u32::from(b)));
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let ms = 6 + u64::from(nanos % 9) + u64::from(offset % 7);
    std::thread::sleep(Duration::from_millis(ms));
}

fn to_bytes(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("serde_json::Value serializes")
}

/// Tag-namespaced cache view (docs/19 L215): `get`/`put`/`flush`.
pub struct TaggedCache<'a> {
    service: &'a CacheService,
    tags: Vec<String>,
}

fn tag_key(tag: &str, key: &str) -> String {
    format!("tag:{tag}:{key}")
}

impl TaggedCache<'_> {
    /// First namespace hit across the tag set (`put` mirrors the value into
    /// every tag namespace, so any tag finds it).
    pub fn get(&self, key: &str) -> Option<Value> {
        self.tags
            .iter()
            .find_map(|t| self.service.get(&tag_key(t, key)))
    }

    /// Write into every tag namespace. Duplicated copies are the price of a
    /// port with no cross-tag indirection (see the module docs).
    pub fn put(&self, key: &str, value: &Value, ttl_secs: Option<u64>) {
        for tag in &self.tags {
            self.service.put(&tag_key(tag, key), value, ttl_secs);
        }
    }

    /// Drop every entry under each tag's prefix — the namespace is the
    /// registry, so prefix invalidation is a complete flush for these tags.
    pub fn flush(&self) {
        for tag in &self.tags {
            self.service
                .port
                .invalidate_prefix(&format!("tag:{tag}:"));
        }
    }
}

/// Lock guard (docs/19 L216): `try_acquire` / `get(f)` / `release` /
/// `force_release`.
pub struct CacheGuard<'a> {
    locks: &'a CacheLock,
    name: String,
    ttl_secs: u64,
    token: Option<LockToken>,
}

impl CacheGuard<'_> {
    /// Acquire (non-blocking; the spec's plain `get`). True when this guard
    /// now holds the lock — already-held-by-this-guard also returns true.
    pub fn try_acquire(&mut self) -> bool {
        if self.token.is_some() {
            return true;
        }
        match self.locks.try_acquire(&self.name, self.ttl_secs) {
            Some(token) => {
                self.token = Some(token);
                true
            }
            None => false,
        }
    }

    /// Closure form (`lock.get(|| ...)`): acquire, run `f`, release.
    /// Returns `None` without running `f` when the lock is held elsewhere.
    /// A panic inside `f` leaks the hold until the lock TTL expires (same
    /// contract as [`CacheLock::with_lock`]).
    pub fn get<R>(&mut self, f: impl FnOnce() -> R) -> Option<R> {
        if !self.try_acquire() {
            return None;
        }
        let out = f();
        self.release();
        Some(out)
    }

    /// Release a held lock; true only when this guard's token freed it.
    pub fn release(&mut self) -> bool {
        match self.token.take() {
            Some(token) => self.locks.release(&token),
            None => false,
        }
    }

    /// Unconditional release (Laravel `forceRelease` parity): deletes the
    /// lock key even when another holder owns it — for use after expiry or a
    /// crashed holder.
    pub fn force_release(&mut self) {
        self.token = None;
        self.locks.port.delete(&lock_key(&self.name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;

    /// Test port: byte map + TTL recording (no expiry — tests drive misses by
    /// deleting). Records what the service asked the adapter to do.
    #[derive(Default)]
    struct RecordingPort {
        map: std::sync::Mutex<std::collections::BTreeMap<String, Vec<u8>>>,
        ttls: std::sync::Mutex<std::collections::BTreeMap<String, Option<u64>>>,
    }

    impl RecordingPort {
        fn keys(&self) -> Vec<String> {
            self.map.lock().unwrap().keys().cloned().collect()
        }

        fn ttl(&self, key: &str) -> Option<Option<u64>> {
            self.ttls.lock().unwrap().get(key).copied()
        }
    }

    impl CachePort for RecordingPort {
        fn get(&self, key: &str) -> Option<Vec<u8>> {
            self.map.lock().unwrap().get(key).cloned()
        }
        fn put(&self, key: &str, value: Vec<u8>, ttl: Option<u64>) {
            self.map.lock().unwrap().insert(key.to_string(), value);
            self.ttls.lock().unwrap().insert(key.to_string(), ttl);
        }
        fn delete(&self, key: &str) {
            self.map.lock().unwrap().remove(key);
            self.ttls.lock().unwrap().remove(key);
        }
        fn invalidate_prefix(&self, prefix: &str) {
            let dead: Vec<String> = {
                let map = self.map.lock().unwrap();
                map.keys().filter(|k| k.starts_with(prefix)).cloned().collect()
            };
            for k in dead {
                self.map.lock().unwrap().remove(&k);
                self.ttls.lock().unwrap().remove(&k);
            }
        }
        fn put_if_absent(&self, key: &str, value: Vec<u8>, ttl: Option<u64>) -> bool {
            let mut map = self.map.lock().unwrap();
            if map.contains_key(key) {
                return false;
            }
            map.insert(key.to_string(), value);
            self.ttls.lock().unwrap().insert(key.to_string(), ttl);
            true
        }
        fn delete_if_equals(&self, key: &str, expected: &[u8]) -> bool {
            let mut map = self.map.lock().unwrap();
            if map.get(key).map(|v| v.as_slice()) == Some(expected) {
                map.remove(key);
                self.ttls.lock().unwrap().remove(key);
                true
            } else {
                false
            }
        }
    }

    fn service() -> (CacheService, Arc<RecordingPort>) {
        let port = Arc::new(RecordingPort::default());
        (CacheService::new(port.clone()), port)
    }

    #[test]
    fn values_roundtrip_and_pull_deletes() {
        let (c, p) = service();
        c.put("greet", &json!({"hello": "world"}), Some(30));
        assert!(c.has("greet"));
        assert_eq!(c.get("greet"), Some(json!({"hello": "world"})));
        assert_eq!(c.pull("greet"), Some(json!({"hello": "world"})));
        assert!(!c.has("greet"));
        assert_eq!(c.get("greet"), None);

        // Foreign/corrupt bytes: `has` sees them, `get` reads a miss, and
        // `pull` drops them anyway.
        p.map.lock().unwrap().insert("junk".into(), b"not-json".to_vec());
        assert!(c.has("junk"));
        assert_eq!(c.get("junk"), None);
        assert_eq!(c.pull("junk"), None);
        assert!(!c.has("junk"));
    }

    #[test]
    fn add_is_atomic_and_forever_skips_the_ttl() {
        let (c, p) = service();
        assert!(c.add("once", &json!(1), Some(30)));
        assert_eq!(p.ttl("once"), Some(Some(30)));
        assert!(
            !c.add("once", &json!(2), Some(30)),
            "second writer loses the atomic race"
        );
        assert_eq!(c.get("once"), Some(json!(1)), "losing write discarded");
        c.forever("keep", &json!("v"));
        assert_eq!(p.ttl("keep"), Some(None), "forever = no expiry");
    }

    #[test]
    fn many_and_put_many_cover_hits_and_misses_in_order() {
        let (c, _p) = service();
        c.put_many(&[("a", json!(1)), ("b", json!(2))], Some(10));
        let got = c.many(&["a", "missing", "b"]);
        assert_eq!(got, vec![Some(json!(1)), None, Some(json!(2))]);
    }

    #[test]
    fn increment_and_decrement_track_integer_counters() {
        let (c, _p) = service();
        assert_eq!(c.increment("hits", 1), 1, "missing starts at `by`");
        assert_eq!(c.increment("hits", 4), 5);
        assert_eq!(c.decrement("hits", 2), 3);
        assert!(c.decrement("hits", 10) < 0, "counters may go negative");
        assert_eq!(c.get("hits"), Some(json!(-7)));

        c.put("word", &json!("nope"), None);
        assert_eq!(c.increment("word", 7), 7, "non-integer treated as 0");
    }

    #[test]
    fn flush_clears_every_entry_at_once() {
        let (c, p) = service();
        c.put("a", &json!(1), None);
        c.put("lock:job", &json!(2), None);
        c.flush();
        assert_eq!(c.get("a"), None);
        assert!(!c.has("lock:job"), "lock keys are cache entries too");
        assert!(p.keys().is_empty());
    }

    #[test]
    fn remember_caches_the_miss_and_recomputes_after_removal() {
        let (c, p) = service();
        let calls = Cell::new(0u32);
        let first = c.remember("answer", Some(60), || {
            calls.set(calls.get() + 1);
            json!(42)
        });
        let second = c.remember("answer", Some(60), || {
            calls.set(calls.get() + 1);
            json!(99)
        });
        assert_eq!(first, json!(42));
        assert_eq!(second, json!(42), "second call is a hit");
        assert_eq!(calls.get(), 1, "closure ran exactly once");

        c.delete("answer");
        let third = c.remember("answer", Some(60), || {
            calls.set(calls.get() + 1);
            json!(7)
        });
        assert_eq!(third, json!(7), "miss recomputes");
        assert_eq!(calls.get(), 2);

        c.remember_forever("forever-key", || json!("v"));
        assert_eq!(p.ttl("forever-key"), Some(None), "forever stores no TTL");

        // The single-flight slot is released after every remember.
        let locks = CacheLock::new(p.clone());
        assert!(
            locks
                .try_acquire(&remember_lock_name("answer"), REMEMBER_LOCK_TTL_SECS)
                .is_some(),
            "remember releases its single-flight lock"
        );
    }

    #[test]
    fn remember_waiter_collapses_onto_the_leader_result() {
        let (c, p) = service();
        // A "leader" is in flight: the single-flight slot is already held...
        let holder = CacheLock::new(p.clone())
            .try_acquire(&remember_lock_name("slow"), REMEMBER_LOCK_TTL_SECS)
            .expect("external leader holds the lock");
        // ...and writes its value shortly after we start waiting.
        let writer = {
            let p = p.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(30));
                p.put("slow", to_bytes(&json!("leader")), Some(60));
            })
        };

        let ran = Cell::new(0u32);
        let out = c.remember("slow", Some(60), || {
            ran.set(ran.get() + 1);
            json!("mine")
        });
        writer.join().unwrap();

        assert_eq!(out, json!("leader"), "waiter observes the leader write");
        assert_eq!(ran.get(), 0, "single-flight: the waiter never computed");
        assert!(CacheLock::new(p.clone()).release(&holder));
    }

    #[test]
    fn remember_falls_back_when_the_leader_never_appears() {
        let (c, p) = service();
        let holder = CacheLock::new(p.clone())
            .try_acquire(&remember_lock_name("stuck"), REMEMBER_LOCK_TTL_SECS)
            .expect("leader holds the lock");

        let out = c.remember("stuck", Some(60), || json!("mine"));

        assert_eq!(out, json!("mine"), "bounded wait, then compute — never error");
        assert_eq!(c.get("stuck"), Some(json!("mine")));
        assert!(
            CacheLock::new(p.clone())
                .try_acquire(&remember_lock_name("stuck"), REMEMBER_LOCK_TTL_SECS)
                .is_none(),
            "the foreign hold is untouched: no steal, no forced release"
        );
        assert!(CacheLock::new(p.clone()).release(&holder));
    }

    #[test]
    fn tags_are_prefix_namespaces_with_prefix_flush() {
        let (c, p) = service();
        let view = c.tags(&["people", "admins"]);
        view.put("1", &json!({"name": "ada"}), Some(60));

        assert_eq!(c.get("1"), None, "tagged entries are namespaced");
        assert_eq!(c.tags(&["people"]).get("1"), Some(json!({"name": "ada"})));
        assert_eq!(
            c.tags(&["admins"]).get("1"),
            Some(json!({"name": "ada"})),
            "mirrored into every tag namespace"
        );
        assert!(
            p.keys().iter().all(|k| k.starts_with("tag:")),
            "both namespaces written under tag-prefixed keys"
        );

        // Flush one tag: its namespace goes, the sibling copy stays until
        // that tag is flushed too (documented prefix-namespace semantic).
        c.tags(&["people"]).flush();
        assert_eq!(c.tags(&["people"]).get("1"), None);
        assert_eq!(c.tags(&["admins"]).get("1"), Some(json!({"name": "ada"})));
        c.tags(&["admins"]).flush();
        assert_eq!(c.tags(&["admins"]).get("1"), None);
        assert!(p.keys().is_empty());
    }

    #[test]
    fn guard_acquire_closure_release_and_force_release() {
        let (c, _p) = service();

        // Closure form: runs under the lock and releases afterwards.
        let mut mine = c.lock("job", 30);
        assert_eq!(mine.get(|| 42), Some(42));
        assert!(!mine.release(), "already released by get()");

        // Contention: the waiter's closure never runs.
        let mut holder = c.lock("job", 30);
        assert!(holder.try_acquire());
        assert!(holder.try_acquire(), "idempotent while held by this guard");
        let ran = Cell::new(false);
        let mut waiter = c.lock("job", 30);
        assert_eq!(
            waiter.get(|| ran.set(true)),
            None,
            "contended get returns None"
        );
        assert!(!ran.get(), "closure skipped under contention");

        // force_release frees the slot regardless of the owner's token.
        waiter.force_release();
        assert!(!holder.release(), "holder's token no longer matches");
        let mut fresh = c.lock("job", 30);
        assert!(fresh.try_acquire(), "slot free after force_release");
    }
}
