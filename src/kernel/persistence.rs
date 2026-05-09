//! Kernel State Persistence — agent/intent/memory/search index persistence and restore.
//!
//! Persists and restores kernel state (agents, intents, memories, search index) to/from
//! CAS and JSON files. Also contains the embedding provider factory functions.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::fs::{OllamaBackend, OpenAIEmbeddingBackend, EmbeddingProvider, LocalEmbeddingBackend, StubEmbeddingProvider, EmbedError, OrtEmbeddingBackend, EmbeddingCircuitBreaker, AdaptiveEmbeddingProvider};
use crate::llm::{LlmProvider, LlmError, OllamaProvider, StubProvider, OpenAICompatibleProvider, CircuitBreakerLlmProvider};

use super::AIKernel;

/// Resolve llama.cpp server URL via unified config.
pub(crate) fn resolve_llama_url() -> String {
    crate::config::PlicoConfig::load(None).resolve_llama_url()
}

pub(crate) fn ensure_v1_suffix(url: &str) -> String {
    crate::config::ensure_v1_suffix(url)
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
    if std::fs::rename(&tmp, path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    Ok(())
}

impl AIKernel {
    pub(crate) fn restore_memories(&self) {
        if let Some(ref persister) = self.memory_persister {
            for id in persister.list_all_agent_ids() {
                let _ = self.memory.restore_agent(&id);
            }
        }
    }

    pub(crate) fn persist_memories(&self) -> usize { self.memory.persist_all() }

    pub fn persist_all(&self) {
        self.persist_memories();
        self.persist_agents();
        self.persist_intents();
        self.persist_permissions();
        self.persist_event_log();
        self.persist_search_index();
        self.fs.flush_tag_index();
        self.persist_checkpoints();
        self.persist_task_store();
        self.persist_tenants();
        self.persist_key_store();
        self.persist_sessions();
        let _ = self.prefetch.persist();
        let _ = self.cost_ledger.persist_to_dir(&self.root.join("prefetch"));
        tracing::info!("All kernel state persisted to disk");
    }

    pub(crate) fn persist_sessions(&self) { let _ = self.session_store.persist(&self.root); }
    pub(crate) fn agent_index_path(&self) -> PathBuf { self.root.join("agent_index.json") }
    pub(crate) fn persist_agents(&self) {
        atomic_write_json(&self.agent_index_path(), &self.scheduler.snapshot_agents());
        self.persist_usage();
    }
    fn usage_index_path(&self) -> PathBuf { self.root.join("usage_index.json") }
    pub(crate) fn persist_usage(&self) { atomic_write_json(&self.usage_index_path(), &self.scheduler.snapshot_usage()); }

    fn restore_usage(&self) {
        let path = self.usage_index_path();
        if let Ok(json) = std::fs::read_to_string(&path) {
            if let Ok(data) = serde_json::from_str(&json) { self.scheduler.restore_usage(data); }
        }
    }

    pub(crate) fn restore_agents(&self) {
        let path = self.agent_index_path();
        if let Ok(json) = std::fs::read_to_string(&path) {
            if let Ok(agents) = serde_json::from_str(&json) { self.scheduler.restore_agents(agents); }
        }
        self.restore_usage();
    }

    fn intent_index_path(&self) -> PathBuf { self.root.join("intent_index.json") }
    pub(crate) fn persist_intents(&self) { atomic_write_json(&self.intent_index_path(), &self.scheduler.snapshot_intents()); }
    pub(crate) fn restore_intents(&self) {
        let path = self.intent_index_path();
        if let Ok(json) = std::fs::read_to_string(&path) {
            if let Ok(intents) = serde_json::from_str(&json) { self.scheduler.restore_intents(intents); }
        }
        let _ = std::fs::remove_file(&path);
    }

    pub(crate) fn persist_search_index(&self) { let _ = self.search_backend.persist_to(&self.root); }

    fn permission_index_path(&self) -> PathBuf { self.root.join("permission_index.json") }
    pub(crate) fn persist_permissions(&self) {
        let grants = self.permissions.snapshot();
        if grants.is_empty() { let _ = std::fs::remove_file(self.permission_index_path()); }
        else { atomic_write_json(&self.permission_index_path(), &grants); }
    }
    pub(crate) fn restore_permissions(&self) {
        let path = self.permission_index_path();
        if let Ok(json) = std::fs::read_to_string(&path) {
            if let Ok(grants) = serde_json::from_str(&json) { self.permissions.restore(grants); }
        }
    }

    pub(crate) fn persist_event_log(&self) {
        let events = self.event_bus.snapshot_events();
        if events.is_empty() { return; }
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).write(true).truncate(true).open(self.root.join("event_log.jsonl")) {
            use std::io::Write;
            for e in &events { if let Ok(json) = serde_json::to_string(e) { let _ = writeln!(file, "{}", json); } }
        }
    }
    pub(crate) fn restore_event_log(&self) {
        if let Ok(events) = super::event_bus::EventBus::load_event_log(&self.root.join("event_log.jsonl")) {
            self.event_bus.restore_events(events);
        }
    }

    pub(crate) fn persist_checkpoints(&self) { self.checkpoint_store.persist(&self.root, &self.cas); }
    pub(crate) fn persist_task_store(&self) { self.task_store.persist(); }
    pub(crate) fn restore_checkpoints(&self) {}
    pub(crate) fn restore_task_store(&self) {}
    pub(crate) fn persist_tenants(&self) { self.tenant_store.persist(&self.root); }
    pub(crate) fn persist_key_store(&self) { self.key_store.persist(&self.root); }
}

