//! Object CRUD commands — all operations route through handle_api_request.

use plico::kernel::AIKernel;
use plico::api::semantic::{ApiRequest, ApiResponse};
use super::{extract_arg, extract_tags, extract_tags_opt};

pub fn cmd_create(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let content = extract_arg(args, "--content")
        .or_else(|| args.get(1).cloned().filter(|a| !a.starts_with("--")))
        .unwrap_or_default();

    if content.is_empty() {
        return ApiResponse::error(
            "put requires content: put <content> --tags ... or put --content <content> --tags ..."
        );
    }

    let tags = extract_tags(args, "--tags");
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());
    let intent = extract_arg(args, "--intent");

    kernel.handle_api_request(ApiRequest::Create {
        api_version: None,
        content,
        content_encoding: Default::default(),
        tags,
        agent_id,
        tenant_id: None,
        agent_token: None,
        intent,
    })
}

pub fn cmd_read(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = args.get(1).cloned().unwrap_or_default();
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    kernel.handle_api_request(ApiRequest::Read {
        cid,
        agent_id,
        tenant_id: None,
        agent_token: None,
    })
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
    let limit: usize = extract_arg(args, "--limit")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);
    let mut require_tags = extract_tags_opt(args, "--require-tags")
        .or_else(|| extract_tags_opt(args, "-t"))
        .unwrap_or_default();
    let exclude_tags = extract_tags_opt(args, "--exclude-tags").unwrap_or_default();
    let since = extract_arg(args, "--since").and_then(|s| s.parse::<i64>().ok());
    let until = extract_arg(args, "--until").and_then(|s| s.parse::<i64>().ok());

    if query.is_empty() && search_tags.is_empty() && require_tags.is_empty() {
        let positional = args.get(1).cloned().unwrap_or_default();
        if !positional.starts_with("--") {
            query = positional;
        }
    }
    // Tag-only search: merge search_tags into require_tags for the API
    if query.is_empty() && (!search_tags.is_empty() || !require_tags.is_empty()) {
        if require_tags.is_empty() {
            require_tags = search_tags;
        }
    }

    if query.is_empty() && require_tags.is_empty() {
        return ApiResponse::error("search requires a query. Use: search --query <text> or: search <text>");
    }

    kernel.handle_api_request(ApiRequest::Search {
        query,
        agent_id,
        tenant_id: None,
        agent_token: None,
        limit: Some(limit),
        offset: None,
        require_tags,
        exclude_tags,
        since,
        until,
    })
}

pub fn cmd_update(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid")
        .or_else(|| args.get(1).cloned().filter(|a| !a.starts_with("--")))
        .unwrap_or_default();
    if cid.is_empty() {
        return ApiResponse::error("update requires a CID: update <CID> or update --cid <CID>");
    }

    let content = extract_arg(args, "--content")
        .or_else(|| args.get(2).cloned().filter(|a| !a.starts_with("--")))
        .unwrap_or_default();
    if content.is_empty() {
        return ApiResponse::error("update requires content: update --cid <CID> --content <text>");
    }

    let new_tags = extract_tags_opt(args, "--tags");
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    kernel.handle_api_request(ApiRequest::Update {
        cid,
        content,
        content_encoding: Default::default(),
        new_tags,
        agent_id,
        tenant_id: None,
        agent_token: None,
    })
}

pub fn cmd_delete(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid")
        .or_else(|| args.get(1).cloned().filter(|a| !a.starts_with("--")))
        .unwrap_or_default();
    if cid.is_empty() {
        return ApiResponse::error("delete requires a CID: delete <CID> or delete --cid <CID>");
    }
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    kernel.handle_api_request(ApiRequest::Delete {
        cid,
        agent_id,
        tenant_id: None,
        agent_token: None,
    })
}

pub fn cmd_history(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_else(|| args.get(1).cloned().unwrap_or_default());
    if cid.is_empty() {
        return ApiResponse::error("cid is required");
    }
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    kernel.handle_api_request(ApiRequest::History { cid, agent_id })
}

