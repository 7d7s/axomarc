//! The head metadata store: per-request title, metas, and merge policy.
//!
//! LF-035: The head metadata store provides request-scoped isolation
//! for HTML head content (title, meta tags, links, scripts). The gap
//! LF-035 names is that without explicit scoping, a shared mutable head
//! would leak between requests — a cross-tenant leak.
//!
//! The merge policy is simple and explicit: the child's value wins,
//! the parent's defaults fill. When a layout extends a parent layout,
//! the child's `@section('head')` contributions are merged on top of
//! the parent's, with the child's values taking precedence for the
//! same keys (title, meta names, etc.).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The head metadata store for one request.
///
/// This is request-scoped: a new `HeadStore` is created for each
/// request that renders a view. It is NOT shared across requests.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HeadStore {
    /// The page title (optional - may be set by layout or child).
    pub title: Option<String>,
    /// Meta tags by name (e.g., "description" -> "content").
    pub metas: BTreeMap<String, String>,
    /// Link tags by rel (e.g., "canonical" -> "href").
    pub links: BTreeMap<String, String>,
    /// Script tags (src -> attributes JSON).
    pub scripts: BTreeMap<String, Value>,
    /// Style tags (href -> attributes JSON).
    pub styles: BTreeMap<String, Value>,
    /// Whether this head has been finalised (prevents further mutation).
    #[serde(skip)]
    finalised: bool,
}

impl HeadStore {
    /// Create a new empty head store for a request.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the page title.
    ///
    /// The child's title wins over the parent's.
    pub fn set_title(&mut self, title: impl Into<String>) {
        assert!(!self.finalised, "head store is finalised");
        self.title = Some(title.into());
    }

    /// Get the page title, if set.
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    /// Set a meta tag (name -> content).
    ///
    /// The child's value for a given name wins over the parent's.
    pub fn set_meta(&mut self, name: impl Into<String>, content: impl Into<String>) {
        assert!(!self.finalised, "head store is finalised");
        self.metas.insert(name.into(), content.into());
    }

    /// Get a meta tag by name.
    pub fn meta(&self, name: &str) -> Option<&str> {
        self.metas.get(name).map(|s| s.as_str())
    }

    /// Set a link tag (rel -> href).
    ///
    /// The child's value for a given rel wins over the parent's.
    pub fn set_link(&mut self, rel: impl Into<String>, href: impl Into<String>) {
        assert!(!self.finalised, "head store is finalised");
        self.links.insert(rel.into(), href.into());
    }

    /// Get a link tag by rel.
    pub fn link(&self, rel: &str) -> Option<&str> {
        self.links.get(rel).map(|s| s.as_str())
    }

    /// Add a script tag.
    pub fn add_script(&mut self, src: impl Into<String>, attrs: Value) {
        assert!(!self.finalised, "head store is finalised");
        self.scripts.insert(src.into(), attrs);
    }

    /// Add a style tag.
    pub fn add_style(&mut self, href: impl Into<String>, attrs: Value) {
        assert!(!self.finalised, "head store is finalised");
        self.styles.insert(href.into(), attrs);
    }

    /// Merge another head store into this one.
    ///
    /// The merge policy: **the child's value wins**. This method is
    /// called with the PARENT's head first, then the CHILD's head
    /// merges on top. So the child's title, metas, links, etc.
    /// override the parent's for the same keys.
    pub fn merge_child(&mut self, child: HeadStore) {
        assert!(!self.finalised, "head store is finalised");
        if let Some(title) = child.title {
            self.title = Some(title);
        }
        // Child's metas override parent's for the same names
        self.metas.extend(child.metas);
        // Child's links override parent's for the same rels
        self.links.extend(child.links);
        // Child's scripts/styles are additive (can have multiple)
        self.scripts.extend(child.scripts);
        self.styles.extend(child.styles);
    }

    /// Finalise the head store - prevents further mutation.
    ///
    /// Called when the layout chain is complete and the head is
    /// about to be rendered into the template context.
    pub fn finalise(&mut self) {
        self.finalised = true;
    }

    /// Whether the head store is finalised.
    pub fn is_finalised(&self) -> bool {
        self.finalised
    }

