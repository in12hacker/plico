//! Semantic Search — Vector Index + BM25 Keyword Search
//!
//! Provides semantic similarity search over stored objects using vector embeddings,
//! complemented by BM25 keyword search for exact-term matching.
//!
//! # Architecture
//!
//! ```text
//! SemanticSearch (trait)
//! ├── InMemoryBackend   — pure Rust, brute-force cosine similarity (MVP)
//! └── HnswBackend       — persistent, HNSW ANN index, production use
//! ```
//!
//! The trait is designed so backends can be swapped without changing callers.
//! Kernel selects the backend via `SEARCH_BACKEND` env var.

pub mod bm25;
pub mod hnsw;
pub mod memory;

use std::path::Path;

use serde::{Deserialize, Serialize};

pub use bm25::Bm25Index;
pub use hnsw::HnswBackend;
pub use memory::InMemoryBackend;

/// Retrieval implementation invoked during one search execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchPath {
    Bm25,
    Vector,
    TagFallback,
    KnowledgeGraphTemporal,
    KnowledgeGraphPpr,
    KnowledgeGraphPathDiscovery,
    KnowledgeGraphCausal,
    Reranker,
}

/// Stable degradation observed at a non-embedding retrieval stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchStageDegradation {
    ExecutionFailed,
}

/// Candidate counts observed at a concrete retrieval path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchPathExecution {
    pub path: SearchPath,
    pub candidates: usize,
    pub accepted: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub degradation: Option<SearchStageDegradation>,
}

/// Stable reason for a query-time embedding degradation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingDegradation {
    ProviderUnavailable,
    ModelUnavailable,
    InputRejected,
    ExecutionFailed,
}

impl From<&crate::fs::embedding::EmbedError> for EmbeddingDegradation {
    fn from(error: &crate::fs::embedding::EmbedError) -> Self {
        use crate::fs::embedding::EmbedError;

        match error {
            EmbedError::Http(_) | EmbedError::ServerUnavailable(_) | EmbedError::SubprocessUnavailable => {
                Self::ProviderUnavailable
            }
            EmbedError::ModelNotFound(_) => Self::ModelUnavailable,
            EmbedError::InputTooLarge(_) => Self::InputRejected,
            EmbedError::Ollama(_) | EmbedError::Api(_) | EmbedError::Runtime(_) | EmbedError::Subprocess(_) => {
                Self::ExecutionFailed
            }
        }
    }
}

/// Whether the embedding provider was actually invoked for this query.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum EmbeddingQueryState {
    NotProbed {
        provider: String,
    },
    Succeeded {
        provider: String,
    },
    Degraded {
        provider: String,
        reason: EmbeddingDegradation,
    },
}

/// Truthful execution metadata for a search request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchExecution {
    pub paths: Vec<SearchPathExecution>,
    pub embedding: EmbeddingQueryState,
}

impl SearchExecution {
    pub(crate) fn new(provider: String) -> Self {
        Self {
            paths: Vec::new(),
            embedding: EmbeddingQueryState::NotProbed { provider },
        }
    }

    pub(crate) fn record_path(&mut self, path: SearchPath, candidates: usize, accepted: usize) {
        if let Some(existing) = self.paths.iter_mut().find(|entry| entry.path == path) {
            existing.candidates += candidates;
            existing.accepted += accepted;
        } else {
            self.paths.push(SearchPathExecution {
                path,
                candidates,
                accepted,
                degradation: None,
            });
        }
    }

    pub(crate) fn record_degradation(&mut self, path: SearchPath, degradation: SearchStageDegradation) {
        if let Some(existing) = self.paths.iter_mut().find(|entry| entry.path == path) {
            existing.degradation = Some(degradation);
        } else {
            self.paths.push(SearchPathExecution {
                path,
                candidates: 0,
                accepted: 0,
                degradation: Some(degradation),
            });
        }
    }
}

/// Search results plus the paths and provider state that produced them.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosedSearch {
    pub results: Vec<crate::fs::types::SearchResult>,
    pub execution: SearchExecution,
}

/// Metadata attached to a stored embedding entry.
#[derive(Debug, Clone)]
pub struct SearchIndexMeta {
    /// CID of the parent AIObject.
    pub cid: String,
    /// Tags from the parent object (used for tag filtering).
    pub tags: Vec<String>,
    /// Human-readable snippet for displaying results.
    pub snippet: String,
    /// Content type string.
    pub content_type: String,
    /// Creation timestamp (Unix ms), used for time-range filtering.
    pub created_at: u64,
    /// Trusted role that created the canonical object.
    pub owner_role: String,
    /// Local persisted namespace. This is internal access metadata, not a
    /// client-selectable tenant dimension.
    pub namespace: String,
    /// Canonical object visibility within the local namespace.
    pub scope: crate::cas::ObjectScope,
    /// Cognitive memory type for type-aware retrieval.
    pub memory_type: Option<crate::memory::layered::MemoryType>,
}

