//! # keel-feature-flags
//! Feature-flag service (blueprint L68, STAGE-0-SPEC §11, S3). Dimensions:
//! global · environment · tenant · region · percentage · capability · actor.
//! Rollout ladder: disabled → internal → 1% → 10% → 50% → 100%, one step at a
//! time in BOTH directions (advance and rollback are the same operation
//! inverted — no special-case rollback path to get wrong).
//!
//! Evaluation is deterministic and pure: `evaluate(def, input)` never
//! touches clocks or IO, so flag decisions are replayable and testable. A
//! request resolves its `FeatureSnapshot` ONCE (keel-context) and carries it
//! through the whole execution — no mid-request flips.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, RwLock};

use keel_context::FeatureSnapshot;
use keel_foundation::{AppError, FeatureRevision, PrincipalId, TenantId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Rollout ladder positions (§11). Ordered; `advance`/`rollback` move one step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum RolloutStage {
    #[default]
    Disabled,
    /// Enabled only for identities marked internal (dogfooding).
    Internal,
    Percent1,
    Percent10,
    Percent50,
    Percent100,
}

impl RolloutStage {
    /// Percentage of actors enabled at this stage (Internal = 0% via stage).
    pub fn percentage(&self) -> u32 {
        match self {
            RolloutStage::Disabled => 0,
            RolloutStage::Internal => 0,
            RolloutStage::Percent1 => 1,
            RolloutStage::Percent10 => 10,
            RolloutStage::Percent50 => 50,
            RolloutStage::Percent100 => 100,
        }
    }

    pub fn advance(self) -> Self {
        match self {
            RolloutStage::Disabled => RolloutStage::Internal,
            RolloutStage::Internal => RolloutStage::Percent1,
            RolloutStage::Percent1 => RolloutStage::Percent10,
            RolloutStage::Percent10 => RolloutStage::Percent50,
            RolloutStage::Percent50 => RolloutStage::Percent100,
            RolloutStage::Percent100 => RolloutStage::Percent100,
        }
    }

    pub fn rollback(self) -> Self {
        match self {
            RolloutStage::Disabled => RolloutStage::Disabled,
            RolloutStage::Internal => RolloutStage::Disabled,
            RolloutStage::Percent1 => RolloutStage::Internal,
            RolloutStage::Percent10 => RolloutStage::Percent1,
            RolloutStage::Percent50 => RolloutStage::Percent10,
            RolloutStage::Percent100 => RolloutStage::Percent50,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RolloutStage::Disabled => "disabled",
            RolloutStage::Internal => "internal",
            RolloutStage::Percent1 => "1%",
            RolloutStage::Percent10 => "10%",
            RolloutStage::Percent50 => "50%",
            RolloutStage::Percent100 => "100%",
        }
    }

    /// Parse `disabled|internal|1%|10%|50%|100%` (percentage forms also accept
    /// bare numbers: `1`, `10`, `50`, `100`).
    pub fn parse(raw: &str) -> Result<Self, AppError> {
        let t = raw.trim().trim_end_matches('%').trim();
        let stage = match t {
            "disabled" | "0" | "off" => RolloutStage::Disabled,
            "internal" => RolloutStage::Internal,
            "1" => RolloutStage::Percent1,
            "10" => RolloutStage::Percent10,
            "50" => RolloutStage::Percent50,
            "100" | "on" | "full" => RolloutStage::Percent100,
            _ => {
                return Err(AppError::validation(format!(
                    "unknown rollout stage {raw:?}"
                )))
            }
        };
        Ok(stage)
    }
}

/// One targeting dimension (§11). A rule matches when its dimension matches
/// the evaluation input; `enable` decides the verdict for that match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "dimension", rename_all = "snake_case")]
pub enum FlagRule {
    /// Whole environment (dev/test/staging/prod).
    Environment { name: String, enable: bool },
    /// One tenant (allow/deny lists).
    Tenant { id: TenantId, enable: bool },
    /// Deployment region (`in`, `eu`, ...).
    Region { name: String, enable: bool },
    /// Named capability granted to the tenant (keel-tenancy capabilities).
    Capability { name: String, enable: bool },
    /// Explicit actor allow/deny (support staff, beta users).
    Actor { id: PrincipalId, enable: bool },
}

/// Static flag definition registered at boot (code or config).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlagDefinition {
    /// Stable machine name, `[a-z][a-z0-9._-]*`.
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Ladder position is runtime state (FlagStore), not definition state;
    /// this is the stage used when the store has no entry yet.
    #[serde(default)]
    pub default_stage: RolloutStage,
    /// Targeting rules evaluated BEFORE the stage percentage.
    #[serde(default)]
    pub rules: Vec<FlagRule>,
}

