//! CAS (Content-Addressed Storage) tool handlers.

use crate::kernel::AIKernel;
use crate::tool::ToolResult;
use serde_json::json;

pub(in crate::kernel) fn handle(kernel: &AIKernel, name: &str, params: &serde_json::Value, agent_id: &str) -> ToolResult {
    match name {
        "cas.create" => {
            let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let tags: Vec<String> = params.get("tags")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let intent = params.get("intent").and_then(|v| v.as_str()).map(String::from);
            match kernel.semantic_create(content.as_bytes().to_vec(), tags, agent_id, intent) {
                Ok(cid) => ToolResult::ok(json!({"cid": cid})),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        "cas.read" => {
            let cid = params.get("cid").and_then(|v| v.as_str()).unwrap_or("");
            match kernel.get_object(cid, agent_id, "default") {
                Ok(obj) => ToolResult::ok(json!({
                    "cid": obj.cid,
                    "data": String::from_utf8_lossy(&obj.data),
                    "tags": obj.meta.tags,
                    "content_type": obj.meta.content_type,
                })),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        "cas.search" => {
            let query = params.get("query").and_then(|v| v.as_str()).unwrap_or("");
            let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
            let require_tags: Vec<String> = params.get("require_tags")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let exclude_tags: Vec<String> = params.get("exclude_tags")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
                .unwrap_or_default();
            let since = params.get("since").and_then(|v| v.as_i64());
            let until = params.get("until").and_then(|v| v.as_i64());
            let mut results = match kernel.semantic_search_with_time(
                crate::kernel::ops::fs::SearchQuery {
                    query, agent_id, tenant_id: "default", limit: limit * 2,
                    require_tags, exclude_tags,
                },
                since, until,
            ) {
                Ok(r) => r,
                Err(e) => return ToolResult::error(e.to_string()),
            };
            let mut seen = std::collections::HashSet::new();
            results.retain(|r| seen.insert(r.cid.clone()));
            results.truncate(limit);
            let dto: Vec<serde_json::Value> = results.into_iter().map(|r| json!({
                "cid": r.cid, "relevance": r.relevance, "tags": r.meta.tags,
            })).collect();
            ToolResult::ok(json!({"results": dto}))
        }
        "cas.update" => {
            let cid = params.get("cid").and_then(|v| v.as_str()).unwrap_or("");
            let content = params.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let new_tags: Option<Vec<String>> = params.get("new_tags")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect());
            match kernel.semantic_update(cid, content.as_bytes().to_vec(), new_tags, agent_id, "default") {
                Ok(new_cid) => ToolResult::ok(json!({"cid": new_cid})),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        "cas.delete" => {
            let cid = params.get("cid").and_then(|v| v.as_str()).unwrap_or("");
            match kernel.semantic_delete(cid, agent_id, "default") {
                Ok(()) => ToolResult::ok(json!({"deleted": cid})),
                Err(e) => ToolResult::error(e.to_string()),
            }
        }
        _ => ToolResult::error(format!("unknown CAS tool: {}", name)),
    }
}
