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
| `client.rs` | — | McpClient: spawn process, JSON-RPC communicate, discover tools; `ManagedChild` RAII owns the subprocess (close transport → bounded grace → kill → always reap) |
| `tests.rs` | — | Unit tests (pure provider wiring, poison-tolerant take, managed-child state machine) |

## Dependencies (Fan-out: 0)

None — depends only on external crates (serde, serde_json); process I/O is synchronous std (`std::process`), not tokio.

## Modification Risk

- Change `McpClient` API → update `tools_external.rs` adapter in kernel
- Change `McpToolDef` → update tool registration path

## Interface Contract

- `McpClient::connect()`: spawns MCP server process, performs `initialize` handshake
- `McpClient::list_tools()`: returns discovered tools from remote server
- `McpClient::call_tool()`: invokes remote tool, returns JSON result
- Thread safety: client holds process handle, not `Clone`; one client per server

## Tests

- Unit (pure, no subprocess): `src/mcp/tests.rs` — kernel tool-provider wiring with an in-memory fake `ExternalToolProvider`
- Integration (real binary): `tests/mcp_test.rs` — raw JSON-RPC public protocol parity, self-contained process lifecycle
- Integration (real binary): `tests/mcp_client_test.rs` — typed `McpClient` cross-validation via Cargo's `CARGO_BIN_EXE_plico-mcp`
- Shared support: `tests/support/mod.rs` — binary location + stubbed environment for typed-client tests
