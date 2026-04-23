# Module: fs/search

Vector similarity + BM25 keyword search — provides the `SemanticSearch` trait with swappable backends.

Status: active | Fan-in: 2 | Fan-out: 0

## Public API

| Export | File | Description |
|--------|------|-------------|
| `SemanticSearch` | `mod.rs` | Trait: `upsert()` / `search()` / `delete()` / `persist_to()` / `restore_from()` |
| `SearchFilter` | `mod.rs` | Tag/type/time filter for narrowing search results |
| `SearchIndexMeta` | `mod.rs` | Metadata attached to stored embedding entries |
| `SearchHit` | `mod.rs` | Search result with CID + score + metadata |
| `SearchIndexEntry` | `mod.rs` | Serializable entry for index persistence |
| `InMemoryBackend` | `memory.rs` | Brute-force cosine similarity (MVP) |
| `HnswBackend` | `hnsw.rs` | HNSW approximate nearest neighbor (production) |
| `Bm25Index` | `bm25.rs` | BM25 keyword search index |

## Dependencies (Fan-out: 0)

External crates only: `serde`, `hnsw_rs`, `bm25`.

## Dependents (Fan-in: 2)

- `src/fs/semantic_fs/mod.rs` → `SemanticSearch`, `SearchFilter`, `SearchIndexMeta`, `Bm25Index`
- `src/kernel/mod.rs` → `InMemoryBackend`, `HnswBackend`, `Bm25Index` (backend selection)

## Interface Contract

- `SemanticSearch::search()`: returns top-k results sorted by descending score; respects `SearchFilter`
- `SearchFilter::matches()`: AND semantics for `require_tags`, OR-exclude for `exclude_tags`
- `HnswBackend`: persistent to disk via `persist_to()` / `restore_from()`; thread-safe via `RwLock`
- `InMemoryBackend`: no persistence; O(n) cosine scan; suitable for small datasets
- `Bm25Index`: BM25 with k1=1.2, b=0.75 (TREC/SIGIR defaults); `RwLock`-protected

## Modification Risk

- Change `SemanticSearch` trait → BREAKING, update both backends + SemanticFS + kernel
- Change `SearchFilter` → update all callers (SemanticFS, kernel ops)
- Change `SearchHit` fields → update API response mapping

## Task Routing

- Fix vector search accuracy → `memory.rs` (InMemory) or `hnsw.rs` (HNSW)
- Fix BM25 ranking → `bm25.rs`
- Add search backend → new file, implement `SemanticSearch`, add to `mod.rs`
- Change filter logic → `mod.rs` `SearchFilter::matches()`

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ~147 | `SemanticSearch` trait, types, `SearchFilter` |
| `memory.rs` | ~332 | `InMemoryBackend` — brute-force cosine |
| `hnsw.rs` | ~573 | `HnswBackend` — persistent HNSW ANN index |
| `bm25.rs` | ~83 | `Bm25Index` — keyword search |

## Tests

- Unit: `memory.rs` (5 tests), `hnsw.rs` (10 tests)
- Integration: `tests/semantic_search_test.rs`
- Untested: `mod.rs` (trait def + filter logic), `bm25.rs`
