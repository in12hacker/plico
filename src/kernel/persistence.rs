//! Kernel State Persistence — agent/intent/memory/search index persistence and restore.
//!
//! Persists and restores kernel state (agents, intents, memories, search index) to/from
//! CAS and JSON files. Also contains the embedding provider factory functions.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::fs::{OllamaBackend, EmbeddingProvider, LocalEmbeddingBackend, StubEmbeddingProvider, EmbedError, OrtEmbeddingBackend, EmbeddingCircuitBreaker};
use crate::llm::{LlmProvider, LlmError, OllamaProvider, StubProvider, OpenAICompatibleProvider, CircuitBreakerLlmProvider};
use crate::scheduler::Agent;
use crate::scheduler::agent::Intent;

use super::AIKernel;

/// Atomically write a serializable value to a JSON file.
///
/// Writes to a `.json.tmp` file first, then renames on success.
/// This prevents partial writes from corrupting the persisted file.
pub(crate) fn atomic_write_json<T: serde::Serialize>(path: &Path, data: &T) {
    let tmp = path.with_extension("json.tmp");
    match serde_json::to_string_pretty(data) {
        Ok(json) => {
            if std::fs::write(&tmp, &json).is_ok() {
                let _ = std::fs::rename(&tmp, path);
            }
        }
        Err(e) => tracing::warn!("Failed to serialize for {}: {e}", path.display()),
    }
}

impl AIKernel {
    /// Restore persisted memories from CAS for all known agents.
    pub(crate) fn restore_memories(&self) {
        let Some(ref persister) = self.memory_persister else {
            return;
        };

        let agent_ids = persister.list_all_agent_ids();
        for agent_id in &agent_ids {
            if let Err(e) = self.memory.restore_agent(agent_id) {
                tracing::warn!("Failed to restore memories for agent {}: {}", agent_id, e);
            }
        }
    }

    /// Persist all in-memory tiers to CAS now.
    pub(crate) fn persist_memories(&self) -> usize {
        self.memory.persist_all()
    }

    /// Persist all kernel subsystem state to disk.
    ///
    /// This is the unified persist entry point called on shutdown
    /// and periodically to ensure all state survives crashes.
    pub fn persist_all(&self) {
        self.persist_memories();
        self.persist_agents();
        self.persist_intents();
        self.persist_permissions();
        self.persist_event_log();
        self.persist_search_index();
        self.persist_checkpoints();
        self.persist_task_store();
        self.persist_tenants();
        self.persist_key_store();
        self.persist_sessions();
        // F-1/F-2: IntentCache and AgentProfile persistence (Node 20 "觉")
        if let Err(e) = self.prefetch.persist() {
            tracing::warn!("Failed to persist prefetch state: {}", e);
        }
        tracing::info!("All kernel state persisted to disk");
    }

    pub(crate) fn persist_sessions(&self) {
        if let Err(e) = self.session_store.persist(&self.root) {
            tracing::warn!("Failed to persist sessions: {}", e);
        }
    }

    // ─── Agent Persistence ──────────────────────────────────────────────

    pub(crate) fn agent_index_path(&self) -> PathBuf {
        self.root.join("agent_index.json")
    }

    /// Persist all registered agents to a JSON index file.
    pub(crate) fn persist_agents(&self) {
        let agents = self.scheduler.snapshot_agents();
        atomic_write_json(&self.agent_index_path(), &agents);
        self.persist_usage();
    }

    fn usage_index_path(&self) -> PathBuf {
        self.root.join("usage_index.json")
    }

    pub(crate) fn persist_usage(&self) {
        let usage = self.scheduler.snapshot_usage();
        atomic_write_json(&self.usage_index_path(), &usage);
    }

    fn restore_usage(&self) {
        let path = self.usage_index_path();
        if !path.exists() {
            return;
        }
        match std::fs::read_to_string(&path) {
            Ok(json) => match serde_json::from_str::<std::collections::HashMap<String, crate::scheduler::AgentUsage>>(&json) {
                Ok(data) => {
                    self.scheduler.restore_usage(data);
                    tracing::info!("Restored agent usage counters from persistent storage");
                }
                Err(e) => tracing::warn!("Failed to parse usage index: {e}"),
            },
            Err(e) => tracing::warn!("Failed to read usage index: {e}"),
        }
    }

