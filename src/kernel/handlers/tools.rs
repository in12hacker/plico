//! Tool execution and hook management handlers.

use crate::api::semantic::{ApiRequest, ApiResponse};
use super::super::hook;

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
