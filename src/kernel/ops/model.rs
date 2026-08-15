//! Model provider operations.
//!
//! Embedding providers are read through one stable snapshot; runtime embedding
//! switching is intentionally absent until derived indexes can switch atomically.
//! LLM providers retain their independent runtime switch path.

use std::sync::{Arc, RwLock};
use std::time::Instant;

use crate::api::semantic::{ModelHealthResponse, ModelSwitchResponse};
use crate::fs::{
    EmbedError, EmbedResult, EmbeddingBuilderIdentity, EmbeddingIdentityError, EmbeddingProvider,
    VerifiedDocumentProviderSnapshot,
};
use crate::llm::{ChatMessage, ChatOptions, LlmProvider};

use super::super::AIKernel;

/// Stable EmbeddingProvider wrapper that captures provider and identity together.
pub struct HotSwapEmbeddingProvider {
    inner: Arc<RwLock<EmbeddingProviderSlot>>,
}

struct EmbeddingProviderSlot {
    provider: Arc<dyn EmbeddingProvider>,
    identity: Result<EmbeddingBuilderIdentity, EmbeddingIdentityError>,
}

impl HotSwapEmbeddingProvider {
    /// Create a new wrapper around the given RwLock-protected provider.
    pub fn new(provider: Arc<dyn EmbeddingProvider>) -> Self {
        let identity = provider.builder_identity();
        Self {
            inner: Arc::new(RwLock::new(EmbeddingProviderSlot { provider, identity })),
        }
    }

    /// Get the current inner provider Arc.
    pub fn current(&self) -> Arc<dyn EmbeddingProvider> {
        Arc::clone(&self.inner.read().unwrap().provider)
    }

    /// Capture one exact provider+identity pair while preventing a concurrent swap.
    pub(crate) fn verified_document_snapshot(
        &self,
    ) -> Result<VerifiedDocumentProviderSnapshot, EmbeddingIdentityError> {
        let guard = self.inner.read().unwrap();
        let expected = guard.identity.as_ref().map_err(|error| *error)?;
        VerifiedDocumentProviderSnapshot::verify_exact(Arc::clone(&guard.provider), expected)
    }
}

impl Clone for HotSwapEmbeddingProvider {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl EmbeddingProvider for HotSwapEmbeddingProvider {
    fn embed(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.current().embed(text)
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        self.current().embed_batch(texts)
    }

    fn embed_query(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.current().embed_query(text)
    }

    fn embed_document(&self, text: &str) -> Result<EmbedResult, EmbedError> {
        self.current().embed_document(text)
    }

    fn embed_document_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        self.current().embed_document_batch(texts)
    }

    fn dimension(&self) -> usize {
        self.inner.read().unwrap().provider.dimension()
    }

    fn raw_dimension(&self) -> usize {
        self.inner.read().unwrap().provider.raw_dimension()
    }

    fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
        self.inner.read().unwrap().identity.clone()
    }

    fn has_plico_adaptive_transform(&self) -> bool {
        self.inner.read().unwrap().provider.has_plico_adaptive_transform()
    }

    fn model_name(&self) -> String {
        self.inner.read().unwrap().provider.model_name()
    }
}

/// Wrapper that implements LlmProvider and delegates to a RwLock-protected inner provider.
/// This allows hot-swapping the underlying provider at runtime.
pub struct HotSwapLlmProvider {
    inner: Arc<RwLock<Arc<dyn LlmProvider>>>,
}

impl HotSwapLlmProvider {
    /// Create a new wrapper around the given RwLock-protected provider.
    pub fn new(inner: Arc<RwLock<Arc<dyn LlmProvider>>>) -> Self {
        Self { inner }
    }

    /// Swap the inner provider. Returns the old provider Arc.
    pub fn swap(&self, new_provider: Arc<dyn LlmProvider>) -> Arc<dyn LlmProvider> {
        let mut guard = self.inner.write().unwrap();
        let old = Arc::clone(&guard);
        *guard = new_provider;
        old
    }

    /// Get the current inner provider Arc.
    pub fn current(&self) -> Arc<dyn LlmProvider> {
        Arc::clone(&self.inner.read().unwrap())
    }
}

