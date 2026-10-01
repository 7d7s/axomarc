//! Field casts (Wave 3 Phase 5): wire <-> storage transforms declared per
//! field with [`crate::FieldDef::cast`].
//!
//! Built-ins cover the parity-plan registry kinds — `date`, `datetime`,
//! `decimal:<n>`, `json`, `array`, `enum:<a,b>` — plus the two security
//! kinds: `encrypted` (seals through the foundation [`Sealer`] port; the
//! AES-256-GCM implementation ships as `keel_tokens::AesGcmSealer`) and
//! `hashed` (argon2id via `keel_identity::hash_password`, never
//! round-trips). Modules register custom [`Cast`] implementations on the
//! runtime with `ModelRuntime::with_cast`.
//!
//! Validation runs at SET time (the mass-assignment pipeline calls
//! [`Cast::to_db`]); reads pass stored values through unchanged so stored
//! shapes keep round-tripping, except `encrypted`, which decrypts back to
//! plaintext. base64 here is hand-rolled on purpose — the platform ships no
//! base64 dependency (same posture as `application/configuration::keys` and
//! `web::http::jwt`).

use keel_foundation::{in_tz, timezone_offset_secs, AppError, Envelope, Sealer, Secret};
use serde_json::Value as Json;
use std::sync::Arc;

use crate::runtime::parse_timestamp;

/// One field-level value transform: wire JSON -> stored JSON ([`Cast::to_db`])
/// and stored JSON -> wire JSON ([`Cast::from_db`]). Implementations must be
/// deterministic on the read path and may validate/normalize on the write
/// path (errors surface as `KER_VALIDATION` from the prepare boundary).
pub trait Cast: Send + Sync {
    /// Transform a validated wire value into the value stored at rest.
    fn to_db(&self, wire: &Json) -> Result<Json, AppError>;

    /// Transform a stored value back into its wire representation.
    // Name mirrors `to_db`; the read side is a method on purpose.
    #[allow(clippy::wrong_self_convention)]
    fn from_db(&self, stored: &Json) -> Result<Json, AppError>;
}

// ------------------------------------------------------------- built-ins

struct DateCast;
struct DatetimeCast {
    /// Wave 8 P2 (docs/19): optional IANA zone — reads localize the stored
    /// UTC instant to the declared zone's wall clock (the timezone helper
    /// feeding model casts); bare `datetime` stays UTC pass-through.
    tz: Option<String>,
}
struct JsonCast;
struct ArrayCast;
struct HashedCast;

struct DecimalCast {
    scale: u8,
}

struct EnumCast {
    variants: Vec<String>,
}

/// `encrypted`: seal plaintext strings at rest, open them on read. Holds the
/// foundation encryption port — construction goes through
/// `ModelRuntime::with_sealer`.
pub struct EncryptedCast {
    sealer: Arc<dyn Sealer>,
    key_id: String,
}

impl EncryptedCast {
    /// Bind a sealer; envelopes record `app` as the key identity (rotation
    /// stays the sealer's business).
    pub fn new(sealer: Arc<dyn Sealer>) -> Self {
        Self {
            sealer,
            key_id: "app".into(),
        }
    }

    /// Override the recorded key identity.
    pub fn with_key_id(mut self, key_id: impl Into<String>) -> Self {
        self.key_id = key_id.into();
        self
    }
}

fn require_string<'a>(value: &'a Json, what: &str) -> Result<&'a str, AppError> {
    value
        .as_str()
        .ok_or_else(|| AppError::validation(format!("{what} expects a string value")))
}

