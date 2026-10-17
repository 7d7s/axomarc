//! Module: oauth2 -- the guest half (Stage C.2).
//!
//! Everything this module *is* lives in `kelson-oauth2-contract`, the
//! host-side contract the kernel links: the `OauthClient` record, the client
//! store, the handler-bundle carrier and the `ModuleProvider`. What lives
//! here is what the module *does* -- the four RFC 6749 handlers -- and it
//! ships as a `cdylib` the runtime loads from `packages.toml` instead of a
//! library `kelson-demo` links.
//!
//! That separation is the whole point of Stage C.2: the kernel must not
//! statically link business behaviour. The artifact build is the only reason
//! this crate exists as a library target:
//!
//!   cargo build --release --target wasm32-wasip1 -p kelson-mod-oauth2
//!
//! The guest's manifest is declared by `guest_entry!` below and must
//! normalize to the reviewed sidecar (`module.toml` beside this crate) or
//! `WasmHost::prepare` refuses the load with `WasmError::Drift`.
use std::sync::Arc;

use std::sync::Mutex;

use kelson_oauth2_contract::*;
use kelson_context::ExecutionContext;
use kelson_foundation::{AppError, ExecutionProfileKind, OperationId, PrincipalId, TenantId};
use kelson_execution::{BoxFuture, ExecutionPlan, ExecutionUnit, MutationRecord, OperationHandler};
use kelson_identity::{hash_password, verify_password, Secret};
use kelson_lifecycle::{BootError, ModuleState, Phase, Provider, ServiceRegistry};
use kelson_models::{
    FieldDef, FieldFilter, FieldType, ModelDef, ModelRuntime, ModelStore, ModelStoreRef, Record,
};
use kelson_tokens::{TokenPurpose, TokenService};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use time::Duration;
use uuid::Uuid;
/// RFC 6749 sec 4.1.2: codes are short-lived (minutes, not hours) and
/// single-use — the shortest TTL the platform's TTL clock can express.
const CODE_TTL: Duration = Duration::minutes(60);
/// Refresh tokens rotate on every grant (single-use `OauthRefresh` rows);
/// 14 days is the validity window between rotations.
const REFRESH_TTL: Duration = Duration::days(14);
/// Access tokens ARE the platform PATs — the same 30-day TTL
/// `POST /auth/login` issues for `TokenPurpose::Api`.
const ACCESS_TTL: Duration = Duration::days(30);
fn require_tenant(ctx: &ExecutionContext) -> Result<TenantId, AppError> {
    ctx.tenant
        .as_ref()
        .map(|t| t.id)
        .ok_or_else(|| AppError::validation("tenant context is required"))
}

/// RFC 3986 unreserved set stays literal; everything else percent-encodes,
/// so an arbitrary `state` round-trips through the Location header intact.
fn pct_encode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn push_query(target: &mut String, key: &str, value: &str) {
    target.push(if target.contains('?') { '&' } else { '?' });
    target.push_str(key);
    target.push('=');
    target.push_str(&pct_encode(value));
}

fn split_scopes(raw: &str) -> Vec<String> {
    raw.split_whitespace().map(str::to_string).collect()
}

fn authenticate_client(store: &OauthClientStore, input: &Value) -> Result<OauthClient, AppError> {
    store.authenticate(&text_field(input, "client_id"), &text_field(input, "client_secret"))
}

/// PAT labels carry `tenant:<uuid>`; the bearer resolver reads the same
/// prefix, so revocation only reaches tokens in the caller's own tenant.
fn label_tenant(label: &Option<String>) -> Option<TenantId> {
    let raw = label.as_deref()?.strip_prefix("tenant:")?;
    Some(TenantId::from_uuid(Uuid::parse_str(raw).ok()?))
}

/// Protocol-shaped list input: absent -> empty, present -> array of
/// non-empty strings (anything else is a validation error).
fn string_list(input: &Value, field: &str) -> Result<Vec<String>, AppError> {
    let Some(raw) = input.get(field) else {
        return Ok(Vec::new());
    };
    let Some(items) = raw.as_array() else {
        return Err(AppError::validation(format!(
            "{field} must be an array of strings"
        )));
    };
    let mut out = Vec::new();
    for item in items {
        let value = item
            .as_str()
            .ok_or_else(|| AppError::validation(format!("{field} entries must be strings")))?
            .trim();
        if value.is_empty() {
            return Err(AppError::validation(format!(
                "{field} entries must not be empty"
            )));
        }
        out.push(value.to_string());
    }
    Ok(out)
}

