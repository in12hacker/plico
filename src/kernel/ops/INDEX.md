# Module: kernel/ops

Kernel operation groups — keeps `kernel/mod.rs` manageable by splitting domain-specific logic into focused files.

Status: active | Fan-in: 1 | Fan-out: 0

## Dependents (Fan-in: 1)

- `src/kernel/mod.rs` → all ops are `impl AIKernel` extension blocks, called from `handle_api_request` dispatch

## Task Routing

| Task | File |
|------|------|
| Fix search / CRUD / storage stats / cold eviction | `fs.rs` |
| Agent register / ensure_registered / suspend / resume | `agent.rs` |
| Memory recall / store / promote / compress | `memory.rs` |
| Session start / end / orchestrate / compound response | `session.rs` |
| Delta change tracking / watch CIDs / watch tags | `delta.rs` |
| Intent prefetch / feedback / async assembly | `prefetch.rs` |
| Hybrid Graph-RAG retrieval | `hybrid.rs` |
| KG node/edge CRUD / traverse / impact / causal_path | `graph.rs` |
| Event bus / event log operations | `events.rs` |
| Dispatch loop / result consumer | `dispatch.rs` |
| Inter-agent messaging | `messaging.rs` |
| SystemStatus / health_indicators | `dashboard.rs` |
| Permission delegation | `permission.rs` |
| External MCP tool provider | `tools_external.rs` |
| LLM model hot-swap / list providers | `model.rs` |
| Multi-layer caching (intent/search/embedding) | `cache.rs` |
| Batch multi-object CRUD | `batch.rs` |
| Agent state checkpoint / restore | `checkpoint.rs` |
| Tenant isolation | `tenant.rs` |
| Task delegation between agents | `task.rs` |
| Metrics / telemetry / performance counters | `observability.rs` |
| Memory tier TTL / promotion maintenance | `tier_maintenance.rs` |
| Distributed operation stubs | `distributed.rs` |

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ~28 | Re-exports |
| `fs.rs` | ~412 | Search, CRUD, storage stats, cold eviction |
| `agent.rs` | ~489 | Agent lifecycle (register, ensure_registered, discover) |
| `memory.rs` | ~617 | Memory tier operations (recall, store, promote, shared) |
| `session.rs` | ~752 | Session lifecycle + compound response orchestration |
| `delta.rs` | ~255 | Delta tracking (changes since seq) |
| `prefetch.rs` | ⚠ ~1842 | Intent prefetcher + feedback + async assembly — needs split |
| `hybrid.rs` | ~355 | Graph-RAG hybrid retrieval (vector + KG fusion) |
| `graph.rs` | ~784 | KG node/edge CRUD, traverse, impact, causal_path |
| `events.rs` | ~69 | Event bus + event log operations |
| `dispatch.rs` | ~95 | Dispatch loop + result consumer |
| `messaging.rs` | ~83 | Inter-agent messaging |
| `dashboard.rs` | ~237 | SystemStatus, health_indicators, cache_stats |
| `permission.rs` | ~42 | Permission delegation |
| `tools_external.rs` | ~81 | External tool provider (MCP client) |
| `model.rs` | ~361 | LLM hot-swap, list providers |
| `cache.rs` | ~458 | Multi-layer caching (intent, search, embedding) |
| `batch.rs` | ~238 | Batch operations (multi-object CRUD) |
| `checkpoint.rs` | ~475 | Agent state checkpoint / restore |
| `tenant.rs` | ~324 | Tenant isolation operations |
| `task.rs` | ~504 | Task delegation between agents |
| `observability.rs` | ~753 | Metrics, telemetry, performance counters |
| `tier_maintenance.rs` | ~222 | Memory tier TTL / promotion |
| `distributed.rs` | ~439 | Distributed operation stubs |

## Modification Risk

- All files are `impl AIKernel` blocks — changes affect kernel's public surface
- `session.rs` changes affect compound session_start response (MCP + CLI)
- `prefetch.rs` changes affect all intent-based context assembly
- `hybrid.rs` changes affect MCP + CLI hybrid retrieval

## Interface Contract

- All ops methods are `pub` or `pub(crate)` on `AIKernel`
- Each file adds methods to `AIKernel` via `impl AIKernel { ... }` blocks
- No standalone types — ops use types from `api/semantic.rs` and subsystem modules

## Tests

- Unit: co-located `#[cfg(test)]` in prefetch.rs (extensive), graph.rs, checkpoint.rs, batch.rs
- Integration: `tests/kernel_test.rs`, `tests/ai_experience_test.rs`, `tests/batch_ops_test.rs`, `tests/node4_*.rs`
