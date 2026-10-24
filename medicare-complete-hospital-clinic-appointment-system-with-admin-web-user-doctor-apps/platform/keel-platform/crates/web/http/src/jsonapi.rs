//! # Wave 8 decision gate (ADR 0007): JSON:API serializer
//!
//! Pure content negotiation + compound-document rendering for resource
//! routes. The serializer reads nothing but its arguments (resource type,
//! the split operation result, the raw query and the request URI), so
//! `render_success` stays the only integration point and the default wire
//! path is untouched: negotiation `Absent` means today's bytes, byte for
//! byte.
//!
//! Divergences are recorded in ADR 0007: errors keep the platform envelope
//! (never JSON:API error objects), and any media-type parameter on a
//! matched `Accept` entry is a 406 envelope rather than a JSON:API
//! document.

use std::collections::{HashMap, HashSet};

use serde_json::{json, Map, Value};

/// The JSON:API media type (1.1). Essence match is case-insensitive; the
/// response never carries media-type parameters.
pub const MEDIA_TYPE: &str = "application/vnd.api+json";

/// Outcome of parsing the request's `Accept` header against [`MEDIA_TYPE`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Negotiation {
    /// A `q>0` entry for the media type with no media-type parameters:
    /// render the JSON:API document.
    Matched,
    /// Header absent, or every entry for the media type is `q=0`: run the
    /// current renderer (byte-identical default).
    Absent,
    /// A `q>0` entry carries a media-type parameter (e.g. `;ext[]`) — 406
    /// with the standard platform envelope.
    UnsupportedParameters,
}

/// Parse `Accept` as a comma list with `q` weights (case-insensitive
/// essence match, ADR 0007 "Activation").
///
/// Order of rules per matching entry: `q=0` (or a `q` parsing below the
/// threshold) makes the entry not acceptable and its parameters are never
/// inspected; a `q>0` entry with any non-`q` parameter refuses the request
/// outright; otherwise the media type matched. An unparseable `q` weights
/// as present (RFC 9110: invalid parameters are ignored).
pub fn negotiate(accept: Option<&str>) -> Negotiation {
    let Some(header) = accept else {
        return Negotiation::Absent;
    };
    let mut matched = false;
    for entry in header.split(',') {
        let mut parts = entry.split(';');
        let essence = parts.next().unwrap_or_default().trim();
        if !essence.eq_ignore_ascii_case(MEDIA_TYPE) {
            continue;
        }
        let mut q: Option<&str> = None;
        let mut parameters = false;
        for param in parts {
            let param = param.trim();
            if param.is_empty() {
                continue;
            }
            match param.split_once('=') {
                Some((name, value)) if name.trim().eq_ignore_ascii_case("q") => {
                    q = Some(value.trim());
                }
                Some(_) => parameters = true,
                None => parameters = true,
            }
        }
        let acceptable = match q {
            Some(raw) => raw
                .trim_matches('"')
                .parse::<f64>()
                .map(|weight| weight > 0.0)
                .unwrap_or(true),
            None => true,
        };
        if !acceptable {
            continue;
        }
        if parameters {
            return Negotiation::UnsupportedParameters;
        }
        matched = true;
    }
    if matched {
        Negotiation::Matched
    } else {
        Negotiation::Absent
    }
}

/// The mapping input split out of one operation result: the resource body
/// (item object or array), the top-level `meta` object and the reserved
/// `included` array. The handler renders through the Laravel-style wrap
/// (`Resource::wrap()`, default `"data"`); the JSON:API document REPLACES
/// that step, so `split` reads the wrap back off — payload is the `data`
/// slot when present, otherwise the remaining members are the body
/// (documented collision: a body with its own `data` key must use the
/// default wrap contract; custom wrap keys read as attributes).
pub struct Split {
    /// The resource body: item object, array of items, or (fail-closed) a
    /// scalar that renders as `data: null`.
    pub payload: Value,
    /// Top-level `meta` object (non-empty; an empty object is omitted).
    pub meta: Option<Value>,
    /// Reserved `included` array, lifted entry-wise into the document.
    pub included: Option<Vec<Value>>,
}