impl Cast for DateCast {
    fn to_db(&self, wire: &Json) -> Result<Json, AppError> {
        let raw = require_string(wire, "date cast")?;
        // Strict canonical shape + calendar validity: 2024-02-30 is refused.
        let bytes = raw.as_bytes();
        if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
            return Err(AppError::validation(format!(
                "date cast expects YYYY-MM-DD, got {raw:?}"
            )));
        }
        let (year, month, day) = (
            calendar_part(&raw[0..4], raw)?,
            calendar_part(&raw[5..7], raw)?,
            calendar_part(&raw[8..10], raw)?,
        );
        let month = u8::try_from(month).map_err(|_| date_err(raw))?;
        let month = time::Month::try_from(month).map_err(|_| date_err(raw))?;
        let day = u8::try_from(day).map_err(|_| date_err(raw))?;
        time::Date::from_calendar_date(year, month, day).map_err(|_| date_err(raw))?;
        Ok(Json::String(raw.to_string()))
    }

    fn from_db(&self, stored: &Json) -> Result<Json, AppError> {
        Ok(stored.clone())
    }
}

fn date_err(raw: &str) -> AppError {
    AppError::validation(format!("date {raw:?} is not a valid calendar date"))
}

fn calendar_part(text: &str, raw: &str) -> Result<i32, AppError> {
    text.parse::<i32>().map_err(|_| date_err(raw))
}

impl Cast for DatetimeCast {
    fn to_db(&self, wire: &Json) -> Result<Json, AppError> {
        let raw = require_string(wire, "datetime cast")?;
        let dt = parse_timestamp(raw)?;
        // The zoned variant stores a UTC instant: normalize the arrival
        // offset to `Z` (the bare cast's offset-preserving write is
        // unchanged — non-regression).
        let dt = if self.tz.is_some() {
            dt.to_offset(time::UtcOffset::UTC)
        } else {
            dt
        };
        dt.format(&time::format_description::well_known::Rfc3339)
            .map(Json::String)
            .map_err(|e| AppError::validation(format!("timestamp {raw:?} is not canonical: {e}")))
    }

    fn from_db(&self, stored: &Json) -> Result<Json, AppError> {
        let Some(tz) = self.tz.as_deref() else {
            return Ok(stored.clone());
        };
        let raw = require_string(stored, "datetime cast")?;
        let dt = parse_timestamp(raw)?;
        // Construction validated the zone against the bundled table, but
        // fail closed anyway: an unknown zone never renders a wrong reading.
        let local = in_tz(dt, tz).ok_or_else(|| {
            AppError::internal(format!("datetime cast zone {tz:?} left the bundled table"))
        })?;
        local
            .format(&time::format_description::well_known::Rfc3339)
            .map(Json::String)
            .map_err(|e| AppError::validation(format!("timestamp {raw:?} is not canonical: {e}")))
    }
}

/// Round an f64 to `scale` decimals through the canonical text form (avoids
/// binary float artifacts like 0.1 * 10 != 1.0).
fn round_decimal(value: f64, scale: u8) -> Result<Json, AppError> {
    let text = format!("{:.*}", scale as usize, value);
    let rounded = text.parse::<f64>().map_err(|_| {
        AppError::validation(format!("decimal value {value} cannot be scaled to {scale} digits"))
    })?;
    serde_json::Number::from_f64(rounded)
        .map(Json::Number)
        .ok_or_else(|| AppError::validation(format!("decimal value {value} is not finite")))
}

impl Cast for DecimalCast {
    fn to_db(&self, wire: &Json) -> Result<Json, AppError> {
        match wire {
            // Numeric fields arrive as numbers (json_to_bind already
            // enforced the column type): round in place.
            Json::Number(_) => {
                let value = wire.as_f64().ok_or_else(|| {
                    AppError::validation("decimal cast expects a finite number")
                })?;
                round_decimal(value, self.scale)
            }
            // Text columns arrive as strings: validate and pin the scale.
            Json::String(raw) => {
                let value = raw.trim().parse::<f64>().map_err(|_| {
                    AppError::validation(format!("decimal cast expects a numeric string, got {raw:?}"))
                })?;
                Ok(Json::String(format!("{:.*}", self.scale as usize, value)))
            }
            _ => Err(AppError::validation(
                "decimal cast expects a number or numeric string",
            )),
        }
    }

    fn from_db(&self, stored: &Json) -> Result<Json, AppError> {
        Ok(stored.clone())
    }
}