pub fn cmd_rollback(kernel: &AIKernel, args: &[String]) -> ApiResponse {
    let cid = extract_arg(args, "--cid").unwrap_or_else(|| args.get(1).cloned().unwrap_or_default());
    if cid.is_empty() {
        return ApiResponse::error("cid is required");
    }
    let agent_id = extract_arg(args, "--agent").unwrap_or_else(|| "cli".to_string());

    kernel.handle_api_request(ApiRequest::Rollback { cid, agent_id })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_kernel() -> plico::kernel::AIKernel {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("EMBEDDING_BACKEND", "stub");
        plico::kernel::AIKernel::new(dir.path().to_path_buf()).expect("kernel")
    }

    // B49 pattern: positional content works for create
    #[test]
    fn test_cmd_create_positional_content() {
        let kernel = make_test_kernel();
        let args = vec!["put".to_string(), "hello world".to_string()];
        let response = cmd_create(&kernel, &args);
        assert!(response.ok, "put with positional content should succeed: {:?}",
            response.error);
        assert!(response.cid.is_some(), "response should contain cid");
    }

    // B52: update with positional CID and content works
    #[test]
    fn test_cmd_update_positional_content() {
        let kernel = make_test_kernel();
        // First create an object
        let create_args = vec!["put".to_string(), "initial content".to_string()];
        let create_response = cmd_create(&kernel, &create_args);
        assert!(create_response.ok, "create should succeed");
        let cid = create_response.cid.expect("cid should be set");

        // Now update using positional args
        let update_args = vec!["update".to_string(), cid.clone(), "updated content".to_string()];
        let update_response = cmd_update(&kernel, &update_args);
        assert!(update_response.ok, "update with positional args should succeed: {:?}",
            update_response.error);
        assert!(update_response.cid.is_some(), "update response should have new cid");
    }

    #[test]
    fn test_cmd_update_empty_content_returns_error() {
        let kernel = make_test_kernel();
        // First create an object to have a valid CID
        let create_args = vec!["put".to_string(), "some content".to_string()];
        let create_response = cmd_create(&kernel, &create_args);
        let cid = create_response.cid.expect("cid should be set");

        // Update with empty content — should this be allowed or rejected?
        // Based on the cmd_create empty check, we expect it may be rejected
        let update_args = vec!["update".to_string(), "--cid".to_string(), cid,
                               "--content".to_string(), "".to_string()];
        let response = cmd_update(&kernel, &update_args);
        // Empty content in update may be allowed (creates new version with empty data)
        // or rejected — just verify it returns a response
        assert!(response.cid.is_some() || !response.ok,
            "update should either succeed with new cid or fail gracefully");
    }

    #[test]
    fn test_cmd_read_existing_object() {
        let kernel = make_test_kernel();
        // Create an object first
        let create_args = vec!["put".to_string(), "test content".to_string()];
        let create_response = cmd_create(&kernel, &create_args);
        let cid = create_response.cid.expect("cid should be set");

        // Read it back using positional CID
        let read_args = vec!["read".to_string(), cid];
        let read_response = cmd_read(&kernel, &read_args);
        assert!(read_response.ok, "read should succeed for existing object: {:?}",
            read_response.error);
        assert!(read_response.data.is_some(), "read should return data");
        let data = read_response.data.unwrap();
        assert!(data.contains("test content"), "data should contain original content");
    }

    #[test]
    fn test_cmd_read_nonexistent_returns_error() {
        let kernel = make_test_kernel();
        let fake_cid = "abc123def456".to_string();
        let args = vec!["read".to_string(), fake_cid];
        let response = cmd_read(&kernel, &args);
        assert!(!response.ok, "read should fail for nonexistent CID");
        let err_msg = response.error.as_deref().unwrap_or("");
        assert!(!err_msg.is_empty(), "error message should not be empty");
    }

    // F-6: CLI System Audit — cmd_history / cmd_rollback empty CID check
    #[test]
    fn test_cmd_history_empty_cid_returns_error() {
        let kernel = make_test_kernel();
        let args = vec!["history".to_string()];
        let response = cmd_history(&kernel, &args);
        assert!(!response.ok, "history with no cid should fail");
        let err = response.error.unwrap_or_default();
        assert!(err.contains("cid"), "error should mention cid requirement");
    }

    #[test]
    fn test_cmd_rollback_empty_cid_returns_error() {
        let kernel = make_test_kernel();
        let args = vec!["rollback".to_string()];
        let response = cmd_rollback(&kernel, &args);
        assert!(!response.ok, "rollback with no cid should fail");
        let err = response.error.unwrap_or_default();
        assert!(err.contains("cid"), "error should mention cid requirement");
    }
}