/// A search hit — a matching entry with relevance score.
#[derive(Debug, Clone)]
pub struct SearchHit {
    /// Content ID of the matched object.
    pub cid: String,
    /// Cosine similarity score [0, 1].
    pub score: f32,
    /// Stored metadata.
    pub meta: SearchIndexMeta,
}

/// Filter for narrowing semantic search.
#[derive(Debug, Clone, Default)]
pub struct SearchFilter {
    /// Require all of these tags (AND).
    pub require_tags: Vec<String>,
    /// Exclude entries with any of these tags.
    pub exclude_tags: Vec<String>,
    /// Content type filter.
    pub content_type: Option<String>,
    /// Inclusive lower bound on creation time (Unix ms). None = no lower bound.
    pub since: Option<i64>,
    /// Inclusive upper bound on creation time (Unix ms). None = no upper bound.
    pub until: Option<i64>,
    /// Cognitive memory type filter.
    pub memory_type: Option<crate::memory::layered::MemoryType>,
    /// Trusted runtime access constraint. Public request payloads must never
    /// construct this value from claimed identity fields.
    pub access: Option<SearchAccess>,
}

/// Trusted local access constraint applied before retrieval top-k selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchAccess {
    role_id: String,
    namespace: String,
    can_read_any: bool,
}

impl SearchAccess {
    pub(crate) fn new(role_id: &str, namespace: &str, can_read_any: bool) -> Self {
        Self {
            role_id: role_id.to_string(),
            namespace: namespace.to_string(),
            can_read_any,
        }
    }
}

impl SearchFilter {
    /// Returns true if the entry passes all filter criteria.
    pub fn matches(&self, meta: &SearchIndexMeta) -> bool {
        if let Some(access) = &self.access {
            if meta.namespace != access.namespace {
                return false;
            }
            if !access.can_read_any
                && meta.scope == crate::cas::ObjectScope::Private
                && meta.owner_role != access.role_id
            {
                return false;
            }
        }
        if !self.require_tags.is_empty() && !self.require_tags.iter().all(|t| meta.tags.contains(t)) {
            return false;
        }
        if !self.exclude_tags.is_empty() && self.exclude_tags.iter().any(|t| meta.tags.contains(t)) {
            return false;
        }
        if let Some(ct) = &self.content_type {
            if &meta.content_type != ct {
                return false;
            }
        }
        if let Some(since) = self.since {
            if (meta.created_at as i64) < since {
                return false;
            }
        }
        if let Some(until) = self.until {
            if (meta.created_at as i64) > until {
                return false;
            }
        }
        if let Some(ref mt) = self.memory_type {
            if meta.memory_type.as_ref() != Some(mt) {
                return false;
            }
        }
        true
    }

    pub fn with_time(mut self, since: i64, until: i64) -> Self {
        self.since = Some(since);
        self.until = Some(until);
        self
    }

    pub(crate) fn with_access(mut self, role_id: &str, namespace: &str, can_read_any: bool) -> Self {
        self.access = Some(SearchAccess::new(role_id, namespace, can_read_any));
        self
    }

    pub(crate) fn is_cache_neutral(&self) -> bool {
        self.require_tags.is_empty()
            && self.exclude_tags.is_empty()
            && self.content_type.is_none()
            && self.since.is_none()
            && self.until.is_none()
            && self.memory_type.is_none()
            && self.access.is_none()
    }
}

/// Serializable search index entry for persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchIndexEntry {
    pub cid: String,
    pub embedding: Vec<f32>,
    pub tags: Vec<String>,
    pub snippet: String,
    pub content_type: String,
    pub created_at: u64,
    pub owner_role: String,
    pub namespace: String,
    pub scope: crate::cas::ObjectScope,
}

/// Trait for semantic similarity search over embeddings.
///
/// Implementations must be thread-safe (`Send + Sync`).
pub trait SemanticSearch: Send + Sync {
    /// Store (or update) an embedding for a CID.
    fn upsert(&self, cid: &str, embedding: &[f32], meta: SearchIndexMeta);

    /// Remove all embeddings for a CID.
    fn delete(&self, cid: &str);

    /// Search for the `k` most similar entries to the query embedding.
    fn search(&self, query: &[f32], k: usize, filter: &SearchFilter) -> Vec<SearchHit>;