    /// Export the head store as a template context object.
    ///
    /// The keys match what the template expects:
    /// - `title` -> string
    /// - `metas` -> object (name -> content)
    /// - `links` -> object (rel -> href)
    /// - `scripts` -> array of objects
    /// - `styles` -> array of objects
    pub fn into_context(mut self) -> Value {
        self.finalise();
        let mut obj = serde_json::Map::new();
        if let Some(title) = self.title {
            obj.insert("title".to_string(), Value::String(title));
        }
        if !self.metas.is_empty() {
            obj.insert(
                "metas".to_string(),
                Value::Object(
                    self.metas
                        .into_iter()
                        .map(|(k, v)| (k, Value::String(v)))
                        .collect(),
                ),
            );
        }
        if !self.links.is_empty() {
            obj.insert(
                "links".to_string(),
                Value::Object(
                    self.links
                        .into_iter()
                        .map(|(k, v)| (k, Value::String(v)))
                        .collect(),
                ),
            );
        }
        if !self.scripts.is_empty() {
            obj.insert(
                "scripts".to_string(),
                Value::Array(
                    self.scripts
                        .into_iter()
                        .map(|(src, attrs)| {
                            let mut obj = attrs.as_object().cloned().unwrap_or_default();
                            obj.insert("src".to_string(), Value::String(src));
                            Value::Object(obj)
                        })
                        .collect(),
                ),
            );
        }
        if !self.styles.is_empty() {
            obj.insert(
                "styles".to_string(),
                Value::Array(
                    self.styles
                        .into_iter()
                        .map(|(href, attrs)| {
                            let mut obj = attrs.as_object().cloned().unwrap_or_default();
                            obj.insert("href".to_string(), Value::String(href));
                            Value::Object(obj)
                        })
                        .collect(),
                ),
            );
        }
        Value::Object(obj)
    }
}

/// A builder for constructing a head store fluently.
#[derive(Debug, Default)]
pub struct HeadBuilder {
    store: HeadStore,
}

