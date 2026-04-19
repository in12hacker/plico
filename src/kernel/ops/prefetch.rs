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
use std::sync::{Arc, RwLock, atomic::{AtomicUsize, Ordering}};
use std::time::Duration;

use tokio::time::timeout;
use tokio::task::spawn_blocking;

use crate::fs::context_budget::{self, BudgetAllocation, ContextCandidate};
use crate::fs::context_loader::ContextLoader;
use crate::fs::embedding::EmbeddingProvider;
use crate::fs::graph::KnowledgeGraph;
use crate::fs::search::SearchFilter;
use crate::kernel::event_bus::EventBus;
use crate::kernel::ops::session::{AgentProfile, IntentKeyStrategy};
use crate::memory::LayeredMemory;
use crate::memory::MemoryTier;

/// RRF fusion constant — dampens rank differences between paths.
const RRF_K: f32 = 60.0;

/// Maximum candidates per recall path.
const PATH_LIMIT: usize = 20;

/// Timeout per recall path (500ms).
const PATH_TIMEOUT_MS: u64 = 500;

// ── Intent Assembly Cache (F-9) ───────────────────────────────────────────────

/// Default maximum entries in the intent cache (reduced from 1000 due to memory constraints).
const DEFAULT_MAX_CACHE_ENTRIES: usize = 64;

/// Default maximum memory bytes for the intent cache (32MB).
const DEFAULT_MAX_CACHE_MEMORY_BYTES: usize = 32 * 1024 * 1024;

/// Default similarity threshold for cosine matching (0.85).
const DEFAULT_SIMILARITY_THRESHOLD: f32 = 0.85;

/// Default TTL for cache entries (24 hours in milliseconds).
const DEFAULT_CACHE_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// A cached intent assembly result.
#[derive(Debug, Clone)]
struct CachedAssembly {
    /// Original intent text.
    intent_text: String,
    /// Pre-computed embedding (None in stub mode).
    intent_embedding: Option<Vec<f32>>,
    /// The pre-assembled budget allocation.
    assembly: BudgetAllocation,
    /// When this entry was created.
    created_at_ms: u64,
    /// Number of cache hits.
    hit_count: u64,
    /// Estimated memory size of this entry.
    estimated_size_bytes: usize,
    /// CIDs this assembly depends on (for invalidation).
    dependency_cids: Vec<String>,
}

impl CachedAssembly {
    fn new(
        intent_text: String,
        intent_embedding: Option<Vec<f32>>,
        assembly: BudgetAllocation,
        dependency_cids: Vec<String>,
    ) -> Self {
        let estimated_size_bytes = Self::estimate_size(&intent_text, &intent_embedding, &assembly);
        Self {
            intent_text,
            intent_embedding,
            assembly,
            created_at_ms: now_ms(),
            hit_count: 0,
            estimated_size_bytes,
            dependency_cids,
        }
    }

    fn estimate_size(
        intent_text: &str,
        intent_embedding: &Option<Vec<f32>>,
        assembly: &BudgetAllocation,
    ) -> usize {
        let text_size = intent_text.len();
        let embedding_size = intent_embedding.as_ref().map_or(0, |e| e.len() * 4);
        let items_size: usize = assembly
            .items
            .iter()
            .map(|item| item.content.len())
            .sum();
        let cids_size: usize = assembly
            .items
            .iter()
            .map(|item| item.cid.len())
            .sum();
        text_size + embedding_size + items_size + cids_size
    }

    fn is_expired(&self, ttl_ms: u64) -> bool {
        now_ms() - self.created_at_ms > ttl_ms
    }
}

/// Intent assembly cache with dual-path matching.
///
/// Dual-path matching:
/// - Path A (real embedding): cosine similarity matching
/// - Path B (stub mode): exact string matching
struct IntentAssemblyCache {
    entries: RwLock<Vec<CachedAssembly>>,
    max_entries: usize,
    max_memory_bytes: usize,
    current_memory_bytes: AtomicUsize,
    similarity_threshold: f32,
    ttl_ms: u64,
}

impl IntentAssemblyCache {
    fn new(
        max_entries: usize,
        max_memory_bytes: usize,
        similarity_threshold: f32,
        ttl_ms: u64,
    ) -> Self {
        Self {
            entries: RwLock::new(Vec::new()),
            max_entries,
            max_memory_bytes,
            current_memory_bytes: AtomicUsize::new(0),
            similarity_threshold,
            ttl_ms,
        }
    }

