# Module: fs/graph

Knowledge graph — typed directed graph with temporal validity, disk persistence, and causal reasoning.

Status: active | Fan-in: 3 | Fan-out: 0

## Public API

| Export | File | Description |
|--------|------|-------------|
| `KnowledgeGraph` | `mod.rs` | Trait: node/edge CRUD, traversal, paths, temporal queries |
| `ExploreDirection` | `mod.rs` | Enum: Outgoing / Incoming / Both |
| `PetgraphBackend` | `backend.rs` | Directed graph with JSON disk persistence |
| `EdgeRecord` | `backend.rs` | Internal edge index record |
| `KGNode` | `types.rs` | Graph node with type, label, properties, temporal validity |
| `KGEdge` | `types.rs` | Typed weighted edge with episode provenance |
| `KGNodeType` | `types.rs` | Node type enum (Entity/Fact/Document/Agent/Memory) |
| `KGEdgeType` | `types.rs` | Edge type enum (RelatedTo/DependsOn/Causes/Produces/...) |
| `KGError` | `types.rs` | Typed graph errors |
| `KGSearchHit` | `types.rs` | Graph search result with score |
| `DiskGraph` | `types.rs` | Serializable graph snapshot for persistence |

## Dependencies (Fan-out: 0)

External crates only: `petgraph`, `serde`, `serde_json`.

## Dependents (Fan-in: 3)

- `src/fs/semantic_fs/mod.rs` → `KnowledgeGraph`, `KGEdge`, `KGEdgeType` (auto-link on create)
- `src/kernel/mod.rs` → `PetgraphBackend`, all types (kernel wiring + graph ops)
- `src/kernel/ops/graph.rs` → `KGNode`, `KGEdge`, `KGEdgeType`, `KGNodeType` (graph CRUD ops)

## Interface Contract

- `KnowledgeGraph::add_node()`: idempotent by node ID
- `KnowledgeGraph::add_edge()`: validates source/target exist; records temporal validity
- `PetgraphBackend::save_to_disk()`: serializes to `kg_nodes.json` + `kg_edges.json` (O(n) full write)
- `PetgraphBackend::load_from_disk()`: restores full graph from JSON files
- Temporal queries: `get_valid_edges_at(t)`, `get_valid_nodes_at(t)` filter by valid_from/valid_until
- Thread safety: `RwLock` on internal graph — safe for concurrent access

## Modification Risk

- Change `KGEdgeType` variants → BREAKING, update Display + all match arms + kernel + API
- Change `KGNodeType` variants → BREAKING, update Display + all match arms
- Change `KnowledgeGraph` trait → BREAKING, update PetgraphBackend + kernel + API
- Change persistence format → BREAKING, invalidates stored graph data

## Task Routing

- Add edge/node type → `types.rs` enum + Display impl + FromStr
- Fix graph traversal → `backend.rs` PetgraphBackend
- Fix persistence → `backend.rs` `save_to_disk` / `load_from_disk`
- Fix temporal queries → `backend.rs` temporal methods

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ~70 | `KnowledgeGraph` trait, `ExploreDirection`, re-exports |
| `types.rs` | ~520 | All graph types, enums, serialization |
| `backend.rs` | ~949 | `PetgraphBackend` — directed graph + JSON persistence |
| `tests.rs` | ~686 | Unit tests (34 tests) |

## Tests

- Unit: `tests.rs` (34 tests covering CRUD, traversal, paths, temporal, persistence)
- Integration: `tests/kg_causal_test.rs`, `tests/node4_knowledge_event.rs`
- Critical: node/edge CRUD, path finding, temporal validity, disk roundtrip
