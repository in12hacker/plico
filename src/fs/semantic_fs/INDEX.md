# Module: fs/semantic_fs

Core CRUD + event storage for the semantic filesystem — AI-friendly operations with no paths.

Status: active | Fan-in: 2 | Fan-out: 4

## Public API

| Export | File | Description |
|--------|------|-------------|
| `SemanticFS` | `mod.rs` | Semantic filesystem — CAS-backed, tag-indexed, vector-searchable |
| `Query` | re-export from `fs/types.rs` | Search query enum (ByCid/ByTags/Semantic/ByType/Hybrid) |
| `SearchResult` | re-export from `fs/types.rs` | Result with CID, relevance score, metadata |
| `FSError` | re-export from `fs/types.rs` | Typed filesystem errors |
| `EventType` | `events.rs` | Event types for structured event storage |
| `EventRelation` | `events.rs` | Event relation types |
| `EventSummary` | `events.rs` | Event summary struct |
| `AuditEntry` | re-export from `fs/types.rs` | Audit trail for operations |
| `RecycleEntry` | re-export from `fs/types.rs` | Recycle bin entry |

## Dependencies (Fan-out: 4)

- `src/cas/` → `CASStorage`, `AIObject`, `AIObjectMeta`
- `src/fs/embedding/` → `EmbeddingProvider`
- `src/fs/search/` → `SemanticSearch`, `SearchFilter`, `SearchIndexMeta`, `Bm25Index`
- `src/fs/graph/` → `KnowledgeGraph`, `KGEdge`, `KGEdgeType`

## Dependents (Fan-in: 2)

- `src/kernel/mod.rs` → `SemanticFS` (kernel wires all subsystems into it)
- `src/api/semantic.rs` → `EventType`, `EventRelation`, `EventSummary` (API type re-exports)

## Interface Contract

- `SemanticFS::create()`: stores in CAS, indexes tags, embeds for vector search, auto L0 summary; returns CID
- `SemanticFS::read()`: retrieves objects by CID, tags, semantic query, type, or hybrid
- `SemanticFS::search()`: hybrid vector + BM25 with RRF score fusion
- `SemanticFS::delete()`: logical delete only (recycle bin); never physical delete
- `SemanticFS::restore()`: restores from recycle bin
- Thread safety: `RwLock` on tag index, recycle bin — safe for concurrent access

## Modification Risk

- Change `SemanticFS` constructor → BREAKING, update kernel wiring
- Change `create()` / `read()` return types → BREAKING, update kernel + API
- Change event operations → update kernel ops/events.rs

## Task Routing

- Fix CRUD operations → `mod.rs`
- Fix event operations → `events.rs`
- Fix search/query dispatch → `mod.rs` `search()` method

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ~762 | SemanticFS struct, constructor, CRUD, search, tag index |
| `events.rs` | ~392 | Event types, create_event, list_events, event operations |
| `tests.rs` | ~497 | Unit + integration tests |

## Tests

- Unit: `tests.rs` (37 tests covering CRUD, search, events)
- Integration: `tests/fs_test.rs`
- Critical: CRUD roundtrip, hybrid search, event creation