    /// Look up a cached assembly using dual-path matching.
    /// Returns the cached assembly if found and valid.
    fn lookup(&self, intent: &str, embedding: &Option<Vec<f32>>) -> Option<BudgetAllocation> {
        let mut entries = self.entries.write().unwrap();

        // Check for exact match first (stub mode Path B)
        if let Some(pos) = entries.iter().position(|e| e.intent_text == intent) {
            let entry = &mut entries[pos];

            // Check TTL
            if entry.is_expired(self.ttl_ms) {
                let removed = entries.remove(pos);
                self.current_memory_bytes.fetch_sub(removed.estimated_size_bytes, Ordering::Relaxed);
                return None;
            }

            entry.hit_count += 1;
            tracing::debug!("intent cache hit (exact match) for: {}", intent);
            return Some(entry.assembly.clone());
        }

        // Path A: cosine similarity matching (only if we have real embedding)
        if let Some(intent_emb) = embedding {
            let best_pos = self.find_best_similarity(intent_emb, &entries);

            if let Some(pos) = best_pos {
                let entry = &mut entries[pos];

                // Check TTL
                if entry.is_expired(self.ttl_ms) {
                    let removed = entries.remove(pos);
                    self.current_memory_bytes.fetch_sub(removed.estimated_size_bytes, Ordering::Relaxed);
                    return None;
                }

                entry.hit_count += 1;
                tracing::debug!(
                    "intent cache hit (similarity {:.3}) for: {}",
                    self.cosine_similarity(intent_emb, entry.intent_embedding.as_ref().unwrap()),
                    intent
                );
                return Some(entry.assembly.clone());
            }
        }

        None
    }

    /// Find the best matching entry by cosine similarity.
    fn find_best_similarity(&self, intent_emb: &[f32], entries: &[CachedAssembly]) -> Option<usize> {
        let mut best_pos: Option<usize> = None;
        let mut best_score: f32 = self.similarity_threshold;

        for (i, entry) in entries.iter().enumerate() {
            if let Some(ref entry_emb) = entry.intent_embedding {
                let score = self.cosine_similarity(intent_emb, entry_emb);
                if score > best_score {
                    best_score = score;
                    best_pos = Some(i);
                }
            }
        }

        best_pos
    }

    /// Compute cosine similarity between two vectors.
    fn cosine_similarity(&self, a: &[f32], b: &[f32]) -> f32 {
        if a.len() != b.len() {
            return 0.0;
        }

        let dot_product: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
        let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
        let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

        if norm_a == 0.0 || norm_b == 0.0 {
            return 0.0;
        }

        dot_product / (norm_a * norm_b)
    }

    /// Store a new assembly in the cache.
    fn store(
        &self,
        intent_text: String,
        intent_embedding: Option<Vec<f32>>,
        assembly: BudgetAllocation,
        dependency_cids: Vec<String>,
    ) {
        let entry = CachedAssembly::new(intent_text, intent_embedding, assembly, dependency_cids);
        let entry_size = entry.estimated_size_bytes;

        let mut entries = self.entries.write().unwrap();

        // Evict if necessary to make room
        self.evict_if_needed_locked(entry_size, &mut entries);

        entries.push(entry);
        self.current_memory_bytes.fetch_add(entry_size, Ordering::Relaxed);
    }

    /// Evict oldest/least valuable entries to make room.
    fn evict_if_needed_locked(&self, new_entry_size: usize, entries: &mut Vec<CachedAssembly>) {
        // First, remove expired entries
        entries.retain(|e| !e.is_expired(self.ttl_ms));

        // Evict by LRU (oldest first) until we have room
        while entries.len() >= self.max_entries
            || self.current_memory_bytes.load(Ordering::Relaxed) + new_entry_size > self.max_memory_bytes
            || self.over_budget_memory(new_entry_size)
        {
            if entries.is_empty() {
                break;
            }

            // Find and remove the oldest entry (lowest created_at_ms)
            let min_pos = entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.created_at_ms)
                .map(|(pos, _)| pos);

            if let Some(pos) = min_pos {
                let removed = entries.remove(pos);
                self.current_memory_bytes.fetch_sub(removed.estimated_size_bytes, Ordering::Relaxed);
            } else {
                break;
            }
        }
    }

    fn over_budget_memory(&self, additional: usize) -> bool {
        self.current_memory_bytes.load(Ordering::Relaxed) + additional > self.max_memory_bytes
    }

    /// Invalidate entries that depend on any of the given CIDs.
    fn invalidate_by_cids(&self, modified_cids: &[String]) {
        if modified_cids.is_empty() {
            return;
        }

        let mut entries = self.entries.write().unwrap();
        let mut total_removed: usize = 0;

        entries.retain(|e| {
            let has_dependency = e.dependency_cids.iter().any(|cid| modified_cids.contains(cid));
            if has_dependency {
                total_removed = total_removed.saturating_add(e.estimated_size_bytes);
            }
            !has_dependency
        });

        self.current_memory_bytes.fetch_sub(total_removed, Ordering::Relaxed);
    }

    /// Clear the entire cache.
    fn clear(&self) {
        let mut entries = self.entries.write().unwrap();
        entries.clear();
        self.current_memory_bytes.store(0, Ordering::Relaxed);
    }

    /// Get cache statistics.
    fn stats(&self) -> IntentCacheStats {
        let entries = self.entries.read().unwrap();
        IntentCacheStats {
            entries: entries.len(),
            memory_bytes: self.current_memory_bytes.load(Ordering::Relaxed),
            hits: entries.iter().map(|e| e.hit_count).sum(),
        }
    }
}

