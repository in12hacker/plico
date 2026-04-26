//! Agent lifecycle tool handlers.

use crate::kernel::AIKernel;
use crate::tool::ToolResult;
use serde_json::json;

pub(in crate::kernel) fn handle(kernel: &AIKernel, name: &str, params: &serde_json::Value, agent_id: &str) -> ToolResult {
    match name {
        "agent.complete" => {
            let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or(agent_id);
            match kernel.agent_complete(target) {
                Ok(()) => ToolResult::ok(json!({"completed": target})),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        "agent.fail" => {
            let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or(agent_id);
            let reason = params.get("reason").and_then(|v| v.as_str()).unwrap_or("unspecified");
            match kernel.agent_fail(target, reason) {
                Ok(()) => ToolResult::ok(json!({"failed": target, "reason": reason})),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        "agent.register" => {
            let name_param = params.get("name").and_then(|v| v.as_str()).unwrap_or("unnamed");
            let id = kernel.register_agent(name_param.to_string());
            ToolResult::ok(json!({"agent_id": id}))
        }
        "agent.status" => {
            let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or(agent_id);
            match kernel.agent_status(target) {
                Some((_id, state, pending)) => ToolResult::ok(json!({
                    "agent_id": target, "state": state, "pending_intents": pending,
                })),
                None => ToolResult::error(format!("agent not found: {}", target)),
            }
        }
        "agent.suspend" => {
            let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or(agent_id);
            match kernel.agent_suspend(target) {
                Ok(()) => ToolResult::ok(json!({"suspended": target})),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        "agent.resume" => {
            let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or(agent_id);
            match kernel.agent_resume(target) {
                Ok(()) => ToolResult::ok(json!({"resumed": target})),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        "agent.terminate" => {
            let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or(agent_id);
            match kernel.agent_terminate(target) {
                Ok(()) => ToolResult::ok(json!({"terminated": target})),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        "agent.set_resources" => {
            let target = params.get("agent_id").and_then(|v| v.as_str()).unwrap_or(agent_id);
            let mq = params.get("memory_quota").and_then(|v| v.as_u64());
            let cq = params.get("cpu_time_quota").and_then(|v| v.as_u64());
            let at: Option<Vec<String>> = params.get("allowed_tools")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect());
            match kernel.agent_set_resources(target, mq, cq, at) {
                Ok(()) => ToolResult::ok(json!({"updated": target})),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        _ => ToolResult::error(format!("unknown agent tool: {}", name)),
    }
}
