//! Adaptive embedding wrapper — applies task prefixes and Matryoshka truncation.
//!
//! Wraps any `EmbeddingProvider` to add:
//! - Configurable query/document prefixes for asymmetric retrieval models
//! - Optional Matryoshka dimension truncation with L2 normalization
//!
//! Configuration (env vars):
//! - `EMBEDDING_QUERY_PREFIX`    — prepended to search queries (e.g. `"Query: "`)
//! - `EMBEDDING_DOCUMENT_PREFIX` — prepended to stored documents (e.g. `"Document: "`)
//! - `EMBEDDING_DIM`             — target dimension for Matryoshka truncation (omit to use native)

use std::sync::Arc;
use crate::fs::embedding::types::{EmbedError, EmbeddingProvider, EmbedResult};

pub struct AdaptiveEmbeddingProvider {
    inner: Arc<dyn EmbeddingProvider>,
    query_prefix: String,
    document_prefix: String,
    /// If set, truncate embeddings to this dimension and L2-normalize.
    target_dim: Option<usize>,
}

impl AdaptiveEmbeddingProvider {
    pub fn new(
        inner: Arc<dyn EmbeddingProvider>,
        query_prefix: String,
        document_prefix: String,
        target_dim: Option<usize>,
    ) -> Self {
        if let Some(td) = target_dim {
            let raw = inner.dimension();
            if td > raw {
                tracing::warn!(
                    "EMBEDDING_DIM={} exceeds model native dimension {}; using native",
                    td, raw,
                );
            }
        }
        Self { inner, query_prefix, document_prefix, target_dim }
    }

    /// Build from environment variables, wrapping a base provider.
    ///
    /// Auto-detects known model families and sets optimal prefixes:
    /// - Qwen3-Embedding: `"Instruct: ...\nQuery: "` for queries, no document prefix
    pub fn from_env(inner: Arc<dyn EmbeddingProvider>) -> Self {
        let mut query_prefix = std::env::var("EMBEDDING_QUERY_PREFIX").unwrap_or_default();
        let mut document_prefix = std::env::var("EMBEDDING_DOCUMENT_PREFIX").unwrap_or_default();
        let target_dim = std::env::var("EMBEDDING_DIM")
            .ok()
            .and_then(|s| s.parse::<usize>().ok());

        // Auto-detect model-specific prefixes when not explicitly set
        if query_prefix.is_empty() {
            let model = inner.model_name().to_lowercase();
            if model.contains("qwen3") && model.contains("embed") {
                query_prefix = "Instruct: Given a web search query, retrieve relevant passages that answer the query\nQuery: ".to_string();
                tracing::info!("Auto-detected Qwen3-Embedding, setting instruction-aware query prefix");
            }
        }

        // Support escaped newlines in env var values
        if query_prefix.contains("\\n") {
            query_prefix = query_prefix.replace("\\n", "\n");
        }
        if document_prefix.contains("\\n") {
            document_prefix = document_prefix.replace("\\n", "\n");
        }

        let has_config = !query_prefix.is_empty()
            || !document_prefix.is_empty()
            || target_dim.is_some();

        if has_config {
            tracing::info!(
                "Adaptive embedding: query_prefix={:?}, doc_prefix={:?}, target_dim={:?}",
                if query_prefix.is_empty() { "(none)" } else { &query_prefix },
                if document_prefix.is_empty() { "(none)" } else { &document_prefix },
                target_dim,
            );
        }

        Self::new(inner, query_prefix, document_prefix, target_dim)
    }

    /// Whether this wrapper is a no-op passthrough (no prefix, no truncation).
    pub fn is_passthrough(&self) -> bool {
        self.query_prefix.is_empty()
            && self.document_prefix.is_empty()
            && self.target_dim.is_none()
    }

    fn effective_dim(&self) -> usize {
        self.target_dim
            .map(|td| td.min(self.inner.dimension()))
            .unwrap_or_else(|| self.inner.dimension())
    }

