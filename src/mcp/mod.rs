//! MCP (Model Context Protocol) Client — connect to external MCP servers.
//!
//! Implements `ExternalToolProvider` — the kernel's protocol-agnostic
//! tool abstraction. MCP is one adapter; when a new protocol emerges,
//! add a new module implementing the same trait. When MCP dies, delete this.

pub mod client;

#[cfg(test)]
mod tests;

pub use client::{McpClient, McpToolDef, McpError};
