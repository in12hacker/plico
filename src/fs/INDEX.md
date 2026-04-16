# Module: fs

Semantic filesystem — AI-friendly CRUD (no paths), vector + BM25 hybrid search, knowledge graph, layered context loading.

Status: active | Fan-in: 2 | Fan-out: 2

## Dependents (Fan-in: 2)

- `src/kernel/mod.rs` → SemanticFS, Query, SearchResult, EmbeddingProvider, SemanticSearch, Summarizer, KnowledgeGraph, PetgraphBackend, InMemoryBackend, OllamaBackend, LocalEmbeddingBackend, StubEmbeddingProvider, EventType, EventRelation, EventSummary
- `src/api/semantic.rs` → EventType, EventRelation, EventSummary, UserFact, ActionSuggestion (type re-exports for API layer)

## Modification Risk

- Change `Query` variants → BREAKING, update kernel dispatch + CLI
- Change `SearchResult` fields → BREAKING, update API response mapping
- Change `EmbeddingProvider` trait → BREAKING, update all 3 backends
- Change `SemanticSearch` trait → BREAKING, update InMemoryBackend
- Change `KnowledgeGraph` trait → BREAKING, update PetgraphBackend + kernel
- Add `KGEdgeType` variant → update Display impl + all match arms
- Add `KGNodeType` variant → update Display impl + all match arms

## Task Routing

- Fix vector search → modify `src/fs/search.rs` InMemoryBackend
- Add embedding backend → modify `src/fs/embedding.rs`, add new impl of EmbeddingProvider
- Fix KG persistence → modify `src/fs/graph.rs` PetgraphBackend::save_to_disk/load_from_disk
- Change context layer logic → modify `src/fs/context_loader.rs` ContextLoader
- Change summarizer → modify `src/fs/summarizer.rs`
- Add CRUD operation → modify `src/fs/semantic_fs.rs` SemanticFS
- Change event system → modify `src/fs/semantic_fs.rs` (⚠ soul violation area)
- Change behavioral pipeline → modify `src/fs/semantic_fs.rs` (⚠ soul violation area)

## Public API

### Core CRUD (soul-aligned)

| Export | File | Description |
|--------|------|-------------|
| `SemanticFS` | `semantic_fs.rs` | Semantic filesystem — CAS-backed, tag-indexed, vector-searchable |
| `Query` | `semantic_fs.rs` | Search query enum (ByCid/ByTags/Semantic/ByType/Hybrid) |
| `SearchResult` | `semantic_fs.rs` | Result with CID, relevance score, metadata |
| `FSError` | `semantic_fs.rs` | Typed filesystem errors |

### Embedding & Search

| Export | File | Description |
|--------|------|-------------|
| `EmbeddingProvider` | `embedding.rs` | Trait: text → vector embedding |
| `OllamaBackend` | `embedding.rs` | Ollama HTTP embedding backend |
| `LocalEmbeddingBackend` | `embedding.rs` | Python subprocess ONNX backend |
| `StubEmbeddingProvider` | `embedding.rs` | Error stub (tag-only search) |
| `SemanticSearch` | `search.rs` | Trait: vector similarity search |
| `InMemoryBackend` | `search.rs` | Pure Rust brute-force cosine similarity |
| `Bm25Index` | `search.rs` | BM25 keyword search index |
| `SearchFilter` | `search.rs` | Tag/type/time filter for search |

### Knowledge Graph

| Export | File | Description |
|--------|------|-------------|
| `KnowledgeGraph` | `graph.rs` | Trait: typed node/edge graph operations |
| `PetgraphBackend` | `graph.rs` | HashMap-based directed graph with disk persistence |
| `KGNode` | `graph.rs` | Graph node (Entity/Fact/Document/Agent/Memory) |
| `KGEdge` | `graph.rs` | Typed weighted edge with episode provenance |
| `KGNodeType` | `graph.rs` | Node type enum (⚠ includes Iteration/Plan/DesignDoc — should be Entity + tags) |
| `KGEdgeType` | `graph.rs` | Edge type enum |

### Context Loading & Summarization

| Export | File | Description |
|--------|------|-------------|
| `ContextLoader` | `context_loader.rs` | L0/L1/L2 layered context loading |
| `Summarizer` | `summarizer.rs` | Trait: text → compressed summary |
| `OllamaSummarizer` | `summarizer.rs` | Ollama LLM summarizer |

### ⚠ Soul Violations (application-layer logic in filesystem)

| Export | File | Description |
|--------|------|-------------|
| `EventType` | `semantic_fs.rs` | ⚠ Hardcoded human activity types (Meeting/Travel/etc.) |
| `EventRelation` | `semantic_fs.rs` | ⚠ Hardcoded relation types (Attendee/Document/etc.) |
| `BehavioralObservation` | `semantic_fs.rs` | ⚠ Hardcoded "order_food"/"at_dinner" scenarios |
| `UserFact` | `semantic_fs.rs` | ⚠ Hardcoded "wine"/"white_congee" preferences |
| `ActionSuggestion` | `semantic_fs.rs` | ⚠ Hardcoded "提醒带红酒" action strings |
| `PatternExtractor` | `semantic_fs.rs` | ⚠ Hardcoded action_for_fact() with food/drink mapping |

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `semantic_fs.rs` | ⚠ ~3164 | SemanticFS CRUD + event/behavioral pipeline — **needs major split** |
| `graph.rs` | ⚠ ~1475 | KnowledgeGraph trait + PetgraphBackend — needs split |
| `embedding.rs` | ~735 | EmbeddingProvider + 3 backends |
| `search.rs` | ~471 | SemanticSearch + InMemoryBackend + BM25 |
| `summarizer.rs` | ~283 | Summarizer trait + OllamaSummarizer |
| `context_loader.rs` | ~231 | L0/L1/L2 context loading |
| `mod.rs` | ~44 | Re-exports |

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

- Unit: `src/fs/semantic_fs.rs` mod tests (extensive — event, behavioral, conflict)
- Integration: `tests/fs_test.rs`, `tests/semantic_search_test.rs`, `tests/embedding_test.rs`
- Critical: CRUD tests, `test_search_with_filter`, `test_hybrid_search_rrf`
