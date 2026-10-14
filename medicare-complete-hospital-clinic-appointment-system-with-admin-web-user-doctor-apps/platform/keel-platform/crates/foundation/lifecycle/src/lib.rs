//! # kelson-lifecycle
//! `ApplicationHost` + `Provider` lifecycle (G03/G04). Providers declare
//! `dependencies()`; the host registers, topo-sorts (cycle = boot refusal, S1),
//! boots in dependency order, then freezes the registry. Missing/duplicate
//! services are boot errors with precise messages (fail fast).

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::Arc;

use kelson_foundation::{AppError, ErrorCategory, Retryability, Severity};

/// Boot phases, strictly ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Phase {
    /// Config, logging, secrets.
    Foundation = 0,
    /// Core engine ports (db pool, execution backend, authorization).
    Kernel = 1,
    /// Framework services (buses, caches, notifications).
    Platform = 2,
    /// Domain modules and their registrations.
    Modules = 3,
    /// Freeze + readiness.
    Ready = 4,
}

/// Module lifecycle states (docs/15-EXTENSION §Modules).
/// 
/// The seven states represent the full lifecycle of a module from discovery
/// to disablement. The first five (`Discovered` through `Ready`) are the
/// normal boot progression; `Degraded` and `Disabled` are operational
/// states that can be entered at runtime.
/// 
/// - `Discovered` — module.toml found on disk (graph compiler input)
/// - `Validated` — manifest passes GRAPH gates (ABI, capability, etc.)
/// - `Registered` — ModuleProvider registered in ServiceRegistry
/// - `Initialized` — provider.init() ran, services registered
/// - `Ready` — provider.boot() completed, module serving
/// - `Degraded` — module serving but with reduced capability (e.g. a
///   dependency unavailable, circuit breaker open)
/// - `Disabled` — module explicitly disabled; all operations refused
/// 
/// Transitions: Discovered -> Validated -> Registered -> Initialized -> Ready
/// From Ready: can transition to Degraded or Disabled.
/// From Degraded: can transition back to Ready (recovery) or Disabled.
/// From Disabled: terminal state (no recovery without restart).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ModuleState {
    Discovered = 0,
    Validated = 1,
    Registered = 2,
    Initialized = 3,
    Ready = 4,
    Degraded = 5,
    Disabled = 6,
}

impl ModuleState {
    /// Whether this state represents a module that is actively serving.
    pub fn is_serving(self) -> bool {
        matches!(self, Self::Ready | Self::Degraded)
    }
    
    /// Whether this state allows transitioning to the given state.
    pub fn can_transition_to(self, target: Self) -> bool {
        use ModuleState::*;
        match (self, target) {
            // Normal boot progression
            (Discovered, Validated) => true,
            (Validated, Registered) => true,
            (Registered, Initialized) => true,
            (Initialized, Ready) => true,
            // Operational transitions from Ready
            (Ready, Degraded) => true,
            (Ready, Disabled) => true,
            // Recovery from Degraded
            (Degraded, Ready) => true,
            (Degraded, Disabled) => true,
            // Disabled is terminal
            _ => false,
        }
    }
    
    /// Transition to a new state, returning an error if the transition is invalid.
    pub fn transition(&mut self, target: Self) -> Result<(), &'static str> {
        if self.can_transition_to(target) {
            *self = target;
            Ok(())
        } else {
            Err("invalid module state transition")
        }
    }
}

impl serde::Serialize for ModuleState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(match self {
            ModuleState::Discovered => "Discovered",
            ModuleState::Validated => "Validated",
            ModuleState::Registered => "Registered",
            ModuleState::Initialized => "Initialized",
            ModuleState::Ready => "Ready",
            ModuleState::Degraded => "Degraded",
            ModuleState::Disabled => "Disabled",
        })
    }
}

