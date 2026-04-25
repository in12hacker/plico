//! Model Hot-Swap Operations (v18.0)
//!
//! Provides runtime model switching for embedding and LLM models without restart.
//! Supports health checking before switch and automatic fallback on failure.

use std::sync::{Arc, RwLock};
use std::time::Instant;

use crate::api::semantic::{ModelSwitchResponse, ModelHealthResponse};
use crate::fs::{EmbeddingProvider, Embedding, EmbedError, EmbedResult};
use crate::llm::{LlmProvider, ChatMessage, ChatOptions};

use super::super::AIKernel;

/// Wrapper that implements EmbeddingProvider and delegates to a RwLock-protected inner provider.
/// This allows hot-swapping the underlying provider at runtime while presenting a stable
/// Arc<dyn EmbeddingProvider> interface to SemanticFS and other consumers.
pub struct HotSwapEmbeddingProvider {
    inner: Arc<RwLock<Arc<dyn EmbeddingProvider>>>,
}

impl HotSwapEmbeddingProvider {
    /// Create a new wrapper around the given RwLock-protected provider.
    pub fn new(inner: Arc<RwLock<Arc<dyn EmbeddingProvider>>>) -> Self {
        Self { inner }
    }

    /// Swap the inner provider. Returns the old provider Arc.
    pub fn swap(&self, new_provider: Arc<dyn EmbeddingProvider>) -> Arc<dyn EmbeddingProvider> {
        let mut guard = self.inner.write().unwrap();
        let old = Arc::clone(&guard);
        *guard = new_provider;
        old
    }

