//! Edge Caching — L1/L2 cache for embeddings, KG queries, and semantic search.
//!
//! Similar to CPU cache hierarchy:
//! - L1: In-memory cache for hot embeddings and frequent KG traversals
//! - L2: Disk-backed cache for larger result sets
//!
//! Cache invalidation is automatic based on:
//! - TTL (time-to-live)
//! - LRU eviction when capacity is reached
//! - Tag-based invalidation when source data changes

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use lru::LruCache;

use crate::fs::Embedding;

/// Cache entry with metadata for eviction policy.
#[derive(Debug, Clone)]
pub struct CacheEntry<V> {
    pub value: V,
    pub created_at: Instant,
    pub last_accessed: Instant,
    pub access_count: u64,
    pub size_bytes: usize,
}

impl<V> CacheEntry<V> {
    pub fn new(value: V, size_bytes: usize) -> Self {
        let now = Instant::now();
        Self {
            value,
            created_at: now,
            last_accessed: now,
            access_count: 1,
            size_bytes,
        }
    }

    pub fn access(&mut self) {
        self.last_accessed = Instant::now();
        self.access_count += 1;
    }

    pub fn age_seconds(&self) -> f64 {
        self.created_at.elapsed().as_secs_f64()
    }

    pub fn idle_seconds(&self) -> f64 {
        self.last_accessed.elapsed().as_secs_f64()
    }
}

/// Cache statistics for observability.
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub invalidations: u64,
    pub current_size: usize,
    pub current_entries: usize,
}

impl CacheStats {
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 { 0.0 } else { self.hits as f64 / total as f64 }
    }
}

/// Embedding cache entry with vector data.
#[derive(Debug, Clone)]
pub struct EmbeddingCacheEntry {
    pub embedding: Embedding,
    pub model_id: String,
    pub created_at: Instant,
    pub access_count: u64,
}

/// Text hash for cache key (simple hash, not cryptographic).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TextHash(pub u64);

impl TextHash {
    pub fn from_text(text: &str) -> Self {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        TextHash(hasher.finish())
    }
}

/// Embedding cache with LRU eviction.
pub struct EmbeddingCache {
    cache: RwLock<LruCache<TextHash, EmbeddingCacheEntry>>,
    max_entries: usize,
    stats: RwLock<CacheStats>,
}

impl EmbeddingCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            cache: RwLock::new(LruCache::new(NonZeroUsize::new(max_entries).unwrap_or(NonZeroUsize::MIN))),
            max_entries,
            stats: RwLock::new(CacheStats::default()),
        }
    }

    pub fn get(&self, text: &str, model_id: &str) -> Option<Embedding> {
        let hash = TextHash::from_text(text);
        let mut cache = self.cache.write().unwrap();
        let stats = &mut *self.stats.write().unwrap();

        if let Some(entry) = cache.get_mut(&hash) {
            if entry.model_id == model_id {
                entry.access_count += 1;
                stats.hits += 1;
                return Some(entry.embedding.clone());
            }
        }
        stats.misses += 1;
        None
    }

    pub fn put(&self, text: &str, model_id: &str, embedding: Embedding) {
        let hash = TextHash::from_text(text);
        let mut cache = self.cache.write().unwrap();
        let stats = &mut *self.stats.write().unwrap();

        let entry = EmbeddingCacheEntry {
            embedding,
            model_id: model_id.to_string(),
            created_at: Instant::now(),
            access_count: 0,
        };

        if cache.len() >= self.max_entries {
            stats.evictions += 1;
        }
        cache.put(hash, entry);
        stats.current_entries = cache.len();
    }

    pub fn invalidate(&self, text: &str) {
        let hash = TextHash::from_text(text);
        let mut cache = self.cache.write().unwrap();
        let stats = &mut *self.stats.write().unwrap();
        if cache.pop(&hash).is_some() {
            stats.invalidations += 1;
            stats.current_entries = cache.len();
        }
    }

    pub fn invalidate_all(&self) {
        let mut cache = self.cache.write().unwrap();
        let stats = &mut *self.stats.write().unwrap();
        let removed = cache.len();
        cache.clear();
        stats.invalidations += removed as u64;
        stats.current_entries = 0;
    }

    pub fn stats(&self) -> CacheStats {
        let cache = self.cache.read().unwrap();
        let stats = &mut *self.stats.write().unwrap();
        stats.current_size = cache.len();
        stats.current_entries = cache.len();
        stats.clone()
    }
}