impl<'de> serde::Deserialize<'de> for ModuleState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ModuleStateVisitor;
        
        impl<'de> serde::de::Visitor<'de> for ModuleStateVisitor {
            type Value = ModuleState;
            
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a module lifecycle state")
            }
            
            fn visit_str<E>(self, value: &str) -> Result<ModuleState, E>
            where
                E: serde::de::Error,
            {
                match value {
                    "Discovered" => Ok(ModuleState::Discovered),
                    "Validated" => Ok(ModuleState::Validated),
                    "Registered" => Ok(ModuleState::Registered),
                    "Initialized" => Ok(ModuleState::Initialized),
                    "Ready" => Ok(ModuleState::Ready),
                    "Degraded" => Ok(ModuleState::Degraded),
                    "Disabled" => Ok(ModuleState::Disabled),
                    _ => Err(serde::de::Error::unknown_variant(value, &[
                        "Discovered", "Validated", "Registered", "Initialized", 
                        "Ready", "Degraded", "Disabled"
                    ])),
                }
            }
            
            fn visit_u64<E>(self, value: u64) -> Result<ModuleState, E>
            where
                E: serde::de::Error,
            {
                match value {
                    0 => Ok(ModuleState::Discovered),
                    1 => Ok(ModuleState::Validated),
                    2 => Ok(ModuleState::Registered),
                    3 => Ok(ModuleState::Initialized),
                    4 => Ok(ModuleState::Ready),
                    5 => Ok(ModuleState::Degraded),
                    6 => Ok(ModuleState::Disabled),
                    _ => Err(serde::de::Error::invalid_value(serde::de::Unexpected::Unsigned(value), &"0-6")),
                }
            }
        }
        
        deserializer.deserialize_any(ModuleStateVisitor)
    }
}

/// Runtime module state tracking (docs/15-EXTENSION §Modules).
/// Tracks each module through the lifecycle: Discovered → Validated →
/// Registered → Initialized → Ready, then Degraded/Disabled at runtime.
/// ApplicationHost calls into this during boot and register_module;
/// operators or health checks can degrade/recover/disable at runtime.
#[derive(Debug, Clone, Default)]
pub struct ModuleRegistry {
    states: std::collections::HashMap<String, ModuleState>,
}

impl ModuleRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a module at Discovered. Overwrites any prior entry.
    pub fn discover(&mut self, name: &str) {
        self.states.insert(name.to_string(), ModuleState::Discovered);
    }

    /// Advance a module one step along the boot path.
    /// Returns Ok on success, Err with a description on an invalid transition.
    pub fn advance(&mut self, name: &str, to: ModuleState) -> Result<(), String> {
        let from = self
            .states
            .get(name)
            .copied()
            .unwrap_or(ModuleState::Discovered);
        if from.can_transition_to(to) {
            self.states.insert(name.to_string(), to);
            Ok(())
        } else {
            Err(format!(
                "module {name}: invalid transition {from:?} -> {to:?}"
            ))
        }
    }

    /// Transition Ready → Degraded. Only valid from Ready.
    pub fn degrade(&mut self, name: &str) -> Result<(), String> {
        let from = self
            .states
            .get(name)
            .copied()
            .unwrap_or(ModuleState::Discovered);
        match from {
            ModuleState::Ready => {
                self.states.insert(name.to_string(), ModuleState::Degraded);
                Ok(())
            }
            _ => Err(format!(
                "module {name}: cannot degrade from {from:?}"
            )),
        }
    }

    /// Transition Degraded → Ready (recovery). Only valid from Degraded.
    pub fn recover(&mut self, name: &str) -> Result<(), String> {
        let from = self
            .states
            .get(name)
            .copied()
            .unwrap_or(ModuleState::Discovered);
        match from {
            ModuleState::Degraded => {
                self.states.insert(name.to_string(), ModuleState::Ready);
                Ok(())
            }
            _ => Err(format!(
                "module {name}: cannot recover from {from:?}"
            )),
        }
    }

    /// Transition to Disabled (terminal). Valid from Ready or Degraded.
    pub fn disable(&mut self, name: &str) -> Result<(), String> {
        let from = self
            .states
            .get(name)
            .copied()
            .unwrap_or(ModuleState::Discovered);
        match from {
            ModuleState::Ready | ModuleState::Degraded => {
                self.states.insert(name.to_string(), ModuleState::Disabled);
                Ok(())
            }
            _ => Err(format!(
                "module {name}: cannot disable from {from:?}"
            )),
        }
    }

    /// Look up a module's current state.
    pub fn state(&self, name: &str) -> Option<ModuleState> {
        self.states.get(name).copied()
    }

    /// Whether a module is currently in a serving state (Ready or Degraded).
    pub fn is_serving(&self, name: &str) -> bool {
        matches!(
            self.states.get(name),
            Some(ModuleState::Ready) | Some(ModuleState::Degraded)
        )
    }

    /// Snapshot of all tracked modules (name → state).
    pub fn snapshot(&self) -> std::collections::HashMap<String, ModuleState> {
        self.states.clone()
    }
}

