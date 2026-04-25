//! Intent Assembly Cache (F-9) — dual-path matching with cosine similarity + exact string.
//!
//! Extracted from `prefetch.rs` for independent evolution.
//! Cache management changes independently from the core prefetch engine.

use std::sync::{RwLock, atomic::{AtomicUsize, AtomicU64, Ordering}};
use std::path::Path;

use serde::{Deserialize, Serialize};
use crate::fs::context_budget::BudgetAllocation;

use super::prefetch::now_ms;

/// Default maximum entries in the intent cache (reduced from 1000 due to memory constraints).
const DEFAULT_MAX_CACHE_ENTRIES: usize = 64;

/// Default maximum memory bytes for the intent cache (32MB).
const DEFAULT_MAX_CACHE_MEMORY_BYTES: usize = 32 * 1024 * 1024;

/// Default similarity threshold for cosine matching (0.85).
const DEFAULT_SIMILARITY_THRESHOLD: f32 = 0.85;

/// Default TTL for cache entries (24 hours in milliseconds).
const DEFAULT_CACHE_TTL_MS: u64 = 24 * 60 * 60 * 1000;

/// Filename for persisting the intent cache.
const CACHE_PERSIST_FILE: &str = "intent_cache.json";

/// A cached intent assembly result.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedAssembly {
    intent_text: String,
    intent_embedding: Option<Vec<f32>>,
    assembly: BudgetAllocation,
    created_at_ms: u64,
    hit_count: u64,
    estimated_size_bytes: usize,
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
pub(crate) struct IntentAssemblyCache {
    entries: RwLock<Vec<CachedAssembly>>,
    max_entries: usize,
    max_memory_bytes: usize,
    current_memory_bytes: AtomicUsize,
    similarity_threshold: f32,
    ttl_ms: u64,
    total_lookups: AtomicU64,
}

