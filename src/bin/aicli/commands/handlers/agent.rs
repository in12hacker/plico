//! Agent management commands — all operations route through handle_api_request.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse};
use super::extract_arg;

pub fn cmd_agent(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    if args.get(1).map(|s| s.as_str()) == Some("set-resources") {
        let target = args.get(2).cloned().unwrap_or_default();
        let resolved = kernel.resolve_agent(&target).unwrap_or_else(|| target.clone());
        let mq = extract_arg(args, "--memory-quota").and_then(|s| s.parse().ok());
        let cq = extract_arg(args, "--cpu-time-quota").and_then(|s| s.parse().ok());
        let at = extract_arg(args, "--allowed-tools")
            .map(|s| s.split(',').map(String::from).collect::<Vec<_>>());
        return kernel.handle_api_request(ApiRequest::AgentSetResources {
            agent_id: resolved,
            memory_quota: mq,
            cpu_time_quota: cq,
            allowed_tools: at,
            caller_agent_id: "cli".to_string(),
        });
    }

    let name = extract_arg(args, "--name").unwrap_or_else(|| "unnamed".to_string());
    kernel.handle_api_request(ApiRequest::RegisterAgent { name })
}

pub fn cmd_agents(kernel: &AIKernel, _args: &[String]) -> ApiResponse {
    kernel.handle_api_request(ApiRequest::ListAgents)
}

pub fn cmd_agent_status(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    kernel.handle_api_request(ApiRequest::AgentStatus { agent_id })
}

pub fn cmd_agent_suspend(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    kernel.handle_api_request(ApiRequest::AgentSuspend { agent_id })
}

pub fn cmd_agent_resume(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    kernel.handle_api_request(ApiRequest::AgentResume { agent_id })
}

pub fn cmd_agent_terminate(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    kernel.handle_api_request(ApiRequest::AgentTerminate { agent_id })
}

pub fn cmd_agent_complete(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    kernel.handle_api_request(ApiRequest::AgentComplete { agent_id })
}

pub fn cmd_agent_fail(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let reason = extract_arg(args, "--reason").unwrap_or_else(|| "unspecified".to_string());
    kernel.handle_api_request(ApiRequest::AgentFail { agent_id, reason })
}

pub fn cmd_agent_checkpoint(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let name_or_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let agent_id = kernel.resolve_agent(&name_or_id).unwrap_or(name_or_id);
    kernel.handle_api_request(ApiRequest::AgentCheckpoint { agent_id })
}

pub fn cmd_agent_restore(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let name_or_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let agent_id = kernel.resolve_agent(&name_or_id).unwrap_or(name_or_id);
    let checkpoint_cid = match extract_arg(args, "--cid") {
        Some(c) => c,
        None => return ApiResponse::error("--cid required"),
    };
    kernel.handle_api_request(ApiRequest::AgentRestore { agent_id, checkpoint_cid })
}

pub fn cmd_quota(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let name_or_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let agent_id = match kernel.resolve_agent(&name_or_id) {
        Some(id) => id,
        None => return ApiResponse::error(format!("Agent not found: {}", name_or_id)),
    };
    kernel.handle_api_request(ApiRequest::AgentUsage { agent_id })
}

pub fn cmd_discover(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let state_filter = extract_arg(args, "--state");
    let tool_filter = extract_arg(args, "--tool");
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    kernel.handle_api_request(ApiRequest::DiscoverAgents { state_filter, tool_filter, agent_id })
}

