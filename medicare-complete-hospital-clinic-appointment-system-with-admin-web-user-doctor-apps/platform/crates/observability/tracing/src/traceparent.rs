//! W3C Trace Context helpers (pure functions over keel-foundation typed ids):
//! parse/serialize `traceparent` so inbound requests continue an existing
//! trace and outbound calls propagate ours.

use keel_foundation::{SpanId, TraceId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Traceparent {
    pub trace_id: TraceId,
    pub parent_span_id: SpanId,
    pub sampled: bool,
}

fn hex_to_uuid(hex: &str) -> Option<TraceId> {
    if hex.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for i in 0..16 {
        bytes[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(TraceId::from_uuid(uuid::Uuid::from_bytes(bytes)))
}

fn hex_to_span(hex: &str) -> Option<SpanId> {
    if hex.len() != 16 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for i in 0..8 {
        bytes[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(SpanId::from_uuid(uuid::Uuid::from_bytes(bytes)))
}

/// Parse a `traceparent` header: `00-<32 hex>-<16 hex>-<flags>`.
pub fn parse_traceparent(header: &str) -> Option<Traceparent> {
    let mut parts = header.trim().split('-');
    let version = parts.next()?;
    if version != "00" {
        return None;
    }
    let trace_hex = parts.next()?;
    let span_hex = parts.next()?;
    let flags = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let flag_byte = u8::from_str_radix(flags, 16).ok()?;
    // Reject all-zero ids per spec.
    if trace_hex.chars().all(|c| c == '0') || span_hex.chars().all(|c| c == '0') {
        return None;
    }
    Some(Traceparent {
        trace_id: hex_to_uuid(trace_hex)?,
        parent_span_id: hex_to_span(span_hex)?,
        sampled: flag_byte & 0x01 == 0x01,
    })
}

/// Serialize a `traceparent` header for outbound propagation.
pub fn make_traceparent(trace_id: TraceId, span_id: SpanId, sampled: bool) -> String {
    let trace_hex: String = trace_id
        .as_uuid()
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let span_hex: String = span_id.as_uuid().as_bytes()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("00-{trace_hex}-{span_hex}-{}", if sampled { "01" } else { "00" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_header() {
        let trace = TraceId::new();
        let span = SpanId::new();
        let header = make_traceparent(trace, span, true);
        let parsed = parse_traceparent(&header).expect("parse");
        assert_eq!(parsed.trace_id, trace);
        assert!(parsed.sampled);
    }

    #[test]
    fn rejects_malformed() {
        assert!(parse_traceparent("garbage").is_none());
        assert!(parse_traceparent("00-00000000000000000000000000000000-0000000000000000-00").is_none());
        assert!(parse_traceparent("01-abcdef-12-00").is_none());
        assert!(parse_traceparent("00-xyz-12-00").is_none());
    }
}