// ----------------------------------------------------------------- claims

/// What an authorization code is bound to, carried in the token row's
/// `label` (the only claim channel `kelson-tokens` persists).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CodeClaims {
    client_id: String,
    redirect_uri: String,
    code_challenge: String,
    scope: String,
    tenant: String,
}

/// What a refresh token is bound to (rotation mints a fresh copy).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RefreshClaims {
    client_id: String,
    scope: String,
    tenant: String,
}

fn claims_label<T: Serialize>(claims: &T) -> Result<String, AppError> {
    serde_json::to_string(claims)
        .map_err(|_| AppError::internal("oauth token claims failed to serialize"))
}

fn parse_claims<T: serde::de::DeserializeOwned>(label: Option<&str>) -> Result<T, AppError> {
    let raw = label.filter(|raw| !raw.is_empty()).ok_or_else(|| {
        AppError::denied("token is not valid")
    })?;
    serde_json::from_str(raw).map_err(|_| AppError::denied("token is not valid"))
}

// -------------------------------------------------------------- authorize

/// `oauth2.authorize` (COMMAND, web plane): the browser-facing half of the
/// code flow. Requires a session (the edge enforces it — the route is not
/// public) and PKCE S256; answers `{"to": "<redirect_uri>?code=..."}` which
/// the dispatcher renders as a 302.
pub struct AuthorizeHandler {
    plan: ExecutionPlan,
    store: Arc<OauthClientStore>,
    tokens: Arc<TokenService>,
}

impl AuthorizeHandler {
    pub fn new(store: Arc<OauthClientStore>, tokens: Arc<TokenService>) -> Self {
        Self {
            plan: ExecutionPlan::new(
                OperationId::from_static(OP_AUTHORIZE),
                ExecutionProfileKind::Command,
            )
            .with_capability(OP_AUTHORIZE),
            store,
            tokens,
        }
    }
}

impl OperationHandler for AuthorizeHandler {
    fn plan(&self) -> &ExecutionPlan {
        &self.plan
    }

    fn handle(
        &self,
        ctx: Arc<ExecutionContext>,
        input: Value,
        unit: &mut dyn ExecutionUnit,
    ) -> BoxFuture<'static, Result<Value, AppError>> {
        let outcome = (|| -> Result<Value, AppError> {
            let tenant = require_tenant(&ctx)?;
            let client_id = text_field(&input, "client_id");
            let redirect_uri = text_field(&input, "redirect_uri");
            if client_id.is_empty() || redirect_uri.is_empty() {
                return Err(AppError::validation(
                    "client_id and redirect_uri are required",
                ));
            }
            if text_field(&input, "response_type") != "code" {
                return Err(AppError::validation("response_type must be code"));
            }
            // Client and redirect are validated BEFORE anything is issued:
            // an unvalidated redirect target never sees a Location header.
            let client = self.store.require(&client_id)?;
            if !client.redirect_uris.contains(&redirect_uri) {
                return Err(AppError::validation(
                    "redirect_uri is not registered for this client",
                ));
            }
            // PKCE (RFC 7636): S256 only — plain is refused outright.
            let challenge = text_field(&input, "code_challenge");
            if challenge.is_empty() {
                return Err(AppError::validation(
                    "code_challenge is required (PKCE S256)",
                ));
            }
            if text_field(&input, "code_challenge_method") != "S256" {
                return Err(AppError::validation(
                    "code_challenge_method must be S256",
                ));
            }
            // Requested scopes narrow the registered set; omitting the
            // parameter grants the full registered scope (RFC defaulting).
            let requested = split_scopes(&text_field(&input, "scope"));
            let granted = if requested.is_empty() {
                client.scopes.clone()
            } else {
                for scope in &requested {
                    if !client.scopes.contains(scope) {
                        return Err(AppError::validation(format!(
                            "scope {scope:?} is not registered for this client"
                        )));
                    }
                }
                requested
            };
            let claims = CodeClaims {
                client_id: client.client_id.clone(),
                redirect_uri: redirect_uri.clone(),
                code_challenge: challenge,
                scope: granted.join(" "),
                tenant: tenant.as_uuid().to_string(),
            };
            let issued = self.tokens.issue(
                ctx.principal.id,
                TokenPurpose::OauthCode,
                CODE_TTL,
                Some(claims_label(&claims)?),
            )?;
            // KER-03 lineage: every authorize success issues a code row —
            // the Command delta contract holds on exactly this invariant
            // (all pre-issuance failures answer as errors, never Ok).
            unit.record_mutation(MutationRecord {
                resource_type: "oauth_code".into(),
                resource_id: Some(issued.token.id),
                resource_version: 1,
                before: None,
                after: json!({
                    "client_id": claims.client_id,
                    "scope": claims.scope,
                }),
                stream: None,
            });
            let mut target = redirect_uri;
            push_query(&mut target, "code", &issued.plaintext);
            let state = text_field(&input, "state");
            if !state.is_empty() {
                push_query(&mut target, "state", &state);
            }
            Ok(json!({ "to": target }))
        })();
        Box::pin(async move { outcome })
    }
}

