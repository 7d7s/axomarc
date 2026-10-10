//! Audit hash-chain primitives (D-74).
//!
//! Chains are anchored per `(tenant, day)` segment: the first row of a
//! segment hashes against the all-zero genesis prev-hash, and every later
//! row hashes `prev_hash || canonical_row`, so tampering with any row
//! breaks every link after it. Legacy pre-chain rows (backfilled NULLs)
//! carry no prev-hash and are skipped by verification.
//!
//! # Canonicalization
//! [`canonical_audit_row`] joins fields with `|`; `detail` is rendered via
//! [`canonical_json`] (sorted object keys, shortest-roundtrip numbers,
//! standard escapes). Verifiers must parse stored detail into a
//! [`serde_json::Value`] before recomputing so hashes stay stable across
//! storage roundtrips (e.g. Postgres jsonb re-emission).

use serde_json::Value;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// All-zero prev-hash anchoring the first row of every chain segment.
pub const GENESIS_HASH: [u8; 32] = [0u8; 32];

fn chain_namespace() -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"kelson.audit.chain.v1")
}

/// Stable identifier for the chain segment covering `tenant` on `day`
/// (`YYYY-MM-DD`); `None` tenant addresses the global chain.
pub fn chain_id_for(tenant: Option<&str>, day: &str) -> Uuid {
    let name = format!("{}::{day}", tenant.unwrap_or("__global__"));
    Uuid::new_v5(&chain_namespace(), name.as_bytes())
}

/// Chain hash for one row: `sha256(prev_hash || canonical)`.
pub fn audit_hash(prev_hash: &[u8; 32], canonical: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(prev_hash);
    hasher.update(canonical.as_bytes());
    hasher.finalize().into()
}

/// Deterministic JSON rendering: sorted object keys, no insignificant
/// whitespace, standard escapes. Independent of serde_json feature flags
/// (e.g. `preserve_order`), so hashes are stable across builds.
pub fn canonical_json(value: &Value) -> String {
    let mut out = String::new();
    write_canonical(value, &mut out);
    out
}

fn write_canonical(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => match serde_json::to_string(s) {
            Ok(quoted) => out.push_str(&quoted),
            Err(_) => out.push_str("\"\""),
        },
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                match serde_json::to_string(key) {
                    Ok(quoted) => out.push_str(&quoted),
                    Err(_) => out.push_str("\"\""),
                }
                out.push(':');
                match map.get(*key) {
                    Some(item) => write_canonical(item, out),
                    None => out.push_str("null"),
                }
            }
            out.push('}');
        }
    }
}

/// Canonical pipe-joined row string that is hashed into the chain. The
/// field order is part of the chain contract; changing it invalidates all
/// previously written chains (see STAGE-SENTINEL-SPEC).
pub fn canonical_audit_row(
    tenant_id: Option<&str>,
    actor: Option<&str>,
    action: &str,
    entity_type: &str,
    entity_id: &str,
    detail: &Value,
    at_iso8601: &str,
) -> String {
    format!(
        "{}|{}|{}|{}|{}|{}|{}",
        tenant_id.unwrap_or(""),
        actor.unwrap_or(""),
        action,
        entity_type,
        entity_id,
        canonical_json(detail),
        at_iso8601,
    )
}

