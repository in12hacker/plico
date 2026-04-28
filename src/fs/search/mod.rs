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

pub mod memory;
pub mod bm25;
pub mod hnsw;

use std::path::Path;

use serde::{Deserialize, Serialize};

pub use memory::InMemoryBackend;
pub use bm25::Bm25Index;
pub use hnsw::HnswBackend;

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
}

impl SearchFilter {
    /// Returns true if the entry passes all filter criteria.
    pub fn matches(&self, meta: &SearchIndexMeta) -> bool {
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

    #[allow(dead_code)]
    pub fn with_time(mut self, since: i64, until: i64) -> Self {
        self.since = Some(since);
        self.until = Some(until);
        self
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
    fn persist_to(&self, _dir: &Path) -> Result<(), String> { Ok(()) }

    /// Restore index state from the given directory.
    /// Default no-op — backends that self-manage persistence override this.
    fn restore_from(&self, _dir: &Path) -> Result<(), String> { Ok(()) }
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
}
