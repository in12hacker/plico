//! In-memory vector search backend — brute-force cosine similarity.
//!
//! Suitable for prototypes and up to ~10k entries. For larger corpora,
//! use `HnswBackend` which provides HNSW ANN indexing with persistence.

use std::sync::RwLock;

use super::{SearchFilter, SearchHit, SearchIndexEntry, SearchIndexMeta, SemanticSearch};

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

    /// Export all index entries for persistence.
    pub fn snapshot(&self) -> Vec<SearchIndexEntry> {
        self.entries.read().unwrap().iter().map(|e| SearchIndexEntry {
            cid: e.cid.clone(),
            embedding: e.embedding.clone(),
            tags: e.meta.tags.clone(),
            snippet: e.meta.snippet.clone(),
            content_type: e.meta.content_type.clone(),
            created_at: e.meta.created_at,
        }).collect()
    }

    /// Bulk-load entries from a persisted snapshot (upsert semantics).
    pub fn restore(&self, entries: Vec<SearchIndexEntry>) {
        let mut store = self.entries.write().unwrap();
        for e in entries {
            if let Some(existing) = store.iter_mut().find(|existing| existing.cid == e.cid) {
                existing.embedding = e.embedding;
                existing.meta = SearchIndexMeta {
                    cid: e.cid.clone(),
                    tags: e.tags,
                    snippet: e.snippet,
                    content_type: e.content_type,
                    created_at: e.created_at,
                };
            } else {
                store.push(IndexEntry {
                    cid: e.cid.clone(),
                    embedding: e.embedding,
                    meta: SearchIndexMeta {
                        cid: e.cid,
                        tags: e.tags,
                        snippet: e.snippet,
                        content_type: e.content_type,
                        created_at: e.created_at,
                    },
                });
            }
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

        if let Some(existing) = entries.iter_mut().find(|e| e.cid == cid) {
            let is_stub = embedding.iter().all(|&v| v == 0.0);
            if !is_stub {
                existing.embedding = embedding.to_vec();
                existing.meta = meta;
            }
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

        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(k);
        scored
    }

    fn len(&self) -> usize {
        self.entries.read().unwrap().len()
    }

    fn list_by_filter(&self, filter: &SearchFilter) -> Vec<String> {
        self.entries
            .read()
            .unwrap()
            .iter()
            .filter(|e| filter.matches(&e.meta))
            .map(|e| e.cid.clone())
            .collect()
    }

    fn persist_to(&self, dir: &std::path::Path) -> Result<(), String> {
        let entries = self.snapshot();
        if entries.is_empty() {
            return Ok(());
        }
        let mut lines = Vec::new();
        for entry in &entries {
            if let Ok(json) = serde_json::to_string(entry) {
                lines.push(json);
            }
        }
        let path = dir.join("search_index.jsonl");
        std::fs::write(&path, lines.join("\n"))
            .map_err(|e| format!("Failed to persist search index: {e}"))?;
        tracing::info!("Persisted {} search index entries", entries.len());
        Ok(())
    }

    fn restore_from(&self, dir: &std::path::Path) -> Result<(), String> {
        let path = dir.join("search_index.jsonl");
        if !path.exists() {
            return Ok(());
        }
        match std::fs::read_to_string(&path) {
            Ok(data) => {
                let entries: Vec<SearchIndexEntry> = data.lines()
                    .filter(|line| !line.trim().is_empty())
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .collect();
                let count = entries.len();
                self.restore(entries);
                if count > 0 {
                    tracing::info!("Restored {} search index entries", count);
                }
                Ok(())
            }
            Err(e) => Err(format!("Failed to read search index: {e}")),
        }
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
        let v = vec![1.0, 0.0, 0.0];
        assert!((InMemoryBackend::cosine(&v, &v) - 1.0).abs() < 1e-6);

        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!(InMemoryBackend::cosine(&a, &b).abs() < 1e-6);

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
                created_at: 0,
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
                created_at: 0,
            },
        );

        assert_eq!(backend.len(), 2);

        let results = backend.search(&sample_embedding(dim, 1.0), 2, &SearchFilter::default());
        assert!(!results.is_empty());
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
            created_at: 0,
        });
        backend.upsert("cid2", &sample_embedding(dim, 2.0), SearchIndexMeta {
            cid: "cid2".to_string(),
            tags: vec!["python".to_string()],
            snippet: "".to_string(),
            content_type: "text".to_string(),
            created_at: 0,
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
            created_at: 0,
        });

        assert_eq!(backend.len(), 1);
        backend.delete("cid1");
        assert!(backend.is_empty());
    }

    #[test]
    fn test_upsert_replaces_existing() {
        let backend = InMemoryBackend::new();

        backend.upsert("cid1", &vec![1.0, 0.0, 0.0, 0.0], SearchIndexMeta {
            cid: "cid1".to_string(),
            tags: vec!["old".to_string()],
            snippet: "old".to_string(),
            content_type: "text".to_string(),
            created_at: 0,
        });
        backend.upsert("cid1", &vec![0.0, 1.0, 0.0, 0.0], SearchIndexMeta {
            cid: "cid1".to_string(),
            tags: vec!["new".to_string()],
            snippet: "new".to_string(),
            content_type: "text".to_string(),
            created_at: 0,
        });

        assert_eq!(backend.len(), 1);
        let filter = SearchFilter::default();
        let results = backend.search(&vec![0.0, 1.0, 0.0, 0.0], 1, &filter);
        assert_eq!(results[0].cid, "cid1");
        assert!(results[0].meta.tags.contains(&"new".to_string()));
    }
}