impl Clone for HotSwapLlmProvider {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl LlmProvider for HotSwapLlmProvider {
    fn chat(
        &self,
        messages: &[ChatMessage],
        options: &ChatOptions,
    ) -> Result<(String, u32, u32), crate::llm::LlmError> {
        self.inner.read().unwrap().chat(messages, options)
    }

    fn model_name(&self) -> &str {
        // Return a static identifier for the wrapper.
        // The actual model name is available via current().model_name().
        "hotswap-llm"
    }
}

/// Create a new LLM provider based on backend and model.
pub(crate) fn create_llm_provider(
    backend: &str,
    model: &str,
    url: Option<&str>,
) -> Result<Arc<dyn LlmProvider>, String> {
    match backend {
        "ollama" => {
            let url = url
                .map(String::from)
                .unwrap_or_else(|| std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".into()));
            crate::llm::OllamaProvider::new(&url, model)
                .map(|p| Arc::new(p) as Arc<dyn LlmProvider>)
                .map_err(|e| format!("ollama error: {}", e))
        }
        "openai" => {
            let base_url = url.map(String::from).unwrap_or_else(|| {
                std::env::var("OPENAI_API_BASE").unwrap_or_else(|_| "https://api.openai.com/v1".into())
            });
            let api_key = std::env::var("OPENAI_API_KEY").ok();
            crate::llm::OpenAICompatibleProvider::new(&base_url, model, api_key)
                .map(|p| Arc::new(p) as Arc<dyn LlmProvider>)
                .map_err(|e| format!("openai error: {}", e))
        }
        "llama" => {
            let base_url = url
                .map(String::from)
                .unwrap_or_else(crate::kernel::persistence::resolve_llama_url);
            crate::llm::OpenAICompatibleProvider::new(&base_url, model, None)
                .map(|p| Arc::new(p) as Arc<dyn LlmProvider>)
                .map_err(|e| format!("llama error: {}", e))
        }
        "stub" => Ok(Arc::new(crate::llm::StubProvider::empty()) as Arc<dyn LlmProvider>),
        other => Err(format!(
            "unknown LLM backend: {}. Use 'ollama', 'openai', 'llama', or 'stub'",
            other
        )),
    }
}

fn check_verified_document_health(embedding: &HotSwapEmbeddingProvider) -> ModelHealthResponse {
    let model = embedding.model_name();
    let start = Instant::now();
    let snapshot = match embedding.verified_document_snapshot() {
        Ok(snapshot) => snapshot,
        Err(_) => {
            return ModelHealthResponse {
                available: false,
                model,
                latency_ms: None,
                error: Some("embedding builder identity unavailable".into()),
            };
        }
    };
    match snapshot.embed_document("plico document health probe v1") {
        Ok(result)
            if result.embedding.len() == snapshot.identity().effective_dimension() as usize
                && result.embedding.iter().all(|component| component.is_finite())
                && result.embedding.iter().any(|component| *component != 0.0) =>
        {
            ModelHealthResponse {
                available: true,
                model,
                latency_ms: Some(start.elapsed().as_millis() as u64),
                error: None,
            }
        }
        Ok(_) | Err(_) => ModelHealthResponse {
            available: false,
            model,
            latency_ms: None,
            error: Some("embedding document probe failed".into()),
        },
    }
}

/// Health check for LLM provider.
pub(crate) fn check_llm_health(llm: &Arc<dyn LlmProvider>) -> ModelHealthResponse {
    let model = llm.model_name().to_string();
    let start = Instant::now();
    let test_messages = [ChatMessage::user("hi")];
    let options = ChatOptions::default();
    match llm.chat(&test_messages, &options) {
        Ok(_) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            ModelHealthResponse {
                available: true,
                model,
                latency_ms: Some(latency_ms),
                error: None,
            }
        }
        Err(e) => ModelHealthResponse {
            available: false,
            model,
            latency_ms: None,
            error: Some(e.to_string()),
        },
    }
}

impl AIKernel {
    /// Switch the LLM model at runtime without restart.
    ///
    /// First verifies the new model is available by performing a health check.
    /// If the health check fails, the current model remains active and an error is returned.
    ///
    /// # Arguments
    /// * `backend` - Backend type: "ollama", "openai", "llama", or "stub"
    /// * `model` - Model name (e.g., "llama3.2")
    /// * `url` - Optional URL override
    ///
    /// # Returns
    /// * `Ok(ModelSwitchResponse)` - Switch was successful
    /// * `Err(String)` - Switch failed (model unavailable or health check failed)
    pub fn switch_llm_model(
        &self,
        backend: &str,
        model: &str,
        url: Option<&str>,
    ) -> Result<ModelSwitchResponse, String> {
        let previous_model = self.llm_provider.model_name().to_string();

        // Create new provider
        let new_provider = create_llm_provider(backend, model, url)?;

        // Health check before switching
        let health = check_llm_health(&new_provider);
        if !health.available {
            return Err(format!(
                "model health check failed for {} ({}): {}",
                model,
                backend,
                health.error.unwrap_or_default()
            ));
        }

        // Perform the switch via the hot-swap wrapper
        let _old_provider = self.llm_provider.swap(new_provider);

        tracing::info!("LLM model hot-swap: {} -> {} ({})", previous_model, model, backend);

        Ok(ModelSwitchResponse {
            success: true,
            previous_model,
            new_model: model.to_string(),
            message: format!("successfully switched to {} ({})", model, backend),
        })
    }