/// Split one operation result into [`Split`] (see the type docs for the
/// wrap contract).
pub fn split(result: &Value) -> Split {
    match result {
        Value::Object(map) => {
            let meta = map
                .get("meta")
                .filter(|meta| meta.as_object().is_some_and(|meta| !meta.is_empty()))
                .cloned();
            let included = map.get("included").and_then(Value::as_array).cloned();
            let payload = match map.get("data") {
                Some(data) if data.is_object() || data.is_array() => data.clone(),
                _ => {
                    let mut body = map.clone();
                    if meta.is_some() {
                        body.remove("meta");
                    }
                    if included.is_some() {
                        body.remove("included");
                    }
                    Value::Object(body)
                }
            };
            Split {
                payload,
                meta,
                included,
            }
        }
        other => Split {
            payload: other.clone(),
            meta: None,
            included: None,
        },
    }
}

/// Render the JSON:API document (ADR 0007 "Resource mapping spec").
///
/// * `type_name` — the route's declared resource name (the `type` of every
///   primary resource; included entries carry their own).
/// * `split` — payload + meta + included from [`split`].
/// * `query` — the raw query string (parse-without-decode posture; field
///   names are simple identifiers so they never need decoding).
/// * `request_uri` — path + query of the request (`links.self`, pagination
///   rewriting).
pub fn render_document(
    type_name: &str,
    split: &Split,
    query: Option<&str>,
    request_uri: &str,
) -> Value {
    let fields = parse_fields(query);
    let mut document = Map::new();

    let data = match &split.payload {
        Value::Array(items) => Value::Array(
            items
                .iter()
                .filter_map(Value::as_object)
                .map(|item| {
                    resource_object(
                        type_name,
                        item,
                        Some(request_uri),
                        fields_for(&fields, type_name),
                    )
                })
                .collect(),
        ),
        Value::Object(item) => resource_object(
            type_name,
            item,
            Some(request_uri),
            fields_for(&fields, type_name),
        ),
        // Fail-closed: primary data is object/array/null, never a scalar.
        _ => Value::Null,
    };
    document.insert("data".to_string(), data);

    if let Some(entries) = &split.included {
        let mut seen: HashSet<(String, String)> = HashSet::new();
        let mut included: Vec<Value> = Vec::new();
        for entry in entries {
            let Some(item) = entry.as_object() else {
                continue; // not an object — dropped, never spec-invalid
            };
            let Some(entry_type) = item
                .get("type")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
            else {
                continue; // no usable `type` — fail-closed drop (ADR 0007)
            };
            let mapped = resource_object(entry_type, item, None, fields_for(&fields, entry_type));
            let key = (entry_type.to_string(), canonical_id(&mapped["id"]));
            if !seen.insert(key) {
                continue; // (type, id) duplicate — first wins
            }
            included.push(mapped);
        }
        if !included.is_empty() {
            document.insert("included".to_string(), Value::Array(included));
        }
    }

    let mut links = Map::new();
    links.insert("self".to_string(), Value::String(request_uri.to_string()));
    if let Some(pagination) = page_links(split.meta.as_ref(), request_uri) {
        links.extend(pagination);
    }
    document.insert("links".to_string(), Value::Object(links));

    if let Some(meta) = &split.meta {
        document.insert("meta".to_string(), meta.clone());
    }

    Value::Object(document)
}

