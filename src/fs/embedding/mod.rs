//! Embedding Service
//!
//! Re-exports all embedding backends.

pub mod adaptive;
pub mod circuit_breaker;
pub mod json_rpc;
pub mod local;
pub mod ollama;
pub mod openai;
pub mod stub;
pub mod types;

pub use adaptive::AdaptiveEmbeddingProvider;
pub use circuit_breaker::EmbeddingCircuitBreaker;
pub use local::LocalEmbeddingBackend;
pub use ollama::OllamaBackend;
pub use openai::OpenAIEmbeddingBackend;
pub use stub::StubEmbeddingProvider;
pub use types::{
    EmbedError, EmbedResult, Embedding, EmbeddingBuilderIdentity, EmbeddingIdentityError, EmbeddingInputContract,
    EmbeddingInputOperation, EmbeddingMeta, EmbeddingNormalization, EmbeddingProvider, EmbeddingProviderFamily,
    VerifiedDocumentProviderSnapshot,
};

use crate::kernel::ops::cache::EmbeddingCache;
use std::sync::Arc;

/// Transparent caching wrapper around any EmbeddingProvider.
///
/// Reuses vectors only when the exact provider identity, operation contract,
/// and input digest all match and the provider identity remains current.
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
            )),
        }
    }

    fn embed_cached<F>(
        &self,
        text: &str,
        operation: EmbeddingInputOperation,
        call: F,
    ) -> Result<EmbedResult, EmbedError>
    where
        F: FnOnce(Arc<dyn EmbeddingProvider>, String) -> Result<EmbedResult, EmbedError> + Send + 'static,
    {
        let Ok(identity) = self.inner.builder_identity() else {
            return self.embed_with_timeout(text, call);
        };
        if let Some(cached) = self.cache.get(text, &identity, operation) {
            let identity_after = self
                .inner
                .builder_identity()
                .map_err(|_| EmbedError::ServerUnavailable("embedding provider identity unavailable".into()))?;
            if identity_after != identity
                || self.inner.raw_dimension() != identity.raw_dimension() as usize
                || self.inner.dimension() != identity.effective_dimension() as usize
                || cached.len() != identity.effective_dimension() as usize
                || cached.iter().any(|component| !component.is_finite())
                || cached.iter().all(|component| *component == 0.0)
            {
                return Err(EmbedError::ServerUnavailable(
                    "embedding provider identity changed".into(),
                ));
            }
            return Ok(EmbedResult {
                embedding: cached,
                input_tokens: 0,
            });
        }
        let result = self.embed_with_timeout(text, call)?;
        let identity_after = self
            .inner
            .builder_identity()
            .map_err(|_| EmbedError::ServerUnavailable("embedding provider identity unavailable".into()))?;
        if identity_after != identity
            || self.inner.raw_dimension() != identity.raw_dimension() as usize
            || self.inner.dimension() != identity.effective_dimension() as usize
            || result.embedding.len() != identity.effective_dimension() as usize
            || result.embedding.iter().any(|component| !component.is_finite())
            || result.embedding.iter().all(|component| *component == 0.0)
        {
            return Err(EmbedError::ServerUnavailable(
                "embedding provider identity changed".into(),
            ));
        }
        self.cache.put(text, &identity, operation, result.embedding.clone());
        Ok(result)
    }
}

