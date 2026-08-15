//! BM25 keyword search — complements vector similarity with exact-term matching.

pub struct Bm25Index {
    engine: std::sync::RwLock<bm25::SearchEngine<String>>,
}

impl Bm25Index {
    pub fn new() -> Self {
        // k1=1.2, b=0.75 are TREC/SIGIR 20-year standard values (Elasticsearch, Lucene defaults).
        // This streaming index has no fixed corpus to fit. The bm25 crate keeps
        // this configured avgdl constant; upserts do not update it dynamically.
        Self {
            engine: std::sync::RwLock::new(
                bm25::SearchEngineBuilder::<String>::with_avgdl(256.0)
                    .k1(1.2)
                    .b(0.75)
                    .build(),
            ),
        }
    }

    pub fn upsert(&self, cid: &str, text: &str) {
        let clean = text.trim();
        if clean.is_empty() {
            return;
        }
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
        self.engine.read().unwrap().iter().count()
    }

    pub fn is_empty(&self) -> bool {
        self.engine.read().unwrap().iter().next().is_none()
    }
}

impl Default for Bm25Index {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upsert_and_search() {
        let idx = Bm25Index::new();
        idx.upsert("doc1", "rust programming language");
        idx.upsert("doc2", "python programming language");
        idx.upsert("doc3", "golang concurrency model");

        let results = idx.search("rust", 5);
        assert!(!results.is_empty());
        assert_eq!(results[0].0, "doc1");
        assert!(results[0].1 > 0.0);
    }

    #[test]
    fn test_search_empty_query() {
        let idx = Bm25Index::new();
        idx.upsert("doc1", "test content");
        assert!(idx.search("", 5).is_empty());
        assert!(idx.search("  ", 5).is_empty());
    }

    #[test]
    fn test_search_no_match() {
        let idx = Bm25Index::new();
        idx.upsert("doc1", "rust programming");
        let results = idx.search("javascript", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_upsert_empty_text_ignored() {
        let idx = Bm25Index::new();
        idx.upsert("doc1", "");
        idx.upsert("doc2", "  ");
        assert!(idx.is_empty());
    }

    #[test]
    fn test_remove() {
        let idx = Bm25Index::new();
        idx.upsert("doc1", "rust programming");
        idx.remove("doc1");
        let results = idx.search("rust", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_score_normalization() {
        let idx = Bm25Index::new();
        idx.upsert("doc1", "the quick brown fox jumps over the lazy dog");
        idx.upsert("doc2", "fox fox fox fox fox");
        let results = idx.search("fox", 5);
        assert!(!results.is_empty());
        assert!(results[0].1 <= 1.0);
        assert!(results[0].1 > 0.0);
    }

    #[test]
    fn test_len() {
        let idx = Bm25Index::new();
        assert_eq!(idx.len(), 0);
        idx.upsert("doc1", "hello world");
        assert_eq!(idx.len(), 1);
        idx.upsert("doc2", "another document");
        assert_eq!(idx.len(), 2);
    }

    #[test]
    fn test_repeated_upsert_replaces_without_inflating_len() {
        let idx = Bm25Index::new();
        idx.upsert("doc1", "old searchable phrase");
        idx.upsert("doc1", "new replacement phrase");

        assert_eq!(idx.len(), 1);
        assert!(idx.search("old", 5).is_empty());
        assert_eq!(idx.search("replacement", 5)[0].0, "doc1");
    }

    #[test]
    fn test_remove_updates_len_and_is_empty() {
        let idx = Bm25Index::new();
        idx.upsert("doc1", "first document");
        idx.upsert("doc2", "second document");

        idx.remove("doc1");
        assert_eq!(idx.len(), 1);
        assert!(!idx.is_empty());

        idx.remove("doc2");
        assert_eq!(idx.len(), 0);
        assert!(idx.is_empty());
    }

    #[test]
    fn test_exact_code_identifier_is_searchable() {
        let idx = Bm25Index::new();
        idx.upsert("code", "SemanticFS calls embed_query before vector search");
        idx.upsert("prose", "unrelated memory lifecycle documentation");

        let results = idx.search("embed_query", 5);
        assert_eq!(results[0].0, "code");
    }

    #[test]
    fn test_benchmark_scenario_lifecycle() {
        // Simulate memory-lifecycle benchmark: create items, search by substring
        let idx = Bm25Index::new();
        for i in 0..20 {
            let content = format!("Lifecycle item {}: memory architecture benchmark test", i);
            idx.upsert(&format!("cid-{}", i), &content);
        }

        for i in 0..20 {
            let query = format!("Lifecycle item {}", i);
            let results = idx.search(&query, 5);
            let expected_cid = format!("cid-{}", i);
            let found = results.iter().any(|(cid, _)| *cid == expected_cid);
            assert!(found, "BM25 should find '{}' for query '{}'", expected_cid, query);
        }
    }

    #[test]
    fn test_benchmark_scenario_checkpoint() {
        // Simulate checkpoint-restore benchmark
        let idx = Bm25Index::new();
        for i in 0..10 {
            let content = format!("Checkpoint-1 item {}: initial knowledge base entry", i);
            idx.upsert(&format!("cp1-{}", i), &content);
        }
        for i in 0..10 {
            let content = format!("Checkpoint-2 item {}: additional knowledge after session", i);
            idx.upsert(&format!("cp2-{}", i), &content);
        }

        for i in 0..10 {
            let query = format!("Checkpoint-1 item {}", i);
            let results = idx.search(&query, 5);
            let expected_cid = format!("cp1-{}", i);
            let found = results.iter().any(|(cid, _)| *cid == expected_cid);
            assert!(found, "BM25 should find '{}' for query '{}'", expected_cid, query);
        }
    }

    #[test]
    fn test_benchmark_scenario_retrieval() {
        // Simulate BEIR retrieval: scientific text with specific terms
        let idx = Bm25Index::new();
        idx.upsert("doc-1", "COVID-19 vaccine efficacy in clinical trials");
        idx.upsert("doc-2", "Machine learning for drug discovery");
        idx.upsert("doc-3", "Quantum computing advances in 2026");

        let results = idx.search("COVID-19 vaccine", 5);
        assert!(!results.is_empty(), "BM25 should find results for 'COVID-19 vaccine'");
        assert_eq!(results[0].0, "doc-1", "Top result should be doc-1");
    }
}
