//! Memory tier commands.

use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;
use plico::memory::{MemoryTier, layered::ProcedureStep};
use super::extract_arg;

pub fn cmd_remember(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let content = extract_arg(args, "--content").unwrap_or_default();
    let tier_str = extract_arg(args, "--tier").unwrap_or_default();
    let tags: Vec<String> = extract_arg(args, "--tags")
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();
    match parse_memory_tier(&tier_str) {
        MemoryTier::Ephemeral => {
            // INV-1: Warn about CLI ephemeral persistence limitation
            eprintln!("Warning: ephemeral memory is stored in-process only and will not persist across CLI command boundaries. Use --tier working or --tier long-term for persistence.");
            match kernel.remember(&agent_id, "default", content) {
                Ok(_) => ApiResponse::ok_with_message(format!("Memory stored for agent '{}'", agent_id)),
                Err(e) => ApiResponse::error(e),
            }
        }
        MemoryTier::Working => {
            let tags_clone = tags.clone();
            match kernel.remember_working(&agent_id, "default", content, tags_clone) {
                Ok(_) => ApiResponse::ok_with_message(format!("Memory stored for agent '{}'", agent_id)),
                Err(e) => ApiResponse::error(e),
            }
        }
        MemoryTier::LongTerm => {
            let tags_clone = tags.clone();
            match kernel.remember_long_term(&agent_id, "default", content, tags_clone, 50) {
                Ok(entry_id) => {
                    // F-5: Memory-KG Binding — link memory to KG for all tiers
                    kernel.link_memory_to_kg(&entry_id, &agent_id, "default", &tags);
                    ApiResponse::ok_with_message(format!("Memory stored for agent '{}'", agent_id))
                }
                Err(e) => ApiResponse::error(e),
            }
        }
        MemoryTier::Procedural => {
            // F-B fix: route to remember_procedural instead of generic remember
            let procedure_steps = vec![ProcedureStep {
                step_number: 0,
                description: content.clone(),
                action: content.clone(),
                expected_outcome: String::new(),
            }];
            match kernel.remember_procedural(
                &agent_id,
                "default",
                "cli-procedure".to_string(),
                content,
                procedure_steps,
                "cli".to_string(),
                tags.clone(),
            ) {
                Ok(entry_id) => {
                    // F-5: Memory-KG Binding — link procedural memory to KG
                    kernel.link_memory_to_kg(&entry_id, &agent_id, "default", &tags);
                    ApiResponse::ok_with_message(format!("Procedural memory stored for agent '{}'", agent_id))
                }
                Err(e) => ApiResponse::error(e),
            }
        }
    }
}

pub fn cmd_recall(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let tier_filter = extract_arg(args, "--tier").map(|s| parse_memory_tier(&s));
    let memories = kernel.recall(&agent_id, "default");
    let filtered: Vec<_> = match tier_filter {
        Some(tier) => memories.into_iter().filter(|m| m.tier == tier).collect(),
        None => memories,
    };
    let strings: Vec<String> = filtered.iter().map(|m| format!("[{:?}] {}", m.tier, m.content.display())).collect();
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
        ApiResponse::ok_with_message(format!("Memory entry {} moved to {:?}", entry_id, tier))
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
        ApiResponse::ok_with_message(format!("Memory entry {} deleted", entry_id))
    } else {
        ApiResponse::error(format!("Memory entry not found: {}", entry_id))
    }
}

pub fn parse_memory_tier(s: &str) -> MemoryTier {
    match s.to_lowercase().replace(['-', '_'], "").as_str() {
        "ephemeral" | "l0" | "ephem" => MemoryTier::Ephemeral,
        "working" | "l1" | "wk" => MemoryTier::Working,
        "longterm" | "l2" | "lt" | "long" => MemoryTier::LongTerm,
        "procedural" | "l3" | "proc" => MemoryTier::Procedural,
        "" => MemoryTier::Working,
        other => {
            eprintln!("Warning: unknown tier '{}', defaulting to Working", other);
            MemoryTier::Working
        }
    }
}