/// Runtime state of one flag (persisted by the store).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlagState {
    pub name: String,
    pub stage: RolloutStage,
    /// Per-tenant stage overrides (rollout to one tenant ahead of the ladder).
    #[serde(default)]
    pub tenant_stages: Vec<(TenantId, RolloutStage)>,
}

/// Evaluation input — everything a deterministic decision needs.
#[derive(Debug, Clone)]
pub struct FlagInput<'a> {
    pub environment: &'a str,
    pub tenant: Option<TenantId>,
    pub region: Option<&'a str>,
    /// Capabilities granted to the tenant for this execution.
    pub capabilities: &'a BTreeSet<String>,
    /// Actor bucket key; percentage rollouts are stable per (flag, actor).
    pub actor: PrincipalId,
    /// True for platform-internal identities (stage `internal`).
    pub internal_actor: bool,
}

/// Deterministic 0..100 bucket for (flag, actor): sha256(name|actor) mod 100.
/// Stable across restarts and processes — the rollout population never
/// reshuffles when the flag set changes.
pub fn rollout_bucket(flag: &str, actor: PrincipalId) -> u32 {
    let mut hasher = Sha256::new();
    hasher.update(flag.as_bytes());
    hasher.update(b"|");
    hasher.update(actor.as_uuid().as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&digest[..8]);
    (u64::from_be_bytes(bytes) % 100) as u32
}

/// Pure evaluation (§11):
/// 1. dimension rules win in order (first match decides),
/// 2. tenant stage override, else global stage,
/// 3. `internal` stage requires an internal actor,
/// 4. percentage stages use the stable (flag, actor) bucket.
pub fn evaluate(def: &FlagDefinition, state: Option<&FlagState>, input: &FlagInput<'_>) -> bool {
    for rule in &def.rules {
        match rule {
            FlagRule::Environment { name, enable } if name == input.environment => return *enable,
            FlagRule::Tenant { id, enable } if input.tenant == Some(*id) => return *enable,
            FlagRule::Region { name, enable } if input.region == Some(name.as_str()) => {
                return *enable
            }
            FlagRule::Capability { name, enable } if input.capabilities.contains(name) => {
                return *enable
            }
            FlagRule::Actor { id, enable } if *id == input.actor => return *enable,
            _ => {}
        }
    }
    let stage = state
        .and_then(|s| {
            input.tenant.and_then(|t| {
                s.tenant_stages
                    .iter()
                    .find(|(tid, _)| *tid == t)
                    .map(|(_, st)| *st)
            })
        })
        .or_else(|| state.map(|s| s.stage))
        .unwrap_or(def.default_stage);
    match stage {
        RolloutStage::Disabled => false,
        RolloutStage::Internal => input.internal_actor,
        RolloutStage::Percent100 => true,
        pct => rollout_bucket(&def.name, input.actor) < pct.percentage(),
    }
}

// ------------------------------------------------------------------ registry

/// Boot-time catalog of known flags. Unknown flags cannot be evaluated,
/// set, or listed — the catalog is the single source of flag names.
#[derive(Debug, Default)]
pub struct FlagRegistry {
    defs: Vec<FlagDefinition>,
}

impl FlagRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, def: FlagDefinition) -> Result<(), AppError> {
        if self.defs.iter().any(|d| d.name == def.name) {
            return Err(AppError::validation(format!(
                "flag registered twice: {}",
                def.name
            )));
        }
        validate_name(&def.name)?;
        self.defs.push(def);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&FlagDefinition> {
        self.defs.iter().find(|d| d.name == name)
    }

    pub fn definitions(&self) -> &[FlagDefinition] {
        &self.defs
    }
}

fn validate_name(name: &str) -> Result<(), AppError> {
    let ok_first = name.chars().next().is_some_and(|c| c.is_ascii_lowercase());
    let ok_rest = !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._-".contains(c));
    if ok_first && ok_rest {
        Ok(())
    } else {
        Err(AppError::validation(format!(
            "flag name {name:?} must match [a-z][a-z0-9._-]* (max 128)"
        )))
    }
}

// -------------------------------------------------------------------- store

