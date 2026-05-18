# Memory & Search Internals: Cross-Session Persistence, Causal Retrieval, Reranker Integration

Technical deep-dive based on source code analysis. All file references use `file:line` format.

---

## Part 1: Cross-Session Memory Persistence

### 1.1 Session Lifecycle Architecture

Sessions are managed by `SessionStore` (`src/kernel/ops/session.rs:304-550`), an in-memory store backed by JSON persistence files.

**Key data structures:**

- `ActiveSession` (`session.rs:247-257`): Tracks `session_id`, `agent_id`, `start_seq` (event bus sequence at session start), `current_intent`, and timestamps.
- `CompletedSession` (`session.rs:282-292`): Records ended sessions with `tokens_used` and optional `SessionSummary` (top tags, object count, intent, summary CID).
- `SessionStore` (`session.rs:305-310`): Two `RwLock<HashMap>` — one for active sessions, one for completed sessions keyed by agent_id.

### 1.2 What Happens During StartSession

`start_session_orchestrate()` (`session.rs:567-701`) performs these steps:

1. **Captures current event seq** from `EventBus` (`session.rs:574`). This is the baseline for delta computation.
2. **Generates a UUID session_id** (`session.rs:577`).
3. **Creates session in store** with `session_store.start_session()` (`session.rs:580-584`).
4. **Persists to disk** immediately via `session_store.persist(root)` (`session.rs:590-592`). Writes to `sessions.json` and `completed_sessions.json` using atomic tmp-rename.
5. **Warms intent cache** from agent profile hot objects (`session.rs:597-601`).
6. **Registers with CognitiveLoop** for proactive optimization (`session.rs:604-614`).
7. **Computes delta** since `last_seen_seq` — returns `ChangeEntry` list of what changed (`session.rs:624-635`).
8. **Stores warm context in CAS** if `intent_hint` is provided — creates a CAS object tagged `warm-context` and returns its CID (`session.rs:638-687`).

**Critical finding: Sessions do NOT track which CAS objects were created during them.** There is no `session_id` field on CAS objects or memory entries. The `start_seq`/`last_seq` mechanism uses the EventBus sequence number to compute deltas, but this is a global counter, not a per-session object list.

### 1.3 What Happens During EndSession

`end_session_orchestrate()` (`session.rs:704-825`) performs:

1. **Validates session ownership** (`session.rs:714-722`).
2. **Runs Memory Consolidation Cycle** (`session.rs:737-763`):
   - `TierMaintenance::run_maintenance_cycle()` — promotes important ephemeral entries to working/long-term.
   - `memory.consolidate_agent()` — dedup, contradiction detection, decay/boost.
3. **Clears only ephemeral tier** (`session.rs:764`): `memory.clear_ephemeral(agent_id)`. Working and long-term memories are **preserved**.
4. **Ends CognitiveLoop session** for skill extraction (`session.rs:767-779`).
5. **Generates SessionSummary** with `top_tags`, `object_count`, `intent` (`session.rs:788-793`).
6. **Records completion** via `session_store.end_session_with_summary()` (`session.rs:796`).
7. **Persists to disk** (`session.rs:799-801`).

**Key insight**: EndSession clears ephemeral memory but preserves working and long-term tiers. The session summary is stored in `CompletedSession` for cross-session recall.

### 1.4 CAS Object Lifecycle — No Session Awareness

`semantic_create()` (`src/kernel/ops/fs.rs:53-102`) creates objects with:
- Content bytes → SHA-256 CID
- Tags, agent_id, intent
- Notifies KG builder for async entity extraction
- Emits `ObjectStored` event

**There is no `session_id` field on CAS objects.** The `AIObjectMeta` struct (defined in `src/cas/`) contains `created_by` (agent_id), `tenant_id`, `tags`, `created_at`, but no session identifier.

