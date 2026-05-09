//! AI Kernel — central orchestrator for all Plico subsystems.
//!
//! Wires together: CAS Storage, Layered Memory, Agent Scheduler,
//! Semantic FS, and Permission Guardrails. Upper-layer AI agents
//! interact with the kernel through the semantic API.

mod builtin_tools;
pub mod cognition;
pub mod event_bus;
pub mod hook;
pub mod persistence;
pub mod ops;
mod tools;
pub mod tests; 

use ops::checkpoint::CheckpointStore;
use ops::prefetch::IntentPrefetcher;
use ops::model::{HotSwapEmbeddingProvider, HotSwapLlmProvider};
use ops::observability::KernelMetrics;
use ops::cache::EdgeCache;
use ops::cost_ledger::{TokenCostLedger, set_global_cost_ledger};
use ops::distributed::{ClusterManager, NodeId};

use crate::api::agent_auth::AgentKeyStore;
use crate::config::PlicoConfig;

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
    pub(crate) config: PlicoConfig,
    pub(crate) root: PathBuf,
    pub(crate) cas: Arc<CASStorage>,
    pub(crate) memory: Arc<LayeredMemory>,
    pub(crate) scheduler: Arc<AgentScheduler>,
    pub(crate) fs: Arc<SemanticFS>,
    pub(crate) permissions: Arc<PermissionGuard>,
    pub(crate) memory_persister: Option<Arc<dyn MemoryPersister + Send + Sync>>,
    pub(crate) embedding: HotSwapEmbeddingProvider,
    pub(crate) llm_provider: HotSwapLlmProvider,
    pub(crate) knowledge_graph: Option<Arc<dyn KnowledgeGraph>>,
    pub(crate) search_backend: Arc<dyn SemanticSearch>,
    search_op_count: Arc<AtomicU64>,
    pub(crate) tool_registry: Arc<ToolRegistry>,
    pub(crate) message_bus: Arc<MessageBus>,
    pub(crate) event_bus: Arc<EventBus>,
    pub hook_registry: Arc<hook::HookRegistry>,
    pub prefetch: Arc<ops::prefetch::IntentPrefetcher>,
    pub(crate) key_store: Arc<AgentKeyStore>,
    pub(crate) tenant_store: Arc<ops::tenant::TenantStore>,
    pub(crate) metrics: Arc<KernelMetrics>,
    pub(crate) edge_cache: Arc<EdgeCache>,
    pub(crate) cluster: Arc<ClusterManager>,
    pub(crate) session_store: Arc<ops::session::SessionStore>,
    pub(crate) checkpoint_store: Arc<CheckpointStore>,
    pub(crate) task_store: Arc<ops::task::TaskStore>,
    pub(crate) cost_ledger: Arc<TokenCostLedger>,
    pub(crate) kg_builder: Option<ops::kg_builder::KgBuilderHandle>,
    pub(crate) prompt_registry: Arc<crate::prompt::PromptRegistry>,
    pub(crate) agent_profiles: Arc<ops::agent_profile::AgentProfileStore>,
    pub(crate) reranker: Option<Arc<dyn crate::fs::reranker::RerankerProvider>>,
    pub(crate) cognitive_loop: Option<Arc<crate::kernel::cognition::CognitiveLoop>>,
}

