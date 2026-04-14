//! AI Kernel — Orchestrates all Plico subsystems
//!
//! The AI Kernel is the central coordinator. It wires together:
//! - CAS Storage (persistence)
//! - Layered Memory (agent memory management)
//! - Agent Scheduler (intent scheduling)
//! - Semantic FS (file operations)
//! - Permission Guardrails (access control)
//!
//! Upper-layer AI agents interact with the kernel through the semantic API.

use std::path::PathBuf;
use std::sync::Arc;

use crate::cas::{CASStorage, AIObject, AIObjectMeta};
use crate::memory::{LayeredMemory, MemoryEntry, CASPersister, MemoryPersister};
use crate::scheduler::{AgentScheduler, Agent, Intent, IntentPriority, AgentHandle};
use crate::fs::{SemanticFS, Query, OllamaBackend, InMemoryBackend, EmbeddingProvider, SemanticSearch};
use crate::api::permission::{PermissionGuard, PermissionContext, PermissionAction};

/// The AI Kernel — all subsystems wired together.
pub struct AIKernel {
    pub cas: Arc<CASStorage>,
    pub memory: Arc<LayeredMemory>,
    pub scheduler: Arc<AgentScheduler>,
    pub fs: Arc<SemanticFS>,
    pub permissions: Arc<PermissionGuard>,
    /// Memory persister for L1/L2/L3 durability.
    pub memory_persister: Option<Arc<dyn MemoryPersister + Send + Sync>>,
    /// Embedding provider for semantic search.
    pub embedding: Arc<dyn EmbeddingProvider>,
    /// Kernel data root (used to create persister on restart).
    root: PathBuf,
}

impl AIKernel {
    /// Initialize the AI Kernel with the given storage root.
    ///
    /// Uses Ollama at `OLLAMA_URL` env var (default `http://localhost:11434`)
    /// with model `OLLAMA_EMBEDDING_MODEL` env var (default `all-minilm-l6-v2`).
    pub fn new(root: PathBuf) -> std::io::Result<Self> {
        let cas = Arc::new(CASStorage::new(root.join("cas"))?);

        // Create embedding provider — Ollama daemon backend.
        // If Ollama is unavailable, the kernel starts anyway; search falls back
        // to tag-based matching when embedding fails at runtime.
        let ollama_url = std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".to_string());
        let embedding_model = std::env::var("OLLAMA_EMBEDDING_MODEL")
            .unwrap_or_else(|_| "all-minilm-l6-v2".to_string());
        let embedding: Arc<dyn EmbeddingProvider> = match OllamaBackend::new(&ollama_url, &embedding_model) {
            Ok(b) => Arc::new(b),
            Err(e) => {
                tracing::warn!(
                    "Ollama not available at {}: {e}. \
                    Semantic search will fall back to tag matching.",
                    ollama_url
                );
                // Use a stub that returns an error on every embed call
                // (triggers tag-based fallback in SemanticFS::search)
                Arc::new(StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>
            }
        };

        // Create search index — pure Rust in-memory with cosine similarity
        let search_index: Arc<dyn SemanticSearch> = Arc::new(InMemoryBackend::new());

        let memory = Arc::new(LayeredMemory::new());
        let scheduler = Arc::new(AgentScheduler::new());

        let fs = Arc::new(SemanticFS::new(root.clone(), embedding.clone(), search_index)?);
        let permissions = Arc::new(PermissionGuard::new());

        // Create memory persister and attach to memory
        let persister = match CASPersister::new(cas.clone(), root.clone()) {
            Ok(p) => {
                let arc_p: Arc<dyn MemoryPersister + Send + Sync> = Arc::new(p);
                memory.set_persister(arc_p.clone());
                Some(arc_p)
            }
            Err(e) => {
                tracing::warn!("Failed to create memory persister: {e}. Memory will not persist across restarts.");
                None
            }
        };

        let kernel = Self {
            cas,
            memory,
            scheduler,
            fs,
            permissions,
            memory_persister: persister,
            embedding,
            root,
        };

        // Restore persisted memories for all previously known agents
        kernel.restore_memories();

        Ok(kernel)
    }

    /// Restore persisted memories from CAS for all known agents.
    fn restore_memories(&self) {
        if self.memory_persister.is_none() {
            return;
        }

        // Get list of agents that had persisted memories
        // We need to read the index file directly
        if let Ok(json) = std::fs::read_to_string(self.root.join("memory_index.json")) {
            if let Ok(index) = serde_json::from_str::<crate::memory::PersistenceIndex>(&json) {
                for agent_id in index.agents.keys() {
                    if let Err(e) = self.memory.restore_agent(agent_id) {
                        tracing::warn!("Failed to restore memories for agent {}: {}", agent_id, e);
                    }
                }
            }
        }
    }

    /// Persist all in-memory tiers to CAS now.
    /// Called automatically by the memory tier on every N operations,
    /// and can be triggered manually.
    pub fn persist_memories(&self) -> usize {
        self.memory.persist_all()
    }

    // ─── CAS Operations ────────────────────────────────────────────────

    /// Store an object directly in CAS.
    pub fn store_object(
        &self,
        data: Vec<u8>,
        meta: AIObjectMeta,
        agent_id: &str,
    ) -> std::io::Result<String> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        let obj = AIObject::new(data, meta);
        self.cas.put(&obj)
    }

