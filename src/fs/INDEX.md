# Module: fs — Semantic Filesystem

AI-native filesystem: tag-based CRUD, layered context loading, vector semantic search. No paths.

Status: active | Fan-in: 1 (kernel) | Fan-out: 1 (cas)

## Public API

| Export | File | Description |
|--------|------|-------------|
| `SemanticFS` | `semantic_fs.rs` | Filesystem: create/read/update/delete/search/list_tags/audit_log/list_deleted/restore |
| `RecycleEntry` | `semantic_fs.rs` | Soft-deleted object entry: cid/deleted_at/original_meta (persisted to recycle_bin.json) |
| `Query` | `semantic_fs.rs` | Enum: ByCid/ByTags/Semantic/ByType/Hybrid |
| `SearchResult` | `semantic_fs.rs` | Result: cid + relevance score + AIObjectMeta |
| `AuditEntry` | `semantic_fs.rs` | Audit log entry: timestamp/action/cid/agent_id |
| `AuditAction` | `semantic_fs.rs` | Enum: Create/Update{previous_cid}/Delete |
| `FSError` | `semantic_fs.rs` | Error: NotFound, CAS, Io, Embedding |
| `ContextLoader` | `context_loader.rs` | L0/L1/L2 layered context; L0/L1 computed on-demand from CAS if not pre-cached |
| `ContextLayer` | `context_loader.rs` | Enum: L0(~100tok)/L1(~2ktok)/L2(full) |
| `LoadedContext` | `context_loader.rs` | Loaded context: cid/layer/content/tokens_estimate |
| `EmbeddingProvider` | `embedding.rs` | Trait: embed/embed_batch/dimension/model_name |
| `OllamaBackend` | `embedding.rs` | Ollama daemon backend; safe to call from tokio::spawn |
| `LocalEmbeddingBackend` | `embedding.rs` | Python subprocess ONNX backend (bge-small-en-v1.5) |
| `StubEmbeddingProvider` | `embedding.rs` | Always errors — enables tag-only fallback in tests; `#[derive(Default)]` |
| `EmbedError` | `embedding.rs` | Error: Http/Ollama/Onnx/ModelNotFound/Subprocess/etc. |
| `SemanticSearch` | `search.rs` | Trait: upsert/delete/search/len/list_by_filter |
| `InMemoryBackend` | `search.rs` | Pure Rust brute-force cosine similarity (MVP, up to ~10k entries) |
| `SearchFilter` | `search.rs` | Filter: require_tags/exclude_tags/content_type |
| `SearchHit` | `search.rs` | A search match: cid + score + meta |
| `SearchIndexMeta` | `search.rs` | Metadata stored in vector index per entry |
| `KnowledgeGraph` | `graph.rs` | Trait: add_node/add_edge/get_neighbors/remove_node/authority_score |
| `PetgraphBackend` | `graph.rs` | HashMap-based directed graph (prod-ready) |
| `KGNode` | `graph.rs` | Graph node: id/label/node_type/agent_id/metadata |
| `KGNodeType` | `graph.rs` | Enum: Document/Entity/Concept/Fact/Agent |
| `KGEdge` | `graph.rs` | Graph edge: source/target/edge_type/weight/created_at |
| `KGEdgeType` | `graph.rs` | Enum: AssociatesWith/Mentions/Follows/PartOf/RelatedTo/Causes/Reminds/SimilarTo |
| `KGSearchHit` | `graph.rs` | A graph search hit: node + edge_type + scores |
| `KGError` | `graph.rs` | Error: NodeNotFound, EdgeAlreadyExists, GraphError |
| `Summarizer` | `summarizer.rs` | Trait: summarize(content, layer) → String |
| `OllamaSummarizer` | `summarizer.rs` | Ollama chat backend for L0/L1 summarization |
| `SummaryLayer` | `summarizer.rs` | Enum: L0/L1 — controls prompt template |

## Dependencies (Fan-out: 1)

- `src/cas/` — CASStorage stored as `Arc<CASStorage>` in SemanticFS; passed to ContextLoader for L2 loading

## Dependents (Fan-in: 1)

- `src/kernel/mod.rs` → SemanticFS, KnowledgeGraph, EmbeddingProvider, all fs types

## Interface Contract

