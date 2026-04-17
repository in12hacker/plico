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
use std::sync::Arc;

use crate::cas::CASStorage;
use crate::memory::{LayeredMemory, CASPersister, MemoryPersister};
use crate::scheduler::{AgentScheduler, IntentPriority};
use crate::scheduler::messaging::MessageBus;
use crate::fs::{SemanticFS, InMemoryBackend, EmbeddingProvider, SemanticSearch, OllamaSummarizer, Summarizer, KnowledgeGraph, PetgraphBackend, StubEmbeddingProvider};
use crate::api::permission::PermissionGuard;
use crate::tool::ToolRegistry;
use crate::intent::{ChainRouter, IntentRouter};

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
    pub(crate) search_backend: Arc<InMemoryBackend>,
    pub(crate) tool_registry: Arc<ToolRegistry>,
    pub(crate) intent_router: Arc<dyn IntentRouter>,
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

        let ollama_url = std::env::var("OLLAMA_URL").unwrap_or_else(|_| "http://localhost:11434".to_string());
        let summarizer_model = std::env::var("OLLAMA_SUMMARIZER_MODEL")
            .unwrap_or_else(|_| "llama3.2".to_string());
        let summarizer: Option<Arc<dyn Summarizer>> = match OllamaSummarizer::new(&ollama_url, &summarizer_model) {
            Ok(s) => {
                tracing::info!("LLM summarizer enabled: {} via {}", summarizer_model, ollama_url);
                Some(Arc::new(s))
            }
            Err(e) => {
                tracing::warn!("Could not create summarizer: {e}. ContextLoader will use heuristic summaries.");
                None
            }
        };

        let search_backend = Arc::new(InMemoryBackend::new());
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
        kernel.restore_agents();
        kernel.restore_intents();
        kernel.restore_memories();
        kernel.restore_search_index();

        Ok(kernel)
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
            // Intent resolution
            ApiRequest::IntentResolve { text, agent_id } => {
                match self.intent_router.resolve(&text, &agent_id) {
                    Ok(resolved) => {
                        let mut r = ApiResponse::ok();
                        r.resolved_intents = Some(resolved);
                        r
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            // Agent resource management
            ApiRequest::AgentSetResources { agent_id, memory_quota, cpu_time_quota, allowed_tools, caller_agent_id: _ } => {
                match self.agent_set_resources(&agent_id, memory_quota, cpu_time_quota, allowed_tools) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
        }
    }
}
