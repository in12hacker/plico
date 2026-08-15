//! MCP client cross-validation against the typed `plico-mcp` server.

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use base64::Engine;

    use crate::api::public::PUBLIC_OPERATIONS;
    use crate::mcp::McpClient;
    use crate::tool::ExternalToolProvider;

    struct TestClient {
        client: McpClient,
        _root: tempfile::TempDir,
    }

    fn plico_mcp_bin() -> String {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        format!("{manifest_dir}/target/debug/plico-mcp")
    }

    fn make_client() -> TestClient {
        let root = tempfile::TempDir::new().unwrap();
        let client = McpClient::new(
            &plico_mcp_bin(),
            &[],
            &[
                ("PLICO_ROOT", root.path().to_str().unwrap()),
                ("EMBEDDING_BACKEND", "stub"),
                ("LLM_BACKEND", "stub"),
            ],
        )
        .expect("failed to create MCP client");
        TestClient { client, _root: root }
    }

    fn call_json(client: &McpClient, name: &str, input: serde_json::Value) -> serde_json::Value {
        let text = client.call_tool(name, &input).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn client_discovers_server_info() {
        let fixture = make_client();
        assert_eq!(fixture.client.server_info().name, "plico-mcp");
        assert_eq!(fixture.client.server_info().version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn client_discovers_exact_public_tool_catalog() {
        let fixture = make_client();
        let names: Vec<&str> = fixture.client.tools().iter().map(|tool| tool.name.as_str()).collect();
        assert_eq!(names, PUBLIC_OPERATIONS);
    }

    #[test]
    fn client_put_and_read_roundtrip() {
        let fixture = make_client();
        let put = call_json(
            &fixture.client,
            "object.put",
            serde_json::json!({
                "content": "MCP client test content",
                "tags": ["mcp-client-test"],
            }),
        );
        assert_eq!(put["ok"], true);
        let cid = put["data"]["result"]["cid"].as_str().unwrap();

        let read = call_json(&fixture.client, "object.get", serde_json::json!({ "cid": cid }));
        let encoded = read["data"]["result"]["content_base64"].as_str().unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD.decode(encoded).unwrap(),
            b"MCP client test content"
        );
    }

    #[test]
    fn client_search_api_returns_typed_diagnostics() {
        let fixture = make_client();
        let put = call_json(
            &fixture.client,
            "object.put",
            serde_json::json!({
                "content": "Dijkstra shortest path algorithm weighted graph",
                "tags": ["experience", "graph"],
            }),
        );
        let cid = put["data"]["result"]["cid"].as_str().unwrap();

        let search = call_json(
            &fixture.client,
            "object.search",
            serde_json::json!({ "query": "Dijkstra weighted path" }),
        );
        assert_eq!(search["ok"], true);
        assert!(search["data"]["result"]["hits"].is_array());
        assert!(search["data"]["result"]["retrieval"].is_array());
        assert!(search["data"]["result"]["embedding_query"].is_object());

        let read = call_json(&fixture.client, "object.get", serde_json::json!({ "cid": cid }));
        assert_eq!(read["ok"], true);
    }

    #[test]
    fn client_unknown_tool_returns_error() {
        let fixture = make_client();
        assert!(fixture
            .client
            .call_tool("nonexistent_tool", &serde_json::json!({}))
            .is_err());
    }

    #[test]
    fn trait_provider_discovers_exact_descriptors() {
        let fixture = make_client();
        let provider: &dyn ExternalToolProvider = &fixture.client;
        assert_eq!(provider.provider_name(), "plico-mcp");
        let names: Vec<String> = provider.discover_tools().into_iter().map(|tool| tool.name).collect();
        assert_eq!(names, PUBLIC_OPERATIONS);
    }

    #[test]
    fn trait_call_tool_succeeds() {
        let fixture = make_client();
        let provider: &dyn ExternalToolProvider = &fixture.client;
        let result = provider.call_tool(
            "object.put",
            &serde_json::json!({
                "content": "trait test data",
                "tags": ["trait-test"],
            }),
        );
        assert!(result.success, "tool call failed: {:?}", result.error);
    }

    #[test]
    fn kernel_add_tool_provider_uses_typed_names() {
        let fixture = make_client();
        let provider: Arc<dyn ExternalToolProvider> = Arc::new(fixture.client);
        let root = tempfile::TempDir::new().unwrap();
        let kernel = crate::kernel::AIKernel::new(root.path().to_path_buf()).unwrap();

        let names = kernel.add_tool_provider(provider, "ext");
        assert_eq!(names.len(), PUBLIC_OPERATIONS.len());
        assert!(names.contains(&"ext.object.put".to_string()));
        assert!(names.contains(&"ext.memory.recall".to_string()));

        let handler = kernel
            .tool_registry
            .get_handler("ext.object.put")
            .expect("typed MCP handler should exist");
        let result = handler.execute(
            &serde_json::json!({
                "content": "kernel integration test",
                "tags": ["kernel-test"],
            }),
            "local-cognitive-role",
        );
        assert!(result.success, "handler failed: {:?}", result.error);
    }
}
