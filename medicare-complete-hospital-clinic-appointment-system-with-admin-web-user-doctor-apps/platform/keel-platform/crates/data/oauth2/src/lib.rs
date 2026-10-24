//! Module: oauth2 — OAuth2 authorization server (B-21, Wave 8 P5).
//!
//! Ships through the module ABI gate (module.toml + crate), never core:
//! four capabilities (`authorize`, `token`, `revoke`, `clients.manage`)
//! registered on the shared CommandBus like any other module.
//!
//! Design posture (docs/19 B-21):
//! - **Codes and refresh tokens reuse `kelson-tokens`** — hashed, single-use,
//!   TTL rows (`TokenPurpose::OauthCode` / `OauthRefresh`, landed earlier
//!   for exactly this). Only sha256 hex of the plaintext is persisted.
//! - **Access tokens are the platform's existing PATs** (`TokenPurpose::Api`)
//!   with the `tenant:<uuid>` label the bearer resolver already reads, so a
//!   minted access token authenticates every api-plane route unchanged.
//! - **PKCE is S256 only** (RFC 7636): the authorize endpoint requires a
//!   `code_challenge`, the token endpoint re-derives it from `code_verifier`.
//! - **The client registry is global** (`oauth_clients`, no tenant column):
//!   the token/revoke endpoints are anonymous machine calls that resolve a
//!   client by its globally unique `client_id`; secrets are argon2 hashed
//!   through the shared identity password helpers and shown exactly once.
//! - **The browser flow is consent-less for first-party clients**: the
//!   session on `GET/POST /oauth/authorize` IS the approval — both verbs
//!   issue a bound code and 302 back to an allowlisted `redirect_uri`
//!   (web plane, CSRF-protected POST). Failures before the redirect is
//!   validated fail closed as envelopes and NEVER redirect.

use std::sync::Arc;

use std::sync::Mutex;

use kelson_execution::OperationHandler;
use kelson_foundation::AppError;
use kelson_lifecycle::{BootError, ModuleState, Phase, Provider, ServiceRegistry};
use kelson_models::{
    FieldDef, FieldFilter, FieldType, ModelDef, ModelRuntime, ModelStore, ModelStoreRef, Record,
};
use kelson_tokens::TokenService;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

pub const OP_AUTHORIZE: &str = "oauth2.authorize";
pub const OP_TOKEN: &str = "oauth2.token";
pub const OP_REVOKE: &str = "oauth2.revoke";
pub const OP_CLIENTS_MANAGE: &str = "oauth2.clients.manage";

// ------------------------------------------------------------ client model

/// One registered OAuth client. The secret hash never leaves the store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OauthClient {
    pub id: Uuid,
    pub name: String,
    /// Public client identifier (opaque, globally unique).
    pub client_id: String,
    /// Exact-match redirect allowlist (RFC 6749 sec 3.1.2.3).
    pub redirect_uris: Vec<String>,
    /// Registered scopes; authorize requests may narrow but never widen.
    pub scopes: Vec<String>,
    pub version: i64,
}

/// The persisted model: mirrors the `20260826000001_oauth2.sql` migration
/// exactly. Not tenant-scoped (see crate docs) and physically deleting —
/// there is no `deleted_at` column to soften into.
fn oauth_client_model() -> ModelDef {
    ModelDef::new("OauthClient", "oauth_clients")
        .hard_deletes()
        .field(FieldDef::new("id", FieldType::Uuid).required())
        .field(FieldDef::new("name", FieldType::Text).required())
        .field(
            FieldDef::new("client_id", FieldType::Text)
                .required()
                .unique(),
        )
        .field(FieldDef::new("secret_hash", FieldType::Text).required())
        .field(FieldDef::new("redirect_uris", FieldType::Json).required())
        .field(FieldDef::new("scopes", FieldType::Json).required())
        .field(
            FieldDef::new("version", FieldType::Int64)
                .required()
                .with_default("1"),
        )
        .field(
            FieldDef::new("created_at", FieldType::Timestamp)
                .required()
                .with_default("now()"),
        )
        .field(
            FieldDef::new("updated_at", FieldType::Timestamp)
                .required()
                .with_default("now()"),
        )
}

