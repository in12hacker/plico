//! BM25 keyword search — complements vector similarity with exact-term matching.

pub struct Bm25Index {
    engine: std::sync::RwLock<bm25::SearchEngine<String>>,
}

impl Bm25Index {
    pub fn new() -> Self {
        Self {
            engine: std::sync::RwLock::new(
                bm25::SearchEngineBuilder::<String>::with_avgdl(100.0).build(),
            ),
        }
    }

    pub fn upsert(&self, cid: &str, text: &str) {
        if text.trim().is_empty() {
            return;
        }
        let doc = bm25::Document::new(cid.to_string(), text);
        self.engine.write().unwrap().upsert(doc);
    }

    pub fn remove(&self, cid: &str) {
        self.engine.write().unwrap().remove(&cid.to_string());
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<(String, f32)> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let results = self.engine.read().unwrap().search(query, Some(limit));
        results
            .into_iter()
            .map(|r| (r.document.id, r.score))
            .collect()
    }

    pub fn len(&self) -> usize {
        self.engine.read().unwrap().iter().count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for Bm25Index {
    fn default() -> Self {
        Self::new()
    }
}
