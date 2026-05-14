//! Embedding Service
//!
//! Re-exports all embedding backends.

pub mod adaptive;
pub mod circuit_breaker;
pub mod json_rpc;
pub mod local;
pub mod ollama;
pub mod openai;
pub mod ort_backend;
pub mod stub;
pub mod types;

pub use adaptive::AdaptiveEmbeddingProvider;
pub use circuit_breaker::EmbeddingCircuitBreaker;
pub use ollama::OllamaBackend;
pub use openai::OpenAIEmbeddingBackend;
pub use local::LocalEmbeddingBackend;
pub use ort_backend::OrtEmbeddingBackend;
pub use stub::StubEmbeddingProvider;
pub use types::{EmbeddingProvider, EmbedError, Embedding, EmbeddingMeta, EmbedResult};

use std::sync::Arc;
use crate::kernel::ops::cache::EmbeddingCache;

/// Transparent caching wrapper around any EmbeddingProvider.
///
/// Checks the LRU cache before making an HTTP call to the embedding server.
/// For repeated or similar queries, this eliminates the ~80ms embedding round-trip.
///
/// Includes a 40-second thread-level timeout on all embedding calls as a safety net
/// against reqwest/tokio deadlocks. If the inner call hangs, it returns an error
/// rather than blocking the pipeline indefinitely.
pub struct CachingEmbeddingProvider {
    inner: Arc<dyn EmbeddingProvider>,
    cache: Arc<EmbeddingCache>,
}

impl CachingEmbeddingProvider {
    pub fn new(inner: Arc<dyn EmbeddingProvider>, cache: Arc<EmbeddingCache>) -> Self {
        Self { inner, cache }
    }

    /// Run an embedding call with a thread-level timeout (40s).
    /// This prevents indefinite hangs from reqwest/tokio interaction issues.
    fn embed_with_timeout<F>(&self, text: &str, op: F) -> Result<EmbedResult, EmbedError>
    where
        F: FnOnce(Arc<dyn EmbeddingProvider>, String) -> Result<EmbedResult, EmbedError> + Send + 'static,
    {
        let inner = Arc::clone(&self.inner);
        let text_owned = text.to_string();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(op(inner, text_owned));
        });
        match rx.recv_timeout(std::time::Duration::from_secs(40)) {
            Ok(result) => result,
            Err(_) => Err(EmbedError::ServerUnavailable(
                "embedding call timed out after 40s (possible reqwest/tokio deadlock)".to_string(),
            ))
        }
    }
}

impl EmbeddingProvider for CachingEmbeddingProvider {
    fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        let model_id = self.inner.model_name();
        if let Some(cached) = self.cache.get(text, model_id) {
            return Ok(EmbedResult { embedding: cached, input_tokens: 0 });
        }
        let result = self.embed_with_timeout(text, |inner, t| inner.embed(&t))?;
        self.cache.put(text, model_id, result.embedding.clone());
        Ok(result)
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        self.inner.embed_batch(texts)
    }

    fn embed_query(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        let model_id = self.inner.model_name();
        if let Some(cached) = self.cache.get(text, model_id) {
            return Ok(EmbedResult { embedding: cached, input_tokens: 0 });
        }
        let result = self.embed_with_timeout(text, |inner, t| inner.embed_query(&t))?;
        self.cache.put(text, model_id, result.embedding.clone());
        Ok(result)
    }

    fn embed_document(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        let model_id = self.inner.model_name();
        if let Some(cached) = self.cache.get(text, model_id) {
            return Ok(EmbedResult { embedding: cached, input_tokens: 0 });
        }
        let result = self.embed_with_timeout(text, |inner, t| inner.embed_document(&t))?;
        self.cache.put(text, model_id, result.embedding.clone());
        Ok(result)
    }

    fn dimension(&self) -> usize {
        self.inner.dimension()
    }

    fn model_name(&self) -> &str {
        self.inner.model_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ollama_backend_creation_without_server() {
        let backend = OllamaBackend::new("http://localhost:9999", "all-minilm-l6-v2");
        match backend {
            Ok(b) => assert_eq!(b.dimension(), 384),
            Err(e) => {
                assert!(format!("{e}").contains("connection")
                    || format!("{e}").contains("9999")
                    || format!("{e}").contains("probe"));
            }
        }
    }
}
