//! Stub embedding provider — returns errors, triggers tag-based fallback.

use crate::fs::embedding::types::{
    EmbedError, EmbedResult, EmbeddingBuilderIdentity, EmbeddingIdentityError, EmbeddingProvider,
};

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
    fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
        Err(EmbedError::ServerUnavailable(
            "No embedding backend available".to_string(),
        ))
    }

    fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        Err(EmbedError::ServerUnavailable(
            "No embedding backend available".to_string(),
        ))
    }

    fn dimension(&self) -> usize {
        384
    }

    fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
        Err(EmbeddingIdentityError::StubProvider)
    }

    fn model_name(&self) -> String {
        "stub".to_string()
    }
}
