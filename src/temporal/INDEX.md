# Module: temporal

Temporal reasoning — converts natural-language time expressions ("几天前", "上周", "last month") into concrete Unix-millisecond ranges for time-bounded search.

Status: stable | Fan-in: 2 | Fan-out: 0

## Dependents (Fan-in: 2)

- `src/fs/semantic_fs.rs` → TemporalResolver (via `list_events_by_time()`)
- `src/kernel/mod.rs` → TemporalResolver, RULE_BASED_RESOLVER (kernel imports for event time queries)

## Modification Risk

- Add `TemporalRule` pattern → compatible, extends recognition
- Change `TemporalResolver` trait → BREAKING, update OllamaTemporalResolver + HeuristicTemporalResolver + StubTemporalResolver
- Change `Granularity` variants → BREAKING, update match arms in resolver
- Change confidence thresholds → behavioral change, affects search window expansion

## Task Routing

- Add time expression rule → modify `src/temporal/rules.rs` RULES array + evaluate()
- Add LLM resolver → modify `src/temporal/resolver.rs` OllamaTemporalResolver
- Change confidence strategy → modify `src/temporal/resolver.rs`
- Add new granularity → modify `src/temporal/rules.rs` Granularity enum

## Public API

| Export | File | Description |
|--------|------|-------------|
| `TemporalResolver` | `resolver.rs` | Trait: expression → TemporalRange |
| `TemporalRange` | `resolver.rs` | Resolved range (since/until Unix ms, confidence, granularity) |
| `OllamaTemporalResolver` | `resolver.rs` | LLM-powered resolver with LRU cache |
| `StubTemporalResolver` | `resolver.rs` | Always returns None (forces pure semantic search) |
| `HeuristicTemporalResolver` | `rules.rs` | Rule-based synchronous resolver |
| `RULE_BASED_RESOLVER` | `rules.rs` | Static default heuristic resolver instance |
| `Granularity` | `rules.rs` | Time granularity (ExactDay/Week/Month/Quarter/Year/Fuzzy) |
| `resolve_heuristic` | `rules.rs` | Direct function for rule-based resolution |

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `resolver.rs` | ~200 | TemporalResolver trait, OllamaTemporalResolver, StubTemporalResolver |
| `rules.rs` | ~339 | HeuristicTemporalResolver, pre-defined rules (中文 + English) |
| `mod.rs` | ~35 | Re-exports |

## Dependencies (Fan-out: 0)

None — temporal is standalone, depends only on external crates (chrono, reqwest, lru, serde_json).

## Interface Contract

- `TemporalResolver::resolve()`: returns `Option<TemporalRange>`; None = expression not understood
- `OllamaTemporalResolver`: tries heuristic first (fast), falls back to LLM; results cached in LRU
- `HeuristicTemporalResolver`: pure rule-based, no network calls, synchronous
- Confidence levels: ≥0.8 strict range; 0.5–0.8 expanded ±7 days; <0.5 fallback to semantic search
- Thread safety: `OllamaTemporalResolver` uses `RwLock` for cache; `HeuristicTemporalResolver` is stateless

## Tests

- Unit: `src/temporal/rules.rs` mod tests
- Critical: `test_heuristic_today`, `test_heuristic_last_week`, `test_unknown_expression`
