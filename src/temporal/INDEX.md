# Module: temporal

Temporal reasoning — converts natural-language time expressions ("几天前", "上周", "last month") into concrete Unix-millisecond ranges for time-bounded search.

Status: stable | Fan-in: 2 | Fan-out: 0

## Dependents (Fan-in: 2)

- `src/fs/semantic_fs.rs` → TemporalResolver (via `list_events_by_time()`)
- `src/kernel/mod.rs` → TemporalResolver, RULE_BASED_RESOLVER (kernel imports for event time queries)

## Modification Risk

- Add `TemporalRule` pattern → compatible, extends recognition
- Change `TemporalResolver` trait → BREAKING, update HeuristicTemporalResolver + StubTemporalResolver
- Change `Granularity` variants → BREAKING, update match arms in resolver
- Change confidence thresholds → behavioral change, affects search filtering

## Task Routing

- Add time expression rule → modify `src/temporal/rules.rs` RULES array + evaluate()
- Change confidence strategy → modify `src/temporal/rules.rs`
- Add new granularity → modify `src/temporal/rules.rs` Granularity enum

## Public API

| Export | File | Description |
|--------|------|-------------|
| `TemporalResolver` | `resolver.rs` | Trait: expression → TemporalRange |
| `TemporalRange` | `resolver.rs` | Resolved range (since/until Unix ms, confidence, granularity) |
| `StubTemporalResolver` | `resolver.rs` | Always returns None (forces pure semantic search) |
| `HeuristicTemporalResolver` | `rules.rs` | Rule-based synchronous resolver |
| `RULE_BASED_RESOLVER` | `rules.rs` | Static default heuristic resolver instance |
| `Granularity` | `rules.rs` | Time granularity (ExactDay/Week/Month/Quarter/Year/Fuzzy) |
| `resolve_heuristic` | `rules.rs` | Direct function for rule-based resolution |
| `TemporalRule` | `rules.rs` | One keyword pattern + resolution strategy |

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `resolver.rs` | ~72 | TemporalResolver trait, TemporalRange, StubTemporalResolver |
| `rules.rs` | ~1065 | HeuristicTemporalResolver, pre-defined rules (中文 + English) |
| `mod.rs` | ~34 | Re-exports |

## Dependencies (Fan-out: 0)

None — temporal is standalone; the only external crate used is `chrono` (rule date math in `rules.rs`).

## Interface Contract

- `TemporalResolver::resolve()`: returns `Option<TemporalRange>`; None = expression not understood
- `HeuristicTemporalResolver`: pure rule-based, no network calls, synchronous
- Confidence levels: ≥0.8 strict range; <0.5 fallback to semantic search; medium-confidence ranges are used exactly as resolved (no ±7-day expansion is implemented)
- Thread safety: `HeuristicTemporalResolver` is stateless

## Tests

- Unit: `src/temporal/rules.rs` mod tests
- Critical: `test_heuristic_today`, `test_heuristic_last_week`, `test_unknown_expression`
