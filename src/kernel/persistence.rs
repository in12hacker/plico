//! Kernel State Persistence — agent/intent/memory/search index persistence and restore.
//!
//! Persists and restores kernel state (agents, intents, memories, search index) to/from
//! CAS and JSON files. Also contains the embedding provider factory functions.

use std::path::PathBuf;
use std::sync::Arc;

use crate::fs::{OllamaBackend, EmbeddingProvider, LocalEmbeddingBackend, StubEmbeddingProvider, EmbedError, InMemoryBackend};
use crate::llm::{LlmProvider, LlmError, OllamaProvider, StubProvider};
use crate::scheduler::Agent;
use crate::scheduler::agent::Intent;

use super::AIKernel;

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
    pub fn persist_memories(&self) -> usize {
        self.memory.persist_all()
    }

    // ─── Agent Persistence ──────────────────────────────────────────────

    pub(crate) fn agent_index_path(&self) -> PathBuf {
        self.root.join("agent_index.json")
    }

    /// Persist all registered agents to a JSON index file.
    pub fn persist_agents(&self) {
        let agents = self.scheduler.snapshot_agents();
        match serde_json::to_string_pretty(&agents) {
            Ok(json) => {
                if let Err(e) = std::fs::write(self.agent_index_path(), json) {
                    tracing::warn!("Failed to persist agent index: {e}");
                }
            }
            Err(e) => tracing::warn!("Failed to serialize agents: {e}"),
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
    }

    // ─── Intent Persistence ─────────────────────────────────────────────

    fn intent_index_path(&self) -> PathBuf {
        self.root.join("intent_index.json")
    }

    /// Persist all pending intents to a JSON index file.
    pub fn persist_intents(&self) {
        let intents = self.scheduler.snapshot_intents();
        match serde_json::to_string_pretty(&intents) {
            Ok(json) => {
                if let Err(e) = std::fs::write(self.intent_index_path(), json) {
                    tracing::warn!("Failed to persist intent index: {e}");
                }
            }
            Err(e) => tracing::warn!("Failed to serialize intents: {e}"),
        }
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

    fn search_index_path(&self) -> PathBuf {
        self.root.join("search_index.jsonl")
    }

    /// Persist the in-memory search index to a JSON Lines file.
    pub fn persist_search_index(&self) {
        let entries = self.search_backend.snapshot();
        if entries.is_empty() {
            return;
        }
        let mut lines = Vec::new();
        for entry in &entries {
            if let Ok(json) = serde_json::to_string(entry) {
                lines.push(json);
            }
        }
        let data = lines.join("\n");
        if let Err(e) = std::fs::write(self.search_index_path(), data) {
            tracing::warn!("Failed to persist search index: {e}");
        } else {
            tracing::info!("Persisted {} search index entries", entries.len());
        }
    }

    /// Restore the search index from a JSON Lines file.
    pub(crate) fn restore_search_index(&self) {
        use std::sync::Arc;
        use crate::fs::InMemoryBackend;
        // SAFETY: search_backend is always InMemoryBackend in practice.
        // This downcast is safe because the kernel always constructs it as such.
        let backend: &InMemoryBackend = unsafe {
            &*Arc::as_ptr(&self.search_backend)
        };
        restore_search_index_into(&self.search_index_path(), backend);
    }
}

/// Restore entries into a search backend from a JSON Lines file at `path`.
///
/// Called BEFORE SemanticFS::new() so the restored embeddings are present
/// before the rebuild-from-CAS step (which uses stub/zero embeddings).
/// The restored real embeddings will NOT be overwritten by stub/zero embeddings
/// because InMemoryBackend::upsert skips overwriting non-stub entries.
pub fn restore_search_index_into(path: &std::path::Path, backend: &InMemoryBackend) {
    if !path.exists() {
        return;
    }
    match std::fs::read_to_string(path) {
        Ok(data) => {
            let entries: Vec<crate::fs::SearchIndexEntry> = data.lines()
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| serde_json::from_str(line).ok())
                .collect();
            let count = entries.len();
            backend.restore(entries);
            if count > 0 {
                tracing::info!("Restored {} search index entries", count);
            }
        }
        Err(e) => tracing::warn!("Failed to read search index: {e}"),
    }
}

/// Create the embedding provider based on EMBEDDING_BACKEND env var.
///
/// Priority: local → ollama → stub
pub(crate) fn create_embedding_provider() -> Result<Arc<dyn EmbeddingProvider>, EmbedError> {
    let backend = std::env::var("EMBEDDING_BACKEND")
        .unwrap_or_else(|_| "local".to_string());

    match backend.as_str() {
        "local" => {
            let model_id = std::env::var("EMBEDDING_MODEL_ID")
                .unwrap_or_else(|_| "BAAI/bge-small-en-v1.5".to_string());
            let python = std::env::var("EMBEDDING_PYTHON")
                .unwrap_or_else(|_| "python3".to_string());
            match LocalEmbeddingBackend::new(&model_id, &python) {
                Ok(b) => {
                    tracing::info!("Embedding backend: local ({})", model_id);
                    Ok(Arc::new(b) as Arc<dyn EmbeddingProvider>)
                }
                Err(EmbedError::SubprocessUnavailable) => {
                    tracing::warn!(
                        "LocalEmbeddingBackend unavailable (python3 not found or pip deps missing). \
                        Install: pip install transformers huggingface_hub onnxruntime"
                    );
                    try_ollama()
                }
                Err(e) => {
                    tracing::warn!("LocalEmbeddingBackend error: {e}. Falling back.");
                    try_ollama()
                }
            }
        }
        "ollama" => try_ollama(),
        "stub" => {
            tracing::info!("Embedding backend: stub (tag-only search)");
            Ok(Arc::new(StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>)
        }
        _ => {
            tracing::warn!("Unknown EMBEDDING_BACKEND={}, trying local", backend);
            try_ollama()
        }
    }
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

    match backend.as_str() {
        "ollama" => {
            let url = std::env::var("OLLAMA_URL")
                .unwrap_or_else(|_| "http://localhost:11434".to_string());
            let model = std::env::var(model_env)
                .unwrap_or_else(|_| default_model.to_string());
            let provider = OllamaProvider::new(&url, &model)?;
            tracing::info!("LLM backend: ollama ({} via {})", model, url);
            Ok(Arc::new(provider) as Arc<dyn LlmProvider>)
        }
        "stub" => {
            tracing::info!("LLM backend: stub");
            Ok(Arc::new(StubProvider::empty()) as Arc<dyn LlmProvider>)
        }
        other => {
            tracing::warn!("Unknown LLM_BACKEND={other}, falling back to ollama");
            let url = std::env::var("OLLAMA_URL")
                .unwrap_or_else(|_| "http://localhost:11434".to_string());
            let model = std::env::var(model_env)
                .unwrap_or_else(|_| default_model.to_string());
            let provider = OllamaProvider::new(&url, &model)?;
            Ok(Arc::new(provider) as Arc<dyn LlmProvider>)
        }
    }
}