/// Service lifetimes understood by the container (STAGE-0-SPEC §6).
/// Recorded per registration for diagnostics and future scoped containers;
/// constructor injection everywhere — no reflection magic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ServiceLifetime {
    /// Pools, caches, clocks — outlive any one application boot.
    Process,
    /// Config, registry, buses — tied to the ApplicationHost (default).
    #[default]
    Application,
    /// Per HTTP request (S3 REQUEST scope).
    Request,
    /// Per execution (ExecutionContext carriers).
    Execution,
    /// Per tenant session (S3 TENANT scope).
    Tenant,
    /// Per transaction scope.
    Transaction,
}

#[derive(Debug, Clone)]
pub enum BootError {
    MissingService(String),
    DuplicateService(String),
    DuplicateProvider(String),
    UnknownDependency {
        provider: String,
        depends_on: String,
    },
    DependencyCycle(Vec<String>),
    ProviderFailed {
        provider: String,
        source: String,
    },
    Frozen,
    NotFrozen,
    NotBooted,
}

impl std::fmt::Display for BootError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BootError::MissingService(name) => write!(f, "boot: required service missing: {name}"),
            BootError::DuplicateService(name) => {
                write!(f, "boot: service registered twice: {name}")
            }
            BootError::DuplicateProvider(name) => {
                write!(f, "boot: provider name registered twice: {name}")
            }
            BootError::UnknownDependency {
                provider,
                depends_on,
            } => {
                write!(
                    f,
                    "boot: provider {provider} depends on unknown provider {depends_on}"
                )
            }
            BootError::DependencyCycle(names) => {
                write!(f, "boot: provider dependency cycle: {}", names.join(" -> "))
            }
            BootError::ProviderFailed { provider, source } => {
                write!(f, "boot: provider {provider} failed: {source}")
            }
            BootError::Frozen => write!(
                f,
                "boot: registry is frozen (post-Ready mutation attempted)"
            ),
            BootError::NotFrozen => write!(f, "boot: operation requires a frozen registry"),
            BootError::NotBooted => write!(f, "boot: host not yet booted"),
        }
    }
}

impl std::error::Error for BootError {}

impl From<BootError> for AppError {
    fn from(e: BootError) -> Self {
        AppError::new(
            kelson_foundation::codes::BOOT,
            ErrorCategory::Internal,
            Severity::Critical,
            Retryability::NotRetryable,
            e.to_string(),
        )
    }
}

/// Typed service map. One instance per concrete type; names are diagnostics.
/// Each registration records its `ServiceLifetime` (S1).
#[derive(Default)]
pub struct ServiceRegistry {
    services: HashMap<TypeId, (String, ServiceLifetime, Arc<dyn Any + Send + Sync>)>,
    frozen: bool,
}

impl ServiceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register with the default (Application) lifetime.
    pub fn insert<T: Send + Sync + 'static>(
        &mut self,
        name: impl Into<String>,
        service: Arc<T>,
    ) -> Result<(), BootError> {
        self.insert_with_lifetime(name, ServiceLifetime::Application, service)
    }

    pub fn insert_with_lifetime<T: Send + Sync + 'static>(
        &mut self,
        name: impl Into<String>,
        lifetime: ServiceLifetime,
        service: Arc<T>,
    ) -> Result<(), BootError> {
        if self.frozen {
            return Err(BootError::Frozen);
        }
        let name = name.into();
        if self
            .services
            .insert(TypeId::of::<T>(), (name.clone(), lifetime, service))
            .is_some()
        {
            return Err(BootError::DuplicateService(name));
        }
        Ok(())
    }

    pub fn get<T: Send + Sync + 'static>(&self) -> Result<Arc<T>, BootError> {
        let (name, _, any) = self
            .services
            .get(&TypeId::of::<T>())
            .ok_or_else(|| BootError::MissingService(std::any::type_name::<T>().to_string()))?;
        any.clone()
            .downcast::<T>()
            .map_err(|_| BootError::ProviderFailed {
                provider: name.clone(),
                source: "service downcast failed".into(),
            })
    }

    /// Recorded lifetime of a registered service (S1 diagnostics/scoping).
    pub fn lifetime_of<T: Send + Sync + 'static>(&self) -> Result<ServiceLifetime, BootError> {
        self.services
            .get(&TypeId::of::<T>())
            .map(|(_, lifetime, _)| *lifetime)
            .ok_or_else(|| BootError::MissingService(std::any::type_name::<T>().to_string()))
    }

    pub fn contains<T: Send + Sync + 'static>(&self) -> bool {
        self.services.contains_key(&TypeId::of::<T>())
    }

    pub fn freeze(&mut self) {
        self.frozen = true;
    }

    pub fn is_frozen(&self) -> bool {
        self.frozen
    }

    pub fn service_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.services.values().map(|(n, _, _)| n.clone()).collect();
        names.sort();
        names
    }
}

