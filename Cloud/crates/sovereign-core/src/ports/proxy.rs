// The proxy port. Implemented by `sovereign-proxy-caddy` (Caddy's
// admin API on `:2019`); future adapters (Nginx, Envoy, Traefik) plug
// in here without use-case changes.
//
// Per `docs/phase-00-mvp.md` §F6, the proxy owns: hostname -> upstream
// routing, TLS termination (V0: Caddy's `tls internal`; V1: ACME), and
// hot reload. The proxy MUST be a pure port — it does not know about
// storage, audit, or the container runtime. A use case composes the
// proxy with storage and runtime.
//
// All methods return `AppError::Upstream` on transport/HTTP errors and
// `AppError::Validation` on bad inputs. The HTTP layer maps
// `AppError::Upstream` to 502 and `Validation` to 400.

use async_trait::async_trait;

use crate::error::AppError;

/// The hostname scheme Sovereign issues for V0. We do not own a public
/// wildcard DNS zone in V0, so deployments get a `.sovereign.local`
/// hostname that the operator resolves via `/etc/hosts` (or
/// `curl --resolve`). V1 will switch this to a real wildcard once
/// ACME DNS-01 is wired.
pub const V0_DEFAULT_HOST_SUFFIX: &str = "sovereign.local";

/// The proxy port. The proxy is the only component that knows the
/// outside-public hostname; the use cases only know the app identity.
#[async_trait]
pub trait ProxyPort: Send + Sync {
    /// Add (or replace) a route from `host` to a single upstream TCP
    /// port on the loopback. The proxy terminates TLS and reverse-
    /// proxies plaintext to `127.0.0.1:<upstream_port>`. Idempotent:
    /// calling twice with the same `host` replaces the existing route.
    ///
    /// `host` MUST be a fully-qualified DNS name (no scheme, no path).
    /// The proxy adapter is responsible for any per-hostname policy
    /// (e.g. lowercase, IDNA encode).
    async fn add_route(&self, host: &str, upstream_port: u16) -> Result<(), AppError>;

    /// Remove the route for `host`. Idempotent: removing a non-existent
    /// route returns `Ok(())` so callers do not need a "does it exist"
    /// pre-check.
    async fn remove_route(&self, host: &str) -> Result<(), AppError>;

    /// Force the proxy to reload its config from disk. V0 implementations
    /// may no-op (Caddy's admin-API mutations are already hot-applied);
    /// V1 uses this for fail-over to a fresh Caddy.
    async fn reload(&self) -> Result<(), AppError>;
}

/// Build the default V0 hostname for an app id. Pure function; no I/O.
/// The form is `<short-id>.sovereign.local` where the short id is the
/// first 8 hex chars of the app's UUID — readable in terminal output
/// (`https://abc12345.sovereign.local`) while remaining unguessable
/// for the duration of a deploy.
///
/// `app_id` is a UUID string in canonical 8-4-4-4-12 form. We do not
/// validate the format here — bad input is a `Validation` at the use-
/// case boundary.
pub fn default_v0_host(app_id: &str) -> String {
    let short = app_id.split('-').next().unwrap_or(app_id);
    let short = &short[..short.len().min(8)];
    format!("{short}.{V0_DEFAULT_HOST_SUFFIX}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_v0_host_truncates_uuid_to_8_chars() {
        let id = "abc12345-1234-1234-1234-123456789012";
        assert_eq!(default_v0_host(id), "abc12345.sovereign.local");
    }

    #[test]
    fn default_v0_host_keeps_short_input_intact() {
        let id = "abc";
        assert_eq!(default_v0_host(id), "abc.sovereign.local");
    }

    #[test]
    fn default_v0_host_handles_no_dash() {
        let id = "0123456789abcdef";
        assert_eq!(default_v0_host(id), "01234567.sovereign.local");
    }
}
