# Plico — AI-Native Operating System

An operating system designed entirely from AI perspective. No human CLI/GUI. All data operations via semantic APIs for upper-layer AI agents. The system is model-agnostic and does not depend on any specific AI or agent.

## Architecture Overview

Four-layer architecture: **Application Layer** (external AI agents) → **AI-Friendly Interface Layer** (semantic API/CLI) → **AI Kernel Layer** (agent scheduler, layered memory, model runtime, permission guardrails) → **AI-Native File System** (CAS, vector index, knowledge graph, layered context).

Core philosophy: management unit = agents/intents (not processes/files); storage addressing = content hashes + semantic tags (not filesystem paths); indexing = vectors + knowledge graphs (not filenames).

## Directory Map

```
src/
├── cas/             # Content-Addressed Storage — SHA-256 object identity, auto-dedup
│   ├── object.rs    # AIObject, AIObjectMeta, ContentType
│   ├── storage.rs   # CASStorage engine (sharded, atomic writes)
│   └── mod.rs       # Re-exports
├── memory/          # Layered memory — Ephemeral / Working / LongTerm / Procedural
│   ├── layered.rs   # LayeredMemory, MemoryTier, MemoryEntry, MemoryContent, cognitive methods
│   ├── persist.rs   # CASPersister, MemoryPersister trait, MemoryLoader
│   ├── relevance.rs # RelevanceScore, scoring, budget selection, TTL, promotion thresholds
│   ├── context_snapshot.rs # ContextSnapshot — suspend/resume cognitive continuity
│   └── mod.rs       # MemoryQuery, MemoryResult (public types)
├── intent/          # Intent router — NL → ApiRequest (heuristic + optional LLM chain)
│   ├── mod.rs       # IntentRouter, ChainRouter, ResolvedIntent, IntentError
│   ├── heuristic.rs # HeuristicRouter — keyword/pattern + temporal bounds
│   ├── llm.rs       # LlmRouter — Ollama-backed resolution
│   └── INDEX.md
├── scheduler/       # Agent lifecycle — registration, priority queue, intent dispatch, messaging
│   ├── agent.rs     # Agent, AgentId, AgentState, Intent, IntentPriority, AgentResources
│   ├── queue.rs     # SchedulerQueue (binary heap, priority + timestamp ordering)
│   ├── dispatch.rs  # AgentExecutor, KernelExecutor, LocalExecutor, TokioDispatchLoop, DispatchHandle
│   ├── messaging.rs # MessageBus — bounded mailboxes, send/read/ack
│   └── mod.rs       # AgentScheduler
├── fs/              # Semantic filesystem — tag-based CRUD, vector search, KG
│   ├── semantic_fs.rs  # ~1540 lines; SemanticFS + event container + CRUD + search
│   ├── embedding.rs    # EmbeddingProvider trait, Ollama/Local/Stub backends
│   ├── search.rs       # SemanticSearch trait, InMemoryBackend, BM25, SearchFilter
│   ├── graph.rs        # ~1470 lines; KnowledgeGraph trait, PetgraphBackend, typed KG
│   ├── context_loader.rs # L0/L1/L2 layered context loading
│   ├── summarizer.rs   # Summarizer trait, OllamaSummarizer
│   └── mod.rs          # Re-exports
├── kernel/          # AI Kernel — orchestrates all subsystems
│   ├── mod.rs       # AIKernel — orchestrator, API dispatch, lifecycle
│   ├── builtin_tools.rs # Built-in ToolRegistry + execute_tool (quotas, allowlist)
│   ├── persistence.rs   # Restore/persist agents, intents, memories, search index
│   └── INDEX.md
├── api/             # API layer — permission guardrails + semantic JSON protocol
│   ├── semantic.rs  # ApiRequest, ApiResponse, JSON-over-TCP protocol types
│   ├── permission.rs # PermissionGuard, PermissionContext, PermissionAction (incl. ReadAny, ownership isolation)
│   └── mod.rs       # Re-exports
├── tool/            # Tool Abstraction — "Everything is a Tool" capability system
│   ├── mod.rs       # Tool trait, ToolResult, ToolSchema, ToolDescriptor
│   └── registry.rs  # ToolRegistry — agent-discoverable capability catalog
├── temporal/        # Temporal reasoning — natural language time → time ranges
│   ├── resolver.rs  # TemporalResolver trait, OllamaTemporalResolver
│   ├── rules.rs     # HeuristicTemporalResolver, pre-defined temporal rules
│   └── mod.rs       # Re-exports
├── bin/
│   ├── plicod.rs    # TCP daemon (port 7878, JSON protocol)
│   └── aicli.rs     # CLI tool (local kernel or TCP mode)
├── lib.rs           # Crate root, public re-exports
└── main.rs          # Stub — directs to plicod/aicli

tests/               # Integration tests
├── fs_test.rs       # SemanticFS CRUD + event tests
├── kernel_test.rs   # AIKernel integration
├── cli_test.rs      # CLI binary tests
├── memory_test.rs   # Layered memory tests
├── memory_persist_test.rs # Memory persistence
├── semantic_search_test.rs # Vector + BM25 hybrid search
├── embedding_test.rs  # Embedding provider tests
└── permission_test.rs # Permission guard tests

Cargo.toml           # Rust crate definition
CLAUDE.md            # AI guidance (soul document reference)
```

