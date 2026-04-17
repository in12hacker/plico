//! Stub embedding provider — returns errors, triggers tag-based fallback.

use crate::fs::embedding::types::{EmbedError, Embedding, EmbeddingProvider};

/// A stub embedding provider used when no backend is available.
/// Always returns an error, triggering tag-based fallback in search.
#[derive(Default)]
pub struct StubEmbeddingProvider;

impl StubEmbeddingProvider {
    pub fn new() -> Self {
        Self
    }
}

impl EmbeddingProvider for StubEmbeddingProvider {
    fn embed(&self, _text: &str) -> Result<Embedding, EmbedError> {
        Err(EmbedError::ServerUnavailable(
            "No embedding backend available".to_string(),
        ))
    }

    fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Embedding>, EmbedError> {
        Err(EmbedError::ServerUnavailable(
            "No embedding backend available".to_string(),
        ))
    }

    fn dimension(&self) -> usize {
        384
    }

    fn model_name(&self) -> &str {
        "stub"
    }
}