/// A lifecycle unit: declares its phase + dependencies, registers services
/// (init), then starts (boot). Dependencies are provider **names** and are
/// resolved by the host into a boot order (S1).
pub trait Provider: Send + Sync {
    fn name(&self) -> &str;
    fn phase(&self) -> Phase;
    /// Provider names this one needs booted first (topo-sorted, S1).
    fn dependencies(&self) -> Vec<&'static str> {
        vec![]
    }
    /// Synchronous registration. Must not perform I/O.
    fn init(&self, registry: &mut ServiceRegistry) -> Result<(), BootError>;
    /// Async startup (connections, warm-up). Default: nothing.
    fn boot<'a>(
        &'a self,
        _registry: &'a ServiceRegistry,
    ) -> kelson_execution_style_future::BoxFuture<'a, Result<(), BootError>> {
        Box::pin(async move { Ok(()) })
    }
}

/// Local alias so the trait does not depend on kelson-execution (CORE independence).
mod kelson_execution_style_future {
    pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;
}

/// Kahn topological sort over declared provider dependencies. Deterministic:
/// among ready providers, lowest (phase, name) goes first.
fn topo_order(providers: &[Box<dyn Provider>]) -> Result<Vec<usize>, BootError> {
    let mut by_name: HashMap<&str, usize> = HashMap::new();
    for (i, p) in providers.iter().enumerate() {
        if by_name.insert(p.name(), i).is_some() {
            return Err(BootError::DuplicateProvider(p.name().to_string()));
        }
    }
    let n = providers.len();
    let mut indegree = vec![0usize; n];
    let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, p) in providers.iter().enumerate() {
        for dep in p.dependencies() {
            let Some(&j) = by_name.get(dep) else {
                return Err(BootError::UnknownDependency {
                    provider: p.name().to_string(),
                    depends_on: dep.to_string(),
                });
            };
            dependents[j].push(i);
            indegree[i] += 1;
        }
    }
    let mut done = vec![false; n];
    let mut order = Vec::with_capacity(n);
    loop {
        let ready: Vec<usize> = (0..n).filter(|&i| !done[i] && indegree[i] == 0).collect();
        if ready.is_empty() {
            if order.len() == n {
                return Ok(order);
            }
            let cycle: Vec<String> = (0..n)
                .filter(|&i| !done[i])
                .map(|i| providers[i].name().to_string())
                .collect();
            return Err(BootError::DependencyCycle(cycle));
        }
        let next = ready
            .iter()
            .min_by_key(|&&i| (providers[i].phase(), providers[i].name().to_string()))
            .copied()
            .unwrap();
        done[next] = true;
        order.push(next);
        for &m in &dependents[next] {
            indegree[m] -= 1;
        }
    }
}

pub struct ApplicationHost {
    providers: Vec<Box<dyn Provider>>,
    registry: ServiceRegistry,
    modules: ModuleRegistry,
    booted: bool,
    /// Wave 7 P3 (docs/19 L340): the injectable clock on the host. Default =
    /// wall clock; tests inject a `FixedClock` so scheduler `next_run` and
    /// every host-captured instant read the SAME frozen time.
    clock: Arc<dyn kelson_foundation::Clock>,
}

