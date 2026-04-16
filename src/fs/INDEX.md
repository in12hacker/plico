# Module: fs — Semantic Filesystem

AI-native filesystem: tag-based CRUD, layered context loading, vector semantic search. No paths.

Status: active | Fan-in: 1 (kernel) | Fan-out: 1 (cas)

## Public API

| Export | File | Description |
|--------|------|-------------|
| `SemanticFS` | `semantic_fs.rs` | Filesystem: create/read/update/delete/search/list_tags/audit_log/list_deleted/restore/create_event/list_events/event_attach/event_add_observation/event_get_observations/add_user_fact/get_user_facts_for_subject/infer_suggestions_for_event |
| `EventType` | `semantic_fs.rs` | Enum: Meeting/Presentation/Review/Interview/Travel/Entertainment/Social/Work/Personal/Other |
| `EventMeta` | `semantic_fs.rs` | Event metadata (stored in KGNode.properties): label/event_type/start_time/attendee_ids/related_cids/observation_ids |
| `EventRelation` | `semantic_fs.rs` | Enum: Attendee/Document/Media/Decision — relation type when attaching to event |
| `EventSummary` | `semantic_fs.rs` | Lightweight event listing: id/label/event_type/start_time/attendee_count/related_count |
| `ActionSuggestion` | `semantic_fs.rs` | AI action suggestion with inline preference + reasoning_chain + confidence + status |
| `SuggestionStatus` | `semantic_fs.rs` | Enum: Pending/Confirmed/Dismissed |
| `PREFERENCE_MIN_CONFIDENCE` | `semantic_fs.rs` | Minimum confidence threshold (0.4) for surfacing suggestions |
| `PREFERENCE_HIGH_CONFIDENCE` | `semantic_fs.rs` | Auto-fire threshold (0.8); no user confirmation needed |
| `PatternExtractor` | `semantic_fs.rs` | Groups observations → UserFacts → ActionSuggestions pipeline: extract() + extract_and_suggest() |
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
| `KnowledgeGraph` | `graph.rs` | Trait: add_node/add_edge/get_neighbors/remove_node/authority_score/all_node_ids |
| `PetgraphBackend` | `graph.rs` | HashMap-based directed graph (prod-ready) |
| `KGNode` | `graph.rs` | Graph node: id/label/node_type/agent_id/metadata |
| `KGNodeType` | `graph.rs` | Enum: Document/Entity/Concept/Fact/Agent |
| `KGEdge` | `graph.rs` | Graph edge: source/target/edge_type/weight/created_at |
| `KGEdgeType` | `graph.rs` | Enum: AssociatesWith/Follows/Mentions/PartOf/RelatedTo/SimilarTo + HasAttendee/HasDocument/HasMedia/HasDecision (event) + HasPreference/SuggestsAction/MotivatedBy (reasoning) |
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
- `SemanticFS::create_event(...)`: Creates event as KG node (Entity type + EventMeta in properties). Works even if KG is absent (returns ID, no-op for KG). Indexes by tags for `list_events` queries.
- `SemanticFS::list_events(since, until, tags, event_type)`: Full-scan over KG Entity nodes filtered by time range + tag intersection + EventType. Returns empty vec if KG is not initialized.
- `SemanticFS::list_events_by_time(time_expr, tags, event_type, resolver)`: Resolves natural-language time expression via `TemporalResolver` → delegates to `list_events`. Returns `Err` if resolver cannot parse expression.
- `SemanticFS::event_attach(event_id, target_id, relation, agent_id)`: Adds typed KG edge + updates EventMeta attendee_ids/related_cids. Returns `Err` if KG is absent or event not found.
- `SemanticFS::event_add_observation(event_id, observation_id)`: Associates a behavioral observation ID with an event (updates EventMeta.observation_ids, no KG edge). Returns `Err` if KG is absent or event not found.
- `SemanticFS::event_get_observations(event_id)`: Returns all observation IDs associated with an event. Returns `Err` if KG is absent or event not found.
- `SemanticFS::add_user_fact(fact)`: Store a UserFact (promoted from behavioral observations) in the preference store, keyed by subject_id.
- `SemanticFS::get_user_facts_for_subject(subject_id)`: Retrieve all UserFacts for a given person ID.
- `SemanticFS::infer_suggestions_for_event(event_id)`: Multi-hop query: Event → attendees → UserFacts → ActionSuggestions. Returns action suggestions for all attendees with known preferences.
- `KnowledgeGraph::all_node_ids()`: Returns IDs of all nodes in the graph. Used for full-scan queries without tag filtering.
- `HeuristicTemporalResolver` (in `src/temporal/`): Implements `TemporalResolver`; resolves "几天前"/"上周"/"上个月"/etc. to Unix-ms ranges via rule-based heuristics. Safe to call from `tokio::spawn`.
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
- Add `KGEdgeType` variant → update Display impl + exhaustive `match` arms; serialization format changes
- Add `KnowledgeGraph::all_node_ids` → implement in all KG backends
- Add `SuggestionStatus` variant → update ActionSuggestion serde + any match arms
- `SemanticFS::create_event` stores in KG: if KG is absent, silently no-ops (returns ID anyway) — callers must handle this if KG is required

## Task Routing

- Add LanceDB vector search → implement `SemanticSearch` + `list_by_filter` for LanceDB, swap in kernel
- Add LLM entity extraction to knowledge graph → extend `PetgraphBackend::upsert_document`, call LLM
- Add L1 auto-generation on create → extend L0 auto-gen pattern in `SemanticFS::create()` for L1 layer

## Tests

- `src/fs/semantic_fs.rs` — 20 unit tests: CRUD, tag-index bug, ByType, Hybrid, L2 real content, event CRUD, list_events_by_time, ActionSuggestion (is_actionable/needs_confirmation/is_too_uncertain), serde roundtrips
- `src/fs/search.rs` — 5 tests: cosine similarity, upsert, tag filter, delete, replace
- `src/fs/graph.rs` — 8 tests: CRUD, edges, neighbors, cascade, centrality
- `src/fs/summarizer.rs` — 2 tests: layer prompts, max_chars
- `src/fs/embedding.rs` — 1 test: backend creation (no server needed)
- `tests/fs_test.rs` — 23 integration tests: CRUD, tag search, context loading (L0/L1/L2 on-demand), update/delete, recycle bin (list/restore/persist/restore-error)
- `tests/semantic_search_test.rs` — 4 E2E tests (require Ollama; skip if unavailable)
- `tests/embedding_test.rs` — 5 E2E tests (require Python + onnxruntime; skip if unavailable)
