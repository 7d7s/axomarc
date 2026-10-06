//! Resources (S2 item 26, L10): ResponseViewModel -> wire rendering helpers.
//! The domain returns business results; a Resource (ResponseViewModel impl)
//! decides the exact wire representation. These helpers keep that path uniform.
//!
//! Wave 3 Phase 7 adds the typed [`Resource`] transformer trait (Laravel
//! `JsonResource` parity): `Value` bodies with conditional fields
//! ([`Fields::when`] — a failed condition omits the key, never `null`), a
//! wrapping key ([`Resource::wrap`], default `"data"`), and list rendering
//! through [`ResourceCollection`] with pagination-meta passthrough. The typed
//! `ResponseViewModel` helpers below stay unchanged — the two paths serve the
//! struct-shaped and the `Value`-shaped presentations respectively.

use serde_json::Value;

use crate::{Envelope, Page, ResponseViewModel};

/// Render one source through its resource representation.
pub fn resource<R, S>(source: &S) -> R
where
    R: ResponseViewModel<Source = S>,
{
    R::from_source(source)
}

/// Render a collection through its resource representation.
pub fn resource_collection<R, S>(sources: &[S]) -> Vec<R>
where
    R: ResponseViewModel<Source = S>,
{
    sources.iter().map(R::from_source).collect()
}

/// One source -> success envelope carrying the resource.
pub fn envelope_resource<R, S>(source: &S) -> Envelope<R>
where
    R: ResponseViewModel<Source = S>,
{
    Envelope::ok(resource(source))
}

/// Page of sources -> success envelope with pagination meta (item 26 shape).
pub fn envelope_page<R, S>(page: Page<S>) -> Envelope<Vec<R>>
where
    R: ResponseViewModel<Source = S>,
{
    let meta = page.meta();
    let items = resource_collection(&page.items);
    Envelope::ok(items).with_meta(meta)
}

// ---------------------------------------- Wave 3 Phase 7: typed transformer

/// Default wrapping key (Laravel `JsonResource::$wrap` = `"data"`).
pub const DEFAULT_WRAP: &str = "data";

/// Typed transformer: business source -> wire JSON (the Laravel
/// `JsonResource` equivalent). Conditional fields go through
/// [`Fields::when`]; lists render through [`ResourceCollection`] with
/// pagination-meta passthrough.
///
/// The `keel make resource` scaffold implements exactly this shape — the
/// doctest below compiles verbatim as the build proof (codegen templates
/// are string-checked only, so this is where the shape is proven real):
///
/// ```rust
/// pub struct Widget;
///
/// pub struct WidgetResource;
///
/// impl keel_http::Resource for WidgetResource {
///     type Source = Widget;
///
///     fn from_source(_source: &Self::Source) -> serde_json::Value {
///         keel_http::Fields::new()
///             .field("type", "widget")
///             .into_value()
///     }
/// }
/// ```
pub trait Resource {
    /// The source type this resource renders.
    type Source;

    /// The wire body for one source (conditional fields included).
    fn from_source(source: &Self::Source) -> Value;

    /// Wrapping key applied by [`Resource::wrapped`] — Laravel's
    /// `JsonResource::$wrap`. `None` renders the body at the top level.
    fn wrap() -> Option<&'static str> {
        Some(DEFAULT_WRAP)
    }

    /// Body under [`Resource::wrap`].
    fn wrapped(source: &Self::Source) -> Value {
        match Self::wrap() {
            Some(key) => {
                let mut object = serde_json::Map::new();
                object.insert(key.to_string(), Self::from_source(source));
                Value::Object(object)
            }
            None => Self::from_source(source),
        }
    }
}

/// Field bag for a [`Resource`] body: unconditional `.field` plus conditional
/// `.when` / `.when_some` (Laravel `$resource->when(...)` / `MissingValue`) —
/// a failed condition omits the key entirely instead of serializing `null`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Fields(serde_json::Map<String, Value>);

impl Fields {
    pub fn new() -> Self {
        Self::default()
    }

    /// Unconditional field.
    pub fn field(mut self, key: impl Into<String>, value: impl Into<Value>) -> Self {
        self.0.insert(key.into(), value.into());
        self
    }