/// Runtime flag state storage. Revision bumps on EVERY mutation; snapshots
/// pin the revision they were computed from (cache keys use it).
pub trait FlagStore: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<FlagState>, AppError>;
    fn all(&self) -> Result<Vec<FlagState>, AppError>;
    fn set_stage(&self, name: &str, stage: RolloutStage) -> Result<FlagState, AppError>;
    fn set_tenant_stage(
        &self,
        name: &str,
        tenant: TenantId,
        stage: RolloutStage,
    ) -> Result<FlagState, AppError>;
    fn revision(&self) -> Result<FeatureRevision, AppError>;
}

#[derive(Debug, Default)]
pub struct InMemoryFlagStore {
    inner: RwLock<(Vec<FlagState>, FeatureRevision)>,
}

impl InMemoryFlagStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn with_state_mut<R>(
        &self,
        name: &str,
        f: impl FnOnce(&mut FlagState) -> R,
    ) -> Result<FlagState, AppError> {
        let mut guard = self.inner.write().unwrap();
        let (states, revision) = &mut *guard;
        let state = match states.iter_mut().find(|s| s.name == name) {
            Some(existing) => existing,
            None => {
                states.push(FlagState {
                    name: name.to_string(),
                    stage: RolloutStage::Disabled,
                    tenant_stages: vec![],
                });
                states.last_mut().expect("just pushed")
            }
        };
        let out = f(state);
        let state = states.iter().find(|s| s.name == name).cloned().unwrap();
        *revision = FeatureRevision(revision.0 + 1);
        drop(out);
        Ok(state)
    }
}

impl FlagStore for InMemoryFlagStore {
    fn get(&self, name: &str) -> Result<Option<FlagState>, AppError> {
        Ok(self
            .inner
            .read()
            .unwrap()
            .0
            .iter()
            .find(|s| s.name == name)
            .cloned())
    }

    fn all(&self) -> Result<Vec<FlagState>, AppError> {
        Ok(self.inner.read().unwrap().0.clone())
    }

    fn set_stage(&self, name: &str, stage: RolloutStage) -> Result<FlagState, AppError> {
        self.with_state_mut(name, |s| s.stage = stage)
    }

    fn set_tenant_stage(
        &self,
        name: &str,
        tenant: TenantId,
        stage: RolloutStage,
    ) -> Result<FlagState, AppError> {
        self.with_state_mut(name, |s| {
            if let Some(entry) = s.tenant_stages.iter_mut().find(|(tid, _)| *tid == tenant) {
                entry.1 = stage;
            } else {
                s.tenant_stages.push((tenant, stage));
            }
        })
    }

    fn revision(&self) -> Result<FeatureRevision, AppError> {
        Ok(self.inner.read().unwrap().1)
    }
}

/// File-backed store for single-node + CLI usage (`storage/framework/flags.json`).
/// Same semantics as InMemoryFlagStore plus atomic-enough persistence
/// (write temp file, rename). Multi-node deployments get a postgres store.
pub struct FileFlagStore {
    path: std::path::PathBuf,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FileFlagDocument {
    #[serde(default)]
    states: Vec<FlagState>,
    #[serde(default)]
    revision: FeatureRevision,
}

impl FileFlagStore {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }

    fn load(&self) -> Result<FileFlagDocument, AppError> {
        match std::fs::read_to_string(&self.path) {
            Ok(raw) => serde_json::from_str(&raw)
                .map_err(|e| AppError::internal(format!("flags file corrupt: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FileFlagDocument::default()),
            Err(e) => Err(AppError::internal(format!("flags file unreadable: {e}"))),
        }
    }

    fn save(&self, doc: &FileFlagDocument) -> Result<(), AppError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| AppError::internal(format!("flags dir: {e}")))?;
        }
        let raw =
            serde_json::to_string_pretty(doc).map_err(|e| AppError::internal(e.to_string()))?;
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, raw).map_err(|e| AppError::internal(format!("flags write: {e}")))?;
        std::fs::rename(&tmp, &self.path)
            .map_err(|e| AppError::internal(format!("flags rename: {e}")))?;
        Ok(())
    }

    fn mutate(&self, name: &str, f: impl FnOnce(&mut FlagState)) -> Result<FlagState, AppError> {
        let mut doc = self.load()?;
        let state = match doc.states.iter_mut().find(|s| s.name == name) {
            Some(existing) => existing,
            None => {
                doc.states.push(FlagState {
                    name: name.to_string(),
                    stage: RolloutStage::Disabled,
                    tenant_stages: vec![],
                });
                doc.states.last_mut().expect("just pushed")
            }
        };
        f(state);
        let out = state.clone();
        doc.revision = FeatureRevision(doc.revision.0 + 1);
        self.save(&doc)?;
        Ok(out)
    }
}

