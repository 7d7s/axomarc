//! Message catalog + translator (blueprint L50, S3 item 43).
//!
//! Framework localization is template-based: catalogs hold `{placeholder}`
//! templates per locale; [`translate`] resolves through the fallback chain
//! (exact locale -> language part -> catalog default) and substitutes named
//! parameters. No OS locale APIs, no ICU dependency.

use std::collections::BTreeMap;

use crate::AppError;

fn validate_locale(raw: &str) -> Result<(), AppError> {
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
}
