//! Rule evaluation (Wave 4 Phase 1 + Phase 2): one `check` per value rule and
//! one `check_presence` per presence/conditional rule, each returning a
//! public-safe violation message or `None`. Pure functions except the field
//! ref rules, which resolve siblings through the [`ValidationContext`].
//!
//! Parity notes where our closed grammar deliberately narrows Laravel:
//! - `gt/gte/lt/lte` compare NUMBERS only; Laravel falls back to size (string
//!   length) comparison for non-numeric same-type values. Ours fails closed
//!   with `must be a number` instead.
//! - `contains`/`doesnt_contain` are substring checks (docs/19 grammar puts
//!   them in the string family); Laravel's `contains` tests array membership.
//! - `timezone` validates against a curated IANA list, not PHP's full tzdb.
//! - `alpha`/`alpha_num`/`alpha_dash` accept Unicode letters/numbers but not
//!   combining marks (Laravel's `\pM`); there is no std primitive for marks.
//! - present `null` fails every rule unless the field declares `nullable`
//!   (Laravel's implicit PHP null-coercion quirks are not mirrored).
//! - the `*_if` family matches sibling values through a CLOSED scalar
//!   grammar (`scalar_match`); PHP's loose `in_array` would make `true`
//!   equal every non-empty token, which we reject as fail-open.
//! - `exclude`/`exclude_if` skip validation of the field entirely; Laravel
//!   strips the key from its validated output data instead (our edge hands
//!   operations the raw input, so skipping is the observable equivalent).
//! - `in_array` resolves a plain dot-path sibling (wildcards are Phase 4);
//!   a missing sibling fails closed, and `distinct` compares array elements
//!   strictly (Laravel defaults to loose comparison).
//!
//! No regex engine: every pattern here is a hand-rolled closed grammar (the
//! documented replacement for Laravel's `regex:` rule).

use crate::validator::{DateTarget, DimBound, NumTarget, Rule, UploadMeta, ValidationContext};
use serde_json::Value;

