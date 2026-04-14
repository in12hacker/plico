# Module: fs — Semantic Filesystem

AI-native filesystem: tag-based CRUD, layered context loading. No paths.

Status: active | Fan-in: 2 (kernel, aicli) | Fan-out: 1 (cas)

## Public API

| Export | File | Description |
|--------|------|-------------|
| `SemanticFS` | `semantic_fs.rs` | Filesystem: create/read/update/delete/search/audit_log |
| `Query` | `semantic_fs.rs` | Enum: ByCid/ByTags/Semantic/ByType/Hybrid |
| `SearchResult` | `semantic_fs.rs` | Result: cid + relevance score + AIObjectMeta |
| `ContextLoader` | `context_loader.rs` | L0/L1/L2 layered context |
| `ContextLayer` | `context_loader.rs` | Enum: L0(~100tok)/L1(~2ktok)/L2(full) |
| `LoadedContext` | `context_loader.rs` | Loaded context: cid + layer + content + tokens |
| `FSError` | `semantic_fs.rs` | Error: NotFound, CAS, Io |

## Dependencies (Fan-out: 1)

- `src/cas/` — all object storage delegated to CASStorage

## Dependents (Fan-in: 2)

- `src/kernel/mod.rs` → `SemanticFS::create`, `read`, `update`, `delete`, `search`
- `src/bin/aicli.rs` → via kernel

## Interface Contract

- `SemanticFS::create(content, tags, agent_id, intent)`: Stores in CAS + indexes tags. Returns CID. **Side effect**: updates tag index, writes audit entry.
- `SemanticFS::delete(cid, agent_id)`: **Logical delete only** — moves to recycle bin. No physical deletion.
- `SemanticFS::update(cid, new_content, new_tags, agent_id)`: Creates new CID (content changed → CID changed). Old CID preserved in audit log.
- `SemanticFS::search(query, limit)`: Tag-based keyword search. Returns `SearchResult` with relevance=0.8 for tag matches. **TODO**: vector semantic search.
- `ContextLoader::load(cid, layer)`: L0 cached in-memory + disk; L1 disk only; L2 from CAS directly.

## Modification Risk

- Add vector semantic search → **BREAKING** search interface, add embedding dependency
- Change delete from logical to physical → **BREAKING**, destroys audit/recoverability
- Add new Query variant → update all `match` arms in `SemanticFS::read()`
- Change tag index structure (HashMap → BTreeMap) → compatible for reads, minor for writes

## Task Routing

- Add semantic/vector search → add `ort` or `candle` + LanceDB, new `search_similar()` method
- Add knowledge graph → new module `fs/graph.rs`, wire into `SemanticFS`
- Add file type inference → extend `AIObjectMeta` or `ContentType` detection
- Add L1 pre-computation (LLM summarization) → extend `ContextLoader`, new `compute_l1()` method

## Tests

- Integration tests via kernel API (CLI-driven): `cargo run --bin aicli -- put --tags test`
- Unit tests for context loading layers
- TODO: property-based tests for CID stability