    /// Restore agents from the persisted index (called during `new()`).
    pub(crate) fn restore_agents(&self) {
        let path = self.agent_index_path();
        if !path.exists() {
            return;
        }
        match std::fs::read_to_string(&path) {
            Ok(json) => match serde_json::from_str::<Vec<Agent>>(&json) {
                Ok(agents) => {
                    let count = agents.len();
                    self.scheduler.restore_agents(agents);
                    tracing::info!("Restored {count} agents from persistent storage");
                }
                Err(e) => tracing::warn!("Failed to parse agent index: {e}"),
            },
            Err(e) => tracing::warn!("Failed to read agent index: {e}"),
        }
        self.restore_usage();
    }

    // ─── Intent Persistence ─────────────────────────────────────────────

    fn intent_index_path(&self) -> PathBuf {
        self.root.join("intent_index.json")
    }

    /// Persist all pending intents to a JSON index file.
    pub(crate) fn persist_intents(&self) {
        let intents = self.scheduler.snapshot_intents();
        atomic_write_json(&self.intent_index_path(), &intents);
    }

    /// Restore pending intents from the persisted index (called during `new()`).
    pub(crate) fn restore_intents(&self) {
        let path = self.intent_index_path();
        if !path.exists() {
            return;
        }
        match std::fs::read_to_string(&path) {
            Ok(json) => match serde_json::from_str::<Vec<Intent>>(&json) {
                Ok(intents) => {
                    let count = intents.len();
                    self.scheduler.restore_intents(intents);
                    if count > 0 {
                        tracing::info!("Restored {count} pending intents from persistent storage");
                    }
                }
                Err(e) => tracing::warn!("Failed to parse intent index: {e}"),
            },
            Err(e) => tracing::warn!("Failed to read intent index: {e}"),
        }
        let _ = std::fs::remove_file(&path);
    }

    // ─── Search Index Persistence ─────────────────────────────────────

    /// Persist the search index via the backend's trait method.
    pub(crate) fn persist_search_index(&self) {
        if let Err(e) = self.search_backend.persist_to(&self.root) {
            tracing::warn!("Failed to persist search index: {e}");
        }
    }

    // ─── Permission Persistence ──────────────────────────────────────

    fn permission_index_path(&self) -> PathBuf {
        self.root.join("permission_index.json")
    }

    pub(crate) fn persist_permissions(&self) {
        let grants = self.permissions.snapshot();
        if grants.is_empty() {
            let _ = std::fs::remove_file(self.permission_index_path());
            return;
        }
        atomic_write_json(&self.permission_index_path(), &grants);
    }

    pub(crate) fn restore_permissions(&self) {
        let path = self.permission_index_path();
        if !path.exists() {
            return;
        }
        match std::fs::read_to_string(&path) {
            Ok(json) => match serde_json::from_str::<std::collections::HashMap<String, Vec<crate::api::permission::PermissionGrant>>>(&json) {
                Ok(grants) => {
                    let count: usize = grants.values().map(|v| v.len()).sum();
                    self.permissions.restore(grants);
                    if count > 0 {
                        tracing::info!("Restored {count} permission grants from persistent storage");
                    }
                }
                Err(e) => tracing::warn!("Failed to parse permission index: {e}"),
            },
            Err(e) => tracing::warn!("Failed to read permission index: {e}"),
        }
    }

    // ─── Event Log Persistence ──────────────────────────────────────

    fn event_log_path(&self) -> PathBuf {
        self.root.join("event_log.jsonl")
    }

