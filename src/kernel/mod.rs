//! AI Kernel — central orchestrator for all Plico subsystems.
//!
//! Wires together: CAS Storage, Layered Memory, Agent Scheduler,
//! Semantic FS, and Permission Guardrails. Upper-layer AI agents
//! interact with the kernel through the semantic API.
//!
//! # Module Structure
//! - `mod.rs` — struct, constructor, handle_api_request
//! - `builtin_tools.rs` — tool registration
//! - `persistence.rs` — state persistence/restore
//! - `ops/` — operation groups (fs, agent, memory, events, graph, dispatch, intent, messaging, dashboard)

mod builtin_tools;
mod persistence;
pub mod ops;

use crate::api::semantic::{ApiRequest, ApiResponse};

use std::path::PathBuf;
use std::sync::{Arc, atomic::{AtomicU64, Ordering}};

use crate::cas::CASStorage;
use crate::memory::{LayeredMemory, MemoryScope, CASPersister, MemoryPersister};
use crate::scheduler::{AgentScheduler, IntentPriority};
use crate::scheduler::messaging::MessageBus;
use crate::fs::{SemanticFS, InMemoryBackend, HnswBackend, EmbeddingProvider, SemanticSearch, LlmSummarizer, Summarizer, KnowledgeGraph, PetgraphBackend, StubEmbeddingProvider};
use crate::api::permission::PermissionGuard;
use crate::tool::ToolRegistry;

/// The AI Kernel — all subsystems wired together.
pub struct AIKernel {
    pub(crate) root: PathBuf,
    pub(crate) cas: Arc<CASStorage>,
    pub(crate) memory: Arc<LayeredMemory>,
    pub(crate) scheduler: Arc<AgentScheduler>,
    pub(crate) fs: Arc<SemanticFS>,
    pub(crate) permissions: Arc<PermissionGuard>,
    pub(crate) memory_persister: Option<Arc<dyn MemoryPersister + Send + Sync>>,
    #[allow(dead_code)]
    pub(crate) embedding: Arc<dyn EmbeddingProvider>,
    #[allow(dead_code)]
    pub(crate) summarizer: Option<Arc<dyn Summarizer>>,
    pub(crate) knowledge_graph: Option<Arc<dyn KnowledgeGraph>>,
    pub(crate) search_backend: Arc<dyn SemanticSearch>,
    /// Counter for search index auto-persist. Every SEARCH_PERSIST_EVERY_N operations,
    /// the search index snapshot is saved to disk for crash recovery.
    search_op_count: Arc<AtomicU64>,
    pub(crate) tool_registry: Arc<ToolRegistry>,
    pub(crate) message_bus: Arc<MessageBus>,
}

impl AIKernel {
    /// Initialize the AI Kernel with the given storage root.
    pub fn new(root: PathBuf) -> std::io::Result<Self> {
        let cas = Arc::new(CASStorage::new(root.join("cas"))?);

        let embedding: Arc<dyn EmbeddingProvider> =
            persistence::create_embedding_provider().unwrap_or_else(|e| {
                tracing::warn!("Embedding backend failed: {e}. Using stub (tag-only search).");
                Arc::new(StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>
            });

        let summarizer: Option<Arc<dyn Summarizer>> = match persistence::create_llm_provider("PLICO_SUMMARIZER_MODEL", "llama3.2") {
            Ok(provider) => {
                tracing::info!("LLM summarizer enabled: {}", provider.model_name());
                Some(Arc::new(LlmSummarizer::new(provider)))
            }
            Err(e) => {
                tracing::warn!("Could not create summarizer: {e}. ContextLoader will use heuristic summaries.");
                None
            }
        };

        let search_backend: Arc<dyn SemanticSearch> = match std::env::var("SEARCH_BACKEND")
            .unwrap_or_else(|_| "memory".into()).as_str()
        {
            "hnsw" => {
                let backend = Arc::new(HnswBackend::new());
                backend.restore_from(&root).ok();
                backend as Arc<dyn SemanticSearch>
            }
            _ => {
                let backend = Arc::new(InMemoryBackend::new());
                backend.restore_from(&root).ok();
                backend as Arc<dyn SemanticSearch>
            }
        };
        let search_index: Arc<dyn SemanticSearch> = search_backend.clone();
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
            search_op_count: Arc::new(AtomicU64::new(0)),
            tool_registry,
            message_bus,
        };

        kernel.register_builtin_tools();
        kernel.restore_agents();
        kernel.restore_intents();
        kernel.restore_memories();
        kernel.restore_permissions();

        Ok(kernel)
    }

