# Module: kernel

AI Kernel — the central orchestrator that wires together all Plico subsystems (CAS, memory, scheduler, FS, permissions, intent, tools, messaging).

Status: active | Fan-in: 3 | Fan-out: 7

## Dependents (Fan-in: 3)

- `src/bin/plicod.rs` → AIKernel (daemon creates kernel, handles API requests)
- `src/bin/plico_mcp.rs` → AIKernel (MCP server creates kernel for JSON-RPC dispatch)
- `src/bin/aicli/main.rs` → AIKernel (CLI creates kernel for local operations)

## Modification Risk

- Add `AIKernel` public method → compatible if callers updated in binaries
- Change `AIKernel::new()` signature → BREAKING, update all 3 binaries
- Remove kernel method → BREAKING, update plicod + plico_mcp + aicli dispatch
- Change `execute_tool` dispatch → affects all tool clients; check `builtin_tools.rs`
- Change `handle_api_request` → affects ALL downstream consumers

## Task Routing

- Add new API operation → `mod.rs` dispatch + `api/semantic.rs` ApiRequest variant + binary dispatch
- Built-in tool registration / `execute_tool` → `builtin_tools.rs`
- Persistence / restore / embedding bootstrap → `persistence.rs`
- Event bus / event log / sequenced events → `event_bus.rs`
- Operation-specific logic → see `ops/INDEX.md` for 24 operation files

## Public API

| Export | File | Description |
|--------|------|-------------|
| `AIKernel` | `mod.rs` | Central orchestrator — all subsystem access |

Key methods (non-exhaustive): `new`, `handle_api_request`, `execute_tool`, `intent_resolve`, `agent_set_resources`, `send_message`, `read_messages`, `ack_message`, session/delta/memory/FS/graph helpers — see `mod.rs`.

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ⚠ ~1839 | AIKernel struct, API dispatch, orchestration — needs split |
| `builtin_tools.rs` | ~434 | `register_builtin_tools`, `execute_tool` (allowlist + memory quota) |
| `persistence.rs` | ~366 | Persist/restore agents, intents, memories, search index, event log |
| `event_bus.rs` | ~959 | EventBus — typed pub/sub, JSONL persistence, restore |
| `ops/` | 24 files | Operation groups — see `ops/INDEX.md` |

## Dependencies (Fan-out: 7)

- `src/cas/` — CASStorage, AIObject
- `src/memory/` — LayeredMemory, persistence traits, relevance, context snapshot
- `src/scheduler/` — AgentScheduler, messaging, dispatch types
- `src/fs/` — SemanticFS, search, KG, embedding
- `src/api/` — PermissionGuard, semantic protocol
- `src/intent/` — ChainRouter, intent resolution
- `src/tool/` — ToolRegistry

## Interface Contract

- `AIKernel::new()`: initializes subsystems; embedding backend from env (`EMBEDDING_BACKEND`, etc.)
- `handle_api_request()`: C-3 lazy agent registration on every call via `ensure_agent_registered`
- `pub(crate)` fields: library-internal only; crate integration tests in `tests/` must use public API
- Thread safety: kernel not `Clone`; daemon uses `Arc<AIKernel>`
- EventBus: JSONL append-on-emit, restore on startup via `restore_event_log()`

## Tests

- Integration: `tests/kernel_test.rs`, `tests/ai_experience_test.rs`
- Critical: semantic CRUD through kernel, agent + intent + tool paths, v0.5 E2E, multi-session AI workflow