- `SemanticFS::create(content, tags, agent_id, intent)`: Stores in CAS + updates tag index + upserts to search index (zero vector if embedding unavailable, allowing filter queries to still work) + upserts to knowledge graph. Returns CID.
- `SemanticFS::new(...)`: Rebuilds vector index from all persisted CAS objects on startup; loads `recycle_bin.json` if present.
- `SemanticFS::update(old_cid, content, new_tags, agent_id)`: **Always** removes old CID from tag index and adds new CID, regardless of whether tags changed (bug fix — CID always changes).
- `SemanticFS::delete(cid, agent_id)`: Logical delete — moves entry to in-memory recycle bin and persists to `recycle_bin.json`. Object data remains in CAS.
- `SemanticFS::list_deleted()`: Returns all soft-deleted entries sorted by `deleted_at` descending.
- `SemanticFS::restore(cid, agent_id)`: Removes from recycle bin and re-indexes to tag index + search index + knowledge graph.
- `SemanticFS::create(...)`: Auto-generates L0 summary via `Summarizer` after store (if summarizer available; failure only warns).
- `SemanticFS::read(Query::ByType(t))`: Scans search index by `content_type` field; works even without embeddings.
- `SemanticFS::read(Query::Hybrid{tags, semantic, content_type})`: Combines vector search + tag + type filter.
- `ContextLoader::load(cid, L0)`: Returns pre-cached summary if available; otherwise computes on-demand via `compute_l0()` (LLM summarizer if present, heuristic fallback). Never returns a placeholder string.
- `ContextLoader::load(cid, L1)`: Returns pre-computed L1 file if available; otherwise falls back to leading 8000 chars of CAS content (~2000 tokens). Never returns a placeholder string.
- `ContextLoader::load(cid, L2)`: Reads actual full content from `Arc<CASStorage>`. Returns `Err` if CID not found.
- `OllamaBackend::embed()`: Safe to call from within a tokio `spawn` task (uses `block_in_place`).
- `OllamaSummarizer::summarize()`: Safe to call from within a tokio `spawn` task (uses `block_in_place`).

## Configuration

Embedding backend (set via `EMBEDDING_BACKEND` env):
- `"local"` (default) — Python subprocess (bge-small-en-v1.5, `pip install transformers onnxruntime`)
- `"ollama"` — Ollama daemon (`OLLAMA_URL`, `OLLAMA_EMBEDDING_MODEL`)
- `"stub"` — tag-only search (no external dependencies)

## Modification Risk

- Change `SearchFilter::matches()` logic → affects all callers (ByType, Hybrid, Semantic)
- Change `SemanticSearch` trait (add `list_by_filter`) → implement in all backends
- Change `EmbeddingProvider` trait → breaking for all implementations
- Add new `Query` variant → add match arm in `SemanticFS::read()`
- Change `ContentType` display format → affects search index `content_type` field string matching
- Change `RecycleEntry` struct fields → affects `recycle_bin.json` deserialization (migration needed)

## Task Routing

- Add LanceDB vector search → implement `SemanticSearch` + `list_by_filter` for LanceDB, swap in kernel
- Add LLM entity extraction to knowledge graph → extend `PetgraphBackend::upsert_document`, call LLM
- Add L1 auto-generation on create → extend L0 auto-gen pattern in `SemanticFS::create()` for L1 layer

## Tests

- `src/fs/semantic_fs.rs` — 3 unit tests: tag-index bug, ByType, Hybrid, L2 real content
- `src/fs/search.rs` — 5 tests: cosine similarity, upsert, tag filter, delete, replace
- `src/fs/graph.rs` — 8 tests: CRUD, edges, neighbors, cascade, centrality
- `src/fs/summarizer.rs` — 2 tests: layer prompts, max_chars
- `src/fs/embedding.rs` — 1 test: backend creation (no server needed)
- `tests/fs_test.rs` — 23 integration tests: CRUD, tag search, context loading (L0/L1/L2 on-demand), update/delete, recycle bin (list/restore/persist/restore-error)
- `tests/semantic_search_test.rs` — 4 E2E tests (require Ollama; skip if unavailable)
- `tests/embedding_test.rs` — 5 E2E tests (require Python + onnxruntime; skip if unavailable)