    /// Auto-persist hook: call after each write operation (create/update/delete).
    /// Persists the search index snapshot every N operations to prevent
    /// loss of real embeddings if the process crashes.
    const SEARCH_PERSIST_EVERY_N: u64 = 50;

    fn maybe_persist_search_index(&self) {
        let count = self.search_op_count.fetch_add(1, Ordering::Relaxed) + 1;
        if count.is_multiple_of(Self::SEARCH_PERSIST_EVERY_N) {
            self.persist_search_index();
        }
    }

    // ─── API Request Handler ───────────────────────────────────────────

    pub fn handle_api_request(&self, req: crate::api::semantic::ApiRequest) -> crate::api::semantic::ApiResponse {
        use crate::api::semantic::{
            SearchResultDto, AgentDto, NeighborDto, DeletedDto,
            KGNodeDto,
        };
        use crate::api::semantic::ContentEncoding;

        fn decode_content(content: &str, encoding: &ContentEncoding) -> Result<Vec<u8>, String> {
            crate::api::semantic::decode_content(content, encoding)
        }

        match req {
            ApiRequest::Create { content, content_encoding, tags, agent_id, intent } => {
                let bytes = match decode_content(&content, &content_encoding) {
                    Ok(b) => b,
                    Err(e) => return ApiResponse::error(e),
                };
                match self.semantic_create(bytes, tags, &agent_id, intent) {
                    Ok(cid) => {
                        self.maybe_persist_search_index();
                        ApiResponse::with_cid(cid)
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::Read { cid, agent_id } => {
                match self.get_object(&cid, &agent_id) {
                    Ok(obj) => ApiResponse::with_data(String::from_utf8_lossy(&obj.data).to_string()),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::Search { query, agent_id, limit, offset, require_tags, exclude_tags, since, until } => {
                let results = match self.semantic_search_with_time(
                    &query, &agent_id, limit.unwrap_or(10) + offset.unwrap_or(0),
                    require_tags, exclude_tags, since, until,
                ) {
                    Ok(r) => r,
                    Err(e) => return ApiResponse::error(e.to_string()),
                };
                let total = results.len();
                let off = offset.unwrap_or(0);
                let lim = limit.unwrap_or(10);
                let page: Vec<SearchResultDto> = results.into_iter().skip(off).take(lim).map(|r| SearchResultDto {
                    cid: r.cid, relevance: r.relevance, tags: r.meta.tags,
                }).collect();
                let mut r = ApiResponse::ok();
                r.total_count = Some(total);
                r.has_more = Some(off + page.len() < total);
                r.results = Some(page);
                r
            }
            ApiRequest::Update { cid, content, content_encoding, new_tags, agent_id } => {
                let bytes = match decode_content(&content, &content_encoding) {
                    Ok(b) => b,
                    Err(e) => return ApiResponse::error(e),
                };
                match self.semantic_update(&cid, bytes, new_tags, &agent_id) {
                    Ok(new_cid) => {
                        self.maybe_persist_search_index();
                        ApiResponse::with_cid(new_cid)
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::Delete { cid, agent_id } => {
                match self.semantic_delete(&cid, &agent_id) {
                    Ok(()) => {
                        self.maybe_persist_search_index();
                        ApiResponse::ok()
                    }
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
                match self.remember(&agent_id, content) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e),
                }
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
            ApiRequest::RememberLongTerm { agent_id, content, tags, importance, scope } => {
                let scope = parse_scope(scope);
                match self.remember_long_term_scoped(&agent_id, content, tags, importance, scope) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::RecallSemantic { agent_id, query, k } => {
                match self.recall_semantic(&agent_id, &query, k) {
                    Ok(entries) => {
                        let memories: Vec<String> = entries.into_iter()
                            .filter_map(|m| match m.content {
                                crate::memory::MemoryContent::Text(t) => Some(t),
                                _ => None,
                            }).collect();
                        let mut r = ApiResponse::ok();
                        r.memory = Some(memories);
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
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
            ApiRequest::GrantPermission { agent_id, action, scope, expires_at } => {
                match PermissionGuard::parse_action(&action) {
                    Some(act) => {
                        self.permission_grant(&agent_id, act, scope, expires_at);
                        ApiResponse::ok()
                    }
                    None => ApiResponse::error(format!("Unknown action: {}", action)),
                }
            }
            ApiRequest::RevokePermission { agent_id, action } => {
                match PermissionGuard::parse_action(&action) {
                    Some(act) => {
                        self.permission_revoke(&agent_id, act);
                        ApiResponse::ok()
                    }
                    None => ApiResponse::error(format!("Unknown action: {}", action)),
                }
            }
            ApiRequest::ListPermissions { agent_id } => {
                let grants = self.permission_list(&agent_id);
                let dto: Vec<serde_json::Value> = grants.into_iter().map(|g| {
                    serde_json::json!({
                        "action": format!("{:?}", g.action),
                        "scope": g.scope,
                        "expires_at": g.expires_at,
                    })
                }).collect();
                let mut r = ApiResponse::ok();
                r.data = Some(serde_json::to_string(&serde_json::json!({"grants": dto})).unwrap_or_default());
                r
            }
            ApiRequest::CheckPermission { agent_id, action } => {
                match PermissionGuard::parse_action(&action) {
                    Some(act) => {
                        let allowed = self.permission_check(&agent_id, act).is_ok();
                        let mut r = ApiResponse::ok();
                        r.data = Some(serde_json::to_string(&serde_json::json!({"allowed": allowed})).unwrap_or_default());
                        r
                    }
                    None => ApiResponse::error(format!("Unknown action: {}", action)),
                }
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
            ApiRequest::History { cid, agent_id } => {
                let chain = self.version_history(&cid, &agent_id);
                let mut r = ApiResponse::ok();
                r.data = Some(serde_json::to_string(&chain).unwrap_or_default());
                r
            }
            ApiRequest::Rollback { cid, agent_id } => {
                match self.rollback(&cid, &agent_id) {
                    Ok(new_cid) => {
                        self.maybe_persist_search_index();
                        ApiResponse::with_cid(new_cid)
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::CreateEvent { label, event_type, start_time, end_time, location, tags, agent_id } => {
                match self.create_event(&label, event_type, start_time, end_time, location.as_deref(), tags, &agent_id) {
                    Ok(id) => ApiResponse::with_cid(id),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::ListEvents { since, until, tags, event_type, agent_id: _, limit, offset } => {
                let all_events = self.list_events(since, until, &tags, event_type);
                let total = all_events.len();
                let off = offset.unwrap_or(0);
                let lim = limit.unwrap_or(total);
                let page: Vec<_> = all_events.into_iter().skip(off).take(lim).collect();
                let mut r = ApiResponse::with_events(page.clone());
                r.total_count = Some(total);
                r.has_more = Some(off + page.len() < total);
                r
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
            ApiRequest::ListNodes { node_type, agent_id, limit, offset } => {
                let nodes = match self.kg_list_nodes(node_type, &agent_id) {
                    Ok(n) => n,
                    Err(e) => return ApiResponse::error(e.to_string()),
                };
                let total = nodes.len();
                let off = offset.unwrap_or(0);
                let lim = limit.unwrap_or(total);
                let dto: Vec<KGNodeDto> = nodes.into_iter().skip(off).take(lim).map(|n| KGNodeDto {
                    id: n.id, label: n.label, node_type: n.node_type,
                    content_cid: n.content_cid, properties: n.properties,
                    agent_id: n.agent_id, created_at: n.created_at,
                }).collect();
                let mut r = ApiResponse::with_nodes(dto.clone());
                r.total_count = Some(total);
                r.has_more = Some(off + dto.len() < total);
                r
            }
            ApiRequest::ListNodesAtTime { node_type, agent_id, t } => {
                let nodes = match self.kg_get_valid_nodes_at(&agent_id, node_type, t) {
                    Ok(n) => n,
                    Err(e) => return ApiResponse::error(e.to_string()),
                };
                let dto: Vec<KGNodeDto> = nodes.into_iter().map(|n| KGNodeDto {
                    id: n.id, label: n.label, node_type: n.node_type,
                    content_cid: n.content_cid, properties: n.properties,
                    agent_id: n.agent_id, created_at: n.created_at,
                }).collect();
                ApiResponse::with_nodes(dto)
            }
            ApiRequest::FindPaths { src_id, dst_id, max_depth, weighted, agent_id: _ } => {
                let depth = max_depth.unwrap_or(3).min(5);
                let dto: Vec<Vec<KGNodeDto>> = if weighted {
                    // Find single highest-weight path
                    if let Some(path) = self.kg_find_weighted_path(&src_id, &dst_id, depth) {
                        vec![path.into_iter().map(|n| KGNodeDto {
                            id: n.id, label: n.label, node_type: n.node_type,
                            content_cid: n.content_cid, properties: n.properties,
                            agent_id: n.agent_id, created_at: n.created_at,
                        }).collect()]
                    } else {
                        vec![]
                    }
                } else {
                    // Find all paths
                    let paths = self.kg_find_paths(&src_id, &dst_id, depth);
                    paths.into_iter().map(|path| {
                        path.into_iter().map(|n| KGNodeDto {
                            id: n.id, label: n.label, node_type: n.node_type,
                            content_cid: n.content_cid, properties: n.properties,
                            agent_id: n.agent_id, created_at: n.created_at,
                        }).collect()
                    }).collect()
                };
                ApiResponse::with_paths(dto)
            }
            ApiRequest::SubmitIntent { description, priority, action, agent_id } => {
                let p = match priority.to_lowercase().as_str() {
                    "critical" => IntentPriority::Critical,
                    "high" => IntentPriority::High,
                    "medium" => IntentPriority::Medium,
                    _ => IntentPriority::Low,
                };
                let id = match self.submit_intent(p, description, action, Some(agent_id)) {
                    Ok(id) => id,
                    Err(e) => return ApiResponse::error(e),
                };
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
            ApiRequest::ReadMessages { agent_id, unread_only, limit, offset } => {
                let all_msgs = self.read_messages(&agent_id, unread_only);
                let total = all_msgs.len();
                let off = offset.unwrap_or(0);
                let lim = limit.unwrap_or(total);
                let page: Vec<_> = all_msgs.into_iter().skip(off).take(lim).collect();
                let mut r = ApiResponse::ok();
                r.messages = Some(page.clone());
                r.total_count = Some(total);
                r.has_more = Some(off + page.len() < total);
                r
            }
            ApiRequest::AckMessage { agent_id, message_id } => {
                if self.ack_message(&agent_id, &message_id) {
                    ApiResponse::ok()
                } else {
                    ApiResponse::error(format!("message not found: {}", message_id))
                }
            }
            // Tool operations — delegated to execute_tool
            ApiRequest::ToolCall { tool, params, agent_id } => {
                let result = self.execute_tool(&tool, &params, &agent_id);
                let mut r = ApiResponse::ok();
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
            ApiRequest::RememberProcedural { agent_id, name, description, steps, learned_from, tags, scope } => {
                let proc_steps: Vec<crate::memory::layered::ProcedureStep> = steps.into_iter().enumerate().map(|(i, s)| {
                    crate::memory::layered::ProcedureStep {
                        step_number: (i + 1) as u32,
                        description: s.description,
                        action: s.action,
                        expected_outcome: s.expected_outcome.unwrap_or_default(),
                    }
                }).collect();
                let scope = parse_scope(scope);
                match self.remember_procedural_scoped(&agent_id, name, description, proc_steps, learned_from.unwrap_or_default(), tags, scope) {
                    Ok(entry_id) => {
                        let mut r = ApiResponse::ok();
                        r.data = Some(serde_json::json!({"entry_id": entry_id}).to_string());
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::RecallProcedural { agent_id, name } => {
                let entries = self.recall_procedural(&agent_id, name.as_deref());
                let mut r = ApiResponse::ok();
                let data: Vec<serde_json::Value> = entries.iter().map(|e| {
                    match &e.content {
                        crate::memory::MemoryContent::Procedure(p) => {
                            serde_json::json!({
                                "id": e.id,
                                "tier": "procedural",
                                "name": p.name,
                                "description": p.description,
                                "steps": p.steps.iter().map(|s| serde_json::json!({
                                    "step_number": s.step_number,
                                    "description": s.description,
                                    "action": s.action,
                                    "expected_outcome": s.expected_outcome,
                                })).collect::<Vec<_>>(),
                                "learned_from": p.learned_from,
                                "tags": e.tags,
                                "importance": e.importance,
                                "scope": format!("{:?}", e.scope),
                            })
                        }
                        _ => serde_json::json!({
                            "id": e.id,
                            "tier": "procedural",
                            "content": e.content.display(),
                            "tags": e.tags,
                            "importance": e.importance,
                            "scope": format!("{:?}", e.scope),
                        })
                    }
                }).collect();
                r.data = Some(serde_json::to_string(&data).unwrap_or_default());
                r
            }
            ApiRequest::RecallVisible { agent_id, groups } => {
                let entries = self.recall_visible(&agent_id, &groups);
                let memories: Vec<String> = entries.into_iter()
                    .map(|m| format!("[{}:{:?}] {}", m.tier.name(), m.scope, m.content.display()))
                    .collect();
                let mut r = ApiResponse::ok();
                r.memory = Some(memories);
                r
            }
            // Agent resource management
            ApiRequest::AgentSetResources { agent_id, memory_quota, cpu_time_quota, allowed_tools, caller_agent_id: _ } => {
                match self.agent_set_resources(&agent_id, memory_quota, cpu_time_quota, allowed_tools) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            // Agent checkpoint & restore
            ApiRequest::AgentCheckpoint { agent_id } => {
                match self.checkpoint_agent(&agent_id) {
                    Ok(cid) => {
                        let mut r = ApiResponse::ok();
                        r.data = Some(cid);
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::AgentRestore { agent_id, checkpoint_cid } => {
                match self.restore_agent_checkpoint(&agent_id, &checkpoint_cid) {
                    Ok(count) => {
                        let mut r = ApiResponse::ok();
                        r.data = Some(format!("{} entries restored", count));
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            // ── Graph CRUD extensions (v0.7) ─────────────────────────────
            ApiRequest::GetNode { node_id, agent_id } => {
                match self.kg_get_node(&node_id, &agent_id) {
                    Ok(Some(n)) => {
                        let dto = crate::api::semantic::KGNodeDto {
                            id: n.id, label: n.label, node_type: n.node_type,
                            content_cid: n.content_cid, properties: n.properties,
                            agent_id: n.agent_id, created_at: n.created_at,
                        };
                        ApiResponse::with_nodes(vec![dto])
                    }
                    Ok(None) => ApiResponse::error(format!("node not found: {}", node_id)),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::ListEdges { agent_id, node_id, limit, offset } => {
                match self.kg_list_edges(&agent_id, node_id.as_deref()) {
                    Ok(edges) => {
                        let total = edges.len();
                        let off = offset.unwrap_or(0);
                        let lim = limit.unwrap_or(total);
                        let dto: Vec<crate::api::semantic::KGEdgeDto> = edges.into_iter().skip(off).take(lim).map(|e| {
                            crate::api::semantic::KGEdgeDto {
                                src: e.src, dst: e.dst, edge_type: e.edge_type,
                                weight: e.weight, created_at: e.created_at,
                            }
                        }).collect();
                        let mut r = ApiResponse::ok();
                        r.edges = Some(dto.clone());
                        r.total_count = Some(total);
                        r.has_more = Some(off + dto.len() < total);
                        r
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::RemoveNode { node_id, agent_id } => {
                match self.kg_remove_node(&node_id, &agent_id) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::RemoveEdge { src_id, dst_id, edge_type, agent_id } => {
                match self.kg_remove_edge(&src_id, &dst_id, edge_type, &agent_id) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::UpdateNode { node_id, label, properties, agent_id } => {
                match self.kg_update_node(&node_id, label.as_deref(), properties, &agent_id) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            // ── Agent lifecycle extensions (v0.7) ────────────────────────
            ApiRequest::AgentComplete { agent_id } => {
                match self.agent_complete(&agent_id) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::AgentFail { agent_id, reason } => {
                match self.agent_fail(&agent_id, &reason) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            // ── Memory tier management (v0.7) ────────────────────────────
            ApiRequest::MemoryMove { agent_id, entry_id, target_tier } => {
                let tier = match target_tier.as_str() {
                    "ephemeral" => crate::memory::MemoryTier::Ephemeral,
                    "working" => crate::memory::MemoryTier::Working,
                    "long_term" => crate::memory::MemoryTier::LongTerm,
                    "procedural" => crate::memory::MemoryTier::Procedural,
                    _ => return ApiResponse::error(format!("unknown tier: {}", target_tier)),
                };
                if self.memory_move(&agent_id, &entry_id, tier) {
                    ApiResponse::ok()
                } else {
                    ApiResponse::error(format!("memory entry not found: {}", entry_id))
                }
            }
            ApiRequest::MemoryDeleteEntry { agent_id, entry_id } => {
                if self.memory_delete(&agent_id, &entry_id) {
                    ApiResponse::ok()
                } else {
                    ApiResponse::error(format!("memory entry not found: {}", entry_id))
                }
            }
            ApiRequest::EvictExpired { agent_id } => {
                let count = self.evict_expired(&agent_id);
                let mut r = ApiResponse::ok();
                r.data = Some(format!("{}", count));
                r
            }

            ApiRequest::LoadContext { cid, layer, agent_id } => {
                let ctx_layer = match crate::fs::ContextLayer::parse_layer(&layer) {
                    Some(l) => l,
                    None => return ApiResponse::error(format!("Invalid layer '{}'. Use L0, L1, or L2.", layer)),
                };
                match self.context_load(&cid, ctx_layer, &agent_id) {
                    Ok(loaded) => {
                        let mut r = ApiResponse::ok();
                        r.context_data = Some(crate::api::semantic::LoadedContextDto {
                            cid: loaded.cid,
                            layer: loaded.layer.name().to_string(),
                            content: loaded.content,
                            tokens_estimate: loaded.tokens_estimate,
                        });
                        r
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }

            ApiRequest::EdgeHistory { src_id, dst_id, edge_type, agent_id } => {
                match self.kg_edge_history(&src_id, &dst_id, edge_type, &agent_id) {
                    Ok(edges) => {
                        let dtos: Vec<crate::api::semantic::KGEdgeDto> = edges.iter().map(|e| {
                            crate::api::semantic::KGEdgeDto {
                                src: e.src.clone(),
                                dst: e.dst.clone(),
                                edge_type: e.edge_type,
                                weight: e.weight,
                                created_at: e.created_at,
                            }
                        }).collect();
                        let mut r = ApiResponse::ok();
                        r.edges = Some(dtos);
                        r
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
        }
    }
}

fn parse_scope(scope: Option<String>) -> MemoryScope {
    match scope.as_deref() {
        None | Some("private") => MemoryScope::Private,
        Some("shared") => MemoryScope::Shared,
        Some(g) if g.starts_with("group:") => MemoryScope::Group(g[6..].to_string()),
        Some(_) => MemoryScope::Private,
    }
}