fn from_record(record: &Record) -> Result<OauthClient, AppError> {
    let id = record
        .get("id")
        .and_then(|v| v.as_str())
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| AppError::validation("oauth client record missing id"))?;
    let list = |field: &str| -> Vec<String> {
        record
            .get(field)
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(OauthClient {
        id,
        name: record
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        client_id: record
            .get("client_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        redirect_uris: list("redirect_uris"),
        scopes: list("scopes"),
        version: record.get("version").and_then(|v| v.as_i64()).unwrap_or(1),
    })
}

// ------------------------------------------------------------ client store

/// Domain store over the framework `ModelStore` port (global scope — every
/// lookup passes `None` for tenant because the registry is not tenant
/// keyed). Backends: `InMemoryModelStore` in the harness, `PgModelStore`
/// in production over the `oauth_clients` table.
#[derive(Clone)]
pub struct OauthClientStore {
    backend: Arc<dyn ModelStore>,
    runtime: ModelRuntime,
    hasher: Arc<dyn SecretHasher>,
}

impl OauthClientStore {
    pub fn new(backend: Arc<dyn ModelStore>, hasher: Arc<dyn SecretHasher>) -> Self {
        Self {
            backend,
            runtime: ModelRuntime::new(oauth_client_model()),
            hasher,
        }
    }

    /// Register a client. Returns the row plus the PLAINTEXT secret — the
    /// only moment it ever exists outside a caller's hands.
    pub fn create(
        &self,
        name: &str,
        redirect_uris: Vec<String>,
        scopes: Vec<String>,
    ) -> Result<(OauthClient, String), AppError> {
        let secret = kelson_tokens::generate_plaintext();
        let secret_hash = self.hasher.hash(&secret)?;
        let mut input = Record::new();
        input.insert("id".into(), json!(Uuid::now_v7().to_string()));
        input.insert("name".into(), json!(name));
        input.insert("client_id".into(), json!(Uuid::now_v7().to_string()));
        input.insert("secret_hash".into(), json!(secret_hash));
        input.insert("redirect_uris".into(), json!(redirect_uris));
        input.insert("scopes".into(), json!(scopes));
        input.insert("version".into(), json!(1));
        let stored = self.runtime.insert(&*self.backend, &input, None)?;
        Ok((from_record(&stored)?, secret))
    }

    fn row(&self, client_id: &str) -> Result<Option<Record>, AppError> {
        let page = self.runtime.list(
            &*self.backend,
            None,
            vec![FieldFilter::eq("client_id", client_id)],
            &[],
            None,
        )?;
        Ok(page.items.into_iter().next())
    }

    /// Authorize-endpoint lookup: unknown client fails the envelope path
    /// (fail closed — no redirect is ever attempted for it).
    pub fn require(&self, client_id: &str) -> Result<OauthClient, AppError> {
        let row = self
            .row(client_id)?
            .ok_or_else(|| AppError::validation("unknown oauth client"))?;
        from_record(&row)
    }

    /// Client authentication for the machine endpoints (RFC 6749 sec 2.3).
    /// Uniform 401: missing, unknown and wrong-secret are indistinguishable.
    pub fn authenticate(&self, client_id: &str, secret: &str) -> Result<OauthClient, AppError> {
        if client_id.is_empty() || secret.is_empty() {
            return Err(AppError::unauthenticated());
        }
        let row = self.row(client_id)?.ok_or_else(AppError::unauthenticated)?;
        let stored = row
            .get("secret_hash")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        match self.hasher.verify(secret, stored) {
            Ok(true) => from_record(&row),
            _ => Err(AppError::unauthenticated()),
        }
    }

    pub fn get(&self, id: Uuid) -> Result<OauthClient, AppError> {
        from_record(&self.runtime.get(&*self.backend, id, None)?)
    }

    pub fn list(&self) -> Result<Vec<OauthClient>, AppError> {
        let page = self.runtime.list(&*self.backend, None, vec![], &[], None)?;
        page.items.iter().map(from_record).collect()
    }

    pub fn update(
        &self,
        id: Uuid,
        name: Option<String>,
        redirect_uris: Option<Vec<String>>,
        scopes: Option<Vec<String>>,
    ) -> Result<OauthClient, AppError> {
        let mut changes = Record::new();
        if let Some(name) = name {
            changes.insert("name".into(), json!(name));
        }
        if let Some(uris) = redirect_uris {
            changes.insert("redirect_uris".into(), json!(uris));
        }
        if let Some(scopes) = scopes {
            changes.insert("scopes".into(), json!(scopes));
        }
        let current = from_record(&self.runtime.get(&*self.backend, id, None)?)?;
        changes.insert("version".into(), json!(current.version + 1));
        let stored = self.runtime.update(&*self.backend, id, None, &changes)?;
        from_record(&stored)
    }

    pub fn delete(&self, id: Uuid) -> Result<(), AppError> {
        self.runtime.delete(&*self.backend, id, None)?;
        Ok(())
    }
}

// -------------------------------------------------------------- primitives

/// Read one input field as a trimmed string. Shared by every handler this
/// module ships *and* by the client store below, so it lives on the contract
/// side of the seam and the guest reaches it through the re-export rather
/// than carrying a second copy.
pub fn text_field(input: &Value, field: &str) -> String {
    input
        .get(field)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}
/// Handler bundle carrier for the `ServiceRegistry`. The registry is a
/// typed singleton map (one instance per concrete type; a duplicate insert
/// refuses boot) and plain `Vec<Arc<dyn OperationHandler>>` is already
/// owned by the appointments module — so this module rides its own
/// newtype, which the host's CommandBus assembly reads alongside the
/// legacy slot.
pub struct OauthHandlerBundle(pub Vec<Arc<dyn OperationHandler>>);

/// Tracks the module's lifecycle state (Discovered -> Validated -> Registered
/// -> Initialized -> Ready -> Degraded/Disabled).
pub struct ModuleProvider {
    state: Mutex<ModuleState>,
}

impl ModuleProvider {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(ModuleState::Discovered),
        }
    }

    pub fn state(&self) -> ModuleState {
        *self.state.lock().unwrap()
    }

    #[allow(dead_code)]
    fn transition_to_validated(&self) {
        *self.state.lock().unwrap() = ModuleState::Validated;
    }

    #[allow(dead_code)]
    fn transition_to_registered(&self) {
        *self.state.lock().unwrap() = ModuleState::Registered;
    }

    #[allow(dead_code)]
    fn transition_to_ready(&self) {
        *self.state.lock().unwrap() = ModuleState::Ready;
    }
}

