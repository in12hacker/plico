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

    let name = extract_arg(args, "--name").unwrap_or_else(|| "unnamed".to_string());
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
        Ok(()) => ApiResponse::ok_with_message(format!("Agent '{}' suspended", agent_id)),
        Err(e) => ApiResponse::error_with_diagnosis(
            e.to_string(),
            "AGENT_OPERATION_FAILED",
            "Check agent ID and try again",
            vec![format!("plico(agent=\"list\")")],
        ),
    }
}

pub fn cmd_agent_resume(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.agent_resume(&agent_id) {
        Ok(()) => ApiResponse::ok_with_message(format!("Agent '{}' resumed", agent_id)),
        Err(e) => ApiResponse::error_with_diagnosis(
            e.to_string(),
            "AGENT_OPERATION_FAILED",
            "Check agent ID and try again",
            vec![format!("plico(agent=\"list\")")],
        ),
    }
}

pub fn cmd_agent_terminate(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.agent_terminate(&agent_id) {
        Ok(()) => ApiResponse::ok_with_message(format!("Agent '{}' terminated", agent_id)),
        Err(e) => ApiResponse::error_with_diagnosis(
            e.to_string(),
            "AGENT_OPERATION_FAILED",
            "Check agent ID and try again",
            vec![format!("plico(agent=\"list\")")],
        ),
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

pub fn cmd_agent_checkpoint(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.checkpoint_agent(&agent_id) {
        Ok(cid) => {
            let mut r = ApiResponse::ok();
            r.data = Some(cid);
            r
        }
        Err(e) => ApiResponse::error(e),
    }
}

pub fn cmd_agent_restore(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let cid = match extract_arg(args, "--cid") {
        Some(c) => c,
        None => return ApiResponse::error("--cid required".to_string()),
    };
    match kernel.restore_agent_checkpoint(&agent_id, &cid) {
        Ok(count) => {
            let mut r = ApiResponse::ok();
            r.data = Some(format!("{} entries restored", count));
            r
        }
        Err(e) => ApiResponse::error(e),
    }
}

pub fn cmd_quota(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let name_or_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    // B21 fix: resolve name to UUID before calling agent_usage
    let resolved_id = match kernel.resolve_agent(&name_or_id) {
        Some(id) => id,
        None => return ApiResponse::error(format!("Agent not found: {}", name_or_id)),
    };
    match kernel.agent_usage(&resolved_id) {
        Some(usage) => {
            let mut r = ApiResponse::ok();
            r.agent_usage = Some(usage);
            r
        }
        None => ApiResponse::error(format!("Agent not found: {}", name_or_id)),
    }
}

pub fn cmd_discover(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let state_filter = extract_arg(args, "--state");
    let tool_filter = extract_arg(args, "--tool");
    let cards = kernel.discover_agents(
        state_filter.as_deref(),
        tool_filter.as_deref(),
    );
    let mut r = ApiResponse::ok();
    r.agent_cards = Some(cards);
    r
}

pub fn cmd_delegate(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let from = extract_arg(args, "--from").unwrap_or_else(|| "cli".to_string());
    let to = match extract_arg(args, "--to") {
        Some(t) => t,
        None => return ApiResponse::error("--to required"),
    };
    // F-4: resolve agent name to UUID if necessary
    let to_id = kernel.resolve_agent(&to).unwrap_or(to);
    let desc = extract_arg(args, "--desc").unwrap_or_else(|| "delegated task".to_string());
    let action = extract_arg(args, "--action");
    let priority_str = extract_arg(args, "--priority").unwrap_or_else(|| "medium".to_string());
    let priority = match priority_str.to_lowercase().as_str() {
        "critical" => plico::scheduler::IntentPriority::Critical,
        "high" => plico::scheduler::IntentPriority::High,
        "medium" => plico::scheduler::IntentPriority::Medium,
        _ => plico::scheduler::IntentPriority::Low,
    };
    match kernel.delegate_task(&from, &to_id, desc, action, priority) {
        Ok((intent_id, msg_id)) => {
            let mut r = ApiResponse::ok();
            r.delegation = Some(plico::api::semantic::DelegationResultDto {
                intent_id, message_id: msg_id, from, to: to_id,
            });
            r
        }
        Err(e) => ApiResponse::error(e),
    }
}
