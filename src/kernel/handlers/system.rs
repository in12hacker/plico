//! System status, cache, cluster, health, and cost handlers.

use crate::api::semantic::{ApiRequest, ApiResponse};
use super::super::ops;

impl super::super::AIKernel {
    pub(crate) fn handle_system(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::SystemStatus => {
                let status = self.system_status();
                let mut r = ApiResponse::ok();
                r.system_status = Some(status);
                r
            }
            ApiRequest::CacheStats => {
                let stats = self.cache_stats();
                let mut r = ApiResponse::ok();
                r.cache_stats = Some(stats);
                r
            }
            ApiRequest::CacheInvalidate => {
                self.cache_invalidate_all();
                ApiResponse::ok()
            }
            ApiRequest::IntentCacheStats => {
                let stats = self.intent_cache_stats();
                let mut r = ApiResponse::ok();
                r.intent_cache_stats = Some(stats);
                r
            }
            ApiRequest::ClusterStatus => {
                let status = self.cluster_status();
                let mut r = ApiResponse::ok();
                r.cluster_status = Some(status);
                r
            }
            ApiRequest::ClusterJoin { host, port } => {
                self.cluster_join(&host, port);
                ApiResponse::ok()
            }
            ApiRequest::ClusterLeave => {
                self.cluster_leave();
                ApiResponse::ok()
            }
            ApiRequest::NodePing { target_host, target_port } => {
                match self.node_ping(&target_host, target_port) {
                    Ok(latency_ms) => {
                        let mut r = ApiResponse::ok();
                        r.data = Some(format!("pong in {}ms", latency_ms));
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::QueryTokenUsage { agent_id: _, session_id: _ } => {
                let mut r = ApiResponse::ok();
                r.data = Some(serde_json::json!({
                    "message": "Token usage is estimated via the token_estimate field in every API response. \
                                Per-session tracking available via GrowthReport (Node 4)."
                }).to_string());
                r
            }
            ApiRequest::HealthReport => {
                let report = self.health_report();
                let mut r = ApiResponse::ok();
                r.health_report = Some(report);
                r
            }
            ApiRequest::CostSessionSummary { session_id } => {
                if let Some(summary) = self.cost_ledger.session_summary(&session_id) {
                    let mut r = ApiResponse::ok();
                    r.cost_session_summary = Some(crate::api::semantic::SessionCostSummary {
                        session_id: summary.session_id, agent_id: summary.agent_id,
                        total_input_tokens: summary.total_input_tokens,
                        total_output_tokens: summary.total_output_tokens,
                        total_cost_millicents: summary.total_cost_millicents,
                        operations_count: summary.operations_count,
                        cache_hits: summary.cache_hits, cache_misses: summary.cache_misses,
                    });
                    r
                } else {
                    ApiResponse::error(format!("No cost data for session {}", session_id))
                }
            }
            ApiRequest::CostAgentTrend { agent_id, last_n_sessions } => {
                let trend = self.cost_ledger.agent_trend(&agent_id, last_n_sessions);
                let mut r = ApiResponse::ok();
                r.cost_agent_trend = Some(trend.into_iter().map(|s| crate::api::semantic::SessionCostSummary {
                    session_id: s.session_id, agent_id: s.agent_id,
                    total_input_tokens: s.total_input_tokens,
                    total_output_tokens: s.total_output_tokens,
                    total_cost_millicents: s.total_cost_millicents,
                    operations_count: s.operations_count,
                    cache_hits: s.cache_hits, cache_misses: s.cache_misses,
                }).collect());
                r
            }
            ApiRequest::CostAnomalyCheck { agent_id } => {
                if let Some(anomaly) = self.cost_ledger.cost_anomaly_check(&agent_id) {
                    let mut r = ApiResponse::ok();
                    r.cost_anomaly = Some(crate::api::semantic::CostAnomalyResult {
                        agent_id: anomaly.agent_id, severity: anomaly.severity,
                        message: anomaly.message,
                        avg_cost_per_session_before: anomaly.avg_cost_per_session_before,
                        avg_cost_per_session_after: anomaly.avg_cost_per_session_after,
                    });
                    r
                } else {
                    ApiResponse::ok()
                }
            }
            ApiRequest::QueryGrowthReport { agent_id, period } => {
                let report = ops::observability::handle_query_growth_report(
                    &agent_id, period, &self.session_store, &self.event_bus,
                    &self.prefetch, self.knowledge_graph.as_deref(),
                );
                let mut r = ApiResponse::ok();
                r.growth_report = Some(report);
                r
            }
            _ => unreachable!("non-system request routed to handle_system"),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::kernel::tests::make_kernel;
    use crate::api::semantic::ApiRequest;

    #[test]
    fn test_system_status() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::SystemStatus);
        assert!(resp.ok, "SystemStatus should succeed");
        assert!(resp.system_status.is_some());
    }

    #[test]
    fn test_cache_stats() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::CacheStats);
        assert!(resp.ok, "CacheStats should succeed");
        assert!(resp.cache_stats.is_some());
    }

    #[test]
    fn test_cache_invalidate() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::CacheInvalidate);
        assert!(resp.ok, "CacheInvalidate should succeed");
    }

    #[test]
    fn test_intent_cache_stats() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::IntentCacheStats);
        assert!(resp.ok, "IntentCacheStats should succeed");
        assert!(resp.intent_cache_stats.is_some());
    }

    #[test]
    fn test_cluster_status() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::ClusterStatus);
        assert!(resp.ok, "ClusterStatus should succeed");
        assert!(resp.cluster_status.is_some());
    }

    #[test]
    fn test_cluster_join_and_leave() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::ClusterJoin {
            host: "127.0.0.1".to_string(),
            port: 9999,
        });
        assert!(resp.ok, "ClusterJoin should succeed");

        let resp = kernel.handle_api_request(ApiRequest::ClusterLeave);
        assert!(resp.ok, "ClusterLeave should succeed");
    }

    #[test]
    fn test_query_token_usage() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::QueryTokenUsage {
            agent_id: "test_agent".to_string(),
            session_id: None,
        });
        assert!(resp.ok, "QueryTokenUsage should succeed");
        assert!(resp.data.is_some());
    }

    #[test]
    fn test_health_report() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::HealthReport);
        assert!(resp.ok, "HealthReport should succeed");
        assert!(resp.health_report.is_some());
    }

    #[test]
    fn test_cost_session_summary_no_data() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::CostSessionSummary {
            session_id: "nonexistent_session".to_string(),
        });
        assert!(!resp.ok, "CostSessionSummary with no data should fail");
        assert!(resp.error.unwrap().contains("No cost data"));
    }

    #[test]
    fn test_cost_agent_trend_empty() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::CostAgentTrend {
            agent_id: "test_agent".to_string(),
            last_n_sessions: 10,
        });
        assert!(resp.ok, "CostAgentTrend should succeed");
        assert!(resp.cost_agent_trend.is_some());
        assert!(resp.cost_agent_trend.unwrap().is_empty());
    }

    #[test]
    fn test_cost_anomaly_check_no_data() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::CostAnomalyCheck {
            agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok, "CostAnomalyCheck should succeed");
        // No anomaly with no history
        assert!(resp.cost_anomaly.is_none());
    }

    #[test]
    fn test_query_growth_report() {
        let (kernel, _dir) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::QueryGrowthReport {
            agent_id: "test_agent".to_string(),
            period: crate::api::dto::GrowthPeriod::Last7Days,
        });
        assert!(resp.ok, "QueryGrowthReport should succeed");
        assert!(resp.growth_report.is_some());
    }
}