/// Apply one value rule to one present value. `field` is the field the rules
/// are bound to (only `confirmed` reads it, to find `<field>_confirmation`);
/// `ctx.input` is the whole validated object so field-ref rules can resolve
/// their sibling.
pub(crate) fn check(
    rule: &Rule,
    field: &str,
    v: &Value,
    ctx: &ValidationContext<'_>,
) -> Option<String> {
    match rule {
        // Structural (`required`/`nullable`/`sometimes`/`bail`/`exclude*`)
        // and presence rules are resolved through `check_presence` by
        // `validate_with` before value checks run; these arms keep the match
        // total without panicking at the edge.
        Rule::Required
        | Rule::Nullable
        | Rule::Sometimes
        | Rule::Bail
        | Rule::Exclude
        | Rule::ExcludeIf(_, _)
        | Rule::RequiredIf(_, _)
        | Rule::RequiredUnless(_, _)
        | Rule::RequiredWith(_)
        | Rule::RequiredWithout(_)
        | Rule::Accepted
        | Rule::AcceptedIf(_, _)
        | Rule::Declined
        | Rule::DeclinedIf(_, _)
        | Rule::Prohibited
        | Rule::ProhibitedIf(_, _)
        | Rule::ProhibitedWith(_)
        | Rule::Missing
        | Rule::MissingIf(_, _)
        | Rule::MissingWith(_)
        | Rule::Filled
        | Rule::Present => None,

        // ------------------------------------------------ Wave 1 core ----
        // Wording is frozen: transports and existing tests rely on it.
        Rule::Min(n) => match measure(v) {
            Some(size) if size >= *n as f64 => None,
            Some(_) => Some(format!("must be at least {n}")),
            None => Some("size rules need a string, array or number".into()),
        },
        Rule::Max(n) => match measure(v) {
            Some(size) if size <= *n as f64 => None,
            Some(_) => Some(format!("must be at most {n}")),
            None => Some("size rules need a string, array or number".into()),
        },
        Rule::Email => match v.as_str() {
            Some(s) if is_email(s) => None,
            Some(_) => Some("must be a valid email".into()),
            None => Some("must be a string email".into()),
        },
        Rule::Url => match v.as_str() {
            Some(s) if is_url(s) => None,
            Some(_) => Some("must be an http(s) URL".into()),
            None => Some("must be a string URL".into()),
        },
        Rule::Uuid => match v.as_str() {
            Some(s) if uuid::Uuid::parse_str(s).is_ok() => None,
            Some(_) => Some("must be a valid UUID".into()),
            None => Some("must be a string UUID".into()),
        },
        Rule::E164 => match v.as_str() {
            Some(s) if is_e164(s) => None,
            Some(_) => Some("must be E.164 (+919876543210)".into()),
            None => Some("must be a string phone".into()),
        },
        Rule::OneOf(allowed) => match v.as_str() {
            Some(s) if allowed.iter().any(|a| a == s) => None,
            Some(_) => Some(format!("must be one of: {}", allowed.join(", "))),
            None => Some("must be a string choice".into()),
        },
        Rule::Range(lo, hi) => match v.as_f64() {
            Some(x) if x >= *lo as f64 && x <= *hi as f64 => None,
            Some(_) => Some(format!("must be {lo}..={hi}")),
            None => Some("must be a number".into()),
        },
        Rule::Date => match v.as_str() {
            Some(s) if is_date(s) => None,
            Some(_) => Some("must be RFC-3339 or YYYY-MM-DD".into()),
            None => Some("must be a string date".into()),
        },

        // ----------------------------------------------------- types ------
        Rule::Boolean => match v {
            Value::Bool(_) => None,
            Value::Number(n) if matches!(n.as_i64(), Some(0) | Some(1)) => None,
            Value::String(s) if s == "0" || s == "1" => None,
            _ => Some("must be a boolean".into()),
        },
        Rule::Integer => match v {
            // JSON 5.0 and the string "5" are integers; "5.0" (string) is not
            // (PHP FILTER_VALIDATE_INT rejects it).
            Value::Number(n) => match n.as_i64() {
                Some(_) => None,
                None if n.as_f64().map(|f| f.fract() == 0.0).unwrap_or(false) => None,
                _ => Some("must be an integer".into()),
            },
            Value::String(s) if s.trim().parse::<i64>().is_ok() => None,
            _ => Some("must be an integer".into()),
        },
        Rule::Numeric => match v {
            Value::Number(_) => None,
            Value::String(s) if is_numeric_str(s) => None,
            _ => Some("must be a number".into()),
        },
        Rule::String => match v {
            Value::String(_) => None,
            _ => Some("must be a string".into()),
        },
        Rule::Array => match v {
            Value::Array(_) => None,
            _ => Some("must be an array".into()),
        },
        Rule::Json => match v {
            // String payloads must decode; already-structured values are
            // JSON by construction (Laravel only ever sees strings here).
            Value::String(s) if serde_json::from_str::<Value>(s).is_err() => {
                Some("must be valid JSON".into())
            }
            // `null` fails without `nullable`, matching Laravel's json rule.
            Value::Null => Some("must be valid JSON".into()),
            _ => None,
        },
        Rule::Ascii => match v {
            Value::String(s) if s.is_ascii() => None,
            _ => Some("must be ASCII".into()),
        },

        // ---------------------------------------------------- strings -----
        Rule::Alpha => match v {
            Value::String(s) if !s.is_empty() && s.chars().all(char::is_alphabetic) => None,
            _ => Some("must contain only letters".into()),
        },
        Rule::AlphaNum => match v {
            Value::String(s) if !s.is_empty() && s.chars().all(|c| c.is_alphanumeric()) => None,
            Value::Number(n) if is_integer_valued(n) => None,
            _ => Some("must contain only letters and numbers".into()),
        },
        Rule::AlphaDash => match v {
            Value::String(s)
                if !s.is_empty()
                    && s.chars()
                        .all(|c| c.is_alphanumeric() || c == '-' || c == '_') =>
            {
                None
            }
            Value::Number(n) if is_integer_valued(n) => None,
            _ => Some("must contain only letters, numbers, dashes and underscores".into()),
        },
        Rule::Lowercase => match v {
            Value::String(s) if s == &s.to_lowercase() => None,
            _ => Some("must be lowercase".into()),
        },
        Rule::Uppercase => match v {
            Value::String(s) if s == &s.to_uppercase() => None,
            _ => Some("must be uppercase".into()),
        },
        Rule::StartsWith(prefix) => match v {
            Value::String(s) if s.starts_with(prefix.as_str()) => None,
            _ => Some(format!("must start with {prefix}")),
        },
        Rule::EndsWith(suffix) => match v {
            Value::String(s) if s.ends_with(suffix.as_str()) => None,
            _ => Some(format!("must end with {suffix}")),
        },
        Rule::Contains(needle) => match v {
            Value::String(s) if s.contains(needle.as_str()) => None,
            _ => Some(format!("must contain {needle}")),
        },
        Rule::DoesntStartWith(prefix) => match v {
            Value::String(s) if !s.starts_with(prefix.as_str()) => None,
            _ => Some(format!("must not start with {prefix}")),
        },
        Rule::DoesntEndWith(suffix) => match v {
            Value::String(s) if !s.ends_with(suffix.as_str()) => None,
            _ => Some(format!("must not end with {suffix}")),
        },
        Rule::DoesntContain(needle) => match v {
            Value::String(s) if !s.contains(needle.as_str()) => None,
            _ => Some(format!("must not contain {needle}")),
        },
        Rule::HexColor => match v {
            Value::String(s) if is_hex_color(s) => None,
            _ => Some("must be a valid hex color".into()),
        },
        Rule::MacAddress => match v {
            Value::String(s) if is_mac_address(s) => None,
            _ => Some("must be a valid MAC address".into()),
        },
        Rule::Timezone => match v {
            Value::String(s) if is_timezone(s) => None,
            _ => Some("must be a valid timezone".into()),
        },
        Rule::Ulid => match v {
            Value::String(s) if is_ulid(s) => None,
            _ => Some("must be a valid ULID".into()),
        },

        // --------------------------------------------------- numeric ------
        Rule::Decimal(n) => match v {
            Value::String(s) if is_numeric_str(s) => match decimal_places(s.trim()) {
                Some(p) if p == *n as usize => None,
                // Numeric but not decimal-shaped (e.g. an exponent) fails the
                // shape check exactly like Laravel's closed regex.
                _ => Some(decimal_message(*n)),
            },
            Value::Number(num) => match decimal_places(&num.to_string()) {
                Some(p) if p == *n as usize => None,
                _ => Some(decimal_message(*n)),
            },
            _ => Some("must be a number".into()),
        },
        Rule::Digits(n) => digits_check(
            v,
            |len| len == *n as usize,
            || {
                format!(
                    "must be exactly {n} digit{}",
                    if *n == 1 { "" } else { "s" }
                )
            },
        ),
        Rule::DigitsBetween(a, b) => digits_check(
            v,
            |len| len >= *a as usize && len <= *b as usize,
            || format!("must be between {a} and {b} digits"),
        ),
        Rule::MultipleOf(n) => match as_number(v) {
            None => Some("must be a number".into()),
            Some(x) if is_multiple_of(x, *n) => None,
            Some(_) => Some(format!("must be a multiple of {n}")),
        },
        Rule::Gt(target) => compare(v, target, ctx, Cmp::Gt),
        Rule::Gte(target) => compare(v, target, ctx, Cmp::Gte),
        Rule::Lt(target) => compare(v, target, ctx, Cmp::Lt),
        Rule::Lte(target) => compare(v, target, ctx, Cmp::Lte),

        // ------------------------------------------------- cross-field ----
        Rule::Same(other) => match ctx.resolve(other) {
            // A missing sibling fails closed (Laravel requires the key).
            Some(ov) if json_same(v, ov) => None,
            _ => Some(format!("must match {other}")),
        },
        Rule::Different(other) => match ctx.resolve(other) {
            Some(ov) if json_same(v, ov) => Some(format!("must differ from {other}")),
            // A missing sibling cannot equal this value, so it passes.
            _ => None,
        },
        // Laravel's `isset` semantics: a missing OR null confirmation fails.
        Rule::Confirmed => match ctx.resolve(&format!("{field}_confirmation")) {
            Some(cv) if !cv.is_null() && json_same(v, cv) => None,
            _ => Some("confirmation does not match".into()),
        },
        Rule::InArray(other) => {
            let found = match ctx.resolve(other) {
                Some(Value::Array(items)) => items.iter().any(|i| json_same(v, i)),
                Some(ov) => json_same(v, ov),
                None => false,
            };
            if found {
                None
            } else {
                Some(format!("must be one of the values in {other}"))
            }
        }
        Rule::Distinct => match v {
            Value::Array(items) => {
                let dup = (0..items.len())
                    .any(|i| (i + 1..items.len()).any(|j| json_same(&items[i], &items[j])));
                if dup {
                    Some("must contain unique values".into())
                } else {
                    None
                }
            }
            _ => Some("must be an array".into()),
        },

        // ------------------------------------------------- Wave 4 dates ---
        Rule::DateFormat(fmt) => match v {
            Value::String(s) if date_format_ok(s, fmt) => None,
            Value::String(_) => Some(format!("must match the format {fmt}")),
            _ => Some("must be a string date".into()),
        },
        Rule::Before(t) => date_cmp(v, t, ctx, DateCmp::Before),
        Rule::After(t) => date_cmp(v, t, ctx, DateCmp::After),
        Rule::BeforeOrEqual(t) => date_cmp(v, t, ctx, DateCmp::BeforeOrEqual),
        Rule::AfterOrEqual(t) => date_cmp(v, t, ctx, DateCmp::AfterOrEqual),

        // ------------------------------------------------- Wave 4 files ---
        // File rules read the stored part; a JSON value with no part fails
        // closed (`required` + multipart is the normal path).
        Rule::File => match ctx.upload_for(field) {
            Some(_) => None,
            None => Some("must be a file".into()),
        },
        Rule::Image => match ctx.upload_for(field) {
            None => Some("must be a file".into()),
            Some(u) if normalize_mime(&u.content_type).starts_with("image/") => None,
            Some(_) => Some("must be an image".into()),
        },
        Rule::Mimes(list) => match ctx.upload_for(field) {
            None => Some("must be a file".into()),
            Some(u) => {
                let mime = normalize_mime(&u.content_type);
                if list.iter().any(|e| resolve_mime(e) == mime) {
                    None
                } else {
                    Some(format!("must be a file of type: {}", list.join(", ")))
                }
            }
        },
        Rule::MimeTypes(list) => match ctx.upload_for(field) {
            None => Some("must be a file".into()),
            Some(u) => {
                let mime = normalize_mime(&u.content_type);
                if list.iter().any(|e| e.eq_ignore_ascii_case(mime.as_str())) {
                    None
                } else {
                    Some(format!("must be a file of type: {}", list.join(", ")))
                }
            }
        },
        Rule::Extensions(list) => match ctx.upload_for(field) {
            None => Some("must be a file".into()),
            Some(u) => match filename_ext(&u.filename) {
                Some(ext) if list.iter().any(|e| e.eq_ignore_ascii_case(ext)) => None,
                _ => Some(format!(
                    "must have one of these extensions: {}",
                    list.join(", ")
                )),
            },
        },
        Rule::Dimensions(bounds) => match ctx.upload_for(field) {
            None => Some("must be a file".into()),
            Some(u) => dimensions_fail(u, bounds),
        },

        // -------------------------------------------------- Wave 4 auth ---
        // Wave 7 P6: the policy instance comes from the context, fed by
        // `[auth.password_policy]` at boot (flows call the same method).
        Rule::Password => match v {
            Value::String(s) => ctx.password_policy().failure(s),
            _ => Some("must be a string".into()),
        },
        // A wrong password, a missing verifier and a missing principal are
        // indistinguishable from the outside — all read "incorrect".
        Rule::CurrentPassword => match v {
            Value::String(s) => match ctx.verifier() {
                Some(verify) if verify(s) => None,
                _ => Some("is incorrect".into()),
            },
            _ => Some("is incorrect".into()),
        },
        Rule::Letters => criterion(
            v,
            |s| s.chars().any(|c| c.is_alphabetic()),
            "must contain a letter",
        ),
        Rule::Numbers => criterion(
            v,
            |s| s.chars().any(|c| c.is_ascii_digit()),
            "must contain a number",
        ),
        Rule::Symbols => criterion(
            v,
            // Laravel parity: any non-alphanumeric character counts — even
            // whitespace (documented in the divergence notes).
            |s| s.chars().any(|c| !c.is_alphanumeric()),
            "must contain a symbol",
        ),
        Rule::MixedCase => criterion(
            v,
            |s| s.chars().any(|c| c.is_uppercase()) && s.chars().any(|c| c.is_lowercase()),
            "must contain uppercase and lowercase letters",
        ),

        // ------------------------------------------- Wave 4 Phase 4: DB ---
        // `unique`/`exists` are intercepted by `validate_with`, which owns
        // the fatal (503) channel; this arm fails closed defensively if a
        // future call path ever reaches them through `check` directly.
        Rule::Unique(_) | Rule::Exists(_) => Some("database probe unavailable".into()),

        // Module-registered rule (manifest `custom = "<name>"`), resolved
        // through the context's lookup. An unregistered name fails closed
        // as a field message — never a silent pass (the GRAPH-RULE-UNKNOWN
        // boot gate should make it unreachable in production).
        Rule::Custom(name) => match ctx.custom_rules().and_then(|l| l.rule(name)) {
            Some(custom) => custom.check(field, v, ctx.input()),
            None => Some(format!("custom rule {name} is not registered")),
        },
    }
}

