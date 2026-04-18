//! Agent management commands.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiResponse, AgentDto};
use super::extract_arg;

pub fn cmd_agent(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    if args.get(1).map(|s| s.as_str()) == Some("set-resources") {
        let target = args.get(2).cloned().unwrap_or_default();
        let mq = extract_arg(args, "--memory-quota").and_then(|s| s.parse().ok());
        let cq = extract_arg(args, "--cpu-time-quota").and_then(|s| s.parse().ok());
        let at = extract_arg(args, "--allowed-tools")
            .map(|s| s.split(',').map(String::from).collect::<Vec<_>>());
        return match kernel.agent_set_resources(&target, mq, cq, at) {
            Ok(()) => ApiResponse::ok(),
            Err(e) => ApiResponse::error(e.to_string()),
        };
    }

    let name = extract_arg(args, "--register").unwrap_or_else(|| "unnamed".to_string());
    let id = kernel.register_agent(name);
    let mut r = ApiResponse::ok();
    r.agent_id = Some(id);
    r
}

pub fn cmd_agents(kernel: &AIKernel, _args: &[String]) -> ApiResponse {
    let agents = kernel.list_agents();
    let dto: Vec<AgentDto> = agents.into_iter().map(|a| AgentDto {
        id: a.id, name: a.name, state: format!("{:?}", a.state),
    }).collect();
    let mut r = ApiResponse::ok();
    r.agents = Some(dto);
    r
}

pub fn cmd_agent_status(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.agent_status(&agent_id) {
        Some((_id, state, pending)) => {
            let mut r = ApiResponse::ok();
            r.agent_state = Some(state);
            r.pending_intents = Some(pending);
            r
        }
        None => ApiResponse::error(format!("Agent not found: {}", agent_id)),
    }
}

pub fn cmd_agent_suspend(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.agent_suspend(&agent_id) {
        Ok(()) => ApiResponse::ok(),
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

pub fn cmd_agent_resume(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.agent_resume(&agent_id) {
        Ok(()) => ApiResponse::ok(),
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

pub fn cmd_agent_terminate(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.agent_terminate(&agent_id) {
        Ok(()) => ApiResponse::ok(),
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

pub fn cmd_agent_complete(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.agent_complete(&agent_id) {
        Ok(()) => ApiResponse::ok(),
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

pub fn cmd_agent_fail(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let reason = extract_arg(args, "--reason").unwrap_or_else(|| "unspecified".to_string());
    match kernel.agent_fail(&agent_id, &reason) {
        Ok(()) => ApiResponse::ok(),
        Err(e) => ApiResponse::error(e.to_string()),
    }
}
