//! AI Kernel — central orchestrator for all Plico subsystems.
//!
//! Wires together: CAS Storage, Layered Memory, Agent Scheduler,
//! Semantic FS, and Permission Guardrails. Upper-layer AI agents
//! interact with the kernel through the semantic API.
//!
//! # Errors
//! Returns `std::io::Error` for initialization; subsystem errors propagated through API layer.
//!
//! # Public API
//! - [`AIKernel`]: the main orchestrator — `new()`, `start_dispatch_loop()`,
//!   `graph_explore_raw()`, `list_deleted()`, `restore_deleted()`, `dashboard_status()`
//!
//! # Module Structure
//! - `mod.rs` — struct definition, initialization, core operations
//! - `builtin_tools.rs` — tool registration and execution dispatch
//! - `persistence.rs` — state persistence/restore + embedding provider factory

mod builtin_tools;
mod persistence;

use std::path::PathBuf;
use std::sync::Arc;

use crate::cas::{CASStorage, AIObject, AIObjectMeta};
use crate::memory::{LayeredMemory, MemoryEntry, CASPersister, MemoryPersister};
use crate::scheduler::{AgentScheduler, Agent, AgentResources, Intent, IntentPriority, AgentHandle};
use crate::scheduler::dispatch::{TokioDispatchLoop, LocalExecutor, KernelExecutor, AgentExecutor, DispatchHandle};
use crate::scheduler::messaging::MessageBus;
use crate::fs::{SemanticFS, Query, InMemoryBackend, EmbeddingProvider, SemanticSearch, OllamaSummarizer, Summarizer, KnowledgeGraph, PetgraphBackend, StubEmbeddingProvider, EventType, EventRelation, EventSummary, KGNode, KGNodeType, KGEdgeType, KGEdge};
use crate::temporal::{TemporalResolver, RULE_BASED_RESOLVER};
use crate::api::permission::{PermissionGuard, PermissionContext, PermissionAction};
use crate::tool::ToolRegistry;
use crate::intent::{ChainRouter, IntentRouter, ResolvedIntent};

