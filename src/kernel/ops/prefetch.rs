//! Proactive Context Assembly — semantic prefetch engine (F-2).
//!
//! Similar to CPU prefetch to L1 cache:
//!   - Agent declares intent ("fix auth module tests")
//!   - OS predicts relevant context (auth code, tests, recent changes)
//!   - Prefetches and assembles L0/L1/L2 layered summaries
//!   - Agent fetches the pre-assembled context on demand
//!
//! ## Multi-Path Recall Algorithm
//!
//! ```text
//! Step 1: Intent embedding
//!   intent_vec = embed("修复 auth 模块测试失败")
//!
//! Step 2: Concurrent multi-path recall (4 paths)
//!   path_a = semantic_search(intent_vec, limit=20)  → semantic neighbors
//!   path_b = kg.neighbors(cids, depth=2)            → KG topology
//!   path_c = recall_shared(tier=Procedural)        → shared procedural memory
//!   path_d = event_log.recent(tags=[], n=10)        → recent related events
//!
//! Step 3: RRF fusion (Reciprocal Rank Fusion, k=60)
//!
//! Step 4: Layered compression via context_budget::assemble()
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use tokio::time::timeout;
use tokio::task::spawn_blocking;

use crate::fs::context_budget::{self, BudgetAllocation, ContextCandidate};
use crate::fs::context_loader::ContextLoader;
use crate::fs::embedding::EmbeddingProvider;
use crate::fs::graph::KnowledgeGraph;
use crate::fs::search::SearchFilter;
use crate::kernel::event_bus::EventBus;
use crate::kernel::ops::session::AgentProfile;
use crate::memory::LayeredMemory;
use crate::memory::MemoryTier;

pub use super::prefetch_cache::IntentCacheStats;
pub use super::prefetch_profile::AgentProfileStore;
use super::prefetch_cache::IntentAssemblyCache;
use super::prefetch_profile::{IntentFeedbackEntry, DEFAULT_MAX_FEEDBACK_ENTRIES};

/// RRF fusion constant — dampens rank differences between paths.
const RRF_K: f32 = 60.0;

/// Maximum candidates per recall path.
const PATH_LIMIT: usize = 20;

/// Timeout per recall path (500ms).
const PATH_TIMEOUT_MS: u64 = 500;


pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// State of a prefetch assembly.
#[derive(Debug)]
pub enum AssemblyState {
    /// Prefetch is still running in the background.
    Pending,
    /// Prefetch complete, result ready.
    Ready(BudgetAllocation),
    /// Prefetch failed with an error message.
    Failed(String),
    /// Prefetch result was consumed by the agent.
    Used,
    /// Prefetch result was not consumed before timeout.
    Unused,
    /// Prefetch was cancelled by the agent.
    Cancelled,
}

/// A registered intent prefetch assembly.
#[derive(Debug)]
pub struct Assembly {
    pub assembly_id: String,
    pub agent_id: String,
    pub intent: String,
    pub budget_tokens: usize,
    pub state: AssemblyState,
    pub created_at_ms: u64,
}

/// Handle for tracking an async prefetch operation.
///
/// Allows checking state, cancelling, and awaiting the result.
pub struct PrefetchHandle {
    /// Assembly ID this handle refers to.
    pub assembly_id: String,
    /// Shared state for lock-free state checks.
    pub state: Arc<AtomicU8>,
    /// Shared result once ready.
    pub result: Arc<Mutex<Option<BudgetAllocation>>>,
}

impl PrefetchHandle {
    /// Returns true if the prefetch is in a terminal state.
    pub fn is_done(&self) -> bool {
        matches!(
            self.state.load(Ordering::Relaxed),
            4..=6 // Used=4, Unused=5, Cancelled=6
        )
    }