fn read_circuit_breaker_config(t_env: &str, c_env: &str, t_def: u32, c_def: u64) -> (u32, u64) {
    let t = std::env::var(t_env).ok().and_then(|v| v.parse().ok()).unwrap_or(t_def);
    let c = std::env::var(c_env).ok().and_then(|v| v.parse().ok()).unwrap_or(c_def);
    (t, c)
}

pub(crate) fn create_embedding_provider(config: &crate::config::InferenceConfig) -> Result<Arc<dyn EmbeddingProvider>, EmbedError> {
    let backend = &config.embedding_backend;
    let base_provider: Arc<dyn EmbeddingProvider> = match backend.as_str() {
        "ort" => {
            let model_dir = std::env::var("PLICO_MODEL_DIR").unwrap_or_else(|_| "./models/all-MiniLM-L6-v2".to_string());
            match OrtEmbeddingBackend::new(std::path::Path::new(&model_dir)) {
                Ok(b) => Arc::new(b) as Arc<dyn EmbeddingProvider>,
                Err(_) => Arc::new(StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>,
            }
        }
        "local" => {
            let model_id = config.embedding_model_id.clone().unwrap_or_else(|| "BAAI/bge-small-en-v1.5".to_string());
            let python = config.embedding_python.clone().unwrap_or_else(|| "python3".to_string());
            match LocalEmbeddingBackend::new(&model_id, &python) {
                Ok(b) => Arc::new(b) as Arc<dyn EmbeddingProvider>,
                Err(_) => try_ollama_circuitbreaker(),
            }
        }
        "openai" => {
            let base_url = config.embedding_api_base.clone().map(|u| crate::config::ensure_v1_suffix(&u)).unwrap_or_else(|| {
                if let Some(port) = crate::config::detect_llama_server_port() { format!("http://127.0.0.1:{port}/v1") } else { "http://127.0.0.1:8080/v1".into() }
            });
            let model = config.embedding_model.clone().unwrap_or_else(|| "all-MiniLM-L6-v2".to_string());
            match OpenAIEmbeddingBackend::new(&base_url, &model, config.api_key.clone()) {
                Ok(b) => Arc::new(b) as Arc<dyn EmbeddingProvider>,
                Err(_) => Arc::new(StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>,
            }
        }
        "stub" => Arc::new(StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>,
        _ => try_ollama_circuitbreaker(),
    };

    let (threshold, cooldown_ms) = read_circuit_breaker_config("EMBEDDING_CB_THRESHOLD", "EMBEDDING_CB_COOLDOWN_MS", 3, 30_000);
    let with_cb = Arc::new(EmbeddingCircuitBreaker::new(base_provider, threshold, cooldown_ms));
    let adaptive = AdaptiveEmbeddingProvider::from_config(with_cb as Arc<dyn EmbeddingProvider>, config);
    Ok(Arc::new(adaptive) as Arc<dyn EmbeddingProvider>)
}

fn try_ollama_circuitbreaker() -> Arc<dyn EmbeddingProvider> {
    match try_ollama() {
        Ok(inner) => {
            let with_cb = Arc::new(EmbeddingCircuitBreaker::new(inner, 3, 30_000));
            Arc::new(AdaptiveEmbeddingProvider::from_env(with_cb as Arc<dyn EmbeddingProvider>)) as Arc<dyn EmbeddingProvider>
        }
        Err(_) => Arc::new(StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>,
    }
}

fn try_ollama() -> Result<Arc<dyn EmbeddingProvider>, EmbedError> {
    let url = std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".to_string());
    let model = std::env::var("OLLAMA_EMBEDDING_MODEL").unwrap_or_else(|_| "all-minilm-l6-v2".to_string());
    match OllamaBackend::new(&url, &model) { Ok(b) => Ok(Arc::new(b) as Arc<dyn EmbeddingProvider>), Err(e) => Err(e) }
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
            Arc::new(OpenAICompatibleProvider::new(&base_url, &model, std::env::var("OPENAI_API_KEY").ok())?) as Arc<dyn LlmProvider>
        }
        "llama" => {
            let base_url = crate::config::PlicoConfig::load(None).resolve_llama_url();
            let model = std::env::var("LLAMA_MODEL").or_else(|_| std::env::var(model_env)).unwrap_or_else(|_| default_model.to_string());
            Arc::new(OpenAICompatibleProvider::new(&base_url, &model, None)?) as Arc<dyn LlmProvider>
        }
        _ => Arc::new(StubProvider::empty()) as Arc<dyn LlmProvider>,
    };
    Ok(Arc::new(CircuitBreakerLlmProvider::new(inner, 5, 60_000)) as Arc<dyn LlmProvider>)
}
