//! **F-23**: the egress policy — what the outbound client may reach.
//!
//! The default is **permissive** ✓: no deployment changes behaviour ✓ and
//! the default path costs nothing ✓ (no DNS, no check ✓). A deployment
//! **opts in** to the refusal ✓ — the reachable path is a URL an untrusted
//! party supplies ✓ (the webhook adapter calls a tenant-registered target ✓).
//!
//! The check resolves the target and refuses a **private** address ✓ —
//! loopback ✓, RFC1918 ✓, link-local (the cloud metadata service lives
//! there ✓), the IPv6 unique-local range ✓ and the unspecified/broadcast
//! addresses ✓. A host in the operator's allowlist is always reachable ✓.
//!
//! The refusal is an `EXTERNAL` error whose **public** message names the
//! policy and never the resolved address ✓; the detail stays internal ✓.

use std::net::IpAddr;

use kelson_foundation::{AppError, ErrorCategory, Retryability, Severity};
use serde::{Deserialize, Serialize};

/// What the client may reach.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EgressPolicy {
    /// Refuse a target that resolves to a private/loopback/link-local
    /// address. Default **false** — an existing deployment changes nothing ✓.
    #[serde(default)]
    pub refuse_private_targets: bool,
    /// Hosts that are always allowed, whatever they resolve to: an exact
    /// host (`api.internal`) or a dotted suffix (`.internal.example`).
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
}

impl EgressPolicy {
    /// Is this host explicitly allowed? An exact match or a dotted suffix
    /// (`.internal.example` covers `a.internal.example` and
    /// `internal.example`).
    pub fn allows_host(&self, host: &str) -> bool {
        let host = host.trim().to_ascii_lowercase();
        self.allowed_hosts.iter().any(|entry| {
            let entry = entry.trim().trim_start_matches('.').to_ascii_lowercase();
            !entry.is_empty() && (entry == host || host.ends_with(&format!(".{entry}")))
        })
    }

    /// Is this address one the policy refuses? Loopback, RFC1918,
    /// link-local (the metadata service lives there), the IPv6 unique-local
    /// range, and the unspecified/broadcast addresses.
    pub fn refuses_address(ip: &IpAddr) -> bool {
        match ip {
            IpAddr::V4(v4) => {
                v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.is_broadcast()
                    // Carrier-grade NAT (100.64.0.0/10) is not RFC1918 but is
                    // just as unroutable from the outside.
                    || (v4.octets()[0] == 100 && (64..=127).contains(&v4.octets()[1]))
            }
            IpAddr::V6(v6) => {
                v6.is_loopback()
                    || v6.is_unique_local()
                    || v6.is_unicast_link_local()
                    || v6.is_unspecified()
                    // An IPv4-mapped address is the IPv4 address in disguise.
                    || v6
                        .to_ipv4_mapped()
                        .is_some_and(|v4| EgressPolicy::refuses_address(&IpAddr::V4(v4)))
            }
        }
    }

    /// The refusal: an `EXTERNAL` error whose public message names the
    /// policy (never the address), with the detail kept internal.
    pub fn refusal(host: &str, detail: &str) -> AppError {
        AppError::new(
            kelson_foundation::codes::EXTERNAL,
            ErrorCategory::External,
            Severity::Medium,
            Retryability::NotRetryable,
            "http egress policy refused the target",
        )
        .with_internal(format!("egress refused {host} ({detail})"))
    }
}

/// Resolve a host to its addresses. DNS is blocking, so it runs on a
/// blocking thread.
pub(crate) async fn resolve_host(host: &str, port: u16) -> Result<Vec<IpAddr>, AppError> {
    let host = host.to_string();
    tokio::task::spawn_blocking(move || {
        use std::net::ToSocketAddrs;
        (host.as_str(), port)
            .to_socket_addrs()
            .map(|addrs| addrs.map(|a| a.ip()).collect::<Vec<_>>())
    })
    .await
    .map_err(|e| AppError::external("http", format!("egress resolve failed: {e}")))?
    .map_err(|e| AppError::external("http", format!("egress resolve failed: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_private_ranges_are_refused() {
        for ip in [
            "127.0.0.1",
            "10.0.0.5",
            "172.16.4.4",
            "192.168.1.1",
            "169.254.169.254", // the cloud metadata service
            "0.0.0.0",
            "100.64.0.1", // carrier-grade NAT
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
        ] {
            assert!(
                EgressPolicy::refuses_address(&ip.parse().unwrap()),
                "{ip} must be refused"
            );
        }
        for ip in ["93.184.216.34", "8.8.8.8", "2606:2800:220:1:248:1893:25c8:1946"] {
            assert!(
                !EgressPolicy::refuses_address(&ip.parse().unwrap()),
                "{ip} must be reachable"
            );
        }
    }

    #[test]
    fn the_allowlist_matches_exactly_or_by_suffix() {
        let policy = EgressPolicy {
            refuse_private_targets: true,
            allowed_hosts: vec![".internal.example".into(), "api.local".into()],
        };
        assert!(policy.allows_host("internal.example"));
        assert!(policy.allows_host("a.internal.example"));
        assert!(policy.allows_host("api.local"));
        assert!(!policy.allows_host("evil-internal.example"));
        assert!(!policy.allows_host("internal.example.evil.com"));
    }
}
