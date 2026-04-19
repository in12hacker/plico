//! Memory tier commands.

use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;
use plico::memory::MemoryTier;
use super::extract_arg;

pub fn cmd_remember(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let content = extract_arg(args, "--content").unwrap_or_default();
    let tier_str = extract_arg(args, "--tier").unwrap_or_default();
    let tags: Vec<String> = extract_arg(args, "--tags")
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();
    let result = match parse_memory_tier(&tier_str) {
        MemoryTier::Working => kernel.remember_working(&agent_id, "default", content, tags),
        MemoryTier::LongTerm => kernel.remember_long_term(&agent_id, "default", content, tags, 50),
        _ => kernel.remember(&agent_id, "default", content),
    };
    match result {
        Ok(()) => ApiResponse::ok(),
        Err(e) => ApiResponse::error(e),
    }
}

pub fn cmd_recall(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let memories = kernel.recall(&agent_id, "default");
    let strings: Vec<String> = memories.iter().map(|m| format!("[{:?}] {}", m.tier, m.content.display())).collect();
    let mut r = ApiResponse::ok();
    r.memory = Some(strings);
    r
}

pub fn cmd_tags(kernel: &AIKernel, _args: &[String]) -> ApiResponse {
    let tags = kernel.list_tags();
    let mut r = ApiResponse::ok();
    r.tags = Some(tags);
    r
}

pub fn cmd_memmove(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let entry_id = match extract_arg(args, "--id") {
        Some(id) => id,
        None => {
            eprintln!("Usage: memmove --id <entry-id> --tier <ephemeral|working|longterm|procedural>");
            return ApiResponse::error("memmove requires --id and --tier");
        }
    };
    let tier_str = extract_arg(args, "--tier").unwrap_or_default();
    let tier = parse_memory_tier(&tier_str);
    if kernel.memory_move(&agent_id, "default", &entry_id, tier) {
        ApiResponse::ok()
    } else {
        ApiResponse::error(format!("Memory entry not found: {}", entry_id))
    }
}

pub fn cmd_memdelete(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let entry_id = match extract_arg(args, "--id") {
        Some(id) => id,
        None => {
            eprintln!("Usage: memdelete --id <entry-id>");
            return ApiResponse::error("memdelete requires --id");
        }
    };
    if kernel.memory_delete(&agent_id, "default", &entry_id) {
        ApiResponse::ok()
    } else {
        ApiResponse::error(format!("Memory entry not found: {}", entry_id))
    }
}

pub fn parse_memory_tier(s: &str) -> MemoryTier {
    match s.to_lowercase().as_str() {
        "ephemeral" | "l0" | "ephem" => MemoryTier::Ephemeral,
        "working" | "l1" | "wk" => MemoryTier::Working,
        "longterm" | "l2" | "lt" | "long" => MemoryTier::LongTerm,
        "procedural" | "l3" | "proc" => MemoryTier::Procedural,
        _ => MemoryTier::Working,
    }
}
