//! AI Kernel — central orchestrator for all Plico subsystems.
//!
//! Wires together: CAS Storage, Layered Memory, Agent Scheduler,
//! Semantic FS, and Permission Guardrails. Upper-layer AI agents
//! interact with the kernel through the semantic API.

mod builtin_tools;
pub mod cognition;
pub mod event_bus;
pub mod hook;
pub mod ops;
pub mod persistence;
mod public_service;
pub mod tests;
mod tools;
pub mod trace;

pub use public_service::{
    PublicAccess, PublicAuthenticationError, PublicCredentialBootstrapError, PublicRequestContext, PublicTransport,
};

use ops::cache::EdgeCache;
use ops::checkpoint::CheckpointStore;
use ops::cost_ledger::{set_global_cost_ledger, TokenCostLedger};
use ops::model::{HotSwapEmbeddingProvider, HotSwapLlmProvider};
use ops::observability::KernelMetrics;
use ops::prefetch::IntentPrefetcher;
use ops::projection_runtime::{ProjectionRuntime, ProjectionWorkerHandle};

use crate::api::agent_auth::AgentKeyStore;
use crate::config::PlicoConfig;

use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex, RwLock,
};

use crate::api::permission::PermissionGuard;
use crate::cas::{CASStorage, PersonalVaultStorage};
use crate::fs::{
    EmbeddingProvider, HnswBackend, InMemoryBackend, KnowledgeGraph, LlmSummarizer, PetgraphBackend, SemanticFS,
    SemanticSearch, Summarizer,
};
use crate::kernel::event_bus::{EventBus, KernelEvent};
use crate::llm::LlmProvider;
use crate::memory::{CASCanonicalLedger, CanonicalLedger, LayeredMemory};
use crate::scheduler::messaging::MessageBus;
use crate::scheduler::AgentScheduler;
use crate::tool::ToolRegistry;

/// The AI Kernel — all subsystems wired together.
pub struct AIKernel {
    pub(crate) config: PlicoConfig,
    pub(crate) root: PathBuf,
    pub(crate) cas: Arc<CASStorage>,
    pub(crate) memory: Arc<LayeredMemory>,
    pub(crate) scheduler: Arc<AgentScheduler>,
    pub(crate) fs: Arc<SemanticFS>,
    pub(crate) permissions: Arc<PermissionGuard>,
    pub(crate) canonical: Arc<CASCanonicalLedger>,
    pub(crate) projection: Arc<ProjectionRuntime>,
    projection_worker: Mutex<Option<ProjectionWorkerHandle>>,
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
    pub(crate) metrics: Arc<KernelMetrics>,
    pub(crate) edge_cache: Arc<EdgeCache>,
    pub(crate) session_store: Arc<ops::session::SessionStore>,
    pub(crate) checkpoint_store: Arc<CheckpointStore>,
    pub(crate) task_store: Arc<ops::task::TaskStore>,
    pub(crate) cost_ledger: Arc<TokenCostLedger>,
    pub(crate) kg_builder: Option<ops::kg_builder::KgBuilderHandle>,
    pub(crate) prompt_registry: Arc<crate::prompt::PromptRegistry>,
    pub(crate) agent_profiles: Arc<ops::agent_profile::AgentProfileStore>,
    pub(crate) cognitive_loop: Arc<RwLock<Option<Arc<crate::kernel::cognition::CognitiveLoop>>>>,
    pub(crate) cognitive_pipeline: Arc<RwLock<Option<ops::cognitive_pipeline::CognitivePipelineHandle>>>,
    pub(crate) diagnostic_store: Arc<ops::diagnostic::DiagnosticStore>,
    pub(crate) intelligent_skill_forge: Arc<ops::skill_forge::IntelligentSkillForge>,
    pub(crate) trace_writer: trace::writer::TraceWriter,
}

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

