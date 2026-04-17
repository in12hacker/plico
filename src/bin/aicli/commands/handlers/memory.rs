//! Memory tier commands.

use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;
use plico::memory::MemoryTier;
use super::extract_arg;

pub fn cmd_remember(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let content = extract_arg(args, "--content").unwrap_or_default();
    kernel.remember(&agent_id, content);
    println!("Remembered for agent: {}", agent_id);
    ApiResponse::ok()
}

pub fn cmd_recall(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let memories = kernel.recall(&agent_id);
    if memories.is_empty() {
        println!("No memories for agent: {}", agent_id);
    } else {
        for m in &memories {
            println!("[{:?}] {}", m.tier, m.content.display());
        }
    }
    ApiResponse::ok()
}

pub fn cmd_tags(kernel: &AIKernel, _args: &[String]) -> ApiResponse {
    let tags = kernel.list_tags();
    if tags.is_empty() {
        println!("No tags in filesystem.");
    } else {
        println!("All tags ({} total):", tags.len());
        for tag in &tags {
            println!("  - {}", tag);
        }
    }
    ApiResponse::ok()
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
    if kernel.memory_move(&agent_id, &entry_id, tier) {
        println!("Moved memory {} to {:?} tier", entry_id, tier);
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
    if kernel.memory_delete(&agent_id, &entry_id) {
        println!("Deleted memory: {}", entry_id);
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
