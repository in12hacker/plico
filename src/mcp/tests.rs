//! MCP Client tests — self-referential: uses plico-mcp as the MCP server.

#[cfg(test)]
mod test {
    use crate::mcp::McpClient;
    use crate::tool::ToolRegistry;

    fn plico_mcp_bin() -> String {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        format!("{}/target/debug/plico-mcp", manifest_dir)
    }

    fn make_client() -> McpClient {
        let dir = tempfile::TempDir::new().unwrap();
        // Use the compiled binary path — requires `cargo build --bin plico-mcp` first.
        // In test, cargo builds all bins automatically.
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
        assert_eq!(tools.len(), 5);
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"plico_search"));
        assert!(names.contains(&"plico_put"));
        assert!(names.contains(&"plico_read"));
        assert!(names.contains(&"plico_nodes"));
        assert!(names.contains(&"plico_tags"));
    }

    #[test]
    fn client_put_and_read_roundtrip() {
        let client = make_client();

        let put_text = client.call_tool("plico_put", &serde_json::json!({
            "content": "MCP client test content",
            "tags": ["mcp-client-test"]
        })).unwrap();

        let put_resp: serde_json::Value = serde_json::from_str(&put_text).unwrap();
        assert!(put_resp["ok"].as_bool().unwrap());
        let cid = put_resp["cid"].as_str().unwrap();

        let read_text = client.call_tool("plico_read", &serde_json::json!({
            "cid": cid
        })).unwrap();

        let read_resp: serde_json::Value = serde_json::from_str(&read_text).unwrap();
        assert_eq!(read_resp["data"].as_str().unwrap(), "MCP client test content");
    }

    #[test]
    fn client_search_finds_content() {
        let client = make_client();

        client.call_tool("plico_put", &serde_json::json!({
            "content": "Dijkstra shortest path algorithm weighted graph",
            "tags": ["plico:type:experience", "plico:module:graph"]
        })).unwrap();

        let text = client.call_tool("plico_search", &serde_json::json!({
            "query": "Dijkstra weighted path"
        })).unwrap();

        let resp: serde_json::Value = serde_json::from_str(&text).unwrap();
        let results = resp["results"].as_array().unwrap();
        assert!(!results.is_empty(), "search should find content via BM25");
    }

    #[test]
    fn client_tags_returns_list() {
        let client = make_client();

        client.call_tool("plico_put", &serde_json::json!({
            "content": "test data",
            "tags": ["plico:type:adr", "plico:module:kernel"]
        })).unwrap();

        let text = client.call_tool("plico_tags", &serde_json::json!({})).unwrap();
        let tags: Vec<String> = serde_json::from_str(&text).unwrap();
        assert!(tags.contains(&"plico:type:adr".to_string()));
    }

    #[test]
    fn client_unknown_tool_returns_error() {
        let client = make_client();
        let result = client.call_tool("nonexistent_tool", &serde_json::json!({}));
        assert!(result.is_err());
    }

    #[test]
    fn register_tools_adds_to_registry() {
        let client = make_client();
        let registry = ToolRegistry::new();

        client.register_tools(&registry, "mcp");
        assert_eq!(registry.count(), 5);
        assert!(registry.contains("mcp.plico_search"));
        assert!(registry.contains("mcp.plico_put"));
        assert!(registry.contains("mcp.plico_read"));
        assert!(registry.contains("mcp.plico_nodes"));
        assert!(registry.contains("mcp.plico_tags"));
    }

    #[test]
    fn registry_handler_calls_through_to_mcp() {
        let client = make_client();
        let registry = ToolRegistry::new();
        client.register_tools(&registry, "mcp");

        let handler = registry.get_handler("mcp.plico_put").expect("handler should exist");
        let result = handler.execute(&serde_json::json!({
            "content": "via registry handler",
            "tags": ["registry-test"]
        }), "test-agent");
        assert!(result.success, "handler should succeed: {:?}", result.error);

        let text = result.output["text"].as_str().unwrap();
        let resp: serde_json::Value = serde_json::from_str(text).unwrap();
        assert!(resp["ok"].as_bool().unwrap());
    }
}