impl Cast for JsonCast {
    fn to_db(&self, wire: &Json) -> Result<Json, AppError> {
        if wire.is_object() || wire.is_array() {
            Ok(wire.clone())
        } else {
            Err(AppError::validation(
                "json cast expects an object or array value",
            ))
        }
    }

    fn from_db(&self, stored: &Json) -> Result<Json, AppError> {
        Ok(stored.clone())
    }
}

impl Cast for ArrayCast {
    fn to_db(&self, wire: &Json) -> Result<Json, AppError> {
        if wire.is_array() {
            Ok(wire.clone())
        } else {
            Err(AppError::validation("array cast expects an array value"))
        }
    }

    fn from_db(&self, stored: &Json) -> Result<Json, AppError> {
        Ok(stored.clone())
    }
}

impl Cast for EnumCast {
    fn to_db(&self, wire: &Json) -> Result<Json, AppError> {
        let raw = require_string(wire, "enum cast")?;
        if self.variants.iter().any(|v| v == raw) {
            Ok(wire.clone())
        } else {
            Err(AppError::validation(format!(
                "enum cast value {raw:?} is not one of {}",
                self.variants.join(", ")
            )))
        }
    }

    fn from_db(&self, stored: &Json) -> Result<Json, AppError> {
        Ok(stored.clone())
    }
}

impl Cast for HashedCast {
    fn to_db(&self, wire: &Json) -> Result<Json, AppError> {
        let plain = require_string(wire, "hashed cast")?;
        // keel-identity carries its own `Secret` wrapper (argon2 PHC at rest).
        let hash = keel_identity::hash_password(&keel_identity::Secret::new(plain))?;
        Ok(Json::String(hash))
    }

    fn from_db(&self, stored: &Json) -> Result<Json, AppError> {
        // Never round-trips: the stored PHC string is what the wire carries.
        Ok(stored.clone())
    }
}

impl Cast for EncryptedCast {
    fn to_db(&self, wire: &Json) -> Result<Json, AppError> {
        let plain = require_string(wire, "encrypted cast")?;
        let envelope = self.sealer.seal(&self.key_id, &Secret::new(plain))?;
        let bytes = serde_json::to_vec(&envelope).map_err(|e| {
            AppError::internal(format!("sealed envelope could not be encoded: {e}"))
        })?;
        Ok(Json::String(base64_encode(&bytes)))
    }

    fn from_db(&self, stored: &Json) -> Result<Json, AppError> {
        let blob = require_string(stored, "encrypted cast")?;
        let bytes = base64_decode(blob)?;
        let envelope: Envelope = serde_json::from_slice(&bytes)
            .map_err(|e| AppError::validation(format!("sealed envelope is malformed: {e}")))?;
        Ok(Json::String(self.sealer.open(&envelope)?.into_string()))
    }
}

// ---------------------------------------------------------------- registry

/// Cast specs that are built into the framework (parameters excluded).
const BUILTIN_NAMES: &[&str] = &["date", "datetime", "json", "array", "hashed", "encrypted"];

/// Resolve a builtin cast spec to its implementation. `Ok(None)` never —
/// unknown specs are errors (customs resolve through the runtime registry
/// first), and `encrypted` without a bound sealer fails with the wiring
/// hint rather than silently storing plaintext.
pub(crate) fn builtin_cast(spec: &str) -> Result<Box<dyn Cast>, AppError> {
    if let Some(rest) = spec.strip_prefix("decimal:") {
        return Ok(Box::new(DecimalCast {
            scale: parse_scale(rest)?,
        }));
    }
    if let Some(rest) = spec.strip_prefix("enum:") {
        return Ok(Box::new(EnumCast {
            variants: parse_variants(rest)?,
        }));
    }
    if let Some(rest) = spec.strip_prefix("datetime:") {
        datetime_tz_or_err(rest)?;
        return Ok(Box::new(DatetimeCast {
            tz: Some(rest.to_string()),
        }));
    }
    match spec {
        "date" => Ok(Box::new(DateCast)),
        "datetime" => Ok(Box::new(DatetimeCast { tz: None })),
        "json" => Ok(Box::new(JsonCast)),
        "array" => Ok(Box::new(ArrayCast)),
        "hashed" => Ok(Box::new(HashedCast)),
        "encrypted" => Err(AppError::validation(
            "cast \"encrypted\" needs a sealing key — bind one with ModelRuntime::with_sealer \
             (foundation Sealer port; AES-256-GCM ships as keel-tokens::AesGcmSealer)",
        )),
        _ => Err(AppError::validation(format!(
            "unknown cast {spec:?} — register it with ModelRuntime::with_cast"
        ))),
    }
}