/// One resource object: `type` + lifted `id` + reserved-stripped (and
/// sparse-fieldset filtered) `attributes` + shape-checked `relationships`
/// + `links.self` (root resources only — included entries never get one).
fn resource_object(
    type_name: &str,
    body: &Map<String, Value>,
    self_href: Option<&str>,
    fields: Option<&HashSet<String>>,
) -> Value {
    let mut resource = Map::new();
    resource.insert("type".to_string(), Value::String(type_name.to_string()));
    resource.insert("id".to_string(), lift_id(body.get("id")));

    let attributes: Map<String, Value> = body
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "id" | "type" | "relationships" | "included"))
        .filter(|(key, _)| fields.is_none_or(|keep| keep.contains(key.as_str())))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    resource.insert("attributes".to_string(), Value::Object(attributes));

    if let Some(relationships) = body.get("relationships").and_then(Value::as_object) {
        let kept: Map<String, Value> = relationships
            .iter()
            .filter(|(name, _)| fields.is_none_or(|keep| keep.contains(name.as_str())))
            .filter(|(_, value)| is_valid_relationship(value))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        if !kept.is_empty() {
            resource.insert("relationships".to_string(), Value::Object(kept));
        }
    }

    if let Some(href) = self_href {
        resource.insert("links".to_string(), json!({ "self": href }));
    }
    Value::Object(resource)
}

/// `id` lift: strings verbatim, other scalars as their canonical string,
/// absent/non-scalar → `null` (ADR 0007 mapping table).
fn lift_id(id: Option<&Value>) -> Value {
    match id {
        Some(Value::String(raw)) => Value::String(raw.clone()),
        Some(Value::Number(raw)) => Value::String(raw.to_string()),
        Some(Value::Bool(raw)) => Value::String(raw.to_string()),
        _ => Value::Null,
    }
}

/// Canonical string of a lifted id (dedupe keys).
fn canonical_id(id: &Value) -> String {
    match id {
        Value::String(raw) => raw.clone(),
        other => other.to_string(),
    }
}

/// Relationship shape check: the member must be an object whose `data` is
/// a `{type, id}` resource identifier or an array of them (`null` and
/// malformed entries fail — an emptied object is omitted by the caller).
fn is_valid_relationship(value: &Value) -> bool {
    let Some(member) = value.as_object() else {
        return false;
    };
    match member.get("data") {
        Some(Value::Object(entry)) => is_resource_identifier(entry),
        Some(Value::Array(entries)) => entries
            .iter()
            .all(|entry| entry.as_object().is_some_and(is_resource_identifier)),
        _ => false,
    }
}

fn is_resource_identifier(entry: &Map<String, Value>) -> bool {
    entry.get("type").and_then(Value::as_str).is_some()
        && matches!(
            entry.get("id"),
            Some(Value::String(_) | Value::Number(_) | Value::Bool(_))
        )
}

/// `fields[<type>]=a,b` → per-type keep sets. Raw parsing (the platform
/// never decodes query strings; field names are `[a-z0-9_.-]` identifiers).
fn parse_fields(query: Option<&str>) -> Option<HashMap<String, HashSet<String>>> {
    let query = query?;
    let mut fields: HashMap<String, HashSet<String>> = HashMap::new();
    let mut found = false;
    for segment in query.split('&') {
        let Some((key, value)) = segment.split_once('=') else {
            continue;
        };
        let Some(type_name) = key
            .strip_prefix("fields[")
            .and_then(|k| k.strip_suffix(']'))
        else {
            continue;
        };
        if type_name.is_empty() {
            continue;
        }
        found = true;
        let keep = fields.entry(type_name.to_string()).or_default();
        for name in value.split(',') {
            if !name.is_empty() {
                keep.insert(name.to_string());
            }
        }
    }
    found.then_some(fields)
}

/// The keep set for one occurring type (a `fields[<type>]` naming an absent
/// type is never looked up, hence ignored).
fn fields_for<'a>(
    fields: &'a Option<HashMap<String, HashSet<String>>>,
    type_name: &str,
) -> Option<&'a HashSet<String>> {
    fields.as_ref().and_then(|map| map.get(type_name))
}

