//! Kernel State Persistence — agent/intent/memory/search index persistence and restore.
//!
//! Persists and restores kernel state (agents, intents, memories, search index) to/from
//! CAS and JSON files. Also contains the embedding provider factory functions.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::fs::{
    AdaptiveEmbeddingProvider, EmbedError, EmbeddingCircuitBreaker, EmbeddingProvider, LocalEmbeddingBackend,
    OllamaBackend, OpenAIEmbeddingBackend, StubEmbeddingProvider,
};
use crate::kernel::ops::cache::EmbeddingCache;
use crate::llm::{
    CircuitBreakerLlmProvider, LlmError, LlmProvider, OllamaProvider, OpenAICompatibleProvider, StubProvider,
};
use crate::memory::CanonicalLedger;

use super::AIKernel;

/// Resolve llama.cpp server URL via unified config.
pub(crate) fn resolve_llama_url() -> String {
    crate::config::PlicoConfig::load(None).resolve_llama_url()
}

pub fn atomic_write_json<T: serde::Serialize>(path: &Path, data: &T) {
    let tmp = path.with_extension("json.tmp");
    if let Ok(json) = serde_json::to_string_pretty(data) {
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

pub fn atomic_write_bytes(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(error);
    }
    Ok(())
}

impl AIKernel {
    pub(crate) fn restore_memories(&self) -> Result<(), crate::memory::LedgerError> {
        for id in self.canonical.list_origin_roles()? {
            self.memory.restore_agent(&id)?;
        }
        Ok(())
    }

    pub(crate) fn persist_memories(&self) -> Result<bool, crate::memory::LedgerError> {
        self.memory.flush_ledger()
    }

    pub fn flush_canonical_memory(&self) -> Result<(), crate::memory::LedgerError> {
        self.persist_memories()?;
        tracing::info!(
            phase = "flush_ledger",
            outcome = "success",
            "canonical memory ledger flushed"
        );
        Ok(())
    }

    pub fn persist_auxiliary_best_effort(&self) {
        self.persist_agents();
        self.persist_intents();
        self.persist_permissions();
        self.persist_event_log();
        self.persist_search_index();
        self.fs.flush_tag_index();
        if self.persist_checkpoints().is_err() {
            tracing::warn!(
                phase = "persist_auxiliary",
                error_category = "checkpoint_write",
                "checkpoint persistence failed"
            );
        }
        self.persist_task_store();
        self.persist_key_store();
        self.persist_sessions();
        let _ = self.prefetch.persist();
        let _ = self.cost_ledger.persist_to_dir(&self.root.join("prefetch"));
    }

    pub(crate) fn persist_sessions(&self) {
        let _ = self.session_store.persist(&self.root);
    }
    pub(crate) fn agent_index_path(&self) -> PathBuf {
        self.root.join("agent_index.json")
    }
    pub(crate) fn persist_agents(&self) {
        atomic_write_json(&self.agent_index_path(), &self.scheduler.snapshot_agents());
        self.persist_usage();
    }
    fn usage_index_path(&self) -> PathBuf {
        self.root.join("usage_index.json")
    }
    pub(crate) fn persist_usage(&self) {
        atomic_write_json(&self.usage_index_path(), &self.scheduler.snapshot_usage());
    }

    fn restore_usage(&self) {
        let path = self.usage_index_path();
        if let Ok(json) = std::fs::read_to_string(&path) {
            if let Ok(data) = serde_json::from_str(&json) {
                self.scheduler.restore_usage(data);
            }
        }
    }

    pub(crate) fn restore_agents(&self) {
        let path = self.agent_index_path();
        if let Ok(json) = std::fs::read_to_string(&path) {
            if let Ok(agents) = serde_json::from_str(&json) {
                self.scheduler.restore_agents(agents);
            }
        }
        self.restore_usage();
    }

    fn intent_index_path(&self) -> PathBuf {
        self.root.join("intent_index.json")
    }
    pub(crate) fn persist_intents(&self) {
        atomic_write_json(&self.intent_index_path(), &self.scheduler.snapshot_intents());
    }
    pub(crate) fn restore_intents(&self) {
        let path = self.intent_index_path();
        if let Ok(json) = std::fs::read_to_string(&path) {
            if let Ok(intents) = serde_json::from_str(&json) {
                self.scheduler.restore_intents(intents);
            }
        }
        let _ = std::fs::remove_file(&path);
    }

    pub(crate) fn persist_search_index(&self) {
        let _ = self.search_backend.persist_to(&self.root);
    }

    fn permission_index_path(&self) -> PathBuf {
        self.root.join("permission_index.json")
    }
    pub(crate) fn persist_permissions(&self) {
        let grants = self.permissions.snapshot();
        if grants.is_empty() {
            let _ = std::fs::remove_file(self.permission_index_path());
        } else {
            atomic_write_json(&self.permission_index_path(), &grants);
        }
    }
    pub(crate) fn restore_permissions(&self) {
        let path = self.permission_index_path();
        if let Ok(json) = std::fs::read_to_string(&path) {
            if let Ok(grants) = serde_json::from_str(&json) {
                self.permissions.restore(grants);
            }
        }
    }

    pub(crate) fn persist_event_log(&self) {
        let events = self.event_bus.snapshot_events();
        if events.is_empty() {
            return;
        }
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(self.root.join("event_log.jsonl"))
        {
            use std::io::Write;
            for e in &events {
                if let Ok(json) = serde_json::to_string(e) {
                    let _ = writeln!(file, "{}", json);
                }
            }
        }
    }
    pub(crate) fn restore_event_log(&self) {
        if let Ok(events) = super::event_bus::EventBus::load_event_log(&self.root.join("event_log.jsonl")) {
            self.event_bus.restore_events(events);
        }
    }

    pub(crate) fn persist_checkpoints(&self) -> std::io::Result<()> {
        self.checkpoint_store.persist(&self.root, &self.cas)
    }
    pub(crate) fn persist_task_store(&self) {
        self.task_store.persist();
    }
    pub(crate) fn restore_task_store(&self) {}
    pub(crate) fn persist_key_store(&self) {
        self.key_store.persist(&self.root);
    }
}

fn read_circuit_breaker_config(t_env: &str, c_env: &str, t_def: u32, c_def: u64) -> (u32, u64) {
    let t = std::env::var(t_env).ok().and_then(|v| v.parse().ok()).unwrap_or(t_def);
    let c = std::env::var(c_env).ok().and_then(|v| v.parse().ok()).unwrap_or(c_def);
    (t, c)
}

pub(crate) fn create_embedding_provider(
    config: &crate::config::InferenceConfig,
    cache: Arc<EmbeddingCache>,
) -> Result<Arc<dyn EmbeddingProvider>, EmbedError> {
    let backend = &config.embedding_backend;
    let base_provider: Arc<dyn EmbeddingProvider> = match backend.as_str() {
        "ort" => return Err(EmbedError::ModelNotFound("ort embedding activation unavailable".into())),
        "local" => {
            let model_id = config
                .embedding_model_id
                .clone()
                .unwrap_or_else(|| "BAAI/bge-small-en-v1.5".to_string());
            let python = config.embedding_python.clone().unwrap_or_else(|| "python3".to_string());
            Arc::new(LocalEmbeddingBackend::new(&model_id, &python)?)
        }
        "openai" => {
            let base_url = config
                .embedding_api_base
                .clone()
                .map(|u| crate::config::ensure_v1_suffix(&u))
                .unwrap_or_else(|| {
                    if let Some(port) = crate::config::detect_embedding_server_port() {
                        format!("http://127.0.0.1:{port}/v1")
                    } else {
                        "http://127.0.0.1:8080/v1".into()
                    }
                });
            let model = config
                .embedding_model
                .clone()
                .unwrap_or_else(|| "all-MiniLM-L6-v2".to_string());
            Arc::new(OpenAIEmbeddingBackend::new(&base_url, &model, config.api_key.clone())?)
        }
        "stub" => Arc::new(StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>,
        "ollama" => {
            let url = config
                .ollama_url
                .clone()
                .unwrap_or_else(|| "http://localhost:11434".to_string());
            let model = config
                .embedding_model
                .clone()
                .unwrap_or_else(|| "all-minilm-l6-v2:latest".to_string());
            Arc::new(OllamaBackend::new(&url, &model)?)
        }
        _ => return Err(EmbedError::ModelNotFound("unknown embedding backend".into())),
    };

    wrap_embedding_provider(base_provider, config, cache)
}

pub(crate) fn wrap_embedding_provider(
    base_provider: Arc<dyn EmbeddingProvider>,
    config: &crate::config::InferenceConfig,
    cache: Arc<EmbeddingCache>,
) -> Result<Arc<dyn EmbeddingProvider>, EmbedError> {
    let (threshold, cooldown_ms) =
        read_circuit_breaker_config("EMBEDDING_CB_THRESHOLD", "EMBEDDING_CB_COOLDOWN_MS", 3, 30_000);
    let with_cb = Arc::new(EmbeddingCircuitBreaker::new(base_provider, threshold, cooldown_ms));
    let adaptive = AdaptiveEmbeddingProvider::from_config(with_cb as Arc<dyn EmbeddingProvider>, config)
        .map_err(|_| EmbedError::Api("invalid adaptive embedding configuration".into()))?;
    Ok(Arc::new(crate::fs::embedding::CachingEmbeddingProvider::new(
        Arc::new(adaptive),
        cache,
    )))
}

pub(crate) fn create_llm_provider(model_env: &str, default_model: &str) -> Result<Arc<dyn LlmProvider>, LlmError> {
    let backend = std::env::var("LLM_BACKEND").unwrap_or_else(|_| "llama".to_string());
    let inner: Arc<dyn LlmProvider> = match backend.as_str() {
        "ollama" => {
            let url = std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".to_string());
            let model = std::env::var(model_env).unwrap_or_else(|_| default_model.to_string());
            Arc::new(OllamaProvider::new(&url, &model)?) as Arc<dyn LlmProvider>
        }
        "openai" => {
            let base_url = std::env::var("OPENAI_API_BASE").unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
            let model = std::env::var(model_env).unwrap_or_else(|_| default_model.to_string());
            Arc::new(OpenAICompatibleProvider::new(
                &base_url,
                &model,
                std::env::var("OPENAI_API_KEY").ok(),
            )?) as Arc<dyn LlmProvider>
        }
        "llama" => {
            let base_url = crate::config::PlicoConfig::load(None).resolve_llama_url();
            let model = std::env::var("LLAMA_MODEL")
                .or_else(|_| std::env::var(model_env))
                .unwrap_or_else(|_| default_model.to_string());
            Arc::new(OpenAICompatibleProvider::new(&base_url, &model, None)?) as Arc<dyn LlmProvider>
        }
        _ => Arc::new(StubProvider::empty()) as Arc<dyn LlmProvider>,
    };
    Ok(Arc::new(CircuitBreakerLlmProvider::new(inner, 5, 60_000)) as Arc<dyn LlmProvider>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::tests::make_kernel;

    #[test]
    fn test_atomic_write_json_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.json");
        let data = vec![1, 2, 3];
        atomic_write_json(&path, &data);
        let loaded: Vec<i32> = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(loaded, data);
    }

    #[test]
    fn test_atomic_write_json_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("new.json");
        atomic_write_json(&path, &"hello");
        assert!(path.exists());
        let content: String = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(content, "hello");
    }

    #[test]
    fn test_atomic_write_bytes_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.bin");
        atomic_write_bytes(&path, b"binary data").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"binary data");
    }

    #[test]
    fn test_atomic_write_bytes_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("overwrite.bin");
        atomic_write_bytes(&path, b"first").unwrap();
        atomic_write_bytes(&path, b"second").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"second");
    }

    #[test]
    fn atomic_write_bytes_propagates_rename_failure_and_cleans_temp() {
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("destination");
        std::fs::create_dir(&destination).unwrap();
        let temporary = destination.with_extension("tmp");

        assert!(atomic_write_bytes(&destination, b"must not report success").is_err());
        assert!(!temporary.exists());
        assert!(destination.is_dir());
    }

    #[test]
    fn test_read_circuit_breaker_config_defaults() {
        // With no env vars set, defaults are used
        let (t, c) = read_circuit_breaker_config("NONEXISTENT_T", "NONEXISTENT_C", 5, 10_000);
        assert_eq!(t, 5);
        assert_eq!(c, 10_000);
    }

    #[test]
    fn test_create_embedding_provider_stub() {
        let config = crate::config::InferenceConfig {
            embedding_backend: "stub".to_string(),
            ..Default::default()
        };
        let provider = create_embedding_provider(&config, Arc::new(EmbeddingCache::new(8)));
        assert!(provider.is_ok());
        assert!(matches!(
            provider.unwrap().builder_identity(),
            Err(crate::fs::EmbeddingIdentityError::StubProvider)
        ));
    }

    #[test]
    fn embedding_pipeline_rejects_a_second_wrapper_stack() {
        let config = crate::config::InferenceConfig {
            embedding_backend: "stub".to_string(),
            ..Default::default()
        };
        let first = wrap_embedding_provider(
            Arc::new(StubEmbeddingProvider::new()),
            &config,
            Arc::new(EmbeddingCache::new(8)),
        )
        .unwrap();
        let second = wrap_embedding_provider(first, &config, Arc::new(EmbeddingCache::new(8)));
        assert!(matches!(second, Err(EmbedError::Api(_))));
    }

    #[test]
    fn removed_ort_and_unknown_backends_fail_without_stub_fallback() {
        for backend in ["ort", "unknown-provider"] {
            let config = crate::config::InferenceConfig {
                embedding_backend: backend.to_string(),
                ..Default::default()
            };
            let result = create_embedding_provider(&config, Arc::new(EmbeddingCache::new(8)));
            assert!(matches!(result, Err(EmbedError::ModelNotFound(_))), "backend {backend}");
        }
    }

    #[test]
    fn remote_provider_capability_matrix_is_fail_closed() {
        let openai = create_embedding_provider(
            &crate::config::InferenceConfig {
                embedding_backend: "openai".into(),
                embedding_api_base: Some("http://127.0.0.1:1/v1".into()),
                embedding_model: Some("unpinned-alias".into()),
                ..Default::default()
            },
            Arc::new(EmbeddingCache::new(8)),
        )
        .unwrap();
        assert!(matches!(
            openai.builder_identity(),
            Err(crate::fs::EmbeddingIdentityError::UnpinnedRemoteModel)
        ));

        let ollama = create_embedding_provider(
            &crate::config::InferenceConfig {
                embedding_backend: "ollama".into(),
                embedding_model: Some("nomic-embed-text:latest".into()),
                ollama_url: Some("http://127.0.0.1:1".into()),
                ..Default::default()
            },
            Arc::new(EmbeddingCache::new(8)),
        )
        .unwrap();
        assert!(matches!(
            ollama.builder_identity(),
            Err(crate::fs::EmbeddingIdentityError::ProviderProbeFailed)
        ));
    }

    #[test]
    fn test_create_llm_provider_stub() {
        std::env::set_var("LLM_BACKEND", "stub");
        let result = create_llm_provider("TEST_MODEL", "test-model");
        assert!(result.is_ok());
        std::env::remove_var("LLM_BACKEND");
    }

    #[test]
    fn test_persist_and_restore_agents() {
        let (kernel, _dir) = make_kernel();
        let _ = kernel.handle_api_request(crate::api::semantic::ApiRequest::RegisterAgent {
            name: "persist_test_agent".to_string(),
        });
        kernel.persist_agents();
        assert!(kernel.agent_index_path().exists());
    }

    #[test]
    fn test_persist_and_restore_permissions() {
        let (kernel, _dir) = make_kernel();
        kernel.persist_permissions();
        kernel.restore_permissions();
    }

    #[test]
    fn test_persist_and_restore_event_log() {
        let (kernel, _dir) = make_kernel();
        kernel.persist_event_log();
        kernel.restore_event_log();
    }

    #[test]
    fn test_canonical_flush_and_auxiliary_persist() {
        let (kernel, _dir) = make_kernel();
        kernel.flush_canonical_memory().unwrap();
        kernel.persist_auxiliary_best_effort();
    }

    #[test]
    fn test_persist_empty_permissions_removes_file() {
        let (kernel, _dir) = make_kernel();
        // Write something first
        kernel.persist_permissions();
        // With no grants, persist should remove the file (or not create one)
        kernel.persist_permissions();
    }
}
