//! The reserved view context: what the framework fills for every view.
//!
//! Every path that renders a view - the request edge, the error edge, the
//! notification edge, the operations dashboard - fills the **same**
//! reserved keys, with the **same** shapes. Before this module each of
//! them did it by hand, and the dashboard did it not at all: a template
//! that read `@lang` worked on a request and silently degraded on the
//! dashboard.
//!
//! One assembler, one set of names, one set of shapes. The names are the
//! framework's documented edge conventions, and they are the keys the
//! views engine reads (`@lang`, `@error`, `@csrf`, `@auth`).
//!
//! This lives in the foundation, not in the web layer, because the
//! notification edge renders a view too - and the notification service
//! must not depend on the web layer to do it.

use serde_json::Value;

use crate::translate::MessageCatalog;
use crate::head::HeadStore;

/// The reserved key the resolved locale is under.
pub const LOCALE_KEY: &str = "_locale";
/// The reserved key the flat message map is under.
pub const LANG_KEY: &str = "_lang";
/// The reserved key the authenticated principal's id is under.
pub const AUTH_KEY: &str = "_auth";
/// The reserved key the validation errors are under.
pub const ERRORS_KEY: &str = "_errors";
/// The reserved key the CSRF token is under.
pub const CSRF_KEY: &str = "_csrf";
/// The reserved key the head metadata is under.
pub const HEAD_KEY: &str = "_head";

/// The reserved context the framework fills for every view.
///
/// A field that is absent stays absent: a path that renders a view
/// outside a request (a notification, say) has no principal and no
/// session, and inventing empty values for them would be a lie the
/// template could not tell from a real one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ViewContext {
    /// The resolved locale ("en", "en-GB", ...).
    pub locale: Option<String>,
    /// The flat message map `@lang` and the `trans` filter read.
    pub lang: Option<Value>,
    /// The authenticated principal's id, when the request carried one.
    pub auth: Option<String>,
    /// The validation errors (field -> message) `@error` reads.
    pub errors: Option<Value>,
    /// The CSRF token `@csrf` renders into a form.
    pub csrf: Option<String>,
    /// The head metadata (title, metas, links, scripts, styles).
    pub head: Option<HeadStore>,
}

impl ViewContext {
    /// The reserved context with nothing in it.
    ///
    /// A render that has no locale, no principal and no token fills
    /// nothing: the template sees exactly what the caller gave it.
    pub fn empty() -> Self {
        Self::default()
    }

    /// The reserved context for a render that resolved a locale against a
    /// message catalog.
    ///
    /// The flat map's shape is decided here, once: the message catalog's
    /// locale overlay over the language part over the configured default.
    pub fn localized(catalog: &MessageCatalog, locale: &str) -> Self {
        Self {
            locale: Some(locale.to_string()),
            lang: Some(serde_json::to_value(catalog.flat(locale)).unwrap_or_default()),
            ..Self::default()
        }
    }

    /// The authenticated principal's id (absent for a guest).
    pub fn with_auth(mut self, principal: Option<String>) -> Self {
        self.auth = principal;
        self
    }

    /// The CSRF token (absent when the request carried no session).
    pub fn with_csrf(mut self, token: Option<String>) -> Self {
        self.csrf = token;
        self
    }

    /// The validation errors (field -> message).
    pub fn with_errors(mut self, errors: Value) -> Self {
        self.errors = Some(errors);
        self
    }

    /// The head metadata (title, metas, links, scripts, styles).
    pub fn with_head(mut self, head: HeadStore) -> Self {
        self.head = Some(head);
        self
    }

    /// Merge the reserved keys into a template context.
    ///
    /// The merge policy, in one place:
    ///
    /// * the framework's keys **win** - `_locale`, `_lang`, `_auth`,
    ///   `_csrf`, and `_head` are the request's truth, not the result's;
    /// * `_errors` does **not** overwrite what the context already
    ///   carries: the operation result's own errors are the
    ///   application's data, and they are what `@error` must render;
    /// * a context that is not an object is left alone - the caller
    ///   passed something the framework does not own.
    pub fn apply(&self, context: &mut Value) {
        let Some(object) = context.as_object_mut() else {
            return;
        };
        if let Some(locale) = &self.locale {
            object.insert(LOCALE_KEY.to_string(), Value::String(locale.clone()));
        }
        if let Some(lang) = &self.lang {
            object.insert(LANG_KEY.to_string(), lang.clone());
        }
        if let Some(auth) = &self.auth {
            object.insert(AUTH_KEY.to_string(), Value::String(auth.clone()));
        }
        if let Some(errors) = &self.errors {
            object
                .entry(ERRORS_KEY.to_string())
                .or_insert_with(|| errors.clone());
        }
        if let Some(csrf) = &self.csrf {
            object.insert(CSRF_KEY.to_string(), Value::String(csrf.clone()));
        }
        if let Some(head) = &self.head {
            object.insert(HEAD_KEY.to_string(), head.clone().into_context());
        }
    }

    /// Merge the reserved keys into a template context and hand it back.
    pub fn into_context(self, mut context: Value) -> Value {
        self.apply(&mut context);
        context
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A catalog with one locale and one message.
    fn catalog() -> MessageCatalog {
        let mut catalog = MessageCatalog::new("en");
        catalog
            .define("en", "greeting", "Hello")
            .expect("the message defines");
        catalog
    }

    #[test]
    fn the_reserved_context_fills_the_documented_keys() {
        let mut context = json!({ "patient": "Ada" });
        ViewContext::localized(&catalog(), "en")
            .with_auth(Some("principal-1".to_string()))
            .with_errors(json!({}))
            .with_csrf(Some("token-1".to_string()))
            .apply(&mut context);
        assert_eq!(context["patient"], json!("Ada"), "the app's own data survives");
        assert_eq!(context[LOCALE_KEY], json!("en"));
        assert_eq!(context[LANG_KEY]["greeting"], json!("Hello"));
        assert_eq!(context[AUTH_KEY], json!("principal-1"));
        assert_eq!(context[ERRORS_KEY], json!({}));
        assert_eq!(context[CSRF_KEY], json!("token-1"));
    }

    #[test]
    fn an_absent_field_stays_absent() {
        let mut context = json!({});
        ViewContext::empty().apply(&mut context);
        assert_eq!(context, json!({}), "nothing is invented");
    }

    #[test]
    fn the_frameworks_keys_win_and_the_results_errors_survive() {
        let mut context = json!({ LOCALE_KEY: "wrong", ERRORS_KEY: { "email": "required" } });
        ViewContext::localized(&catalog(), "en")
            .with_errors(json!({}))
            .apply(&mut context);
        assert_eq!(context[LOCALE_KEY], json!("en"), "the request's locale wins");
        assert_eq!(
            context[ERRORS_KEY]["email"],
            json!("required"),
            "the result's own errors are the application's data"
        );
    }

    #[test]
    fn a_context_that_is_not_an_object_is_left_alone() {
        let mut context = json!("not an object");
        ViewContext::localized(&catalog(), "en").apply(&mut context);
        assert_eq!(context, json!("not an object"));
    }
}
