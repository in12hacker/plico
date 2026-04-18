//! HNSW ANN backend — persistent, production-scale vector search.
//!
//! Uses `hnsw_rs` for approximate nearest neighbor search with cosine similarity.
//! Metadata + embeddings persisted as JSONL; HNSW graph rebuilt on restore.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::RwLock;

use hnsw_rs::prelude::*;

use super::{SearchFilter, SearchHit, SearchIndexEntry, SearchIndexMeta, SemanticSearch};

const MAX_NB_CONNECTION: usize = 16;
const MAX_ELEMENTS: usize = 100_000;
const MAX_LAYER: usize = 16;
const EF_CONSTRUCTION: usize = 200;
const EF_SEARCH: usize = 64;
const OVERFETCH_FACTOR: usize = 3;

pub struct HnswBackend {
    hnsw: RwLock<Hnsw<'static, f32, DistCosine>>,
    entries: RwLock<HashMap<String, HnswEntry>>,
    id_to_cid: RwLock<HashMap<usize, String>>,
    next_id: AtomicUsize,
}

struct HnswEntry {
    point_id: usize,
    embedding: Vec<f32>,
    meta: SearchIndexMeta,
}

impl HnswBackend {
    pub fn new() -> Self {
        Self {
            hnsw: RwLock::new(Self::create_hnsw(MAX_ELEMENTS)),
            entries: RwLock::new(HashMap::new()),
            id_to_cid: RwLock::new(HashMap::new()),
            next_id: AtomicUsize::new(0),
        }
    }

    fn create_hnsw(max_elements: usize) -> Hnsw<'static, f32, DistCosine> {
        Hnsw::<f32, DistCosine>::new(
            MAX_NB_CONNECTION,
            max_elements,
            MAX_LAYER,
            EF_CONSTRUCTION,
            DistCosine,
        )
    }
}

impl Default for HnswBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SemanticSearch for HnswBackend {
    fn upsert(&self, cid: &str, embedding: &[f32], meta: SearchIndexMeta) {
        let is_stub = embedding.iter().all(|&v| v == 0.0);

        let to_insert = {
            let mut entries = self.entries.write().unwrap();
            let mut id_map = self.id_to_cid.write().unwrap();

            if let Some(entry) = entries.get_mut(cid) {
                if is_stub {
                    return;
                }
                id_map.remove(&entry.point_id);
                let new_id = self.next_id.fetch_add(1, Ordering::Relaxed);
                id_map.insert(new_id, cid.to_string());
                *entry = HnswEntry {
                    point_id: new_id,
                    embedding: embedding.to_vec(),
                    meta,
                };
                Some((embedding.to_vec(), new_id))
            } else {
                let id = self.next_id.fetch_add(1, Ordering::Relaxed);
                id_map.insert(id, cid.to_string());
                entries.insert(cid.to_string(), HnswEntry {
                    point_id: id,
                    embedding: embedding.to_vec(),
                    meta,
                });
                Some((embedding.to_vec(), id))
            }
        };

        if let Some((emb, id)) = to_insert {
            self.hnsw.read().unwrap().insert((&emb, id));
        }
    }

    fn delete(&self, cid: &str) {
        let mut entries = self.entries.write().unwrap();
        if let Some(entry) = entries.remove(cid) {
            self.id_to_cid.write().unwrap().remove(&entry.point_id);
        }
    }

    fn search(&self, query: &[f32], k: usize, filter: &SearchFilter) -> Vec<SearchHit> {
        let entries = self.entries.read().unwrap();
        if entries.is_empty() {
            return Vec::new();
        }

        let fetch_k = k * OVERFETCH_FACTOR;
        let ef = EF_SEARCH.max(fetch_k);
        let neighbours = self.hnsw.read().unwrap().search(query, fetch_k, ef);

        let id_map = self.id_to_cid.read().unwrap();
        let mut results: Vec<SearchHit> = neighbours
            .iter()
            .filter_map(|n| {
                let cid = id_map.get(&n.d_id)?;
                let entry = entries.get(cid)?;
                if !filter.matches(&entry.meta) {
                    return None;
                }
                let similarity = 1.0 - n.distance;
                Some(SearchHit {
                    cid: cid.clone(),
                    score: similarity,
                    meta: entry.meta.clone(),
                })
            })
            .collect();

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(k);
        results
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
        let entries = self.entries.read().unwrap();
        if entries.is_empty() {
            return Ok(());
        }

        let lines: Vec<String> = entries
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
        std::fs::write(&path, lines.join("\n"))
            .map_err(|e| format!("Failed to persist HNSW index: {e}"))?;
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

        let count = loaded.len();
        let new_hnsw = Self::create_hnsw(count.max(MAX_ELEMENTS));

        let mut entries = self.entries.write().unwrap();
        let mut id_map = self.id_to_cid.write().unwrap();
        entries.clear();
        id_map.clear();

        for e in loaded {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            new_hnsw.insert((&e.embedding, id));
            id_map.insert(id, e.cid.clone());
            entries.insert(e.cid.clone(), HnswEntry {
                point_id: id,
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

        drop(entries);
        drop(id_map);
        *self.hnsw.write().unwrap() = new_hnsw;

        tracing::info!("Restored {} HNSW index entries", count);
        Ok(())
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
        let backend = HnswBackend::new();
        let dim = 32;

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
        let backend = HnswBackend::new();
        let dim = 32;

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
            let backend = HnswBackend::new();
            backend.upsert("cid1", &sample_embedding(dim, 1.0), make_meta("cid1", vec!["rust"]));
            backend.upsert("cid2", &sample_embedding(dim, 2.0), make_meta("cid2", vec!["python"]));
            backend.persist_to(dir.path()).unwrap();
        }

        {
            let backend = HnswBackend::new();
            backend.restore_from(dir.path()).unwrap();
            assert_eq!(backend.len(), 2);

            let results = backend.search(&sample_embedding(dim, 1.0), 2, &SearchFilter::default());
            assert!(!results.is_empty());
            assert_eq!(results[0].cid, "cid1");
        }
    }

    #[test]
    fn test_hnsw_delete() {
        let backend = HnswBackend::new();
        let dim = 32;

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
        let backend = HnswBackend::new();
        let results = backend.search(&vec![1.0; 32], 5, &SearchFilter::default());
        assert!(results.is_empty());
    }

    #[test]
    fn test_hnsw_cosine_ranking() {
        let backend = HnswBackend::new();
        let dim = 32;

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
        let backend = HnswBackend::new();
        let dim = 32;

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
}
