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
            self.providers[i]
                .init(&mut self.registry)
                .map_err(|e| BootError::ProviderFailed {
                    provider: self.providers[i].name().to_string(),
                    source: e.to_string(),
                })?;
        }
        for &i in &order {
            self.providers[i].boot(&self.registry).await.map_err(|e| {
                BootError::ProviderFailed {
                    provider: self.providers[i].name().to_string(),
                    source: e.to_string(),
                }
            })?;
        }
        self.registry.freeze();
        self.booted = true;
        Ok(())
    }

    pub fn registry(&self) -> &ServiceRegistry {
        &self.registry
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
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), BootError>> + Send + 's>>
        {
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
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), BootError>> + Send + 's>>
        {
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
}