`semantic_search_with_time()` (`fs.rs:121-160`) filters by:
- Tenant isolation (`meta.tenant_id`)
- Tag requirements/exclusions
- Time range (`since`/`until` timestamps)
- **No session filtering whatsoever**

### 1.5 How Cross-Session Memory Actually Works

The benchmark test `_test_cross_session_memory()` (`benchmarks/src/plico_benchmarks/suites/session_lifecycle.py:118-164`) reveals the mechanism:

**Session 1:**
```python
self.client.start_session(agent_id, goals=["Create initial knowledge"])
# Creates memories via client.create() — JSON-RPC "create" method
self.client.end_session(agent_id, session_id=sid1)
```

**Session 2:**
```python
self.client.start_session(agent_id, goals=["Verify previous knowledge"])
# Searches via client.search() — JSON-RPC "search" method
# Also tests client.recall() — JSON-RPC "recall" method
self.client.end_session(agent_id, session_id=sid2)
```

**Why it works**: CAS objects are **session-agnostic**. They persist in the filesystem (content-addressed storage) regardless of session boundaries. The `search` method queries the semantic FS which has no session filter. The `recall` method queries `LayeredMemory` which stores entries by agent_id, not session_id.

### 1.6 Client API Methods (JSON-RPC Calls)

From `benchmarks/src/plico_benchmarks/core/client.py`:

| Method | JSON-RPC call | Purpose |
|--------|---------------|---------|
| `create()` (`client.py:135-147`) | `{"method": "create", "content": ..., "tags": ..., "agent_id": ...}` | Store in CAS via semantic_create |
| `search()` (`client.py:162-183`) | `{"method": "search", "query": ..., "agent_id": ..., "limit": ...}` | Semantic search across all CAS objects |
| `remember()` (`client.py:185-188`) | `{"method": "remember", "agent_id": ..., "content": ...}` | Store in ephemeral (L0) memory tier |
| `recall()` (`client.py:190-200`) | `{"method": "recall", "agent_id": ..., "query": ..., "limit": ...}` | Retrieve from layered memory (all tiers) |
| `remember_long_term()` (`client.py:297-312`) | `{"method": "remember_long_term", "agent_id": ..., "content": ..., "importance": ...}` | Store in long-term (L2) tier with embedding |
| `start_session()` (`client.py:278-284`) | `{"method": "start_session", "agent_id": ..., "goals": [...]}` | Start session orchestration |
| `end_session()` (`client.py:286-295`) | `{"method": "end_session", "agent_id": ..., "session_id": ...}` | End session with consolidation |

**`create` vs `remember`**: `create` stores in CAS (content-addressed, searchable via semantic search). `remember` stores in the layered memory system (ephemeral tier, accessible via `recall`). Both persist across sessions, but through different storage backends.

### 1.7 Session Persistence Summary

```
StartSession
  ├── Create ActiveSession in store
  ├── Persist to sessions.json
  ├── Warm intent cache from agent profile
  ├── Compute delta since last_seen_seq
  └── Store warm context in CAS (if intent_hint)

[Session Active — CAS objects and memory entries created without session_id]

EndSession
  ├── Run tier maintenance (promote ephemeral → working/long-term)
  ├── Run memory consolidation (dedup, contradiction, decay/boost)
  ├── Clear ONLY ephemeral tier
  ├── Generate SessionSummary
  ├── Record CompletedSession
  └── Persist to completed_sessions.json
```

**Cross-session persistence is a side effect of session-agnostic storage**, not an explicit feature. CAS objects and working/long-term memory entries survive session boundaries because they are never scoped to a session.

---

## Part 2: Causal Retrieval Mechanism

### 2.1 CausalGraph Data Structure

`CausalGraph` (`src/memory/causal.rs:27-31`) is a lightweight in-memory graph built from `MemoryEntry` slices:

```rust
pub struct CausalGraph {
    children: HashMap<String, Vec<(String, CausalEdge)>>,
    parents: HashMap<String, Vec<(String, CausalEdge)>>,
    all_ids: HashSet<String>,
}
```

