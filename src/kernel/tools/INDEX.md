# Module: kernel/tools

Built-in tool handlers — each module implements `handle()` for one tool category. Registered in the tool registry and callable via `CallTool` API request.

Status: stable | Fan-in: 1 | Fan-out: 6+

## Public API

All handlers are `pub(in crate::kernel)` — called from tool dispatch.

| Tool | File | Tools Handled |
|------|------|---------------|
| `cas` | `cas.rs` | `cas.put`, `cas.get`, `cas.delete`, `cas.exists` |
| `memory` | `memory.rs` | `memory.recall`, `memory.store`, `memory.forget` |
| `graph` | `graph.rs` | `graph.add_node`, `graph.add_edge`, `graph.traverse`, `graph.find_paths` |
| `agent` | `agent.rs` | `agent.list`, `agent.get`, `agent.discover` |
| `system` | `system.rs` | `system.status`, `system.health` |
| `messaging` | `messaging.rs` | `messaging.send`, `messaging.broadcast` |
| `permission` | `permission.rs` | `permission.grant`, `permission.check` |

## Dependencies (Fan-out: 6+)

- `src/kernel/mod.rs` — `AIKernel` self reference
- `src/fs/` — SemanticFS operations
- `src/memory/` — LayeredMemory operations
- `src/scheduler/` — AgentScheduler operations
- `src/cas/` — CASStorage operations
- `src/api/permission.rs` — `PermissionGuard`

## Dependents (Fan-in: 1)

- `src/kernel/mod.rs` → tool dispatch in `handle_api_request(CallTool{..})`

## Interface Contract

- Each handler: `pub(in crate::kernel) fn handle(kernel: &AIKernel, tool_name: &str, params: &Value, agent_id: &str) -> ToolResult`
- `ToolResult`: `{ ok: bool, output: Value, error: Option<String> }`
- Tools are read-only or state-mutating — documented per tool

## Modification Risk

| Change | Risk |
|--------|------|
| Add new tool handler | Low — register in mod.rs |
| Change handler signature | Medium — affects tool dispatch |
| Change ToolResult shape | High — affects all tool consumers |

## Task Routing

| Task | File |
|------|------|
| Fix CAS tools | `cas.rs` |
| Fix memory tools | `memory.rs` |
| Fix graph tools | `graph.rs` |
| Fix agent tools | `agent.rs` |
| Fix system tools | `system.rs` |
| Fix messaging tools | `messaging.rs` |
| Fix permission tools | `permission.rs` |

## Tests

- Unit: co-located `#[cfg(test)]` in each tool file
- Integration: tested via kernel tests that exercise tool dispatch
