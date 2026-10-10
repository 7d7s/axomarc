//! # ConfigRuntime (target-arch §4.1 + §5.4)
//!
//! Versioned config snapshots with an atomic swap, subscriber notification
//! and a history ring for rollback. The boot-time `FrozenConfig` stays the
//! compatibility view (getters only); this runtime adds the reload
//! dimension ON TOP: readers hold `Arc<ConfigSnapshot>` and observe a new
//! revision only at swap points — in-flight readers keep their old `Arc`
//! (§4.2 semantics: no torn reads, no restart).
//!
//! Triggers (§4.1): file mtime via [`ConfigRuntime::poll`] (cheap probe,
//! no work when unchanged), manual via [`ConfigRuntime::reload`] (the
//! `kelson config:reload` CLI path). SIGHUP and etcd/consul are explicit
//! plug points — implement [`ConfigSource`] for them; no watcher
//! dependency ships in this crate.
//!
//! Class note: `ConfigSnapshot` carries ONLY kelson-config's own
//! [`PlatformConfig`]. The §4.1 sketch inlines `tenant_overrides`,
//! `feature_flags` and `route_manifest` into the snapshot — those are
//! Enterprise/registry-class types this crate may not depend on (arch
//! matrix), so they subscribe through [`ConfigReloader`] instead, matching
//! the §8 `ConfigReloader` port row.
//!
//! Fail-safe: a revision is validated by the SAME layered load boot uses
//! (`PlatformConfig::load` — schema gate, parse gate, secret asserts), so
//! an invalid config can never swap in; the current revision stays active
//! and the error surfaces to the caller.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::SystemTime;

use kelson_foundation::AppError;

use crate::{FrozenConfig, PlatformConfig};

/// §5.4: the history ring keeps the last N revisions for `rollback`.
pub const HISTORY_MAX: usize = 10;

/// One immutable, content-addressed config revision (§4.1).
#[derive(Debug, Clone)]
pub struct ConfigSnapshot {
    /// Monotonic revision number (starts at 1 at boot; never reused, even
    /// after a rollback, so `rollback(target)` stays unambiguous).
    pub version: u64,
    pub config: PlatformConfig,
    /// SHA-256 over the canonical effective config — the change oracle:
    /// equal configs hash equal, so a touched-but-identical file does not
    /// bump the version or notify subscribers.
    pub hash: String,
}

impl ConfigSnapshot {
    /// Compatibility view: the getters-only handle boot already consumes.
    /// A consumer holding `FrozenConfig` reads the revision it was made
    /// from; re-freeze after a swap to observe the new one.
    pub fn frozen(&self) -> FrozenConfig {
        FrozenConfig(Arc::new(self.config.clone()))
    }
}

/// Subscriber port (§4.1 `on_reload`, §8 `ConfigReloader`): called AFTER a
/// successful swap with the new revision. Route-cache and feature-flag
/// snapshots subscribe here and re-derive their own artifacts — that is
/// the hot-swap wiring (Phase 3 step 2).
pub trait ConfigReloader: Send + Sync {
    fn on_reload(&self, snapshot: &ConfigSnapshot);
}

/// Change-detection + load source (§4.1 triggers). The file impl ships
/// here; SIGHUP/etcd/consul are additional impls — the runtime only ever
/// talks to this trait.
pub trait ConfigSource: Send + Sync {
    /// Cheap probe: is a reload worth attempting? Must be side-effect free
    /// (peek): a failed reload leaves the stamp untouched so the next poll
    /// retries instead of silently skipping a half-written file.
    fn changed(&self) -> bool;
    /// Full layered load — identical gates to boot (schema violations,
    /// parse errors, secret asserts), so reload can never accept what boot
    /// would refuse.
    fn load(&self) -> Result<PlatformConfig, AppError>;
    /// Baseline advance: called by the runtime after a successful reload
    /// (swap or hash-equal no-op) so `changed()` stops reporting it.
    fn mark_applied(&self) {}
}

/// File source with an mtime stamp (§4.1 "file mtime change" trigger).
pub struct FileConfigSource {
    path: PathBuf,
    stamp: RwLock<Option<SystemTime>>,
}

impl FileConfigSource {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let source = Self {
            path: path.into(),
            stamp: RwLock::new(None),
        };
        // Baseline at construction: boot-time state is "applied".
        source.mark_applied();
        source
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    fn mtime(&self) -> Option<SystemTime> {
        std::fs::metadata(&self.path).and_then(|m| m.modified()).ok()
    }
}

