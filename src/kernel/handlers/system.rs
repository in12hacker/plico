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
