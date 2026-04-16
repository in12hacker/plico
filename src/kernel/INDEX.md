# Module: kernel

AI Kernel — the central orchestrator that wires together all Plico subsystems (CAS, memory, scheduler, FS, permissions, intent, tools, messaging).

Status: active | Fan-in: 2 | Fan-out: 7

## Dependents (Fan-in: 2)

- `src/bin/plicod.rs` → AIKernel (daemon creates kernel, handles API requests)
- `src/bin/aicli.rs` → AIKernel (CLI creates kernel for local operations)

## Modification Risk

- Add `AIKernel` public method → compatible if callers updated in binaries
- Change `AIKernel::new()` signature → BREAKING, update both binaries
- Remove kernel method → BREAKING, update plicod.rs + aicli.rs dispatch
- Change `execute_tool` dispatch → affects all tool clients; check `builtin_tools.rs`

## Task Routing

- Add new API operation → `mod.rs` (`handle_api_request` / public method) + `plicod.rs` dispatch + `api/semantic.rs` if new variant
- Built-in tool registration / `execute_tool` → `builtin_tools.rs`
- Persistence / restore / embedding bootstrap → `persistence.rs`
- Intent resolution wiring → `mod.rs` + `intent/`
- Agent messaging → `mod.rs` + `scheduler/messaging.rs`

## Public API

| Export | File | Description |
|--------|------|-------------|
| `AIKernel` | `mod.rs` | Central orchestrator — all subsystem access |

Key methods (non-exhaustive): `new`, `handle_api_request`, `execute_tool`, `intent_resolve`, `agent_set_resources`, `send_message`, `read_messages`, `ack_message`, dispatch/memory/FS graph helpers — see `mod.rs`.

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ⚠ ~1154 | AIKernel struct, API dispatch, orchestration — still large; further splits TBD |
| `builtin_tools.rs` | ~434 | `register_builtin_tools`, `execute_tool` (allowlist + memory quota) |
| `persistence.rs` | ~217 | Persist/restore agents, intents, memories, search index; embedding factory |

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
- `pub(crate)` fields: library-internal only; crate integration tests in `tests/` must use public API
- Thread safety: kernel not `Clone`; daemon uses `Arc<AIKernel>`

## Tests

- Integration: `tests/kernel_test.rs`
- Critical: semantic CRUD through kernel, agent + intent + tool paths, v0.5 E2E (resources, messaging, intent temporal)
