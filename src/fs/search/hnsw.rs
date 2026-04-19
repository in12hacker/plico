//! HNSW ANN backend — persistent, production-scale vector search.
//!
//! Uses `edgevec` for approximate nearest neighbor search with binary quantization.
//! Two-phase search: Binary quantization (fast coarse search) + F32 rescoring (accurate re-ranking).
//!
//! Memory efficiency:
//! - F32: 768D × 4 bytes = 3KB per vector
//! - BQ:  768D / 8 = 96 bytes per vector (32x compression)
//!
//! Search strategy:
//! - Phase 1: BQ search with Hamming distance, recall top-N candidates
//! - Phase 2: Rescore top candidates with original F32 vectors for accurate ranking
//! - Result: ~95% recall at ~3x speedup vs full F32 search

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

use edgevec::hnsw::HnswIndex;
use edgevec::storage::VectorStorage;
use edgevec::HnswConfig;

use super::{SearchFilter, SearchHit, SearchIndexEntry, SearchIndexMeta, SemanticSearch};

const DEFAULT_DIM: usize = 768;
const BQ_RESCORE_FACTOR: usize = 5; // Overfetch multiplier for two-phase search

pub struct HnswBackend {
    config: HnswConfig,
    /// Inner HNSW index with BQ support
    index: RwLock<HnswIndex>,
    /// Vector storage for original f32 vectors (kept for rescoring)
    storage: RwLock<VectorStorage>,
    /// Entries mapping CID to vector metadata
    entries: RwLock<HashMap<String, HnswEntry>>,
    /// Next available vector ID (atomic for thread safety)
    next_id: AtomicU64,
}

struct HnswEntry {
    vector_id: edgevec::hnsw::VectorId,
    embedding: Vec<f32>,
    meta: SearchIndexMeta,
}

impl HnswBackend {
    pub fn new() -> Self {
        Self::with_dim(DEFAULT_DIM)
    }

    pub fn with_dim(dim: usize) -> Self {
        let config = HnswConfig::new(dim as u32);
        let storage = VectorStorage::new(&config, None);
        let index = HnswIndex::with_bq(config.clone(), &storage)
            .expect("Failed to create HNSW index with BQ");

        Self {
            config,
            index: RwLock::new(index),
            storage: RwLock::new(storage),
            entries: RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(0),
        }
    }

    /// Returns the dimension of vectors in this index.
    pub fn dim(&self) -> usize {
        self.config.dimensions as usize
    }

    /// Compute cosine similarity between two vectors
    fn cosine_similarity(&self, a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() {
            return 0.0;
        }
        let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm_a == 0.0 || norm_b == 0.0 {
            0.0
        } else {
            dot / (norm_a * norm_b)
        }
    }

    /// Estimate memory usage for the index
    pub fn memory_usage(&self) -> IndexMemoryUsage {
        let entries = self.entries.read().unwrap();
        let vector_count = entries.len();
        let dim = self.dim();

        // F32 storage (original vectors kept for rescoring)
        let f32_bytes = vector_count * dim * 4;

        // BQ storage (binary quantized vectors in HNSW)
        let bq_bytes = vector_count * (dim / 8);

        // HNSW graph overhead (approximate)
        let graph_overhead = vector_count * 64; // ~64 bytes per node for connections

        let total_bytes = f32_bytes + bq_bytes + graph_overhead;

        IndexMemoryUsage {
            vector_count,
            dimension: dim,
            f32_bytes,
            bq_bytes,
            graph_overhead_bytes: graph_overhead,
            total_bytes,
            compression_ratio: if bq_bytes > 0 {
                f32_bytes as f32 / bq_bytes as f32
            } else {
                1.0
            },
        }
    }

    /// Fallback search when BQ fails - iterate through entries and compute similarity
    fn fallback_search(&self, query: &[f32], k: usize, filter: &SearchFilter) -> Vec<SearchHit> {
        let entries = self.entries.read().unwrap();
        let mut results: Vec<SearchHit> = entries
            .iter()
            .filter(|(_, entry)| filter.matches(&entry.meta))
            .map(|(cid, entry)| {
                let similarity = self.cosine_similarity(query, &entry.embedding);
                SearchHit {
                    cid: cid.clone(),
                    score: similarity,
                    meta: entry.meta.clone(),
                }
            })
            .collect();

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k);
        results
    }
}

