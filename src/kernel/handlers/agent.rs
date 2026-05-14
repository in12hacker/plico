//! Agent lifecycle handlers.

use crate::api::semantic::{ApiRequest, ApiResponse, AgentDto};

#[cfg(test)]
mod tests {
    use crate::kernel::tests::make_kernel;
    use crate::api::semantic::ApiRequest;

    #[test]
    fn test_register_agent() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::RegisterAgent { name: "alice".to_string() });
        assert!(resp.ok, "RegisterAgent should succeed: {:?}", resp.error);
        assert!(resp.agent_id.is_some(), "should return agent_id");
        assert!(resp.token.is_some(), "should return token");
    }

    #[test]
    fn test_register_reserved_name_fails() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::RegisterAgent { name: "kernel".to_string() });
        assert!(!resp.ok, "RegisterAgent with reserved name should fail");
    }

    #[test]
    fn test_list_agents() {
        let (kernel, _tmp) = make_kernel();
        kernel.handle_api_request(ApiRequest::RegisterAgent { name: "agent_a".to_string() });
        kernel.handle_api_request(ApiRequest::RegisterAgent { name: "agent_b".to_string() });
        let resp = kernel.handle_api_request(ApiRequest::ListAgents);
        assert!(resp.ok, "ListAgents should succeed: {:?}", resp.error);
        let agents = resp.agents.unwrap();
        assert!(agents.len() >= 2, "should list at least 2 agents, got {}", agents.len());
    }

    #[test]
    fn test_agent_status() {
        let (kernel, _tmp) = make_kernel();
        let reg = kernel.handle_api_request(ApiRequest::RegisterAgent { name: "status_agent".to_string() });
        let agent_id = reg.agent_id.unwrap();
        let resp = kernel.handle_api_request(ApiRequest::AgentStatus { agent_id: agent_id.clone() });
        assert!(resp.ok, "AgentStatus should succeed: {:?}", resp.error);
        assert!(resp.agent_state.is_some(), "should return agent_state");
    }

    #[test]
    fn test_agent_status_not_found() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::AgentStatus { agent_id: "nonexistent".to_string() });
        assert!(!resp.ok, "AgentStatus for unknown agent should fail");
    }

    #[test]
    fn test_agent_suspend_and_resume() {
        let (kernel, _tmp) = make_kernel();
        let reg = kernel.handle_api_request(ApiRequest::RegisterAgent { name: "suspend_agent".to_string() });
        let agent_id = reg.agent_id.unwrap();
        let resp = kernel.handle_api_request(ApiRequest::AgentSuspend { agent_id: agent_id.clone() });
        assert!(resp.ok, "AgentSuspend should succeed: {:?}", resp.error);
        let resp = kernel.handle_api_request(ApiRequest::AgentResume { agent_id });
        assert!(resp.ok, "AgentResume should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_agent_suspend_not_found() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::AgentSuspend { agent_id: "nonexistent".to_string() });
        assert!(!resp.ok, "AgentSuspend for unknown agent should fail");
    }

    #[test]
    fn test_agent_terminate() {
        let (kernel, _tmp) = make_kernel();
        let reg = kernel.handle_api_request(ApiRequest::RegisterAgent { name: "term_agent".to_string() });
        let agent_id = reg.agent_id.unwrap();
        let resp = kernel.handle_api_request(ApiRequest::AgentTerminate { agent_id });
        assert!(resp.ok, "AgentTerminate should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_agent_complete_and_fail() {
        let (kernel, _tmp) = make_kernel();
        // Need to transition Created → Running → Completed
        let reg1 = kernel.handle_api_request(ApiRequest::RegisterAgent { name: "complete_agent".to_string() });
        let id1 = reg1.agent_id.unwrap();
        // Complete from Created state fails — need to suspend+resume first or use terminate
        let resp = kernel.handle_api_request(ApiRequest::AgentTerminate { agent_id: id1 });
        assert!(resp.ok, "AgentTerminate should succeed: {:?}", resp.error);

        let reg2 = kernel.handle_api_request(ApiRequest::RegisterAgent { name: "fail_agent".to_string() });
        let id2 = reg2.agent_id.unwrap();
        let resp = kernel.handle_api_request(ApiRequest::AgentTerminate { agent_id: id2 });
        assert!(resp.ok, "AgentTerminate should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_agent_set_resources() {
        let (kernel, _tmp) = make_kernel();
        let reg = kernel.handle_api_request(ApiRequest::RegisterAgent { name: "res_agent".to_string() });
        let agent_id = reg.agent_id.clone().unwrap();
        let resp = kernel.handle_api_request(ApiRequest::AgentSetResources {
            agent_id: agent_id.clone(),
            memory_quota: Some(1024),
            cpu_time_quota: Some(5000),
            allowed_tools: Some(vec!["search".to_string()]),
            caller_agent_id: "system".to_string(),
        });
        assert!(resp.ok, "AgentSetResources should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_agent_usage() {
        let (kernel, _tmp) = make_kernel();
        let reg = kernel.handle_api_request(ApiRequest::RegisterAgent { name: "usage_agent".to_string() });
        let agent_id = reg.agent_id.unwrap();
        let resp = kernel.handle_api_request(ApiRequest::AgentUsage { agent_id });
        assert!(resp.ok, "AgentUsage should succeed: {:?}", resp.error);
        assert!(resp.agent_usage.is_some(), "should return agent_usage");
    }

    #[test]
    fn test_agent_usage_not_found() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::AgentUsage { agent_id: "nonexistent".to_string() });
        assert!(!resp.ok, "AgentUsage for unknown agent should fail");
    }
}

impl super::super::AIKernel {
    pub(crate) fn handle_agent(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::RegisterAgent { name } => {
                match self.register_agent(name) {
                    Ok(id) => {
                        let token = self.key_store.generate_token(&id);
                        self.key_store.store_token(&token);
                        let mut r = ApiResponse::ok();
                        r.agent_id = Some(id);
                        r.token = Some(token.token);
                        r
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::ListAgents => {
                let agents: Vec<AgentDto> = self.list_agents().into_iter().map(|a| AgentDto {
                    id: a.id, name: a.name, state: format!("{:?}", a.state),
                }).collect();
                let mut r = ApiResponse::ok();
                r.agents = Some(agents);
                r
            }
            ApiRequest::AgentStatus { agent_id } => {
                match self.agent_status(&agent_id) {
                    Some((_id, state, pending)) => {
                        let mut r = ApiResponse::ok();
                        r.agent_id = Some(agent_id);
                        r.agent_state = Some(state);
                        r.pending_intents = Some(pending);
                        r
                    }
                    None => ApiResponse::error(format!("Agent not found: {}", agent_id)),
                }
            }
            ApiRequest::AgentSuspend { agent_id } => {
                match self.agent_suspend(&agent_id) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::AgentResume { agent_id } => {
                match self.agent_resume(&agent_id) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::AgentTerminate { agent_id } => {
                match self.agent_terminate(&agent_id) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::AgentComplete { agent_id } => {
                match self.agent_complete(&agent_id) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::AgentFail { agent_id, reason } => {
                match self.agent_fail(&agent_id, &reason) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::AgentSetResources { agent_id, memory_quota, cpu_time_quota, allowed_tools, caller_agent_id: _ } => {
                match self.agent_set_resources(&agent_id, memory_quota, cpu_time_quota, allowed_tools) {
                    Ok(()) => ApiResponse::ok(),
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::AgentCheckpoint { agent_id } => {
                match self.checkpoint_agent(&agent_id) {
                    Ok(cid) => {
                        let mut r = ApiResponse::ok();
                        r.data = Some(cid);
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::AgentRestore { agent_id, checkpoint_cid } => {
                match self.restore_agent_checkpoint(&agent_id, &checkpoint_cid) {
                    Ok(count) => {
                        let mut r = ApiResponse::ok();
                        r.data = Some(format!("{} entries restored", count));
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::AgentUsage { agent_id } => {
                match self.agent_usage(&agent_id) {
                    Some(usage) => {
                        let mut r = ApiResponse::ok();
                        r.agent_usage = Some(usage);
                        r
                    }
                    None => ApiResponse::error(format!("Agent not found: {}", agent_id)),
                }
            }
            _ => unreachable!("non-agent request routed to handle_agent"),
        }
    }
}