impl ConfigSource for FileConfigSource {
    fn changed(&self) -> bool {
        self.mtime() != *self.stamp.read().unwrap()
    }

    fn load(&self) -> Result<PlatformConfig, AppError> {
        PlatformConfig::load(Some(&self.path))
    }

    fn mark_applied(&self) {
        *self.stamp.write().unwrap() = self.mtime();
    }
}

/// The runtime (§4.1): current revision + history ring + subscribers.
/// All reads are `RwLock`-shared `Arc` clones — cheap, and never blocking
/// a swap's notify phase for readers already holding the old revision.
///
/// Readers deliberately do NOT take [`ConfigRuntime::commit`]: only the
/// mutating paths serialize, so the reader guarantee above is preserved.
pub struct ConfigRuntime {
    source: Box<dyn ConfigSource>,
    current: RwLock<Arc<ConfigSnapshot>>,
    history: Mutex<VecDeque<Arc<ConfigSnapshot>>>,
    reloaders: Mutex<Vec<Arc<dyn ConfigReloader>>>,
    /// Next revision number (monotonic; rollback does not rewind it).
    next_version: AtomicU64,
    /// Serializes each mutation END TO END: load → compare → version bump →
    /// swap → history push → notify.
    ///
    /// Without this, two concurrent `reload()`s interleave as
    /// `fetch_add(1)` (line ~208) / `current.write()` (line ~212): they can
    /// draw version numbers in one order and commit them in the REVERSE
    /// order. That regresses `current` to an older payload (a lost update —
    /// the newest file content is superseded by a stale load) and notifies
    /// route/flag subscribers out of order, so the hot-swap wiring installs
    /// stale routes. Proven by the Phase 5 chaos test, which fails with
    /// `[.. 7, 6 ..]` when this lock is absent.
    ///
    /// The lock must cover `source.load()` too: serializing only the compare
    /// and commit still lets an earlier thread's stale load land after a
    /// newer one. Lock order everywhere is `commit` → (`current` | `history`
    /// | `reloaders`), never the reverse, so no deadlock is reachable.
    commit: Mutex<()>,
}

/// Content hash over the canonical effective config — delegates to the
/// shared `kelson-registry` primitive so the runtime, `kelson config:cache`
/// and the boot drift-refuse gate all agree byte-for-byte.
pub fn hash_of(config: &PlatformConfig) -> Result<String, AppError> {
    let value = serde_json::to_value(config)
        .map_err(|e| AppError::internal(format!("config hash: {e}")))?;
    Ok(kelson_registry::config_hash(&value))
}

impl ConfigRuntime {
    /// Boot an already-loaded configuration (version 1). The caller has
    /// just run the same layered load, so no duplicate validation happens
    /// here — only hashing.
    pub fn boot(
        source: impl ConfigSource + 'static,
        config: PlatformConfig,
    ) -> Result<Self, AppError> {
        let hash = hash_of(&config)?;
        let snap = Arc::new(ConfigSnapshot {
            version: 1,
            config,
            hash,
        });
        let mut history = VecDeque::new();
        history.push_back(snap.clone());
        Ok(Self {
            source: Box::new(source),
            current: RwLock::new(snap),
            history: Mutex::new(history),
            reloaders: Mutex::new(Vec::new()),
            next_version: AtomicU64::new(2),
            commit: Mutex::new(()),
        })
    }

    /// Boot straight from a source: the initial load runs the full gate
    /// (fail-closed, like boot) and errors surface here.
    pub fn from_source(source: impl ConfigSource + 'static) -> Result<Self, AppError> {
        let config = source.load()?;
        Self::boot(source, config)
    }

    /// Lock-light read: clone the current `Arc` (§4.1 `get`).
    pub fn get(&self) -> Arc<ConfigSnapshot> {
        self.current.read().unwrap().clone()
    }

    /// Register a subscriber (route cache, flag service, …). Subscribers
    /// boot with their own initial state; this is for REVISIONS only.
    pub fn subscribe(&self, reloader: impl ConfigReloader + 'static) {
        self.reloaders
            .lock()
            .unwrap()
            .push(Arc::new(reloader));
    }