/// Phase 4 database rules — the one evaluation with an ERROR channel.
/// `Ok(None)` passes; `Ok(Some(msg))` is the field failure ("has already
/// been taken" / "is invalid"); `Err(detail)` is a store failure or an
/// unregistered probe, which [`crate::Validator::validate_with`] turns into
/// [`crate::ValidationReport::fatal`] → 503 envelope (docs/19: DB errors
/// never pass validation).
///
/// The validated value must be a JSON string: columns compare as text (the
/// models store their wire values as declared), so anything else fails
/// closed with a field message instead of guessing a coercion.
pub(crate) fn db_check(
    rule: &Rule,
    field: &str,
    v: &Value,
    ctx: &ValidationContext<'_>,
) -> Result<Option<String>, String> {
    let (probe, unique) = match rule {
        Rule::Unique(p) => (p, true),
        Rule::Exists(p) => (p, false),
        _ => return Ok(None),
    };
    let Value::String(value) = v else {
        return Ok(Some("must be a string".into()));
    };
    let missing = format!("no row probe registered for model {}", probe.model);
    let lookup = ctx.probes().ok_or_else(|| missing.clone())?;
    let registered = lookup.probe(&probe.model).ok_or(missing)?;
    // Match column: explicit override, else the final segment of the
    // validated field path (`profile.email` → `email`).
    let column = probe
        .column
        .as_deref()
        .unwrap_or_else(|| field.rsplit('.').next().unwrap_or(field));
    let matched = registered
        .has_row(
            ctx.tenant(),
            column,
            value,
            &probe.wheres,
            probe.ignore_id.as_deref(),
        )
        .map_err(|e| format!("row probe failed for model {}: {e}", probe.model))?;
    if unique {
        Ok(matched.then(|| "has already been taken".to_string()))
    } else {
        Ok((!matched).then(|| "is invalid".to_string()))
    }
}

// ------------------------------------------ Wave 4 files & auth -----------

/// Content type with any parameters stripped, lowercased (`image/png;
/// charset=binary` → `image/png`).
fn normalize_mime(raw: &str) -> String {
    raw.split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// `mimes:` entries are extension-style (`jpg`, Laravel's mime guessing
/// derives the type from the extension); resolve them to canonical MIME
/// types. Entries already carrying `/` or unknown ones compare literally.
fn resolve_mime(entry: &str) -> String {
    let e = entry.to_ascii_lowercase();
    if e.contains('/') {
        return e;
    }
    match e.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" | "svgz" => "image/svg+xml",
        "tif" | "tiff" => "image/tiff",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "json" => "application/json",
        "xml" => "application/xml",
        "html" | "htm" => "text/html",
        "zip" => "application/zip",
        "gz" | "gzip" => "application/gzip",
        "tar" => "application/x-tar",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        other => other,
    }
    .to_string()
}

/// Extension after the final dot, lowercased (`scan.TAR.GZ` → `gz`).
fn filename_ext(filename: &str) -> Option<&str> {
    let base = filename.rsplit(['/', '\\']).next().unwrap_or(filename);
    base.rsplit_once('.')
        .map(|(_, ext)| ext)
        .filter(|ext| !ext.is_empty())
}

/// First failing bound in canonical declaration order; a part whose pixel
/// dimensions could not be sniffed fails closed.
fn dimensions_fail(part: &UploadMeta, bounds: &[DimBound]) -> Option<String> {
    let (Some(w), Some(h)) = (part.width, part.height) else {
        return Some("must have readable image dimensions".into());
    };
    for bound in bounds {
        let msg = match bound {
            DimBound::Width(n) if w != *n => format!("must be {n} pixels wide"),
            DimBound::Height(n) if h != *n => format!("must be {n} pixels tall"),
            DimBound::MinWidth(n) if w < *n => format!("must be at least {n} pixels wide"),
            DimBound::MaxWidth(n) if w > *n => format!("must be at most {n} pixels wide"),
            DimBound::MinHeight(n) if h < *n => format!("must be at least {n} pixels tall"),
            DimBound::MaxHeight(n) if h > *n => format!("must be at most {n} pixels tall"),
            _ => continue,
        };
        return Some(msg);
    }
    None
}

/// Standalone criterion token shape: strings are tested, anything else
/// fails closed (`password`'s sub-rules are usable on any field).
fn criterion(v: &Value, ok: impl FnOnce(&str) -> bool, msg: &str) -> Option<String> {
    match v {
        Value::String(s) if ok(s) => None,
        Value::String(_) => Some(msg.to_string()),
        _ => Some("must be a string".into()),
    }
}

// ------------------------------------------ presence & conditionals -------

/// Presence/conditional evaluation (Wave 4 Phase 2): `value` is the resolved
/// lookup for the field under validation (`None` = key absent). Structural
/// rules return `None` here — `validate_with` resolves them itself — and
/// value rules never arrive (they are evaluated through [`check`]).
pub(crate) fn check_presence(
    rule: &Rule,
    field: &str,
    value: Option<&Value>,
    ctx: &ValidationContext<'_>,
) -> Option<String> {
    match rule {
        Rule::Required => required_fail(value).then(|| format!("{field} is required")),
        Rule::Present => value.is_none().then(|| format!("{field} must be present")),
        Rule::RequiredIf(other, vals) => {
            if triggered(ctx, other, vals) && required_fail(value) {
                Some(format!(
                    "{field} is required when {other} is {}",
                    vals.join(" or ")
                ))
            } else {
                None
            }
        }
        Rule::RequiredUnless(other, vals) => {
            // An absent sibling resolves as `null`, which only matches a
            // literal `null` token — everything else still requires.
            if !any_token(ctx.resolve(other), vals) && required_fail(value) {
                Some(format!(
                    "{field} is required unless {other} is {}",
                    vals.join(" or ")
                ))
            } else {
                None
            }
        }
        Rule::RequiredWith(other) => {
            if ctx.resolve(other).is_some_and(passes_required) && required_fail(value) {
                Some(format!("{field} is required when {other} is present"))
            } else {
                None
            }
        }
        Rule::RequiredWithout(other) => {
            if !ctx.resolve(other).is_some_and(passes_required) && required_fail(value) {
                Some(format!("{field} is required when {other} is missing"))
            } else {
                None
            }
        }
        Rule::Accepted => {
            (!value.is_some_and(accepted_ok)).then(|| format!("{field} must be accepted"))
        }
        Rule::AcceptedIf(other, vals) => {
            if triggered(ctx, other, vals) && !value.is_some_and(accepted_ok) {
                Some(format!(
                    "{field} must be accepted when {other} is {}",
                    vals.join(" or ")
                ))
            } else {
                None
            }
        }
        Rule::Declined => {
            (!value.is_some_and(declined_ok)).then(|| format!("{field} must be declined"))
        }
        Rule::DeclinedIf(other, vals) => {
            if triggered(ctx, other, vals) && !value.is_some_and(declined_ok) {
                Some(format!(
                    "{field} must be declined when {other} is {}",
                    vals.join(" or ")
                ))
            } else {
                None
            }
        }
        Rule::Prohibited => value
            .is_some_and(passes_required)
            .then(|| format!("{field} must be omitted or empty")),
        Rule::ProhibitedIf(other, vals) => {
            if triggered(ctx, other, vals) && value.is_some_and(passes_required) {
                Some(format!(
                    "{field} must be omitted or empty when {other} is {}",
                    vals.join(" or ")
                ))
            } else {
                None
            }
        }
        Rule::ProhibitedWith(other) => {
            if ctx.resolve(other).is_some() && value.is_some_and(passes_required) {
                Some(format!(
                    "{field} must be omitted or empty when {other} is present"
                ))
            } else {
                None
            }
        }
        Rule::Missing => value.is_some().then(|| format!("{field} must be missing")),
        Rule::MissingIf(other, vals) => {
            if triggered(ctx, other, vals) && value.is_some() {
                Some(format!(
                    "{field} must be missing when {other} is {}",
                    vals.join(" or ")
                ))
            } else {
                None
            }
        }
        Rule::MissingWith(other) => {
            if ctx.resolve(other).is_some() && value.is_some() {
                Some(format!("{field} must be missing when {other} is present"))
            } else {
                None
            }
        }
        Rule::Filled => match value {
            // Absent keys pass (`Arr::has` is false); present keys must
            // satisfy `required` — including `null`, because `filled` is
            // implicit and therefore ignores `nullable`.
            None => None,
            Some(v) if passes_required(v) => None,
            Some(_) => Some(format!("{field} must not be empty")),
        },
        // Structural: `validate_with` resolves these before evaluation.
        Rule::Sometimes | Rule::Exclude | Rule::ExcludeIf(_, _) => None,
        // Value rules never reach `check_presence`.
        _ => None,
    }
}

