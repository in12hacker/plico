# Module: llm

LLM provider abstraction — model-agnostic chat interface for intent resolution, temporal reasoning, and summarization.

Status: stable | Fan-in: 2 | Fan-out: 0

## Dependents (Fan-in: 2)

- `src/intent/llm.rs` → LlmProvider, ChatMessage, ChatOptions (intent resolution via LLM)
- `src/kernel/mod.rs` → LlmProvider (kernel holds provider for model hot-swap)

## Public API

| Export | File | Description |
|--------|------|-------------|
| `LlmProvider` | `mod.rs` | Trait: chat completion interface |
| `ChatMessage` | `mod.rs` | Role + content message struct |
| `ChatOptions` | `mod.rs` | Temperature, max_tokens, model name |
| `LlmError` | `mod.rs` | Typed LLM errors |
| `OllamaProvider` | `ollama.rs` | Local Ollama daemon backend |
| `OpenAICompatibleProvider` | `openai.rs` | OpenAI-compatible API endpoint backend |
| `StubProvider` | `stub.rs` | Fixed responses for testing |

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | — | LlmProvider trait, ChatMessage, ChatOptions, LlmError, re-exports |
| `ollama.rs` | — | OllamaProvider (HTTP API to local Ollama) |
| `openai.rs` | — | OpenAICompatibleProvider (any OpenAI-compatible endpoint) |
| `stub.rs` | — | StubProvider (deterministic test responses) |

## Dependencies (Fan-out: 0)

None — depends only on external crates (reqwest, serde, serde_json).

## Modification Risk

- Change `LlmProvider` trait → BREAKING, update all 3 backends + intent + kernel
- Change `ChatMessage` fields → BREAKING, update all callers
- Add new provider → compatible, add new file + registration in kernel

## Interface Contract

- `LlmProvider::chat()`: async, returns `Result<String, LlmError>`
- `OllamaProvider`: connects to `OLLAMA_URL` (default `http://localhost:11434`)
- `StubProvider`: returns pre-configured response, no network calls
- Thread safety: all providers are `Send + Sync`

## Tests

- Unit: co-located in each provider file
- Integration: tested indirectly via intent tests and kernel tests