## Quick Navigation

| Area | Entry Point | Purpose |
|------|-------------|---------|
| CAS storage | `src/cas/INDEX.md` | AIObject, CASStorage, content addressing |
| Memory system | `src/memory/INDEX.md` | LayeredMemory, 4-tier hierarchy, persistence |
| Intent router | `src/intent/INDEX.md` | NL → `ApiRequest`, ChainRouter, heuristic + LLM |
| Agent scheduling | `src/scheduler/INDEX.md` | AgentScheduler, Intent, messaging, resources, dispatch |
| Semantic FS | `src/fs/INDEX.md` | SemanticFS, vector search, KG, context loading |
| AI Kernel | `src/kernel/INDEX.md` | AIKernel — central orchestrator |
| API layer | `src/api/INDEX.md` | Permission guard, semantic JSON protocol |
| Tool system | `src/tool/INDEX.md` | ToolRegistry, ToolDescriptor, execute_tool — "Everything is a Tool" |
| Memory relevance | `src/memory/relevance.rs` | RelevanceScore, TTL eviction, tier promotion |
| Context snapshot | `src/memory/context_snapshot.rs` | Suspend/resume cognitive continuity |
| Temporal | `src/temporal/INDEX.md` | Time expression → Unix ms range resolution |
| TCP daemon | `src/bin/plicod.rs` | JSON API server on port 7878 |
| CLI tool | `src/bin/aicli.rs` | `put`, `get`, `search`, `agent` (incl. `set-resources`), `tool`, `intent` (NL resolve vs `--description`), `send` / `messages` / `ack`, `status`, `suspend`, `resume`, `terminate`, `node`, `edge`, etc. |

## Build & Test

| Command | Purpose |
|---------|---------|
| `cargo build` | Build all targets |
| `cargo build --bin aicli` | Build CLI binary only |
| `cargo build --bin plicod` | Build daemon binary only |
| `cargo test --lib` | Run unit tests (co-located in source) |
| `cargo test` | Run all tests (unit + integration) |
| `cargo test [test_name]` | Run a single test |
| `cargo clippy` | Lint check (must be zero warnings) |
| `cargo build --release` | Release build |
| `cargo run --bin aicli -- --root /tmp/plico put --content "test" --tags "test"` | Quick CLI test |
| `cargo run --bin plicod -- --port 7878 --root /tmp/plico` | Run daemon |

## Conventions

- **Files**: `snake_case.rs`, one concept per file, target < 300 lines
- **Naming**: `snake_case` functions, `PascalCase` types, `SCREAMING_SNAKE` constants
- **Modules**: `pub mod` in `mod.rs`, submodules in `subdir/` with `mod.rs`
- **Public API**: `pub fn`, private by default
- **L2 file headers**: doc comment (`//!`) with module purpose + `# Panics`, `# Errors`, `# Safety`
- **Tests**: `#[cfg(test)] mod tests` co-located in same file

## Architectural Constraints

- Dependency direction: **api/bin → kernel → tool/fs → cas/memory/scheduler** (never reverse)
- `kernel/` is the only module that imports all other modules — all subsystem calls go through `AIKernel`
- `AIKernel` fields are `pub(crate)` — visible only inside the `plico` crate; integration tests in `tests/` use the public API
- Binaries (`bin/`) import only `kernel/` and `api/`, never subsystem modules directly
- CAS is the only module that touches the filesystem directly
- No `unsafe` blocks in library code without a `# Safety` doc comment
- All known soul violations from prior iterations have been resolved:
  - Behavioral pipeline (BehavioralObservation, UserFact, PatternExtractor, ActionSuggestion) removed from `semantic_fs.rs`
  - Dashboard hardcoded dev data removed from `kernel/mod.rs`; now reports runtime metrics only
  - Project-management KGNodeType/KGEdgeType (Iteration, Plan, DesignDoc) removed from `graph.rs`
  - ProjectStatus/IterationDto/PlanDto/DesignDocDto removed from `api/semantic.rs`
  - All test scenarios converted from human-centric ("商务晚餐") to AI-native ("agent-sync-task")

