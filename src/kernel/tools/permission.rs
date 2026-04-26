//! Permission tool handlers (grant, revoke, list, check).

use crate::kernel::AIKernel;
use crate::tool::ToolResult;
use serde_json::json;

pub(in crate::kernel) fn handle(kernel: &AIKernel, name: &str, params: &serde_json::Value, agent_id: &str) -> ToolResult {
    match name {
        "permission.grant" => {
            let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or("");
            let action_str = params.get("action").and_then(|v| v.as_str()).unwrap_or("");
            let scope = params.get("scope").and_then(|v| v.as_str()).map(String::from);
            let expires_at = params.get("expires_at").and_then(|v| v.as_u64());
            match crate::api::permission::PermissionGuard::parse_action(action_str) {
                Some(action) => {
                    kernel.permission_grant(target, action, scope, expires_at);
                    ToolResult::ok(json!({"granted": true, "agent_id": target, "action": action_str}))
                }
                None => ToolResult::error(format!("Unknown action: {}", action_str)),
            }
        }
        "permission.revoke" => {
            let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or("");
            let action_str = params.get("action").and_then(|v| v.as_str()).unwrap_or("");
            match crate::api::permission::PermissionGuard::parse_action(action_str) {
                Some(action) => {
                    kernel.permission_revoke(target, action);
                    ToolResult::ok(json!({"revoked": true, "agent_id": target, "action": action_str}))
                }
                None => ToolResult::error(format!("Unknown action: {}", action_str)),
            }
        }
        "permission.list" => {
            let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or(agent_id);
            let grants = kernel.permission_list(target);
            let dto: Vec<serde_json::Value> = grants.into_iter().map(|g| json!({
                "action": format!("{:?}", g.action),
                "scope": g.scope,
                "expires_at": g.expires_at,
            })).collect();
            ToolResult::ok(json!({"agent_id": target, "grants": dto}))
        }
        "permission.check" => {
            let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or(agent_id);
            let action_str = params.get("action").and_then(|v| v.as_str()).unwrap_or("");
            match crate::api::permission::PermissionGuard::parse_action(action_str) {
                Some(action) => {
                    let allowed = kernel.permission_check(target, action).is_ok();
                    ToolResult::ok(json!({"agent_id": target, "action": action_str, "allowed": allowed}))
                }
                None => ToolResult::error(format!("Unknown action: {}", action_str)),
            }
        }
        _ => ToolResult::error(format!("unknown permission tool: {}", name)),
    }
}