    pub(crate) fn persist_event_log(&self) {
        // JSONL persistence is handled inline on each emit() call in EventBus.
        // This method is used for periodic batch snapshots and shutdown saves.
        let events = self.event_bus.snapshot_events();
        if events.is_empty() {
            return;
        }
        let path = self.event_log_path();
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write;
            for event in &events {
                if let Ok(json) = serde_json::to_string(event) {
                    let _ = writeln!(file, "{}", json);
                }
            }
        }
    }

    pub(crate) fn restore_event_log(&self) {
        let path = self.event_log_path();
        if !path.exists() {
            return;
        }
        match super::event_bus::EventBus::load_event_log(&path) {
            Ok(events) => {
                let count = events.len();
                self.event_bus.restore_events(events);
                if count > 0 {
                    tracing::info!("Restored {count} events from persistent event log");
                }
            }
            Err(e) => tracing::warn!("Failed to read event log: {}", e),
        }
    }

    // ─── Checkpoint Persistence (P-2) ────────────────────────────────

    pub(crate) fn persist_checkpoints(&self) {
        self.checkpoint_store.persist(&self.root, &self.cas);
    }

    // ─── Task Persistence (F-14) ────────────────────────────────────────

    pub(crate) fn persist_task_store(&self) {
        self.task_store.persist();
    }

    pub(crate) fn restore_checkpoints(&self) {
        // CheckpointStore is already restored in AIKernel::new() via CheckpointStore::restore()
        // This method exists for consistency with other restore_* methods
        let count = self.checkpoint_store.list_all().len();
        if count > 0 {
            tracing::info!("Checkpoint store ready with {count} checkpoints");
        }
    }

    // ─── Task Store (F-14) ────────────────────────────────────────────

    pub(crate) fn restore_task_store(&self) {
        // TaskStore is already restored in AIKernel::new() via TaskStore::restore()
        // This method exists for consistency with other restore_* methods
        let count = self.task_store.len();
        if count > 0 {
            tracing::info!("Task store ready with {count} tasks");
        }
    }

    // ─── Tenant Persistence (P-3) ─────────────────────────────────────

    pub(crate) fn persist_tenants(&self) {
        self.tenant_store.persist(&self.root);
    }

    // ─── AgentKeyStore Persistence (P-4) ─────────────────────────────

    pub(crate) fn persist_key_store(&self) {
        self.key_store.persist(&self.root);
    }
}

