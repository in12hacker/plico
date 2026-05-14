//! Intent submission and context assembly handlers.

use crate::api::semantic::{ApiRequest, ApiResponse};
use crate::scheduler::{AgentId, IntentPriority};
use crate::DEFAULT_TENANT;

#[cfg(test)]
mod tests {
    use crate::kernel::tests::make_kernel;
    use crate::api::semantic::ApiRequest;

    #[test]
    fn test_submit_intent_medium() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::SubmitIntent {
            description: "analyze data".to_string(),
            priority: "medium".to_string(),
            action: None,
            agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok, "SubmitIntent should succeed: {:?}", resp.error);
        assert!(resp.intent_id.is_some(), "should return intent_id");
    }

    #[test]
    fn test_submit_intent_critical() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::SubmitIntent {
            description: "urgent task".to_string(),
            priority: "critical".to_string(),
            action: None,
            agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok, "SubmitIntent critical should succeed: {:?}", resp.error);
        assert!(resp.intent_id.is_some());
    }

    #[test]
    fn test_submit_intent_high() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::SubmitIntent {
            description: "high priority task".to_string(),
            priority: "high".to_string(),
            action: None,
            agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok);
    }

    #[test]
    fn test_submit_intent_low() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::SubmitIntent {
            description: "low priority task".to_string(),
            priority: "low".to_string(),
            action: None,
            agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok);
    }

    #[test]
    fn test_context_assemble() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::ContextAssemble {
            agent_id: "test_agent".to_string(),
            cids: vec![],
            budget_tokens: 4096,
        });
        assert!(resp.ok, "ContextAssemble should succeed: {:?}", resp.error);
        assert!(resp.context_assembly.is_some());
    }

    #[test]
    fn test_intent_feedback() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::IntentFeedback {
            intent_id: "some_intent_id".to_string(),
            used_cids: vec!["cid1".to_string()],
            unused_cids: vec!["cid2".to_string()],
            agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok, "IntentFeedback should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_batch_submit_intent() {
        use crate::api::dto::IntentSpec;
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::BatchSubmitIntent {
            intents: vec![
                IntentSpec {
                    description: "task 1".to_string(),
                    priority: "high".to_string(),
                    action: None,
                },
                IntentSpec {
                    description: "task 2".to_string(),
                    priority: "low".to_string(),
                    action: None,
                },
            ],
            agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok, "BatchSubmitIntent should succeed: {:?}", resp.error);
        let batch = resp.batch_submit_intent.unwrap();
        assert_eq!(batch.results.len(), 2);
        assert_eq!(batch.successful, 2);
    }

    #[test]
    fn test_batch_query() {
        use crate::api::dto::QuerySpec;
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::BatchQuery {
            queries: vec![
                QuerySpec::Recall,
            ],
            agent_id: "test_agent".to_string(),
            tenant_id: None,
        });
        assert!(resp.ok, "BatchQuery should succeed: {:?}", resp.error);
        assert!(resp.batch_query.is_some());
    }
}

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
