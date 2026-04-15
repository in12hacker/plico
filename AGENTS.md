# Plico — AI-Native Operating System

An operating system designed entirely from AI perspective. No human CLI/GUI. All data operations via semantic APIs for upper-layer AI agents.

## Directory Map

```
src/
├── cas/              # Content-Addressed Storage — SHA-256 object identity
│   ├── object.rs     # AIObject + AIObjectMeta + ContentType
│   └── storage.rs    # CASStorage engine (sharded, atomic writes)
├── memory/           # Layered memory — Ephemeral / Working / LongTerm / Procedural
│   ├── layered.rs    # LayeredMemory + MemoryTier + MemoryEntry + MemoryContent
│   ├── persist.rs    # CASPersister, MemoryPersister trait, MemoryLoader, PersistenceIndex
│   └── mod.rs        # MemoryQuery, MemoryResult (public types)
├── scheduler/        # Agent lifecycle — registration, priority queue, intent scheduling
│   ├── agent.rs      # Agent, AgentId, AgentState, Intent, IntentPriority
│   ├── queue.rs      # SchedulerQueue (binary heap, priority + timestamp ordering)
│   ├── dispatch.rs   # AgentExecutor trait, TokioDispatchLoop, DispatchHandle, LocalExecutor
│   └── mod.rs        # AgentScheduler
├── fs/               # Semantic filesystem — tag-based CRUD, no paths
│   ├── semantic_fs.rs  # SemanticFS + Query + SearchResult + audit/recycle log
│   ├── context_loader.rs # L0/L1/L2 layered context loading (L2 reads real CAS content)
│   ├── embedding.rs    # EmbeddingProvider trait, OllamaBackend, LocalEmbeddingBackend, StubEmbeddingProvider
│   ├── search.rs       # SemanticSearch trait, InMemoryBackend, SearchFilter, SearchHit
│   ├── graph.rs        # KnowledgeGraph trait, PetgraphBackend, KGNode, KGEdge, KGEdgeType
│   └── summarizer.rs   # Summarizer trait, OllamaSummarizer, SummaryLayer (LocalONNXSummarizer removed — was never implemented)
├── kernel/           # AI Kernel — orchestrates all subsystems
│   └── mod.rs        # AIKernel: wires CAS, memory, scheduler, fs, permissions
│                     #   Fields: pub(crate) — accessible in tests/, not external crates
│                     #   start_dispatch_loop() → DispatchHandle
│                     #   graph_explore_raw()  → Vec<(String,String,String,String,f32)>
│                     #   list_deleted() / restore_deleted() — recycle bin API
├── api/              # API layer — permission guardrails + semantic JSON protocol
│   ├── semantic.rs   # ApiRequest, ApiResponse, ContentEncoding, decode_content (JSON over TCP)
│   └── permission.rs # PermissionGuard, PermissionContext, PermissionAction
└── bin/
    ├── plicod.rs     # TCP daemon (port 7878, JSON protocol)
    └── aicli.rs      # CLI tool (local kernel or TCP mode)

Cargo.toml            # Rust crate definition
CLAUDE.md             # Project-level guidance for Claude Code
```

## Quick Navigation

| Area | Entry Point | Purpose |
|------|-------------|---------|
| CAS storage | `src/cas/mod.rs` | `AIObject`, `CASStorage`, `AIObjectMeta` |
| Memory system | `src/memory/mod.rs` | `LayeredMemory`, `MemoryTier`, `MemoryEntry` |
| Memory persistence | `src/memory/persist.rs` | `CASPersister`, `MemoryPersister`, `MemoryLoader` |
| Agent scheduling | `src/scheduler/mod.rs` | `AgentScheduler`, `Intent`, `IntentPriority` |
| Agent dispatch | `src/scheduler/dispatch.rs` | `AgentExecutor`, `TokioDispatchLoop`, `DispatchHandle` |
| Semantic FS | `src/fs/mod.rs` | `SemanticFS`, `Query`, `SearchResult` |
| Vector search | `src/fs/search.rs` | `SemanticSearch`, `InMemoryBackend`, `SearchFilter` |
| Embedding backends | `src/fs/embedding.rs` | `OllamaBackend`, `LocalEmbeddingBackend`, `StubEmbeddingProvider` |
| Knowledge graph | `src/fs/graph.rs` | `KnowledgeGraph`, `PetgraphBackend`, `KGNode`, `KGEdge` |
| Summarizer | `src/fs/summarizer.rs` | `OllamaSummarizer`, `SummaryLayer` |
| AI Kernel | `src/kernel/mod.rs` | `AIKernel` — the main orchestrator |
| Permission guard | `src/api/permission.rs` | `PermissionGuard`, `PermissionAction` |
| TCP daemon | `src/bin/plicod.rs` | JSON API server on port 7878 |
| CLI tool | `src/bin/aicli.rs` | `put`, `get`, `search` (accepts `--query` flag or positional arg), `update`, `delete`, `tags`, `explore` (uses `graph_explore_raw`, respects `--edge-type`, clamps depth≤3), `agent`, `remember`, `recall`, `deleted`, `restore` |

