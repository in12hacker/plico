# Plico — AI-Native Operating System

An operating system designed entirely from AI perspective. No human CLI/GUI. All data operations via semantic APIs for upper-layer AI agents. The system is model-agnostic and does not depend on any specific AI or agent.

## Architecture Overview

Four-layer architecture: **Application Layer** (external AI agents) → **AI-Friendly Interface Layer** (semantic API/CLI/MCP) → **AI Kernel Layer** (agent scheduler, layered memory, event bus, tool registry, permission guardrails) → **AI-Native File System** (CAS, vector index, knowledge graph, layered context).

Core philosophy: management unit = agents/intents (not processes/files); storage addressing = content hashes + semantic tags (not filesystem paths); indexing = vectors + knowledge graphs (not filenames).

## Directory Map

```
src/
├── cas/                 # Content-Addressed Storage — SHA-256 object identity, auto-dedup
│   ├── object.rs        # AIObject, AIObjectMeta, ContentType
│   ├── storage.rs       # CASStorage engine (sharded, atomic writes)
│   └── mod.rs           # Re-exports
├── memory/              # Layered memory — Ephemeral / Working / LongTerm / Procedural
│   ├── layered/
│   │   ├── mod.rs       # LayeredMemory, MemoryTier, MemoryEntry, MemoryContent, cognitive methods
│   │   └── tests.rs     # Unit tests
│   ├── persist.rs       # CASPersister, MemoryPersister trait, MemoryLoader
│   ├── relevance.rs     # RelevanceScore, scoring, budget selection, TTL, promotion thresholds
│   ├── context_snapshot.rs # ContextSnapshot — suspend/resume cognitive continuity
│   └── mod.rs           # MemoryQuery, MemoryResult (public types)
├── intent/              # Intent router — NL → ApiRequest (heuristic + optional LLM chain)
│   ├── mod.rs           # IntentRouter, ChainRouter, ResolvedIntent, IntentError, RoutingAction
│   ├── heuristic.rs     # HeuristicRouter — keyword/pattern + temporal bounds
│   ├── llm.rs           # LlmRouter — Ollama-backed resolution
│   ├── execution.rs     # execute_sync — app-layer NL→execute→learn loop
│   └── INDEX.md
├── scheduler/           # Agent lifecycle — registration, priority queue, intent dispatch, messaging
│   ├── agent.rs         # Agent, AgentId, AgentState, Intent, IntentPriority, AgentResources, AgentUsage
│   ├── queue.rs         # SchedulerQueue (binary heap, priority + timestamp ordering)
│   ├── dispatch.rs      # AgentExecutor, KernelExecutor, LocalExecutor, TokioDispatchLoop, DispatchHandle
│   ├── messaging.rs     # MessageBus — bounded mailboxes, send/read/ack
│   └── mod.rs           # AgentScheduler, AgentHandle
├── fs/                  # Semantic filesystem — tag-based CRUD, vector search, KG
│   ├── semantic_fs/
│   │   ├── mod.rs       # SemanticFS + CRUD + search + event container
│   │   ├── events.rs    # Event types and operations
│   │   └── tests.rs     # Unit tests
│   ├── embedding/
│   │   ├── mod.rs       # EmbeddingProvider trait + re-exports
│   │   ├── types.rs     # Shared embedding types
│   │   ├── ollama.rs    # OllamaBackend
│   │   ├── local.rs     # LocalEmbeddingBackend (Python ONNX)
│   │   ├── stub.rs      # StubEmbeddingProvider
│   │   └── json_rpc.rs  # JSON-RPC embedding adapter
│   ├── search/
│   │   ├── mod.rs       # SemanticSearch trait, SearchFilter, re-exports
│   │   ├── memory.rs    # InMemoryBackend (brute-force cosine)
│   │   ├── bm25.rs      # BM25 keyword search index
│   │   └── hnsw.rs      # HnswBackend (approximate NN via hnsw_rs)
│   ├── graph/
│   │   ├── mod.rs       # KnowledgeGraph trait, ExploreDirection, re-exports
│   │   ├── types.rs     # KGNode, KGEdge, KGNodeType, KGEdgeType, DiskGraph
│   │   ├── backend.rs   # PetgraphBackend, EdgeRecord — directed graph + disk persistence
│   │   └── tests.rs     # Unit tests
│   ├── context_loader.rs # L0/L1/L2 layered context loading
│   ├── context_budget.rs # Context budget engine — adaptive multi-object assembly
│   ├── summarizer.rs    # Summarizer trait, LlmSummarizer
│   ├── types.rs         # Shared FS types
│   └── mod.rs           # Re-exports
├── kernel/              # AI Kernel — orchestrates all subsystems
│   ├── mod.rs           # AIKernel struct, constructor, handle_api_request dispatch
│   ├── builtin_tools.rs # Built-in ToolRegistry + execute_tool (quotas, allowlist)
│   ├── persistence.rs   # Restore/persist agents, intents, memories, search index, event log
│   ├── event_bus.rs     # EventBus — typed pub/sub, kernel events, persisted log
│   ├── ops/             # Operation groups (keeps mod.rs manageable)
│   │   ├── mod.rs       # Re-exports
│   │   ├── fs.rs        # FS-related kernel operations (search, CRUD, stats, evict)
│   │   ├── agent.rs     # Agent lifecycle (register, ensure_registered, suspend, resume)
│   │   ├── memory.rs    # Memory tier operations (recall, store, promote, compress)
│   │   ├── events.rs    # Event bus + event log operations
│   │   ├── graph.rs     # Knowledge graph operations (CRUD, traverse, impact, causal)
│   │   ├── dispatch.rs  # Dispatch loop + result consumer
│   │   ├── messaging.rs # Inter-agent messaging
│   │   ├── dashboard.rs # SystemStatus, health_indicators
│   │   ├── permission.rs # Permission delegation
│   │   ├── tools_external.rs # External tool provider integration (MCP)
│   │   ├── session.rs   # Session lifecycle (start, end, orchestrate, compound response)
│   │   ├── delta.rs     # Delta tracking (changes since seq, watch CIDs/tags)
│   │   ├── prefetch.rs  # ⚠ 1842 lines — Intent prefetcher + feedback + async assembly
│   │   ├── hybrid.rs    # Graph-RAG hybrid retrieval (vector + KG)
│   │   ├── model.rs     # LLM model management (hot-swap, list providers)
│   │   ├── cache.rs     # Multi-layer caching (intent, search, embedding)
│   │   ├── batch.rs     # Batch operations (multi-object CRUD)
│   │   ├── checkpoint.rs # Agent state checkpoint / restore
│   │   ├── tenant.rs    # Tenant isolation operations
│   │   ├── task.rs      # Task delegation between agents
│   │   ├── observability.rs # Metrics, telemetry, performance counters
│   │   ├── tier_maintenance.rs # Memory tier maintenance (TTL, promotion)
│   │   └── distributed.rs # Distributed operation stubs
│   └── INDEX.md
├── api/                 # API layer — permission guardrails + semantic JSON protocol
│   ├── semantic.rs      # ApiRequest, ApiResponse, SystemStatus, protocol types
│   ├── permission.rs    # PermissionGuard, PermissionContext, PermissionAction
│   ├── mod.rs           # Re-exports
│   └── INDEX.md
├── tool/                # Tool Abstraction — "Everything is a Tool" capability system
│   ├── mod.rs           # ToolDescriptor, ToolResult, ToolSchema, ToolHandler trait
│   ├── registry.rs      # ToolRegistry — agent-discoverable capability catalog
│   ├── procedure_provider.rs # Procedural memory → tool bridge
│   └── INDEX.md
├── temporal/            # Temporal reasoning — natural language time → time ranges
│   ├── resolver.rs      # TemporalResolver trait, OllamaTemporalResolver, StubTemporalResolver
│   ├── rules.rs         # HeuristicTemporalResolver, pre-defined temporal rules
│   ├── mod.rs           # Re-exports
│   └── INDEX.md
├── llm/                 # LLM provider abstraction — model-agnostic chat interface
│   ├── mod.rs           # LlmProvider trait, ChatMessage, ChatOptions, LlmError
│   ├── ollama.rs        # OllamaProvider — local Ollama daemon
│   ├── openai.rs        # OpenAICompatibleProvider — OpenAI-compatible endpoints
│   └── stub.rs          # StubProvider — fixed responses for testing
├── mcp/                 # MCP client — connect to external MCP servers
│   ├── mod.rs           # Re-exports (ExternalToolProvider adapter)
│   ├── client.rs        # McpClient, McpToolDef, McpError
│   └── tests.rs         # Unit tests
├── bin/
│   ├── plicod.rs        # TCP daemon (port 7878, JSON ApiRequest/ApiResponse, no HTTP)
│   ├── plico_mcp.rs     # MCP stdio server (JSON-RPC 2.0 over stdin/stdout)
│   └── aicli/           # AI-friendly semantic CLI
│       ├── main.rs      # CLI entry: local kernel or --tcp daemon mode
│       └── commands/
│           ├── mod.rs   # execute_local dispatch + shared parse utilities
│           └── handlers/
│               ├── mod.rs       # Re-exports all handler functions
│               ├── crud.rs      # put/get/search/update/delete/history/rollback
│               ├── agent.rs     # agent register/status/suspend/resume/terminate/complete/fail/checkpoint/restore/quota/discover/delegate
│               ├── memory.rs    # remember/recall/memmove/memdelete
│               ├── graph.rs     # node/edge/nodes/edges/paths/get-node/rm-node/rm-edge/update-node/edge-history/explore
│               ├── deleted.rs   # deleted/restore (recycle bin)
│               ├── intent.rs    # intent (NL resolve + --description submit)
│               ├── messaging.rs # send/messages/ack
│               ├── tool.rs      # tool list/describe/call
│               ├── events.rs    # events list/by-time
│               ├── context.rs   # context assembly
│               ├── skills.rs    # skills register/discover
│               ├── session.rs   # session-start/session-end/growth
│               ├── delta.rs     # delta change tracking
│               └── hybrid.rs    # hybrid Graph-RAG retrieval
├── lib.rs               # Crate root: pub mod declarations + PlicoError + re-exports
└── main.rs              # Stub — directs to plicod/aicli/plico-mcp

tests/                   # Integration tests (28 files)
├── kernel_test.rs       # AIKernel full integration (agents, CRUD, tools, events, dispatch, status)
├── ai_experience_test.rs # C-6: Multi-session AI agent workflow (cross-agent search, auto-reg)
├── fs_test.rs           # SemanticFS CRUD + event tests
├── cli_test.rs          # CLI binary tests
├── memory_test.rs       # Layered memory tests
├── memory_persist_test.rs # Memory persistence
├── semantic_search_test.rs # Vector + BM25 hybrid search
├── embedding_test.rs    # Embedding provider tests
├── permission_test.rs   # Permission guard tests
├── intent_test.rs       # Intent router tests
├── mcp_test.rs          # MCP server tests
├── batch_ops_test.rs    # Batch operation tests
├── kg_causal_test.rs    # Knowledge graph causal reasoning
├── observability_test.rs # Metrics and observability
├── node4_hybrid.rs      # Node 4 hybrid retrieval tests
├── node4_knowledge_event.rs # Node 4 knowledge events
├── node4_task.rs        # Node 4 task delegation
├── node4_crash_recovery.rs # Node 4 crash recovery
├── node5_self_evolving_test.rs # Node 5 self-evolving skills
├── api_version_test.rs  # API versioning tests
├── model_hot_swap_test.rs # Model hot-swap tests
├── integration_demo_test.rs # E2E demo scenario
└── ...                  # + benchmark/metrics tests (v9, v11, v22)

Cargo.toml               # Rust crate definition (3 binaries: plicod, aicli, plico-mcp)
CLAUDE.md                # AI guidance (soul document reference)

docs/                    # Tier B — iteration-end human docs (not maintained per-commit)
└── plans/               # Milestone plans (Git-visible); see docs/plans/INDEX.md
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
| Temporal | `src/temporal/INDEX.md` | Time expression → Unix ms range resolution |
| LLM providers | `src/llm/INDEX.md` | LlmProvider trait, Ollama/OpenAI/Stub backends |
| MCP client | `src/mcp/INDEX.md` | External tool integration via MCP protocol |
| Event bus | `src/kernel/event_bus.rs` | Kernel pub/sub, persisted event log |
| Kernel ops | `src/kernel/ops/INDEX.md` | 24 operation files (session, delta, prefetch, hybrid, etc.) |
| Binaries | `src/bin/INDEX.md` | plicod (TCP), plico-mcp (MCP stdio), aicli (CLI) |
| Milestone plans | `docs/plans/INDEX.md` | v0.5+ roadmap copies for Git; sync at iteration end |

## Build & Test

| Command | Purpose |
|---------|---------|
| `cargo build` | Build all targets |
| `cargo build --bin aicli` | Build CLI binary only |
| `cargo build --bin plicod` | Build daemon binary only |
| `cargo build --bin plico-mcp` | Build MCP server only |
| `cargo test --lib` | Run unit tests (co-located in source) |
| `cargo test` | Run all tests (unit + integration) |
| `cargo test [test_name]` | Run a single test |
| `cargo clippy` | Lint check (must be zero warnings) |
| `cargo build --release` | Release build (LTO + single codegen unit) |
| `cargo run --bin aicli -- --root /tmp/plico put --content "test" --tags "test"` | Quick CLI test |
| `cargo run --bin aicli -- --root /tmp/plico system-status` | Check kernel health |
| `cargo run --bin plicod -- --port 7878 --root /tmp/plico` | Run daemon |

## Conventions

- **Files**: `snake_case.rs`, one concept per file, target < 300 lines
- **Naming**: `snake_case` functions, `PascalCase` types, `SCREAMING_SNAKE` constants
- **Modules**: `pub mod` in `mod.rs`; large modules split into `dir/mod.rs` + subfiles (see `fs/`, `memory/`, `kernel/ops/`)
- **Public API**: `pub fn`, private by default
- **L2 file headers**: doc comment (`//!`) with module purpose + `# Panics`, `# Errors`, `# Safety`
- **Tests**: `#[cfg(test)] mod tests` co-located in same file; large test suites in separate `tests.rs` under the module dir