fn parse_scale(rest: &str) -> Result<u8, AppError> {
    let scale = rest.parse::<u8>().map_err(|_| scale_err(rest))?;
    if scale > 18 {
        return Err(scale_err(rest));
    }
    Ok(scale)
}

/// `datetime:<tz>` registration-time guard: the zone must be in the
/// bundled IANA table (same fail-closed posture as a schedule item's
/// zone), so a typo refuses registration instead of storing UTC readings.
fn datetime_tz_or_err(rest: &str) -> Result<(), AppError> {
    if timezone_offset_secs(rest).is_some() {
        Ok(())
    } else {
        Err(AppError::validation(format!(
            "datetime cast timezone {rest:?} is not in the bundled IANA table \
             (fixed offsets; unknown zones refuse registration)"
        )))
    }
}

fn scale_err(rest: &str) -> AppError {
    AppError::validation(format!(
        "decimal cast scale {rest:?} must be an integer 0..=18"
    ))
}

fn parse_variants(rest: &str) -> Result<Vec<String>, AppError> {
    let variants: Vec<String> = rest.split(',').map(str::trim).map(str::to_string).collect();
    if variants.iter().any(|v| v.is_empty()) {
        return Err(AppError::validation(format!(
            "enum cast {rest:?} needs a comma-separated variant list"
        )));
    }
    Ok(variants)
}

/// Register-time spec validation (called from `ModelRegistry::validate_model`):
/// builtin specs must carry well-formed parameters, customs must be
/// identifier-shaped. Typos of builtin names pass here as customs and fail
/// at first write with the unknown-cast error.
pub(crate) fn validate_cast_spec(spec: &str) -> Result<(), AppError> {
    if spec.is_empty() {
        return Err(AppError::validation("cast spec must not be empty"));
    }
    if let Some(rest) = spec.strip_prefix("decimal:") {
        return parse_scale(rest).map(|_| ());
    }
    if let Some(rest) = spec.strip_prefix("enum:") {
        return parse_variants(rest).map(|_| ());
    }
    if let Some(rest) = spec.strip_prefix("datetime:") {
        return datetime_tz_or_err(rest);
    }
    if spec == "decimal" {
        return Err(AppError::validation("decimal cast needs a scale: decimal:<n>"));
    }
    if spec == "enum" {
        return Err(AppError::validation("enum cast needs variants: enum:<a,b,c>"));
    }
    if BUILTIN_NAMES.contains(&spec) {
        return Ok(());
    }
    crate::validate_ident(spec, "cast")
}

// ------------------------------------------------------------------ base64

