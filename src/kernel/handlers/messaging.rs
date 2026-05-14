//! Inter-agent messaging and task delegation handlers.

use crate::api::semantic::{ApiRequest, ApiResponse};

#[cfg(test)]
mod tests {
    use crate::kernel::tests::make_kernel;
    use crate::api::semantic::ApiRequest;

    fn grant_send_perm(kernel: &crate::kernel::AIKernel, agent: &str) {
        kernel.handle_api_request(ApiRequest::GrantPermission {
            agent_id: agent.to_string(),
            action: "send_message".to_string(),
            scope: None,
            expires_at: None,
        });
    }

    fn grant_read_perm(kernel: &crate::kernel::AIKernel, agent: &str) {
        kernel.handle_api_request(ApiRequest::GrantPermission {
            agent_id: agent.to_string(),
            action: "read".to_string(),
            scope: None,
            expires_at: None,
        });
    }

    #[test]
    fn test_send_message() {
        let (kernel, _tmp) = make_kernel();
        grant_send_perm(&kernel, "agent_a");
        let resp = kernel.handle_api_request(ApiRequest::SendMessage {
            from: "agent_a".to_string(),
            to: "agent_b".to_string(),
            payload: serde_json::json!({"text": "hello"}),
        });
        assert!(resp.ok, "SendMessage should succeed: {:?}", resp.error);
        assert!(resp.data.is_some(), "should return message id");
    }

    #[test]
    fn test_read_messages() {
        let (kernel, _tmp) = make_kernel();
        grant_send_perm(&kernel, "agent_a");
        grant_send_perm(&kernel, "agent_c");
        grant_read_perm(&kernel, "agent_b");
        kernel.handle_api_request(ApiRequest::SendMessage {
            from: "agent_a".to_string(),
            to: "agent_b".to_string(),
            payload: serde_json::json!({"text": "msg1"}),
        });
        kernel.handle_api_request(ApiRequest::SendMessage {
            from: "agent_c".to_string(),
            to: "agent_b".to_string(),
            payload: serde_json::json!({"text": "msg2"}),
        });
        let resp = kernel.handle_api_request(ApiRequest::ReadMessages {
            agent_id: "agent_b".to_string(),
            unread_only: false,
            limit: None,
            offset: None,
        });
        assert!(resp.ok, "ReadMessages should succeed: {:?}", resp.error);
        let msgs = resp.messages.unwrap();
        assert_eq!(msgs.len(), 2, "should have 2 messages");
    }

    #[test]
    fn test_read_messages_with_limit() {
        let (kernel, _tmp) = make_kernel();
        grant_send_perm(&kernel, "agent_a");
        grant_read_perm(&kernel, "agent_b");
        for i in 0..3 {
            kernel.handle_api_request(ApiRequest::SendMessage {
                from: "agent_a".to_string(),
                to: "agent_b".to_string(),
                payload: serde_json::json!({"text": format!("msg{}", i)}),
            });
        }
        let resp = kernel.handle_api_request(ApiRequest::ReadMessages {
            agent_id: "agent_b".to_string(),
            unread_only: false,
            limit: Some(1),
            offset: None,
        });
        assert!(resp.ok);
        let msgs = resp.messages.unwrap();
        assert_eq!(msgs.len(), 1, "should return 1 message (limit=1)");
        assert_eq!(resp.total_count, Some(3));
        assert_eq!(resp.has_more, Some(true));
    }

    #[test]
    fn test_ack_message() {
        let (kernel, _tmp) = make_kernel();
        grant_send_perm(&kernel, "agent_a");
        let send = kernel.handle_api_request(ApiRequest::SendMessage {
            from: "agent_a".to_string(),
            to: "agent_b".to_string(),
            payload: serde_json::json!({"text": "ack me"}),
        });
        let msg_id = send.data.unwrap();
        let resp = kernel.handle_api_request(ApiRequest::AckMessage {
            agent_id: "agent_b".to_string(),
            message_id: msg_id,
        });
        assert!(resp.ok, "AckMessage should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_ack_message_not_found() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::AckMessage {
            agent_id: "agent_a".to_string(),
            message_id: "nonexistent_msg".to_string(),
        });
        assert!(!resp.ok, "AckMessage for unknown message should fail");
    }

    #[test]
    fn test_discover_agents() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::DiscoverAgents {
            state_filter: None,
            tool_filter: None,
            agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok, "DiscoverAgents should succeed: {:?}", resp.error);
        assert!(resp.agent_cards.is_some());
    }

    #[test]
    fn test_delegate_task() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::DelegateTask {
            task_id: "task_001".to_string(),
            from_agent: "agent_a".to_string(),
            to_agent: "agent_b".to_string(),
            intent: "process data".to_string(),
            context_cids: vec![],
            deadline_ms: None,
        });
        assert!(resp.ok, "DelegateTask should succeed: {:?}", resp.error);
        let task = resp.task_result.unwrap();
        assert_eq!(task.task_id, "task_001");
    }

    #[test]
    fn test_task_lifecycle() {
        let (kernel, _tmp) = make_kernel();
        // Create task
        kernel.handle_api_request(ApiRequest::DelegateTask {
            task_id: "task_lifecycle".to_string(),
            from_agent: "agent_a".to_string(),
            to_agent: "agent_b".to_string(),
            intent: "do work".to_string(),
            context_cids: vec![],
            deadline_ms: None,
        });

        // Query status
        let resp = kernel.handle_api_request(ApiRequest::QueryTaskStatus {
            task_id: "task_lifecycle".to_string(),
        });
        assert!(resp.ok, "QueryTaskStatus should succeed: {:?}", resp.error);

        // Start task
        let resp = kernel.handle_api_request(ApiRequest::TaskStart {
            task_id: "task_lifecycle".to_string(),
            agent_id: "agent_b".to_string(),
        });
        assert!(resp.ok, "TaskStart should succeed: {:?}", resp.error);

        // Complete task
        let resp = kernel.handle_api_request(ApiRequest::TaskComplete {
            task_id: "task_lifecycle".to_string(),
            agent_id: "agent_b".to_string(),
            result_cids: vec!["result_cid".to_string()],
        });
        assert!(resp.ok, "TaskComplete should succeed: {:?}", resp.error);
        let task = resp.task_result.unwrap();
        assert_eq!(task.status, crate::api::dto::TaskStatus::Completed);
        assert_eq!(task.result_cids, vec!["result_cid".to_string()]);
    }

    #[test]
    fn test_task_fail() {
        let (kernel, _tmp) = make_kernel();
        kernel.handle_api_request(ApiRequest::DelegateTask {
            task_id: "task_fail".to_string(),
            from_agent: "agent_a".to_string(),
            to_agent: "agent_b".to_string(),
            intent: "failing task".to_string(),
            context_cids: vec![],
            deadline_ms: None,
        });
        let resp = kernel.handle_api_request(ApiRequest::TaskFail {
            task_id: "task_fail".to_string(),
            agent_id: "agent_b".to_string(),
            reason: "couldn't complete".to_string(),
        });
        assert!(resp.ok, "TaskFail should succeed: {:?}", resp.error);
        let task = resp.task_result.unwrap();
        assert_eq!(task.status, crate::api::dto::TaskStatus::Failed);
        assert_eq!(task.failure_reason, Some("couldn't complete".to_string()));
    }

    #[test]
    fn test_query_task_status_not_found() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::QueryTaskStatus {
            task_id: "nonexistent_task".to_string(),
        });
        assert!(!resp.ok, "QueryTaskStatus for unknown task should fail");
    }
}

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