    /// Retrieve an object by CID.
    pub fn get_object(&self, cid: &str, agent_id: &str) -> std::io::Result<AIObject> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Read)?;
        self.cas.get(cid).map_err(|e| std::io::Error::new(std::io::ErrorKind::NotFound, e.to_string()))
    }

    // ─── Semantic FS Operations ────────────────────────────────────────

    /// Create an object with semantic metadata.
    pub fn semantic_create(
        &self,
        content: Vec<u8>,
        tags: Vec<String>,
        agent_id: &str,
        intent: Option<String>,
    ) -> std::io::Result<String> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        self.fs.create(content, tags, agent_id.to_string(), intent)
    }

    /// Semantic search.
    pub fn semantic_search(&self, query: &str, agent_id: &str, limit: usize) -> Vec<crate::fs::SearchResult> {
        let ctx = PermissionContext::new(agent_id.to_string());
        let _ = self.permissions.check(&ctx, PermissionAction::Read);
        self.fs.search(query, limit)
    }

    /// Semantic read.
    pub fn semantic_read(&self, query: &Query, agent_id: &str) -> std::io::Result<Vec<AIObject>> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Read)?;
        self.fs.read(query)
    }

    /// Semantic update.
    pub fn semantic_update(
        &self,
        cid: &str,
        new_content: Vec<u8>,
        new_tags: Option<Vec<String>>,
        agent_id: &str,
    ) -> std::io::Result<String> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        self.fs.update(cid, new_content, new_tags, agent_id.to_string())
    }

    /// Semantic delete (soft delete).
    pub fn semantic_delete(&self, cid: &str, agent_id: &str) -> std::io::Result<()> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Delete)?;
        self.fs.delete(cid, agent_id.to_string())
    }

    // ─── Agent Operations ──────────────────────────────────────────────

    /// Register a new agent.
    pub fn register_agent(&self, name: String) -> String {
        let agent = Agent::new(name);
        let id = agent.id().to_string();
        self.scheduler.register(agent);
        id
    }

    /// List all active agents.
    pub fn list_agents(&self) -> Vec<AgentHandle> {
        self.scheduler.list_agents()
    }

    /// Submit an intent for scheduling.
    pub fn submit_intent(&self, priority: IntentPriority, description: String, agent_id: Option<String>) {
        let mut intent = Intent::new(priority, description);
        if let Some(aid) = agent_id {
            intent = intent.with_agent(crate::scheduler::AgentId(aid));
        }
        self.scheduler.submit(intent);
    }

    // ─── Memory Operations ─────────────────────────────────────────────

    /// Store an ephemeral memory for an agent.
    pub fn remember(&self, agent_id: &str, content: String) {
        let entry = MemoryEntry::ephemeral(agent_id, content);
        self.memory.store(entry);
    }

    /// Retrieve all memories for an agent.
    pub fn recall(&self, agent_id: &str) -> Vec<MemoryEntry> {
        self.memory.get_all(agent_id)
    }

    /// Evict ephemeral memories for an agent.
    pub fn forget_ephemeral(&self, agent_id: &str) {
        self.memory.evict_ephemeral(agent_id);
    }

    /// List all semantic tags in the filesystem.
    pub fn list_tags(&self) -> Vec<String> {
        self.fs.list_tags()
    }
}

// ─── Stub Embedding Provider ─────────────────────────────────────────────────

/// A stub embedding provider used when Ollama is not available.
/// Always returns an error, triggering tag-based fallback in search.
struct StubEmbeddingProvider;

impl StubEmbeddingProvider {
    fn new() -> Self {
        Self
    }
}

impl EmbeddingProvider for StubEmbeddingProvider {
    fn embed(&self, _text: &str) -> Result<Vec<f32>, crate::fs::embedding::EmbedError> {
        Err(crate::fs::embedding::EmbedError::ServerUnavailable(
            "Ollama not configured".to_string(),
        ))
    }

    fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, crate::fs::embedding::EmbedError> {
        Err(crate::fs::embedding::EmbedError::ServerUnavailable(
            "Ollama not configured".to_string(),
        ))
    }

    fn dimension(&self) -> usize {
        384 // Placeholder
    }

    fn model_name(&self) -> &str {
        "stub"
    }
}
