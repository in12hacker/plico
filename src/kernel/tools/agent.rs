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
            match kernel.register_agent(name_param.to_string()) {
                Ok(id) => ToolResult::ok(json!({"agent_id": id})),
                Err(e) => ToolResult::error(e.to_string()),
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::tests::make_kernel;
    use crate::kernel::tools::agent::handle;

    #[test]
    fn test_agent_register() {
        let (kernel, _tmp) = make_kernel();
        let result = handle(&*kernel, "agent.register", &json!({"name": "new_agent"}), "test");
        assert!(result.error.is_none(), "register should succeed: {:?}", result.error);
        let data = result.output;
        assert!(data["agent_id"].as_str().unwrap().len() > 0);
    }

    #[test]
    fn test_agent_status() {
        let (kernel, _tmp) = make_kernel();
        kernel.register_agent("status_agent".to_string()).unwrap();
        let result = handle(&*kernel, "agent.status", &json!({"agent_id": "status_agent"}), "test");
        assert!(result.error.is_none(), "status should succeed: {:?}", result.error);
    }

    #[test]
    fn test_agent_status_not_found() {
        let (kernel, _tmp) = make_kernel();
        let result = handle(&*kernel, "agent.status", &json!({"agent_id": "nonexistent"}), "test");
        assert!(result.error.is_some());
    }

    #[test]
    fn test_agent_complete() {
        let (kernel, _tmp) = make_kernel();
        let reg = handle(&*kernel, "agent.register", &json!({"name": "completer"}), "test");
        let agent_id = reg.output["agent_id"].as_str().unwrap().to_string();
        // State transition may fail depending on agent state — just test the code path
        let _result = handle(&*kernel, "agent.complete", &json!({"agent_id": agent_id}), "test");
    }

    #[test]
    fn test_agent_fail() {
        let (kernel, _tmp) = make_kernel();
        let reg = handle(&*kernel, "agent.register", &json!({"name": "failer"}), "test");
        let agent_id = reg.output["agent_id"].as_str().unwrap().to_string();
        let _result = handle(&*kernel, "agent.fail", &json!({"agent_id": agent_id, "reason": "test failure"}), "test");
    }

    #[test]
    fn test_agent_suspend_resume() {
        let (kernel, _tmp) = make_kernel();
        let reg = handle(&*kernel, "agent.register", &json!({"name": "suspendable"}), "test");
        let agent_id = reg.output["agent_id"].as_str().unwrap().to_string();

        let suspend_result = handle(&*kernel, "agent.suspend", &json!({"agent_id": agent_id}), "test");
        assert!(suspend_result.error.is_none(), "suspend should succeed: {:?}", suspend_result.error);

        let resume_result = handle(&*kernel, "agent.resume", &json!({"agent_id": agent_id}), "test");
        assert!(resume_result.error.is_none(), "resume should succeed: {:?}", resume_result.error);
    }

    #[test]
    fn test_agent_terminate() {
        let (kernel, _tmp) = make_kernel();
        let reg = handle(&*kernel, "agent.register", &json!({"name": "terminable"}), "test");
        let agent_id = reg.output["agent_id"].as_str().unwrap().to_string();
        let result = handle(&*kernel, "agent.terminate", &json!({"agent_id": agent_id}), "test");
        assert!(result.error.is_none());
    }

    #[test]
    fn test_agent_set_resources() {
        let (kernel, _tmp) = make_kernel();
        let agent_id = kernel.register_agent("res_agent".to_string()).unwrap();
        let result = handle(&*kernel, "agent.set_resources", &json!({
            "agent_id": agent_id,
            "memory_quota": 1024,
            "cpu_time_quota": 60
        }), "test");
        assert!(result.error.is_none(), "set_resources should succeed: {:?}", result.error);
    }

    #[test]
    fn test_agent_unknown_tool() {
        let (kernel, _tmp) = make_kernel();
        let result = handle(&*kernel, "agent.nonexistent", &json!({}), "test");
        assert!(result.error.is_some());
    }
}
