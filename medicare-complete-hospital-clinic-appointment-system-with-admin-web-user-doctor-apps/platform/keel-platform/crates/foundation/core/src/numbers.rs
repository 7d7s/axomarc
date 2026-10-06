//! Number helpers (Wave 8 P2, docs/19): locale-aware currency, percentage,
//! precision formatting and `clamp`.
//!
//! Locale-awareness rides the Wave 6 locale identities (the shipped `en` /
//! `fr` / `ar` catalogs): the PRIMARY subtag selects the rules (`en-IN`,
//! `fr-CA` match; an unknown locale falls back to `en` rules — display-only,
//! never fail-closed). Documented subset: ASCII digits everywhere (no
//! Arabic-Indic transliteration), 3-digit grouping everywhere (no Indian
//! lakh grouping), fixed offsets per locale. Currency symbols come from a
//! curated ISO table (unknown code renders the code itself) and minor units
//! from the same table (`JPY`/`KRW` = 0, default 2).

use crate::money::MoneyCents;

/// Per-locale rendering rules (documented subset above).
struct NumberRules {
    decimal: char,
    group: char,
    /// Symbol before the number (`$1.00`) vs after (`1,00 €`).
    symbol_before: bool,
    /// Gap between symbol and number for the after-position form.
    symbol_gap: bool,
    /// Percent sign (locale-specific) and the gap before it.
    percent_sign: &'static str,
    percent_gap: bool,
}

fn rules_for(locale: &str) -> NumberRules {
    let primary = locale
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    match primary.as_str() {
        "fr" => NumberRules {
            decimal: ',',
            group: ' ',
            symbol_before: false,
            symbol_gap: true,
            percent_sign: "%",
            percent_gap: true,
        },
        "ar" => NumberRules {
            decimal: '٫',  // ARABIC DECIMAL SEPARATOR
            group: '٬',    // ARABIC THOUSANDS SEPARATOR
            symbol_before: false,
            symbol_gap: true,
            percent_sign: "٪", // ARABIC PERCENT SIGN
            percent_gap: false,
        },
        // "en" and every unknown locale.
        _ => NumberRules {
            decimal: '.',
            group: ',',
            symbol_before: true,
            symbol_gap: false,
            percent_sign: "%",
            percent_gap: false,
        },
    }
}

/// Curated ISO-4217 symbol table; an unknown code renders as itself.
fn currency_symbol(code: &str) -> &'static str {
    match code {
        "USD" => "$",
        "EUR" => "€",
        "GBP" => "£",
        "INR" => "₹",
        "JPY" => "¥",
        "CNY" => "¥",
        "KRW" => "₩",
        "AUD" => "A$",
        "CAD" => "C$",
        "SGD" => "S$",
        "CHF" => "CHF",
        "BRL" => "R$",
        "ZAR" => "R",
        "AED" => "AED",
        _ => "", // caller renders the code itself
    }
}

/// Minor units for the code (curated table; default 2).
fn minor_units(code: &str) -> usize {
    match code {
        "JPY" | "KRW" => 0,
        _ => 2,
    }
}

/// Fixed-point text of a magnitude (no sign), `decimals` places.
fn fixed_text(magnitude: f64, decimals: usize) -> String {
    format!("{magnitude:.*}", decimals)
}

/// Apply locale grouping to integer digits and swap the decimal point.
fn localize(text: &str, rules: &NumberRules) -> String {
    let (int, frac) = match text.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (text, None),
    };
    let mut grouped = String::with_capacity(int.len() + int.len() / 3 + 2);
    for (index, c) in int.char_indices() {
        if index > 0 && (int.len() - index) % 3 == 0 {
            grouped.push(rules.group);
        }
        grouped.push(c);
    }
    if let Some(f) = frac {
        grouped.push(rules.decimal);
        grouped.push_str(f);
    }
    grouped
}

/// Non-finite inputs (NaN/inf) render their Rust text as-is instead of
/// running through grouping/separators that would mangle them.
fn non_finite_passthrough(value: f64) -> Option<String> {
    (!value.is_finite()).then(|| value.to_string())
}

/// Currency formatting: symbol from the ISO table, decimals from the
/// minor-unit table, grouping + separators from the locale, sign outermost
/// (`-` before the symbol in symbol-before locales).
pub fn format_currency(value: f64, currency: &str, locale: &str) -> String {
    if let Some(passthrough) = non_finite_passthrough(value) {
        return passthrough;
    }
    let rules = rules_for(locale);
    let (sign, magnitude) = split_sign(value);
    let body = localize(&fixed_text(magnitude, minor_units(currency)), &rules);
    let symbol = currency_symbol(currency);
    let symbol = if symbol.is_empty() { currency } else { symbol };
    let gap = if rules.symbol_gap { " " } else { "" };
    if rules.symbol_before {
        format!("{sign}{symbol}{body}")
    } else {
        format!("{sign}{body}{gap}{symbol}")
    }
}

/// `MoneyCents` rendering: the type's own contract is hundredths, so this
/// always renders 2 decimals (the minor-unit table applies to
/// [`format_currency`], whose input is a plain value).
pub fn format_cents(cents: &MoneyCents, locale: &str) -> String {
    format_currency(cents.amount as f64 / 100.0, cents.currency_str(), locale)
}

/// Percentage formatting: `value` IS the percentage (75 renders `75%`),
/// locale decimal separator + sign, no thousands grouping (documented).
pub fn format_percent(value: f64, decimals: usize, locale: &str) -> String {
    if let Some(passthrough) = non_finite_passthrough(value) {
        return passthrough;
    }
    let rules = rules_for(locale);
    let (sign, magnitude) = split_sign(value);
    let body = fixed_text(magnitude, decimals).replace('.', &rules.decimal.to_string());
    let gap = if rules.percent_gap { " " } else { "" };
    format!("{sign}{body}{gap}{}", rules.percent_sign)
}

