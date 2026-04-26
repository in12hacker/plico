//! Inter-agent messaging and task delegation handlers.

use crate::api::semantic::{ApiRequest, ApiResponse};

impl super::super::AIKernel {
    pub(crate) fn handle_messaging(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::SendMessage { from, to, payload } => {
                match self.send_message(&from, &to, payload) {
                    Ok(msg_id) => {
                        let mut r = ApiResponse::ok();
                        r.data = Some(msg_id);
                        r
                    }
                    Err(e) => ApiResponse::error(e.to_string()),
                }
            }
            ApiRequest::ReadMessages { agent_id, unread_only, limit, offset } => {
                let all_msgs = self.read_messages(&agent_id, unread_only);
                let total = all_msgs.len();
                let off = offset.unwrap_or(0);
                let lim = limit.unwrap_or(total);
                let page: Vec<_> = all_msgs.into_iter().skip(off).take(lim).collect();
                let mut r = ApiResponse::ok();
                r.messages = Some(page.clone());
                r.total_count = Some(total);
                r.has_more = Some(off + page.len() < total);
                r
            }
            ApiRequest::AckMessage { agent_id, message_id } => {
                if self.ack_message(&agent_id, &message_id) {
                    ApiResponse::ok()
                } else {
                    ApiResponse::error(format!("message not found: {}", message_id))
                }
            }
            ApiRequest::DiscoverAgents { state_filter, tool_filter, agent_id: _ } => {
                let cards = self.discover_agents(state_filter.as_deref(), tool_filter.as_deref());
                let mut r = ApiResponse::ok();
                r.agent_cards = Some(cards);
                r
            }
            ApiRequest::DelegateTask { task_id, from_agent, to_agent, intent, context_cids, deadline_ms } => {
                let task = self.task_store.create_task(
                    task_id, from_agent, to_agent, intent, context_cids, deadline_ms,
                );
                let mut r = ApiResponse::ok();
                r.task_result = Some(crate::api::semantic::TaskResult {
                    task_id: task.task_id, agent_id: task.to_agent, status: task.status,
                    result_cids: task.result_cids, failure_reason: task.failure_reason,
                    created_at_ms: task.created_at_ms, updated_at_ms: task.updated_at_ms,
                });
                r
            }
            ApiRequest::QueryTaskStatus { task_id } => {
                match self.task_store.get(&task_id) {
                    Some(task) => {
                        let mut r = ApiResponse::ok();
                        r.task_result = Some(crate::api::semantic::TaskResult {
                            task_id: task.task_id, agent_id: task.to_agent, status: task.status,
                            result_cids: task.result_cids, failure_reason: task.failure_reason,
                            created_at_ms: task.created_at_ms, updated_at_ms: task.updated_at_ms,
                        });
                        r
                    }
                    None => ApiResponse::error(format!("Task not found: {}", task_id)),
                }
            }
            ApiRequest::TaskStart { task_id, agent_id } => {
                match self.task_store.start_task(&task_id, &agent_id) {
                    Ok(task) => {
                        let mut r = ApiResponse::ok();
                        r.task_result = Some(crate::api::semantic::TaskResult {
                            task_id: task.task_id, agent_id: task.to_agent, status: task.status,
                            result_cids: task.result_cids, failure_reason: task.failure_reason,
                            created_at_ms: task.created_at_ms, updated_at_ms: task.updated_at_ms,
                        });
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::TaskComplete { task_id, agent_id, result_cids } => {
                match self.task_store.complete_task(&task_id, &agent_id, result_cids) {
                    Ok(task) => {
                        let mut r = ApiResponse::ok();
                        r.task_result = Some(crate::api::semantic::TaskResult {
                            task_id: task.task_id, agent_id: task.to_agent, status: task.status,
                            result_cids: task.result_cids, failure_reason: task.failure_reason,
                            created_at_ms: task.created_at_ms, updated_at_ms: task.updated_at_ms,
                        });
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            ApiRequest::TaskFail { task_id, agent_id, reason } => {
                match self.task_store.fail_task(&task_id, &agent_id, reason) {
                    Ok(task) => {
                        let mut r = ApiResponse::ok();
                        r.task_result = Some(crate::api::semantic::TaskResult {
                            task_id: task.task_id, agent_id: task.to_agent, status: task.status,
                            result_cids: task.result_cids, failure_reason: task.failure_reason,
                            created_at_ms: task.created_at_ms, updated_at_ms: task.updated_at_ms,
                        });
                        r
                    }
                    Err(e) => ApiResponse::error(e),
                }
            }
            _ => unreachable!("non-messaging request routed to handle_messaging"),
        }
    }
}
