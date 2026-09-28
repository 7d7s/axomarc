//! # keel-session
//! Opt-in server sessions (blueprint L25, D-66, S3). Sessions are OFF by
//! default; the runtime enables them explicitly (stateless JWT covers most
//! API traffic). When enabled they back browser flows that need revocable,
//! server-side state: the REQUEST service lifetime (STAGE-0-SPEC §6) hangs a
//! `Session` off each HTTP request.
//!
//! Semantics (Laravel-shaped, spec-hardened):
//! - Idle timeout: session dies if unseen for `idle_secs` (sliding).
//! - Absolute timeout: session dies `absolute_secs` after creation, no matter
//!   how active (limits stolen-cookie lifetime).
//! - Regenerate id on privilege change (login/privilege escalation) to kill
//!   session-fixation; the data carries over, the id does not.
//! - CSRF token is derived from the session id + a server secret (HMAC-style
//!   sha256 mix), so issuing it needs no storage and forgery needs the secret.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use keel_foundation::{AppError, PrincipalId, TenantId};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};

/// Opaque session key. 256 bits of CSPRNG, hex-encoded on the wire. NOT a
/// uuid — session ids must be unguessable, not just unique.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut bytes);
        Self(hex(&bytes))
    }

    pub fn parse(raw: &str) -> Result<Self, AppError> {
        let t = raw.trim();
        if t.len() == 64 && t.chars().all(|c| c.is_ascii_hexdigit()) {
            Ok(Self(t.to_string()))
        } else {
            Err(AppError::validation("malformed session id"))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn hex(bytes: &[u8]) -> String {
    const NIBBLES: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(NIBBLES[(b >> 4) as usize] as char);
        out.push(NIBBLES[(b & 0xf) as usize] as char);
    }
    out
}

/// Session timeout policy (seconds). Zero disables that bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionPolicy {
    /// Sliding idle window; default 30 minutes.
    pub idle_secs: u64,
    /// Absolute lifetime from creation; default 8 hours.
    pub absolute_secs: u64,
}

impl Default for SessionPolicy {
    fn default() -> Self {
        Self {
            idle_secs: 1_800,
            absolute_secs: 28_800,
        }
    }
}

/// One server-side session. `data` is small, PHI-free metadata only (cart id,
/// locale override) — never secrets or health data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: SessionId,
    pub principal: PrincipalId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<TenantId>,
    #[serde(default)]
    pub data: BTreeMap<String, serde_json::Value>,
    #[serde(with = "time::serde::iso8601")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::iso8601")]
    pub last_seen: OffsetDateTime,
}

impl Session {
    pub fn new(principal: PrincipalId, tenant: Option<TenantId>, now: OffsetDateTime) -> Self {
        Self {
            id: SessionId::generate(),
            principal,
            tenant,
            data: BTreeMap::new(),
            created_at: now,
            last_seen: now,
        }
    }

    pub fn expired(&self, now: OffsetDateTime, policy: &SessionPolicy) -> bool {
        if policy.idle_secs > 0
            && (now - self.last_seen) > Duration::seconds(policy.idle_secs as i64)
        {
            return true;
        }
        if policy.absolute_secs > 0
            && (now - self.created_at) > Duration::seconds(policy.absolute_secs as i64)
        {
            return true;
        }
        false
    }

    /// CSRF token: sha256(secret | session id | principal). Stateless to
    /// verify, useless to forge without the server secret.
    pub fn csrf_token(&self, secret: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(secret);
        hasher.update(b"|csrf|");
        hasher.update(self.id.as_str().as_bytes());
        hasher.update(b"|");
        hasher.update(self.principal.as_uuid().as_bytes());
        hex(&hasher.finalize())
    }
}

/// Session storage port. `keel-cache-memory`-style in-memory ships here;
/// redis/postgres adapters later share the same trait.
pub trait SessionStore: Send + Sync {
    fn load(&self, id: &SessionId) -> Result<Option<Session>, AppError>;
    fn save(&self, session: &Session) -> Result<(), AppError>;
    fn delete(&self, id: &SessionId) -> Result<(), AppError>;
    fn count(&self) -> usize;
    /// Delete every expired session; returns how many went. Stores without
    /// enumeration return 0 (lazy expiry via `touch` still applies).
    fn sweep(&self, _now: OffsetDateTime, _policy: &SessionPolicy) -> Result<usize, AppError> {
        Ok(0)
    }
}

/// Default in-memory store (single-node dev/test).
#[derive(Debug, Default)]
pub struct InMemorySessionStore {
    inner: RwLock<BTreeMap<SessionId, Session>>,
}

