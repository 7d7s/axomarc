//! Wave 6 P3: locale resolution at the HTTP edge (docs/19 `i18n`).
//!
//! Spec order: session preference -> `Accept-Language` (first supported
//! locale, quality-ordered) -> the configured default (the catalog's
//! default locale). The parser/matcher are pure so the order is unit
//! testable. "Supported" = the locales the boot loader actually shipped
//! plus the configured default, so an app with no `lang/` files still
//! resolves to its default and every key degrades to itself.

use kelson_foundation::MessageCatalog;

/// Session bag key carrying an explicit locale preference — the session
/// `data` doc names "locale override" as its intended content.
pub const SESSION_LOCALE_KEY: &str = "locale";

/// Parse an `Accept-Language` header into candidate tags, best quality
/// first (input order breaks ties). Per RFC 9110: `q=0` means "not
/// acceptable" and is dropped, a malformed quality falls back to 1.0, and
/// the `*` wildcard matches nothing specific here — the configured
/// default answers it at the end of resolution.
pub fn parse_accept_language(header: &str) -> Vec<String> {
    let mut scored: Vec<(String, f32, usize)> = Vec::new();
    for (index, part) in header.split(',').enumerate() {
        let mut pieces = part.split(';');
        let Some(first) = pieces.next() else {
            continue;
        };
        let tag = first.trim().to_ascii_lowercase();
        if tag.is_empty() || tag == "*" {
            continue;
        }
        let mut quality = 1.0f32;
        for param in pieces {
            if let Some((name, value)) = param.split_once('=') {
                if name.trim().eq_ignore_ascii_case("q") {
                    quality = value.trim().parse::<f32>().unwrap_or(1.0);
                }
            }
        }
        if quality > 0.0 {
            scored.push((tag, quality, index));
        }
    }
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.2.cmp(&b.2))
    });
    scored.into_iter().map(|(tag, _, _)| tag).collect()
}

/// First supported candidate in the quality-ordered header: an exact
/// (case-insensitive) match wins, then the language part matches a
/// shipped tag in either direction (`fr-CA` accepts a shipped `fr`, and
/// a shipped `fr-FR` accepts `fr`).
pub fn negotiate(accept: &str, supported: &[&str]) -> Option<String> {
    for tag in parse_accept_language(accept) {
        let hit = supported.iter().find(|candidate| {
            candidate.eq_ignore_ascii_case(&tag)
                || candidate
                    .split('-')
                    .next()
                    .is_some_and(|part| part.eq_ignore_ascii_case(&tag))
                || tag.split('-').next().is_some_and(|tag_lang| {
                    candidate
                        .split('-')
                        .next()
                        .is_some_and(|part| part.eq_ignore_ascii_case(tag_lang))
                })
        });
        if let Some(hit) = hit {
            return Some((*hit).to_string());
        }
    }
    None
}

/// Resolve the request locale (spec order): a non-empty session
/// preference wins outright; then the first supported `Accept-Language`
/// candidate; then the catalog default. An unknown session tag is not
/// rejected here — the catalog fallback chain degrades it to the
/// default's messages while `_locale` keeps reporting what was chosen.
pub fn resolve(catalog: &MessageCatalog, session: Option<&str>, accept: Option<&str>) -> String {
    if let Some(preference) = session.map(str::trim).filter(|value| !value.is_empty()) {
        return preference.to_string();
    }
    let mut supported: Vec<&str> = catalog.locales();
    let default = catalog.default_locale();
    if !supported.contains(&default) {
        supported.push(default);
    }
    if let Some(accept) = accept {
        if let Some(hit) = negotiate(accept, &supported) {
            return hit;
        }
    }
    default.to_string()
}