/// RFC 4648 §4 base64 WITH padding — hand-rolled (no base64 dependency,
/// platform posture).
fn base64_encode(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((triple >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(triple & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

fn base64_decode(text: &str) -> Result<Vec<u8>, AppError> {
    fn sextet(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some((byte - b'A') as u32),
            b'a'..=b'z' => Some((byte - b'a' + 26) as u32),
            b'0'..=b'9' => Some((byte - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::new();
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for &byte in text.as_bytes() {
        if byte == b'=' {
            break;
        }
        let value = sextet(byte).ok_or_else(|| {
            AppError::validation("encrypted cast payload is not valid base64")
        })?;
        buf = (buf << 6) | value;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xFF) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use keel_tokens::{AesGcmSealer, KEY_LEN};

    fn builtin(spec: &str) -> Box<dyn Cast> {
        builtin_cast(spec).unwrap_or_else(|e| panic!("builtin {spec:?}: {}", e.public_message))
    }

    #[test]
    fn date_and_datetime_validate_at_set_time() {
        let date = builtin("date");
        assert_eq!(
            date.to_db(&Json::String("2024-01-02".into())).unwrap(),
            Json::String("2024-01-02".into())
        );
        let feb30 = date
            .to_db(&Json::String("2024-02-30".into()))
            .unwrap_err();
        assert!(feb30.public_message.contains("valid calendar date"));
        assert!(date.to_db(&Json::String("2024-1-2".into())).is_err());

        let datetime = builtin("datetime");
        let canonical = datetime
            .to_db(&Json::String("2024-01-02T03:04:05Z".into()))
            .unwrap();
        assert_eq!(canonical, Json::String("2024-01-02T03:04:05Z".into()));
        assert!(datetime
            .to_db(&Json::String("not-a-timestamp".into()))
            .is_err());
        assert!(datetime.to_db(&Json::Number(7.into())).is_err());
    }

    #[test]
    fn datetime_tz_cast_localizes_reads_and_fails_closed_off_the_table() {
        let cast = builtin("datetime:Asia/Kolkata");
        // Writes stay canonical UTC instants, whatever offset arrives.
        let canonical = cast
            .to_db(&Json::String("2024-01-02T08:34:05+05:30".into()))
            .unwrap();
        assert_eq!(canonical, Json::String("2024-01-02T03:04:05Z".into()));
        // Reads render the declared zone's wall clock with its offset —
        // the timezone helper feeding the cast (Wave 8 P2).
        let local = cast
            .from_db(&Json::String("2024-01-02T03:04:05Z".into()))
            .unwrap();
        assert_eq!(local, Json::String("2024-01-02T08:34:05+05:30".into()));
        // Malformed stored values fail closed.
        assert!(cast.from_db(&Json::String("not-a-timestamp".into())).is_err());

        // The bare datetime cast keeps its UTC pass-through.
        let plain = builtin("datetime");
        assert_eq!(
            plain.from_db(&Json::String("2024-01-02T03:04:05Z".into())).unwrap(),
            Json::String("2024-01-02T03:04:05Z".into())
        );

        // Zones outside the bundled table refuse at construction AND at
        // registration — a typo never silently stores UTC readings.
        assert!(builtin_cast("datetime:Mars/Olympus_Mons").is_err());
        assert!(builtin_cast("datetime:").is_err());
        assert!(validate_cast_spec("datetime:Mars/Olympus_Mons").is_err());
        assert!(validate_cast_spec("datetime:").is_err());
        assert!(validate_cast_spec("datetime:Asia/Kolkata").is_ok());
        assert!(validate_cast_spec("datetime").is_ok());
    }

    #[test]
    fn decimal_scale_and_enum_validate_at_set_time() {
        let money = builtin("decimal:2");
        assert_eq!(
            money
                .to_db(&Json::Number(serde_json::Number::from_f64(12.3456).unwrap()))
                .unwrap(),
            Json::Number(serde_json::Number::from_f64(12.35).unwrap())
        );
        assert_eq!(
            money.to_db(&Json::String("7".into())).unwrap(),
            Json::String("7.00".into())
        );
        assert!(money.to_db(&Json::String("abc".into())).is_err());

        let kind = builtin("enum:paid,pending");
        assert_eq!(
            kind.to_db(&Json::String("paid".into())).unwrap(),
            Json::String("paid".into())
        );
        let refused = kind.to_db(&Json::String("refunded".into())).unwrap_err();
        assert!(refused.public_message.contains("paid, pending"));
    }

    #[test]
    fn json_and_array_casts_require_containers() {
        let json = builtin("json");
        assert!(json.to_db(&Json::Object(serde_json::Map::new())).is_ok());
        assert!(json.to_db(&Json::Array(vec![])).is_ok());
        assert!(json.to_db(&Json::String("scalar".into())).is_err());

        let array = builtin("array");
        assert!(array.to_db(&Json::Array(vec![])).is_ok());
        assert!(array
            .to_db(&Json::Object(serde_json::Map::new()))
            .is_err());
    }

    #[test]
    fn encrypted_cast_round_trips_through_real_aes_gcm() {
        let sealer: Arc<dyn Sealer> = Arc::new(AesGcmSealer::new([7u8; KEY_LEN]));
        let cast = EncryptedCast::new(sealer);
        let sealed = cast.to_db(&Json::String("hush".into())).unwrap();
        let blob = sealed.as_str().expect("seal stores base64 text");
        assert_ne!(blob, "hush");
        // The payload really decodes to an AES-256-GCM envelope.
        let envelope: Envelope =
            serde_json::from_slice(&base64_decode(blob).unwrap()).unwrap();
        assert_eq!(envelope.alg, "AES-256-GCM");
        assert_eq!(cast.from_db(&sealed).unwrap(), Json::String("hush".into()));

        // Tampering with the ciphertext fails closed (auth tag mismatch).
        // Flip a MIDDLE character: the last base64 char carries unused
        // padding bits that a decoder legitimately ignores.
        let mut chars: Vec<char> = blob.chars().collect();
        let mid = chars.len() / 2;
        chars[mid] = if chars[mid] == 'A' { 'B' } else { 'A' };
        let tampered = Json::String(chars.into_iter().collect());
        assert!(cast.from_db(&tampered).is_err());

        // A different master key cannot open the envelope.
        let other: Arc<dyn Sealer> = Arc::new(AesGcmSealer::new([8u8; KEY_LEN]));
        assert!(EncryptedCast::new(other).from_db(&sealed).is_err());
    }

    #[test]
    fn hashed_cast_uses_argon2_and_passes_reads_through() {
        let cast = builtin("hashed");
        let first = cast.to_db(&Json::String("hunter2".into())).unwrap();
        let second = cast.to_db(&Json::String("hunter2".into())).unwrap();
        let hash = first.as_str().unwrap();
        assert!(hash.starts_with("$argon2"), "got {hash:?}");
        assert_ne!(first, Json::String("hunter2".into()));
        assert_ne!(first, second, "fresh salt per write");
        // Reads never re-hash and never reveal the plaintext.
        assert_eq!(cast.from_db(&first).unwrap(), first);
    }

    #[test]
    fn encrypted_builtin_without_a_sealer_fails_with_the_wiring_hint() {
        let err = builtin_cast("encrypted")
            .err()
            .expect("encrypted without a bound sealer must fail");
        assert!(err.public_message.contains("with_sealer"));
        assert!(err.public_message.contains("keel-tokens"));
    }

    #[test]
    fn validate_cast_spec_rejects_malformed_specs() {
        assert!(validate_cast_spec("").is_err());
        assert!(validate_cast_spec("decimal").is_err());
        assert!(validate_cast_spec("decimal:x").is_err());
        assert!(validate_cast_spec("decimal:19").is_err());
        assert!(validate_cast_spec("enum").is_err());
        assert!(validate_cast_spec("enum:").is_err());
        assert!(validate_cast_spec("enum:a,,b").is_err());
        assert!(validate_cast_spec("date:").is_err());
        assert!(validate_cast_spec("money-v2").is_err());
        assert!(validate_cast_spec("decimal:2").is_ok());
        assert!(validate_cast_spec("enum:a,b").is_ok());
        assert!(validate_cast_spec("datetime").is_ok());
        assert!(validate_cast_spec("encrypted").is_ok());
        assert!(validate_cast_spec("money_v2").is_ok());
    }

    #[test]
    fn base64_round_trips_binary_payloads() {
        for bytes in [
            vec![],
            vec![0],
            vec![0, 1],
            vec![0, 1, 2],
            b"sealed envelope bytes".to_vec(),
        ] {
            assert_eq!(base64_decode(&base64_encode(&bytes)).unwrap(), bytes);
        }
        assert!(base64_decode("not base64!").is_err());
    }
}
