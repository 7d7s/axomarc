//! The driver registry.
//!
//! An application should be able to choose its infrastructure **by
//! configuration** and to plug in **its own** driver without forking the
//! framework. Today the choice is an implicit environment variable read at
//! one place in the boot, with no way to see which driver is active and no
//! way to say what a driver can do.
//!
//! The registry makes the choice explicit and checkable:
//!
//! * a **named driver** with a **capability manifest** - a shared cache
//!   (across instances) is a different thing from a process-local one, and
//!   an application that needs the shared property must be able to ASK;
//! * a driver is a **recipe**, not an instance: it is BUILT when the
//!   configuration selects it, so a deployment that runs on a local cache
//!   never opens a connection to the shared one;
//! * **fail-closed** resolution - a name that is not registered is an error
//!   that names the name and lists what IS registered, never a silent
//!   fallback to a default;
//! * **swappable** - activating a driver swaps the runtime holder, so a
//!   configuration reload changes the infrastructure without a restart;
//! * **open registration** - an application registers its own driver with
//!   the same shape as the built-in ones.
//!
//! The registry holds the *cache* drivers today. The shape is deliberately
//! the one every adapter kind wants: a name, a capability manifest, and a
//! way to build the thing.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use kelson_foundation::AppError;

use crate::swappable::SwappableCache;
use crate::CachePort;

/// Builds a driver's store. The recipe runs ONCE per driver: the instance
/// it produces is memoized, so every holder of the driver talks to the
/// SAME store.
pub type CacheDriverBuilder =
    Arc<dyn Fn() -> Result<Arc<dyn CachePort>, AppError> + Send + Sync + 'static>;

/// What a cache driver can do.
///
/// The properties matter to an application, not to the framework: a
/// deployment that runs more than one instance needs a SHARED cache (or it
/// silently serves stale data from one instance), and a cache that claims
/// atomic set-if-absent must actually provide it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheCapabilities {
    /// The cache is shared across the instances of a deployment (Redis, a
    /// managed cache), rather than process-local.
    pub shared: bool,
    /// The cache provides the atomic primitives (set-if-absent,
    /// compare-and-delete) the cache locks rely on.
    pub atomic_primitives: bool,
    /// The maximum number of entries the driver holds (a process-local
    /// driver is bounded; a shared one usually is not).
    pub max_entries: Option<u64>,
}

impl CacheCapabilities {
    /// A process-local, bounded cache with the atomic primitives.
    pub fn process_local(max_entries: u64) -> Self {
        Self {
            shared: false,
            atomic_primitives: true,
            max_entries: Some(max_entries),
        }
    }

    /// A shared cache (across the instances of a deployment).
    pub fn shared() -> Self {
        Self {
            shared: true,
            atomic_primitives: true,
            max_entries: None,
        }
    }
}

/// One registered cache driver.
pub struct CacheDriver {
    name: String,
    capabilities: CacheCapabilities,
    builder: CacheDriverBuilder,
    /// The instance the recipe produced (memoized: the recipe runs once).
    built: RwLock<Option<Arc<dyn CachePort>>>,
}

impl CacheDriver {
    /// Register a driver over a store that already exists.
    ///
    /// The convenience form for a store the application built itself. The
    /// framework's own drivers use [`CacheDriver::with_builder`] so that
    /// nothing is built until the configuration selects it.
    pub fn new(
        name: impl Into<String>,
        capabilities: CacheCapabilities,
        store: Arc<dyn CachePort>,
    ) -> Self {
        Self::with_builder(name, capabilities, move || Ok(Arc::clone(&store)))
    }

