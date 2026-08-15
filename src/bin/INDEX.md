# Module: bin

Binary entry points — three runtime clients plus one feature-gated offline vault migrator.

Status: active | Fan-in: 0 (entry points) | Fan-out: 3

## Binaries

| Binary | File | Description |
|--------|------|-------------|
| `plicod` | `plicod.rs` | Daemon — TCP + UDS, length-prefixed JSON framing, PID lifecycle |
| `plico-mcp` | `plico_mcp/` | MCP stdio server (JSON-RPC 2.0 over stdin/stdout) |
| `aicli` | `aicli/main.rs` | Semantic CLI — daemon-first, `--embedded` fallback, `--tcp` remote |
| `plico-memory-migrate` | `plico_memory_migrate/` | `offline-migration` only: inspect/dry-run/migrate legacy personal memory under the runtime vault lock |

## Connection Modes (Daemon-First Architecture)

```
Default:    aicli → UDS (~/.plico/plico.sock) → plicod → AIKernel
Embedded:   aicli --embedded → AIKernel (direct, for testing)
TCP:        aicli --tcp 1.2.3.4:7878 → plicod → AIKernel
```

Transport abstraction: `src/client.rs` — `KernelClient` trait with `EmbeddedClient` and `RemoteClient`.
All three modes carry the same 14-operation `plico.personal.v2` contract. UDS/Embedded/MCP inject the
trusted local-owner context and reject payload auth. TCP requires `PLICO_BEARER_TOKEN`; `plicod` creates
the stable `personal-owner` credential on first start in the owner-only `agent_tokens.json` under the
configured `PLICO_ROOT`. Read that file locally and never pass the bearer as a CLI argument or log it.

The single-track P3-A cutover removed `memory.index_status`, added `projection.status` and owner-only
`projection.rebuild`, and removed the v1 reader/alias. All binaries use the same manifest-backed
status contract; Memory vector/hybrid retrieval is still unsupported.

## Task Routing

| Task | File |
|------|------|
| Change a public MCP tool schema | `plico_mcp/tools.rs` after changing `api/public/` |
| Add CLI operation | `api/public/` first, then `aicli/input.rs`; catalog parity must remain exact |
| Fix daemon protocol / UDS | `plicod.rs` |
| Change CLI output format | `aicli/main.rs` typed `PublicResponse` output |

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `plicod.rs` | TCP + UDS daemon, auth bootstrap, typed framing and PID lifecycle |
| `plico_mcp/main.rs` | MCP stdio entry point and typed public-client dispatch |
| `plico_mcp/tools.rs` | Exact 14-tool catalog and `PublicCommand` input mapping |
| `aicli/main.rs` | CLI entry: daemon / --embedded / --tcp and typed response output |
| `aicli/input.rs` | Strict dotted-operation argument parser; identity/auth are never operation inputs |
| `plico_memory_migrate/` | Exact legacy DTO/preflight, canonical plan and atomic offline publisher CLI |

## Dependencies (Fan-out: 3)

- `src/kernel/` → AIKernel (all binaries)
- `src/api/public/` → `PublicRequest`, `PublicResponse`, 14 typed commands
- `src/client.rs` → KernelClient, EmbeddedClient, RemoteClient (transport)

## Interface Contract

- `plicod`: TCP + UDS; length-prefixed JSON framing (`[4-byte BE length][JSON payload]`)
- `plico-mcp`: JSON-RPC 2.0 over stdio; advertises tools only. Tool names exactly equal
  `PUBLIC_OPERATIONS`, calls use `KernelClient::request(PublicRequest)`, and domain failures
  remain typed `PublicResponse` values with MCP `isError=true`.
- MCP embedded mode uses the trusted local-owner `PublicTransport::Mcp` context. `--daemon`
  selects UDS only; `--tcp ADDRESS` requires `PLICO_BEARER_TOKEN` and never accepts auth in tool input.
- `aicli`: daemon-first; `--root`, `--embedded`, `--tcp HOST:PORT`, followed by one exact dotted
  operation. There is no `--agent`, generic action, or hidden legacy mode.
- Exact operation/tool set: `capabilities.describe`, `runtime.readiness`, `object.put/get/search`,
  `memory.create/get/recall/update/delete`, `projection.status/rebuild`, `session.start/end`.
- TCP bootstrap is local daemon infrastructure, not a fifteenth business operation. First start
  atomically creates or reuses the `personal-owner` entry in owner-only `agent_tokens.json`; startup
  fails closed if it cannot durably persist the credential.
- All binaries use `KernelClient` trait; never import subsystem modules directly

## Tests

- CLI: `tests/cli_test.rs`
- MCP: `src/bin/plico_mcp/` tests plus `src/mcp/tests.rs` transport-client tests
- TCP/UDS framing and auth: `src/bin/plicod.rs` tests
- Typed client: `src/client.rs` tests
- Offline migration: inline binary/CAS tests and `tests/memory_migration_test.rs`