    /// Explicit reload (§4.1): load → hash-compare → swap → notify.
    /// - load error → `Err`, current revision untouched (fail-safe),
    /// - hash equal → no swap, no notify, `Ok(current)`,
    /// - hash differs → version bump, swap, history push, notify in
    ///   registration order.
    pub fn reload(&self) -> Result<Arc<ConfigSnapshot>, AppError> {
        // Mutation critical section (see `commit`). The load sits INSIDE the
        // lock deliberately: serializing only the compare-and-commit still
        // lets an earlier thread's stale load land after a newer one.
        let _commit = self.commit.lock().unwrap();
        let new = self.source.load()?;
        let hash = hash_of(&new)?;
        let current = self.current.read().unwrap().clone();
        let next = if hash == current.hash {
            current
        } else {
            let next = Arc::new(ConfigSnapshot {
                version: self.next_version.fetch_add(1, Ordering::SeqCst),
                config: new,
                hash,
            });
            *self.current.write().unwrap() = next.clone();
            {
                let mut history = self.history.lock().unwrap();
                if history.len() >= HISTORY_MAX {
                    history.pop_front();
                }
                history.push_back(next.clone());
            }
            for reloader in self.reloaders.lock().unwrap().iter() {
                reloader.on_reload(&next);
            }
            next
        };
        self.source.mark_applied();
        Ok(next)
    }

    /// Poll trigger (mtime): `Ok(None)` = unchanged (no load, no notify,
    /// no subscriber churn). A failed load leaves the stamp untouched, so
    /// the next poll retries until the file settles.
    pub fn poll(&self) -> Result<Option<Arc<ConfigSnapshot>>, AppError> {
        if self.source.changed() {
            self.reload().map(Some)
        } else {
            Ok(None)
        }
    }

    /// §5.4 rollback: swap back to a held revision and re-notify. The
    /// source file is NOT rewritten — the next file change reloads on top
    /// again (rollback is a runtime-state decision, `versions()` keeps
    /// every revision unambiguous because numbers are never reused).
    pub fn rollback(&self, target_version: u64) -> Result<Arc<ConfigSnapshot>, AppError> {
        // Same mutation critical section as `reload`: a rollback racing a
        // reload would otherwise notify subscribers out of revision order.
        let _commit = self.commit.lock().unwrap();
        let target = self
            .history
            .lock()
            .unwrap()
            .iter()
            .find(|s| s.version == target_version)
            .cloned()
            .ok_or_else(|| AppError::not_found(format!("config version {target_version}")))?;
        *self.current.write().unwrap() = target.clone();
        for reloader in self.reloaders.lock().unwrap().iter() {
            reloader.on_reload(&target);
        }
        Ok(target)
    }

    /// Every revision still held (ascending), for CLI/tests.
    pub fn versions(&self) -> Vec<u64> {
        self.history
            .lock()
            .unwrap()
            .iter()
            .map(|s| s.version)
            .collect()
    }

    /// Number of revisions in the ring (≤ [`HISTORY_MAX`]).
    pub fn history_len(&self) -> usize {
        self.history.lock().unwrap().len()
    }
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Recording subscriber: remembers every notified revision, and does a
    /// fake "route manifest + flag snapshot" swap so the DoD timing test
    /// exercises realistic subscriber work.
    struct Recorder {
        seen: Mutex<Vec<u64>>,
        routes: Mutex<Arc<str>>,
        flags: Mutex<u32>,
    }

    impl Recorder {
        fn new() -> Self {
            Self {
                seen: Mutex::new(Vec::new()),
                routes: Mutex::new(Arc::from("v1-routes")),
                flags: Mutex::new(0),
            }
        }
    }

    impl ConfigReloader for Arc<Recorder> {
        fn on_reload(&self, snapshot: &ConfigSnapshot) {
            self.seen.lock().unwrap().push(snapshot.version);
            *self.routes.lock().unwrap() =
                Arc::from(format!("routes-{}", snapshot.version));
            *self.flags.lock().unwrap() += 1;
        }
    }

    fn temp_config(dir: &std::path::Path, operations: u64) -> PathBuf {
        let path = dir.join("config.toml");
        let raw = format!(
            "[app]\nname = \"reload-probe\"\nenv = \"dev\"\n\n[limits]\ndefault_monthly_operations = {operations}\n"
        );
        std::fs::write(&path, raw).unwrap();
        path
    }

    fn runtime_with(dir: &std::path::Path, operations: u64) -> (ConfigRuntime, Arc<Recorder>) {
        let path = temp_config(dir, operations);
        let source = FileConfigSource::new(&path);
        let config = PlatformConfig::load(Some(&path)).expect("probe config loads");
        let runtime = ConfigRuntime::boot(source, config).expect("boot hashes");
        let recorder = Arc::new(Recorder::new());
        runtime.subscribe(recorder.clone());
        (runtime, recorder)
    }