    /// Register a driver as a RECIPE: the store is built when the
    /// configuration selects the driver, and never before.
    ///
    /// This is the form that makes a runtime swap honest: a deployment
    /// that runs on the local cache does not open a connection to the
    /// shared one until the configuration names it.
    pub fn with_builder(
        name: impl Into<String>,
        capabilities: CacheCapabilities,
        builder: impl Fn() -> Result<Arc<dyn CachePort>, AppError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            capabilities,
            builder: Arc::new(builder),
            built: RwLock::new(None),
        }
    }

    /// The driver's name (the name a configuration selects).
    pub fn name(&self) -> &str {
        &self.name
    }

    /// What the driver can do.
    pub fn capabilities(&self) -> CacheCapabilities {
        self.capabilities
    }

    /// The driver's store, BUILT ONCE.
    ///
    /// A driver whose recipe fails (a shared cache that is unreachable) is
    /// an error, never an empty store: the caller refuses rather than
    /// running on an infrastructure it was not configured for.
    pub fn store(&self) -> Result<Arc<dyn CachePort>, AppError> {
        if let Some(store) = self.built.read().ok().and_then(|built| built.clone()) {
            return Ok(store);
        }
        let store = (self.builder)()?;
        let mut built = self
            .built
            .write()
            .map_err(|_| AppError::persistence("the cache driver store lock is poisoned"))?;
        // Another caller may have won the race. Keeping the FIRST instance
        // means every holder of the driver talks to the same store.
        if let Some(existing) = built.as_ref() {
            return Ok(Arc::clone(existing));
        }
        *built = Some(Arc::clone(&store));
        Ok(store)
    }
}

impl Clone for CacheDriver {
    /// A clone shares the recipe and the instance it built.
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            capabilities: self.capabilities,
            builder: Arc::clone(&self.builder),
            built: RwLock::new(self.built.read().ok().and_then(|built| built.clone())),
        }
    }
}

impl std::fmt::Debug for CacheDriver {
    /// The name and the capabilities - the store itself is not printable.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CacheDriver")
            .field("name", &self.name)
            .field("capabilities", &self.capabilities)
            .finish()
    }
}

/// The registry: the named drivers, and which one is active.
#[derive(Default)]
pub struct CacheDriverRegistry {
    drivers: BTreeMap<String, CacheDriver>,
    active: RwLock<Option<String>>,
    /// The runtime holder: the active store, behind a lock, so a
    /// configuration reload can swap it without a restart.
    holder: RwLock<Option<Arc<SwappableCache>>>,
}

impl Clone for CacheDriverRegistry {
    /// A clone shares the drivers but not the active selection: the active
    /// driver is per-instance state (each process activates its own), while
    /// the registry of what EXISTS is the same everywhere. The runtime
    /// holder is per-instance too - a clone swaps its OWN holder, never
    /// the original's.
    fn clone(&self) -> Self {
        Self {
            drivers: self.drivers.clone(),
            active: RwLock::new(self.active.read().ok().and_then(|a| a.clone())),
            holder: RwLock::new(None),
        }
    }
}