Two edge types (`causal.rs:14-20`):
- `CausalEdge::Caused` — A caused B (B.causal_parent == A.id)
- `CausalEdge::Supersedes` — B supersedes A (B.supersedes == A.id)

**Construction** (`causal.rs:35-71`): Iterates all `MemoryEntry` items, building parent/child maps from `causal_parent` and `supersedes` fields. O(n) construction, immutable once built.

**Traversal operations:**
- `ancestors()` (`causal.rs:75-77`): Walk backward following causal edges only, returns oldest-first chain.
- `descendants()` (`causal.rs:86-106`): BFS forward from start node, all edge types.
- `root_cause()` (`causal.rs:110-113`): Follows causal ancestors to the root.
- `latest_version()` (`causal.rs:116-137`): Follows supersession chains forward.
- `shortest_path_len()` (`causal.rs:175-200`): BFS undirected shortest path.
- `common_ancestors()` (`causal.rs:203-207`): Intersection of two ancestor sets.

### 2.2 How causal_parent Links Are Created

The `causal_parent` field on `MemoryEntry` is set in exactly one place: the **ingest pipeline** inside `remember_long_term_scoped()` (`src/kernel/ops/memory.rs:343-506`).

When a long-term memory is stored:

1. The original entry is created with `causal_parent: None` (`memory.rs:395`).
2. The ingest pipeline runs (`memory.rs:444-500`):
   - **Regex preference extraction** (always, zero cost): `extract_preference_signals(&text)`
   - **LLM fact extraction** (only when `PLICO_INGEST_EXTRACT=1`): `extract_facts(llm, &text)`
3. Each extracted fact is stored as a **new MemoryEntry** with:
   - `causal_parent: Some(entry_id.clone())` — pointing to the original entry (`memory.rs:485`)
   - `importance: importance.saturating_add(5).min(100)` — slightly higher than parent
   - `memory_type: fact.fact_type.to_memory_type()` — typed based on extraction

**This means causal_parent links are created automatically during memory ingest, not explicitly by the user.** The parent is always the original memory entry that was ingested; the children are the extracted facts.

### 2.3 How Causal Reasoning Is Tested

The benchmark `_test_causal_retrieval()` (`benchmarks/src/plico_benchmarks/suites/causal_reasoning.py:147-215`) tests causal retrieval through **content-level causality**, not through the CausalGraph:

1. **Creates cause/effect pairs** as separate CAS objects (`causal_reasoning.py:156-173`):
   ```python
   causal_pairs = [
       ("The server crashed due to a memory leak in the connection pool",
        "After the memory leak was fixed, the server stability improved to 99.9% uptime"),
       # ... 5 pairs total
   ]
   for cause, effect in causal_pairs:
       resp_c = self.client.create(cause, tags=["causal", "cause"], agent_id=agent_id)
       resp_e = self.client.create(effect, tags=["causal", "effect"], agent_id=agent_id)
   ```

2. **Searches for cause, checks if effect appears** (`causal_reasoning.py:186-194`):
   ```python
   resp = self.client.search(cause_text, agent_id=agent_id, limit=10)
   result_cids = {h.get("cid", "") for h in resp.get("results", [])}
   if effect_cid in result_cids:
       cause_finds_effect += 1
   ```

3. **Searches for effect, checks if cause appears** (`causal_reasoning.py:197-205`).

4. **Uses LLM-as-judge** for semantic correctness (`causal_reasoning.py:191-194`).

**Key finding**: The benchmark does NOT use the CausalGraph or causal_parent links. It relies on **semantic similarity** between cause and effect text. The `bidirectional_rate` metric measures whether the search pipeline can retrieve semantically related content in both directions.