impl IntentAssemblyCache {
    pub(crate) fn new(
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
            total_lookups: AtomicU64::new(0),
        }
    }

    pub(crate) fn lookup(&self, intent: &str, embedding: &Option<Vec<f32>>) -> Option<BudgetAllocation> {
        self.total_lookups.fetch_add(1, Ordering::Relaxed);

        let mut entries = self.entries.write().unwrap();

        if let Some(pos) = entries.iter().position(|e| e.intent_text == intent) {
            let entry = &mut entries[pos];

            if entry.is_expired(self.ttl_ms) {
                let removed = entries.remove(pos);
                self.current_memory_bytes.fetch_sub(removed.estimated_size_bytes, Ordering::Relaxed);
                return None;
            }

            entry.hit_count += 1;
            tracing::debug!("intent cache hit (exact match) for: {}", intent);
            return Some(entry.assembly.clone());
        }

        if let Some(intent_emb) = embedding {
            let best_pos = self.find_best_similarity(intent_emb, &entries);

            if let Some(pos) = best_pos {
                let entry = &mut entries[pos];

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

    pub(crate) fn cosine_similarity(&self, a: &[f32], b: &[f32]) -> f32 {
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

    pub(crate) fn store(
        &self,
        intent_text: String,
        intent_embedding: Option<Vec<f32>>,
        assembly: BudgetAllocation,
        dependency_cids: Vec<String>,
    ) {
        let entry = CachedAssembly::new(intent_text, intent_embedding, assembly, dependency_cids);
        let entry_size = entry.estimated_size_bytes;

        let mut entries = self.entries.write().unwrap();

        self.evict_if_needed_locked(entry_size, &mut entries);

        entries.push(entry);
        self.current_memory_bytes.fetch_add(entry_size, Ordering::Relaxed);
    }

    fn evict_if_needed_locked(&self, new_entry_size: usize, entries: &mut Vec<CachedAssembly>) {
        entries.retain(|e| !e.is_expired(self.ttl_ms));

        while entries.len() >= self.max_entries
            || self.current_memory_bytes.load(Ordering::Relaxed) + new_entry_size > self.max_memory_bytes
            || self.over_budget_memory(new_entry_size)
        {
            if entries.is_empty() {
                break;
            }

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

    pub(crate) fn invalidate_by_cids(&self, modified_cids: &[String]) {
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

    pub(crate) fn clear(&self) {
        let mut entries = self.entries.write().unwrap();
        entries.clear();
        self.current_memory_bytes.store(0, Ordering::Relaxed);
    }

    pub(crate) fn stats(&self) -> IntentCacheStats {
        let entries = self.entries.read().unwrap();
        IntentCacheStats {
            entries: entries.len(),
            memory_bytes: self.current_memory_bytes.load(Ordering::Relaxed),
            hits: entries.iter().map(|e| e.hit_count).sum(),
            total_lookups: self.total_lookups.load(Ordering::Relaxed),
        }
    }

    /// Persist the cache entries to a JSON file at `dir/prefetch_cache.json`.
    /// Returns Ok(()) on success, Error on failure.
    pub(crate) fn persist_to_dir(&self, dir: &Path) -> std::io::Result<()> {
        let entries = self.entries.read().unwrap();
        let persist_entries: Vec<_> = entries.iter().cloned().collect();
        let json = serde_json::to_string_pretty(&persist_entries)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let path = dir.join(CACHE_PERSIST_FILE);
        std::fs::write(path, json)
    }

    /// Restore the cache entries from `dir/prefetch_cache.json`.
    /// Expired entries are filtered out. Missing file is not an error.
    /// Returns the number of entries restored.
    pub(crate) fn restore_from_dir(&self, dir: &Path) -> std::io::Result<usize> {
        let path = dir.join(CACHE_PERSIST_FILE);
        if !path.exists() {
            return Ok(0);
        }
        let json = std::fs::read_to_string(&path)?;
        let entries: Vec<CachedAssembly> = serde_json::from_str(&json)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        let mut restored = 0;
        let mut total_mem = 0usize;
        let mut final_entries = Vec::new();

        for entry in entries {
            if entry.is_expired(self.ttl_ms) {
                continue;
            }
            total_mem += entry.estimated_size_bytes;
            final_entries.push(entry);
            restored += 1;
        }

        let mut entries_guard = self.entries.write().unwrap();
        *entries_guard = final_entries;
        self.current_memory_bytes.store(total_mem, Ordering::Relaxed);
        Ok(restored)
    }

    /// Warm the cache from AgentProfile's predicted intents.
    ///
    /// Uses exact-match predictions from the profile (not embedding similarity)
    /// to avoid dependency on a live embedding provider at session-start time.
    ///
    /// Returns the number of entries warmed.
    pub fn warm_from_profile(
        &self,
        profile: &crate::kernel::ops::session::AgentProfile,
        assembler: &dyn Fn(&str) -> Option<crate::fs::context_budget::BudgetAllocation>,
    ) -> usize {
        // Predict top-3 next intents from the profile
        let mut warmed = 0;

        // Collect predicted intents from transition matrix
        let predictions: Vec<(String, f32)> = profile
            .intent_transitions
            .iter()
            .flat_map(|(from_intent, succs)| {
                succs.iter().map(move |(to_intent, count)| {
                    let confidence = *count as f32;
                    (format!("{} → {}", from_intent, to_intent), confidence)
                })
            })
            .collect();

        // Sort by confidence descending and take top-3
        let mut predictions = predictions;
        predictions.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        for (intent_text, confidence) in predictions.into_iter().take(3) {
            // Skip if confidence too low
            if confidence < 0.3 {
                continue;
            }

            // Skip if already in cache (exact match)
            if self.lookup(&intent_text, &None).is_some() {
                continue;
            }

            // Try to assemble context for this predicted intent
            if let Some(assembly) = assembler(&intent_text) {
                self.store(intent_text, None, assembly, vec![]);
                warmed += 1;
            }
        }

        // Also warm from hot_objects: top CIDs that the agent frequently accesses
        for (cid, count) in profile.hot_objects.iter().take(5) {
            if *count < 2 {
                continue;
            }
            // Create a synthetic intent from the hot CID for cache warming
            let synthetic_intent = format!("[hot:{}]", cid);
            if self.lookup(&synthetic_intent, &None).is_none() {
                let assembly = crate::fs::context_budget::BudgetAllocation {
                    items: vec![crate::fs::context_loader::LoadedContext {
                        cid: cid.clone(),
                        content: format!("hot object: {}", cid),
                        layer: crate::fs::context_loader::ContextLayer::L1,
                        tokens_estimate: 50,
                        actual_layer: None,
                        degraded: false,
                        degradation_reason: None,
                    }],
                    total_tokens: 50,
                    budget: 100,
                    candidates_considered: 1,
                    candidates_included: 1,
                };
                self.store(synthetic_intent, None, assembly, vec![cid.clone()]);
                warmed += 1;
            }
        }

        warmed
    }
}

impl Default for IntentAssemblyCache {
    fn default() -> Self {
        Self::new(
            DEFAULT_MAX_CACHE_ENTRIES,
            DEFAULT_MAX_CACHE_MEMORY_BYTES,
            DEFAULT_SIMILARITY_THRESHOLD,
            DEFAULT_CACHE_TTL_MS,
        )
    }
}

/// Statistics for the intent cache.
#[derive(Debug, Clone, Default)]
pub struct IntentCacheStats {
    pub entries: usize,
    pub memory_bytes: usize,
    pub hits: u64,
    /// Total number of cache lookups (for calculating hit rate).
    pub total_lookups: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::context_budget::BudgetAllocation;
    use crate::fs::context_loader::{LoadedContext, ContextLayer};

    fn make_allocation(cid: &str) -> BudgetAllocation {
        BudgetAllocation {
            items: vec![LoadedContext {
                cid: cid.to_string(),
                content: format!("content for {}", cid),
                layer: ContextLayer::L0,
                tokens_estimate: 10,
                actual_layer: None,
                degraded: false,
                degradation_reason: None,
            }],
            total_tokens: 10,
            budget: 100,
            candidates_considered: 1,
            candidates_included: 1,
        }
    }

    #[test]
    fn store_and_exact_lookup() {
        let cache = IntentAssemblyCache::default();
        cache.store("fix auth".into(), None, make_allocation("c1"), vec![]);

        let result = cache.lookup("fix auth", &None);
        assert!(result.is_some());
        assert_eq!(result.unwrap().items[0].cid, "c1");
    }

    #[test]
    fn lookup_miss_returns_none() {
        let cache = IntentAssemblyCache::default();
        cache.store("fix auth".into(), None, make_allocation("c1"), vec![]);
        assert!(cache.lookup("unknown intent", &None).is_none());
    }

    #[test]
    fn stats_track_lookups_and_hits() {
        let cache = IntentAssemblyCache::default();
        cache.store("a".into(), None, make_allocation("c1"), vec![]);

        cache.lookup("a", &None);
        cache.lookup("a", &None);
        cache.lookup("miss", &None);

        let stats = cache.stats();
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.total_lookups, 3);
        assert_eq!(stats.hits, 2);
    }

    #[test]
    fn clear_empties_cache() {
        let cache = IntentAssemblyCache::default();
        cache.store("a".into(), None, make_allocation("c1"), vec![]);
        cache.store("b".into(), None, make_allocation("c2"), vec![]);
        assert_eq!(cache.stats().entries, 2);

        cache.clear();
        assert_eq!(cache.stats().entries, 0);
        assert_eq!(cache.stats().memory_bytes, 0);
    }

    #[test]
    fn invalidate_by_cids_removes_dependent_entries() {
        let cache = IntentAssemblyCache::default();
        cache.store("a".into(), None, make_allocation("c1"), vec!["dep1".into()]);
        cache.store("b".into(), None, make_allocation("c2"), vec!["dep2".into()]);
        cache.store("c".into(), None, make_allocation("c3"), vec!["dep1".into(), "dep3".into()]);

        assert_eq!(cache.stats().entries, 3);

        cache.invalidate_by_cids(&["dep1".into()]);
        assert_eq!(cache.stats().entries, 1);
        assert!(cache.lookup("b", &None).is_some());
        assert!(cache.lookup("a", &None).is_none());
        assert!(cache.lookup("c", &None).is_none());
    }

    #[test]
    fn invalidate_empty_cids_is_noop() {
        let cache = IntentAssemblyCache::default();
        cache.store("a".into(), None, make_allocation("c1"), vec!["dep1".into()]);
        cache.invalidate_by_cids(&[]);
        assert_eq!(cache.stats().entries, 1);
    }

    #[test]
    fn eviction_by_max_entries() {
        let cache = IntentAssemblyCache::new(2, 1024 * 1024, 0.85, u64::MAX);
        cache.store("a".into(), None, make_allocation("c1"), vec![]);
        cache.store("b".into(), None, make_allocation("c2"), vec![]);
        assert_eq!(cache.stats().entries, 2);

        cache.store("c".into(), None, make_allocation("c3"), vec![]);
        assert_eq!(cache.stats().entries, 2);
    }

    #[test]
    fn cosine_similarity_identical() {
        let cache = IntentAssemblyCache::default();
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cache.cosine_similarity(&a, &b) - 1.0).abs() < 0.001);
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let cache = IntentAssemblyCache::default();
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!(cache.cosine_similarity(&a, &b).abs() < 0.001);
    }

    #[test]
    fn cosine_similarity_mismatched_length() {
        let cache = IntentAssemblyCache::default();
        assert_eq!(cache.cosine_similarity(&[1.0], &[1.0, 2.0]), 0.0);
    }

    #[test]
    fn cosine_similarity_zero_vector() {
        let cache = IntentAssemblyCache::default();
        assert_eq!(cache.cosine_similarity(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn embedding_similarity_lookup() {
        let cache = IntentAssemblyCache::new(64, 32 * 1024 * 1024, 0.85, u64::MAX);
        let emb = vec![1.0, 0.0, 0.0];
        cache.store("a".into(), Some(emb.clone()), make_allocation("c1"), vec![]);

        let similar_emb = vec![0.99, 0.1, 0.0];
        let result = cache.lookup("different text", &Some(similar_emb));
        assert!(result.is_some());
    }

    #[test]
    fn embedding_below_threshold_returns_none() {
        let cache = IntentAssemblyCache::new(64, 32 * 1024 * 1024, 0.95, u64::MAX);
        let emb = vec![1.0, 0.0, 0.0];
        cache.store("a".into(), Some(emb), make_allocation("c1"), vec![]);

        let dissimilar = vec![0.0, 1.0, 0.0];
        let result = cache.lookup("other", &Some(dissimilar));
        assert!(result.is_none());
    }

    #[test]
    fn default_cache_has_expected_config() {
        let cache = IntentAssemblyCache::default();
        assert_eq!(cache.max_entries, 64);
        assert_eq!(cache.max_memory_bytes, 32 * 1024 * 1024);
        assert!((cache.similarity_threshold - 0.85).abs() < 0.001);
    }

    // F-1: Prefetch Persistence tests
    #[test]
    fn persist_and_restore() {
        let dir = tempfile::tempdir().unwrap();
        let cache = IntentAssemblyCache::default();
        cache.store("fix auth".into(), None, make_allocation("c1"), vec![]);
        cache.store("fix auth v2".into(), None, make_allocation("c2"), vec![]);

        cache.persist_to_dir(dir.path()).unwrap();

        let restored = IntentAssemblyCache::default();
        let count = restored.restore_from_dir(dir.path()).unwrap();
        assert_eq!(count, 2);
        assert!(restored.lookup("fix auth", &None).is_some());
        assert!(restored.lookup("fix auth v2", &None).is_some());
        // Hit count should be preserved
        let stats = restored.stats();
        assert_eq!(stats.entries, 2);
    }

    #[test]
    fn restore_missing_file_is_zero() {
        let dir = tempfile::tempdir().unwrap();
        let cache = IntentAssemblyCache::default();
        let count = cache.restore_from_dir(dir.path()).unwrap();
        assert_eq!(count, 0);
    }

    // F-3: CacheWarmPipeline tests
    #[test]
    fn test_warm_from_profile_populates_cache() {
        use crate::kernel::ops::session::AgentProfile;
        let cache = IntentAssemblyCache::default();
        let mut profile = AgentProfile::new("agent-1".to_string());
        profile.record_intent("fix auth", Some("test auth"));
        profile.record_cid_usage("cid1");

        let assembler = |intent: &str| {
            if intent.contains("fix auth") {
                Some(make_allocation("cid_from_assembler"))
            } else {
                None
            }
        };

        let warmed = cache.warm_from_profile(&profile, &assembler);
        assert!(warmed > 0, "Expected at least one entry warmed");
    }

    #[test]
    fn test_warm_skips_low_confidence() {
        use crate::kernel::ops::session::AgentProfile;
        let cache = IntentAssemblyCache::default();
        let mut profile = AgentProfile::new("agent-1".to_string());
        // Record transition but with count=1 (low confidence)
        profile.record_intent("fix auth", Some("test auth"));

        let assembler = |_intent: &str| Some(make_allocation("cid1"));

        let warmed = cache.warm_from_profile(&profile, &assembler);
        // Confidence from 1 count is 1.0, should pass 0.3 threshold
        assert!(warmed <= 3, "At most top-3 predictions");
    }

    #[test]
    fn test_warm_skips_existing_entries() {
        use crate::kernel::ops::session::AgentProfile;
        let cache = IntentAssemblyCache::default();

        // Pre-populate cache
        cache.store("fix auth → test auth".into(), None, make_allocation("cid1"), vec![]);

        let mut profile = AgentProfile::new("agent-1".to_string());
        profile.record_intent("fix auth", Some("test auth"));

        let assembler = |_intent: &str| Some(make_allocation("cid2"));

        let warmed = cache.warm_from_profile(&profile, &assembler);
        assert_eq!(warmed, 0, "Should skip already-cached entries");
    }

    #[test]
    fn test_warm_returns_count() {
        use crate::kernel::ops::session::AgentProfile;
        let cache = IntentAssemblyCache::default();
        let mut profile = AgentProfile::new("agent-1".to_string());
        profile.record_intent("fix auth", Some("test auth"));
        profile.record_intent("fix auth", Some("deploy"));
        profile.record_cid_usage("cid1");

        let assembler = |intent: &str| {
            if intent.contains("fix") {
                Some(make_allocation("cid_assembled"))
            } else {
                None
            }
        };

        let warmed = cache.warm_from_profile(&profile, &assembler);
        // Confidence from 1 count is 1.0, should pass 0.3 threshold
        assert!(warmed <= 3);
    }

    #[test]
    fn test_warm_with_empty_profile() {
        use crate::kernel::ops::session::AgentProfile;
        let cache = IntentAssemblyCache::default();
        let profile = AgentProfile::new("agent-1".to_string());

        let assembler = |_intent: &str| Some(make_allocation("cid1"));

        let warmed = cache.warm_from_profile(&profile, &assembler);
        assert_eq!(warmed, 0, "Empty profile should warm nothing");
    }
}