impl CacheDriverRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a driver. Registering the same name twice replaces it -
    /// the last registration wins, so an application can override a
    /// built-in driver with its own.
    pub fn register(&mut self, driver: CacheDriver) {
        self.drivers.insert(driver.name.clone(), driver);
    }

    /// Register a driver and make it active.
    ///
    /// The activation can FAIL (the driver's recipe cannot build the
    /// store), so this is fallible: the caller decides what a boot does
    /// with a driver it cannot build.
    pub fn register_active(&mut self, driver: CacheDriver) -> Result<(), AppError> {
        let name = driver.name.clone();
        self.register(driver);
        self.activate(&name)
    }

    /// The registered names, sorted.
    pub fn names(&self) -> Vec<String> {
        self.drivers.keys().cloned().collect()
    }

    /// Resolve a driver by name.
    ///
    /// **Fail closed**: a name that is not registered is an error that
    /// names the name and lists what IS registered. A silent fallback to a
    /// default would run the deployment on an infrastructure it was not
    /// configured for.
    pub fn driver(&self, name: &str) -> Result<&CacheDriver, AppError> {
        self.drivers.get(name).ok_or_else(|| {
            AppError::validation(format!(
                "the cache driver {name:?} is not registered (registered: {})",
                self.names().join(", ")
            ))
        })
    }

    /// Make a registered driver the active one.
    ///
    /// This is the SWAP: the driver's store is built (once) and installed
    /// in the runtime holder, so every cache read and write in the
    /// process follows the new driver from the next call on. A driver
    /// that cannot be built is refused and the previous driver stays
    /// active - fail-closed, never a half-applied change.
    pub fn activate(&self, name: &str) -> Result<(), AppError> {
        // Resolve first: activating an unregistered name is refused, and
        // the ACTIVE driver is left as it was.
        let driver = self.driver(name)?;
        // Build the store: a shared cache that is unreachable is a
        // refusal, not a silent fallback to a local one.
        let store = driver.store()?;
        // Install the store in the holder BEFORE the name is recorded, so
        // the active name and the active store never disagree.
        {
            let mut holder = self
                .holder
                .write()
                .map_err(|_| AppError::persistence("the cache driver registry lock is poisoned"))?;
            match holder.as_ref() {
                Some(holder) => holder.swap(Arc::clone(&store))?,
                None => {
                    *holder = Some(Arc::new(SwappableCache::new(Arc::clone(&store))));
                }
            }
        }
        let mut active = self
            .active
            .write()
            .map_err(|_| AppError::persistence("the cache driver registry lock is poisoned"))?;
        *active = Some(name.to_string());
        Ok(())
    }

    /// The active driver, if one is active.
    pub fn active(&self) -> Result<Option<CacheDriver>, AppError> {
        let active = self
            .active
            .read()
            .map_err(|_| AppError::persistence("the cache driver registry lock is poisoned"))?;
        Ok(active
            .as_deref()
            .and_then(|name| self.drivers.get(name))
            .cloned())
    }

    /// The active driver's store, if one is active.
    pub fn active_store(&self) -> Result<Option<Arc<dyn CachePort>>, AppError> {
        match self.active()? {
            Some(driver) => Ok(Some(driver.store()?)),
            None => Ok(None),
        }
    }

    /// The runtime holder: the active store, behind a lock.
    ///
    /// The holder is what a deployment hands to the cache service, so that
    /// a configuration reload can change the active driver under the
    /// running service.
    pub fn holder(&self) -> Option<Arc<SwappableCache>> {
        self.holder.read().ok().and_then(|holder| holder.clone())
    }

    /// How many times the active driver CHANGED at runtime.
    ///
    /// The operator's evidence that a configuration change took effect:
    /// a reload that re-activates the same driver is not a change.
    pub fn swaps(&self) -> u64 {
        self.holder().map(|holder| holder.swaps()).unwrap_or(0)
    }

    /// Whether the active driver is SHARED across a deployment's instances.
    ///
    /// An application that runs more than one instance and needs a
    /// consistent cache asks this before it decides the deployment is
    /// safe - the property that is easy to assume and expensive to get
    /// wrong.
    pub fn active_is_shared(&self) -> Result<bool, AppError> {
        Ok(self
            .active()?
            .map(|d| d.capabilities.shared)
            .unwrap_or(false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// A trivial in-process store (the registry's tests are about the
    /// registry, not about a cache).
    #[derive(Default)]
    struct TestStore {
        entries: Mutex<BTreeMap<String, Vec<u8>>>,
    }

    impl CachePort for TestStore {
        fn get(&self, key: &str) -> Option<Vec<u8>> {
            self.entries.lock().unwrap().get(key).cloned()
        }
        fn put(&self, key: &str, value: Vec<u8>, _ttl: Option<u64>) {
            self.entries.lock().unwrap().insert(key.to_string(), value);
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

    fn driver(name: &str, shared: bool) -> CacheDriver {
        CacheDriver::new(
            name,
            if shared {
                CacheCapabilities::shared()
            } else {
                CacheCapabilities::process_local(1_000)
            },
            Arc::new(TestStore::default()),
        )
    }

    #[test]
    fn a_registered_driver_resolves_and_activates() {
        let mut registry = CacheDriverRegistry::new();
        registry.register_active(driver("memory", false)).unwrap();
        assert_eq!(registry.names(), vec!["memory".to_string()]);
        assert_eq!(registry.active().unwrap().unwrap().name(), "memory");
        assert!(!registry.active_is_shared().unwrap());
    }

    #[test]
    fn an_unregistered_name_refuses_and_lists_what_is_registered() {
        let mut registry = CacheDriverRegistry::new();
        registry.register_active(driver("memory", false)).unwrap();
        let err = registry
            .driver("redis")
            .expect_err("an unregistered name is refused");
        assert!(
            err.public_message.contains("redis") && err.public_message.contains("memory"),
            "the refusal must name the name and list what IS registered: {}",
            err.public_message
        );
    }

    #[test]
    fn activating_an_unregistered_name_leaves_the_active_driver_alone() {
        let mut registry = CacheDriverRegistry::new();
        registry.register_active(driver("memory", false)).unwrap();
        let _ = registry.activate("redis");
        assert_eq!(
            registry.active().unwrap().unwrap().name(),
            "memory",
            "a refused activation must not clear the active driver"
        );
    }

    #[test]
    fn an_application_can_replace_a_built_in_driver() {
        let mut registry = CacheDriverRegistry::new();
        registry.register_active(driver("memory", false)).unwrap();
        registry.register_active(driver("memory", true)).unwrap();
        assert!(
            registry.active_is_shared().unwrap(),
            "the last registration wins, so an application can override a built-in"
        );
    }

    #[test]
    fn the_capability_manifest_is_readable() {
        let mut registry = CacheDriverRegistry::new();
        registry.register(driver("shared", true));
        let caps = registry.driver("shared").unwrap().capabilities();
        assert!(caps.shared);
        assert!(caps.atomic_primitives);
        assert_eq!(caps.max_entries, None);
    }

    #[test]
    fn the_recipe_runs_when_the_driver_is_selected_not_when_it_is_registered() {
        let built = Arc::new(Mutex::new(0usize));
        let counter = Arc::clone(&built);
        let mut registry = CacheDriverRegistry::new();
        registry.register(CacheDriver::with_builder(
            "shared",
            CacheCapabilities::shared(),
            move || {
                *counter.lock().unwrap() += 1;
                Ok(Arc::new(TestStore::default()) as Arc<dyn CachePort>)
            },
        ));
        assert_eq!(
            *built.lock().unwrap(),
            0,
            "registering a driver must not build it: a deployment that runs on another cache must not open this one"
        );
        registry.activate("shared").unwrap();
        assert_eq!(*built.lock().unwrap(), 1, "the selection builds it");
        registry.activate("shared").unwrap();
        assert_eq!(
            *built.lock().unwrap(),
            1,
            "the recipe runs ONCE: the instance is memoized"
        );
    }

    #[test]
    fn a_driver_that_cannot_be_built_is_refused_and_the_previous_driver_stays() {
        let mut registry = CacheDriverRegistry::new();
        registry.register_active(driver("memory", false)).unwrap();
        registry.register(CacheDriver::with_builder(
            "redis",
            CacheCapabilities::shared(),
            || {
                Err(AppError::validation(
                    "the cache driver `redis` is unreachable",
                ))
            },
        ));
        let err = registry
            .activate("redis")
            .expect_err("a driver that cannot be built is refused");
        assert!(err.public_message.contains("unreachable"));
        assert_eq!(
            registry.active().unwrap().unwrap().name(),
            "memory",
            "a refused activation leaves the previous driver active"
        );
    }

    #[test]
    fn activating_a_driver_swaps_the_runtime_holder() {
        let mut registry = CacheDriverRegistry::new();
        registry.register_active(driver("memory", false)).unwrap();
        let holder = registry.holder().expect("the holder exists");
        holder.store().put("k", b"memory".to_vec(), None);
        assert_eq!(registry.swaps(), 0, "the first activation is not a swap");

        registry.register_active(driver("redis", true)).unwrap();
        assert_eq!(registry.swaps(), 1, "the activation changed the driver");
        assert_eq!(
            holder.store().get("k"),
            None,
            "the holder reads the NEW driver"
        );
        assert_eq!(
            registry.active().unwrap().unwrap().name(),
            "redis",
            "the active driver is the new one"
        );
    }
}
