//! Intent submission and context assembly handlers.

use crate::api::semantic::{ApiRequest, ApiResponse};
use crate::scheduler::{AgentId, IntentPriority};
use crate::DEFAULT_TENANT;

impl super::super::AIKernel {
    pub(crate) fn handle_intent(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::SubmitIntent { description, priority, action, agent_id } => {
                let p = match priority.to_lowercase().as_str() {
                    "critical" => IntentPriority::Critical,
                    "high" => IntentPriority::High,
                    "medium" => IntentPriority::Medium,
                    _ => IntentPriority::Low,
                };
                let id = match self.submit_intent(p, description, action, Some(agent_id)) {
                    Ok(id) => id,
                    Err(e) => return ApiResponse::error(e),
                };
                let mut r = ApiResponse::ok();
                r.intent_id = Some(id);
                r
            }
            ApiRequest::ContextAssemble { agent_id, cids, budget_tokens } => {
                let candidates: Vec<crate::fs::context_budget::ContextCandidate> = cids
                    .into_iter()
                    .map(|c| crate::fs::context_budget::ContextCandidate {
                        cid: c.cid, relevance: c.relevance,
                    })
                    .collect();
                match self.context_assemble(&candidates, budget_tokens, &agent_id) {
                    Ok(allocation) => {
                        let used_tokens = allocation.total_tokens as u64;
                        if used_tokens > 0 {
                            self.scheduler.record_token_usage(&AgentId(agent_id.clone()), used_tokens);
                        }
                        let mut r = ApiResponse::ok();
                        r.context_assembly = Some(allocation);
                        r
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::DeclareIntent { agent_id, intent, related_cids, budget_tokens } => {
                match self.declare_intent(&agent_id, &intent, related_cids, budget_tokens) {
                    Ok(assembly_id) => {
                        let mut r = ApiResponse::ok();
                        r.assembly_id = Some(assembly_id);
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::FetchAssembledContext { agent_id, assembly_id } => {
                match self.fetch_assembled_context(&agent_id, &assembly_id) {
                    Some(Ok(allocation)) => {
                        let mut r = ApiResponse::ok();
                        r.context_assembly = Some(allocation);
                        r
                    }
                    Some(Err(e)) => ApiResponse::error(e),
                    None => ApiResponse::error(format!("assembly not found: {}", assembly_id)),
                }
            }
            ApiRequest::IntentFeedback { intent_id, used_cids, unused_cids, agent_id: _ } => {
                tracing::debug!(
                    "IntentFeedback for {}: {} used, {} unused",
                    intent_id, used_cids.len(), unused_cids.len()
                );
                ApiResponse::ok()
            }
            ApiRequest::BatchSubmitIntent { intents, agent_id } => {
                let batch_results = self.handle_batch_submit_intent(intents, &agent_id);
                let mut r = ApiResponse::ok();
                r.batch_submit_intent = Some(batch_results);
                r
            }
            ApiRequest::BatchQuery { queries, agent_id, tenant_id } => {
                let tenant = tenant_id.unwrap_or_else(|| DEFAULT_TENANT.to_string());
                let batch_results = self.handle_batch_query(queries, &agent_id, &tenant);
                let mut r = ApiResponse::ok();
                r.batch_query = Some(batch_results);
                r
            }
            _ => unreachable!("non-intent request routed to handle_intent"),
        }
    }
}
