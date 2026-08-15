# Plico (太初) — AI-Native Operating System Kernel

Rust (edition 2021) personal digital-twin kernel. Memory/CAS are canonical; indexes and knowledge
graphs are derived; human documents are generated projections rather than the primary data model.

## Quick Reference

| Command | Purpose |
|---------|---------|
| `cargo build` | Build all targets |
| `EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test --lib` | Unit tests (fast) |
| `EMBEDDING_BACKEND=stub LLM_BACKEND=stub cargo test` | All tests |
| `cargo clippy -- -D warnings` | Lint (must be zero warnings) |
| `cargo fmt --check` | Format check |

## Rules

- **Stub backends for tests**: Always set `EMBEDDING_BACKEND=stub LLM_BACKEND=stub`
- **No clippy suppressions**: Fix issues, never use `#[allow(clippy::...)]`
- **Typed errors only**: Use `thiserror`, no panics in library code
- **Atomic writes**: Use `atomic_write_json()`, never `std::fs::write()` for JSON/index files
- **Reserved names**: `kernel`, `system`, `root`, `admin` cannot be user-registered agents
- **One public wire**: `plico.personal.v2`, exactly 14 typed object/memory/projection/session/readiness operations
- **No compatibility shell**: public code must not import legacy `ApiRequest`/`ApiResponse`
- **No organization control plane**: public input never contains tenant, organization, cluster, team, or role
- **Sensitive tracing**: never log bearer, content, full query, provider raw errors, or host-private paths

## Architecture

```
api/bin → kernel → tool/fs/intent → cas/memory/scheduler/temporal/llm
```

- `kernel/` is the central orchestrator — all subsystem calls go through `AIKernel`
- CAS (SHA-256) is the only module touching host filesystem
- JSON is the sole kernel interface format
- `src/api/public/` and `AIKernel::handle_public_request` are the external contract; legacy semantic
  commands are internal migration debt
- UDS/Embedded/MCP inject local owner. TCP requires `PLICO_BEARER_TOKEN`, bootstrapped once in the
  0600 `agent_tokens.json` under `PLICO_ROOT`; bootstrap is not a business operation

## File Layout

```
src/
├── cas/          # Content-Addressed Storage
├── memory/       # 4-tier layered memory
├── intent/       # Internal NL routing, not public wire
├── scheduler/    # Agent lifecycle + dispatch
├── fs/           # Semantic FS: vectors, BM25, KG
├── kernel/       # AIKernel orchestration
├── api/          # plico.personal.v2 + internal legacy commands
├── tool/         # Tool trait + registry
├── temporal/     # Time reasoning
├── llm/          # LLM provider abstraction
├── mcp/          # MCP client
└── bin/          # plicod, aicli, plico-mcp
```