## Build & Test

| Command | Purpose |
|---------|---------|
| `cargo build` | Build release binary |
| `cargo build --bin aicli` | Build CLI binary only |
| `cargo build --bin plicod` | Build daemon binary only |
| `cargo test --lib` | Run all unit tests |
| `cargo test scheduler::tests::test_priority_ordering` | Run single test |
| `cargo run --bin aicli -- put --content "..." --tags "..."` | Quick CLI test |
| `cargo run --bin plicod -- --port 7878 --root /var/plico` | Run daemon |

**Rust toolchain**: Requires `cargo` (installed via rustup). Source `~/.cargo/env` if not in PATH.

## Code Conventions (Rust)

- **Files**: `snake_case.rs`, one concept per file, target < 300 lines
- **Naming**: `snake_case` functions, `PascalCase` types, `SCREAMING_SNAKE` constants
- **Modules**: `pub mod` in `mod.rs`, submodules in `subdir/` with `mod.rs`
- **Public API**: `pub fn`, private by default
- **L2 file headers**: doc comment (`//!`) with module purpose + `# Panics`, `# Errors`, `# Safety`
- **Tests**: `#[cfg(test)] mod tests` co-located in same file

## Architectural Constraints

- Dependency direction: **api/permission → kernel → fs → cas/memory/scheduler** (never reverse)
- `kernel/` is the only module that imports all other modules — all subsystem calls go through `AIKernel`
- `AIKernel` fields are `pub(crate)` — integration tests in `tests/` can access them, external crates cannot
- Binaries (`bin/`) import only `kernel/` and `api/`, never subsystem modules directly
- CAS is the only module that touches the filesystem directly
- No `unsafe` blocks in library code without a `# Safety` doc comment

## Cross-Cutting Patterns

### Error Handling
- All errors typed: `CASError`, `MemoryError`, `SchedulerError`, `FSError` (all `thiserror`)
- `Result<T>` aliases per module; library root exposes `PlicoError`
- I/O errors converted to `std::io::Error` at API boundary (daemon/CLI)
- Never panicking in library code except for critical invariants (`expect()` with message)

### Logging
- `tracing` crate in Cargo.toml; structured logging with `tracing::info!/warn!/debug!`
- `tracing_subscriber::fmt::init()` called in both `plicod.rs` and `aicli.rs` main() — reads `RUST_LOG` env var
- Library code uses `tracing` only; subscriber setup is binary responsibility

### Concurrency
- `RwLock` for in-memory maps (ephemeral, working, long-term, procedural memories)
- `tokio` available for async TCP server; currently using blocking `std::net` for simplicity
- All `Arc<...>` wrapping shared kernel state in daemon
- `OllamaBackend` and `OllamaSummarizer`: safe to call from within `tokio::spawn` (use `block_in_place`)

### Clippy Policy
- `cargo clippy` runs clean (zero warnings) — all lint violations are either fixed or suppressed with `#[allow(...)]` comments explaining why

### Serialization
- JSON for: CAS object persistence, TCP protocol, `serde` on all public types
- `serde_json` for serialization; `serde` derive for `Serialize`/`Deserialize`

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
- [ ] Open module `INDEX.md` (when created) and check Dependents before modifying public API
- [ ] Confirm Modification Risk for signature/error-type changes
- [ ] Run `cargo test --lib` — all tests must pass before finishing
- [ ] If binary changed: `cargo build --bin [name]` succeeds
- [ ] Update AGENTS.md if new modules or build commands were added

## Index Exclusions

```
target/          # Cargo build output
Cargo.lock       # Lock file — do not commit
.claude/         # Claude Code settings
*.rlib
```
