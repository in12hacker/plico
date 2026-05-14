//! Tool execution and hook management handlers.

use crate::api::semantic::{ApiRequest, ApiResponse};
use super::super::hook;

#[cfg(test)]
mod tests {
    use crate::kernel::tests::make_kernel;
    use crate::api::semantic::ApiRequest;

    #[test]
    fn test_tool_list() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::ToolList {
            agent_id: "test_agent".to_string(),
        });
        assert!(resp.ok, "ToolList should succeed: {:?}", resp.error);
        assert!(resp.tools.is_some(), "should return tools list");
    }

    #[test]
    fn test_tool_describe_not_found() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::ToolDescribe {
            tool: "nonexistent_tool".to_string(),
            agent_id: "test_agent".to_string(),
        });
        assert!(!resp.ok, "ToolDescribe for unknown tool should fail");
    }

    #[test]
    fn test_hook_list_empty() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::HookList);
        assert!(resp.ok, "HookList should succeed: {:?}", resp.error);
        assert!(resp.hook_list.is_some());
    }

    #[test]
    fn test_hook_register_block() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::HookRegister {
            point: "PreToolCall".to_string(),
            action: "block".to_string(),
            tool_pattern: Some("dangerous".to_string()),
            reason: Some("safety".to_string()),
            priority: Some(10),
        });
        assert!(resp.ok, "HookRegister block should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_hook_register_log() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::HookRegister {
            point: "PostToolCall".to_string(),
            action: "log".to_string(),
            tool_pattern: None,
            reason: None,
            priority: None,
        });
        assert!(resp.ok, "HookRegister log should succeed: {:?}", resp.error);
    }

    #[test]
    fn test_hook_register_unknown_point() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::HookRegister {
            point: "InvalidPoint".to_string(),
            action: "block".to_string(),
            tool_pattern: None,
            reason: None,
            priority: None,
        });
        assert!(!resp.ok, "HookRegister with unknown point should fail");
    }

    #[test]
    fn test_hook_register_unknown_action() {
        let (kernel, _tmp) = make_kernel();
        let resp = kernel.handle_api_request(ApiRequest::HookRegister {
            point: "PreToolCall".to_string(),
            action: "invalid_action".to_string(),
            tool_pattern: None,
            reason: None,
            priority: None,
        });
        assert!(!resp.ok, "HookRegister with unknown action should fail");
    }

    #[test]
    fn test_hook_list_after_register() {
        let (kernel, _tmp) = make_kernel();
        let before = kernel.handle_api_request(ApiRequest::HookList);
        let count_before = before.hook_list.as_ref().unwrap().len();
        kernel.handle_api_request(ApiRequest::HookRegister {
            point: "PreWrite".to_string(),
            action: "log".to_string(),
            tool_pattern: None,
            reason: None,
            priority: Some(25),
        });
        let resp = kernel.handle_api_request(ApiRequest::HookList);
        assert!(resp.ok);
        let hooks = resp.hook_list.unwrap();
        assert_eq!(hooks.len(), count_before + 1, "should have 1 more hook after register");
    }

    #[test]
    fn test_tool_call() {
        let (kernel, _tmp) = make_kernel();
        // Grant Execute permission to agent
        kernel.handle_api_request(ApiRequest::GrantPermission {
            agent_id: "test_agent".to_string(),
            action: "execute".to_string(),
            scope: None,
            expires_at: None,
        });
        let resp = kernel.handle_api_request(ApiRequest::ToolCall {
            tool: "nonexistent".to_string(),
            params: serde_json::json!({}),
            agent_id: "test_agent".to_string(),
        });
        // Tool call should succeed even if tool not found (returns error in ToolResult)
        assert!(resp.ok, "ToolCall should return ok: {:?}", resp.error);
        assert!(resp.tool_result.is_some(), "should return tool_result");
    }
}

struct ApiBlockHook {
    tool_pattern: Option<String>,
    reason: String,
}