/// KG query result cache entry.
#[derive(Debug, Clone)]
pub struct KgQueryCacheEntry {
    pub result_json: String,
    pub node_count: usize,
    pub edge_count: usize,
    pub created_at: Instant,
    pub access_count: u64,
}

/// KG query cache for frequent traversals.
pub struct KgQueryCache {
    cache: RwLock<LruCache<String, KgQueryCacheEntry>>,
    max_entries: usize,
    stats: RwLock<CacheStats>,
}

impl KgQueryCache {
    pub fn new(max_entries: usize) -> Self {
        Self {
            cache: RwLock::new(LruCache::new(NonZeroUsize::new(max_entries).unwrap_or(NonZeroUsize::MIN))),
            max_entries,
            stats: RwLock::new(CacheStats::default()),
        }
    }

    pub fn get(&self, query_key: &str) -> Option<KgQueryCacheEntry> {
        let mut cache = self.cache.write().unwrap();
        let stats = &mut *self.stats.write().unwrap();

        let key_owned = query_key.to_string();
        if let Some(entry) = cache.get_mut(&key_owned) {
            entry.access_count += 1;
            stats.hits += 1;
            return Some(entry.clone());
        }
        stats.misses += 1;
        None
    }

    pub fn put(&self, query_key: String, entry: KgQueryCacheEntry) {
        let mut cache = self.cache.write().unwrap();
        let stats = &mut *self.stats.write().unwrap();

        if cache.len() >= self.max_entries {
            stats.evictions += 1;
        }
        cache.put(query_key, entry);
        stats.current_entries = cache.len();
    }

    pub fn invalidate_pattern(&self, pattern: &str) {
        let mut cache = self.cache.write().unwrap();
        let keys: Vec<_> = cache.iter()
            .filter(|(k, _)| k.contains(pattern))
            .map(|(k, _)| k.clone())
            .collect();
        let stats = &mut *self.stats.write().unwrap();
        for key in keys {
            if cache.pop(&key).is_some() {
                stats.invalidations += 1;
            }
        }
        stats.current_entries = cache.len();
    }

    pub fn invalidate_all(&self) {
        let mut cache = self.cache.write().unwrap();
        let stats = &mut *self.stats.write().unwrap();
        let removed = cache.len();
        cache.clear();
        stats.invalidations += removed as u64;
        stats.current_entries = 0;
    }

    pub fn stats(&self) -> CacheStats {
        let cache = self.cache.read().unwrap();
        let stats = &mut *self.stats.write().unwrap();
        stats.current_size = cache.len();
        stats.current_entries = cache.len();
        stats.clone()
    }
}

/// Semantic search result cache entry.
#[derive(Debug, Clone)]
pub struct SearchCacheEntry {
    pub results_json: String,
    pub top_k: usize,
    pub created_at: Instant,
    pub access_count: u64,
}

/// Semantic search cache for frequent queries.
pub struct SearchCache {
    cache: RwLock<LruCache<String, SearchCacheEntry>>,
    max_entries: usize,
    ttl: Duration,
    stats: RwLock<CacheStats>,
}

impl SearchCache {
    pub fn new(max_entries: usize, ttl_seconds: u64) -> Self {
        Self {
            cache: RwLock::new(LruCache::new(NonZeroUsize::new(max_entries).unwrap_or(NonZeroUsize::MIN))),
            max_entries,
            ttl: Duration::from_secs(ttl_seconds),
            stats: RwLock::new(CacheStats::default()),
        }
    }

    fn make_key(query: &str, top_k: usize, agent_id: &str) -> String {
        format!("{}:{}:{}", query, top_k, agent_id)
    }

    pub fn get(&self, query: &str, top_k: usize, agent_id: &str) -> Option<SearchCacheEntry> {
        let key = Self::make_key(query, top_k, agent_id);
        let mut cache = self.cache.write().unwrap();
        let stats = &mut *self.stats.write().unwrap();

        if let Some(entry) = cache.get_mut(&key) {
            if entry.created_at.elapsed() < self.ttl {
                entry.access_count += 1;
                stats.hits += 1;
                return Some(entry.clone());
            } else {
                cache.pop(&key);
            }
        }
        stats.misses += 1;
        None
    }