impl ApplicationHost {
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            registry: ServiceRegistry::new(),
            modules: ModuleRegistry::new(),
            booted: false,
            clock: Arc::new(kelson_foundation::SystemClock),
        }
    }

    /// Wave 7 P3: inject the time source (tests freeze it, then `travel`).
    pub fn with_clock(mut self, clock: Arc<dyn kelson_foundation::Clock>) -> Self {
        self.clock = clock;
        self
    }

    /// The host's injected clock — spawn sites read `clock.now()` instead of
    /// `OffsetDateTime::now_utc()`.
    pub fn clock(&self) -> &Arc<dyn kelson_foundation::Clock> {
        &self.clock
    }

    pub fn add_provider(&mut self, provider: Box<dyn Provider>) {
        self.providers.push(provider);
    }

    /// register → dependency topo-sort (cycle = boot refusal) → init + boot in
    /// topo order → FREEZE (STAGE-0-SPEC §6).
    pub async fn boot(&mut self) -> Result<(), BootError> {
        // Deterministic registration order: phase, then name (stable
        // diagnostics). The typed provider graph (S1) then orders BOTH the
        // synchronous init and the async boot: every provider registers its
        // services strictly after its declared dependencies. Init used to
        // run in plain (phase, name) order, which let a bus-assembling
        // provider run BEFORE the providers that insert handlers into it
        // (Wave 9 B-30 fixed the latent ordering bug for the oauth2/wasm
        // handler bundles). With no dependencies the two orders coincide.
        self.providers
            .sort_by_key(|p| (p.phase(), p.name().to_string()));
        let order = topo_order(&self.providers)?;
        for &i in &order {
            let name = self.providers[i].name().to_string();
            // Track the module through the boot path (docs/15-EXTENSION).
            self.modules.discover(&name);
            let _ = self.modules.advance(&name, ModuleState::Validated);
            self.providers[i]
                .init(&mut self.registry)
                .map_err(|e| BootError::ProviderFailed {
                    provider: self.providers[i].name().to_string(),
                    source: e.to_string(),
                })?;
            let _ = self.modules.advance(&name, ModuleState::Registered);
        }
        for &i in &order {
            let name = self.providers[i].name().to_string();
            let _ = self.modules.advance(&name, ModuleState::Initialized);
            self.providers[i].boot(&self.registry).await.map_err(|e| {
                BootError::ProviderFailed {
                    provider: self.providers[i].name().to_string(),
                    source: e.to_string(),
                }
            })?;
            let _ = self.modules.advance(&name, ModuleState::Ready);
        }
        self.registry.freeze();
        self.booted = true;
        Ok(())
    }

    /// Dynamically register a module provider after boot (hot reload).
    /// Initializes the provider, boots it, adds it to the provider list, and returns.
    /// Note: Full graph recompilation and route cache regeneration
    /// are handled by the RouteReloader when the route cache artifact
    /// is updated via `kelson route:cache`.
    pub async fn register_module(&mut self, provider: Box<dyn Provider>) -> Result<(), BootError> {
        if !self.booted {
            return Err(BootError::NotBooted);
        }

        let name = provider.name().to_string();
        // Track the module through the boot path (docs/15-EXTENSION).
        self.modules.discover(&name);
        let _ = self.modules.advance(&name, ModuleState::Validated);
        provider.init(&mut self.registry).map_err(|e| BootError::ProviderFailed {
            provider: name.clone(),
            source: e.to_string(),
        })?;
        let _ = self.modules.advance(&name, ModuleState::Registered);
        let _ = self.modules.advance(&name, ModuleState::Initialized);
        provider.boot(&self.registry).await.map_err(|e| BootError::ProviderFailed {
            provider: name.clone(),
            source: e.to_string(),
        })?;
        let _ = self.modules.advance(&name, ModuleState::Ready);

        // Add to providers list
        self.providers.push(provider);

        Ok(())
    }

    pub fn registry(&self) -> &ServiceRegistry {
        &self.registry
    }

    /// Access the runtime module state registry.
    pub fn modules(&self) -> &ModuleRegistry {
        &self.modules
    }

    /// Mutable access to the runtime module state registry (degrade/recover/disable).
    pub fn modules_mut(&mut self) -> &mut ModuleRegistry {
        &mut self.modules
    }

    pub fn is_booted(&self) -> bool {
        self.booted
    }
}