    /// Conditional field: inserted only when `condition` holds — a false
    /// condition omits the key (Laravel `when` semantics).
    pub fn when(
        mut self,
        condition: bool,
        key: impl Into<String>,
        value: impl Into<Value>,
    ) -> Self {
        if condition {
            self.0.insert(key.into(), value.into());
        }
        self
    }

    /// Conditional field from an `Option`: omitted when `None` (Laravel
    /// `whenHas` — never a null placeholder).
    pub fn when_some(mut self, key: impl Into<String>, value: Option<impl Into<Value>>) -> Self {
        if let Some(value) = value {
            self.0.insert(key.into(), value.into());
        }
        self
    }

    /// The rendered body.
    pub fn into_value(self) -> Value {
        Value::Object(self.0)
    }
}

/// List rendering through a [`Resource`] (Laravel `ResourceCollection`):
/// every source goes through `R::from_source`, and a [`Page`]'s meta block
/// passes through verbatim.
pub struct ResourceCollection;

impl ResourceCollection {
    /// Render a slice: items under `R::wrap()` (a top-level array when the
    /// wrap key is `None`).
    pub fn from_sources<R: Resource>(sources: &[R::Source]) -> Value {
        let items: Vec<Value> = sources.iter().map(R::from_source).collect();
        match R::wrap() {
            Some(key) => {
                let mut object = serde_json::Map::new();
                object.insert(key.to_string(), Value::Array(items));
                Value::Object(object)
            }
            None => Value::Array(items),
        }
    }

    /// Render a page: items under the wrap key and `page.meta()` verbatim
    /// under `meta` (page/per_page/total/has_more passthrough — Laravel's
    /// paginated `ResourceCollection`). A rendered page is always an object,
    /// so a resource that disables the wrap key falls back to
    /// [`DEFAULT_WRAP`] for the item slot — `meta` stays addressable.
    pub fn from_page<R: Resource>(page: &Page<R::Source>) -> Value {
        let items: Vec<Value> = page.items.iter().map(R::from_source).collect();
        let mut object = serde_json::Map::new();
        object.insert(
            R::wrap().unwrap_or(DEFAULT_WRAP).to_string(),
            Value::Array(items),
        );
        object.insert("meta".to_string(), page.meta());
        Value::Object(object)
    }
}

