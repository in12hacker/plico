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
pub mod event_bus;
mod persistence;
pub mod ops;

use ops::prefetch::IntentPrefetcher;
use ops::model::{HotSwapEmbeddingProvider, HotSwapLlmProvider};
use ops::observability::{KernelMetrics, OperationTimer, OpType};

use crate::api::semantic::{ApiRequest, ApiResponse};
use crate::api::agent_auth::AgentKeyStore;

use std::path::PathBuf;
use std::sync::{Arc, RwLock, atomic::{AtomicU64, Ordering}};

use crate::cas::CASStorage;
use crate::memory::{LayeredMemory, MemoryScope, CASPersister, MemoryPersister};
use crate::scheduler::{AgentScheduler, IntentPriority};
use crate::scheduler::messaging::MessageBus;
use crate::fs::{SemanticFS, InMemoryBackend, HnswBackend, EmbeddingProvider, SemanticSearch, LlmSummarizer, Summarizer, KnowledgeGraph, PetgraphBackend, StubEmbeddingProvider};
use crate::llm::LlmProvider;
use crate::api::permission::PermissionGuard;
use crate::tool::ToolRegistry;
use crate::kernel::event_bus::EventBus;

/// The AI Kernel — all subsystems wired together.
pub struct AIKernel {
    pub(crate) root: PathBuf,
    pub(crate) cas: Arc<CASStorage>,
    pub(crate) memory: Arc<LayeredMemory>,
    pub(crate) scheduler: Arc<AgentScheduler>,
    pub(crate) fs: Arc<SemanticFS>,
    pub(crate) permissions: Arc<PermissionGuard>,
    pub(crate) memory_persister: Option<Arc<dyn MemoryPersister + Send + Sync>>,
    /// Embedding provider — hot-swap wrapper for runtime model switching (v18.0).
    pub(crate) embedding: HotSwapEmbeddingProvider,
    /// LLM provider for summarization — hot-swap wrapper for runtime model switching (v18.0).
    pub(crate) llm_provider: HotSwapLlmProvider,
    #[allow(dead_code)]
    pub(crate) summarizer: Option<Arc<dyn Summarizer>>,
    pub(crate) knowledge_graph: Option<Arc<dyn KnowledgeGraph>>,
    pub(crate) search_backend: Arc<dyn SemanticSearch>,
    /// Counter for search index auto-persist. Every SEARCH_PERSIST_EVERY_N operations,
    /// the search index snapshot is saved to disk for crash recovery.
    search_op_count: Arc<AtomicU64>,
    pub(crate) tool_registry: Arc<ToolRegistry>,
    pub(crate) message_bus: Arc<MessageBus>,
    pub(crate) event_bus: Arc<EventBus>,
    /// Proactive context assembly — semantic prefetch engine.
    pub(crate) prefetch: Arc<ops::prefetch::IntentPrefetcher>,
    /// Agent authentication — cryptographic token store.
    pub(crate) key_store: Arc<AgentKeyStore>,
    /// Tenant registry — manages all tenants in the system.
    pub(crate) tenant_store: Arc<ops::tenant::TenantStore>,
    /// Observability metrics — operation counters and latency histograms (v14.0).
    pub(crate) metrics: Arc<KernelMetrics>,
}

