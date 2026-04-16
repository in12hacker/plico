# Module: intent

Natural language intent router — bridges the NL-first interface goal. Translates free-form text into structured `ApiRequest` actions.

Status: active | Fan-in: 1 | Fan-out: 3

## Public API

| Symbol | Kind | Description |
|--------|------|-------------|
| `IntentRouter` | trait | NL → Vec<ResolvedIntent> resolution |
| `ResolvedIntent` | struct | Confidence + action + explanation |
| `IntentError` | enum | Ambiguous / Unresolvable / LlmUnavailable |
| `ChainRouter` | struct | Default: heuristic → LLM fallback |
| `HeuristicRouter` | struct | Keyword/pattern matching (always available) |
| `LlmRouter` | struct | Ollama-backed NL understanding (optional) |

## Dependencies (Fan-out: 3)

- `src/api/semantic.rs` — `ApiRequest` types
- `src/temporal/` — `resolve_heuristic` for time phrase resolution
- `src/tool/` — `ToolDescriptor` (LLM catalog)

## Dependents (Fan-in: 1)

- `src/kernel/mod.rs` — `AIKernel::intent_resolve()` / `ChainRouter`
- `src/bin/aicli.rs` — `intent` CLI command

## Interface Contract

- `IntentRouter::resolve()` always returns at least one result (low-confidence fallback search) or an error
- Confidence: ≥0.7 = reliable heuristic match, <0.5 = fallback guess
- LlmRouter is optional; system works without Ollama

## Modification Risk

| Change | Risk |
|--------|------|
| Add new pattern to HeuristicRouter | Low — additive |
| Change ResolvedIntent fields | Medium — affects API response |
| Change IntentRouter trait | High — kernel + all impls |

## Files

| File | Purpose | Lines |
|------|---------|-------|
| `mod.rs` | IntentRouter trait, ChainRouter, types | ~130 |
| `heuristic.rs` | HeuristicRouter keyword matching | ~330 |
| `llm.rs` | LlmRouter Ollama integration | ~130 |