impl Default for ApplicationHost {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct P1;
    impl Provider for P1 {
        fn name(&self) -> &str {
            "p1"
        }
        fn phase(&self) -> Phase {
            Phase::Kernel
        }
        fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
            r.insert("svc-u64", Arc::new(42u64))
        }
    }

    struct P0;
    impl Provider for P0 {
        fn name(&self) -> &str {
            "p0"
        }
        fn phase(&self) -> Phase {
            Phase::Foundation
        }
        fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
            r.insert_with_lifetime(
                "svc-string",
                ServiceLifetime::Process,
                Arc::new(String::from("foundation")),
            )
        }
    }

    /// Wave 7 P3 (docs/19 L340): the host clock is injectable — the default
    /// tracks wall time, an injected `FixedClock` is what spawn sites read.
    #[test]
    fn host_clock_is_injectable() {
        let wall = ApplicationHost::new();
        let seen = wall.clock().now();
        assert!(seen.unix_timestamp() > 0, "default host clock reads wall time");

        let frozen = ApplicationHost::new()
            .with_clock(Arc::new(kelson_foundation::FixedClock::new(seen)));
        assert_eq!(
            frozen.clock().now(),
            seen,
            "injected clock wins over the wall clock"
        );
    }

    #[tokio::test]
    async fn providers_boot_in_phase_order_and_freeze() {
        let mut host = ApplicationHost::new();
        host.add_provider(Box::new(P1));
        host.add_provider(Box::new(P0));
        host.boot().await.unwrap();
        assert!(host.registry().is_frozen());
        assert_eq!(host.registry().get::<u64>().unwrap().as_ref(), &42);
        assert_eq!(
            host.registry().get::<String>().unwrap().as_str(),
            "foundation"
        );
        assert_eq!(
            host.registry().lifetime_of::<String>().unwrap(),
            ServiceLifetime::Process
        );
        assert_eq!(
            host.registry().lifetime_of::<u64>().unwrap(),
            ServiceLifetime::Application
        );
    }

    #[tokio::test]
    async fn frozen_registry_rejects_inserts() {
        let mut host = ApplicationHost::new();
        host.add_provider(Box::new(P0));
        host.boot().await.unwrap();
        let err = host.registry().get::<u64>().unwrap_err();
        assert!(matches!(err, BootError::MissingService(_)));
    }

    #[tokio::test]
    async fn duplicate_service_is_boot_error() {
        struct DupA;
        impl Provider for DupA {
            fn name(&self) -> &str {
                "dup-a"
            }
            fn phase(&self) -> Phase {
                Phase::Platform
            }
            fn init(&self, r: &mut ServiceRegistry) -> Result<(), BootError> {
                r.insert("x", Arc::new(1u64))?;
                r.insert("x2", Arc::new(2u64))
            }
        }
        let mut host = ApplicationHost::new();
        host.add_provider(Box::new(DupA));
        let err = host.boot().await.unwrap_err();
        assert!(matches!(err, BootError::ProviderFailed { .. }));
    }

    // --- S1: typed provider graph ---

    static BOOT_LOG: Mutex<Vec<String>> = Mutex::new(Vec::new());

    struct A;
    impl Provider for A {
        fn name(&self) -> &str {
            "a"
        }
        fn phase(&self) -> Phase {
            Phase::Platform
        }
        fn init(&self, _: &mut ServiceRegistry) -> Result<(), BootError> {
            Ok(())
        }
        fn boot<'s>(
            &'s self,
            _: &'s ServiceRegistry,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), BootError>> + Send + 's>> {
            Box::pin(async move {
                BOOT_LOG.lock().unwrap().push("a".into());
                Ok(())
            })
        }
    }

    /// Declared in an EARLIER phase but depends on A → must boot AFTER A.
    struct B;
    impl Provider for B {
        fn name(&self) -> &str {
            "b"
        }
        fn phase(&self) -> Phase {
            Phase::Kernel
        }
        fn dependencies(&self) -> Vec<&'static str> {
            vec!["a"]
        }
        fn init(&self, _: &mut ServiceRegistry) -> Result<(), BootError> {
            Ok(())
        }
        fn boot<'s>(
            &'s self,
            _: &'s ServiceRegistry,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), BootError>> + Send + 's>> {
            Box::pin(async move {
                BOOT_LOG.lock().unwrap().push("b".into());
                Ok(())
            })
        }
    }

    #[tokio::test]
    async fn dependencies_override_phase_order() {
        BOOT_LOG.lock().unwrap().clear();
        let mut host = ApplicationHost::new();
        host.add_provider(Box::new(B));
        host.add_provider(Box::new(A));
        host.boot().await.unwrap();
        assert_eq!(
            *BOOT_LOG.lock().unwrap(),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[tokio::test]
    async fn cycle_is_boot_refusal() {
        struct X;
        impl Provider for X {
            fn name(&self) -> &str {
                "x"
            }
            fn phase(&self) -> Phase {
                Phase::Platform
            }
            fn dependencies(&self) -> Vec<&'static str> {
                vec!["y"]
            }
            fn init(&self, _: &mut ServiceRegistry) -> Result<(), BootError> {
                Ok(())
            }
        }
        struct Y;
        impl Provider for Y {
            fn name(&self) -> &str {
                "y"
            }
            fn phase(&self) -> Phase {
                Phase::Platform
            }
            fn dependencies(&self) -> Vec<&'static str> {
                vec!["x"]
            }
            fn init(&self, _: &mut ServiceRegistry) -> Result<(), BootError> {
                Ok(())
            }
        }
        let mut host = ApplicationHost::new();
        host.add_provider(Box::new(X));
        host.add_provider(Box::new(Y));
        let err = host.boot().await.unwrap_err();
        assert!(matches!(err, BootError::DependencyCycle(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn unknown_dependency_is_boot_refusal() {
        struct Z;
        impl Provider for Z {
            fn name(&self) -> &str {
                "z"
            }
            fn phase(&self) -> Phase {
                Phase::Platform
            }
            fn dependencies(&self) -> Vec<&'static str> {
                vec!["ghost"]
            }
            fn init(&self, _: &mut ServiceRegistry) -> Result<(), BootError> {
                Ok(())
            }
        }
        let mut host = ApplicationHost::new();
        host.add_provider(Box::new(Z));
        let err = host.boot().await.unwrap_err();
        assert!(
            matches!(err, BootError::UnknownDependency { .. }),
            "got {err:?}"
        );
    }

    // --- ModuleState lifecycle ---

    #[test]
    fn module_state_normal_boot_progression() {
        let mut s = ModuleState::Discovered;
        assert!(s.transition(ModuleState::Validated).is_ok());
        assert_eq!(s, ModuleState::Validated);
        assert!(s.transition(ModuleState::Registered).is_ok());
        assert_eq!(s, ModuleState::Registered);
        assert!(s.transition(ModuleState::Initialized).is_ok());
        assert_eq!(s, ModuleState::Initialized);
        assert!(s.transition(ModuleState::Ready).is_ok());
        assert_eq!(s, ModuleState::Ready);
    }

    #[test]
    fn module_state_ready_can_go_degraded_or_disable() {
        let mut s = ModuleState::Ready;
        assert!(s.transition(ModuleState::Degraded).is_ok());
        assert_eq!(s, ModuleState::Degraded);
        
        // Recovery from Degraded
        assert!(s.transition(ModuleState::Ready).is_ok());
        assert_eq!(s, ModuleState::Ready);
        
        // Then disable
        assert!(s.transition(ModuleState::Disabled).is_ok());
        assert_eq!(s, ModuleState::Disabled);
    }

    #[test]
    fn module_state_degraded_can_recover_or_disable() {
        let mut s = ModuleState::Degraded;
        assert!(s.transition(ModuleState::Ready).is_ok());
        assert_eq!(s, ModuleState::Ready);
        
        assert!(s.transition(ModuleState::Degraded).is_ok());
        assert_eq!(s, ModuleState::Degraded);
        
        assert!(s.transition(ModuleState::Disabled).is_ok());
        assert_eq!(s, ModuleState::Disabled);
    }

    #[test]
    fn module_state_disabled_is_terminal() {
        let mut s = ModuleState::Disabled;
        assert!(s.transition(ModuleState::Ready).is_err());
        assert!(s.transition(ModuleState::Degraded).is_err());
        assert!(s.transition(ModuleState::Discovered).is_err());
        assert_eq!(s, ModuleState::Disabled);
    }

    #[test]
    fn module_state_invalid_transitions_rejected() {
        let mut s = ModuleState::Discovered;
        // Can't skip states
        assert!(s.transition(ModuleState::Registered).is_err());
        assert!(s.transition(ModuleState::Ready).is_err());
        
        s = ModuleState::Validated;
        assert!(s.transition(ModuleState::Initialized).is_err());
        assert!(s.transition(ModuleState::Disabled).is_err());
        
        s = ModuleState::Ready;
        assert!(s.transition(ModuleState::Validated).is_err());
    }

    #[test]
    fn module_state_is_serving() {
        assert!(!ModuleState::Discovered.is_serving());
        assert!(!ModuleState::Validated.is_serving());
        assert!(!ModuleState::Registered.is_serving());
        assert!(!ModuleState::Initialized.is_serving());
        assert!(ModuleState::Ready.is_serving());
        assert!(ModuleState::Degraded.is_serving());
        assert!(!ModuleState::Disabled.is_serving());
    }

    #[test]
    fn module_state_serde_roundtrip() {
        use serde_json;
        
        for state in [
            ModuleState::Discovered,
            ModuleState::Validated,
            ModuleState::Registered,
            ModuleState::Initialized,
            ModuleState::Ready,
            ModuleState::Degraded,
            ModuleState::Disabled,
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let deserialized: ModuleState = serde_json::from_str(&json).unwrap();
            assert_eq!(state, deserialized);
        }
    }

    #[test]
    fn module_registry_discover_starts_discovered() {
        let mut reg = ModuleRegistry::new();
        reg.discover("orders");
        assert_eq!(reg.state("orders"), Some(ModuleState::Discovered));
        assert!(!reg.is_serving("orders"));
    }

    #[test]
    fn module_registry_boot_path_reaches_ready() {
        let mut reg = ModuleRegistry::new();
        reg.discover("payments");
        reg.advance("payments", ModuleState::Validated).unwrap();
        reg.advance("payments", ModuleState::Registered).unwrap();
        reg.advance("payments", ModuleState::Initialized).unwrap();
        reg.advance("payments", ModuleState::Ready).unwrap();
        assert_eq!(reg.state("payments"), Some(ModuleState::Ready));
        assert!(reg.is_serving("payments"));
    }

    #[test]
    fn module_registry_degrade_and_recover() {
        let mut reg = ModuleRegistry::new();
        reg.discover("notifications");
        reg.advance("notifications", ModuleState::Validated).unwrap();
        reg.advance("notifications", ModuleState::Registered).unwrap();
        reg.advance("notifications", ModuleState::Initialized).unwrap();
        reg.advance("notifications", ModuleState::Ready).unwrap();

        reg.degrade("notifications").unwrap();
        assert_eq!(reg.state("notifications"), Some(ModuleState::Degraded));
        // Degraded still counts as serving (docs/15-EXTENSION).
        assert!(reg.is_serving("notifications"));

        reg.recover("notifications").unwrap();
        assert_eq!(reg.state("notifications"), Some(ModuleState::Ready));
    }

    #[test]
    fn module_registry_disable_is_terminal() {
        let mut reg = ModuleRegistry::new();
        reg.discover("legacy");
        reg.advance("legacy", ModuleState::Validated).unwrap();
        reg.advance("legacy", ModuleState::Registered).unwrap();
        reg.advance("legacy", ModuleState::Initialized).unwrap();
        reg.advance("legacy", ModuleState::Ready).unwrap();

        reg.disable("legacy").unwrap();
        assert_eq!(reg.state("legacy"), Some(ModuleState::Disabled));
        assert!(!reg.is_serving("legacy"));

        // Disabled is terminal: no recovery.
        assert!(reg.recover("legacy").is_err());
        assert!(reg.degrade("legacy").is_err());
    }

    #[test]
    fn module_registry_rejects_invalid_transition() {
        let mut reg = ModuleRegistry::new();
        reg.discover("broken");
        // Skips Validated → Registered: should fail.
        assert!(reg.advance("broken", ModuleState::Registered).is_err());
    }

    #[test]
    fn module_registry_snapshot_copies_all_states() {
        let mut reg = ModuleRegistry::new();
        reg.discover("a");
        reg.discover("b");
        reg.advance("a", ModuleState::Validated).unwrap();
        let snap = reg.snapshot();
        assert_eq!(snap.len(), 2);
        assert_eq!(snap.get("a"), Some(&ModuleState::Validated));
        assert_eq!(snap.get("b"), Some(&ModuleState::Discovered));
    }

    #[tokio::test]
    async fn host_boot_tracks_module_states() {
        let mut host = ApplicationHost::new();
        host.add_provider(Box::new(P1));
        host.add_provider(Box::new(P0));
        host.boot().await.unwrap();

        assert_eq!(host.modules().state("p0"), Some(ModuleState::Ready));
        assert_eq!(host.modules().state("p1"), Some(ModuleState::Ready));
        assert!(host.modules().is_serving("p0"));
        assert!(host.modules().is_serving("p1"));
    }
}