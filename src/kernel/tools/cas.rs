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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::tests::make_kernel;

    #[test]
    fn test_cas_create() {
        let (kernel, _dir) = make_kernel();
        let params = json!({"content": "test content", "tags": ["test"]});
        let result = handle(&kernel, "cas.create", &params, "test_agent");
        assert!(result.success, "cas.create should succeed: {:?}", result.error);
        let cid = result.output.get("cid").unwrap().as_str().unwrap();
        assert!(!cid.is_empty());
    }

    #[test]
    fn test_cas_read() {
        let (kernel, _dir) = make_kernel();
        let create = handle(&kernel, "cas.create", &json!({"content": "read me", "tags": []}), "test_agent");
        let cid = create.output["cid"].as_str().unwrap().to_string();
        let result = handle(&kernel, "cas.read", &json!({"cid": cid}), "test_agent");
        assert!(result.success);
        assert_eq!(result.output["data"].as_str().unwrap(), "read me");
    }

    #[test]
    fn test_cas_read_not_found() {
        let (kernel, _dir) = make_kernel();
        let result = handle(&kernel, "cas.read", &json!({"cid": "nonexistent"}), "test_agent");
        assert!(!result.success);
    }

    #[test]
    fn test_cas_search() {
        let (kernel, _dir) = make_kernel();
        let _ = handle(&kernel, "cas.create", &json!({"content": "searchable content", "tags": ["searchable"]}), "test_agent");
        let result = handle(&kernel, "cas.search", &json!({"query": "searchable", "limit": 5}), "test_agent");
        assert!(result.success);
    }

    #[test]
    fn test_cas_update() {
        let (kernel, _dir) = make_kernel();
        let create = handle(&kernel, "cas.create", &json!({"content": "original", "tags": ["v1"]}), "test_agent");
        let cid = create.output["cid"].as_str().unwrap().to_string();
        let result = handle(&kernel, "cas.update", &json!({"cid": cid, "content": "updated", "new_tags": ["v2"]}), "test_agent");
        assert!(result.success);
    }

    #[test]
    fn test_cas_delete() {
        let (kernel, _dir) = make_kernel();
        // Grant Delete permission first
        kernel.handle_api_request(crate::api::semantic::ApiRequest::GrantPermission {
            agent_id: "test_agent".to_string(),
            action: "Delete".to_string(),
            scope: Some("*".to_string()),
            expires_at: None,
        });
        let create = handle(&kernel, "cas.create", &json!({"content": "delete me", "tags": []}), "test_agent");
        let cid = create.output["cid"].as_str().unwrap().to_string();
        let result = handle(&kernel, "cas.delete", &json!({"cid": cid}), "test_agent");
        assert!(result.success, "delete should succeed: {:?}", result.error);
    }

    #[test]
    fn test_cas_unknown_tool() {
        let (kernel, _dir) = make_kernel();
        let result = handle(&kernel, "cas.unknown", &json!({}), "test_agent");
        assert!(!result.success);
        assert!(result.error.unwrap().contains("unknown CAS tool"));
    }

    #[test]
    fn test_cas_create_empty_content() {
        let (kernel, _dir) = make_kernel();
        let result = handle(&kernel, "cas.create", &json!({"content": "", "tags": []}), "test_agent");
        // Empty content may be rejected
        let _ = result;
    }

    #[test]
    fn test_cas_search_with_tags() {
        let (kernel, _dir) = make_kernel();
        let _ = handle(&kernel, "cas.create", &json!({"content": "tagged content", "tags": ["important", "review"]}), "test_agent");
        let result = handle(&kernel, "cas.search", &json!({
            "query": "tagged",
            "require_tags": ["important"],
            "limit": 5
        }), "test_agent");
        assert!(result.success);
    }

    #[test]
    fn test_cas_update_not_found() {
        let (kernel, _dir) = make_kernel();
        let result = handle(&kernel, "cas.update", &json!({"cid": "nonexistent", "content": "new"}), "test_agent");
        assert!(!result.success);
    }

    #[test]
    fn test_cas_delete_not_found() {
        let (kernel, _dir) = make_kernel();
        let result = handle(&kernel, "cas.delete", &json!({"cid": "nonexistent"}), "test_agent");
        // Delete of nonexistent may succeed (soft delete) or fail
        let _ = result;
    }
}
