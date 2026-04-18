//! External tool provider integration — protocol-agnostic.
//!
//! The kernel doesn't know about MCP, Agent Skills, or A2A.
//! It only knows about `ExternalToolProvider`. When a protocol
//! becomes obsolete, delete its adapter. When a new one emerges,
//! add one. This file never changes.

use std::sync::Arc;

use crate::kernel::AIKernel;
use crate::tool::{ExternalToolProvider, ToolHandler, ToolResult};

impl AIKernel {
    /// Register an external tool provider and expose its tools through PWP.
    ///
    /// Discovers tools from the provider, registers each in the ToolRegistry
    /// with a `{prefix}.{tool_name}` qualified name and a handler that
    /// delegates back to the provider.
    ///
    /// After this call, agents discover these tools via `tool_list` and
    /// invoke them via `tool_call` — standard PWP, protocol-transparent.
    pub fn add_tool_provider(
        &self,
        provider: Arc<dyn ExternalToolProvider>,
        prefix: &str,
    ) -> Vec<String> {
        let tools = provider.discover_tools();

        let tool_names: Vec<String> = tools.iter()
            .map(|t| format!("{}.{}", prefix, t.name))
            .collect();

        tracing::info!(
            "External tool provider '{}': {} tools registered with prefix '{}'",
            provider.provider_name(),
            tool_names.len(),
            prefix,
        );

        for tool in tools {
            let qualified_name = format!("{}.{}", prefix, tool.name);
            let desc = crate::tool::ToolDescriptor {
                name: qualified_name,
                description: format!("[{}] {}", provider.provider_name(), tool.description),
                schema: tool.schema,
            };
            let handler = ProviderToolHandler {
                provider: Arc::clone(&provider),
                tool_name: tool.name,
            };
            self.tool_registry.register_with_handler(desc, Arc::new(handler));
        }

        tool_names
    }
}

struct ProviderToolHandler {
    provider: Arc<dyn ExternalToolProvider>,
    tool_name: String,
}

impl ToolHandler for ProviderToolHandler {
    fn execute(&self, params: &serde_json::Value, _agent_id: &str) -> ToolResult {
        self.provider.call_tool(&self.tool_name, params)
    }
}