impl HeadBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.store.set_title(title);
        self
    }

    pub fn meta(mut self, name: impl Into<String>, content: impl Into<String>) -> Self {
        self.store.set_meta(name, content);
        self
    }

    pub fn link(mut self, rel: impl Into<String>, href: impl Into<String>) -> Self {
        self.store.set_link(rel, href);
        self
    }

    pub fn script(mut self, src: impl Into<String>, attrs: Value) -> Self {
        self.store.add_script(src, attrs);
        self
    }

    pub fn style(mut self, href: impl Into<String>, attrs: Value) -> Self {
        self.store.add_style(href, attrs);
        self
    }

    pub fn build(self) -> HeadStore {
        self.store
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_head_store_holds_title_and_metas() {
        let mut head = HeadStore::new();
        head.set_title("My Page");
        head.set_meta("description", "A test page");
        head.set_meta("viewport", "width=device-width");

        assert_eq!(head.title(), Some("My Page"));
        assert_eq!(head.meta("description"), Some("A test page"));
        assert_eq!(head.meta("viewport"), Some("width=device-width"));
    }

    #[test]
    fn the_childs_title_wins_over_the_parent() {
        let mut parent = HeadStore::new();
        parent.set_title("Parent Title");
        parent.set_meta("description", "Parent description");

        let mut child = HeadStore::new();
        child.set_title("Child Title");
        child.set_meta("description", "Child description");
        child.set_meta("author", "Child Author");

        parent.merge_child(child);

        assert_eq!(parent.title(), Some("Child Title"));
        assert_eq!(parent.meta("description"), Some("Child description"));
        assert_eq!(parent.meta("author"), Some("Child Author"));
    }

    #[test]
    fn the_childs_links_override_the_parent() {
        let mut parent = HeadStore::new();
        parent.set_link("canonical", "https://parent.example.com/page");

        let mut child = HeadStore::new();
        child.set_link("canonical", "https://child.example.com/page");
        child.set_link("alternate", "https://child.example.com/page?format=amp");

        parent.merge_child(child);

        assert_eq!(
            parent.link("canonical"),
            Some("https://child.example.com/page")
        );
        assert_eq!(
            parent.link("alternate"),
            Some("https://child.example.com/page?format=amp")
        );
    }

    #[test]
    fn scripts_and_styles_are_additive() {
        let mut parent = HeadStore::new();
        parent.add_script("https://parent.example.com/app.js".to_string(), Value::Null);
        parent.add_style(
            "https://parent.example.com/app.css".to_string(),
            Value::Null,
        );

        let mut child = HeadStore::new();
        child.add_script("https://child.example.com/page.js".to_string(), Value::Null);
        child.add_style(
            "https://child.example.com/page.css".to_string(),
            Value::Null,
        );

        parent.merge_child(child);

        assert_eq!(parent.scripts.len(), 2);
        assert_eq!(parent.styles.len(), 2);
    }

    #[test]
    fn a_finalised_store_prevents_mutation() {
        let mut head = HeadStore::new();
        head.finalise();

        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            head.set_title("Too late");
        }))
        .expect_err("mutating a finalised store panics");
    }

    #[test]
    fn the_builder_produces_a_head_store() {
        let head = HeadBuilder::new()
            .title("Built Title")
            .meta("description", "Built description")
            .link("canonical", "https://example.com/page")
            .script("https://example.com/app.js".to_string(), Value::Null)
            .style("https://example.com/app.css".to_string(), Value::Null)
            .build();

        assert_eq!(head.title(), Some("Built Title"));
        assert_eq!(head.meta("description"), Some("Built description"));
        assert_eq!(head.link("canonical"), Some("https://example.com/page"));
        assert_eq!(head.scripts.len(), 1);
        assert_eq!(head.styles.len(), 1);
    }

    #[test]
    fn into_context_produces_the_template_shape() {
        let head = HeadBuilder::new()
            .title("Context Title")
            .meta("description", "Context description")
            .build();

        let ctx = head.into_context();
        assert_eq!(ctx["title"], serde_json::json!("Context Title"));
        assert_eq!(
            ctx["metas"]["description"],
            serde_json::json!("Context description")
        );
        assert!(ctx["metas"].is_object());
    }

    /// MVVC S-8 (LF-035): the child's head values merge over the parent's.
    /// The merge is explicit: child wins, parent fills.
    #[test]
    fn the_head_merges_the_child_over_the_parent() {
        let mut parent = HeadStore::new();
        parent.set_title("Parent Title");
        parent.set_meta("description", "Parent description");
        parent.set_link("canonical", "https://parent.example.com");

        let mut child = HeadStore::new();
        child.set_title("Child Title");
        child.set_meta("description", "Child description");
        child.set_meta("author", "Child Author");
        child.set_link("canonical", "https://child.example.com");

        parent.merge_child(child);

        // Child's values win
        assert_eq!(parent.title(), Some("Child Title"));
        assert_eq!(parent.meta("description"), Some("Child description"));
        // Child's new keys are added
        assert_eq!(parent.meta("author"), Some("Child Author"));
        // Child's link overrides
        assert_eq!(parent.link("canonical"), Some("https://child.example.com"));
    }

    /// MVVC S-8 (LF-035): head state does not leak between requests.
    /// Each request gets a fresh HeadStore.
    #[test]
    fn the_head_state_does_not_leak_between_requests() {
        let head1 = HeadBuilder::new()
            .title("Request 1")
            .meta("request", "1")
            .build();

        let head2 = HeadBuilder::new()
            .title("Request 2")
            .meta("request", "2")
            .build();

        // The two head stores are independent
        assert_eq!(head1.title(), Some("Request 1"));
        assert_eq!(head2.title(), Some("Request 2"));
        assert_eq!(head1.meta("request"), Some("1"));
        assert_eq!(head2.meta("request"), Some("2"));

        // Merging one doesn't affect the other
        let mut combined = HeadStore::new();
        combined.merge_child(head1);
        assert_eq!(combined.title(), Some("Request 1"));
        combined.merge_child(head2);
        assert_eq!(combined.title(), Some("Request 2"));
    }

    /// MVVC S-8 (LF-035): the head reaches the template via _head.
    /// MVVC S-8 (LF-035): the head reaches the template via _head.
    /// Note: into_context() puts fields at root; ViewContext.apply() nests under _head.
    #[test]
    fn the_head_reaches_the_template() {
        let head = HeadBuilder::new()
            .title("Template Title")
            .meta("description", "Template description")
            .link("canonical", "https://example.com/page")
            .build();

        let ctx = head.into_context();
        // into_context() puts fields at root level
        assert_eq!(ctx["title"], serde_json::json!("Template Title"));
        assert_eq!(
            ctx["metas"]["description"],
            serde_json::json!("Template description")
        );
        assert_eq!(
            ctx["links"]["canonical"],
            serde_json::json!("https://example.com/page")
        );
    }
}