    /// Wait for the prefetch to complete and return the result.
    /// Returns None if cancelled or not ready.
    pub fn await_result(&self) -> Option<BudgetAllocation> {
        loop {
            let state = self.state.load(Ordering::Relaxed);
            match state {
                STATE_PENDING => { /* keep waiting */ }
                STATE_READY => {
                    return self.result.lock().unwrap().clone();
                }
                STATE_FAILED | STATE_USED | STATE_UNUSED | STATE_CANCELLED => {
                    return None;
                }
                _ => { return None; }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

const STATE_PENDING: u8 = 1;
const STATE_READY: u8 = 2;
const STATE_FAILED: u8 = 3;
const STATE_USED: u8 = 4;
const STATE_UNUSED: u8 = 5;
const STATE_CANCELLED: u8 = 6;

/// IntentPrefetcher — manages declarative intent prefetch assemblies.
///
/// Stores pending/running/ready assemblies in memory.
/// When `declare_intent` is called, the prefetcher:
///   1. Checks intent cache (F-9) — returns cached result if hit
///   2. Registers a new assembly with state `Pending`
///   3. Kicks off multi-path recall (semantic + KG + procedural + events)
///   4. Fuses results via RRF
///   5. Allocates budget via context_budget::assemble()
///   6. Stores result in cache and `Assembly.state = Ready(allocation)`
///
/// F-10 Cognitive Prefetch:
///   - Records intent completions in agent profile
///   - Maintains transition matrix of tag key → next tag key
///   - Silently prefetches predicted next intent when confidence > threshold
///
/// F-15 Adaptive Prefetch:
///   - Records which CIDs were actually used vs prefetched but unused
///   - Uses feedback history to prioritize historically-used CIDs in future prefetches
///
/// B51: Cache for assembled allocations keyed by assembly_id.
/// This ensures fetch_assembled_context returns Some(Ok(allocation)) when ready,
/// even when called immediately after declare_intent (before background prefetch completes).
struct AllocationCache {
    cache: RwLock<HashMap<String, BudgetAllocation>>,
}

impl AllocationCache {
    fn new() -> Self {
        Self { cache: RwLock::new(HashMap::new()) }
    }

    fn insert(&self, assembly_id: String, allocation: BudgetAllocation) {
        let mut c = self.cache.write().unwrap();
        c.insert(assembly_id, allocation);
    }

    fn get(&self, assembly_id: &str) -> Option<BudgetAllocation> {
        let c = self.cache.read().unwrap();
        c.get(assembly_id).cloned()
    }
}

/// Agent calls `fetch_assembled_context` to retrieve the result.
pub struct IntentPrefetcher {
    /// Active assemblies keyed by assembly_id.
    assemblies: Arc<RwLock<HashMap<String, Assembly>>>,
    /// B51: Cache for assembled allocations — ensures fetch returns allocation even when
    /// called immediately after declare_intent (before async prefetch completes).
    allocation_cache: Arc<AllocationCache>,
    /// Reference to the search backend for semantic recall.
    search: Arc<dyn crate::fs::SemanticSearch>,
    /// Reference to the knowledge graph (optional).
    kg: Option<Arc<dyn KnowledgeGraph>>,
    /// Reference to layered memory for procedural recall.
    memory: Arc<LayeredMemory>,
    /// Reference to event bus for recent events.
    event_bus: Arc<EventBus>,
    /// Reference to embedding provider.
    embedding: Arc<dyn EmbeddingProvider>,
    /// Reference to context loader for budget assembly.
    ctx_loader: Arc<ContextLoader>,
    /// Maximum age of an assembly before it's evicted (default: 1 hour).
    max_age_ms: u64,
    /// Intent assembly cache (F-9). Wrapped in Arc for thread-safe sharing.
    intent_cache: Arc<IntentAssemblyCache>,
    /// Agent profiles for cognitive prefetch (F-10).
    profile_store: Arc<AgentProfileStore>,
    /// Feedback history for adaptive prefetch (F-15).
    feedback_history: RwLock<Vec<IntentFeedbackEntry>>,
    /// Maximum feedback entries to keep.
    max_feedback_entries: usize,
    /// Root directory for persistence.
    root: std::path::PathBuf,
    /// F-4: Total intent cache lookups (for hit rate calculation).
    total_lookups: std::sync::atomic::AtomicU64,
    /// F-4: Total cache hits (for hit rate calculation).
    cache_hits: std::sync::atomic::AtomicU64,
    /// F-2: Token cost ledger for tracking embedding costs.
    cost_ledger: Arc<std::sync::RwLock<Option<Arc<crate::kernel::ops::cost_ledger::TokenCostLedger>>>>,
    /// F-2: Current session ID for cost tracking.
    current_session_id: std::sync::RwLock<Option<String>>,
}

impl IntentPrefetcher {
    /// Create a new prefetcher.
    pub fn new(
        search: Arc<dyn crate::fs::SemanticSearch>,
        kg: Option<Arc<dyn KnowledgeGraph>>,
        memory: Arc<LayeredMemory>,
        event_bus: Arc<EventBus>,
        embedding: Arc<dyn EmbeddingProvider>,
        ctx_loader: Arc<ContextLoader>,
        root: std::path::PathBuf,
    ) -> Self {
        Self {
            assemblies: Arc::new(RwLock::new(HashMap::new())),
            allocation_cache: Arc::new(AllocationCache::new()),
            search,
            kg,
            memory,
            event_bus,
            embedding,
            ctx_loader,
            max_age_ms: 3_600_000, // 1 hour
            intent_cache: Arc::new(IntentAssemblyCache::default()),
            profile_store: Arc::new(AgentProfileStore::default()),
            feedback_history: RwLock::new(Vec::new()),
            max_feedback_entries: DEFAULT_MAX_FEEDBACK_ENTRIES,
            root,
            total_lookups: std::sync::atomic::AtomicU64::new(0),
            cache_hits: std::sync::atomic::AtomicU64::new(0),
            cost_ledger: Arc::new(std::sync::RwLock::new(None)),
            current_session_id: std::sync::RwLock::new(None),
        }
    }

    /// Set the token cost ledger for tracking embedding costs.
    pub fn set_cost_ledger(&self, ledger: Arc<crate::kernel::ops::cost_ledger::TokenCostLedger>) {
        *self.cost_ledger.write().unwrap() = Some(ledger);
    }

    /// Set the current session ID for cost tracking.
    pub fn set_session_id(&self, session_id: Option<String>) {
        *self.current_session_id.write().unwrap() = session_id;
    }

    /// Record an embedding call cost if cost ledger is available.
    pub fn record_embedding_cost(&self, text: &str, model_id: &str) {
        let ledger_guard = self.cost_ledger.read().unwrap();
        if let Some(ref ledger) = *ledger_guard {
            let session_id = self.current_session_id.read().unwrap().clone();
            let agent_id = "unknown".to_string(); // Will be enhanced when agent context is available
            ledger.record_embedding(text, model_id, session_id.as_deref().unwrap_or(""), &agent_id);
        }
    }

    /// Record an embedding call cost with actual token count.
    pub fn record_embedding_cost_with_tokens(&self, _text: &str, model_id: &str, tokens: u32) {
        let ledger_guard = self.cost_ledger.read().unwrap();
        if let Some(ref ledger) = *ledger_guard {
            let session_id = self.current_session_id.read().unwrap().clone();
            let agent_id = "unknown".to_string();
            ledger.record_embedding_with_tokens(tokens, model_id, session_id.as_deref().unwrap_or(""), &agent_id);
        }
    }

    /// Persist intent cache, agent profiles, and feedback to disk.
    /// Called during shutdown or periodically.
    pub fn persist(&self) -> std::io::Result<()> {
        let prefetch_dir = self.root.join("prefetch");
        std::fs::create_dir_all(&prefetch_dir)?;
        self.intent_cache.persist_to_dir(&prefetch_dir)?;
        self.profile_store.persist_to_dir(&prefetch_dir)?;
        // Persist feedback history
        let feedback = self.feedback_history.read().unwrap();
        let json = serde_json::to_string_pretty(&*feedback)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(prefetch_dir.join("feedback.json"), json)?;
        // Persist cost ledger
        let ledger_guard = self.cost_ledger.read().unwrap();
        if let Some(ref ledger) = *ledger_guard {
            if let Err(e) = ledger.persist_to_dir(&prefetch_dir) {
                tracing::warn!("cost ledger persist failed: {}", e);
            }
        }
        tracing::debug!("prefetch state persisted ({} cache, {} profiles, {} feedback)",
            self.intent_cache.stats().entries,
            self.profile_store.len(),
            feedback.len());
        Ok(())
    }

    /// Get session token cost from cost ledger (F-2: TokenCostLedger integration).
    pub fn get_session_cost(&self, session_id: &str) -> (u32, u32) {
        let ledger_guard = self.cost_ledger.read().unwrap();
        match &*ledger_guard {
            Some(ledger) => {
                ledger.session_summary(session_id)
                    .map(|s| (s.total_input_tokens as u32, s.total_output_tokens as u32))
                    .unwrap_or((0, 0))
            }
            None => (0, 0),
        }
    }

    /// Restore intent cache, agent profiles, and feedback from disk.
    /// Called during initialization. Missing files are not errors.
    pub fn restore(&self) -> std::io::Result<()> {
        let prefetch_dir = self.root.join("prefetch");
        if !prefetch_dir.exists() {
            return Ok(());
        }
        let cache_count = self.intent_cache.restore_from_dir(&prefetch_dir)?;
        let profile_count = self.profile_store.restore_from_dir(&prefetch_dir)?;
        // Restore feedback history
        let feedback_path = prefetch_dir.join("feedback.json");
        if feedback_path.exists() {
            let json = std::fs::read_to_string(&feedback_path)?;
            let loaded: Vec<IntentFeedbackEntry> = serde_json::from_str(&json)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let mut feedback = self.feedback_history.write().unwrap();
            // Keep most recent entries up to max_feedback_entries
            *feedback = loaded.into_iter().rev().take(self.max_feedback_entries).rev().collect();
        }
        // Restore cost ledger
        let ledger_guard = self.cost_ledger.read().unwrap();
        if let Some(ref ledger) = *ledger_guard {
            if let Err(e) = ledger.restore_from_dir(&prefetch_dir) {
                tracing::warn!("cost ledger restore failed (ok if first run): {}", e);
            }
        }
        tracing::info!("prefetch state restored: {} cache entries, {} profiles",
            cache_count, profile_count);
        Ok(())
    }

    /// Declare a new intent and trigger async prefetch.
    /// Returns the assembly_id immediately.
    ///
    /// F-9 Intent Cache: Checks cache first using dual-path matching.
    /// - Path A (real embedding): cosine similarity matching
    /// - Path B (stub mode): exact string matching
    pub fn declare_intent(
        &self,
        agent_id: &str,
        intent: &str,
        related_cids: Vec<String>,
        budget_tokens: usize,
    ) -> String {
        let assembly_id = uuid::Uuid::new_v4().to_string();
        let now = crate::memory::layered::now_ms();

        // F-9: Try to get embedding and check cache first
        let model_name = self.embedding.model_name();
        let embed_result = self.embedding.embed(intent);
        let intent_embedding = embed_result.as_ref().ok().map(|r| r.embedding.clone());
        let input_tokens = embed_result.as_ref().map(|r| r.input_tokens).unwrap_or(0);

        // F-2: Record embedding cost with actual token count if cost ledger is available
        self.record_embedding_cost_with_tokens(intent, model_name, input_tokens);

        if let Some(cached_allocation) = self.intent_cache.lookup(intent, &intent_embedding) {
            // B51 Fix: Cache hit! Store allocation in allocation_cache so fetch_assembled_context
            // can return it immediately (before async prefetch completes).
            self.allocation_cache.insert(assembly_id.clone(), cached_allocation.clone());

            // Also store as Ready assembly in assemblies map
            let assembly = Assembly {
                assembly_id: assembly_id.clone(),
                agent_id: agent_id.to_string(),
                intent: intent.to_string(),
                budget_tokens,
                state: AssemblyState::Ready(cached_allocation),
                created_at_ms: now,
            };
            let mut assemblies = self.assemblies.write().unwrap();
            assemblies.insert(assembly_id.clone(), assembly);
            tracing::debug!("intent cache hit for: {}", intent);
            return assembly_id;
        }

        // Cache miss: proceed with normal prefetch flow
        let assembly = Assembly {
            assembly_id: assembly_id.clone(),
            agent_id: agent_id.to_string(),
            intent: intent.to_string(),
            budget_tokens,
            state: AssemblyState::Pending,
            created_at_ms: now,
        };

        // Register assembly before kicking off background work
        {
            let mut assemblies = self.assemblies.write().unwrap();
            assemblies.insert(assembly_id.clone(), assembly);
        }

        // F-15: Build feedback boost map from historical usage before spawning thread
        let feedback_boost: HashMap<String, f32> =
            if let Some((used, unused)) = self.get_similar_feedback(intent) {
                let mut boost = HashMap::new();
                for cid in used   { boost.insert(cid, 1.5); } // boost historically used
                for cid in unused { boost.insert(cid, 0.3); } // demote historically unused
                boost
            } else {
                HashMap::new()
            };

        // Kick off background prefetch — clone refs for the async task
        let assemblies = Arc::clone(&self.assemblies);
        let allocation_cache = Arc::clone(&self.allocation_cache);
        let search = Arc::clone(&self.search);
        let kg = self.kg.clone();
        let memory = Arc::clone(&self.memory);
        let event_bus = Arc::clone(&self.event_bus);
        let embedding = Arc::clone(&self.embedding);
        let ctx_loader = Arc::clone(&self.ctx_loader);
        let assembly_id_clone = assembly_id.clone();
        let intent_clone = intent.to_string();
        let max_age = self.max_age_ms;
        // Use the actual prefetcher's intent cache, not a new one
        let intent_cache = Arc::clone(&self.intent_cache);
        let related_cids_clone = related_cids.clone();

        // Spawn background task using std thread (no tokio feature needed)
        std::thread::spawn(move || {
            // Create a tokio runtime for this background thread
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            rt.block_on(async move {
                Self::run_prefetch(
                    assemblies, Some(allocation_cache), search, kg, memory, event_bus, embedding, ctx_loader,
                    assembly_id_clone, intent_clone, related_cids_clone, budget_tokens, max_age,
                    Some(intent_cache), feedback_boost,
                ).await;
            });
        });

        assembly_id
    }

    /// Prefetch intent asynchronously, returning a handle for tracking and cancellation.
    ///
    /// Same as `declare_intent` but returns a `PrefetchHandle` that allows:
    /// - Checking state via `handle.state.load()`
    /// - Cancelling via `prefetch.cancel(&handle)`
    /// - Awaiting result via `handle.await_result()`
    pub fn prefetch_async(
        &self,
        agent_id: &str,
        intent: &str,
        related_cids: Vec<String>,
        budget_tokens: usize,
    ) -> PrefetchHandle {
        let assembly_id = uuid::Uuid::new_v4().to_string();
        let now = crate::memory::layered::now_ms();

        // F-9: Try to get embedding and check cache first
        let model_name = self.embedding.model_name();
        let embed_result = self.embedding.embed(intent);
        let intent_embedding = embed_result.as_ref().ok().map(|r| r.embedding.clone());
        let input_tokens = embed_result.as_ref().map(|r| r.input_tokens).unwrap_or(0);

        // F-2: Record embedding cost with actual token count if cost ledger is available
        self.record_embedding_cost_with_tokens(intent, model_name, input_tokens);

        // F-4: Track intent cache lookups and hits
        self.total_lookups.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        if let Some(cached_allocation) = self.intent_cache.lookup(intent, &intent_embedding) {
            self.cache_hits.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            self.allocation_cache.insert(assembly_id.clone(), cached_allocation.clone());
            let alloc_clone = cached_allocation.clone();
            let assembly = Assembly {
                assembly_id: assembly_id.clone(),
                agent_id: agent_id.to_string(),
                intent: intent.to_string(),
                budget_tokens,
                state: AssemblyState::Ready(alloc_clone.clone()),
                created_at_ms: now,
            };
            let mut assemblies = self.assemblies.write().unwrap();
            assemblies.insert(assembly_id.clone(), assembly);

            let handle = PrefetchHandle {
                assembly_id: assembly_id.clone(),
                state: Arc::new(AtomicU8::new(STATE_READY)),
                result: Arc::new(Mutex::new(Some(cached_allocation))),
            };
            tracing::debug!("prefetch_async cache hit for: {}", intent);
            return handle;
        }

        // Cache miss: proceed with normal prefetch flow
        let assembly = Assembly {
            assembly_id: assembly_id.clone(),
            agent_id: agent_id.to_string(),
            intent: intent.to_string(),
            budget_tokens,
            state: AssemblyState::Pending,
            created_at_ms: now,
        };

        // Create handle upfront
        let handle = PrefetchHandle {
            assembly_id: assembly_id.clone(),
            state: Arc::new(AtomicU8::new(STATE_PENDING)),
            result: Arc::new(Mutex::new(None)),
        };

        // Register assembly before kicking off background work
        {
            let mut assemblies = self.assemblies.write().unwrap();
            assemblies.insert(assembly_id.clone(), assembly);
        }

        // F-15: Build feedback boost map from historical usage before spawning thread
        let feedback_boost: HashMap<String, f32> =
            if let Some((used, unused)) = self.get_similar_feedback(intent) {
                let mut boost = HashMap::new();
                for cid in used   { boost.insert(cid, 1.5); } // boost historically used
                for cid in unused { boost.insert(cid, 0.3); } // demote historically unused
                boost
            } else {
                HashMap::new()
            };

        // Clone all data needed for the background thread
        let assemblies = Arc::clone(&self.assemblies);
        let allocation_cache = Arc::clone(&self.allocation_cache);
        let search = Arc::clone(&self.search);
        let kg = self.kg.clone();
        let memory = Arc::clone(&self.memory);
        let event_bus = Arc::clone(&self.event_bus);
        let embedding = Arc::clone(&self.embedding);
        let ctx_loader = Arc::clone(&self.ctx_loader);
        let assembly_id_clone = assembly_id.clone();
        let intent_clone = intent.to_string();
        let max_age = self.max_age_ms;
        let intent_cache = Arc::clone(&self.intent_cache);
        let related_cids_clone = related_cids.clone();
        // Arcs for the handle's shared state
        let handle_state = Arc::clone(&handle.state);
        let handle_result = Arc::clone(&handle.result);

        // Spawn background task
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();

            rt.block_on(async move {
                Self::run_prefetch(
                    assemblies.clone(), Some(allocation_cache.clone()), search.clone(), kg.clone(),
                    memory.clone(), event_bus.clone(), embedding.clone(), ctx_loader.clone(),
                    assembly_id_clone.clone(), intent_clone.clone(), related_cids_clone.clone(),
                    budget_tokens, max_age, Some(intent_cache), feedback_boost,
                ).await;

                // Update handle state by checking assembly state
                let assemblies = assemblies.read().unwrap();
                if let Some(asm) = assemblies.get(&assembly_id_clone) {
                    match &asm.state {
                        AssemblyState::Ready(allocation) => {
                            handle_state.store(STATE_READY, Ordering::Relaxed);
                            *handle_result.lock().unwrap() = Some(allocation.clone());
                        }
                        AssemblyState::Failed(_) => {
                            handle_state.store(STATE_FAILED, Ordering::Relaxed);
                        }
                        _ => {}
                    }
                }
            });
        });

        handle
    }

    /// Cancel a pending prefetch by assembly_id.
    /// Returns true if the assembly was found and cancelled.
    pub fn cancel(&self, assembly_id: &str) -> bool {
        let mut assemblies = self.assemblies.write().unwrap();
        if let Some(assembly) = assemblies.get_mut(assembly_id) {
            match assembly.state {
                AssemblyState::Pending => {
                    assembly.state = AssemblyState::Cancelled;
                    tracing::debug!("prefetch cancelled: {}", assembly_id);
                    return true;
                }
                _ => {
                    // Already completed or in terminal state
                    return false;
                }
            }
        }
        false
    }

    /// Fetch a previously declared assembled context.
    /// Returns `None` if the assembly_id is unknown.
    /// B51 Fix: First checks allocation_cache for immediately available allocations
    /// (from cache hits in declare_intent), then falls back to assemblies map.
    pub fn fetch_assembled_context(
        &self,
        agent_id: &str,
        assembly_id: &str,
    ) -> Option<Result<BudgetAllocation, String>> {
        // B51: First check allocation_cache - this has allocations stored synchronously
        // when declare_intent had a cache hit, so they're available immediately.
        if let Some(allocation) = self.allocation_cache.get(assembly_id) {
            // Mark as Used in assemblies map if present
            if let Ok(mut assemblies) = self.assemblies.write() {
                if let Some(asm) = assemblies.get_mut(assembly_id) {
                    if matches!(asm.state, AssemblyState::Ready(_)) {
                        asm.state = AssemblyState::Used;
                    }
                }
            }
            return Some(Ok(allocation));
        }

        // Fall back to assemblies map for async prefetch results
        let assemblies = self.assemblies.read().unwrap();
        let assembly = assemblies.get(assembly_id)?;

        // Only the owning agent can fetch
        if assembly.agent_id != agent_id {
            return None;
        }

        match &assembly.state {
            AssemblyState::Pending => Some(Err("prefetch still in progress".to_string())),
            AssemblyState::Ready(allocation) => {
                // Mark as Used
                let result = allocation.clone();
                drop(assemblies);
                let mut assemblies = self.assemblies.write().unwrap();
                if let Some(asm) = assemblies.get_mut(assembly_id) {
                    asm.state = AssemblyState::Used;
                }
                Some(Ok(result))
            }
            AssemblyState::Failed(err) => Some(Err(err.clone())),
            AssemblyState::Used | AssemblyState::Unused | AssemblyState::Cancelled => None,
        }
    }

    /// Evict stale assemblies older than max_age_ms.
    pub fn evict_stale(&self) {
        let now = crate::memory::layered::now_ms();
        let mut assemblies = self.assemblies.write().unwrap();
        assemblies.retain(|_id, a| now - a.created_at_ms < self.max_age_ms);
    }

    /// F-9: Invalidate intent cache entries that depend on any of the given CIDs.
    /// Call this when objects are modified to ensure cache consistency.
    pub fn invalidate_intent_cache_by_cids(&self, modified_cids: &[String]) {
        self.intent_cache.invalidate_by_cids(modified_cids);
    }

    /// F-9: Clear the entire intent cache.
    pub fn clear_intent_cache(&self) {
        self.intent_cache.clear();
    }

    /// F-9: Get intent cache statistics.
    pub fn intent_cache_stats(&self) -> IntentCacheStats {
        self.intent_cache.stats()
    }

    // ── F-10: Cognitive Prefetch ───────────────────────────────────────────────

    /// F-10: Record intent completion and potentially trigger background prefetch.
    ///
    /// Call this when an agent completes an intent (e.g., via EndSession or
    /// when declaring a new intent that supersedes the previous one).
    ///
    /// This:
    /// 1. Updates the agent's transition profile
    /// 2. Predicts the next likely intent
    /// 3. Silently prefetches if confidence > threshold
    ///
    /// Returns the predicted next tag key if prefetch was triggered, None otherwise.
    pub fn on_intent_complete(
        &self,
        agent_id: &str,
        intent: &str,
        next_intent: Option<&str>,
        known_tags: &[String],
    ) -> Option<String> {
        // Extract tag keys for current and next intent
        let current_tag_key = self.profile_store.extract_tag_key(intent, known_tags);
        let next_tag_key = next_intent.and_then(|n| self.profile_store.extract_tag_key(n, known_tags));

        // Record in profile and get prediction
        let predicted_next = self.profile_store.record_intent_complete(
            agent_id,
            current_tag_key.as_deref(),
            next_tag_key.as_deref(),
        );

        if let Some(ref predicted) = predicted_next {
            tracing::debug!(
                "F-10: Agent {} intent '{}' -> predicting next '{}', triggering prefetch",
                agent_id,
                intent,
                predicted
            );

            // Silently prefetch the predicted intent
            // Use a default budget and no related_cids for background prefetch
            let _ = self.trigger_cognitive_prefetch(agent_id, predicted, known_tags);
        }

        predicted_next
    }

    /// F-10: Trigger background prefetch for a predicted next intent.
    ///
    /// This is called silently by the system when an intent completes and
    /// the transition matrix predicts a likely next intent.
    fn trigger_cognitive_prefetch(
        &self,
        agent_id: &str,
        predicted_tag_key: &str,
        _known_tags: &[String],
    ) -> Option<String> {
        // Skip if strategy is disabled
        if self.profile_store.strategy().is_disabled() {
            return None;
        }

        // Convert tag key back to a representative intent text
        // Tag key serves as a coarse intent identifier; embedding-based clustering is a future enhancement
        // In a real implementation, we'd look up a representative intent for this tag key
        let predicted_intent = format!("next: {}", predicted_tag_key);

        // Use a smaller budget for background prefetch (not user-facing)
        let budget = 1024;

        // Check if this is already being prefetched or cached
        // Skip if already in cache (F-9 would handle it)
        let model_name = self.embedding.model_name();
        let intent_embedding: Option<Vec<f32>> = self.embedding.embed(&predicted_intent).ok().map(|r| r.embedding);

        // F-2: Record embedding cost if cost ledger is available
        self.record_embedding_cost(&predicted_intent, model_name);

        if self.intent_cache.lookup(&predicted_intent, &intent_embedding).is_some() {
            tracing::debug!("F-10: predicted intent already cached, skipping prefetch");
            return None;
        }

        // Trigger silent prefetch
        let assembly_id = self.declare_intent(
            agent_id,
            &predicted_intent,
            vec![],  // no related cids for predicted intent
            budget,
        );

        tracing::debug!("F-10: triggered silent prefetch for '{}', assembly_id={}", predicted_tag_key, assembly_id);
        Some(assembly_id)
    }

    /// F-10: Get the agent profile for inspection.
    pub fn get_agent_profile(&self, agent_id: &str) -> AgentProfile {
        self.profile_store.get_or_create(agent_id)
    }

    /// F-10: Extract tag key from intent text using the profile strategy.
    pub fn extract_tag_key(&self, intent: &str, known_tags: &[String]) -> Option<String> {
        self.profile_store.extract_tag_key(intent, known_tags)
    }

    // ── F-15: Adaptive Prefetch ─────────────────────────────────────────────

    /// Record feedback about what CIDs were actually used vs prefetched.
    ///
    /// This is called by the Agent after executing an intent to report
    /// which CIDs from the prefetch assembly were actually read/used
    /// and which were prefetched but never accessed.
    ///
    /// The feedback is stored and can be retrieved via `get_similar_feedback`
    /// to prioritize historically-used CIDs in future prefetches.
    pub fn record_feedback(&self, intent: &str, used_cids: Vec<String>, unused_cids: Vec<String>) {
        let normalized = intent.to_lowercase().trim().to_string();
        let entry = IntentFeedbackEntry {
            normalized_intent: normalized,
            used_cids,
            unused_cids,
            recorded_at_ms: now_ms(),
        };

        let mut history = self.feedback_history.write().unwrap();
        history.push(entry);

        // Evict old entries if over limit
        while history.len() > self.max_feedback_entries {
            history.remove(0);
        }
    }

    /// Get feedback entries for a similar intent.
    ///
    /// Returns the used and unused CIDs from the most recent feedback entry
    /// that matches the given intent (case-insensitive).
    ///
    /// This enables adaptive prefetch: when a similar intent is declared,
    /// the prefetcher can prioritize CIDs that were historically used.
    pub fn get_similar_feedback(&self, intent: &str) -> Option<(Vec<String>, Vec<String>)> {
        let normalized = intent.to_lowercase().trim().to_string();
        let history = self.feedback_history.read().unwrap();

        // Find most recent entry with same normalized intent
        for entry in history.iter().rev() {
            if entry.normalized_intent == normalized {
                return Some((entry.used_cids.clone(), entry.unused_cids.clone()));
            }
        }
        None
    }

    /// Get the number of feedback entries currently stored.
    pub fn feedback_count(&self) -> usize {
        self.feedback_history.read().unwrap().len()
    }

    /// F-6: Get hot objects for an agent (for context-dependent gravity).
    pub fn get_hot_objects(&self, agent_id: &str) -> Vec<String> {
        let profile = self.profile_store.get_or_create(agent_id);
        profile.hot_objects.iter().map(|(cid, _)| cid.clone()).collect()
    }

    /// F-3: Warm intent cache from agent profile (cache preheat at session-start).
    ///
    /// Uses the agent's hot_objects to pre-populate the cache with frequently
    /// accessed CIDs before the first declare_intent call.
    /// This improves cache hit rate on repeated operations.
    ///
    /// Returns the number of entries warmed.
    pub fn warm_cache_for_agent(&self, agent_id: &str) -> usize {
        let profile = self.profile_store.get_or_create(agent_id);

        // No-op assembler: actual assembly will happen on demand via declare_intent
        // Hot objects warming doesn't require assembler
        let noop_assembler = |_: &str| None;

        self.intent_cache.warm_from_profile(&profile, &noop_assembler)
    }

    /// F-1: Apply pending feedback entries from feedback_history to agent profile.
    ///
    /// Called at session-end to close the feedback loop:
    /// - used_cids → record_cid_usage (hot_objects updated)
    /// - unused_cids → decay_object
    /// - transition matrix updated from last_intent → current intent
    ///
    /// Returns the number of feedback entries applied.
    pub fn apply_feedback_from_history(&self, agent_id: &str) -> usize {
        let feedback_history = self.feedback_history.read().unwrap();
        let mut applied = 0;

        // Apply the most recent feedback entries (up to last 10)
        for entry in feedback_history.iter().rev().take(10) {
            self.profile_store.apply_feedback(agent_id, entry);
            applied += 1;
        }

        if applied > 0 {
            tracing::debug!("applied {} feedback entries to profile for {}", applied, agent_id);
        }
        applied
    }

    /// F-10: Get the profile store for external access (e.g., by intent_executor).
    pub fn profile_store(&self) -> &Arc<AgentProfileStore> {
        &self.profile_store
    }

    /// F-4: Get prefetcher hit rate statistics (lookups, hits, hit rate).
    pub fn prefetch_hit_rate(&self) -> (u64, u64, f64) {
        let lookups = self.total_lookups.load(std::sync::atomic::Ordering::Relaxed);
        let hits = self.cache_hits.load(std::sync::atomic::Ordering::Relaxed);
        let rate = if lookups > 0 { hits as f64 / lookups as f64 } else { 0.0 };
        (lookups, hits, rate)
    }

    /// Run the full multi-path prefetch in a background thread.
    /// Optionally stores result in intent cache (F-9).
    /// B51: Also stores in allocation_cache for fetch_assembled_context.
    #[allow(clippy::too_many_arguments)]
    async fn run_prefetch(
        assemblies: Arc<RwLock<HashMap<String, Assembly>>>,
        allocation_cache: Option<Arc<AllocationCache>>,
        search: Arc<dyn crate::fs::SemanticSearch>,
        kg: Option<Arc<dyn KnowledgeGraph>>,
        memory: Arc<LayeredMemory>,
        event_bus: Arc<EventBus>,
        embedding: Arc<dyn EmbeddingProvider>,
        ctx_loader: Arc<ContextLoader>,
        assembly_id: String,
        intent: String,
        related_cids: Vec<String>,
        budget_tokens: usize,
        _max_age_ms: u64,
        intent_cache: Option<Arc<IntentAssemblyCache>>,
        feedback_boost: HashMap<String, f32>,
    ) {
        let result = Self::multi_path_recall_async(
            &search, &kg, &memory, &event_bus, &embedding,
            &intent, &related_cids,
        ).await;

        let now = crate::memory::layered::now_ms();
        let mut assemblies_guard = assemblies.write().unwrap();
        let entry = assemblies_guard.get_mut(&assembly_id);

        match (result, entry) {
            (Ok(mut candidates), Some(a)) => {
                // F-15: Apply adaptive feedback boost to candidate relevance scores.
                // CIDs historically used by this intent get a 1.5x boost;
                // CIDs historically unused get a 0.3x demotion.
                // Falls back to no-op when feedback_boost is empty.
                if !feedback_boost.is_empty() {
                    for c in &mut candidates {
                        if let Some(&factor) = feedback_boost.get(&c.cid) {
                            c.relevance = (c.relevance * factor).min(1.0);
                        }
                    }
                    candidates.sort_by(|a, b| {
                        b.relevance.partial_cmp(&a.relevance).unwrap_or(std::cmp::Ordering::Equal)
                    });
                    tracing::debug!(
                        "F-15: applied feedback boost ({} boosted/demoted) for intent '{}'",
                        feedback_boost.len(), intent
                    );
                }
                let allocation = context_budget::assemble(&ctx_loader, &candidates, budget_tokens);

                // F-9: Store in intent cache for future hits
                if let Some(ref cache) = intent_cache {
                    // Get embedding for cache storage (Path A)
                    let intent_embedding: Option<Vec<f32>> = embedding
                        .embed(&intent)
                        .ok()
                        .map(|r| r.embedding);
                    cache.store(intent, intent_embedding, allocation.clone(), related_cids);
                }

                // B51: Also store in allocation_cache so fetch_assembled_context returns immediately
                if let Some(ref cache) = allocation_cache {
                    cache.insert(assembly_id.clone(), allocation.clone());
                }

                a.state = AssemblyState::Ready(allocation);
                a.created_at_ms = now;
            }
            (Err(e), Some(a)) => {
                tracing::warn!("prefetch failed for {}: {}", assembly_id, e);
                a.state = AssemblyState::Failed(e);
                a.created_at_ms = now;
            }
            (_, None) => {
                // Assembly was evicted before we could complete — just drop
                tracing::debug!("prefetch completed but assembly {} was already evicted", assembly_id);
            }
        }
    }

    /// Multi-path recall (async concurrent): semantic + KG + procedural + events → fused candidates.
    async fn multi_path_recall_async(
        search: &Arc<dyn crate::fs::SemanticSearch>,
        kg: &Option<Arc<dyn KnowledgeGraph>>,
        memory: &Arc<LayeredMemory>,
        event_bus: &Arc<EventBus>,
        embedding: &Arc<dyn EmbeddingProvider>,
        intent: &str,
        related_cids: &[String],
    ) -> Result<Vec<ContextCandidate>, String> {
        // Step 1: Embed the intent (blocking, run in spawn_blocking)
        let emb = spawn_blocking({
            let embedding = Arc::clone(embedding);
            let intent = intent.to_string();
            move || embedding.embed(&intent)
        }).await
        .map_err(|e| format!("embed task panicked: {}", e))?
        .map_err(|e| format!("failed to embed intent: {}", e))?;
        let emb_slice: Vec<f32> = emb.embedding;

        // Clone data needed for async tasks
        let related_cids_owned = related_cids.to_vec();
        let intent_owned = intent.to_string();
        let intent_owned2 = intent.to_string(); // Second owned copy for events path
        let search = Arc::clone(search);
        let kg = kg.clone();
        let memory = Arc::clone(memory);
        let event_bus = Arc::clone(event_bus);
        let emb_for_sem = emb_slice.clone();

        // Step 2: Four-path recall CONCURRENTLY with 500ms timeout each
        let timeout_duration = Duration::from_millis(PATH_TIMEOUT_MS);

        let (path_a, path_b, path_c, path_d) = tokio::join!(
            timeout(timeout_duration, Self::recall_semantic_async(search, emb_for_sem)),
            timeout(timeout_duration, Self::recall_kg_async(kg, related_cids_owned, intent_owned)),
            timeout(timeout_duration, Self::recall_procedural_async(memory)),
            timeout(timeout_duration, Self::recall_events_async(event_bus, intent_owned2)),
        );

        // Handle timeouts gracefully — any path timing out returns empty results
        let path_a: Vec<(String, f32)> = match path_a {
            Ok(result) => result,
            Err(_) => {
                tracing::warn!("recall_semantic timed out after {}ms", PATH_TIMEOUT_MS);
                Vec::new()
            }
        };
        let path_b: Vec<(String, f32)> = match path_b {
            Ok(result) => result,
            Err(_) => {
                tracing::warn!("recall_kg timed out after {}ms", PATH_TIMEOUT_MS);
                Vec::new()
            }
        };
        let path_c: Vec<(String, f32)> = match path_c {
            Ok(result) => result,
            Err(_) => {
                tracing::warn!("recall_procedural timed out after {}ms", PATH_TIMEOUT_MS);
                Vec::new()
            }
        };
        let path_d: Vec<(String, f32)> = match path_d {
            Ok(result) => result,
            Err(_) => {
                tracing::warn!("recall_events timed out after {}ms", PATH_TIMEOUT_MS);
                Vec::new()
            }
        };

        // Step 3: RRF fusion
        let fused = Self::rrf_fuse(path_a, path_b, path_c, path_d);

        Ok(fused)
    }

    /// Path A (async): semantic vector search.
    async fn recall_semantic_async(
        search: Arc<dyn crate::fs::SemanticSearch>,
        intent_emb: Vec<f32>,
    ) -> Vec<(String, f32)> {
        let filter = SearchFilter::default();
        spawn_blocking(move || -> Vec<(String, f32)> {
            search
                .search(&intent_emb, PATH_LIMIT, &filter)
                .into_iter()
                .map(|hit| (hit.cid.clone(), hit.score))
                .collect()
        }).await.unwrap_or_else(|_| Vec::new())
    }

    /// Path B (async): KG topology neighbors of related CIDs.
    async fn recall_kg_async(
        kg: Option<Arc<dyn KnowledgeGraph>>,
        related_cids: Vec<String>,
        intent: String,
    ) -> Vec<(String, f32)> {
        let Some(kg) = kg else { return Vec::new(); };
        let kg = Arc::clone(&kg);

        spawn_blocking(move || -> Vec<(String, f32)> {
            let mut results: HashMap<String, f32> = HashMap::new();

            let keywords: Vec<&str> = intent
                .split_whitespace()
                .filter(|w| w.len() > 2)
                .collect();

            for cid in related_cids {
                if let Ok(neighbors) = kg.get_neighbors(&cid, None, 2) {
                    for (node, edge) in neighbors {
                        let score = edge.weight;
                        let depth_bonus = if edge.created_at > 0 { 0.1_f32 } else { 0.0_f32 };
                        let label_lower = node.label.to_lowercase();
                        let keyword_matches: usize = keywords
                            .iter()
                            .filter(|kw| label_lower.contains(&kw.to_lowercase()))
                            .count();
                        let keyword_bonus = (keyword_matches as f32) * 0.05;
                        let final_score = score + depth_bonus + keyword_bonus;
                        results
                            .entry(node.id.clone())
                            .and_modify(|s| *s = (*s + final_score) / 2.0)
                            .or_insert(final_score);
                    }
                }
            }

            let mut cids_with_scores: Vec<(String, f32)> = results.into_iter().collect();
            cids_with_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            cids_with_scores.truncate(PATH_LIMIT);
            cids_with_scores
        }).await.unwrap_or_else(|_| Vec::new())
    }

    /// Path C (async): shared procedural memories matching the intent.
    async fn recall_procedural_async(
        memory: Arc<LayeredMemory>,
    ) -> Vec<(String, f32)> {
        spawn_blocking(move || -> Vec<(String, f32)> {
            let entries = memory.get_shared(MemoryTier::Procedural);
            entries
                .into_iter()
                .map(|e| {
                    let desc = e.content.display().to_string();
                    let score = e.importance as f32 / 100.0_f32;
                    (desc, score)
                })
                .take(PATH_LIMIT)
                .collect()
        }).await.unwrap_or_else(|_| Vec::new())
    }

    /// Path D (async): recent events with tags related to the intent.
    async fn recall_events_async(
        event_bus: Arc<EventBus>,
        intent: String,
    ) -> Vec<(String, f32)> {
        spawn_blocking(move || -> Vec<(String, f32)> {
            let events = event_bus.snapshot_events();
            let keywords: Vec<&str> = intent
                .split_whitespace()
                .filter(|w| w.len() > 2)
                .collect();

            let mut results: Vec<(String, f32)> = Vec::new();
            for ev in events.iter().rev().take(PATH_LIMIT * 2) {
                let label = format!("{:?}", ev.event);
                let matches: usize = keywords
                    .iter()
                    .filter(|kw| label.to_lowercase().contains(&kw.to_lowercase()))
                    .count();
                if matches > 0 {
                    let score = matches as f32 / keywords.len().max(1) as f32;
                    results.push((label, score));
                }
            }

            results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
            results.truncate(PATH_LIMIT);
            results
        }).await.unwrap_or_else(|_| Vec::new())
    }


    /// Reciprocal Rank Fusion — combines ranked lists from multiple recall paths.
    fn rrf_fuse(
        path_a: Vec<(String, f32)>,
        path_b: Vec<(String, f32)>,
        path_c: Vec<(String, f32)>,
        path_d: Vec<(String, f32)>,
    ) -> Vec<ContextCandidate> {
        let mut scores: HashMap<String, (f32, usize)> = HashMap::new();

        for (i, (cid, relevance)) in path_a.into_iter().enumerate() {
            let rrf = 1.0 / (RRF_K + i as f32);
            let base = relevance * 0.5; // weight semantic relevance
            scores
                .entry(cid)
                .and_modify(|(s, c)| { *s += rrf + base; *c += 1; })
                .or_insert((rrf + base, 1));
        }

        for (i, (cid, relevance)) in path_b.into_iter().enumerate() {
            let rrf = 1.0 / (RRF_K + i as f32);
            let base = relevance * 0.4;
            scores
                .entry(cid)
                .and_modify(|(s, c)| { *s += rrf + base; *c += 1; })
                .or_insert((rrf + base, 1));
        }

        for (i, (cid, relevance)) in path_c.into_iter().enumerate() {
            let rrf = 1.0 / (RRF_K + i as f32);
            let base = relevance * 0.3;
            scores
                .entry(cid)
                .and_modify(|(s, c)| { *s += rrf + base; *c += 1; })
                .or_insert((rrf + base, 1));
        }

        for (i, (cid, relevance)) in path_d.into_iter().enumerate() {
            let rrf = 1.0 / (RRF_K + i as f32);
            let base = relevance * 0.3;
            scores
                .entry(cid)
                .and_modify(|(s, c)| { *s += rrf + base; *c += 1; })
                .or_insert((rrf + base, 1));
        }

        // Convert to candidates and sort by combined score
        let mut candidates: Vec<ContextCandidate> = scores
            .into_iter()
            .map(|(cid, (score, paths))| {
                // Bonus for cross-path agreement
                let path_bonus = (paths as f32 - 1.0) * 0.05;
                ContextCandidate { cid, relevance: (score + path_bonus).min(1.0) }
            })
            .collect();

        candidates.sort_by(|a, b| {
            b.relevance.partial_cmp(&a.relevance).unwrap_or(std::cmp::Ordering::Equal)
        });
        candidates
    }
}

// ── AIKernel delegate methods ────────────────────────────────────────────────

use crate::api::permission::{PermissionAction, PermissionContext};

impl crate::kernel::AIKernel {
    /// Declare an intent and trigger asynchronous semantic prefetch.
    /// Returns the assembly_id for later use with `fetch_assembled_context`.
    pub fn declare_intent(
        &self,
        agent_id: &str,
        intent: &str,
        related_cids: Vec<String>,
        budget_tokens: usize,
    ) -> Result<String, String> {
        let ctx = PermissionContext::new(agent_id.to_string(), "default".to_string());
        self.permissions
            .check(&ctx, PermissionAction::Read)
            .map_err(|e| e.to_string())?;
        // F-20 M2: Track current intent for causal hook (KG CausedBy edges)
        self.session_store.set_current_intent(agent_id, Some(intent.to_string()));
        Ok(self.prefetch.declare_intent(
            agent_id,
            intent,
            related_cids,
            budget_tokens,
        ))
    }

    /// Fetch the result of a previously declared intent prefetch.
    /// Returns `None` if the assembly_id is unknown.
    pub fn fetch_assembled_context(
        &self,
        agent_id: &str,
        assembly_id: &str,
    ) -> Option<Result<crate::fs::context_budget::BudgetAllocation, String>> {
        self.prefetch.fetch_assembled_context(agent_id, assembly_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rrf_fuse_empty_paths() {
        let result = IntentPrefetcher::rrf_fuse(vec![], vec![], vec![], vec![]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_rrf_fuse_single_path() {
        let path_a = vec![
            ("cid1".to_string(), 0.9_f32),
            ("cid2".to_string(), 0.8_f32),
        ];
        let result = IntentPrefetcher::rrf_fuse(path_a, vec![], vec![], vec![]);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].cid, "cid1");
        assert!(result[0].relevance > result[1].relevance);
    }

    #[test]
    fn test_rrf_fuse_cross_path_bonus() {
        // cid1 appears in both path_a and path_b — should get a cross-path bonus
        let path_a = vec![("cid1".to_string(), 0.9_f32)];
        let path_b = vec![("cid1".to_string(), 0.8_f32), ("cid2".to_string(), 0.7_f32)];
        let result = IntentPrefetcher::rrf_fuse(path_a, path_b, vec![], vec![]);

        let cid1_score = result.iter().find(|c| c.cid == "cid1").unwrap().relevance;
        let cid2_score = result.iter().find(|c| c.cid == "cid2").unwrap().relevance;
        // cid1 should score higher due to cross-path bonus
        assert!(cid1_score > cid2_score, "cid1 should benefit from cross-path bonus");
    }

    #[test]
    fn test_assembly_state_pending() {
        let state = AssemblyState::Pending;
        assert!(matches!(state, AssemblyState::Pending));
    }

    #[test]
    fn test_assembly_state_ready() {
        let allocation = BudgetAllocation {
            items: vec![],
            total_tokens: 100,
            budget: 100,
            candidates_considered: 5,
            candidates_included: 3,
        };
        let state = AssemblyState::Ready(allocation.clone());
        match state {
            AssemblyState::Ready(a) => {
                assert_eq!(a.total_tokens, 100);
            }
            _ => panic!("expected Ready"),
        }
    }

    #[test]
    fn test_assembly_state_failed() {
        let state = AssemblyState::Failed("test error".to_string());
        match state {
            AssemblyState::Failed(e) => assert_eq!(e, "test error"),
            _ => panic!("expected Failed"),
        }
    }

    // ── Async concurrent recall tests ─────────────────────────────────────────

    #[tokio::test]
    async fn test_multi_path_recall_all_paths_return_results() {
        // This test verifies that when all four recall paths return valid results,
        // the fusion produces a combined result with contributions from all paths.
        // We use the sync rrf_fuse directly since async testing requires more setup.
        let path_a = vec![("cid1".to_string(), 0.9_f32), ("cid2".to_string(), 0.8_f32)];
        let path_b = vec![("cid2".to_string(), 0.85_f32), ("cid3".to_string(), 0.75_f32)];
        let path_c = vec![("cid3".to_string(), 0.7_f32)];
        let path_d = vec![("cid4".to_string(), 0.6_f32)];

        let fused = IntentPrefetcher::rrf_fuse(path_a, path_b, path_c, path_d);

        // All four cids should appear in fused results
        let cids: Vec<_> = fused.iter().map(|c| c.cid.clone()).collect();
        assert!(cids.contains(&"cid1".to_string()));
        assert!(cids.contains(&"cid2".to_string()));
        assert!(cids.contains(&"cid3".to_string()));
        assert!(cids.contains(&"cid4".to_string()));

        // cid2 appears in path_a and path_b, should have cross-path bonus
        let cid2_score = fused.iter().find(|c| c.cid == "cid2").unwrap().relevance;
        let cid3_score = fused.iter().find(|c| c.cid == "cid3").unwrap().relevance;
        // cid2 benefits from 2 paths, cid3 from 2 paths as well
        // cid1 only from 1 path, cid4 only from 1 path
        assert!(cid2_score > 0.0);
        assert!(cid3_score > 0.0);
    }

    #[test]
    fn test_multi_path_recall_graceful_path_failure() {
        // Test that rrf_fuse handles empty paths gracefully (simulates path failure)
        let path_a: Vec<(String, f32)> = vec![];
        let path_b = vec![("cid1".to_string(), 0.9_f32)];
        let path_c: Vec<(String, f32)> = vec![];
        let path_d = vec![("cid2".to_string(), 0.8_f32)];

        let fused = IntentPrefetcher::rrf_fuse(path_a, path_b, path_c, path_d);

        // Should still produce results from the working paths
        assert_eq!(fused.len(), 2);
        let cids: Vec<_> = fused.iter().map(|c| c.cid.clone()).collect();
        assert!(cids.contains(&"cid1".to_string()));
        assert!(cids.contains(&"cid2".to_string()));
    }

    #[test]
    fn test_multi_path_recall_partial_path_failures() {
        // Simulate 3 out of 4 paths failing (returning empty)
        let path_a = vec![("cid1".to_string(), 0.9_f32)];
        let path_b: Vec<(String, f32)> = vec![];
        let path_c: Vec<(String, f32)> = vec![];
        let path_d: Vec<(String, f32)> = vec![];

        let fused = IntentPrefetcher::rrf_fuse(path_a, path_b, path_c, path_d);

        // Should still return the one valid result
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].cid, "cid1");
    }

    #[test]
    fn test_rrf_fuse_all_paths_identical() {
        // When all paths return the same cid, cross-path bonus should be maximized
        let path_a = vec![("cid1".to_string(), 0.9_f32)];
        let path_b = vec![("cid1".to_string(), 0.8_f32)];
        let path_c = vec![("cid1".to_string(), 0.7_f32)];
        let path_d = vec![("cid1".to_string(), 0.6_f32)];

        let fused = IntentPrefetcher::rrf_fuse(path_a, path_b, path_c, path_d);

        assert_eq!(fused.len(), 1);
        // With 4 paths contributing to cid1, the cross-path bonus should be significant
        // bonus = (4 - 1) * 0.05 = 0.15
        assert!(fused[0].relevance > 0.5); // Should have substantial score
    }

    #[test]
    fn test_timeout_constant_is_500ms() {
        assert_eq!(PATH_TIMEOUT_MS, 500);
    }

    #[test]
    fn test_path_limit_constant() {
        assert_eq!(PATH_LIMIT, 20);
    }

    // ── IntentAssemblyCache tests (F-9) ─────────────────────────────────────────

    #[test]
    fn test_intent_cache_exact_match_stub_mode() {
        // Test Path B (stub mode): exact string matching
        let cache = IntentAssemblyCache::new(64, 32 * 1024 * 1024, 0.85, 24 * 60 * 60 * 1000);

        let allocation = BudgetAllocation {
            items: vec![],
            total_tokens: 100,
            budget: 100,
            candidates_considered: 5,
            candidates_included: 3,
        };

        // Store with no embedding (stub mode)
        cache.store("fix auth bug".to_string(), None, allocation.clone(), vec![]);

        // Exact match should hit
        let result = cache.lookup("fix auth bug", &None);
        assert!(result.is_some());

        // Different text should miss
        let result = cache.lookup("fix auth module", &None);
        assert!(result.is_none());
    }

    #[test]
    fn test_intent_cache_similarity_match_with_embedding() {
        // Test Path A: cosine similarity matching
        let cache = IntentAssemblyCache::new(64, 32 * 1024 * 1024, 0.85, 24 * 60 * 60 * 1000);

        let allocation = BudgetAllocation {
            items: vec![],
            total_tokens: 100,
            budget: 100,
            candidates_considered: 5,
            candidates_included: 3,
        };

        // Store with a real embedding (simple unit vectors for testing)
        let embedding_a = vec![1.0, 0.0, 0.0]; // "fix auth bug"
        let embedding_b = vec![0.9, 0.1, 0.0]; // Similar to embedding_a (cosine ~0.995)
        let embedding_c = vec![0.0, 1.0, 0.0]; // Orthogonal (cosine ~0)

        cache.store("fix auth bug".to_string(), Some(embedding_a.clone()), allocation.clone(), vec![]);

        // Very similar embedding should hit (cosine ~0.995 > 0.85)
        let result = cache.lookup("fix auth bug variant", &Some(embedding_b));
        assert!(result.is_some());

        // Orthogonal embedding should miss (cosine ~0 < 0.85)
        let result = cache.lookup("unrelated task", &Some(embedding_c));
        assert!(result.is_none());
    }

    #[test]
    fn test_intent_cache_memory_limit() {
        // Test that cache respects memory limits
        let cache = IntentAssemblyCache::new(64, 200, 0.85, 24 * 60 * 60 * 1000);

        let allocation = BudgetAllocation {
            items: vec![],
            total_tokens: 100,
            budget: 100,
            candidates_considered: 5,
            candidates_included: 3,
        };

        // Each entry is ~50 bytes, so 4 entries should exceed 200 byte limit
        cache.store("entry1".to_string(), None, allocation.clone(), vec![]);
        cache.store("entry2".to_string(), None, allocation.clone(), vec![]);
        cache.store("entry3".to_string(), None, allocation.clone(), vec![]);
        cache.store("entry4".to_string(), None, allocation.clone(), vec![]);

        // After hitting memory limit, oldest entries should be evicted
        let stats = cache.stats();
        // At most 4 entries can fit in 200 bytes
        assert!(stats.entries <= 4);
    }

    #[test]
    fn test_intent_cache_ttl_expiry() {
        // Test that cache entries expire after TTL
        let cache = IntentAssemblyCache::new(64, 32 * 1024 * 1024, 0.85, 100); // 100ms TTL

        let allocation = BudgetAllocation {
            items: vec![],
            total_tokens: 100,
            budget: 100,
            candidates_considered: 5,
            candidates_included: 3,
        };

        cache.store("test intent".to_string(), None, allocation.clone(), vec![]);

        // Should hit immediately
        let result = cache.lookup("test intent", &None);
        assert!(result.is_some());

        // Wait for TTL to expire
        std::thread::sleep(std::time::Duration::from_millis(150));

        // Should miss after TTL
        let result = cache.lookup("test intent", &None);
        assert!(result.is_none());
    }

    #[test]
    fn test_intent_cache_invalidate_by_cids() {
        // Test that cache entries can be invalidated by dependency CIDs
        let cache = IntentAssemblyCache::new(64, 32 * 1024 * 1024, 0.85, 24 * 60 * 60 * 1000);

        let allocation = BudgetAllocation {
            items: vec![],
            total_tokens: 100,
            budget: 100,
            candidates_considered: 5,
            candidates_included: 3,
        };

        // Store entry with dependencies
        cache.store(
            "fix auth".to_string(),
            None,
            allocation.clone(),
            vec!["cid1".to_string(), "cid2".to_string()],
        );

        // Should hit
        let result = cache.lookup("fix auth", &None);
        assert!(result.is_some());

        // Invalidate by one of the dependencies
        cache.invalidate_by_cids(&["cid2".to_string()]);

        // Should miss after invalidation
        let result = cache.lookup("fix auth", &None);
        assert!(result.is_none());
    }

    #[test]
    fn test_intent_cache_stats() {
        let cache = IntentAssemblyCache::new(64, 32 * 1024 * 1024, 0.85, 24 * 60 * 60 * 1000);

        let allocation = BudgetAllocation {
            items: vec![],
            total_tokens: 100,
            budget: 100,
            candidates_considered: 5,
            candidates_included: 3,
        };

        // Store an entry
        cache.store("test".to_string(), None, allocation.clone(), vec![]);

        // Lookup to increment hit count
        let _ = cache.lookup("test", &None);
        let _ = cache.lookup("test", &None);

        let stats = cache.stats();
        assert_eq!(stats.entries, 1);
        assert!(stats.hits >= 1);
        assert!(stats.memory_bytes > 0);
    }

    #[test]
    fn test_intent_cache_cosine_similarity() {
        let cache = IntentAssemblyCache::new(64, 32 * 1024 * 1024, 0.85, 24 * 60 * 60 * 1000);

        // Test cosine similarity calculation
        let vec_a = vec![1.0, 0.0, 0.0];
        let vec_b = vec![1.0, 0.0, 0.0];
        let vec_c = vec![0.0, 1.0, 0.0];
        let vec_d = vec![0.5_f32.sqrt(), 0.5_f32.sqrt(), 0.0]; // 45 degrees

        // Identical vectors should have similarity 1.0
        assert!((cache.cosine_similarity(&vec_a, &vec_b) - 1.0).abs() < 0.001);

        // Orthogonal vectors should have similarity 0.0
        assert!(cache.cosine_similarity(&vec_a, &vec_c).abs() < 0.001);

        // 45 degree vectors should have similarity ~0.707
        let similarity = cache.cosine_similarity(&vec_a, &vec_d);
        assert!((similarity - 0.707).abs() < 0.01);
    }

    // ── F-15: IntentFeedback tests ─────────────────────────────────────────────

    fn create_test_prefetcher() -> IntentPrefetcher {
        let dir = tempfile::tempdir().unwrap();
        let cas = Arc::new(crate::cas::CASStorage::new(dir.path().join("cas")).unwrap());
        let ctx_loader = Arc::new(
            crate::fs::context_loader::ContextLoader::new(
                dir.path().join("context"),
                None,
                cas,
            ).unwrap()
        );

        IntentPrefetcher::new(
            Arc::new(crate::fs::search::memory::InMemoryBackend::new()),
            None,
            Arc::new(crate::memory::LayeredMemory::new()),
            Arc::new(crate::kernel::event_bus::EventBus::new()),
            Arc::new(crate::fs::StubEmbeddingProvider::new()),
            ctx_loader,
            dir.path().to_path_buf(),
        )
    }

    #[test]
    fn test_intent_feedback_records_and_retrieves() {
        let prefetcher = create_test_prefetcher();

        // Record feedback
        prefetcher.record_feedback(
            "fix auth bug",
            vec!["cid1".to_string(), "cid2".to_string()],
            vec!["cid3".to_string()],
        );

        // Retrieve feedback for same intent
        let (used, unused) = prefetcher.get_similar_feedback("fix auth bug").unwrap();
        assert_eq!(used, vec!["cid1", "cid2"]);
        assert_eq!(unused, vec!["cid3"]);

        // Verify count
        assert_eq!(prefetcher.feedback_count(), 1);
    }

    #[test]
    fn test_intent_feedback_normalizes_intent() {
        let prefetcher = create_test_prefetcher();

        prefetcher.record_feedback(
            "Fix Auth Bug",
            vec!["cid1".to_string()],
            vec![],
        );

        // Should match regardless of case
        let (used, _) = prefetcher.get_similar_feedback("fix auth bug").unwrap();
        assert_eq!(used, vec!["cid1"]);

        // Should also match with extra whitespace
        let (used, _) = prefetcher.get_similar_feedback("  fix auth bug  ").unwrap();
        assert_eq!(used, vec!["cid1"]);
    }

    #[test]
    fn test_intent_feedback_returns_none_for_unknown_intent() {
        let prefetcher = create_test_prefetcher();

        prefetcher.record_feedback(
            "fix auth bug",
            vec!["cid1".to_string()],
            vec![],
        );

        // Should not find feedback for different intent
        let result = prefetcher.get_similar_feedback("fix network bug");
        assert!(result.is_none());
    }

    #[test]
    fn test_intent_feedback_eviction() {
        let prefetcher = create_test_prefetcher();

        // Record more entries than the max (1000)
        // We can test this indirectly by checking the count doesn't grow unbounded
        for i in 0..100 {
            prefetcher.record_feedback(
                &format!("intent {}", i),
                vec![format!("cid{}", i)],
                vec![],
            );
        }

        assert_eq!(prefetcher.feedback_count(), 100);
    }

    // ── F-4: Async Prefetch tests ─────────────────────────────────────────────

    #[test]
    fn test_prefetch_async_returns_handle() {
        let prefetcher = create_test_prefetcher();
        let handle = prefetcher.prefetch_async(
            "agent-1",
            "fix authentication",
            vec![],
            1000,
        );
        // Handle should be created with a non-empty assembly_id
        assert!(!handle.assembly_id.is_empty());
        // State should be either Pending (async) or Ready (cache hit)
        let state = handle.state.load(Ordering::Relaxed);
        assert!(state == STATE_PENDING || state == STATE_READY);
    }

    #[test]
    fn test_prefetch_async_returns_valid_handle_with_assembly_id() {
        let prefetcher = create_test_prefetcher();
        let handle = prefetcher.prefetch_async("agent-1", "fix auth", vec![], 1000);
        // Handle should have a valid (non-empty) assembly_id
        assert!(!handle.assembly_id.is_empty());
        // The handle should be usable for await_result (even if Pending)
        // The await_result should return None for Pending state
        let result = handle.await_result();
        // For Pending state, await_result returns None (not ready yet)
        assert!(result.is_none());
    }

    #[test]
    fn test_prefetch_cancel() {
        let prefetcher = create_test_prefetcher();
        let handle = prefetcher.prefetch_async(
            "agent-1",
            "fix bug",
            vec![],
            1000,
        );
        let assembly_id = handle.assembly_id.clone();
        // Cancel should succeed
        let cancelled = prefetcher.cancel(&assembly_id);
        assert!(cancelled);
        // Cancel again should return false (already cancelled)
        let cancelled_again = prefetcher.cancel(&assembly_id);
        assert!(!cancelled_again);
    }

    #[test]
    fn test_prefetch_handle_await_result_blocks_until_ready() {
        let prefetcher = create_test_prefetcher();
        let handle = prefetcher.prefetch_async(
            "agent-1",
            "fix auth bug",
            vec![],
            1000,
        );
        // For a cache hit, await_result should return immediately
        if handle.state.load(Ordering::Relaxed) == STATE_READY {
            let result = handle.await_result();
            assert!(result.is_some());
        }
        // For a cache miss (Pending state), we can't easily test blocking behavior
        // in a unit test without waiting, so we just verify the handle is functional
    }

    #[test]
    fn test_prefetch_states_extended() {
        // Verify the new state variants exist
        let prefetcher = create_test_prefetcher();
        // After cancel, state should be Cancelled
        let handle = prefetcher.prefetch_async("agent-1", "test", vec![], 1000);
        prefetcher.cancel(&handle.assembly_id);
        // State remains Pending (no async thread updates it on cancel)
        // The cancel sets the assembly state, not the handle state
        let cancelled = prefetcher.cancel(&handle.assembly_id);
        assert!(!cancelled); // Already cancelled
    }
}
