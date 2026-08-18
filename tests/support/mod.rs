//! Shared support for MCP integration tests.
//!
//! The typed-client side spawns the real `plico-mcp` binary through the
//! production `McpClient` with a fresh `TempDir` as `PLICO_ROOT` and stub
//! embedding/LLM backends, so no test reaches the network or a real model.
//! The binary location comes from Cargo's official `CARGO_BIN_EXE_plico-mcp`
//! and therefore resolves correctly under any `CARGO_TARGET_DIR`.
//!
//! The raw JSON-RPC parity harness stays in `tests/mcp_test.rs` unchanged;
//! it owns its process lifecycle (explicit kill+wait on drop) deliberately
//! so the protocol-parity file keeps a self-contained, review-frozen diff.

use std::path::Path;

/// Compile-time absolute path of the `plico-mcp` binary. Build-machine
/// constant provided by Cargo for integration-test targets — never user
/// input.
pub const PLICO_MCP_BIN: &str = env!("CARGO_BIN_EXE_plico-mcp");

/// Spawns the typed `McpClient` against the real binary with the stubbed
/// environment. The server exits when the client is dropped (stdin EOF),
/// which is the production shutdown path, so no explicit kill is needed.
pub fn typed_client(root: &Path) -> plico::mcp::McpClient {
    plico::mcp::McpClient::new(
        PLICO_MCP_BIN,
        &[],
        &[
            ("PLICO_ROOT", root.to_str().expect("utf-8 PLICO_ROOT path")),
            ("EMBEDDING_BACKEND", "stub"),
            ("LLM_BACKEND", "stub"),
        ],
    )
    .expect("typed MCP client over the real plico-mcp binary")
}
