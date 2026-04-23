# Module: fs/graph

Knowledge graph — typed directed graph with temporal validity, redb ACID persistence, and causal reasoning.

Status: active | Fan-in: 3 | Fan-out: 1 (redb)

## Public API

| Export | File | Description |
|--------|------|-------------|
| `KnowledgeGraph` | `mod.rs` | Trait: node/edge CRUD, traversal, paths, temporal queries |
| `ExploreDirection` | `mod.rs` | Enum: Outgoing / Incoming / Both |
| `PetgraphBackend` | `backend.rs` | Directed graph with redb 4.0 ACID persistence |
| `EdgeRecord` | `backend.rs` | Flattened edge record for JSON export |
| `KGNode` | `types.rs` | Graph node with type, label, properties, temporal validity |
| `KGEdge` | `types.rs` | Typed weighted edge with episode provenance |
| `KGNodeType` | `types.rs` | Node type enum (Entity/Fact/Document/Agent/Memory) |
| `KGEdgeType` | `types.rs` | Edge type enum (RelatedTo/DependsOn/Causes/Produces/...) |
| `KGError` | `types.rs` | Typed graph errors |
| `KGSearchHit` | `types.rs` | Graph search result with score |
| `DiskGraph` | `types.rs` | Serializable graph snapshot for JSON export |

## Dependencies (Fan-out: 1)

- `redb` 4.0 — embedded ACID KV store for node/edge persistence
- External crates: `serde`, `serde_json`, `tracing`

## Dependents (Fan-in: 3)

- `src/fs/semantic_fs/mod.rs` → `KnowledgeGraph`, `KGEdge`, `KGEdgeType` (auto-link on create)
- `src/kernel/mod.rs` → `PetgraphBackend`, all types (kernel wiring + graph ops)
- `src/kernel/ops/graph.rs` → `KGNode`, `KGEdge`, `KGEdgeType`, `KGNodeType` (graph CRUD ops)

## Persistence Architecture

Two complementary strategies:

| Layer | Format | Use | Performance |
|-------|--------|-----|-------------|
| Runtime | redb 4.0 (`kg.redb`) | Every add/remove/update | O(1) per op, ACID |
| Export | JSON (`kg_nodes.json` + `kg_edges.json`) | `save_to_disk`/`load_from_disk` | O(n) full write |

### redb Edge Key Format (v2)

`"src_id|dst_id|EdgeType|created_at_ms"` — 4-part key ensures each temporal
version is distinct, preserving full edge history across restarts.

Backward compatibility: old 3-part keys (`src|dst|type`) are auto-migrated
to 4-part format on first `open()`.

### Atomic Transactions

- `add_edge`: invalidated predecessors + new edge in one ACID transaction
- `remove_node`: node + all connected edges in one ACID transaction
- `remove_edge`: all matching edge versions in one ACID transaction

## Interface Contract

- `KnowledgeGraph::add_node()`: idempotent by node ID
- `KnowledgeGraph::add_edge()`: auto-invalidates conflicting predecessors (same src/dst/type)
- `invalidate_conflicts()`: sets `invalid_at` on predecessors AND persists to redb
- `remove_node()`: cascading — removes node + all connected edges from memory AND redb
- `save_to_disk()` / `load_from_disk()`: JSON export/import (portable, human-readable)
- Temporal queries: `get_valid_edges_at(t)`, `get_valid_nodes_at(t)` filter by valid_at/invalid_at
- Thread safety: `RwLock` on internal maps — safe for concurrent access

## Modification Risk

- Change `KGEdgeType` variants → BREAKING, update Display + all match arms + kernel + API
- Change `KGNodeType` variants → BREAKING, update Display + all match arms
- Change `KnowledgeGraph` trait → BREAKING, update PetgraphBackend + kernel + API
- Change redb table schema → requires migration in `open()`, update `migrate_old_edge_keys()`

## Task Routing

- Add edge/node type → `types.rs` enum + Display impl + FromStr
- Fix graph traversal → `backend.rs` PetgraphBackend
- Fix runtime persistence → `backend.rs` persist_* / remove_* methods
- Fix JSON export → `backend.rs` `save_to_disk` / `load_from_disk`
- Fix temporal queries → `backend.rs` temporal methods
- Fix redb migration → `backend.rs` `migrate_old_edge_keys` / `bulk_persist_to_redb`

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ~70 | `KnowledgeGraph` trait, `ExploreDirection`, re-exports |
| `types.rs` | ~520 | All graph types, enums, serialization |
| `backend.rs` | ~700 | `PetgraphBackend` — directed graph + redb 4.0 ACID persistence |
| `tests.rs` | ~750 | Unit tests (39 tests covering CRUD, traversal, temporal, redb persistence) |

## Tests

- Unit: `tests.rs` (39 tests covering CRUD, traversal, paths, temporal, redb persistence, migration)
- Regression: 5 tests specifically for redb bug fixes (edge history, invalidation persistence, cascade removal)
- Integration: `tests/kg_causal_test.rs`, `tests/node4_knowledge_event.rs`
- Critical: node/edge CRUD, path finding, temporal validity, redb roundtrip, edge history preservation