    /// Check the health of a model.
    ///
    /// # Arguments
    /// * `model_type` - "embedding" or "llm"
    ///
    /// # Returns
    /// `ModelHealthResponse` with availability status and latency
    pub fn check_model_health(&self, model_type: &str) -> ModelHealthResponse {
        match model_type {
            "embedding" => check_verified_document_health(&self.embedding),
            "llm" => check_llm_health(&self.llm_provider.current()),
            other => ModelHealthResponse {
                available: false,
                model: String::new(),
                latency_ms: None,
                error: Some(format!("unknown model type: {}. Use 'embedding' or 'llm'", other)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
    use std::sync::{Barrier, Mutex};

    struct TestEmbeddingProvider {
        model: &'static str,
        contract: &'static str,
    }

    impl EmbeddingProvider for TestEmbeddingProvider {
        fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
            Ok(EmbedResult::new(vec![1.0, 2.0], 1))
        }

        fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
            texts.iter().map(|text| self.embed(text)).collect()
        }

        fn embed_document(&self, text: &str) -> Result<EmbedResult, EmbedError> {
            self.embed(text)
        }

        fn dimension(&self) -> usize {
            2
        }

        fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
            Ok(EmbeddingBuilderIdentity::test_deterministic(
                self.model,
                2,
                self.contract,
            ))
        }

        fn model_name(&self) -> String {
            self.model.into()
        }
    }

    fn test_embedding(model: &'static str, contract: &'static str) -> Arc<dyn EmbeddingProvider> {
        Arc::new(TestEmbeddingProvider { model, contract })
    }

    // ─── HotSwapEmbeddingProvider ────────────────────────────────────────────

    #[test]
    fn test_hotswap_embedding_provider_new() {
        let provider = HotSwapEmbeddingProvider::new(test_embedding("first", "contract-a"));
        assert_eq!(provider.model_name(), "first");
        assert!(provider.builder_identity().is_ok());
    }

    #[test]
    fn test_hotswap_embedding_provider_current() {
        let provider = HotSwapEmbeddingProvider::new(test_embedding("first", "contract-a"));
        let current = provider.current();
        assert_eq!(current.model_name(), "first");
    }

    #[test]
    fn test_hotswap_embedding_provider_clone() {
        let provider = HotSwapEmbeddingProvider::new(test_embedding("first", "contract-a"));
        let _cloned = provider.clone();
    }

    #[test]
    fn test_hotswap_embedding_provider_embed_delegates() {
        let provider = HotSwapEmbeddingProvider::new(test_embedding("first", "contract-a"));
        let result = provider.embed("test text");
        assert!(result.is_ok());
    }

    #[test]
    fn verified_snapshot_never_pairs_stale_identity_with_changed_provider() {
        struct GatedIdentityProvider {
            identity: Mutex<EmbeddingBuilderIdentity>,
            value: AtomicU32,
            identity_calls: AtomicUsize,
            entered: Arc<Barrier>,
            release: Arc<Barrier>,
        }

        impl EmbeddingProvider for GatedIdentityProvider {
            fn embed(&self, _text: &str) -> Result<EmbedResult, EmbedError> {
                let value = self.value.load(Ordering::Acquire) as f32;
                Ok(EmbedResult::new(vec![value, value], 1))
            }

            fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
                texts.iter().map(|text| self.embed(text)).collect()
            }

            fn dimension(&self) -> usize {
                2
            }

            fn builder_identity(&self) -> Result<EmbeddingBuilderIdentity, EmbeddingIdentityError> {
                if self.identity_calls.fetch_add(1, Ordering::SeqCst) == 1 {
                    self.entered.wait();
                    self.release.wait();
                }
                Ok(self.identity.lock().unwrap().clone())
            }

            fn model_name(&self) -> String {
                "gated-identity".into()
            }
        }

        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let provider = Arc::new(GatedIdentityProvider {
            identity: Mutex::new(EmbeddingBuilderIdentity::test_deterministic(
                "gated-identity",
                2,
                "identity-a",
            )),
            value: AtomicU32::new(1),
            identity_calls: AtomicUsize::new(0),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
        });
        let hot_swap = HotSwapEmbeddingProvider::new(provider.clone());
        let reader = hot_swap.clone();
        let thread = std::thread::spawn(move || reader.verified_document_snapshot());
        entered.wait();
        *provider.identity.lock().unwrap() =
            EmbeddingBuilderIdentity::test_deterministic("gated-identity", 2, "identity-b");
        provider.value.store(2, Ordering::Release);
        release.wait();

        assert!(matches!(
            thread.join().unwrap(),
            Err(EmbeddingIdentityError::ProviderChanged)
        ));
    }

