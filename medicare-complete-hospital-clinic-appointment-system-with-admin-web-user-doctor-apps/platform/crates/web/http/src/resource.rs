//! Resources (S2 item 26, L10): ResponseViewModel -> wire rendering helpers.
//! The domain returns business results; a Resource (ResponseViewModel impl)
//! decides the exact wire representation. These helpers keep that path uniform.

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
}
