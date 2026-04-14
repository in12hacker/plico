# Module: kernel — AI Kernel (Orchestrator)

Central coordinator. Wires all subsystems and enforces permission checks.

Status: active | Fan-in: 3 (binaries) | Fan-out: 5 (all subsystems)

## Public API

| Export | File | Description |
|--------|------|-------------|
| `AIKernel` | `mod.rs` | Orchestrator: new(), all subsystem access methods |
| `AgentHandle` | `mod.rs` | Lightweight agent reference for external use |

## Dependencies (Fan-out: 5)

- `src/cas/` — `CASStorage`
- `src/memory/` — `LayeredMemory`
- `src/scheduler/` — `AgentScheduler`
- `src/fs/` — `SemanticFS`
- `src/api/permission/` — `PermissionGuard`

## Dependents (Fan-in: 3)

- `src/bin/plicod.rs` — one `AIKernel` instance, shared across threads via `Arc`
- `src/bin/aicli.rs` — local kernel instance per invocation
- Tests

## Interface Contract

- `AIKernel::new(root)`: Initializes all subsystems at `root/{cas,objects,context}`. **Errors**: propagates from CASStorage or SemanticFS init.
- All state-changing operations (`store_object`, `semantic_create`, `semantic_update`, `semantic_delete`) call `PermissionGuard::check()` first.
- `semantic_search`, `get_object` (read): check permission, then delegate.
- `remember`/`recall`: bypass permission for MVP (ephemeral memory).
- `register_agent`: creates `Agent`, registers with scheduler, returns `AgentId`.

## Modification Risk

- Add new subsystem → update `AIKernel::new()`, add field + initialization
- Change permission policy → update `PermissionGuard::check()` in each operation
- Add transaction support (atomic multi-subsystem ops) → significant redesign
- Add agent resource quotas → enforce in `AIKernel` before delegation

## Task Routing

- Add new subsystem → add field to `AIKernel`, initialize in `new()`, add delegating methods
- Change permission model → update `PermissionGuard` and all `AIKernel` operation wrappers
- Add async support → wrap subsystems in `tokio::sync::RwLock`, change method signatures
- Add metrics/telemetry → add `tracing` spans to each operation in kernel

## Tests

- `cargo test --lib` — all subsystem unit tests
- Integration via CLI: `cargo run --bin aicli -- [command]`
- Daemon tests: start `plicod`, send JSON requests over TCP