/// Structural exclusion: `exclude` ignores the field always; `exclude_if`
/// ignores it only when the sibling key exists AND matches (an absent
/// sibling never triggers). Resolved by `validate_with` before evaluation.
pub(crate) fn is_excluded(rule: &Rule, ctx: &ValidationContext<'_>) -> bool {
    match rule {
        Rule::Exclude => true,
        Rule::ExcludeIf(other, vals) => triggered(ctx, other, vals),
        _ => false,
    }
}

/// Does the sibling key exist AND equal one of the condition values? An
/// absent sibling never triggers (Laravel's `! Arr::has(...) => true` guard).
fn triggered(ctx: &ValidationContext<'_>, other: &str, vals: &[String]) -> bool {
    ctx.resolve(other)
        .is_some_and(|ov| any_token(Some(ov), vals))
}

/// `required`-family truthiness (Laravel's `validateRequired`): present,
/// non-null, non-blank.
pub(crate) fn passes_required(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::String(s) => !s.trim().is_empty(),
        Value::Array(a) => !a.is_empty(),
        _ => true,
    }
}

/// The field must satisfy `required` for its current key state.
fn required_fail(value: Option<&Value>) -> bool {
    !value.is_some_and(passes_required)
}

/// `accepted`: required first, then Laravel's strict set
/// (`yes`, `on`, `1`, `1`, `true`, `"true"`).
fn accepted_ok(v: &Value) -> bool {
    passes_required(v)
        && match v {
            Value::Bool(b) => *b,
            Value::Number(n) => matches!(n.as_i64(), Some(1)),
            Value::String(s) => matches!(s.as_str(), "yes" | "on" | "1" | "true"),
            _ => false,
        }
}

/// `declined`: required first, then Laravel's strict set
/// (`no`, `off`, `0`, `0`, `false`, `"false"`).
fn declined_ok(v: &Value) -> bool {
    passes_required(v)
        && match v {
            Value::Bool(b) => !*b,
            Value::Number(n) => matches!(n.as_i64(), Some(0)),
            Value::String(s) => matches!(s.as_str(), "no" | "off" | "0" | "false"),
            _ => false,
        }
}

/// Does the sibling value match ANY of the condition tokens? `None`
/// (absent) resolves as `null` for `required_unless`'s null-world.
fn any_token(other: Option<&Value>, vals: &[String]) -> bool {
    vals.iter().any(|t| scalar_match(other, t))
}

/// Closed-grammar scalar comparison for the `*_if` family. Divergence from
/// PHP: Laravel compares with a loose `in_array`, where `true == "anything"`
/// holds for every non-empty token; we only accept the documented spellings
/// (fail-closed). Absent siblings resolve as `null`.
fn scalar_match(other: Option<&Value>, token: &str) -> bool {
    match other {
        None | Some(Value::Null) => token.eq_ignore_ascii_case("null"),
        Some(Value::Bool(b)) => {
            if *b {
                matches!(token, "true" | "1" | "yes" | "on")
            } else {
                matches!(token, "false" | "0" | "no" | "off")
            }
        }
        Some(Value::Number(n)) => n
            .as_f64()
            .is_some_and(|f| matches!(token.trim().parse::<f64>(), Ok(x) if x == f)),
        Some(Value::String(s)) => s == token,
        Some(_) => false,
    }
}

/// Strict JSON equality (mirrors PHP `===`): `1` and `1.0` differ, `"1"`
/// and `1` differ — serde_json keeps the number-variant split.
fn json_same(a: &Value, b: &Value) -> bool {
    a == b
}

// ----------------------------------------------------------------- helpers

/// Size of a value for min/max: string chars, array length, numeric value.
pub(crate) fn measure(v: &Value) -> Option<f64> {
    match v {
        Value::String(s) => Some(s.chars().count() as f64),
        Value::Array(a) => Some(a.len() as f64),
        Value::Number(_) => v.as_f64(),
        _ => None,
    }
}

/// Numeric coercion shared by `numeric` and the comparisons: JSON numbers
/// directly, strings through a finite f64 parse (whitespace-trimmed).
pub(crate) fn as_number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite()),
        _ => None,
    }
}

fn is_numeric_str(s: &str) -> bool {
    s.trim()
        .parse::<f64>()
        .map(|f| f.is_finite())
        .unwrap_or(false)
}

fn is_integer_valued(n: &serde_json::Number) -> bool {
    n.as_i64().is_some() || n.as_f64().map(|f| f.fract() == 0.0).unwrap_or(false)
}

fn decimal_message(n: u8) -> String {
    format!(
        "must have exactly {n} decimal place{}",
        if n == 1 { "" } else { "s" }
    )
}

/// Decimal places in an already-numeric string. `None` = not decimal-shaped
/// (exponent notation fails, mirroring Laravel's `/^[+-]?\d*\.?(\d*)$/`).
fn decimal_places(s: &str) -> Option<usize> {
    match s.find('.') {
        None => Some(0),
        Some(i) => {
            let frac = &s[i + 1..];
            if frac.is_empty() || frac.chars().all(|c| c.is_ascii_digit()) {
                Some(frac.len())
            } else {
                None
            }
        }
    }
}

/// `digits`/`digits_between`: pure digit strings (numbers render through
/// `Display`, so `1234.5` fails the shape with the rule's message).
fn digits_check(
    v: &Value,
    len_ok: impl Fn(usize) -> bool,
    fail: impl FnOnce() -> String,
) -> Option<String> {
    let shape = match v {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        _ => return Some("must be a number".into()),
    };
    if !shape.chars().all(|c| c.is_ascii_digit()) {
        return Some(fail());
    }
    if len_ok(shape.len()) {
        None
    } else {
        Some(fail())
    }
}

/// Float-safe multiple-of: the quotient must be within 1e-9 relative
/// tolerance of an integer (0.3 / 0.1 = 2.999… still passes).
fn is_multiple_of(value: f64, divisor: f64) -> bool {
    if divisor == 0.0 || !value.is_finite() {
        return false;
    }
    let q = value / divisor;
    (q - q.round()).abs() <= 1e-9 * q.abs().max(1.0)
}

/// Comparison operators for `gt`/`gte`/`lt`/`lte`.
#[derive(Clone, Copy)]
enum Cmp {
    Gt,
    Gte,
    Lt,
    Lte,
}

impl Cmp {
    fn test(self, x: f64, y: f64) -> bool {
        match self {
            Cmp::Gt => x > y,
            Cmp::Gte => x >= y,
            Cmp::Lt => x < y,
            Cmp::Lte => x <= y,
        }
    }

    fn phrase(self) -> &'static str {
        match self {
            Cmp::Gt => "greater than",
            Cmp::Gte => "greater than or equal to",
            Cmp::Lt => "less than",
            Cmp::Lte => "less than or equal to",
        }
    }

    fn message(self, target: &NumTarget) -> String {
        let label = match target {
            NumTarget::Literal(n) => n.to_string(),
            NumTarget::Field(path) => path.clone(),
        };
        format!("must be {} {label}", self.phrase())
    }
}

/// `gt:<n|field>` family: numbers only. A missing, null or non-numeric
/// field reference fails closed with the comparison message.
fn compare(v: &Value, target: &NumTarget, ctx: &ValidationContext<'_>, op: Cmp) -> Option<String> {
    let Some(x) = as_number(v) else {
        return Some("must be a number".into());
    };
    let y = match target {
        NumTarget::Literal(n) => *n,
        NumTarget::Field(path) => {
            let Some(other) = ctx.resolve(path).filter(|o| !o.is_null()) else {
                return Some(op.message(target));
            };
            let Some(y) = as_number(other) else {
                return Some(op.message(target));
            };
            y
        }
    };
    if op.test(x, y) {
        None
    } else {
        Some(op.message(target))
    }
}