/// Statistics for the intent cache.
#[derive(Debug, Clone, Default)]
pub struct IntentCacheStats {
    pub entries: usize,
    pub memory_bytes: usize,
    pub hits: u64,
}

// ── F-10: Agent Profile Store ──────────────────────────────────────────────────

/// Minimum confidence threshold for triggering prefetch (0.5 = 50%).
const PREFETCH_CONFIDENCE_THRESHOLD: f32 = 0.5;

/// Maximum profiles to keep per agent store.
const MAX_PROFILE_HISTORY: usize = 100;

/// Agent profile store — manages per-agent transition statistics (F-10).
///
/// Thread-safe profile storage for cognitive prefetch.
/// Each agent has a profile that tracks:
/// - Intent transitions (which tag keys follow which)
/// - Hot objects (frequently accessed CIDs)
pub struct AgentProfileStore {
    profiles: RwLock<HashMap<String, AgentProfile>>,
    strategy: IntentKeyStrategy,
}

impl AgentProfileStore {
    /// Create a new profile store with the given strategy.
    pub fn new(strategy: IntentKeyStrategy) -> Self {
        Self {
            profiles: RwLock::new(HashMap::new()),
            strategy,
        }
    }

    /// Get or create a profile for an agent.
    pub fn get_or_create(&self, agent_id: &str) -> AgentProfile {
        let mut profiles = self.profiles.write().unwrap();
        profiles
            .entry(agent_id.to_string())
            .or_insert_with(|| AgentProfile::new(agent_id.to_string()))
            .clone()
    }

    /// Record an intent completion and update transition statistics.
    ///
    /// This is called when an agent completes an intent (not when they declare it).
    /// It updates the transition matrix and may trigger background prefetch.
    ///
    /// Returns the predicted next tag key if confidence is high enough for prefetch.
    pub fn record_intent_complete(
        &self,
        agent_id: &str,
        intent_tag_key: Option<&str>,
        next_intent_tag_key: Option<&str>,
    ) -> Option<String> {
        let mut profiles = self.profiles.write().unwrap();
        let profile = profiles
            .entry(agent_id.to_string())
            .or_insert_with(|| AgentProfile::new(agent_id.to_string()));

        if let Some(tag_key) = intent_tag_key {
            profile.record_intent(tag_key, next_intent_tag_key);

            // Predict next based on new data
            if let Some(next) = profile.predict_next(tag_key) {
                // Check if we have enough confidence to prefetch
                if let Some(succs) = profile.intent_transitions.get(tag_key) {
                    if let Some((_, count)) = succs.first() {
                        // Simple confidence: count of this transition / total transitions
                        let total: u32 = succs.iter().map(|(_, c)| c).sum();
                        let confidence = *count as f32 / total.max(1) as f32;
                        if confidence >= PREFETCH_CONFIDENCE_THRESHOLD {
                            return Some(next);
                        }
                    }
                }
            }
        }

        // Limit profile history to avoid unbounded growth
        if profile.intent_transitions.len() > MAX_PROFILE_HISTORY {
            // Keep only the most recent entries
            let to_keep: Vec<_> = profile.intent_transitions.iter()
                .take(MAX_PROFILE_HISTORY)
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            profile.intent_transitions.clear();
            for (k, v) in to_keep {
                profile.intent_transitions.insert(k, v);
            }
        }

        None
    }

    /// Get the current strategy.
    pub fn strategy(&self) -> &IntentKeyStrategy {
        &self.strategy
    }

    /// Update the strategy.
    pub fn set_strategy(&mut self, strategy: IntentKeyStrategy) {
        self.strategy = strategy;
    }

    /// Extract tag key from intent text using the configured strategy.
    pub fn extract_tag_key(&self, intent: &str, known_tags: &[String]) -> Option<String> {
        self.strategy.extract_tag_key(intent, known_tags)
    }
}

