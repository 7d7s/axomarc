//! # keel-search — search contract (item 32, L45/L46)
//!
//! Backend-agnostic search surface: `Searchable`, `Index`, `SearchQuery`,
//! `SearchResult`, `Facet`, `VectorQuery`, `HybridQuery` (11-SPEC item 32).
//! PostgreSQL/pgvector and OpenSearch plug in as adapters behind the same
//! [`SearchEngine`] port; the application never sees the engine.
//!
//! Tenancy is part of the contract: every document carries a tenant tag and
//! every query is executed inside a tenant — mismatches are invisible.

use std::collections::BTreeMap;
use std::sync::Mutex;

use keel_foundation::{AppError, TenantId};
use serde::{Deserialize, Serialize};

/// One indexable document. `text` feeds keyword scoring, `fields` feed filters
/// and facets, `vector` feeds similarity search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchDocument {
    pub id: String,
    pub tenant: TenantId,
    /// Keyword body (concatenated searchable text).
    pub text: String,
    /// Structured attributes for exact-match filters and facets.
    #[serde(default)]
    pub fields: BTreeMap<String, String>,
    /// Optional embedding for vector/hybrid search.
    #[serde(default)]
    pub vector: Vec<f32>,
}

/// Typeface for indexes: application-scoped names (`invoices`, `patients`).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Index(pub String);

/// Domain -> document projection. The data layer owns the mapping; search
/// never reaches back into aggregates.
pub trait Searchable {
    fn id(&self) -> String;
    fn text(&self) -> String;
    fn fields(&self) -> BTreeMap<String, String>;
    fn vector(&self) -> Vec<f32> {
        Vec::new()
    }
}

/// Keyword query: text match + exact filters + facets + pagination.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchQuery {
    pub tenant: TenantId,
    pub text: String,
    /// Exact field equality filters (AND-combined).
    #[serde(default)]
    pub filters: BTreeMap<String, String>,
    /// Fields to facet over the matching set.
    #[serde(default)]
    pub facets: Vec<String>,
    pub limit: usize,
    pub offset: usize,
}

/// One bucket inside a facet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FacetValue {
    pub value: String,
    pub count: usize,
}

/// Facet over one field of the matching set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Facet {
    pub field: String,
    pub values: Vec<FacetValue>,
}

/// One scored hit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hit {
    pub id: String,
    pub score: f32,
    pub fields: BTreeMap<String, String>,
}

/// Query outcome: total over the full match set, page of hits, facets.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchResult {
    pub total: usize,
    pub hits: Vec<Hit>,
    pub facets: Vec<Facet>,
}

/// Pure similarity query (pgvector `<=>` / OpenSearch kNN semantics).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorQuery {
    pub tenant: TenantId,
    pub vector: Vec<f32>,
    pub limit: usize,
    /// Minimum cosine similarity (0..=1); lower scores are dropped.
    pub min_score: f32,
}

/// Keyword + vector in one pass. `alpha` weights the keyword score
/// (`alpha * text + (1 - alpha) * similarity`); `vector` is the probe
/// embedding compared against each document embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridQuery {
    pub query: SearchQuery,
    pub vector: Vec<f32>,
    pub alpha: f32,
}

/// The port every engine implements (in-memory, pgvector, OpenSearch).
pub trait SearchEngine: Send + Sync {
    fn index(&self, index: &Index, doc: SearchDocument) -> Result<(), AppError>;
    fn delete(&self, index: &Index, id: &str) -> Result<(), AppError>;
    fn search(&self, index: &Index, query: &SearchQuery) -> Result<SearchResult, AppError>;
    fn vector_search(&self, index: &Index, query: &VectorQuery) -> Result<SearchResult, AppError>;
    fn hybrid_search(&self, index: &Index, query: &HybridQuery) -> Result<SearchResult, AppError>;
    fn count(&self, index: &Index, tenant: TenantId) -> Result<usize, AppError>;
}

/// In-memory reference engine: keyword scoring, exact filters, facets,
/// cosine similarity and alpha-blended hybrid. Deterministic ordering keeps
/// tests stable.
#[derive(Debug, Default)]
pub struct InMemorySearchEngine {
    /// index -> id -> document.
    docs: Mutex<BTreeMap<Index, BTreeMap<String, SearchDocument>>>,
}

impl InMemorySearchEngine {
    pub fn new() -> Self {
        Self::default()
    }