// -------------------------------------------------------- closed grammars

pub(crate) fn is_email(s: &str) -> bool {
    let mut parts = s.split('@');
    let local = parts.next().unwrap_or("");
    let domain = parts.next().unwrap_or("");
    parts.next().is_none()
        && !local.is_empty()
        && local.chars().count() <= 64
        && !local.starts_with('.')
        && !local.ends_with('.')
        && !local.contains(char::is_whitespace)
        && domain.chars().count() >= 3
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !domain.contains(char::is_whitespace)
}

pub(crate) fn is_url(s: &str) -> bool {
    let rest = match s
        .strip_prefix("http://")
        .or_else(|| s.strip_prefix("https://"))
    {
        Some(r) => r,
        None => return false,
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty() && !host.contains(char::is_whitespace)
}

pub(crate) fn is_e164(s: &str) -> bool {
    s.starts_with('+') && (8..=16).contains(&s.len()) && s[1..].chars().all(|c| c.is_ascii_digit())
}

/// Parse the closed date shapes the `date` rule accepts into an absolute
/// instant: RFC-3339, minute-precision `T10:00Z` / `T10:00±HH:MM` (the shape
/// appointment slots use), and bare `YYYY-MM-DD` (midnight UTC). `is_date`,
/// `before:*`/`after:*` and `date_format` all read this one grammar, so a
/// value that passes `date` is always comparable.
pub(crate) fn date_instant(s: &str) -> Option<time::OffsetDateTime> {
    if let Ok(dt) = time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339) {
        return Some(dt);
    }
    // RFC 3339 strictly requires seconds; also accept minute-precision
    // timestamps (T10:00Z / T10:00+05:30), the shape appointment slots use.
    if let Ok(fmt) =
        time::format_description::parse_borrowed::<2>("[year]-[month]-[day]T[hour]:[minute]Z")
    {
        if let Ok(dt) = time::PrimitiveDateTime::parse(s, &fmt) {
            return Some(dt.assume_utc());
        }
    }
    if let Ok(fmt) = time::format_description::parse_borrowed::<2>(
        "[year]-[month]-[day]T[hour]:[minute][offset_hour sign:mandatory]:[offset_minute]",
    ) {
        if let Ok(dt) = time::OffsetDateTime::parse(s, &fmt) {
            return Some(dt);
        }
    }
    match time::format_description::parse_borrowed::<2>("[year]-[month]-[day]") {
        Ok(fmt) => time::Date::parse(s, &fmt)
            .ok()
            .and_then(|d| d.with_hms(0, 0, 0).ok())
            .map(time::PrimitiveDateTime::assume_utc),
        Err(_) => None,
    }
}

pub(crate) fn is_date(s: &str) -> bool {
    date_instant(s).is_some()
}

// ---------------------------------------- Wave 4 dates (Phase 3) ----------

/// Which side of the target the value must sit on.
#[derive(Clone, Copy)]
enum DateCmp {
    Before,
    After,
    BeforeOrEqual,
    AfterOrEqual,
}

/// Compare a date value against a literal, `now`, or a sibling field. An
/// unparseable VALUE fails with the `date` rule's shape message; an
/// unresolvable TARGET fails closed with the comparison message (the claim
/// is unprovable). Offsets normalize through `date_instant`, so the compare
/// is chronological across zones.
fn date_cmp(
    v: &Value,
    target: &DateTarget,
    ctx: &ValidationContext<'_>,
    cmp: DateCmp,
) -> Option<String> {
    let (display, target_at) = match target {
        DateTarget::Literal(s) => (s.as_str(), date_instant(s)),
        DateTarget::Now => ("now", Some(time::OffsetDateTime::now_utc())),
        DateTarget::Field(name) => (
            name.as_str(),
            ctx.resolve(name)
                .and_then(Value::as_str)
                .and_then(date_instant),
        ),
    };
    let cmp_msg = |cmp: DateCmp| match cmp {
        DateCmp::Before => format!("must be before {display}"),
        DateCmp::After => format!("must be after {display}"),
        DateCmp::BeforeOrEqual => format!("must be on or before {display}"),
        DateCmp::AfterOrEqual => format!("must be on or after {display}"),
    };
    // Fail closed on both sides: an unparseable value is not a date, and an
    // unresolvable target cannot be proven (the comparison message reads as
    // "unestablished", never as a pass).
    let value_at = match v.as_str() {
        Some(s) => match date_instant(s) {
            Some(at) => at,
            None => return Some("must be RFC-3339 or YYYY-MM-DD".into()),
        },
        None => return Some("must be a string date".into()),
    };
    let Some(target_at) = target_at else {
        return Some(cmp_msg(cmp));
    };
    let ok = match cmp {
        DateCmp::Before => value_at < target_at,
        DateCmp::After => value_at > target_at,
        DateCmp::BeforeOrEqual => value_at <= target_at,
        DateCmp::AfterOrEqual => value_at >= target_at,
    };
    if ok {
        None
    } else {
        Some(cmp_msg(cmp))
    }
}

/// Boot validation for `date_format:<fmt>`: the closed `Y m d H i s` token
/// set lives in `kelson-foundation` (Wave 8 P2 moved the grammar into the TCB
/// so the date helpers and this rule read ONE implementation).
pub(crate) fn check_date_format_spec(fmt: &str) -> Result<(), String> {
    kelson_foundation::check_date_format_spec(fmt)
}

/// Match a value against the closed `Y m d H i s` format — the grammar now
/// lives in `kelson_foundation::parse_date_with` (Wave 8 P2: one source of
/// truth for the rule and the helpers).
fn date_format_ok(s: &str, fmt: &str) -> bool {
    kelson_foundation::parse_date_with(s, fmt).is_some()
}

/// `#` + exactly 3/4/6/8 hex digits (Laravel's hex_color regex verbatim:
/// `/^#(?:(?:[0-9a-f]{3}){1,2}|(?:[0-9a-f]{4}){1,2})$/i` — the `#` is
/// REQUIRED, there is no optional-hash form).
fn is_hex_color(s: &str) -> bool {
    let Some(body) = s.strip_prefix('#') else {
        return false;
    };
    matches!(body.len(), 3 | 4 | 6 | 8) && body.chars().all(|c| c.is_ascii_hexdigit())
}