## Architectural Constraints

- Dependency direction: **api/bin → kernel → tool/fs/intent → cas/memory/scheduler/temporal/llm** (never reverse)
- `kernel/` is the only module that imports all other modules — all subsystem calls go through `AIKernel`
- `AIKernel` fields are `pub(crate)` — visible only inside the `plico` crate; integration tests in `tests/` use the public API
- Binaries (`bin/`) import only `kernel/` and `api/`, never subsystem modules directly
- CAS is the only module that touches the host filesystem directly
- No `unsafe` blocks in library code without a `# Safety` doc comment
- `plicod` is **TCP-only** — no HTTP endpoints; `SystemStatus` is queried via the semantic API (`ApiRequest::SystemStatus`)
- All known soul violations from prior iterations have been resolved:
  - Behavioral pipeline removed from `semantic_fs`
  - Dashboard hardcoded dev data removed; replaced by `SystemStatus` (runtime metrics only)
  - Project-management KGNodeType/KGEdgeType removed from graph types
  - All test scenarios converted from human-centric to AI-native

## Cross-Cutting Patterns

### Error Handling
- All errors typed: `CASError`, `MemoryError`, `SchedulerError`, `FSError`, `KGError`, `LlmError`, `McpError` (all `thiserror`)
- `Result<T>` aliases per module; crate root exposes `PlicoError`
- I/O errors converted to `std::io::Error` at API boundary (daemon/CLI)
- Never panicking in library code except for critical invariants (`expect()` with message)

