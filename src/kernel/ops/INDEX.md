# Module: kernel/ops

Kernel operation groups — keeps `kernel/mod.rs` manageable by splitting domain-specific logic into focused files.

Status: active | Fan-in: 1 | Fan-out: 0

## Public API

Most legacy ops are `impl AIKernel` extension blocks. Projection orchestration uses sealed crate-local
runtime/controller types owned only by `AIKernel`.

| Group | File | Key Methods |
|-------|------|-------------|
| Storage | `fs.rs` | `search()`, object CRUD, `get_storage_stats()` |
| Agent | `agent.rs` | `register_agent()`, `ensure_registered()`, `discover_agents()` |
| Memory | `memory.rs` | `memory_recall()`, `memory_store()`, `promote_memory()` |
| Projection runtime | `projection_runtime.rs` | lifecycle, wake queue, worker health, owner maintenance |
| Projection controller | `projection_controller/` | canonical guard, reconciliation, lease, artifact and Ready sequencing |
| Readiness | `readiness.rs` | side-effect-free canonical/persister/worker/provider configuration state |
| Cognitive pipeline | `cognitive_pipeline.rs` | bounded background work plus queue-drain watermarks and root-attempt outcome telemetry |
| Session | `session.rs` | durable `start_session()`, `end_session()`, timeout cleanup, `compound_response()` |
| Graph | `graph.rs` | `kg_add_node()`, `kg_traverse()`, `kg_impact()`, `causal_path()` |
| Checkpoint | `checkpoint.rs` | internal checkpoint capture/store; live-memory restore is unsupported |
| Observability | `observability.rs` | `metrics_snapshot()`, `health_indicators()` |
| Prefetch | `prefetch.rs` | `prefetch_for_intent()`, `feedback_score()` |

The durable `memory_embedding` manifest is the sole retry/lease/status truth; the memory queue is only
a wake-up hint. There is no inline embedding state, runtime-only retry truth, v1 reader or dual write.

## Dependencies (Fan-out: 0)

All ops depend on `AIKernel` fields (self-referencing). No external module dependencies — ops are the leaf layer.

## Dependents (Fan-in: 1)

- `src/kernel/mod.rs` → owns operation state and the sole projection runtime; public operations dispatch through `public_service.rs`

## Task Routing

| Task | File |
|------|------|
| Fix search / CRUD / storage stats | `fs.rs` |
| Agent register / ensure_registered / suspend / resume | `agent.rs` |
| Memory recall / store / promote / compress | `memory.rs` |
| Projection lifecycle / owner maintenance | `projection_runtime.rs` |
| Projection reconcile / worker sequencing | `projection_controller/` |
| Cognitive indexing completion / queue progress | `cognitive_pipeline.rs` + `readiness.rs` |
| Session start / end / orchestrate / compound response | `session.rs` |
| Delta change tracking / watch CIDs / watch tags | `delta.rs` |
| Intent prefetch / feedback / async assembly | `prefetch.rs` |
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
| Task delegation between agents | `task.rs` |
| Metrics / telemetry / performance counters | `observability.rs` |
| Memory tier TTL / promotion maintenance | `tier_maintenance.rs` |

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ~28 | Re-exports |
| `fs.rs` | Search, CRUD, storage stats |
| `agent.rs` | ~489 | Agent lifecycle (register, ensure_registered, discover) |
| `memory.rs` | Canonical memory writes, lexical recall, and procedural memory operations |
| `session.rs` | Session store, durable lifecycle primitives, profile helpers |
| `delta.rs` | ~255 | Delta tracking (changes since seq) |
| `prefetch.rs` | ⚠ ~1842 | Intent prefetcher + feedback + async assembly — needs split |
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
| `task.rs` | ~504 | Task delegation between agents |
| `observability.rs` | ~753 | Metrics, telemetry, performance counters |
| `tier_maintenance.rs` | ~222 | Memory tier TTL / promotion |
| `projection_runtime.rs` | Projection lifecycle, bounded wakes, worker shutdown and readiness |
| `projection_controller/` | Typed reconcile/claim/embed/complete pipeline over the manifest core |
| `readiness.rs` | <220 | Read-only runtime readiness and structured component-state logging without provider or storage probes |

## Modification Risk

- Operation extensions and the sealed projection runtime are owned by `AIKernel`; changes can affect the public service
- `session.rs` changes affect durable session state and EventBus watermark responses (MCP + CLI)
- `prefetch.rs` changes affect all intent-based context assembly

## Interface Contract

- Legacy operation methods are on `AIKernel`; projection controller/store writers are not public capabilities.
- Public dispatch uses `api/public` typed commands. Internal semantic commands do not enter transports.
- Projection lock order is runtime lifecycle → canonical proof → projection store.
- Cognitive tasks retain one channel receiver and a configured bounded execution set (default four tasks).
  The completed watermark advances only across a contiguous accepted prefix, so a caller can wait for a
  captured ingest boundary without treating a later searchable probe as proof that older work finished.
  Vector-success, lexical-degradation and failure counters describe root task attempts, including synchronous
  queue fallback; they do not prove per-CID source-watermark completeness.

## Tests

- Unit: co-located `#[cfg(test)]` in prefetch.rs (extensive), graph.rs, checkpoint.rs, batch.rs
- Integration: `tests/kernel_test.rs`, `tests/ai_experience_test.rs`, `tests/batch_ops_test.rs`, `tests/node4_*.rs`