impl Default for ModuleProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for ModuleProvider {
    fn name(&self) -> &str {
        "module.oauth2"
    }
    fn phase(&self) -> Phase {
        Phase::Modules
    }
    fn init(&self, registry: &mut ServiceRegistry) -> Result<(), BootError> {
        // Transition: Discovered -> Validated -> Registered
        self.transition_to_validated();
        // Stage C.2: the handlers this provider used to register now arrive
        // from the module's WASM artifact, so what remains here is the
        // boot-time check that the two stores a module reads and writes
        // exist at all -- the same fail-closed posture the seam's
        // `install_services` applies, moved up to boot. A host without them
        // refuses here rather than at first dispatch.
        let _backend = registry.get::<ModelStoreRef>()?;
        let _tokens = registry.get::<TokenService>()?;
        // Transition: Registered
        self.transition_to_registered();
        // Transition: Initialized -> Ready
        self.transition_to_ready();
        Ok(())
    }
}

// ------------------------------------------------------------- the KDF side

/// Who runs the KDF for this store.
///
/// Injected rather than called directly: argon2's baseline (19 * 1024 KiB
/// and billions of instructions) is affordable natively but not inside a
/// 16 MiB / 10M-fuel guest, so a guest reaches it through
/// `host.password_hash` while a host calls `kelson_identity` itself. Which
/// of the two a build is doing belongs to that build - and a data crate
/// reaches neither an SDK nor a host, hence a seam here instead of a `cfg`.
///
/// Both paths run the same argon2id parameters, so a hash written by one is
/// readable by the other; only where the seconds are spent differs.
pub trait SecretHasher: Send + Sync {
    /// Hash a secret for storage. The plaintext never leaves the caller.
    fn hash(&self, secret: &str) -> Result<String, AppError>;
    /// Whether a presented secret matches what was stored.
    fn verify(&self, secret: &str, hash: &str) -> Result<bool, AppError>;
}

/// The KDF as the platform runs it outside a sandbox: `kelson_identity`
/// directly, no seam, no metering.
pub struct LocalSecretHasher;

impl SecretHasher for LocalSecretHasher {
    fn hash(&self, secret: &str) -> Result<String, AppError> {
        kelson_identity::hash_password(&kelson_identity::Secret::new(secret))
    }

    fn verify(&self, secret: &str, hash: &str) -> Result<bool, AppError> {
        kelson_identity::verify_password(&kelson_identity::Secret::new(secret), hash)
    }
}