## Cross-Cutting Patterns

### Error Handling
- All errors typed: `CASError`, `MemoryError`, `SchedulerError`, `FSError` (all `thiserror`)
- `Result<T>` aliases per module; crate root exposes `PlicoError`
- I/O errors converted to `std::io::Error` at API boundary (daemon/CLI)
- Never panicking in library code except for critical invariants (`expect()` with message)

### Logging
- `tracing` crate for structured logging with `tracing::info!/warn!/debug!`
- `tracing_subscriber::fmt::init()` called in both `plicod.rs` and `aicli.rs` — reads `RUST_LOG` env var
- Library code uses `tracing` only; subscriber setup is binary responsibility

### Concurrency
- `RwLock` for in-memory maps (memory tiers, tag index, recycle bin)
- `tokio` for async TCP server; blocking `std::net` used in some places for simplicity
- All `Arc<...>` wrapping shared kernel state in daemon
- `OllamaBackend` and `OllamaSummarizer`: safe within `tokio::spawn` (use `block_in_place`)

### Serialization
- JSON for: CAS object persistence, TCP protocol, `serde` on all public types
- `serde_json` for serialization; `serde` derive for `Serialize`/`Deserialize`

### Clippy Policy
- `cargo clippy` runs clean (zero warnings) — all lint violations either fixed or suppressed with `#[allow(...)]` and an explanation

## Dogfooding Tag Convention

Plico manages its own project data (ADRs, progress, experiences) through its standard API.
Tags use the `plico:` namespace with colon-separated hierarchical dimensions:

| Dimension | Values | Purpose |
|-----------|--------|---------|
| `plico:type:<T>` | adr, progress, experience, test-result, bug, code-change, doc | Artifact type |
| `plico:module:<M>` | cas, fs, kernel, api, scheduler, memory, graph, temporal, cli, daemon | Module scope |
| `plico:status:<S>` | active, superseded, resolved, wip | Lifecycle state |
| `plico:milestone:<V>` | v0.1, v0.2, v0.3, ... | Target milestone |
| `plico:severity:<L>` | critical, high, medium, low | Bug severity only |

KG nodes use generic types (Entity, Fact) with `properties` JSON to encode domain meaning.
No project-specific KGNodeType or KGEdgeType — all semantics via tags + properties.

## Key Environment Variables

| Variable | Purpose | Required |
|----------|---------|----------|
| `EMBEDDING_BACKEND` | `"local"` (default) / `"ollama"` / `"stub"` | No |
| `EMBEDDING_MODEL_ID` | HuggingFace model ID (default: `BAAI/bge-small-en-v1.5`) | No |
| `EMBEDDING_PYTHON` | Python interpreter path (default: `python3`) | No |
| `OLLAMA_URL` | Ollama daemon URL (default: `http://localhost:11434`) | No |
| `OLLAMA_EMBEDDING_MODEL` | Ollama embedding model (default: `all-minilm-l6-v2`) | No |
| `OLLAMA_SUMMARIZER_MODEL` | Ollama chat model for summaries (default: `llama3.2`) | No |
| `RUST_LOG` | Tracing log level filter (default: `info`) | No |

## AI Agent Instructions

This project uses a two-tier documentation model:

- **Tier A (this file + all `INDEX.md` + file doc headers)**: Maintain in real-time, atomic with every code change. A code change is NOT complete until Tier A indexes reflect it.
- **Tier B (`README.md`, `system.md`, `docs/`)**: Do NOT read or update during active development. Updated only at iteration-end sync.

When modifying code:
1. Update the parent `INDEX.md` if files are added/removed/renamed
2. Update this `AGENTS.md` if modules or navigation entries change
3. Update `INDEX.md` Dependents/Dependencies if call relationships change
4. Do NOT touch Tier B files during active development

## Agent Workflow (Read Before Edit)

**Checklist-at-END — complete before any code modification:**

- [ ] Locate target module via Quick Navigation table above
- [ ] Open module `INDEX.md` and check Dependents before modifying public API
- [ ] Confirm Modification Risk for signature/error-type changes
- [ ] Run `cargo test --lib` — all tests must pass before finishing
- [ ] If binary changed: `cargo build --bin [name]` succeeds
- [ ] Update AGENTS.md if new modules or build commands were added

## Index Exclusions

```
target/          # Cargo build output
Cargo.lock       # Lock file
.claude/         # Claude Code settings
*.rlib           # Compiled Rust library files
docs/design/     # Tier B design documents
```
