# Module: fs — Semantic Filesystem

AI-native filesystem: tag-based CRUD, layered context loading, vector semantic search. No paths.

Status: active | Fan-in: 2 (kernel, aicli) | Fan-out: 1 (cas)

## Public API

| Export | File | Description |
|--------|------|-------------|
| `SemanticFS` | `semantic_fs.rs` | Filesystem: create/read/update/delete/search/audit_log |
| `Query` | `semantic_fs.rs` | Enum: ByCid/ByTags/Semantic{Hybrid}/ByType/Hybrid |
| `SearchResult` | `semantic_fs.rs` | Result: cid + relevance score + AIObjectMeta |
| `FSError` | `semantic_fs.rs` | Error: NotFound, CAS, Io, Embedding |
| `ContextLoader` | `context_loader.rs` | L0/L1/L2 layered context |
| `ContextLayer` | `context_loader.rs` | Enum: L0(~100tok)/L1(~2ktok)/L2(full) |
| `EmbeddingProvider` | `embedding.rs` | Trait: embed/embed_batch/dimension/model_name |
| `OllamaBackend` | `embedding.rs` | Ollama daemon backend (default, MVP) |
| `LocalONNXBackend` | `embedding.rs` | Stub for future native ONNX inference |
| `SemanticSearch` | `search.rs` | Trait: upsert/delete/search/len |
| `InMemoryBackend` | `search.rs` | Pure Rust HNSW-free cosine similarity (MVP) |
| `SearchFilter` | `search.rs` | Filter: require_tags/exclude_tags/content_type |
| `SearchHit` | `search.rs` | A search match: cid + score + meta |

## Dependencies (Fan-out: 1)

- `src/cas/` — all object storage delegated to CASStorage

## Dependents (Fan-in: 2)

- `src/kernel/mod.rs` → `SemanticFS::create`, `read`, `update`, `delete`, `search`
- `src/bin/aicli.rs` → via kernel

## Interface Contract

- `SemanticFS::create(content, tags, agent_id, intent)`: Stores in CAS + indexes tags + auto-embeds for semantic search. Returns CID. **Side effects**: tag index update, search index upsert, audit entry.
- `SemanticFS::search(query, limit)`: Vector semantic search using `EmbeddingProvider` + `SemanticSearch`. Falls back to tag-based keyword search if embeddings unavailable.
- `SemanticFS::delete(cid, agent_id)`: **Logical delete only** — moves to recycle bin AND removes from search index.
- `EmbeddingProvider::embed(text)`: Returns `Embedding` (Vec<f32>) or error. Thread-safe.
- `SemanticSearch::upsert(cid, embedding, meta)`: Stores/updates embedding for a CID.
- `SemanticSearch::search(query, k, filter)`: Returns top-k `SearchHit` sorted by cosine similarity.

## Configuration

Embedding backend configured via environment variables:
- `OLLAMA_URL` (default: `http://localhost:11434`)
- `OLLAMA_EMBEDDING_MODEL` (default: `all-minilm-l6-v2`)

## Modification Risk

- Add `LanceDBBackend` → implement `SemanticSearch`, non-breaking
- Change `SemanticSearch` trait → breaking for all implementations
- Change `EmbeddingProvider` trait → breaking for all implementations
- Add new `Query` variant → update all `match` arms in `SemanticFS::read()`

## Task Routing

- Add LanceDB vector search → implement `SemanticSearch` for LanceDB, update kernel
- Add knowledge graph → new module `fs/graph.rs`, wire into `SemanticFS`
- Add LLM summarization → extend `ContextLoader`, new `compute_l1()` method

## Tests

- `tests/fs_test.rs` — 18 tests: CRUD, search, tag index, context loading, update/delete
- `src/fs/search.rs` — 5 tests: cosine similarity, upsert, tag filter, delete, replace
- `src/fs/embedding.rs` — 1 test: backend creation (no server needed)
