//! MCP Client tests — self-referential: uses plico-mcp as the MCP server.
//! Tests both the MCP-specific client and the protocol-agnostic ExternalToolProvider trait.

#[cfg(test)]
mod test {
    use std::sync::Arc;
    use crate::mcp::McpClient;
    use crate::tool::ExternalToolProvider;

    fn plico_mcp_bin() -> String {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        format!("{}/target/debug/plico-mcp", manifest_dir)
    }

    fn make_client() -> McpClient {
        let dir = tempfile::TempDir::new().unwrap();
        McpClient::new(
            &plico_mcp_bin(),
            &[],
            &[
                ("PLICO_ROOT", dir.path().to_str().unwrap()),
                ("EMBEDDING_BACKEND", "stub"),
            ],
        ).expect("failed to create MCP client")
    }

    #[test]
    fn client_discovers_server_info() {
        let client = make_client();
        assert_eq!(client.server_info().name, "plico-mcp");
        assert_eq!(client.server_info().version, "1.0.0");
    }

    #[test]
    fn client_discovers_tools() {
        let client = make_client();
        let tools = client.tools();
        assert_eq!(tools.len(), 3, "should have 3 tools: plico, plico_store, plico_skills");
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"plico"));
        assert!(names.contains(&"plico_store"));
        assert!(names.contains(&"plico_skills"));
    }

    #[test]
    fn client_put_and_read_roundtrip() {
        let client = make_client();
        // Use plico_store put
        let put_text = client.call_tool("plico_store", &serde_json::json!({
            "action": "put",
            "content": "MCP client test content",
            "tags": ["mcp-client-test"],
            "agent_id": "test"
        })).unwrap();
        let put_resp: serde_json::Value = serde_json::from_str(&put_text).unwrap();
        assert!(put_resp["ok"].as_bool().unwrap());
        let cid = put_resp["cid"].as_str().unwrap();

        // Use plico_store read
        let read_text = client.call_tool("plico_store", &serde_json::json!({
            "action": "read",
            "cid": cid,
            "agent_id": "test"
        })).unwrap();
        let read_resp: serde_json::Value = serde_json::from_str(&read_text).unwrap();
        assert_eq!(read_resp["data"].as_str().unwrap(), "MCP client test content");
    }

    #[test]
    fn client_search_finds_content() {
        let client = make_client();
        // Store via plico_store
        client.call_tool("plico_store", &serde_json::json!({
            "action": "put",
            "content": "Dijkstra shortest path algorithm weighted graph",
            "tags": ["plico:type:experience", "plico:module:graph"],
            "agent_id": "test"
        })).unwrap();

        // Search via plico action
        let text = client.call_tool("plico", &serde_json::json!({
            "action": "search",
            "agent_id": "test",
            "query": "Dijkstra weighted path"
        })).unwrap();
        let resp: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(!resp["results"].as_array().unwrap().is_empty());
    }

    #[test]
    fn client_unknown_tool_returns_error() {
        let client = make_client();
        let result = client.call_tool("nonexistent_tool", &serde_json::json!({}));
        assert!(result.is_err());
    }

    // ── ExternalToolProvider trait tests ──────────────────────────────────

    #[test]
    fn trait_provider_name_matches() {
        let client = make_client();
        let provider: &dyn ExternalToolProvider = &client;
        assert_eq!(provider.provider_name(), "plico-mcp");
    }

    #[test]
    fn trait_discover_tools_returns_descriptors() {
        let client = make_client();
        let provider: &dyn ExternalToolProvider = &client;
        let tools = provider.discover_tools();
        assert_eq!(tools.len(), 3);
        assert!(tools.iter().any(|t| t.name == "plico"));
    }

    #[test]
    fn trait_call_tool_succeeds() {
        let client = make_client();
        let provider: &dyn ExternalToolProvider = &client;

        // Use plico_store put
        let put_result = provider.call_tool("plico_store", &serde_json::json!({
            "action": "put",
            "content": "trait test data",
            "tags": ["trait-test"],
            "agent_id": "test"
        }));
        assert!(put_result.success, "ExternalToolProvider::call_tool failed: {:?}", put_result.error);
    }

    #[test]
    fn kernel_add_tool_provider_integration() {
        let client = make_client();
        let provider: Arc<dyn ExternalToolProvider> = Arc::new(client);

        let kernel = {
            let dir = tempfile::TempDir::new().unwrap();
            crate::kernel::AIKernel::new(dir.path().to_path_buf()).unwrap()
        };

        let names = kernel.add_tool_provider(provider, "ext");
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"ext.plico".to_string()));
        assert!(names.contains(&"ext.plico_store".to_string()));

        let tools = kernel.tool_registry.list();
        assert!(tools.iter().any(|t| t.name == "ext.plico"));

        let handler = kernel.tool_registry.get_handler("ext.plico_store").expect("handler should exist");
        let result = handler.execute(&serde_json::json!({
            "action": "put",
            "content": "kernel integration test",
            "tags": ["kernel-test"],
            "agent_id": "test-agent"
        }), "test-agent");
        assert!(result.success, "handler failed: {:?}", result.error);
    }
}
