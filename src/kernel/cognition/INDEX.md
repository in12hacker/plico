# Module: cognition

Soul v3.0 — Cognitive Symbiotic Engine.

Plico is not a neutral infrastructure; it is the Agent's cognitive symbiont.
This module actively optimizes the Agent's cognitive environment:
- Compresses redundant / stale / temporary context
- Prefetches relevant context via intent semantic network
- Extracts, validates, and evolves skills from operation history
- Tracks cognitive trajectories and failure patterns

All optimization behaviors are observable, overridable, and debuggable by the Agent.

Status: active | Fan-in: 1 | Fan-out: 4

## Dependents (Fan-in: 1)

- `src/kernel/mod.rs` → initializes CognitiveLoop in `AIKernel::new()`
- `src/kernel/ops/prefetch.rs` → calls `CognitiveLoop::on_intent_declared` during declare_intent
- `src/kernel/ops/session.rs` → registers/ends sessions with CognitiveLoop
- `src/kernel/ops/intent_executor.rs` → reports step completions to CognitiveLoop

## Modification Risk

- Change `CognitiveLoop` public API → BREAKING, update all callers in `ops/`
- Change `CognitiveError` variant → check all `match` sites in kernel
- Add new `OptimizationAction` → update report consumers (event bus, metrics)
- Modify `Skill` enum → affects serialization; check `skill_registry.rs` persistence

## Task Routing

- Context quality analysis / compression → `context_quality.rs`
- Intent semantic relations / predictions → `intent_network.rs`
- Skill extraction / validation / registration → `skill_forge.rs` + `skill_validator.rs` + `skill_registry.rs`
- Agent trajectory tracking → `trajectory_tracker.rs`
- Experience mining from history → `experience_miner.rs`
- Skill composition / DSL execution / WASM runtime → `skill_composer.rs` + `dsl_interpreter.rs` + `wasm_runtime.rs`

## Public API

| Export | File | Description |
|--------|------|-------------|
| `CognitiveLoop` | `cognitive_loop.rs` | Core engine — `on_intent_declared`, `on_operation_completed`, `register_session`, `end_session` |
| `ContextQualityEngine` | `context_quality.rs` | Analyze and compress context quality |
| `IntentSemanticNetwork` | `intent_network.rs` | Learn and query intent semantic relations |
| `SkillForge` | `skill_forge.rs` | Extract and recommend skills from history |
| `TrajectoryTracker` | `trajectory_tracker.rs` | Track agent operations and failures |
| `Skill` | `mod.rs` | Unified skill type (Knowledge / Config / Code) |
| `CognitiveError` | `mod.rs` | Unified error type for cognition module |

## Files

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ~332 | Re-exports + shared types (CognitiveError, Skill, KnowledgeSkill, ConfigSkill, CodeSkill, etc.) |
| `cognitive_loop.rs` | ~382 | CognitiveLoop — active optimization orchestrator |
| `context_quality.rs` | ~200 | ContextQualityEngine — token analysis, redundancy detection, compression |
| `intent_network.rs` | ~500 | IntentSemanticNetwork — embedding-based causal/temporal/associative relations |
| `skill_forge.rs` | ~200 | SkillForge — skill candidate extraction and recommendation |
| `skill_registry.rs` | ~250 | SkillRegistry — versioned skill storage and retrieval |
| `skill_validator.rs` | ~200 | SkillValidator — backtest skills against history |
| `skill_composer.rs` | ~100 | SkillComposer — cross-domain skill composition |
| `trajectory_tracker.rs` | ~200 | TrajectoryTracker — operation/failure pattern tracking |
| `experience_miner.rs` | ~150 | ExperienceMiner — pattern extraction from operation history |
| `dsl_interpreter.rs` | ~300 | DslInterpreter — execute Config skills via DSL |
| `wasm_runtime.rs` | ~80 | WasmRuntime — execute Code skills in WASM sandbox |

## Dependencies (Fan-out: 4)

- `src/fs/embedding/` — EmbeddingProvider for semantic analysis
- `src/fs/search/` — SemanticSearch for context quality checks
- `src/fs/graph/` — KnowledgeGraph for causal/stale detection
- `src/memory/` — LayeredMemory for context manipulation

## Interface Contract

- `CognitiveLoop::new()`: requires EmbeddingProvider, SemanticSearch, LayeredMemory
- `on_intent_declared()`: async; returns `CognitiveOptimizationReport` with optimization actions
- `on_operation_completed()`: async; lightweight; triggers background skill extraction on success
- `register_session()`: async; initializes session cognitive state
- `end_session()`: async; extracts final skills from session trajectory
- All methods are `tokio::sync::RwLock` safe; designed for concurrent multi-agent access

## Tests

- Unit tests co-located in each file under `#[cfg(test)] mod tests`
- Integration: `tests/ai_experience_test.rs`, `tests/kernel_test.rs` (cognitive path coverage)