impl FlagStore for FileFlagStore {
    fn get(&self, name: &str) -> Result<Option<FlagState>, AppError> {
        Ok(self.load()?.states.into_iter().find(|s| s.name == name))
    }

    fn all(&self) -> Result<Vec<FlagState>, AppError> {
        self.load().map(|doc| doc.states)
    }

    fn set_stage(&self, name: &str, stage: RolloutStage) -> Result<FlagState, AppError> {
        self.mutate(name, |s| s.stage = stage)
    }

    fn set_tenant_stage(
        &self,
        name: &str,
        tenant: TenantId,
        stage: RolloutStage,
    ) -> Result<FlagState, AppError> {
        self.mutate(name, |s| {
            if let Some(entry) = s.tenant_stages.iter_mut().find(|(tid, _)| *tid == tenant) {
                entry.1 = stage;
            } else {
                s.tenant_stages.push((tenant, stage));
            }
        })
    }

    fn revision(&self) -> Result<FeatureRevision, AppError> {
        self.load().map(|doc| doc.revision)
    }
}

// ----------------------------------------------------------------- snapshot

/// Resolve the per-execution `FeatureSnapshot` (once per execution, §11).
pub fn resolve_snapshot(
    registry: &FlagRegistry,
    store: &dyn FlagStore,
    input: &FlagInput<'_>,
) -> Result<FeatureSnapshot, AppError> {
    let states = store.all()?;
    let mut enabled = BTreeSet::new();
    for def in registry.definitions() {
        let state = states.iter().find(|s| s.name == def.name);
        if evaluate(def, state, input) {
            enabled.insert(def.name.clone());
        }
    }
    Ok(FeatureSnapshot {
        revision: store.revision()?,
        flags: enabled,
    })
}

/// Service facade wiring registry + store (what the runtime registers).
pub struct FlagService {
    pub registry: Arc<FlagRegistry>,
    pub store: Arc<dyn FlagStore>,
}

impl FlagService {
    pub fn new(registry: Arc<FlagRegistry>, store: Arc<dyn FlagStore>) -> Self {
        Self { registry, store }
    }

    /// Validate that a flag exists, then set its global stage.
    pub fn set_stage(&self, name: &str, stage: RolloutStage) -> Result<FlagState, AppError> {
        self.require_known(name)?;
        self.store.set_stage(name, stage)
    }

    pub fn set_tenant_stage(
        &self,
        name: &str,
        tenant: TenantId,
        stage: RolloutStage,
    ) -> Result<FlagState, AppError> {
        self.require_known(name)?;
        self.store.set_tenant_stage(name, tenant, stage)
    }

    /// Advance one ladder step (100% stays at 100%).
    pub fn advance(&self, name: &str) -> Result<FlagState, AppError> {
        self.require_known(name)?;
        let current = self
            .store
            .get(name)?
            .map(|s| s.stage)
            .unwrap_or(RolloutStage::Disabled);
        self.store.set_stage(name, current.advance())
    }

    /// Roll back one ladder step (disabled stays disabled).
    pub fn rollback(&self, name: &str) -> Result<FlagState, AppError> {
        self.require_known(name)?;
        let current = self
            .store
            .get(name)?
            .map(|s| s.stage)
            .unwrap_or(RolloutStage::Disabled);
        self.store.set_stage(name, current.rollback())
    }

    pub fn enabled(&self, name: &str, input: &FlagInput<'_>) -> Result<bool, AppError> {
        let def = self.require_known(name)?;
        let state = self.store.get(name)?;
        Ok(evaluate(def, state.as_ref(), input))
    }

    pub fn snapshot(&self, input: &FlagInput<'_>) -> Result<FeatureSnapshot, AppError> {
        resolve_snapshot(&self.registry, &*self.store, input)
    }

    fn require_known(&self, name: &str) -> Result<&FlagDefinition, AppError> {
        self.registry
            .get(name)
            .ok_or_else(|| AppError::not_found(format!("flag {name}")))
    }
}

