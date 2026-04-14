//! Semantic Search — Vector Index
//!
//! Provides semantic similarity search over stored objects using vector embeddings.
//!
//! # Architecture
//!
//! ```
//! SemanticSearch (trait)
//! ├── InMemoryBackend   — pure Rust, brute-force cosine similarity (MVP)
//! └── LanceDBBackend   — persistent, HNSW index, production use
//! ```
//!
//! The trait is designed so backends can be swapped without changing callers.
//!
//! # Embedding Flow
//!
//! When an object is stored via `SemanticFS::create()`:
//!   1. `EmbeddingProvider::embed(text)` → `Embedding` (Vec<f32>)
//!   2. `SemanticSearch::upsert(cid, embedding, meta)` → stored in index
//!
//! When searching via `SemanticFS::search()`:
//!   1. `EmbeddingProvider::embed(query)` → query embedding
//!   2. `SemanticSearch::search(query_embedding, k, filter)` → top-k results
//!
//! # Cosine Similarity
//!
//! All backends use cosine similarity: `cosine(A, B) = dot(A, B) / (|A| * |B|)`
//! Results are ranked by similarity score (higher = more relevant).

use std::sync::RwLock;

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
}

impl SearchFilter {
    pub fn matches(&self, meta: &SearchIndexMeta) -> bool {
        // Tag filtering
        if !self.require_tags.is_empty() {
            if !self.require_tags.iter().all(|t| meta.tags.contains(t)) {
                return false;
            }
        }
        if !self.exclude_tags.is_empty() {
            if self.exclude_tags.iter().any(|t| meta.tags.contains(t)) {
                return false;
            }
        }
        // Content type filtering
        if let Some(ct) = &self.content_type {
            if &meta.content_type != ct {
                return false;
            }
        }
        true
    }
}

/// Trait for semantic similarity search over embeddings.
///
/// Implementations must be thread-safe (`Send + Sync`).
pub trait SemanticSearch: Send + Sync {
    /// Store (or update) an embedding for a CID.
    /// If the CID already exists, its embedding is replaced.
    fn upsert(&self, cid: &str, embedding: &[f32], meta: SearchIndexMeta);

    /// Remove all embeddings for a CID.
    fn delete(&self, cid: &str);

    /// Search for the `k` most similar entries to the query embedding.
    ///
    /// Only entries matching `filter` are considered.
    /// Returns results sorted by score descending.
    fn search(&self, query: &[f32], k: usize, filter: &SearchFilter) -> Vec<SearchHit>;

    /// Total number of entries in the index.
    fn len(&self) -> usize;

    /// Check if the index is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

// ─── Pure-Rust In-Memory Backend ─────────────────────────────────────────────

/// An in-memory semantic search backend using brute-force cosine similarity.
///
/// Suitable for prototypes and up to ~10k entries. For larger corpora,
/// use `LanceDBBackend` which provides HNSW indexing.
pub struct InMemoryBackend {
    entries: RwLock<Vec<IndexEntry>>,
}

struct IndexEntry {
    cid: String,
    embedding: Vec<f32>,
    meta: SearchIndexMeta,
}

impl InMemoryBackend {
    pub fn new() -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
        }
    }

    /// Compute cosine similarity between two vectors.
    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() {
            return 0.0;
        }

        let mut dot = 0.0f32;
        let mut norm_a = 0.0f32;
        let mut norm_b = 0.0f32;

        for i in 0..a.len() {
            dot += a[i] * b[i];
            norm_a += a[i] * a[i];
            norm_b += b[i] * b[i];
        }

        let norm_product = (norm_a.sqrt()) * (norm_b.sqrt());
        if norm_product < 1e-10 {
            0.0
        } else {
            dot / norm_product
        }
    }
}

impl Default for InMemoryBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SemanticSearch for InMemoryBackend {
    fn upsert(&self, cid: &str, embedding: &[f32], meta: SearchIndexMeta) {
        let mut entries = self.entries.write().unwrap();

        // Replace existing entry for this CID
        if let Some(existing) = entries.iter_mut().find(|e| e.cid == cid) {
            existing.embedding = embedding.to_vec();
            existing.meta = meta;
            return;
        }

        entries.push(IndexEntry {
            cid: cid.to_string(),
            embedding: embedding.to_vec(),
            meta,
        });
    }

    fn delete(&self, cid: &str) {
        let mut entries = self.entries.write().unwrap();
        entries.retain(|e| e.cid != cid);
    }

