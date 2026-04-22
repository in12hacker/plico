//! CLI handler for procedural skills (learned workflows).

use plico::api::semantic::ApiResponse;
use plico::kernel::AIKernel;
use plico::memory::MemoryContent;
use super::extract_arg;

pub fn cmd_skills(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    match args.get(1).map(|s| s.as_str()) {
        Some("list") => cmd_skills_list(kernel, args),
        Some("describe") => cmd_skills_describe(kernel, args),
        Some("register") => cmd_skills_register(kernel, args),
        Some("discover") => cmd_skills_discover(kernel, args),
        _ => {
            eprintln!("Usage: skills <list|describe|register|discover> [--agent ID] [NAME]");
            ApiResponse::error("unknown skills subcommand")
        }
    }
}

fn cmd_skills_list(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_input = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let agent_id = match kernel.resolve_agent(&agent_input) {
        Some(id) => id,
        None => return ApiResponse::error(format!("Agent not found: {}", agent_input)),
    };
    let entries = kernel.recall_procedural(&agent_id, "default", None);
    let mut r = ApiResponse::ok();
    let skills: Vec<serde_json::Value> = entries.iter().filter_map(|e| {
        if let MemoryContent::Procedure(p) = &e.content {
            Some(serde_json::json!({
                "name": p.name,
                "description": p.description,
                "steps_count": p.steps.len(),
                "learned_from": p.learned_from,
                "tags": e.tags,
            }))
        } else {
            None
        }
    }).collect();
    r.data = Some(format!("Skills ({} total):\n{}",
        skills.len(),
        skills.iter().map(|s| format!("  {} — {} ({} steps)",
            s["name"].as_str().unwrap_or("?"),
            s["description"].as_str().unwrap_or(""),
            s["steps_count"]
        )).collect::<Vec<_>>().join("\n")
    ));
    r
}

fn cmd_skills_describe(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_input = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let agent_id = match kernel.resolve_agent(&agent_input) {
        Some(id) => id,
        None => return ApiResponse::error(format!("Agent not found: {}", agent_input)),
    };
    let name = args.get(2).filter(|a| !a.starts_with('-'));
    let name_str = match name {
        Some(n) => n.as_str(),
        None => {
            return ApiResponse::error("Usage: skills describe <name> [--agent ID]");
        }
    };
    let entries = kernel.recall_procedural(&agent_id, "default", Some(name_str));
    if entries.is_empty() {
        return ApiResponse::error(format!("No skill named '{}' found for agent '{}'", name_str, agent_id));
    }
    let mut r = ApiResponse::ok();
    let mut output = String::new();
    for entry in &entries {
        if let MemoryContent::Procedure(p) = &entry.content {
            output.push_str(&format!("Skill: {}\n", p.name));
            output.push_str(&format!("Description: {}\n", p.description));
            output.push_str(&format!("Learned from: {}\n", p.learned_from));
            output.push_str(&format!("Tags: {:?}\n", entry.tags));
            output.push_str(&format!("Steps ({}):\n", p.steps.len()));
            for step in &p.steps {
                output.push_str(&format!("  {}. {} [action: {}] → {}\n",
                    step.step_number, step.description, step.action, step.expected_outcome));
            }
        }
    }
    r.data = Some(output);
    r
}

fn cmd_skills_register(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_input = match extract_arg(args, "--agent") {
        Some(a) => a,
        None => return ApiResponse::error("--agent required for skills register"),
    };
    let agent_id = match kernel.resolve_agent(&agent_input) {
        Some(id) => id,
        None => return ApiResponse::error(format!("Agent not found: {}", agent_input)),
    };
    let name = match extract_arg(args, "--name") {
        Some(n) => n,
        None => return ApiResponse::error("--name required for skills register"),
    };
    let description = extract_arg(args, "--description").unwrap_or_default();
    let tags: Vec<String> = extract_arg(args, "--tags")
        .map(|s| s.split(',').map(String::from).collect())
        .unwrap_or_default();

    let req = plico::api::semantic::ApiRequest::RegisterSkill {
        agent_id,
        name,
        description,
        tags,
    };
    kernel.handle_api_request(req)
}

fn cmd_skills_discover(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let query = extract_arg(args, "--query");
    let agent_id_filter = extract_arg(args, "--agent").and_then(|input| {
        kernel.resolve_agent(&input)
    });
    let tag_filter = extract_arg(args, "--tag");

    let req = plico::api::semantic::ApiRequest::DiscoverSkills {
        query,
        agent_id_filter,
        tag_filter,
    };
    kernel.handle_api_request(req)
}
