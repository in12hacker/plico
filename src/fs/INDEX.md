# Module: fs

Semantic filesystem — AI-friendly CRUD (no paths), vector + BM25 hybrid search, knowledge graph, layered context loading.

Status: active | Fan-in: 2 | Fan-out: 2

## Dependents (Fan-in: 2)

- `src/kernel/mod.rs` → SemanticFS, Query, SearchResult, EmbeddingProvider, SemanticSearch, Summarizer, KnowledgeGraph, PetgraphBackend, InMemoryBackend, HnswBackend, Bm25Index, OllamaBackend, LocalEmbeddingBackend, StubEmbeddingProvider, EventType, EventRelation, EventSummary
- `src/api/semantic.rs` → EventType, EventRelation, EventSummary (type re-exports for API layer)

## Modification Risk

- Change `Query` variants → BREAKING, update kernel dispatch + CLI
- Change `SearchResult` fields → BREAKING, update API response mapping
- Change `EmbeddingProvider` trait → BREAKING, update all 4 backends
- Change `SemanticSearch` trait → BREAKING, update InMemoryBackend + HnswBackend
- Change `KnowledgeGraph` trait → BREAKING, update PetgraphBackend + kernel
- Add `KGEdgeType` variant → update Display impl + all match arms
- Add `KGNodeType` variant → update Display impl + all match arms

## Task Routing

- Fix vector search → `search/memory.rs` (InMemoryBackend) or `search/hnsw.rs` (HnswBackend)
- Add embedding backend → `embedding/` subdir, add new impl of EmbeddingProvider
- Fix KG persistence → `graph/backend.rs` PetgraphBackend::save_to_disk/load_from_disk
- Change context layer logic → `context_loader.rs` ContextLoader
- Change context budget → `context_budget.rs`
- Change summarizer → `summarizer.rs`
- Add CRUD operation → `semantic_fs/mod.rs` SemanticFS
- Change event operations → `semantic_fs/events.rs`

## Public API

### Core CRUD

| Export | File | Description |
|--------|------|-------------|
| `SemanticFS` | `semantic_fs/mod.rs` | Semantic filesystem — CAS-backed, tag-indexed, vector-searchable |
| `Query` | `semantic_fs/mod.rs` | Search query enum (ByCid/ByTags/Semantic/ByType/Hybrid) |
| `SearchResult` | `semantic_fs/mod.rs` | Result with CID, relevance score, metadata |
| `FSError` | `semantic_fs/mod.rs` | Typed filesystem errors |
| `EventType` | `semantic_fs/events.rs` | Event types for structured event storage |
| `EventRelation` | `semantic_fs/events.rs` | Event relation types |
| `EventSummary` | `semantic_fs/events.rs` | Event summary struct |

### Embedding & Search

| Export | File | Description |
|--------|------|-------------|
| `EmbeddingProvider` | `embedding/mod.rs` | Trait: text → vector embedding |
| `OllamaBackend` | `embedding/ollama.rs` | Ollama HTTP embedding backend |
| `LocalEmbeddingBackend` | `embedding/local.rs` | Python subprocess ONNX backend |
| `StubEmbeddingProvider` | `embedding/stub.rs` | Zero-vector stub (tag-only search) |
| `SemanticSearch` | `search/mod.rs` | Trait: vector similarity search |
| `InMemoryBackend` | `search/memory.rs` | Brute-force cosine similarity |
| `HnswBackend` | `search/hnsw.rs` | HNSW approximate nearest neighbor |
| `Bm25Index` | `search/bm25.rs` | BM25 keyword search index |
| `SearchFilter` | `search/mod.rs` | Tag/type/time filter for search |

### Knowledge Graph