// ------------------------------------------------------------------ token

/// `oauth2.token` (COMMAND, public): machine clients authenticate with
/// `client_id` + `client_secret` inside the handler (the edge's session
/// stages are stripped on purpose — this is RFC 6749 sec 4.1.3 / sec 6).
/// Grants: `authorization_code` (single-use + PKCE) and `refresh_token`
/// (single-use rotation). Access tokens are platform PATs.
pub struct TokenHandler {
    plan: ExecutionPlan,
    store: Arc<OauthClientStore>,
    tokens: Arc<TokenService>,
}

impl TokenHandler {
    pub fn new(store: Arc<OauthClientStore>, tokens: Arc<TokenService>) -> Self {
        let mut plan = ExecutionPlan::new(
            OperationId::from_static(OP_TOKEN),
            ExecutionProfileKind::Command,
        )
        .with_capability(OP_TOKEN);
        // Public route: the edge admits anonymous callers because the plan
        // says so; client credential checks happen below, per RFC.
        plan.authorization.required = false;
        Self {
            plan,
            store,
            tokens,
        }
    }

    /// Mint an access PAT (tenant-labelled, bearer-resolvable) plus a
    /// rotated refresh token — shared by both grants. The issuance is the
    /// execution's KER-03 effect: Command profiles demand a mutation on
    /// every success, and every token endpoint success mints here.
    fn mint(
        &self,
        unit: &mut dyn ExecutionUnit,
        principal: PrincipalId,
        claims: &RefreshClaims,
    ) -> Result<Value, AppError> {
        let access = self.tokens.issue(
            principal,
            TokenPurpose::Api,
            ACCESS_TTL,
            Some(format!("tenant:{}", claims.tenant)),
        )?;
        let refresh = self.tokens.issue(
            principal,
            TokenPurpose::OauthRefresh,
            REFRESH_TTL,
            Some(claims_label(claims)?),
        )?;
        unit.record_mutation(MutationRecord {
            resource_type: "oauth_token".into(),
            resource_id: Some(access.token.id),
            resource_version: 1,
            before: None,
            after: json!({
                "client_id": claims.client_id,
                "scope": claims.scope,
            }),
            stream: None,
        });
        Ok(json!({
            "access_token": access.plaintext,
            "token_type": "Bearer",
            "expires_in": ACCESS_TTL.whole_seconds(),
            "refresh_token": refresh.plaintext,
            "scope": claims.scope,
        }))
    }