    fn keyword_score(query_text: &str, doc_text: &str) -> f32 {
        let query_terms: Vec<String> = tokenize(query_text);
        if query_terms.is_empty() {
            // No text: match everything with a neutral score so filters/facets
            // still work; ordering stays stable by id.
            return 0.0;
        }
        let doc_terms = tokenize(doc_text);
        let mut hits = 0usize;
        for term in &query_terms {
            if doc_terms.iter().any(|t| t.contains(term)) {
                hits += 1;
            }
        }
        hits as f32 / query_terms.len() as f32
    }

    fn matches_filters(doc: &SearchDocument, filters: &BTreeMap<String, String>) -> bool {
        filters
            .iter()
            .all(|(field, value)| doc.fields.get(field) == Some(value))
    }

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() || a.is_empty() {
            return 0.0;
        }
        let mut dot = 0.0f32;
        let mut na = 0.0f32;
        let mut nb = 0.0f32;
        for i in 0..a.len() {
            dot += a[i] * b[i];
            na += a[i] * a[i];
            nb += b[i] * b[i];
        }
        if na == 0.0 || nb == 0.0 {
            return 0.0;
        }
        dot / (na.sqrt() * nb.sqrt())
    }
}

fn tokenize(raw: &str) -> Vec<String> {
    raw.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

fn facet_docs(matched: &[&SearchDocument], fields: &[String]) -> Vec<Facet> {
    let mut facets = Vec::new();
    for field in fields {
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for doc in matched {
            if let Some(value) = doc.fields.get(field) {
                *counts.entry(value.as_str()).or_insert(0) += 1;
            }
        }
        let mut values: Vec<FacetValue> = counts
            .into_iter()
            .map(|(value, count)| FacetValue {
                value: value.to_string(),
                count,
            })
            .collect();
        values.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
        facets.push(Facet {
            field: field.clone(),
            values,
        });
    }
    facets
}

impl SearchEngine for InMemorySearchEngine {
    fn index(&self, index: &Index, doc: SearchDocument) -> Result<(), AppError> {
        if doc.id.is_empty() {
            return Err(AppError::validation("search document needs an id"));
        }
        self.docs
            .lock()
            .expect("search engine poisoned")
            .entry(index.clone())
            .or_default()
            .insert(doc.id.clone(), doc);
        Ok(())
    }

    fn delete(&self, index: &Index, id: &str) -> Result<(), AppError> {
        let mut docs = self.docs.lock().expect("search engine poisoned");
        match docs.get_mut(index) {
            Some(bucket) => {
                bucket.remove(id);
                Ok(())
            }
            None => Ok(()),
        }
    }

    fn search(&self, index: &Index, query: &SearchQuery) -> Result<SearchResult, AppError> {
        let docs = self.docs.lock().expect("search engine poisoned");
        let bucket = match docs.get(index) {
            Some(bucket) => bucket,
            None => return Ok(SearchResult::default()),
        };
        let mut matched: Vec<(f32, &SearchDocument)> = bucket
            .values()
            .filter(|doc| doc.tenant == query.tenant)
            .filter(|doc| Self::matches_filters(doc, &query.filters))
            .map(|doc| (Self::keyword_score(&query.text, &doc.text), doc))
            .filter(|(score, _)| query.text.is_empty() || *score > 0.0)
            .collect();
        matched.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.id.cmp(&b.1.id))
        });
        let total = matched.len();
        let facets = facet_docs(
            &matched.iter().map(|(_, doc)| *doc).collect::<Vec<_>>(),
            &query.facets,
        );
        let hits = matched
            .iter()
            .skip(query.offset)
            .take(if query.limit == 0 { total } else { query.limit })
            .map(|(score, doc)| Hit {
                id: doc.id.clone(),
                score: *score,
                fields: doc.fields.clone(),
            })
            .collect();
        Ok(SearchResult {
            total,
            hits,
            facets,
        })
    }

    fn vector_search(&self, index: &Index, query: &VectorQuery) -> Result<SearchResult, AppError> {
        let docs = self.docs.lock().expect("search engine poisoned");
        let bucket = match docs.get(index) {
            Some(bucket) => bucket,
            None => return Ok(SearchResult::default()),
        };
        let mut scored: Vec<(f32, &SearchDocument)> = bucket
            .values()
            .filter(|doc| doc.tenant == query.tenant && !doc.vector.is_empty())
            .map(|doc| (Self::cosine(&query.vector, &doc.vector), doc))
            .filter(|(score, _)| *score >= query.min_score)
            .collect();
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.id.cmp(&b.1.id))
        });
        let total = scored.len();
        let hits = scored
            .iter()
            .take(if query.limit == 0 { total } else { query.limit })
            .map(|(score, doc)| Hit {
                id: doc.id.clone(),
                score: *score,
                fields: doc.fields.clone(),
            })
            .collect();
        Ok(SearchResult {
            total,
            hits,
            facets: Vec::new(),
        })
    }

    fn hybrid_search(&self, index: &Index, query: &HybridQuery) -> Result<SearchResult, AppError> {
        let alpha = query.alpha.clamp(0.0, 1.0);
        let has_text = !query.query.text.is_empty();
        let has_probe = !query.vector.is_empty();
        let docs = self.docs.lock().expect("search engine poisoned");
        let bucket = match docs.get(index) {
            Some(bucket) => bucket,
            None => return Ok(SearchResult::default()),
        };
        let candidates: Vec<(f32, f32, &SearchDocument)> = bucket
            .values()
            .filter(|doc| doc.tenant == query.query.tenant)
            .filter(|doc| Self::matches_filters(doc, &query.query.filters))
            .map(|doc| {
                let kw = Self::keyword_score(&query.query.text, &doc.text);
                let sim = if has_probe && !doc.vector.is_empty() {
                    Self::cosine(&query.vector, &doc.vector)
                } else {
                    0.0
                };
                (kw, sim, doc)
            })
            .filter(|(kw, sim, _)| {
                if !has_text && !has_probe {
                    return true;
                }
                (has_text && *kw > 0.0) || (has_probe && *sim > 0.0)
            })
            .collect();
        let max_keyword = candidates
            .iter()
            .map(|(kw, _, _)| *kw)
            .fold(0.0f32, f32::max);
        let mut blended: Vec<(f32, &SearchDocument)> = candidates
            .into_iter()
            .map(|(kw, sim, doc)| {
                let kw_norm = if max_keyword > 0.0 {
                    kw / max_keyword
                } else {
                    0.0
                };
                (alpha * kw_norm + (1.0 - alpha) * sim, doc)
            })
            .collect();
        blended.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.id.cmp(&b.1.id))
        });
        let total = blended.len();
        let facets = facet_docs(
            &blended.iter().map(|(_, doc)| *doc).collect::<Vec<_>>(),
            &query.query.facets,
        );
        let hits = blended
            .iter()
            .skip(query.query.offset)
            .take(if query.query.limit == 0 {
                total
            } else {
                query.query.limit
            })
            .map(|(score, doc)| Hit {
                id: doc.id.clone(),
                score: *score,
                fields: doc.fields.clone(),
            })
            .collect();
        Ok(SearchResult {
            total,
            hits,
            facets,
        })
    }

    fn count(&self, index: &Index, tenant: TenantId) -> Result<usize, AppError> {
        let docs = self.docs.lock().expect("search engine poisoned");
        Ok(docs
            .get(index)
            .map(|bucket| bucket.values().filter(|doc| doc.tenant == tenant).count())
            .unwrap_or(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tenant() -> TenantId {
        TenantId::new()
    }

    fn doc(
        id: &str,
        tenant: TenantId,
        text: &str,
        fields: &[(&str, &str)],
        vector: Vec<f32>,
    ) -> SearchDocument {
        SearchDocument {
            id: id.into(),
            tenant,
            text: text.into(),
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            vector,
        }
    }

    #[test]
    fn keyword_search_scores_and_filters() {
        let engine = InMemorySearchEngine::new();
        let t = tenant();
        let index = Index("invoices".into());
        engine
            .index(
                &index,
                doc(
                    "1",
                    t,
                    "acme hospital invoice march",
                    &[("status", "posted")],
                    vec![],
                ),
            )
            .unwrap();
        engine
            .index(
                &index,
                doc(
                    "2",
                    t,
                    "acme hospital invoice april draft",
                    &[("status", "draft")],
                    vec![],
                ),
            )
            .unwrap();
        let result = engine
            .search(
                &index,
                &SearchQuery {
                    tenant: t,
                    text: "acme invoice".into(),
                    limit: 10,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(result.total, 2);

        let posted = engine
            .search(
                &index,
                &SearchQuery {
                    tenant: t,
                    text: "acme invoice".into(),
                    filters: [("status".into(), "posted".into())].into_iter().collect(),
                    limit: 10,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(posted.total, 1);
        assert_eq!(posted.hits[0].id, "1");
    }

    #[test]
    fn facets_count_the_matching_set() {
        let engine = InMemorySearchEngine::new();
        let t = tenant();
        let index = Index("invoices".into());
        engine
            .index(
                &index,
                doc("1", t, "invoice", &[("status", "posted")], vec![]),
            )
            .unwrap();
        engine
            .index(
                &index,
                doc("2", t, "invoice", &[("status", "posted")], vec![]),
            )
            .unwrap();
        engine
            .index(
                &index,
                doc("3", t, "invoice", &[("status", "draft")], vec![]),
            )
            .unwrap();
        let result = engine
            .search(
                &index,
                &SearchQuery {
                    tenant: t,
                    text: "invoice".into(),
                    facets: vec!["status".into()],
                    limit: 10,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(result.facets.len(), 1);
        assert_eq!(
            result.facets[0].values[0],
            FacetValue {
                value: "posted".into(),
                count: 2
            }
        );
        assert_eq!(
            result.facets[0].values[1],
            FacetValue {
                value: "draft".into(),
                count: 1
            }
        );
    }

    #[test]
    fn tenant_isolation_is_invisible() {
        let engine = InMemorySearchEngine::new();
        let index = Index("invoices".into());
        let ours = tenant();
        let theirs = tenant();
        engine
            .index(&index, doc("1", ours, "acme invoice", &[], vec![]))
            .unwrap();
        engine
            .index(&index, doc("2", theirs, "acme invoice", &[], vec![]))
            .unwrap();
        let result = engine
            .search(
                &index,
                &SearchQuery {
                    tenant: ours,
                    text: "acme".into(),
                    limit: 10,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(result.total, 1);
        assert_eq!(result.hits[0].id, "1");
        assert_eq!(engine.count(&index, ours).unwrap(), 1);
    }

    #[test]
    fn vector_search_orders_by_similarity() {
        let engine = InMemorySearchEngine::new();
        let t = tenant();
        let index = Index("notes".into());
        engine
            .index(&index, doc("close", t, "a", &[], vec![1.0, 0.0]))
            .unwrap();
        engine
            .index(&index, doc("far", t, "b", &[], vec![0.0, 1.0]))
            .unwrap();
        let result = engine
            .vector_search(
                &index,
                &VectorQuery {
                    tenant: t,
                    vector: vec![0.9, 0.1],
                    limit: 2,
                    min_score: 0.5,
                },
            )
            .unwrap();
        assert_eq!(result.total, 1);
        assert_eq!(result.hits[0].id, "close");
    }

    #[test]
    fn hybrid_blends_keyword_and_similarity() {
        let engine = InMemorySearchEngine::new();
        let t = tenant();
        let index = Index("notes".into());
        // Text match with an orthogonal embedding vs. vector match with no text match.
        engine
            .index(
                &index,
                doc("text-only", t, "billing summary", &[], vec![0.0, 1.0]),
            )
            .unwrap();
        engine
            .index(
                &index,
                doc("vector-only", t, "unrelated words", &[], vec![1.0, 0.0]),
            )
            .unwrap();
        let result = engine
            .hybrid_search(
                &index,
                &HybridQuery {
                    query: SearchQuery {
                        tenant: t,
                        text: "billing".into(),
                        limit: 10,
                        ..Default::default()
                    },
                    vector: vec![1.0, 0.0],
                    alpha: 0.5,
                },
            )
            .unwrap();
        assert_eq!(result.total, 2);
        let ids: Vec<&str> = result.hits.iter().map(|h| h.id.as_str()).collect();
        assert!(ids.contains(&"text-only") && ids.contains(&"vector-only"));
        // alpha=1.0 collapses to pure keyword ordering.
        let keyword_only = engine
            .hybrid_search(
                &index,
                &HybridQuery {
                    query: SearchQuery {
                        tenant: t,
                        text: "billing".into(),
                        limit: 10,
                        ..Default::default()
                    },
                    vector: vec![1.0, 0.0],
                    alpha: 1.0,
                },
            )
            .unwrap();
        assert_eq!(keyword_only.hits[0].id, "text-only");
    }

    #[test]
    fn delete_removes_documents() {
        let engine = InMemorySearchEngine::new();
        let t = tenant();
        let index = Index("invoices".into());
        engine
            .index(&index, doc("1", t, "acme", &[], vec![]))
            .unwrap();
        engine.delete(&index, "1").unwrap();
        assert_eq!(engine.count(&index, t).unwrap(), 0);
    }
}