/// Precision formatting: exactly `decimals` places with locale grouping
/// and separator (`1,234.50` en / `1 234,50` fr).
pub fn format_precision(value: f64, decimals: usize, locale: &str) -> String {
    if let Some(passthrough) = non_finite_passthrough(value) {
        return passthrough;
    }
    let rules = rules_for(locale);
    let (sign, magnitude) = split_sign(value);
    format!("{sign}{}", localize(&fixed_text(magnitude, decimals), &rules))
}

/// Bound `value` into `[min, max]`. `NaN` inputs and a degenerate range
/// (`min > max` or a non-finite bound) are unrepresentable answers — both
/// return `NaN` (never a panic inside the TCB); `±inf` clamps to the
/// matching bound like the standard library does.
pub fn clamp(value: f64, min: f64, max: f64) -> f64 {
    if value.is_nan()
        || min > max
        || !min.is_finite()
        || !max.is_finite()
    {
        return f64::NAN;
    }
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

/// Separate the sign so it can sit OUTSIDE the symbol/grouping (`abs`
/// normalizes `-0.0`, which would otherwise print a lone inner minus).
fn split_sign(value: f64) -> (String, f64) {
    if value < 0.0 {
        ("-".into(), -value)
    } else {
        (String::new(), value.abs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn currency_formats_per_locale_with_the_iso_symbol() {
        assert_eq!(format_currency(1234.56, "USD", "en"), "$1,234.56");
        assert_eq!(format_currency(1234.56, "EUR", "fr"), "1 234,56 €");
        assert_eq!(format_currency(1234.56, "USD", "ar"), "1٬234٫56 $");
        assert_eq!(format_currency(1234.56, "GBP", "en-GB"), "£1,234.56");
        // Sign sits outermost, before the symbol.
        assert_eq!(format_currency(-5.0, "USD", "en"), "-$5.00");
        assert_eq!(format_currency(-5.0, "EUR", "fr"), "-5,00 €");
    }

    #[test]
    fn currency_minor_units_and_fallbacks_are_documented() {
        // JPY/KRW carry 0 minor units; everything else defaults to 2.
        assert_eq!(format_currency(1234.56, "JPY", "en"), "¥1,235");
        assert_eq!(format_currency(1234.0, "KRW", "en"), "₩1,234");
        // Primary-subtag match: en-IN selects en rules.
        assert_eq!(format_currency(1234.56, "USD", "en-IN"), "$1,234.56");
        // Unknown locale falls back to en rules (display-only).
        assert_eq!(format_currency(1234.56, "USD", "de"), "$1,234.56");
        // Unknown currency code renders the code itself.
        assert_eq!(format_currency(1234.56, "XYZ", "en"), "XYZ1,234.56");
        // Non-finite inputs pass through instead of mangling separators.
        assert_eq!(format_currency(f64::NAN, "USD", "en"), "NaN");
    }

    #[test]
    fn cents_renders_the_money_type_in_both_placements() {
        let cents = MoneyCents::new(123_456, "USD").expect("iso code");
        assert_eq!(format_cents(&cents, "en"), "$1,234.56");
        assert_eq!(format_cents(&cents, "fr"), "1 234,56 $");
        let owed = MoneyCents::new(-500, "USD").expect("iso code");
        assert_eq!(format_cents(&owed, "en"), "-$5.00");
    }

    #[test]
    fn percent_uses_the_locale_separator_and_sign() {
        assert_eq!(format_percent(75.0, 1, "en"), "75.0%");
        assert_eq!(format_percent(75.0, 1, "fr"), "75,0 %");
        assert_eq!(format_percent(75.0, 1, "ar"), "75٫0٪");
        assert_eq!(format_percent(33.333, 2, "en"), "33.33%");
        assert_eq!(format_percent(-5.0, 1, "en"), "-5.0%");
        // Percent is a display number, not a value: no grouping (documented).
        assert_eq!(format_percent(1234.5, 1, "en"), "1234.5%");
    }

    #[test]
    fn precision_groups_and_pins_the_scale() {
        assert_eq!(format_precision(1234.5, 2, "en"), "1,234.50");
        assert_eq!(format_precision(1234.5, 2, "fr"), "1 234,50");
        assert_eq!(format_precision(1.2, 3, "en"), "1.200");
        assert_eq!(format_precision(1235.0, 0, "en"), "1,235");
        assert_eq!(format_precision(-1234.5, 2, "en"), "-1,234.50");
        assert_eq!(format_precision(999.999, 2, "en"), "1,000.00");
        assert_eq!(format_precision(f64::INFINITY, 2, "fr"), "inf");
    }

    #[test]
    fn clamp_bounds_and_degenerate_ranges_never_panic() {
        assert_eq!(clamp(5.0, 0.0, 10.0), 5.0);
        assert_eq!(clamp(-1.0, 0.0, 10.0), 0.0);
        assert_eq!(clamp(11.0, 0.0, 10.0), 10.0);
        assert_eq!(clamp(0.0, 0.0, 10.0), 0.0);
        assert_eq!(clamp(10.0, 0.0, 10.0), 10.0);
        assert!(clamp(f64::NAN, 0.0, 10.0).is_nan());
        // Degenerate (min > max) is an unrepresentable answer, not a panic.
        assert!(clamp(1.0, 10.0, 0.0).is_nan());
        assert!(clamp(1.0, 0.0, f64::INFINITY).is_nan());
        // Infinities clamp to the matching bound.
        assert_eq!(clamp(f64::INFINITY, 0.0, 10.0), 10.0);
        assert_eq!(clamp(f64::NEG_INFINITY, 0.0, 10.0), 0.0);
    }
}