/// Page of sources -> success envelope carrying the rendered resources plus
/// the page's pagination meta (the [`Resource`] trait's twin of
/// [`envelope_page`]).
pub fn envelope_resource_page<R, S>(page: Page<S>) -> Envelope<Vec<Value>>
where
    R: Resource<Source = S>,
{
    let meta = page.meta();
    let items: Vec<Value> = page.items.iter().map(R::from_source).collect();
    Envelope::ok(items).with_meta(meta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    struct User {
        name: String,
        #[allow(dead_code)]
        internal_ref: String,
    }

    #[derive(Serialize)]
    struct UserResource {
        name: String,
    }

    impl ResponseViewModel for UserResource {
        type Source = User;
        fn from_source(source: &User) -> Self {
            Self {
                name: source.name.clone(),
            }
        }
    }

    fn users() -> Vec<User> {
        vec![
            User {
                name: "alice".into(),
                internal_ref: "x1".into(),
            },
            User {
                name: "bob".into(),
                internal_ref: "x2".into(),
            },
        ]
    }

    #[test]
    fn resource_hides_non_wire_fields() {
        let env = envelope_resource::<UserResource, _>(&users()[0]);
        let wire = serde_json::to_value(&env).unwrap();
        assert_eq!(wire["data"]["name"], "alice");
        assert!(!wire.to_string().contains("internal_ref"));
    }

    #[test]
    fn page_envelope_carries_pagination_meta() {
        let page = Page::new(users(), 1, 2, Some(5));
        let env = envelope_page::<UserResource, _>(page);
        let wire = serde_json::to_value(&env).unwrap();
        assert_eq!(wire["data"].as_array().unwrap().len(), 2);
        assert_eq!(wire["meta"]["page"], 1);
        assert_eq!(wire["meta"]["total"], 5);
        assert_eq!(wire["meta"]["has_more"], true);
    }

    // ---------------------------- Wave 3 Phase 7: typed Resource transformer

    struct Account {
        name: String,
        admin: bool,
        nickname: Option<String>,
    }

    struct AccountResource;

    impl Resource for AccountResource {
        type Source = Account;

        fn from_source(source: &Self::Source) -> Value {
            Fields::new()
                .field("name", source.name.as_str())
                .when(source.admin, "admin", true)
                .when_some("nickname", source.nickname.as_deref())
                .into_value()
        }
    }

    /// A resource that renders its body at the top level (no wrap key).
    struct BareAccountResource;

    impl Resource for BareAccountResource {
        type Source = Account;

        fn from_source(source: &Self::Source) -> Value {
            AccountResource::from_source(source)
        }

        fn wrap() -> Option<&'static str> {
            None
        }
    }

    fn account(name: &str, admin: bool, nickname: Option<&str>) -> Account {
        Account {
            name: name.into(),
            admin,
            nickname: nickname.map(str::to_string),
        }
    }

    #[test]
    fn when_omits_false_conditions_and_never_emits_null() {
        let body = AccountResource::from_source(&account("alice", true, None));
        assert_eq!(body["admin"], serde_json::json!(true));
        assert!(body.get("nickname").is_none(), "None must omit, not null");
        assert!(!body.to_string().contains("null"));

        let body = AccountResource::from_source(&account("bob", false, Some("bobby")));
        assert!(body.get("admin").is_none(), "false must omit the key");
        assert_eq!(body["nickname"], serde_json::json!("bobby"));
    }

    #[test]
    fn wrap_key_defaults_to_data_and_can_be_disabled() {
        let alice = account("alice", true, None);
        let wrapped = AccountResource::wrapped(&alice);
        assert_eq!(wrapped["data"]["name"], serde_json::json!("alice"));

        let bare = BareAccountResource::wrapped(&alice);
        assert_eq!(bare["name"], serde_json::json!("alice"));
        assert!(bare.get("data").is_none());
    }

    #[test]
    fn collection_of_sources_honors_the_wrap_key() {
        let accounts = vec![
            account("alice", false, None),
            account("bob", true, Some("b")),
        ];
        let rendered = ResourceCollection::from_sources::<AccountResource>(&accounts);
        assert_eq!(rendered["data"].as_array().unwrap().len(), 2);
        assert_eq!(rendered["data"][0]["name"], serde_json::json!("alice"));

        let bare = ResourceCollection::from_sources::<BareAccountResource>(&accounts);
        assert!(bare.is_array());
        assert_eq!(bare.as_array().unwrap().len(), 2);
    }

    #[test]
    fn collection_page_passes_pagination_meta_through() {
        let page = Page::new(
            vec![
                account("alice", false, None),
                account("bob", true, None),
            ],
            2,
            10,
            Some(42),
        );
        let rendered = ResourceCollection::from_page::<AccountResource>(&page);
        assert_eq!(rendered["data"].as_array().unwrap().len(), 2);
        assert_eq!(rendered["data"][1]["name"], serde_json::json!("bob"));
        // The page's own meta block passes through verbatim.
        assert_eq!(rendered["meta"], page.meta());
        assert_eq!(rendered["meta"]["page"], 2);
        assert_eq!(rendered["meta"]["per_page"], 10);
        assert_eq!(rendered["meta"]["total"], 42);
        assert_eq!(rendered["meta"]["has_more"], true);
    }

    #[test]
    fn envelope_resource_page_matches_the_envelope_wire_shape() {
        let page = Page::new(vec![account("alice", false, None)], 1, 10, Some(1));
        let env = envelope_resource_page::<AccountResource, _>(page);
        let wire = serde_json::to_value(&env).unwrap();
        assert_eq!(wire["ok"], true);
        assert_eq!(wire["data"][0]["name"], serde_json::json!("alice"));
        assert_eq!(wire["meta"]["total"], 1);
        assert_eq!(wire["meta"]["has_more"], false);
    }
}