/// The AI Kernel — all subsystems wired together.
///
/// All fields are `pub(crate)` — accessible within the `plico` crate (e.g. for
/// integration tests in `tests/`) but not exposed to external crates. External
/// callers must go through the kernel's public methods.
pub struct AIKernel {
    pub(crate) root: PathBuf,
    pub(crate) cas: Arc<CASStorage>,
    pub(crate) memory: Arc<LayeredMemory>,
    pub(crate) scheduler: Arc<AgentScheduler>,
    pub(crate) fs: Arc<SemanticFS>,
    pub(crate) permissions: Arc<PermissionGuard>,
    /// Memory persister for L1/L2/L3 durability.
    pub(crate) memory_persister: Option<Arc<dyn MemoryPersister + Send + Sync>>,
    /// Embedding provider for semantic search.
    #[allow(dead_code)]
    pub(crate) embedding: Arc<dyn EmbeddingProvider>,
    /// Summarizer for L0/L1 context generation.
    #[allow(dead_code)]
    pub(crate) summarizer: Option<Arc<dyn Summarizer>>,
    /// Knowledge graph for entity/relationship tracking.
    pub(crate) knowledge_graph: Option<Arc<dyn KnowledgeGraph>>,
    /// Concrete search index reference for snapshot/restore (shared with SemanticFS).
    pub(crate) search_backend: Arc<InMemoryBackend>,
    /// Tool registry — "Everything is a Tool" capability catalog.
    pub(crate) tool_registry: Arc<ToolRegistry>,
    /// Intent router — NL → ApiRequest translation.
    pub(crate) intent_router: Arc<dyn IntentRouter>,
    /// Message bus — agent-to-agent communication.
    pub(crate) message_bus: Arc<MessageBus>,
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
            persistence::create_embedding_provider().unwrap_or_else(|e| {
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
        let search_backend = Arc::new(InMemoryBackend::new());
        let search_index: Arc<dyn SemanticSearch> = search_backend.clone();

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

        let tool_registry = Arc::new(ToolRegistry::new());
        let message_bus = Arc::new(MessageBus::new());

        // Build intent router: heuristic always available, LLM optional.
        let llm_router = {
            let ollama_url = std::env::var("OLLAMA_URL")
                .unwrap_or_else(|_| "http://localhost:11434".to_string());
            let intent_model = std::env::var("OLLAMA_INTENT_MODEL")
                .unwrap_or_else(|_| "llama3.2".to_string());
            match crate::intent::llm::LlmRouter::new(&ollama_url, &intent_model, Vec::new()) {
                r => {
                    tracing::info!("Intent LLM router configured: {} via {}", intent_model, ollama_url);
                    Some(r)
                }
            }
        };
        let intent_router: Arc<dyn IntentRouter> = Arc::new(ChainRouter::new(llm_router));

        let kernel = Self {
            root: root.clone(),
            cas,
            memory,
            scheduler,
            fs,
            permissions,
            memory_persister: persister,
            embedding,
            summarizer,
            knowledge_graph,
            search_backend,
            tool_registry,
            intent_router,
            message_bus,
        };

        kernel.register_builtin_tools();

        // Restore persisted state from prior sessions
        kernel.restore_agents();
        kernel.restore_intents();
        kernel.restore_memories();
        kernel.restore_search_index();

        Ok(kernel)
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
        let results = self.fs.read(&crate::fs::Query::ByCid(cid.to_string()))?;
        let obj = results.into_iter().next().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("CID={}", cid))
        })?;
        self.permissions.check_ownership(agent_id, &obj.meta.created_by)?;
        Ok(obj)
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

    /// Semantic search with optional tag filtering.
    pub fn semantic_search(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
        require_tags: Vec<String>,
        exclude_tags: Vec<String>,
    ) -> Vec<crate::fs::SearchResult> {
        self.semantic_search_with_time(query, agent_id, limit, require_tags, exclude_tags, None, None)
    }

    /// Semantic search with time-range bounds.
    ///
    /// `since` / `until` — Unix milliseconds. Both optional; None means unbounded.
    /// When only one is provided, the other is left open.
    pub fn semantic_search_with_time(
        &self,
        query: &str,
        agent_id: &str,
        limit: usize,
        require_tags: Vec<String>,
        exclude_tags: Vec<String>,
        since: Option<i64>,
        until: Option<i64>,
    ) -> Vec<crate::fs::SearchResult> {
        let ctx = PermissionContext::new(agent_id.to_string());
        let _ = self.permissions.check(&ctx, PermissionAction::Read);
        let can_read_any = self.permissions.can_read_any(agent_id);

        let filter = crate::fs::SearchFilter {
            require_tags,
            exclude_tags,
            content_type: None,
            since,
            until,
        };

        let results = self.fs.search_with_filter(query, limit * 2, filter);
        if can_read_any {
            results.into_iter().take(limit).collect()
        } else {
            results.into_iter()
                .filter(|r| r.meta.created_by == agent_id)
                .take(limit)
                .collect()
        }
    }

    /// Semantic read with ownership isolation.
    pub fn semantic_read(&self, query: &Query, agent_id: &str) -> std::io::Result<Vec<AIObject>> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Read)?;
        let results = self.fs.read(query)?;
        if self.permissions.can_read_any(agent_id) {
            Ok(results)
        } else {
            Ok(results.into_iter()
                .filter(|obj| obj.meta.created_by == agent_id)
                .collect())
        }
    }

    /// Semantic update — only owner or trusted can update.
    pub fn semantic_update(
        &self,
        cid: &str,
        new_content: Vec<u8>,
        new_tags: Option<Vec<String>>,
        agent_id: &str,
    ) -> std::io::Result<String> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        if let Ok(obj) = self.fs.read(&crate::fs::Query::ByCid(cid.to_string())) {
            if let Some(existing) = obj.first() {
                self.permissions.check_ownership(agent_id, &existing.meta.created_by)?;
            }
        }
        self.fs.update(cid, new_content, new_tags, agent_id.to_string())
    }

    /// Semantic delete (soft delete) — only owner or trusted can delete.
    pub fn semantic_delete(&self, cid: &str, agent_id: &str) -> std::io::Result<()> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Delete)?;
        if let Ok(obj) = self.fs.read(&crate::fs::Query::ByCid(cid.to_string())) {
            if let Some(existing) = obj.first() {
                self.permissions.check_ownership(agent_id, &existing.meta.created_by)?;
            }
        }
        self.fs.delete(cid, agent_id.to_string())
    }

    // ─── Agent Operations ──────────────────────────────────────────────

    /// Register a new agent.
    pub fn register_agent(&self, name: String) -> String {
        let agent = Agent::new(name);
        let id = agent.id().to_string();
        self.scheduler.register(agent);
        self.persist_agents();
        id
    }

    /// List all active agents.
    pub fn list_agents(&self) -> Vec<AgentHandle> {
        self.scheduler.list_agents()
    }

    /// Number of pending intents in the scheduler queue.
    pub fn pending_intent_count(&self) -> usize {
        self.scheduler.snapshot_intents().len()
    }

    /// Submit an intent for scheduling.
    ///
    /// `action` — optional JSON-encoded ApiRequest. When present, the
    /// KernelExecutor will deserialize and execute it. Without an action,
    /// the intent is descriptive only (acknowledged but not dispatched).
    pub fn submit_intent(
        &self,
        priority: IntentPriority,
        description: String,
        action: Option<String>,
        agent_id: Option<String>,
    ) -> String {
        let mut intent = Intent::new(priority, description);
        if let Some(a) = action {
            intent = intent.with_action(a);
        }
        if let Some(aid) = agent_id {
            intent = intent.with_agent(crate::scheduler::AgentId(aid));
        }
        let id = intent.id.0.clone();
        self.scheduler.submit(intent);
        self.persist_intents();
        id
    }

    /// Get agent status: state + pending intent count.
    pub fn agent_status(&self, agent_id: &str) -> Option<(String, String, usize)> {
        use crate::scheduler::AgentId;
        let agent = self.scheduler.get(&AgentId(agent_id.to_string()))?;
        let pending = self.scheduler.snapshot_intents()
            .iter()
            .filter(|i| i.agent_id.as_ref().map(|a| a.0.as_str()) == Some(agent_id))
            .count();
        Some((agent.id().to_string(), format!("{:?}", agent.state()), pending))
    }

    /// Suspend an agent (pause execution) with automatic context snapshot.
    pub fn agent_suspend(&self, agent_id: &str) -> std::io::Result<()> {
        use crate::scheduler::{AgentId, AgentState};
        use crate::memory::context_snapshot::ContextSnapshot;

        let aid = AgentId(agent_id.to_string());
        let agent = self.scheduler.get(&aid).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Agent not found: {}", agent_id))
        })?;
        if agent.state().is_terminal() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Agent {} is in terminal state {:?}", agent_id, agent.state()),
            ));
        }

        let state_before = format!("{:?}", agent.state());
        let memories = self.memory.get_all(agent_id);
        let pending = self.scheduler.snapshot_intents()
            .iter()
            .filter(|i| i.agent_id.as_ref().map(|a| a.0.as_str()) == Some(agent_id))
            .count();
        let last_intent = self.scheduler.snapshot_intents()
            .iter()
            .filter(|i| i.agent_id.as_ref().map(|a| a.0.as_str()) == Some(agent_id))
            .last()
            .map(|i| i.description.clone());

        let snapshot = ContextSnapshot {
            agent_id: agent_id.to_string(),
            timestamp_ms: crate::memory::layered::now_ms(),
            state_before_suspend: state_before,
            pending_intents: pending,
            active_memory_count: memories.len(),
            last_intent_description: last_intent,
        };
        self.memory.store(snapshot.to_memory_entry());

        self.scheduler.update_state(&aid, AgentState::Suspended);
        self.persist_agents();
        Ok(())
    }

    /// Resume a suspended agent with automatic context restoration.
    pub fn agent_resume(&self, agent_id: &str) -> std::io::Result<()> {
        use crate::scheduler::{AgentId, AgentState};
        use crate::memory::context_snapshot::{find_latest_snapshot, SNAPSHOT_TAG};

        let aid = AgentId(agent_id.to_string());
        let agent = self.scheduler.get(&aid).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Agent not found: {}", agent_id))
        })?;
        if agent.state() != AgentState::Suspended {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("Agent {} is not suspended (state: {:?})", agent_id, agent.state()),
            ));
        }

        let working_memories = self.memory.get_by_tags(
            agent_id,
            crate::memory::MemoryTier::Working,
            &[SNAPSHOT_TAG.to_string()],
        );
        if let Some(snapshot) = find_latest_snapshot(&working_memories) {
            let context_str = snapshot.to_context_string();
            self.remember(agent_id, context_str);
            tracing::info!("Restored context snapshot for agent {}", agent_id);
        }

        self.scheduler.update_state(&aid, AgentState::Waiting);
        self.persist_agents();
        Ok(())
    }

    /// Terminate an agent permanently.
    pub fn agent_terminate(&self, agent_id: &str) -> std::io::Result<()> {
        use crate::scheduler::{AgentId, AgentState};
        let aid = AgentId(agent_id.to_string());
        let _agent = self.scheduler.get(&aid).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Agent not found: {}", agent_id))
        })?;
        self.scheduler.update_state(&aid, AgentState::Terminated);
        self.persist_agents();
        Ok(())
    }

    // ─── Memory Operations ─────────────────────────────────────────────

    /// Store an ephemeral memory for an agent.
    pub fn remember(&self, agent_id: &str, content: String) {
        let entry = MemoryEntry::ephemeral(agent_id, content);
        self.memory.store(entry);
    }

    /// Store a working memory (persists across restarts, unlike ephemeral).
    pub fn remember_working(&self, agent_id: &str, content: String, tags: Vec<String>) {
        use crate::memory::MemoryContent;
        let entry = MemoryEntry::long_term(agent_id, MemoryContent::Text(content), tags);
        let mut entry = entry;
        entry.tier = crate::memory::MemoryTier::Working;
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

    /// Retrieve memories ranked by relevance within a token budget.
    pub fn recall_relevant(&self, agent_id: &str, budget_tokens: usize) -> Vec<MemoryEntry> {
        self.memory.recall_relevant(agent_id, budget_tokens)
    }

    /// Evict expired TTL-based memories for an agent.
    pub fn evict_expired(&self, agent_id: &str) -> usize {
        self.memory.evict_expired(agent_id)
    }

    /// Check and execute tier promotions for an agent.
    pub fn promote_check(&self, agent_id: &str) {
        self.memory.promote_check(agent_id);
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
    /// Uses `KernelExecutor` which dispatches intent actions back through
    /// the kernel's API request handler. Falls back to `LocalExecutor` for
    /// intents without an action payload.
    pub fn start_dispatch_loop(self: &Arc<Self>) -> DispatchHandle {
        let kernel = Arc::clone(self);
        let executor: Arc<dyn AgentExecutor> = Arc::new(KernelExecutor::new(
            move |action_json: &str| {
                use crate::api::semantic::{ApiRequest, ApiResponse};
                let req: ApiRequest = match serde_json::from_str(action_json) {
                    Ok(r) => r,
                    Err(e) => return serde_json::to_string(
                        &ApiResponse::error(format!("Invalid action JSON: {e}"))
                    ).unwrap_or_default(),
                };
                let resp = kernel.handle_api_request(req);
                serde_json::to_string(&resp).unwrap_or_default()
            }
        ));
        let loop_ = TokioDispatchLoop::new(Arc::clone(&self.scheduler), executor, 60_000);
        loop_.spawn()
    }

    /// Start the dispatch loop with LocalExecutor (test-only, no kernel dispatch).
    pub fn start_dispatch_loop_local(&self) -> DispatchHandle {
        let executor: Arc<dyn AgentExecutor> = Arc::new(LocalExecutor);
        let loop_ = TokioDispatchLoop::new(Arc::clone(&self.scheduler), executor, 60_000);
        loop_.spawn()
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

    // ─── Event Operations ───────────────────────────────────────────────

    /// Create an event and register it in the knowledge graph.
    pub fn create_event(
        &self,
        label: &str,
        event_type: EventType,
        start_time: Option<u64>,
        end_time: Option<u64>,
        location: Option<&str>,
        tags: Vec<String>,
        agent_id: &str,
    ) -> std::io::Result<String> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        self.fs.create_event(label, event_type, start_time, end_time, location, tags, agent_id)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    }

    /// List events matching time range, tags, and optional event type.
    pub fn list_events(
        &self,
        since: Option<u64>,
        until: Option<u64>,
        tags: &[String],
        event_type: Option<EventType>,
    ) -> Vec<EventSummary> {
        self.fs.list_events(since, until, tags, event_type).unwrap_or_default()
    }

    /// List events by natural-language time expression (e.g. "几天前", "上周").
    ///
    /// Uses the built-in rule-based resolver server-side.
    pub fn list_events_text(
        &self,
        time_expression: &str,
        tags: &[String],
        event_type: Option<EventType>,
    ) -> std::io::Result<Vec<EventSummary>> {
        let resolver: &dyn TemporalResolver = &RULE_BASED_RESOLVER;
        self.fs.list_events_by_time(time_expression, tags, event_type, resolver)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    }

    /// Attach a target (person, document, media, decision) to an event.
    pub fn event_attach(
        &self,
        event_id: &str,
        target_id: &str,
        relation: EventRelation,
        agent_id: &str,
    ) -> std::io::Result<()> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        self.fs.event_attach(event_id, target_id, relation, agent_id)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    }

    /// Associate a behavioral observation with an event.
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
    // ─── Knowledge Graph Direct Operations ─────────────────────────────────

    /// Create an arbitrary KG node (Entity, Fact, Document, Agent, Memory).
    ///
    /// Returns the generated node ID. Permission-gated via Write.
    pub fn kg_add_node(
        &self,
        label: &str,
        node_type: KGNodeType,
        properties: serde_json::Value,
        agent_id: &str,
    ) -> std::io::Result<String> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "knowledge graph not available"));
        };
        let id = uuid::Uuid::new_v4().to_string();
        let now = chrono::Utc::now().timestamp_millis() as u64;
        let node = KGNode {
            id: id.clone(),
            label: label.to_string(),
            node_type,
            content_cid: None,
            properties,
            agent_id: agent_id.to_string(),
            created_at: now,
            valid_at: Some(now),
            invalid_at: None,
            expired_at: None,
        };
        kg.add_node(node)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))?;
        Ok(id)
    }

    /// Create an edge between two KG nodes.
    ///
    /// Permission-gated via Write.
    pub fn kg_add_edge(
        &self,
        src: &str,
        dst: &str,
        edge_type: KGEdgeType,
        weight: Option<f32>,
        agent_id: &str,
    ) -> std::io::Result<()> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        let Some(ref kg) = self.knowledge_graph else {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "knowledge graph not available"));
        };
        let now = chrono::Utc::now().timestamp_millis() as u64;
        let edge = KGEdge {
            src: src.to_string(),
            dst: dst.to_string(),
            edge_type,
            weight: weight.unwrap_or(1.0),
            evidence_cid: None,
            created_at: now,
            valid_at: Some(now),
            invalid_at: None,
            expired_at: None,
            episode: None,
        };
        kg.add_edge(edge)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    }

    /// List KG nodes, optionally filtered by type.
    pub fn kg_list_nodes(
        &self,
        node_type: Option<KGNodeType>,
        agent_id: &str,
    ) -> Vec<KGNode> {
        let ctx = PermissionContext::new(agent_id.to_string());
        let _ = self.permissions.check(&ctx, PermissionAction::Read);
        let Some(ref kg) = self.knowledge_graph else {
            return Vec::new();
        };
        kg.list_nodes(agent_id, node_type).unwrap_or_default()
    }

    /// Find all paths between two KG nodes up to a given depth.
    pub fn kg_find_paths(
        &self,
        src: &str,
        dst: &str,
        max_depth: u8,
    ) -> Vec<Vec<KGNode>> {
        let Some(ref kg) = self.knowledge_graph else {
            return Vec::new();
        };
        kg.find_paths(src, dst, max_depth).unwrap_or_default()
    }

    // ─── Centralized API Request Dispatch ──────────────────────────────────

    /// Handle an `ApiRequest` and return an `ApiResponse`.
    ///
    /// This is the centralized dispatch point used by:
    /// - `plicod` TCP daemon (external AI agent requests)
    /// - `KernelExecutor` (intent action execution within the dispatch loop)
    pub fn handle_api_request(&self, req: crate::api::semantic::ApiRequest) -> crate::api::semantic::ApiResponse {
        use crate::api::semantic::*;

        match req {
            ApiRequest::Create { content, content_encoding, tags, agent_id, intent } => {
                let bytes = match decode_content(&content, &content_encoding) {
                    Ok(b) => b,
                    Err(e) => return ApiResponse::error(e),
                };
                match self.semantic_create(bytes, tags, &agent_id, intent) {
                    Ok(cid) => ApiResponse::with_cid(cid),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::Read { cid, agent_id } => {
                match self.get_object(&cid, &agent_id) {
                    Ok(obj) => ApiResponse::with_data(String::from_utf8_lossy(&obj.data).to_string()),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::Search { query, agent_id, limit, require_tags, exclude_tags, since, until } => {
                let results = self.semantic_search_with_time(
                    &query, &agent_id, limit.unwrap_or(10),
                    require_tags, exclude_tags, since, until,
                );
                let dto: Vec<SearchResultDto> = results.into_iter().map(|r| SearchResultDto {
                    cid: r.cid, relevance: r.relevance, tags: r.meta.tags,
                }).collect();
                let mut r = ApiResponse::ok();
                r.results = Some(dto);
                r
            }
            ApiRequest::Update { cid, content, content_encoding, new_tags, agent_id } => {
                let bytes = match decode_content(&content, &content_encoding) {
                    Ok(b) => b,
                    Err(e) => return ApiResponse::error(e),
                };
                match self.semantic_update(&cid, bytes, new_tags, &agent_id) {
                    Ok(new_cid) => ApiResponse::with_cid(new_cid),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::Delete { cid, agent_id } => {
                match self.semantic_delete(&cid, &agent_id) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::RegisterAgent { name } => {
                let id = self.register_agent(name);
                let mut r = ApiResponse::ok();
                r.agent_id = Some(id);
                r
            }
            ApiRequest::ListAgents => {
                let agents: Vec<AgentDto> = self.list_agents().into_iter().map(|a| AgentDto {
                    id: a.id, name: a.name, state: format!("{:?}", a.state),
                }).collect();
                let mut r = ApiResponse::ok();
                r.agents = Some(agents);
                r
            }
            ApiRequest::Remember { agent_id, content } => {
                self.remember(&agent_id, content);
                ApiResponse::ok()
            }
            ApiRequest::Recall { agent_id } => {
                let memories: Vec<String> = self.recall(&agent_id).into_iter()
                    .filter_map(|m| match m.content {
                        crate::memory::MemoryContent::Text(t) => Some(t),
                        _ => None,
                    }).collect();
                let mut r = ApiResponse::ok();
                r.memory = Some(memories);
                r
            }
            ApiRequest::Explore { cid, edge_type, depth, agent_id: _ } => {
                let depth = depth.unwrap_or(1).min(3);
                let raw = self.graph_explore_raw(&cid, edge_type.as_deref(), depth);
                let dto: Vec<NeighborDto> = raw.into_iter().map(|(node_id, label, node_type, edge_type, authority_score)| {
                    NeighborDto { node_id, label, node_type, edge_type, authority_score }
                }).collect();
                let mut r = ApiResponse::ok();
                r.neighbors = Some(dto);
                r
            }
            ApiRequest::ListDeleted { agent_id } => {
                let entries = self.list_deleted(&agent_id);
                let dto: Vec<DeletedDto> = entries.into_iter().map(|e| DeletedDto {
                    cid: e.cid, deleted_at: e.deleted_at, tags: e.original_meta.tags,
                }).collect();
                let mut r = ApiResponse::ok();
                r.deleted = Some(dto);
                r
            }
            ApiRequest::Restore { cid, agent_id } => {
                match self.restore_deleted(&cid, &agent_id) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::CreateEvent { label, event_type, start_time, end_time, location, tags, agent_id } => {
                match self.create_event(&label, event_type, start_time, end_time, location.as_deref(), tags, &agent_id) {
                    Ok(id) => ApiResponse::with_cid(id),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::ListEvents { since, until, tags, event_type, agent_id: _ } => {
                let events = self.list_events(since, until, &tags, event_type);
                ApiResponse::with_events(events)
            }
            ApiRequest::ListEventsText { time_expression, tags, event_type, agent_id: _ } => {
                match self.list_events_text(&time_expression, &tags, event_type) {
                    Ok(events) => ApiResponse::with_events(events),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::EventAttach { event_id, target_id, relation, agent_id } => {
                match self.event_attach(&event_id, &target_id, relation, &agent_id) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::AddNode { label, node_type, properties, agent_id } => {
                match self.kg_add_node(&label, node_type, properties, &agent_id) {
                    Ok(id) => ApiResponse::with_node_id(id),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::AddEdge { src_id, dst_id, edge_type, weight, agent_id } => {
                match self.kg_add_edge(&src_id, &dst_id, edge_type, weight, &agent_id) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::ListNodes { node_type, agent_id } => {
                let nodes = self.kg_list_nodes(node_type, &agent_id);
                let dto: Vec<KGNodeDto> = nodes.into_iter().map(|n| KGNodeDto {
                    id: n.id, label: n.label, node_type: n.node_type,
                    content_cid: n.content_cid, properties: n.properties,
                    agent_id: n.agent_id, created_at: n.created_at,
                }).collect();
                ApiResponse::with_nodes(dto)
            }
            ApiRequest::FindPaths { src_id, dst_id, max_depth, agent_id: _ } => {
                let depth = max_depth.unwrap_or(3).min(5);
                let paths = self.kg_find_paths(&src_id, &dst_id, depth);
                let dto: Vec<Vec<KGNodeDto>> = paths.into_iter().map(|path| {
                    path.into_iter().map(|n| KGNodeDto {
                        id: n.id, label: n.label, node_type: n.node_type,
                        content_cid: n.content_cid, properties: n.properties,
                        agent_id: n.agent_id, created_at: n.created_at,
                    }).collect()
                }).collect();
                ApiResponse::with_paths(dto)
            }

            // ── Agent Lifecycle ───────────────────────────────────────
            ApiRequest::SubmitIntent { description, priority, action, agent_id } => {
                let p = match priority.to_lowercase().as_str() {
                    "critical" => IntentPriority::Critical,
                    "high" => IntentPriority::High,
                    "medium" => IntentPriority::Medium,
                    _ => IntentPriority::Low,
                };
                let id = self.submit_intent(p, description, action, Some(agent_id));
                let mut r = ApiResponse::ok();
                r.intent_id = Some(id);
                r
            }
            ApiRequest::AgentStatus { agent_id } => {
                match self.agent_status(&agent_id) {
                    Some((_id, state, pending)) => {
                        let mut r = ApiResponse::ok();
                        r.agent_id = Some(agent_id);
                        r.agent_state = Some(state);
                        r.pending_intents = Some(pending);
                        r
                    }
                    None => ApiResponse::error(format!("Agent not found: {}", agent_id)),
                }
            }
            ApiRequest::AgentSuspend { agent_id } => {
                match self.agent_suspend(&agent_id) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::AgentResume { agent_id } => {
                match self.agent_resume(&agent_id) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::AgentTerminate { agent_id } => {
                match self.agent_terminate(&agent_id) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }

            // ── Tool operations ─────────────────────────────────────
            ApiRequest::ToolCall { tool, params, agent_id } => {
                let result = self.execute_tool(&tool, &params, &agent_id);
                let mut r = if result.success { ApiResponse::ok() } else { ApiResponse::error(result.error.clone().unwrap_or_default()) };
                r.tool_result = Some(result);
                r
            }
            ApiRequest::ToolList { agent_id: _ } => {
                let tools = self.tool_registry.list();
                let mut r = ApiResponse::ok();
                r.tools = Some(tools);
                r
            }
            ApiRequest::ToolDescribe { tool, agent_id: _ } => {
                match self.tool_registry.get(&tool) {
                    Some(desc) => {
                        let mut r = ApiResponse::ok();
                        r.tools = Some(vec![desc]);
                        r
                    }
                    None => ApiResponse::error(format!("tool not found: {}", tool)),
                }
            }

            // ── Intent Resolution ─────────────────────────────────────
            ApiRequest::IntentResolve { text, agent_id } => {
                let intents = self.intent_resolve(&text, &agent_id);
                let mut r = ApiResponse::ok();
                r.resolved_intents = Some(intents);
                r
            }

            // ── Agent Resource Management ─────────────────────────────
            ApiRequest::AgentSetResources { agent_id, memory_quota, cpu_time_quota, allowed_tools, caller_agent_id } => {
                let ctx = PermissionContext::new(caller_agent_id.clone());
                if let Err(e) = self.permissions.check(&ctx, PermissionAction::Write) {
                    return ApiResponse::error(e.to_string());
                }
                match self.agent_set_resources(&agent_id, memory_quota, cpu_time_quota, allowed_tools) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }

            // ── Agent Messaging ───────────────────────────────────────
            ApiRequest::SendMessage { from, to, payload } => {
                match self.send_message(&from, &to, payload) {
                    Ok(msg_id) => {
                        let mut r = ApiResponse::ok();
                        r.data = Some(msg_id);
                        r
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::ReadMessages { agent_id, unread_only } => {
                let msgs = self.read_messages(&agent_id, unread_only);
                let mut r = ApiResponse::ok();
                r.messages = Some(msgs);
                r
            }
            ApiRequest::AckMessage { agent_id, message_id } => {
                if self.ack_message(&agent_id, &message_id) {
                    ApiResponse::ok()
                } else {
                    ApiResponse::error(format!("message not found: {}", message_id))
                }
            }
        }
    }

    // ─── Intent Resolution ─────────────────────────────────────────────

    /// Resolve natural language text into structured API requests.
    pub fn intent_resolve(&self, text: &str, agent_id: &str) -> Vec<ResolvedIntent> {
        match self.intent_router.resolve(text, agent_id) {
            Ok(results) => results,
            Err(e) => {
                tracing::warn!("Intent resolution failed: {}", e);
                vec![]
            }
        }
    }

    // ─── Agent Resource Management ────────────────────────────────────

    /// Update an agent's resource limits.
    pub fn agent_set_resources(
        &self,
        agent_id: &str,
        memory_quota: Option<u64>,
        cpu_time_quota: Option<u64>,
        allowed_tools: Option<Vec<String>>,
    ) -> std::io::Result<()> {
        use crate::scheduler::AgentId;
        let aid = AgentId(agent_id.to_string());
        let current = self.scheduler.get_resources(&aid).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::NotFound, format!("Agent not found: {}", agent_id))
        })?;

        let resources = AgentResources {
            memory_quota: memory_quota.unwrap_or(current.memory_quota),
            cpu_time_quota: cpu_time_quota.unwrap_or(current.cpu_time_quota),
            allowed_tools: allowed_tools.unwrap_or(current.allowed_tools),
        };

        if self.scheduler.set_resources(&aid, resources) {
            self.persist_agents();
            Ok(())
        } else {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Agent not found"))
        }
    }

    // ─── Agent Messaging ──────────────────────────────────────────────

    /// Send a message from one agent to another.
    pub fn send_message(
        &self,
        from: &str,
        to: &str,
        payload: serde_json::Value,
    ) -> std::io::Result<String> {
        let ctx = PermissionContext::new(from.to_string());
        self.permissions.check(&ctx, PermissionAction::SendMessage)?;
        let msg_id = self.message_bus.send(from, to, payload);
        Ok(msg_id)
    }

    /// Read messages for an agent.
    pub fn read_messages(&self, agent_id: &str, unread_only: bool) -> Vec<crate::scheduler::messaging::AgentMessage> {
        self.message_bus.read(agent_id, unread_only)
    }

    /// Acknowledge (mark as read) a message.
    pub fn ack_message(&self, agent_id: &str, message_id: &str) -> bool {
        self.message_bus.ack(agent_id, message_id)
    }

    // ─── Project Self-Management (Dogfooding Plico) ─────────────────────────

    /// Build runtime kernel metrics from live system state.
    pub fn dashboard_status(&self) -> crate::api::semantic::DashboardStatus {
        use crate::api::semantic::DashboardStatus;

        let kg_node_count = self.knowledge_graph.as_ref()
            .map(|kg| kg.node_count().unwrap_or(0))
            .unwrap_or(0);
        let kg_edge_count = self.knowledge_graph.as_ref()
            .map(|kg| kg.edge_count().unwrap_or(0))
            .unwrap_or(0);

        let cas_object_count = {
            let objects_path = self.fs.root().join("objects");
            let mut count = 0usize;
            if let Ok(entries) = std::fs::read_dir(&objects_path) {
                for entry in entries.filter_map(|e| e.ok()) {
                    let path = entry.path();
                    if path.is_dir() {
                        if let Ok(sub_entries) = std::fs::read_dir(&path) {
                            count += sub_entries.filter_map(|se| se.ok())
                                .filter(|se| se.file_type().is_ok_and(|ft| ft.is_file()))
                                .count();
                        }
                    }
                }
            }
            count
        };

        DashboardStatus {
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            cas_object_count,
            agent_count: self.scheduler.list_agents().len(),
            tag_count: self.fs.list_tags().len(),
            kg_node_count,
            kg_edge_count,
        }
    }

}