/// Storage-agnostic row projection consumed by [`verify_chain`].
#[derive(Debug, Clone)]
pub struct ChainRow {
    pub row_id: Uuid,
    /// `None` marks a legacy (pre-chain) row: skipped, chain state kept.
    pub prev_hash: Option<[u8; 32]>,
    pub chain_hash: [u8; 32],
    pub canonical: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakReason {
    /// Recomputed hash differs from stored `chain_hash` (row tampered).
    HashMismatch,
    /// Stored `prev_hash` does not equal the previous row's `chain_hash`.
    BrokenLink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainBreak {
    pub position: u64,
    pub row_id: Uuid,
    pub reason: BreakReason,
}

/// Verify an ordered chain segment (oldest first). Returns the number of
/// chained rows verified, or the first divergence. Legacy rows
/// (`prev_hash == None`) are skipped without advancing chain state.
pub fn verify_chain(rows: &[ChainRow]) -> Result<u64, ChainBreak> {
    let mut expected_prev = GENESIS_HASH;
    let mut chained = 0u64;
    for (position, row) in rows.iter().enumerate() {
        let Some(prev_hash) = row.prev_hash else {
            continue;
        };
        let position = position as u64;
        if prev_hash != expected_prev {
            return Err(ChainBreak {
                position,
                row_id: row.row_id,
                reason: BreakReason::BrokenLink,
            });
        }
        let computed = audit_hash(&prev_hash, &row.canonical);
        if computed != row.chain_hash {
            return Err(ChainBreak {
                position,
                row_id: row.row_id,
                reason: BreakReason::HashMismatch,
            });
        }
        expected_prev = row.chain_hash;
        chained += 1;
    }
    Ok(chained)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn link(prev: [u8; 32], canonical: &str) -> ChainRow {
        ChainRow {
            row_id: Uuid::now_v7(),
            prev_hash: Some(prev),
            chain_hash: audit_hash(&prev, canonical),
            canonical: canonical.to_string(),
        }
    }

    #[test]
    fn chain_verifies_clean() {
        let first = link(GENESIS_HASH, "one");
        let second = link(first.chain_hash, "two");
        let third = link(second.chain_hash, "three");
        assert_eq!(verify_chain(&[first, second, third]), Ok(3));
    }

    #[test]
    fn tampered_row_detected() {
        let first = link(GENESIS_HASH, "one");
        let mut second = link(first.chain_hash, "two");
        second.canonical = "evil".to_string();
        let third = link(second.chain_hash, "three");
        let err = verify_chain(&[first, second, third]).unwrap_err();
        assert_eq!(err.position, 1);
        assert_eq!(err.reason, BreakReason::HashMismatch);
    }

    #[test]
    fn broken_link_detected() {
        let first = link(GENESIS_HASH, "one");
        let second = link(first.chain_hash, "two");
        let mut third = link(second.chain_hash, "three");
        third.prev_hash = Some([0xAB; 32]);
        let err = verify_chain(&[first, second, third]).unwrap_err();
        assert_eq!(err.position, 2);
        assert_eq!(err.reason, BreakReason::BrokenLink);
    }

    #[test]
    fn legacy_rows_skipped() {
        let legacy = ChainRow {
            row_id: Uuid::now_v7(),
            prev_hash: None,
            chain_hash: [0u8; 32],
            canonical: "old row without chain".to_string(),
        };
        let first = link(GENESIS_HASH, "one");
        assert_eq!(verify_chain(&[legacy, first]), Ok(1));
    }

    #[test]
    fn canonical_json_sorts_keys() {
        let value = json!({"b": 1, "a": {"d": true, "c": null}, "list": ["x", 2]});
        assert_eq!(
            canonical_json(&value),
            r#"{"a":{"c":null,"d":true},"b":1,"list":["x",2]}"#
        );
    }

    #[test]
    fn canonical_row_shape() {
        let row = canonical_audit_row(
            Some("t1"),
            None,
            "patient.create",
            "patient",
            "p-1",
            &json!({"b": 2, "a": 1}),
            "2026-08-23T10:00:00Z",
        );
        assert_eq!(
            row,
            "t1||patient.create|patient|p-1|{\"a\":1,\"b\":2}|2026-08-23T10:00:00Z"
        );
    }

    #[test]
    fn chain_ids_partitioned_and_stable() {
        let a = chain_id_for(Some("tenant-a"), "2026-08-23");
        let b = chain_id_for(Some("tenant-b"), "2026-08-23");
        let next_day = chain_id_for(Some("tenant-a"), "2026-08-24");
        assert_eq!(a, chain_id_for(Some("tenant-a"), "2026-08-23"));
        assert_ne!(a, b);
        assert_ne!(a, next_day);
    }
}
