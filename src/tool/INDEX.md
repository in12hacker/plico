# Module: tool

Tool Abstraction — "Everything is a Tool" capability system for agent-discoverable operations.

Status: stable | Fan-in: 2 | Fan-out: 0

## Public API

| Export | File | Description |
|--------|------|-------------|
| `ToolDescriptor` | `mod.rs` | Name + description + JSON schema for a tool |
| `ToolResult` | `mod.rs` | Execution result (success/error + JSON output) |
| `ToolSchema` | `mod.rs` | Type alias for `serde_json::Value` |
| `ToolRegistry` | `registry.rs` | Thread-safe catalog of registered tools |

## Dependencies (Fan-out: 0)

None — tool module defines types only. Depends on `serde`, `serde_json` (external crates).

## Dependents (Fan-in: 2)

- `src/kernel/mod.rs` → ToolRegistry, ToolDescriptor, ToolResult (registers built-in tools, dispatches execute_tool)
- `src/api/semantic.rs` → ToolDescriptor, ToolResult (ApiResponse fields for tool_result/tools)

## Interface Contract

- `ToolRegistry::register()`: overwrites if name already exists; thread-safe via RwLock
- `ToolRegistry::list()`: returns tools sorted by name for deterministic output
- `ToolResult::ok(value)`: success=true, error=None
- `ToolResult::error(msg)`: success=false, output=Null
- Tool execution is NOT in registry — kernel's `execute_tool()` dispatches based on tool name

## Modification Risk

- Add field to `ToolDescriptor` → compatible if `#[serde(default)]`
- Change `ToolResult` structure → BREAKING, update kernel + API layer
- Add new built-in tool → modify `src/kernel/mod.rs` register_builtin_tools + execute_tool

## Task Routing

- Register a new built-in tool → modify `src/kernel/mod.rs` register_builtin_tools() + execute_tool()
- Add external tool support → extend `ToolRegistry` with handler closures
- Change tool schema format → modify `ToolDescriptor` + all register calls in kernel

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ~91 | ToolResult, ToolSchema, ToolDescriptor types + tests |
| `registry.rs` | ~100 | ToolRegistry — register/get/list/unregister/contains |

## Tests

- Unit: `src/tool/mod.rs` mod tests (3 tests), `src/tool/registry.rs` mod tests (4 tests)
- Integration: `tests/kernel_test.rs` test_e2e_tool_cognitive_memory_cycle
- Critical: tool list discovery, tool call dispatch, unknown tool error
