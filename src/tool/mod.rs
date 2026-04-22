//! Tool Abstraction — "Everything is a Tool"
//!
//! AIOS equivalent of Unix's "Everything is a File". All capabilities are
//! unified under the Tool abstraction. Agents discover and invoke tools
//! through a standard protocol, enabling extensibility without kernel changes.
//!
//! # Architecture
//!
//! ```text
//! ToolRegistry
//! ├── Built-in tools       — kernel methods exposed as tools (cas.*, memory.*, kg.*, agent.*)
//! └── ExternalToolProvider  — protocol-agnostic adapter (MCP, Agent Skills, A2A, future protocols)
//!     ├── McpToolProvider   — MCP JSON-RPC over stdio
//!     └── (future adapters) — Agent Skills, A2A, custom protocols
//! ```
//!
//! # Discovery Flow
//!
//! 1. Agent calls `tools.list` → gets all `ToolDescriptor`s
//! 2. Agent calls `tools.describe <name>` → gets schema for a specific tool
//! 3. Agent calls `tool_call <name> <params>` → kernel dispatches and returns result

pub mod registry;
pub mod procedure_provider;

use serde::{Deserialize, Serialize};

/// JSON Schema describing a tool's input parameters.
pub type ToolSchema = serde_json::Value;

/// Result of a tool execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub output: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ToolResult {
    pub fn ok(output: serde_json::Value) -> Self {
        Self { success: true, output, error: None }
    }

    pub fn error(msg: impl Into<String>) -> Self {
        Self { success: false, output: serde_json::Value::Null, error: Some(msg.into()) }
    }

    pub fn is_ok(&self) -> bool {
        self.success
    }

    pub fn is_err(&self) -> bool {
        !self.success
    }

    pub fn unwrap(self) -> serde_json::Value {
        if self.success {
            self.output
        } else {
            panic!("ToolResult::unwrap called on error: {:?}", self.error)
        }
    }

    pub fn unwrap_err(self) -> String {
        if !self.success {
            self.error.unwrap_or_default()
        } else {
            panic!("ToolResult::unwrap_err called on success")
        }
    }
}

/// Metadata describing a tool's interface — enough for an agent to decide
/// whether and how to invoke it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDescriptor {
    pub name: String,
    pub description: String,
    pub schema: ToolSchema,
}

pub use registry::ToolRegistry;
pub use procedure_provider::ProcedureToolProvider;

/// Trait for dynamic tool execution handlers.
///
/// Implement this to register custom tool handlers that execute without
/// modifying the kernel's built-in match arms.
pub trait ToolHandler: Send + Sync {
    fn execute(&self, params: &serde_json::Value, agent_id: &str) -> ToolResult;
}

impl<F> ToolHandler for F
where
    F: Fn(&serde_json::Value, &str) -> ToolResult + Send + Sync,
{
    fn execute(&self, params: &serde_json::Value, agent_id: &str) -> ToolResult {
        (self)(params, agent_id)
    }
}

/// Protocol-agnostic external tool provider.
///
/// The kernel's tool system is designed to be protocol-agnostic:
/// - `LlmProvider` abstracts inference (Ollama, OpenAI, vLLM — any dies, swap the adapter)
/// - `ExternalToolProvider` abstracts tool protocols (MCP, Agent Skills, A2A — same principle)
///
/// When a protocol becomes obsolete, delete the adapter.
/// When a new one emerges, add one. The kernel never changes.
pub trait ExternalToolProvider: Send + Sync {
    /// Human-readable name of this provider instance (e.g., "plico-mcp", "web-search").
    fn provider_name(&self) -> &str;

    /// Discover available tools from the external source.
    fn discover_tools(&self) -> Vec<ToolDescriptor>;

    /// Invoke a tool by name. The name is the tool's raw name (without prefix).
    fn call_tool(&self, name: &str, params: &serde_json::Value) -> ToolResult;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_result_ok_roundtrip() {
        let r = ToolResult::ok(serde_json::json!({"cid": "abc"}));
        assert!(r.success);
        assert!(r.error.is_none());
        let json = serde_json::to_string(&r).unwrap();
        let decoded: ToolResult = serde_json::from_str(&json).unwrap();
        assert!(decoded.success);
    }

    #[test]
    fn tool_result_error_roundtrip() {
        let r = ToolResult::error("not found");
        assert!(!r.success);
        assert_eq!(r.error.as_deref(), Some("not found"));
    }

    #[test]
    fn tool_descriptor_serialization() {
        let d = ToolDescriptor {
            name: "cas.create".into(),
            description: "Create a CAS object".into(),
            schema: serde_json::json!({"type": "object", "properties": {"content": {"type": "string"}}}),
        };
        let json = serde_json::to_string(&d).unwrap();
        let decoded: ToolDescriptor = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.name, "cas.create");
    }
}
