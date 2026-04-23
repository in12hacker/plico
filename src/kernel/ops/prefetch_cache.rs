//! Intent Assembly Cache (F-9) — dual-path matching with cosine similarity + exact string.
//!
//! Extracted from `prefetch.rs` for independent evolution.
//! Cache management changes independently from the core prefetch engine.

use std::sync::{RwLock, atomic::{AtomicUsize, AtomicU64, Ordering}};

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

/// A cached intent assembly result.
#[derive(Debug, Clone)]
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
}
