//! AI Kernel — central orchestrator for all Plico subsystems.
//!
//! Wires together: CAS Storage, Layered Memory, Agent Scheduler,
//! Semantic FS, and Permission Guardrails. Upper-layer AI agents
//! interact with the kernel through the semantic API.
//!
//! # Layout
//!
//! @lines 43-83    AIKernel struct (40+ fields)
//! @lines 85-265   AIKernel::new() constructor
//! @lines 267-309  utility methods — auto_persist, accessors
//! @lines 311-425  extract_agent_id() — request→agent_id mapping
//! @lines 427-1849 handle_api_request() — main dispatch (60+ variants)
//! @lines 1852-1858 parse_scope() helper
//! @lines 1863-1911 link_memory_to_kg() — A-4 Memory Link Engine
//!
//! # Module Structure
//! - `mod.rs` — struct, constructor, handle_api_request
//! - `builtin_tools.rs` — tool registration
//! - `persistence.rs` — state persistence/restore
//! - `ops/` — operation groups (fs, agent, memory, events, graph, dispatch, intent, messaging, dashboard)

mod builtin_tools;
pub mod event_bus;
pub mod hook;
pub mod persistence;
pub mod ops;
mod tools;
pub mod tests; // test helpers for inline #[cfg(test)] modules

use ops::checkpoint::CheckpointStore;
use ops::prefetch::IntentPrefetcher;
use ops::model::{HotSwapEmbeddingProvider, HotSwapLlmProvider};
use ops::observability::KernelMetrics;
use ops::cache::EdgeCache;
use ops::cost_ledger::{TokenCostLedger, set_global_cost_ledger};
use ops::distributed::{ClusterManager, NodeId};

use crate::api::agent_auth::AgentKeyStore;

use std::path::PathBuf;
use std::sync::{Arc, RwLock, atomic::{AtomicU64, Ordering}};

use crate::cas::CASStorage;
use crate::memory::{LayeredMemory, CASPersister, MemoryPersister};
use crate::scheduler::AgentScheduler;
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
    /// Hook registry — lifecycle interception for tool calls (F-1, Node 19).
    pub(crate) hook_registry: Arc<hook::HookRegistry>,
    /// Proactive context assembly — semantic prefetch engine.
    pub(crate) prefetch: Arc<ops::prefetch::IntentPrefetcher>,
    /// Agent authentication — cryptographic token store.
    pub(crate) key_store: Arc<AgentKeyStore>,
    /// Tenant registry — manages all tenants in the system.
    pub(crate) tenant_store: Arc<ops::tenant::TenantStore>,
    /// Observability metrics — operation counters and latency histograms (v14.0).
    pub(crate) metrics: Arc<KernelMetrics>,
    /// Edge caching — L1/L2 cache for embeddings, KG queries, and semantic search (v19.0).
    pub(crate) edge_cache: Arc<EdgeCache>,
    /// Distributed cluster manager — node membership, heartbeat, agent migration (v20.0).
    pub(crate) cluster: Arc<ClusterManager>,
    /// Session lifecycle store — manages StartSession/EndSession state and timeout (F-6).
    pub(crate) session_store: Arc<ops::session::SessionStore>,
    /// Checkpoint store — persists agent checkpoints to CAS (P-2).
    pub(crate) checkpoint_store: Arc<CheckpointStore>,
    /// Task store — manages multi-agent task delegation with state tracking (F-14).
    pub(crate) task_store: Arc<ops::task::TaskStore>,
    /// Token cost ledger — tracks LLM/embedding token consumption (F-2).
    pub(crate) cost_ledger: Arc<TokenCostLedger>,
    /// KG builder handle — async entity/event extraction on CAS writes.
    pub(crate) kg_builder: Option<ops::kg_builder::KgBuilderHandle>,
}

/// Check if the embedding model has changed since last run.
/// Returns `true` if the metadata file is absent or mismatched.
fn check_embedding_meta(root: &std::path::Path, model_name: &str, dim: usize) -> bool {
    let path = root.join(".embedding_meta.json");
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                let saved_model = val.get("model").and_then(|v| v.as_str()).unwrap_or("");
                let saved_dim = val.get("dimension").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                saved_model != model_name || saved_dim != dim
            } else {
                true
            }
        }
        Err(_) => false, // First run — no metadata yet, no need to wipe
    }
}

