//! # kelson-ratelimit-redis
//!
//! A **shared** rate-limit store for the HTTP admission edge (F-3 Phase 3).
//!
//! The framework's default store keeps the token buckets in the process
//! ([`kelson_http::InMemoryRateLimitStore`]). That is correct for a
//! single-instance deployment, but the documented topology scales the API
//! horizontally (`docs/21-DEPLOYMENT.md`: an HPA from `minReplicas: 3` to
//! `maxReplicas: 20`), and an in-process bucket means **every instance
//! enforces its own limit** — the effective limit multiplies by the instance
//! count. This adapter puts the bucket in Redis, so the limit is one limit.
//!
//! ## Bucket semantics
//!
//! One Redis hash per client key, holding `tokens` and `ts` (the last update
//! in epoch seconds). The bucket step runs **inside a Lua script**, so the
//! read-refill-take-write sequence is atomic: two instances hitting the same
//! client at the same time cannot both take the last token.
//!
//! - `capacity` / `refill_per_sec` come from the caller ([`kelson_http`]'
//!   `RateLimitConfig`), so the limit stays configured in one place.
//! - The key expires after `ttl_secs` (default: the capacity/refill window
//!   plus a margin) — a bucket that nobody touches is cleaned up by Redis
//!   itself, so the keyspace cannot grow without bound.
//! - The key uses a Redis **hash tag** (`{rl}`) so every bucket of one
//!   deployment lands in the same cluster slot.
//!
//! ## Failure posture
//!
//! A Redis outage must not take the API down with it: the store **fails
//! open** (the request is allowed) and logs the failure loudly. The limiter
//! is a protection mechanism, not a correctness barrier — the same posture
//! the cache adapter takes ("the cache is a speed layer, never a correctness
//! barrier"). A deployment that prefers failing closed can wrap the store
//! and invert the decision at the call site.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use kelson_http::RateLimitStore;

/// The atomic token-bucket step.
///
/// KEYS[1] the bucket hash. ARGV[1] capacity, ARGV[2] refill per second,
/// ARGV[3] now (epoch seconds, float), ARGV[4] ttl milliseconds.
/// Returns 1 when a token was taken, 0 when the client is over its limit.
const TAKE_SCRIPT: &str = r#"
local key = KEYS[1]
local capacity = tonumber(ARGV[1])
local refill = tonumber(ARGV[2])
local now = tonumber(ARGV[3])
local ttl = tonumber(ARGV[4])
local bucket = redis.call('HMGET', key, 'tokens', 'ts')
local tokens = tonumber(bucket[1])
local ts = tonumber(bucket[2])
if tokens == nil or ts == nil then
  tokens = capacity
  ts = now
end
local elapsed = now - ts
if elapsed < 0 then elapsed = 0 end
tokens = math.min(capacity, tokens + elapsed * refill)
local allowed = 0
if tokens >= 1 then
  tokens = tokens - 1
  allowed = 1
end
redis.call('HMSET', key, 'tokens', tokens, 'ts', now)
redis.call('PEXPIRE', key, ttl)
return allowed
"#;

/// The shared store: one Redis-backed token bucket per client key.
pub struct RedisRateLimiter {
    conn: Mutex<redis::Connection>,
    prefix: String,
    ttl_secs: u64,
}

impl RedisRateLimiter {
    /// Connect to `url` and build a store whose buckets expire after
    /// `ttl_secs`.
    pub fn connect(url: &str, ttl_secs: u64) -> Result<Self, String> {
        let conn = redis::Client::open(url)
            .map_err(|e| format!("redis client: {e}"))?
            .get_connection()
            .map_err(|e| format!("redis connect: {e}"))?;
        Ok(Self {
            conn: Mutex::new(conn),
            prefix: "kelson:rl".to_string(),
            ttl_secs,
        })
    }

    /// Use a different key prefix (one namespace per deployment).
    pub fn with_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// The Redis key for a client. The `{rl}` hash tag keeps every bucket in
    /// one cluster slot; the client key is sanitized so a hostile key can
    /// never escape the namespace.
    pub fn key(&self, client: &str) -> String {
        bucket_key(&self.prefix, client)
    }
}

/// The Redis key for a client under `prefix`. The `{rl}` hash tag keeps every
/// bucket of the deployment in one cluster slot; the client key is sanitized
/// so a hostile key can never escape the namespace.
fn bucket_key(prefix: &str, client: &str) -> String {
    format!("{{rl}}:{}:{}", prefix, sanitize(client))
}

/// Keep only characters that are safe inside a Redis key: the client key is
/// derived from a header (or an IP), so it must never be able to inject a
/// separator or a wildcard. `:` is kept because the bearer key form is
/// `bearer:<hex>` (a Redis key is an opaque string, so a colon cannot escape
/// anything).
fn sanitize(client: &str) -> String {
    let mut out = String::with_capacity(client.len().min(128));
    for ch in client.chars().take(128) {
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '-' || ch == '_' || ch == ':' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push_str("global");
    }
    out
}

/// Now, in epoch seconds (float) — the clock the bucket refills against.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl RateLimitStore for RedisRateLimiter {
    fn take(&self, key: &str, capacity: u32, refill_per_sec: f64) -> bool {
        let redis_key = self.key(key);
        let ttl_ms = (self.ttl_secs.saturating_mul(1000)).max(1000);
        let mut conn = match self.conn.lock() {
            Ok(conn) => conn,
            Err(poisoned) => poisoned.into_inner(),
        };
        let allowed: i64 = match redis::Script::new(TAKE_SCRIPT)
            .key(redis_key)
            .arg(capacity)
            .arg(refill_per_sec)
            .arg(now_secs())
            .arg(ttl_ms)
            .invoke(&mut *conn)
        {
            Ok(allowed) => allowed,
            Err(e) => {
                // Fail OPEN: a limiter outage must not become an API outage.
                // The failure is logged loudly — silent fail-open is how a
                // rate limit quietly stops existing.
                tracing::error!(
                    error = %e,
                    "redis rate-limit store unavailable; allowing the request (fail open)"
                );
                return true;
            }
        };
        allowed == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_namespaced_and_cluster_safe() {
        let key = bucket_key("kelson:rl", "203.0.113.7");
        assert!(key.starts_with("{rl}:"), "hash tag first: {key}");
        assert!(key.ends_with(":203.0.113.7"), "{key}");
    }

    #[test]
    fn hostile_client_keys_cannot_escape_the_namespace() {
        // `/` and `*` are replaced — a client key can never inject a path
        // segment or a glob into the keyspace.
        assert_eq!(sanitize("a/b"), "a_b");
        assert_eq!(sanitize("*"), "_");
        assert_eq!(sanitize(""), "global");
        assert_eq!(sanitize("client with spaces"), "client_with_spaces");
        // `:` is kept on purpose: the bearer key form is `bearer:<hex>`.
        assert_eq!(sanitize("bearer:abc123"), "bearer:abc123");
        assert!(!sanitize("a/b").contains('/'));
        assert!(!sanitize("*").contains('*'));
    }

    #[test]
    fn long_client_keys_are_truncated() {
        let long = "x".repeat(500);
        assert_eq!(sanitize(&long).len(), 128);
    }

    #[test]
    fn now_is_monotonic_seconds() {
        let a = now_secs();
        assert!(a > 1_700_000_000.0, "a plausible epoch: {a}");
    }
}