impl hook::HookHandler for ApiBlockHook {
    fn handle(&self, _point: hook::HookPoint, context: &hook::HookContext) -> hook::HookResult {
        if let Some(ref pattern) = self.tool_pattern {
            if !context.tool_name.contains(pattern.as_str()) {
                return hook::HookResult::Continue;
            }
        }
        hook::HookResult::Block { reason: self.reason.clone() }
    }
}

struct ApiLogHook {
    tool_pattern: Option<String>,
}

impl hook::HookHandler for ApiLogHook {
    fn handle(&self, point: hook::HookPoint, context: &hook::HookContext) -> hook::HookResult {
        if let Some(ref pattern) = self.tool_pattern {
            if !context.tool_name.contains(pattern.as_str()) {
                return hook::HookResult::Continue;
            }
        }
        tracing::info!(point = ?point, tool = %context.tool_name, agent = %context.agent_id, "API hook triggered");
        hook::HookResult::Continue
    }
}

impl super::super::AIKernel {
    pub(crate) fn handle_tools(&self, req: ApiRequest) -> ApiResponse {
        match req {
            ApiRequest::ToolCall { tool, params, agent_id } => {
                let result = self.execute_tool(&tool, &params, &agent_id);
                let mut r = ApiResponse::ok();
                r.tool_result = Some(result);
                r
            }
            ApiRequest::ToolList { agent_id: _ } => {
                let tools = self.tool_registry.list();
                let mut r = ApiResponse::ok();
                r.tools = Some(tools);
                r
            }
            ApiRequest::ToolDescribe { tool, agent_id: _ } => {
                match self.tool_registry.get(&tool) {
                    Some(desc) => {
                        let mut r = ApiResponse::ok();
                        r.tools = Some(vec![desc]);
                        r
                    }
                    None => ApiResponse::error(format!("tool not found: {}", tool)),
                }
            }
            ApiRequest::HookList => {
                let hooks = self.hook_registry.list_hooks();
                let dtos: Vec<crate::api::semantic::HookEntryDto> = hooks.into_iter()
                    .map(|(point, prio)| crate::api::semantic::HookEntryDto { point, priority: prio })
                    .collect();
                let mut r = ApiResponse::ok();
                r.hook_list = Some(dtos);
                r
            }
            ApiRequest::HookRegister { point, action, tool_pattern, reason, priority } => {
                let hook_point = match point.to_lowercase().as_str() {
                    "pretoolcall" | "pre-tool-call" | "pre" => hook::HookPoint::PreToolCall,
                    "posttoolcall" | "post-tool-call" | "post" => hook::HookPoint::PostToolCall,
                    "prewrite" | "pre-write" => hook::HookPoint::PreWrite,
                    "predelete" | "pre-delete" => hook::HookPoint::PreDelete,
                    "presessionstart" | "pre-session" => hook::HookPoint::PreSessionStart,
                    _ => return ApiResponse::error(format!("Unknown hook point: {}. Use: PreToolCall, PostToolCall, PreWrite, PreDelete, PreSessionStart", point)),
                };
                let prio = priority.unwrap_or(50);
                let reason_str = reason.unwrap_or_else(|| "blocked by API hook".to_string());
                let handler: std::sync::Arc<dyn hook::HookHandler> = match action.to_lowercase().as_str() {
                    "block" => std::sync::Arc::new(ApiBlockHook { tool_pattern: tool_pattern.clone(), reason: reason_str }),
                    "log" | "continue" => std::sync::Arc::new(ApiLogHook { tool_pattern: tool_pattern.clone() }),
                    _ => return ApiResponse::error(format!("Unknown action: {}. Use: block, log", action)),
                };
                self.hook_registry.register(hook_point.clone(), prio, handler);
                ApiResponse::ok_with_message(format!("Hook registered: {:?} priority={} action={}", hook_point, prio, action))
            }
            _ => unreachable!("non-tools request routed to handle_tools"),
        }
    }
}