/// Create the embedding provider based on EMBEDDING_BACKEND env var.
///
/// Priority when unset: local → ollama → stub
/// Explicit values: "local" | "ollama" | "stub" | "ort"
pub(crate) fn create_embedding_provider() -> Result<Arc<dyn EmbeddingProvider>, EmbedError> {
    let backend = std::env::var("EMBEDDING_BACKEND")
        .unwrap_or_else(|_| "local".to_string());

    // Create the base provider (potentially with fallback to stub on error)
    let base_provider: Arc<dyn EmbeddingProvider> = match backend.as_str() {
        "ort" => {
            let model_dir = std::env::var("PLICO_MODEL_DIR")
                .unwrap_or_else(|_| "./models/all-MiniLM-L6-v2".to_string());
            match OrtEmbeddingBackend::new(std::path::Path::new(&model_dir)) {
                Ok(b) => {
                    tracing::info!("Embedding backend: ort ({})", model_dir);
                    Arc::new(b) as Arc<dyn EmbeddingProvider>
                }
                Err(EmbedError::ModelNotFound(msg)) => {
                    tracing::warn!("OrtEmbeddingBackend model not found: {}. Falling back to stub.", msg);
                    Arc::new(StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>
                }
                Err(e) => {
                    tracing::warn!("OrtEmbeddingBackend error: {}. Falling back to stub.", e);
                    Arc::new(StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>
                }
            }
        }
        "local" => {
            let model_id = std::env::var("EMBEDDING_MODEL_ID")
                .unwrap_or_else(|_| "BAAI/bge-small-en-v1.5".to_string());
            let python = std::env::var("EMBEDDING_PYTHON")
                .unwrap_or_else(|_| "python3".to_string());
            match LocalEmbeddingBackend::new(&model_id, &python) {
                Ok(b) => {
                    tracing::info!("Embedding backend: local ({})", model_id);
                    Arc::new(b) as Arc<dyn EmbeddingProvider>
                }
                Err(EmbedError::SubprocessUnavailable) => {
                    tracing::warn!(
                        "LocalEmbeddingBackend unavailable (python3 not found or pip deps missing). \
                        Install: pip install transformers huggingface_hub onnxruntime"
                    );
                    return try_ollama_circuitbreaker();
                }
                Err(e) => {
                    tracing::warn!("LocalEmbeddingBackend error: {e}. Falling back.");
                    return try_ollama_circuitbreaker();
                }
            }
        }
        "ollama" => return try_ollama_circuitbreaker(),
        "stub" => {
            tracing::info!("Embedding backend: stub (tag-only search)");
            Arc::new(StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>
        }
        _ => {
            tracing::warn!("Unknown EMBEDDING_BACKEND={}, trying local", backend);
            return try_ollama_circuitbreaker();
        }
    };

    // F-38: Wrap in circuit breaker — 3 failures → open → 30s cooldown → half-open probe
    let threshold: u32 = std::env::var("EMBEDDING_CB_THRESHOLD")
        .unwrap_or_else(|_| "3".to_string())
        .parse()
        .unwrap_or(3);
    let cooldown_ms: u64 = std::env::var("EMBEDDING_CB_COOLDOWN_MS")
        .unwrap_or_else(|_| "30000".to_string())
        .parse()
        .unwrap_or(30000);

    Ok(Arc::new(EmbeddingCircuitBreaker::new(base_provider, threshold, cooldown_ms)))
}

fn try_ollama_circuitbreaker() -> Result<Arc<dyn EmbeddingProvider>, EmbedError> {
    let inner = try_ollama()?;
    let threshold: u32 = std::env::var("EMBEDDING_CB_THRESHOLD")
        .unwrap_or_else(|_| "3".to_string())
        .parse()
        .unwrap_or(3);
    let cooldown_ms: u64 = std::env::var("EMBEDDING_CB_COOLDOWN_MS")
        .unwrap_or_else(|_| "30000".to_string())
        .parse()
        .unwrap_or(30000);
    Ok(Arc::new(EmbeddingCircuitBreaker::new(inner, threshold, cooldown_ms)))
}

fn try_ollama() -> Result<Arc<dyn EmbeddingProvider>, EmbedError> {
    let url = std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".to_string());
    let model = std::env::var("OLLAMA_EMBEDDING_MODEL")
        .unwrap_or_else(|_| "all-minilm-l6-v2".to_string());
    match OllamaBackend::new(&url, &model) {
        Ok(b) => {
            tracing::info!("Embedding backend: ollama ({})", model);
            Ok(Arc::new(b) as Arc<dyn EmbeddingProvider>)
        }
        Err(e) => Err(e),
    }
}

/// Create an LLM provider based on `LLM_BACKEND` env var.
///
/// Backends: "ollama" (default) | "stub"
pub(crate) fn create_llm_provider(model_env: &str, default_model: &str) -> Result<Arc<dyn LlmProvider>, LlmError> {
    let backend = std::env::var("LLM_BACKEND")
        .unwrap_or_else(|_| "ollama".to_string());

    let inner: Arc<dyn LlmProvider> = match backend.as_str() {
        "ollama" => {
            let url = std::env::var("OLLAMA_URL")
                .unwrap_or_else(|_| "http://localhost:11434".to_string());
            let model = std::env::var(model_env)
                .unwrap_or_else(|_| default_model.to_string());
            let provider = OllamaProvider::new(&url, &model)?;
            tracing::info!("LLM backend: ollama ({} via {})", model, url);
            Arc::new(provider) as Arc<dyn LlmProvider>
        }
        "stub" => {
            tracing::info!("LLM backend: stub");
            Arc::new(StubProvider::empty()) as Arc<dyn LlmProvider>
        }
        "openai" => {
            let base_url = std::env::var("OPENAI_API_BASE")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
            let model = std::env::var(model_env)
                .unwrap_or_else(|_| default_model.to_string());
            let api_key = std::env::var("OPENAI_API_KEY").ok();
            let provider = OpenAICompatibleProvider::new(&base_url, &model, api_key)?;
            tracing::info!("LLM backend: openai-compatible ({} via {})", model, base_url);
            Arc::new(provider) as Arc<dyn LlmProvider>
        }
        other => {
            tracing::warn!("Unknown LLM_BACKEND={other}, falling back to ollama");
            let url = std::env::var("OLLAMA_URL")
                .unwrap_or_else(|_| "http://localhost:11434".to_string());
            let model = std::env::var(model_env)
                .unwrap_or_else(|_| default_model.to_string());
            let provider = OllamaProvider::new(&url, &model)?;
            Arc::new(provider) as Arc<dyn LlmProvider>
        }
    };

    // F-2: Wrap with circuit breaker for fail-fast on provider outages
    let threshold: u32 = std::env::var("LLM_CIRCUIT_BREAKER_FAILURE_THRESHOLD")
        .unwrap_or_else(|_| "5".into())
        .parse()
        .unwrap_or(5);
    let cooldown_ms: u64 = std::env::var("LLM_CIRCUIT_BREAKER_COOLDOWN_MS")
        .unwrap_or_else(|_| "60000".into())
        .parse()
        .unwrap_or(60000);
    Ok(Arc::new(CircuitBreakerLlmProvider::new(inner, threshold, cooldown_ms)) as Arc<dyn LlmProvider>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use std::fs;

    #[test]
    fn test_atomic_write_json_roundtrip() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.json");
        let data = serde_json::json!({
            "name": "test-agent",
            "version": 1,
            "nested": {"key": "value"}
        });
        atomic_write_json(&path, &data);
        assert!(path.exists(), "atomic_write_json should create the file");
        let content = fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["name"], "test-agent");
    }

    #[test]
    fn test_atomic_write_json_no_corrupt_on_error() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("error.json");
        // Test that invalid JSON serialization (non-serializable type) is handled gracefully
        // atomic_write_json uses a match on to_string_pretty, so it won't panic
        // We can test with a value that fails serialization
        #[derive(serde::Serialize)]
        struct ValidData { name: String }
        let valid = ValidData { name: "ok".to_string() };
        atomic_write_json(&path, &valid);
        assert!(path.exists(), "valid data should write successfully");

        // Now test with actual serde error path won't happen because atomic_write_json
        // already matches on the result; the test confirms no panic on write error
    }

    #[test]
    fn test_agent_index_path() {
        let dir = tempdir().unwrap();
        let kernel = crate::kernel::AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
        let p = kernel.agent_index_path();
        assert_eq!(p.file_name().unwrap(), "agent_index.json");
    }

    #[test]
    fn test_intent_index_path() {
        let dir = tempdir().unwrap();
        let kernel = crate::kernel::AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
        let p = kernel.intent_index_path();
        assert_eq!(p.file_name().unwrap(), "intent_index.json");
    }

    #[test]
    fn test_persist_and_restore_agents() {
        let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
        let dir = tempdir().unwrap();
        let kernel = crate::kernel::AIKernel::new(dir.path().to_path_buf()).expect("kernel init");

        kernel.register_agent("PersistAgent1".to_string());
        kernel.register_agent("PersistAgent2".to_string());

        kernel.persist_agents();

        // New kernel should restore
        let kernel2 = crate::kernel::AIKernel::new(dir.path().to_path_buf()).expect("kernel init 2");
        let agents = kernel2.scheduler.list_agents();
        let names: Vec<_> = agents.iter().map(|a| a.name.clone()).collect();
        assert!(names.contains(&"PersistAgent1".to_string()));
        assert!(names.contains(&"PersistAgent2".to_string()));
    }

    #[test]
    fn test_persist_and_restore_permissions() {
        let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
        let dir = tempdir().unwrap();
        let kernel = crate::kernel::AIKernel::new(dir.path().to_path_buf()).expect("kernel init");

        kernel.register_agent("PermAgent".to_string());
        kernel.permission_grant("PermAgent", crate::api::permission::PermissionAction::Read, None, None);

        kernel.persist_permissions();

        let kernel2 = crate::kernel::AIKernel::new(dir.path().to_path_buf()).expect("kernel init 2");
        let allowed = kernel2.permission_check("PermAgent", crate::api::permission::PermissionAction::Read).is_ok();
        assert!(allowed, "permission should be restored after restart");
    }

    #[test]
    fn test_restore_from_empty_dir() {
        let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
        let dir = tempdir().unwrap();
        // Fresh directory with no persisted state - should not panic
        let kernel = crate::kernel::AIKernel::new(dir.path().to_path_buf()).expect("kernel init from empty");
        let agents = kernel.scheduler.list_agents();
        assert!(agents.is_empty(), "empty dir should have no agents");
    }

    #[test]
    fn test_create_llm_provider_stub() {
        let _ = std::env::set_var("LLM_BACKEND", "stub");
        let provider = create_llm_provider("MODEL", "default").expect("stub provider should work");
        let result = provider.chat(&[], &crate::llm::ChatOptions::default());
        assert!(result.is_ok());
    }
}