/// Framework-level flags the runtimes consult (S3 wiring). Module flags
/// register on top of this registry during boot.
pub fn platform_registry() -> FlagRegistry {
    let mut registry = FlagRegistry::new();
    registry
        .register(FlagDefinition {
            name: "app.sessions".into(),
            description: "opt-in server sessions (REQUEST lifetime backing; D-66)".into(),
            default_stage: RolloutStage::Disabled,
            rules: vec![],
        })
        .expect("static definition");
    registry
        .register(FlagDefinition {
            name: "app.realtime".into(),
            description: "opt-in realtime WS/SSE endpoints (L42/L43)".into(),
            default_stage: RolloutStage::Disabled,
            rules: vec![],
        })
        .expect("static definition");
    registry
        .register(FlagDefinition {
            name: "app.metrics".into(),
            description: "expose the Prometheus /metrics endpoint (S10 B-17)".into(),
            default_stage: RolloutStage::Disabled,
            rules: vec![],
        })
        .expect("static definition");
    registry
        .register(FlagDefinition {
            name: "app.dashboard".into(),
            description: "serve the Horizon-lite /_dashboard UI (S10 B-20)".into(),
            default_stage: RolloutStage::Disabled,
            rules: vec![],
        })
        .expect("static definition");
    registry
        .register(FlagDefinition {
            name: "app.jwt".into(),
            description:
                "opt-in stateless HS256 JWT bearer interop (B-22 / report R5); PAT+session stays the default"
                    .into(),
            default_stage: RolloutStage::Disabled,
            rules: vec![],
        })
        .expect("static definition");
    registry
}