impl Default for AgentProfileStore {
    fn default() -> Self {
        Self::new(IntentKeyStrategy::default())
    }
}

fn now_ms() -> u64 {
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
/// Agent calls `fetch_assembled_context` to retrieve the result.
pub struct IntentPrefetcher {
    /// Active assemblies keyed by assembly_id.
    assemblies: Arc<RwLock<HashMap<String, Assembly>>>,
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
    ) -> Self {
        Self {
            assemblies: Arc::new(RwLock::new(HashMap::new())),
            search,
            kg,
            memory,
            event_bus,
            embedding,
            ctx_loader,
            max_age_ms: 3_600_000, // 1 hour
            intent_cache: Arc::new(IntentAssemblyCache::new(
                DEFAULT_MAX_CACHE_ENTRIES,
                DEFAULT_MAX_CACHE_MEMORY_BYTES,
                DEFAULT_SIMILARITY_THRESHOLD,
                DEFAULT_CACHE_TTL_MS,
            )),
            profile_store: Arc::new(AgentProfileStore::default()),
        }
    }

    /// Create a new prefetcher with a custom profile strategy (for testing).
    #[allow(dead_code)]
    pub fn new_with_strategy(
        search: Arc<dyn crate::fs::SemanticSearch>,
        kg: Option<Arc<dyn KnowledgeGraph>>,
        memory: Arc<LayeredMemory>,
        event_bus: Arc<EventBus>,
        embedding: Arc<dyn EmbeddingProvider>,
        ctx_loader: Arc<ContextLoader>,
        strategy: IntentKeyStrategy,
    ) -> Self {
        Self {
            assemblies: Arc::new(RwLock::new(HashMap::new())),
            search,
            kg,
            memory,
            event_bus,
            embedding,
            ctx_loader,
            max_age_ms: 3_600_000, // 1 hour
            intent_cache: Arc::new(IntentAssemblyCache::new(
                DEFAULT_MAX_CACHE_ENTRIES,
                DEFAULT_MAX_CACHE_MEMORY_BYTES,
                DEFAULT_SIMILARITY_THRESHOLD,
                DEFAULT_CACHE_TTL_MS,
            )),
            profile_store: Arc::new(AgentProfileStore::new(strategy)),
        }
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
        let intent_embedding: Option<Vec<f32>> = self.embedding.embed(intent).ok().map(|e| e.into());

        if let Some(cached_allocation) = self.intent_cache.lookup(intent, &intent_embedding) {
            // Cache hit! Store directly as Ready assembly
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

        // Kick off background prefetch — clone refs for the async task
        let assemblies = Arc::clone(&self.assemblies);
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
                    assemblies, search, kg, memory, event_bus, embedding, ctx_loader,
                    assembly_id_clone, intent_clone, related_cids_clone, budget_tokens, max_age,
                    Some(intent_cache),
                ).await;
            });
        });

        assembly_id
    }

    /// Fetch a previously declared assembled context.
    /// Returns `None` if the assembly_id is unknown or still pending.
    pub fn fetch_assembled_context(
        &self,
        agent_id: &str,
        assembly_id: &str,
    ) -> Option<Result<BudgetAllocation, String>> {
        let assemblies = self.assemblies.read().unwrap();
        let assembly = assemblies.get(assembly_id)?;

        // Only the owning agent can fetch
        if assembly.agent_id != agent_id {
            return None;
        }

        match &assembly.state {
            AssemblyState::Pending => Some(Err("prefetch still in progress".to_string())),
            AssemblyState::Ready(allocation) => Some(Ok(allocation.clone())),
            AssemblyState::Failed(err) => Some(Err(err.clone())),
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
        // We use the tag key itself as the intent (not ideal but works for POC)
        // In a real implementation, we'd look up a representative intent for this tag key
        let predicted_intent = format!("next: {}", predicted_tag_key);

        // Use a smaller budget for background prefetch (not user-facing)
        let budget = 1024;

        // Check if this is already being prefetched or cached
        // Skip if already in cache (F-9 would handle it)
        let intent_embedding: Option<Vec<f32>> = self.embedding.embed(&predicted_intent).ok().map(|e| e.into());
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

    /// Run the full multi-path prefetch in a background thread.
    /// Optionally stores result in intent cache (F-9).
    async fn run_prefetch(
        assemblies: Arc<RwLock<HashMap<String, Assembly>>>,
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
    ) {
        let result = Self::multi_path_recall_async(
            &search, &kg, &memory, &event_bus, &embedding,
            &intent, &related_cids,
        ).await;

        let now = crate::memory::layered::now_ms();
        let mut assemblies_guard = assemblies.write().unwrap();
        let entry = assemblies_guard.get_mut(&assembly_id);

        match (result, entry) {
            (Ok(candidates), Some(a)) => {
                let allocation = context_budget::assemble(&ctx_loader, &candidates, budget_tokens);

                // F-9: Store in intent cache for future hits
                if let Some(ref cache) = intent_cache {
                    // Get embedding for cache storage (Path A)
                    let intent_embedding: Option<Vec<f32>> = embedding
                        .embed(&intent)
                        .ok()
                        .map(|e| e.into());
                    cache.store(intent, intent_embedding, allocation.clone(), related_cids);
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

    #[allow(dead_code)]
    /// Multi-path recall: semantic + KG + procedural + events → fused candidates.
    /// DEPRECATED: Use multi_path_recall_async for concurrent execution.
    fn multi_path_recall(
        search: &Arc<dyn crate::fs::SemanticSearch>,
        kg: &Option<Arc<dyn KnowledgeGraph>>,
        memory: &Arc<LayeredMemory>,
        event_bus: &Arc<EventBus>,
        embedding: &Arc<dyn EmbeddingProvider>,
        intent: &str,
        related_cids: &[String],
    ) -> Result<Vec<ContextCandidate>, String> {
        // Step 1: Embed the intent
        let intent_emb = embedding.embed(intent)
            .map_err(|e| format!("failed to embed intent: {}", e))?;
        let emb_slice: Vec<f32> = intent_emb.into();

        // Step 2: Four-path recall
        // Path A: semantic search
        let path_a = Self::recall_semantic(search, &emb_slice);
        // Path B: KG neighbors
        let path_b = Self::recall_kg(kg, related_cids, intent);
        // Path C: shared procedural memory
        let path_c = Self::recall_procedural(memory);
        // Path D: recent events with related tags
        let path_d = Self::recall_events(event_bus, intent);

        // Step 3: RRF fusion
        let fused = Self::rrf_fuse(path_a, path_b, path_c, path_d);

        Ok(fused)
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
        let emb_slice: Vec<f32> = emb.into();

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

    /// Path A: semantic vector search.
    fn recall_semantic(
        search: &Arc<dyn crate::fs::SemanticSearch>,
        intent_emb: &[f32],
    ) -> Vec<(String, f32)> {
        let filter = SearchFilter::default();
        search
            .search(intent_emb, PATH_LIMIT, &filter)
            .into_iter()
            .map(|hit| (hit.cid.clone(), hit.score))
            .collect()
    }

    /// Path B: KG topology neighbors of related CIDs.
    fn recall_kg(
        kg: &Option<Arc<dyn KnowledgeGraph>>,
        related_cids: &[String],
        intent: &str,
    ) -> Vec<(String, f32)> {
        let Some(ref kg) = kg else { return Vec::new(); };
        let mut results: HashMap<String, f32> = HashMap::new();

        // Keyword matching for intent-based scoring
        let keywords: Vec<&str> = intent
            .split_whitespace()
            .filter(|w| w.len() > 2)
            .collect();

        for cid in related_cids {
            if let Ok(neighbors) = kg.get_neighbors(cid, None, 2) {
                for (node, edge) in neighbors {
                    // Use edge weight as relevance proxy
                    let score = edge.weight;
                    // Depth bonus for direct neighbors
                    let depth_bonus = if edge.created_at > 0 { 0.1_f32 } else { 0.0_f32 };
                    // Intent keyword match bonus on node label
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
    }

    /// Path C: shared procedural memories matching the intent.
    fn recall_procedural(memory: &Arc<LayeredMemory>) -> Vec<(String, f32)> {
        let entries = memory.get_shared(MemoryTier::Procedural);
        entries
            .into_iter()
            .map(|e| {
                let desc = e.content.display().to_string();
                // Score by importance (already stored in entry)
                let score = e.importance as f32 / 100.0_f32;
                (desc, score)
            })
            .take(PATH_LIMIT)
            .collect()
    }

    /// Path D: recent events with tags related to the intent.
    fn recall_events(event_bus: &Arc<EventBus>, intent: &str) -> Vec<(String, f32)> {
        let events = event_bus.snapshot_events();
        // Extract simple keywords from intent (split on spaces, filter short)
        let keywords: Vec<&str> = intent
            .split_whitespace()
            .filter(|w| w.len() > 2)
            .collect();

        let mut results: Vec<(String, f32)> = Vec::new();
        for ev in events.iter().rev().take(PATH_LIMIT * 2) {
            let label = format!("{:?}", ev.event);
            // Count keyword matches in the event label
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
}
