# Module: fs/embedding

Text → vector embedding backends — provides the `EmbeddingProvider` trait and multiple backend implementations for generating dense embeddings from text.

Status: active | Fan-in: 2 | Fan-out: 0

## Public API

| Export | File | Description |
|--------|------|-------------|
| `EmbeddingProvider` | `types.rs` | Trait: `embed()` / `embed_batch()` / `dimension()` / `model_name()` |
| `Embedding` | `types.rs` | Type alias: `Vec<f32>` |
| `EmbedError` | `types.rs` | Typed error enum (Http, Ollama, Onnx, Subprocess, etc.) |
| `EmbeddingMeta` | `types.rs` | Chunk metadata (cid, chunk_id, text, tags, offsets) |
| `OllamaBackend` | `ollama.rs` | Ollama HTTP API backend |
| `LocalEmbeddingBackend` | `local.rs` | Python ONNX subprocess backend |
| `OrtEmbeddingBackend` | `ort_backend.rs` | ONNX Runtime in-process backend (feature-gated: `ort-backend`) |
| `StubEmbeddingProvider` | `stub.rs` | Returns errors, triggers tag-based fallback in search |
| `EmbeddingCircuitBreaker` | `circuit_breaker.rs` | 3-state circuit breaker wrapping any provider |

## Dependencies (Fan-out: 0)

External crates only: `reqwest`, `serde`, `serde_json`, `tokio` (for subprocess).

## Dependents (Fan-in: 2)

- `src/fs/semantic_fs/mod.rs` → `EmbeddingProvider` (embeds content on create)
- `src/kernel/mod.rs` → all backends + `EmbeddingCircuitBreaker` (selects backend from env)

## Interface Contract

- `EmbeddingProvider::embed()`: synchronous, returns `Result<Embedding, EmbedError>`
- `EmbeddingCircuitBreaker`: wraps provider; Closed→Open after N failures, HalfOpen probe after cooldown
- `OllamaBackend::new()`: probes server connectivity on construction; returns `EmbedError` if unreachable
- `LocalEmbeddingBackend`: spawns Python child process, communicates via JSON-RPC over stdio
- `OrtEmbeddingBackend`: feature-gated (`ort-backend`); requires `PLICO_MODEL_DIR` with `model.onnx` + `tokenizer.json`
- Thread safety: all providers are `Send + Sync`

## Modification Risk

- Change `EmbeddingProvider` trait → BREAKING, update all 4 backends + circuit breaker + kernel
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
| `mod.rs` | ~37 | Re-exports + 1 test |
| `types.rs` | ~69 | `EmbeddingProvider` trait, `Embedding`, `EmbedError`, `EmbeddingMeta` |
| `ollama.rs` | ~278 | `OllamaBackend` (HTTP API to Ollama) |
| `local.rs` | ~230 | `LocalEmbeddingBackend` (Python ONNX subprocess) |
| `ort_backend.rs` | ~249 | `OrtEmbeddingBackend` (ONNX Runtime, feature-gated) |
| `stub.rs` | ~36 | `StubEmbeddingProvider` (testing/fallback) |
| `circuit_breaker.rs` | ~248 | `EmbeddingCircuitBreaker` (3-state: Closed/Open/HalfOpen) |
| `json_rpc.rs` | ~30 | JSON-RPC request/response types for local backend |

## Tests

- Unit: `mod.rs` (1 test), `circuit_breaker.rs` (2 tests)
- Integration: `tests/embedding_test.rs`
- Untested: `ollama.rs`, `local.rs`, `ort_backend.rs` (require real services; mock recommended)