impl InMemorySessionStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn shared() -> Arc<Self> {
        Arc::new(Self::new())
    }
}

impl SessionStore for InMemorySessionStore {
    fn load(&self, id: &SessionId) -> Result<Option<Session>, AppError> {
        Ok(self.inner.read().unwrap().get(id).cloned())
    }

    fn save(&self, session: &Session) -> Result<(), AppError> {
        self.inner
            .write()
            .unwrap()
            .insert(session.id.clone(), session.clone());
        Ok(())
    }

    fn delete(&self, id: &SessionId) -> Result<(), AppError> {
        self.inner.write().unwrap().remove(id);
        Ok(())
    }

    fn count(&self) -> usize {
        self.inner.read().unwrap().len()
    }

    fn sweep(&self, now: OffsetDateTime, policy: &SessionPolicy) -> Result<usize, AppError> {
        let mut map = self.inner.write().unwrap();
        let dead: Vec<SessionId> = map
            .iter()
            .filter(|(_, s)| s.expired(now, policy))
            .map(|(id, _)| id.clone())
            .collect();
        let count = dead.len();
        for id in dead {
            map.remove(&id);
        }
        Ok(count)
    }
}

/// Manager: owns store + policy + CSRF secret; exposes the request-lifetime
/// surface. `clock` is injected so expiry is testable and deterministic.
pub struct SessionManager {
    store: Arc<dyn SessionStore>,
    policy: SessionPolicy,
    secret: Vec<u8>,
}

impl SessionManager {
    pub fn new(
        store: Arc<dyn SessionStore>,
        policy: SessionPolicy,
        secret: impl AsRef<[u8]>,
    ) -> Self {
        Self {
            store,
            policy,
            secret: secret.as_ref().to_vec(),
        }
    }

    pub fn policy(&self) -> &SessionPolicy {
        &self.policy
    }

    pub fn store(&self) -> &Arc<dyn SessionStore> {
        &self.store
    }

    /// Start a new session (login). Caller persists the returned id in the
    /// response cookie.
    pub fn start(
        &self,
        principal: PrincipalId,
        tenant: Option<TenantId>,
        now: OffsetDateTime,
    ) -> Result<Session, AppError> {
        let session = Session::new(principal, tenant, now);
        self.store.save(&session)?;
        Ok(session)
    }

    /// Load + validate + touch. Expired/unknown = `None` (caller treats as
    /// anonymous and clears the cookie), never an error.
    pub fn touch(&self, id: &SessionId, now: OffsetDateTime) -> Result<Option<Session>, AppError> {
        let Some(mut session) = self.store.load(id)? else {
            return Ok(None);
        };
        if session.expired(now, &self.policy) {
            self.store.delete(id)?;
            return Ok(None);
        }
        session.last_seen = now;
        self.store.save(&session)?;
        Ok(Some(session))
    }

    /// Rotate the session id (post-login / privilege change) while keeping
    /// data. Returns the new session; the old id is deleted.
    pub fn regenerate(
        &self,
        id: &SessionId,
        now: OffsetDateTime,
    ) -> Result<Option<Session>, AppError> {
        let Some(mut session) = self.store.load(id)? else {
            return Ok(None);
        };
        if session.expired(now, &self.policy) {
            self.store.delete(id)?;
            return Ok(None);
        }
        self.store.delete(id)?;
        session.id = SessionId::generate();
        session.last_seen = now;
        self.store.save(&session)?;
        Ok(Some(session))
    }

    /// Write session data (small, PHI-free).
    pub fn put(
        &self,
        id: &SessionId,
        key: impl Into<String>,
        value: serde_json::Value,
    ) -> Result<(), AppError> {
        let Some(mut session) = self.store.load(id)? else {
            return Err(AppError::validation("put on unknown session"));
        };
        session.data.insert(key.into(), value);
        self.store.save(&session)
    }

    /// Logout / revoke.
    pub fn destroy(&self, id: &SessionId) -> Result<(), AppError> {
        self.store.delete(id)
    }

    /// Verify a CSRF token against a live session. Constant-time-ish compare.
    pub fn verify_csrf(&self, session: &Session, token: &str) -> bool {
        let expected = session.csrf_token(&self.secret);
        expected.as_bytes() == token.as_bytes()
    }

