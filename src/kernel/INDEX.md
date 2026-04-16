# Module: kernel

AI Kernel — the central orchestrator that wires together all Plico subsystems (CAS, memory, scheduler, FS, permissions).

Status: active | Fan-in: 2 | Fan-out: 5

## Dependents (Fan-in: 2)

- `src/bin/plicod.rs` → AIKernel (daemon creates kernel, handles API requests)
- `src/bin/aicli.rs` → AIKernel (CLI creates kernel for local operations)

## Modification Risk

- Add `AIKernel` public method → compatible, no breaking change
- Change `AIKernel::new()` signature → BREAKING, update both binaries
- Remove kernel method → BREAKING, update plicod.rs + aicli.rs dispatch
- Change embedding provider selection → behavioral change, affects search quality

## Task Routing

- Add new API operation → modify `src/kernel/mod.rs` (add method) + `src/bin/plicod.rs` (add dispatch)
- Change kernel initialization → modify `src/kernel/mod.rs` AIKernel::new()
- Fix embedding fallback → modify `src/kernel/mod.rs` create_embedding_provider()
- Fix dashboard → modify `src/kernel/mod.rs` dashboard_status() (⚠ soul violation area)

## Public API

| Export | File | Description |
|--------|------|-------------|
| `AIKernel` | `mod.rs` | Central orchestrator — all subsystem access |

Key methods:
- `AIKernel::new(root)` — initialize all subsystems from storage root
- `AIKernel::start_dispatch_loop()` → DispatchHandle
- `AIKernel::graph_explore_raw()` → Vec of KG neighbor tuples
- `AIKernel::list_deleted()` / `restore_deleted()` — recycle bin API
- `AIKernel::dashboard_status()` → DashboardStatus (⚠ contains hardcoded dev data)

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ⚠ ~1011 | AIKernel struct + all methods + dashboard — needs split |

## Dependencies (Fan-out: 5)

- `src/cas/` — CASStorage, AIObject, AIObjectMeta
- `src/memory/` — LayeredMemory, MemoryEntry, CASPersister, MemoryPersister
- `src/scheduler/` — AgentScheduler, Agent, Intent, dispatch types
- `src/fs/` — SemanticFS, Query, embedding/search/KG types
- `src/api/` — PermissionGuard, PermissionContext, PermissionAction; DashboardStatus types

## Interface Contract

- `AIKernel::new()`: initializes all subsystems; embedding backend auto-detected from env vars (local → ollama → stub fallback)
- All `pub(crate)` fields: accessible in integration tests, not in external crates
- Thread safety: kernel itself is not Clone; daemon wraps in `Arc<AIKernel>` for shared access
- Side effect: constructor creates directory structure under `root`

## Tests

- Unit: none (kernel is integration-level)
- Integration: `tests/kernel_test.rs`
- Critical: `test_kernel_create_and_read`, `test_semantic_search_through_kernel`