pub fn cmd_delegate(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let from = extract_arg(args, "--from").unwrap_or_else(|| "cli".to_string());
    let to = match extract_arg(args, "--to") {
        Some(t) => t,
        None => return ApiResponse::error("--to required"),
    };
    let to_id = kernel.resolve_agent(&to).unwrap_or(to);
    let desc = extract_arg(args, "--desc").unwrap_or_else(|| "delegated task".to_string());
    let context_cids = extract_arg(args, "--context-cids")
        .map(|s| s.split(',').map(String::from).collect::<Vec<_>>())
        .unwrap_or_default();
    let deadline = extract_arg(args, "--deadline").and_then(|s| s.parse().ok());
    let task_id = extract_arg(args, "--task-id")
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    kernel.handle_api_request(ApiRequest::DelegateTask {
        task_id,
        from_agent: from,
        to_agent: to_id,
        intent: desc,
        context_cids,
        deadline_ms: deadline,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_kernel() -> AIKernel {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("EMBEDDING_BACKEND", "stub");
        AIKernel::new(dir.path().to_path_buf()).expect("kernel")
    }

    #[test]
    fn test_cmd_agent_register_basic() {
        let kernel = make_test_kernel();
        let args = vec!["agent".to_string(), "--name".to_string(), "test-agent".to_string()];
        let r = cmd_agent(&kernel, &args);
        assert!(r.error.is_none());
        assert!(r.agent_id.is_some());
    }

    #[test]
    fn test_cmd_agents_list_basic() {
        let kernel = make_test_kernel();
        // Register an agent first so list has something
        let register_args = vec!["agent".to_string(), "--name".to_string(), "list-test-agent".to_string()];
        cmd_agent(&kernel, &register_args);

        let r = cmd_agents(&kernel, &[]);
        assert!(r.error.is_none());
        assert!(r.agents.is_some());
        let agents = r.agents.unwrap();
        assert!(!agents.is_empty());
    }

    #[test]
    fn test_cmd_agent_status_nonexistent() {
        let kernel = make_test_kernel();
        let args = vec!["agent".to_string(), "status".to_string(), "--agent".to_string(), "nonexistent-agent".to_string()];
        let r = cmd_agent_status(&kernel, &args);
        assert!(r.error.is_some());
        assert!(r.error.unwrap().contains("Agent not found"));
    }

    #[test]
    fn test_cmd_agent_set_resources_basic() {
        let kernel = make_test_kernel();
        // Register an agent first
        let reg_args = vec!["agent".to_string(), "--name".to_string(), "resource-test-agent".to_string()];
        cmd_agent(&kernel, &reg_args);

        // Set resources
        let set_args = vec![
            "agent".to_string(), "set-resources".to_string(),
            "resource-test-agent".to_string(),
            "--memory-quota".to_string(), "1024".to_string(),
            "--cpu-time-quota".to_string(), "3600".to_string(),
        ];
        let r = cmd_agent(&kernel, &set_args);
        assert!(r.error.is_none());
    }

    #[test]
    fn test_cmd_agent_suspend_basic() {
        let kernel = make_test_kernel();
        let reg_args = vec!["agent".to_string(), "--name".to_string(), "suspend-test-agent".to_string()];
        cmd_agent(&kernel, &reg_args);

        let args = vec!["agent".to_string(), "suspend".to_string(), "--agent".to_string(), "suspend-test-agent".to_string()];
        let r = cmd_agent_suspend(&kernel, &args);
        assert!(r.error.is_none());
    }

    #[test]
    fn test_cmd_agent_resume_basic() {
        let kernel = make_test_kernel();
        let reg_args = vec!["agent".to_string(), "--name".to_string(), "resume-test-agent".to_string()];
        cmd_agent(&kernel, &reg_args);

        // Suspend first
        let suspend_args = vec!["agent".to_string(), "suspend".to_string(), "--agent".to_string(), "resume-test-agent".to_string()];
        cmd_agent_suspend(&kernel, &suspend_args);

        // Then resume
        let args = vec!["agent".to_string(), "resume".to_string(), "--agent".to_string(), "resume-test-agent".to_string()];
        let r = cmd_agent_resume(&kernel, &args);
        assert!(r.error.is_none());
    }

    #[test]
    fn test_cmd_quota_basic() {
        let kernel = make_test_kernel();
        let reg_args = vec!["agent".to_string(), "--name".to_string(), "quota-test-agent".to_string()];
        cmd_agent(&kernel, &reg_args);

        let args = vec!["quota".to_string(), "--agent".to_string(), "quota-test-agent".to_string()];
        let r = cmd_quota(&kernel, &args);
        assert!(r.error.is_none());
        assert!(r.agent_usage.is_some());
    }
}