    fn authorization_code(
        &self,
        unit: &mut dyn ExecutionUnit,
        input: &Value,
        client: OauthClient,
    ) -> Result<Value, AppError> {
        let code = text_field(input, "code");
        let redirect_uri = text_field(input, "redirect_uri");
        let verifier = text_field(input, "code_verifier");
        if code.is_empty() {
            return Err(AppError::validation("code is required"));
        }
        if redirect_uri.is_empty() {
            return Err(AppError::validation("redirect_uri is required"));
        }
        if verifier.is_empty() {
            return Err(AppError::validation("code_verifier is required"));
        }
        // Single-use redemption: the FIRST exchange wins; every replay dies
        // on consume. Client auth already happened above, so a caller can
        // only burn codes with valid credentials of their own.
        let consumed = self.tokens.consume(TokenPurpose::OauthCode, &code)?;
        let claims: CodeClaims = parse_claims(consumed.label.as_deref())?;
        if claims.client_id != client.client_id {
            return Err(AppError::denied(
                "authorization code was not issued to this client",
            ));
        }
        if claims.redirect_uri != redirect_uri {
            return Err(AppError::denied(
                "redirect_uri does not match the authorization request",
            ));
        }
        if !kelson_tokens::valid_code_verifier(&verifier)
            || kelson_tokens::pkce_s256_challenge(&verifier) != claims.code_challenge
        {
            return Err(AppError::denied("PKCE verification failed"));
        }
        self.mint(
            unit,
            consumed.principal_id,
            &RefreshClaims {
                client_id: claims.client_id,
                scope: claims.scope,
                tenant: claims.tenant,
            },
        )
    }

    fn refresh_token(
        &self,
        unit: &mut dyn ExecutionUnit,
        input: &Value,
        client: OauthClient,
    ) -> Result<Value, AppError> {
        let refresh = text_field(input, "refresh_token");
        if refresh.is_empty() {
            return Err(AppError::validation("refresh_token is required"));
        }
        // Rotation: consume kills the presented token; a fresh pair comes
        // back. Client binding is checked after consumption so a stolen
        // token is dead either way (fail closed).
        let consumed = self
            .tokens
            .consume(TokenPurpose::OauthRefresh, &refresh)?;
        let claims: RefreshClaims = parse_claims(consumed.label.as_deref())?;
        if claims.client_id != client.client_id {
            return Err(AppError::denied(
                "refresh token was not issued to this client",
            ));
        }
        self.mint(unit, consumed.principal_id, &claims)
    }
}

impl OperationHandler for TokenHandler {
    fn plan(&self) -> &ExecutionPlan {
        &self.plan
    }

    fn handle(
        &self,
        _ctx: Arc<ExecutionContext>,
        input: Value,
        unit: &mut dyn ExecutionUnit,
    ) -> BoxFuture<'static, Result<Value, AppError>> {
        let outcome = (|| -> Result<Value, AppError> {
            let grant = text_field(&input, "grant_type");
            // RFC 6749 sec 5.2: client authentication precedes grant
            // handling — unknown clients never touch a token row.
            let client = authenticate_client(&self.store, &input)?;
            match grant.as_str() {
                "authorization_code" => self.authorization_code(unit, &input, client),
                "refresh_token" => self.refresh_token(unit, &input, client),
                other => Err(AppError::validation(format!(
                    "unsupported grant_type {other:?}"
                ))),
            }
        })();
        Box::pin(async move { outcome })
    }
}

// ----------------------------------------------------------------- revoke

/// `oauth2.revoke` (COMMAND, public): RFC 7009. Client-authenticated;
/// refresh tokens are consumed when they belong to the caller, PATs are
/// revoked when their tenant label matches the caller's tenant. Unknown
/// tokens answer `{"revoked": false}` — never a probing oracle.
pub struct RevokeHandler {
    plan: ExecutionPlan,
    store: Arc<OauthClientStore>,
    tokens: Arc<TokenService>,
}

impl RevokeHandler {
    pub fn new(store: Arc<OauthClientStore>, tokens: Arc<TokenService>) -> Self {
        let mut plan = ExecutionPlan::new(
            OperationId::from_static(OP_REVOKE),
            ExecutionProfileKind::Command,
        )
        .with_capability(OP_REVOKE);
        plan.authorization.required = false;
        // RFC 7009 sec 2.2 answers 200 even for tokens that never existed
        // (invalid tokens are NOT an error), so a protocol-legal success may
        // transition nothing — relax the Command delta demand in lockstep
        // (G06 coherence: the profile flag and the plan flag move together)
        // rather than fabricate a transition for the no-op answer. Real
        // revocations still record their effect below.
        plan.profile.delta_required = false;
        plan.delta.required = false;
        Self {
            plan,
            store,
            tokens,
        }
    }