fn create_hnsw_or_tag_only(
    root: &std::path::Path,
    embedding: &dyn EmbeddingProvider,
    maintain_legacy_metadata: bool,
) -> Arc<dyn SemanticSearch> {
    let dimension = embedding.dimension();
    if dimension == 0 {
        tracing::info!(
            operation = "embedding_search_initialization",
            outcome = "tag_only",
            "embedding dimension unavailable; vector search remains inactive"
        );
        return Arc::new(InMemoryBackend::new());
    }

    let model_name = embedding.model_name();
    if maintain_legacy_metadata && check_embedding_meta(root, &model_name, dimension) {
        tracing::warn!(
            operation = "legacy_object_vector_index_initialization",
            outcome = "fresh_index",
            dimension,
            "embedding model contract changed; legacy object vector index starts empty"
        );
        let _ = std::fs::remove_file(root.join("hnsw_index.jsonl"));
    }
    let backend = Arc::new(HnswBackend::with_dim(dimension));
    backend.restore_from(root).ok();
    if maintain_legacy_metadata {
        save_embedding_meta(root, &model_name, dimension);
    }
    backend
}

impl AIKernel {
    pub fn with_providers(
        root: PathBuf,
        embedding: Arc<dyn EmbeddingProvider>,
        llm: Arc<dyn LlmProvider>,
    ) -> std::io::Result<Arc<Self>> {
        let config = PlicoConfig::load(Some(root.clone()));
        let vault =
            Arc::new(PersonalVaultStorage::open(&root, Some("memory_index.json")).map_err(std::io::Error::other)?);
        let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).map_err(std::io::Error::other)?);
        let ledger: Arc<dyn CanonicalLedger + Send + Sync> = canonical.clone();
        let cas = Arc::new(CASStorage::new(vault.object_cas_root())?);

        let edge_cache = Arc::new(EdgeCache::default());
        let embedding =
            persistence::wrap_embedding_provider(embedding, &config.inference, Arc::clone(&edge_cache.embedding))
                .map_err(std::io::Error::other)?;
        let embedding_hswap = HotSwapEmbeddingProvider::new(embedding);

        let llm_inner: Arc<RwLock<Arc<dyn LlmProvider>>> = Arc::new(RwLock::new(llm.clone()));
        let llm_hswap = HotSwapLlmProvider::new(llm_inner.clone());

        let summarizer: Option<Arc<dyn Summarizer>> =
            Some(Arc::new(LlmSummarizer::new(llm.clone())) as Arc<dyn Summarizer>);

        let search_backend = create_hnsw_or_tag_only(&root, &embedding_hswap, false);
        let search_index = search_backend.clone();
        let knowledge_graph: Option<Arc<dyn KnowledgeGraph>> = Some(Arc::new(PetgraphBackend::open(root.clone())));
        let memory = Arc::new(LayeredMemory::new());
        memory.set_ledger(Arc::clone(&ledger));
        let projection =
            ProjectionRuntime::initialize(Arc::clone(&vault), Arc::clone(&canonical), embedding_hswap.clone());
        let scheduler = Arc::new(AgentScheduler::new());
        let reranker = crate::fs::reranker::create_reranker_provider();

        let mut fs = SemanticFS::with_reranker(
            root.clone(),
            cas.clone(),
            Arc::new(embedding_hswap.clone()) as Arc<dyn EmbeddingProvider>,
            search_index.clone(),
            summarizer.clone(),
            knowledge_graph.clone(),
            reranker.clone(),
        )?;
        fs.set_chunking_mode(config.tuning.chunking_mode.clone());
        fs.set_auto_summarize(config.tuning.auto_summarize);
        let fs_arc = Arc::new(fs);

        let ev_bus = Arc::new(crate::kernel::event_bus::EventBus::new());

        let kg_builder = if config.tuning.kg_auto_extract {
            let builder_cfg = crate::kernel::ops::kg_builder::KgBuilderConfig {
                enabled: true,
                batch_size: 1, // Fast for tests
                timeout_ms: 100,
            };
            Some(crate::kernel::ops::kg_builder::start_kg_builder(
                knowledge_graph.clone().unwrap(),
                Arc::new(llm_hswap.clone()),
                ev_bus.clone(),
                builder_cfg,
                Some(Arc::new(embedding_hswap.clone())),
            ))
        } else {
            None
        };

        let diagnostic_store = Arc::new(crate::kernel::ops::diagnostic::DiagnosticStore::new());

        let kernel = Self {
            root: root.clone(),
            config,
            cas,
            embedding: embedding_hswap.clone(),
            llm_provider: llm_hswap,
            fs: fs_arc.clone(),
            search_backend,
            knowledge_graph: knowledge_graph.clone(),
            memory: memory.clone(),
            scheduler,
            permissions: Arc::new(PermissionGuard::new()),
            canonical,
            projection,
            projection_worker: Mutex::new(None),
            search_op_count: Arc::new(AtomicU64::new(0)),
            tool_registry: Arc::new(ToolRegistry::new()),
            message_bus: Arc::new(crate::kernel::MessageBus::new()),
            event_bus: ev_bus.clone(),
            hook_registry: Arc::new(crate::kernel::hook::HookRegistry::new()),
            prefetch: Arc::new(ops::prefetch::IntentPrefetcher::new(
                search_index,
                knowledge_graph,
                memory,
                ev_bus.clone(),
                Arc::new(embedding_hswap.clone()) as Arc<dyn EmbeddingProvider>,
                fs_arc.ctx_loader_arc(),
                root.clone(),
            )),
            key_store: Arc::new(AgentKeyStore::new()),
            metrics: Arc::new(KernelMetrics::new()),
            edge_cache,
            session_store: Arc::new(ops::session::SessionStore::new()),
            checkpoint_store: Arc::new(CheckpointStore::new(10)), // max 10
            task_store: Arc::new(ops::task::TaskStore::new(root.join("tasks.json"), ev_bus.clone())),
            cost_ledger: Arc::new(TokenCostLedger::new()),
            kg_builder,
            prompt_registry: Arc::new(crate::prompt::PromptRegistry::new()),
            agent_profiles: Arc::new(ops::agent_profile::AgentProfileStore::new()),
            cognitive_loop: Arc::new(RwLock::new(None)),
            cognitive_pipeline: Arc::new(RwLock::new(None)),
            diagnostic_store,
            intelligent_skill_forge: Arc::new(ops::skill_forge::IntelligentSkillForge::new()),
            trace_writer: trace::writer::TraceWriter::new(root.clone()),
        };

        let kernel_arc = Arc::new(kernel);
        kernel_arc
            .restore_memories()
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;

        Ok(kernel_arc)
    }

    pub fn new(root: PathBuf) -> std::io::Result<Arc<Self>> {
        let config = PlicoConfig::load(Some(root.clone()));
        let vault =
            Arc::new(PersonalVaultStorage::open(&root, Some("memory_index.json")).map_err(std::io::Error::other)?);
        let canonical = Arc::new(CASCanonicalLedger::new(Arc::clone(&vault)).map_err(std::io::Error::other)?);
        let ledger: Arc<dyn CanonicalLedger + Send + Sync> = canonical.clone();
        let cas = Arc::new(CASStorage::new(vault.object_cas_root())?);

        let edge_cache = Arc::new(EdgeCache::default());
        let embedding_raw: Arc<dyn EmbeddingProvider> =
            persistence::create_embedding_provider(&config.inference, Arc::clone(&edge_cache.embedding))
                .map_err(|_| std::io::Error::other("embedding backend initialization failed"))?;
        let embedding = HotSwapEmbeddingProvider::new(embedding_raw);
        let projection = ProjectionRuntime::initialize(Arc::clone(&vault), Arc::clone(&canonical), embedding.clone());

        let llm_raw: Arc<dyn LlmProvider> =
            match persistence::create_llm_provider("PLICO_SUMMARIZER_MODEL", "qwen2.5-coder-7b-instruct") {
                Ok(provider) => {
                    tracing::info!("LLM summarizer enabled: {}", provider.model_name());
                    provider
                }
                Err(e) => {
                    tracing::warn!("Could not create LLM provider: {e}. Using stub provider.");
                    Arc::new(crate::llm::StubProvider::empty()) as Arc<dyn LlmProvider>
                }
            };
        let llm_inner: Arc<RwLock<Arc<dyn LlmProvider>>> = Arc::new(RwLock::new(llm_raw));
        let llm_provider = HotSwapLlmProvider::new(llm_inner.clone());

        let summarizer: Option<Arc<dyn Summarizer>> = {
            let lp = llm_inner.read().unwrap().clone();
            Some(Arc::new(LlmSummarizer::new(lp)) as Arc<dyn Summarizer>)
        };

        let search_backend: Arc<dyn SemanticSearch> = match std::env::var("SEARCH_BACKEND")
            .unwrap_or_else(|_| "hnsw".into())
            .as_str()
        {
            "memory" => {
                let b = Arc::new(InMemoryBackend::new());
                b.restore_from(&root).ok();
                b as Arc<dyn SemanticSearch>
            }
            _ => create_hnsw_or_tag_only(&root, &embedding, true),
        };
        let search_index = search_backend.clone();
        let knowledge_graph: Option<Arc<dyn KnowledgeGraph>> = Some(Arc::new(PetgraphBackend::open(root.clone())));
        let memory = Arc::new(LayeredMemory::new());
        let scheduler = Arc::new(AgentScheduler::new());
        let reranker = crate::fs::reranker::create_reranker_provider();

        let mut fs = SemanticFS::with_reranker_and_cache(
            root.clone(),
            cas.clone(),
            Arc::new(embedding.clone()) as Arc<dyn EmbeddingProvider>,
            search_index,
            summarizer.clone(),
            knowledge_graph.clone(),
            reranker.clone(),
            Some(edge_cache.search.clone()),
        )?;
        fs.set_chunking_mode(config.tuning.chunking_mode.clone());
        fs.set_auto_summarize(config.tuning.auto_summarize);
        let fs_arc = Arc::new(fs);

        let permissions = Arc::new(PermissionGuard::new());
        memory.set_ledger(Arc::clone(&ledger));

        let _tool_registry = Arc::new(ToolRegistry::new());
        let message_bus = Arc::new(MessageBus::new());
        let event_bus = Arc::new(EventBus::with_persistence(root.join("event_log.jsonl")));
        let hook_registry = Arc::new(hook::HookRegistry::new());
        let session_store = Arc::new(ops::session::SessionStore::restore(&root));

        if let Some(ref kg) = knowledge_graph {
            let causal_handler = Arc::new(ops::causal_hook::CausalHookHandler::new(
                Arc::clone(kg),
                Arc::clone(&session_store),
            ));
            hook_registry.register(hook::HookPoint::PostToolCall, 100, causal_handler);
        }

        let verification_handler = Arc::new(ops::verification::VerificationHookHandler::new(
            Arc::clone(&fs_arc),
            Arc::clone(&event_bus),
        ));
        hook_registry.register(hook::HookPoint::PostToolCall, 90, verification_handler);

        let cost_ledger = Arc::new(TokenCostLedger::new());
        set_global_cost_ledger(Arc::clone(&cost_ledger));

        let prefetch = Arc::new(IntentPrefetcher::new(
            search_backend.clone(),
            knowledge_graph.clone(),
            memory.clone(),
            event_bus.clone(),
            Arc::new(embedding.clone()) as Arc<dyn EmbeddingProvider>,
            fs_arc.ctx_loader_arc(),
            root.clone(),
        ));
        prefetch.set_cost_ledger(Arc::clone(&cost_ledger));

        if let Err(e) = prefetch.restore() {
            tracing::warn!("prefetch restore failed: {e}");
        }
        let key_store = Arc::new(AgentKeyStore::open(&root));
        let metrics = Arc::new(KernelMetrics::new());

        let timeout_session_store = Arc::clone(&session_store);
        let timeout_event_bus = Arc::clone(&event_bus);
        let timeout_root = root.clone();
        std::thread::spawn(move || {
            ops::session::spawn_session_timeout_scanner(timeout_session_store, timeout_event_bus, timeout_root);
        });

        let checkpoint_store = Arc::new(CheckpointStore::restore(&root, &cas, 10));
        let task_store = Arc::new(ops::task::TaskStore::restore(root.clone(), event_bus.clone()));

        let kg_builder_config = ops::kg_builder::KgBuilderConfig::from_env();
        let kg_builder = if kg_builder_config.enabled {
            if let Some(ref kg) = knowledge_graph {
                let handle = ops::kg_builder::start_kg_builder(
                    Arc::clone(kg),
                    Arc::new(llm_provider.clone()),
                    event_bus.clone(),
                    kg_builder_config,
                    Some(Arc::new(embedding.clone()) as Arc<dyn crate::fs::embedding::EmbeddingProvider>),
                );
                tracing::info!("KG auto-extraction worker started");
                Some(handle)
            } else {
                None
            }
        } else {
            None
        };

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
            let intent_network = Arc::new(crate::kernel::cognition::IntentSemanticNetwork::new(Arc::new(
                embedding.clone(),
            )
                as Arc<dyn EmbeddingProvider>));
            let tracker = Arc::new(crate::kernel::cognition::TrajectoryTracker::new());
            let skill_forge = Arc::new(
                crate::kernel::cognition::SkillForge::new()
                    .with_trajectory_tracker(tracker.clone())
                    .with_embedding(Arc::new(embedding.clone()) as Arc<dyn EmbeddingProvider>),
            );

            let cl = crate::kernel::cognition::CognitiveLoop::with_shared_tracker(
                context_analyzer,
                intent_network,
                skill_forge,
                tracker,
            );
            let arc = Arc::new(cl);

            let _ = prefetch.cognitive_loop.set(Arc::clone(&arc));

            if tokio::runtime::Handle::try_current().is_ok() {
                let loop_ref = Arc::downgrade(&arc);
                let sub_id = event_bus.subscribe();
                let bus = Arc::downgrade(&event_bus);
                tokio::spawn(async move {
                    loop {
                        let Some(bus) = bus.upgrade() else {
                            break;
                        };
                        let Some(loop_ref) = loop_ref.upgrade() else {
                            break;
                        };
                        if let Some(events) = bus.poll(&sub_id) {
                            for e in &events {
                                loop_ref.on_event(e);
                            }
                        }
                        drop(loop_ref);
                        drop(bus);
                        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    }
                });
            }
            Some(arc)
        };

        let trace_writer = trace::writer::TraceWriter::new(root.clone());

        let kernel = Self {
            config,
            root,
            cas,
            memory,
            scheduler,
            fs: fs_arc,
            permissions,
            canonical,
            projection,
            projection_worker: Mutex::new(None),
            embedding,
            llm_provider,
            knowledge_graph,
            search_backend,
            search_op_count: Arc::new(AtomicU64::new(0)),
            tool_registry: Arc::new(ToolRegistry::new()),
            message_bus,
            event_bus,
            hook_registry,
            prefetch,
            key_store,
            metrics,
            edge_cache,
            session_store,
            checkpoint_store,
            task_store,
            cost_ledger: Arc::new(TokenCostLedger::new()),
            kg_builder,
            prompt_registry,
            agent_profiles: Arc::new(ops::agent_profile::AgentProfileStore::new()),
            cognitive_loop: Arc::new(RwLock::new(cognitive_loop)),
            cognitive_pipeline: Arc::new(RwLock::new(None)),
            diagnostic_store: Arc::new(ops::diagnostic::DiagnosticStore::new()),
            intelligent_skill_forge: Arc::new(ops::skill_forge::IntelligentSkillForge::new()),
            trace_writer,
        };

        let kernel_arc = Arc::new(kernel);
        kernel_arc.register_builtin_tools();
        kernel_arc.restore_agents();
        kernel_arc.restore_intents();
        kernel_arc
            .restore_memories()
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        kernel_arc.restore_permissions();
        kernel_arc.restore_event_log();
        kernel_arc.restore_task_store();

        Ok(kernel_arc)
    }

    /// Returns a reference to the event bus (for test subscriptions).
    pub fn event_bus(&self) -> &Arc<EventBus> {
        &self.event_bus
    }

    /// Returns a reference to the LLM provider (for test hot-swap).
    pub fn llm_provider(&self) -> &HotSwapLlmProvider {
        &self.llm_provider
    }

    /// Starts background cognitive workers after the kernel is wrapped in Arc.
    /// Repeated calls preserve the existing cognitive pipeline and its counters.
    pub fn start_workers(self: &Arc<Self>) {
        if let Ok(mut worker) = self.projection_worker.lock() {
            if worker.is_none() {
                *worker = self.projection.start_worker();
            }
        }
        let (cp_handle, newly_started) = {
            let mut pipeline = self.cognitive_pipeline.write().unwrap();
            match pipeline.as_ref() {
                Some(handle) => (handle.clone(), false),
                None => {
                    let handle = ops::cognitive_pipeline::start_cognitive_pipeline(
                        Arc::clone(self),
                        self.config.tuning.cognitive_pipeline_queue_capacity,
                        self.config.tuning.cognitive_pipeline_max_in_flight,
                    );
                    *pipeline = Some(handle.clone());
                    (handle, true)
                }
            }
        };
        self.fs.set_cognitive_pipeline(cp_handle);
        if !newly_started {
            return;
        }

        // Start background conflict detection
        if let Some(ref kg) = self.knowledge_graph {
            let kg = Arc::clone(kg);
            let embedder = Some(Arc::new(self.embedding.clone()) as Arc<dyn crate::fs::embedding::EmbeddingProvider>);
            let event_bus = Arc::clone(&self.event_bus);
            let agent_profiles = Arc::clone(&self.agent_profiles);
            tokio::spawn(async move {
                let detector = ops::conflict_detector::ConflictDetector::new(kg, embedder);
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                    let agents = agent_profiles.list_agents();
                    for agent_id in agents {
                        let (conflicts, repairs) = detector.detect_and_repair(&agent_id);
                        if repairs > 0 {
                            tracing::info!(agent = %agent_id, repairs = repairs, "Conflict auto-repair completed");
                        }
                        for conflict in conflicts {
                            event_bus.emit(KernelEvent::CognitiveConflictDetected {
                                conflict_id: conflict.conflict_id,
                                conflict_type: conflict.conflict_type,
                                description: conflict.description,
                                involved_cids: conflict.involved_cids,
                                agent_id: conflict.agent_id,
                                severity: conflict.severity.to_string(),
                            });
                        }
                    }
                }
            });
        }
    }

    /// Stop and join the projection worker before persistence or process exit.
    /// This operation is idempotent.
    pub fn shutdown_projection_worker(&self) {
        self.projection.begin_shutdown();
        let worker = self.projection_worker.lock().ok().and_then(|mut worker| worker.take());
        drop(worker);
        self.projection.finish_shutdown_barrier();
    }

    const SEARCH_PERSIST_EVERY_N: u64 = 50;
    const EVENT_LOG_PERSIST_EVERY_N: u64 = 100;

    fn maybe_persist_search_index(&self) {
        let count = self.search_op_count.fetch_add(1, Ordering::Relaxed) + 1;
        if count.is_multiple_of(Self::SEARCH_PERSIST_EVERY_N) {
            let backend = Arc::clone(&self.search_backend);
            let root = self.root.clone();
            let fs = Arc::clone(&self.fs);
            tokio::spawn(async move {
                if let Err(e) = backend.persist_to(&root) {
                    tracing::warn!("Async search index persistence failed: {e}");
                }
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

    pub fn event_subscribe(&self) -> String {
        self.event_bus.subscribe()
    }
    pub fn event_subscribe_filtered(&self, filter: Option<event_bus::EventFilter>) -> String {
        self.event_bus.subscribe_filtered(filter)
    }
    pub fn event_poll(&self, subscription_id: &str) -> Option<Vec<event_bus::KernelEvent>> {
        self.event_bus.poll(subscription_id)
    }
    pub fn metrics(&self) -> &KernelMetrics {
        &self.metrics
    }
    pub fn event_unsubscribe(&self, subscription_id: &str) -> bool {
        self.event_bus.unsubscribe(subscription_id)
    }
    pub fn prompt_registry(&self) -> &crate::prompt::PromptRegistry {
        &self.prompt_registry
    }
}

impl crate::kernel::cognition::ToolExecutor for AIKernel {
    fn execute_tool(
        &self,
        name: &str,
        params: &serde_json::Value,
        agent_id: &str,
    ) -> Result<serde_json::Value, String> {
        let result = AIKernel::execute_tool(self, name, params, agent_id);
        if result.success {
            Ok(result.output)
        } else {
            Err(result.error.unwrap_or_else(|| format!("Tool '{}' failed", name)))
        }
    }
}

mod api_dispatch;
mod handlers;
mod memory_link;

#[cfg(test)]
mod kernel_mod_tests {
    use super::{create_hnsw_or_tag_only, AIKernel};
    use crate::api::semantic::ApiRequest;
    use crate::fs::{EmbeddingProvider, OpenAIEmbeddingBackend};
    use crate::kernel::tests::make_kernel;

    #[test]
    fn test_kernel_new_creates_valid_kernel() {
        std::env::set_var("EMBEDDING_BACKEND", "stub");
        std::env::set_var("LLAMA_MODEL", "stub");
        let dir = tempfile::tempdir().unwrap();
        let kernel = AIKernel::new(dir.path().to_path_buf()).expect("kernel init");
        assert!(!kernel.root.as_os_str().is_empty());
    }

    #[test]
    fn offline_openai_zero_dimension_does_not_touch_legacy_hnsw() {
        let directory = tempfile::tempdir().unwrap();
        let index_path = directory.path().join("hnsw_index.jsonl");
        let metadata_path = directory.path().join(".embedding_meta.json");
        std::fs::write(&index_path, b"LEGACY_HNSW_SENTINEL").unwrap();
        std::fs::write(&metadata_path, b"LEGACY_METADATA_SENTINEL").unwrap();
        let provider = OpenAIEmbeddingBackend::new("http://127.0.0.1:1/v1", "offline-openai-test", None).unwrap();
        assert_eq!(provider.dimension(), 0);

        let _search = create_hnsw_or_tag_only(directory.path(), &provider, true);

        assert_eq!(std::fs::read(&index_path).unwrap(), b"LEGACY_HNSW_SENTINEL");
        assert_eq!(std::fs::read(&metadata_path).unwrap(), b"LEGACY_METADATA_SENTINEL");
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
            api_version: None,
            content: "hello".into(),
            content_encoding: Default::default(),
            tags: vec!["test".into()],
            agent_id: "a1".into(),
            tenant_id: None,
            agent_token: None,
            intent: None,
            scope: None,
        };
        let resp = kernel.handle_api_request(req);
        assert!(resp.cid.is_some());
    }
}
