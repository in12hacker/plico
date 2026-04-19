//! System status operations — runtime kernel metrics.

use crate::api::semantic::{SystemStatus, CacheStatsDto};

impl crate::kernel::AIKernel {
    /// Build runtime kernel metrics from live system state.
    pub fn system_status(&self) -> SystemStatus {
        let kg_node_count = self.knowledge_graph.as_ref()
            .map(|kg| kg.node_count().unwrap_or(0))
            .unwrap_or(0);
        let kg_edge_count = self.knowledge_graph.as_ref()
            .map(|kg| kg.edge_count().unwrap_or(0))
            .unwrap_or(0);

        let cas_object_count = self.cas.list_cids()
            .map(|c| c.len())
            .unwrap_or(0);

        // Get cache statistics (v19.0)
        let cache_stats = self.cache_stats();

        SystemStatus {
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            cas_object_count,
            agent_count: self.scheduler.list_agents().len(),
            tag_count: self.fs.list_tags().len(),
            kg_node_count,
            kg_edge_count,
            cache_stats: Some(cache_stats),
        }
    }

    /// Get combined cache statistics (v19.0).
    pub fn cache_stats(&self) -> CacheStatsDto {
        let (emb_stats, kg_stats, search_stats) = self.edge_cache.stats();
        CacheStatsDto {
            embedding_cache_entries: emb_stats.current_entries,
            kg_cache_entries: kg_stats.current_entries,
            search_cache_entries: search_stats.current_entries,
            embedding_hit_rate: emb_stats.hit_rate(),
            kg_hit_rate: kg_stats.hit_rate(),
            search_hit_rate: search_stats.hit_rate(),
        }
    }

    /// Invalidate all caches (v19.0).
    pub fn cache_invalidate_all(&self) {
        self.edge_cache.invalidate_all();
    }
}