/// Standard state-file location relative to the workspace root.
pub fn default_flags_path(root: &Path) -> std::path::PathBuf {
    root.join("storage/framework/flags.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(name: &str) -> FlagDefinition {
        FlagDefinition {
            name: name.into(),
            description: String::new(),
            default_stage: RolloutStage::Disabled,
            rules: vec![],
        }
    }

    static EMPTY_CAPS: std::sync::LazyLock<BTreeSet<String>> =
        std::sync::LazyLock::new(BTreeSet::new);

    fn input<'a>(env: &'a str, actor: PrincipalId) -> FlagInput<'a> {
        FlagInput {
            environment: env,
            tenant: None,
            region: None,
            capabilities: &EMPTY_CAPS,
            actor,
            internal_actor: false,
        }
    }

    #[test]
    fn ladder_moves_one_step_in_both_directions() {
        use RolloutStage::*;
        let mut stage = Disabled;
        let up = [Internal, Percent1, Percent10, Percent50, Percent100];
        for expected in up {
            stage = stage.advance();
            assert_eq!(stage, expected);
        }
        assert_eq!(Percent100.advance(), Percent100, "100% is the ceiling");
        let mut stage = Percent100;
        let down = [Percent50, Percent10, Percent1, Internal, Disabled];
        for expected in down {
            stage = stage.rollback();
            assert_eq!(stage, expected);
        }
        assert_eq!(Disabled.rollback(), Disabled, "disabled is the floor");
    }

    #[test]
    fn stage_parse_roundtrip() {
        assert_eq!(
            RolloutStage::parse("disabled").unwrap(),
            RolloutStage::Disabled
        );
        assert_eq!(
            RolloutStage::parse("internal").unwrap(),
            RolloutStage::Internal
        );
        assert_eq!(RolloutStage::parse("1%").unwrap(), RolloutStage::Percent1);
        assert_eq!(RolloutStage::parse("10").unwrap(), RolloutStage::Percent10);
        assert_eq!(RolloutStage::parse("50").unwrap(), RolloutStage::Percent50);
        assert_eq!(
            RolloutStage::parse("100%").unwrap(),
            RolloutStage::Percent100
        );
        assert_eq!(RolloutStage::parse("on").unwrap(), RolloutStage::Percent100);
        assert!(RolloutStage::parse("sometimes").is_err());
    }

    #[test]
    fn dimension_rules_take_precedence_over_stage() {
        let tenant = TenantId::new();
        let mut d = def("beta.op");
        d.rules.push(FlagRule::Tenant {
            id: tenant,
            enable: true,
        });
        d.rules.push(FlagRule::Environment {
            name: "prod".into(),
            enable: false,
        });
        let mut inp = input("dev", PrincipalId::new());
        inp.tenant = Some(tenant);
        assert!(
            evaluate(&d, None, &inp),
            "tenant allow rule wins over missing stage"
        );
        let inp_prod = input("prod", PrincipalId::new());
        assert!(!evaluate(&d, None, &inp_prod), "prod env deny rule wins");
    }

    #[test]
    fn internal_stage_only_for_internal_actors() {
        let d = def("internal.tool");
        let state = FlagState {
            name: d.name.clone(),
            stage: RolloutStage::Internal,
            tenant_stages: vec![],
        };
        let mut inp = input("dev", PrincipalId::new());
        assert!(!evaluate(&d, Some(&state), &inp));
        inp.internal_actor = true;
        assert!(evaluate(&d, Some(&state), &inp));
    }

    #[test]
    fn percentage_rollout_is_stable_and_bounded() {
        let actor = PrincipalId::new();
        let bucket = rollout_bucket("billing.v2", actor);
        assert_eq!(
            bucket,
            rollout_bucket("billing.v2", actor),
            "stable per (flag, actor)"
        );
        assert!(bucket < 100);
        // A *single* second actor collides with the first about once per 100
        // runs (100 buckets), which would make the gate flaky rather than
        // meaningful: sample until one lands elsewhere. The collision odds
        // are now (1/100)^16, i.e. never in practice.
        let mut independent = false;
        for _ in 0..16 {
            if rollout_bucket("billing.v2", PrincipalId::new()) != bucket {
                independent = true;
                break;
            }
        }
        assert!(
            independent,
            "different actors get independent buckets (probabilistically)"
        );

        // Population bounds: over many actors, 50% +/- tolerance.
        let mut enabled = 0u32;
        for _ in 0..1000 {
            if rollout_bucket("billing.v2", PrincipalId::new()) < 50 {
                enabled += 1;
            }
        }
        assert!(
            (400..600).contains(&enabled),
            "50% stage enables ~half, got {enabled}/1000"
        );
    }

    #[test]
    fn tenant_stage_override_beats_global_stage() {
        let tenant = TenantId::new();
        let d = def("search.v2");
        let state = FlagState {
            name: d.name.clone(),
            stage: RolloutStage::Disabled,
            tenant_stages: vec![(tenant, RolloutStage::Percent100)],
        };
        let mut inp = input("dev", PrincipalId::new());
        inp.tenant = Some(tenant);
        assert!(evaluate(&d, Some(&state), &inp));
        inp.tenant = Some(TenantId::new());
        assert!(
            !evaluate(&d, Some(&state), &inp),
            "other tenants keep the global stage"
        );
    }

    #[test]
    fn registry_rejects_duplicates_and_bad_names() {
        let mut reg = FlagRegistry::new();
        reg.register(def("billing.v2")).unwrap();
        assert!(reg.register(def("billing.v2")).is_err());
        assert!(reg.register(def("Bad Name")).is_err());
        assert!(reg.get("billing.v2").is_some());
    }

    #[test]
    fn service_advance_rollback_and_snapshot() {
        let mut reg = FlagRegistry::new();
        reg.register(def("billing.v2")).unwrap();
        reg.register(def("rabbit.hole")).unwrap();
        let svc = FlagService::new(Arc::new(reg), Arc::new(InMemoryFlagStore::new()));

        let s = svc.advance("billing.v2").unwrap();
        assert_eq!(s.stage, RolloutStage::Internal);
        let s = svc.advance("billing.v2").unwrap();
        assert_eq!(s.stage, RolloutStage::Percent1);
        let s = svc.rollback("billing.v2").unwrap();
        assert_eq!(s.stage, RolloutStage::Internal);

        // Snapshot: internal actor sees the internal-stage flag only.
        let mut inp = input("dev", PrincipalId::new());
        inp.internal_actor = true;
        let snap = svc.snapshot(&inp).unwrap();
        assert!(snap.flags.contains("billing.v2"));
        assert!(!snap.flags.contains("rabbit.hole"));
        assert_eq!(
            snap.revision,
            FeatureRevision(3),
            "revision counts every mutation"
        );

        assert!(svc.advance("ghost").is_err());
    }

    #[test]
    fn file_store_persists_across_instances() {
        let dir = std::env::temp_dir().join(format!("keel-flags-test-{}", uuid::Uuid::now_v7()));
        let path = dir.join("flags.json");
        let store = FileFlagStore::new(&path);
        store
            .set_stage("billing.v2", RolloutStage::Percent10)
            .unwrap();
        let store2 = FileFlagStore::new(&path);
        let state = store2.get("billing.v2").unwrap().unwrap();
        assert_eq!(state.stage, RolloutStage::Percent10);
        assert_eq!(store2.revision().unwrap(), FeatureRevision(1));
        std::fs::remove_dir_all(&dir).ok();
    }
}
