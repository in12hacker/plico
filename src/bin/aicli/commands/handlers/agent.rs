//! Agent management commands.

use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;
use super::extract_arg;

pub fn cmd_agent(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    if args.get(1).map(|s| s.as_str()) == Some("set-resources") {
        let target = args.get(2).cloned().unwrap_or_default();
        let mq = extract_arg(args, "--memory-quota").and_then(|s| s.parse().ok());
        let cq = extract_arg(args, "--cpu-time-quota").and_then(|s| s.parse().ok());
        let at = extract_arg(args, "--allowed-tools")
            .map(|s| s.split(',').map(String::from).collect::<Vec<_>>());
        return match kernel.agent_set_resources(&target, mq, cq, at) {
            Ok(()) => {
                println!("Resources updated for agent: {}", target);
                ApiResponse::ok()
            }
            Err(e) => ApiResponse::error(e.to_string()),
        };
    }

    let name = extract_arg(args, "--register").unwrap_or_else(|| "unnamed".to_string());
    let id = kernel.register_agent(name.clone());
    println!("Agent registered: {} (ID: {})", name, id);
    let mut r = ApiResponse::ok();
    r.agent_id = Some(id);
    r
}

pub fn cmd_agents(kernel: &AIKernel, _args: &[String]) -> ApiResponse {
    let agents = kernel.list_agents();
    if agents.is_empty() {
        println!("No active agents.");
    } else {
        for a in &agents {
            println!("- {} ({}) [{:?}]", a.name, a.id, a.state);
        }
    }
    ApiResponse::ok()
}

pub fn cmd_agent_status(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.agent_status(&agent_id) {
        Some((_id, state, pending)) => {
            println!("Agent: {}", agent_id);
            println!("State: {}", state);
            println!("Pending intents: {}", pending);
            let mut r = ApiResponse::ok();
            r.agent_state = Some(state);
            r.pending_intents = Some(pending);
            r
        }
        None => {
            println!("Agent not found: {}", agent_id);
            ApiResponse::error(format!("Agent not found: {}", agent_id))
        }
    }
}

pub fn cmd_agent_suspend(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.agent_suspend(&agent_id) {
        Ok(()) => { println!("Agent {} suspended", agent_id); ApiResponse::ok() }
        Err(e) => { println!("Error: {}", e); ApiResponse::error(e.to_string()) }
    }
}

pub fn cmd_agent_resume(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.agent_resume(&agent_id) {
        Ok(()) => { println!("Agent {} resumed", agent_id); ApiResponse::ok() }
        Err(e) => { println!("Error: {}", e); ApiResponse::error(e.to_string()) }
    }
}

pub fn cmd_agent_terminate(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.agent_terminate(&agent_id) {
        Ok(()) => { println!("Agent {} terminated", agent_id); ApiResponse::ok() }
        Err(e) => { println!("Error: {}", e); ApiResponse::error(e.to_string()) }
    }
}
