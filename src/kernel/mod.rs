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
use crate::fs::{SemanticFS, Query, OllamaBackend, InMemoryBackend, EmbeddingProvider, SemanticSearch, OllamaSummarizer, Summarizer, KnowledgeGraph, PetgraphBackend, LocalEmbeddingBackend, StubEmbeddingProvider, EmbedError, EventType, EventRelation, EventSummary};
use crate::temporal::{TemporalResolver, RULE_BASED_RESOLVER};
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

        // Initialize project KG nodes for dogfooding
        kernel.init_project_nodes();

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
        let filter = crate::fs::SearchFilter {
            require_tags,
            exclude_tags,
            content_type: None,
            since,
            until,
        };
        self.fs.search_with_filter(query, limit, filter)
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
    pub fn event_add_observation(
        &self,
        event_id: &str,
        observation_id: &str,
        agent_id: &str,
    ) -> std::io::Result<()> {
        let ctx = PermissionContext::new(agent_id.to_string());
        self.permissions.check(&ctx, PermissionAction::Write)?;
        self.fs.event_add_observation(event_id, observation_id)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    }

    /// Get all behavioral observation IDs associated with an event.
    pub fn event_get_observations(
        &self,
        event_id: &str,
    ) -> std::io::Result<Vec<String>> {
        self.fs.event_get_observations(event_id)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    }

    /// Store a UserFact (promoted behavioral pattern) in the preference store.
    pub fn add_user_fact(&self, fact: crate::fs::UserFact) {
        self.fs.add_user_fact(fact);
    }

    /// Get all UserFacts for a given subject (person).
    pub fn get_user_facts_for_subject(&self, subject_id: &str) -> Vec<crate::fs::UserFact> {
        self.fs.get_user_facts_for_subject(subject_id)
    }

    /// Infer action suggestions for an event by traversing:
    /// Event → HasAttendee → Person → UserFact → ActionSuggestion
    pub fn infer_suggestions_for_event(
        &self,
        event_id: &str,
    ) -> std::io::Result<Vec<crate::fs::ActionSuggestion>> {
        self.fs.infer_suggestions_for_event(event_id)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    }

    /// Get all pending (unconfirmed/undismissed) suggestions across all events.
    pub fn get_pending_suggestions(&self) -> Vec<crate::fs::ActionSuggestion> {
        self.fs.get_pending_suggestions()
    }

    /// Get all suggestions for a specific event.
    pub fn get_suggestions_for_event(&self, event_id: &str) -> Vec<crate::fs::ActionSuggestion> {
        self.fs.get_suggestions_for_event(event_id)
    }

    /// Confirm a suggestion by ID.
    pub fn confirm_suggestion(&self, suggestion_id: &str) -> std::io::Result<()> {
        self.fs.confirm_suggestion(suggestion_id)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
    }

    /// Dismiss a suggestion by ID.
    pub fn dismiss_suggestion(&self, suggestion_id: &str) -> std::io::Result<()> {
        self.fs.dismiss_suggestion(suggestion_id)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
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
    // ─── Project Self-Management (Dogfooding Plico) ─────────────────────────

    /// Returns the current project status by querying KG nodes.
    ///
    /// Collects all Iteration, Plan, and DesignDoc KG nodes and assembles
    /// them into a `ProjectStatus` struct. Git branch/commit are read from
    /// the environment or derived from git commands.
    pub fn project_status(&self) -> crate::api::semantic::ProjectStatus {
        use crate::api::semantic::*;
        use crate::fs::KGNodeType;

        let Some(ref kg) = self.knowledge_graph else {
            return ProjectStatus {
                iteration: 0,
                git_branch: String::new(),
                git_commit: String::new(),
                iterations: Vec::new(),
                plans: Vec::new(),
                design_docs: Vec::new(),
                soul_alignment_percent: 0,
                key_gaps: Vec::new(),
            };
        };

        // Collect all nodes
        let all_nodes: Vec<_> = kg.all_node_ids()
            .into_iter()
            .filter_map(|id| kg.get_node(&id).ok().flatten())
            .collect();

        let iterations: Vec<_> = all_nodes.iter()
            .filter(|n| n.node_type == KGNodeType::Iteration)
            .map(|n| IterationDto {
                id: n.id.clone(),
                name: n.properties.get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&n.label)
                    .to_string(),
                completed_phases: n.properties.get("completed_phases")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default(),
                active_phase: n.properties.get("active_phase")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                commit_hash: n.properties.get("commit_hash")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                date: n.properties.get("date")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
            .collect();

        let plans: Vec<_> = all_nodes.iter()
            .filter(|n| n.node_type == KGNodeType::Plan)
            .map(|n| PlanDto {
                id: n.id.clone(),
                title: n.label.clone(),
                phase: n.properties.get("phase")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                status: n.properties.get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("pending")
                    .to_string(),
                priority: n.properties.get("priority")
                    .and_then(|v| v.as_str())
                    .unwrap_or("P1")
                    .to_string(),
                description: n.properties.get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
            .collect();

        let design_docs: Vec<_> = all_nodes.iter()
            .filter(|n| n.node_type == KGNodeType::DesignDoc)
            .map(|n| DesignDocDto {
                id: n.id.clone(),
                name: n.label.clone(),
                path: n.properties.get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                version: n.properties.get("version")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                description: n.properties.get("description")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
            .collect();

        // Git info from environment or git commands
        let git_branch = std::env::var("GIT_BRANCH")
            .unwrap_or_else(|_| {
                std::process::Command::new("git")
                    .args(["rev-parse", "--abbrev-ref", "HEAD"])
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|| "feat/plico-self-management".to_string())
            });

        let git_commit = std::env::var("GIT_COMMIT")
            .unwrap_or_else(|_| {
                std::process::Command::new("git")
                    .args(["rev-parse", "--short", "HEAD"])
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default()
            });

        // Current iteration number — highest numeric iter in KG
        let iteration = iterations.iter()
            .filter_map(|i| i.name.strip_prefix("iter").and_then(|s| s.parse::<u32>().ok()))
            .max()
            .unwrap_or(0);

        // Soul alignment: computed from AI-managed KG nodes — not hardcoded
        // Read from ProjectConfig node if set by AI, else derive from coverage
        let soul_alignment_percent = all_nodes.iter()
            .find(|n| n.id == "project-config")
            .and_then(|n| n.properties.get("soul_alignment_percent")?.as_u64())
            .map(|v| v as u8)
            .unwrap_or_else(|| {
                // Fallback: derive from presence of project management nodes
                let has_iters = !iterations.is_empty();
                let has_plans = !plans.is_empty();
                let has_docs = design_docs.len() >= 3;
                let has_active_work = plans.iter().any(|p| p.status == "in_progress");
                match (has_iters, has_plans, has_docs, has_active_work) {
                    (true, true, true, true) => 80,
                    (true, true, true, false) => 70,
                    (true, true, false, _) => 55,
                    (true, false, _, _) => 40,
                    _ => 20,
                }
            });

        // Key gaps — Plan nodes with "gap" in title or description
        let key_gaps: Vec<_> = plans.iter()
            .filter(|p| {
                p.title.to_lowercase().contains("gap") ||
                p.description.to_lowercase().contains("gap")
            })
            .map(|p| GapDto {
                title: p.title.clone(),
                priority: p.priority.clone(),
                blocks: Vec::new(),
                description: p.description.clone(),
            })
            .collect();

        ProjectStatus {
            iteration,
            git_branch,
            git_commit,
            iterations,
            plans,
            design_docs,
            soul_alignment_percent,
            key_gaps,
        }
    }

    /// Initialize project KG nodes on first startup.
    ///
    /// Creates Iteration, Plan, and DesignDoc nodes if they don't exist yet.
    /// Safe to call multiple times — idempotent based on node IDs.
    pub fn init_project_nodes(&self) {
        use crate::fs::{KGNodeType, KGEdgeType};

        let Some(ref kg) = self.knowledge_graph else { return; };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        // Check if already initialized
        let existing = kg.all_node_ids();
        if existing.iter().any(|id| id == "iter12") {
            return; // already initialized
        }

        // Create iter12 node — phase/completion data comes from AI use, not hardcode
        let iter12_props = serde_json::json!({
            "name": "iter12",
            "completed_phases": Vec::<String>::new(),
            "active_phase": null,
            "commit_hash": "",
            "date": chrono::Local::now().format("%Y-%m-%d").to_string(),
        });
        let _ = kg.add_node(crate::fs::KGNode {
            id: "iter12".into(),
            label: "iter12".into(),
            node_type: KGNodeType::Iteration,
            content_cid: None,
            properties: iter12_props,
            agent_id: "system".into(),
            created_at: now,
            valid_at: None,
            invalid_at: None,
            expired_at: None,
        });

        // Create plan: complete project self-management
        let plan_props = serde_json::json!({
            "phase": "iter12",
            "status": "in_progress",
            "priority": "P0",
            "description": "Implement Plico self-management: KG nodes for project state, HTTP API for dashboard",
        });
        let _ = kg.add_node(crate::fs::KGNode {
            id: "plan-plico-self-mgmt".into(),
            label: "complete project self-management".into(),
            node_type: KGNodeType::Plan,
            content_cid: None,
            properties: plan_props,
            agent_id: "system".into(),
            created_at: now,
            valid_at: None,
            invalid_at: None,
            expired_at: None,
        });

        // Create edge: iter12 HasPlan plan-plico-self-mgmt
        let _ = kg.add_edge(crate::fs::KGEdge {
            src: "iter12".into(),
            dst: "plan-plico-self-mgmt".into(),
            edge_type: KGEdgeType::HasPlan,
            weight: 1.0,
            evidence_cid: None,
            created_at: now,
            valid_at: None,
            invalid_at: None,
            expired_at: None,
            episodes: vec![],
        });

        // DesignDoc nodes are created by the AI through semantic_create as design docs are written.
        // Do NOT hardcode design doc list here — knowledge comes from use.

        tracing::info!("Initialized project KG nodes for Plico self-management");
    }

    /// Signal that the AI should sync project state from external inputs (git, filesystem).
    ///
    /// This is a no-op in the kernel — actual KG updates are performed by the AI
    /// via `semantic_create` / `semantic_update` API calls after observing state.
    ///
    /// Triggered by: post-commit hooks, cron jobs, or AI self-assessment.
    pub fn sync_project_state(&self) {
        tracing::info!("sync_project_state triggered — AI should observe git/filesystem and update KG via semantic API");
    }

    /// Build the dashboard status response from live kernel state.
    pub fn dashboard_status(&self) -> crate::api::semantic::DashboardStatus {
        use crate::api::semantic::{
            DashboardStatus, PhaseStatus, ModuleStatus, SoulAlignment,
            PrincipleStatus, ExampleCoverage, ChainStep, NextStep,
        };

        let git_branch = std::env::var("GIT_BRANCH").unwrap_or_else(|_| "unknown".to_string());
        let git_commit = std::env::var("GIT_COMMIT").unwrap_or_else(|_| "unknown".to_string());

        let kg_node_count = self.knowledge_graph.as_ref()
            .map(|kg| kg.node_count().unwrap_or(0))
            .unwrap_or(0);
        let kg_edge_count = self.knowledge_graph.as_ref()
            .map(|kg| kg.edge_count().unwrap_or(0))
            .unwrap_or(0);

        let phases = vec![
            PhaseStatus { name: "Phase A".into(), percent: 100, status: "done".into() },
            PhaseStatus { name: "Phase B".into(), percent: 100, status: "done".into() },
            PhaseStatus { name: "Phase C".into(), percent: 100, status: "done".into() },
            PhaseStatus { name: "Phase D".into(), percent: 100, status: "done".into() },
            PhaseStatus { name: "Phase E".into(), percent: 30, status: "active".into() },
            PhaseStatus { name: "Phase F".into(), percent: 0, status: "pending".into() },
        ];

        let modules = vec![
            ModuleStatus { name: "CAS".into(), path: "src/cas/".into(), status: "done".into() },
            ModuleStatus { name: "Memory".into(), path: "src/memory/".into(), status: "done".into() },
            ModuleStatus { name: "Scheduler".into(), path: "src/scheduler/".into(), status: "active".into() },
            ModuleStatus { name: "SemanticFS".into(), path: "src/fs/".into(), status: "done".into() },
            ModuleStatus { name: "Kernel".into(), path: "src/kernel/".into(), status: "done".into() },
            ModuleStatus { name: "API".into(), path: "src/api/".into(), status: "done".into() },
            ModuleStatus { name: "ToolRegistry".into(), path: "src/scheduler/tool.rs".into(), status: "pending".into() },
        ];

        let soul_alignment = SoulAlignment {
            principles: vec![
                PrincipleStatus { number: 1, title: "内容即地址".into(), description: "CAS + SHA-256，内容哈希即身份".into(), aligned: "aligned".into() },
                PrincipleStatus { number: 2, title: "语义即索引".into(), description: "向量嵌入 + 知识图谱，而非路径/文件名".into(), aligned: "aligned".into() },
                PrincipleStatus { number: 3, title: "事件为第一公民".into(), description: "EventContainer + 事件关系边".into(), aligned: "aligned".into() },
                PrincipleStatus { number: 4, title: "AI 自我迭代".into(), description: "BehavioralObservation → UserFact → ActionSuggestion".into(), aligned: "partial".into() },
            ],
            overall_percent: 92,
        };

        let examples = vec![
            ExampleCoverage {
                name: "会议总结生成PPT".into(),
                reasoning_chain: vec![
                    ChainStep { name: "semantic_search".into(), done: true },
                    ChainStep { name: "L0/L1/L2".into(), done: true },
                    ChainStep { name: "EntityExtractor".into(), done: false },
                    ChainStep { name: "LLM总结".into(), done: false },
                ],
                execution_chain: vec![
                    ChainStep { name: "Tool执行".into(), done: false },
                ],
            },
            ExampleCoverage {
                name: "宿醉后主动点白粥".into(),
                reasoning_chain: vec![
                    ChainStep { name: "BehavioralObservation".into(), done: true },
                    ChainStep { name: "PatternExtractor".into(), done: true },
                    ChainStep { name: "UserFact".into(), done: true },
                    ChainStep { name: "ActionSuggestion".into(), done: true },
                ],
                execution_chain: vec![
                    ChainStep { name: "Scheduler触发".into(), done: false },
                    ChainStep { name: "点餐API".into(), done: false },
                ],
            },
            ExampleCoverage {
                name: "王总吃饭提醒带酒".into(),
                reasoning_chain: vec![
                    ChainStep { name: "Event(吃饭)".into(), done: true },
                    ChainStep { name: "HasAttendee(王总)".into(), done: true },
                    ChainStep { name: "UserFact".into(), done: true },
                    ChainStep { name: "ActionSuggestion".into(), done: true },
                ],
                execution_chain: vec![
                    ChainStep { name: "Scheduler触发".into(), done: false },
                    ChainStep { name: "提醒推送".into(), done: false },
                ],
            },
        ];

        let next_steps = vec![
            NextStep { order: 1, title: "ToolRegistry 实现".into(), description: "HashMap<String, Arc<dyn Tool>> + register/call".into(), priority: "P0".into() },
            NextStep { order: 2, title: "confirm_suggestion API".into(), description: "ActionSuggestion → Scheduler 触发".into(), priority: "P0".into() },
            NextStep { order: 3, title: "外部工具集成".into(), description: "点餐 API + 提醒推送 工具实现".into(), priority: "P0".into() },
            NextStep { order: 4, title: "EntityExtractor".into(), description: "create() 时自动触发实体提取 + KG 边".into(), priority: "P1".into() },
        ];

        DashboardStatus {
            iteration: 12,
            started_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            now: chrono::Utc::now().timestamp_millis(),
            git_branch,
            git_commit,
            tests_passed: None,
            cas_object_count: {
                // SemanticFS stores at root/objects/, not root/cas/
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
            },
            agent_count: self.scheduler.list_agents().len(),
            tag_count: self.fs.list_tags().len(),
            kg_node_count,
            kg_edge_count,
            event_count: 0,
            pending_suggestions: self.fs.pending_suggestion_count(),
            phases,
            modules,
            soul_alignment,
            examples,
            next_steps,
        }
    }
}

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
