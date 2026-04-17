//! Object CRUD commands.

use plico::kernel::AIKernel;
use plico::api::semantic::ApiResponse;
use super::{extract_arg, extract_tags, extract_tags_opt};

pub fn cmd_create(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let content = extract_arg(args, "--content").unwrap_or_default();
    let tags = extract_tags(args, "--tags");
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let intent = extract_arg(args, "--intent");

    match kernel.semantic_create(content.into_bytes(), tags, &agent_id, intent) {
        Ok(cid) => ApiResponse::with_cid(cid),
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

pub fn cmd_read(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = args.get(1).cloned().unwrap_or_default();
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    match kernel.get_object(&cid, &agent_id) {
        Ok(obj) => {
            println!("CID: {}", obj.cid);
            println!("Tags: {:?}", obj.meta.tags);
            println!("Type: {}", obj.meta.content_type);
            if let Some(intent) = obj.meta.intent {
                println!("Intent: {}", intent);
            }
            println!("---");
            println!("{}", String::from_utf8_lossy(&obj.data));
            ApiResponse::ok()
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

pub fn cmd_search(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let query = extract_arg(args, "--query")
        .or_else(|| args.get(1).cloned())
        .unwrap_or_default();
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let limit = extract_arg(args, "--limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let require_tags = extract_tags_opt(args, "--require-tags")
        .or_else(|| extract_tags_opt(args, "-t"))
        .unwrap_or_default();
    let exclude_tags = extract_tags_opt(args, "--exclude-tags").unwrap_or_default();
    let since = extract_arg(args, "--since").and_then(|s| s.parse::<i64>().ok());
    let until = extract_arg(args, "--until").and_then(|s| s.parse::<i64>().ok());

    if query.is_empty() {
        eprintln!("Error: search requires a query. Use: search --query <text> or: search <text>");
        return ApiResponse::error("empty query");
    }

    let results = kernel.semantic_search_with_time(
        &query, &agent_id, limit, require_tags, exclude_tags, since, until,
    );

    if results.is_empty() {
        println!("No results for: {}", query);
    } else {
        for (i, r) in results.iter().enumerate() {
            println!("{}. [relevance={:.2}] {}", i + 1, r.relevance, r.cid);
            println!("   Tags: {:?}", r.meta.tags);
            if let Some(intent) = &r.meta.intent {
                println!("   Intent: {}", intent);
            }
        }
    }
    ApiResponse::ok()
}

pub fn cmd_update(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_default();
    let content = extract_arg(args, "--content").unwrap_or_default();
    let new_tags = extract_tags_opt(args, "--tags");
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.semantic_update(&cid, content.into_bytes(), new_tags, &agent_id) {
        Ok(new_cid) => {
            println!("Updated. Old CID: {}", cid);
            println!("New CID: {}", new_cid);
            ApiResponse::with_cid(new_cid)
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

pub fn cmd_delete(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_default();
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.semantic_delete(&cid, &agent_id) {
        Ok(()) => {
            println!("Deleted (logical): {}", cid);
            ApiResponse::ok()
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}
