//! Object CRUD commands.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiResponse, SearchResultDto};
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
    match kernel.get_object(&cid, &agent_id, "default") {
        Ok(obj) => {
            let mut r = ApiResponse::with_cid(obj.cid);
            r.tags = Some(obj.meta.tags);
            let mut text = String::new();
            text.push_str(&format!("Type: {}\n", obj.meta.content_type));
            if let Some(intent) = &obj.meta.intent {
                text.push_str(&format!("Intent: {}\n", intent));
            }
            text.push_str("---\n");
            text.push_str(&String::from_utf8_lossy(&obj.data));
            r.data = Some(text);
            r
        }
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

pub fn cmd_search(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let search_tags = extract_tags_opt(args, "--tags").unwrap_or_default();
    let mut query = if search_tags.is_empty() {
        extract_arg(args, "--query")
            .or_else(|| args.get(1).cloned().filter(|a| !a.starts_with("--")))
            .unwrap_or_default()
    } else {
        extract_arg(args, "--query").unwrap_or_default()
    };
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

// A-8a/F-4: tag-only search — handle require_tags AND semantics
    // Trigger when: no query text, but have search_tags OR require_tags
    // Only use positional arg as query if both tag sources are empty
    if query.is_empty() && search_tags.is_empty() && require_tags.is_empty() {
        // No tags at all — check if positional arg looks like a tag (starts with --)
        let positional = args.get(1).cloned
    if query.is_empty() && (!search_tags.is_empty() || !require_tags.is_empty()) {
        // F-4: require_tags uses AND semantics (all tags must match)
        let results = if !require_tags.is_empty() {
            kernel.search_by_tags_intersection(&require_tags, limit)
        } else {
            kernel.search_by_tags(&search_tags, limit)
        };
        let dto: Vec<SearchResultDto> = results.into_iter().map(|r| SearchResultDto {
            cid: r.cid, relevance: r.relevance, tags: r.meta.tags,
            snippet: r.snippet.clone(),
            content_type: r.meta.content_type.to_string(),
            created_at: r.meta.created_at,
        }).collect();
        let mut r = ApiResponse::ok();
        r.results = Some(dto);
        return r;
    }

    if query.is_empty() {
        eprintln!("Error: search requires a query. Use: search --query <text> or: search <text>");
        return ApiResponse::error("empty query");
    }

    let results = match kernel.semantic_search_with_time(
        &query, &agent_id, "default", limit, require_tags, exclude_tags, since, until,
    ) {
        Ok(r) => r,
        Err(e) => return ApiResponse::error(e.to_string()),
    };

    let dto: Vec<SearchResultDto> = results.into_iter().map(|r| SearchResultDto {
        cid: r.cid, relevance: r.relevance, tags: r.meta.tags,
        snippet: r.snippet.clone(),
        content_type: r.meta.content_type.to_string(),
        created_at: r.meta.created_at,
    }).collect();
    let mut r = ApiResponse::ok();
    r.results = Some(dto);
    r
}

pub fn cmd_update(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_default();
    let content = extract_arg(args, "--content").unwrap_or_default();
    let new_tags = extract_tags_opt(args, "--tags");
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.semantic_update(&cid, content.into_bytes(), new_tags, &agent_id, "default") {
        Ok(new_cid) => ApiResponse::with_cid(new_cid),
        Err(e) => ApiResponse::error(e.to_string()),
    }
}

pub fn cmd_delete(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_default();
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.semantic_delete(&cid, &agent_id, "default") {
        Ok(()) => ApiResponse::ok_with_message(format!("Deleted: {} → recycle bin", cid)),
        Err(e) => ApiResponse::error_with_diagnosis(
            e.to_string(),
            "DELETE_FAILED",
            "Check CID validity and permissions",
            vec!["plico(action=\"search\", query=\"...\")".to_string()],
        ),
    }
}

pub fn cmd_history(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_else(|| args.get(1).cloned().unwrap_or_default());
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    let chain = kernel.version_history(&cid, &agent_id);
    let mut r = ApiResponse::ok();
    r.data = Some(serde_json::to_string(&chain).unwrap_or_default());
    r
}

pub fn cmd_rollback(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_else(|| args.get(1).cloned().unwrap_or_default());
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    match kernel.rollback(&cid, &agent_id) {
        Ok(new_cid) => ApiResponse::with_cid(new_cid),
        Err(e) => ApiResponse::error(e),
    }
}