fn check_embedding_meta(root: &std::path::Path, model_name: &str, dim: usize) -> bool {
    let path = root.join(".embedding_meta.json");
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
                let saved_model = val.get("model").and_then(|v| v.as_str()).unwrap_or("");
                let saved_dim = val.get("dimension").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                saved_model != model_name || saved_dim != dim
            } else { true }
        }
        Err(_) => false,
    }
}

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
    pub fn new(root: PathBuf) -> std::io::Result<Self> {
        let config = PlicoConfig::load(Some(root.clone()));
        let cas = Arc::new(CASStorage::new(root.join("cas"))?);

        let embedding_raw: Arc<dyn EmbeddingProvider> =
            persistence::create_embedding_provider(&config.inference).unwrap_or_else(|e| {
                tracing::warn!("Embedding backend failed: {e}. Using stub (tag-only search).");
                Arc::new(StubEmbeddingProvider::new()) as Arc<dyn EmbeddingProvider>
            });
        let embedding_inner: Arc<RwLock<Arc<dyn EmbeddingProvider>>> = Arc::new(RwLock::new(embedding_raw));
        let embedding = HotSwapEmbeddingProvider::new(embedding_inner.clone());

        let llm_raw: Arc<dyn LlmProvider> = match persistence::create_llm_provider("PLICO_SUMMARIZER_MODEL", "qwen2.5-coder-7b-instruct") {
            Ok(provider) => { tracing::info!("LLM summarizer enabled: {}", provider.model_name()); provider }
            Err(e) => { tracing::warn!("Could not create LLM provider: {e}. Using stub provider."); Arc::new(crate::llm::StubProvider::empty()) as Arc<dyn LlmProvider> }
        };
        let llm_inner: Arc<RwLock<Arc<dyn LlmProvider>>> = Arc::new(RwLock::new(llm_raw));
        let llm_provider = HotSwapLlmProvider::new(llm_inner.clone());

        let summarizer: Option<Arc<dyn Summarizer>> = {
            let lp = llm_inner.read().unwrap().clone();
            Some(Arc::new(LlmSummarizer::new(lp)) as Arc<dyn Summarizer>)
        };

        let search_backend: Arc<dyn SemanticSearch> = match std::env::var("SEARCH_BACKEND").unwrap_or_else(|_| "hnsw".into()).as_str() {
            "memory" => { let b = Arc::new(InMemoryBackend::new()); b.restore_from(&root).ok(); b as Arc<dyn SemanticSearch> }
            _ => {
                let dim = embedding.dimension();
                let model_name = embedding.model_name().to_string();
                let meta_changed = check_embedding_meta(&root, &model_name, dim);
                let b = Arc::new(HnswBackend::with_dim(dim));
                if meta_changed {
                    tracing::warn!("Embedding model changed (now {}@{}d) — starting with fresh HNSW index", model_name, dim);
                    let _ = std::fs::remove_file(root.join("hnsw_index.jsonl"));
                } else { b.restore_from(&root).ok(); }
                save_embedding_meta(&root, &model_name, dim);
                b as Arc<dyn SemanticSearch>
            }
        };
        let search_index = search_backend.clone();
        let knowledge_graph: Option<Arc<dyn KnowledgeGraph>> = Some(Arc::new(PetgraphBackend::open(root.clone())));
        let memory = Arc::new(LayeredMemory::new());
        let scheduler = Arc::new(AgentScheduler::new());
        let reranker = crate::fs::reranker::create_reranker_provider();

        let mut fs = SemanticFS::with_reranker(
            root.clone(),
            Arc::new(embedding.clone()) as Arc<dyn EmbeddingProvider>,
            search_index,
            summarizer.clone(),
            knowledge_graph.clone(),
            reranker.clone(),
        )?;
        fs.set_chunking_mode(config.tuning.chunking_mode.clone());
        fs.set_auto_summarize(config.tuning.auto_summarize);
        let fs = Arc::new(fs);
        
        let permissions = Arc::new(PermissionGuard::new());
        let persister = match CASPersister::new(cas.clone(), root.clone()) {
            Ok(p) => { let ap: Arc<dyn MemoryPersister + Send + Sync> = Arc::new(p); memory.set_persister(ap.clone()); Some(ap) }
            Err(e) => { tracing::warn!("Failed to create memory persister: {e}"); None }
        };

        let tool_registry = Arc::new(ToolRegistry::new());
        let message_bus = Arc::new(MessageBus::new());
        let event_bus = Arc::new(EventBus::with_persistence(root.join("event_log.jsonl")));
        let hook_registry = Arc::new(hook::HookRegistry::new());
        let session_store = Arc::new(ops::session::SessionStore::restore(&root));

        if let Some(ref kg) = knowledge_graph {
            let causal_handler = Arc::new(ops::causal_hook::CausalHookHandler::new(Arc::clone(kg), Arc::clone(&session_store)));
            hook_registry.register(hook::HookPoint::PostToolCall, 100, causal_handler);
        }

        let verification_handler = Arc::new(ops::verification::VerificationHookHandler::new(Arc::clone(&fs), Arc::clone(&event_bus)));
        hook_registry.register(hook::HookPoint::PostToolCall, 90, verification_handler);

        let cost_ledger = Arc::new(TokenCostLedger::new());
        set_global_cost_ledger(Arc::clone(&cost_ledger));

        let prefetch = Arc::new(IntentPrefetcher::new(
            search_backend.clone(), knowledge_graph.clone(), memory.clone(), event_bus.clone(),
            Arc::new(embedding.clone()) as Arc<dyn EmbeddingProvider>, fs.ctx_loader_arc(), root.clone(),
        ));
        prefetch.set_cost_ledger(Arc::clone(&cost_ledger));

        if let Err(e) = prefetch.restore() { tracing::warn!("prefetch restore failed: {e}"); }
        let key_store = Arc::new(AgentKeyStore::open(&root));
        let tenant_store = Arc::new(ops::tenant::TenantStore::restore(&root));
        let metrics = Arc::new(KernelMetrics::new());
        let edge_cache = Arc::new(EdgeCache::default());
        
        let cluster = Arc::new(ClusterManager::new(
            NodeId::new(), "plico-cluster".into(), true, "127.0.0.1".into(), 7878,
        ));

        let timeout_session_store = Arc::clone(&session_store);
        let timeout_memory = memory.clone();
        let timeout_root = root.clone();
        std::thread::spawn(move || { ops::session::spawn_session_timeout_scanner(timeout_session_store, timeout_memory, timeout_root); });

        let checkpoint_store = Arc::new(CheckpointStore::restore(&root, &cas, 10));
        let task_store = Arc::new(ops::task::TaskStore::restore(root.clone(), event_bus.clone()));

        let kg_builder_config = ops::kg_builder::KgBuilderConfig::from_env();
        let kg_builder = if kg_builder_config.enabled {
            if let Some(ref kg) = knowledge_graph {
                let handle = ops::kg_builder::start_kg_builder(Arc::clone(kg), Arc::new(llm_provider.clone()), kg_builder_config);
                tracing::info!("KG auto-extraction worker started");
                Some(handle)
            } else { None }
        } else { None };

        let prompt_registry = {
            let mut reg = crate::prompt::PromptRegistry::new();
            crate::prompt::register_defaults(&mut reg);
            Arc::new(reg)
        };

        let cognitive_loop = {
            let context_analyzer = Arc::new(crate::kernel::cognition::ContextQualityEngine::new(
                Arc::new(embedding.clone()) as Arc<dyn EmbeddingProvider>,
                search_backend.clone(),
                memory.clone(),
                cas.clone(),
            ));
            let intent_network = Arc::new(crate::kernel::cognition::IntentSemanticNetwork::new(
                Arc::new(embedding.clone()) as Arc<dyn EmbeddingProvider>,
            ));
            let tracker = Arc::new(crate::kernel::cognition::TrajectoryTracker::new());
            let skill_forge = Arc::new(crate::kernel::cognition::SkillForge::new()
                .with_trajectory_tracker(tracker.clone())
                .with_embedding(Arc::new(embedding.clone()) as Arc<dyn EmbeddingProvider>));
            
            let cl = crate::kernel::cognition::CognitiveLoop::with_shared_tracker(
                context_analyzer,
                intent_network,
                skill_forge,
                tracker,
            );
            let arc = Arc::new(cl);
            
            let _ = prefetch.cognitive_loop.set(Arc::clone(&arc));

            if tokio::runtime::Handle::try_current().is_ok() {
                let loop_ref = Arc::clone(&arc);
                let sub_id = event_bus.subscribe();
                let bus = Arc::clone(&event_bus);
                tokio::spawn(async move {
                    loop {
                        if let Some(events) = bus.poll(&sub_id) {
                            for e in &events {
                                loop_ref.on_event(e);
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                });
            }
            Some(arc)
        };

        let mut kernel = Self {
            config, root, cas, memory, scheduler, fs, permissions, memory_persister: persister,
            embedding, llm_provider, knowledge_graph, search_backend, search_op_count: Arc::new(AtomicU64::new(0)),
            tool_registry, message_bus, event_bus, hook_registry, prefetch, key_store, tenant_store, metrics,
            edge_cache, cluster, session_store, checkpoint_store, task_store, cost_ledger, kg_builder, prompt_registry,
            agent_profiles: Arc::new(ops::agent_profile::AgentProfileStore::new()),
            reranker,
            cognitive_loop,
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

    const SEARCH_PERSIST_EVERY_N: u64 = 50;
    const EVENT_LOG_PERSIST_EVERY_N: u64 = 100;

    fn maybe_persist_search_index(&self) {
        let count = self.search_op_count.fetch_add(1, Ordering::Relaxed) + 1;
        if count % Self::SEARCH_PERSIST_EVERY_N == 0 {
            let backend = Arc::clone(&self.search_backend);
            let root = self.root.clone();
            let fs = Arc::clone(&self.fs);
            tokio::spawn(async move {
                if let Err(e) = backend.persist_to(&root) { tracing::warn!("Async search index persistence failed: {e}"); }
                fs.flush_tag_index();
            });
        }
    }

    fn maybe_persist_event_log(&self) {
        let seq = self.event_bus.current_seq();
        if seq > 1 && (seq - 1).is_multiple_of(Self::EVENT_LOG_PERSIST_EVERY_N) {
            self.persist_event_log();
        }
    }

    pub fn event_subscribe(&self) -> String { self.event_bus.subscribe() }
    pub fn event_subscribe_filtered(&self, filter: Option<event_bus::EventFilter>) -> String { self.event_bus.subscribe_filtered(filter) }
    pub fn event_poll(&self, subscription_id: &str) -> Option<Vec<event_bus::KernelEvent>> { self.event_bus.poll(subscription_id) }
    pub fn metrics(&self) -> &KernelMetrics { &self.metrics }
    pub fn event_unsubscribe(&self, subscription_id: &str) -> bool { self.event_bus.unsubscribe(subscription_id) }
    pub fn prompt_registry(&self) -> &crate::prompt::PromptRegistry { &self.prompt_registry }
}

mod api_dispatch;
mod handlers;
mod memory_link;

#[cfg(test)]
mod kernel_mod_tests {
    use super::AIKernel;
    use crate::api::semantic::ApiRequest;
    use crate::kernel::tests::make_kernel;

    #[test]
    fn test_kernel_new_creates_valid_kernel() {
        let _ = std::env::set_var("EMBEDDING_BACKEND", "stub");
        let _ = std::env::set_var("LLAMA_MODEL", "stub");
        let dir = tempfile::tempdir().unwrap();
        let kernel = AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
        assert!(!kernel.root.as_os_str().is_empty());
    }

    #[test]
    fn test_tool_registry_has_builtin_tools() {
        let (kernel, _dir) = make_kernel();
        let tools = kernel.tool_registry.list();
        assert!(!tools.is_empty());
    }

    #[test]
    fn test_handle_api_request_create_success() {
        let (kernel, _dir) = make_kernel();
        let req = ApiRequest::Create {
            api_version: None, content: "hello".into(), content_encoding: Default::default(),
            tags: vec!["test".into()], agent_id: "a1".into(), tenant_id: None, agent_token: None, intent: None,
        };
        let resp = kernel.handle_api_request(req);
        assert!(resp.cid.is_some());
    }
}
