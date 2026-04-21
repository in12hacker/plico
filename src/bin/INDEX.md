# Module: bin

Binary entry points — three executables for different deployment scenarios.

Status: active | Fan-in: 0 (entry points) | Fan-out: 2

## Binaries

| Binary | File | Description |
|--------|------|-------------|
| `plicod` | `plicod.rs` | TCP daemon (port 7878, JSON ApiRequest/ApiResponse) |
| `plico-mcp` | `plico_mcp.rs` | MCP stdio server (JSON-RPC 2.0 over stdin/stdout) |
| `aicli` | `aicli/main.rs` | AI-friendly semantic CLI |

## Task Routing

| Task | File |
|------|------|
| Add MCP tool/resource/action | `plico_mcp.rs` |
| Add CLI command | `aicli/commands/handlers/` + `aicli/commands/mod.rs` dispatch |
| Fix TCP daemon protocol | `plicod.rs` |
| Change CLI output format | `aicli/commands/mod.rs` format functions |

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `plicod.rs` | — | TCP daemon, JSON API server |
| `plico_mcp.rs` | ⚠ ~1763 | MCP stdio server — needs split (dispatch/resources/tests) |
| `aicli/main.rs` | — | CLI entry: local kernel or --tcp mode |
| `aicli/commands/mod.rs` | — | Dispatch + output formatting |
| `aicli/commands/handlers/` | 15 files | Command handlers (see AGENTS.md directory map) |

## Dependencies (Fan-out: 2)

- `src/kernel/` → AIKernel (all binaries)
- `src/api/` → ApiRequest, ApiResponse, PermissionGuard (protocol types)

## Modification Risk

- Change `ApiRequest` handling in plico_mcp.rs → affects all MCP clients (Cursor, Claude, etc.)
- Change CLI dispatch → affects all CLI users
- Change TCP protocol → affects plicod clients

## Interface Contract

- `plicod`: TCP-only, no HTTP; reads/writes JSON `ApiRequest`/`ApiResponse` per line
- `plico-mcp`: JSON-RPC 2.0 over stdio; 3 composite tools (`plico`, `plico_cold`, `plico_skills`)
- `aicli`: subcommand dispatch; supports `--root`, `--tcp`, `--agent`, `AICLI_OUTPUT=json`
- All binaries call `AIKernel::new()` or connect via TCP; never import subsystem modules directly

## Tests

- CLI: `tests/cli_test.rs`
- MCP: `tests/mcp_test.rs`
- TCP: tested via `tests/kernel_test.rs` (kernel API directly)