The CausalGraph is used in the `recall_routed()` pipeline (`memory.rs:778-783`):
```rust
let causal_graph = if entries.iter().any(|e| e.causal_parent.is_some() || e.supersedes.is_some()) {
    Some(crate::memory::causal::CausalGraph::build(&entries))
} else {
    None
};
```
It is then passed to `rfe.rank()` for re-ranking with causal signals.

### 2.4 KG-Based Causal Chain Construction

The benchmark also tests KG-based causal chains (`causal_reasoning.py:87-112`):
```python
hub = self.client.add_node("causal-hub", node_type="Event", agent_id="causal-test")
for i in range(n):
    resp = self.client.add_node(f"cause-event-{i}", node_type="Event", agent_id="causal-test")
    self.client.add_edge(hub_id, node_id, edge_type="CausedBy", agent_id="causal-test")
    if i > 0:
        self.client.add_edge(nodes[i-1], node_id, edge_type="CausedBy", agent_id="causal-test")
```

Path traversal is tested via `find_paths()` (`causal_reasoning.py:114-145`), which calls the KG's path-finding algorithm.

### 2.5 Causal-Aware Search Ranking

In `recall_routed_with_k()` (`memory.rs:633-857`), the RFE (Retrieval Fusion Engine) uses the CausalGraph:

1. Builds CausalGraph from entries that have causal fields (`memory.rs:778-783`).
2. Passes to `rfe.rank()` which can boost entries that are causally related to the query context (`memory.rs:793`).

In `search_with_filter()` (`src/fs/semantic_fs/mod.rs:704`), there is **no causal-aware ranking**. The CAS search pipeline uses:
- Vector similarity (HNSW)
- BM25 keyword matching
- RRF (Reciprocal Rank Fusion) to combine
- Optional reranker stage
- PPR (Personalized PageRank) for multi-hop queries

The CausalGraph is only used in the **memory recall pipeline** (`recall_routed`), not in the **CAS search pipeline** (`semantic_search`).

---

## Part 3: Reranker Integration

### 3.1 Reranker Implementation

The reranker is implemented in `src/fs/reranker/mod.rs`:

**Trait** (`reranker/mod.rs:53-61`):
```rust
pub trait RerankerProvider: Send + Sync {
    fn rerank(&self, query: &str, documents: &[(String, String)]) -> Result<Vec<RerankResult>, RerankError>;
    fn model_name(&self) -> &str;
}
```

**LlamaCppReranker** (`reranker/mod.rs:64-205`): Calls `POST {base_url}/rerank` with:
```json
{
    "model": "bge-reranker-v2-m3",
    "query": "search query",
    "documents": ["doc1 text", "doc2 text", ...],
    "top_n": 10
}
```

Handles both async (when tokio runtime exists) and sync contexts via `block_in_place` pattern (`reranker/mod.rs:190-199`).

### 3.2 Environment Variable Configuration

`create_reranker_provider()` (`reranker/mod.rs:215-238`):
- `PLICO_RERANKER_API_BASE` — reranker server URL (e.g., `http://127.0.0.1:18922/v1`). **If not set, reranker is disabled.**
- `PLICO_RERANKER_MODEL` — model name (default: `bge-reranker-v2-m3`)
- `PLICO_RERANKER_TOP_N` — max documents to return (default: `10`)

### 3.3 Where the Reranker Is Initialized

In `AIKernel::new()` (`src/kernel/mod.rs:132`):
```rust
let reranker = crate::fs::reranker::create_reranker_provider();
```

The `Option<Arc<dyn RerankerProvider>>` is passed to:
1. `SemanticFS::with_reranker()` (`kernel/mod.rs:134-142`) — for CAS search pipeline
2. Stored in `AIKernel.reranker` (`kernel/mod.rs:205`) — for memory recall pipeline

### 3.4 Reranker in the CAS Search Pipeline

In `SemanticFS::search_with_filter()` (`src/fs/semantic_fs/mod.rs:920-967`):