/// Six octets of two hex digits, separated by `:` or `-` (PHP
/// FILTER_VALIDATE_MAC accepts both separators; mixed separators fail).
fn is_mac_address(s: &str) -> bool {
    let sep = if s.contains(':') {
        ':'
    } else if s.contains('-') {
        '-'
    } else {
        return false;
    };
    let parts: Vec<&str> = s.split(sep).collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// 26-char Crockford base32, uppercase only (Laravel's `Str::isUlid`:
/// `/^[0-9A-HJKMNP-TV-Z]{26}$/` — I, L, O and U are excluded).
fn is_ulid(s: &str) -> bool {
    const CROCKFORD: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    s.len() == 26 && s.bytes().all(|b| CROCKFORD.contains(&b))
}

/// Curated IANA timezone identifiers (plus the legacy aliases PHP lists).
/// A deliberately bounded subset of tzdata — the closed-grammar stance that
/// replaces PHP's full `timezone_identifiers_list()` (documented divergence).
static TIMEZONES: &[&str] = &[
    // legacy aliases
    "UTC",
    "GMT",
    "UCT",
    "Universal",
    "Zulu",
    "Greenwich",
    "Etc/UTC",
    "Etc/GMT",
    // Africa
    "Africa/Abidjan",
    "Africa/Accra",
    "Africa/Addis_Ababa",
    "Africa/Algiers",
    "Africa/Asmara",
    "Africa/Bamako",
    "Africa/Bangui",
    "Africa/Banjul",
    "Africa/Bissau",
    "Africa/Blantyre",
    "Africa/Bujumbura",
    "Africa/Cairo",
    "Africa/Casablanca",
    "Africa/Ceuta",
    "Africa/Conakry",
    "Africa/Dakar",
    "Africa/Dar_es_Salaam",
    "Africa/Djibouti",
    "Africa/Douala",
    "Africa/El_Aaiun",
    "Africa/Freetown",
    "Africa/Gaborone",
    "Africa/Harare",
    "Africa/Juba",
    "Africa/Johannesburg",
    "Africa/Kampala",
    "Africa/Khartoum",
    "Africa/Kigali",
    "Africa/Kinshasa",
    "Africa/Kisumu",
    "Africa/Lagos",
    "Africa/Libreville",
    "Africa/Lome",
    "Africa/Luanda",
    "Africa/Lubumbashi",
    "Africa/Lusaka",
    "Africa/Malabo",
    "Africa/Maputo",
    "Africa/Maseru",
    "Africa/Mbabane",
    "Africa/Mogadishu",
    "Africa/Monrovia",
    "Africa/Nairobi",
    "Africa/Ndjamena",
    "Africa/Niamey",
    "Africa/Nouakchott",
    "Africa/Ouagadougou",
    "Africa/Porto-Novo",
    "Africa/Sao_Tome",
    "Africa/Tripoli",
    "Africa/Tunis",
    "Africa/Windhoek",
    // America
    "America/Adak",
    "America/Anchorage",
    "America/Anguilla",
    "America/Antigua",
    "America/Araguaina",
    "America/Argentina/Buenos_Aires",
    "America/Argentina/Catamarca",
    "America/Argentina/Cordoba",
    "America/Argentina/Jujuy",
    "America/Argentina/La_Rioja",
    "America/Argentina/Mendoza",
    "America/Argentina/Rio_Gallegos",
    "America/Argentina/Salta",
    "America/Argentina/San_Juan",
    "America/Argentina/San_Luis",
    "America/Argentina/Tucuman",
    "America/Argentina/Ushuaia",
    "America/Aruba",
    "America/Asuncion",
    "America/Atikokan",
    "America/Bahia",
    "America/Bahia_Banderas",
    "America/Barbados",
    "America/Belem",
    "America/Belize",
    "America/Blanc-Sablon",
    "America/Boise",
    "America/Cambridge_Bay",
    "America/Cancun",
    "America/Caracas",
    "America/Cayman",
    "America/Chicago",
    "America/Chihuahua",
    "America/Ciudad_Juarez",
    "America/Costa_Rica",
    "America/Cuiaba",
    "America/Curacao",
    "America/Dawson",
    "America/Dawson_Creek",
    "America/Denver",
    "America/Detroit",
    "America/Dominica",
    "America/Edmonton",
    "America/El_Salvador",
    "America/Fort_Nelson",
    "America/Fortaleza",
    "America/Glace_Bay",
    "America/Godthab",
    "America/Goose_Bay",
    "America/Grand_Turk",
    "America/Grenada",
    "America/Guadeloupe",
    "America/Guatemala",
    "America/Guayaquil",
    "America/Guyana",
    "America/Halifax",
    "America/Havana",
    "America/Hermosillo",
    "America/Indiana/Indianapolis",
    "America/Indiana/Knox",
    "America/Indiana/Marengo",
    "America/Indiana/Petersburg",
    "America/Indiana/Tell_City",
    "America/Indiana/Vevay",
    "America/Indiana/Vincennes",
    "America/Indiana/Winamac",
    "America/Indianapolis",
    "America/Inuvik",
    "America/Iqaluit",
    "America/Jamaica",
    "America/Juneau",
    "America/Kentucky/Louisville",
    "America/Kentucky/Monticello",
    "America/Lima",
    "America/Los_Angeles",
    "America/Maceio",
    "America/Managua",
    "America/Manaus",
    "America/Martinique",
    "America/Matamoros",
    "America/Mazatlan",
    "America/Mendoza",
    "America/Menominee",
    "America/Merida",
    "America/Metlakatla",
    "America/Mexico_City",
    "America/Moncton",
    "America/Monterrey",
    "America/Montevideo",
    "America/Montserrat",
    "America/Nassau",
    "America/New_York",
    "America/Nome",
    "America/Noronha",
    "America/North_Dakota/Center",
    "America/North_Dakota/New_Salem",
    "America/Nuuk",
    "America/Ojinaga",
    "America/Panama",
    "America/Pangnirtung",
    "America/Paramaribo",
    "America/Phoenix",
    "America/Port-au-Prince",
    "America/Port_of_Spain",
    "America/Puerto_Rico",
    "America/Punta_Arenas",
    "America/Regina",
    "America/Resolute",
    "America/Rio_Branco",
    "America/Santarem",
    "America/Santiago",
    "America/Santo_Domingo",
    "America/Sao_Paulo",
    "America/Scoresbysund",
    "America/Sitka",
    "America/St_Johns",
    "America/St_Kitts",
    "America/St_Lucia",
    "America/St_Thomas",
    "America/St_Vincent",
    "America/Swift_Current",
    "America/Tegucigalpa",
    "America/Thule",
    "America/Thunder_Bay",
    "America/Tijuana",
    "America/Toronto",
    "America/Vancouver",
    "America/Whitehorse",
    "America/Winnipeg",
    "America/Yakutat",
    "America/Yellowknife",
    // Antarctica
    "Antarctica/Casey",
    "Antarctica/Davis",
    "Antarctica/DumontDUrville",
    "Antarctica/Mawson",
    "Antarctica/Palmer",
    "Antarctica/Rothera",
    "Antarctica/Syowa",
    "Antarctica/Troll",
    "Antarctica/Vostok",
    // Asia
    "Asia/Almaty",
    "Asia/Amman",
    "Asia/Anadyr",
    "Asia/Aqtau",
    "Asia/Aqtobe",
    "Asia/Ashgabat",
    "Asia/Baghdad",
    "Asia/Baku",
    "Asia/Bangkok",
    "Asia/Barnaul",
    "Asia/Beirut",
    "Asia/Bishkek",
    "Asia/Brunei",
    "Asia/Chita",
    "Asia/Choibalsan",
    "Asia/Colombo",
    "Asia/Damascus",
    "Asia/Dhaka",
    "Asia/Dili",
    "Asia/Dubai",
    "Asia/Dushanbe",
    "Asia/Famagusta",
    "Asia/Gaza",
    "Asia/Hebron",
    "Asia/Ho_Chi_Minh",
    "Asia/Hong_Kong",
    "Asia/Hovd",
    "Asia/Irkutsk",
    "Asia/Jakarta",
    "Asia/Jerusalem",
    "Asia/Kabul",
    "Asia/Kathmandu",
    "Asia/Khandyga",
    "Asia/Kolkata",
    "Asia/Krasnoyarsk",
    "Asia/Kuala_Lumpur",
    "Asia/Kuching",
    "Asia/Macau",
    "Asia/Magadan",
    "Asia/Makassar",
    "Asia/Manila",
    "Asia/Muscat",
    "Asia/Novokuznetsk",
    "Asia/Novosibirsk",
    "Asia/Omsk",
    "Asia/Oral",
    "Asia/Phnom_Penh",
    "Asia/Pontianak",
    "Asia/Pyongyang",
    "Asia/Qatar",
    "Asia/Riyadh",
    "Asia/Sakhalin",
    "Asia/Samarkand",
    "Asia/Seoul",
    "Asia/Shanghai",
    "Asia/Singapore",
    "Asia/Srednekolymsk",
    "Asia/Taipei",
    "Asia/Tashkent",
    "Asia/Tbilisi",
    "Asia/Tehran",
    "Asia/Thimphu",
    "Asia/Tokyo",
    "Asia/Tomsk",
    "Asia/Ulaanbaatar",
    "Asia/Urumqi",
    "Asia/Ust-Nera",
    "Asia/Vientiane",
    "Asia/Vladivostok",
    "Asia/Yakutsk",
    "Asia/Yangon",
    "Asia/Yekaterinburg",
    "Asia/Yerevan",
    // Atlantic
    "Atlantic/Azores",
    "Atlantic/Bermuda",
    "Atlantic/Canary",
    "Atlantic/Cape_Verde",
    "Atlantic/Faroe",
    "Atlantic/Madeira",
    "Atlantic/Reykjavik",
    "Atlantic/South_Georgia",
    "Atlantic/St_Helena",
    "Atlantic/Stanley",
    // Australia + Pacific islands (Australian block)
    "Australia/Adelaide",
    "Australia/Brisbane",
    "Australia/Broken_Hill",
    "Australia/Currie",
    "Australia/Darwin",
    "Australia/Eucla",
    "Australia/Hobart",
    "Australia/Lindeman",
    "Australia/Lord_Howe",
    "Australia/Melbourne",
    "Australia/Perth",
    "Australia/Sydney",
    // Europe
    "Europe/Amsterdam",
    "Europe/Andorra",
    "Europe/Astrakhan",
    "Europe/Athens",
    "Europe/Belgrade",
    "Europe/Berlin",
    "Europe/Bratislava",
    "Europe/Brussels",
    "Europe/Bucharest",
    "Europe/Budapest",
    "Europe/Chisinau",
    "Europe/Copenhagen",
    "Europe/Dublin",
    "Europe/Gibraltar",
    "Europe/Helsinki",
    "Europe/Istanbul",
    "Europe/Kaliningrad",
    "Europe/Kyiv",
    "Europe/Lisbon",
    "Europe/Ljubljana",
    "Europe/London",
    "Europe/Luxembourg",
    "Europe/Madrid",
    "Europe/Malta",
    "Europe/Minsk",
    "Europe/Monaco",
    "Europe/Moscow",
    "Europe/Oslo",
    "Europe/Paris",
    "Europe/Prague",
    "Europe/Riga",
    "Europe/Rome",
    "Europe/Samara",
    "Europe/Sarajevo",
    "Europe/Saratov",
    "Europe/Simferopol",
    "Europe/Skopje",
    "Europe/Sofia",
    "Europe/Stockholm",
    "Europe/Tallinn",
    "Europe/Tirane",
    "Europe/Vaduz",
    "Europe/Vienna",
    "Europe/Vilnius",
    "Europe/Volgograd",
    "Europe/Warsaw",
    "Europe/Zurich",
    // Indian Ocean
    "Indian/Antananarivo",
    "Indian/Chagos",
    "Indian/Christmas",
    "Indian/Cocos",
    "Indian/Comoro",
    "Indian/Maldives",
    "Indian/Mauritius",
    "Indian/Mayotte",
    "Indian/Reunion",
    // Pacific
    "Pacific/Auckland",
    "Pacific/Bougainville",
    "Pacific/Chatham",
    "Pacific/Chuuk",
    "Pacific/Easter",
    "Pacific/Efate",
    "Pacific/Fakaofo",
    "Pacific/Fiji",
    "Pacific/Funafuti",
    "Pacific/Galapagos",
    "Pacific/Gambier",
    "Pacific/Guadalcanal",
    "Pacific/Guam",
    "Pacific/Honolulu",
    "Pacific/Honiara",
    "Pacific/Kiritimati",
    "Pacific/Kosrae",
    "Pacific/Kwajalein",
    "Pacific/Majuro",
    "Pacific/Marquesas",
    "Pacific/Midway",
    "Pacific/Nauru",
    "Pacific/Niue",
    "Pacific/Norfolk",
    "Pacific/Noumea",
    "Pacific/Pago_Pago",
    "Pacific/Palau",
    "Pacific/Pitcairn",
    "Pacific/Pohnpei",
    "Pacific/Port_Moresby",
    "Pacific/Rarotonga",
    "Pacific/Saipan",
    "Pacific/Tahiti",
    "Pacific/Tarawa",
    "Pacific/Tongatapu",
    "Pacific/Wake",
    "Pacific/Wallis",
];

fn is_timezone(s: &str) -> bool {
    TIMEZONES.contains(&s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Evaluate one rule against one value with an empty-object context.
    fn run(rule: &Rule, v: &Value) -> Option<String> {
        let empty = json!({});
        let ctx = ValidationContext::new(&empty);
        check(rule, "field", v, &ctx)
    }

    fn passes(rule: Rule, v: Value) -> bool {
        run(&rule, &v).is_none()
    }

    fn fails(rule: Rule, v: Value) -> bool {
        run(&rule, &v).is_some()
    }

    #[test]
    fn type_rules_pass_and_fail() {
        // boolean: Laravel's acceptable set = true/false/0/1/"0"/"1".
        assert!(passes(Rule::Boolean, json!(true)));
        assert!(passes(Rule::Boolean, json!(0)));
        assert!(passes(Rule::Boolean, json!("1")));
        assert!(fails(Rule::Boolean, json!(2)));
        assert!(fails(Rule::Boolean, json!("yes")));
        assert!(fails(Rule::Boolean, json!([true])));

        // integer: JSON 5.0 and "5" pass; "5.0" (string) and 5.5 fail.
        assert!(passes(Rule::Integer, json!(5)));
        assert!(passes(Rule::Integer, json!(5.0)));
        assert!(passes(Rule::Integer, json!("5")));
        assert!(passes(Rule::Integer, json!("-5")));
        assert!(fails(Rule::Integer, json!("5.0")));
        assert!(fails(Rule::Integer, json!(5.5)));
        assert!(fails(Rule::Integer, json!("five")));
        assert!(fails(Rule::Integer, json!(true)));

        assert!(passes(Rule::Numeric, json!(1.5)));
        assert!(passes(Rule::Numeric, json!("2")));
        assert!(passes(Rule::Numeric, json!("1e3")));
        assert!(fails(Rule::Numeric, json!("NaN")));
        assert!(fails(Rule::Numeric, json!(true)));

        assert!(passes(Rule::String, json!("x")));
        assert!(fails(Rule::String, json!(1)));

        assert!(passes(Rule::Array, json!([])));
        assert!(passes(Rule::Array, json!([1, "a"])));
        assert!(fails(Rule::Array, json!({})));

        // json: strings must decode; structured values already are JSON;
        // null fails without `nullable`.
        assert!(passes(Rule::Json, json!(r#"{"a":1}"#)));
        assert!(passes(Rule::Json, json!("[1,2]")));
        assert!(passes(Rule::Json, json!("null")));
        assert!(passes(Rule::Json, json!({"a": 1})));
        assert!(fails(Rule::Json, json!("{not json")));
        assert!(fails(Rule::Json, Value::Null));

        assert!(passes(Rule::Ascii, json!("abc")));
        assert!(passes(Rule::Ascii, json!("")));
        assert!(fails(Rule::Ascii, json!("héllo")));
        assert!(fails(Rule::Ascii, json!(1)));
    }

    #[test]
    fn string_rules_pass_and_fail() {
        assert!(passes(Rule::Alpha, json!("héllo")));
        assert!(fails(Rule::Alpha, json!("héllo42")));
        assert!(fails(Rule::Alpha, json!("")));
        assert!(fails(Rule::Alpha, json!(42)), "alpha is string-only");

        assert!(passes(Rule::AlphaNum, json!("abc123")));
        assert!(
            passes(Rule::AlphaNum, json!(42)),
            "integers pass in Laravel"
        );
        assert!(fails(Rule::AlphaNum, json!("ab-c")));
        assert!(fails(Rule::AlphaNum, json!(4.5)));

        assert!(passes(Rule::AlphaDash, json!("ab-c_9")));
        assert!(fails(Rule::AlphaDash, json!("ab c")));

        assert!(passes(Rule::Lowercase, json!("abc")));
        assert!(passes(Rule::Lowercase, json!("123")));
        assert!(fails(Rule::Lowercase, json!("Abc")));
        assert!(fails(Rule::Lowercase, json!(1)), "non-strings fail");

        assert!(passes(Rule::Uppercase, json!("ABC")));
        assert!(fails(Rule::Uppercase, json!("Abc")));

        assert!(passes(
            Rule::StartsWith("kelson".into()),
            json!("kelson-http")
        ));
        assert!(fails(Rule::StartsWith("kelson".into()), json!("axum")));
        assert!(fails(Rule::StartsWith("kelson".into()), json!(1)));

        assert!(passes(Rule::EndsWith(".png".into()), json!("logo.png")));
        assert!(fails(Rule::EndsWith(".png".into()), json!("logo.jpg")));

        assert!(passes(Rule::Contains("@".into()), json!("a@b")));
        assert!(fails(Rule::Contains("@".into()), json!("ab")));

        assert!(passes(
            Rule::DoesntStartWith("tmp".into()),
            json!("report.pdf")
        ));
        assert!(fails(Rule::DoesntStartWith("tmp".into()), json!("tmpfile")));

        assert!(passes(
            Rule::DoesntEndWith(".exe".into()),
            json!("notes.txt")
        ));
        assert!(fails(
            Rule::DoesntEndWith(".exe".into()),
            json!("setup.exe")
        ));

        assert!(passes(
            Rule::DoesntContain("<script".into()),
            json!("safe text")
        ));
        assert!(fails(
            Rule::DoesntContain("<script".into()),
            json!("<script>alert(1)")
        ));
    }

    #[test]
    fn closed_grammars_pass_and_fail() {
        // hex_color: `#` is required (Laravel regex), 3/4/6/8 digits.
        assert!(passes(Rule::HexColor, json!("#fff")));
        assert!(passes(Rule::HexColor, json!("#aabbcc")));
        assert!(passes(Rule::HexColor, json!("#aabbccdd")));
        assert!(fails(Rule::HexColor, json!("fff")), "hash required");
        assert!(fails(Rule::HexColor, json!("#ab")));
        assert!(fails(Rule::HexColor, json!("#gggggg")));

        // mac_address: six octets, `:` or `-` separated.
        assert!(passes(Rule::MacAddress, json!("00:1A:2B:3C:4D:5E")));
        assert!(passes(Rule::MacAddress, json!("00-1a-2b-3c-4d-5e")));
        assert!(fails(Rule::MacAddress, json!("00:1A:2B:3C:4D")));
        assert!(fails(Rule::MacAddress, json!("001A2B3C4D5E")));
        assert!(
            fails(Rule::MacAddress, json!("00:1A:2B:3C:4D:5E:6F")),
            "EUI-64 not accepted"
        );

        assert!(passes(Rule::Timezone, json!("Europe/Berlin")));
        assert!(passes(Rule::Timezone, json!("UTC")));
        assert!(fails(Rule::Timezone, json!("Mars/Olympus")));
        assert!(fails(Rule::Timezone, json!("berlin")), "case-sensitive");

        // ulid: 26 chars of Crockford base32 (no I, L, O, U).
        assert!(passes(Rule::Ulid, json!("01HZYQ3F4K8N7V2C6M9P5RXTVB")));
        assert!(fails(Rule::Ulid, json!("01HZYQ3F4K8N7V2C6M9P5RXTV")));
        assert!(
            fails(Rule::Ulid, json!("01HZYQ3F4K8N7V2C6M9P5RXTVI")),
            "I is excluded"
        );
        assert!(
            fails(Rule::Ulid, json!("01hzyq3f4k8n7v2c6m9p5rxtvb")),
            "lowercase rejected"
        );
    }

    #[test]
    fn numeric_shape_rules_pass_and_fail() {
        // decimal:<n> = EXACTLY n decimal places (Laravel 12 semantics).
        assert!(passes(Rule::Decimal(2), json!("1.25")));
        assert!(passes(Rule::Decimal(2), json!(1.25)));
        assert!(passes(Rule::Decimal(0), json!("42")));
        assert!(passes(Rule::Decimal(0), json!(42)));
        assert!(fails(Rule::Decimal(2), json!("1.5")));
        assert!(fails(Rule::Decimal(2), json!("1")), "needs two places");
        assert!(fails(Rule::Decimal(2), json!("1.250")));
        assert!(fails(Rule::Decimal(2), json!("abc")));
        assert!(fails(Rule::Decimal(2), json!("1e3")), "exponent shape");

        assert!(passes(Rule::Digits(4), json!("1234")));
        assert!(passes(Rule::Digits(4), json!(1234)));
        assert!(fails(Rule::Digits(4), json!("123")));
        assert!(fails(Rule::Digits(4), json!("12a4")));
        assert!(fails(Rule::Digits(4), json!("-123")));
        assert!(fails(Rule::Digits(4), json!(1234.5)));

        assert!(passes(Rule::DigitsBetween(4, 6), json!("12345")));
        assert!(fails(Rule::DigitsBetween(4, 6), json!("123")));
        assert!(fails(Rule::DigitsBetween(4, 6), json!("1234567")));

        assert!(passes(Rule::MultipleOf(3.0), json!(9)));
        assert!(passes(Rule::MultipleOf(0.1), json!(0.3)), "float-safe");
        assert!(fails(Rule::MultipleOf(3.0), json!(10)));
        assert!(fails(Rule::MultipleOf(3.0), json!("abc")));
    }

    #[test]
    fn field_ref_comparisons_resolve_siblings() {
        let input = json!({ "floor": 18, "age": 21, "label": "x" });
        let ctx = ValidationContext::new(&input);

        // literal targets
        assert!(check(&Rule::Gt(NumTarget::Literal(18.0)), "age", &json!(21), &ctx).is_none());
        assert!(check(&Rule::Gt(NumTarget::Literal(18.0)), "age", &json!(18), &ctx).is_some());
        assert!(check(
            &Rule::Gte(NumTarget::Literal(18.0)),
            "age",
            &json!(18),
            &ctx
        )
        .is_none());
        assert!(check(&Rule::Lt(NumTarget::Literal(20.0)), "age", &json!(21), &ctx).is_some());
        assert!(check(
            &Rule::Lte(NumTarget::Literal(21.0)),
            "age",
            &json!(21),
            &ctx
        )
        .is_none());

        // field targets resolve from the shared input
        let age = Rule::Gt(NumTarget::Field("floor".into()));
        assert!(check(&age, "age", &json!(21), &ctx).is_none());
        assert!(check(&age, "age", &json!(17), &ctx).is_some());

        // missing / null / non-numeric references fail closed
        let missing = Rule::Gt(NumTarget::Field("absent".into()));
        let msg = check(&missing, "age", &json!(21), &ctx).expect("missing ref fails");
        assert!(msg.contains("absent"), "{msg}");

        let null_ref = json!({ "other": null });
        let null_ctx = ValidationContext::new(&null_ref);
        assert!(check(&missing, "age", &json!(21), &null_ctx).is_some());

        let non_num = json!({ "other": "abc" });
        let nn_ctx = ValidationContext::new(&non_num);
        let target = Rule::Gt(NumTarget::Field("other".into()));
        assert!(check(&target, "age", &json!(21), &nn_ctx).is_some());

        // non-numeric values under test fail with the number message
        let msg = check(&age, "age", &json!("abc"), &ctx).expect("non-number fails");
        assert_eq!(msg, "must be a number");

        // numeric strings compare numerically (Laravel is_numeric parity)
        assert!(check(&age, "age", &json!("21"), &ctx).is_none());
    }

    #[test]
    fn condition_token_matching_is_a_closed_grammar() {
        // numbers compare numerically, strings exactly.
        assert!(scalar_match(Some(&json!(21)), "21"));
        assert!(scalar_match(Some(&json!(21.0)), "21"));
        assert!(scalar_match(Some(&json!("21")), "21"));
        assert!(!scalar_match(Some(&json!(22)), "21"));
        assert!(!scalar_match(Some(&json!("guest")), "hot"));

        // bools only match the documented spellings — PHP's loose in_array
        // would make `true` equal ANY non-empty token (fail-open).
        assert!(scalar_match(Some(&json!(true)), "true"));
        assert!(scalar_match(Some(&json!(true)), "yes"));
        assert!(scalar_match(Some(&json!(false)), "0"));
        assert!(!scalar_match(Some(&json!(true)), "admin"));
        assert!(!scalar_match(Some(&json!(false)), "true"));

        // absent resolves as null: only the literal `null` token matches.
        assert!(scalar_match(None, "null"));
        assert!(scalar_match(Some(&Value::Null), "null"));
        assert!(!scalar_match(None, "x"));

        // structured values never match a scalar token.
        assert!(!scalar_match(Some(&json!(["a"])), "a"));
        assert!(!scalar_match(Some(&json!({"a": 1})), "a"));
    }

    #[test]
    fn wave1_rules_keep_their_wire_wording() {
        let msg = run(&Rule::Email, &json!("nope")).expect("fails");
        assert_eq!(msg, "must be a valid email");
        let msg = run(&Rule::Email, &json!(1)).expect("fails");
        assert_eq!(msg, "must be a string email");
        assert!(run(&Rule::Min(3), &json!("a")).is_some());
        assert!(run(&Rule::Range(1, 5), &json!(9)).is_some());
        assert!(passes(Rule::Date, json!("2026-08-20")));
        assert!(is_email("alice@example.co.in"));
        assert!(!is_email("alice@@example.com"));
        assert!(is_url("https://kelson.dev/docs"));
        assert!(!is_url("ftp://kelson.dev"));
        assert!(is_e164("+919876543210"));
        assert!(is_date("2026-08-20T10:00Z"));
        assert!(!is_date("20/08/2026"));
    }

    #[test]
    fn float_tolerance_and_multiple_of_edges() {
        assert!(is_multiple_of(0.3, 0.1));
        assert!(is_multiple_of(9.0, 3.0));
        assert!(!is_multiple_of(10.0, 3.0));
        assert!(!is_multiple_of(1.0, 0.0));
        assert!(!is_multiple_of(f64::NAN, 1.0));
    }
}