/// The session preference for this request (see [`SESSION_LOCALE_KEY`]).
pub fn session_locale(session: Option<&kelson_http::ResolvedSession>) -> Option<&str> {
    session?
        .session
        .data
        .get(SESSION_LOCALE_KEY)
        .and_then(|value| value.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> MessageCatalog {
        let mut catalog = MessageCatalog::new("en");
        catalog.define("en", "k", "en").unwrap();
        catalog.define("fr", "k", "fr").unwrap();
        catalog
    }

    /// Wave 6 P3: quality ordering with q=0 refusal, malformed-q fallback
    /// and wildcard skipping.
    #[test]
    fn accept_language_parses_quality_order() {
        assert_eq!(
            parse_accept_language("fr-CA, fr;q=0.9, en;q=0.8, de;q=0"),
            vec!["fr-ca", "fr", "en"]
        );
        assert_eq!(parse_accept_language(" * "), Vec::<String>::new());
        assert_eq!(parse_accept_language("en;q=banana"), vec!["en"]);
        // Equal qualities keep header order (stable sort).
        assert_eq!(
            parse_accept_language("en;q=0.5, fr;q=0.5"),
            vec!["en", "fr"]
        );
        assert_eq!(parse_accept_language(""), Vec::<String>::new());
        assert_eq!(parse_accept_language(",,"), Vec::<String>::new());
    }

    /// Wave 6 P3: negotiation matches exact tags first, then language
    /// parts in either direction.
    #[test]
    fn negotiate_matches_supported_languages() {
        let supported = ["en", "fr", "ar"];
        assert_eq!(
            negotiate("fr-CA, en;q=0.5", &supported).as_deref(),
            Some("fr")
        );
        assert_eq!(negotiate("de-DE", &supported), None);
        assert_eq!(
            negotiate("en-us, ar", &supported).as_deref(),
            Some("en")
        );
        // Regional shipped tag accepts its plain language and vice versa.
        assert_eq!(negotiate("fr", &["fr-FR"]).as_deref(), Some("fr-FR"));
        assert_eq!(negotiate("fr-FR", &["fr"]).as_deref(), Some("fr"));
    }

    /// Wave 6 P3: the full resolution order — session > Accept-Language
    /// (quality-ordered, supported only) > configured default.
    #[test]
    fn resolve_follows_session_accept_default_order() {
        let catalog = catalog();
        // Session preference wins even over a matching header.
        assert_eq!(resolve(&catalog, Some("fr"), Some("en")), "fr");
        // Empty/whitespace preference falls through.
        assert_eq!(resolve(&catalog, Some("  "), Some("en")), "en");
        // First supported candidate, quality-ordered; `de` is skipped.
        assert_eq!(
            resolve(&catalog, None, Some("de, fr;q=0.9, en;q=0.8")),
            "fr"
        );
        // Nothing supported → configured default.
        assert_eq!(resolve(&catalog, None, Some("de")), "en");
        assert_eq!(resolve(&catalog, None, None), "en");
        // The default is always supported, even with no shipped files.
        let empty = MessageCatalog::new("en");
        assert_eq!(resolve(&empty, None, Some("fr")), "en");
    }

    /// Wave 6 P3: the session preference reads the documented `locale`
    /// bag entry; absent keys and non-strings degrade to `None`.
    #[test]
    fn session_locale_reads_the_bag() {
        use time::OffsetDateTime;

        let principal = kelson_foundation::PrincipalId::from_uuid(uuid::Uuid::now_v7());
        let mut session = kelson_session::Session::new(principal, None, OffsetDateTime::now_utc());
        assert_eq!(session_locale(None), None);
        assert_eq!(
            session_locale(Some(&kelson_http::ResolvedSession {
                session: session.clone(),
            })),
            None
        );

        session
            .data
            .insert("locale".into(), serde_json::json!("fr"));
        let resolved = kelson_http::ResolvedSession { session };
        assert_eq!(session_locale(Some(&resolved)), Some("fr"));

        // Non-string values are not a locale.
        let mut session = resolved.session.clone();
        session.data.insert("locale".into(), serde_json::json!(7));
        assert_eq!(
            session_locale(Some(&kelson_http::ResolvedSession { session })),
            None
        );
    }

    /// Wave 6 P3: the shipped workspace catalogs (`platform/lang`) load
    /// through the same boot loader production uses — en/fr/ar with the
    /// validation keys and the CLDR choice vocabulary present.
    #[test]
    fn shipped_workspace_lang_files_load() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let catalog = kelson_config::load_messages(&root, &kelson_config::I18nSection::default())
            .expect("shipped lang files load");
        assert_eq!(catalog.default_locale(), "en");
        assert_eq!(catalog.locales(), vec!["ar", "en", "fr"]);
        assert_eq!(
            catalog.get("en", "validation.required"),
            Some("{attribute} is required")
        );
        assert!(catalog.get("fr", "validation.required").is_some());
        assert!(catalog.get("ar", "validation.required").is_some());

        // The choice vocabulary resolves per locale through `trans_choice`
        // (fr: explicit zero form; ar: the full six-bucket matrix).
        assert_eq!(
            kelson_foundation::trans_choice(&catalog, "en", "appointments.count", 1).text,
            "One appointment"
        );
        assert_eq!(
            kelson_foundation::trans_choice(&catalog, "fr", "appointments.count", 0).text,
            "Aucun rendez-vous"
        );
        assert_eq!(
            kelson_foundation::trans_choice(&catalog, "ar", "appointments.count", 5).text,
            "5 مواعيد"
        );
    }
}