    fn postprocess(&self, mut result: EmbedResult) -> EmbedResult {
        if let Some(td) = self.target_dim {
            let td = td.min(result.embedding.len());
            if td < result.embedding.len() {
                result.embedding.truncate(td);
                l2_normalize(&mut result.embedding);
            }
        }
        result
    }

    fn postprocess_batch(&self, results: Vec<EmbedResult>) -> Vec<EmbedResult> {
        if self.target_dim.is_some() {
            results.into_iter().map(|r| self.postprocess(r)).collect()
        } else {
            results
        }
    }

    fn prefixed(&self, prefix: &str, text: &str) -> String {
        if prefix.is_empty() {
            text.to_string()
        } else {
            format!("{prefix}{text}")
        }
    }
}

fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 1e-10 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

impl EmbeddingProvider for AdaptiveEmbeddingProvider {
    fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        let result = self.inner.embed(text)?;
        Ok(self.postprocess(result))
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        let results = self.inner.embed_batch(texts)?;
        Ok(self.postprocess_batch(results))
    }

    fn embed_query(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        let prefixed = self.prefixed(&self.query_prefix, text);
        let result = self.inner.embed(&prefixed)?;
        Ok(self.postprocess(result))
    }

    fn embed_document(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        let prefixed = self.prefixed(&self.document_prefix, text);
        let result = self.inner.embed(&prefixed)?;
        Ok(self.postprocess(result))
    }

    fn dimension(&self) -> usize {
        self.effective_dim()
    }

    fn raw_dimension(&self) -> usize {
        self.inner.dimension()
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeProvider;

    impl EmbeddingProvider for FakeProvider {
        fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
            let dim = 8;
            let embedding: Vec<f32> = (0..dim).map(|i| (i as f32 + 1.0) * if text.starts_with("Query:") { 2.0 } else { 1.0 }).collect();
            Ok(EmbedResult::new(embedding, text.len() as u32 / 4))
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
            texts.iter().map(|t| self.embed(t)).collect()
        }

        fn dimension(&self) -> usize { 8 }
        fn model_name(&self) -> &str { "fake" }
    }

    #[test]
    fn test_passthrough_no_config() {
        let inner = Arc::new(FakeProvider);
        let adaptive = AdaptiveEmbeddingProvider::new(
            inner, String::new(), String::new(), None,
        );
        assert!(adaptive.is_passthrough());
        assert_eq!(adaptive.dimension(), 8);

        let result = adaptive.embed("hello").unwrap();
        assert_eq!(result.embedding.len(), 8);
    }

    #[test]
    fn test_matryoshka_truncation() {
        let inner = Arc::new(FakeProvider);
        let adaptive = AdaptiveEmbeddingProvider::new(
            inner, String::new(), String::new(), Some(4),
        );
        assert!(!adaptive.is_passthrough());
        assert_eq!(adaptive.dimension(), 4);
        assert_eq!(adaptive.raw_dimension(), 8);

        let result = adaptive.embed("hello").unwrap();
        assert_eq!(result.embedding.len(), 4);

        let norm: f32 = result.embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "should be L2-normalized, got norm={norm}");
    }

    #[test]
    fn test_query_document_prefixes() {
        let inner = Arc::new(FakeProvider);
        let adaptive = AdaptiveEmbeddingProvider::new(
            inner, "Query: ".to_string(), "Document: ".to_string(), None,
        );

        let q_result = adaptive.embed_query("hello").unwrap();
        let d_result = adaptive.embed_document("hello").unwrap();

        // FakeProvider multiplies by 2.0 when text starts with "Query:"
        assert!(q_result.embedding[0] > d_result.embedding[0],
            "query embedding should differ from document embedding");
    }

    #[test]
    fn test_target_dim_exceeds_native_clamped() {
        let inner = Arc::new(FakeProvider);
        let adaptive = AdaptiveEmbeddingProvider::new(
            inner, String::new(), String::new(), Some(100),
        );
        assert_eq!(adaptive.dimension(), 8, "should clamp to native dim");
    }
}
