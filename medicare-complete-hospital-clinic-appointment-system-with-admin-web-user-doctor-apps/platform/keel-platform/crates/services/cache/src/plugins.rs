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
//! * **fail-closed** resolution - a name that is not registered is an error
//!   that names the name and lists what IS registered, never a silent
//!   fallback to a default;
//! * **swappable** - the active driver sits behind a lock, so a
//!   configuration reload can swap it without a restart;
//! * **open registration** - an application registers its own driver with
//!   the same shape as the built-in ones.
//!
//! The registry holds the *cache* drivers today. The shape is deliberately
//! the one every adapter kind wants: a name, a capability manifest, and an
//! instance.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use kelson_foundation::AppError;

use crate::CachePort;

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
#[derive(Clone)]
pub struct CacheDriver {
    name: String,
    capabilities: CacheCapabilities,
    store: Arc<dyn CachePort>,
}

impl CacheDriver {
    /// Register a driver under `name`.
    pub fn new(
        name: impl Into<String>,
        capabilities: CacheCapabilities,
        store: Arc<dyn CachePort>,
    ) -> Self {
        Self {
            name: name.into(),
            capabilities,
            store,
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

    /// The driver's store.
    pub fn store(&self) -> Arc<dyn CachePort> {
        Arc::clone(&self.store)
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
}

impl Clone for CacheDriverRegistry {
    /// A clone shares the drivers but not the active selection: the active
    /// driver is per-instance state (each process activates its own), while
    /// the registry of what EXISTS is the same everywhere.
    fn clone(&self) -> Self {
        Self {
            drivers: self.drivers.clone(),
            active: RwLock::new(self.active.read().ok().and_then(|a| a.clone())),
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
    pub fn register_active(&mut self, driver: CacheDriver) {
        let name = driver.name.clone();
        self.register(driver);
        self.activate(&name).expect("a registered driver activates");
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
    pub fn activate(&self, name: &str) -> Result<(), AppError> {
        // Resolve first: activating an unregistered name is refused, and
        // the ACTIVE driver is left as it was.
        self.driver(name)?;
        let mut active = self.active.write().map_err(|_| {
            AppError::persistence("the cache driver registry lock is poisoned")
        })?;
        *active = Some(name.to_string());
        Ok(())
    }

    /// The active driver, if one is active.
    pub fn active(&self) -> Result<Option<CacheDriver>, AppError> {
        let active = self
            .active
            .read()
            .map_err(|_| AppError::persistence("the cache driver registry lock is poisoned"))?;
        Ok(active.as_deref().and_then(|name| self.drivers.get(name)).cloned())
    }

    /// The active driver's store, if one is active.
    pub fn active_store(&self) -> Result<Option<Arc<dyn CachePort>>, AppError> {
        Ok(self.active()?.map(|driver| driver.store()))
    }

    /// Whether the active driver is SHARED across a deployment's instances.
    ///
    /// An application that runs more than one instance and needs a
    /// consistent cache asks this before it decides the deployment is
    /// safe - the property that is easy to assume and expensive to get
    /// wrong.
    pub fn active_is_shared(&self) -> Result<bool, AppError> {
        Ok(self.active()?.map(|d| d.capabilities.shared).unwrap_or(false))
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
        registry.register_active(driver("memory", false));
        assert_eq!(registry.names(), vec!["memory".to_string()]);
        assert_eq!(registry.active().unwrap().unwrap().name(), "memory");
        assert!(!registry.active_is_shared().unwrap());
    }

    #[test]
    fn an_unregistered_name_refuses_and_lists_what_is_registered() {
        let mut registry = CacheDriverRegistry::new();
        registry.register_active(driver("memory", false));
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
        registry.register_active(driver("memory", false));
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
        registry.register_active(driver("memory", false));
        registry.register_active(driver("memory", true));
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
}
