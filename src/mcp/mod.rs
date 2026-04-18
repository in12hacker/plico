//! MCP (Model Context Protocol) Client — connect to external MCP servers.
//!
//! Implements the client side of MCP over stdio, enabling Plico agents to
//! call tools from any MCP-compatible server (10,000+ in the ecosystem).
//!
//! # Architecture
//!
//! ```text
//! Plico Agent → tool_call("mcp.web_search", params)
//!     → ToolRegistry → McpToolHandler
//!         → McpClient (JSON-RPC 2.0 over stdio)
//!             → External MCP Server subprocess
//! ```

pub mod client;

#[cfg(test)]
mod tests;

pub use client::{McpClient, McpToolDef, McpError};