```rust
// Reranker stage: if available, apply cross-encoder reranking on top-N RRF candidates
if let Some(ref reranker) = self.reranker {
    let rerank_candidates: usize = (limit * 3).min(sorted.len());
    let candidates: Vec<(String, String)> = sorted[..rerank_candidates]
        .iter()
        .filter_map(|(cid, _)| {
            self.cas.get(cid).ok().map(|obj| {
                let text = String::from_utf8_lossy(&obj.data[..512]).to_string();
                (cid.clone(), text)
            })
        })
        .collect();

    match reranker.rerank(query, &candidates) {
        Ok(reranked) => {
            // Replace RRF results with reranked results
            return reranked_results;
        }
        Err(e) => {
            tracing::warn!("Reranker failed, degrading to RRF: {e}");
        }
    }
}
```

**Pipeline position**: After RRF fusion, before final result assembly. Takes top `limit * 3` RRF candidates, reranks them, returns top `limit`.

**Fallback**: If reranker fails, degrades gracefully to RRF results.

### 3.5 Reranker in the Memory Recall Pipeline

In `recall_routed_with_k()` (`src/kernel/ops/memory.rs:795-827`):

```rust
if config.use_reranker {
    if let Some(ref reranker) = self.reranker {
        let docs: Vec<(String, String)> = fused.iter().map(|r| {
            let text = match &r.entry.content {
                MemoryContent::Text(t) => t.clone(),
                _ => format!("{:?}", r.entry.content),
            };
            (r.entry.id.clone(), text)
        }).collect();
        match reranker.rerank(query, &docs) {
            Ok(reranked) => {
                // Reorder by reranker scores
            }
            Err(e) => {
                tracing::warn!("reranker failed, using RFE order: {e}");
            }
        }
    }
}
```

**Conditional**: Only used when `config.use_reranker` is true (determined by intent classification). For multi-session/temporal intents, MMR diversity selection is used instead (`memory.rs:829-857`).

### 3.6 Is the Reranker Actually Being Invoked?

**Yes, if `PLICO_RERANKER_API_BASE` is set.** The code path is:

1. `create_reranker_provider()` reads env var (`reranker/mod.rs:216`)
2. Returns `Some(Arc<dyn RerankerProvider>)` if URL is non-empty
3. Stored in `SemanticFS.reranker` field (`semantic_fs/mod.rs:105`)
4. Checked with `if let Some(ref reranker) = self.reranker` in both pipelines

**If the env var is not set**, `create_reranker_provider()` returns `None` (`reranker/mod.rs:217-219`), and the reranker stage is skipped entirely.

**Benchmark configuration** (`benchmarks/configs/benchmark.yaml` and `benchmarks/scripts/run_full_benchmark.sh`) should set `PLICO_RERANKER_API_BASE=http://127.0.0.1:18926/v1` to enable the bge-reranker-v2-m3 server on port 18926.

---

## Summary of Key Findings

| Aspect | Finding |
|--------|---------|
| **Session-scoped objects** | No `session_id` on CAS objects or memory entries. Persistence is session-agnostic. |
| **Cross-session memory** | Works because storage is not scoped to sessions. Ephemeral tier is cleared at EndSession; working/long-term persist. |
| **Causal parent links** | Created automatically by ingest pipeline in `remember_long_term_scoped()`, linking extracted facts to their source entry. |
| **Causal retrieval in benchmarks** | Does NOT use CausalGraph. Relies on semantic similarity between cause/effect text. |
| **CausalGraph in recall_routed** | Built on-demand from entries with causal fields, passed to RFE for re-ranking. |
| **Reranker in CAS search** | Active after RRF fusion, takes top 3x candidates, cross-encoder reranks. |
| **Reranker in memory recall** | Conditional on intent classification. Used for precision intents; MMR for multi-session. |
| **Reranker activation** | Requires `PLICO_RERANKER_API_BASE` env var. Disabled by default. |
