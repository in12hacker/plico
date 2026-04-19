# Module: intent

Natural language intent router — bridges the NL-first interface goal. Translates free-form text into structured `ApiRequest` actions.

**Status**: active (在接口层，不在内核) | **Fan-in**: 1 | **Fan-out**: 2

## Public API

| Symbol | Kind | Description |
|--------|------|-------------|
| `IntentRouter` | trait | NL → Vec<ResolvedIntent> resolution |
| `ResolvedIntent` | struct | Confidence + action + explanation |
| `IntentError` | enum | Ambiguous / Unresolvable / LlmUnavailable |
| `ChainRouter` | struct | Default: heuristic → LLM fallback |
| `HeuristicRouter` | struct | Keyword/pattern matching (always available) |
| `LlmRouter` | struct | Ollama-backed NL understanding (optional) |

## Dependencies (Fan-out: 2)

- `src/api/semantic.rs` — `ApiRequest` types
- `src/temporal/` — `resolve_heuristic` for time phrase resolution

## Dependents (Fan-in: 1)

- `src/bin/aicli.rs` — `intent` CLI command（接口层使用，内核无感知）

## 架构说明（v3.0-M1 Soul Alignment Fix）

**历史**：IntentRouter 最初在内核中（v1-v2），但这违背了"OS 不应理解自然语言"的原则。

**修复**：IntentRouter 已迁至接口层（`src/intent/`），内核只接受结构化 `ApiRequest`，不再理解自然语言。

```
v1-v2（错误）:  用户 NL → 内核 IntentRouter → ApiRequest
v3+（正确）:    用户 NL → aicli IntentRouter → ApiRequest → 内核
```

## Interface Contract

- `IntentRouter::resolve()` always returns at least one result (low-confidence fallback search) or an error
- Confidence: ≥0.7 = reliable heuristic match, <0.5 = fallback guess
- LlmRouter is optional; system works without Ollama

## Modification Risk

| Change | Risk |
|--------|------|
| Add new pattern to HeuristicRouter | Low — additive |
| Change ResolvedIntent fields | Medium — affects API response |
| Change IntentRouter trait | Medium — affects aicli + intent impls |

## Files

| File | Purpose | Lines |
|------|---------|-------|
| `mod.rs` | IntentRouter trait, ChainRouter, types | ~130 |
| `heuristic.rs` | HeuristicRouter keyword matching | ~330 |
| `llm.rs` | LlmRouter Ollama integration | ~130 | |
