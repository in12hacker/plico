# Module: kernel/handlers

API request handlers — each module handles one domain of `ApiRequest` variants. Dispatched from `AIKernel::handle_api_request()`.

Status: stable | Fan-in: 1 | Fan-out: 14+

## Public API

All handlers are `pub(in crate::kernel)` — called only from `kernel/mod.rs` dispatch.

| Handler | File | ApiRequest Variants |
|---------|------|---------------------|
| `cas` | `cas.rs` | Put, Get, Delete, Exists, BatchGet |
| `memory` | `memory.rs` | Remember, Recall, Forget, ListMemories |
| `agent` | `agent.rs` | RegisterAgent, ListAgents, GetAgent, SuspendAgent, ResumeAgent |
| `graph` | `graph.rs` | AddNode, AddEdge, RemoveNode, FindPaths, Traverse, Impact, CausalPath |
| `intent` | `intent.rs` | ExecuteIntent, ListIntents |
| `events` | `events.rs` | PublishEvent, Subscribe, Unsubscribe, ListEvents |
| `session` | `session.rs` | StartSession, EndSession, CompoundResponse |
| `system` | `system.rs` | SystemStatus, HealthCheck |
| `tools` | `tools.rs` | ListTools, CallTool, RegisterTool |
| `messaging` | `messaging.rs` | SendMessage, ListMessages, Broadcast |
| `permission` | `permission.rs` | GrantPermission, RevokePermission, CheckPermission |
| `tenant` | `tenant.rs` | CreateTenant, SwitchTenant, ListTenants |
| `model` | `model.rs` | HotSwapModel, ListModels |
| `storage` | `storage.rs` | StorageStats, ColdEvict |
| `prompt` | `prompt.rs` | RegisterPrompt, ListPrompts |
| `import` | `import.rs` | ImportFile, ImportDirectory |
| `core_ops` | `core_ops.rs` | BatchCreate, BatchDelete, CompositeOps |

## Dependencies (Fan-out: 14+)

- `src/kernel/mod.rs` — `AIKernel` self reference
- `src/api/semantic.rs` — `ApiRequest`, `ApiResponse` types
- `src/fs/` — SemanticFS operations
- `src/memory/` — LayeredMemory operations
- `src/scheduler/` — AgentScheduler operations
- `src/cas/` — CASStorage operations
- All subsystem modules accessed via `self.*` on `AIKernel`

## Dependents (Fan-in: 1)

- `src/kernel/mod.rs` → `handle_api_request()` dispatches to each handler

## Interface Contract

- Each handler is an `impl AIKernel` block with `pub(in crate::kernel)` methods
- Input: domain-specific fields extracted from `ApiRequest` enum variant
- Output: `ApiResponse` (always `ok: true/false` + payload or error)
- Side effects: may mutate kernel state (memory, scheduler, FS, KG)

## Modification Risk

| Change | Risk |
|--------|------|
| Add new handler file | Low — register in mod.rs, add dispatch arm |
| Change handler signature | Medium — affects kernel dispatch |
| Change ApiResponse shape | High — affects all consumers (CLI, MCP, SSE) |

## Task Routing

| Task | File |
|------|------|
| Fix CAS put/get/delete | `cas.rs` |
| Fix memory recall/store | `memory.rs` |
| Fix agent registration/lifecycle | `agent.rs` |
| Fix graph traversal/query | `graph.rs` |
| Fix event pub/sub | `events.rs` |
| Fix session management | `session.rs` |
| Fix tool registration/call | `tools.rs` |
| Fix inter-agent messaging | `messaging.rs` |
| Fix permission checks | `permission.rs` |
| Fix tenant isolation | `tenant.rs` |
| Fix model hot-swap | `model.rs` |
| Fix storage stats/eviction | `storage.rs` |
| Fix file import | `import.rs` |
| Fix batch/composite ops | `core_ops.rs` |

## Tests

- Unit: co-located `#[cfg(test)]` in each handler file
- Integration: `tests/kernel_test.rs`, `tests/permission_test.rs`, `tests/batch_ops_test.rs`
