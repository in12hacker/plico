//! Storage governance and hybrid retrieval handlers.

use crate::api::semantic::{ApiRequest, ApiResponse};
use crate::DEFAULT_TENANT;

impl super::super::AIKernel {
    pub(crate) fn handle_storage(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::HybridRetrieve { query_text, seed_tags, graph_depth, edge_types, max_results, token_budget, agent_id: _, tenant_id } => {
                let _tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                let depth = if graph_depth == 0 { 2 } else { graph_depth };
                let max = if max_results == 0 { 20 } else { max_results };
                let result = self.hybrid_retrieve(&query_text, &seed_tags, depth, &edge_types, max, token_budget);
                let mut r = ApiResponse::ok();
                r.hybrid_result = Some(result);
                r
            }
            ApiRequest::ObjectUsage { cid, agent_id: _ } => {
                let usage = self.get_object_usage(&cid);
                let mut r = ApiResponse::ok();
                r.object_usage = Some(crate::api::semantic::ObjectUsageResult {
                    created_at: usage.created_at,
                    last_accessed_at: usage.last_accessed_at,
                    access_count: usage.access_count,
                    referenced_by_kg: self.is_cid_referenced(&cid),
                    referenced_by_memory: self.memory.is_cid_referenced(&cid),
                });
                r
            }
            ApiRequest::StorageStats { agent_id: _ } => {
                let stats = self.get_storage_stats();
                let memory_stats = self.memory.get_stats();
                let mut r = ApiResponse::ok();
                r.storage_stats = Some(crate::api::semantic::StorageStatsResult {
                    total_objects: stats.total_objects,
                    total_bytes: stats.total_bytes,
                    by_tier: crate::api::semantic::TierStats {
                        ephemeral_count: memory_stats.ephemeral_entries,
                        ephemeral_bytes: 0,
                        working_count: memory_stats.working_entries,
                        working_bytes: 0,
                        longterm_count: memory_stats.longterm_entries,
                        longterm_bytes: 0,
                    },
                    cold_objects: stats.cold_objects,
                    about_to_expire: stats.about_to_expire,
                });
                r
            }
            ApiRequest::EvictCold { agent_id: _, dry_run } => {
                let result = self.evict_cold(dry_run);
                let mut r = ApiResponse::ok();
                r.evict_result = Some(crate::api::semantic::EvictColdResult {
                    evicted_count: result.evicted_count,
                    evicted_bytes: result.evicted_bytes,
                    remaining_cold: result.remaining_cold,
                });
                r
            }
            _ => unreachable!("non-storage request routed to handle_storage"),
        }
    }
}
