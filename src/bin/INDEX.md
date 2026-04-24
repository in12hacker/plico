# Module: bin

Binary entry points — four executables for different deployment scenarios.

Status: active | Fan-in: 0 (entry points) | Fan-out: 3

## Binaries

| Binary | File | Description |
|--------|------|-------------|
| `plicod` | `plicod.rs` | Daemon — TCP + UDS, length-prefixed JSON framing, PID lifecycle |
| `plico-mcp` | `plico_mcp/` | MCP stdio server (JSON-RPC 2.0 over stdin/stdout) |
| `plico-sse` | `plico_sse.rs` | SSE streaming adapter for A2A protocol compatibility |
| `aicli` | `aicli/main.rs` | Semantic CLI — daemon-first, `--embedded` fallback, `--tcp` remote |

## Connection Modes (Daemon-First Architecture)

```
Default:    aicli → UDS (~/.plico/plico.sock) → plicod → AIKernel
Embedded:   aicli --embedded → AIKernel (direct, for testing)
TCP:        aicli --tcp 1.2.3.4:7878 → plicod → AIKernel
```

Transport abstraction: `src/client.rs` — `KernelClient` trait with `EmbeddedClient` and `RemoteClient`.

## Task Routing

| Task | File |
|------|------|
| Add MCP tool/resource/action | `plico_mcp/dispatch.rs` + `plico_mcp/tools.rs` |
| Add CLI command | `aicli/commands/handlers/` + `aicli/commands/mod.rs` dispatch |
| Fix daemon protocol / UDS | `plicod.rs` |
| Change CLI output format | `aicli/commands/mod.rs` format functions |
| Fix SSE streaming / A2A | `plico_sse.rs` |

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `plicod.rs` | ~161 | TCP + UDS daemon, length-prefixed JSON framing |
| `plico_mcp/main.rs` | ~200 | MCP stdio entry point |
| `plico_mcp/dispatch.rs` | ~1138 | MCP tool call dispatcher (40+ routes) |
| `plico_mcp/tools.rs` | ~400 | MCP tool definitions |
| `plico_mcp/format.rs` | ~140 | Response formatting |
| `plico_sse.rs` | ~1106 | SSE streaming adapter (A2A protocol) |
| `aicli/main.rs` | ~695 | CLI entry: daemon / --embedded / --tcp mode |
| `aicli/commands/mod.rs` | ~494 | Dispatch + output formatting |
| `aicli/commands/handlers/` | 17 files | Command handlers (see `handlers/INDEX.md`) |

## Dependencies (Fan-out: 3)

- `src/kernel/` → AIKernel (all binaries)
- `src/api/` → ApiRequest, ApiResponse, PermissionGuard (protocol types)
- `src/client.rs` → KernelClient, EmbeddedClient, RemoteClient (transport)

## Interface Contract

- `plicod`: TCP + UDS; length-prefixed JSON framing (`[4-byte BE length][JSON payload]`)
- `plico-mcp`: JSON-RPC 2.0 over stdio; 3 composite tools (`plico`, `plico_cold`, `plico_skills`)
- `aicli`: daemon-first; `--root`, `--embedded`, `--tcp`, `--agent`, `AICLI_OUTPUT=json` (default)
- All binaries use `KernelClient` trait; never import subsystem modules directly

## Tests

- CLI: `tests/cli_test.rs`
- MCP: `tests/mcp_test.rs`
- TCP: tested via `tests/kernel_test.rs` (kernel API directly)
