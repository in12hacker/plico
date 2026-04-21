# Module: mcp

MCP client — connect to external MCP servers to discover and call remote tools.

Status: stable | Fan-in: 1 | Fan-out: 0

## Dependents (Fan-in: 1)

- `src/kernel/ops/tools_external.rs` → ExternalToolProvider adapter (kernel registers external MCP tools)

## Public API

| Export | File | Description |
|--------|------|-------------|
| `McpClient` | `client.rs` | JSON-RPC 2.0 client for MCP stdio servers |
| `McpToolDef` | `client.rs` | External tool definition (name, description, schema) |
| `McpError` | `client.rs` | Typed MCP client errors |

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | — | Re-exports (ExternalToolProvider adapter) |
| `client.rs` | — | McpClient: spawn process, JSON-RPC communicate, discover tools |
| `tests.rs` | — | Unit tests |

## Dependencies (Fan-out: 0)

None — depends only on external crates (serde, serde_json, tokio for process I/O).

## Modification Risk

- Change `McpClient` API → update `tools_external.rs` adapter in kernel
- Change `McpToolDef` → update tool registration path

## Interface Contract

- `McpClient::connect()`: spawns MCP server process, performs `initialize` handshake
- `McpClient::list_tools()`: returns discovered tools from remote server
- `McpClient::call_tool()`: invokes remote tool, returns JSON result
- Thread safety: client holds process handle, not `Clone`; one client per server

## Tests

- Unit: `src/mcp/tests.rs`
- Integration: `tests/mcp_test.rs`
