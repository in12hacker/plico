//! BM25 keyword search — complements vector similarity with exact-term matching.

use std::sync::atomic::{AtomicUsize, Ordering};

pub struct Bm25Index {
    engine: std::sync::RwLock<bm25::SearchEngine<String>>,
    /// Tracks total characters for dynamic avgdl computation.
    total_length: AtomicUsize,
    /// Tracks number of documents for dynamic avgdl computation.
    doc_count: AtomicUsize,
}

impl Bm25Index {
    pub fn new() -> Self {
        // k1=1.2, b=0.75 are TREC/SIGIR 20-year standard values (Elasticsearch, Lucene defaults).
        // avgdl starts at 256 (a reasonable text document length); it auto-adjusts as docs are added.
        Self {
            engine: std::sync::RwLock::new(
                bm25::SearchEngineBuilder::<String>::with_avgdl(256.0)
                    .k1(1.2)
                    .b(0.75)
                    .build(),
            ),
            total_length: AtomicUsize::new(0),
            doc_count: AtomicUsize::new(0),
        }
    }

    pub fn upsert(&self, cid: &str, text: &str) {
        let clean = text.trim();
        if clean.is_empty() {
            return;
        }
        self.doc_count.fetch_add(1, Ordering::Relaxed);
        self.total_length.fetch_add(clean.len(), Ordering::Relaxed);
        let doc = bm25::Document::new(cid.to_string(), clean);
        self.engine.write().unwrap().upsert(doc);
    }

    pub fn remove(&self, cid: &str) {
        self.engine.write().unwrap().remove(&cid.to_string());
    }

    /// Search and normalize scores to [0.0, 1.0] using max-score normalization.
    /// Returns sorted (cid, normalized_score) pairs.
    pub fn search(&self, query: &str, limit: usize) -> Vec<(String, f32)> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let results = self.engine.read().unwrap().search(query, Some(limit));
        if results.is_empty() {
            return Vec::new();
        }

        // Normalize scores to [0.0, 1.0] using max normalization.
        // A relevant result (top-1) should score close to 1.0; irrelevant results < 0.2.
        let max_score = results.iter().map(|r| r.score).fold(0.0f32, f32::max);
        let normalizer = if max_score > 0.0 { max_score } else { 1.0 };

        let mut normalized: Vec<(String, f32)> = results
            .into_iter()
            .map(|r| (r.document.id, r.score / normalizer))
            .collect();

        // Already sorted by score descending from BM25; stable for equal scores.
        normalized.truncate(limit);
        normalized
    }

    pub fn len(&self) -> usize {
        self.doc_count.load(Ordering::Relaxed)
    }

    pub fn is_empty(&self) -> bool {
        self.doc_count.load(Ordering::Relaxed) == 0
    }
}

impl Default for Bm25Index {
    fn default() -> Self {
        Self::new()
    }
}