/// `links.first/prev/next/last` from `page`/`per_page` (+ `total`) in the
/// top-level meta: `first` at page+per_page, bounds only with `total`
/// (ADR 0007 "Pagination + links"); `self` always lives in the caller.
fn page_links(meta: Option<&Value>, request_uri: &str) -> Option<Map<String, Value>> {
    let meta = meta?;
    let page = meta.get("page").and_then(Value::as_u64)?;
    let per_page = meta.get("per_page").and_then(Value::as_u64)?;
    let mut links = Map::new();
    links.insert(
        "first".to_string(),
        Value::String(with_page(request_uri, 1)),
    );
    if let Some(total) = meta.get("total").and_then(Value::as_u64) {
        let last = total.div_ceil(per_page.max(1)).max(1);
        if page > 1 {
            links.insert(
                "prev".to_string(),
                Value::String(with_page(request_uri, page - 1)),
            );
        }
        if page < last {
            links.insert(
                "next".to_string(),
                Value::String(with_page(request_uri, page + 1)),
            );
        }
        links.insert(
            "last".to_string(),
            Value::String(with_page(request_uri, last)),
        );
    }
    Some(links)
}

/// Rewrite the `page` query parameter on the request URI (replacing any
/// existing `page`, preserving every other parameter and the raw posture).
fn with_page(request_uri: &str, page: u64) -> String {
    let (path, query) = request_uri.split_once('?').unwrap_or((request_uri, ""));
    let mut segments: Vec<String> = query
        .split('&')
        .filter(|segment| !segment.is_empty())
        .filter(|segment| segment.split('=').next().unwrap_or_default() != "page")
        .map(str::to_string)
        .collect();
    segments.push(format!("page={page}"));
    format!("{path}?{}", segments.join("&"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(type_name: &str, result: Value, query: Option<&str>, uri: &str) -> Value {
        render_document(type_name, &split(&result), query, uri)
    }

    // ------------------------------------------------------- negotiation

    #[test]
    fn accept_absent_is_not_negotiated() {
        assert_eq!(negotiate(None), Negotiation::Absent);
        assert_eq!(
            negotiate(Some("application/json, */*")),
            Negotiation::Absent
        );
    }

    #[test]
    fn accept_q_zero_falls_back_to_the_default_renderer() {
        assert_eq!(
            negotiate(Some("application/vnd.api+json;q=0")),
            Negotiation::Absent
        );
        assert_eq!(
            negotiate(Some("text/html, application/vnd.api+json;q=0.0")),
            Negotiation::Absent
        );
    }

    #[test]
    fn accept_match_is_case_insensitive_and_q_weighted() {
        assert_eq!(
            negotiate(Some("Application/VND.API+JSON")),
            Negotiation::Matched
        );
        assert_eq!(
            negotiate(Some("text/html;q=0.5, application/vnd.api+json;q=0.9")),
            Negotiation::Matched
        );
        assert_eq!(
            negotiate(Some("application/vnd.api+json")),
            Negotiation::Matched
        );
    }

    #[test]
    fn media_type_parameters_refuse_the_request() {
        assert_eq!(
            negotiate(Some("application/vnd.api+json;ext[]=x")),
            Negotiation::UnsupportedParameters
        );
        assert_eq!(
            negotiate(Some("application/vnd.api+json;q=0.9;profile=a")),
            Negotiation::UnsupportedParameters
        );
        // q=0 makes the parameterized entry not acceptable first.
        assert_eq!(
            negotiate(Some("application/vnd.api+json;ext[]=x;q=0")),
            Negotiation::Absent
        );
        // One good entry does not mask a parameterized one.
        assert_eq!(
            negotiate(Some(
                "application/vnd.api+json, application/vnd.api+json;ext[]=x"
            )),
            Negotiation::UnsupportedParameters
        );
    }

    // --------------------------------------------- item + collection docs

    #[test]
    fn item_document_lifts_id_and_type_and_strips_reserved_members() {
        let body = json!({
            "data": {
                "id": "n-1",
                "title": "hello",
                "body": "world",
                "type": "note",
                "relationships": { "author": { "data": { "type": "people", "id": "p-1" } } },
                "included": [{ "type": "people", "id": "p-1" }]
            }
        });
        let document = doc("notes", body, None, "/notes/n-1");
        assert_eq!(document["data"]["type"], "notes");
        assert_eq!(document["data"]["id"], "n-1");
        assert_eq!(document["data"]["attributes"]["title"], "hello");
        assert_eq!(document["data"]["attributes"]["body"], "world");
        assert!(document["data"]["attributes"].get("id").is_none());
        assert!(document["data"]["attributes"].get("type").is_none());
        assert!(document["data"]["attributes"]
            .get("relationships")
            .is_none());
        assert!(document["data"]["attributes"].get("included").is_none());
        assert_eq!(document["data"]["links"]["self"], "/notes/n-1");
        assert_eq!(document["links"]["self"], "/notes/n-1");
        assert_eq!(
            document["data"]["relationships"]["author"]["data"]["type"],
            "people"
        );
    }

    #[test]
    fn non_scalar_ids_lift_as_canonical_strings_and_absent_as_null() {
        let body = json!([{ "id": 7, "name": "seven" }, { "name": "no id" }]);
        let document = doc("notes", body, None, "/notes");
        assert_eq!(document["data"][0]["id"], "7");
        assert_eq!(document["data"][1]["id"], Value::Null);
        assert_eq!(document["data"][0]["type"], "notes");
        assert_eq!(document["data"][1]["type"], "notes");
        assert_eq!(document["data"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn collection_preserves_order_and_scalar_payloads_fail_closed_to_null() {
        let body = json!([{ "id": "a" }, { "id": "b" }, { "id": "c" }]);
        let document = doc("notes", body, None, "/notes");
        let ids: Vec<&str> = document["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["a", "b", "c"]);

        let scalar = doc("notes", json!("nope"), None, "/notes");
        assert_eq!(scalar["data"], Value::Null);
    }

    // ----------------------------------------------------- relationships

    #[test]
    fn relationships_survive_only_in_shape_and_empty_omits() {
        let body = json!({
            "id": "n-1",
            "relationships": {
                "author": { "data": { "type": "people", "id": "p-1" } },
                "tags": { "data": [{ "type": "tags", "id": "t-1" }] },
                "missing": { "data": null },
                "garbage": { "data": { "type": "people" } },
                "malformed": "not-an-object"
            }
        });
        let document = doc("notes", body, None, "/notes/n-1");
        let relationships = &document["data"]["relationships"];
        assert!(relationships.get("author").is_some());
        assert!(relationships.get("tags").is_some());
        assert!(relationships.get("missing").is_none());
        assert!(relationships.get("garbage").is_none());
        assert!(relationships.get("malformed").is_none());

        let none = doc("notes", json!({ "id": "n-1" }), None, "/notes/n-1");
        assert!(none["data"].get("relationships").is_none());
    }

    // ---------------------------------------------------------- includes

    #[test]
    fn includes_lift_dedupe_and_drop_unusable_entries() {
        let body = json!({
            "data": [{ "id": "n-1", "title": "first" }],
            "included": [
                { "type": "people", "id": "p-1", "name": "Ada" },
                { "type": "people", "id": "p-1", "name": "dupe" },
                { "id": "p-2", "name": "no type" },
                { "type": "", "name": "empty type" },
                "not-an-object",
                { "type": "people", "id": 9, "name": "numeric id" }
            ]
        });
        let document = doc("notes", body, None, "/notes");
        let included = document["included"].as_array().unwrap();
        assert_eq!(included.len(), 2);
        assert_eq!(included[0]["type"], "people");
        assert_eq!(included[0]["id"], "p-1");
        assert_eq!(included[0]["attributes"]["name"], "Ada");
        assert!(included[0].get("links").is_none(), "no self on included");
        assert_eq!(included[1]["id"], "9");

        let empty = json!({ "data": { "id": "n-1" }, "included": [{ "id": "x" }] });
        let document = doc("notes", empty, None, "/notes/n-1");
        assert!(document.get("included").is_none(), "empty result omitted");
    }

    // ------------------------------------------------- sparse fieldsets

    #[test]
    fn sparse_fieldsets_filter_attributes_and_relationships_per_type() {
        let body = json!({
            "data": {
                "id": "n-1",
                "title": "keep",
                "body": "drop",
                "relationships": {
                    "author": { "data": { "type": "people", "id": "p-1" } },
                    "tags": { "data": [] }
                }
            },
            "included": [
                { "type": "people", "id": "p-1", "name": "Ada", "email": "a@x" }
            ]
        });
        let document = doc(
            "notes",
            body.clone(),
            Some("fields[notes]=title,author&fields[people]=name"),
            "/notes",
        );
        assert_eq!(document["data"]["attributes"]["title"], "keep");
        assert!(document["data"]["attributes"].get("body").is_none());
        assert!(document["data"]["relationships"].get("author").is_some());
        assert!(document["data"]["relationships"].get("tags").is_none());
        // id/type always present under a fieldset.
        assert_eq!(document["data"]["id"], "n-1");
        assert_eq!(document["data"]["type"], "notes");
        // Included entries filter by THEIR type; unknown types are ignored.
        let included = &document["included"][0];
        assert_eq!(included["attributes"]["name"], "Ada");
        assert!(included["attributes"].get("email").is_none());

        let unknown = doc("notes", body.clone(), Some("fields[absent]=x"), "/notes");
        assert_eq!(unknown["data"]["attributes"]["title"], "keep");
    }

    // ---------------------------------------------------- meta and links

    #[test]
    fn meta_is_verbatim_and_empty_is_omitted() {
        let body = json!({
            "data": [{ "id": "n-1" }],
            "meta": { "page": 1, "per_page": 15, "total": 42, "has_more": true }
        });
        let document = doc("notes", body, None, "/notes?page=1");
        assert_eq!(document["meta"]["has_more"], true);
        assert_eq!(document["meta"]["total"], 42);

        let body = json!({ "data": [{ "id": "n-1" }], "meta": {} });
        let document = doc("notes", body, None, "/notes");
        assert!(document.get("meta").is_none());
    }

    #[test]
    fn pagination_links_rewrite_page_at_the_bounds() {
        let result = json!({
            "data": [{ "id": "n-1" }],
            "meta": { "page": 2, "per_page": 2, "total": 5, "has_more": true }
        });
        let document = doc("notes", result, None, "/notes?page=2&per_page=2");
        assert_eq!(document["links"]["self"], "/notes?page=2&per_page=2");
        assert_eq!(document["links"]["first"], "/notes?per_page=2&page=1");
        assert_eq!(document["links"]["prev"], "/notes?per_page=2&page=1");
        assert_eq!(document["links"]["next"], "/notes?per_page=2&page=3");
        assert_eq!(document["links"]["last"], "/notes?per_page=2&page=3");

        let first = json!({
            "data": [{ "id": "n-1" }],
            "meta": { "page": 1, "per_page": 2, "total": 2, "has_more": false }
        });
        let document = doc("notes", first, None, "/notes?page=1&per_page=2");
        assert!(document["links"].get("prev").is_none(), "prev omits at 1");
        assert!(
            document["links"].get("next").is_none(),
            "next omits at last"
        );
        assert_eq!(document["links"]["last"], "/notes?per_page=2&page=1");
    }

    #[test]
    fn pagination_links_without_total_carry_first_and_self_only() {
        let result = json!({
            "data": [{ "id": "n-1" }],
            "meta": { "page": 3, "per_page": 15, "has_more": true }
        });
        let document = doc("notes", result, None, "/notes?page=3");
        assert_eq!(document["links"]["first"], "/notes?page=1");
        assert!(document["links"].get("next").is_none());
        assert!(document["links"].get("prev").is_none());
        assert!(document["links"].get("last").is_none());

        let no_meta = doc("notes", json!({ "id": "n-1" }), None, "/notes/n-1");
        assert_eq!(no_meta["links"]["self"], "/notes/n-1");
        assert_eq!(no_meta["links"].as_object().unwrap().len(), 1);
    }
}
