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
use crate::scheduler::dispatch::{TokioDispatchLoop, LocalExecutor, AgentExecutor, DispatchHandle};
use crate::fs::{SemanticFS, Query, OllamaBackend, InMemoryBackend, EmbeddingProvider, SemanticSearch, OllamaSummarizer, Summarizer, KnowledgeGraph, PetgraphBackend, LocalEmbeddingBackend, StubEmbeddingProvider, EmbedError};
use crate::api::permission::{PermissionGuard, PermissionContext, PermissionAction};

/// The AI Kernel — all subsystems wired together.
///
/// All fields are `pub(crate)` — accessible within the `plico` crate (e.g. for
/// integration tests in `tests/`) but not exposed to external crates. External
/// callers must go through the kernel's public methods.
pub struct AIKernel {
    pub(crate) cas: Arc<CASStorage>,
    pub(crate) memory: Arc<LayeredMemory>,
    pub(crate) scheduler: Arc<AgentScheduler>,
    pub(crate) fs: Arc<SemanticFS>,
    pub(crate) permissions: Arc<PermissionGuard>,
    /// Memory persister for L1/L2/L3 durability.
    pub(crate) memory_persister: Option<Arc<dyn MemoryPersister + Send + Sync>>,
    /// Embedding provider for semantic search.
    /// Kept alive here so the Arc doesn't drop while `fs` holds a weak reference.
    #[allow(dead_code)]
    pub(crate) embedding: Arc<dyn EmbeddingProvider>,
    /// Summarizer for L0/L1 context generation.
    /// Kept alive here so the Arc doesn't drop while `fs` holds a weak reference.
    #[allow(dead_code)]
    pub(crate) summarizer: Option<Arc<dyn Summarizer>>,
    /// Knowledge graph for entity/relationship tracking.
    pub(crate) knowledge_graph: Option<Arc<dyn KnowledgeGraph>>,
}

impl AIKernel {
    /// Initialize the AI Kernel with the given storage root.
    ///
    /// Embedding backend priority (set via `EMBEDDING_BACKEND` env):
    ///   "local" (default) — Python subprocess with bge-small-en-v1.5 ONNX model
    ///   "ollama"          — Ollama daemon (OLLAMA_URL / OLLAMA_EMBEDDING_MODEL)
    ///   "stub"            — always returns error (tag-only search)
    ///
    /// For local backend: `pip install transformers huggingface_hub onnxruntime`
    /// Model auto-downloads (~24MB for bge-small-en-v1.5).
    pub fn new(root: PathBuf) -> std::io::Result<Self> {
        let cas = Arc::new(CASStorage::new(root.join("cas"))?);

        let embedding: Arc<dyn EmbeddingProvider> =
            create_embedding_provider().unwrap_or_else(|e| {
                tracing::warn!("Embedding backend failed: {e}. Using stub (tag-only search).");
                Arc::new(StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>
            });

        // Create summarizer — Ollama chat model for L0/L1 summaries.
        // Falls back to heuristic if Ollama is unavailable.
        let ollama_url = std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".to_string());
        let summarizer_model = std::env::var("OLLAMA_SUMMARIZER_MODEL")
            .unwrap_or_else(|_| "llama3.2".to_string());
        let summarizer: Option<Arc<dyn Summarizer>> = match OllamaSummarizer::new(&ollama_url, &summarizer_model) {
            Ok(s) => {
                tracing::info!("LLM summarizer enabled: {} via {}", summarizer_model, ollama_url);
                Some(Arc::new(s))
            }
            Err(e) => {
                tracing::warn!(
                    "Could not create summarizer: {e}. \
                    ContextLoader will use heuristic summaries."
                );
                None
            }
        };

        // Create search index — pure Rust in-memory with cosine similarity
        let search_index: Arc<dyn SemanticSearch> = Arc::new(InMemoryBackend::new());

        // Create knowledge graph — persisted HashMap directed graph
        let knowledge_graph: Option<Arc<dyn KnowledgeGraph>> = {
            let kg: Arc<dyn KnowledgeGraph> = Arc::new(PetgraphBackend::open(root.clone()));
            Some(kg)
        };

        let memory = Arc::new(LayeredMemory::new());
        let scheduler = Arc::new(AgentScheduler::new());

        let fs = Arc::new(SemanticFS::new(
            root.clone(),
            embedding.clone(),
            search_index,
            summarizer.clone(),
            knowledge_graph.clone(),
        )?);
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
            summarizer,
            knowledge_graph,
        };

        // Restore persisted memories for all previously known agents
        kernel.restore_memories();

        Ok(kernel)
    }