| Export | File | Description |
|--------|------|-------------|
| `KnowledgeGraph` | `graph/mod.rs` | Trait: typed node/edge graph operations |
| `PetgraphBackend` | `graph/backend.rs` | Directed graph with disk persistence |
| `KGNode` | `graph/types.rs` | Graph node (Entity/Fact/Document/Agent/Memory) |
| `KGEdge` | `graph/types.rs` | Typed weighted edge with episode provenance |
| `KGNodeType` | `graph/types.rs` | Node type enum |
| `KGEdgeType` | `graph/types.rs` | Edge type enum |

### Context Loading & Summarization

| Export | File | Description |
|--------|------|-------------|
| `ContextLoader` | `context_loader.rs` | L0/L1/L2 layered context loading |
| `Summarizer` | `summarizer.rs` | Trait: text → compressed summary |
| `LlmSummarizer` | `summarizer.rs` | LLM-backed summarizer |

## Files

### `semantic_fs/` — Core CRUD + Events

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ~679 | SemanticFS CRUD + search + tag index + recycle bin |
| `events.rs` | ~205 | Event types, event operations |
| `tests.rs` | (co-located) | Unit tests |

### `embedding/` — Vector Embedding Backends

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ~32 | EmbeddingProvider trait + re-exports |
| `types.rs` | ~69 | Shared embedding types (Embedding, EmbedError, EmbeddingMeta) |
| `ollama.rs` | ~278 | OllamaBackend (HTTP API) |
| `local.rs` | ~230 | LocalEmbeddingBackend (Python ONNX subprocess) |
| `stub.rs` | ~36 | StubEmbeddingProvider (testing) |
| `json_rpc.rs` | ~30 | JSON-RPC embedding adapter |

### `search/` — Vector + Keyword Search

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ~147 | SemanticSearch trait, SearchFilter, re-exports |
| `memory.rs` | ~332 | InMemoryBackend (brute-force cosine) |
| `hnsw.rs` | ~573 | HnswBackend (approximate NN via hnsw_rs) |
| `bm25.rs` | ~52 | BM25 keyword search index |

### `graph/` — Knowledge Graph

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ~70 | KnowledgeGraph trait, ExploreDirection, re-exports |
| `types.rs` | ~325 | KGNode, KGEdge, KGNodeType, KGEdgeType, DiskGraph |
| `backend.rs` | ~749 | PetgraphBackend — directed graph + disk persistence |
| `tests.rs` | (co-located) | Unit tests |

### Root-level

| File | Lines | Purpose |
|------|-------|---------|
| `context_loader.rs` | ~271 | L0/L1/L2 layered context loading |
| `context_budget.rs` | ~253 | Context budget engine — adaptive multi-object assembly |
| `summarizer.rs` | ~166 | Summarizer trait, LlmSummarizer |
| `types.rs` | ~196 | Shared FS types |
| `mod.rs` | ~46 | Re-exports |

## Dependencies (Fan-out: 2)

- `src/cas/` — CASStorage for object persistence, AIObject/AIObjectMeta types
- `src/temporal/` — TemporalResolver for `list_events_by_time()`

## Interface Contract

- `SemanticFS::create()`: stores content in CAS, indexes tags, embeds for vector search, auto-generates L0 summary; returns CID
- `SemanticFS::read()`: retrieves objects by CID, tags, semantic query, content type, or hybrid
- `SemanticFS::search()`: hybrid vector + BM25 search with RRF score fusion
- `SemanticFS::delete()`: logical delete only (recycle bin); never physical delete
- `SemanticFS::restore()`: restores from recycle bin to active index
- Thread safety: all methods use `RwLock` — safe for concurrent access
- Side effect: `create()` writes to CAS, tag index, vector index, BM25 index, KG, and optionally L0 summary

## Tests

- Unit: `src/fs/semantic_fs/tests.rs`, `src/fs/graph/tests.rs`
- Integration: `tests/fs_test.rs`, `tests/semantic_search_test.rs`, `tests/embedding_test.rs`
- Critical: CRUD tests, `test_search_with_filter`, `test_hybrid_search_rrf`