impl Default for HnswBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SemanticSearch for HnswBackend {
    fn upsert(&self, cid: &str, embedding: &[f32], meta: SearchIndexMeta) {
        // Validate dimension
        let dim = self.dim();
        if embedding.len() != dim {
            tracing::warn!(
                "Embedding dimension mismatch: expected {}, got {}",
                dim,
                embedding.len()
            );
            return;
        }

        // Check for stub vector (all zeros)
        let is_stub = embedding.iter().all(|&v| v == 0.0);
        if is_stub {
            return;
        }

        let _vector_id = {
            let mut entries = self.entries.write().unwrap();
            if let Some(entry) = entries.get_mut(cid) {
                // Update existing entry
                entry.embedding = embedding.to_vec();
                entry.meta = meta;
                entry.vector_id
            } else {
                // Insert new entry - start from 1 since 0 is INVALID in edgevec
                let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
                let vid = edgevec::hnsw::VectorId(id);
                entries.insert(cid.to_string(), HnswEntry {
                    vector_id: vid,
                    embedding: embedding.to_vec(),
                    meta,
                });
                vid
            }
        };

        // Insert into edgevec index with BQ
        let mut index = self.index.write().unwrap();
        let mut storage = self.storage.write().unwrap();
        if let Err(e) = index.insert_bq(embedding, &mut storage) {
            tracing::error!("Failed to insert vector into BQ index: {}", e);
        }
    }

    fn delete(&self, cid: &str) {
        let mut entries = self.entries.write().unwrap();
        if let Some(entry) = entries.remove(cid) {
            let mut storage = self.storage.write().unwrap();
            storage.mark_deleted(entry.vector_id);
            // Note: HNSW doesn't support node deletion easily, but soft-delete in storage is enough
            tracing::debug!("Soft-deleted vector {} (HNSW node remains)", cid);
        }
    }