    /// Get the current inner provider Arc.
    pub fn current(&self) -> Arc<dyn EmbeddingProvider> {
        Arc::clone(&self.inner.read().unwrap())
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
        self.inner.read().unwrap().embed(text)
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<EmbedResult>, EmbedError> {
        self.inner.read().unwrap().embed_batch(texts)
    }

    fn dimension(&self) -> usize {
        self.inner.read().unwrap().dimension()
    }

    fn model_name(&self) -> &str {
        // We need to return &str, but accessing through the guard causes lifetime issues.
        // The Arc itself is owned by the RwLock and stays alive even after guard drops.
        // But we can't return &str referencing data inside it because the guard's lifetime
        // is what the compiler tracks. As a workaround, we use a static string for the wrapper.
        // Note: This means model_name() on the wrapper returns "hotswap" rather than the actual
        // inner model name. For the actual model name, use current().model_name().
        "hotswap"
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
    fn chat(&self, messages: &[ChatMessage], options: &ChatOptions) -> Result<String, crate::llm::LlmError> {
        self.inner.read().unwrap().chat(messages, options)
    }

    fn model_name(&self) -> &str {
        // Return a static identifier for the wrapper.
        // The actual model name is available via current().model_name().
        "hotswap-llm"
    }
}

/// Create a new embedding provider based on model_type and model_id.
pub(crate) fn create_embedding_provider(
    model_type: &str,
    model_id: &str,
    python_path: Option<&str>,
) -> Result<Arc<dyn EmbeddingProvider>, String> {
    match model_type {
        "local" => {
            let python = python_path
                .map(String::from)
                .unwrap_or_else(|| std::env::var("EMBEDDING_PYTHON").unwrap_or_else(|_| "python3".into()));
            crate::fs::LocalEmbeddingBackend::new(model_id, &python)
                .map(|b| Arc::new(b) as Arc<dyn EmbeddingProvider>)
                .map_err(|e| format!("local embedding error: {}", e))
        }
        "ollama" => {
            let url = std::env::var("OLLAMA_URL")
                .unwrap_or_else(|_| "http://localhost:11434".to_string());
            crate::fs::OllamaBackend::new(&url, model_id)
                .map(|b| Arc::new(b) as Arc<dyn EmbeddingProvider>)
                .map_err(|e| format!("ollama embedding error: {}", e))
        }
        "openai" => {
            let base_url = std::env::var("EMBEDDING_API_BASE")
                .map(|u| crate::kernel::persistence::ensure_v1_suffix(&u))
                .unwrap_or_else(|_| crate::kernel::persistence::resolve_llama_url());
            let api_key = std::env::var("EMBEDDING_API_KEY").ok();
            crate::fs::OpenAIEmbeddingBackend::new(&base_url, model_id, api_key)
                .map(|b| Arc::new(b) as Arc<dyn EmbeddingProvider>)
                .map_err(|e| format!("openai embedding error: {}", e))
        }
        "stub" => {
            Ok(Arc::new(crate::fs::StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>)
        }
        other => Err(format!("unknown embedding backend type: {}. Use 'local', 'ollama', 'openai', or 'stub'", other)),
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
            let base_url = url
                .map(String::from)
                .unwrap_or_else(|| std::env::var("OPENAI_API_BASE").unwrap_or_else(|_| "https://api.openai.com/v1".into()));
            let api_key = std::env::var("OPENAI_API_KEY").ok();
            crate::llm::OpenAICompatibleProvider::new(&base_url, model, api_key)
                .map(|p| Arc::new(p) as Arc<dyn LlmProvider>)
                .map_err(|e| format!("openai error: {}", e))
        }
        "llama" => {
            let base_url = url
                .map(String::from)
                .unwrap_or_else(|| crate::kernel::persistence::resolve_llama_url());
            crate::llm::OpenAICompatibleProvider::new(&base_url, model, None)
                .map(|p| Arc::new(p) as Arc<dyn LlmProvider>)
                .map_err(|e| format!("llama error: {}", e))
        }
        "stub" => {
            Ok(Arc::new(crate::llm::StubProvider::empty()) as Arc<dyn LlmProvider>)
        }
        other => Err(format!("unknown LLM backend: {}. Use 'ollama', 'openai', 'llama', or 'stub'", other)),
    }
}

/// Health check for embedding provider.
pub(crate) fn check_embedding_health(
    embedding: &Arc<dyn EmbeddingProvider>,
) -> ModelHealthResponse {
    let model = embedding.model_name().to_string();
    let start = Instant::now();
    match embedding.embed("health check probe") {
        Ok(_) => {
            let latency_ms = start.elapsed().as_millis() as u64;
            ModelHealthResponse {
                available: true,
                model,
                latency_ms: Some(latency_ms),
                error: None,
            }
        }
        Err(e) => {
            ModelHealthResponse {
                available: false,
                model,
                latency_ms: None,
                error: Some(e.to_string()),
            }
        }
    }
}

/// Health check for LLM provider.
pub(crate) fn check_llm_health(
    llm: &Arc<dyn LlmProvider>,
) -> ModelHealthResponse {
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
        Err(e) => {
            ModelHealthResponse {
                available: false,
                model,
                latency_ms: None,
                error: Some(e.to_string()),
            }
        }
    }
}

impl AIKernel {
    /// Switch the embedding model at runtime without restart.
    ///
    /// First verifies the new model is available by performing a health check.
    /// If the health check fails, the current model remains active and an error is returned.
    ///
    /// # Arguments
    /// * `model_type` - Backend type: "local", "ollama", "openai", or "stub"
    /// * `model_id` - Model identifier (e.g., "BAAI/bge-small-en-v1.5")
    /// * `python_path` - Optional Python interpreter path for local backend
    ///
    /// # Returns
    /// * `Ok(ModelSwitchResponse)` - Switch was successful
    /// * `Err(String)` - Switch failed (model unavailable or health check failed)
    pub fn switch_embedding_model(
        &self,
        model_type: &str,
        model_id: &str,
        python_path: Option<&str>,
    ) -> Result<ModelSwitchResponse, String> {
        let previous_model = self.embedding.model_name().to_string();

        // Create new provider
        let new_provider = create_embedding_provider(model_type, model_id, python_path)?;

        // Health check before switching
        let health = check_embedding_health(&new_provider);
        if !health.available {
            return Err(format!(
                "model health check failed for {} ({}): {}",
                model_id, model_type, health.error.unwrap_or_default()
            ));
        }

        // Perform the switch via the hot-swap wrapper
        let _old_provider = self.embedding.swap(new_provider);

        tracing::info!(
            "embedding model hot-swap: {} -> {} ({})",
            previous_model,
            model_id,
            model_type
        );

        Ok(ModelSwitchResponse {
            success: true,
            previous_model,
            new_model: model_id.to_string(),
            message: format!("successfully switched to {} ({})", model_id, model_type),
        })
    }

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
                model, backend, health.error.unwrap_or_default()
            ));
        }

        // Perform the switch via the hot-swap wrapper
        let _old_provider = self.llm_provider.swap(new_provider);

        tracing::info!(
            "LLM model hot-swap: {} -> {} ({})",
            previous_model,
            model,
            backend
        );

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
            "embedding" => {
                check_embedding_health(&self.embedding.current())
            }
            "llm" => {
                check_llm_health(&self.llm_provider.current())
            }
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

    // ─── HotSwapEmbeddingProvider ────────────────────────────────────────────

    #[test]
    fn test_hotswap_embedding_provider_new() {
        let inner = Arc::new(RwLock::new(Arc::new(crate::fs::StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>));
        let provider = HotSwapEmbeddingProvider::new(Arc::clone(&inner));
        assert_eq!(provider.model_name(), "hotswap");
    }

    #[test]
    fn test_hotswap_embedding_provider_current() {
        let stub = Arc::new(crate::fs::StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>;
        let inner = Arc::new(RwLock::new(stub.clone()));
        let provider = HotSwapEmbeddingProvider::new(inner);
        let current = provider.current();
        assert_eq!(current.model_name(), "stub");
    }

    #[test]
    fn test_hotswap_embedding_provider_swap() {
        let inner = Arc::new(RwLock::new(Arc::new(crate::fs::StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>));
        let provider = HotSwapEmbeddingProvider::new(Arc::clone(&inner));
        let new_stub = Arc::new(crate::fs::StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>;
        let old = provider.swap(new_stub.clone());
        assert!(old.model_name() == "stub" || old.model_name() == "stub");
    }

    #[test]
    fn test_hotswap_embedding_provider_clone() {
        let inner = Arc::new(RwLock::new(Arc::new(crate::fs::StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>));
        let provider = HotSwapEmbeddingProvider::new(Arc::clone(&inner));
        let _cloned = provider.clone();
        // Clone should not panic
    }

    #[test]
    fn test_hotswap_embedding_provider_embed_delegates() {
        let inner = Arc::new(RwLock::new(Arc::new(crate::fs::StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>));
        let provider = HotSwapEmbeddingProvider::new(inner);
        let result = provider.embed("test text");
        // StubEmbeddingProvider always returns error
        assert!(result.is_err(), "stub should return error for embed");
    }

    // ─── HotSwapLlmProvider ─────────────────────────────────────────────────

    #[test]
    fn test_hotswap_llm_provider_new() {
        let inner = Arc::new(RwLock::new(Arc::new(crate::llm::StubProvider::empty()) as Arc<dyn LlmProvider>));
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
        let inner = Arc::new(RwLock::new(Arc::new(crate::llm::StubProvider::empty()) as Arc<dyn LlmProvider>));
        let provider = HotSwapLlmProvider::new(inner);
        let new_stub = Arc::new(crate::llm::StubProvider::empty()) as Arc<dyn LlmProvider>;
        let _old = provider.swap(new_stub);
    }

    #[test]
    fn test_hotswap_llm_provider_clone() {
        let inner = Arc::new(RwLock::new(Arc::new(crate::llm::StubProvider::empty()) as Arc<dyn LlmProvider>));
        let provider = HotSwapLlmProvider::new(inner);
        let _cloned = provider.clone();
    }

    #[test]
    fn test_hotswap_llm_provider_chat_delegates() {
        let inner = Arc::new(RwLock::new(Arc::new(crate::llm::StubProvider::empty()) as Arc<dyn LlmProvider>));
        let provider = HotSwapLlmProvider::new(inner);
        let result = provider.chat(&[ChatMessage::user("hello")], &ChatOptions::default());
        assert!(result.is_ok(), "stub should always succeed: {:?}", result);
    }

    // ─── Provider Creation ──────────────────────────────────────────────────

    #[test]
    fn test_create_embedding_provider_stub() {
        let result = create_embedding_provider("stub", "test-model", None);
        assert!(result.is_ok(), "stub should always succeed");
        let provider = result.unwrap();
        assert_eq!(provider.model_name(), "stub");
    }

    #[test]
    fn test_create_embedding_provider_unknown_type() {
        let result = create_embedding_provider("unknown_type", "model", None);
        match result {
            Err(e) => assert!(e.contains("unknown embedding backend type")),
            Ok(_) => panic!("expected error, got ok"),
        }
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

    #[test]
    fn test_create_embedding_provider_local_missing_python() {
        // Should fail gracefully when python path is invalid
        let result = create_embedding_provider("local", "model", Some("/nonexistent/python"));
        assert!(result.is_err());
    }

    // ─── Health Checks ───────────────────────────────────────────────────────

    #[test]
    fn test_check_embedding_health_stub_available() {
        let stub = Arc::new(crate::fs::StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>;
        let result = check_embedding_health(&stub);
        assert!(!result.available, "stub should not be available");
        assert_eq!(result.model, "stub");
    }

    #[test]
    fn test_check_llm_health_stub_available() {
        let stub = Arc::new(crate::llm::StubProvider::empty()) as Arc<dyn LlmProvider>;
        let result = check_llm_health(&stub);
        assert!(result.available, "stub should be available");
        assert_eq!(result.model, "stub");
    }
}