    /// Total number of entries in the index.
    fn len(&self) -> usize;

    /// Check if the index is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Return all CIDs whose metadata matches the filter (no vector ranking).
    fn list_by_filter(&self, filter: &SearchFilter) -> Vec<String>;

    /// Persist the index state to the given directory.
    /// Default no-op — backends that self-manage persistence override this.
    fn persist_to(&self, _dir: &Path) -> Result<(), String> {
        Ok(())
    }

    /// Restore index state from the given directory.
    /// Default no-op — backends that self-manage persistence override this.
    fn restore_from(&self, _dir: &Path) -> Result<(), String> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(tags: &[&str], ct: &str, created: u64) -> SearchIndexMeta {
        SearchIndexMeta {
            cid: "test".into(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            snippet: "".into(),
            content_type: ct.into(),
            created_at: created,
            owner_role: "owner".into(),
            namespace: "default".into(),
            scope: crate::cas::ObjectScope::Shared,
            memory_type: None,
        }
    }

    #[test]
    fn test_filter_no_constraints() {
        let f = SearchFilter::default();
        assert!(f.matches(&meta(&["a"], "text", 1000)));
    }

    #[test]
    fn test_filter_require_tags() {
        let f = SearchFilter {
            require_tags: vec!["a".into(), "b".into()],
            ..Default::default()
        };
        assert!(f.matches(&meta(&["a", "b", "c"], "text", 1000)));
        assert!(!f.matches(&meta(&["a"], "text", 1000)));
        assert!(!f.matches(&meta(&[], "text", 1000)));
    }

    #[test]
    fn test_filter_exclude_tags() {
        let f = SearchFilter {
            exclude_tags: vec!["spam".into()],
            ..Default::default()
        };
        assert!(f.matches(&meta(&["a"], "text", 1000)));
        assert!(!f.matches(&meta(&["a", "spam"], "text", 1000)));
    }

    #[test]
    fn test_filter_content_type() {
        let f = SearchFilter {
            content_type: Some("image".into()),
            ..Default::default()
        };
        assert!(f.matches(&meta(&[], "image", 1000)));
        assert!(!f.matches(&meta(&[], "text", 1000)));
    }

    #[test]
    fn test_filter_time_range() {
        let f = SearchFilter {
            since: Some(500),
            until: Some(1500),
            ..Default::default()
        };
        assert!(f.matches(&meta(&[], "text", 1000)));
        assert!(!f.matches(&meta(&[], "text", 400)));
        assert!(!f.matches(&meta(&[], "text", 1600)));
    }

    #[test]
    fn test_filter_with_time_builder() {
        let f = SearchFilter::default().with_time(100, 200);
        assert!(f.matches(&meta(&[], "text", 150)));
        assert!(!f.matches(&meta(&[], "text", 50)));
    }

    #[test]
    fn search_execution_serializes_stable_path_and_degradation_names() {
        let execution = SearchExecution {
            paths: vec![SearchPathExecution {
                path: SearchPath::Bm25,
                candidates: 3,
                accepted: 2,
                degradation: None,
            }],
            embedding: EmbeddingQueryState::Degraded {
                provider: "test-provider".to_string(),
                reason: EmbeddingDegradation::ProviderUnavailable,
            },
        };

        let value = serde_json::to_value(execution).unwrap();
        assert_eq!(value["paths"][0]["path"], "bm25");
        assert_eq!(value["embedding"]["state"], "degraded");
        assert_eq!(value["embedding"]["reason"], "provider_unavailable");
    }

    #[test]
    fn access_filter_rejects_cross_role_private_objects_before_ranking() {
        let filter = SearchFilter::default().with_access("reader", "default", false);
        let mut private = meta(&[], "text", 1000);
        private.owner_role = "other".into();
        private.scope = crate::cas::ObjectScope::Private;
        assert!(!filter.matches(&private));

        private.scope = crate::cas::ObjectScope::Shared;
        assert!(filter.matches(&private));
        private.namespace = "other-local-space".into();
        assert!(!filter.matches(&private));
    }

    #[test]
    fn embedding_degradation_classifies_without_exposing_provider_error_text() {
        let unavailable = crate::fs::embedding::EmbedError::ServerUnavailable("secret endpoint".into());
        let rejected = crate::fs::embedding::EmbedError::InputTooLarge("private content".into());

        assert_eq!(
            EmbeddingDegradation::from(&unavailable),
            EmbeddingDegradation::ProviderUnavailable
        );
        assert_eq!(
            EmbeddingDegradation::from(&rejected),
            EmbeddingDegradation::InputRejected
        );
    }
}