    // ─── HotSwapLlmProvider ─────────────────────────────────────────────────

    #[test]
    fn test_hotswap_llm_provider_new() {
        let inner = Arc::new(RwLock::new(
            Arc::new(crate::llm::StubProvider::empty()) as Arc<dyn LlmProvider>
        ));
        let provider = HotSwapLlmProvider::new(Arc::clone(&inner));
        assert_eq!(provider.model_name(), "hotswap-llm");
    }

    #[test]
    fn test_hotswap_llm_provider_current() {
        let stub = Arc::new(crate::llm::StubProvider::empty()) as Arc<dyn LlmProvider>;
        let inner = Arc::new(RwLock::new(stub));
        let provider = HotSwapLlmProvider::new(inner);
        let current = provider.current();
        assert_eq!(current.model_name(), "stub");
    }

    #[test]
    fn test_hotswap_llm_provider_swap() {
        let inner = Arc::new(RwLock::new(
            Arc::new(crate::llm::StubProvider::empty()) as Arc<dyn LlmProvider>
        ));
        let provider = HotSwapLlmProvider::new(inner);
        let new_stub = Arc::new(crate::llm::StubProvider::empty()) as Arc<dyn LlmProvider>;
        let _old = provider.swap(new_stub);
    }

    #[test]
    fn test_hotswap_llm_provider_clone() {
        let inner = Arc::new(RwLock::new(
            Arc::new(crate::llm::StubProvider::empty()) as Arc<dyn LlmProvider>
        ));
        let provider = HotSwapLlmProvider::new(inner);
        let _cloned = provider.clone();
    }

    #[test]
    fn test_hotswap_llm_provider_chat_delegates() {
        let inner = Arc::new(RwLock::new(
            Arc::new(crate::llm::StubProvider::empty()) as Arc<dyn LlmProvider>
        ));
        let provider = HotSwapLlmProvider::new(inner);
        let result = provider.chat(&[ChatMessage::user("hello")], &ChatOptions::default());
        assert!(result.is_ok(), "stub should always succeed: {:?}", result);
    }

    #[test]
    fn test_create_llm_provider_stub() {
        let result = create_llm_provider("stub", "test-model", None);
        assert!(result.is_ok(), "stub should always succeed");
        let provider = result.unwrap();
        assert_eq!(provider.model_name(), "stub");
    }

    #[test]
    fn test_create_llm_provider_unknown_backend() {
        let result = create_llm_provider("unknown_backend", "model", None);
        match result {
            Err(e) => assert!(e.contains("unknown LLM backend")),
            Ok(_) => panic!("expected error, got ok"),
        }
    }

    // ─── Health Checks ───────────────────────────────────────────────────────

    #[test]
    fn test_check_llm_health_stub_available() {
        let stub = Arc::new(crate::llm::StubProvider::empty()) as Arc<dyn LlmProvider>;
        let result = check_llm_health(&stub);
        assert!(result.available, "stub should be available");
        assert_eq!(result.model, "stub");
    }
}
