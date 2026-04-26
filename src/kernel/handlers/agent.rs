//! Agent lifecycle handlers.

use crate::api::semantic::{ApiRequest, ApiResponse, AgentDto};

impl super::super::AIKernel {
    pub(crate) fn handle_agent(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::RegisterAgent { name } => {
                let id = self.register_agent(name);
                let token = self.key_store.generate_token(&id);
                self.key_store.store_token(&token);
                let mut r = ApiResponse::ok();
                r.agent_id = Some(id);
                r.token = Some(token.token);
                r
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