### Logging
- `tracing` crate for structured logging with `tracing::info!/warn!/debug!`
- `tracing_subscriber::fmt` + `env_filter` called in `plicod.rs` and `aicli/main.rs` — reads `RUST_LOG` env var
- Library code uses `tracing` only; subscriber setup is binary responsibility

### Concurrency
- `RwLock` for in-memory maps (memory tiers, tag index, recycle bin, event log)
- `tokio` for async TCP server (`plicod`); `aicli` runs on `std` blocking
- All `Arc<...>` wrapping shared kernel state in daemon
- `EventBus` uses `tokio::sync::broadcast` for pub/sub + `Mutex` for subscriptions

### Serialization
- JSON for: CAS object persistence, TCP protocol, event log, graph persistence, MCP messages
- `serde_json` for serialization; `serde` derive on all public types
- MCP protocol: JSON-RPC 2.0 over stdio

### Clippy Policy
- `cargo clippy` runs clean (zero warnings) — all lint violations either fixed or suppressed with `#[allow(...)]` and an explanation

## Dogfooding Tag Convention

Plico manages its own project data (ADRs, progress, experiences) through its standard API.
Tags use the `plico:` namespace with colon-separated hierarchical dimensions:

| Dimension | Values | Purpose |
|-----------|--------|---------|
| `plico:type:<T>` | adr, progress, experience, test-result, bug, code-change, doc | Artifact type |
| `plico:module:<M>` | cas, fs, kernel, api, scheduler, memory, graph, temporal, cli, daemon, intent, llm, mcp, tool | Module scope |
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
| `PLICO_ROOT` | Storage root for `plicod` / `plico-mcp` (default: `/tmp/plico`) | No |
| `RUST_LOG` | Tracing log level filter (default: `info`) | No |
| `AICLI_OUTPUT` | CLI output format: `json` for machine-readable (default: human-readable) | No |

## AI Agent Instructions

This project uses a two-tier documentation model:

- **Tier A (this file + all `INDEX.md` + file doc headers)**: Maintain in real-time, atomic with every code change. A code change is NOT complete until Tier A indexes reflect it.
- **Tier B (`README.md`, `README_zh.md`, `system.md`, `docs/`)**: Do NOT read or update during active development. Updated only at iteration-end sync.

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
.cursor/         # Cursor settings
*.rlib           # Compiled Rust library files
*.bak            # Backup files
docs/design/     # Tier B design documents
docs/plans/      # Tier B milestone plans (see docs/plans/INDEX.md)
```
