# Plico (太初) — AI-Native Operating System Kernel

Rust (edition 2021) project. AI-native OS kernel with semantic APIs, content-addressed storage, knowledge graph, and layered memory.

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

## Architecture

```
api/bin → kernel → tool/fs/intent → cas/memory/scheduler/temporal/llm
```

- `kernel/` is the central orchestrator — all subsystem calls go through `AIKernel`
- CAS (SHA-256) is the only module touching host filesystem
- JSON is the sole kernel interface format

## File Layout

```
src/
├── cas/          # Content-Addressed Storage
├── memory/       # 4-tier layered memory
├── intent/       # NL → ApiRequest router
├── scheduler/    # Agent lifecycle + dispatch
├── fs/           # Semantic FS: vectors, BM25, KG
├── kernel/       # AIKernel orchestration
├── api/          # API layer + permissions
├── tool/         # Tool trait + registry
├── temporal/     # Time reasoning
├── llm/          # LLM provider abstraction
├── mcp/          # MCP client
└── bin/          # plicod, aicli, plico-mcp, plico-sse
```