    fn search(&self, query: &[f32], k: usize, filter: &SearchFilter) -> Vec<SearchHit> {
        let dim = self.dim();
        if query.len() != dim {
            tracing::warn!("Query dimension mismatch: expected {}, got {}", dim, query.len());
            return Vec::new();
        }

        if self.entries.read().unwrap().is_empty() {
            return Vec::new();
        }

        // Use two-phase search: BQ + rescored
        let storage = self.storage.read().unwrap();
        let index = self.index.read().unwrap();

        // search_bq_rescored does both phases internally
        match index.search_bq_rescored(query, k * BQ_RESCORE_FACTOR, BQ_RESCORE_FACTOR, &storage) {
            Ok(results) => {
                // Collect all candidates with their cosine similarity
                let entries = self.entries.read().unwrap();
                let mut search_results: Vec<SearchHit> = Vec::new();

                for (vid, _bq_similarity) in results {
                    // Find the CID for this vector_id
                    if let Some((cid, entry)) = entries.iter().find(|(_, e)| e.vector_id == vid) {
                        if !filter.matches(&entry.meta) {
                            continue;
                        }
                        // Calculate cosine similarity from original vectors for accurate scoring
                        let similarity = self.cosine_similarity(query, &entry.embedding);
                        search_results.push(SearchHit {
                            cid: cid.clone(),
                            score: similarity,
                            meta: entry.meta.clone(),
                        });
                    }
                }

                // Sort by cosine similarity (descending)
                search_results.sort_by(|a, b| {
                    b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal)
                });
                search_results.truncate(k);

                search_results
            }
            Err(e) => {
                tracing::warn!("BQ search failed, falling back to iteration: {}", e);
                // Fallback: iterate and compute similarity directly
                drop(storage);
                drop(index);
                self.fallback_search(query, k, filter)
            }
        }
    }

    fn len(&self) -> usize {
        self.entries.read().unwrap().len()
    }

    fn list_by_filter(&self, filter: &SearchFilter) -> Vec<String> {
        self.entries
            .read()
            .unwrap()
            .iter()
            .filter(|(_, e)| filter.matches(&e.meta))
            .map(|(cid, _)| cid.clone())
            .collect()
    }

    fn persist_to(&self, dir: &Path) -> Result<(), String> {
        if self.entries.read().unwrap().is_empty() {
            return Ok(());
        }

        let lines: Vec<String> = self
            .entries
            .read()
            .unwrap()
            .iter()
            .filter_map(|(cid, e)| {
                serde_json::to_string(&SearchIndexEntry {
                    cid: cid.clone(),
                    embedding: e.embedding.clone(),
                    tags: e.meta.tags.clone(),
                    snippet: e.meta.snippet.clone(),
                    content_type: e.meta.content_type.clone(),
                    created_at: e.meta.created_at,
                })
                .ok()
            })
            .collect();

        let path = dir.join("hnsw_index.jsonl");
        let tmp = path.with_extension("jsonl.tmp");
        std::fs::write(&tmp, lines.join("\n"))
            .map_err(|e| format!("Failed to persist HNSW index: {e}"))?;
        std::fs::rename(&tmp, &path)
            .map_err(|e| format!("Failed to rename HNSW index: {e}"))?;
        tracing::info!("Persisted {} HNSW index entries", lines.len());
        Ok(())
    }

    fn restore_from(&self, dir: &Path) -> Result<(), String> {
        let path = dir.join("hnsw_index.jsonl");
        if !path.exists() {
            return Ok(());
        }

        let data = std::fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read HNSW index: {e}"))?;

        let loaded: Vec<SearchIndexEntry> = data
            .lines()
            .filter(|line| !line.trim().is_empty())
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();

        if loaded.is_empty() {
            return Ok(());
        }

        // Validate dimension from first entry
        let dim = loaded[0].embedding.len();
        if dim != self.dim() {
            return Err(format!(
                "Dimension mismatch: index has {}, persisted has {}",
                self.dim(),
                dim
            ));
        }

        let count = loaded.len();

        for e in loaded {
            // Start from 1 since 0 is INVALID in edgevec
            let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
            let vector_id = edgevec::hnsw::VectorId(id);

            // Insert into edgevec index
            let mut index = self.index.write().unwrap();
            let mut storage = self.storage.write().unwrap();
            if let Err(err) = index.insert_bq(&e.embedding, &mut storage) {
                tracing::error!("Failed to restore vector {}: {}", e.cid, err);
                continue;
            }

            self.entries.write().unwrap().insert(e.cid.clone(), HnswEntry {
                vector_id,
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

        tracing::info!("Restored {} HNSW index entries", count);
        Ok(())
    }
}

/// Memory usage statistics for the index
#[derive(Debug, Clone)]
pub struct IndexMemoryUsage {
    pub vector_count: usize,
    pub dimension: usize,
    pub f32_bytes: usize,
    pub bq_bytes: usize,
    pub graph_overhead_bytes: usize,
    pub total_bytes: usize,
    pub compression_ratio: f32,
}

impl std::fmt::Display for IndexMemoryUsage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "IndexMemoryUsage: {} vectors @ {}D\n  F32 storage: {:.2} KB\n  BQ storage: {:.2} KB\n  Graph overhead: {:.2} KB\n  Total: {:.2} KB\n  Compression: {:.1}x",
            self.vector_count,
            self.dimension,
            self.f32_bytes as f32 / 1024.0,
            self.bq_bytes as f32 / 1024.0,
            self.graph_overhead_bytes as f32 / 1024.0,
            self.total_bytes as f32 / 1024.0,
            self.compression_ratio,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_embedding(dim: usize, seed: f32) -> Vec<f32> {
        (0..dim).map(|i| (seed * (i + 1) as f32).sin().abs()).collect()
    }

    fn make_meta(cid: &str, tags: Vec<&str>) -> SearchIndexMeta {
        SearchIndexMeta {
            cid: cid.to_string(),
            tags: tags.into_iter().map(|s| s.to_string()).collect(),
            snippet: String::new(),
            content_type: "text".to_string(),
            created_at: 0,
        }
    }

    #[test]
    fn test_hnsw_upsert_and_search() {
        let backend = HnswBackend::with_dim(32);
        let dim = backend.dim();
        assert_eq!(dim, 32);

        backend.upsert("cid1", &sample_embedding(dim, 1.0), make_meta("cid1", vec!["rust"]));
        backend.upsert("cid2", &sample_embedding(dim, 2.0), make_meta("cid2", vec!["python"]));
        backend.upsert("cid3", &sample_embedding(dim, 3.0), make_meta("cid3", vec!["go"]));

        assert_eq!(backend.len(), 3);

        let results = backend.search(&sample_embedding(dim, 1.0), 2, &SearchFilter::default());
        assert!(!results.is_empty());
        assert_eq!(results[0].cid, "cid1");
        assert!(results[0].score > 0.9);
    }

    #[test]
    fn test_hnsw_search_with_filter() {
        let backend = HnswBackend::with_dim(32);
        let dim = backend.dim();

        backend.upsert("cid1", &sample_embedding(dim, 1.0), make_meta("cid1", vec!["rust"]));
        backend.upsert("cid2", &sample_embedding(dim, 2.0), make_meta("cid2", vec!["python"]));

        let filter = SearchFilter {
            require_tags: vec!["rust".to_string()],
            ..Default::default()
        };
        let results = backend.search(&sample_embedding(dim, 1.0), 10, &filter);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].cid, "cid1");
    }

    #[test]
    fn test_hnsw_persistence_roundtrip() {
        let dir = tempfile::TempDir::new().unwrap();
        let dim = 32;

        {
            let backend = HnswBackend::with_dim(dim);
            backend.upsert("cid1", &sample_embedding(dim, 1.0), make_meta("cid1", vec!["rust"]));
            backend.upsert("cid2", &sample_embedding(dim, 2.0), make_meta("cid2", vec!["python"]));
            backend.persist_to(dir.path()).unwrap();
        }

        {
            let backend = HnswBackend::with_dim(dim);
            backend.restore_from(dir.path()).unwrap();
            assert_eq!(backend.len(), 2);

            let results = backend.search(&sample_embedding(dim, 1.0), 2, &SearchFilter::default());
            assert!(!results.is_empty());
            assert_eq!(results[0].cid, "cid1");
        }
    }

    #[test]
    fn test_hnsw_delete() {
        let backend = HnswBackend::with_dim(32);
        let dim = backend.dim();

        backend.upsert("cid1", &sample_embedding(dim, 1.0), make_meta("cid1", vec![]));
        assert_eq!(backend.len(), 1);

        backend.delete("cid1");
        assert_eq!(backend.len(), 0);
        assert!(backend.is_empty());

        let results = backend.search(&sample_embedding(dim, 1.0), 1, &SearchFilter::default());
        assert!(results.is_empty());
    }

    #[test]
    fn test_hnsw_empty_search() {
        let backend = HnswBackend::with_dim(32);
        let results = backend.search(&vec![1.0; 32], 5, &SearchFilter::default());
        assert!(results.is_empty());
    }

    #[test]
    fn test_hnsw_cosine_ranking() {
        let backend = HnswBackend::with_dim(32);
        let dim = backend.dim();

        let query = sample_embedding(dim, 1.0);
        backend.upsert("cid1", &query, make_meta("cid1", vec![]));
        backend.upsert("cid2", &sample_embedding(dim, 5.0), make_meta("cid2", vec![]));

        let results = backend.search(&query, 2, &SearchFilter::default());
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].cid, "cid1");
        assert!(results[0].score > results[1].score);
    }

    #[test]
    fn test_hnsw_list_by_filter() {
        let backend = HnswBackend::with_dim(32);
        let dim = backend.dim();

        backend.upsert("cid1", &sample_embedding(dim, 1.0), make_meta("cid1", vec!["rust"]));
        backend.upsert("cid2", &sample_embedding(dim, 2.0), make_meta("cid2", vec!["python"]));

        let filter = SearchFilter {
            require_tags: vec!["python".to_string()],
            ..Default::default()
        };
        let cids = backend.list_by_filter(&filter);
        assert_eq!(cids.len(), 1);
        assert!(cids.contains(&"cid2".to_string()));
    }

    #[test]
    fn test_memory_usage() {
        let backend = HnswBackend::with_dim(768);
        let dim = backend.dim();

        // Insert a few vectors
        backend.upsert("cid1", &sample_embedding(dim, 1.0), make_meta("cid1", vec![]));
        backend.upsert("cid2", &sample_embedding(dim, 2.0), make_meta("cid2", vec![]));

        let usage = backend.memory_usage();
        assert_eq!(usage.vector_count, 2);
        assert_eq!(usage.dimension, 768);

        // F32: 2 * 768 * 4 = 6144 bytes
        assert_eq!(usage.f32_bytes, 6144);

        // BQ: 2 * 768 / 8 = 192 bytes
        assert_eq!(usage.bq_bytes, 192);

        // Compression ratio should be 32x
        assert!((usage.compression_ratio - 32.0).abs() < 0.01);
    }

    #[test]
    fn test_dimension_mismatch_handling() {
        let backend = HnswBackend::with_dim(32);

        // Query with wrong dimension should return empty
        let results = backend.search(&vec![1.0; 64], 5, &SearchFilter::default());
        assert!(results.is_empty());

        // Upsert with wrong dimension should be ignored
        backend.upsert("cid1", &vec![1.0; 64], make_meta("cid1", vec![]));
        assert_eq!(backend.len(), 0);
    }

    #[test]
    fn test_stub_vector_handling() {
        let backend = HnswBackend::with_dim(32);
        let dim = backend.dim();

        // Insert stub vector (all zeros)
        backend.upsert("cid1", &vec![0.0; dim], make_meta("cid1", vec![]));
        assert_eq!(backend.len(), 0); // Should be ignored
    }
}