    /// DoD (§10 "boot-time freeze"): a reload swaps the revision and the
    /// subscribers' route/flag views — with NO restart — in well under the
    /// 50 ms budget.
    #[test]
    fn config_reload_swaps_routes_and_flags_under_50ms() {
        let dir = std::env::temp_dir().join(format!("kelson-cfg-reload-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (runtime, recorder) = runtime_with(&dir, 100_000);

        // Edit the file (env-override-proof key: limits.*).
        temp_config(&dir, 42);
        let started = std::time::Instant::now();
        let next = runtime.reload().expect("reload valid config");
        let elapsed = started.elapsed();

        assert_eq!(next.version, 2, "hash differs -> version bump");
        assert_eq!(next.config.limits.default_monthly_operations, 42);
        assert!(
            elapsed < std::time::Duration::from_millis(50),
            "reload + subscriber swaps must stay under 50ms, took {elapsed:?}"
        );
        assert_eq!(*recorder.seen.lock().unwrap(), vec![2]);
        assert_eq!(
            recorder.routes.lock().unwrap().as_ref(),
            &"routes-2".to_string(),
            "route view swapped without restart"
        );
        assert_eq!(*recorder.flags.lock().unwrap(), 1, "flags re-resolved");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Touched-but-identical file: hash equal → no bump, no notify.
    #[test]
    fn config_reload_unchanged_content_is_a_noop() {
        let dir = std::env::temp_dir().join(format!("kelson-cfg-noop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (runtime, recorder) = runtime_with(&dir, 100_000);

        // Touched (rewritten) but byte-identical: hash equal.
        temp_config(&dir, 100_000);
        let same = runtime.reload().expect("valid, unchanged");
        assert_eq!(same.version, 1, "equal hash keeps the revision");
        assert!(recorder.seen.lock().unwrap().is_empty(), "no notify");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Invalid config (schema-gate offender) can never swap in: current
    /// revision stays, subscribers stay silent, error surfaces.
    #[test]
    fn config_reload_invalid_config_keeps_current_revision() {
        let dir = std::env::temp_dir().join(format!("kelson-cfg-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (runtime, recorder) = runtime_with(&dir, 100_000);

        std::fs::write(dir.join("config.toml"), "[app]\nnmae = \"typo\"\n").unwrap();
        let err = runtime.reload().expect_err("schema gate must refuse");
        assert!(
            err.to_string().contains(crate::CONFIG_SCHEMA_CODE),
            "reload refuses exactly what boot refuses: {err}"
        );
        let current = runtime.get();
        assert_eq!(current.version, 1, "old revision stays active");
        assert!(recorder.seen.lock().unwrap().is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    /// mtime poll: baseline at boot is quiet; a changed file is seen;
    /// after reload the stamp is current again.
    #[test]
    fn config_poll_detects_file_change_once() {
        let dir = std::env::temp_dir().join(format!("kelson-cfg-poll-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (runtime, _recorder) = runtime_with(&dir, 100_000);

        assert!(runtime.poll().unwrap().is_none(), "baseline is quiet");

        // NTFS mtime resolution is ~100ns — the rewrite below is picked up.
        temp_config(&dir, 7);

        let swapped = runtime.poll().unwrap().expect("poll sees the change");
        assert_eq!(swapped.version, 2);
        assert_eq!(swapped.config.limits.default_monthly_operations, 7);
        assert!(runtime.poll().unwrap().is_none(), "stamp is current");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// §5.4 rollback: swap back to a held revision, re-notify, and never
    /// reuse a version number.
    #[test]
    fn config_rollback_returns_a_held_revision() {
        let dir = std::env::temp_dir().join(format!("kelson-cfg-rb-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (runtime, recorder) = runtime_with(&dir, 100_000);

        temp_config(&dir, 42);
        runtime.reload().expect("v2");
        let back = runtime.rollback(1).expect("v1 is held");
        assert_eq!(back.version, 1);
        assert_eq!(back.config.limits.default_monthly_operations, 100_000);
        assert_eq!(*recorder.seen.lock().unwrap(), vec![2, 1]);
        assert_eq!(runtime.get().version, 1);

        // Numbers are never reused: the next change becomes v3, not v2.
        temp_config(&dir, 7);
        let next = runtime.reload().expect("v3");
        assert_eq!(next.version, 3);
        assert_eq!(runtime.versions(), vec![1, 2, 3]);

        let missing = runtime.rollback(99).expect_err("unknown revision");
        assert!(missing.to_string().contains("config version 99"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// History ring caps at HISTORY_MAX (oldest evicted).
    #[test]
    fn config_history_ring_caps_at_max() {
        let dir = std::env::temp_dir().join(format!("kelson-cfg-ring-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (runtime, _recorder) = runtime_with(&dir, 1);

        for i in 2..=(HISTORY_MAX as u64 + 3) {
            temp_config(&dir, i);
            runtime.reload().expect("each revision valid");
        }
        assert_eq!(runtime.history_len(), HISTORY_MAX);
        assert_eq!(runtime.get().version, HISTORY_MAX as u64 + 3);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// The frozen compatibility view maps to the snapshot's revision.
    #[test]
    fn config_snapshot_frozen_view_matches_revision() {
        let dir = std::env::temp_dir().join(format!("kelson-cfg-frz-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (runtime, _recorder) = runtime_with(&dir, 100_000);
        let frozen = runtime.get().frozen();
        assert_eq!(frozen.app().name, "reload-probe");
        assert_eq!(frozen.limits().default_monthly_operations, 100_000);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Chaos (Phase 5, §9 "config reload under load"): four writers reload a
    /// mutating file while four readers hammer `get()`. Four invariants must
    /// hold at every observation and at quiescence:
    ///
    /// 1. every observed snapshot is content-addressed — `hash` still matches
    ///    the payload it carries (no torn revision);
    /// 2. a reader never watches the revision go backwards (this test never
    ///    calls `rollback`, so a regression means a lost update);
    /// 3. subscribers are notified once per swap, in strictly ascending order;
    /// 4. the last notified revision IS the active revision, and the history
    ///    ring is ordered to match.
    ///
    /// The writers deliberately reload OUTSIDE the file gate so two swaps can
    /// interleave — that interleaving is precisely what is under test.
    #[test]
    fn config_reload_under_load_never_tears_or_regresses_a_revision() {
        const WRITERS: usize = 4;
        const READERS: usize = 4;
        const PER_WRITER: usize = 50;

        let dir = std::env::temp_dir().join(format!("kelson-cfg-hammer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (runtime, recorder) = runtime_with(&dir, 100_000);
        let runtime = Arc::new(runtime);

        let file_gate = Arc::new(Mutex::new(()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let mut writers = Vec::new();
        for w in 0..WRITERS {
            let rt = runtime.clone();
            let gate = file_gate.clone();
            let path_dir = dir.clone();
            writers.push(std::thread::spawn(move || {
                for i in 0..PER_WRITER {
                    let ops = 100_000 + (w as u64) * 1_000 + i as u64;
                    {
                        let _held = gate.lock().unwrap();
                        temp_config(&path_dir, ops);
                    }
                    // A half-written file must fail safe (current untouched,
                    // error surfaced) — that is correct, so failures pass.
                    let _ = rt.reload();
                }
            }));
        }

        let mut readers = Vec::new();
        for _ in 0..READERS {
            let rt = runtime.clone();
            let stop = stop.clone();
            readers.push(std::thread::spawn(move || {
                let mut last = 0u64;
                let mut observed = 0usize;
                while !stop.load(Ordering::Relaxed) {
                    let snap = rt.get();

                    let recomputed = hash_of(&snap.config).expect("snapshot hashes");
                    assert_eq!(
                        recomputed, snap.hash,
                        "torn revision: version {} carries another payload's hash",
                        snap.version
                    );

                    assert!(
                        snap.version >= last,
                        "revision regressed under load: {last} -> {}",
                        snap.version
                    );
                    last = snap.version;
                    observed += 1;
                }
                observed
            }));
        }

        for h in writers {
            match h.join() {
                Ok(()) => {}
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }
        stop.store(true, Ordering::Relaxed);
        for h in readers {
            match h.join() {
                Ok(observed) => assert!(observed > 0, "a reader never observed a revision"),
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }

        let seen = recorder.seen.lock().unwrap().clone();
        assert!(!seen.is_empty(), "writers swapped but no subscriber ran");
        assert!(
            seen.windows(2).all(|w| w[0] < w[1]),
            "subscriber notifications arrived out of ascending order: {seen:?}"
        );

        let active = runtime.get().version;
        assert_eq!(
            seen.last().copied(),
            Some(active),
            "subscribers were last told {:?} but the runtime holds revision {active}",
            seen.last()
        );

        let versions = runtime.versions();
        assert!(
            versions.windows(2).all(|w| w[0] < w[1]),
            "history ring committed out of ascending order: {versions:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
