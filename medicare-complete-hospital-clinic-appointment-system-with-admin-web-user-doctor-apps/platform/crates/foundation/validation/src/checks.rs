//! Rule evaluation (Wave 4 Phase 1): one `check` per [`Rule`], returning a
//! public-safe violation message or `None`. Pure functions except the four
//! numeric comparisons, which resolve sibling values through the
//! [`ValidationContext`] (`gt:<n|field>` field refs).
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
//!
//! No regex engine: every pattern here is a hand-rolled closed grammar (the
//! documented replacement for Laravel's `regex:` rule).

use crate::validator::{NumTarget, Rule, ValidationContext};
use serde_json::Value;

/// Apply one rule to one presence-resolved value. `ctx.input` is the whole
/// validated object so field-ref rules can resolve their sibling.
pub(crate) fn check(rule: &Rule, v: &Value, ctx: &ValidationContext<'_>) -> Option<String> {
    match rule {
        Rule::Required | Rule::Nullable => {
            unreachable!("structural rules are resolved before rule checks")
        }

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
                    && s
                        .chars()
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
        Rule::Digits(n) => digits_check(v, |len| len == *n as usize, || {
            format!(
                "must be exactly {n} digit{}",
                if *n == 1 { "" } else { "s" }
            )
        }),
        Rule::DigitsBetween(a, b) => digits_check(v, |len| {
            len >= *a as usize && len <= *b as usize
        }, || format!("must be between {a} and {b} digits")),
        Rule::MultipleOf(n) => match as_number(v) {
            None => Some("must be a number".into()),
            Some(x) if is_multiple_of(x, *n) => None,
            Some(_) => Some(format!("must be a multiple of {n}")),
        },
        Rule::Gt(target) => compare(v, target, ctx, Cmp::Gt),
        Rule::Gte(target) => compare(v, target, ctx, Cmp::Gte),
        Rule::Lt(target) => compare(v, target, ctx, Cmp::Lt),
        Rule::Lte(target) => compare(v, target, ctx, Cmp::Lte),
    }
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
    s.trim().parse::<f64>().map(|f| f.is_finite()).unwrap_or(false)
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

pub(crate) fn is_date(s: &str) -> bool {
    if time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).is_ok() {
        return true;
    }
    // RFC 3339 strictly requires seconds; also accept minute-precision
    // timestamps (T10:00Z / T10:00+05:30), the shape appointment slots use.
    if let Ok(fmt) =
        time::format_description::parse_borrowed::<2>("[year]-[month]-[day]T[hour]:[minute]Z")
    {
        if time::PrimitiveDateTime::parse(s, &fmt).is_ok() {
            return true;
        }
    }
    if let Ok(fmt) = time::format_description::parse_borrowed::<2>(
        "[year]-[month]-[day]T[hour]:[minute][offset_hour sign:mandatory]:[offset_minute]",
    ) {
        if time::OffsetDateTime::parse(s, &fmt).is_ok() {
            return true;
        }
    }
    match time::format_description::parse_borrowed::<2>("[year]-[month]-[day]") {
        Ok(fmt) => time::Date::parse(s, &fmt).is_ok(),
        Err(_) => false,
    }
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
    "UTC", "GMT", "UCT", "Universal", "Zulu", "Greenwich",
    "Etc/UTC", "Etc/GMT",
    // Africa
    "Africa/Abidjan", "Africa/Accra", "Africa/Addis_Ababa", "Africa/Algiers",
    "Africa/Asmara", "Africa/Bamako", "Africa/Bangui", "Africa/Banjul",
    "Africa/Bissau", "Africa/Blantyre", "Africa/Bujumbura", "Africa/Cairo",
    "Africa/Casablanca", "Africa/Ceuta", "Africa/Conakry", "Africa/Dakar",
    "Africa/Dar_es_Salaam", "Africa/Djibouti", "Africa/Douala",
    "Africa/El_Aaiun", "Africa/Freetown", "Africa/Gaborone", "Africa/Harare",
    "Africa/Juba", "Africa/Johannesburg", "Africa/Kampala", "Africa/Khartoum",
    "Africa/Kigali", "Africa/Kinshasa", "Africa/Kisumu", "Africa/Lagos",
    "Africa/Libreville", "Africa/Lome", "Africa/Luanda", "Africa/Lubumbashi",
    "Africa/Lusaka", "Africa/Malabo", "Africa/Maputo", "Africa/Maseru",
    "Africa/Mbabane", "Africa/Mogadishu", "Africa/Monrovia", "Africa/Nairobi",
    "Africa/Ndjamena", "Africa/Niamey", "Africa/Nouakchott",
    "Africa/Ouagadougou", "Africa/Porto-Novo", "Africa/Sao_Tome",
    "Africa/Tripoli", "Africa/Tunis", "Africa/Windhoek",
    // America
    "America/Adak", "America/Anchorage", "America/Anguilla",
    "America/Antigua", "America/Araguaina", "America/Argentina/Buenos_Aires",
    "America/Argentina/Catamarca", "America/Argentina/Cordoba",
    "America/Argentina/Jujuy", "America/Argentina/La_Rioja",
    "America/Argentina/Mendoza", "America/Argentina/Rio_Gallegos",
    "America/Argentina/Salta", "America/Argentina/San_Juan",
    "America/Argentina/San_Luis", "America/Argentina/Tucuman",
    "America/Argentina/Ushuaia", "America/Aruba", "America/Asuncion",
    "America/Atikokan", "America/Bahia", "America/Bahia_Banderas",
    "America/Barbados", "America/Belem", "America/Belize", "America/Blanc-Sablon",
    "America/Boise", "America/Cambridge_Bay", "America/Cancun",
    "America/Caracas", "America/Cayman", "America/Chicago",
    "America/Chihuahua", "America/Ciudad_Juarez", "America/Costa_Rica",
    "America/Cuiaba", "America/Curacao", "America/Dawson",
    "America/Dawson_Creek", "America/Denver", "America/Detroit",
    "America/Dominica", "America/Edmonton", "America/El_Salvador",
    "America/Fort_Nelson", "America/Fortaleza", "America/Glace_Bay",
    "America/Godthab", "America/Goose_Bay", "America/Grand_Turk",
    "America/Grenada", "America/Guadeloupe", "America/Guatemala",
    "America/Guayaquil", "America/Guyana", "America/Halifax", "America/Havana",
    "America/Hermosillo", "America/Indiana/Indianapolis",
    "America/Indiana/Knox", "America/Indiana/Marengo",
    "America/Indiana/Petersburg", "America/Indiana/Tell_City",
    "America/Indiana/Vevay", "America/Indiana/Vincennes",
    "America/Indiana/Winamac", "America/Indianapolis", "America/Inuvik",
    "America/Iqaluit", "America/Jamaica", "America/Juneau",
    "America/Kentucky/Louisville", "America/Kentucky/Monticello",
    "America/Lima", "America/Los_Angeles", "America/Maceio", "America/Managua",
    "America/Manaus", "America/Martinique", "America/Matamoros",
    "America/Mazatlan", "America/Mendoza", "America/Menominee",
    "America/Merida", "America/Metlakatla", "America/Mexico_City",
    "America/Moncton", "America/Monterrey", "America/Montevideo",
    "America/Montserrat", "America/Nassau", "America/New_York",
    "America/Nome", "America/Noronha", "America/North_Dakota/Center",
    "America/North_Dakota/New_Salem", "America/Nuuk", "America/Ojinaga",
    "America/Panama", "America/Pangnirtung", "America/Paramaribo",
    "America/Phoenix", "America/Port-au-Prince", "America/Port_of_Spain",
    "America/Puerto_Rico", "America/Punta_Arenas", "America/Regina",
    "America/Resolute", "America/Rio_Branco", "America/Santarem",
    "America/Santiago", "America/Santo_Domingo", "America/Sao_Paulo",
    "America/Scoresbysund", "America/Sitka", "America/St_Johns",
    "America/St_Kitts", "America/St_Lucia", "America/St_Thomas",
    "America/St_Vincent", "America/Swift_Current", "America/Tegucigalpa",
    "America/Thule", "America/Thunder_Bay", "America/Tijuana",
    "America/Toronto", "America/Vancouver", "America/Whitehorse",
    "America/Winnipeg", "America/Yakutat", "America/Yellowknife",
    // Antarctica
    "Antarctica/Casey", "Antarctica/Davis", "Antarctica/DumontDUrville",
    "Antarctica/Mawson", "Antarctica/Palmer", "Antarctica/Rothera",
    "Antarctica/Syowa", "Antarctica/Troll", "Antarctica/Vostok",
    // Asia
    "Asia/Almaty", "Asia/Amman", "Asia/Anadyr", "Asia/Aqtau", "Asia/Aqtobe",
    "Asia/Ashgabat", "Asia/Baghdad", "Asia/Baku", "Asia/Bangkok",
    "Asia/Barnaul", "Asia/Beirut", "Asia/Bishkek", "Asia/Brunei",
    "Asia/Chita", "Asia/Choibalsan", "Asia/Colombo", "Asia/Damascus",
    "Asia/Dhaka", "Asia/Dili", "Asia/Dubai", "Asia/Dushanbe",
    "Asia/Famagusta", "Asia/Gaza", "Asia/Hebron", "Asia/Ho_Chi_Minh",
    "Asia/Hong_Kong", "Asia/Hovd", "Asia/Irkutsk", "Asia/Jakarta",
    "Asia/Jerusalem", "Asia/Kabul", "Asia/Kathmandu", "Asia/Khandyga",
    "Asia/Kolkata", "Asia/Krasnoyarsk", "Asia/Kuala_Lumpur", "Asia/Kuching",
    "Asia/Macau", "Asia/Magadan", "Asia/Makassar", "Asia/Manila",
    "Asia/Muscat", "Asia/Novokuznetsk", "Asia/Novosibirsk", "Asia/Omsk",
    "Asia/Oral", "Asia/Phnom_Penh", "Asia/Pontianak", "Asia/Pyongyang",
    "Asia/Qatar", "Asia/Riyadh", "Asia/Sakhalin", "Asia/Samarkand",
    "Asia/Seoul", "Asia/Shanghai", "Asia/Singapore", "Asia/Srednekolymsk",
    "Asia/Taipei", "Asia/Tashkent", "Asia/Tbilisi", "Asia/Tehran",
    "Asia/Thimphu", "Asia/Tokyo", "Asia/Tomsk", "Asia/Ulaanbaatar",
    "Asia/Urumqi", "Asia/Ust-Nera", "Asia/Vientiane", "Asia/Vladivostok",
    "Asia/Yakutsk", "Asia/Yangon", "Asia/Yekaterinburg", "Asia/Yerevan",
    // Atlantic
    "Atlantic/Azores", "Atlantic/Bermuda", "Atlantic/Canary",
    "Atlantic/Cape_Verde", "Atlantic/Faroe", "Atlantic/Madeira",
    "Atlantic/Reykjavik", "Atlantic/South_Georgia", "Atlantic/St_Helena",
    "Atlantic/Stanley",
    // Australia + Pacific islands (Australian block)
    "Australia/Adelaide", "Australia/Brisbane", "Australia/Broken_Hill",
    "Australia/Currie", "Australia/Darwin", "Australia/Eucla",
    "Australia/Hobart", "Australia/Lindeman", "Australia/Lord_Howe",
    "Australia/Melbourne", "Australia/Perth", "Australia/Sydney",
    // Europe
    "Europe/Amsterdam", "Europe/Andorra", "Europe/Astrakhan", "Europe/Athens",
    "Europe/Belgrade", "Europe/Berlin", "Europe/Bratislava", "Europe/Brussels",
    "Europe/Bucharest", "Europe/Budapest", "Europe/Chisinau",
    "Europe/Copenhagen", "Europe/Dublin", "Europe/Gibraltar",
    "Europe/Helsinki", "Europe/Istanbul", "Europe/Kaliningrad",
    "Europe/Kyiv", "Europe/Lisbon", "Europe/Ljubljana", "Europe/London",
    "Europe/Luxembourg", "Europe/Madrid", "Europe/Malta", "Europe/Minsk",
    "Europe/Monaco", "Europe/Moscow", "Europe/Oslo", "Europe/Paris",
    "Europe/Prague", "Europe/Riga", "Europe/Rome", "Europe/Samara",
    "Europe/Sarajevo", "Europe/Saratov", "Europe/Simferopol", "Europe/Skopje",
    "Europe/Sofia", "Europe/Stockholm", "Europe/Tallinn", "Europe/Tirane",
    "Europe/Vaduz", "Europe/Vienna", "Europe/Vilnius", "Europe/Volgograd",
    "Europe/Warsaw", "Europe/Zurich",
    // Indian Ocean
    "Indian/Antananarivo", "Indian/Chagos", "Indian/Christmas",
    "Indian/Cocos", "Indian/Comoro", "Indian/Maldives", "Indian/Mauritius",
    "Indian/Mayotte", "Indian/Reunion",
    // Pacific
    "Pacific/Auckland", "Pacific/Bougainville", "Pacific/Chatham",
    "Pacific/Chuuk", "Pacific/Easter", "Pacific/Efate", "Pacific/Fakaofo",
    "Pacific/Fiji", "Pacific/Funafuti", "Pacific/Galapagos",
    "Pacific/Gambier", "Pacific/Guadalcanal", "Pacific/Guam",
    "Pacific/Honolulu", "Pacific/Honiara", "Pacific/Kiritimati",
    "Pacific/Kosrae", "Pacific/Kwajalein", "Pacific/Majuro",
    "Pacific/Marquesas", "Pacific/Midway", "Pacific/Nauru", "Pacific/Niue",
    "Pacific/Norfolk", "Pacific/Noumea", "Pacific/Pago_Pago",
    "Pacific/Palau", "Pacific/Pitcairn", "Pacific/Pohnpei",
    "Pacific/Port_Moresby", "Pacific/Rarotonga", "Pacific/Saipan",
    "Pacific/Tahiti", "Pacific/Tarawa", "Pacific/Tongatapu",
    "Pacific/Wake", "Pacific/Wallis",
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
        check(rule, v, &ctx)
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
        assert!(passes(Rule::AlphaNum, json!(42)), "integers pass in Laravel");
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
            Rule::StartsWith("keel".into()),
            json!("keel-http")
        ));
        assert!(fails(Rule::StartsWith("keel".into()), json!("axum")));
        assert!(fails(Rule::StartsWith("keel".into()), json!(1)));

        assert!(passes(Rule::EndsWith(".png".into()), json!("logo.png")));
        assert!(fails(Rule::EndsWith(".png".into()), json!("logo.jpg")));

        assert!(passes(Rule::Contains("@".into()), json!("a@b")));
        assert!(fails(Rule::Contains("@".into()), json!("ab")));

        assert!(passes(
            Rule::DoesntStartWith("tmp".into()),
            json!("report.pdf")
        ));
        assert!(fails(
            Rule::DoesntStartWith("tmp".into()),
            json!("tmpfile")
        ));

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

        assert!(passes(
            Rule::DigitsBetween(4, 6),
            json!("12345")
        ));
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
        assert!(check(&Rule::Gt(NumTarget::Literal(18.0)), &json!(21), &ctx).is_none());
        assert!(check(&Rule::Gt(NumTarget::Literal(18.0)), &json!(18), &ctx).is_some());
        assert!(check(&Rule::Gte(NumTarget::Literal(18.0)), &json!(18), &ctx).is_none());
        assert!(check(&Rule::Lt(NumTarget::Literal(20.0)), &json!(21), &ctx).is_some());
        assert!(check(&Rule::Lte(NumTarget::Literal(21.0)), &json!(21), &ctx).is_none());

        // field targets resolve from the shared input
        let age = Rule::Gt(NumTarget::Field("floor".into()));
        assert!(check(&age, &json!(21), &ctx).is_none());
        assert!(check(&age, &json!(17), &ctx).is_some());

        // missing / null / non-numeric references fail closed
        let missing = Rule::Gt(NumTarget::Field("absent".into()));
        let msg = check(&missing, &json!(21), &ctx).expect("missing ref fails");
        assert!(msg.contains("absent"), "{msg}");

        let null_ref = json!({ "other": null });
        let null_ctx = ValidationContext::new(&null_ref);
        assert!(check(&missing, &json!(21), &null_ctx).is_some());

        let non_num = json!({ "other": "abc" });
        let nn_ctx = ValidationContext::new(&non_num);
        let target = Rule::Gt(NumTarget::Field("other".into()));
        assert!(check(&target, &json!(21), &nn_ctx).is_some());

        // non-numeric values under test fail with the number message
        let msg = check(&age, &json!("abc"), &ctx).expect("non-number fails");
        assert_eq!(msg, "must be a number");

        // numeric strings compare numerically (Laravel is_numeric parity)
        assert!(check(&age, &json!("21"), &ctx).is_none());
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
        assert!(is_url("https://keel.dev/docs"));
        assert!(!is_url("ftp://keel.dev"));
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
