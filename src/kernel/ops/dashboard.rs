//! System status operations — runtime kernel metrics.

use crate::api::semantic::{
    SystemStatus, CacheStatsDto, ClusterStatusDto, NodeInfoDto, IntentCacheStatsDto,
    HealthIndicators,
};

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

        // Get health indicators (F-19)
        let health = self.health_indicators();

        SystemStatus {
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            cas_object_count,
            agent_count: self.scheduler.list_agents().len(),
            tag_count: self.fs.list_tags().len(),
            kg_node_count,
            kg_edge_count,
            cache_stats: Some(cache_stats),
            health: Some(health),
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

    /// Get intent cache statistics (F-9).
    pub fn intent_cache_stats(&self) -> IntentCacheStatsDto {
        let stats = self.prefetch.intent_cache_stats();
        IntentCacheStatsDto {
            entries: stats.entries,
            memory_bytes: stats.memory_bytes,
            hits: stats.hits,
        }
    }

    /// Compute health indicators across all subsystems (F-19).
    ///
    /// Health is determined by:
    /// - Memory: usage below 90%
    /// - Cache: average hit rate above 30%
    /// - EventBus: queue depth below 1000 events
    /// - Scheduler: active agents below 100
    pub fn health_indicators(&self) -> HealthIndicators {
        // Memory health — use sysinfo if available, otherwise estimate at 0
        let (memory_used_bytes, memory_total_bytes) = get_memory_usage();
        let memory_usage_percent = if memory_total_bytes > 0 {
            (memory_used_bytes as f64 / memory_total_bytes as f64) * 100.0
        } else {
            0.0
        };
        let memory_healthy = memory_usage_percent < 90.0;

        // Cache health — average hit rate across all tiers
        let cache_stats = self.cache_stats();
        let avg_hit_rate = (cache_stats.embedding_hit_rate
            + cache_stats.kg_hit_rate
            + cache_stats.search_hit_rate)
            / 3.0;
        let cache_hit_rate_percent = avg_hit_rate * 100.0;
        let cache_healthy = avg_hit_rate > 0.3;

        // EventBus health
        let eventbus_queue_depth = self.event_bus.event_count();
        let eventbus_subscriber_count = self.event_bus.subscription_count();
        let eventbus_healthy = eventbus_queue_depth < 1000;

        // Scheduler health
        let scheduler_active_agents = self.scheduler.list_agents().len();
        let scheduler_pending_intents = self.scheduler.pending_intent_count();
        let scheduler_healthy = scheduler_active_agents < 100;

        // Overall health
        let overall_healthy =
            memory_healthy && cache_healthy && eventbus_healthy && scheduler_healthy;

        // Health score: 1.0 if all healthy, 0.5 if only critical subsystems unhealthy, 0.0 otherwise
        let health_score = if overall_healthy {
            1.0
        } else if memory_healthy && scheduler_healthy {
            // Core systems healthy, secondary systems have issues
            0.7
        } else if memory_healthy || scheduler_healthy {
            // One core system healthy
            0.4
        } else {
            0.0
        };

        HealthIndicators {
            memory_healthy,
            memory_usage_percent,
            memory_total_bytes,
            memory_used_bytes,
            cache_healthy,
            cache_hit_rate_percent,
            eventbus_healthy,
            eventbus_queue_depth,
            eventbus_subscriber_count,
            scheduler_healthy,
            scheduler_active_agents,
            scheduler_pending_intents,
            overall_healthy,
            health_score,
        }
    }

    /// Get cluster status (v20.0).
    pub fn cluster_status(&self) -> ClusterStatusDto {
        let stats = self.cluster.cluster_stats();
        let membership = self.cluster.membership();

        let known_nodes: Vec<NodeInfoDto> = membership.known_nodes
            .values()
            .map(|n| NodeInfoDto {
                node_id: n.node_id.0.clone(),
                host: n.host.clone(),
                port: n.port,
                is_seed: n.is_seed,
                last_heartbeat_ms: n.last_heartbeat_ms,
                is_stale: n.is_stale(15000), // 15 second threshold
            })
            .collect();

        ClusterStatusDto {
            cluster_name: stats.cluster_name,
            total_nodes: stats.total_nodes,
            local_node_id: stats.local_node_id.0,
            is_seed: stats.is_seed,
            version: stats.version,
            pending_migrations: stats.pending_migrations,
            known_nodes,
        }
    }

    /// Join a cluster by connecting to a seed node (v20.0).
    pub fn cluster_join(&self, seed_host: &str, seed_port: u16) {
        self.cluster.add_seed_node(seed_host.to_string(), seed_port);
    }

    /// Leave the current cluster (v20.0).
    pub fn cluster_leave(&self) {
        // Clear all non-local nodes from membership
        let membership = self.cluster.membership();
        for node in membership.other_nodes() {
            self.cluster.membership().remove_node(&node.node_id);
        }
    }

    /// Ping a remote node and return latency in ms (v20.0).
    pub fn node_ping(&self, target_host: &str, target_port: u16) -> Result<u64, String> {
        use std::time::Instant;
        use std::net::TcpStream;
        use std::io::{Read, Write};

        let addr = format!("{}:{}", target_host, target_port);
        let start = Instant::now();

        // Try to connect (simplified - real impl would use proper protocol)
        match TcpStream::connect_timeout(
            &addr.parse().map_err(|e: std::net::AddrParseError| e.to_string())?,
            std::time::Duration::from_secs(5),
        ) {
            Ok(mut stream) => {
                // Send a simple ping
                let ping = b"PING\r\n";
                if stream.write_all(ping).is_ok() {
                    let mut buf = [0u8; 64];
                    let _ = stream.read(&mut buf);
                }
                Ok(start.elapsed().as_millis() as u64)
            }
            Err(e) => Err(format!("Failed to ping {}: {}", addr, e)),
        }
    }
}

/// Get system memory usage (used_bytes, total_bytes).
/// Returns (0, 0) if memory info is unavailable.
/// TODO: integrate sysinfo crate for accurate memory metrics.
fn get_memory_usage() -> (u64, u64) {
    // Stub implementation — returns 0,0 until sysinfo is integrated.
    // On Linux, this could read /proc/meminfo as a lightweight alternative.
    #[cfg(target_os = "linux")]
    {
        use std::fs::read_to_string;
        if let Ok(meminfo) = read_to_string("/proc/meminfo") {
            let mut total: u64 = 0;
            let mut available: u64 = 0;
            for line in meminfo.lines() {
                if line.starts_with("MemTotal:") {
                    if let Some(val) = line.split_whitespace().nth(1) {
                        total = val.parse::<u64>().unwrap_or(0) * 1024; // KB to bytes
                    }
                } else if line.starts_with("MemAvailable:") {
                    if let Some(val) = line.split_whitespace().nth(1) {
                        available = val.parse::<u64>().unwrap_or(0) * 1024; // KB to bytes
                    }
                }
            }
            if total > 0 {
                return (total - available, total);
            }
        }
    }
    (0, 0)
}
