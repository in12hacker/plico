//! System status operations — runtime kernel metrics.

use crate::api::semantic::{
    SystemStatus, CacheStatsDto, ClusterStatusDto, NodeInfoDto,
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