    fn search(&self, query: &[f32], k: usize, filter: &SearchFilter) -> Vec<SearchHit> {
        let entries = self.entries.read().unwrap();

        let mut scored: Vec<_> = entries
            .iter()
            .filter(|e| filter.matches(&e.meta))
            .map(|e| {
                let score = Self::cosine(query, &e.embedding);
                SearchHit {
                    cid: e.cid.clone(),
                    score,
                    meta: e.meta.clone(),
                }
            })
            .collect();

        // Sort by score descending
        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        scored.truncate(k);
        scored
    }

    fn len(&self) -> usize {
        self.entries.read().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_embedding(dim: usize, seed: f32) -> Vec<f32> {
        (0..dim).map(|i| (seed * (i + 1) as f32).sin().abs()).collect()
    }

    #[test]
    fn test_cosine_similarity() {
        // Identical vectors → cosine = 1.0
        let v = vec![1.0, 0.0, 0.0];
        assert!((InMemoryBackend::cosine(&v, &v) - 1.0).abs() < 1e-6);

        // Orthogonal vectors → cosine ≈ 0.0
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!(InMemoryBackend::cosine(&a, &b).abs() < 1e-6);

        // Opposite vectors → cosine = -1.0
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((InMemoryBackend::cosine(&a, &b) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_upsert_and_search() {
        let backend = InMemoryBackend::new();
        let dim = 4;

        backend.upsert(
            "cid1",
            &sample_embedding(dim, 1.0),
            SearchIndexMeta {
                cid: "cid1".to_string(),
                tags: vec!["rust".to_string(), "ai".to_string()],
                snippet: "Rust AI systems".to_string(),
                content_type: "text".to_string(),
            },
        );
        backend.upsert(
            "cid2",
            &sample_embedding(dim, 2.0),
            SearchIndexMeta {
                cid: "cid2".to_string(),
                tags: vec!["python".to_string()],
                snippet: "Python web app".to_string(),
                content_type: "text".to_string(),
            },
        );

        assert_eq!(backend.len(), 2);

        // Search for something similar to the first embedding
        let results = backend.search(&sample_embedding(dim, 1.0), 2, &SearchFilter::default());
        assert!(!results.is_empty());
        // cid1 should be top result (identical embedding)
        assert_eq!(results[0].cid, "cid1");
        assert!((results[0].score - 1.0).abs() < 1e-4);
    }

    #[test]
    fn test_search_with_tag_filter() {
        let backend = InMemoryBackend::new();
        let dim = 4;

        backend.upsert("cid1", &sample_embedding(dim, 1.0), SearchIndexMeta {
            cid: "cid1".to_string(),
            tags: vec!["rust".to_string()],
            snippet: "".to_string(),
            content_type: "text".to_string(),
        });
        backend.upsert("cid2", &sample_embedding(dim, 2.0), SearchIndexMeta {
            cid: "cid2".to_string(),
            tags: vec!["python".to_string()],
            snippet: "".to_string(),
            content_type: "text".to_string(),
        });

        let filter = SearchFilter {
            require_tags: vec!["rust".to_string()],
            ..Default::default()
        };
        let results = backend.search(&sample_embedding(dim, 1.0), 10, &filter);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].cid, "cid1");
    }

    #[test]
    fn test_delete() {
        let backend = InMemoryBackend::new();
        let dim = 4;

        backend.upsert("cid1", &sample_embedding(dim, 1.0), SearchIndexMeta {
            cid: "cid1".to_string(),
            tags: vec![],
            snippet: "".to_string(),
            content_type: "text".to_string(),
        });

        assert_eq!(backend.len(), 1);
        backend.delete("cid1");
        assert!(backend.is_empty());
    }

    #[test]
    fn test_upsert_replaces_existing() {
        let backend = InMemoryBackend::new();
        let dim = 4;

        backend.upsert("cid1", &vec![1.0, 0.0, 0.0, 0.0], SearchIndexMeta {
            cid: "cid1".to_string(),
            tags: vec!["old".to_string()],
            snippet: "old".to_string(),
            content_type: "text".to_string(),
        });
        backend.upsert("cid1", &vec![0.0, 1.0, 0.0, 0.0], SearchIndexMeta {
            cid: "cid1".to_string(),
            tags: vec!["new".to_string()],
            snippet: "new".to_string(),
            content_type: "text".to_string(),
        });

        assert_eq!(backend.len(), 1);
        let filter = SearchFilter::default();
        let results = backend.search(&vec![0.0, 1.0, 0.0, 0.0], 1, &filter);
        assert_eq!(results[0].cid, "cid1");
        assert!(results[0].meta.tags.contains(&"new".to_string()));
    }
}
