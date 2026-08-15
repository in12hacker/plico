# Module: fs/embedding

Text → vector embedding backends — provides the `EmbeddingProvider` trait and multiple backend implementations for generating dense embeddings from text.

Status: active | Fan-in: 2 | Fan-out: 0

## Public API

| Export | File | Description |
|--------|------|-------------|
| `EmbeddingProvider` | `types.rs` | Operation-specific embedding trait plus mandatory typed document-builder identity |
| `EmbeddingBuilderIdentity` | `types.rs` | Immutable, non-secret `embed_document` compatibility identity |
| `VerifiedDocumentProviderSnapshot` | `types.rs` | Provider and recomputed identity captured as one checked pair |
| `Embedding` | `types.rs` | Type alias: `Vec<f32>` |
| `EmbedError` | `types.rs` | Typed operational embedding failures |
| `EmbeddingMeta` | `types.rs` | Chunk metadata (cid, chunk_id, text, tags, offsets) |
| `OllamaBackend` | `ollama.rs` | Ollama HTTP API backend |
| `OpenAIEmbeddingBackend` | `openai.rs` | Legacy Object-search backend; P3 identity unavailable |
| `LocalEmbeddingBackend` | `local.rs` | Legacy Object-search Python worker; P3 identity unavailable |
| `StubEmbeddingProvider` | `stub.rs` | Explicit test/tag-only provider; never has a production identity |
| `EmbeddingCircuitBreaker` | `circuit_breaker.rs` | 3-state breaker; rejects work while open and never fabricates vectors |

## Dependencies (Fan-out: 0)

External crates only: `reqwest`, `serde`, `serde_json`, `tokio` (for subprocess).

## Dependents (Fan-in: 2)

- `src/fs/semantic_fs/mod.rs` → `EmbeddingProvider` (embeds content on create)
- `src/kernel/mod.rs` → all backends + `EmbeddingCircuitBreaker` (selects backend from env)

## Interface Contract

- `EmbeddingProvider`: keeps Generic / Query / Document operations distinct; `builder_identity()` has no default.
- P3-A publishable identity is currently **Ollama only**: explicit full tag, canonical digest, server version, `/api/embed` with `truncate=false`, and before/after digest guards.
- `OllamaBackend::new()` is side-effect free; the first identity/document operation performs the protocol proof. Missing or drifting evidence returns a typed unavailable error and discards the vector.
- OpenAI-compatible, Local, and Stub remain operational only for legacy Object/tag flows and cannot publish a P3 builder identity. ORT activation was removed because its model contract and dimension were hard-coded rather than proven.
- `LocalEmbeddingBackend` embeds the production `local_worker.py`, serializes each JSON-RPC request/response under one lock, applies a deadline, and validates exact finite nonzero shape.
- `EmbeddingCircuitBreaker`: Closed→Open after N failures, one leased HalfOpen probe after cooldown; Open/HalfOpen returns typed errors, never fallback vectors.
- Cache keys bind domain-separated SHA-256 input, complete builder identity, and operation; identity and vector shape are rechecked before publication.
- The identity contract is only for the Memory embedding projection. Legacy Object HNSW metadata does not bind the compatibility digest, so this module does not claim Object-vector correctness across provider changes; admitting Object search requires a separate projection ADR and atomic rebuild protocol.
- Thread safety: all providers are `Send + Sync`

## Modification Risk

- Change `EmbeddingProvider` trait → BREAKING, update every provider, wrapper, kernel factory, and test provider
- Change `EmbedError` variants → update all error handling in backends
- Add new backend → compatible, add file + re-export in `mod.rs` + kernel selection

## Task Routing

- Add embedding backend → new file in this dir, implement `EmbeddingProvider`, add to `mod.rs` re-exports
- Fix circuit breaker → `circuit_breaker.rs`
- Fix Python subprocess comms → `local.rs` + `json_rpc.rs`
- Fix Ollama probe/embed → `ollama.rs`
- Change embedding types → `types.rs`

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | wrappers + re-exports | Identity-aware cache wrapper |
| `types.rs` | contracts | Provider trait, identity, operation and normalization contracts |
| `ollama.rs` | provider | Only currently publishable P3 document builder |
| `ollama/tests.rs` | protocol tests | Exact tag/digest/version, drift, shape/value and privacy gates |
| `local.rs` | provider | Operational local worker boundary; P3 identity unavailable |
| `local_worker.py` | worker | Compiled-in Python CPU worker used by `local.rs` |
| `openai.rs` | provider | OpenAI-compatible legacy Object backend; P3 identity unavailable |
| `stub.rs` | provider | Explicit non-production identity-unavailable stub |
| `adaptive.rs` | wrapper | Registered prefix and Matryoshka/L2 contracts |
| `circuit_breaker.rs` | wrapper | Closed/Open/HalfOpen failure protection without fallback vectors |
| `json_rpc.rs` | protocol | Local worker JSON-RPC envelopes |

## Tests

- Unit: identity/cache/wrapper matrices plus deterministic Ollama protocol server
- Integration: `tests/embedding_test.rs`
- Local real-model execution remains environment-dependent; its P3 identity is deliberately unavailable.