impl EmbeddingProvider for CachingEmbeddingProvider {
    fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.embed_cached(text, EmbeddingInputOperation::Generic, |inner, text| inner.embed(&text))
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        texts.iter().map(|text| self.embed(text)).collect()
    }

    fn embed_query(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.embed_cached(text, EmbeddingInputOperation::Query, |inner, text| {
            inner.embed_query(&text)
        })
    }

    fn embed_document(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.embed_cached(text, EmbeddingInputOperation::Document, |inner, text| {
            inner.embed_document(&text)
        })
    }

    fn embed_document_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        texts.iter().map(|text| self.embed_document(text)).collect()
    }

    fn dimension(&self) -> usize {
        self.inner.dimension()
    }

    fn raw_dimension(&self) -> usize {
        self.inner.raw_dimension()
    }

    fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
        self.inner.builder_identity()
    }

    fn has_plico_adaptive_transform(&self) -> bool {
        self.inner.has_plico_adaptive_transform()
    }

    fn model_name(&self) -> String {
        self.inner.model_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    struct RecordingProvider {
        identity: Mutex<EmbeddingBuilderIdentity>,
        replacement_identity: EmbeddingBuilderIdentity,
        calls: Mutex<Vec<EmbeddingInputOperation>>,
        drift_on_call: AtomicBool,
    }

    impl RecordingProvider {
        fn new(contract: &'static str) -> Self {
            Self {
                identity: Mutex::new(EmbeddingBuilderIdentity::test_deterministic(
                    "same-model-name",
                    2,
                    contract,
                )),
                replacement_identity: EmbeddingBuilderIdentity::test_deterministic(
                    "same-model-name",
                    2,
                    "replacement-contract",
                ),
                calls: Mutex::new(Vec::new()),
                drift_on_call: AtomicBool::new(false),
            }
        }

        fn result(&self, operation: EmbeddingInputOperation) -> EmbedResult {
            self.calls.lock().unwrap().push(operation);
            if self.drift_on_call.swap(false, Ordering::AcqRel) {
                *self.identity.lock().unwrap() = self.replacement_identity.clone();
            }
            let value = match operation {
                EmbeddingInputOperation::Generic => 1.0,
                EmbeddingInputOperation::Query => 2.0,
                EmbeddingInputOperation::Document => 3.0,
            };
            EmbedResult::new(vec![value, value], 1)
        }
    }

    impl EmbeddingProvider for RecordingProvider {
        fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
            Ok(self.result(EmbeddingInputOperation::Generic))
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
            texts.iter().map(|text| self.embed(text)).collect()
        }

        fn embed_query(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
            Ok(self.result(EmbeddingInputOperation::Query))
        }

        fn embed_document(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
            Ok(self.result(EmbeddingInputOperation::Document))
        }

        fn dimension(&self) -> usize {
            2
        }

        fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
            Ok(self.identity.lock().unwrap().clone())
        }

        fn model_name(&self) -> String {
            "same-model-name".into()
        }
    }

    #[test]
    fn test_ollama_backend_creation_without_server() {
        let backend = OllamaBackend::new("http://localhost:9999", "all-minilm-l6-v2:latest").unwrap();
        assert_eq!(backend.dimension(), 0);
        assert!(backend.builder_identity().is_err());
    }

    #[test]
    fn cache_separates_generic_query_and_document_operations() {
        let inner = Arc::new(RecordingProvider::new("operation-contract"));
        let cache = Arc::new(EmbeddingCache::new(16));
        let provider = CachingEmbeddingProvider::new(inner.clone(), cache.clone());

        assert_eq!(provider.embed("same text").unwrap().embedding, vec![1.0, 1.0]);
        assert_eq!(provider.embed_query("same text").unwrap().embedding, vec![2.0, 2.0]);
        assert_eq!(provider.embed_document("same text").unwrap().embedding, vec![3.0, 3.0]);
        provider.embed("same text").unwrap();
        provider.embed_query("same text").unwrap();
        provider.embed_document("same text").unwrap();

        assert_eq!(inner.calls.lock().unwrap().len(), 3);
        assert_eq!(cache.stats().current_entries, 3);
        assert_eq!(cache.stats().hits, 3);
    }

    #[test]
    fn cache_separates_same_name_providers_with_different_identity() {
        let cache = Arc::new(EmbeddingCache::new(16));
        let first = Arc::new(RecordingProvider::new("contract-a"));
        let second = Arc::new(RecordingProvider::new("contract-b"));
        let first_cached = CachingEmbeddingProvider::new(first.clone(), cache.clone());
        let second_cached = CachingEmbeddingProvider::new(second.clone(), cache.clone());

        first_cached.embed_document("same text").unwrap();
        second_cached.embed_document("same text").unwrap();

        assert_eq!(first.calls.lock().unwrap().len(), 1);
        assert_eq!(second.calls.lock().unwrap().len(), 1);
        assert_eq!(cache.stats().current_entries, 2);
    }

    #[test]
    fn cache_does_not_publish_result_after_identity_changes() {
        let inner = Arc::new(RecordingProvider::new("contract-before"));
        inner.drift_on_call.store(true, Ordering::Release);
        let cache = Arc::new(EmbeddingCache::new(16));
        let provider = CachingEmbeddingProvider::new(inner, cache.clone());

        assert!(provider.embed_document("same text").is_err());
        assert_eq!(cache.stats().current_entries, 0);
    }
}
