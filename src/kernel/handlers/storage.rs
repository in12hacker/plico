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

#[cfg(test)]
mod tests {
    use crate::kernel::tests::make_kernel;
    use crate::api::semantic::ApiRequest;

    #[test]
    fn test_hybrid_retrieve() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::HybridRetrieve {
            query_text: "test query".to_string(),
            seed_tags: vec![],
            graph_depth: 0,
            edge_types: vec![],
            max_results: 0,
            token_budget: None,
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        assert!(resp.ok, "HybridRetrieve should succeed: {:?}", resp.error);
        assert!(resp.hybrid_result.is_some());
    }

    #[test]
    fn test_hybrid_retrieve_with_options() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::HybridRetrieve {
            query_text: "search something".to_string(),
            seed_tags: vec!["tag1".to_string()],
            graph_depth: 3,
            edge_types: vec!["causes".to_string()],
            max_results: 5,
            token_budget: Some(2048),
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        assert!(resp.ok, "HybridRetrieve with options should succeed: {:?}", resp.error);
        assert!(resp.hybrid_result.is_some());
    }

    #[test]
    fn test_object_usage() {
        let (kernel, _dir) = make_kernel();
        // Create an object first
        let create_resp = kernel.handle_api_request(ApiRequest::Create {
            api_version: None,
            content: "test content".to_string(),
            content_encoding: Default::default(),
            tags: vec!["test".to_string()],
            agent_id: "test_agent".to_string(),
            tenant_id: None,
            agent_token: None,
            intent: None,
        });
        assert!(create_resp.ok);
        let cid = create_resp.cid.unwrap();

        let resp = kernel.handle_api_request(ApiRequest::ObjectUsage {
            cid,
            agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok, "ObjectUsage should succeed: {:?}", resp.error);
        assert!(resp.object_usage.is_some());
        let usage = resp.object_usage.unwrap();
        assert!(usage.created_at > 0);
    }

    #[test]
    fn test_object_usage_nonexistent_cid() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::ObjectUsage {
            cid: "nonexistent_hash".to_string(),
            agent_id: "test_agent".to_string(),
        });
        // Should still succeed (returns default/zeroed usage)
        assert!(resp.ok, "ObjectUsage for missing CID should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_storage_stats() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::StorageStats {
            agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok, "StorageStats should succeed: {:?}", resp.error);
        assert!(resp.storage_stats.is_some());
        let stats = resp.storage_stats.unwrap();
        assert_eq!(stats.total_objects, 0);
    }

    #[test]
    fn test_storage_stats_with_objects() {
        let (kernel, _dir) = make_kernel();
        // Create a couple of objects
        kernel.handle_api_request(ApiRequest::Create {
            api_version: None,
            content: "object one".to_string(),
            content_encoding: Default::default(),
            tags: vec![],
            agent_id: "test_agent".to_string(),
            tenant_id: None,
            agent_token: None,
            intent: None,
        });
        kernel.handle_api_request(ApiRequest::Create {
            api_version: None,
            content: "object two".to_string(),
            content_encoding: Default::default(),
            tags: vec![],
            agent_id: "test_agent".to_string(),
            tenant_id: None,
            agent_token: None,
            intent: None,
        });

        let resp = kernel.handle_api_request(ApiRequest::StorageStats {
            agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok);
        let stats = resp.storage_stats.unwrap();
        assert_eq!(stats.total_objects, 2);
        assert!(stats.total_bytes > 0);
    }

    #[test]
    fn test_evict_cold_dry_run() {
        use crate::api::permission::PermissionAction;
        let (kernel, _dir) = make_kernel();
        kernel.permission_grant("test_agent", PermissionAction::Delete, None, None);
        let resp = kernel.handle_api_request(ApiRequest::EvictCold {
            agent_id: "test_agent".to_string(),
            dry_run: true,
        });
        assert!(resp.ok, "EvictCold dry_run should succeed: {:?}", resp.error);
        assert!(resp.evict_result.is_some());
        let result = resp.evict_result.unwrap();
        assert_eq!(result.evicted_count, 0);
    }

    #[test]
    fn test_evict_cold_real() {
        use crate::api::permission::PermissionAction;
        let (kernel, _dir) = make_kernel();
        kernel.permission_grant("test_agent", PermissionAction::Delete, None, None);
        let resp = kernel.handle_api_request(ApiRequest::EvictCold {
            agent_id: "test_agent".to_string(),
            dry_run: false,
        });
        assert!(resp.ok, "EvictCold real should succeed: {:?}", resp.error);
        assert!(resp.evict_result.is_some());
    }
}
