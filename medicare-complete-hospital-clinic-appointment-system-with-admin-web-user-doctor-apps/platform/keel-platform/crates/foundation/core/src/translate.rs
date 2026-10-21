//! Message catalog + translator (blueprint L50, S3 item 43).
//!
//! Framework localization is template-based: catalogs hold `{placeholder}`
//! templates per locale; [`translate`] resolves through the fallback chain
//! (exact locale -> language part -> catalog default) and substitutes named
//! parameters. No OS locale APIs, no ICU dependency.

use std::collections::BTreeMap;

use crate::AppError;

/// Validate a locale tag (Wave 6 P3: pub so boot loaders can check a
/// configured default before any file defines it).
pub fn validate_locale(raw: &str) -> Result<(), AppError> {
    if raw.is_empty() || raw.len() > 16 {
        return Err(AppError::validation(format!(
            "invalid locale {raw:?}: bad length"
        )));
    }
    for seg in raw.split('-') {
        if seg.is_empty() || !seg.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(AppError::validation(format!(
                "invalid locale segment {seg:?}"
            )));
        }
    }
    Ok(())
}

fn validate_key(raw: &str) -> Result<(), AppError> {
    if raw.is_empty() {
        return Err(AppError::validation("message key is empty"));
    }
    for seg in raw.split('.') {
        let ok = !seg.is_empty()
            && seg.len() <= 64
            && seg
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-');
        if !ok {
            return Err(AppError::validation(format!(
                "invalid message key segment {seg:?}"
            )));
        }
    }
    Ok(())
}

/// Per-locale message templates keyed by dot-separated message keys.
#[derive(Debug, Clone, Default)]
pub struct MessageCatalog {
    default_locale: String,
    messages: BTreeMap<String, BTreeMap<String, String>>,
}

impl MessageCatalog {
    /// Empty catalog falling back to `default_locale` (e.g. `en`).
    pub fn new(default_locale: impl Into<String>) -> Self {
        let default_locale = default_locale.into();
        Self {
            default_locale,
            messages: BTreeMap::new(),
        }
    }

    pub fn default_locale(&self) -> &str {
        &self.default_locale
    }

    /// Define one template; locale/key formats validated at registration.
    pub fn define(
        &mut self,
        locale: &str,
        key: &str,
        template: impl Into<String>,
    ) -> Result<(), AppError> {
        validate_locale(locale)?;
        validate_key(key)?;
        self.messages
            .entry(locale.to_string())
            .or_default()
            .insert(key.to_string(), template.into());
        Ok(())
    }

    /// Bulk-load templates for one locale.
    pub fn load(&mut self, locale: &str, entries: &[(&str, &str)]) -> Result<(), AppError> {
        for (key, template) in entries {
            self.define(locale, key, *template)?;
        }
        Ok(())
    }

    pub fn get(&self, locale: &str, key: &str) -> Option<&str> {
        self.messages.get(locale)?.get(key).map(|s| s.as_str())
    }

    pub fn locales(&self) -> Vec<&str> {
        self.messages.keys().map(|s| s.as_str()).collect()
    }

    /// Wave 6 P3: chain-merged messages for `locale` as one flat map of
    /// dotted keys — the view edge injects this as the `_lang` context, so
    /// `@lang` and the `trans` filter resolve through the fallback chain
    /// with later entries overlaying earlier ones (exact locale wins).
    pub fn flat(&self, locale: &str) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        // Reverse: default first, language next, exact last (it overwrites).
        for candidate in self.fallback_chain(locale).into_iter().rev() {
            if let Some(messages) = self.messages.get(&candidate) {
                for (key, template) in messages {
                    out.insert(key.clone(), template.clone());
                }
            }
        }
        out
    }

    /// Fallback chain for a locale request: exact tag, then its language
    /// part, then the catalog default (deduplicated, in that order).
    pub fn fallback_chain(&self, locale: &str) -> Vec<String> {
        let mut chain = Vec::with_capacity(3);
        if !locale.is_empty() {
            chain.push(locale.to_string());
            if let Some(language) = locale.split('-').next() {
                if !language.is_empty() && language != locale {
                    chain.push(language.to_string());
                }
            }
        }
        if !self.default_locale.is_empty() && !chain.contains(&self.default_locale) {
            chain.push(self.default_locale.clone());
        }
        chain
    }
}