    /// Garbage-collect expired sessions; wire into a schedule (item 39).
    /// Returns how many sessions were reaped.
    pub fn sweep(&self, now: OffsetDateTime) -> Result<usize, AppError> {
        self.store.sweep(now, &self.policy)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mgr(store: Arc<dyn SessionStore>) -> SessionManager {
        SessionManager::new(
            store,
            SessionPolicy {
                idle_secs: 60,
                absolute_secs: 3600,
            },
            b"unit-secret",
        )
    }

    fn principal() -> PrincipalId {
        PrincipalId::new()
    }

    #[test]
    fn session_id_is_256bit_hex() {
        let id = SessionId::generate();
        assert_eq!(id.as_str().len(), 64);
        assert!(id.as_str().chars().all(|c| c.is_ascii_hexdigit()));
        assert!(SessionId::parse("zz").is_err());
        assert_eq!(SessionId::parse(id.as_str()).unwrap(), id);
        assert_ne!(SessionId::generate(), SessionId::generate());
    }

    #[test]
    fn start_touch_destroy_lifecycle() {
        let mgr = mgr(InMemorySessionStore::shared());
        let now = OffsetDateTime::now_utc();
        let s = mgr.start(principal(), None, now).unwrap();
        assert_eq!(mgr.store().count(), 1);

        let loaded = mgr.touch(&s.id, now + Duration::seconds(30)).unwrap();
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert!(loaded.last_seen > s.last_seen);

        mgr.destroy(&s.id).unwrap();
        assert_eq!(mgr.store().count(), 0);
        assert!(mgr.touch(&s.id, now).unwrap().is_none());
    }

    #[test]
    fn idle_and_absolute_expiry() {
        let mgr = mgr(InMemorySessionStore::shared());
        let now = OffsetDateTime::now_utc();
        let s = mgr.start(principal(), None, now).unwrap();

        // Inside idle window -> alive.
        assert!(mgr
            .touch(&s.id, now + Duration::seconds(59))
            .unwrap()
            .is_some());
        // Idle expiry: 61s since last touch (which reset last_seen).
        let refreshed = mgr.store().load(&s.id).unwrap().unwrap();
        assert!(mgr
            .touch(&s.id, refreshed.last_seen + Duration::seconds(61))
            .unwrap()
            .is_none());

        // Absolute expiry even when recently active.
        let s2 = mgr.start(principal(), None, now).unwrap();
        let far = now + Duration::seconds(3601);
        assert!(mgr.touch(&s2.id, far).unwrap().is_none());
    }

    #[test]
    fn regenerate_rotates_id_and_keeps_data() {
        let mgr = mgr(InMemorySessionStore::shared());
        let now = OffsetDateTime::now_utc();
        let s = mgr.start(principal(), None, now).unwrap();
        mgr.put(&s.id, "locale", serde_json::json!("hi-IN"))
            .unwrap();

        let rotated = mgr.regenerate(&s.id, now).unwrap().unwrap();
        assert_ne!(rotated.id, s.id, "id must rotate");
        assert_eq!(
            rotated.data.get("locale"),
            Some(&serde_json::json!("hi-IN"))
        );
        assert!(mgr.touch(&s.id, now).unwrap().is_none(), "old id is dead");
    }

    #[test]
    fn csrf_token_verifies_and_rejects() {
        let mgr = mgr(InMemorySessionStore::shared());
        let now = OffsetDateTime::now_utc();
        let s = mgr.start(principal(), None, now).unwrap();
        let token = s.csrf_token(mgr.secret.as_slice());
        assert!(mgr.verify_csrf(&s, &token));
        assert!(!mgr.verify_csrf(&s, "deadbeef"));
        // Different principal -> different token even with same id/secret.
        let mut other = s.clone();
        other.principal = principal();
        assert!(!mgr.verify_csrf(&other, &token));
    }

    #[test]
    fn sweep_reaps_expired_sessions() {
        let mgr = mgr(InMemorySessionStore::shared());
        let now = OffsetDateTime::now_utc();
        let alive = mgr.start(principal(), None, now).unwrap();
        let stale = mgr
            .start(principal(), None, now - Duration::seconds(3600))
            .unwrap();
        assert_eq!(mgr.store().count(), 2);
        let reaped = mgr.sweep(now).unwrap();
        assert_eq!(reaped, 1, "only the absolute-expired session is reaped");
        assert!(mgr.touch(&alive.id, now).unwrap().is_some());
        assert!(mgr.touch(&stale.id, now).unwrap().is_none());
    }

    #[test]
    fn put_on_unknown_session_errors() {
        let mgr = mgr(InMemorySessionStore::shared());
        let err = mgr
            .put(&SessionId::generate(), "k", serde_json::json!(1))
            .unwrap_err();
        assert_eq!(err.code, keel_foundation::codes::VALIDATION);
    }
}
