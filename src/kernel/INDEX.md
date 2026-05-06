# Module: kernel

AI Kernel — the central orchestrator that wires together all Plico subsystems (CAS, memory, scheduler, FS, permissions, intent, tools, messaging).

Status: active | Fan-in: 3 | Fan-out: 7

## Dependents (Fan-in: 4)

- `src/bin/plicod.rs` → AIKernel (daemon hosts kernel, serves via UDS + TCP)
- `src/bin/plico_mcp/` → AIKernel (MCP server creates kernel for JSON-RPC dispatch)
- `src/bin/aicli/main.rs` → AIKernel via `KernelClient` (daemon-first, `--embedded` fallback)
- `src/client.rs` → `EmbeddedClient` wraps AIKernel directly

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
- Cognitive optimization / skill extraction / intent network → `cognition/`

## Public API

| Export | File | Description |
|--------|------|-------------|
| `AIKernel` | `mod.rs` | Central orchestrator — all subsystem access |

Key methods (non-exhaustive): `new`, `handle_api_request`, `execute_tool`, `intent_resolve`, `agent_set_resources`, `send_message`, `read_messages`, `ack_message`, session/delta/memory/FS/graph helpers — see `mod.rs`.

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ~574 | AIKernel struct, orchestration core |
| `api_dispatch.rs` | ~250 | Thin API dispatch → 14 handler modules in `handlers/` |
| `handlers/` | 14 files | Domain-specific API request handlers (cas, memory, agent, graph, etc.) |
| `builtin_tools.rs` | ~480 | Tool registration + dispatch → 7 tool modules in `tools/` |
| `tools/` | 7 files | Built-in tool handlers (cas, memory, graph, agent, system, messaging, permission) |
| `hook.rs` | ~266 | HookRegistry — 5 interception points, Block/Continue results |
| `persistence.rs` | ~366 | Persist/restore agents, intents, memories, search index, event log |
| `event_bus.rs` | ~959 | EventBus — typed pub/sub, JSONL persistence, restore |
| `ops/` | 24 files | Operation groups — see `ops/INDEX.md` |
| `cognition/` | 12 files | Soul v3.0 cognitive symbiotic engine — see `cognition/INDEX.md` |

## Dependencies (Fan-out: 8)

- `src/cas/` — CASStorage, AIObject
- `src/memory/` — LayeredMemory, persistence traits, relevance, context snapshot
- `src/scheduler/` — AgentScheduler, messaging, dispatch types
- `src/fs/` — SemanticFS, search, KG, embedding
- `src/api/` — PermissionGuard, semantic protocol
- `src/intent/` — ChainRouter, intent resolution
- `src/tool/` — ToolRegistry
- `src/kernel/cognition/` — CognitiveLoop, SkillForge, IntentSemanticNetwork (internal dependency, initialized in `AIKernel::new`)

## Interface Contract

- `AIKernel::new()`: initializes subsystems; embedding backend from env (`EMBEDDING_BACKEND`, etc.)
- `handle_api_request()`: C-3 lazy agent registration on every call via `ensure_agent_registered`
- `pub(crate)` fields: library-internal only; crate integration tests in `tests/` must use public API
- Thread safety: kernel not `Clone`; daemon uses `Arc<AIKernel>`
- EventBus: JSONL append-on-emit, restore on startup via `restore_event_log()`

## Tests

- Integration: `tests/kernel_test.rs`, `tests/ai_experience_test.rs`
- Critical: semantic CRUD through kernel, agent + intent + tool paths, v0.5 E2E, multi-session AI workflow