/// Substitute `{name}` placeholders; unknown names are left verbatim so a
/// missing parameter is visible rather than silently empty.
pub fn render_template(template: &str, params: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) => {
                let name = &after[..close];
                match params.iter().find(|(k, _)| *k == name) {
                    Some((_, value)) => out.push_str(value),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = &after[close + 1..];
            }
            None => {
                out.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Translation outcome: rendered text + which locale satisfied it (None when
/// the key was unresolved everywhere and the key itself was returned).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Translated {
    pub text: String,
    pub locale_used: Option<String>,
}

/// Translate `key` for `locale` against `catalog`, substituting `params`,
/// walking the fallback chain (see [`MessageCatalog::fallback_chain`]).
pub fn translate(
    catalog: &MessageCatalog,
    locale: &str,
    key: &str,
    params: &[(&str, &str)],
) -> Translated {
    for candidate in catalog.fallback_chain(locale) {
        if let Some(template) = catalog.get(&candidate, key) {
            return Translated {
                text: render_template(template, params),
                locale_used: Some(candidate),
            };
        }
    }
    Translated {
        text: key.to_string(),
        locale_used: None,
    }
}

/// The six CLDR-style plural buckets a choice template may name.
pub const CHOICE_BUCKETS: [&str; 6] = ["zero", "one", "two", "few", "many", "other"];

/// Wave 6 P3: CLDR-style plural bucket for `n` in `locale` — a hand-rolled
/// rule table covering the shipped locales (en, fr, ar); every other
/// locale falls back to `other` (spec: unsupported locale = `other`).
/// `n` is the absolute-count convention (CLDR evaluates |n|).
pub fn plural_bucket(locale: &str, n: i64) -> &'static str {
    let language = locale.split('-').next().unwrap_or(locale);
    match language {
        // CLDR en: `one` for |n| == 1.
        "en" => {
            if n == 1 || n == -1 {
                "one"
            } else {
                "other"
            }
        }
        // fr (hand-rolled): `zero` for 0 so "Aucun rendez-vous" reads
        // correctly — CLDR groups 0 with `one`, which would render
        // "Un rendez-vous" at zero; 1 and the rest follow CLDR.
        "fr" => match n {
            0 => "zero",
            1 => "one",
            _ => "other",
        },
        // CLDR ar: zero 0 · one 1 · two 2 · few n%100 in 3..=10 ·
        // many n%100 in 11..=99 · other (100, 101, ...).
        "ar" => match n {
            0 => "zero",
            1 => "one",
            2 => "two",
            _ => match n % 100 {
                3..=10 => "few",
                11..=99 => "many",
                _ => "other",
            },
        },
        _ => "other",
    }
}

/// Pick the bucket form out of a pipe-separated choice template. Every
/// segment must be a `bucket=text` pair to enter named mode; otherwise the
/// template is plain text (it still gets `{count}`). Selection order:
/// requested bucket -> `other` -> first form.
fn pick_choice_form(template: &str, bucket: &str) -> String {
    let forms: Vec<(&str, &str)> = template
        .split('|')
        .map(str::trim)
        .map(|segment| {
            let mut parts = segment.splitn(2, '=');
            let name = parts.next().unwrap_or("").trim();
            let body = parts.next().map(str::trim).unwrap_or("");
            (name, body)
        })
        .collect();
    let named = !forms.is_empty()
        && forms
            .iter()
            .all(|(name, body)| !body.is_empty() && CHOICE_BUCKETS.contains(name));
    if !named {
        return template.to_string();
    }
    forms
        .iter()
        .find(|(name, _)| *name == bucket)
        .or_else(|| forms.iter().find(|(name, _)| *name == "other"))
        .or_else(|| forms.first())
        .map(|(_, body)| body.to_string())
        .unwrap_or_else(|| template.to_string())
}

/// Wave 6 P3: pluralized translation (spec subset of
/// `trans_choice("key", n)`). The template is a pipe-separated list of
/// bucket forms (`zero=... | one=... | other=...`) selected by
/// [`plural_bucket`] for the requested locale; a plain (form-less)
/// template passes through as-is. Resolution walks the same fallback chain
/// as [`translate`] and substitutes `{count}`; an unresolved key returns
/// the key with `locale_used: None`.
pub fn trans_choice(catalog: &MessageCatalog, locale: &str, key: &str, n: i64) -> Translated {
    for candidate in catalog.fallback_chain(locale) {
        if let Some(template) = catalog.get(&candidate, key) {
            // Bucket rules follow the REQUESTED locale (spec), even when the
            // message itself resolved through the fallback chain.
            let form = pick_choice_form(template, plural_bucket(locale, n));
            let count = n.to_string();
            return Translated {
                text: render_template(&form, &[("count", count.as_str())]),
                locale_used: Some(candidate),
            };
        }
    }
    Translated {
        text: key.to_string(),
        locale_used: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> MessageCatalog {
        let mut c = MessageCatalog::new("en");
        c.load(
            "en",
            &[
                (
                    "appointment.booked",
                    "Appointment booked for {patient} on {date}.",
                ),
                ("appointment.cancelled", "Appointment cancelled."),
            ],
        )
        .unwrap();
        c.define(
            "hi",
            "appointment.booked",
            "{date} par {patient} ka appointment booked hua.",
        )
        .unwrap();
        c.define("en-IN", "appointment.patience", "Please wait, {patient}.")
            .unwrap();
        c
    }

    #[test]
    fn exact_locale_hit() {
        let out = translate(
            &catalog(),
            "hi",
            "appointment.booked",
            &[("patient", "Asha"), ("date", "21 Aug")],
        );
        assert_eq!(out.locale_used.as_deref(), Some("hi"));
        assert!(out.text.contains("Asha") && out.text.contains("21 Aug"));
    }

    #[test]
    fn regional_falls_back_to_language() {
        let c = catalog();
        // en-IN has the key only on en; chain en-IN -> en satisfies it.
        let out = translate(&c, "en-IN", "appointment.cancelled", &[]);
        assert_eq!(out.locale_used.as_deref(), Some("en"));
        assert_eq!(out.text, "Appointment cancelled.");
    }

    #[test]
    fn regional_key_found_before_fallback() {
        let out = translate(
            &catalog(),
            "en-IN",
            "appointment.patience",
            &[("patient", "Asha")],
        );
        assert_eq!(out.locale_used.as_deref(), Some("en-IN"));
        assert_eq!(out.text, "Please wait, Asha.");
    }

    #[test]
    fn unknown_locale_uses_default() {
        let out = translate(&catalog(), "fr", "appointment.cancelled", &[]);
        assert_eq!(out.locale_used.as_deref(), Some("en"));
    }

    #[test]
    fn unresolved_key_returns_key() {
        let out = translate(&catalog(), "en", "appointment.missing", &[]);
        assert_eq!(out.locale_used, None);
        assert_eq!(out.text, "appointment.missing");
    }

    #[test]
    fn unknown_placeholder_stays_visible() {
        let out = translate(
            &catalog(),
            "en",
            "appointment.booked",
            &[("patient", "Asha")],
        );
        assert!(
            out.text.contains("{date}"),
            "missing param must stay visible: {}",
            out.text
        );
    }

    #[test]
    fn repeated_placeholder_substituted_everywhere() {
        let mut c = MessageCatalog::new("en");
        c.define("en", "echo", "{x} and {x}").unwrap();
        let out = translate(&c, "en", "echo", &[("x", "1")]);
        assert_eq!(out.text, "1 and 1");
    }

    #[test]
    fn malformed_locale_and_key_rejected() {
        let mut c = MessageCatalog::new("en");
        assert!(c.define("bad locale", "k", "v").is_err());
        assert!(c.define("en", "UPPER.key", "v").is_err());
        assert!(c.define("en", "ok..key", "v").is_err());
        assert!(c.define("en-IN", "ok.key", "v").is_ok());
    }

    #[test]
    fn fallback_chain_order() {
        let c = catalog();
        assert_eq!(
            c.fallback_chain("en-IN"),
            vec!["en-IN".to_string(), "en".to_string()]
        );
        assert_eq!(c.fallback_chain("en"), vec!["en".to_string()]);
        assert_eq!(c.fallback_chain(""), vec!["en".to_string()]);
    }

    #[test]
    fn unclosed_brace_passes_through() {
        let mut c = MessageCatalog::new("en");
        c.define("en", "raw", "broken {x").unwrap();
        let out = translate(&c, "en", "raw", &[("x", "y")]);
        assert_eq!(out.text, "broken {x");
    }

    /// Wave 6 P3: plural bucket matrix — shipped locales follow CLDR,
    /// regional tags match through their language part, and every
    /// unsupported locale collapses to `other` (spec).
    #[test]
    fn plural_bucket_matrix() {
        // en: one for |n| == 1.
        assert_eq!(plural_bucket("en", 1), "one");
        assert_eq!(plural_bucket("en", -1), "one");
        assert_eq!(plural_bucket("en", 0), "other");
        assert_eq!(plural_bucket("en", 2), "other");
        assert_eq!(plural_bucket("en-IN", 1), "one");
        // fr: explicit zero form, one for 1 (the shipped deviation: CLDR
        // groups 0 with `one`, which would render "Un rendez-vous" at 0).
        assert_eq!(plural_bucket("fr", 0), "zero");
        assert_eq!(plural_bucket("fr", 1), "one");
        assert_eq!(plural_bucket("fr", 2), "other");
        // ar: the full six-bucket matrix, CLDR modulo boundaries.
        assert_eq!(plural_bucket("ar", 0), "zero");
        assert_eq!(plural_bucket("ar", 1), "one");
        assert_eq!(plural_bucket("ar", 2), "two");
        assert_eq!(plural_bucket("ar", 3), "few");
        assert_eq!(plural_bucket("ar", 10), "few");
        assert_eq!(plural_bucket("ar", 11), "many");
        assert_eq!(plural_bucket("ar", 99), "many");
        assert_eq!(plural_bucket("ar", 100), "other");
        assert_eq!(plural_bucket("ar", 103), "few");
        assert_eq!(plural_bucket("ar", 111), "many");
        // Unsupported locales fall back to `other`.
        assert_eq!(plural_bucket("de", 1), "other");
        assert_eq!(plural_bucket("de-CH", 0), "other");
    }

    fn choice_catalog() -> MessageCatalog {
        let mut c = MessageCatalog::new("en");
        c.load(
            "en",
            &[(
                "inbox.count",
                "zero=No messages | one=One message | other={count} messages",
            )],
        )
        .unwrap();
        c.define("fr", "inbox.count", "one=Un message | other={count} messages")
            .unwrap();
        c.define(
            "ar",
            "inbox.count",
            "zero=لا رسائل | one=رسالة واحدة | two=رسالتان | few={count} رسائل | many={count} رسالة | other={count} رسالة",
        )
        .unwrap();
        c.define("en", "inbox.plain", "processing {count} rows")
            .unwrap();
        c
    }

    /// Wave 6 P3: `trans_choice` picks the bucket form for the requested
    /// locale (full ar matrix), substitutes `{count}`, honors the fallback
    /// chain, and degrades honestly on plain templates and missing keys.
    #[test]
    fn trans_choice_picks_buckets_and_substitutes_count() {
        let c = choice_catalog();
        // en: one for 1, other otherwise; locale resolves exactly.
        let one = trans_choice(&c, "en", "inbox.count", 1);
        assert_eq!(one.text, "One message");
        assert_eq!(one.locale_used.as_deref(), Some("en"));
        assert_eq!(trans_choice(&c, "en", "inbox.count", 0).text, "0 messages");
        assert_eq!(
            trans_choice(&c, "en", "inbox.count", 7).text,
            "7 messages"
        );
        // ar: zero/two/few/many/other all reachable.
        let ar = |n: i64| trans_choice(&c, "ar", "inbox.count", n).text;
        assert_eq!(ar(0), "لا رسائل");
        assert_eq!(ar(1), "رسالة واحدة");
        assert_eq!(ar(2), "رسالتان");
        assert_eq!(ar(5), "5 رسائل");
        assert_eq!(ar(11), "11 رسالة");
        assert_eq!(ar(100), "100 رسالة");
        // Unsupported locale: the message resolves through the chain but
        // the bucket is `other` even when n = 1.
        let de = trans_choice(&c, "de", "inbox.count", 1);
        assert_eq!(de.locale_used.as_deref(), Some("en"));
        assert_eq!(de.text, "1 messages");
        // A plain template (no bucket forms) passes through with {count}.
        assert_eq!(
            trans_choice(&c, "en", "inbox.plain", 3).text,
            "processing 3 rows"
        );
        // Unresolved key: the key itself, no locale claim.
        let miss = trans_choice(&c, "fr", "missing.key", 2);
        assert_eq!(miss.text, "missing.key");
        assert!(miss.locale_used.is_none());
    }

    /// Wave 6 P3: `flat()` merges the whole fallback chain into one dotted
    /// map (the view edge's `_lang` injection), exact locale overlaying
    /// the language part, which overlays the catalog default.
    #[test]
    fn flat_merges_the_fallback_chain_exact_wins() {
        let mut c = MessageCatalog::new("en");
        c.define("en", "greet", "Hello").unwrap();
        c.define("en", "only.en", "EN only").unwrap();
        c.define("fr", "greet", "Bonjour").unwrap();

        let flat = c.flat("fr-CA");
        assert_eq!(flat.get("greet").map(String::as_str), Some("Bonjour"));
        assert_eq!(flat.get("only.en").map(String::as_str), Some("EN only"));

        // An unknown locale still yields the default's messages.
        let flat = c.flat("de");
        assert_eq!(flat.get("greet").map(String::as_str), Some("Hello"));
    }

    /// **LF-078**: the locale is a VALUE, never ambient state — switching the
    /// locale and switching back returns the original text, and a
    /// translation never leaves the catalog in another language.
    #[test]
    fn a_locale_switch_never_leaks_into_the_next_translation() {
        let c = catalog();
        // Switch to another locale ...
        let hi = translate(
            &c,
            "hi",
            "appointment.booked",
            &[("patient", "Asha"), ("date", "21 Aug")],
        );
        assert_eq!(hi.locale_used.as_deref(), Some("hi"));
        // ... and back: the original locale is intact.
        let en = translate(
            &c,
            "en",
            "appointment.booked",
            &[("patient", "Asha"), ("date", "21 Aug")],
        );
        assert_eq!(en.locale_used.as_deref(), Some("en"));
        assert!(
            en.text.starts_with("Appointment booked"),
            "the original locale is intact: {}",
            en.text
        );
        // A regional tag still resolves through its language part.
        let regional = translate(&c, "en-IN", "appointment.cancelled", &[]);
        assert_eq!(regional.locale_used.as_deref(), Some("en"));
    }

    /// **LF-078**: two concurrent translations in different locales never
    /// cross — the catalog is immutable, so one request's locale cannot leak
    /// into another's message.
    #[test]
    fn concurrent_locales_never_cross() {
        let c = std::sync::Arc::new(catalog());
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let mut handles = Vec::new();
        for (locale, key) in [("hi", "appointment.booked"), ("en-IN", "appointment.patience")] {
            let c = c.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                translate(&c, locale, key, &[("patient", "Asha")])
            }));
        }
        let outs: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(
            outs[0].locale_used.as_deref(),
            Some("hi"),
            "the first thread kept its locale"
        );
        assert_eq!(
            outs[1].locale_used.as_deref(),
            Some("en-IN"),
            "the second thread kept its locale"
        );
        assert!(outs[0].text.contains("21 Aug") || outs[0].text.contains("Asha"));
        assert!(outs[1].text.contains("Please wait"));
    }
}