    fn revoke_refresh(&self, client_id: &str, token: &str) -> Result<bool, AppError> {
        let Ok(auth) = self.tokens.verify(TokenPurpose::OauthRefresh, token) else {
            return Ok(false);
        };
        let Ok(claims) = serde_json::from_str::<RefreshClaims>(auth.label.as_deref().unwrap_or(""))
        else {
            return Ok(false);
        };
        if claims.client_id != client_id {
            // Not ours: leave it alone and reveal nothing (RFC 7009 sec 2.2).
            return Ok(false);
        }
        match self.tokens.consume(TokenPurpose::OauthRefresh, token) {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }

    fn revoke_access(&self, tenant: Option<TenantId>, token: &str) -> Result<bool, AppError> {
        let Ok(auth) = self.tokens.verify(TokenPurpose::Api, token) else {
            return Ok(false);
        };
        match (label_tenant(&auth.label), tenant) {
            (Some(owned), Some(requested)) if owned.as_uuid() == requested.as_uuid() => {
                self.tokens.revoke(auth.id)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

impl OperationHandler for RevokeHandler {
    fn plan(&self) -> &ExecutionPlan {
        &self.plan
    }

    fn handle(
        &self,
        ctx: Arc<ExecutionContext>,
        input: Value,
        unit: &mut dyn ExecutionUnit,
    ) -> BoxFuture<'static, Result<Value, AppError>> {
        let outcome = (|| -> Result<Value, AppError> {
            let token = text_field(&input, "token");
            if token.is_empty() {
                return Err(AppError::validation("token is required"));
            }
            let client = authenticate_client(&self.store, &input)?;
            let tenant = ctx.tenant.as_ref().map(|t| t.id);
            let hint = text_field(&input, "token_type_hint");
            let revoked = if hint == "refresh_token" {
                self.revoke_refresh(&client.client_id, &token)? || self.revoke_access(tenant, &token)?
            } else {
                self.revoke_access(tenant, &token)? || self.revoke_refresh(&client.client_id, &token)?
            };
            if revoked {
                // A real transition hit the token store. The presented token
                // VALUE never rides the delta stream — resource_id stays
                // None on purpose (the record documents the effect, not the
                // credential).
                unit.record_mutation(MutationRecord {
                    resource_type: "oauth_revoke".into(),
                    resource_id: None,
                    resource_version: 1,
                    before: None,
                    after: json!({ "revoked": true }),
                    stream: None,
                });
            }
            Ok(json!({ "revoked": revoked }))
        })();
        Box::pin(async move { outcome })
    }
}

// --------------------------------------------------------- clients.manage

/// `oauth2.clients.manage` (COMMAND): registry administration through
/// `kelson call oauth2.clients.manage` (routes/console.toml) — no HTTP
/// surface, the protocol endpoints stay exactly the four docs/19 names.
/// One operation, an `action` discriminator: create / list / update /
/// delete. Create returns the client secret EXACTLY once.
pub struct ClientsManageHandler {
    plan: ExecutionPlan,
    store: Arc<OauthClientStore>,
}

impl ClientsManageHandler {
    pub fn new(store: Arc<OauthClientStore>) -> Self {
        let mut plan = ExecutionPlan::new(
            OperationId::from_static(OP_CLIENTS_MANAGE),
            ExecutionProfileKind::Command,
        )
        .with_capability(OP_CLIENTS_MANAGE);
        // Mixed surface: `list` is a pure read while create / update /
        // delete project real registry transitions (records below). Relax
        // the Command delta demand in lockstep (G06 coherence: the profile
        // flag and the plan flag move together) so reads pass without
        // fabricating a mutation they did not perform.
        plan.profile.delta_required = false;
        plan.delta.required = false;
        Self { plan, store }
    }

    fn parse_id(input: &Value) -> Result<Uuid, AppError> {
        let raw = text_field(input, "id");
        Uuid::parse_str(&raw).map_err(|_| AppError::validation("id must be a uuid"))
    }
}

impl OperationHandler for ClientsManageHandler {
    fn plan(&self) -> &ExecutionPlan {
        &self.plan
    }

    fn handle(
        &self,
        _ctx: Arc<ExecutionContext>,
        input: Value,
        unit: &mut dyn ExecutionUnit,
    ) -> BoxFuture<'static, Result<Value, AppError>> {
        let outcome = (|| -> Result<Value, AppError> {
            match text_field(&input, "action").as_str() {
                "create" => {
                    let name = text_field(&input, "name");
                    if name.is_empty() {
                        return Err(AppError::validation("name is required"));
                    }
                    let redirect_uris = string_list(&input, "redirect_uris")?;
                    if redirect_uris.is_empty() {
                        return Err(AppError::validation(
                            "redirect_uris must register at least one URI",
                        ));
                    }
                    let scopes = string_list(&input, "scopes")?;
                    let (client, secret) = self.store.create(&name, redirect_uris, scopes)?;
                    unit.record_mutation(MutationRecord {
                        resource_type: "oauth_client".into(),
                        resource_id: Some(client.id),
                        resource_version: client.version,
                        before: None,
                        after: json!(client),
                        stream: None,
                    });
                    Ok(json!({
                        "id": client.id.to_string(),
                        "client_id": client.client_id,
                        "secret": secret,
                        "name": client.name,
                        "redirect_uris": client.redirect_uris,
                        "scopes": client.scopes,
                    }))
                }
                "list" => Ok(json!({ "clients": self.store.list()? })),
                "update" => {
                    let id = Self::parse_id(&input)?;
                    let name = match input.get("name") {
                        Some(_) => {
                            let name = text_field(&input, "name");
                            if name.is_empty() {
                                return Err(AppError::validation("name must not be empty"));
                            }
                            Some(name)
                        }
                        None => None,
                    };
                    let redirect_uris = match input.get("redirect_uris") {
                        Some(_) => {
                            let uris = string_list(&input, "redirect_uris")?;
                            if uris.is_empty() {
                                return Err(AppError::validation(
                                    "redirect_uris must register at least one URI",
                                ));
                            }
                            Some(uris)
                        }
                        None => None,
                    };
                    let scopes = match input.get("scopes") {
                        Some(_) => Some(string_list(&input, "scopes")?),
                        None => None,
                    };
                    if name.is_none() && redirect_uris.is_none() && scopes.is_none() {
                        return Err(AppError::validation(
                            "pass at least one of name, redirect_uris, or scopes",
                        ));
                    }
                    let before = self.store.get(id)?;
                    let client = self.store.update(id, name, redirect_uris, scopes)?;
                    unit.record_mutation(MutationRecord {
                        resource_type: "oauth_client".into(),
                        resource_id: Some(client.id),
                        resource_version: client.version,
                        before: Some(json!(before)),
                        after: json!(client),
                        stream: None,
                    });
                    Ok(json!({
                        "id": client.id.to_string(),
                        "client_id": client.client_id,
                        "name": client.name,
                        "redirect_uris": client.redirect_uris,
                        "scopes": client.scopes,
                        "version": client.version,
                    }))
                }
                "delete" => {
                    let id = Self::parse_id(&input)?;
                    let before = self.store.get(id)?;
                    self.store.delete(id)?;
                    unit.record_mutation(MutationRecord {
                        resource_type: "oauth_client".into(),
                        resource_id: Some(id),
                        resource_version: before.version + 1,
                        before: Some(json!(before)),
                        after: json!({ "deleted": id.to_string() }),
                        stream: None,
                    });
                    Ok(json!({ "deleted": id.to_string() }))
                }
                other => Err(AppError::validation(format!(
                    "action must be create, list, update, or delete (got {other:?})"
                ))),
            }
        })();
        Box::pin(async move { outcome })
    }
}

// ------------------------------------------------------------ registration

/// Bundle every oauth2 operation handler.
pub fn handlers(
    store: Arc<OauthClientStore>,
    tokens: Arc<TokenService>,
) -> Vec<Arc<dyn OperationHandler>> {
    vec![
        Arc::new(AuthorizeHandler::new(store.clone(), tokens.clone())),
        Arc::new(TokenHandler::new(store.clone(), tokens.clone())),
        Arc::new(RevokeHandler::new(store.clone(), tokens)),
        Arc::new(ClientsManageHandler::new(store)),
    ]
}

// ------------------------------------------------------------- module entry

/// What this guest declares to `WasmHost::prepare` (see the appointments
/// guest for why this string is generated from the sidecar).
const MANIFEST_JSON: &str = r#"{"module":{"name":"oauth2","version":"0.1.0","maturity":"L0","plane":"tenant","description":"OAuth2 authorization server: authorize, token, revoke, client registry","api":1},"capabilities":{"authorize":{"profile":"COMMAND","delta":"required","audit":"required","idempotency":"required","inputs":["client_id","redirect_uri","response_type","state","scope","code_challenge","code_challenge_method"]},"clients.manage":{"profile":"COMMAND","delta":"required","audit":"required","idempotency":"required","inputs":["action","id","name","redirect_uris","scopes"]},"revoke":{"profile":"COMMAND","delta":"required","audit":"required","idempotency":"required","inputs":["token","token_type_hint","client_id","client_secret"]},"token":{"profile":"COMMAND","delta":"required","audit":"required","idempotency":"required","inputs":["grant_type","code","code_verifier","refresh_token","client_id","client_secret","redirect_uri"]}},"dependencies":{"contracts":[]},"routes":[{"method":"GET","path":"/oauth/authorize","operation":"authorize","capability":"authorize","version":"v1","name":"authorize","resource":null,"only":[],"except":[],"middleware":[],"public":false,"deprecated":false,"response":"redirect","bindings":[],"plane":"web","group":null,"constraints":[],"singleton":false,"fallback":false,"excluded_middleware":[],"view":null,"domain":null,"subdomain":null},{"method":"POST","path":"/oauth/authorize","operation":"authorize","capability":"authorize","version":"v1","name":"authorize_post","resource":null,"only":[],"except":[],"middleware":[],"public":false,"deprecated":false,"response":"redirect","bindings":[],"plane":"web","group":null,"constraints":[],"singleton":false,"fallback":false,"excluded_middleware":[],"view":null,"domain":null,"subdomain":null},{"method":"POST","path":"/oauth/token","operation":"token","capability":"token","version":"v1","name":"token","resource":null,"only":[],"except":[],"middleware":[],"public":true,"deprecated":false,"response":"json","bindings":[],"plane":null,"group":null,"constraints":[],"singleton":false,"fallback":false,"excluded_middleware":[],"view":null,"domain":null,"subdomain":null},{"method":"POST","path":"/oauth/revoke","operation":"revoke","capability":"revoke","version":"v1","name":"revoke","resource":null,"only":[],"except":[],"middleware":[],"public":true,"deprecated":false,"response":"json","bindings":[],"plane":null,"group":null,"constraints":[],"singleton":false,"fallback":false,"excluded_middleware":[],"view":null,"domain":null,"subdomain":null}],"events":[],"workflows":[],"validation":[],"route_groups":[],"console_commands":[],"channels":[],"url":{"defaults":{}},"config":null,"budget":{"max_memory_mb":null,"max_fuel":null,"max_concurrency":null,"execution_timeout_ms":null}}"#;

/// Build the handlers a dispatch session sees, behind the seam's
/// host-backed stores: client rows travel through `host.model_*` and codes,
/// refresh tokens and PATs through `host.token_*`, each landing in the store
/// the runtime installed at boot. The kernel therefore owns both stores while
/// never linking the RFC 6749 behaviour that reads and writes them.
fn build_handlers() -> Vec<Arc<dyn OperationHandler>> {
    let store = Arc::new(OauthClientStore::new(Arc::new(
        kelson_module_sdk::HostModelStore::new(),
    )));
    let tokens = Arc::new(TokenService::new(Arc::new(
        kelson_module_sdk::HostTokenStore::new(),
    )));
    handlers(store, tokens)
}

kelson_module_sdk::guest_entry! {
    manifest: MANIFEST_JSON,
    handlers: build_handlers
}