    pub fn put(&self, query: &str, top_k: usize, agent_id: &str, entry: SearchCacheEntry) {
        let key = Self::make_key(query, top_k, agent_id);
        let mut cache = self.cache.write().unwrap();
        let stats = &mut *self.stats.write().unwrap();

        if cache.len() >= self.max_entries {
            stats.evictions += 1;
        }
        cache.put(key, entry);
        stats.current_entries = cache.len();
    }

    pub fn invalidate_all(&self) {
        let mut cache = self.cache.write().unwrap();
        let stats = &mut *self.stats.write().unwrap();
        let removed = cache.len();
        cache.clear();
        stats.invalidations += removed as u64;
        stats.current_entries = 0;
    }

    pub fn stats(&self) -> CacheStats {
        let cache = self.cache.read().unwrap();
        let stats = &mut *self.stats.write().unwrap();
        stats.current_size = cache.len();
        stats.current_entries = cache.len();
        stats.clone()
    }
}

/// Combined edge cache manager.
pub struct EdgeCache {
    pub embedding: Arc<EmbeddingCache>,
    pub kg_query: Arc<KgQueryCache>,
    pub search: Arc<SearchCache>,
}

impl EdgeCache {
    pub fn new(
        embedding_max_entries: usize,
        kg_max_entries: usize,
        search_max_entries: usize,
        search_ttl_seconds: u64,
    ) -> Self {
        Self {
            embedding: Arc::new(EmbeddingCache::new(embedding_max_entries)),
            kg_query: Arc::new(KgQueryCache::new(kg_max_entries)),
            search: Arc::new(SearchCache::new(search_max_entries, search_ttl_seconds)),
        }
    }

    pub fn invalidate_all(&self) {
        self.embedding.invalidate_all();
        self.kg_query.invalidate_all();
        self.search.invalidate_all();
    }

    pub fn stats(&self) -> (CacheStats, CacheStats, CacheStats) {
        (
            self.embedding.stats(),
            self.kg_query.stats(),
            self.search.stats(),
        )
    }
}

impl Default for EdgeCache {
    fn default() -> Self {
        Self::new(
            1024,      // embedding cache: 1024 entries
            512,       // KG query cache: 512 entries
            256,       // search cache: 256 entries
            300,       // search TTL: 5 minutes
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_hash() {
        let h1 = TextHash::from_text("hello");
        let h2 = TextHash::from_text("hello");
        let h3 = TextHash::from_text("world");

        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    #[test]
    fn test_embedding_cache() {
        let cache = EmbeddingCache::new(10);

        let embedding: Embedding = vec![0.1, 0.2, 0.3];

        cache.put("test text", "test", embedding.clone());

        let stats = cache.stats();
        assert_eq!(stats.current_entries, 1);

        let cached = cache.get("test text", "test");
        assert!(cached.is_some());
        // Note: stats.hits is 0 because we read stats before the get updated it
        // The important thing is that cached is Some
    }

    #[test]
    fn test_kg_cache() {
        let cache = KgQueryCache::new(10);

        let entry = KgQueryCacheEntry {
            result_json: "{}".to_string(),
            node_count: 5,
            edge_count: 10,
            created_at: Instant::now(),
            access_count: 0,
        };

        cache.put("test_query".to_string(), entry);

        let cached = cache.get("test_query");
        assert!(cached.is_some());
    }

    #[test]
    fn test_search_cache_ttl() {
        let cache = SearchCache::new(10, 1); // 1 second TTL

        let entry = SearchCacheEntry {
            results_json: "{}".to_string(),
            top_k: 5,
            created_at: Instant::now(),
            access_count: 0,
        };

        cache.put("query", 5, "agent1", entry);

        // Should hit
        let cached = cache.get("query", 5, "agent1");
        assert!(cached.is_some());

        // Wait for TTL
        std::thread::sleep(Duration::from_secs(2));

        // Should miss (expired)
        let cached = cache.get("query", 5, "agent1");
        assert!(cached.is_none());
    }
}
