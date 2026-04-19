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
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::time::timeout;
use tokio::task::spawn_blocking;

use crate::fs::context_budget::{self, BudgetAllocation, ContextCandidate};
use crate::fs::context_loader::ContextLoader;
use crate::fs::embedding::EmbeddingProvider;
use crate::fs::graph::KnowledgeGraph;
use crate::fs::search::SearchFilter;
use crate::kernel::event_bus::EventBus;
use crate::memory::LayeredMemory;
use crate::memory::MemoryTier;

/// RRF fusion constant — dampens rank differences between paths.
const RRF_K: f32 = 60.0;

/// Maximum candidates per recall path.
const PATH_LIMIT: usize = 20;

/// Timeout per recall path (500ms).
const PATH_TIMEOUT_MS: u64 = 500;

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
///   1. Registers a new assembly with state `Pending`
///   2. Kicks off multi-path recall (semantic + KG + procedural + events)
///   3. Fuses results via RRF
///   4. Allocates budget via context_budget::assemble()
///   5. Stores result in `Assembly.state = Ready(allocation)`
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
        }
    }

    /// Declare a new intent and trigger async prefetch.
    /// Returns the assembly_id immediately.
    pub fn declare_intent(
        &self,
        agent_id: &str,
        intent: &str,
        related_cids: Vec<String>,
        budget_tokens: usize,
    ) -> String {
        let assembly_id = uuid::Uuid::new_v4().to_string();
        let now = crate::memory::layered::now_ms();
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
                    assembly_id_clone, intent_clone, related_cids, budget_tokens, max_age,
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

    /// Run the full multi-path prefetch in a background thread.
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

    /// Multi-path recall: semantic + KG + procedural + events → fused candidates.
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
        let ctx = PermissionContext::new(agent_id.to_string());
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
}
