//! Pure unit tests for the MCP client layer — no subprocess is spawned.
//!
//! Subprocess cross-validation against the real `plico-mcp` binary lives in
//! `tests/mcp_client_test.rs`: only integration targets receive Cargo's
//! official `CARGO_BIN_EXE_plico-mcp` location, which honors any
//! `CARGO_TARGET_DIR` and is built automatically as a test dependency.

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use serde_json::json;

    use crate::api::public::PUBLIC_OPERATIONS;
    use crate::kernel::AIKernel;
    use crate::tool::{ExternalToolProvider, ToolDescriptor, ToolResult};

    /// In-memory `ExternalToolProvider` exposing exactly the public catalog
    /// shape, so the kernel wiring is tested without a real subprocess and
    /// without exposing any new production API.
    struct FakeCatalogProvider;

    impl ExternalToolProvider for FakeCatalogProvider {
        fn provider_name(&self) -> &str {
            "fake-catalog"
        }

        fn discover_tools(&self) -> Vec<ToolDescriptor> {
            PUBLIC_OPERATIONS
                .iter()
                .map(|name| ToolDescriptor {
                    name: (*name).to_string(),
                    description: format!("fake {name}"),
                    schema: json!({ "type": "object" }),
                })
                .collect()
        }

        fn call_tool(&self, name: &str, _params: &serde_json::Value) -> ToolResult {
            ToolResult::ok(json!({ "echo": name }))
        }
    }

    #[test]
    fn kernel_add_tool_provider_uses_typed_names() {
        let provider: Arc<dyn ExternalToolProvider> = Arc::new(FakeCatalogProvider);
        let root = tempfile::TempDir::new().unwrap();
        let kernel = AIKernel::new(root.path().to_path_buf()).unwrap();

        let names = kernel.add_tool_provider(provider, "ext");
        assert_eq!(names.len(), PUBLIC_OPERATIONS.len());
        assert!(names.contains(&"ext.object.put".to_string()));
        assert!(names.contains(&"ext.memory.recall".to_string()));

        let handler = kernel
            .tool_registry
            .get_handler("ext.object.put")
            .expect("typed external handler should exist");
        let result = handler.execute(
            &json!({
                "content": "kernel integration test",
                "tags": ["kernel-test"],
            }),
            "local-cognitive-role",
        );
        assert!(result.success, "handler failed: {:?}", result.error);
    }
}