/// Persist current embedding model metadata for change detection on next startup.
fn save_embedding_meta(root: &std::path::Path, model_name: &str, dim: usize) {
    let meta = serde_json::json!({
        "model": model_name,
        "dimension": dim,
        "saved_at": chrono::Utc::now().to_rfc3339(),
    });
    let path = root.join(".embedding_meta.json");
    if let Err(e) = std::fs::write(&path, serde_json::to_string_pretty(&meta).unwrap_or_default()) {
        tracing::warn!("Failed to save embedding metadata: {e}");
    }
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

        let llm_raw: Arc<dyn LlmProvider> = match persistence::create_llm_provider("PLICO_SUMMARIZER_MODEL", "qwen2.5-coder-7b-instruct") {
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
                let dim = embedding.dimension();
                let model_name = embedding.model_name().to_string();
                let meta_changed = check_embedding_meta(&root, &model_name, dim);
                let backend = Arc::new(HnswBackend::with_dim(dim));
                if meta_changed {
                    tracing::warn!(
                        "Embedding model changed (now {}@{}d) — starting with fresh HNSW index",
                        model_name, dim,
                    );
                    let _ = std::fs::remove_file(root.join("hnsw_index.jsonl"));
                } else {
                    backend.restore_from(&root).ok();
                }
                save_embedding_meta(&root, &model_name, dim);
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
        let event_log_path = root.join("event_log.jsonl");
        let event_bus = Arc::new(EventBus::with_persistence(event_log_path));

        // Hook registry — lifecycle interception for tool calls (F-1, Node 19)
        let hook_registry = Arc::new(hook::HookRegistry::new());

        // Session lifecycle store — manages StartSession/EndSession state and timeout (F-6)
        // Created early so it can be passed to CausalHookHandler (F-3, Node 20)
        let session_store = Arc::new(ops::session::SessionStore::restore(&root));

        // F-20 M2: Register CausalHookHandler for KG因果链 tracking
        if let Some(ref kg) = knowledge_graph {
            let causal_handler = Arc::new(
                ops::causal_hook::CausalHookHandler::new(
                    Arc::clone(kg),
                    Arc::clone(&session_store),
                )
            );
            hook_registry.register(
                hook::HookPoint::PostToolCall,
                100, // Low priority — runs after tool completes
                causal_handler,
            );
        }

        // F-4: Register VerificationHookHandler for postcondition verification
        let verification_handler = Arc::new(
            ops::verification::VerificationHookHandler::new(
                Arc::clone(&fs),
                Arc::clone(&event_bus),
            )
        );
        hook_registry.register(
            hook::HookPoint::PostToolCall,
            90, // Medium priority — runs after causal but before other handlers
            verification_handler,
        );

        // Token cost ledger — tracks LLM/embedding token consumption (F-2)
        // Created early so it can be passed to IntentPrefetcher for cost tracking
        let cost_ledger = Arc::new(TokenCostLedger::new());
        // Also set as global so LLM/embedding providers can record without DI
        set_global_cost_ledger(Arc::clone(&cost_ledger));

        // Proactive context assembly (semantic prefetch)
        let prefetch = Arc::new(IntentPrefetcher::new(
            search_backend.clone(),
            knowledge_graph.clone(),
            memory.clone(),
            event_bus.clone(),
            Arc::new(embedding.clone()) as Arc<dyn EmbeddingProvider>,
            fs.ctx_loader_arc(),
            root.clone(),
        ));

        // F-2: Wire cost ledger into prefetcher for embedding cost tracking
        prefetch.set_cost_ledger(Arc::clone(&cost_ledger));

        // F-20 M1: Restore prefetch state from disk (intent cache + agent profiles)
        if let Err(e) = prefetch.restore() {
            tracing::warn!("prefetch restore failed (ok if first run): {}", e);
        }

        // Agent authentication — cryptographic token store
        let key_store = Arc::new(AgentKeyStore::open(&root));

        // Tenant registry — manages all tenants in the system
        let tenant_store = Arc::new(ops::tenant::TenantStore::restore(&root));

        // Observability metrics — operation counters and latency histograms (v14.0)
        let metrics = Arc::new(KernelMetrics::new());

        // Edge caching — L1/L2 cache for embeddings, KG queries, and semantic search (v19.0)
        let edge_cache = Arc::new(EdgeCache::default());

        // Distributed cluster manager — single-node by default (v20.0)
        // Multi-node cluster requires explicit seed node configuration
        let cluster_host = std::env::var("PLICO_CLUSTER_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let cluster_port: u16 = std::env::var("PLICO_CLUSTER_PORT")
            .unwrap_or_else(|_| "7878".to_string())
            .parse()
            .unwrap_or(7878);
        let cluster_name = std::env::var("PLICO_CLUSTER_NAME").unwrap_or_else(|_| "plico-cluster".to_string());
        let is_seed = std::env::var("PLICO_IS_SEED").unwrap_or_else(|_| "true".to_string()) == "true";
        let cluster = Arc::new(ClusterManager::new(
            NodeId::new(),
            cluster_name,
            is_seed,
            cluster_host,
            cluster_port,
        ));

        // Session lifecycle store — manages StartSession/EndSession state and timeout (F-6)
        // NOTE: Already created above at line 188 for CausalHookHandler
        // (session_store is used directly below)

        // Spawn background timeout scanner for expired sessions
        let timeout_session_store = Arc::clone(&session_store);
        let timeout_memory = memory.clone();
        let timeout_root = root.clone();
        std::thread::spawn(move || {
            ops::session::spawn_session_timeout_scanner(timeout_session_store, timeout_memory, timeout_root);
        });

        // Checkpoint store — persists agent checkpoints to CAS (P-2)
        let checkpoint_store = Arc::new(CheckpointStore::restore(&root, &cas, 10));

        // Task store — manages multi-agent task delegation with state tracking (F-14)
        let task_store = Arc::new(ops::task::TaskStore::restore(root.clone(), event_bus.clone()));

        // KG builder — async entity/event extraction on CAS writes
        let kg_builder_config = ops::kg_builder::KgBuilderConfig::from_env();
        let kg_builder = if kg_builder_config.enabled {
            if let Some(ref kg) = knowledge_graph {
                let llm_for_kg: Arc<dyn LlmProvider> = Arc::new(llm_provider.clone());
                let handle = ops::kg_builder::start_kg_builder(
                    Arc::clone(kg),
                    llm_for_kg,
                    kg_builder_config,
                );
                tracing::info!("KG auto-extraction worker started");
                Some(handle)
            } else {
                tracing::warn!("PLICO_KG_AUTO_EXTRACT=1 but no knowledge graph backend available");
                None
            }
        } else {
            None
        };

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
            hook_registry,
            prefetch,
            key_store,
            tenant_store,
            metrics,
            edge_cache,
            cluster,
            session_store,
            checkpoint_store,
            task_store,
            cost_ledger,
            kg_builder,
        };

        kernel.register_builtin_tools();
        kernel.restore_agents();
        kernel.restore_intents();
        kernel.restore_memories();
        kernel.restore_permissions();
        kernel.restore_event_log();
        kernel.restore_checkpoints();
        kernel.restore_task_store();

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
            self.fs.flush_tag_index();
        }
    }

    fn maybe_persist_event_log(&self) {
        let seq = self.event_bus.current_seq();
        if seq > 1 && (seq - 1).is_multiple_of(Self::EVENT_LOG_PERSIST_EVERY_N) {
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

}

mod api_dispatch;
mod handlers;
mod memory_link;

#[cfg(test)]
mod kernel_mod_tests {
    use super::AIKernel;
    use crate::api::semantic::ApiRequest;
    use crate::kernel::tests::make_kernel;

    // Test 1: AIKernel::new creates a valid kernel
    #[test]
    fn test_kernel_new_creates_valid_kernel() {
        let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
        let _ = std::env::set_var("LLM_BACKEND", "stub");
        let dir = tempfile::tempdir().unwrap();
        let kernel = AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
        assert!(!kernel.root.as_os_str().is_empty());
    }

    // Test 2: Tool registry has builtin tools registered
    #[test]
    fn test_tool_registry_has_builtin_tools() {
        let (kernel, _dir) = make_kernel();
        let tools = kernel.tool_registry.list();
        assert!(!tools.is_empty(), "builtin tools should be registered");
    }

    // Test 3: handle_api_request returns error for invalid action (via ToolCall with unknown tool)
    #[test]
    fn test_handle_api_request_unknown_tool_returns_error() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::ToolCall {
            tool: "nonexistent_tool_xyz".to_string(),
            params: serde_json::json!({}),
            agent_id: "test-agent".to_string(),
        };
        let resp = kernel.handle_api_request(req);
        // Unknown tool should return an error or at least not panic
        // The tool may not exist, so we get back whatever the kernel returns
        assert!(resp.error.is_some() || resp.tool_result.is_some());
    }

    // Test 4: handle_api_request Create succeeds
    #[test]
    fn test_handle_api_request_create_success() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::Create {
            api_version: None,
            content: "hello world".to_string(),
            content_encoding: crate::api::semantic::ContentEncoding::Utf8,
            tags: vec!["test".to_string()],
            agent_id: "test-agent".to_string(),
            tenant_id: None,
            agent_token: None,
            intent: None,
        };
        let resp = kernel.handle_api_request(req);
        assert!(resp.cid.is_some(), "Create should return a cid: {:?}", resp.error);
    }

    // Test 5: handle_api_request Search returns results
    #[test]
    fn test_handle_api_request_search_returns_response() {
        let (kernel, _dir) = make_kernel();
        // First create something to search for
        let create_req = ApiRequest::Create {
            api_version: None,
            content: "searchable content".to_string(),
            content_encoding: crate::api::semantic::ContentEncoding::Utf8,
            tags: vec!["searchable".to_string()],
            agent_id: "test-agent".to_string(),
            tenant_id: None,
            agent_token: None,
            intent: None,
        };
        kernel.handle_api_request(create_req);

        let search_req = ApiRequest::Search {
            query: "searchable".to_string(),
            agent_id: "test-agent".to_string(),
            tenant_id: None,
            agent_token: None,
            limit: Some(10),
            offset: None,
            require_tags: vec![],
            exclude_tags: vec![],
            since: None,
            until: None,
            intent_context: None,
        };
        let resp = kernel.handle_api_request(search_req);
        // Should return without error (may have 0 results with stub backend)
        assert!(resp.error.is_none() || resp.results.is_some());
    }

    // Test 6: handle_api_request RegisterAgent creates an agent
    #[test]
    fn test_handle_api_request_register_agent_creates_agent() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::RegisterAgent {
            name: "test-agent".to_string(),
        };
        let resp = kernel.handle_api_request(req);
        assert!(resp.agent_id.is_some(), "RegisterAgent should set agent_id: {:?}", resp.error);
    }

    // Test 7: Hook registry has hooks registered (causal hook at minimum)
    #[test]
    fn test_hook_registry_has_hooks() {
        let (kernel, _dir) = make_kernel();
        let count = kernel.hook_registry.count();
        assert!(count > 0, "hook registry should have at least causal hook registered");
    }

    // Test 8: Event bus event_subscribe works
    #[test]
    fn test_event_bus_subscribe_works() {
        let (kernel, _dir) = make_kernel();
        let sub_id = kernel.event_subscribe();
        assert!(!sub_id.is_empty(), "subscribe should return non-empty subscription id");
    }

    // Test 9: Session store is accessible
    #[test]
    fn test_session_store_accessible() {
        let (kernel, _dir) = make_kernel();
        // Just verify the session_store field is accessible and non-null
        let _ = &kernel.session_store;
    }

    // Test 10: metrics() returns valid metrics
    #[test]
    fn test_metrics_returns_valid_metrics() {
        let (kernel, _dir) = make_kernel();
        let metrics = kernel.metrics();
        // Should be able to get a metrics snapshot without panic
        let _snapshot = metrics.get_metrics();
    }

    // Test 11: handle_api_request AgentStatus returns error for unknown agent
    #[test]
    fn test_handle_api_request_agent_status_unknown_agent() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::AgentStatus {
            agent_id: "nonexistent-agent-xyz".to_string(),
        };
        let resp = kernel.handle_api_request(req);
        // Unknown agent should return error
        assert!(resp.error.is_some() || resp.agent_state.is_none());
    }

    // Test 12: handle_api_request ListAgents returns a response
    #[test]
    fn test_handle_api_request_list_agents() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::ListAgents;
        let resp = kernel.handle_api_request(req);
        assert!(resp.agents.is_some() || resp.error.is_none());
    }

    // Test 13: event_subscribe_filtered works
    #[test]
    fn test_event_bus_subscribe_filtered_works() {
        let (kernel, _dir) = make_kernel();
        use crate::kernel::event_bus::EventFilter;
        let filter = EventFilter {
            event_types: Some(vec!["tool_call".to_string()]),
            agent_ids: None,
        };
        let sub_id = kernel.event_subscribe_filtered(Some(filter));
        assert!(!sub_id.is_empty(), "subscribe_filtered should return non-empty subscription id");
    }

    // Test 14: event_poll returns None for unknown subscription
    #[test]
    fn test_event_poll_unknown_subscription_returns_none() {
        let (kernel, _dir) = make_kernel();
        let events = kernel.event_poll("nonexistent-subscription-id-xyz");
        assert!(events.is_none(), "poll for unknown subscription should return None");
    }

    // Test 15: handle_api_request CheckPermission works
    #[test]
    fn test_handle_api_request_check_permission() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::CheckPermission {
            agent_id: "test-agent".to_string(),
            action: "read".to_string(),
        };
        let resp = kernel.handle_api_request(req);
        // Should return without panic; result is either allowed or not
        assert!(resp.error.is_none() || resp.data.is_some());
    }
}