impl AIKernel {
    /// Initialize the AI Kernel with the given storage root.
    pub fn new(root: PathBuf) -> std::io::Result<Self> {
        let cas = Arc::new(CASStorage::new(root.join("cas"))?);

        let embedding_raw: Arc<dyn EmbeddingProvider> =
            persistence::create_embedding_provider().unwrap_or_else(|e| {
                tracing::warn!("Embedding backend failed: {e}. Using stub (tag-only search).");
                Arc::new(StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>
            });
        // Wrap in RwLock for hot-swap support (RwLock stores Arc for cloneability), then wrap in HotSwapEmbeddingProvider
        let embedding_inner: Arc<RwLock<Arc<dyn EmbeddingProvider>>> =
            Arc::new(RwLock::new(embedding_raw));
        let embedding = HotSwapEmbeddingProvider::new(embedding_inner.clone());

        let llm_raw: Arc<dyn LlmProvider> = match persistence::create_llm_provider("PLICO_SUMMARIZER_MODEL", "llama3.2") {
            Ok(provider) => {
                tracing::info!("LLM summarizer enabled: {}", provider.model_name());
                provider
            }
            Err(e) => {
                tracing::warn!("Could not create LLM provider: {e}. Using stub provider.");
                Arc::new(crate::llm::StubProvider::empty()) as Arc<dyn LlmProvider>
            }
        };
        // Wrap in RwLock for hot-swap support (RwLock stores Arc for cloneability), then wrap in HotSwapLlmProvider
        let llm_inner: Arc<RwLock<Arc<dyn LlmProvider>>> =
            Arc::new(RwLock::new(llm_raw));
        let llm_provider = HotSwapLlmProvider::new(llm_inner.clone());

        let summarizer: Option<Arc<dyn Summarizer>> = {
            // Get the actual LLM provider from the inner Arc
            let llm_provider = llm_inner.read().unwrap().clone();
            Some(Arc::new(LlmSummarizer::new(llm_provider)) as Arc<dyn Summarizer>)
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
            Arc::new(embedding.clone()) as Arc<dyn EmbeddingProvider>,
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
        let event_bus = Arc::new(EventBus::new());

        // Proactive context assembly (semantic prefetch)
        let prefetch = Arc::new(IntentPrefetcher::new(
            search_backend.clone(),
            knowledge_graph.clone(),
            memory.clone(),
            event_bus.clone(),
            Arc::new(embedding.clone()) as Arc<dyn EmbeddingProvider>,
            fs.ctx_loader_arc(),
        ));

        // Agent authentication — cryptographic token store
        let key_store = Arc::new(AgentKeyStore::new());

        // Tenant registry — manages all tenants in the system
        let tenant_store = Arc::new(ops::tenant::TenantStore::new());

        // Observability metrics — operation counters and latency histograms (v14.0)
        let metrics = Arc::new(KernelMetrics::new());

        let kernel = Self {
            root: root.clone(),
            cas,
            memory,
            scheduler,
            fs,
            permissions,
            memory_persister: persister,
            embedding,
            llm_provider,
            summarizer,
            knowledge_graph,
            search_backend,
            search_op_count: Arc::new(AtomicU64::new(0)),
            tool_registry,
            message_bus,
            event_bus,
            prefetch,
            key_store,
            tenant_store,
            metrics,
        };

        kernel.register_builtin_tools();
        kernel.restore_agents();
        kernel.restore_intents();
        kernel.restore_memories();
        kernel.restore_permissions();
        kernel.restore_event_log();

        Ok(kernel)
    }

    /// Auto-persist hook: call after each write operation (create/update/delete).
    /// Persists the search index snapshot every N operations to prevent
    /// loss of real embeddings if the process crashes.
    const SEARCH_PERSIST_EVERY_N: u64 = 50;
    const EVENT_LOG_PERSIST_EVERY_N: u64 = 100;

    fn maybe_persist_search_index(&self) {
        let count = self.search_op_count.fetch_add(1, Ordering::Relaxed) + 1;
        if count.is_multiple_of(Self::SEARCH_PERSIST_EVERY_N) {
            self.persist_search_index();
        }
    }

    fn maybe_persist_event_log(&self) {
        let count = self.event_bus.event_count() as u64;
        if count > 0 && count.is_multiple_of(Self::EVENT_LOG_PERSIST_EVERY_N) {
            self.persist_event_log();
        }
    }

    pub fn event_subscribe(&self) -> String {
        self.event_bus.subscribe()
    }

    pub fn event_subscribe_filtered(&self, filter: Option<event_bus::EventFilter>) -> String {
        self.event_bus.subscribe_filtered(filter)
    }

    pub fn event_poll(&self, subscription_id: &str) -> Option<Vec<event_bus::KernelEvent>> {
        self.event_bus.poll(subscription_id)
    }

    /// Get the kernel metrics for observability (v14.0).
    pub fn metrics(&self) -> &KernelMetrics {
        &self.metrics
    }

    pub fn event_unsubscribe(&self, subscription_id: &str) -> bool {
        self.event_bus.unsubscribe(subscription_id)
    }

    // ─── API Request Handler ───────────────────────────────────────────

    pub fn handle_api_request(&self, req: crate::api::semantic::ApiRequest) -> crate::api::semantic::ApiResponse {
        use crate::api::semantic::{
            SearchResultDto, AgentDto, NeighborDto, DeletedDto,
            KGNodeDto,
        };
        use crate::api::semantic::ContentEncoding;

        // Generate correlation ID for distributed tracing (v14.0)
        let correlation_id = ops::observability::CorrelationId::new();
        let _timer = OperationTimer::new(&self.metrics, OpType::HandleApiRequest);
        let span = tracing::info_span!(
            "handle_api_request",
            operation = "handle_api_request",
            correlation_id = %correlation_id,
        );
        let _guard = span.enter();

        fn decode_content(content: &str, encoding: &ContentEncoding) -> Result<Vec<u8>, String> {
            crate::api::semantic::decode_content(content, encoding)
        }

        let _corr_id = correlation_id; // Used in tracing span above
        let response = match req {
            ApiRequest::Create { content, content_encoding, tags, agent_id, agent_token, intent, .. } => {
                // Verify token (Optional mode: allow no token)
                if let Err(e) = self.key_store.verify_agent_token(&agent_id, agent_token.as_deref()) {
                    return ApiResponse::error(e);
                }
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
            ApiRequest::Read { cid, agent_id, agent_token, tenant_id, .. } => {
                if let Err(e) = self.key_store.verify_agent_token(&agent_id, agent_token.as_deref()) {
                    return ApiResponse::error(e);
                }
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                match self.get_object(&cid, &agent_id, &tenant) {
                    Ok(obj) => ApiResponse::with_data(String::from_utf8_lossy(&obj.data).to_string()),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::Search { query, agent_id, agent_token, tenant_id, limit, offset, require_tags, exclude_tags, since, until, .. } => {
                if let Err(e) = self.key_store.verify_agent_token(&agent_id, agent_token.as_deref()) {
                    return ApiResponse::error(e);
                }
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                let results = match self.semantic_search_with_time(
                    &query, &agent_id, &tenant, limit.unwrap_or(10) + offset.unwrap_or(0),
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
            ApiRequest::Update { cid, content, content_encoding, new_tags, agent_id, agent_token, tenant_id, .. } => {
                if let Err(e) = self.key_store.verify_agent_token(&agent_id, agent_token.as_deref()) {
                    return ApiResponse::error(e);
                }
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                let bytes = match decode_content(&content, &content_encoding) {
                    Ok(b) => b,
                    Err(e) => return ApiResponse::error(e),
                };
                match self.semantic_update(&cid, bytes, new_tags, &agent_id, &tenant) {
                    Ok(new_cid) => {
                        self.maybe_persist_search_index();
                        ApiResponse::with_cid(new_cid)
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::Delete { cid, agent_id, agent_token, tenant_id, .. } => {
                if let Err(e) = self.key_store.verify_agent_token(&agent_id, agent_token.as_deref()) {
                    return ApiResponse::error(e);
                }
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                match self.semantic_delete(&cid, &agent_id, &tenant) {
                    Ok(()) => {
                        self.maybe_persist_search_index();
                        ApiResponse::ok()
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::RegisterAgent { name } => {
                let id = self.register_agent(name);
                // Generate cryptographic token for this agent
                let token = self.key_store.generate_token(&id);
                self.key_store.store_token(&token);
                let mut r = ApiResponse::ok();
                r.agent_id = Some(id);
                r.token = Some(token.token);
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
            ApiRequest::Remember { agent_id, content, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                match self.remember(&agent_id, &tenant, content) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::Recall { agent_id } => {
                let memories: Vec<String> = self.recall(&agent_id, "default").into_iter()
                    .filter_map(|m| match m.content {
                        crate::memory::MemoryContent::Text(t) => Some(t),
                        _ => None,
                    }).collect();
                let mut r = ApiResponse::ok();
                r.memory = Some(memories);
                r
            }
            ApiRequest::RememberLongTerm { agent_id, content, tags, importance, scope, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                let scope = parse_scope(scope);
                match self.remember_long_term_scoped(&agent_id, &tenant, content, tags, importance, scope) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::RecallSemantic { agent_id, query, k } => {
                match self.recall_semantic(&agent_id, "default", &query, k) {
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
            ApiRequest::AddNode { label, node_type, properties, agent_id, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                match self.kg_add_node(&label, node_type, properties, &agent_id, &tenant) {
                    Ok(id) => ApiResponse::with_node_id(id),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::AddEdge { src_id, dst_id, edge_type, weight, agent_id, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                match self.kg_add_edge(&src_id, &dst_id, edge_type, weight, &agent_id, &tenant) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::ListNodes { node_type, agent_id, tenant_id, limit, offset, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                let nodes = match self.kg_list_nodes(node_type, &agent_id, &tenant) {
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
            ApiRequest::ListNodesAtTime { node_type, agent_id, tenant_id, t, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                let nodes = match self.kg_get_valid_nodes_at(&agent_id, &tenant, node_type, t) {
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
            ApiRequest::FindPaths { src_id, dst_id, max_depth, weighted, agent_id: _, .. } => {
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
                match self.remember_procedural_scoped(&agent_id, "default", name, description, proc_steps, learned_from.unwrap_or_default(), tags, scope) {
                    Ok(entry_id) => {
                        let mut r = ApiResponse::ok();
                        r.data = Some(serde_json::json!({"entry_id": entry_id}).to_string());
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::RecallProcedural { agent_id, name } => {
                let entries = self.recall_procedural(&agent_id, "default", name.as_deref());
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
                let entries = self.recall_visible(&agent_id, "default", &groups);
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
            ApiRequest::GetNode { node_id, agent_id, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                match self.kg_get_node(&node_id, &agent_id, &tenant) {
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
            ApiRequest::ListEdges { agent_id, node_id, tenant_id, limit, offset, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                match self.kg_list_edges(&agent_id, &tenant, node_id.as_deref()) {
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
            ApiRequest::RemoveNode { node_id, agent_id, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                match self.kg_remove_node(&node_id, &agent_id, &tenant) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::RemoveEdge { src_id, dst_id, edge_type, agent_id, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                match self.kg_remove_edge(&src_id, &dst_id, edge_type, &agent_id, &tenant) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::UpdateNode { node_id, label, properties, agent_id, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                match self.kg_update_node(&node_id, label.as_deref(), properties, &agent_id, &tenant) {
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
            ApiRequest::MemoryMove { agent_id, entry_id, target_tier, .. } => {
                let tier = match target_tier.as_str() {
                    "ephemeral" => crate::memory::MemoryTier::Ephemeral,
                    "working" => crate::memory::MemoryTier::Working,
                    "long_term" => crate::memory::MemoryTier::LongTerm,
                    "procedural" => crate::memory::MemoryTier::Procedural,
                    _ => return ApiResponse::error(format!("unknown tier: {}", target_tier)),
                };
                // Note: memory_move doesn't have tenant_id in current signature, using "default"
                if self.memory_move(&agent_id, "default", &entry_id, tier) {
                    ApiResponse::ok()
                } else {
                    ApiResponse::error(format!("memory entry not found: {}", entry_id))
                }
            }
            ApiRequest::MemoryDeleteEntry { agent_id, entry_id, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                if self.memory_delete(&agent_id, &tenant, &entry_id) {
                    ApiResponse::ok()
                } else {
                    ApiResponse::error(format!("memory entry not found: {}", entry_id))
                }
            }
            ApiRequest::EvictExpired { agent_id, .. } => {
                let count = self.evict_expired(&agent_id);
                let mut r = ApiResponse::ok();
                r.data = Some(format!("{}", count));
                r
            }

            ApiRequest::LoadContext { cid, layer, agent_id, .. } => {
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

            ApiRequest::EdgeHistory { src_id, dst_id, edge_type, agent_id, tenant_id, .. } => {
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                match self.kg_edge_history(&src_id, &dst_id, edge_type, &agent_id, &tenant) {
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

            // ── Event Bus (v5.0) ───────────────────────────────────────

            ApiRequest::EventSubscribe { agent_id: _, event_types, agent_ids } => {
                let filter = if event_types.is_some() || agent_ids.is_some() {
                    Some(event_bus::EventFilter { event_types, agent_ids })
                } else {
                    None
                };
                let sub_id = self.event_subscribe_filtered(filter);
                let mut r = ApiResponse::ok();
                r.subscription_id = Some(sub_id);
                r
            }
            ApiRequest::EventPoll { subscription_id } => {
                match self.event_poll(&subscription_id) {
                    Some(events) => {
                        let mut r = ApiResponse::ok();
                        r.kernel_events = Some(events);
                        r
                    }
                    None => ApiResponse::error(format!("Unknown subscription: {}", subscription_id)),
                }
            }
            ApiRequest::EventUnsubscribe { subscription_id } => {
                if self.event_unsubscribe(&subscription_id) {
                    ApiResponse::ok()
                } else {
                    ApiResponse::error(format!("Unknown subscription: {}", subscription_id))
                }
            }
            ApiRequest::SystemStatus => {
                let status = self.system_status();
                let mut r = ApiResponse::ok();
                r.system_status = Some(status);
                r
            }
            ApiRequest::ContextAssemble { agent_id, cids, budget_tokens } => {
                let candidates: Vec<crate::fs::context_budget::ContextCandidate> = cids
                    .into_iter()
                    .map(|c| crate::fs::context_budget::ContextCandidate {
                        cid: c.cid,
                        relevance: c.relevance,
                    })
                    .collect();
                match self.context_assemble(&candidates, budget_tokens, &agent_id) {
                    Ok(allocation) => {
                        let mut r = ApiResponse::ok();
                        r.context_assembly = Some(allocation);
                        r
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }

            ApiRequest::AgentUsage { agent_id } => {
                match self.agent_usage(&agent_id) {
                    Some(usage) => {
                        let mut r = ApiResponse::ok();
                        r.agent_usage = Some(usage);
                        r
                    }
                    None => ApiResponse::error(format!("Agent not found: {}", agent_id)),
                }
            }

            ApiRequest::DiscoverAgents { state_filter, tool_filter, agent_id: _ } => {
                let cards = self.discover_agents(
                    state_filter.as_deref(),
                    tool_filter.as_deref(),
                );
                let mut r = ApiResponse::ok();
                r.agent_cards = Some(cards);
                r
            }

            ApiRequest::DelegateTask { from, to, description, action, priority } => {
                let p = match priority.to_lowercase().as_str() {
                    "critical" => IntentPriority::Critical,
                    "high" => IntentPriority::High,
                    "medium" => IntentPriority::Medium,
                    _ => IntentPriority::Low,
                };
                match self.delegate_task(&from, &to, description, action, p) {
                    Ok((intent_id, msg_id)) => {
                        let mut r = ApiResponse::ok();
                        r.delegation = Some(crate::api::semantic::DelegationResultDto {
                            intent_id,
                            message_id: msg_id,
                            from,
                            to,
                        });
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }

            ApiRequest::EventHistory { since_seq, agent_id_filter, limit } => {
                let events = match (&since_seq, &agent_id_filter) {
                    (_, Some(aid)) => {
                        let mut evts = self.event_bus.events_by_agent(aid);
                        if let Some(seq) = since_seq {
                            evts.retain(|e| e.seq > seq);
                        }
                        evts
                    }
                    (Some(seq), None) => self.event_bus.events_since(*seq),
                    (None, None) => self.event_bus.snapshot_events(),
                };
                let limited = if let Some(lim) = limit {
                    events.into_iter().take(lim).collect()
                } else {
                    events
                };
                let mut r = ApiResponse::ok();
                r.event_history = Some(limited);
                r
            }

            ApiRequest::RegisterSkill { agent_id, name, description, tags } => {
                match self.register_skill(&agent_id, &name, &description, tags) {
                    Ok(node_id) => {
                        let mut r = ApiResponse::ok();
                        r.node_id = Some(node_id);
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }

            ApiRequest::DiscoverSkills { query, agent_id_filter, tag_filter } => {
                let skills = self.discover_skills(
                    query.as_deref(),
                    agent_id_filter.as_deref(),
                    tag_filter.as_deref(),
                );
                let mut r = ApiResponse::ok();
                r.discovered_skills = Some(skills);
                r
            }

            // ── Proactive Context Assembly (F-2) ─────────────────────────

            ApiRequest::DeclareIntent { agent_id, intent, related_cids, budget_tokens } => {
                match self.declare_intent(&agent_id, &intent, related_cids, budget_tokens) {
                    Ok(assembly_id) => {
                        let mut r = ApiResponse::ok();
                        r.assembly_id = Some(assembly_id);
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }

            ApiRequest::FetchAssembledContext { agent_id, assembly_id } => {
                match self.fetch_assembled_context(&agent_id, &assembly_id) {
                    Some(Ok(allocation)) => {
                        let mut r = ApiResponse::ok();
                        r.context_assembly = Some(allocation);
                        r
                    }
                    Some(Err(e)) => ApiResponse::error(e),
                    None => ApiResponse::error(format!("assembly not found: {}", assembly_id)),
                }
            }

            // ── Tenant Management (Phase 3C) ──────────────────────────────

            ApiRequest::CreateTenant { tenant_id, admin_agent_id, caller_agent_id } => {
                // Only trusted agents (kernel, system) or existing admins can create tenants
                if !self.permissions.is_trusted(&caller_agent_id) {
                    // Check if caller has CrossTenant permission (needed for tenant management)
                    let ctx = crate::api::permission::PermissionContext::new(
                        caller_agent_id.clone(),
                        "default".to_string(),
                    );
                    if let Err(e) = self.permissions.check(&ctx, crate::api::permission::PermissionAction::CrossTenant) {
                        return ApiResponse::error(format!(
                            "Agent '{}' cannot create tenants: {}. Only trusted agents or those with CrossTenant permission can create tenants.",
                            caller_agent_id, e
                        ));
                    }
                }
                match self.tenant_store.create(&tenant_id, &admin_agent_id) {
                    Ok(_tenant) => {
                        // Tenant created successfully
                        ApiResponse::ok()
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }

            ApiRequest::ListTenants { agent_id } => {
                let tenants = self.tenant_store.list_for_agent(&agent_id);
                let dtos: Vec<crate::api::semantic::TenantDto> = tenants.into_iter().map(|t| {
                    crate::api::semantic::TenantDto {
                        id: t.id,
                        admin_agent_id: t.admin_agent_id,
                        created_at_ms: t.created_at_ms,
                    }
                }).collect();
                let mut r = ApiResponse::ok();
                r.tenants = Some(dtos);
                r
            }

            ApiRequest::TenantShare { from_tenant, to_tenant, resource_type, resource_pattern, agent_id } => {
                // Validate resource type
                if !crate::kernel::ops::tenant::TenantShare::is_valid_resource_type(&resource_type) {
                    return ApiResponse::error(format!(
                        "Invalid resource_type '{}'. Must be 'kg', 'memory', or 'cas'.",
                        resource_type
                    ));
                }

                // Check CrossTenant permission for the agent
                let ctx = crate::api::permission::PermissionContext::new(
                    agent_id.clone(),
                    from_tenant.clone(),
                );
                if let Err(e) = self.permissions.check(&ctx, crate::api::permission::PermissionAction::CrossTenant) {
                    return ApiResponse::error(format!(
                        "Agent '{}' in tenant '{}' cannot share resources with tenant '{}': {}. CrossTenant permission required.",
                        agent_id, from_tenant, to_tenant, e
                    ));
                }

                // Verify both tenants exist
                if !self.tenant_store.exists(&from_tenant) {
                    return ApiResponse::error(format!("Source tenant '{}' does not exist.", from_tenant));
                }
                if !self.tenant_store.exists(&to_tenant) {
                    return ApiResponse::error(format!("Destination tenant '{}' does not exist.", to_tenant));
                }

                // TODO: Implement actual resource sharing logic for each resource type.
                // For now, we just validate that the operation is authorized.
                let mut r = ApiResponse::ok();
                r.data = Some(serde_json::json!({
                    "message": format!(
                        "Share {} resources matching '{}' from tenant '{}' to tenant '{}' initiated by agent '{}'.",
                        resource_type, resource_pattern, from_tenant, to_tenant, agent_id
                    ),
                    "from_tenant": from_tenant,
                    "to_tenant": to_tenant,
                    "resource_type": resource_type,
                    "resource_pattern": resource_pattern,
                }).to_string());
                r
            }

            // ── Batch Operations (v15.0) ─────────────────────────────────

            ApiRequest::BatchCreate { items, agent_id, tenant_id } => {
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                let batch_results = self.handle_batch_create(items, &agent_id, &tenant);
                let mut r = ApiResponse::ok();
                r.batch_create = Some(batch_results);
                r
            }

            ApiRequest::BatchMemoryStore { entries, agent_id, tenant_id } => {
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                let batch_results = self.handle_batch_memory_store(entries, &agent_id, &tenant);
                let mut r = ApiResponse::ok();
                r.batch_memory_store = Some(batch_results);
                r
            }

            ApiRequest::BatchSubmitIntent { intents, agent_id } => {
                let batch_results = self.handle_batch_submit_intent(intents, &agent_id);
                let mut r = ApiResponse::ok();
                r.batch_submit_intent = Some(batch_results);
                r
            }

            ApiRequest::BatchQuery { queries, agent_id, tenant_id } => {
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                let batch_results = self.handle_batch_query(queries, &agent_id, &tenant);
                let mut r = ApiResponse::ok();
                r.batch_query = Some(batch_results);
                r
            }

            // ── KG Causal Reasoning (v16.0) ─────────────────────────────────

            ApiRequest::KGCausalPath { source_id, target_id, max_depth, agent_id: _, tenant_id: _ } => {
                let depth = max_depth.max(1).min(5);
                let paths = self.kg_find_causal_path(&source_id, &target_id, depth);
                let dtos: Vec<crate::api::semantic::CausalPathDto> = paths.into_iter().map(|p| {
                    crate::api::semantic::CausalPathDto {
                        nodes: p.nodes.into_iter().map(|n| crate::api::semantic::KGNodeDto {
                            id: n.id, label: n.label, node_type: n.node_type,
                            content_cid: n.content_cid, properties: n.properties,
                            agent_id: n.agent_id, created_at: n.created_at,
                        }).collect(),
                        edges: p.edges.into_iter().map(|e| crate::api::semantic::KGEdgeDto {
                            src: e.src, dst: e.dst, edge_type: e.edge_type,
                            weight: e.weight, created_at: e.created_at,
                        }).collect(),
                        causal_strength: p.causal_strength,
                    }
                }).collect();
                let mut r = ApiResponse::ok();
                r.causal_paths = Some(dtos);
                r
            }

            ApiRequest::KGImpactAnalysis { node_id, propagation_depth, agent_id, tenant_id } => {
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                let depth = propagation_depth.max(1).min(5);
                // Check tenant access first
                if let Ok(Some(node)) = self.kg_get_node(&node_id, &agent_id, &tenant) {
                    let ctx = crate::api::permission::PermissionContext::new(agent_id.clone(), tenant);
                    if let Err(e) = self.permissions.check_tenant_access(&ctx, &node.tenant_id) {
                        return ApiResponse::error(e.to_string());
                    }
                }
                let impact = self.kg_impact_analysis(&node_id, depth);
                let mut r = ApiResponse::ok();
                r.impact_analysis = Some(crate::api::semantic::ImpactAnalysisDto {
                    affected_nodes: impact.affected_nodes,
                    propagation_depth: impact.propagation_depth,
                    severity: impact.severity,
                });
                r
            }

            ApiRequest::KGTemporalChanges { from_ms, to_ms, agent_id, tenant_id } => {
                let tenant = tenant_id.unwrap_or_else(|| "default".to_string());
                match self.kg_temporal_changes(from_ms, to_ms, &agent_id, &tenant) {
                    Ok(changes) => {
                        let dtos: Vec<crate::api::semantic::TemporalChangeDto> = changes.into_iter().map(|c| {
                            crate::api::semantic::TemporalChangeDto {
                                before: c.before.map(|n| crate::api::semantic::KGNodeDto {
                                    id: n.id, label: n.label, node_type: n.node_type,
                                    content_cid: n.content_cid, properties: n.properties,
                                    agent_id: n.agent_id, created_at: n.created_at,
                                }),
                                after: c.after.map(|n| crate::api::semantic::KGNodeDto {
                                    id: n.id, label: n.label, node_type: n.node_type,
                                    content_cid: n.content_cid, properties: n.properties,
                                    agent_id: n.agent_id, created_at: n.created_at,
                                }),
                                change_type: match c.change_type {
                                    crate::kernel::ops::graph::ChangeType::Created => "created".to_string(),
                                    crate::kernel::ops::graph::ChangeType::Modified => "modified".to_string(),
                                    crate::kernel::ops::graph::ChangeType::Deleted => "deleted".to_string(),
                                },
                                timestamp_ms: c.timestamp_ms,
                            }
                        }).collect();
                        let mut r = ApiResponse::ok();
                        r.temporal_changes = Some(dtos);
                        r
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }

            // ── Model Hot-Swap (v18.0) ────────────────────────────────────

            ApiRequest::SwitchEmbeddingModel { model_type, model_id, python_path } => {
                match self.switch_embedding_model(&model_type, &model_id, python_path.as_deref()) {
                    Ok(resp) => {
                        let mut r = ApiResponse::ok();
                        r.model_switch = Some(resp);
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }

            ApiRequest::SwitchLlmModel { backend, model, url } => {
                match self.switch_llm_model(&backend, &model, url.as_deref()) {
                    Ok(resp) => {
                        let mut r = ApiResponse::ok();
                        r.model_switch = Some(resp);
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }

            ApiRequest::CheckModelHealth { model_type } => {
                let health = self.check_model_health(&model_type);
                let mut r = ApiResponse::ok();
                r.model_health = Some(health);
                r
            }
        };
        self.maybe_persist_event_log();
        response
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