    /// Restore persisted memories from CAS for all known agents.
    ///
    /// Uses `MemoryPersister::list_all_agent_ids()` so no direct filesystem access
    /// is needed — satisfies the arch constraint that CAS is the only module touching
    /// the filesystem.
    fn restore_memories(&self) {
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
        // Delegate to SemanticFS so reads go through the same CAS instance as writes
        let results = self.fs.read(&crate::fs::Query::ByCid(cid.to_string()))?;
        results.into_iter().next().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("CID={}", cid))
        })
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

    /// List all logically deleted objects (recycle bin contents).
    pub fn list_deleted(&self, agent_id: &str) -> Vec<crate::fs::RecycleEntry> {
        let ctx = PermissionContext::new(agent_id.to_string());
        let _ = self.permissions.check(&ctx, PermissionAction::Read);
        self.fs.list_deleted()
    }

    /// Restore a deleted object from the recycle bin.
    pub fn restore_deleted(&self, cid: &str, agent_id: &str) -> std::io::Result<()> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        self.fs.restore(cid, agent_id.to_string())
    }

    /// Start the agent dispatch loop as a background tokio task.
    ///
    /// Returns a `DispatchHandle` that can be used to shut down the loop.
    /// This must be called from within a tokio runtime context.
    ///
    /// Binaries should call this instead of importing scheduler types directly.
    pub fn start_dispatch_loop(&self) -> DispatchHandle {
        let executor: Arc<dyn AgentExecutor> = Arc::new(LocalExecutor);
        let loop_ = TokioDispatchLoop::new(Arc::clone(&self.scheduler), executor, 60_000);
        let (_join, handle) = loop_.spawn();
        handle
    }

    /// Explore graph neighbors of a CID, returning plain data (no fs types).
    ///
    /// Returns `Vec<(node_id, label, node_type, edge_type, authority_score)>`.
    /// Designed for use by the API layer without importing fs subsystem types.
    pub fn graph_explore_raw(
        &self,
        cid: &str,
        edge_type_str: Option<&str>,
        depth: u8,
    ) -> Vec<(String, String, String, String, f32)> {
        let edge_filter = edge_type_str.and_then(|s| match s {
            "associates_with" => Some(crate::fs::KGEdgeType::AssociatesWith),
            "mentions"        => Some(crate::fs::KGEdgeType::Mentions),
            "follows"         => Some(crate::fs::KGEdgeType::Follows),
            "part_of"         => Some(crate::fs::KGEdgeType::PartOf),
            "related_to"      => Some(crate::fs::KGEdgeType::RelatedTo),
            _ => None,
        });
        self.graph_explore(cid, edge_filter, depth)
            .into_iter()
            .map(|hit| {
                let node_type = format!("{:?}", hit.node.node_type).to_lowercase();
                let edge_type = hit.edge_type
                    .map(|et| format!("{:?}", et).to_lowercase())
                    .unwrap_or_default();
                (hit.node.id, hit.node.label, node_type, edge_type, hit.authority_score)
            })
            .collect()
    }

    /// Explore graph neighbors of a CID at a given depth.
    pub fn graph_explore(&self, cid: &str, edge_type: Option<crate::fs::KGEdgeType>, depth: u8) -> Vec<crate::fs::KGSearchHit> {
        let Some(ref kg) = self.knowledge_graph else {
            return Vec::new();
        };
        let neighbors = match kg.get_neighbors(cid, edge_type, depth) {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("graph_explore failed for {}: {}", cid, e);
                return Vec::new();
            }
        };
        neighbors
            .into_iter()
            .map(|(node, edge)| crate::fs::KGSearchHit {
                node,
                edge_type: Some(edge.edge_type),
                vector_score: 0.0,
                authority_score: kg.authority_score(cid).unwrap_or(0.0),
                combined_score: 0.0,
            })
            .collect()
    }
}

// ─── Stub Embedding Provider ─────────────────────────────────────────────────

/// Create the embedding provider based on EMBEDDING_BACKEND env var.
///
/// Priority: local → ollama → stub
fn create_embedding_provider() -> Result<Arc<dyn EmbeddingProvider>, crate::fs::embedding::EmbedError> {
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
                    // Fall through to next backend
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
